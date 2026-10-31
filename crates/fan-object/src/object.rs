//! 对象系统核心：`Object` 枚举。
//!
//! 凡语言的值分两类语义（语法方案 §9.5）：
//!
//! - **值语义**：整数、小数、字符串、布尔、字节、字符、空、多元值——`Clone` 即独立副本
//! - **引用语义**：数组、字典、模型实例——`Clone` 是浅拷贝，共享同一份数据
//!
//! Rust 侧用 `Rc`/`Rc<RefCell<…>>` 让这个区分在类型上显式可见，
//! 从而避免 Go 版中“什么时候该深拷贝”的隐式约定。

use crate::collection::{dict_key, ErrorValue, FanArray, FanDict, ModuleValue};
use crate::kind::Kind;
use crate::runtime_entity::{Builtin, Class, Function, Instance};
use fan_ast::format_f64;
use std::cell::RefCell;
use std::rc::Rc;

/// 命名类型值：包装一个底层值，携带用户定义的类型身份。
///
/// 参考 Go 中 `type Status int`：运算时按底层类型参与，
/// 类型判断与赋值检查时保留独立身份。
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub type_name: String,
    pub value: Object,
}

/// 一个凡语言值。
#[derive(Debug, Clone, Default)]
pub enum Object {
    #[default]
    Nil,
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bool(bool),
    /// 8 位无符号整数，0–255。
    Byte(u8),
    /// 单个 Unicode 字符。
    Char(char),
    /// 命名类型值。
    Named(Rc<Named>),
    Array(FanArray),
    Dict(FanDict),
    /// 多元值（多返回值载体）。
    Tuple(Rc<Vec<Object>>),
    Error(Rc<RefCell<ErrorValue>>),
    Module(Rc<RefCell<ModuleValue>>),
    Func(Rc<Function>),
    Class(Rc<Class>),
    Instance(Rc<Instance>),
    Builtin(Rc<Builtin>),
}

impl Object {
    /// 值种类。命名类型返回其底层类型的种类（与 Go 版 `Named.Kind()` 一致）。
    pub fn kind(&self) -> Kind {
        match self {
            Object::Int(_) => Kind::Int,
            Object::Float(_) => Kind::Float,
            Object::Str(_) => Kind::Str,
            Object::Bool(_) => Kind::Bool,
            Object::Byte(_) => Kind::Byte,
            Object::Char(_) => Kind::Char,
            Object::Nil => Kind::Nil,
            Object::Named(n) => n.value.kind(),
            Object::Array(_) => Kind::Array,
            Object::Dict(_) => Kind::Dict,
            Object::Tuple(_) => Kind::Tuple,
            Object::Error(_) => Kind::Error,
            Object::Module(_) => Kind::Module,
            Object::Func(_) => Kind::Func,
            Object::Class(_) => Kind::Class,
            Object::Instance(_) => Kind::Instance,
            Object::Builtin(_) => Kind::Builtin,
        }
    }

    /// 值的显示文本，用于 `打印` 与字符串插值。
    pub fn inspect(&self) -> String {
        match self {
            Object::Int(v) => v.to_string(),
            Object::Float(v) => format_f64(*v),
            Object::Str(v) => v.to_string(),
            Object::Bool(v) => if *v { "真" } else { "假" }.to_string(),
            Object::Byte(v) => v.to_string(),
            Object::Char(v) => v.to_string(),
            Object::Nil => "空".to_string(),
            Object::Named(n) => n.value.inspect(),
            Object::Array(a) => a.inspect(),
            Object::Dict(d) => d.inspect(),
            Object::Tuple(v) => {
                let parts: Vec<String> = v.iter().map(Object::inspect).collect();
                format!("({})", parts.join(", "))
            }
            Object::Error(e) => e.borrow().inspect(),
            Object::Module(m) => m.borrow().inspect(),
            Object::Func(f) => {
                if f.name.is_empty() {
                    "函数".to_string()
                } else {
                    format!("函数 {}", f.name)
                }
            }
            Object::Class(c) => format!("模型 {}", c.name),
            Object::Instance(i) => {
                let parts: Vec<String> = i
                    .class
                    .fields
                    .iter()
                    .map(|f| {
                        let v = i
                            .get(&f.name)
                            .map(|o| o.inspect())
                            .unwrap_or_else(|| "空".to_string());
                        format!("{}: {}", f.name, v)
                    })
                    .collect();
                format!("{}{{{}}}", i.class.name, parts.join(", "))
            }
            Object::Builtin(b) => format!("内建 {}", b.name),
        }
    }

