//! 表达式节点。
//!
//! 与 Go 版 `internal/ast` 的表达式节点一一对应，字段语义保持一致，
//! `Display` 实现复刻 Go 版 `String()` 的输出，便于与 Go 版做快照对拍。

use crate::decl_type::DeclType;
use crate::position::Position;
use std::fmt;

/// 表达式节点。
///
/// 用枚举而非 trait 对象：求值器与类型检查器可以靠 `match` 的穷尽性
/// 在编译期保证所有节点都被处理。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Integer(IntegerLit),
    Float(FloatLit),
    Str(StringLit),
    InterpStr(InterpString),
    Char(CharLit),
    Bool(BoolLit),
    Nil(NilLit),
    Ident(Ident),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    Ternary(TernaryExpr),
    Update(UpdateExpr),
    MaybeCall(MaybeCallExpr),
    Call(CallExpr),
    Index(IndexExpr),
    Slice(SliceExpr),
    Member(MemberExpr),
    ImplicitReceiverCall(ImplicitReceiverCallExpr),
    Array(ArrayLit),
    Dict(DictLit),
    Spread(SpreadExpr),
    Tuple(TupleExpr),
    Function(FunctionLit),
    Composite(CompositeLit),
    TypeTest(TypeTestExpr),
    TypeCast(TypeCastExpr),
    Check(CheckExpr),
    Through(ThroughExpr),
    Tag(TagExpr),
}

impl Expr {
    /// 节点在源码中的起始位置。
    pub fn pos(&self) -> Position {
        match self {
            Expr::Integer(n) => n.pos,
            Expr::Float(n) => n.pos,
            Expr::Str(n) => n.pos,
            Expr::InterpStr(n) => n.pos,
            Expr::Char(n) => n.pos,
            Expr::Bool(n) => n.pos,
            Expr::Nil(n) => n.pos,
            Expr::Ident(n) => n.pos,
            Expr::Unary(n) => n.pos,
            Expr::Binary(n) => n.pos,
            Expr::Ternary(n) => n.pos,
            Expr::Update(n) => n.pos,
            Expr::MaybeCall(n) => n.pos,
            Expr::Call(n) => n.pos,
            Expr::Index(n) => n.pos,
            Expr::Slice(n) => n.pos,
            Expr::Member(n) => n.pos,
            Expr::ImplicitReceiverCall(n) => n.pos,
            Expr::Array(n) => n.pos,
            Expr::Dict(n) => n.pos,
            Expr::Spread(n) => n.pos,
            Expr::Tuple(n) => n.pos,
            Expr::Function(n) => n.pos,
            Expr::Composite(n) => n.pos,
            Expr::TypeTest(n) => n.pos,
            Expr::TypeCast(n) => n.pos,
            Expr::Check(n) => n.pos,
            Expr::Through(n) => n.pos,
            Expr::Tag(n) => n.pos,
        }
    }

    /// 标识符节点的名字（非标识符返回 `None`）。
    pub fn as_ident_name(&self) -> Option<&str> {
        match self {
            Expr::Ident(n) => Some(&n.name),
            _ => None,
        }
    }

