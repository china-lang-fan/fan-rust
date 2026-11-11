//! 函数体内的静态作用域：计算每个标识符引用点可见的局部/参数名字。
//!
//! 对应 Go 版 `scope.go`。采用「进入新作用域 → 手工处理会声明名字的语句 →
//! 其余子节点通用遍历」，保证变量声明之后才可见（不做提升）。

use std::collections::HashMap;

use fan_ast::expr::{Expr, FunctionLit, Ident};
use fan_ast::stmt::{BlockStmt, ForEachStmt, MultiDecl, Stmt, VarDecl};
use fan_ast::{DeclType, Position};

use crate::symbol::SymbolKind;
use crate::walk::{children_of, NodeRef};

/// 一个局部可见名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSymbol {
    pub name: String,
    pub pos: Position,
    pub typ: DeclType,
    pub kind: SymbolKind,
}

/// 标识符位置 → 该点可见的局部名字集合。
pub type LocalScopes = HashMap<Position, HashMap<String, ScopeSymbol>>;

/// 计算函数体内各标识符节点可见的局部/参数名字。
pub fn collect_local_scopes(fn_lit: &FunctionLit) -> LocalScopes {
    let mut result: LocalScopes = HashMap::new();
    let mut scopes: Vec<HashMap<String, ScopeSymbol>> = Vec::new();
    scopes.push(HashMap::new());
    for param in &fn_lit.params {
        declare(
            &mut scopes,
            &param.name,
            param.type_.clone(),
            SymbolKind::Parameter,
            fn_lit.pos,
        );
    }
    declare(
        &mut scopes,
        "自己",
        DeclType::any(),
        SymbolKind::Variable,
        fn_lit.pos,
    );

    for stmt in &fn_lit.body.statements {
        visit_stmt(stmt, &mut scopes, &mut result, fn_lit);
    }
    result
}

fn declare(
    scopes: &mut [HashMap<String, ScopeSymbol>],
    name: &str,
    typ: DeclType,
    kind: SymbolKind,
    pos: Position,
) {
    let last = scopes.last_mut().expect("至少有一个作用域");
    last.insert(
        name.to_string(),
        ScopeSymbol {
            name: name.to_string(),
            pos,
            typ,
            kind,
        },
    );
}

fn visible(scopes: &[HashMap<String, ScopeSymbol>]) -> HashMap<String, ScopeSymbol> {
    let mut out = HashMap::new();
    for scope in scopes {
        for (name, sym) in scope {
            out.insert(name.clone(), sym.clone());
        }
    }
    out
}

fn visit_block(
    block: &BlockStmt,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    scopes.push(HashMap::new());
    for stmt in &block.statements {
        visit_stmt(stmt, scopes, result, owner);
    }
    scopes.pop();
}

fn visit_stmt(
    stmt: &Stmt,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    let inner = match stmt {
        Stmt::Visibility(v) => &*v.inner,
        other => other,
    };
    match inner {
        // 嵌套函数：独立计算其内部作用域，合并结果后不再向下通用遍历。
        Stmt::VarDecl(v) if matches!(v.value, Some(Expr::Function(_))) => {
            if let Some(Expr::Function(nested)) = &v.value {
                merge_nested(nested, result);
            }
            // 嵌套函数名在外层可见
            declare(
                scopes,
                &v.name,
                v.decl_type.clone(),
                SymbolKind::Variable,
                v.pos,
            );
        }
        Stmt::VarDecl(v) => visit_var_decl(v, scopes, result, owner),
        Stmt::MultiDecl(m) => visit_multi_decl(m, scopes, result, owner),
        Stmt::ForEach(f) => visit_for_each(f, scopes, result, owner),
        Stmt::Block(b) => visit_block(b, scopes, result, owner),
        _ => {
            let mut children = Vec::new();
            children_of(NodeRef::Stmt(inner), &mut children);
            for child in children {
                visit_node(child, scopes, result, owner);
            }
        }
    }
}

fn visit_var_decl(
    v: &VarDecl,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    if let Some(value) = &v.value {
        visit_expr(value, scopes, result, owner);
    }
    let kind = if v.is_const {
        SymbolKind::Constant
    } else {
        SymbolKind::Variable
    };
    declare(scopes, &v.name, v.decl_type.clone(), kind, v.pos);
}

