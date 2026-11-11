//! 凡语言静态分析。
//!
//! 在不执行源码的前提下，对凡语言程序做符号收集、诊断与编辑器查询，
//! 供 LSP、lint 等工具复用。对应 Go 版 `internal/analysis`。
//!
//! ```text
//! fan-ast → fan-parser → fan-analysis → fan-lsp
//! ```

/// 静态分析总入口（聚合各项诊断）。
pub mod analysis;
/// 内建名字与类型关键字集合。
pub mod builtins;
/// 静态诊断检查（导入、未定义名）。
pub mod check;
/// 模型复合字面量检查。
pub mod compositecheck;
/// 诊断严重级别与诊断结构。
pub mod diagnostic;
/// 单文件静态视图：符号收集、文档注释、函数范围。
pub mod document;
/// 同层嵌入歧义检查。
pub mod embedcheck;
/// 跨文件符号索引（静态解析导入）。
pub mod index;
/// 分段函数段间空白与签名匹配检查。
pub mod mixfixcheck;
/// `定义 本地(...)` native 绑定的静态告警。
pub mod nativecheck;
/// file:// URI 与路径互转。
pub mod paths;
/// 编辑器查询：悬停、定义跳转、补全。
pub mod query;
/// 函数体内静态作用域收集。
pub mod scope;
/// 符号与符号种类。
pub mod symbol;
/// LSP 列换算与光标词定位。
pub mod utf16;
/// AST 遍历工具（子节点枚举与深度优先遍历）。
pub mod walk;

pub use analysis::{analyze, analyze_index};
pub use diagnostic::{Diagnostic, Severity};
pub use document::{Document, FnRange, ImportInfo};
pub use index::Index;
pub use query::{CompletionItem, HoverResult, Location, Position, Range};
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
