//! LSP 列换算与光标词定位工具。
//!
//! LSP 的 character 以 UTF-16 码元计、0-based；分析内部以 rune 列（1-based）
//! 计。本模块负责两者互转，并在一行内定位光标下的标识符词。

/// 判断字符是否可作为凡语言标识符的一部分（含 CJK 扩展区）。
pub fn is_letter(ch: char) -> bool {
    ch == '_'
        || ch.is_ascii_alphabetic()
        || ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3400}'..='\u{4DBF}').contains(&ch)
}

/// 把 1-based、rune 计的列转换为 LSP 使用的 0-based UTF-16 码元列。
pub fn utf16_column(line: &str, rune_column: usize) -> usize {
    if rune_column <= 1 {
        return 0;
    }
    let count = rune_column - 1;
    let runes: Vec<char> = line.chars().collect();
    let take = count.min(runes.len());
    runes[..take].iter().map(|c| c.len_utf16()).sum()
}

/// 把 0-based UTF-16 列转换为 rune 索引（0-based）。
pub fn utf16_to_rune_col(line: &str, utf16_col: usize) -> usize {
    let mut units = 0usize;
    for (i, ch) in line.chars().enumerate() {
        if units >= utf16_col {
            return i;
        }
        units += ch.len_utf16();
    }
    line.chars().count()
}

/// 光标下的标识符词，区间以 1-based rune 列表示（end 为闭区间后一位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordMatch {
    pub text: String,
    pub start_col: usize,
    pub end_col: usize,
}

/// 在指定 0-based 行、0-based UTF-16 列处取标识符词。
pub fn word_at(line: &str, char16: usize) -> WordMatch {
    let runes: Vec<char> = line.chars().collect();
    let mut rune_col = utf16_to_rune_col(line, char16);
    if rune_col > runes.len() {
        return WordMatch {
            text: String::new(),
            start_col: 0,
            end_col: 0,
        };
    }
    // 光标停在整行末尾的词右侧边界时左移到词内；行内空白/符号处保持原位。
    if rune_col >= runes.len() && rune_col > 0 && is_letter(runes[rune_col - 1]) {
        rune_col -= 1;
    }
    // 光标落在非标识符字符上（空白、符号）时不返回任何词。
    if rune_col >= runes.len() || !is_letter(runes[rune_col]) {
        return WordMatch {
            text: String::new(),
            start_col: 0,
            end_col: 0,
        };
    }
    let mut start = rune_col;
    while start > 0 && is_letter(runes[start - 1]) {
        start -= 1;
    }
    let mut end = rune_col;
    while end < runes.len() && is_letter(runes[end]) {
        end += 1;
    }
    if start == end {
        return WordMatch {
            text: String::new(),
            start_col: 0,
            end_col: 0,
        };
    }
    WordMatch {
        text: runes[start..end].iter().collect(),
        start_col: start + 1,
        end_col: end + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 定位中文标识符词() {
        let w = word_at("变量 甲 = 1", 3);
        assert_eq!(w.text, "甲");
        assert_eq!(w.start_col, 4);
    }

    #[test]
    fn 空白处返回空词() {
        assert!(word_at("a b", 1).text.is_empty());
    }

    #[test]
    fn utf16列与rune列换算() {
        assert_eq!(utf16_column("甲乙", 1), 0);
        assert_eq!(utf16_column("甲乙", 2), 1);
        assert_eq!(utf16_to_rune_col("甲乙", 1), 1);
    }
}