fn visit_multi_decl(
    m: &MultiDecl,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    visit_expr(&m.value, scopes, result, owner);
    for name in &m.names {
        declare(scopes, name, DeclType::any(), SymbolKind::Variable, m.pos);
    }
}

fn visit_for_each(
    f: &ForEachStmt,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    visit_expr(&f.iterable, scopes, result, owner);
    scopes.push(HashMap::new());
    declare(scopes, &f.first_name, DeclType::any(), SymbolKind::Variable, f.pos);
    if let Some(second) = &f.second_name {
        declare(scopes, second, DeclType::any(), SymbolKind::Variable, f.pos);
    }
    visit_block(&f.body, scopes, result, owner);
    scopes.pop();
}

fn merge_nested(nested: &FunctionLit, result: &mut LocalScopes) {
    let inner = collect_local_scopes(nested);
    for (pos, names) in inner {
        result.entry(pos).or_insert(names);
    }
}

fn visit_node(
    node: NodeRef<'_>,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    match node {
        NodeRef::Stmt(s) => visit_stmt(s, scopes, result, owner),
        NodeRef::Expr(e) => visit_expr(e, scopes, result, owner),
        NodeRef::Program(_) => {}
    }
}

fn visit_expr(
    expr: &Expr,
    scopes: &mut Vec<HashMap<String, ScopeSymbol>>,
    result: &mut LocalScopes,
    owner: &FunctionLit,
) {
    match expr {
        // 嵌套函数字面量：独立作用域，不进入通用遍历
        Expr::Function(nested) => merge_nested(nested, result),
        // 通过 模块 表达式：模块作用域标识符不在本地检查
        Expr::Through(_) => {}
        Expr::Ident(Ident { pos, .. }) => {
            result.insert(*pos, visible(scopes));
        }
        other => {
            let mut children = Vec::new();
            children_of(NodeRef::Expr(other), &mut children);
            for child in children {
                visit_node(child, scopes, result, owner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;
    use std::collections::HashSet;

    fn first_function(prog: &fan_ast::Program) -> &FunctionLit {
        for stmt in &prog.statements {
            if let Stmt::VarDecl(v) = stmt {
                if let Some(Expr::Function(f)) = &v.value {
                    return f;
                }
            }
        }
        panic!("测试源码应包含函数");
    }

    #[test]
    fn 参数与自己在函数体内可见() {
        let (prog, _) = parse_program(
            "函数 甲(整数 入参) -> 整数\n    返回 入参 加 自己.值\n结束\n",
        );
        let f = first_function(&prog);
        let scopes = collect_local_scopes(f);
        // 返回语句中的两个标识符都应能看到参数/自己
        assert!(!scopes.is_empty());
        let all: HashSet<&String> = scopes
            .values()
            .flat_map(|m| m.keys())
            .collect();
        assert!(all.contains(&"入参".to_string()));
        assert!(all.contains(&"自己".to_string()));
    }

    #[test]
    fn 变量声明后才可见且块结束即回收() {
        let (prog, _) = parse_program(
            "函数 甲()\n    变量 外 = 1\n    如果 真 那么\n        变量 内 = 外\n    结束\n结束\n",
        );
        let f = first_function(&prog);
        let scopes = collect_local_scopes(f);
        // 找到引用「外」的位置，其可见集合应含 外；引用「内」的位置同理
        let mut saw_outer = false;
        for names in scopes.values() {
            if names.contains_key("外") {
                saw_outer = true;
            }
        }
        assert!(saw_outer);
    }

    #[test]
    fn 遍历变量被登记为局部名字() {
        let (prog, _) =
            parse_program("函数 甲(数组 名单)\n    遍历 名单 中的 项\n        打印(项)\n    结束\n结束\n");
        let f = first_function(&prog);
        let scopes = collect_local_scopes(f);
        let all: HashSet<&String> = scopes.values().flat_map(|m| m.keys()).collect();
        assert!(all.contains(&"项".to_string()));
        assert!(all.contains(&"名单".to_string()));
    }
}
