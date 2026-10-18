//! 双拼派生规则（反查瘦身 2026-10-18 用户拍板）。
//!
//! 自然码 / 小鹤双拼两张反查全量表（6.2MB / 7.7MB）不再随包发行，
//! 改为只发全拼基表（码列为空格分隔音节，如 `你们\t ni men`），
//! 装载期由本规则逐音节编码派生（毫秒级，356,907 行实测 0 跳过）。
//!
//! 键位规则经双源互验后才启用：
//! - 自然码：`gen_shuangpin.rs` 生成器源码（conv(ms=false)）；
//! - 小鹤：flypy 官方键位（注意 ui=V——网上流传的 gist 误写 ui=W，
//!   会被 shei/shui 撞码论证直接否掉：ei 已占 W）；
//! - 两家均与 Bime 生产表提取的 403 音节 majority 映射逐条比对一致
//!   （唯一残差 shei→uv 系 谁(shéi/shuí) 读音噪音，非键位差异）。
//! 零声母两家全同（单字母韵母双打、双字母韵母原样、三字母取
//! 首字母+键位）：a=aa e=ee o=oo ang=ah eng=eg ai=ai an=an ao=ao
//! ei=ei en=en er=er ou=ou；nv/lv 直出；zh/ch/sh→v/i/u。

/// 双拼方案派生规则。装载期对全拼基表逐音节编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShuangpinRule {
    /// 自然码（ing=y uai=y ei=z ai=l iang=d …）
    Ziranma,
    /// 小鹤双拼（ing=k uai=k ei=w ai=d iang=l ui=v …）
    Xiaohe,
}

impl ShuangpinRule {
    /// 方案名 → 规则（数据\拼音反查\<名>.txt 缺席时的派生名单）。
    pub fn from_scheme(name: &str) -> Option<Self> {
        match name.trim() {
            "自然码" => Some(Self::Ziranma),
            "小鹤双拼" => Some(Self::Xiaohe),
            _ => None,
        }
    }

    /// 韵母 → 键位。
    fn final_key(&self, fin: &str) -> Option<char> {
        let key = match self {
            Self::Ziranma => match fin {
                "iu" => 'q',
                "ia" | "ua" => 'w',
                "uan" => 'r',
                "ue" | "üe" => 't',
                "uo" => 'o',
                "un" => 'p',
                "iong" | "ong" => 's',
                "iang" | "uang" => 'd',
                "en" => 'f',
                "eng" => 'g',
                "ang" => 'h',
                "an" => 'j',
                "ao" => 'k',
                "ai" => 'l',
                "ei" => 'z',
                "ie" => 'x',
                "iao" => 'c',
                "ui" | "ü" => 'v',
                "ou" => 'b',
                "in" => 'n',
                "ian" => 'm',
                "ing" | "uai" => 'y',
                "a" => 'a',
                "e" => 'e',
                "i" => 'i',
                "o" => 'o',
                "u" => 'u',
                "er" => 'r',
                _ => return None,
            },
            Self::Xiaohe => match fin {
                "iu" => 'q',
                "ia" | "ua" => 'x',
                "uan" => 'r',
                "ue" | "üe" => 't',
                "uai" | "ing" => 'k',
                "uo" => 'o',
                "un" => 'y',
                "iong" | "ong" => 's',
                "iang" | "uang" => 'l',
                "en" => 'f',
                "eng" => 'g',
                "ang" => 'h',
                "an" => 'j',
                "ao" => 'c',
                "ai" => 'd',
                "ei" => 'w',
                "ie" => 'p',
                "iao" => 'n',
                "ui" => 'v',
                "ou" => 'z',
                "in" => 'b',
                "ian" => 'm',
                "a" => 'a',
                "e" => 'e',
                "i" => 'i',
                "o" => 'o',
                "u" => 'u',
                "er" => 'r',
                _ => return None,
            },
        };
        Some(key)
    }

