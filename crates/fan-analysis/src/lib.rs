//! 凡语言静态分析。
//!
//! 在不执行源码的前提下，对凡语言程序做符号收集、诊断与编辑器查询，
//! 供 LSP、lint 等工具复用。对应 Go 版 `internal/analysis`。
//!
//! ```text
//! fan-ast → fan-parser → fan-analysis → fan-lsp
//! ```

/// 诊断严重级别与诊断结构。
pub mod diagnostic;

pub use diagnostic::{Diagnostic, Severity};

/// 库版本号，供 LSP `serverInfo` 等场景引用。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_可正常引用诊断类型() {
        let d = Diagnostic::new(1, 1, Severity::Error, "syntax", "测试");
        assert_eq!(d.message, "测试");
        assert_eq!(d.severity, Severity::Error);
    }
}
