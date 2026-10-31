//! 语句节点。
//!
//! 与 Go 版 `internal/ast` 的语句节点一一对应，`Display` 复刻 Go 版 `String()`
//! 的输出，便于与 Go 版做 AST 快照对拍。

use crate::decl_type::DeclType;
use crate::expr::{Expr, FunctionLit, Parameter};
use crate::position::Position;
use std::fmt;

/// 语句节点。
// 各变体承载的信息量差异较大（空语句 vs 模型定义），盒子化会引入额外的
// 解引用噪音；此处保留与 Go 版一致的结构，编译期即可接受该尺寸差。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    VarDecl(VarDecl),
    Assign(AssignStmt),
    CompoundAssign(CompoundAssignStmt),
    MultiDecl(MultiDecl),
    MultiAssign(MultiAssign),
    Expr(ExpressionStmt),
    Block(BlockStmt),
    If(IfStmt),
    Switch(SwitchStmt),
    While(WhileStmt),
    Repeat(RepeatStmt),
    Times(TimesStmt),
    ForEach(ForEachStmt),
    Break(BreakStmt),
    Continue(ContinueStmt),
    Return(ReturnStmt),
    Defer(DeferStmt),
    Throw(ThrowStmt),
    Import(ImportStmt),
    Visibility(VisibilityStmt),
    Class(ClassStmt),
    MethodDef(MethodDef),
    Interface(InterfaceStmt),
    TypeDef(TypeDefStmt),
    FieldAssign(FieldAssignExpr),
    IndexAssign(IndexAssignStmt),
}

impl Stmt {
    /// 节点在源码中的起始位置。
    pub fn pos(&self) -> Position {
        match self {
            Stmt::VarDecl(n) => n.pos,
            Stmt::Assign(n) => n.pos,
            Stmt::CompoundAssign(n) => n.pos,
            Stmt::MultiDecl(n) => n.pos,
            Stmt::MultiAssign(n) => n.pos,
            Stmt::Expr(n) => n.pos,
            Stmt::Block(n) => n.pos,
            Stmt::If(n) => n.pos,
            Stmt::Switch(n) => n.pos,
            Stmt::While(n) => n.pos,
            Stmt::Repeat(n) => n.pos,
            Stmt::Times(n) => n.pos,
            Stmt::ForEach(n) => n.pos,
            Stmt::Break(n) => n.pos,
            Stmt::Continue(n) => n.pos,
            Stmt::Return(n) => n.pos,
            Stmt::Defer(n) => n.pos,
            Stmt::Throw(n) => n.pos,
            Stmt::Import(n) => n.pos,
            Stmt::Visibility(n) => n.pos,
            Stmt::Class(n) => n.pos,
            Stmt::MethodDef(n) => n.pos,
            Stmt::Interface(n) => n.pos,
            Stmt::TypeDef(n) => n.pos,
            Stmt::FieldAssign(n) => n.pos,
            Stmt::IndexAssign(n) => n.pos,
        }
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stmt::VarDecl(n) => n.fmt(f),
            Stmt::Assign(n) => n.fmt(f),
            Stmt::CompoundAssign(n) => n.fmt(f),
            Stmt::MultiDecl(n) => n.fmt(f),
            Stmt::MultiAssign(n) => n.fmt(f),
            Stmt::Expr(n) => n.fmt(f),
            Stmt::Block(n) => n.fmt(f),
            Stmt::If(n) => n.fmt(f),
            Stmt::Switch(n) => n.fmt(f),
            Stmt::While(n) => n.fmt(f),
            Stmt::Repeat(n) => n.fmt(f),
            Stmt::Times(n) => n.fmt(f),
            Stmt::ForEach(n) => n.fmt(f),
            Stmt::Break(n) => n.fmt(f),
            Stmt::Continue(n) => n.fmt(f),
            Stmt::Return(n) => n.fmt(f),
            Stmt::Defer(n) => n.fmt(f),
            Stmt::Throw(n) => n.fmt(f),
            Stmt::Import(n) => n.fmt(f),
            Stmt::Visibility(n) => n.fmt(f),
            Stmt::Class(n) => n.fmt(f),
            Stmt::MethodDef(n) => n.fmt(f),
            Stmt::Interface(n) => n.fmt(f),
            Stmt::TypeDef(n) => n.fmt(f),
            Stmt::FieldAssign(n) => n.fmt(f),
            Stmt::IndexAssign(n) => n.fmt(f),
        }
    }
}

// ---------------------------------------------------------------- 声明与赋值