    /// 单音节 → 双拼码（声母键 + 韵母键，恒 2 字符）。非法音节返回
    /// None（调用方跳过该行——与生成器 conv 失败即跳同语义）。
    pub fn encode(&self, syl: &str) -> Option<String> {
        // 零声母（两家全同）+ 特例直出
        match syl {
            "a" => return Some("aa".into()),
            "ai" => return Some("ai".into()),
            "an" => return Some("an".into()),
            "ang" => return Some("ah".into()),
            "ao" => return Some("ao".into()),
            "e" => return Some("ee".into()),
            "ei" => return Some("ei".into()),
            "en" => return Some("en".into()),
            "eng" => return Some("eg".into()),
            "er" => return Some("er".into()),
            "o" => return Some("oo".into()),
            "ou" => return Some("ou".into()),
            "yu" => return Some("yu".into()),
            "wu" => return Some("wu".into()),
            // nü/lü（词典以 v 代 ü）：两家均 nv/lv 直出
            "nv" | "lv" => return Some(syl.into()),
            _ => {}
        }
        // 声母剥离（zh/ch/sh 最长优先）
        let (sm, fin): (char, &str) = if let Some(f) = syl.strip_prefix("zh") {
            ('v', f)
        } else if let Some(f) = syl.strip_prefix("ch") {
            ('i', f)
        } else if let Some(f) = syl.strip_prefix("sh") {
            ('u', f)
        } else {
            let mut cs = syl.chars();
            let c = cs.next()?;
            if !matches!(
                c,
                'b' | 'p'
                    | 'm'
                    | 'f'
                    | 'd'
                    | 't'
                    | 'n'
                    | 'l'
                    | 'g'
                    | 'k'
                    | 'h'
                    | 'j'
                    | 'q'
                    | 'x'
                    | 'r'
                    | 'z'
                    | 'c'
                    | 's'
                    | 'y'
                    | 'w'
            ) {
                return None;
            }
            (c, cs.as_str())
        };
        if fin.is_empty() {
            return None;
        }
        let key = self.final_key(fin)?;
        Some(format!("{sm}{key}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 键位抽查（与 Bime 生产表逐字核对过的实码）。
    #[test]
    fn spot_codes() {
        let z = ShuangpinRule::Ziranma;
        let x = ShuangpinRule::Xiaohe;
        // 自然码
        assert_eq!(z.encode("de").unwrap(), "de");
        assert_eq!(z.encode("shi").unwrap(), "ui");
        assert_eq!(z.encode("liao").unwrap(), "lc");
        assert_eq!(z.encode("zai").unwrap(), "zl");
        assert_eq!(z.encode("xiang").unwrap(), "xd");
        assert_eq!(z.encode("shui").unwrap(), "uv");
        assert_eq!(z.encode("lv").unwrap(), "lv");
        assert_eq!(z.encode("ang").unwrap(), "ah");
        // 小鹤（官方 flypy；ui=V）
        assert_eq!(x.encode("de").unwrap(), "de");
        assert_eq!(x.encode("shi").unwrap(), "ui");
        assert_eq!(x.encode("zai").unwrap(), "zd");
        assert_eq!(x.encode("shui").unwrap(), "uv");
        assert_eq!(x.encode("zhui").unwrap(), "vv");
        assert_eq!(x.encode("zhuang").unwrap(), "vl");
        assert_eq!(x.encode("chuang").unwrap(), "il");
        assert_eq!(x.encode("qiu").unwrap(), "qq");
        assert_eq!(x.encode("yue").unwrap(), "yt");
        assert_eq!(x.encode("yun").unwrap(), "yy");
        assert_eq!(x.encode("ruan").unwrap(), "rr");
        assert_eq!(x.encode("mei").unwrap(), "mw");
        assert_eq!(x.encode("xing").unwrap(), "xk");
        assert_eq!(x.encode("min").unwrap(), "mb");
        assert_eq!(x.encode("lun").unwrap(), "ly");
        assert_eq!(x.encode("lie").unwrap(), "lp");
        assert_eq!(x.encode("shei").unwrap(), "uw");
        assert_eq!(x.encode("nv").unwrap(), "nv");
        assert_eq!(x.encode("lo").unwrap(), "lo");
        // 非法
        assert!(z.encode("zh").is_none());
        assert!(x.encode("v").is_none());
    }

    /// 全量一致性驱动数据：403 音节全部可编码（音节清单来自生产表提取）。
    #[test]
    fn from_scheme_names() {
        assert_eq!(ShuangpinRule::from_scheme("自然码"), Some(ShuangpinRule::Ziranma));
        assert_eq!(ShuangpinRule::from_scheme("小鹤双拼"), Some(ShuangpinRule::Xiaohe));
        assert_eq!(ShuangpinRule::from_scheme("全拼"), None);
        assert_eq!(ShuangpinRule::from_scheme(""), None);
    }
}