    pub fn is_ident(&self) -> bool {
        matches!(self, Expr::Ident(_))
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Integer(n) => n.fmt(f),
            Expr::Float(n) => n.fmt(f),
            Expr::Str(n) => n.fmt(f),
            Expr::InterpStr(n) => n.fmt(f),
            Expr::Char(n) => n.fmt(f),
            Expr::Bool(n) => n.fmt(f),
            Expr::Nil(n) => n.fmt(f),
            Expr::Ident(n) => n.fmt(f),
            Expr::Unary(n) => n.fmt(f),
            Expr::Binary(n) => n.fmt(f),
            Expr::Ternary(n) => n.fmt(f),
            Expr::Update(n) => n.fmt(f),
            Expr::MaybeCall(n) => n.fmt(f),
            Expr::Call(n) => n.fmt(f),
            Expr::Index(n) => n.fmt(f),
            Expr::Slice(n) => n.fmt(f),
            Expr::Member(n) => n.fmt(f),
            Expr::ImplicitReceiverCall(n) => n.fmt(f),
            Expr::Array(n) => n.fmt(f),
            Expr::Dict(n) => n.fmt(f),
            Expr::Spread(n) => n.fmt(f),
            Expr::Tuple(n) => n.fmt(f),
            Expr::Function(n) => n.fmt(f),
            Expr::Composite(n) => n.fmt(f),
            Expr::TypeTest(n) => n.fmt(f),
            Expr::TypeCast(n) => n.fmt(f),
            Expr::Check(n) => n.fmt(f),
            Expr::Through(n) => n.fmt(f),
            Expr::Tag(n) => n.fmt(f),
        }
    }
}

// ---------------------------------------------------------------- 字面量

/// 整数字面量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegerLit {
    pub pos: Position,
    pub value: i64,
}

impl fmt::Display for IntegerLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

/// 小数字面量。
#[derive(Debug, Clone, PartialEq)]
pub struct FloatLit {
    pub pos: Position,
    pub value: f64,
}

impl fmt::Display for FloatLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_f64(self.value))
    }
}

/// 复刻 Go 的 `%g` 格式：去掉尾随零，必要时使用科学计数法。
///
/// 该格式在 AST 字面量显示与运行时值显示（`Inspect`）中必须一致，
/// 因此在此处统一实现，供 `fan-object` 等下游 crate 复用。
pub fn format_f64(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    if v.is_nan() {
        return "NaN".to_string();
    }
    let abs = v.abs();
    // Go 的 %g 在指数 < -4 或 >= 精度时使用科学计数法。
    let exp = abs.log10().floor();
    if !(-4.0..21.0).contains(&exp) {
        let mantissa = abs / 10f64.powf(exp);
        let sign = if v < 0.0 { "-" } else { "" };
        return format!("{}{}e{:02}", sign, trim_float(mantissa), exp as i64);
    }
    trim_float(v)
}

fn trim_float(v: f64) -> String {
    let s = format!("{:.}", v);
    let s = if v.fract() == 0.0 {
        format!("{:.0}", v)
    } else {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// 字符串字面量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLit {
    pub pos: Position,
    pub value: String,
}

impl fmt::Display for StringLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

/// 插值字符串：`Parts` 与 `Exprs` 交错出现。
///
/// 形如 `"a$x${y+1}b"` 表示 `Parts=["a","","b"]`、`Exprs=[x, y+1]`。
#[derive(Debug, Clone, PartialEq)]
pub struct InterpString {
    pub pos: Position,
    pub parts: Vec<String>,
    pub exprs: Vec<Expr>,
}

impl fmt::Display for InterpString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("插值字符串")
    }
}

/// 字符字面量（单个 Unicode 字符）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharLit {
    pub pos: Position,
    pub value: char,
}

impl fmt::Display for CharLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.value)
    }
}

/// 布尔字面量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoolLit {
    pub pos: Position,
    pub value: bool,
}

impl fmt::Display for BoolLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.value { "真" } else { "假" })
    }
}

/// 空值字面量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NilLit {
    pub pos: Position,
}

impl fmt::Display for NilLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("空")
    }
}

/// 标识符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub pos: Position,
    pub name: String,
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

// ---------------------------------------------------------------- 运算

/// 一元运算：`-x`、`非 x`、`位取反 x` 等。
#[derive(Debug, Clone, PartialEq)]
pub struct UnaryExpr {
    pub pos: Position,
    pub op: String,
    pub right: Box<Expr>,
}

impl fmt::Display for UnaryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.op == "-" || self.op == "!" {
            ""
        } else {
            " "
        };
        write!(f, "({}{}{})", self.op, sep, self.right)
    }
}

