//! 数组与字典。
//!
//! 二者在凡语言中是**引用语义**：赋值、传参共享同一份数据（语法方案 §9.5）。
//! 因此 Rust 侧用 `Rc<RefCell<…>>` 承载，`Clone` 即“浅拷贝=共享”，
//! 正好匹配语言规范，无需额外的引用/值两套 API。

use crate::kind::Kind;
use crate::Object;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// ---------------------------------------------------------------- 数组

/// 数组对象。内部共享，修改对所有持有者可见。
#[derive(Debug, Clone, PartialEq)]
pub struct FanArray(pub Rc<RefCell<Vec<Object>>>);

impl FanArray {
    pub fn new(elements: Vec<Object>) -> Self {
        FanArray(Rc::new(RefCell::new(elements)))
    }

    pub fn empty() -> Self {
        FanArray::new(Vec::new())
    }

    pub fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    pub fn get(&self, index: usize) -> Option<Object> {
        self.0.borrow().get(index).cloned()
    }

    pub fn push(&self, value: Object) {
        self.0.borrow_mut().push(value);
    }

    /// 就地替换指定下标的元素。
    pub fn set(&self, index: usize, value: Object) {
        if let Some(slot) = self.0.borrow_mut().get_mut(index) {
            *slot = value;
        }
    }

    /// 取出全部元素的快照副本。
    pub fn elements(&self) -> Vec<Object> {
        self.0.borrow().clone()
    }

    pub fn inspect(&self) -> String {
        let parts: Vec<String> = self.0.borrow().iter().map(Object::format).collect();
        format!("[{}]", parts.join(", "))
    }
}

// ---------------------------------------------------------------- 字典

/// 字典内部存储：既保留插入顺序（`keys`），又能按键快速查找（`entries`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DictInner {
    /// 按插入顺序保存的键对象。
    pub keys: Vec<Object>,
    /// 键字符串 → 值。
    pub entries: HashMap<String, Object>,
}

/// 字典对象。内部共享，修改对所有持有者可见。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FanDict(pub Rc<RefCell<DictInner>>);

impl FanDict {
    pub fn new() -> Self {
        FanDict(Rc::new(RefCell::new(DictInner::default())))
    }

    pub fn len(&self) -> usize {
        self.0.borrow().keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().keys.is_empty()
    }

    pub fn get(&self, key: &Object) -> Option<Object> {
        let key_str = dict_key(key)?;
        self.0.borrow().entries.get(&key_str).cloned()
    }

    /// 写入键值对。键不存在时追加到插入序列表，已存在时只更新值。
    pub fn set(&mut self, key: Object, value: Object) {
        let Some(key_str) = dict_key(&key) else {
            return;
        };
        let mut inner = self.0.borrow_mut();
        if !inner.entries.contains_key(&key_str) {
            inner.keys.push(key);
        }
        inner.entries.insert(key_str, value);
    }

    /// 删除键，返回是否确实删掉了。
    pub fn remove(&self, key: &Object) -> bool {
        let Some(key_str) = dict_key(key) else {
            return false;
        };
        let mut inner = self.0.borrow_mut();
        if inner.entries.remove(&key_str).is_none() {
            return false;
        }
        inner
            .keys
            .retain(|k| dict_key(k).as_deref() != Some(key_str.as_str()));
        true
    }

    pub fn contains(&self, key: &Object) -> bool {
        match dict_key(key) {
            Some(k) => self.0.borrow().entries.contains_key(&k),
            None => false,
        }
    }

    pub fn keys(&self) -> Vec<Object> {
        self.0.borrow().keys.clone()
    }

    /// 按值的文本表示稳定排序的键列表。
    pub fn sorted_keys(&self) -> Vec<Object> {
        let mut keys = self.keys();
        keys.sort_by_key(|a| a.inspect());
        keys
    }

    pub fn inspect(&self) -> String {
        let inner = self.0.borrow();
        let parts: Vec<String> = inner
            .keys
            .iter()
            .filter_map(|k| {
                let key_str = dict_key(k)?;
                let v = inner.entries.get(&key_str)?;
                Some(format!("{}: {}", Object::format(k), Object::format(v)))
            })
            .collect();
        format!("{{{}}}", parts.join(", "))
    }
}

// FanDict 的默认实现即空字典。

