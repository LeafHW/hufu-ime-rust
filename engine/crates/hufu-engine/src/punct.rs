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
