//! 模型复合字面量的静态检查：模型是否存在、字段是否属于该模型、字段是否重复。
//!
//! 对应 Go 版 `compositecheck.go`。仅在模型可静态定位时报告，避免对
//! 鸭子类型/动态写法误伤。

use std::collections::HashSet;

use fan_ast::expr::{CompositeLit, Expr};

use crate::diagnostic::{Diagnostic, Severity};
use crate::index::Index;
use crate::walk::{walk, NodeRef};

/// 模型的静态信息：直接字段、直接嵌入、方法名集合。
#[derive(Debug, Default)]
pub struct ClassStaticInfo {
    pub embeds: Vec<String>,
    pub fields: HashSet<String>,
    pub methods: HashSet<String>,
}

impl Index {
    /// 聚合索引内全部文档的模型静态信息。
    pub fn build_class_info(&self) -> std::collections::HashMap<String, ClassStaticInfo> {
        let mut infos = std::collections::HashMap::new();
        for doc in self.all_docs() {
            for sym in doc.top_levels.iter() {
                if sym.kind != crate::symbol::SymbolKind::Class {
                    continue;
                }
                let info: &mut ClassStaticInfo = infos.entry(sym.name.clone()).or_default();
                info.embeds = sym.embeds.clone();
                for child in &sym.children {
                    match child.kind {
                        crate::symbol::SymbolKind::Field => {
                            info.fields.insert(child.name.clone());
                        }
                        crate::symbol::SymbolKind::Method => {
                            info.methods.insert(child.name.clone());
                        }
                        _ => {}
                    }
                }
            }
            // 方法定义在模型语句之外，按 owner 归并
            for sym in doc.top_levels.iter() {
                if sym.kind == crate::symbol::SymbolKind::Method && !sym.owner.is_empty() {
                    infos
                        .entry(sym.owner.clone())
                        .or_default()
                        .methods
                        .insert(sym.name.clone());
                }
            }
        }
        infos
    }

    /// 检查入口文档中的复合字面量。
    pub fn check_composite_literals(&self) -> Vec<Diagnostic> {
        let infos = self.build_class_info();
        let prog = self.root().prog.clone();
        let mut diags = Vec::new();
        walk(NodeRef::Program(&prog), |node| {
            if let NodeRef::Expr(Expr::Composite(lit)) = node {
                diagnose_composite(lit, &infos, &mut diags);
            }
            true
        });
        diags
    }
}

fn composite_model_name(model: &Expr) -> Option<(&str, bool)> {
    match model {
        Expr::Ident(id) => Some((id.name.as_str(), false)),
        Expr::Member(m) => Some((m.name.as_str(), true)),
        _ => None,
    }
}

fn diagnose_composite(
    lit: &CompositeLit,
    infos: &std::collections::HashMap<String, ClassStaticInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some((model_name, is_member)) = composite_model_name(&lit.model) else {
        return;
    };
    let Some(info) = infos.get(model_name) else {
        // 跨模块成员且模块未加载时保守跳过
        if is_member {
            return;
        }
        diags.push(Diagnostic {
            line: lit.pos.line,
            column: lit.pos.column,
            end_line: lit.pos.line,
            end_column: lit.pos.column + model_name.chars().count(),
            severity: Severity::Error,
            code: "composite-unknown-model".to_string(),
            message: format!("未定义的模型：{model_name}"),
        });
        return;
    };
    let mut seen: HashSet<&str> = HashSet::new();
    for field in &lit.fields {
        if seen.contains(field.name.as_str()) {
            diags.push(Diagnostic {
                line: field.name_pos.line,
                column: field.name_pos.column,
                end_line: field.name_pos.line,
                end_column: field.name_pos.column + field.name.chars().count(),
                severity: Severity::Error,
                code: "composite-duplicate-field".to_string(),
                message: format!("复合字面量中字段 {} 重复指定", field.name),
            });
        }
        seen.insert(field.name.as_str());
        if info.fields.contains(&field.name) || info.embeds.contains(&field.name) {
            continue;
        }
        diags.push(Diagnostic {
            line: field.name_pos.line,
            column: field.name_pos.column,
            end_line: field.name_pos.line,
            end_column: field.name_pos.column + field.name.chars().count(),
            severity: Severity::Error,
            code: "composite-unknown-field".to_string(),
            message: format!("模型 {model_name} 没有字段 {}", field.name),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(diags: &[Diagnostic]) -> Vec<String> {
        diags.iter().map(|d| d.code.clone()).collect()
    }

    #[test]
    fn 未定义模型与未知字段报错() {
        let src = "模型 矩形\n    属性 整数 宽\n结束\n变量 a = 矩形{宽: 1, 高: 2}\n变量 b = 圆形{半径: 1}\n";
        let index = Index::build("file:///t.凡", src);
        let diags = index.check_composite_literals();
        assert!(
            codes(&diags).contains(&"composite-unknown-field".to_string()),
            "{diags:?}"
        );
        assert!(
            codes(&diags).contains(&"composite-unknown-model".to_string()),
            "{diags:?}"
        );
    }

    #[test]
    fn 合法复合字面量不报错() {
        // 注：同字段重复在解析期已被 parser 以 syntax 错误拒绝，
        // analysis 的重复分支仅作防御性兜底，正常解析路径不会到达。
        let ok_src =
            "模型 矩形\n    属性 整数 宽\n    属性 整数 高\n结束\n变量 a = 矩形{宽: 1, 高: 2}\n";
        let ok = Index::build("file:///t.凡", ok_src).check_composite_literals();
        assert!(ok.is_empty(), "合法字面量不应报错：{ok:?}");
    }

    #[test]
    fn 嵌入名作为字段合法() {
        let src = "模型 形状\n    属性 整数 边数\n结束\n模型 矩形\n    嵌入 形状\n    属性 整数 宽\n结束\n变量 a = 矩形{形状: 空, 宽: 1}\n";
        let index = Index::build("file:///t.凡", src);
        assert!(index.check_composite_literals().is_empty());
    }
}
