//! 静态诊断检查：导入解析、未定义名字。
//!
//! 对应 Go 版 `check.go`。第一版采取保守策略，只在证据充分时报告，
//! 避免对鸭子类型、动态特性误伤。

use std::collections::HashSet;

use fan_ast::expr::{Expr, FunctionLit, Ident};
use fan_ast::stmt::Stmt;

use crate::builtins::{builtin_names, soft_keywords, type_keywords};
use crate::diagnostic::{Diagnostic, Severity};
use crate::document::Document;
use crate::index::Index;
use crate::scope::collect_local_scopes;
use crate::symbol::SymbolKind;
use crate::walk::{walk, NodeRef};

impl Document {
    /// 收集文件内全局可见的名字：顶层符号、模型/接口/类型、导入、内建、类型关键字。
    fn global_defined_names(&self) -> HashSet<String> {
        let mut defined: HashSet<String> = HashSet::new();
        for sym in &self.top_levels {
            defined.insert(sym.name.clone());
        }
        for name in self.imports.keys() {
            defined.insert(name.clone());
        }
        for name in builtin_names() {
            defined.insert(name.to_string());
        }
        for name in type_keywords() {
            defined.insert(name.to_string());
        }
        // 局部声明的模型/接口/类型、接口嵌入名
        walk(NodeRef::Program(&self.prog), |node| {
            match node {
                NodeRef::Stmt(Stmt::Class(c)) => {
                    defined.insert(c.name.clone());
                }
                NodeRef::Stmt(Stmt::Interface(i)) => {
                    defined.insert(i.name.clone());
                    for embed in &i.embeds {
                        defined.insert(embed.clone());
                    }
                }
                NodeRef::Stmt(Stmt::TypeDef(t)) => {
                    defined.insert(t.name.clone());
                }
                _ => {}
            }
            true
        });
        defined
    }

    /// 检查导入路径能否静态解析。
    pub fn check_imports(&self, index: &Index) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for info in self.imports.values() {
            if index.resolve(&info.path).is_some() {
                continue;
            }
            let end_col = info.name_pos.column + info.path.chars().count() + 2;
            diags.push(Diagnostic {
                line: info.name_pos.line,
                column: info.name_pos.column,
                end_line: info.name_pos.line,
                end_column: end_col,
                severity: Severity::Error,
                code: "import-not-found".to_string(),
                message: format!("找不到模块：{}", info.path),
            });
        }
        diags
    }

    /// 未定义名字检查：顶层表达式 + 各函数体局部作用域。
    pub fn check_undefined_names(&self) -> Vec<Diagnostic> {
        let globals = self.global_defined_names();
        let mut diags = Vec::new();
        diags.extend(self.check_top_level_expressions(&globals));
        for stmt in &self.prog.statements {
            let inner = match stmt {
                Stmt::Visibility(v) => &*v.inner,
                other => other,
            };
            match inner {
                Stmt::VarDecl(v) if matches!(v.value, Some(Expr::Function(_))) => {
                    if let Some(Expr::Function(f)) = &v.value {
                        diags.extend(check_function_body(f, &globals));
                    }
                }
                Stmt::MethodDef(m) => diags.extend(check_function_body(&m.function, &globals)),
                _ => {}
            }
        }
        diags
    }

    fn check_top_level_expressions(&self, globals: &HashSet<String>) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for stmt in &self.prog.statements {
            let inner = match stmt {
                Stmt::Visibility(v) => &*v.inner,
                other => other,
            };
            let expr: Option<&Expr> = match inner {
                Stmt::VarDecl(v) => match &v.value {
                    Some(Expr::Function(_)) => continue,
                    other => other.as_ref(),
                },
                Stmt::Expr(e) => Some(&e.expr),
                Stmt::Assign(a) => Some(&a.value),
                Stmt::CompoundAssign(c) => Some(&c.value),
                Stmt::Return(r) => r.values.first(),
                _ => None,
            };
            if let Some(e) = expr {
                check_expr_undefined(e, globals, &mut diags);
            }
        }
        diags
    }
}

fn is_known(name: &str, globals: &HashSet<String>) -> bool {
    globals.contains(name)
        || soft_keywords().contains(name)
        || builtin_names().contains(name)
}

