//! 用户数据：用户调整（追加式日志）与用户词库。
//!
//! 【格式统一 2026-09-06】主文件 `用户调整.txt`，所有行统一
//! `{标记}码\t词` 格式：`{置顶}`（调频）、`{添加}`（/jc 加词，可选
//! 第三列 pN 选重位）、`{加权}`（/jq，第三列权重）、`{删除}`（删词）。
//! 追加式操作日志，回放得到当前调整态；写入端同词旧行先清后写，
//! 文件始终最新。旧 `用户词.txt`（TSV 词行+{标记}行混载）只读兼容。

use crate::dict::Dict;
use crate::entry::DictEntry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 调整操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustOp {
    /// 置顶：把 `码→词` 移到候选首位（重复置顶按时间累积，最新在前）
    Pin,
    /// 添加：把词条加入该码候选（不存在则新增）
    Add,
    /// 删除：把词条从该码候选中隐藏
    Remove,
    /// 加权：把 `码→词` 提到候选前部（用户词 weight 列）
    Weight,
}

/// 回放后的调整状态。
#[derive(Debug, Default, Clone)]
pub struct UserAdjust {
    /// 置顶日志（时间序）
    pins: Vec<(String, String)>,
    adds: Vec<(String, String)>,
    removes: HashSet<(String, String)>,
    /// 加权：码→词 → 权重（{加权}行第三列；缺省 1000）
    pub weights: HashMap<(String, String), f64>,
}

