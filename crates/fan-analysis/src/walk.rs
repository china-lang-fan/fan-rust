//! AST 遍历工具。
//!
//! 提供与 Go 版 `childrenOf` / `walkWithParents` 等价的能力：把语句与
//! 表达式统一为 [`NodeRef`]，枚举直接子节点并做深度优先遍历。分析层的
//! 各项检查都建立在此之上，避免各自重复写穷尽匹配。

use fan_ast::expr::{Expr, FunctionLit};
use fan_ast::stmt::{Stmt, Stmt as St};
use fan_ast::{Position, Program};

/// 对 AST 节点的不可变引用，统一语句与表达式两类。
#[derive(Debug, Clone, Copy)]
pub enum NodeRef<'a> {
    Program(&'a Program),
    Stmt(&'a Stmt),
    Expr(&'a Expr),
}

impl<'a> NodeRef<'a> {
    /// 节点起始位置。
    pub fn pos(&self) -> Position {
        match self {
            NodeRef::Program(p) => p.statements.first().map(|s| s.pos()).unwrap_or_default(),
            NodeRef::Stmt(s) => s.pos(),
            NodeRef::Expr(e) => e.pos(),
        }
    }

    /// 若是表达式节点，返回底层表达式引用。
    pub fn as_expr(&self) -> Option<&'a Expr> {
        match self {
            NodeRef::Expr(e) => Some(e),
            _ => None,
        }
    }

    /// 若是语句节点，返回底层语句引用。
    pub fn as_stmt(&self) -> Option<&'a Stmt> {
        match self {
            NodeRef::Stmt(s) => Some(s),
            _ => None,
        }
    }
}

/// 枚举一个节点的直接子节点（语句与表达式），顺序与源码出现顺序一致。
pub fn children_of<'a>(node: NodeRef<'a>, out: &mut Vec<NodeRef<'a>>) {
    match node {
        NodeRef::Program(p) => {
            for s in &p.statements {
                out.push(NodeRef::Stmt(s));
            }
        }
        NodeRef::Stmt(s) => children_of_stmt(s, out),
        NodeRef::Expr(e) => children_of_expr(e, out),
    }
}

fn children_of_stmt<'a>(s: &'a St, out: &mut Vec<NodeRef<'a>>) {
    let push_stmt = |out: &mut Vec<NodeRef<'a>>, s: &'a Stmt| out.push(NodeRef::Stmt(s));
    let push_expr = |out: &mut Vec<NodeRef<'a>>, e: &'a Expr| out.push(NodeRef::Expr(e));
    let push_block = |out: &mut Vec<NodeRef<'a>>, b: &'a fan_ast::BlockStmt| {
        for s in &b.statements {
            push_stmt(out, s);
        }
    };

    match s {
        St::VarDecl(v) => {
            if let Some(value) = &v.value {
                push_expr(out, value);
            }
        }
        St::MultiDecl(m) => push_expr(out, &m.value),
        St::Assign(a) => push_expr(out, &a.value),
        St::CompoundAssign(c) => {
            push_expr(out, &c.target);
            push_expr(out, &c.value);
        }
        St::MultiAssign(m) => push_expr(out, &m.value),
        St::Expr(e) => push_expr(out, &e.expr),
        St::Block(b) => push_block(out, b),
        St::If(i) => {
            for branch in &i.branches {
                push_expr(out, &branch.condition);
                push_block(out, &branch.body);
            }
            if let Some(else_block) = &i.else_ {
                push_block(out, else_block);
            }
        }
        St::Switch(sw) => {
            if let Some(subject) = &sw.subject {
                push_expr(out, subject);
            }
            for branch in &sw.branches {
                for value in &branch.values {
                    push_expr(out, value);
                }
                push_block(out, &branch.body);
            }
        }
        St::While(w) => {
            push_expr(out, &w.condition);
            push_block(out, &w.body);
        }
        St::Repeat(r) => {
            push_block(out, &r.body);
            push_expr(out, &r.until);
        }
        St::Times(t) => {
            push_expr(out, &t.count);
            push_block(out, &t.body);
        }
        St::ForEach(f) => {
            push_expr(out, &f.iterable);
            push_block(out, &f.body);
        }
        St::Return(r) => {
            for value in &r.values {
                push_expr(out, value);
            }
        }
        St::Defer(d) => push_expr(out, &d.call),
        St::Throw(t) => push_expr(out, &t.value),
        St::Visibility(v) => push_stmt(out, &v.inner),
        St::Class(_) | St::Interface(_) | St::Import(_) | St::TypeDef(_) => {}
        St::MethodDef(m) => {
            for param in &m.function.params {
                if let Some(default) = &param.default {
                    push_expr(out, default);
                }
            }
            push_block(out, &m.function.body);
        }
        St::FieldAssign(f) => push_expr(out, &f.value),
        St::IndexAssign(i) => {
            push_expr(out, &i.target);
            push_expr(out, &i.index);
            push_expr(out, &i.value);
        }
        St::Break(_) | St::Continue(_) => {}
    }
}