/// 变量或常量声明。`is_const` 为真表示常量。
#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub pos: Position,
    pub is_const: bool,
    /// 是否显式写出了 `变量`/`常量` 关键字。
    pub is_explicit: bool,
    pub decl_type: DeclType,
    pub name: String,
    /// 初值；无初值时为 `None`（初始为 `空`）。
    pub value: Option<Expr>,
    pub visibility: Visibility,
}

impl fmt::Display for VarDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.is_const { "常量" } else { "变量" };
        let typ = if self.decl_type.is_any() {
            String::new()
        } else {
            format!(" {}", self.decl_type)
        };
        match &self.value {
            Some(v) => write!(f, "{}{} {} = {}", kind, typ, self.name, v),
            None => write!(f, "{}{} {}", kind, typ, self.name),
        }
    }
}

/// 普通赋值 `甲 = 1`。
#[derive(Debug, Clone, PartialEq)]
pub struct AssignStmt {
    pub pos: Position,
    pub name: String,
    pub value: Expr,
}

impl fmt::Display for AssignStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {}", self.name, self.value)
    }
}

/// 复合赋值 `甲 += 1`。
#[derive(Debug, Clone, PartialEq)]
pub struct CompoundAssignStmt {
    pub pos: Position,
    pub target: Expr,
    pub op: String,
    pub value: Expr,
}

impl fmt::Display for CompoundAssignStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {} {})", self.target, self.op, self.value)
    }
}

/// 多目标声明 `变量 甲、乙 = f()`。
#[derive(Debug, Clone, PartialEq)]
pub struct MultiDecl {
    pub pos: Position,
    pub is_const: bool,
    pub is_explicit: bool,
    pub names: Vec<String>,
    pub value: Expr,
    pub visibility: Visibility,
}

impl fmt::Display for MultiDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "变量 {} = {}", self.names.join("、"), self.value)
    }
}

/// 多目标赋值 `甲、乙 = f()`。
#[derive(Debug, Clone, PartialEq)]
pub struct MultiAssign {
    pub pos: Position,
    pub names: Vec<String>,
    pub value: Expr,
}

impl fmt::Display for MultiAssign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {}", self.names.join("、"), self.value)
    }
}

/// 单独的表达式语句。
#[derive(Debug, Clone, PartialEq)]
pub struct ExpressionStmt {
    pub pos: Position,
    pub expr: Expr,
}

impl fmt::Display for ExpressionStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.expr.fmt(f)
    }
}

/// 语句块，引入新的作用域。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStmt {
    pub pos: Position,
    pub statements: Vec<Stmt>,
}

impl fmt::Display for BlockStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        for s in &self.statements {
            out.push_str(&s.to_string());
            out.push_str("; ");
        }
        f.write_str(out.trim_end().trim_end_matches(';'))
    }
}

// ---------------------------------------------------------------- 控制流

/// `如果` 语句的一个分支。
#[derive(Debug, Clone, PartialEq)]
pub struct IfBranch {
    pub condition: Expr,
    pub body: BlockStmt,
}

/// 条件语句，整条分支链共用一个 `结束`。
#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub pos: Position,
    pub branches: Vec<IfBranch>,
    pub else_: Option<BlockStmt>,
}

impl fmt::Display for IfStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, br) in self.branches.iter().enumerate() {
            if idx == 0 {
                f.write_str("如果 ")?;
            } else {
                f.write_str("否则如果 ")?;
            }
            write!(f, "{} 那么 {{ ", br.condition)?;
            write!(f, "{}", br.body)?;
            f.write_str(" } ")?;
        }
        if let Some(e) = &self.else_ {
            write!(f, "否则 {{ {} }} ", e)?;
        }
        f.write_str("结束")
    }
}

/// `判断` 语句的一个分支。
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchBranch {
    pub pos: Position,
    /// `当 条件` 形式的守卫分支。
    pub guard: bool,
    /// `为 值1、值2` 形式的值匹配列表。
    pub values: Vec<Expr>,
    pub body: BlockStmt,
    /// `其他` 默认分支。
    pub default: bool,
}

/// 判断语句。
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchStmt {
    pub pos: Position,
    /// 判断目标；`判断` 后不写表达式时为 `None`。
    pub subject: Option<Expr>,
    pub branches: Vec<SwitchBranch>,
}

impl fmt::Display for SwitchStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.subject {
            Some(s) => write!(f, "判断 {} ... 结束", s),
            None => f.write_str("判断 ... 结束"),
        }
    }
}

