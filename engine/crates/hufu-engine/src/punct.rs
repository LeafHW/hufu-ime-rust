//! 标点与全半角映射。

/// 半角标点 → 全角（可能映射到多字符，如 ^ → ……）
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
        '{' => '｛'.into(),
        '}' => '｝'.into(),
        '<' => "《".into(),
        '>' => "》".into(),
        '\\' => "、".into(),
        '~' => "～".into(),
        '@' => "＠".into(),
        '#' => "＃".into(),
        '$' => "￥".into(),
        '%' => "％".into(),
        '&' => "＆".into(),
        '*' => "＊".into(),
        '^' => "……".into(),
        '-' => "-".into(),
        // 【符号自查 2026-09-06】Shift 形态全角补全：|（Shift+\ 编码态
        // 此前映射缺失一路 fallthrough 直通乱象）、＋／＝、_（Shift+-
        // 空态；编码态已被翻页/顶字复用键拦为 ——）
        '|' => "｜".into(),
        '+' => "＋".into(),
        '=' => "＝".into(),
        '_' => "——".into(),
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
