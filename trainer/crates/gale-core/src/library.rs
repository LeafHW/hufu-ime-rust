//! 本地文库管理 —— Pain 风格分类目录 + 文章配置 + 进度（`文库/` 根放整本 txt/epub，
//! `文章/入门|综合|英文|难` 子目录分类；`ArticleConfig.txt` TAB 分隔配置，蓝本 Pain 打器）。
//!
//! Pain 原格式（ArticleConfig.txt，TAB 分隔）：
//! ```text
//! 每段字数	200
//! 字集过滤	是
//! 去除空格	是
//! 智能分段	否
//! 当前文章	入门/彷徨之刃.epub
//! 进度_入门/彷徨之刃.epub	400
//! ```
//! 兼容：`文库/library.json` 旧进度（BookProgress）继续读写；分类子目录缺失时自动创建。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::segment::{split_smart, Segment};
use crate::GaleResult;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BookProgress {
    /// 书名 → 下一段序号
    pub next_para: BTreeMap<String, usize>,
    /// 书名 → 累计已打字数
    pub typed_chars: BTreeMap<String, u64>,
}

/// 文章分类（Pain `文章/` 子目录对齐：入门/综合/英文/难）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Beginner, // 入门
    General,  // 综合
    English,  // 英文
    Hard,     // 难
    Root,     // 文库根目录（未分类，兼容旧文库布局）
}

impl Category {
    pub fn dir_name(self) -> &'static str {
        match self {
            Category::Beginner => "入门",
            Category::General => "综合",
            Category::English => "英文",
            Category::Hard => "难",
            Category::Root => "",
        }
    }
    pub fn from_dir_name(s: &str) -> Option<Self> {
        Some(match s {
            "入门" => Category::Beginner,
            "综合" => Category::General,
            "英文" => Category::English,
            "难" => Category::Hard,
            _ => return None,
        })
    }
}

/// ArticleConfig.txt（TAB 分隔 key→value，Pain 风格）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleConfig {
    /// 每段字数（切段目标；Pain 默认 200）
    pub seg_chars: usize,
    /// 字集过滤（是/否；true 时按白名单滤掉不在字集的字符）
    pub charset_filter: bool,
    /// 去除空格（是/否；英文文章外的中文常开）
    pub strip_spaces: bool,
    /// 智能分段（是/否；false = 硬切）
    pub smart_split: bool,
    /// 当前文章（相对路径，如 "入门/彷徨之刃.epub"）
    pub current_article: String,
    /// 进度表：文章相对路径 → 已打字数（Pain 用 `进度_{path}` 键）
    #[serde(skip)]
    pub progress: BTreeMap<String, u64>,
}

impl Default for ArticleConfig {
    fn default() -> Self {
        Self {
            seg_chars: 200,
            charset_filter: false,
            strip_spaces: false,
            smart_split: false,
            current_article: String::new(),
            progress: BTreeMap::new(),
        }
    }
}

impl ArticleConfig {
    /// 解析 Pain 风格 ArticleConfig.txt（TAB 分隔；行无 TAB 时跳过）
    pub fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('\t') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "每段字数" => {
                    if let Ok(n) = value.parse::<usize>() {
                        if n > 0 {
                            cfg.seg_chars = n;
                        }
                    }
                }
                "字集过滤" => cfg.charset_filter = value == "是",
                "去除空格" => cfg.strip_spaces = value == "是",
                "智能分段" => cfg.smart_split = value == "是",
                "当前文章" => cfg.current_article = value.to_string(),
                k => {
                    if let Some(path) = k.strip_prefix("进度_") {
                        if let Ok(n) = value.parse::<u64>() {
                            cfg.progress.insert(path.to_string(), n);
                        }
                    }
                }
            }
        }
        cfg
    }

    /// 序列化为 Pain 风格文本（TAB 分隔；进度键 `进度_{path}`）
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("每段字数\t{}\n", self.seg_chars));
        out.push_str(&format!("字集过滤\t{}\n", yes_no(self.charset_filter)));
        out.push_str(&format!("去除空格\t{}\n", yes_no(self.strip_spaces)));
        out.push_str(&format!("智能分段\t{}\n", yes_no(self.smart_split)));
        if !self.current_article.is_empty() {
            out.push_str(&format!("当前文章\t{}\n", self.current_article));
        }
        for (path, chars) in &self.progress {
            out.push_str(&format!("进度_{path}\t{chars}\n"));
        }
        out
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "是"
    } else {
        "否"
    }
}