/// 条件循环 `当 条件 循环 ... 结束`。
#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    pub pos: Position,
    pub condition: Expr,
    pub body: BlockStmt,
}

impl fmt::Display for WhileStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "当 {} 循环 {{ {} }} 结束", self.condition, self.body)
    }
}

/// 后判断循环 `重复 ... 直到 条件 结束`。
#[derive(Debug, Clone, PartialEq)]
pub struct RepeatStmt {
    pub pos: Position,
    pub body: BlockStmt,
    /// 停止条件，为真时退出。
    pub until: Expr,
}

impl fmt::Display for RepeatStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "重复 {{ {} }} 直到 {} 结束", self.body, self.until)
    }
}

/// 计数循环 `重复 N 次 ... 结束`。
#[derive(Debug, Clone, PartialEq)]
pub struct TimesStmt {
    pub pos: Position,
    pub count: Expr,
    pub body: BlockStmt,
}

impl fmt::Display for TimesStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "重复 {} 次 {{ {} }} 结束", self.count, self.body)
    }
}

/// 遍历循环 `遍历 集合 中的 变量 [变量] ... 结束`。
#[derive(Debug, Clone, PartialEq)]
pub struct ForEachStmt {
    pub pos: Position,
    pub iterable: Expr,
    /// 第一个迭代变量名（数组/字符串为元素，字典为值）。
    pub first_name: String,
    /// 第二个迭代变量名（数组/字符串为序号，字典为键）。
    pub second_name: Option<String>,
    pub body: BlockStmt,
}

impl fmt::Display for ForEachStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.second_name {
            Some(second) => write!(
                f,
                "遍历 {} 中的 {} {} {{ {} }} 结束",
                self.iterable, self.first_name, second, self.body
            ),
            None => write!(
                f,
                "遍历 {} 中的 {} {{ {} }} 结束",
                self.iterable, self.first_name, self.body
            ),
        }
    }
}

/// `跳出` 语句。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakStmt {
    pub pos: Position,
}

impl fmt::Display for BreakStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("跳出")
    }
}

/// `继续` 语句。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueStmt {
    pub pos: Position,
}

impl fmt::Display for ContinueStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("继续")
    }
}

/// `返回` 语句，支持多返回值。
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStmt {
    pub pos: Position,
    pub values: Vec<Expr>,
}

impl fmt::Display for ReturnStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.values.is_empty() {
            return f.write_str("返回");
        }
        let parts: Vec<String> = self.values.iter().map(|v| v.to_string()).collect();
        write!(f, "返回 {}", parts.join("、"))
    }
}

/// `稍后` 语句：函数返回前执行。
#[derive(Debug, Clone, PartialEq)]
pub struct DeferStmt {
    pub pos: Position,
    pub call: Expr,
}

impl fmt::Display for DeferStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "稍后 {}", self.call)
    }
}

/// `抛出` 语句：抛出运行时崩溃。
#[derive(Debug, Clone, PartialEq)]
pub struct ThrowStmt {
    pub pos: Position,
    pub value: Expr,
}

impl fmt::Display for ThrowStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "抛出 {}", self.value)
    }
}

// ---------------------------------------------------------------- 模块与可见性

/// 导入语句 `导入 "路径" 作为 名字`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportStmt {
    pub pos: Position,
    pub path: String,
    /// 模块别名；未写 `作为` 时取路径最后一段。
    pub name: String,
}

impl fmt::Display for ImportStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "导入 {:?} 作为 {}", self.path, self.name)
    }
}

/// 可见性级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    /// 默认：仅当前文件（模块）内可访问。
    #[default]
    File,
    /// `公开`：可跨模块访问。
    Public,
    /// `私有`：模块级等同默认；模型字段/方法仅本模型方法内可访问。
    Private,
}

impl Visibility {
    pub fn keyword(&self) -> &'static str {
        match self {
            Visibility::File => "",
            Visibility::Public => "公开",
            Visibility::Private => "私有",
        }
    }
}

/// `公开`/`私有` 修饰的声明，包裹一条内部语句。
#[derive(Debug, Clone, PartialEq)]
pub struct VisibilityStmt {
    pub pos: Position,
    /// 被修饰的声明名。
    pub name: String,
    pub visibility: Visibility,
    pub inner: Box<Stmt>,
}

impl fmt::Display for VisibilityStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.visibility.keyword(), self.inner)
    }
}

// ---------------------------------------------------------------- 模型与接口