    /// 与 Go 版 `object.Format` 等价：空引用也显示为“空”。
    pub fn format(o: &Object) -> String {
        o.inspect()
    }

    /// 调试文本，含种类信息。
    pub fn debug(&self) -> String {
        format!("{}({})", self.kind(), self.inspect())
    }

    // ------------------------------------------------------------ 便利构造

    pub fn int(v: i64) -> Object {
        Object::Int(v)
    }

    pub fn float(v: f64) -> Object {
        Object::Float(v)
    }

    pub fn string(v: impl Into<String>) -> Object {
        Object::Str(Rc::from(v.into()))
    }

    pub fn bool_of(v: bool) -> Object {
        Object::Bool(v)
    }

    pub fn nil() -> Object {
        Object::Nil
    }

    pub fn array(elements: Vec<Object>) -> Object {
        Object::Array(FanArray::new(elements))
    }

    pub fn dict() -> Object {
        Object::Dict(FanDict::new())
    }

    pub fn tuple(values: Vec<Object>) -> Object {
        Object::Tuple(Rc::new(values))
    }

    /// 用底层值与类型名构造命名类型值。
    pub fn named(type_name: impl Into<String>, value: Object) -> Object {
        Object::Named(Rc::new(Named {
            type_name: type_name.into(),
            value,
        }))
    }

    // ------------------------------------------------------------ 取值

    /// 取整数值；非整数返回 `None`（命名类型会先解包）。
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Object::Int(v) => Some(*v),
            Object::Byte(v) => Some(*v as i64),
            Object::Named(n) => n.value.as_int(),
            _ => None,
        }
    }

    /// 取小数值；整数会自动提升为小数。
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Object::Float(v) => Some(*v),
            Object::Int(v) => Some(*v as f64),
            Object::Byte(v) => Some(*v as f64),
            Object::Named(n) => n.value.as_float(),
            _ => None,
        }
    }

    /// 取字符串值。
    pub fn as_str(&self) -> Option<String> {
        match self {
            Object::Str(v) => Some(v.to_string()),
            Object::Char(v) => Some(v.to_string()),
            Object::Named(n) => n.value.as_str(),
            _ => None,
        }
    }

    /// 拆开命名类型包装，返回底层值。
    pub fn unwrap_named(&self) -> Object {
        match self {
            Object::Named(n) => n.value.unwrap_named(),
            other => other.clone(),
        }
    }

    /// 命名类型的类型名；非命名类型返回 `None`。
    pub fn type_name(&self) -> Option<String> {
        match self {
            Object::Named(n) => Some(n.type_name.clone()),
            _ => None,
        }
    }

    /// 是否为错误值。
    pub fn is_error(&self) -> bool {
        matches!(self, Object::Error(_))
    }

    /// 是否为“空”。
    pub fn is_nil(&self) -> bool {
        matches!(self, Object::Nil)
    }

    /// 真值判断：只有 `假` 与 `空` 为假（与 Go 版一致）。
    pub fn is_truthy(&self) -> bool {
        match self {
            Object::Bool(v) => *v,
            Object::Nil => false,
            _ => true,
        }
    }

    /// 是否可作为字典键。
    pub fn is_dict_key(&self) -> bool {
        self.kind().is_dict_key()
    }

    /// 内部字符串键；不可作键时返回 `None`。
    pub fn dict_key(&self) -> Option<String> {
        dict_key(self)
    }
}

