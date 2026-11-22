//! 拼音/双拼编码与匹配，供 LSP 的“打拼音补中文”使用。
//!
//! 设计：先借助 `pinyin` crate 把每个汉字转为无声调全拼音节，再在其之上
//! 计算三种编码：
//! - 全拼（full）：各字音节直接拼接，如 `打招呼 → dazhaohu`；
//! - 首字母（initial）：各字音节首字母拼接，如 `打招呼 → dzh`；
//! - 双拼（double）：每个音节拆成声母+韵母，按选定方案各映射到一个键，
//!   每字固定两键。键位表严格对齐 Rime 官方双拼方案（自然码/小鹤）的
//!   `speller/algebra`，含零声母（a/o/e 开头）与 `j/q/x/y + u` 的规则。
//!
//! ## 后续演进：外挂词库与自由中文输入
//!
//! 本期只对“语言关键字 + 项目符号”这些有限、可枚举的词做内存匹配，不打包
//! 通用大词库。若将来需要注释/字符串里的自由整句输入，推荐演进路线是接入
//! [librime](https://github.com/rime/librime)（Rime 引擎，可内嵌的跨平台
//! 纯引擎库，不依赖系统 IME 守护进程），以 sidecar 或 FFI 形式提供整句组字，
//! 并通过下方 [`PinyinOptions::external_words`] 的外挂词库入口接入；不建议直接
//! 调用各平台系统输入法接口（IBus/Fcitx/IMK/TSF），因为 LSP 是无窗口的 stdio
//! 子进程，拿不到编辑器的输入上下文。

use pinyin::ToPinyin;

/// 双拼键位方案。键位对齐 Rime `rime-double-pinyin` 仓库的同名方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DoublePinyinScheme {
    /// 自然码双拼（Rime `double_pinyin`），作为默认方案。
    #[default]
    Ziranma,
    /// 小鹤双拼（Rime `double_pinyin_flypy`）。
    Xiaohe,
}

impl DoublePinyinScheme {
    /// 从配置字符串解析方案，无法识别时回退到自然码。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "xiaohe" | "flypy" | "小鹤" => Self::Xiaohe,
            _ => Self::Ziranma,
        }
    }

    /// 零声母音节（a/o/e 开头）的双拼两键，对齐 Rime `derive/^([aoe])([ioun])$/`
    /// 与 `xform/^([aoe])(ng)?$/`：首字母双写 + 韵母键。
    ///
    /// 韵母键直接对**完整音节**查表（如 `an→j`、`en→f`）；仅当完整音节不是
    /// 登记韵母（单韵母 `a/o/e` 或 `ng` 收尾的特殊形）时，才退回首字母本身。
    fn zero_initial(&self, syllable: &str) -> Option<String> {
        let first = syllable.chars().next()?;
        let final_key = self.final_key(syllable).unwrap_or(first);
        Some(format!("{first}{final_key}"))
    }

    /// 韵母（含零声母首字双写后留下的 `ng`/单韵母等）到键的映射。
    fn final_key(&self, fin: &str) -> Option<char> {
        let key = match self {
            // 自然码：Rime double_pinyin.schema.yaml 的 algebra。
            DoublePinyinScheme::Ziranma => match fin {
                "iu" => 'q',
                "ia" | "ua" => 'w',
                "uan" | "van" => 'r',
                "ue" | "ve" => 't',
                "ing" | "uai" => 'y',
                "uo" => 'o',
                "un" | "vn" => 'p',
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
                "ui" => 'v',
                "ou" => 'b',
                "in" => 'n',
                "ian" => 'm',
                // 零声母的单韵母与 ng 收尾特殊形，键取其首字母。
                "a" | "o" | "e" | "i" | "u" | "ng" => fin.chars().next()?,
                _ => return None,
            },
            // 小鹤：Rime double_pinyin_flypy.schema.yaml 的 algebra。
            DoublePinyinScheme::Xiaohe => match fin {
                "iu" => 'q',
                "ei" => 'w',
                "uan" => 'r',
                "ue" | "ve" => 't',
                "un" => 'y',
                "uo" => 'o',
                "ie" => 'p',
                "iong" | "ong" => 's',
                "ai" => 'd',
                "en" => 'f',
                "eng" => 'g',
                "ang" => 'h',
                "an" => 'j',
                "ing" | "uai" => 'k',
                "iang" | "uang" => 'l',
                "ou" => 'z',
                "ia" | "ua" => 'x',
                "iao" => 'n',
                "ao" => 'c',
                "ui" => 'v',
                "in" => 'b',
                "ian" => 'm',
                // 零声母的单韵母与 ng 收尾特殊形，键取其首字母。
                "a" | "o" | "e" | "i" | "u" | "ng" => fin.chars().next()?,
                _ => return None,
            },
        };
        Some(key)
    }

    /// 声母到键的映射。`zh/ch/sh` 在两套方案里同为 v/i/u，其余声母键即其首字母。
    fn initial_key(init: &str) -> Option<char> {
        match init {
            "zh" => Some('v'),
            "ch" => Some('i'),
            "sh" => Some('u'),
            // b p m f d t n l g k h j q x r z c s y w 都用自身首字母。
            other => other.chars().next(),
        }
    }
}