impl UserAdjust {
    pub fn parse(lines: &[String]) -> Self {
        let mut adj = UserAdjust::default();
        for line in lines {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            // 兼容 `{置顶}码\t词` 与裸日志格式
            let (op, rest) = if let Some(r) = t.strip_prefix("{置顶}") {
                (AdjustOp::Pin, r)
            } else if let Some(r) = t.strip_prefix("{添加}") {
                (AdjustOp::Add, r)
            } else if let Some(r) = t.strip_prefix("{删除}") {
                (AdjustOp::Remove, r)
            } else if let Some(r) = t.strip_prefix("{加权}") {
                (AdjustOp::Weight, r)
            } else {
                continue;
            };
            // 【虎爪内嵌兼容 2026-09-06】列分隔宽容：TAB 或空白均可；
            // 超过两列时多余列忽略（虎爪码表第三列常为日期
            // 2026-09-04 / 20260904 之类——只取 码+词）。
            let parts: Vec<&str> = rest
                .split(|c| c == '\t' || c == ' ' || c == '\u{3000}')
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() < 2 {
                continue;
            }
            let code = parts[0].trim().to_string();
            let word = parts[1].trim().to_string();
            if code.is_empty() || word.is_empty() {
                continue;
            }
            match op {
                AdjustOp::Pin => {
                    adj.pins.push((code, word));
                }
                AdjustOp::Add => {
                    adj.adds.push((code.clone(), word.clone()));
                    // 添加隐含取消删除（明确想要它）
                    adj.removes.remove(&(code, word));
                }
                AdjustOp::Remove => {
                    adj.removes.insert((code, word));
                }
                AdjustOp::Weight => {
                    // 第三列=权重（{加权}code\t词\t3000；缺省 1000）
                    let w = parts
                        .get(2)
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .unwrap_or(1000.0);
                    adj.weights.insert((code.clone(), word.clone()), w);
                    // 加权隐含取消删除（明确想要它）
                    adj.removes.remove(&(code, word));
                }
            }
        }
        adj
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&crate::parse::read_lines(path)?))
    }

    /// 序列化为追加日志文本（可回放）。
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (code, word) in &self.pins {
            out.push(format!("{{置顶}}{code}\t{word}"));
        }
        for (code, word) in &self.adds {
            out.push(format!("{{添加}}{code}\t{word}"));
        }
        for (code, word) in &self.removes {
            out.push(format!("{{删除}}{code}\t{word}"));
        }
        out
    }

    pub fn pin(&mut self, code: &str, word: &str) {
        self.pins.retain(|(c, w)| !(c == code && w == word));
        self.pins.push((code.to_string(), word.to_string()));
        // 置顶隐含取消删除
        self.removes.remove(&(code.to_string(), word.to_string()));
    }

    pub fn add(&mut self, code: &str, word: &str) {
        self.adds.retain(|(c, w)| !(c == code && w == word));
        self.adds.push((code.to_string(), word.to_string()));
        self.removes.remove(&(code.to_string(), word.to_string()));
    }

    pub fn remove(&mut self, code: &str, word: &str) {
        self.pins.retain(|(c, w)| !(c == code && w == word));
        self.adds.retain(|(c, w)| !(c == code && w == word));
        self.removes.insert((code.to_string(), word.to_string()));
    }

    /// 该 码→词 是否处于删除态（用户词分支过滤用：adjust.apply 只
    /// 过滤码表 base，用户词在 schema.candidates 单独合并）。
    pub fn removed(&self, code: &str, word: &str) -> bool {
        self.removes.contains(&(code.to_string(), word.to_string()))
    }

    /// 【格式统一 2026-09-06】用户数据统一落 `用户调整.txt`：
    /// `{置顶}/{添加}/{删除}/{加权}` 四种标记行（码\t词 主体，可选
    /// 第三列：{添加}=pN 选重位、{加权}=权重）。本函数按行前缀分拣
    /// ——返回 (词行, 调整行)：{添加} 行语义=词行（转 TSV 喂
    /// UserDict），其余进调整流。旧 `用户词.txt`（TSV 词行+{标记}行
    /// 混载）只读兼容，同样分拣。
    pub fn split_adjust_lines(lines: &[String]) -> (Vec<String>, Vec<String>) {
        let mut word_lines = Vec::new();
        let mut adj_lines = Vec::new();
        for l in lines {
            let t = l.trim_start().to_string();
            if let Some(rest) = t.strip_prefix("{添加}") {
                // {添加}code\t词[\tpN] → 词行 code\t词\t1[\tpN]；
                // 原行同时保留在调整流（UserAdjust 回放：Add 在文件序上
                // 取消同词 {删除}——加词=明确想要它，时序语义不能丢）
                let parts: Vec<&str> = rest
                    .split('\t')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() >= 2 {
                    let stem = parts.get(2).filter(|s| s.starts_with('p')).map(|s| *s);
                    let mut w = format!("{}\t{}\t1", parts[0], parts[1]);
                    if let Some(st) = stem {
                        w.push('\t');
                        w.push_str(st);
                    }
                    word_lines.push(w);
                    adj_lines.push(l.clone());
                    continue;
                }
                // 格式坏行原样归调整流（不丢数据）
                adj_lines.push(l.clone());
            } else if t.starts_with("{置顶}")
                || t.starts_with("{删除}")
                || t.starts_with("{加权}")
            {
                adj_lines.push(l.clone());
            } else if t.starts_with('#') || t.is_empty() {
                // 头注释/空行跳过
            } else {
                // 旧 TSV 词行（码\t词\tweight[\tpN]）原样词行
                word_lines.push(l.clone());
            }
        }
        (word_lines, adj_lines)
    }

    /// 应用到字典候选列表：返回调整后的条目序列。
    pub fn apply(&self, code: &str, base: &[DictEntry]) -> Vec<DictEntry> {
        let mut out: Vec<DictEntry> = Vec::new();
        // 1) 置顶（最新在前；命中码表的条目也标记 pinned）。
        //    【回放语义 2026-09-06】置顶之后又删除的（日志后操作=删除，
        //    虎爪码表内嵌置顶 + 用户文件删除的覆盖场景）不显示——
        //    removes 赢，与实时 pin()/remove() 的联动语义对齐。
        let pinned: Vec<&(String, String)> = self
            .pins
            .iter()
            .filter(|(c, _)| c == code)
            .collect();
        for (c, w) in pinned.iter().rev() {
            if self.removes.contains(&((*c).clone(), ((*w).clone()))) {
                continue;
            }
            if let Some(mut e) = base.iter().find(|e| e.code == *c && e.text == *w).cloned() {
                e.pinned = true;
                out.push(e);
            } else {
                let mut e = DictEntry::new(c.clone(), w.clone(), u32::MAX);
                e.pinned = true;
                out.push(e);
            }
        }
        // 2) 原始候选（跳过已置顶与已删除）
        for e in base {
            if self.removes.contains(&(e.code.clone(), e.text.clone())) {
                continue;
            }
            if pinned.iter().any(|(c, w)| *c == e.code && *w == e.text) {
                continue;
            }
            out.push(e.clone());
        }
        // 3) 添加（不存在时追加到尾部）
        for (c, w) in &self.adds {
            if c != code || self.removes.contains(&(c.clone(), w.clone())) {
                continue;
            }
            if !out.iter().any(|e| e.code == *c && e.text == *w) {
                out.push(DictEntry::new(c.clone(), w.clone(), u32::MAX - 1));
            }
        }
        out
    }
}