/// 二元运算。
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryExpr {
    pub pos: Position,
    pub op: String,
    pub left: Box<Expr>,
    pub right: Box<Expr>,
}

impl fmt::Display for BinaryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {} {})", self.left, self.op, self.right)
    }
}

/// 三元表达式 `条件 ? 真值 : 假值`。
#[derive(Debug, Clone, PartialEq)]
pub struct TernaryExpr {
    pub pos: Position,
    pub cond: Box<Expr>,
    pub then: Box<Expr>,
    pub else_: Box<Expr>,
}

impl fmt::Display for TernaryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} ? {} : {})", self.cond, self.then, self.else_)
    }
}

/// 自增/自减表达式。`prefix` 为真表示前缀形态 `++x`。
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateExpr {
    pub pos: Position,
    pub target: Box<Expr>,
    pub op: String,
    pub prefix: bool,
}

impl fmt::Display for UpdateExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.prefix {
            write!(f, "({}{})", self.op, self.target)
        } else {
            write!(f, "({}{})", self.target, self.op)
        }
    }
}

/// 可能是调用的表达式：用于无括号调用场景，由解析器决定是否需要调用。
#[derive(Debug, Clone, PartialEq)]
pub struct MaybeCallExpr {
    pub pos: Position,
    pub callee: Box<Expr>,
    /// 无参时是否自动调用（函数或方法无参可省略括号）。
    pub auto_call: bool,
}

impl fmt::Display for MaybeCallExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.callee.fmt(f)
    }
}

/// 函数调用 `f(a, b)`。
#[derive(Debug, Clone, PartialEq)]
pub struct CallExpr {
    pub pos: Position,
    pub callee: Box<Expr>,
    pub args: Vec<Expr>,
    /// 分段（mixfix）函数调用的根段名，由 parser 在识别到多段调用时填充；
    /// 普通调用为 `None`。
    pub mixfix_root: Option<String>,
}

impl fmt::Display for CallExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.args.iter().map(|a| a.to_string()).collect();
        write!(f, "{}({})", self.callee, parts.join(", "))
    }
}

/// 下标访问 `a[i]`。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexExpr {
    pub pos: Position,
    pub target: Box<Expr>,
    pub index: Box<Expr>,
}

impl fmt::Display for IndexExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.target, self.index)
    }
}

/// 切片表达式 `目标[低:高]`，`low`/`high` 为 `None` 表示该侧省略。
#[derive(Debug, Clone, PartialEq)]
pub struct SliceExpr {
    pub pos: Position,
    pub target: Box<Expr>,
    pub low: Option<Box<Expr>>,
    pub high: Option<Box<Expr>>,
    pub colon_pos: Position,
}

impl fmt::Display for SliceExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let low = self.low.as_ref().map(|e| e.to_string()).unwrap_or_default();
        let high = self
            .high
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_default();
        write!(f, "{}[{}:{}]", self.target, low, high)
    }
}

/// 成员访问 `obj 的 name` / `obj.name`。
#[derive(Debug, Clone, PartialEq)]
pub struct MemberExpr {
    pub pos: Position,
    pub object: Box<Expr>,
    pub name: String,
    /// 连接词（`的` 或 `.`）。
    pub connector: String,
    /// 对象与连接词之间是否有空白。多段函数调用要求各段紧邻，
    /// 带空格时按“普通调用返回值的后续成员”处理。
    pub spaced: bool,
}

impl fmt::Display for MemberExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} 的 {}", self.object, self.name)
    }
}

/// 接收者调用 `名单 追加 "张三"`，等价于 `追加(名单, "张三")`。
#[derive(Debug, Clone, PartialEq)]
pub struct ImplicitReceiverCallExpr {
    pub pos: Position,
    pub receiver: Box<Expr>,
    pub name: String,
    pub args: Vec<Expr>,
    pub connector: String,
    /// 本段与前一段之间是否存在空白；多段函数要求各段紧邻。
    pub spaced: bool,
}