pub struct Library {
    pub dir: PathBuf,
    pub progress: BookProgress,
    /// Pain 风格文章配置（ArticleConfig.txt）
    pub article_config: ArticleConfig,
    /// 最近打开的文章相对路径（新的在前，去重，上限 10）
    pub recent: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Book {
    pub name: String,
    pub path: PathBuf,
    /// 相对文库根的路径（分类前缀，如 "入门/秘密.txt"；根目录书 = 文件名）
    pub rel_path: String,
    pub category: Category,
    pub segments: Vec<Segment>,
    /// 原始全文（load_book 智能切段用；进度按 resume_para 对齐可能偏移——
    /// 段号不一致时从头切也是安全退化）
    pub raw_text: String,
}

impl Library {
    pub fn open(dir: &Path) -> GaleResult<Self> {
        std::fs::create_dir_all(dir)?;
        // 分类子目录确保存在（Pain 布局）
        for cat in [
            Category::Beginner,
            Category::General,
            Category::English,
            Category::Hard,
        ] {
            std::fs::create_dir_all(dir.join(cat.dir_name())).ok();
        }
        let pfile = dir.join("library.json");
        let progress = if pfile.exists() {
            match serde_json::from_str(&std::fs::read_to_string(&pfile)?) {
                Ok(p) => p,
                Err(_) => BookProgress::default(),
            }
        } else {
            BookProgress::default()
        };
        let cfg_path = dir.join("ArticleConfig.txt");
        let article_config = if cfg_path.exists() {
            ArticleConfig::parse(&read_text_auto(&cfg_path)?)
        } else {
            ArticleConfig::default()
        };
        let recent = load_recent(dir);
        Ok(Self {
            dir: dir.to_path_buf(),
            progress,
            article_config,
            recent,
        })
    }

    pub fn save_progress(&self) -> GaleResult<()> {
        let pfile = self.dir.join("library.json");
        std::fs::write(&pfile, serde_json::to_string_pretty(&self.progress)?)?;
        // ArticleConfig 同步落盘（当前文章 + 进度键）
        std::fs::write(
            self.dir.join("ArticleConfig.txt"),
            self.article_config.to_text(),
        )?;
        Ok(())
    }

    /// 列出所有书（分类子目录 + 根目录 txt；分类内按文件名排序，分类顺序 入门<综合<英文<难<根）
    pub fn books(&self) -> Vec<Book> {
        let mut out = Vec::new();
        self.push_books(Category::Root, &mut out);
        for cat in [
            Category::Beginner,
            Category::General,
            Category::English,
            Category::Hard,
        ] {
            self.push_books(cat, &mut out);
        }
        out
    }

