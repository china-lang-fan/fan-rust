//! 静态分析总入口：聚合语法错误与各项静态诊断。
//!
//! 对应 Go 版 `analysis.go` 的 `Analyze` / `AnalyzeIndex`。

use fan_parser::parse_program;

use crate::diagnostic::{Diagnostic, Severity};
use crate::index::Index;
use crate::mixfixcheck::check_mixfix_spacing;

/// 对单份源码做完整静态分析（无真实文件 URI，导入解析能力有限）。
pub fn analyze(source: &str) -> Vec<Diagnostic> {
    let index = Index::build("", source);
    analyze_index(&index)
}

/// 基于跨文件索引做完整诊断，供 LSP 使用（含真实文件 URI）。
pub fn analyze_index(index: &Index) -> Vec<Diagnostic> {
    let source = &index.root().source;
    let (prog, errors) = parse_program(source);
    let mut diags: Vec<Diagnostic> = errors
        .0
        .iter()
        .map(Diagnostic::from_parse_error)
        .collect();

    if prog.statements.is_empty() && !errors.is_empty() {
        return diags;
    }

    diags.extend(check_mixfix_spacing(&prog));
    diags.extend(index.root().check_imports(index));
    diags.extend(index.root().check_undefined_names());
    diags.extend(index.check_embed_ambiguity());
    diags.extend(index.check_composite_literals());
    diags
}

/// 仅把解析错误转成诊断，便于编辑器在其它检查关闭时使用。
pub fn syntax_diagnostics(source: &str) -> Vec<Diagnostic> {
    let (_prog, errors) = parse_program(source);
    errors
        .0
        .iter()
        .map(Diagnostic::from_parse_error)
        .collect()
}

/// 解析错误以外的诊断都不是 Error 级；提供一个便捷的严重级别判定。
pub fn is_error(d: &Diagnostic) -> bool {
    matches!(d.severity, Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 语法错误转syntax诊断() {
        let diags = analyze("变量 1 = \n");
        assert!(diags.iter().any(|d| d.code == "syntax"), "实际：{diags:?}");
    }

    #[test]
    fn 完整分析聚合多类诊断() {
        let src = "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n    返回 值\n结束\n变量 x = 修改(\"a\") 的状态为(\"b\")\n";
        let diags = analyze(src);
        assert!(
            diags.iter().any(|d| d.code == "mixfix-space"),
            "应包含段间空格诊断：{diags:?}"
        );
    }

    #[test]
    fn 干净源码无诊断() {
        let src = "函数 加一(整数 x) -> 整数\n    返回 x 加 1\n结束\n变量 y = 加一(1)\n";
        assert!(analyze(src).is_empty(), "实际：{:?}", analyze(src));
    }
}
