//! 同层嵌入歧义检查：当同层多个嵌入提供同名成员时，提示用嵌入名限定。
//!
//! 对应 Go 版 `embedcheck.go`。仅在接收者类型可静态确定时报告，避免对
//! 鸭子类型写法误伤。

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;

use crate::compositecheck::ClassStaticInfo;
use crate::diagnostic::{Diagnostic, Severity};
use crate::index::Index;
use crate::walk::{walk, NodeRef};

type ClassInfoMap = std::collections::HashMap<String, ClassStaticInfo>;

impl Index {
    /// 对入口文档做同层嵌入歧义诊断。
    pub fn check_embed_ambiguity(&self) -> Vec<Diagnostic> {
        let infos = self.build_class_info();
        let prog = self.root().prog.clone();
        let mut diags = Vec::new();
        walk(NodeRef::Program(&prog), |node| {
            match node {
                NodeRef::Expr(Expr::Member(m)) => {
                    let owner = self.static_owner_type(&m.object, &infos);
                    report(&infos, owner, &m.name, m.pos, &mut diags);
                }
                NodeRef::Stmt(Stmt::FieldAssign(f)) => {
                    let owner = self.static_owner_type(&f.object, &infos);
                    report(&infos, owner, &f.name, f.pos, &mut diags);
                }
                _ => {}
            }
            true
        });
        diags
    }

    /// 仅对类型明确的接收者表达式返回模型名，否则空串。
    fn static_owner_type(
        &self,
        expr: &Expr,
        infos: &ClassInfoMap,
    ) -> String {
        match expr {
            // 模型构造调用 模型(...)
            Expr::Call(call) => {
                if let Expr::Ident(id) = &*call.callee {
                    if infos.contains_key(&id.name) {
                        return id.name.clone();
                    }
                }
                String::new()
            }
            Expr::Ident(id) => self.typed_identifier(&id.name, infos),
            Expr::Member(m) => {
                let owner = self.static_owner_type(&m.object, infos);
                if owner.is_empty() {
                    return String::new();
                }
                if let Some(info) = infos.get(&owner) {
                    if info.embeds.contains(&m.name) {
                        return m.name.clone();
                    }
                }
                String::new()
            }
            _ => String::new(),
        }
    }

    /// 文件内可静态确定的标识符类型：带类型标注且类型是已知模型的变量/参数。
    fn typed_identifier(&self, name: &str, infos: &ClassInfoMap) -> String {
        for doc in self.all_docs() {
            for sym in doc.find_by_name(name) {
                if (sym.kind == crate::symbol::SymbolKind::Variable
                    || sym.kind == crate::symbol::SymbolKind::Parameter)
                    && !sym.detail.is_empty()
                    && infos.contains_key(&sym.detail)
                {
                    return sym.detail.clone();
                }
            }
        }
        // 函数参数中带模型类型标注者，以及 `变量 x = 模型(...)` 构造初始化
        let mut found = String::new();
        let prog = self.root().prog.clone();
        walk(NodeRef::Program(&prog), |node| {
            if !found.is_empty() {
                return false;
            }
            match node {
                NodeRef::Expr(Expr::Function(fn_lit)) => {
                    for p in &fn_lit.params {
                        let t = p.type_.to_string();
                        if p.name == name && !p.type_.is_any() && infos.contains_key(&t) {
                            found = t;
                            return false;
                        }
                    }
                }
                NodeRef::Stmt(Stmt::VarDecl(v)) if v.name == name => {
                    if let Some(Expr::Call(call)) = &v.value {
                        if let Expr::Ident(id) = &*call.callee {
                            if infos.contains_key(&id.name) {
                                found = id.name.clone();
                                return false;
                            }
                        }
                    }
                }
                _ => {}
            }
            true
        });
        found
    }
}

fn report(
    infos: &ClassInfoMap,
    owner: String,
    name: &str,
    pos: fan_ast::Position,
    diags: &mut Vec<Diagnostic>,
) {
    if owner.is_empty() {
        return;
    }
    let Some(info) = infos.get(&owner) else {
        return;
    };
    let mut seen = std::collections::HashSet::new();
    let ambiguous_field = ambiguous_member(infos, info, name, true, &mut seen);
    let mut seen = std::collections::HashSet::new();
    let ambiguous_method = ambiguous_member(infos, info, name, false, &mut seen);
    if ambiguous_field || ambiguous_method {
        diags.push(Diagnostic {
            line: pos.line,
            column: pos.column,
            end_line: pos.line,
            end_column: pos.column + owner.chars().count(),
            severity: Severity::Error,
            code: "embed-ambiguous".to_string(),
            message: format!(
                "{owner} 的成员 {name} 在同层嵌入中有歧义，应用 嵌入名.{name} 限定"
            ),
        });
    }
}

/// 同层直接嵌入中，提供该成员的嵌入数量是否达到 2 个。
fn ambiguous_member(
    infos: &ClassInfoMap,
    info: &ClassStaticInfo,
    name: &str,
    field: bool,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    let own = if field {
        info.fields.contains(name)
    } else {
        info.methods.contains(name)
    };
    if own {
        return false;
    }
    let mut count = 0;
    for embed_name in &info.embeds {
        if !seen.insert(embed_name.clone()) {
            continue;
        }
        if let Some(embed) = infos.get(embed_name) {
            if member_exists(infos, embed, name, field) {
                count += 1;
            }
        }
    }
    count >= 2
}

/// 沿嵌入链判断成员是否存在（含深层嵌入）。
fn member_exists(
    infos: &ClassInfoMap,
    info: &ClassStaticInfo,
    name: &str,
    field: bool,
) -> bool {
    let own = if field {
        info.fields.contains(name)
    } else {
        info.methods.contains(name)
    };
    if own {
        return true;
    }
    for embed_name in &info.embeds {
        if let Some(embed) = infos.get(embed_name) {
            if member_exists(infos, embed, name, field) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同层嵌入同名成员报歧义() {
        let src = "\
模型 甲
    属性 整数 共享
结束
模型 乙
    属性 整数 共享
结束
模型 组合
    嵌入 甲
    嵌入 乙
结束
变量 组合 c = 组合()
变量 x = c.共享
";
        let index = Index::build("file:///t.凡", src);
        let diags = index.check_embed_ambiguity();
        assert!(
            diags.iter().any(|d| d.code == "embed-ambiguous"),
            "应报嵌入歧义：{diags:?}"
        );
    }

    #[test]
    fn 无歧义的成员访问不报错() {
        let src = "\
模型 甲
    属性 整数 独有
结束
模型 组合
    嵌入 甲
结束
变量 组合 c = 组合()
变量 x = c.独有
";
        let index = Index::build("file:///t.凡", src);
        assert!(index.check_embed_ambiguity().is_empty());
    }

    #[test]
    fn 构造初始化变量也能推断接收者类型() {
        let src = "\
模型 甲
    属性 整数 共享
结束
模型 乙
    属性 整数 共享
结束
模型 组合
    嵌入 甲
    嵌入 乙
结束
变量 c = 组合()
变量 x = c.共享
";
        let index = Index::build("file:///t.凡", src);
        assert!(index
            .check_embed_ambiguity()
            .iter()
            .any(|d| d.code == "embed-ambiguous"));
    }
}