    fn push_books(&self, cat: Category, out: &mut Vec<Book>) {
        let sub = if cat == Category::Root {
            self.dir.clone()
        } else {
            self.dir.join(cat.dir_name())
        };
        let Ok(rd) = std::fs::read_dir(&sub) else {
            return;
        };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("txt") | Some("epub")
                    )
                    // 文库自身的元数据文件不算书（ArticleConfig.txt 也是 .txt！）
                    && p.file_name()
                        .map(|n| !n.eq_ignore_ascii_case("ArticleConfig.txt"))
                        .unwrap_or(true)
            })
            .collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let rel_path = if cat == Category::Root {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                format!(
                    "{}/{}",
                    cat.dir_name(),
                    p.file_name().unwrap_or_default().to_string_lossy()
                )
            };
            // epub 目前只编目（ VersOne.Epub 是 Pain 的 C# 依赖；正文提取留待平台层）
            if rel_path.ends_with(".epub") {
                out.push(Book {
                    name,
                    path: p,
                    rel_path,
                    category: cat,
                    segments: Vec::new(),
                    raw_text: String::new(),
                });
                continue;
            }
            if let Ok(raw_text) = read_text_auto(&p) {
                let text = raw_text.clone();
                let cfg = &self.article_config;
                let text = apply_text_options(&text, cfg);
                let seg_chars = cfg.seg_chars.max(1);
                let segments = if cfg.smart_split {
                    split_smart(&text, seg_chars)
                } else {
                    split_hard(&text, seg_chars)
                };
                out.push(Book {
                    name,
                    path: p,
                    rel_path,
                    category: cat,
                    segments,
                    raw_text,
                });
            }
        }
    }

    pub fn book(&self, name: &str) -> Option<Book> {
        self.books().into_iter().find(|b| b.name == name)
    }

    /// 按相对路径找书（ArticleConfig.current_article 用这个键）
    pub fn book_by_rel_path(&self, rel_path: &str) -> Option<Book> {
        self.books().into_iter().find(|b| b.rel_path == rel_path)
    }

    /// 全文库搜索：标题或正文包含关键词的书 → (书, 命中段序号列表)
    pub fn search(&self, keyword: &str) -> Vec<(Book, Vec<usize>)> {
        let kw = keyword.trim();
        if kw.is_empty() {
            return Vec::new();
        }
        self.books()
            .into_iter()
            .filter_map(|b| {
                let mut hits: Vec<usize> = b
                    .segments
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.text.contains(kw))
                    .map(|(i, _)| i)
                    .collect();
                if b.name.contains(kw) && !b.segments.is_empty() && hits.is_empty() {
                    hits.push(0); // 标题命中 → 跳到第一段
                }
                if hits.is_empty() && !b.name.contains(kw) {
                    None
                } else {
                    Some((b, hits))
                }
            })
            .collect()
    }

    /// 记住进度（书名键，旧 library.json 通道）
    pub fn advance(&mut self, book: &str, para_index: usize, typed: u64) {
        let e = self.progress.next_para.entry(book.to_string()).or_insert(0);
        if para_index + 1 > *e {
            *e = para_index + 1;
        }
        *self
            .progress
            .typed_chars
            .entry(book.to_string())
            .or_insert(0) += typed;
    }

    pub fn resume_para(&self, book: &str) -> usize {
        self.progress.next_para.get(book).copied().unwrap_or(0)
    }

    /// 打开文章（更新 recent + current_article；**不**自动清进度）
    pub fn open_article(&mut self, rel_path: &str) {
        self.recent.retain(|r| r != rel_path);
        self.recent.insert(0, rel_path.to_string());
        self.recent.truncate(10);
        self.article_config.current_article = rel_path.to_string();
        persist_recent(&self.dir, &self.recent);
    }

    /// 累计已打字数（Pain `进度_{path}` 通道）
    pub fn typed_of(&self, rel_path: &str) -> u64 {
        self.article_config
            .progress
            .get(rel_path)
            .copied()
            .unwrap_or(0)
    }

    /// 累加已打字数（Pain 通道）
    pub fn add_typed(&mut self, rel_path: &str, chars: u64) {
        *self
            .article_config
            .progress
            .entry(rel_path.to_string())
            .or_insert(0) += chars;
    }
}

/// Explicit legacy text options affect the working copy only. Default loading keeps
/// spaces, watermark-looking lines, circled digits and joiners exactly as supplied.
fn apply_text_options(text: &str, cfg: &ArticleConfig) -> String {
    // This legacy explicit option only affects the training copy; Book.raw_text is
    // always retained. Watermark/circled-digit transformations require a previewed preset.
    if cfg.strip_spaces {
        text.replace(' ', "").replace('\u{3000}', "")
    } else {
        text.to_string()
    }
}

/// 硬切分段（智能分段=否 时用）：固定字数直切，段号 1..n
fn split_hard(text: &str, target: usize) -> Vec<Segment> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut cut = start.saturating_add(target.max(1)).min(chars.len());
        if chars[cut - 1] == '\r' && chars.get(cut) == Some(&'\n') {
            if cut > start + 1 {
                cut -= 1;
            } else {
                cut += 1;
            }
        }
        out.push(Segment {
            mark: (out.len() + 1).to_string(),
            text: chars[start..cut].iter().collect(),
        });
        start = cut;
    }
    out
}