/// 模型字段声明。
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub pos: Position,
    pub name: String,
    pub type_: DeclType,
    pub visibility: Visibility,
}

/// 模型定义语句。模型内只定义字段与嵌入，方法在模型外定义。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassStmt {
    pub pos: Position,
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// 嵌入的字段名列表。
    pub embeds: Vec<String>,
    pub tags: Vec<crate::expr::TagExpr>,
    pub visibility: Visibility,
}

impl fmt::Display for ClassStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "类 {} ", self.name)?;
        for fl in &self.fields {
            if fl.type_.is_any() {
                write!(f, "变量 {} ", fl.name)?;
            } else {
                write!(f, "变量 {} {} ", fl.type_, fl.name)?;
            }
        }
        for e in &self.embeds {
            write!(f, "嵌入 {} ", e)?;
        }
        f.write_str("结束")
    }
}

/// 模型方法定义 `定义 矩形 的 方法 面积() ...`。
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDef {
    pub pos: Position,
    pub class_name: String,
    pub method_name: String,
    pub function: FunctionLit,
    /// 是否为原生类型的方法扩展（如 `定义 字符串 的 方法`）。
    pub primitive: bool,
    /// 显式接收者名（可为空，此时用 `自己`）。
    pub receiver_name: String,
    pub visibility: Visibility,
}

impl fmt::Display for MethodDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "定义 {} 的 方法 {} {}",
            self.class_name, self.method_name, self.function
        )
    }
}

/// 接口中的方法签名（无函数体）。
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceMethodSig {
    pub name: String,
    pub params: Vec<Parameter>,
    pub return_types: Vec<DeclType>,
    pub pos: Position,
}

/// 接口声明。模型无需声明实现，拥有全部方法即自动满足。
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceStmt {
    pub pos: Position,
    pub name: String,
    /// 嵌入的接口名。
    pub embeds: Vec<String>,
    pub methods: Vec<InterfaceMethodSig>,
    pub visibility: Visibility,
}

impl fmt::Display for InterfaceStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "接口 {}", self.name)
    }
}

/// 自定义类型声明 `定义 类型 状态 = 整数`。
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDefStmt {
    pub pos: Position,
    pub name: String,
    /// 底层类型，可以是基础类型、模型/接口名或联合类型。
    pub base_type: DeclType,
    pub visibility: Visibility,
}

impl fmt::Display for TypeDefStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "类型 {}", self.name)
    }
}

/// 字段赋值 `obj.字段 = 值`。
#[derive(Debug, Clone, PartialEq)]
pub struct FieldAssignExpr {
    pub pos: Position,
    pub object: Expr,
    pub name: String,
    pub value: Expr,
}

impl fmt::Display for FieldAssignExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{} = {}", self.object, self.name, self.value)
    }
}

/// 下标赋值 `a[0] = 1`。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexAssignStmt {
    pub pos: Position,
    pub target: Expr,
    pub index: Expr,
    pub value: Expr,
}

impl fmt::Display for IndexAssignStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}] = {}", self.target, self.index, self.value)
    }
}