/// 用户词库（自造词），HuFu 原生格式持久化。
#[derive(Debug, Default, Clone)]
pub struct UserDict {
    pub entries: Vec<DictEntry>,
    /// 隐藏的词
    pub hidden: HashSet<(String, String)>,
    /// 自定义权重（词 → 权重）
    pub weights: HashMap<(String, String), f64>,
    /// 【码索引 2026-10-02】code → entries 下标（merge_into 每键热路径
    /// 用）。entries 的增删改必须经 add_word/absorb/retain_rebuild
    /// 维护；直接 push 会漏登记（外部仅测试这么干，retain_rebuild
    /// 可兜底重建）。十万级用户码表（多多导出误入）全量线性扫
    /// 每键 1-2ms×多次调用——索引后只碰命中码的条目。
    by_code: HashMap<String, Vec<usize>>,
}

impl UserDict {
    pub fn parse(lines: &[String]) -> Self {
        let t = crate::parse::native::parse(lines);
        UserDict {
            entries: t.rows,
            hidden: HashSet::new(),
            weights: HashMap::new(),
            by_code: HashMap::new(),
        }
        .reindexed()
    }

    /// 按 entries 重建码索引（吞并/裁剪后调用）。
    pub fn reindexed(mut self) -> Self {
        self.by_code = HashMap::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            self.by_code.entry(e.code.clone()).or_default().push(i);
        }
        self
    }

    /// 【多多用户码表并入 O(n) 2026-10-02】旧实现每行对 entries 全量
    /// 线性查重——10.7 万行 ≈ 57 亿次比较 ≈ 118s（用户把多多导出当
    /// 码表拖进方案目录的实测病灶）。HashSet 一次建成，并入整体线性。
    /// 返回实际并入条数。
    pub fn absorb(&mut self, rows: Vec<DictEntry>) -> usize {
        let mut seen: HashSet<(String, String)> = self
            .entries
            .iter()
            .map(|e| (e.code.clone(), e.text.clone()))
            .collect();
        let mut n = 0usize;
        for mut e in rows {
            if !seen.insert((e.code.clone(), e.text.clone())) {
                continue;
            }
            e.weight = 1.0;
            // 并入置顶标记清零（用户词默认不置顶，置顶由
            // 用户调整.txt 的 {置顶} 行控制；原为 clippy 自赋值修复）
            e.pinned = false;
            let code = e.code.clone();
            self.by_code.entry(code).or_default().push(self.entries.len());
            self.entries.push(e);
            n += 1;
        }
        n
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&crate::parse::read_lines(path)?))
    }

    pub fn to_lines(&self) -> Vec<String> {
        let mut out = vec!["#hufu-dict v1 name=user_words".to_string()];
        for e in &self.entries {
            let hidden = if self.hidden.contains(&(e.code.clone(), e.text.clone())) {
                "\t#hidden"
            } else {
                ""
            };
            out.push(format!("{}\t{}\t{}{}", e.code, e.text, e.weight as i64, hidden));
        }
        out
    }

    pub fn add_word(&mut self, code: &str, word: &str) {
        if let Some(&i) = self
            .by_code
            .get(code)
            .and_then(|idxs| idxs.iter().find(|&&i| self.entries[i].text == word))
        {
            self.entries[i].weight += 1.0;
            self.hidden.remove(&(code.to_string(), word.to_string()));
        } else {
            let mut e = DictEntry::new(code, word, self.entries.len() as u32);
            e.weight = 1.0;
            self.by_code.entry(code.to_string()).or_default().push(self.entries.len());
            self.entries.push(e);
        }
    }

    /// 合入字典检索结果：用户词优先于同码低权重系统词。
    /// 【码索引 2026-10-02】旧实现每键全量扫 entries；十万级用户
    /// 码表下每 candidates() 调用 1-2ms（每次按键调多次）。改走
    /// by_code 只碰命中码条目。
    pub fn merge_into(&self, code: &str, base: &Dict, out: &mut Vec<DictEntry>) {
        if let Some(idxs) = self.by_code.get(code) {
            for &i in idxs {
                let e = &self.entries[i];
                if !self.hidden.contains(&(e.code.clone(), e.text.clone())) {
                    let mut e = e.clone();
                    if let Some(w) = self.weights.get(&(e.code.clone(), e.text.clone())) {
                        e.weight = *w;
                    }
                    out.push(e);
                }
            }
        }
        let _ = base;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<DictEntry> {
        vec![
            DictEntry::new("a", "来", 0),
            DictEntry::new("a", "叉", 1),
            DictEntry::new("a", "氨", 2),
        ]
    }

    #[test]
    fn adjust_replay() {
        let lines: Vec<String> = vec![
            "{置顶}a\t叉".into(),
            "{删除}a\t氨".into(),
        ];
        let adj = UserAdjust::parse(&lines);
        let out = adj.apply("a", &base());
        let texts: Vec<&str> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["叉", "来"]);

        // 序列化回放等价
        let adj2 = UserAdjust::parse(&adj.to_lines());
        assert_eq!(adj2.apply("a", &base()), out);
    }

    #[test]
    fn add_and_pin() {
        let mut adj = UserAdjust::default();
        adj.add("a", "哎呦");
        adj.pin("a", "叉");
        let out = adj.apply("a", &base());
        let texts: Vec<&str> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["叉", "来", "氨", "哎呦"]);
    }

    #[test]
    fn user_dict_weighting() {
        let mut ud = UserDict::default();
        ud.add_word("jj", "自己");
        ud.add_word("jj", "自己");
        assert_eq!(ud.entries.len(), 1);
        assert_eq!(ud.entries[0].weight, 2.0);
    }

    // 【码索引/O(n) 并入 2026-10-02】十万级用户码表（多多导出误入方案
    // 目录）旧实现：absorb 前身逐行线性查重 O(n²)≈118s、merge_into
    // 每键全量扫 1-2ms。absorb 去重 + by_code 索引后两条路径都只碰
    // 命中项；外部直接 push 的 entries 由 reindexed 兜底重建索引。
    #[test]
    fn absorb_dedup_and_index_consistency() {
        let mut ud = UserDict::default();
        ud.add_word("jj", "自己");
        let rows = vec![
            DictEntry::new("a", "工", 0),
            DictEntry::new("jj", "自己", 0), // 与已有重复 → 跳过
            DictEntry::new("a", "工", 1),    // 组内重复 → 跳过
            DictEntry::new("zzt", "自造词", 0),
        ];
        let n = ud.absorb(rows);
        assert_eq!(n, 2, "只并入两个新词");
        assert_eq!(ud.entries.len(), 3);
        // absorb 清零权重/置顶（用户词语义）
        assert_eq!(ud.entries.iter().find(|e| e.text == "工").unwrap().weight, 1.0);
        // 索引生效：merge_into 只出命中码
        let mut out = Vec::new();
        ud.merge_into("zzt", &Dict::new("t"), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "自造词");
        let mut out2 = Vec::new();
        ud.merge_into("jj", &Dict::new("t"), &mut out2);
        assert_eq!(out2.len(), 1, "旧词仍可命中（含重复行未重复并入）");
        // 直接 push 绕过索引 → reindexed 兜底
        ud.entries.push(DictEntry::new("qq", "补", 0));
        let ud = ud.reindexed();
        let mut out3 = Vec::new();
        ud.merge_into("qq", &Dict::new("t"), &mut out3);
        assert_eq!(out3.len(), 1, "reindexed 重建索引后命中");
    }

    // 【虎爪内嵌兼容】空格/全角空格分隔 + 第三列日期（任意形态）忽略
    #[test]
    fn parse_tigerclaw_embedded_lines() {
        let lines: Vec<String> = vec![
            "{置顶}a 叉 2026-09-04".into(),
            "{添加}a 哎呦 20260904".into(),
            "{删除}a\u{3000}氨\t2026/09/05".into(),
            // 裸日志（原生 TAB）不受影响
            "{置顶}ab\t你好".into(),
        ];
        let adj = UserAdjust::parse(&lines);
        let out = adj.apply("a", &base());
        let texts: Vec<&str> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["叉", "来", "哎呦"]); // 氨被删、哎呦添加
        let out2 = adj.apply("ab", &[]);
        assert_eq!(out2[0].text, "你好");
    }
}
