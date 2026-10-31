//! 类型标注。
//!
//! 凡语言的动态类型为主，类型标注可选；标注后在运行时强制检查。
//! 内部统一以 `|` 连接多个成员表示联合类型，与 Go 版 `DeclType` 保持相同的字符串形态。

use std::fmt;

/// 声明类型。空串 `TypeAny` 表示未标注（动态类型）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DeclType(pub String);

impl DeclType {
    /// 未标注的动态类型。
    pub const ANY: DeclType = DeclType::any();
}

/// 内置基础类型名常量，供解析与检查复用。
pub mod builtin {
    pub const INT: &str = "整数";
    pub const FLOAT: &str = "小数";
    pub const STRING: &str = "字符串";
    pub const BOOL: &str = "布尔";
    pub const ARRAY: &str = "数组";
    pub const DICT: &str = "字典";
    pub const ERROR: &str = "错误";
    pub const BYTE: &str = "字节";
    pub const CHAR: &str = "字符";
}

impl DeclType {
    /// 未标注的动态类型。
    pub const fn any() -> Self {
        DeclType(String::new())
    }

    /// 具名类型（基础类型、模型名、接口名、自定义类型名）。
    pub fn named(name: impl Into<String>) -> Self {
        DeclType(name.into())
    }

    /// 由多个成员构成联合类型，内部以 `|` 连接。
    pub fn union(members: &[String]) -> Self {
        DeclType(members.join("|"))
    }

    pub fn is_any(&self) -> bool {
        self.0.is_empty()
    }

    /// 是否为联合类型（含两个及以上成员）。
    pub fn is_union(&self) -> bool {
        self.0.contains('|')
    }

    /// 拆分为成员列表；单个类型时返回长度为 1 的切片内容。
    pub fn members(&self) -> Vec<&str> {
        if self.0.is_empty() {
            return Vec::new();
        }
        self.0.split('|').collect()
    }

    /// 是否为内置基础类型名。
    pub fn is_builtin(&self) -> bool {
        matches!(
            self.0.as_str(),
            builtin::INT
                | builtin::FLOAT
                | builtin::STRING
                | builtin::BOOL
                | builtin::ARRAY
                | builtin::DICT
                | builtin::ERROR
                | builtin::BYTE
                | builtin::CHAR
        )
    }
}

impl fmt::Display for DeclType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeclType {
    fn from(s: &str) -> Self {
        DeclType(s.to_string())
    }
}

impl From<String> for DeclType {
    fn from(s: String) -> Self {
        DeclType(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 动态类型为空串() {
        assert!(DeclType::any().is_any());
        assert!(!DeclType::any().is_union());
        assert!(DeclType::any().members().is_empty());
    }

    #[test]
    fn 具名类型保留类型名() {
        let t = DeclType::named("整数");
        assert_eq!(t.to_string(), "整数");
        assert!(t.is_builtin());
        assert!(!t.is_union());
        assert_eq!(t.members(), vec!["整数"]);
    }

    #[test]
    fn 联合类型以竖线连接并可拆分() {
        let t = DeclType::union(&["整数".to_string(), "字符串".to_string()]);
        assert_eq!(t.to_string(), "整数|字符串");
        assert!(t.is_union());
        assert_eq!(t.members(), vec!["整数", "字符串"]);
    }

    #[test]
    fn 模型名不是内置类型() {
        assert!(!DeclType::named("矩形").is_builtin());
    }

    #[test]
    fn 全部内置类型识别正确() {
        for name in [
            "整数",
            "小数",
            "字符串",
            "布尔",
            "数组",
            "字典",
            "错误",
            "字节",
            "字符",
        ] {
            assert!(DeclType::named(name).is_builtin(), "{name} 应为内置类型");
        }
    }

    #[test]
    fn 可从字符串与字符串切片构造() {
        assert_eq!(DeclType::from("整数"), DeclType::named("整数"));
        assert_eq!(DeclType::from("整数".to_string()), DeclType::named("整数"));
    }
}