// 测试中 3.14 只是普通小数，并非要近似 π
#[cfg_attr(test, allow(clippy::approx_constant))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Ident, IntegerLit};

    fn pos() -> Position {
        Position::new(1, 1)
    }

    fn int(v: i64) -> Expr {
        Expr::Integer(IntegerLit {
            pos: pos(),
            value: v,
        })
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(Ident {
            pos: pos(),
            name: name.to_string(),
        })
    }

    fn empty_block() -> BlockStmt {
        BlockStmt {
            pos: pos(),
            statements: vec![],
        }
    }

    #[test]
    fn 变量声明显示类型与初值() {
        let d = VarDecl {
            pos: pos(),
            is_const: false,
            is_explicit: true,
            decl_type: DeclType::named("整数"),
            name: "年龄".to_string(),
            value: Some(int(18)),
            visibility: Visibility::File,
        };
        assert_eq!(d.to_string(), "变量 整数 年龄 = 18");
    }

    #[test]
    fn 常量声明显示常量关键字() {
        let d = VarDecl {
            pos: pos(),
            is_const: true,
            is_explicit: true,
            decl_type: DeclType::any(),
            name: "圆周率".to_string(),
            value: Some(crate::expr::Expr::Float(crate::expr::FloatLit {
                pos: pos(),
                value: 3.14,
            })),
            visibility: Visibility::Public,
        };
        assert_eq!(d.to_string(), "常量 圆周率 = 3.14");
    }

    #[test]
    fn 无初值声明不显示等号() {
        let d = VarDecl {
            pos: pos(),
            is_const: false,
            is_explicit: true,
            decl_type: DeclType::named("字符串"),
            name: "名称".to_string(),
            value: None,
            visibility: Visibility::File,
        };
        assert_eq!(d.to_string(), "变量 字符串 名称");
    }

    #[test]
    fn 赋值与复合赋值显示() {
        let a = AssignStmt {
            pos: pos(),
            name: "甲".to_string(),
            value: int(1),
        };
        assert_eq!(a.to_string(), "甲 = 1");

        let c = CompoundAssignStmt {
            pos: pos(),
            target: ident("甲"),
            op: "+=".to_string(),
            value: int(2),
        };
        assert_eq!(c.to_string(), "(甲 += 2)");
    }

    #[test]
    fn 多目标声明与赋值以顿号连接() {
        let d = MultiDecl {
            pos: pos(),
            is_const: false,
            is_explicit: true,
            names: vec!["甲".to_string(), "乙".to_string()],
            value: int(1),
            visibility: Visibility::File,
        };
        assert_eq!(d.to_string(), "变量 甲、乙 = 1");

        let a = MultiAssign {
            pos: pos(),
            names: vec!["甲".to_string(), "乙".to_string()],
            value: int(1),
        };
        assert_eq!(a.to_string(), "甲、乙 = 1");
    }

    #[test]
    fn 语句块以分号连接且无尾随分号() {
        let b = BlockStmt {
            pos: pos(),
            statements: vec![
                Stmt::Assign(AssignStmt {
                    pos: pos(),
                    name: "甲".to_string(),
                    value: int(1),
                }),
                Stmt::Assign(AssignStmt {
                    pos: pos(),
                    name: "乙".to_string(),
                    value: int(2),
                }),
            ],
        };
        assert_eq!(b.to_string(), "甲 = 1; 乙 = 2");
    }

    #[test]
    fn 空语句块显示为空串() {
        assert_eq!(empty_block().to_string(), "");
    }

    #[test]
    fn 条件语句显示分支链与否则块() {
        let s = IfStmt {
            pos: pos(),
            branches: vec![
                IfBranch {
                    condition: ident("甲"),
                    body: empty_block(),
                },
                IfBranch {
                    condition: ident("乙"),
                    body: empty_block(),
                },
            ],
            else_: Some(empty_block()),
        };
        assert_eq!(
            s.to_string(),
            "如果 甲 那么 {  } 否则如果 乙 那么 {  } 否则 {  } 结束"
        );
    }

    #[test]
    fn 判断语句显示主题() {
        let with = SwitchStmt {
            pos: pos(),
            subject: Some(ident("状态码")),
            branches: vec![],
        };
        assert_eq!(with.to_string(), "判断 状态码 ... 结束");

        let without = SwitchStmt {
            pos: pos(),
            subject: None,
            branches: vec![],
        };
        assert_eq!(without.to_string(), "判断 ... 结束");
    }

    #[test]
    fn 三种循环语句显示() {
        let w = WhileStmt {
            pos: pos(),
            condition: ident("甲"),
            body: empty_block(),
        };
        assert_eq!(w.to_string(), "当 甲 循环 {  } 结束");

        let r = RepeatStmt {
            pos: pos(),
            body: empty_block(),
            until: ident("甲"),
        };
        assert_eq!(r.to_string(), "重复 {  } 直到 甲 结束");

        let t = TimesStmt {
            pos: pos(),
            count: int(3),
            body: empty_block(),
        };
        assert_eq!(t.to_string(), "重复 3 次 {  } 结束");
    }

    #[test]
    fn 遍历循环单变量与双变量() {
        let one = ForEachStmt {
            pos: pos(),
            iterable: ident("名单"),
            first_name: "人".to_string(),
            second_name: None,
            body: empty_block(),
        };
        assert_eq!(one.to_string(), "遍历 名单 中的 人 {  } 结束");

        let two = ForEachStmt {
            pos: pos(),
            iterable: ident("名单"),
            first_name: "人".to_string(),
            second_name: Some("序号".to_string()),
            body: empty_block(),
        };
        assert_eq!(two.to_string(), "遍历 名单 中的 人 序号 {  } 结束");
    }

    #[test]
    fn 跳出与继续显示中文关键字() {
        assert_eq!(BreakStmt { pos: pos() }.to_string(), "跳出");
        assert_eq!(ContinueStmt { pos: pos() }.to_string(), "继续");
    }

    #[test]
    fn 返回语句无值与多值() {
        assert_eq!(
            ReturnStmt {
                pos: pos(),
                values: vec![]
            }
            .to_string(),
            "返回"
        );
        let r = ReturnStmt {
            pos: pos(),
            values: vec![int(1), int(2)],
        };
        assert_eq!(r.to_string(), "返回 1、2");
    }

    #[test]
    fn 稍后与抛出显示() {
        let d = DeferStmt {
            pos: pos(),
            call: ident("关闭"),
        };
        assert_eq!(d.to_string(), "稍后 关闭");

        let t = ThrowStmt {
            pos: pos(),
            value: ident("错误"),
        };
        assert_eq!(t.to_string(), "抛出 错误");
    }

    #[test]
    fn 导入语句显示路径与别名() {
        let i = ImportStmt {
            pos: pos(),
            path: "路径/模块名".to_string(),
            name: "m".to_string(),
        };
        assert_eq!(i.to_string(), "导入 \"路径/模块名\" 作为 m");
    }

    #[test]
    fn 可见性关键字映射() {
        assert_eq!(Visibility::File.keyword(), "");
        assert_eq!(Visibility::Public.keyword(), "公开");
        assert_eq!(Visibility::Private.keyword(), "私有");
        assert_eq!(Visibility::default(), Visibility::File);
    }

    #[test]
    fn 可见性语句包裹内部声明() {
        let inner = Stmt::VarDecl(VarDecl {
            pos: pos(),
            is_const: false,
            is_explicit: true,
            decl_type: DeclType::any(),
            name: "版本".to_string(),
            value: Some(Expr::Str(crate::expr::StringLit {
                pos: pos(),
                value: "1.0".to_string(),
            })),
            visibility: Visibility::Public,
        });
        let v = VisibilityStmt {
            pos: pos(),
            name: "版本".to_string(),
            visibility: Visibility::Public,
            inner: Box::new(inner),
        };
        assert_eq!(v.to_string(), "公开 变量 版本 = \"1.0\"");
    }

    #[test]
    fn 模型定义显示字段与嵌入() {
        let c = ClassStmt {
            pos: pos(),
            name: "矩形".to_string(),
            fields: vec![FieldDecl {
                pos: pos(),
                name: "宽".to_string(),
                type_: DeclType::named("整数"),
                visibility: Visibility::File,
            }],
            embeds: vec!["人".to_string()],
            tags: vec![],
            visibility: Visibility::File,
        };
        assert_eq!(c.to_string(), "类 矩形 变量 整数 宽 嵌入 人 结束");
    }

    #[test]
    fn 方法与接口与类型定义显示() {
        let m = MethodDef {
            pos: pos(),
            class_name: "矩形".to_string(),
            method_name: "面积".to_string(),
            function: FunctionLit {
                pos: pos(),
                params: vec![],
                body: empty_block(),
                name: String::new(),
                name_segments: vec![],
                connectors: vec![],
                group_sizes: vec![],
                signature: String::new(),
                return_types: vec![],
                tags: vec![],
            },
            primitive: false,
            receiver_name: String::new(),
            visibility: Visibility::File,
        };
        assert_eq!(m.to_string(), "定义 矩形 的 方法 面积 函数()  结束");

        let i = InterfaceStmt {
            pos: pos(),
            name: "说话者".to_string(),
            embeds: vec![],
            methods: vec![],
            visibility: Visibility::File,
        };
        assert_eq!(i.to_string(), "接口 说话者");

        let t = TypeDefStmt {
            pos: pos(),
            name: "状态".to_string(),
            base_type: DeclType::named("整数"),
            visibility: Visibility::File,
        };
        assert_eq!(t.to_string(), "类型 状态");
    }

    #[test]
    fn 字段赋值与下标赋值显示() {
        let f = FieldAssignExpr {
            pos: pos(),
            object: ident("自己"),
            name: "宽".to_string(),
            value: int(3),
        };
        assert_eq!(f.to_string(), "自己.宽 = 3");

        let i = IndexAssignStmt {
            pos: pos(),
            target: ident("数字"),
            index: int(0),
            value: int(9),
        };
        assert_eq!(i.to_string(), "数字[0] = 9");
    }

    #[test]
    fn 语句可取位置() {
        let p = Position::new(5, 2);
        let s = Stmt::Break(BreakStmt { pos: p });
        assert_eq!(s.pos(), p);
    }
}
