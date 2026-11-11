//! 与具体编辑器协议无关的诊断结构。
//!
//! 行列均为 1-based，列以 Unicode 码点（rune）计；与 Go 版
//! `analysis.Diagnostic` 保持一致。LSP 层负责再转成 0-based、UTF-16 列。

use fan_parser::ParseError;

/// 诊断严重级别（数值与 LSP DiagnosticSeverity 对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

/// 一条静态诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 起始行（1-based）。
    pub line: usize,
    /// 起始列（1-based，rune 计）。
    pub column: usize,
    /// 结束行（1-based）。
    pub end_line: usize,
    /// 结束列（1-based，闭区间后一位，rune 计）。
    pub end_column: usize,
    pub severity: Severity,
    /// 诊断代码，如 `syntax`、`mixfix-space`。
    pub code: String,
    pub message: String,
}

impl Diagnostic {
    /// 创建一条单行诊断，结束列默认为起始列后一位。
    pub fn new(
        line: usize,
        column: usize,
        severity: Severity,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            line,
            column,
            end_line: line,
            end_column: column + 1,
            severity,
            code: code.into(),
            message: message.into(),
        }
    }

    /// 把解析错误转换为 `syntax` 诊断。
    pub fn from_parse_error(e: &ParseError) -> Self {
        Diagnostic::new(
            e.line,
            e.column,
            Severity::Error,
            "syntax",
            e.message.clone(),
        )
    }
}
