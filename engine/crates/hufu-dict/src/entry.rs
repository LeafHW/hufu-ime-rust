//! 字典条目与排序。

/// 一条码表条目。
#[derive(Debug, Clone, PartialEq)]
pub struct DictEntry {
    /// 编码
    pub code: String,
    /// 词条
    pub text: String,
    /// 权重（无权重格式为 0，排序时回退到 `seq` 保持原序）
    pub weight: f64,
    /// Rime stem（出简让全标记）
    pub stem: Option<String>,
    /// 多多 `#固`：固定置顶
    pub pinned: bool,
    /// 多多 `显示=>输出`：上屏文本与显示不同
    pub commit_override: Option<String>,
    /// 原始行号（稳定性 tiebreaker）
    pub seq: u32,
}

impl DictEntry {
    pub fn new(code: impl Into<String>, text: impl Into<String>, seq: u32) -> Self {
        DictEntry {
            code: code.into(),
            text: text.into(),
            weight: 0.0,
            stem: None,
            pinned: false,
            commit_override: None,
            seq,
        }
    }
}

/// 排序比较：权重降序，同权重按原始顺序。
pub fn rank_cmp(a: &DictEntry, b: &DictEntry) -> std::cmp::Ordering {
    b.weight
        .partial_cmp(&a.weight)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.seq.cmp(&b.seq))
}
