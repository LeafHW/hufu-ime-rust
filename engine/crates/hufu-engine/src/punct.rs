//! 标点与全半角映射。

/// 半角标点 → 全角（可能映射到多字符，如 ^ → ……）
///
/// 【对齐虎爪 2026-09-06】tiger_sentence.symbols.yaml 的 half_shape
/// （中文态默认档）：常用中文标点全角；码农/网络常用符号（- + = @ #
/// % & * | ~ \` { }）保持半角——映射为自身（而非删除），编码态标点
/// 顶字（候选+符号）语义得以保留且与虎爪一致。英文态一律半角直通
/// （on_char 早退，不经此表）。
pub fn to_full_width_punct(c: char) -> Option<String> {
    let s: String = match c {
        ',' => "，".into(),
        '.' => "。".into(),
        '?' => "？".into(),
        '!' => "！".into(),
        ':' => "：".into(),
        ';' => "；".into(),
        '(' => "（".into(),
        ')' => "）".into(),
        '[' => "【".into(),
        ']' => "】".into(),
        '<' => "《".into(),
        '>' => "》".into(),
        '\\' => "、".into(),
        '$' => "￥".into(),
        '^' => "……".into(),
        '_' => "——".into(),
        // 虎爪 half_shape 半角组（值=自身）
        '{' => "{".into(),
        '}' => "}".into(),
        '|' => "|".into(),
        '~' => "~".into(),
        '`' => "`".into(),
        '@' => "@".into(),
        '#' => "#".into(),
        '%' => "%".into(),
        '&' => "&".into(),
        '*' => "*".into(),
        '-' => "-".into(),
        '+' => "+".into(),
        '=' => "=".into(),
        _ => return None,
    };
    Some(s)
}

/// 成对引号状态：单双引号交替输出左右引号。
#[derive(Debug, Default, Clone)]
pub struct PairState {
    single_open: bool,
    double_open: bool,
}

impl PairState {
    pub fn quote(&mut self, c: char) -> Option<char> {
        match c {
            '\'' => {
                let out = if self.single_open { '’' } else { '‘' };
                self.single_open = !self.single_open;
                Some(out)
            }
            '"' => {
                let out = if self.double_open { '”' } else { '“' };
                self.double_open = !self.double_open;
                Some(out)
            }
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.single_open = false;
        self.double_open = false;
    }
}