fn undefined_diagnostic(id: &Ident) -> Diagnostic {
    Diagnostic {
        line: id.pos.line,
        column: id.pos.column,
        end_line: id.pos.line,
        end_column: id.pos.column + id.name.chars().count(),
        severity: Severity::Warning,
        code: "undefined-name".to_string(),
        message: format!("未定义的名字：{}", id.name),
    }
}

/// 检查表达式中的未定义标识符；嵌套函数字面量与 通过 模块 表达式整体跳过。
fn check_expr_undefined(expr: &Expr, globals: &HashSet<String>, diags: &mut Vec<Diagnostic>) {
    walk(NodeRef::Expr(expr), |node| match node {
        // 嵌套函数字面量由函数作用域检查处理，跳过其子树
        NodeRef::Expr(Expr::Function(_)) => false,
        NodeRef::Expr(Expr::Through(_)) => false,
        NodeRef::Expr(Expr::Ident(id)) => {
            if !is_known(&id.name, globals) {
                diags.push(undefined_diagnostic(id));
            }
            true
        }
        _ => true,
    });
}

/// 基于局部作用域结果检查函数体内的未定义标识符。
fn check_function_body(fn_lit: &FunctionLit, globals: &HashSet<String>) -> Vec<Diagnostic> {
    let local_map = collect_local_scopes(fn_lit);
    // 汇总该函数所有作用域出现过的局部/参数名字（保守并集，宁可漏报不误报）。
    let mut all_locals: HashSet<String> = HashSet::new();
    for locals in local_map.values() {
        for name in locals.keys() {
            all_locals.insert(name.clone());
        }
    }
    let mut diags = Vec::new();
    walk(NodeRef::Expr(&Expr::Function(fn_lit.clone())), |node| {
        if let NodeRef::Expr(Expr::Ident(id)) = node {
            if !globals.contains(&id.name)
                && !all_locals.contains(&id.name)
                && !soft_keywords().contains(id.name.as_str())
            {
                diags.push(undefined_diagnostic(id));
            }
        }
        true
    });
    diags
}

/// 供其他检查复用：标识符号是否为模型/接口等类型符号。
pub fn is_type_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Type
    )
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::path_to_uri;

    fn codes(diags: &[Diagnostic]) -> Vec<String> {
        diags.iter().map(|d| d.code.clone()).collect()
    }

    #[test]
    fn 报告未定义名字但放过内建与类型关键字() {
        let doc = Document::parse(
            "file:///t.凡",
            "变量 甲 = 未知变量 加 长度(名单)\n打印(甲)\n整数 是 类型\n",
        );
        let diags = doc.check_undefined_names();
        assert!(
            diags.iter().any(|d| d.message.contains("未知变量")),
            "应报告 未知变量：{diags:?}"
        );
        // 内建 长度/打印、类型 整数 不应报
        assert!(!diags.iter().any(|d| d.message.contains("长度")));
        assert!(!diags.iter().any(|d| d.message.contains("打印")));
        assert!(!diags.iter().any(|d| d.message.contains("整数")));
    }

    #[test]
    fn 函数参数与局部变量不算未定义() {
        let doc = Document::parse(
            "file:///t.凡",
            "函数 甲(整数 入参) -> 整数\n    变量 局部 = 入参 加 1\n    返回 局部\n结束\n",
        );
        let diags = doc.check_undefined_names();
        assert!(diags.is_empty(), "不应有未定义名：{diags:?}");
    }

    #[test]
    fn 找不到导入模块报导入错误() {
        let doc = Document::parse(
            "file:///tmp/孤立.凡",
            "导入 \"不存在的模块xyz\" 作为 模\n变量 x = 1\n",
        );
        let index = Index::build("file:///tmp/孤立.凡", "变量 x = 1\n");
        let diags = doc.check_imports(&index);
        assert!(codes(&diags).contains(&"import-not-found".to_string()));
    }

    #[test]
    fn 能解析的本地导入不报错() {
        let dir = std::env::temp_dir().join("fan_analysis_check_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("工具.凡"), "函数 帮助() -> 整数\n    返回 1\n结束\n").unwrap();
        let entry = dir.join("主.凡");
        let source = "导入 \"工具\" 作为 工具模块\n";
        let index = Index::build(&path_to_uri(&entry.to_string_lossy()), source);
        let doc = Document::parse(path_to_uri(&entry.to_string_lossy()), source);
        assert!(doc.check_imports(&index).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