/// recent.json：最近打开列表
fn load_recent(dir: &Path) -> Vec<String> {
    let p = dir.join("recent.json");
    if let Ok(text) = std::fs::read_to_string(&p) {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(&text) {
            return v;
        }
    }
    Vec::new()
}

fn persist_recent(dir: &Path, recent: &[String]) {
    if let Ok(json) = serde_json::to_string_pretty(recent) {
        std::fs::write(dir.join("recent.json"), json).ok();
    }
}

/// UTF-8 / UTF-16 / GBK 自动识别读取（蓝本：疾风1代 载文逻辑 + P2' 勘误补 UTF-16 BOM）
pub fn read_text_auto(path: &Path) -> GaleResult<String> {
    let bytes = std::fs::read(path)?;
    // BOM 检测：UTF-8 BOM 直接剥
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Ok(String::from_utf8_lossy(&bytes[3..]).into_owned());
    }
    // P2'：记事本"Unicode"默认存法（UTF-16LE FF FE / UTF-16BE FE FF），必踩坑
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Ok(decode_utf16(&bytes[2..], true));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(decode_utf16(&bytes[2..], false));
    }
    if let Ok(s) = String::from_utf8(bytes.clone()) {
        return Ok(s);
    }
    // GBK 简易解码：常见汉字范围映射（无第三方依赖的兜底；优先 UTF-8）
    // 若需要精确 GBK，后续在 gale-win 加 encoding_rs（平台层允许重依赖）
    decode_gbk_fallback(&bytes)
}

/// UTF-16 手写解码（无新依赖，含代理对；参考 jifeng decode_text article.rs:82-99）。
/// 无 BOM 尾字节（奇数长度）忽略；孤立代理替换 U+FFFD。
fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

