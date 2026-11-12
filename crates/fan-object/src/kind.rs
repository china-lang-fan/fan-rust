//! 对象种类。
//!
//! 与 Go 版 `object.Kind` 的中文字符串常量一一对应，`Display` 输出相同文本，
//! 保证错误信息的措辞在两版之间完全一致。

use std::fmt;

/// 凡语言值的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// 整数 `整数`
    Int,
    /// 小数 `小数`
    Float,
    /// 字符串 `字符串`
    Str,
    /// 布尔 `布尔`
    Bool,
    /// 字节 `字节`
    Byte,
    /// 字符 `字符`
    Char,
    /// 空值 `空`
    Nil,
    /// 数组 `数组`
    Array,
    /// 字典 `字典`
    Dict,
    /// 多元值 `多元值`
    Tuple,
    /// 错误 `错误`
    Error,
    /// 模块 `模块`
    Module,
    /// 函数 `函数`
    Func,
    /// 模型 `模型`
    Class,
    /// 模型实例 `实例`
    Instance,
    /// 内建函数 `内建`
    Builtin,
    /// 标签处理上下文 `标签上下文`
    TagContext,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Kind::Int => "整数",
            Kind::Float => "小数",
            Kind::Str => "字符串",
            Kind::Bool => "布尔",
            Kind::Byte => "字节",
            Kind::Char => "字符",
            Kind::Nil => "空",
            Kind::Array => "数组",
            Kind::Dict => "字典",
            Kind::Tuple => "多元值",
            Kind::Error => "错误",
            Kind::Module => "模块",
            Kind::Func => "函数",
            Kind::Class => "模型",
            Kind::Instance => "实例",
            Kind::Builtin => "内建",
            Kind::TagContext => "标签上下文",
        };
        f.write_str(s)
    }
}

impl Kind {
    /// 是否可作为字典键。
    ///
    /// 规则与 Go 版 `ValidDictKey` 一致：整数、小数、字符串、布尔、字节、字符。
    /// 命名类型取其底层类型判断。
    pub fn is_dict_key(&self) -> bool {
        matches!(
            self,
            Kind::Int | Kind::Float | Kind::Str | Kind::Bool | Kind::Byte | Kind::Char
        )
    }

    /// 是否为数值类型（整数或小数）。
    pub fn is_number(&self) -> bool {
        matches!(self, Kind::Int | Kind::Float | Kind::Byte)
    }
}

/// 把凡语言的内置类型名映射为内部种类标识。
///
/// 与 Go 版 `primitiveKindFromName` 一一对应，用于 `是`/`转为` 的类型判定。
pub fn primitive_kind_from_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "整数" => "integer",
        "小数" => "float",
        "字符串" => "string",
        "布尔" => "boolean",
        "字节" => "byte",
        "字符" => "char",
        "数组" => "array",
        "字典" => "dict",
        _ => return None,
    })
}

/// 取值的运行时类型标识（命名类型穿透到底层）。
pub fn runtime_type_name(value: &crate::Object) -> &'static str {
    use crate::Object;
    match value {
        Object::Named(n) => runtime_type_name(&n.value),
        Object::Int(_) => "integer",
        Object::Float(_) => "float",
        Object::Str(_) => "string",
        Object::Bool(_) => "boolean",
        Object::Byte(_) => "byte",
        Object::Char(_) => "char",
        Object::Nil => "nil",
        Object::Array(_) => "array",
        Object::Dict(_) => "dict",
        Object::Tuple(_) => "tuple",
        Object::Error(_) => "error",
        Object::Module(_) => "module",
        Object::Func(_) => "function",
        Object::Class(_) => "class",
        Object::Instance(_) => "instance",
        Object::Builtin(_) => "builtin",
        Object::TagContext(_) => "tag_context",
    }
}

/// 把基本类型标识转回凡语言类型名。
pub fn kind_display_name(kind: &str) -> &'static str {
    match kind {
        "integer" => "整数",
        "float" => "小数",
        "string" => "字符串",
        "boolean" => "布尔",
        "byte" => "字节",
        "char" => "字符",
        "array" => "数组",
        "dict" => "字典",
        _ => "未知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 种类显示为中文() {
        assert_eq!(Kind::Int.to_string(), "整数");
        assert_eq!(Kind::Float.to_string(), "小数");
        assert_eq!(Kind::Str.to_string(), "字符串");
        assert_eq!(Kind::Bool.to_string(), "布尔");
        assert_eq!(Kind::Byte.to_string(), "字节");
        assert_eq!(Kind::Char.to_string(), "字符");
        assert_eq!(Kind::Nil.to_string(), "空");
        assert_eq!(Kind::Array.to_string(), "数组");
        assert_eq!(Kind::Dict.to_string(), "字典");
        assert_eq!(Kind::Tuple.to_string(), "多元值");
        assert_eq!(Kind::Error.to_string(), "错误");
        assert_eq!(Kind::Module.to_string(), "模块");
        assert_eq!(Kind::Func.to_string(), "函数");
        assert_eq!(Kind::Class.to_string(), "模型");
        assert_eq!(Kind::Instance.to_string(), "实例");
        assert_eq!(Kind::Builtin.to_string(), "内建");
    }

    #[test]
    fn 字典键类型判定() {
        for k in [
            Kind::Int,
            Kind::Float,
            Kind::Str,
            Kind::Bool,
            Kind::Byte,
            Kind::Char,
        ] {
            assert!(k.is_dict_key(), "{k} 应可作为字典键");
        }
        for k in [
            Kind::Nil,
            Kind::Array,
            Kind::Dict,
            Kind::Tuple,
            Kind::Error,
            Kind::Module,
        ] {
            assert!(!k.is_dict_key(), "{k} 不应作为字典键");
        }
    }

    #[test]
    fn 数值类型判定() {
        assert!(Kind::Int.is_number());
        assert!(Kind::Float.is_number());
        assert!(Kind::Byte.is_number());
        assert!(!Kind::Str.is_number());
        assert!(!Kind::Bool.is_number());
    }

    #[test]
    fn 内置类型名映射到内部标识() {
        assert_eq!(primitive_kind_from_name("整数"), Some("integer"));
        assert_eq!(primitive_kind_from_name("小数"), Some("float"));
        assert_eq!(primitive_kind_from_name("字符串"), Some("string"));
        assert_eq!(primitive_kind_from_name("布尔"), Some("boolean"));
        assert_eq!(primitive_kind_from_name("字节"), Some("byte"));
        assert_eq!(primitive_kind_from_name("字符"), Some("char"));
        assert_eq!(primitive_kind_from_name("数组"), Some("array"));
        assert_eq!(primitive_kind_from_name("字典"), Some("dict"));
        assert_eq!(primitive_kind_from_name("矩形"), None);
    }

    #[test]
    fn 运行时类型标识穿透命名类型() {
        use crate::Object;
        assert_eq!(runtime_type_name(&Object::int(1)), "integer");
        assert_eq!(runtime_type_name(&Object::string("甲")), "string");
        assert_eq!(runtime_type_name(&Object::nil()), "nil");
        let n = Object::named("状态", Object::int(1));
        assert_eq!(runtime_type_name(&n), "integer");
    }

    #[test]
    fn 内部标识转回中文类型名() {
        assert_eq!(kind_display_name("integer"), "整数");
        assert_eq!(kind_display_name("dict"), "字典");
    }

    #[test]
    fn 种类可比较() {
        assert_eq!(Kind::Int, Kind::Int);
        assert_ne!(Kind::Int, Kind::Float);
    }
}
