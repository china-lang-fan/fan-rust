//! 静态收集到的符号与符号种类。

use fan_ast::Position;

/// 符号种类，与 Go 版 `analysis.SymbolKind` 对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    MixfixFunction,
    Class,
    Interface,
    Type,
    Field,
    Method,
    Variable,
    Constant,
    Parameter,
    Module,
}

/// 一个静态符号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 类型或签名的可读描述。
    pub detail: String,
    /// 文档注释（紧贴声明上方的连续注释块）。
    pub doc: String,
    pub pos: Position,
    pub end_pos: Position,
    /// 字段/方法所属模型，或参数所属函数；顶层符号为空。
    pub owner: String,
    /// 子符号（模型字段、接口方法、函数参数）。
    pub children: Vec<Symbol>,
    /// 模型嵌入的类型名（按声明顺序），仅模型符号使用。
    pub embeds: Vec<String>,
}

impl Symbol {
    pub fn new(name: impl Into<String>, kind: SymbolKind, pos: Position) -> Self {
        Symbol {
            name: name.into(),
            kind,
            detail: String::new(),
            doc: String::new(),
            pos,
            end_pos: pos,
            owner: String::new(),
            children: Vec::new(),
            embeds: Vec::new(),
        }
    }
}