/// 把键对象编码为内部字符串键。
///
/// 前缀区分类型，避免 `1`（整数）与 `"1"`（字符串）冲突。
/// 规则与 Go 版 `dictKey` 完全一致；不可作为键的类型返回 `None`。
pub fn dict_key(key: &Object) -> Option<String> {
    Some(match key {
        Object::Int(v) => format!("i:{}", v),
        Object::Float(v) => format!("f:{}", Object::Float(*v).inspect()),
        Object::Str(v) => format!("s:{}", v),
        Object::Bool(v) => format!("b:{}", if *v { 1 } else { 0 }),
        Object::Byte(v) => format!("y:{}", v),
        Object::Char(v) => format!("c:{}", v),
        Object::Named(n) => return dict_key(&n.value),
        _ => return None,
    })
}

/// 判断对象能否作为字典键（规则同 Go 版 `ValidDictKey`）。
pub fn valid_dict_key(key: &Object) -> bool {
    match key {
        Object::Named(n) => valid_dict_key(&n.value),
        other => other.kind().is_dict_key(),
    }
}

// ---------------------------------------------------------------- 错误值

/// 凡语言的一等错误值。
///
/// 除消息外还可携带错误码、上下文字典、原因链与调用栈。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ErrorValue {
    pub message: String,
    pub code: String,
    pub details: Option<FanDict>,
    /// 被包装的原始错误，形成原因链。
    pub cause: Option<Box<ErrorValue>>,
    pub stack: Vec<StackFrame>,
}

/// 错误调用栈中的一帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    pub function: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
}

impl StackFrame {
    /// 与 Go 版 `StackFrame.String()` 输出一致。
    pub fn display(&self) -> String {
        let loc = if self.file.is_empty() {
            "<脚本>"
        } else {
            &self.file
        };
        if self.line > 0 {
            format!("  由 {} 调用（{} 第{}行）", self.function, loc, self.line)
        } else {
            format!("  由 {} 调用（{}）", self.function, loc)
        }
    }
}

impl ErrorValue {
    pub fn new(message: impl Into<String>) -> Self {
        ErrorValue {
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = code.into();
        self
    }

    pub fn with_details(mut self, details: FanDict) -> Self {
        self.details = Some(details);
        self
    }

    pub fn with_cause(mut self, cause: ErrorValue) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    /// 与 Go 版 `Error.Inspect()` 一致：只包含消息与可选错误码。
    pub fn inspect(&self) -> String {
        if self.code.is_empty() {
            format!("错误：{}", self.message)
        } else {
            format!("错误：{}（{}）", self.message, self.code)
        }
    }

    /// 与 Go 版 `Error.Trace()` 一致：错误 + 原因链 + 调用栈。
    pub fn trace(&self) -> String {
        let mut out = self.inspect();
        let mut cur = self.cause.as_deref();
        while let Some(c) = cur {
            out.push_str("\n原因：");
            out.push_str(&c.inspect());
            cur = c.cause.as_deref();
        }
        for frame in &self.stack {
            out.push('\n');
            out.push_str(&frame.display());
        }
        out
    }
}

/// 便捷构造：创建一个只带消息的错误对象。
pub fn new_error(message: impl Into<String>) -> Object {
    Object::Error(Rc::new(RefCell::new(ErrorValue::new(message))))
}

/// 便捷构造：带错误码。
pub fn new_error_with_code(message: impl Into<String>, code: impl Into<String>) -> Object {
    Object::Error(Rc::new(RefCell::new(
        ErrorValue::new(message).with_code(code),
    )))
}

/// 便捷构造：包装一个已有错误作为原因。
pub fn wrap_error(message: impl Into<String>, cause: &Object) -> Object {
    let cause_value = match cause {
        Object::Error(e) => Some(e.borrow().clone()),
        _ => None,
    };
    let mut err = ErrorValue::new(message);
    if let Some(c) = cause_value {
        err = err.with_cause(c);
    }
    Object::Error(Rc::new(RefCell::new(err)))
}

// ---------------------------------------------------------------- 模块

/// 模块对象：一组导出符号。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleValue {
    pub name: String,
    pub exports: HashMap<String, Object>,
}

impl ModuleValue {
    pub fn new(name: impl Into<String>) -> Self {
        ModuleValue {
            name: name.into(),
            exports: HashMap::new(),
        }
    }

    pub fn set(&mut self, name: impl Into<String>, value: Object) {
        self.exports.insert(name.into(), value);
    }

