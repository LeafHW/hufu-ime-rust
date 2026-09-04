//! 方案（Schema）加载：目录即方案，按文件角色自动识别装配。

use crate::annotation::{AnnotationTable, ReverseTable};
use crate::dict::Dict;
use crate::entry::DictEntry;
use crate::parse::{self, parse_file, TableFormat};
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
            adjust: UserAdjust::default(),
            user_dict: UserDict::default(),
            encoder_rules: Vec::new(),
        };

        let mut rime_dicts: Vec<PathBuf> = Vec::new();
        let mut big_tables: Vec<PathBuf> = Vec::new();
        let mut duoduo_user: Option<PathBuf> = None;
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
                if stem.contains("用户码表") {
                    duoduo_user = Some(path.clone());
                } else if stem.contains("反查") {
                    // 【性能】懒加载：只记路径（见 reverse 字段注释）
                    schema.reverse_path = Some(path.clone());
                } else {
                    // 其余 txt：可能是主码表（多多/QQ五笔/虎整句/多多用户词）
                    if stem.contains("用户词") || stem.contains("用户调整") {
                        // 【2026-09-06】用户数据文件不进主码表候选也不进
                        // 多多并入（上方 match 分支已消化：entries 并入
                        // user_dict、调整行进回放）——落进 big_tables 的话
                        // 文件大于真码表时 max_by_key 会把它误选成主表，
                        // 整个输入法只剩几个用户词（测试 user_word_placement
                        // 抓获旧名；格式统一后新名「用户调整」不含「用户词」
                        // 字样漏拦，decoder_phrase_not_lift_existing_user_word
                        // 抓获——小目录里它比码表大即被误选，dict 变空）。
                    } else if stem.contains("用户码表") {
                        duoduo_user = Some(path.clone());
                    } else {
                        big_tables.push(path.clone());
                    }
                }
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

        // 多多用户码表并入用户词库
        if let Some(u) = duoduo_user {
            let t = parse_file(&u)?;
            for e in t.rows {
                if !schema.user_dict.entries.iter().any(|x| x.code == e.code && x.text == e.text) {
                    let mut e = e;
                    e.weight = 1.0;
                    e.pinned = e.pinned;
                    schema.user_dict.entries.push(e);
                }
            }
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

        Ok(schema)
    }

    /// 某编码的最终候选：用户词 + 调整回放 + 系统候选。
    pub fn candidates(&self, code: &str) -> Vec<DictEntry> {
        let base: Vec<DictEntry> = self.dict.lookup(code).into_iter().cloned().collect();
        let out = self.adjust.apply(code, &base);
        // 用户词插到置顶之后、系统词之前；带选重位标记（stem="pN"，
        // /jc 加词第三框指定「第 N 选」）的用户词按绝对位次插入最终
        // 列表：N 超出现有候选数 → 排最后；否则插到第 N 位，原第 N
        // 位及以后统一后移一位（2026-09-06 用户需求）。
        let mut user_entries: Vec<DictEntry> = Vec::new();
        self.user_dict.merge_into(code, &self.dict, &mut user_entries);
        // 用户词行版本的 text 集合：apply 的 {添加} 追加副本（adjust.adds，
        // 无选重位）与 user_dict 词行（带 pN/权重）双写重复——词行版本
        // 由下方 pinned_users/placed 段负责（含位次语义），out 里的追加
        // 副本跳过，防它先占位挡住按位插入（nl 什么东西 p2 案例：
        // 副本占尾位 → placed 去重 continue → p2 失效）。
        let user_texts: std::collections::HashSet<String> =
            user_entries.iter().map(|e| e.text.clone()).collect();
        let mut pinned_users: Vec<DictEntry> = Vec::new();
        let mut placed: Vec<(usize, DictEntry)> = Vec::new();
        for ue in user_entries {
            let pos = ue
                .stem
                .as_deref()
                .and_then(|s| s.strip_prefix('p'))
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| *n >= 1);
            match pos {
                Some(n) => placed.push((n, ue)),
                None => pinned_users.push(ue),
            }
        }
        let mut merged: Vec<DictEntry> = Vec::new();
        let pinned: Vec<DictEntry> = out.iter().filter(|e| e.pinned).cloned().collect();
        merged.extend(pinned);
        for ue in pinned_users {
            // 【删词对用户词生效 2026-09-06】adjust.apply 只过滤码表
            // base；用户词（/jc 加的）在删除态也须隐藏（Ctrl+Shift+数字）
            if self.adjust.removed(&ue.code, &ue.text) {
                continue;
            }
            if !merged.iter().any(|e| e.text == ue.text) {
                merged.push(ue);
            }
        }
        for e in out {
            if e.pinned {
                continue;
            }
            if user_texts.contains(e.text.as_str()) {
                continue;
            }
            if !merged.iter().any(|x| x.text == e.text) {
                merged.push(e);
            }
        }
        // 选重位用户词：按位次从大到小插入（先大后小，位次小的
        // 后插不受先插者下标位移影响）
        placed.sort_by(|a, b| b.0.cmp(&a.0));
        for (n, ue) in placed {
            if merged.iter().any(|x| x.text == ue.text) {
                continue;
            }
            if self.adjust.removed(&ue.code, &ue.text) {
                continue;
            }
            let idx = (n - 1).min(merged.len());
            merged.insert(idx, ue);
        }
        merged
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
        // 内嵌回放：氨置顶、那个删除、哎呦添加
        let texts: Vec<String> = s.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["氨".to_string(), "来".to_string(), "哎呦".to_string()]);

        // 用户文件覆盖内嵌：用户删掉内嵌置顶的「氨」
        write(&tmp, "用户调整.txt", "{删除}a\t氨\n");
        let s2 = Schema::load(&tmp).unwrap();
        let texts2: Vec<String> = s2.candidates("a").iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts2, ["来".to_string(), "哎呦".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // 【/jc 选重位】用户词带 stem="pN"（加词窗第三框「第 N 选」）按
    // 绝对位次插入最终候选：N 超出候选数 → 排最后；否则插第 N 位、
    // 原第 N 位起后移。
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
        // 用户词：pos=3（插第 3 位）、pos=9（超出 → 最后）、无 pos（默认置顶）
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
                "子".to_string(), // 无 pos：置顶（v1 行为）
                "甲".to_string(),
                "酉".to_string(), // 第 3 选
                "乙".to_string(),
                "丙".to_string(),
                "丁".to_string(),
                "戊".to_string(),
                "己".to_string(),
                "戌".to_string(), // pos=9 超出（8 个）→ 最后
            ],
            "选重位插入: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