impl fmt::Display for ImplicitReceiverCallExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.args.iter().map(|a| a.to_string()).collect();
        write!(f, "{} {}({})", self.receiver, self.name, parts.join(", "))
    }
}

// ---------------------------------------------------------------- 复合字面量

/// 数组字面量 `[1, 2, 3]`。
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayLit {
    pub pos: Position,
    pub elements: Vec<Expr>,
}

impl fmt::Display for ArrayLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.elements.iter().map(|e| e.to_string()).collect();
        write!(f, "[{}]", parts.join(", "))
    }
}

/// 字典字面量中的一个键值对。`spread` 为真时表示 `{...d}` 展开，仅 `value` 有效。
#[derive(Debug, Clone, PartialEq)]
pub struct DictPair {
    pub key: Option<Expr>,
    pub value: Expr,
    pub spread: bool,
}

/// 字典字面量 `{键: 值, ...}`。
#[derive(Debug, Clone, PartialEq)]
pub struct DictLit {
    pub pos: Position,
    pub pairs: Vec<DictPair>,
}

impl fmt::Display for DictLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .pairs
            .iter()
            .map(|p| {
                if p.spread {
                    format!("...{}", p.value)
                } else {
                    let key = p.key.as_ref().map(|k| k.to_string()).unwrap_or_default();
                    format!("{}: {}", key, p.value)
                }
            })
            .collect();
        write!(f, "{{{}}}", parts.join(", "))
    }
}

/// 展开表达式 `...值`，用于数组字面量、调用实参与字典字面量。
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadExpr {
    pub pos: Position,
    pub value: Box<Expr>,
}

impl fmt::Display for SpreadExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "...{}", self.value)
    }
}

/// 多目标声明/赋值右侧的逗号分隔表达式序列，如 `a, b = 1, 2`。
#[derive(Debug, Clone, PartialEq)]
pub struct TupleExpr {
    pub pos: Position,
    pub elements: Vec<Expr>,
}

impl fmt::Display for TupleExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.elements.iter().map(|e| e.to_string()).collect();
        write!(f, "({})", parts.join(", "))
    }
}

/// 模型复合字面量 `矩形{宽: 3, 高: 4}`。
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeLit {
    pub pos: Position,
    /// 模型表达式，通常是标识符，跨模块时为成员表达式。
    pub model: Box<Expr>,
    pub fields: Vec<CompositeField>,
}

/// 复合字面量中的一个具名字段初始化项。
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeField {
    pub name: String,
    pub name_pos: Position,
    pub value: Option<Expr>,
}

impl fmt::Display for CompositeLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .fields
            .iter()
            .map(|fl| match &fl.value {
                Some(v) => format!("{}: {}", fl.name, v),
                None => fl.name.clone(),
            })
            .collect();
        write!(f, "{}{{{}}}", self.model, parts.join(", "))
    }
}

// ---------------------------------------------------------------- 函数

/// 函数参数。
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub type_: DeclType,
    /// 是否为可变参数 `名字 ...类型`。
    pub variadic: bool,
    /// 默认值表达式，在调用时求值。
    pub default: Option<Expr>,
}

/// 函数字面量（具名函数与匿名函数共用）。
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLit {
    pub pos: Position,
    pub params: Vec<Parameter>,
    pub body: BlockStmt,
    /// 函数名；匿名函数为空。
    pub name: String,
    /// 分段函数的各段名，如 `根(a)的段(b)`。
    pub name_segments: Vec<String>,
    /// 分段函数各段之间的连接词（通常是 `的`）。
    pub connectors: Vec<String>,
    /// 分段函数每段的参数个数。
    pub group_sizes: Vec<usize>,
    /// 分段函数的签名展示串。
    pub signature: String,
    pub return_types: Vec<DeclType>,
    pub tags: Vec<TagExpr>,
}