    /// 与 Go 版 `Module.Inspect()` 一致：导出名按字典序，以 `、` 连接。
    pub fn inspect(&self) -> String {
        let mut names: Vec<&str> = self.exports.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        format!("<模块 {}：{}>", self.name, names.join("、"))
    }
}

// ---------------------------------------------------------------- 多元值

/// 多元值（多返回值的载体）。内容为值语义的不可变列表。
pub type TupleValue = Rc<Vec<Object>>;

/// 构造多元值。
pub fn new_tuple(values: Vec<Object>) -> Object {
    Object::Tuple(Rc::new(values))
}

/// 判断对象是否为错误种类。
pub fn is_error(o: &Object) -> bool {
    o.kind() == Kind::Error
}

/// 取错误消息；非错误返回 `None`。
pub fn error_message(o: &Object) -> Option<String> {
    match o {
        Object::Error(e) => Some(e.borrow().message.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i64) -> Object {
        Object::Int(v)
    }

    #[test]
    fn 数组是引用语义() {
        let a = FanArray::new(vec![int(1)]);
        let b = a.clone();
        b.push(int(2));
        // 通过 b 的修改对 a 可见
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn 数组显示用逗号加空格连接() {
        let a = FanArray::new(vec![int(1), int(2), int(3)]);
        assert_eq!(a.inspect(), "[1, 2, 3]");
    }

    #[test]
    fn 空数组显示为空方括号() {
        assert_eq!(FanArray::empty().inspect(), "[]");
        assert!(FanArray::empty().is_empty());
    }

    #[test]
    fn 数组读写元素() {
        let a = FanArray::new(vec![int(1), int(2)]);
        assert_eq!(a.get(0), Some(int(1)));
        a.set(0, int(9));
        assert_eq!(a.get(0), Some(int(9)));
        assert_eq!(a.elements(), vec![int(9), int(2)]);
    }

    #[test]
    fn 字典保持插入顺序() {
        let mut d = FanDict::new();
        d.set(Object::Str(Rc::from("乙")), int(2));
        d.set(Object::Str(Rc::from("甲")), int(1));
        assert_eq!(d.inspect(), "{乙: 2, 甲: 1}");
    }

    #[test]
    fn 字典覆盖已有键不改变顺序() {
        let mut d = FanDict::new();
        d.set(Object::Str(Rc::from("甲")), int(1));
        d.set(Object::Str(Rc::from("乙")), int(2));
        d.set(Object::Str(Rc::from("甲")), int(9));
        assert_eq!(d.inspect(), "{甲: 9, 乙: 2}");
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn 字典是引用语义() {
        let mut d = FanDict::new();
        let shared = d.clone();
        d.set(int(1), int(10));
        assert_eq!(shared.get(&int(1)), Some(int(10)));
    }

    #[test]
    fn 字典键按类型前缀区分() {
        // 整数 1 与字符串 "1" 是不同键
        let mut d = FanDict::new();
        d.set(int(1), Object::Str(Rc::from("整数值")));
        d.set(
            Object::Str(Rc::from("1")),
            Object::Str(Rc::from("字符串值")),
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d.get(&int(1)), Some(Object::Str(Rc::from("整数值"))));
        assert_eq!(
            d.get(&Object::Str(Rc::from("1"))),
            Some(Object::Str(Rc::from("字符串值")))
        );
    }

    #[test]
    fn 各类字典键前缀正确() {
        assert_eq!(dict_key(&int(42)), Some("i:42".to_string()));
        assert_eq!(dict_key(&Object::Float(1.5)), Some("f:1.5".to_string()));
        assert_eq!(
            dict_key(&Object::Str(Rc::from("甲"))),
            Some("s:甲".to_string())
        );
        assert_eq!(dict_key(&Object::Bool(true)), Some("b:1".to_string()));
        assert_eq!(dict_key(&Object::Bool(false)), Some("b:0".to_string()));
        assert_eq!(dict_key(&Object::Byte(7)), Some("y:7".to_string()));
        assert_eq!(dict_key(&Object::Char('中')), Some("c:中".to_string()));
    }

    #[test]
    fn 不可作键的类型返回空() {
        assert_eq!(dict_key(&Object::Nil), None);
        assert_eq!(dict_key(&Object::Array(FanArray::empty())), None);
        assert_eq!(dict_key(&Object::Dict(FanDict::new())), None);
    }

    #[test]
    fn 字典键合法性判断() {
        assert!(valid_dict_key(&int(1)));
        assert!(valid_dict_key(&Object::Str(Rc::from("甲"))));
        assert!(valid_dict_key(&Object::Bool(true)));
        assert!(!valid_dict_key(&Object::Nil));
        assert!(!valid_dict_key(&Object::Array(FanArray::empty())));
    }

    #[test]
    fn 命名类型取其底层类型作键() {
        let named = Object::Named(std::rc::Rc::new(crate::Named {
            type_name: "状态".to_string(),
            value: int(3),
        }));
        assert_eq!(dict_key(&named), Some("i:3".to_string()));
        assert!(valid_dict_key(&named));
    }

    #[test]
    fn 字典删除键() {
        let mut d = FanDict::new();
        d.set(int(1), int(10));
        d.set(int(2), int(20));
        assert!(d.remove(&int(1)));
        assert!(!d.remove(&int(1)));
        assert_eq!(d.len(), 1);
        assert_eq!(d.inspect(), "{2: 20}");
    }

    #[test]
    fn 字典包含判断() {
        let mut d = FanDict::new();
        d.set(int(1), int(10));
        assert!(d.contains(&int(1)));
        assert!(!d.contains(&int(2)));
    }

    #[test]
    fn 字典键排序() {
        let mut d = FanDict::new();
        d.set(Object::Str(Rc::from("丙")), int(3));
        d.set(Object::Str(Rc::from("甲")), int(1));
        d.set(Object::Str(Rc::from("乙")), int(2));
        let keys: Vec<String> = d.sorted_keys().iter().map(|k| k.inspect()).collect();
        assert_eq!(keys, vec!["丙", "乙", "甲"]);
    }

    #[test]
    fn 错误显示消息与错误码() {
        let e = ErrorValue::new("文件不存在");
        assert_eq!(e.inspect(), "错误：文件不存在");
        assert_eq!(e.trace(), "错误：文件不存在");

        let e = ErrorValue::new("文件不存在").with_code("ENOENT");
        assert_eq!(e.inspect(), "错误：文件不存在（ENOENT）");
    }

    #[test]
    fn 错误原因链与调用栈() {
        let inner = ErrorValue::new("底层失败");
        let mut outer = ErrorValue::new("上层失败").with_cause(inner);
        outer.stack.push(StackFrame {
            function: "读取".to_string(),
            file: "测试.凡".to_string(),
            line: 12,
            column: 3,
        });
        let trace = outer.trace();
        assert!(trace.starts_with("错误：上层失败"));
        assert!(trace.contains("原因：错误：底层失败"));
        assert!(trace.contains("由 读取 调用（测试.凡 第12行）"));
    }

    #[test]
    fn 栈帧无文件与无行号时的显示() {
        let f = StackFrame {
            function: "甲".to_string(),
            file: String::new(),
            line: 0,
            column: 0,
        };
        assert_eq!(f.display(), "  由 甲 调用（<脚本>）");

        let f = StackFrame {
            function: "甲".to_string(),
            file: "a.凡".to_string(),
            line: 0,
            column: 0,
        };
        assert_eq!(f.display(), "  由 甲 调用（a.凡）");
    }

    #[test]
    fn 错误便捷构造与判断() {
        let e = new_error("失败");
        assert!(is_error(&e));
        assert_eq!(error_message(&e), Some("失败".to_string()));
        assert!(!is_error(&int(1)));
        assert_eq!(error_message(&int(1)), None);
    }

    #[test]
    fn 包装错误保留原因() {
        let inner = new_error("根因");
        let outer = wrap_error("外层", &inner);
        match &outer {
            Object::Error(e) => {
                assert_eq!(e.borrow().message, "外层");
                assert_eq!(e.borrow().cause.as_ref().unwrap().message, "根因");
            }
            other => panic!("应为错误对象，实际 {other:?}"),
        }
    }

    #[test]
    fn 模块导出名按字典序显示() {
        let mut m = ModuleValue::new("数学");
        m.set("平方根", int(1));
        m.set("常量", int(2));
        assert_eq!(m.inspect(), "<模块 数学：常量、平方根>");
    }

    #[test]
    fn 空模块显示() {
        assert_eq!(ModuleValue::new("空").inspect(), "<模块 空：>");
    }

    #[test]
    fn 多元值构造与显示() {
        let t = new_tuple(vec![int(1), int(2)]);
        match &t {
            Object::Tuple(v) => assert_eq!(v.len(), 2),
            other => panic!("应为多元值，实际 {other:?}"),
        }
    }
}
