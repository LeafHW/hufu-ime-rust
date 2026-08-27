//! 动态变量：日期 / 时间 / 星期 / 数字转中文（无外部依赖）。

/// Unix 秒 → 本地（UTC+8）民用时间。
fn now_civil() -> (i64, u32, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = secs + 8 * 3600;
    let days = local.div_euclid(86400);
    let sod = local.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 是星期四（0=星期日）
    let wd = (days + 4).rem_euclid(7) as u32;
    (
        y,
        m,
        d,
        (sod / 3600) as u32,
        ((sod % 3600) / 60) as u32,
        (sod % 60) as u32,
        wd,
    )
}

/// Howard Hinnant civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn date_string() -> String {
    let (y, m, d, _, _, _, _) = now_civil();
    format!("{y}年{m:02}月{d:02}日")
}

pub fn date_string_iso() -> String {
    let (y, m, d, _, _, _, _) = now_civil();
    format!("{y}-{m:02}-{d:02}")
}

pub fn time_string() -> String {
    let (_, _, _, h, mi, s, _) = now_civil();
    format!("{h:02}:{mi:02}:{s:02}")
}

pub fn time_short() -> String {
    let (_, _, _, h, mi, _, _) = now_civil();
    format!("{h:02}:{mi:02}")
}

pub fn week_string() -> String {
    let (_, _, _, _, _, _, wd) = now_civil();
    let names = ["日", "一", "二", "三", "四", "五", "六"];
    format!("星期{}", names[wd as usize])
}

const DIGITS_LOWER: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
const DIGITS_UPPER: [char; 10] = ['零', '壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];
const UNITS_LOWER: [&str; 4] = ["", "万", "亿", "兆"];
const UNITS_UPPER: [&str; 4] = ["", "萬", "億", "兆"];

/// 数字串 → 中文（小写：一千二百三十四；upper=true 大写金额数字：壹仟贰佰叁拾肆）。
/// 非法输入返回 None。
pub fn number_to_chinese(s: &str, upper: bool) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 16 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if s == "0" || s.chars().all(|c| c == '0') {
        return Some(DIGITS_LOWER[0].to_string());
    }
    let digs: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();

    // 4 位一组
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let first_nz = digs.iter().position(|d| *d != 0)?;
    let mut rest = &digs[first_nz..];
    while !rest.is_empty() {
        let take = rest.len().min(4);
        groups.insert(0, rest[rest.len() - take..].to_vec());
        rest = &rest[..rest.len() - take];
    }

    let digs_of = |d: u32, upper: bool| -> char {
        if upper {
            DIGITS_UPPER[d as usize]
        } else {
            DIGITS_LOWER[d as usize]
        }
    };

    let mut out = String::new();
    let n_groups = groups.len();
    for (gi, g) in groups.iter().enumerate() {
        let unit_idx = n_groups - 1 - gi;
        let mut section = String::new();
        let len = g.len();
        let mut last_was_zero = false;
        let mut section_zero = true;
        for (i, d) in g.iter().enumerate() {
            let place = len - 1 - i; // 3=千 2=百 1=十 0=个
            if *d == 0 {
                last_was_zero = true;
            } else {
                section_zero = false;
                if last_was_zero && !section.is_empty() {
                    section.push(DIGITS_LOWER[0]);
                }
                last_was_zero = false;
                section.push(digs_of(*d, upper));
                let place_name = match place {
                    3 => "千",
                    2 => "百",
                    1 => "十",
                    _ => "",
                };
                let place_name = if upper {
                    match place {
                        3 => "仟",
                        2 => "佰",
                        1 => "拾",
                        _ => "",
                    }
                } else {
                    place_name
                };
                section.push_str(place_name);
            }
        }
        if !section_zero {
            // 跨组零：组内最高位（千位）为 0 且已有输出 → 补零（一万零一 / 一万零五百）
            if !out.is_empty() && g[0] == 0 && !out.ends_with(DIGITS_LOWER[0]) {
                out.push(DIGITS_LOWER[0]);
            }
            out.push_str(&section);
            let u = if upper {
                UNITS_UPPER[unit_idx]
            } else {
                UNITS_LOWER[unit_idx]
            };
            out.push_str(u);
        } else if !out.is_empty() {
            // 全零组：仅当后续还有非零组时补一个零（一亿零一）
            let later_nonzero = groups[gi + 1..].iter().any(|x| x.iter().any(|d| *d != 0));
            if later_nonzero && !out.ends_with(DIGITS_LOWER[0]) {
                out.push(DIGITS_LOWER[0]);
            }
        }
    }
    // 「一十」开头简化为「十」（仅小写）
    if !upper && out.starts_with("一十") {
        out = out[3..].to_string();
    }
    if out.is_empty() {
        return Some(DIGITS_LOWER[0].to_string());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
    }

    #[test]
    fn numbers() {
        assert_eq!(number_to_chinese("0", false).unwrap(), "零");
        assert_eq!(number_to_chinese("7", false).unwrap(), "七");
        assert_eq!(number_to_chinese("10", false).unwrap(), "十");
        assert_eq!(number_to_chinese("15", false).unwrap(), "十五");
        assert_eq!(number_to_chinese("100", false).unwrap(), "一百");
        assert_eq!(number_to_chinese("105", false).unwrap(), "一百零五");
        assert_eq!(number_to_chinese("1234", false).unwrap(), "一千二百三十四");
        assert_eq!(
            number_to_chinese("10000", false).unwrap(),
            "一万"
        );
        assert_eq!(
            number_to_chinese("10001", false).unwrap(),
            "一万零一"
        );
        assert_eq!(
            number_to_chinese("123456789", false).unwrap(),
            "一亿二千三百四十五万六千七百八十九"
        );
        assert_eq!(
            number_to_chinese("100000001", false).unwrap(),
            "一亿零一"
        );
        assert_eq!(
            number_to_chinese("10500", false).unwrap(),
            "一万零五百"
        );
        assert_eq!(number_to_chinese("1234", true).unwrap(), "壹仟贰佰叁拾肆");
        assert!(number_to_chinese("12a3", false).is_none());
        assert!(number_to_chinese("", false).is_none());
    }

    #[test]
    fn date_shapes() {
        let d = date_string();
        assert!(d.contains('年') && d.contains('月') && d.contains('日'));
        assert!(week_string().starts_with("星期"));
        assert!(time_string().len() == 8 && time_string().contains(':'));
    }
}
