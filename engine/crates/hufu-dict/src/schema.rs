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
    /// 反查表（码 → 词）
    pub reverse: Option<ReverseTable>,
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
            adjust: UserAdjust::default(),
            user_dict: UserDict::default(),
            encoder_rules: Vec::new(),
        };

        let mut rime_dicts: Vec<PathBuf> = Vec::new();
        let mut big_tables: Vec<PathBuf> = Vec::new();
        let mut duoduo_user: Option<PathBuf> = None;

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
                "用户调整" => schema.adjust = UserAdjust::load(&path)?,
                "用户词" => schema.user_dict = UserDict::load(&path)?,
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
                    schema.reverse = Some(ReverseTable::load(&path)?);
                } else {
                    // 其余 txt：可能是主码表（多多/QQ五笔/虎整句/多多用户词）
                    if stem.contains("用户词") {
                        duoduo_user = Some(path.clone());
                    }
                    big_tables.push(path.clone());
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
            let table = parse_file(main)?;
            schema.dict = std::sync::Arc::new(Dict::from_entries(name.clone(), table.rows));
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
        // 用户词插到置顶之后、系统词之前
        let mut user_entries: Vec<DictEntry> = Vec::new();
        self.user_dict.merge_into(code, &self.dict, &mut user_entries);
        let mut merged: Vec<DictEntry> = Vec::new();
        let pinned: Vec<DictEntry> = out.iter().filter(|e| e.pinned).cloned().collect();
        merged.extend(pinned);
        for ue in user_entries {
            if !merged.iter().any(|e| e.text == ue.text) {
                merged.push(ue);
            }
        }
        for e in out {
            if e.pinned {
                continue;
            }
            if !merged.iter().any(|x| x.text == e.text) {
                merged.push(e);
            }
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
}