/// 启用哪些输入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSchemes {
    /// 全拼前缀匹配，如 `dazhaohu`。
    pub full: bool,
    /// 首字母前缀匹配，如 `dzh`。
    pub initial: bool,
    /// 双拼前缀匹配。
    pub double: bool,
}

impl Default for InputSchemes {
    fn default() -> Self {
        // 默认三种模式全开，并行前缀匹配，用户无需切换输入法状态。
        Self {
            full: true,
            initial: true,
            double: true,
        }
    }
}

/// 拼音补全的运行配置（由 LSP `initializationOptions` 映射而来）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PinyinOptions {
    /// 是否启用拼音补全；关闭后完全走原有符号补全。
    pub enabled: bool,
    /// 候选上限。
    pub max_suggest: usize,
    /// 启用的输入模式。
    pub schemes: InputSchemes,
    /// 双拼键位方案。
    pub double_scheme: DoublePinyinScheme,
    /// 外挂词库词条（预留：本期只支持内存注入，后续可由词库文件/librime 填充）。
    pub external_words: Vec<String>,
}

impl PinyinOptions {
    /// 一套合理的默认配置：启用、上限 50、三模式全开、自然码。
    pub fn sensible_default() -> Self {
        Self {
            enabled: true,
            max_suggest: 50,
            schemes: InputSchemes::default(),
            double_scheme: DoublePinyinScheme::default(),
            external_words: Vec::new(),
        }
    }
}

/// 一个中文词预计算出的全部编码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinyinCodes {
    /// 全拼串，如 `dazhaohu`。
    pub full: String,
    /// 首字母串，如 `dzh`。
    pub initial: String,
    /// 双拼串（按当前方案），每字两键拼接。
    pub double: String,
}

/// 把一个汉字音节拆为 `(声母, 韵母)`；零声母字返回 `("", 整个音节)`。
/// 声母按最长匹配（zh/ch/sh 优先）。
pub fn split_syllable(syllable: &str) -> (&str, &str) {
    const INITIALS: &[&str] = &[
        "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x",
        "r", "z", "c", "s", "y", "w",
    ];
    for init in INITIALS {
        if let Some(rest) = syllable.strip_prefix(init) {
            return (init, rest);
        }
    }
    ("", syllable)
}

/// 单个无声调音节按方案转双拼两键。
fn syllable_to_double(syllable: &str, scheme: DoublePinyinScheme) -> Option<String> {
    // 零声母（a/o/e 开头）走首字母双写规则。
    if matches!(syllable.chars().next(), Some('a' | 'o' | 'e')) {
        return scheme.zero_initial(syllable);
    }
    let (init, fin) = split_syllable(syllable);
    let init_key = DoublePinyinScheme::initial_key(init)?;
    let fin_key = scheme.final_key(fin)?;
    Some(format!("{init_key}{fin_key}"))
}

/// 计算一个中文词（可含非汉字字符）的全拼、首字母与双拼编码。
///
/// 非汉字字符（ASCII 字母/数字等）原样保留并参与全拼，不计入双拼/首字母；
/// 多音字取 `pinyin` crate 的默认读音。
pub fn codes_of(word: &str, scheme: DoublePinyinScheme) -> PinyinCodes {
    let mut full = String::new();
    let mut initial = String::new();
    let mut double = String::new();
    for (ch, py) in word.chars().zip(word.to_pinyin()) {
        match py {
            Some(p) => {
                let syllable = p.plain();
                full.push_str(syllable);
                if let Some(first) = syllable.chars().next() {
                    initial.push(first);
                }
                if let Some(two) = syllable_to_double(syllable, scheme) {
                    double.push_str(&two);
                }
            }
            None => {
                // 非汉字：全拼原样保留，便于英文/混合标识符匹配。
                full.push(ch);
            }
        }
    }
    PinyinCodes {
        full,
        initial,
        double,
    }
}