/// GBK 兜底解码（覆盖常用汉字/标点的查表不现实，这里用"替换字符"策略并尝试 BOM 检测）
fn decode_gbk_fallback(bytes: &[u8]) -> GaleResult<String> {
    // 无损保底：损失中文可读性但绝不崩溃；gale-win 层可用 encoding_rs 精确解码
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_bom_decoding() {
        // P2'：记事本"Unicode"默认存法（UTF-16LE / UTF-16BE 带 BOM）
        let dir = crate::test_util::tmp("enc");
        std::fs::create_dir_all(&dir).unwrap();
        // UTF-16LE（Windows 记事本 "Unicode"）
        let mut le: Vec<u8> = vec![0xFF, 0xFE];
        for u in "疾风2跟打器，第①行。".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        // 含代理对（U+2000D 𠀍，扩展B区汉字）
        let mut le2 = le.clone();
        for u in "\u{2000D}测试".encode_utf16() {
            le2.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(dir.join("le.txt"), &le2).unwrap();
        assert_eq!(
            read_text_auto(&dir.join("le.txt")).unwrap(),
            "疾风2跟打器，第①行。\u{2000D}测试"
        );
        // UTF-16BE
        let mut be: Vec<u8> = vec![0xFE, 0xFF];
        for u in "大江东去浪淘尽。".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        std::fs::write(dir.join("be.txt"), &be).unwrap();
        assert_eq!(
            read_text_auto(&dir.join("be.txt")).unwrap(),
            "大江东去浪淘尽。"
        );
        // 奇数长度尾字节：不 panic，忽略尾字节
        let mut odd = le.clone();
        odd.pop();
        std::fs::write(dir.join("odd.txt"), &odd).unwrap();
        let t = read_text_auto(&dir.join("odd.txt")).unwrap();
        assert!(t.starts_with("疾风2跟打器"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn library_progress_roundtrip() {
        let dir = crate::test_util::tmp("lib");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("测试书.txt"), "第一句。第二句。第三句。").unwrap();
        let mut lib = Library::open(&dir).unwrap();
        let books = lib.books();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "测试书");
        assert_eq!(books[0].category, Category::Root);
        assert_eq!(books[0].rel_path, "测试书.txt");
        lib.advance("测试书", 0, 5);
        lib.save_progress().unwrap();
        let lib2 = Library::open(&dir).unwrap();
        assert_eq!(lib2.resume_para("测试书"), 1);
        assert_eq!(lib2.progress.typed_chars["测试书"], 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn article_config_parse_pain_format() {
        // Pain 实测样例（ArticleConfig.txt，TAB 分隔）
        let text = "每段字数\t200\n字集过滤\t是\n去除空格\t是\n智能分段\t否\n当前文章\t入门/彷徨之刃.epub\n进度_入门/彷徨之刃.epub\t400\n";
        let cfg = ArticleConfig::parse(text);
        assert_eq!(cfg.seg_chars, 200);
        assert!(cfg.charset_filter);
        assert!(cfg.strip_spaces);
        assert!(!cfg.smart_split);
        assert_eq!(cfg.current_article, "入门/彷徨之刃.epub");
        assert_eq!(cfg.progress.get("入门/彷徨之刃.epub"), Some(&400));
        // roundtrip
        let rt = ArticleConfig::parse(&cfg.to_text());
        assert_eq!(rt.seg_chars, cfg.seg_chars);
        assert_eq!(rt.charset_filter, cfg.charset_filter);
        assert_eq!(rt.strip_spaces, cfg.strip_spaces);
        assert_eq!(rt.smart_split, cfg.smart_split);
        assert_eq!(rt.current_article, cfg.current_article);
        assert_eq!(rt.progress, cfg.progress);
    }

    #[test]
    fn article_config_defaults_and_garbage() {
        let cfg = ArticleConfig::parse("");
        assert_eq!(cfg.seg_chars, 200);
        assert!(!cfg.charset_filter);
        assert!(!cfg.strip_spaces);
        // 坏值不炸：非数字回落默认
        let cfg = ArticleConfig::parse("每段字数\tabc\n无TAB行也跳过\n去除空格\t否\n");
        assert_eq!(cfg.seg_chars, 200);
        assert!(!cfg.strip_spaces);
    }

    #[test]
    fn categories_scan_pain_layout() {
        let dir = crate::test_util::tmp("libcat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("入门")).unwrap();
        std::fs::create_dir_all(dir.join("难")).unwrap();
        std::fs::write(
            dir.join("入门").join("秘密.txt"),
            &"床前明月光。".repeat(50),
        )
        .unwrap();
        std::fs::write(
            dir.join("难").join("文心雕龙.txt"),
            &"文之为德也大矣。".repeat(50),
        )
        .unwrap();
        std::fs::write(dir.join("根目录书.txt"), "根目录正文。").unwrap();
        // 非 txt/epub 忽略
        std::fs::write(dir.join("笔记.md"), "markdown").unwrap();
        let lib = Library::open(&dir).unwrap();
        let books = lib.books();
        // 排序：根目录书在前，然后 入门 → 难
        assert_eq!(books.len(), 3);
        assert_eq!(books[0].name, "根目录书");
        assert_eq!(books[0].category, Category::Root);
        assert_eq!(books[1].name, "秘密");
        assert_eq!(books[1].category, Category::Beginner);
        assert_eq!(books[1].rel_path, "入门/秘密.txt");
        assert_eq!(books[2].name, "文心雕龙");
        assert_eq!(books[2].category, Category::Hard);
        // 默认智能分段=否 → 硬切 200 字
        assert!(books[1]
            .segments
            .iter()
            .all(|s| s.text.chars().count() <= 200));
        // rel_path 检索
        assert!(lib.book_by_rel_path("入门/秘密.txt").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn epub_cataloged_without_text() {
        let dir = crate::test_util::tmp("libepub");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("入门")).unwrap();
        std::fs::write(dir.join("入门").join("书.epub"), b"PK\x03\x04fake").unwrap();
        let lib = Library::open(&dir).unwrap();
        let books = lib.books();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "书");
        assert!(books[0].raw_text.is_empty()); // 正文提取留平台层
        assert!(books[0].segments.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_hits_title_and_content() {
        let dir = crate::test_util::tmp("libsearch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("三国演义.txt"),
            "话说天下大势，分久必合，合久必分。",
        )
        .unwrap();
        std::fs::write(dir.join("菜谱.txt"), "红烧肉要冰糖炒糖色。").unwrap();
        let lib = Library::open(&dir).unwrap();
        let hits = lib.search("天下大势");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.name, "三国演义");
        assert_eq!(hits[0].1, vec![0]);
        // 标题命中
        let hits = lib.search("菜谱");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains(&0));
        // 无命中
        assert!(lib.search("不存在的词").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_and_typed_progress() {
        let dir = crate::test_util::tmp("librecent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "内容甲。").unwrap();
        std::fs::write(dir.join("b.txt"), "内容乙。").unwrap();
        let mut lib = Library::open(&dir).unwrap();
        lib.open_article("a.txt");
        lib.open_article("b.txt");
        lib.open_article("a.txt"); // 去重置顶
        assert_eq!(lib.recent, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert_eq!(lib.article_config.current_article, "a.txt");
        lib.add_typed("a.txt", 120);
        lib.add_typed("a.txt", 80);
        assert_eq!(lib.typed_of("a.txt"), 200);
        lib.save_progress().unwrap();
        // 重开：recent + 进度 + 当前文章都恢复
        let lib2 = Library::open(&dir).unwrap();
        assert_eq!(lib2.recent, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert_eq!(lib2.article_config.current_article, "a.txt");
        assert_eq!(lib2.typed_of("a.txt"), 200);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn strip_spaces_option() {
        let dir = crate::test_util::tmp("libspace");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("带空格.txt"),
            "床 前 明 月 光 。\u{3000}疑 是 地 上 霜 。",
        )
        .unwrap();
        // Default preserves every character and raw_text always stays original.
        let lib = Library::open(&dir).unwrap();
        let books = lib.books();
        assert!(books[0].raw_text.contains("床 前"));
        assert!(books[0].segments[0].text.contains(' '));
        drop(lib);
        // 关掉后保留（同一目录换配置重开）
        let cfg = ArticleConfig {
            strip_spaces: true,
            ..ArticleConfig::default()
        };
        std::fs::write(dir.join("ArticleConfig.txt"), cfg.to_text()).unwrap();
        let lib2 = Library::open(&dir).unwrap();
        assert!(lib2.books()[0].raw_text.contains("床 前"));
        assert!(!lib2.books()[0].segments[0].text.contains(' '));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hard_split_vs_smart_split() {
        // 智能分段=否（默认）：段尾不回退句末
        let cfg = ArticleConfig {
            smart_split: false,
            ..ArticleConfig::default()
        };
        assert!(!cfg.smart_split);
        let segs = split_hard("一二三四五六七八九十。", 4);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "一二三四");
        assert_eq!(segs[0].mark, "1");
        assert_eq!(segs[2].text, "九十。");
        // 智能分段=是：split_smart 段尾回退句末标点——只验内容不丢 + 段数>0
        // （具体回退窗口行为由 segment.rs 的 smart_split_* 测试锁定）
        let smart = split_smart("一二三四五六七八九十。", 4);
        assert!(!smart.is_empty());
        let joined: String = smart.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "一二三四五六七八九十。");
        // 有内部句读时：硬切固定 4 字；智能切段尾落在句末标点
        let hard2 = split_hard("一二三。四五六七八九十。", 4);
        assert_eq!(hard2[0].text, "一二三。");
        assert_eq!(hard2.len(), 3); // 4+4+4 硬切
        let smart2 = split_smart("一二三。四五六七八九十。", 4);
        // 智能切段尾优先句末标点（"一二三。"）；回退窗口（60%..100%）内无句读时按 target 硬切
        assert_eq!(smart2[0].text, "一二三。");
        assert_eq!(smart2[1].text, "四五六七"); // 窗口内无句末 → 不回退
        for target in 1..=6 {
            let raw = "甲\r\n乙\r\n\r\n丙";
            let parts = split_hard(raw, target);
            assert_eq!(
                parts.iter().map(|p| p.text.as_str()).collect::<String>(),
                raw
            );
            assert!(!parts
                .windows(2)
                .any(|p| p[0].text.ends_with('\r') && p[1].text.starts_with('\n')));
        }
    }
}
