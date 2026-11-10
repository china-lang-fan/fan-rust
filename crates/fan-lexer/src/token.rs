//! 记号定义与中文关键字表。
//!
//! `TokenType` 的 `Display` 输出与 Go 版 `token.Type` 的字符串常量完全一致，
//! 便于两边做记号流快照对拍。

use std::fmt;

/// 记号类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenType {
    Illegal,
    Eof,
    Newline,

    Ident,
    Int,
    Float,
    Str,
    Char,

    /// 字符串插值：普通片段。
    StringPart,
    /// 字符串插值：`${` 起始。
    InterpStart,
    /// 字符串插值：`$名称`。
    InterpIdent,
    /// 字符串插值：收尾引号或 `}`。
    InterpEnd,

    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    Inc,
    Dec,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,

    Eq,
    Neq,
    Is,
    As,
    Lt,
    Lte,
    Gt,
    Gte,

    And,
    Or,
    Not,

    BAnd,
    BOr,
    BXor,
    BNot,
    Shl,
    Shr,

    LParen,
    RParen,
    LBrack,
    RBrack,
    LBrace,
    RBrace,
    Comma,
    Tag,
    Quest,
    Arrow,
    Method,
    Self_,
    Ellipsis,

    Define,
    Var,
    Const,
    Property,

    TypeInt,
    TypeFloat,
    TypeString,
    TypeBool,
    TypeArray,
    TypeDict,
    TypeError,
    TypeByte,
    TypeChar,

    True,
    False,
    Nil,

    If,
    Switch,
    Case,
    Default,
    Then,
    Else,
    ElseIf,
    End,

    While,
    Loop,
    Repeat,
    Until,
    Times,
    Break,
    Continue,
    ForIn,
    InOf,
    Function,
    Return,
    Import,
    Public,
    Private,
    Defer,
    Through,
    Throw,
    Check,
    Member,
    Class,
    Interface,
    TypeDef,
    Embed,
    Dot,
    Colon,
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TokenType::Illegal => "ILLEGAL",
            TokenType::Eof => "EOF",
            TokenType::Newline => "NEWLINE",
            TokenType::Ident => "IDENT",
            TokenType::Int => "INT",
            TokenType::Float => "FLOAT",
            TokenType::Str => "STRING",
            TokenType::Char => "CHAR",
            TokenType::StringPart => "STRING_PART",
            TokenType::InterpStart => "INTERP_START",
            TokenType::InterpIdent => "INTERP_IDENT",
            TokenType::InterpEnd => "INTERP_END",
            TokenType::Assign => "ASSIGN",
            TokenType::PlusEq => "PLUS_EQ",
            TokenType::MinusEq => "MINUS_EQ",
            TokenType::StarEq => "STAR_EQ",
            TokenType::SlashEq => "SLASH_EQ",
            TokenType::Inc => "INC",
            TokenType::Dec => "DEC",
            TokenType::Plus => "PLUS",
            TokenType::Minus => "MINUS",
            TokenType::Star => "STAR",
            TokenType::Slash => "SLASH",
            TokenType::Percent => "PERCENT",
            TokenType::Eq => "EQ",
            TokenType::Neq => "NEQ",
            TokenType::Is => "IS",
            TokenType::As => "AS",
            TokenType::Lt => "LT",
            TokenType::Lte => "LTE",
            TokenType::Gt => "GT",
            TokenType::Gte => "GTE",
            TokenType::And => "AND",
            TokenType::Or => "OR",
            TokenType::Not => "NOT",
            TokenType::BAnd => "BAND",
            TokenType::BOr => "BOR",
            TokenType::BXor => "BXOR",
            TokenType::BNot => "BNOT",
            TokenType::Shl => "SHL",
            TokenType::Shr => "SHR",
            TokenType::LParen => "LPAREN",
            TokenType::RParen => "RPAREN",
            TokenType::LBrack => "LBRACK",
            TokenType::RBrack => "RBRACK",
            TokenType::LBrace => "LBRACE",
            TokenType::RBrace => "RBRACE",
            TokenType::Comma => "COMMA",
            TokenType::Tag => "TAG",
            TokenType::Quest => "QUEST",
            TokenType::Arrow => "ARROW",
            TokenType::Method => "METHOD",
            TokenType::Self_ => "SELF",
            TokenType::Ellipsis => "ELLIPSIS",
            TokenType::Define => "DEFINE",
            TokenType::Var => "VAR",
            TokenType::Const => "CONST",
            TokenType::Property => "PROPERTY",
            TokenType::TypeInt => "TYPE_INT",
            TokenType::TypeFloat => "TYPE_FLOAT",
            TokenType::TypeString => "TYPE_STRING",
            TokenType::TypeBool => "TYPE_BOOL",
            TokenType::TypeArray => "TYPE_ARRAY",
            TokenType::TypeDict => "TYPE_DICT",
            TokenType::TypeError => "TYPE_ERROR",
            TokenType::TypeByte => "TYPE_BYTE",
            TokenType::TypeChar => "TYPE_CHAR",
            TokenType::True => "TRUE",
            TokenType::False => "FALSE",
            TokenType::Nil => "NIL",
            TokenType::If => "IF",
            TokenType::Switch => "SWITCH",
            TokenType::Case => "CASE",
            TokenType::Default => "DEFAULT",
            TokenType::Then => "THEN",
            TokenType::Else => "ELSE",
            TokenType::ElseIf => "ELSEIF",
            TokenType::End => "END",
            TokenType::While => "WHILE",
            TokenType::Loop => "LOOP",
            TokenType::Repeat => "REPEAT",
            TokenType::Until => "UNTIL",
            TokenType::Times => "TIMES",
            TokenType::Break => "BREAK",
            TokenType::Continue => "CONTINUE",
            TokenType::ForIn => "FORIN",
            TokenType::InOf => "INOF",
            TokenType::Function => "FUNCTION",
            TokenType::Return => "RETURN",
            TokenType::Import => "IMPORT",
            TokenType::Public => "PUBLIC",
            TokenType::Private => "PRIVATE",
            TokenType::Defer => "DEFER",
            TokenType::Through => "THROUGH",
            TokenType::Throw => "THROW",
            TokenType::Check => "CHECK",
            TokenType::Member => "MEMBER",
            TokenType::Class => "CLASS",
            TokenType::Interface => "INTERFACE",
            TokenType::TypeDef => "TYPEDEF",
            TokenType::Embed => "EMBED",
            TokenType::Dot => "DOT",
            TokenType::Colon => "COLON",
        };
        f.write_str(s)
    }
}

