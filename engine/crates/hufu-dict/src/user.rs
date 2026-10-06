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
    /// 添加：把词条加入该码候选（不存在则新增；可选 pN 选重位）
    Add,
    /// 删除：把词条从该码候选中隐藏
    Remove,
    /// 加权：把 `码→词` 提到候选前部（用户词 weight 列）
    Weight,
}

/// 一条调整日志（保持文件时序——回放语义的基础）。
#[derive(Debug, Clone)]
struct AdjustEntry {
    op: AdjustOp,
    code: String,
    word: String,
    /// 【时序回放 2026-11 用户拍板】{添加} 第三列 pN 选重位（1 基）。
    /// 插入位次以**该操作时刻**的列表度量；其后的 {删除} 会让它连同
    /// 后面所有词一起前移（用户实测：删除 2 选后 p5 词应变 4 选）。
    pos: Option<usize>,
}

/// 回放后的调整状态。
#[derive(Debug, Default, Clone)]
pub struct UserAdjust {
    /// 全部操作日志（文件时序：内嵌 → 旧用户词 → 用户调整）。
    log: Vec<AdjustEntry>,
    /// 当前删除态集合（回放派生：{添加}/{置顶}/{加权} 隐含取消删除）
    removes: std::collections::HashSet<(String, String)>,
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
            // 【列分隔 2026-11 修复】含 TAB 的行严格按 TAB 分割——词可
            // 含空格（{添加}ae\tipad mini\tp6 的「ipad mini」曾是空格
            // 分割丢尾成幽灵「ipad」）。无 TAB 行（旧虎爪内嵌行）保持
            // 空白宽容分割。
            let parts: Vec<&str> = if rest.contains('\t') {
                rest.split('\t')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                rest.split(|c| c == ' ' || c == '\u{3000}')
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            if parts.len() < 2 {
                continue;
            }
            let code = parts[0].trim().to_string();
            let word = parts[1].trim().to_string();
            if code.is_empty() || word.is_empty() {
                continue;
            }
            // 【时序回放 2026-11】pN 选重位只对 {添加} 有意义（第三列
            // 以 p 开头+数字）；其他标记的第三列（日期等）忽略。
            let pos = if op == AdjustOp::Add {
                parts
                    .get(2)
                    .and_then(|s| s.strip_prefix('p'))
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| *n >= 1)
            } else {
                None
            };
            match op {
                AdjustOp::Pin | AdjustOp::Add | AdjustOp::Weight => {
                    // 添加/置顶/加权隐含取消删除（明确想要它）；日志
                    // 保持原样追加（时序回放语义不能因状态折叠丢失）
                    adj.removes.remove(&(code.clone(), word.clone()));
                    if op == AdjustOp::Weight {
                        let w = parts
                            .get(2)
                            .and_then(|s| s.trim().parse::<f64>().ok())
                            .unwrap_or(1000.0);
                        adj.weights.insert((code.clone(), word.clone()), w);
                    }
                }
                AdjustOp::Remove => {
                    adj.removes.insert((code.clone(), word.clone()));
                }
            }
            adj.log.push(AdjustEntry {
                op,
                code,
                word,
                pos,
            });
        }
        adj
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&crate::parse::read_lines(path)?))
    }

    /// 序列化为追加日志文本（可回放）。
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.log {
            let mark = match e.op {
                AdjustOp::Pin => "{置顶}",
                AdjustOp::Add => "{添加}",
                AdjustOp::Remove => "{删除}",
                AdjustOp::Weight => "{加权}",
            };
            let mut l = format!("{mark}{}\t{}", e.code, e.word);
            if let Some(p) = e.pos {
                l.push_str(&format!("\tp{p}"));
            }
            out.push(l);
        }
        out
    }

    pub fn pin(&mut self, code: &str, word: &str) {
        self.log.retain(|e| !(e.code == code && e.word == word));
        self.log.push(AdjustEntry {
            op: AdjustOp::Pin,
            code: code.to_string(),
            word: word.to_string(),
            pos: None,
        });
        // 置顶隐含取消删除
        self.removes.remove(&(code.to_string(), word.to_string()));
    }

    pub fn add(&mut self, code: &str, word: &str) {
        self.add_at(code, word, None);
    }

    /// 【右键调频·整序落盘 2026-10-07】把该码当前完整词序写成一整套
    /// {添加}pN（1..n 逐词绝对位），同码旧 {添加} 行全清。此前「移一
    /// 位」只写个别 pN——回放时与陈旧行交错（旧 看了看 p2 行把刚移
    /// 的择顶到末尾=「移一位跳末尾」实锤）；且置顶会清该词 {添加} 行
    /// → added() 去重失效 → 用户词副本二次并入=「重复字+真词消失」。
    /// 整套落盘后：回放序=写入序（升位逐词落槽，无交错），每个词都
    /// 有 {添加} 行（added() 恒真，用户词合并不再重复）。置顶/删除/
    /// 加权行不动（他词语义保留）。
    pub fn set_order(&mut self, code: &str, words: &[String]) {
        self.log
            .retain(|e| !(e.code == code && e.op == AdjustOp::Add));
        // 【置顶归一 2026-10-07】整序集内的词位次全由 pN 管理——同码
        // 的 {置顶} 行一并清（否则内存置顶残留占首选位：加词默认首选
        // 被顶成 2 选；且置顶词对 pN 重排「不动」会吃掉后续调频）。
        // 真固定位（Ctrl+数字钉的）不经本路径，不受影响。
        self.log.retain(|e| {
            !(e.code == code
                && e.op == AdjustOp::Pin
                && words.iter().any(|w| w == &e.word))
        });
        for (i, w) in words.iter().enumerate() {
            self.log.push(AdjustEntry {
                op: AdjustOp::Add,
                code: code.to_string(),
                word: w.clone(),
                pos: Some(i + 1),
            });
            self.removes.remove(&(code.to_string(), w.clone()));
        }
    }

    /// 带选重位加词（/jc 第三框 pN）。
    pub fn add_at(&mut self, code: &str, word: &str, pos: Option<usize>) {
        self.log.retain(|e| !(e.code == code && e.word == word));
        self.log.push(AdjustEntry {
            op: AdjustOp::Add,
            code: code.to_string(),
            word: word.to_string(),
            pos,
        });
        self.removes.remove(&(code.to_string(), word.to_string()));
    }

    pub fn remove(&mut self, code: &str, word: &str) {
        self.log.retain(|e| !(e.code == code && e.word == word));
        self.log.push(AdjustEntry {
            op: AdjustOp::Remove,
            code: code.to_string(),
            word: word.to_string(),
            pos: None,
        });
        self.removes.insert((code.to_string(), word.to_string()));
    }

    /// 该 码→词 是否处于删除态（用户词分支过滤用：adjust.apply 只
    /// 过滤码表 base，用户词在 schema.candidates 单独合并）。
    pub fn removed(&self, code: &str, word: &str) -> bool {
        self.removes.contains(&(code.to_string(), word.to_string()))
    }

    /// 该 码→词 是否有 {添加} 日志（schema 层去重：adjust.apply 已
    /// 就位/追加过，user_dict 同词词行不再二次并入）。
    /// 【置顶同计 2026-10-07】{置顶} 词同样由 apply 就位（置顶块）——
    /// 不计入时 pin 会替换掉 Add 行 → added()=false → user_dict 旧副
    /// 本二次并入=「重复字」（真机快照实锤：front 后 [看了看 看了看
    /// 择]，第二个=用户词副本；合并点的 pinned 查重拦不住它）。
    pub fn added(&self, code: &str, word: &str) -> bool {
        self.log.iter().any(|e| {
            e.code == code
                && e.word == word
                && matches!(e.op, AdjustOp::Add | AdjustOp::Pin)
        })
    }

    /// 全部 {添加} 日志的 (码, 词)（整句词图注入用——pN 词不再入
    /// user_dict，同步管道从这里取）。
    pub fn added_words(&self) -> Vec<(String, String)> {
        self.log
            .iter()
            .filter(|e| e.op == AdjustOp::Add)
            .map(|e| (e.code.clone(), e.word.clone()))
            .collect()
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
            } else if l.contains('\t') && l.split('\t').count() >= 3 {
                let f: Vec<&str> = l.split('\t').map(|s| s.trim()).collect();
                // 【时序回放 2026-11】旧 用户词.txt 的 pN 词行（码\t词
                // [\tweight]\tpN）升格为 {添加} 调整日志（时序回放插位）；
                // 不再走 user_dict 合并路径（那里已无 pN 插位语义）。
                let pn = [f.get(2), f.get(3)]
                    .into_iter()
                    .flatten()
                    .find(|s| s.starts_with('p') && s.len() > 1);
                if let Some(pn) = pn {
                    adj_lines.push(format!("{{添加}}{}\t{}\t{}", f[0], f[1], pn));
                } else {
                    // 旧 TSV 词行（码\t词\tweight）原样词行
                    word_lines.push(l.clone());
                }
            } else {
                // 旧 TSV 词行（码\t词\tweight）原样词行
                word_lines.push(l.clone());
            }
        }
        (word_lines, adj_lines)
    }

    /// 应用到字典候选列表：**时序回放**（2026-11 用户拍板）。逐条按
    /// 日志顺序在演化的列表上执行：
    /// - {置顶}：该词（码表内则原条目标 pinned，自造则新条目）移到最前；
    /// - {添加}pN：以**该时刻列表**度量插第 N 位（超出→末尾）；无 pN
    ///   → **首选**（置顶块后、系统词前——v1 语义；2026-10-06 回归修复，
    ///   重构曾误作末尾追加）；
    /// - {删除}：移除该词——**其后所有词（含先前插入的 pN 用户词）
    ///   统一前移一位**（用户实测：删 2 选后 p5 词变 4 选）；
    /// - {加权}：不动位（权重由 user_dict.weights 消费）。
    /// 同码同词的重复日志行已被写入端去重（append 清旧行），回放端
    /// 对重复行天然幂等（重演同一词）。
    pub fn apply(&self, code: &str, base: &[DictEntry]) -> Vec<DictEntry> {
        let mut out: Vec<DictEntry> = base.to_vec();
        for e in &self.log {
            if e.code != code {
                continue;
            }
            match e.op {
                AdjustOp::Pin => {
                    if let Some(hit) = out.iter().position(|x| x.text == e.word) {
                        let mut entry = out.remove(hit);
                        entry.pinned = true;
                        out.insert(0, entry);
                    } else {
                        let mut entry = DictEntry::new(e.code.clone(), e.word.clone(), u32::MAX);
                        entry.pinned = true;
                        out.insert(0, entry);
                    }
                }
                AdjustOp::Add => {
                    if let Some(n) = e.pos {
                        if let Some(hit) = out.iter().position(|x| x.text == e.word) {
                            // 显式选重位重排：移到第 N 位（1 基；超出→
                            // 末尾）。置顶语义更强，置顶词不动。
                            let entry = out.remove(hit);
                            if !entry.pinned {
                                let idx = (n - 1).min(out.len());
                                out.insert(idx, entry);
                            } else {
                                out.insert(hit.min(out.len()), entry);
                            }
                        } else {
                            let idx = (n - 1).min(out.len());
                            out.insert(
                                idx,
                                DictEntry::new(e.code.clone(), e.word.clone(), u32::MAX - 1),
                            );
                        }
                    } else {
                        // 【留空=首选 2026-10-06 回归修复】无 pN 的 {添加}
                        //（/jc 第三框留空）恢复 v1 语义：插到置顶块之后、
                        // 系统词之前（无置顶词时即首选）。时序回放重构误改
                        // 成「末尾追加」＝加词窗「留空=首选」文案的反面
                        //（用户实测「默认最后」回归）。已在列表（含码表
                        // 同词）：移出重插到该位（顶替/提频）；置顶同词
                        // 不动（pinned 语义更强，且保持同码 text 唯一）。
                        // 不置 pinned——后续显式 pN / Ctrl+数字仍可再排。
                        match out.iter().position(|x| x.text == e.word) {
                            Some(hit) => {
                                if !out[hit].pinned {
                                    out.remove(hit);
                                    let pos =
                                        out.iter().position(|x| !x.pinned).unwrap_or(out.len());
                                    out.insert(
                                        pos,
                                        DictEntry::new(
                                            e.code.clone(),
                                            e.word.clone(),
                                            u32::MAX - 1,
                                        ),
                                    );
                                }
                            }
                            None => {
                                let pos =
                                    out.iter().position(|x| !x.pinned).unwrap_or(out.len());
                                out.insert(
                                    pos,
                                    DictEntry::new(e.code.clone(), e.word.clone(), u32::MAX - 1),
                                );
                            }
                        }
                    }
                }
                AdjustOp::Remove => {
                    out.retain(|x| x.text != e.word);
                }
                AdjustOp::Weight => {
                    // 权重经 user_dict.weights 消费（merge_into），位次不动
                }
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
        // 【留空=首选 2026-10-06】无 pN {添加} 插到置顶块之后、系统词
        // 之前（原断言「末尾追加」=回归期行为，已废止）。
        assert_eq!(texts, ["叉", "哎呦", "来", "氨"]);
    }

    // 【时序回放 2026-11 用户拍板】用户实测 ae 序列：{添加}p5 → {置顶}
    // → {删除}2选词 → {添加}p6。期望：删除后 p5 词前移成 4 选，p6 词
    // 在第 6 位（原实现删除先全做完、pN 绝对位插入 → p5 钉死 5 选）。
    #[test]
    fn sequential_replay_delete_shifts_placed_words() {
        let lines: Vec<String> = vec![
            "{添加}ae\t测试\tp5".into(),
            "{置顶}ae\t乛".into(),
            "{删除}ae\t那样".into(),
            "{添加}ae\tipad mini\tp6".into(),
        ];
        // 基表：闲 那样 乛 の 爱 安 按 岸 暗
        let base: Vec<DictEntry> = ["闲", "那样", "乛", "の", "爱", "安", "按", "岸", "暗"]
            .iter()
            .enumerate()
            .map(|(i, t)| DictEntry::new("ae", *t, i as u32))
            .collect();
        let adj = UserAdjust::parse(&lines);
        let out = adj.apply("ae", &base);
        let texts: Vec<&str> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "乛", "闲", "の", "测试", "爱", "ipad mini", "安", "按", "岸", "暗"
            ],
            "时序回放：删 2 选后 p5 前移 4 选: {texts:?}"
        );

        // 无删除干扰的纯选重位：p5 恒第 5 位
        let lines2: Vec<String> = vec!["{添加}ae\t测试\tp5".into()];
        let adj2 = UserAdjust::parse(&lines2);
        let out2 = adj2.apply("ae", &base);
        let texts2: Vec<&str> = out2.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts2,
            ["闲", "那样", "乛", "の", "测试", "爱", "安", "按", "岸", "暗"],
            "纯 p5 位次: {texts2:?}"
        );
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
            // 【2026-11 修复】混合分隔（TAB+全角空格）按 TAB 分割：
            // 码 a、词「氨」（全角空格在 TAB 列内——第三列日期丢弃）
            "{删除}a\t氨\t2026/09/05".into(),
            // 裸日志（原生 TAB）不受影响
            "{置顶}ab\t你好".into(),
            // 【词含空格 2026-11】TAB 行词列的空格必须保留（ipad mini）
            "{添加}ae\tipad mini\tp6".into(),
        ];
        let adj = UserAdjust::parse(&lines);
        let out = adj.apply("a", &base());
        let texts: Vec<&str> = out.iter().map(|e| e.text.as_str()).collect();
        // 氨被删；哎呦留空添加=置顶块（叉）后首选（2026-10-06 回归修复）
        assert_eq!(texts, ["叉", "哎呦", "来"]);
        let out2 = adj.apply("ab", &[]);
        assert_eq!(out2[0].text, "你好");
        let out3 = adj.apply("ae", &[]);
        assert_eq!(out3[0].text, "ipad mini", "TAB 行词含空格完整保留");
    }
}
