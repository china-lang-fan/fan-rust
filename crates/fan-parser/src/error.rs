//! 语法错误。
//!
//! 解析器不因单个错误中断，而是收集全部错误继续解析，
//! 与 Go 版 `Errors` 的行为一致。

use std::fmt;

/// 单个语法错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(line: usize, column: usize, message: impl Into<String>) -> Self {
        ParseError {
            line,
            column,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "第{}行第{}列：{}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

/// 语法错误列表。空列表视为无错误。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Errors(pub Vec<ParseError>);

impl Errors {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn push(&mut self, e: ParseError) {
        self.0.push(e);
    }

    pub fn first_message(&self) -> Option<&str> {
        self.0.first().map(|e| e.message.as_str())
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.first() {
            Some(e) => e.fmt(f),
            None => Ok(()),
        }
    }
}

impl std::error::Error for Errors {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误显示含行列与消息() {
        let e = ParseError::new(3, 7, "缺少右括号 )");
        assert_eq!(e.to_string(), "第3行第7列：缺少右括号 )");
    }

    #[test]
    fn 空错误列表视为无错误() {
        let es = Errors::default();
        assert!(es.is_empty());
        assert_eq!(es.len(), 0);
        assert_eq!(es.to_string(), "");
    }

    #[test]
    fn 错误列表显示首个错误() {
        let mut es = Errors::default();
        es.push(ParseError::new(1, 1, "第一个"));
        es.push(ParseError::new(2, 1, "第二个"));
        assert!(!es.is_empty());
        assert_eq!(es.len(), 2);
        assert_eq!(es.to_string(), "第1行第1列：第一个");
        assert_eq!(es.first_message(), Some("第一个"));
    }
}