impl PartialEq for Object {
    /// 结构相等。
    ///
    /// 引用语义类型（数组/字典/实例）比较的是**内容**而非身份，
    /// 与 Go 版 `DeepEqual` 的语义一致；内建函数按名字与实现指针比较。
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Object::Int(a), Object::Int(b)) => a == b,
            (Object::Float(a), Object::Float(b)) => a == b,
            (Object::Str(a), Object::Str(b)) => a == b,
            (Object::Bool(a), Object::Bool(b)) => a == b,
            (Object::Byte(a), Object::Byte(b)) => a == b,
            (Object::Char(a), Object::Char(b)) => a == b,
            (Object::Nil, Object::Nil) => true,
            (Object::Named(a), Object::Named(b)) => {
                a.type_name == b.type_name && a.value == b.value
            }
            (Object::Array(a), Object::Array(b)) => *a.0.borrow() == *b.0.borrow(),
            (Object::Dict(a), Object::Dict(b)) => {
                let (x, y) = (a.0.borrow(), b.0.borrow());
                x.keys.len() == y.keys.len()
                    && x.keys.iter().all(|k| match dict_key(k) {
                        Some(key) => x.entries.get(&key) == y.entries.get(&key),
                        None => false,
                    })
            }
            (Object::Tuple(a), Object::Tuple(b)) => a == b,
            (Object::Error(a), Object::Error(b)) => a.borrow().eq(&b.borrow()),
            (Object::Module(a), Object::Module(b)) => {
                let (x, y) = (a.borrow(), b.borrow());
                x.name == y.name
                    && x.exports.len() == y.exports.len()
                    && x.exports.iter().all(|(k, v)| y.exports.get(k) == Some(v))
            }
            (Object::Func(a), Object::Func(b)) => Rc::ptr_eq(a, b),
            (Object::Class(a), Object::Class(b)) => a.name == b.name && Rc::ptr_eq(a, b),
            (Object::Instance(a), Object::Instance(b)) => Rc::ptr_eq(a, b),
            (Object::Builtin(a), Object::Builtin(b)) => {
                a.name == b.name && Rc::ptr_eq(&a.func, &b.func)
            }
            _ => false,
        }
    }
}