fn children_of_expr<'a>(e: &'a Expr, out: &mut Vec<NodeRef<'a>>) {
    let push = |out: &mut Vec<NodeRef<'a>>, child: &'a Expr| out.push(NodeRef::Expr(child));

    match e {
        Expr::Integer(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Char(_)
        | Expr::Bool(_)
        | Expr::Nil(_)
        | Expr::Ident(_) => {}
        Expr::InterpStr(i) => {
            for child in &i.exprs {
                push(out, child);
            }
        }
        Expr::Unary(u) => push(out, &u.right),
        Expr::Binary(b) => {
            push(out, &b.left);
            push(out, &b.right);
        }
        Expr::Ternary(t) => {
            push(out, &t.cond);
            push(out, &t.then);
            push(out, &t.else_);
        }
        Expr::Update(u) => push(out, &u.target),
        Expr::MaybeCall(m) => push(out, &m.callee),
        Expr::Call(c) => {
            push(out, &c.callee);
            for arg in &c.args {
                push(out, arg);
            }
        }
        Expr::Index(i) => {
            push(out, &i.target);
            push(out, &i.index);
        }
        Expr::Slice(s) => {
            push(out, &s.target);
            if let Some(low) = &s.low {
                push(out, low);
            }
            if let Some(high) = &s.high {
                push(out, high);
            }
        }
        Expr::Member(m) => push(out, &m.object),
        Expr::ImplicitReceiverCall(i) => {
            push(out, &i.receiver);
            for arg in &i.args {
                push(out, arg);
            }
        }
        Expr::Array(a) => {
            for element in &a.elements {
                push(out, element);
            }
        }
        Expr::Dict(d) => {
            for pair in &d.pairs {
                if let Some(key) = &pair.key {
                    push(out, key);
                }
                push(out, &pair.value);
            }
        }
        Expr::Spread(s) => push(out, &s.value),
        Expr::Tuple(t) => {
            for element in &t.elements {
                push(out, element);
            }
        }
        Expr::Function(f) => children_of_function(f, out),
        Expr::Composite(c) => {
            push(out, &c.model);
            for field in &c.fields {
                if let Some(value) = &field.value {
                    push(out, value);
                }
            }
        }
        Expr::TypeTest(t) => push(out, &t.object),
        Expr::TypeCast(t) => push(out, &t.object),
        Expr::Check(c) => push(out, &c.call),
        Expr::Through(t) => push(out, &t.expr),
        Expr::Tag(t) => {
            for arg in &t.positional {
                push(out, arg);
            }
            for arg in &t.named {
                push(out, &arg.value);
            }
        }
    }
}

fn children_of_function<'a>(f: &'a FunctionLit, out: &mut Vec<NodeRef<'a>>) {
    for param in &f.params {
        if let Some(default) = &param.default {
            out.push(NodeRef::Expr(default));
        }
    }
    for s in &f.body.statements {
        out.push(NodeRef::Stmt(s));
    }
}

/// 深度优先遍历；回调收到节点及其直接父节点（根的父节点为 `None`）。
/// 回调返回 `false` 时跳过该节点的子树。
pub fn walk_with_parents<'a, F>(root: NodeRef<'a>, mut visit: F)
where
    F: FnMut(NodeRef<'a>, Option<NodeRef<'a>>) -> bool,
{
    fn go<'a>(
        node: NodeRef<'a>,
        parent: Option<NodeRef<'a>>,
        visit: &mut impl FnMut(NodeRef<'a>, Option<NodeRef<'a>>) -> bool,
    ) {
        if !visit(node, parent) {
            return;
        }
        let mut children = Vec::new();
        children_of(node, &mut children);
        for child in children {
            go(child, Some(node), visit);
        }
    }
    go(root, None, &mut visit);
}

/// 深度优先遍历，不提供父节点。
pub fn walk<'a, F>(root: NodeRef<'a>, mut visit: F)
where
    F: FnMut(NodeRef<'a>) -> bool,
{
    walk_with_parents(root, |node, _| visit(node));
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;

    #[test]
    fn 遍历覆盖嵌套表达式与语句() {
        let (prog, errors) =
            parse_program("变量 x = 1 加 2\n如果 x 大于 1 那么\n    打印(x)\n结束\n");
        assert!(errors.is_empty(), "解析错误：{errors:?}");
        let mut count = 0;
        let mut saw_call = false;
        walk(NodeRef::Program(&prog), |node| {
            count += 1;
            if let NodeRef::Expr(Expr::Call(c)) = node {
                if let Expr::Ident(id) = &*c.callee {
                    if id.name == "打印" {
                        saw_call = true;
                    }
                }
            }
            true
        });
        assert!(count > 3);
        assert!(saw_call, "应遍历到 打印 调用");
    }

    #[test]
    fn 回调返回假时跳过子树() {
        let (prog, _) = parse_program("变量 x = 1 加 2\n");
        let mut expr_count = 0;
        walk(NodeRef::Program(&prog), |node| {
            if matches!(node, NodeRef::Stmt(_)) {
                // 跳过语句子树，则其内部表达式都不会被访问
                return false;
            }
            if matches!(node, NodeRef::Expr(_)) {
                expr_count += 1;
            }
            true
        });
        assert_eq!(expr_count, 0);
    }
}