/// 判断输入的拉丁前缀是否命中某组编码（按启用的模式做前缀匹配）。
pub fn matches_input(input: &str, codes: &PinyinCodes, schemes: InputSchemes) -> bool {
    if input.is_empty() {
        return false;
    }
    let lower = input.to_ascii_lowercase();
    (schemes.full && codes.full.starts_with(&lower))
        || (schemes.initial && codes.initial.starts_with(&lower))
        || (schemes.double && codes.double.starts_with(&lower))
}

/// 判断字符串是否为可触发拼音补全的拉丁前缀（纯 ASCII 字母，至少一个）。
pub fn is_latin_prefix(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 全拼与首字母() {
        let c = codes_of("打招呼", DoublePinyinScheme::Ziranma);
        assert_eq!(c.full, "dazhaohu");
        assert_eq!(c.initial, "dzh");
    }

    #[test]
    fn 音节拆分() {
        assert_eq!(split_syllable("shuang"), ("sh", "uang"));
        assert_eq!(split_syllable("an"), ("", "an"));
        assert_eq!(split_syllable("da"), ("d", "a"));
    }

    #[test]
    fn 自然码零声母() {
        assert_eq!(
            syllable_to_double("an", DoublePinyinScheme::Ziranma),
            Some("aj".to_string())
        );
        assert_eq!(
            syllable_to_double("en", DoublePinyinScheme::Ziranma),
            Some("ef".to_string())
        );
        assert_eq!(
            syllable_to_double("ang", DoublePinyinScheme::Ziranma),
            Some("ah".to_string())
        );
    }

    #[test]
    fn 小鹤零声母() {
        assert_eq!(
            syllable_to_double("an", DoublePinyinScheme::Xiaohe),
            Some("aj".to_string())
        );
        assert_eq!(
            syllable_to_double("en", DoublePinyinScheme::Xiaohe),
            Some("ef".to_string())
        );
    }

    #[test]
    fn 自然码普通字双拼() {
        assert_eq!(
            syllable_to_double("da", DoublePinyinScheme::Ziranma),
            Some("da".to_string())
        );
        // 双 shuang：sh→u，uang→d。
        assert_eq!(
            syllable_to_double("shuang", DoublePinyinScheme::Ziranma),
            Some("ud".to_string())
        );
    }

    #[test]
    fn 小鹤普通字双拼() {
        // 双 shuang：sh→u，uang→l。
        assert_eq!(
            syllable_to_double("shuang", DoublePinyinScheme::Xiaohe),
            Some("ul".to_string())
        );
    }

    #[test]
    fn 前缀匹配三种模式() {
        let c = codes_of("打招呼", DoublePinyinScheme::Ziranma);
        let all = InputSchemes::default();
        assert!(matches_input("dazhao", &c, all));
        assert!(matches_input("dzh", &c, all));
        // 双拼半程也能前缀命中。
        assert!(matches_input(&c.double[..2], &c, all));
        assert!(!matches_input("xyz", &c, all));
        assert!(!matches_input("", &c, all));
    }

    #[test]
    fn 模式开关生效() {
        let c = codes_of("函数", DoublePinyinScheme::Ziranma);
        let only_initial = InputSchemes {
            full: false,
            initial: true,
            double: false,
        };
        assert!(matches_input("hs", &c, only_initial));
        assert!(!matches_input("hanshu", &c, only_initial));
    }

    #[test]
    fn 拉丁前缀判定() {
        assert!(is_latin_prefix("dzh"));
        assert!(!is_latin_prefix("打"));
        assert!(!is_latin_prefix("a1"));
        assert!(!is_latin_prefix(""));
    }

    #[test]
    fn 方案解析回退() {
        assert_eq!(
            DoublePinyinScheme::parse("xiaohe"),
            DoublePinyinScheme::Xiaohe
        );
        assert_eq!(
            DoublePinyinScheme::parse("小鹤"),
            DoublePinyinScheme::Xiaohe
        );
        assert_eq!(
            DoublePinyinScheme::parse("anything"),
            DoublePinyinScheme::Ziranma
        );
    }
}
