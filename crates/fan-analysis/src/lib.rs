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
/// 内建名字与类型关键字集合。
pub mod builtins;
/// 单文件静态视图：符号收集、文档注释、函数范围。
pub mod document;
/// 跨文件符号索引（静态解析导入）。
pub mod index;
/// file:// URI 与路径互转。
pub mod paths;
/// 符号与符号种类。
pub mod symbol;
/// 函数体内静态作用域收集。
pub mod scope;
/// AST 遍历工具（子节点枚举与深度优先遍历）。
pub mod walk;

pub use diagnostic::{Diagnostic, Severity};
pub use document::{Document, FnRange, ImportInfo};
pub use index::Index;
pub use symbol::{Symbol, SymbolKind};

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
