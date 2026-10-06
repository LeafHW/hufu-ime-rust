//! 方案（Schema）加载：目录即方案，按文件角色自动识别装配。

use crate::annotation::{AnnotationTable, ReverseTable};
use crate::dict::Dict;
use crate::entry::DictEntry;
use crate::parse::{self, parse_file};
use crate::supplement::Supplement;
use crate::symbols::SymbolTables;
use crate::user::{UserAdjust, UserDict};
use std::path::{Path, PathBuf};

/// 一个输入方案：主码表 + 符号 + 注释 + 反查 + 用户数据 + 构词规则。
pub struct Schema {
    pub name: String,
    pub dir: PathBuf,
    pub dict: std::sync::Arc<Dict>,
    pub symbols: SymbolTables,
    pub supplement: Supplement,
    /// 拼音注释
    pub pinyin: Option<AnnotationTable>,
    /// Unicode 分区注释
    pub unicode_block: Option<AnnotationTable>,
    /// 拆分提示
    pub split: Option<AnnotationTable>,
    /// 反查表（码 → 词）——【性能】懒加载：启动只记 reverse_path
    /// （7.7MB 文本解析 ~700ms 是冷启动大头之一），首次反查或后台
    /// 预热线程调用 Engine::ensure_reverse 时才真正装载。
    pub reverse: Option<ReverseTable>,
    /// 反查表源文件（未装载时记录，装载后置 None）
    pub reverse_path: Option<PathBuf>,
    /// 【` 引导超集码表 2026-11】独立超集副表（文件名含 超集/超字集，
    /// 码全带 ` 前缀如 `aaaa）。` 加入编码字母表时 ` 起段编码查此表；
    /// 无副表（Arc 空表）时 ` 维持原行为。见 Schema::load 装载段。
    pub super_dict: std::sync::Arc<Dict>,
    /// 用户调整（置顶/添加/删除日志）
    pub adjust: UserAdjust,
    /// 用户词库
    pub user_dict: UserDict,
    /// Rime encoder 构词规则（造词用）
    pub encoder_rules: Vec<parse::EncoderRule>,
}