impl FunctionLit {
    /// 是否为分段函数（函数名由多段构成）。
    pub fn is_segmented(&self) -> bool {
        self.name_segments.len() > 1
    }
}

impl fmt::Display for FunctionLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.is_empty() {
            f.write_str("函数")?;
        } else {
            write!(f, "函数 {}", self.name)?;
        }
        f.write_str("(")?;
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            if p.variadic {
                if !p.type_.is_any() {
                    write!(f, "{} ", p.type_)?;
                }
                write!(f, "...{}", p.name)?;
            } else {
                f.write_str(&p.name)?;
                if !p.type_.is_any() {
                    write!(f, " {}", p.type_)?;
                }
                if let Some(d) = &p.default {
                    write!(f, " = {}", d)?;
                }
            }
        }
        f.write_str(") ")?;
        if !self.return_types.is_empty() {
            f.write_str("返回 ")?;
            for (i, dt) in self.return_types.iter().enumerate() {
                if i > 0 {
                    f.write_str("、")?;
                }
                if dt.is_any() {
                    f.write_str("任意")?;
                } else {
                    write!(f, "{}", dt)?;
                }
            }
            f.write_str(" ")?;
        }
        write!(f, "{}", self.body)?;
        f.write_str(" 结束")
    }
}

// ---------------------------------------------------------------- 类型操作

/// `值 是 类型名`，等价于 Java 的 instanceof。
#[derive(Debug, Clone, PartialEq)]
pub struct TypeTestExpr {
    pub pos: Position,
    pub object: Box<Expr>,
    pub type_name: String,
}

impl fmt::Display for TypeTestExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.type_name)
    }
}

/// `值 转为 类型名`，求值结果为 `(结果, 是否成功)`。
#[derive(Debug, Clone, PartialEq)]
pub struct TypeCastExpr {
    pub pos: Position,
    pub object: Box<Expr>,
    pub type_name: String,
}

impl fmt::Display for TypeCastExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.type_name)
    }
}

/// `检查 调用`：调用返回错误时立即从当前函数返回。
#[derive(Debug, Clone, PartialEq)]
pub struct CheckExpr {
    pub pos: Position,
    pub call: Box<Expr>,
}

impl fmt::Display for CheckExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "检查 {}", self.call)
    }
}

/// `通过 模块名 表达式`：在指定模块的公开作用域内求值。
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughExpr {
    pub pos: Position,
    pub module_name: String,
    pub expr: Box<Expr>,
}

impl fmt::Display for ThroughExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "通过 {} {}", self.module_name, self.expr)
    }
}

/// 标签表达式 `@标记(说明 = "示例")`。
#[derive(Debug, Clone, PartialEq)]
pub struct TagExpr {
    pub pos: Position,
    pub type_name: String,
    pub positional: Vec<Expr>,
    pub named: Vec<NamedTagArg>,
}

/// 标签的命名参数。
#[derive(Debug, Clone, PartialEq)]
pub struct NamedTagArg {
    pub name: String,
    pub value: Expr,
}

impl fmt::Display for TagExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.type_name)?;
        if self.positional.is_empty() && self.named.is_empty() {
            return Ok(());
        }
        f.write_str("(")?;
        let mut first = true;
        for e in &self.positional {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "{}", e)?;
            first = false;
        }
        for a in &self.named {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "{} = {}", a.name, a.value)?;
            first = false;
        }
        f.write_str(")")
    }
}

impl TagExpr {
    /// 标签的完整名字，形如 `@标记`（与 Go 版 `TagNames` 一致）。
    pub fn tag_name(&self) -> String {
        format!("@{}", self.type_name)
    }
}

use crate::stmt::BlockStmt;

