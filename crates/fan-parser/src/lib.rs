//! 凡语言语法分析器。
//!
//! 把词法记号流解析为抽象语法树，采用 Pratt（优先级爬升）算法。
//! 与 Go 版 `internal/parser` 的语法规则和错误消息保持一致。
//!
//! ```text
//! fan-ast → fan-lexer → fan-parser → fan-object → fan-eval → …
//! ```

pub mod error;
pub mod parser;
pub mod precedence;

pub use error::{Errors, ParseError};
pub use fan_ast::Program;
pub use parser::{Parser, Precedence};

/// 解析源码，返回语法树与错误列表（错误不中断解析，会尽量收集全部）。
pub fn parse_program(source: &str) -> (Program, Errors) {
    let mut parser = Parser::new(source);
    let program = parser.parse_program();
    (program, parser.errors().clone())
}

/// 仅做语法检查：有错误返回 `Err`。
pub fn check(source: &str) -> Result<Program, Errors> {
    let (program, errors) = parse_program(source);
    if errors.is_empty() {
        Ok(program)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析成功返回程序且无错误() {
        let (prog, errs) = parse_program("变量 甲 = 1");
        assert!(errs.is_empty());
        assert_eq!(prog.statements.len(), 1);
    }

    #[test]
    fn 解析失败收集错误() {
        let (_prog, errs) = parse_program("如果 甲\n    乙 = 1\n结束");
        assert!(!errs.is_empty());
    }

    #[test]
    fn 检查接口在成功时返回程序() {
        assert!(check("变量 甲 = 1").is_ok());
        assert!(check("如果 甲\n    乙 = 1\n结束").is_err());
    }

    #[test]
    fn 空源码可解析() {
        let (prog, errs) = parse_program("");
        assert!(errs.is_empty());
        assert!(prog.is_empty());
    }
}