// 测试中 3.14 只是普通小数，并非要近似 π
#[cfg_attr(test, allow(clippy::approx_constant))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 各基础值的种类正确() {
        assert_eq!(Object::int(1).kind(), Kind::Int);
        assert_eq!(Object::float(1.5).kind(), Kind::Float);
        assert_eq!(Object::string("甲").kind(), Kind::Str);
        assert_eq!(Object::bool_of(true).kind(), Kind::Bool);
        assert_eq!(Object::Byte(1).kind(), Kind::Byte);
        assert_eq!(Object::Char('中').kind(), Kind::Char);
        assert_eq!(Object::nil().kind(), Kind::Nil);
        assert_eq!(Object::array(vec![]).kind(), Kind::Array);
        assert_eq!(Object::dict().kind(), Kind::Dict);
        assert_eq!(Object::tuple(vec![]).kind(), Kind::Tuple);
    }

    #[test]
    fn 命名类型的种类取底层类型() {
        let n = Object::named("状态", Object::int(1));
        assert_eq!(n.kind(), Kind::Int);
        assert_eq!(n.type_name(), Some("状态".to_string()));
        assert_eq!(Object::int(1).type_name(), None);
    }

    #[test]
    fn 命名类型可解包() {
        let n = Object::named("状态", Object::named("内层", Object::int(7)));
        assert_eq!(n.unwrap_named(), Object::int(7));
    }

    #[test]
    fn 基础值显示() {
        assert_eq!(Object::int(42).inspect(), "42");
        assert_eq!(Object::float(3.14).inspect(), "3.14");
        assert_eq!(Object::float(2.0).inspect(), "2");
        assert_eq!(Object::string("你好").inspect(), "你好");
        assert_eq!(Object::bool_of(true).inspect(), "真");
        assert_eq!(Object::bool_of(false).inspect(), "假");
        assert_eq!(Object::Byte(255).inspect(), "255");
        assert_eq!(Object::Char('中').inspect(), "中");
        assert_eq!(Object::nil().inspect(), "空");
    }

    #[test]
    fn 命名值显示为其底层值() {
        assert_eq!(Object::named("状态", Object::int(5)).inspect(), "5");
    }

    #[test]
    fn 数组与字典与多元值显示() {
        assert_eq!(
            Object::array(vec![Object::int(1), Object::int(2)]).inspect(),
            "[1, 2]"
        );
        let mut d = FanDict::new();
        d.set(Object::string("甲"), Object::int(1));
        assert_eq!(Object::Dict(d).inspect(), "{甲: 1}");
        assert_eq!(
            Object::tuple(vec![Object::int(1), Object::int(2)]).inspect(),
            "(1, 2)"
        );
    }

    #[test]
    fn 调试文本含种类() {
        assert_eq!(Object::int(42).debug(), "整数(42)");
        assert_eq!(Object::nil().debug(), "空(空)");
    }

    #[test]
    fn 取值转换() {
        assert_eq!(Object::int(7).as_int(), Some(7));
        assert_eq!(Object::Byte(7).as_int(), Some(7));
        assert_eq!(Object::int(7).as_float(), Some(7.0));
        assert_eq!(Object::float(7.5).as_float(), Some(7.5));
        assert_eq!(Object::string("甲").as_str(), Some("甲".to_string()));
        assert_eq!(Object::Char('甲').as_str(), Some("甲".to_string()));
        assert_eq!(Object::int(7).as_str(), None);
        assert_eq!(Object::nil().as_int(), None);
    }

    #[test]
    fn 命名类型的取值转换穿透包装() {
        let n = Object::named("状态", Object::int(7));
        assert_eq!(n.as_int(), Some(7));
        assert_eq!(n.as_float(), Some(7.0));
    }

    #[test]
    fn 真值判断只有假与空为假() {
        assert!(Object::bool_of(true).is_truthy());
        assert!(!Object::bool_of(false).is_truthy());
        assert!(!Object::nil().is_truthy());
        assert!(Object::int(0).is_truthy());
        assert!(Object::string("").is_truthy());
        assert!(Object::array(vec![]).is_truthy());
    }

    #[test]
    fn 空值与错误判断() {
        assert!(Object::nil().is_nil());
        assert!(!Object::int(1).is_nil());
        let e = crate::collection::new_error("失败");
        assert!(e.is_error());
        assert!(!Object::nil().is_error());
    }

    #[test]
    fn 相等比较按内容而非身份() {
        assert_eq!(Object::int(1), Object::int(1));
        assert_ne!(Object::int(1), Object::int(2));
        assert_ne!(Object::int(1), Object::float(1.0));

        // 数组引用语义：不同实例但内容相同即相等
        assert_eq!(
            Object::array(vec![Object::int(1)]),
            Object::array(vec![Object::int(1)])
        );
        // 字典同理
        let mut d1 = FanDict::new();
        d1.set(Object::int(1), Object::int(10));
        let mut d2 = FanDict::new();
        d2.set(Object::int(1), Object::int(10));
        assert_eq!(Object::Dict(d1), Object::Dict(d2));
    }

    #[test]
    fn 数组比较受元素影响() {
        assert_ne!(
            Object::array(vec![Object::int(1)]),
            Object::array(vec![Object::int(2)])
        );
        assert_ne!(Object::array(vec![Object::int(1)]), Object::array(vec![]));
    }

    #[test]
    fn 字典比较受键值影响() {
        let mut d1 = FanDict::new();
        d1.set(Object::int(1), Object::int(10));
        let mut d2 = FanDict::new();
        d2.set(Object::int(1), Object::int(11));
        assert_ne!(Object::Dict(d1), Object::Dict(d2));
    }

    #[test]
    fn 多元值比较() {
        assert_eq!(
            Object::tuple(vec![Object::int(1)]),
            Object::tuple(vec![Object::int(1)])
        );
        assert_ne!(
            Object::tuple(vec![Object::int(1)]),
            Object::tuple(vec![Object::int(2)])
        );
    }

    #[test]
    fn 命名类型比较含类型身份() {
        assert_eq!(
            Object::named("甲", Object::int(1)),
            Object::named("甲", Object::int(1))
        );
        assert_ne!(
            Object::named("甲", Object::int(1)),
            Object::named("乙", Object::int(1))
        );
    }

    #[test]
    fn 错误对象比较按内容() {
        let a = crate::collection::new_error("失败");
        let b = crate::collection::new_error("失败");
        let c = crate::collection::new_error("其他");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn 默认值为空() {
        assert_eq!(Object::default(), Object::nil());
    }

    #[test]
    fn 字典键判断与内部键一致() {
        assert!(Object::int(1).is_dict_key());
        assert!(Object::string("甲").is_dict_key());
        assert!(!Object::nil().is_dict_key());
        assert_eq!(Object::int(1).dict_key(), Some("i:1".to_string()));
        assert_eq!(Object::nil().dict_key(), None);
    }
}