fn file_stem_lower(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 【超集副表内容识别 2026-11 用户拍板】文件**内容**是否为 ` 前缀码
/// 表（超集单字表）：非空、解析成功、且**全部**条目的编码都以 `
/// 开头。文件名无关（用户文件可叫任何名字）；混合表（部分 ` 部分
/// 普通）保守按普通主码表处理。琉璃超集格式 `字\t\`code`（词前
/// TSV）经 parse_auto 解析后 code="`code"，天然命中。
fn is_backtick_table(path: &Path) -> bool {
    let Ok(lines) = parse::read_lines(path) else {
        return false;
    };
    let t = parse::parse_auto(&lines);
    if t.rows.is_empty() {
        return false;
    }
    t.rows.iter().all(|e| e.code.starts_with('`'))
}

impl Schema {
    /// 加载方案目录。
    pub fn load(dir: &Path) -> std::io::Result<Schema> {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "schema".into());
        let mut schema = Schema {
            name: name.clone(),
            dir: dir.to_path_buf(),
            dict: std::sync::Arc::new(Dict::new(&name)),
            symbols: SymbolTables::default(),
            supplement: Supplement::default(),
            pinyin: None,
            unicode_block: None,
            split: None,
            reverse: None,
            reverse_path: None,
            super_dict: std::sync::Arc::new(Dict::new("super")),
            adjust: UserAdjust::default(),
            user_dict: UserDict::default(),
            encoder_rules: Vec::new(),
        };

        let mut rime_dicts: Vec<PathBuf> = Vec::new();
        let mut big_tables: Vec<PathBuf> = Vec::new();
        // 【` 引导超集码表 2026-11】独立副表：**内容**识别（非空且
        // 全部行编码带 ` 前缀——文件名任意），见装载段与 is_backtick_table。
        let mut super_tables: Vec<PathBuf> = Vec::new();
        // 用户码表/用户词类文件（多多/QQ五笔语义=个人用户词）。命名只作
        // 初筛，是否晋升主码表见下方 BIG_USER_TABLE_AS_MAIN。
        let mut user_tables: Vec<PathBuf> = Vec::new();
        // 【文件整合 2026-09-06】调整行不再即读即 set：统一收集到加载
        // 收尾回放（码表内嵌 ++ 旧用户调整.txt ++ 旧用户词.txt，
        // 后者最新在后，覆盖语义正确）。
        // 【格式统一 2026-09-06】新主文件=用户调整.txt（{置顶}/{添加}/
        // {删除}/{加权} 统一标记格式）；旧 用户词.txt 只读兼容。
        let mut adj_file_lines: Vec<String> = Vec::new();
        let mut word_adj_lines: Vec<String> = Vec::new();
        let mut embedded_adj: Vec<String> = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                // 子目录（如 拼音反查码表/）暂不递归主码表
                continue;
            }
            let stem = file_stem_lower(&path);
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            match stem.as_str() {
                "快符" => {
                    schema.symbols.quick =
                        SymbolTables::parse_quick(&parse::read_lines(&path)?);
                }
                "常用符号" => {
                    schema.symbols.slash =
                        SymbolTables::parse_slash(&parse::read_lines(&path)?);
                }
                "一简符号" => {
                    schema.symbols.simple =
                        SymbolTables::parse_simple(&parse::read_lines(&path)?);
                }
                "补充语料" => schema.supplement = Supplement::load(&path)?,
                // 新主文件：{置顶}/{添加}/{删除}/{加权} 统一标记格式
                //（split 分拣：{添加}→词行，其余→调整行）。entries 用
                // extend：read_dir 遍历序不定，两用户文件谁先都能合入
                "用户调整" => {
                    if let Ok(l) = parse::read_lines(&path) {
                        let (dict_lines, adj_lines) = UserAdjust::split_adjust_lines(&l);
                        let d = UserDict::parse(&dict_lines);
                        schema.user_dict.entries.extend(d.entries);
                        adj_file_lines = adj_lines;
                    }
                }
                // 旧文件：只读兼容（历史数据，引擎不再写入）；词行并入
                // 用户词库，调整行先回放（新主文件覆盖它）
                "用户词" => {
                    if let Ok(l) = parse::read_lines(&path) {
                        let (dict_lines, adj_lines) =
                            UserAdjust::split_adjust_lines(&l);
                        let old_dict = UserDict::parse(&dict_lines);
                        schema.user_dict.entries.extend(old_dict.entries);
                        word_adj_lines = adj_lines;
                    }
                }
                _ => {}
            }
            if stem.contains("拼音") && ext == "注释" {
                schema.pinyin = Some(AnnotationTable::load(&path)?);
            } else if stem.contains("unicode") && ext == "注释" {
                schema.unicode_block = Some(AnnotationTable::load(&path)?);
            } else if ext == "拆分" {
                schema.split = Some(AnnotationTable::load(&path)?);
            } else if ext == "yaml" && stem.ends_with(".dict") {
                rime_dicts.push(path.clone());
            } else if ext == "txt" {
                // 【用户码表/用户词 2026-10-02】精确 stem "用户词"/"用户调整"
                // 已由上方 match 分支处理（用户调整.txt 本就进 big_tables
                // 参加最大者胜选——历史行为保留）；此处只收"文件名含
                // 用户码表/用户词"的导出类文件：不得冒然进主码表候选
                // （/jc 落盘用户词文件若大于真码表，max_by_key 会把它选
                // 成主表——整个输入法只剩几个用户词；测试
                // user_word_placement 抓获）。先收集，体量分级见
                // BIG_USER_TABLE_AS_MAIN。
                if stem == "用户词" {
                    // match 分支已并入，不重复处理
                } else if stem.contains("用户码表") || stem.contains("用户词") {
                    user_tables.push(path.clone());
                } else if stem.contains("反查") {
                    // 【性能】懒加载：只记路径（见 reverse 字段注释）
                    schema.reverse_path = Some(path.clone());
                } else {
                    // 其余 txt：内容分拣——` 前缀码全量副表（超集单字，
                    // 文件名任意）vs 主码表候选（多多/QQ五笔/虎整句）。
                    // 【2026-11 用户拍板】超集表按**内容**识别：非空且
                    // **全部**行的编码都以 ` 开头（如 `aaaa）。不再看
                    // 文件名（超集/超字集）——用户文件叫什么名无关。
                    // 1.2MB 副表若混进主表竞选会抢主码表位，这里就地
                    // 分流到 super_tables（独立装载，` 域直查）。
                    if is_backtick_table(&path) {
                        super_tables.push(path.clone());
                    } else {
                        big_tables.push(path.clone());
                    }
                }
            }
        }

        // 【大体量用户码表晋升主码表 2026-10-02】五用户实测病灶：把多多/
        // QQ五笔「导出 - 主码 - 用户码表.txt」（10.7 万行≈全量词库，QQ五笔
        // 用户码表会累积全部词汇）拖进方案目录——文件名含「用户码表」被
        // 当个人用户词并入 UserDict：主词典 0 条（打不出字），且十万级
        // 用户词把每键热路径拖慢。体量分级：≥512KB 的"用户码表"事实是
        // 全量码表（QQ五笔86 主表也才 1.2MB），晋升 big_tables 走 Trie/
        // HashMap 索引与正常排序；小文件维持用户词语义不变（/jc 落盘的
        // 用户词.txt 永远是 KB 级）。注：晋升后仍参加 max_by_key 按最大
        // 选主表，与目录里真主表并存时大的赢——语义正确（导出即用户
        // 的完整词库快照）。
        const BIG_USER_TABLE_AS_MAIN: u64 = 512 * 1024;
        let mut duoduo_user: Option<PathBuf> = None;
        for p in user_tables {
            let mut big = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) >= BIG_USER_TABLE_AS_MAIN;
            if big && !rime_dicts.is_empty() {
                // Rime dict.yaml 在场=方案主体明确，用户导出不抢主表位
                //（虎码等 Rime 方案目录混入多多导出的场景），仍并入用户词。
                big = false;
            }
            if big {
                big_tables.push(p);
            } else {
                duoduo_user = Some(p);
            }
        }

        // 主码表选择（Rime dict.yaml 优先）：
        //   1) 预解析全部 dict.yaml，取「未被其他表导入」的表为候选
        //   2) 候选中优先聚合表（自身声明了 import_tables），其次与目录同名，再次最大
        //   3) 选定后按 import_tables 闭包递归合并
        let mut rime_loaded: Vec<(PathBuf, parse::RawTable)> = Vec::new();
        for p in &rime_dicts {
            if let Ok(t) = parse_file(p) {
                rime_loaded.push((p.clone(), t));
            }
        }
        let imported_names: std::collections::HashSet<String> = rime_loaded
            .iter()
            .flat_map(|(_, t)| t.meta.imports.iter().cloned())
            .collect();
        let pick_rime = rime_loaded
            .iter()
            .filter(|(p, t)| {
                rime_loaded.len() == 1
                    || !imported_names.contains(&t.meta.name)
                    || file_stem_lower(p) == t.meta.name // 名字对不上时保守保留
            })
            .max_by_key(|(p, t)| {
                let agg = (!t.meta.imports.is_empty()) as i32 * 8;
                let stem = file_stem_lower(p);
                let nm = (t.meta.name == name || stem == name.to_lowercase()) as i32 * 4;
                let known = (stem == "tiger.dict" || stem == "tigress.dict") as i32 * 2;
                (agg + nm + known, std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            });
        if let Some((main_path, main_table)) = pick_rime {
            schema.encoder_rules = main_table.meta.encoder_rules.clone();
            let main_name = if main_table.meta.name.is_empty() {
                file_stem_lower(main_path)
                    .trim_end_matches(".dict")
                    .to_string()
            } else {
                main_table.meta.name.clone()
            };
            let mut dict = Dict::from_entries(main_name.clone(), main_table.rows.clone());
            // import_tables 闭包（BFS，防环）
            let mut visited: std::collections::HashSet<String> =
                std::collections::HashSet::from([main_name]);
            let mut queue: Vec<String> = main_table.meta.imports.clone();
            while let Some(imp) = queue.pop() {
                if !visited.insert(imp.clone()) {
                    continue;
                }
                let imp_path = main_path.with_file_name(format!("{imp}.dict.yaml"));
                if let Ok(sub) = parse_file(&imp_path) {
                    for next in sub.meta.imports.clone() {
                        if !visited.contains(&next) {
                            queue.push(next);
                        }
                    }
                    dict.merge(&Dict::from_entries(imp.clone(), sub.rows));
                }
            }
            schema.dict = std::sync::Arc::new(dict);
        } else if let Some(main) = big_tables.iter().max_by_key(|p| {
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
        }) {
            // 【虎爪码表内嵌调整 2026-09-06】虎爪导出的码表把学习记录直接
            // 嵌在码表里：`{置顶}码 词 [日期]`、`{添加}…`、`{删除}…`（第三
            // 列常为日期，UserAdjust::parse 宽容忽略）。此前这些行被当普通
            // 词条（码=「{添加}xx」非法编码，静默成死数据）。现在解析前抽
            // 走：词典不含死行；抽出的行进统一回放（见加载收尾）。
            let lines = parse::read_lines(main)?;
            let is_adjust = |l: &String| {
                let t = l.trim_start();
                t.starts_with("{置顶}") || t.starts_with("{添加}") || t.starts_with("{删除}")
            };
            embedded_adj = lines.iter().filter(|l| is_adjust(l)).cloned().collect();
            let dict_lines: Vec<String> =
                lines.into_iter().filter(|l| !is_adjust(&l)).collect();
            let table = parse::parse_auto(&dict_lines);
            schema.dict = std::sync::Arc::new(Dict::from_entries(name.clone(), table.rows));
        }

        // 【` 引导超集码表 2026-11】超集副表装载：内容识别（全部行编码
        // ` 前缀——文件名任意）。词前 TSV（`字\t\`code`）解析后并入独立
        // super_dict；多个副表按文件名序合并。此表码全带 ` 前缀——与
        // 主码表（无 ` 编码）天然分域，引擎 ` 编码态直查此表。
        if !super_tables.is_empty() {
            super_tables.sort();
            let mut rows: Vec<DictEntry> = Vec::new();
            for p in &super_tables {
                let lines = parse::read_lines(p)?;
                let t = parse::parse_auto(&lines);
                rows.extend(t.rows);
            }
            schema.super_dict = std::sync::Arc::new(Dict::from_entries(
                format!("{name}-超集"),
                rows,
            ));
        }

        // 【文件整合 2026-09-06】调整统一回放收尾：码表内嵌（作者）→
        // 旧用户词.txt 调整行（历史）→ 用户调整.txt（新主文件，用户
        // 最新操作在后覆盖前面的语义）。
        // 【权重回放 2026-09-06】{加权} 行回放出的 weights 填入
        // user_dict.weights（merge_into 用户词分支消费）。
        {
            let mut replay = embedded_adj;
            replay.extend(word_adj_lines);
            replay.extend(adj_file_lines);
            if !replay.is_empty() {
                schema.adjust = UserAdjust::parse(&replay);
                for ((c, w), v) in schema.adjust.weights.iter() {
                    schema
                        .user_dict
                        .weights
                        .insert((c.clone(), w.clone()), *v);
                }
            }
        }

        // 多多用户码表并入用户词库（【O(n) 2026-10-02】原逐行线性查重
        // 是 10.7 万行≈118s 的二次方病灶，改 absorb 整体线性并入）
        if let Some(u) = duoduo_user {
            let t = parse_file(&u)?;
            schema.user_dict.absorb(t.rows);
        }

        // 符号行并入符号命名空间（虎整句格式的 `/xx`、`;x` 行）
        let mut quick = schema.symbols.quick.clone();
        let mut slash = schema.symbols.slash.clone();
        for e in &schema.dict.entries {
            if e.code.starts_with(';') && e.code.chars().count() == 2 {
                quick
                    .entry(e.code.clone())
                    .or_default()
                    .push(crate::symbols::SymbolEntry {
                        code: e.code.clone(),
                        text: e.text.clone(),
                        weight: 1000.0,
                    });
            } else if e.code.starts_with('/') && e.code.chars().count() >= 2 {
                slash
                    .entry(e.code.clone())
                    .or_default()
                    .push(crate::symbols::SymbolEntry {
                        code: e.code.clone(),
                        text: e.text.clone(),
                        weight: e.weight,
                    });
            }
        }
        schema.symbols.quick = quick;
        schema.symbols.slash = slash;

        // 【码索引 2026-10-02】加载过程中 用户调整.txt/旧用户词.txt 词行
        // 直接 extend 进 entries（绕过 UserDict 增改 API），此处统一
        // 重建 by_code 索引，保证 merge_into 每键热路径命中。
        schema.user_dict = std::mem::take(&mut schema.user_dict).reindexed();

        Ok(schema)
    }

    /// 某编码的最终候选：用户词 + 调整回放 + 系统候选。
    pub fn candidates(&self, code: &str) -> Vec<DictEntry> {
        let base: Vec<DictEntry> = self.dict.lookup(code).into_iter().cloned().collect();
        // 【` 引导超集码表 2026-11】` 前缀编码查超集副表（独立域，
        // 与主码表无码冲突；无副表=空 Dict 查不到，零开销短路）。
        // 超集表内的序 = 文件行序（rank_cmp 权重相同时按 seq），用户
        // 调整/用户词不作用于此域（` 域无 /jc 加词——raw 含 ` 时
        // 引擎 on_space 直接查表上屏）。
        if code.starts_with('`') && !self.super_dict.is_empty() {
            return self.super_dict.lookup(code).into_iter().cloned().collect();
        }
        // 【时序回放 2026-11 用户拍板】{置顶}/{添加}pN/{删除} 逐条按
        // 日志时序在演化列表上执行（adjust.apply）——删除会让先前
        // 插入的 pN 用户词连同后续词前移（删 2 选后 p5 变 4 选）。
        // pN 用户词不再走旧的「合并后绝对位次插入」路径。
        let mut out = self.adjust.apply(code, &base);
        // 无选重位的用户词（会话调频 learn / 无 pN 的 /jc 加词 /
        // 多多小用户表）仍并入：v1 语义——插到置顶块之后、系统词之前。
        let mut user_entries: Vec<DictEntry> = Vec::new();
        self.user_dict.merge_into(code, &self.dict, &mut user_entries);
        for ue in user_entries {
            // 【删词对用户词生效 2026-09-06】删除态的用户词隐藏
            if self.adjust.removed(&ue.code, &ue.text) {
                continue;
            }
            // 【时序回放去重】{添加}(pN) 词已由 adjust.apply 就位——
            // user_dict 里同词的行不再重复并入（按 text 查重，码表
            // 域同码下 text 唯一是既定不变量）。无 pN 的 {添加} 词行
            // 同样在 adjust.apply 落位（置顶块后=留空首选，2026-10-06
            // 回归修复），这里也不再并入——会话调频 learn 才走到下方
            // 插入（v1：置顶块后、系统词前）。
            if self.adjust.added(&ue.code, &ue.text) {
                continue;
            }
            // 【调频置顶 v1 语义】learn 的用户词顶替码表同词位、排到
            // 置顶块之后系统词之前（学习=提频，位置前移）；码表原位
            // 的同词移除（旧实现按 text 查重跳过会把学过的词钉死在
            // 码表原位——keymap 学习后就/到的/加 的 2 选错成 到的）。
            if let Some(hit) = out.iter().position(|e| e.text == ue.text && !e.pinned) {
                out.remove(hit);
            }
            let pos = out.iter().position(|e| !e.pinned).unwrap_or(out.len());
            out.insert(pos, ue);
        }
        out
    }

    /// 词的最优码（反查注释 / 造词）。
    pub fn best_code_of(&self, text: &str) -> Option<String> {
        self.dict
            .best_code_of(text)
            .map(|s| s.to_string())
            .or_else(|| {
                self.user_dict
                    .entries
                    .iter()
                    .find(|e| e.text == text)
                    .map(|e| e.code.clone())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn load_tiger_like_schema() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-schema-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "tiger.dict.yaml",
            "---\nname: tiger\nsort: by_weight\n...\n的\tu\t10359470\nt\t我\t9000000\ntu\t们\t500\n",
        );
        write(&tmp, "快符.txt", "！\t;a\n。\t;b\n");
        write(&tmp, "常用符号.txt", "™\t/tm\n℃\t/ssd\n");
        write(&tmp, "补充语料.txt", "赢麻了\t8000\n");
        write(&tmp, "用户调整.txt", "{置顶}u\t底\n{删除}u\t的\n");
        write(&tmp, "虎码.拆分", "我\t丿扌戈\n们\t亻门\n");
        write(
            &tmp,
            "tiger.user.dict.yaml",
            "---\nname: tiger.user\n...\n:\"\t;q\n",
        );

        let s = Schema::load(&tmp).unwrap();
        // 主表 3 行（无 import_tables 时不合并 tiger.user）
        assert_eq!(s.dict.len(), 3);
        let cands = s.candidates("u");
        let texts: Vec<String> = cands.iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["底".to_string()]); // 「的」被删除，置顶「底」生效
        assert!(s.symbols.quick.get(";a").is_some());
        assert!(s.symbols.slash.get("/tm").is_some());
        assert_eq!(s.supplement.entries[0].word, "赢麻了");
        assert_eq!(s.split.as_ref().unwrap().get('我'), Some("丿扌戈"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_duoduo_schema() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-duoduo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "多多B常用字词.txt", "---config@码表分类=主码-系统码表\n的\tu\n他\tje\n");
        write(&tmp, "用户调整.txt", "{置顶}je\t她\n");

        let s = Schema::load(&tmp).unwrap();
        assert_eq!(s.dict.len(), 2);
        let texts: Vec<String> = s.candidates("je").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["她".to_string(), "他".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_sentence_schema() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-sent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "转换后的码表.txt", "t 我 我们\na 来 那个\naaaa 魑魅魍魉 卍\n/jc {加词}\n");
        let s = Schema::load(&tmp).unwrap();
        assert!(s.dict.len() >= 7);
        assert_eq!(s.dict.lookup("t")[0].text, "我");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pin_ordering_no_dup() {
        // 同码多次置顶：最新在前、无重复；且置顶词应带 pinned 标记
        // （混排 自造词 + 码表内词 时顺序仍一致）
        let tmp = std::env::temp_dir().join(format!("hufu-test-pinord-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "main.txt", "#hufu-dict v1 name=t\na\t来\na\t那个\n");
        let mut s = Schema::load(&tmp).unwrap();
        // pin 码表内词 + 自造词混合
        s.adjust.pin("a", "那个"); // 码表内
        s.adjust.pin("a", "abc"); // 自造
        let cands = s.candidates("a");
        let texts: Vec<String> = cands.iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["abc", "那个", "来"], "最新 pin(abc) 在最前: {texts:?}");
        assert!(cands.iter().all(|e| (e.text == "来") ^ e.pinned), "两个 pin 词都应带 pinned");
        // 再 pin 已 pin 的 → 移到最前，无重复
        s.adjust.pin("a", "那个");
        let texts: Vec<String> = s.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["那个", "abc", "来"], "重 pin: {texts:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【虎爪码表内嵌调整】主码表里的 {置顶}/{添加}/{删除}（带日期列）
    // 解析前抽走：不进词典（无死行）、候选生效；用户文件覆盖内嵌。
    #[test]
    fn embedded_adjust_in_main_dict() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // 虎整句空格格式主表 + 内嵌三行（日期形态各异）
        write(
            &tmp,
            "码表.txt",
            "a 来 那个 氨\n{置顶}a 氨 2026-09-04\n{添加}a 哎呦 20260904\n{删除}a 那个 2026/09/05\n",
        );
        let s = Schema::load(&tmp).unwrap();
        // 词典不含内嵌死行（1 行 ×3 真词；{} 码查不到任何东西）
        assert_eq!(s.dict.len(), 3, "内嵌调整行不得进词典");
        assert!(s.dict.lookup("{置顶}a").is_empty(), "不得残留花括号死码");
        // 内嵌回放：氨置顶、那个删除、哎呦添加（留空=首选：置顶块后）
        let texts: Vec<String> = s.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["氨".to_string(), "哎呦".to_string(), "来".to_string()]);

        // 用户文件覆盖内嵌：用户删掉内嵌置顶的「氨」
        write(&tmp, "用户调整.txt", "{删除}a\t氨\n");
        let s2 = Schema::load(&tmp).unwrap();
        let texts2: Vec<String> = s2.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts2, ["哎呦".to_string(), "来".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【/jc 选重位 2026-09-06 → 时序回放 2026-11】用户词带 stem="pN"
    // （加词窗第三框「第 N 选」）经 {添加}…pN 行回放：插入位次以操作
    // 时刻列表度量（超出 → 排最后）。
    #[test]
    fn user_word_placement() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(
            &tmp,
            "码表.txt",
            "a 甲 乙 丙 丁 戊 己\n",
        );
        // 用户词：pN 词行升格 {添加}pN（时序回放插位）；无 pN 的行
        // 仍走 user_dict 并入路径（v1：最前）
        write(
            &tmp,
            "用户词.txt",
            "#hufu-dict v1 name=user_words\na\t酉\t1\tp3\na\t戌\t1\tp9\na\t子\n",
        );
        let s = Schema::load(&tmp).unwrap();
        let texts: Vec<String> = s.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(
            texts,
            [
                "子".to_string(), // 无 pN：插到最前（v1 行为）
                "甲".to_string(),
                "乙".to_string(),
                "酉".to_string(), // p3 = 第 3 位（该时刻列表度量）
                "丙".to_string(),
                "丁".to_string(),
                "戊".to_string(),
                "己".to_string(),
                "戌".to_string(), // p9 超出 → 最后
            ],
            "选重位插入: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【留空=首选 2026-10-06 回归修复】/jc 第三框留空（{添加} 无 pN）
    // 恢复 v1 语义：无置顶词 → 首选；有置顶词 → 紧随置顶块；码表同词
    // → 顶替到该位。时序回放重构曾误作「末尾追加」＝用户实测
    // 「选重位不输入默认最后」回归（与加词窗文案「留空=首选」相悖）。
    #[test]
    fn user_word_add_no_pn_first_choice() {
        // 场景 1：无置顶词 → 新词插到最前（首选）
        let tmp = std::env::temp_dir().join(format!("hufu-test-nopn1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "码表.txt", "a 甲 乙 丙\n");
        write(&tmp, "用户调整.txt", "{添加}a\t丁\n");
        let s = Schema::load(&tmp).unwrap();
        let texts: Vec<String> = s.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(
            texts,
            ["丁".to_string(), "甲".to_string(), "乙".to_string(), "丙".to_string()],
            "留空加词=首选: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);

        // 场景 2：置顶词在场 → 插到置顶块之后（置顶语义更强）。
        // 注：用户调整.txt 也参加主码表「最大者胜选」（历史行为），
        // 码表.txt 必须明显大于 用户调整.txt 才能当主表。
        let tmp2 = std::env::temp_dir().join(format!("hufu-test-nopn2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp2);
        std::fs::create_dir_all(&tmp2).unwrap();
        write(&tmp2, "码表.txt", "a 甲 乙 丙\nb 不 不必 不然\nx 下 下午\nh 好 很好 好吧\njd 就是 就算\n");
        write(&tmp2, "用户调整.txt", "{置顶}a\t乙\n{添加}a\t丁\n");
        let s2 = Schema::load(&tmp2).unwrap();
        let texts2: Vec<String> = s2.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(
            texts2,
            ["乙".to_string(), "丁".to_string(), "甲".to_string(), "丙".to_string()],
            "留空加词让位置顶块: {texts2:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp2);

        // 场景 3：码表已有同词 → 顶替到首选位（提频，非「不动」）
        let tmp3 = std::env::temp_dir().join(format!("hufu-test-nopn3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp3);
        std::fs::create_dir_all(&tmp3).unwrap();
        write(&tmp3, "码表.txt", "a 甲 乙 丙\n");
        write(&tmp3, "用户调整.txt", "{添加}a\t乙\n");
        let s3 = Schema::load(&tmp3).unwrap();
        let texts3: Vec<String> = s3.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(
            texts3,
            ["乙".to_string(), "甲".to_string(), "丙".to_string()],
            "留空加词顶替码表同词到首选: {texts3:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp3);
    }

    // 【选重位顶替码表同词 2026-09-10】码表行里已有同 text 词（空格
    // 多词格式的行尾词常见）时，/jc 显式选重位的用户词顶替码表位插
    // 到第 N 位——旧逻辑按 text 查重直接跳过插入，用户词被码表原位
    // 屏蔽（实测：/jc ae 二 候选2，加完「二」仍在最后）。
    #[test]
    fn user_word_placement_replaces_dict_dupe() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "码表.txt", "ae 闲 那样 二\n");
        write(&tmp, "用户调整.txt", "{添加}ae\t二\tp2\n");
        let s = Schema::load(&tmp).unwrap();
        let texts: Vec<String> = s.candidates("ae").iter().map(|e| e.text.clone()).collect();
        assert_eq!(
            texts,
            ["闲".to_string(), "二".to_string(), "那样".to_string()],
            "选重位顶替码表同词: {texts:?}"
        );
        // 删除态：{删除} 后词彻底离场（码表原位也不在）
        let mut s2 = Schema::load(&tmp).unwrap();
        s2.adjust.remove("ae", "二");
        let texts2: Vec<String> = s2.candidates("ae").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts2, ["闲".to_string(), "那样".to_string()], "删除态: {texts2:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【大体量用户码表晋升主码表 2026-10-02】五用户实测：把多多/QQ五笔
    // 「导出 - 主码 - 用户码表.txt」（全量词库）拖进方案目录。文件名含
    // 「用户码表」→ 旧逻辑并入用户词：主词典 0 条打不出字 + 十万级
    // 用户词二次方并入 118s。≥512KB 晋升主码表（Trie 索引正常排序），
    // 小文件维持用户词语义。
    #[test]
    fn big_duoduo_user_export_becomes_main_table() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-biguser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // 拼一个 ≥512KB 的 word-first 导出（多多/QQ五笔导出同构）
        let mut body = String::from("#QQ五笔导出\n");
        let fill = "工\ta\n匠\tar\n牙尖嘴利\taikt\n问题\tu\n";
        while body.len() < 600 * 1024 {
            body.push_str(fill);
        }
        write(&tmp, "导出 - 主码 - 用户码表.txt", &body);
        let s = Schema::load(&tmp).unwrap();
        // 【T4·重复行去重 2026-10-06】本夹具靠重复 4 行凑 600KB——
        // 去重后唯一词条就是 4 条（真实用户导出是十几万条不同词）。
        // 晋升信号看三件事：主词典非空（晋升成功）、用户词零条（不再
        // 二次并入）、主表可正常出字。
        assert!(
            !s.dict.is_empty() && s.dict.len() <= 4,
            "大体量用户码表应晋升主码表（去重后唯一词条 4 条），实际 {} 条",
            s.dict.len()
        );
        assert_eq!(s.user_dict.entries.len(), 0, "晋升后不再重复进用户词");
        // 主表可正常出字
        assert_eq!(s.candidates("a")[0].text, "工");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 小体量「用户码表」（个人用户词语义）不晋升：仍并入用户词库。
    #[test]
    fn small_duoduo_user_table_stays_user_dict() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-smalluser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write(&tmp, "导出 - 主码 - 用户码表.txt", "自造词\tzzt\n工\ta\n");
        write(&tmp, "QQ五笔86.txt", "工\ta\n问题\tu\n");
        let s = Schema::load(&tmp).unwrap();
        assert_eq!(s.dict.len(), 2, "小用户码表不抢主表位");
        assert_eq!(s.user_dict.entries.len(), 2, "个人用户词（含表内同词）并入用户词库");
        let texts: Vec<String> = s.candidates("zzt").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["自造词".to_string()], "用户词可出字: {texts:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【用户实测场景 2026-11 修复·时序回放】ae 的操作序列（用户调整.txt
    // 回放）：{添加}ae 测试 p5 → {置顶}ae 乛 → {删除}ae 那样 →
    // {添加}ae ipad mini p6。码表：ae 行 = 闲 那样 乛 の …。
    // 用户拍板语义（2026-11）：**按操作时序回放**——删除「那样」后，
    // 它后面的所有词（含先前插入的 p5「测试」）前移一位 → 测试变
    // 4 选；ipad mini p6 在删除后的列表第 6 位；「ipad mini」含空格
    // 完整保留。
    #[test]
    fn user_adjust_rank_and_space_word_regression() {
        let tmp = std::env::temp_dir().join(format!("hufu-test-rankfix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // 码表：ae 前几名 闲 那样 乛 の 爱 安 按 岸 暗（qq/ww 补行撑
        // 体量——big_tables 最大者胜选主表，码表.txt 必须大于
        // 用户调整.txt 才能当主表）
        write(
            &tmp,
            "码表.txt",
            "ae 闲 那样 乛 の 爱 安 按 岸 暗\nqq 悉 蟋 惜 熄 硒 矽 硅 锡 膝 夕\nww 巫 诬 屋 污 乌 钨 呜 坞 吴 悟\n",
        );
        write(
            &tmp,
            "用户调整.txt",
            "{添加}ae\t测试\tp5\n{置顶}ae\t乛\n{删除}ae\t那样\n{添加}ae\tipad mini\tp6\n",
        );
        let s = Schema::load(&tmp).unwrap();
        let texts: Vec<String> = s.candidates("ae").iter().map(|e| e.text.clone()).collect();
        // 期望序（时序回放，2026-11 用户拍板）：
        // 0 乛(置顶) 1 闲 2 の 3 测试(删2选后前移) 4 爱 5 ipad mini(p6) 6 安 7 按 8 岸 9 暗
        assert_eq!(
            texts,
            [
                "乛".to_string(),
                "闲".to_string(),
                "の".to_string(),
                "测试".to_string(),
                "爱".to_string(),
                "ipad mini".to_string(),
                "安".to_string(),
                "按".to_string(),
                "岸".to_string(),
                "暗".to_string(),
            ],
            "时序回放：删 2 选后 p5 前移 4 选: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
