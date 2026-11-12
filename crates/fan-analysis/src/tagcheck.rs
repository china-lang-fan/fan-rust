//! 未知标签的静态诊断。
//!
//! 旧的 `@内建` 标签处理器已移除，原生函数改由 `定义 本地(...)` 绑定。标签
//! 框架仍保留（供 `@测试`、切面等自定义标签使用），运行时遇到未注册标签会
//! 静默忽略。这里在静态层对「非已知标签名」给出 Warning，帮助发现拼写错误或
//! 已废弃的 `@内建`，但不阻断运行。

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;
use fan_ast::Program;

use crate::diagnostic::{Diagnostic, Severity};

/// 框架默认识别的标签名：testrun 的测试标签。
///
/// 其余标签需由运行实例通过 `注册标签处理` 显式注册；静态分析无法看到运行时
/// 注册，因此对不在此名单的标签统一给 Warning 而非 Error。
fn is_known_tag(name: &str) -> bool {
    matches!(name, "测试" | "test")
}

/// 遍历程序里所有函数上的标签，对未知标签名产出 Warning 级诊断。
pub fn check_unknown_tags(prog: &Program) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for stmt in &prog.statements {
        let inner = match stmt {
            Stmt::Visibility(v) => &*v.inner,
            other => other,
        };
        let Stmt::VarDecl(v) = inner else { continue };
        let Some(Expr::Function(f)) = &v.value else {
            continue;
        };
        for tag in &f.tags {
            if !is_known_tag(&tag.type_name) {
                diags.push(Diagnostic::new(
                    tag.pos.line,
                    tag.pos.column,
                    Severity::Warning,
                    "unknown-tag",
                    format!(
                        "未注册的标签 @{}：运行时将静默忽略；原生函数请改用「定义 本地(...) 函数」",
                        tag.type_name
                    ),
                ));
            }
        }
    }
    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;

    fn warnings(src: &str) -> Vec<Diagnostic> {
        let (prog, errors) = parse_program(src);
        assert!(errors.is_empty(), "解析错误：{errors}");
        check_unknown_tags(&prog)
    }

    #[test]
    fn 测试标签无诊断() {
        let src = "@测试\n函数 测试甲()\n    返回 1\n结束\n";
        assert!(warnings(src).is_empty());
        let src = "@test\n函数 测试乙()\n    返回 1\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 废弃的内建标签告警() {
        let src = "@内建(\"打印\")\n函数 输出(任意... 内容)\n    返回 空\n结束\n";
        let diags = warnings(src);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "unknown-tag");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("定义 本地"));
    }

    #[test]
    fn 无标签函数无诊断() {
        let src = "函数 甲()\n    返回 1\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 多标签中含未知标签逐个告警() {
        let src = "@测试\n@切面\n函数 甲()\n    返回 1\n结束\n";
        let diags = warnings(src);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "unknown-tag");
        assert!(diags[0].message.contains("切面"));
    }
}
