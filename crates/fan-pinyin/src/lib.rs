//! 拼音/双拼编码与内置补全引擎。
//!
//! 本 crate 是 [`fan_analysis::pinyin::PinyinEngine`] 的内置实现：先借助
//! `pinyin` crate 把每个汉字转为无声调全拼音节，再在其之上计算三种编码：
//! - 全拼（full）：各字音节直接拼接，如 `打招呼 → dazhaohu`；
//! - 首字母（initial）：各字音节首字母拼接，如 `打招呼 → dzh`；
//! - 双拼（double）：每个音节拆成声母+韵母，按选定方案各映射到一个键，
//!   每字固定两键。键位表严格对齐 Rime 官方双拼方案（自然码/小鹤）的
//!   `speller/algebra`，含零声母（a/o/e 开头）与 `j/q/x/y + u` 的规则。
//!
//! 引擎抽象与配置 DTO 定义在 `fan-analysis`，本 crate 只提供实现，不依赖
//! LSP、不触碰文件系统与动态库；librime 等可选后端在 `fan-lsp` 组合根注入。

use fan_analysis::pinyin::{
    DoublePinyinScheme, EngineCandidate, InputSchemes, PinyinEngine, PinyinMatch, PinyinOptions,
};
use pinyin::ToPinyin;

/// 内置拼音引擎：纯函数、无系统资源，永远可用，作为缺失外部引擎时的回退。
///
/// `suggest` 恒返回空——内置引擎不产自由词，自由整句/大词库由 librime 等
/// 外部引擎通过同一 trait 提供。
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinEngine;

impl PinyinEngine for BuiltinEngine {
    fn rank(&self, input: &str, word: &str, opts: &PinyinOptions) -> Option<PinyinMatch> {
        if !is_latin_prefix(input) {
            return None;
        }
        let codes = codes_of(word, opts.double_scheme);
        if !matches_input(input, &codes, opts.schemes) {
            return None;
        }
        Some(PinyinMatch {
            rank: rank_match(input, &codes, opts.schemes),
            filter_text: format!("{} {} {}", codes.full, codes.initial, codes.double),
        })
    }

    fn suggest(&self, _input: &str, _opts: &PinyinOptions) -> Vec<EngineCandidate> {
        Vec::new()
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

/// 零声母音节（a/o/e 开头）的双拼两键，对齐 Rime `derive/^([aoe])([ioun])$/`
/// 与 `xform/^([aoe])(ng)?$/`：首字母双写 + 韵母键。
///
/// 韵母键直接对**完整音节**查表（如 `an→j`、`en→f`）；仅当完整音节不是
/// 登记韵母（单韵母 `a/o/e` 或 `ng` 收尾的特殊形）时，才退回首字母本身。
fn zero_initial(scheme: DoublePinyinScheme, syllable: &str) -> Option<String> {
    let first = syllable.chars().next()?;
    let final_key = final_key(scheme, syllable).unwrap_or(first);
    Some(format!("{first}{final_key}"))
}

/// 韵母（含零声母首字双写后留下的 `ng`/单韵母等）到键的映射。
fn final_key(scheme: DoublePinyinScheme, fin: &str) -> Option<char> {
    let key = match scheme {
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

/// 单个无声调音节按方案转双拼两键。
fn syllable_to_double(syllable: &str, scheme: DoublePinyinScheme) -> Option<String> {
    // 零声母（a/o/e 开头）走首字母双写规则。
    if matches!(syllable.chars().next(), Some('a' | 'o' | 'e')) {
        return zero_initial(scheme, syllable);
    }
    let (init, fin) = split_syllable(syllable);
    let init_key = initial_key(init)?;
    let fin_key = final_key(scheme, fin)?;
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

/// 计算匹配精度分，数值越小越优先；与 `matches_input` 的命中模式一一对应。
fn rank_match(input: &str, codes: &PinyinCodes, schemes: InputSchemes) -> usize {
    let lower = input.to_ascii_lowercase();
    let exact = |code: &str| code == lower;
    let prefix = |code: &str| code.starts_with(&lower);
    if schemes.double && exact(&codes.double) {
        0
    } else if schemes.double && prefix(&codes.double) {
        1
    } else if schemes.full && exact(&codes.full) {
        2
    } else if schemes.full && prefix(&codes.full) {
        3
    } else if schemes.initial && exact(&codes.initial) {
        4
    } else if schemes.initial && prefix(&codes.initial) {
        5
    } else {
        6
    }
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
    fn 内置引擎命中已知词并给精度分() {
        let engine = BuiltinEngine;
        let opts = PinyinOptions::sensible_default();
        let m = engine.rank("dzh", "打招呼", &opts).expect("首字母应命中");
        assert!(m.filter_text.contains("dazhaohu"));
        assert!(engine.rank("xyz", "打招呼", &opts).is_none());
        assert!(engine.suggest("dzh", &opts).is_empty());
    }

    #[test]
    fn 非拉丁输入引擎不命中() {
        let engine = BuiltinEngine;
        let opts = PinyinOptions::sensible_default();
        assert!(engine.rank("打", "打招呼", &opts).is_none());
    }
}
