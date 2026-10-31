//! 凡语言词法分析器。
//!
//! 负责把 UTF-8 源码切成记号流，处理中文关键字、全角标点兼容、
//! 字符串插值与注释。本 crate 不依赖解析器，可独立测试。

pub mod lexer;
pub mod token;

pub use lexer::Lexer;
pub use token::{Token, TokenType};

/// 词法错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} {}", self.line, self.column, self.message)
    }
}
