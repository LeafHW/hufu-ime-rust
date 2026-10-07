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
        // 【2026-09-06 用户规格】\ 取消命令模式与顿号映射——原样录入自身
        '\\' => "\\".into(),
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
        // 【/无引导直出 2026-11】/ 无引导（slash_dunhao 关且码表无 / 前缀
        // 词条）时空态按 / 原先落到 passthrough——DLL TestDown 对单字符
        // 预吞 TRUE，信任 TestDown 的宿主（WPS/Word 等 CUAS）不再自产
        // WM_CHAR → 键蒸发「按了没反应」（用户实锤；与八十四修大写
        // 字母蒸发同病根）。入表自映=consumed+commit 走 TSF 插入通道，
        // 全宿主统一；编码态与 { } | ~ 同语义（有候选顶字+符号）。
        // 有引导（has_continuation_prefix）与 slash_dunhao 档在各自
        // 早退分支，不受此行影响。
        '/' => "/".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 【/无引导直出 2026-11 回归】/ 必须在 half_shape 表内自映——
    /// 空态中文按 / 走 consumed+commit（TSF 插入通道），否则信任
    /// TestDown 的宿主键蒸发（用户实锤「按了没反应」）。
    #[test]
    fn slash_maps_to_self() {
        assert_eq!(to_full_width_punct('/').as_deref(), Some("/"));
        // 半角保持组仍在（编码态顶字语义依赖）
        assert_eq!(to_full_width_punct('=').as_deref(), Some("="));
        assert_eq!(to_full_width_punct('{').as_deref(), Some("{"));
        // 未入表字符仍 None（引擎回落其他分支）
        assert_eq!(to_full_width_punct('a'), None);
        assert_eq!(to_full_width_punct('、'), None);
    }
}