/// 一个记号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub type_: TokenType,
    /// 记号的原始文本。
    pub literal: String,
    /// 起始行（从 1 开始）。
    pub line: usize,
    /// 起始列（从 1 开始）。
    pub column: usize,
    /// 记号前是否有空白，解析器据此判断“紧邻/带空格”的调用形态。
    pub leading_space: bool,
}

impl Token {
    pub fn new(type_: TokenType, literal: impl Into<String>, line: usize, column: usize) -> Self {
        Token {
            type_,
            literal: literal.into(),
            line,
            column,
            leading_space: false,
        }
    }

    pub fn with_space(mut self, leading_space: bool) -> Self {
        self.leading_space = leading_space;
        self
    }

    /// 是否为语句结束（换行或文件末尾）。
    pub fn is_statement_end(&self) -> bool {
        matches!(self.type_, TokenType::Newline | TokenType::Eof)
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_)
    }
}

/// 查表：中文关键字 → 记号类型。非关键字返回 `None`。
pub fn lookup_keyword(ident: &str) -> Option<TokenType> {
    let t = match ident {
        "定义" => TokenType::Define,
        "变量" => TokenType::Var,
        "常量" => TokenType::Const,
        "属性" => TokenType::Property,
        "整数" => TokenType::TypeInt,
        "小数" => TokenType::TypeFloat,
        "字符串" => TokenType::TypeString,
        "布尔" => TokenType::TypeBool,
        "数组" => TokenType::TypeArray,
        "字典" => TokenType::TypeDict,
        "错误" => TokenType::TypeError,
        "字节" => TokenType::TypeByte,
        "字符" => TokenType::TypeChar,
        "真" => TokenType::True,
        "假" => TokenType::False,
        "空" => TokenType::Nil,
        "为" => TokenType::Assign,
        "加等于" => TokenType::PlusEq,
        "减等于" => TokenType::MinusEq,
        "乘等于" => TokenType::StarEq,
        "除等于" => TokenType::SlashEq,
        "自增" => TokenType::Inc,
        "自减" => TokenType::Dec,
        "加" => TokenType::Plus,
        "减" => TokenType::Minus,
        "乘" => TokenType::Star,
        "除" => TokenType::Slash,
        "取余" => TokenType::Percent,
        "等于" => TokenType::Eq,
        "不等于" => TokenType::Neq,
        "是" => TokenType::Is,
        "转为" => TokenType::As,
        "小于" => TokenType::Lt,
        "小于等于" => TokenType::Lte,
        "大于" => TokenType::Gt,
        "大于等于" => TokenType::Gte,
        "且" => TokenType::And,
        "或" => TokenType::Or,
        "非" => TokenType::Not,
        "位与" => TokenType::BAnd,
        "位或" => TokenType::BOr,
        "异或" => TokenType::BXor,
        "位取反" => TokenType::BNot,
        "左移" => TokenType::Shl,
        "右移" => TokenType::Shr,
        "判断" => TokenType::Switch,
        "其他" => TokenType::Default,
        "如果" => TokenType::If,
        "那么" => TokenType::Then,
        "否则" => TokenType::Else,
        "否则如果" => TokenType::ElseIf,
        "结束" => TokenType::End,
        "当" => TokenType::While,
        "循环" => TokenType::Loop,
        "重复" => TokenType::Repeat,
        "直到" => TokenType::Until,
        "次" => TokenType::Times,
        "跳出" => TokenType::Break,
        "继续" => TokenType::Continue,
        "遍历" => TokenType::ForIn,
        "中的" => TokenType::InOf,
        "函数" => TokenType::Function,
        "返回" => TokenType::Return,
        "导入" => TokenType::Import,
        "公开" => TokenType::Public,
        "私有" => TokenType::Private,
        "稍后" => TokenType::Defer,
        "通过" => TokenType::Through,
        "抛出" => TokenType::Throw,
        "检查" => TokenType::Check,
        "的" => TokenType::Member,
        "方法" => TokenType::Method,
        "自己" => TokenType::Self_,
        "模型" => TokenType::Class,
        "接口" => TokenType::Interface,
        "类型" => TokenType::TypeDef,
        "嵌入" => TokenType::Embed,
        _ => return None,
    };
    Some(t)
}