// 测试中 3.14 只是普通小数，并非要近似 π
#[cfg_attr(test, allow(clippy::approx_constant))]
#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn 整数字面量显示为数字() {
        assert_eq!(int(42).to_string(), "42");
        assert_eq!(int(-7).to_string(), "-7");
    }

    #[test]
    fn 小数字面量去除尾随零() {
        assert_eq!(
            FloatLit {
                pos: pos(),
                value: 3.14
            }
            .to_string(),
            "3.14"
        );
        assert_eq!(
            FloatLit {
                pos: pos(),
                value: 2.0
            }
            .to_string(),
            "2"
        );
        assert_eq!(
            FloatLit {
                pos: pos(),
                value: 0.5
            }
            .to_string(),
            "0.5"
        );
        assert_eq!(
            FloatLit {
                pos: pos(),
                value: 0.0
            }
            .to_string(),
            "0"
        );
    }

    #[test]
    fn 小数在极大或极小时使用科学计数法() {
        // 对齐 Go 的 %g 行为。
        let s = FloatLit {
            pos: pos(),
            value: 1e30,
        }
        .to_string();
        assert!(s.contains('e'), "大数应为科学计数法，实际 {s}");
        let s = FloatLit {
            pos: pos(),
            value: 1e-5,
        }
        .to_string();
        assert!(s.contains('e'), "小数应为科学计数法，实际 {s}");
    }

    #[test]
    fn 字符串字面量带转义引号() {
        let e = Expr::Str(StringLit {
            pos: pos(),
            value: "你好".to_string(),
        });
        assert_eq!(e.to_string(), "\"你好\"");
    }

    #[test]
    fn 字符与布尔与空值显示() {
        assert_eq!(
            CharLit {
                pos: pos(),
                value: '中'
            }
            .to_string(),
            "'中'"
        );
        assert_eq!(
            BoolLit {
                pos: pos(),
                value: true
            }
            .to_string(),
            "真"
        );
        assert_eq!(
            BoolLit {
                pos: pos(),
                value: false
            }
            .to_string(),
            "假"
        );
        assert_eq!(NilLit { pos: pos() }.to_string(), "空");
    }

    #[test]
    fn 一元运算负号与非紧贴操作数() {
        let e = Expr::Unary(UnaryExpr {
            pos: pos(),
            op: "-".to_string(),
            right: Box::new(int(5)),
        });
        assert_eq!(e.to_string(), "(-5)");
        let e = Expr::Unary(UnaryExpr {
            pos: pos(),
            op: "非".to_string(),
            right: Box::new(ident("甲")),
        });
        assert_eq!(e.to_string(), "(非 甲)");
    }

    #[test]
    fn 二元运算显示左右与操作符() {
        let e = Expr::Binary(BinaryExpr {
            pos: pos(),
            op: "+".to_string(),
            left: Box::new(int(1)),
            right: Box::new(int(2)),
        });
        assert_eq!(e.to_string(), "(1 + 2)");
    }

    #[test]
    fn 三元表达式显示三个分支() {
        let e = Expr::Ternary(TernaryExpr {
            pos: pos(),
            cond: Box::new(ident("甲")),
            then: Box::new(int(1)),
            else_: Box::new(int(2)),
        });
        assert_eq!(e.to_string(), "(甲 ? 1 : 2)");
    }

    #[test]
    fn 自增前缀与后缀显示不同() {
        let prefix = Expr::Update(UpdateExpr {
            pos: pos(),
            target: Box::new(ident("计数")),
            op: "++".to_string(),
            prefix: true,
        });
        let postfix = Expr::Update(UpdateExpr {
            pos: pos(),
            target: Box::new(ident("计数")),
            op: "++".to_string(),
            prefix: false,
        });
        assert_eq!(prefix.to_string(), "(++计数)");
        assert_eq!(postfix.to_string(), "(计数++)");
    }

    #[test]
    fn 调用表达式显示被调者与实参() {
        let e = Expr::Call(CallExpr {
            pos: pos(),
            callee: Box::new(ident("打印")),
            args: vec![int(1), int(2)],
            mixfix_root: None,
        });
        assert_eq!(e.to_string(), "打印(1, 2)");
    }

    #[test]
    fn 下标与切片显示() {
        let idx = Expr::Index(IndexExpr {
            pos: pos(),
            target: Box::new(ident("数字")),
            index: Box::new(int(0)),
        });
        assert_eq!(idx.to_string(), "数字[0]");

        let slice = Expr::Slice(SliceExpr {
            pos: pos(),
            target: Box::new(ident("数字")),
            low: Some(Box::new(int(1))),
            high: None,
            colon_pos: pos(),
        });
        assert_eq!(slice.to_string(), "数字[1:]");
    }

    #[test]
    fn 数组与字典字面量显示() {
        let arr = Expr::Array(ArrayLit {
            pos: pos(),
            elements: vec![int(1), int(2)],
        });
        assert_eq!(arr.to_string(), "[1, 2]");

        let dict = Expr::Dict(DictLit {
            pos: pos(),
            pairs: vec![DictPair {
                key: Some(Expr::Str(StringLit {
                    pos: pos(),
                    value: "名".to_string(),
                })),
                value: int(1),
                spread: false,
            }],
        });
        assert_eq!(dict.to_string(), "{\"名\": 1}");
    }

    #[test]
    fn 字典展开项显示为三点前缀() {
        let dict = Expr::Dict(DictLit {
            pos: pos(),
            pairs: vec![DictPair {
                key: None,
                value: ident("d"),
                spread: true,
            }],
        });
        assert_eq!(dict.to_string(), "{...d}");
    }

    #[test]
    fn 展开与元组表达式显示() {
        let spread = Expr::Spread(SpreadExpr {
            pos: pos(),
            value: Box::new(ident("前")),
        });
        assert_eq!(spread.to_string(), "...前");

        let tuple = Expr::Tuple(TupleExpr {
            pos: pos(),
            elements: vec![int(1), int(2)],
        });
        assert_eq!(tuple.to_string(), "(1, 2)");
    }

    #[test]
    fn 函数字面量显示参数与返回类型() {
        let f = FunctionLit {
            pos: pos(),
            params: vec![
                Parameter {
                    name: "甲".to_string(),
                    type_: DeclType::named("整数"),
                    variadic: false,
                    default: None,
                },
                Parameter {
                    name: "乙".to_string(),
                    type_: DeclType::any(),
                    variadic: true,
                    default: None,
                },
            ],
            body: BlockStmt {
                pos: pos(),
                statements: vec![],
            },
            name: "求和".to_string(),
            name_segments: vec![],
            connectors: vec![],
            group_sizes: vec![],
            signature: String::new(),
            return_types: vec![DeclType::named("整数")],
            tags: vec![],
        };
        assert_eq!(f.to_string(), "函数 求和(甲 整数, ...乙) 返回 整数  结束");
    }

    #[test]
    fn 匿名函数无名字时不显示名字() {
        let f = FunctionLit {
            pos: pos(),
            params: vec![],
            body: BlockStmt {
                pos: pos(),
                statements: vec![],
            },
            name: String::new(),
            name_segments: vec![],
            connectors: vec![],
            group_sizes: vec![],
            signature: String::new(),
            return_types: vec![],
            tags: vec![],
        };
        assert_eq!(f.to_string(), "函数()  结束");
    }

    #[test]
    fn 默认参数显示为等号赋值() {
        let f = FunctionLit {
            pos: pos(),
            params: vec![Parameter {
                name: "称呼".to_string(),
                type_: DeclType::any(),
                variadic: false,
                default: Some(Expr::Str(StringLit {
                    pos: pos(),
                    value: "你好".to_string(),
                })),
            }],
            body: BlockStmt {
                pos: pos(),
                statements: vec![],
            },
            name: "问候".to_string(),
            name_segments: vec![],
            connectors: vec![],
            group_sizes: vec![],
            signature: String::new(),
            return_types: vec![],
            tags: vec![],
        };
        assert_eq!(f.to_string(), "函数 问候(称呼 = \"你好\")  结束");
    }

    #[test]
    fn 标签表达式显示命名与位置参数() {
        let tag = TagExpr {
            pos: pos(),
            type_name: "内建".to_string(),
            positional: vec![Expr::Str(StringLit {
                pos: pos(),
                value: "sqrt".to_string(),
            })],
            named: vec![NamedTagArg {
                name: "值".to_string(),
                value: int(1),
            }],
        };
        assert_eq!(tag.to_string(), "@内建(\"sqrt\", 值 = 1)");
        assert_eq!(tag.tag_name(), "@内建");
    }

    #[test]
    fn 无参标签只显示名字() {
        let tag = TagExpr {
            pos: pos(),
            type_name: "测试".to_string(),
            positional: vec![],
            named: vec![],
        };
        assert_eq!(tag.to_string(), "@测试");
    }

    #[test]
    fn 类型判断与转换显示类型名() {
        let t = TypeTestExpr {
            pos: pos(),
            object: Box::new(ident("谁")),
            type_name: "狗".to_string(),
        };
        assert_eq!(t.to_string(), "狗");
        let c = TypeCastExpr {
            pos: pos(),
            object: Box::new(ident("谁")),
            type_name: "整数".to_string(),
        };
        assert_eq!(c.to_string(), "整数");
    }

    #[test]
    fn 检查与通过表达式显示() {
        let chk = CheckExpr {
            pos: pos(),
            call: Box::new(ident("读取")),
        };
        assert_eq!(chk.to_string(), "检查 读取");

        let thru = ThroughExpr {
            pos: pos(),
            module_name: "m".to_string(),
            expr: Box::new(ident("根")),
        };
        assert_eq!(thru.to_string(), "通过 m 根");
    }

    #[test]
    fn 表达式可取位置与标识符名() {
        let p = Position::new(3, 4);
        let e = Expr::Ident(Ident {
            pos: p,
            name: "甲".to_string(),
        });
        assert_eq!(e.pos(), p);
        assert_eq!(e.as_ident_name(), Some("甲"));
        assert!(e.is_ident());
        assert_eq!(int(1).as_ident_name(), None);
    }

    #[test]
    fn 成员表达式显示对象与成员名() {
        let m = MemberExpr {
            pos: pos(),
            object: Box::new(ident("矩形")),
            name: "面积".to_string(),
            connector: "的".to_string(),
            spaced: true,
        };
        assert_eq!(m.to_string(), "矩形 的 面积");
    }

    #[test]
    fn 接收者调用显示接收者与方法名() {
        let c = ImplicitReceiverCallExpr {
            pos: pos(),
            receiver: Box::new(ident("名单")),
            name: "追加".to_string(),
            args: vec![Expr::Str(StringLit {
                pos: pos(),
                value: "张三".to_string(),
            })],
            connector: String::new(),
            spaced: true,
        };
        assert_eq!(c.to_string(), "名单 追加(\"张三\")");
    }

    #[test]
    fn 复合字面量显示模型与字段() {
        let lit = CompositeLit {
            pos: pos(),
            model: Box::new(ident("矩形")),
            fields: vec![CompositeField {
                name: "宽".to_string(),
                name_pos: pos(),
                value: Some(int(3)),
            }],
        };
        assert_eq!(lit.to_string(), "矩形{宽: 3}");
    }

    #[test]
    fn 分段函数识别为多段() {
        let mut f = FunctionLit {
            pos: pos(),
            params: vec![],
            body: BlockStmt {
                pos: pos(),
                statements: vec![],
            },
            name: "根".to_string(),
            name_segments: vec!["根".to_string(), "段".to_string()],
            connectors: vec!["的".to_string()],
            group_sizes: vec![1, 1],
            signature: String::new(),
            return_types: vec![],
            tags: vec![],
        };
        assert!(f.is_segmented());
        f.name_segments = vec!["根".to_string()];
        assert!(!f.is_segmented());
    }
}
