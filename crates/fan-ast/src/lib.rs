//! 凡语言抽象语法树。
//!
//! 该 crate 不依赖任何其他凡语言 crate，是整个编译/解释流水线的叶子：
//!
//! ```text
//! fan-ast → fan-lexer → fan-parser → fan-object → fan-eval → fan-native/fan-testrun → fan-cli
//! ```
//!
//! 与 Go 版 `internal/ast` 的节点一一对应，字段语义与 `String()` 输出保持一致，
//! 以便与 Go 版做 AST 快照对拍。

pub mod decl_type;
pub mod expr;
pub mod position;
pub mod stmt;

pub use decl_type::DeclType;
pub use expr::format_f64;
pub use expr::{
    ArrayLit, BinaryExpr, BoolLit, CallExpr, CharLit, CheckExpr, CompositeField, CompositeLit,
    DictLit, DictPair, Expr, FloatLit, FunctionLit, Ident, ImplicitReceiverCallExpr, IndexExpr,
    IntegerLit, InterpString, MaybeCallExpr, MemberExpr, NamedTagArg, NativeDecl, NilLit,
    Parameter, SliceExpr,
    SpreadExpr, StringLit, TagExpr, TernaryExpr, ThroughExpr, TupleExpr, TypeCastExpr,
    TypeTestExpr, UnaryExpr, UpdateExpr,
};
pub use position::Position;
pub use stmt::{
    AssignStmt, BlockStmt, BreakStmt, ClassStmt, CompoundAssignStmt, ContinueStmt, DeferStmt,
    ExpressionStmt, FieldAssignExpr, FieldDecl, ForEachStmt, IfBranch, IfStmt, ImportStmt,
    IndexAssignStmt, InterfaceMethodSig, InterfaceStmt, MethodDef, MultiAssign, MultiDecl,
    RepeatStmt, ReturnStmt, Stmt, SwitchBranch, SwitchStmt, ThrowStmt, TimesStmt, TypeDefStmt,
    VarDecl, Visibility, VisibilityStmt, WhileStmt,
};

/// 一个完整的凡语言程序（一个源文件）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub statements: Vec<Stmt>,
}

impl Program {
    pub fn new(statements: Vec<Stmt>) -> Self {
        Program { statements }
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }
}

impl std::fmt::Display for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for s in &self.statements {
            writeln!(f, "{}", s)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expr::{Ident, IntegerLit};
    use stmt::{AssignStmt, ExpressionStmt};

    fn pos() -> Position {
        Position::new(1, 1)
    }

    #[test]
    fn 空程序无语句() {
        let p = Program::default();
        assert!(p.is_empty());
        assert_eq!(p.to_string(), "");
    }

    #[test]
    fn 程序按行输出各语句() {
        let stmt = Stmt::Assign(AssignStmt {
            pos: pos(),
            name: "甲".to_string(),
            value: Expr::Integer(IntegerLit {
                pos: pos(),
                value: 1,
            }),
        });
        let p = Program::new(vec![stmt.clone(), stmt]);
        assert_eq!(p.to_string(), "甲 = 1\n甲 = 1\n");
        assert_eq!(p.statements.len(), 2);
    }

    #[test]
    fn 表达式语句包裹表达式() {
        let e = ExpressionStmt {
            pos: pos(),
            expr: Expr::Ident(Ident {
                pos: pos(),
                name: "甲".to_string(),
            }),
        };
        assert_eq!(e.to_string(), "甲");
    }
}