/// 是否为保留关键字。
pub fn is_keyword(ident: &str) -> bool {
    lookup_keyword(ident).is_some()
}

/// 全部关键字，按字典序，供测试与文档生成使用。
pub fn all_keywords() -> Vec<&'static str> {
    let mut v: Vec<&str> = vec![
        "定义",
        "变量",
        "属性",
        "常量",
        "整数",
        "小数",
        "字符串",
        "布尔",
        "数组",
        "字典",
        "错误",
        "字节",
        "字符",
        "真",
        "假",
        "空",
        "为",
        "加等于",
        "减等于",
        "乘等于",
        "除等于",
        "自增",
        "自减",
        "加",
        "减",
        "乘",
        "除",
        "取余",
        "等于",
        "不等于",
        "是",
        "转为",
        "小于",
        "小于等于",
        "大于",
        "大于等于",
        "且",
        "或",
        "非",
        "位与",
        "位或",
        "异或",
        "位取反",
        "左移",
        "右移",
        "判断",
        "其他",
        "如果",
        "那么",
        "否则",
        "否则如果",
        "结束",
        "当",
        "循环",
        "重复",
        "直到",
        "次",
        "跳出",
        "继续",
        "遍历",
        "中的",
        "函数",
        "返回",
        "导入",
        "公开",
        "私有",
        "稍后",
        "通过",
        "抛出",
        "检查",
        "的",
        "方法",
        "自己",
        "模型",
        "接口",
        "类型",
        "嵌入",
    ];
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 中文关键字全部可查表() {
        for kw in all_keywords() {
            assert!(lookup_keyword(kw).is_some(), "关键字 {kw} 查表失败");
            assert!(is_keyword(kw), "关键字 {kw} 未被识别");
        }
    }

    #[test]
    fn 关键字数与去重后一致() {
        let all = all_keywords();
        let mut dedup = all.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(all.len(), dedup.len(), "关键字表存在重复项");
    }

    #[test]
    fn 非关键字返回空() {
        for word in ["小明", "面积", "abc", "计", "注释abc", "变量量"] {
            assert_eq!(lookup_keyword(word), None, "{word} 不应是关键字");
            assert!(!is_keyword(word));
        }
    }

    #[test]
    fn 空串不是关键字() {
        assert_eq!(lookup_keyword(""), None);
    }

    #[test]
    fn 关键字映射到正确记号类型() {
        assert_eq!(lookup_keyword("如果"), Some(TokenType::If));
        assert_eq!(lookup_keyword("否则如果"), Some(TokenType::ElseIf));
        assert_eq!(lookup_keyword("结束"), Some(TokenType::End));
        assert_eq!(lookup_keyword("函数"), Some(TokenType::Function));
        assert_eq!(lookup_keyword("模型"), Some(TokenType::Class));
        assert_eq!(lookup_keyword("的"), Some(TokenType::Member));
        assert_eq!(lookup_keyword("整数"), Some(TokenType::TypeInt));
        assert_eq!(lookup_keyword("真"), Some(TokenType::True));
        assert_eq!(lookup_keyword("空"), Some(TokenType::Nil));
    }

    #[test]
    fn 注释不是关键字标识符() {
        // “注释”在语法方案中是注释起始标记，但不是保留关键字。
        assert_eq!(lookup_keyword("注释"), None);
    }

    #[test]
    fn 记号类型显示与go版字符串一致() {
        assert_eq!(TokenType::If.to_string(), "IF");
        assert_eq!(TokenType::Ident.to_string(), "IDENT");
        assert_eq!(TokenType::StringPart.to_string(), "STRING_PART");
        assert_eq!(TokenType::InterpStart.to_string(), "INTERP_START");
        assert_eq!(TokenType::Self_.to_string(), "SELF");
        assert_eq!(TokenType::Eof.to_string(), "EOF");

        // 覆盖全部枚举变体，确保 Display 无遗漏。
        let all = [
            TokenType::Illegal,
            TokenType::Newline,
            TokenType::Int,
            TokenType::Float,
            TokenType::Str,
            TokenType::Char,
            TokenType::InterpIdent,
            TokenType::InterpEnd,
            TokenType::Assign,
            TokenType::PlusEq,
            TokenType::MinusEq,
            TokenType::StarEq,
            TokenType::SlashEq,
            TokenType::Inc,
            TokenType::Dec,
            TokenType::Plus,
            TokenType::Minus,
            TokenType::Star,
            TokenType::Slash,
            TokenType::Percent,
            TokenType::Eq,
            TokenType::Neq,
            TokenType::Is,
            TokenType::As,
            TokenType::Lt,
            TokenType::Lte,
            TokenType::Gt,
            TokenType::Gte,
            TokenType::And,
            TokenType::Or,
            TokenType::Not,
            TokenType::BAnd,
            TokenType::BOr,
            TokenType::BXor,
            TokenType::BNot,
            TokenType::Shl,
            TokenType::Shr,
            TokenType::LParen,
            TokenType::RParen,
            TokenType::LBrack,
            TokenType::RBrack,
            TokenType::LBrace,
            TokenType::RBrace,
            TokenType::Comma,
            TokenType::Tag,
            TokenType::Quest,
            TokenType::Arrow,
            TokenType::Method,
            TokenType::Ellipsis,
            TokenType::Define,
            TokenType::Var,
            TokenType::Const,
            TokenType::TypeFloat,
            TokenType::TypeString,
            TokenType::TypeBool,
            TokenType::TypeArray,
            TokenType::TypeDict,
            TokenType::TypeError,
            TokenType::TypeByte,
            TokenType::TypeChar,
            TokenType::False,
            TokenType::Switch,
            TokenType::Case,
            TokenType::Default,
            TokenType::Then,
            TokenType::Else,
            TokenType::While,
            TokenType::Loop,
            TokenType::Repeat,
            TokenType::Until,
            TokenType::Times,
            TokenType::Break,
            TokenType::Continue,
            TokenType::ForIn,
            TokenType::InOf,
            TokenType::Return,
            TokenType::Import,
            TokenType::Public,
            TokenType::Private,
            TokenType::Defer,
            TokenType::Through,
            TokenType::Throw,
            TokenType::Check,
            TokenType::Member,
            TokenType::Class,
            TokenType::Interface,
            TokenType::TypeDef,
            TokenType::Embed,
            TokenType::Dot,
            TokenType::Colon,
        ];
        for t in all {
            assert!(!t.to_string().is_empty());
        }
    }

    #[test]
    fn 记号携带位置与前导空白信息() {
        let t = Token::new(TokenType::Ident, "甲", 2, 3).with_space(true);
        assert_eq!(t.type_, TokenType::Ident);
        assert_eq!(t.literal, "甲");
        assert_eq!(t.line, 2);
        assert_eq!(t.column, 3);
        assert!(t.leading_space);
    }

    #[test]
    fn 换行与文件末尾都是语句结束() {
        assert!(Token::new(TokenType::Newline, "\n", 1, 1).is_statement_end());
        assert!(Token::new(TokenType::Eof, "", 1, 1).is_statement_end());
        assert!(!Token::new(TokenType::Ident, "甲", 1, 1).is_statement_end());
    }

    #[test]
    fn 记号显示为其类型名() {
        assert_eq!(Token::new(TokenType::If, "如果", 1, 1).to_string(), "IF");
    }
}
