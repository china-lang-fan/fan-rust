//! 标签处理上下文。
//!
//! 凡语言标签处理器（通过 `注册标签处理(标签名, 处理函数)` 注册）在函数声明期被
//! 回调，回调时收到一个标签上下文对象，处理器据此读取标签元信息、当前实现，
//! 并可用 `替换实现(新实现)` 改写目标函数的实现。
//!
//! 对应 Go 版 `evaluator/tag.go` 中的 `runtimeTagContext`。Rust 侧用
//! `Rc<RefCell<…>>` 提供内部可变性：上下文以 `Object::TagContext` 形式传给处理器，
//! 处理器调用其成员方法即可改写其中的替换实现，求值器在回调结束后读取该替换。

use crate::{Function, Object};
use std::cell::RefCell;
use std::rc::Rc;

/// 标签上下文：一次标签分派中传给凡语言处理器的运行时对象。
pub struct TagContextValue {
    /// 标签模型实例（携带 `值`、命名参数等字段）；未解析到模型时为 `空`。
    tag: Object,
    /// 被标注函数在替换前的实现（一个凡函数值）。
    current: Object,
    /// 处理器通过 `替换实现` 写入的新实现；`None` 表示不替换。
    replacement: RefCell<Option<Rc<Function>>>,
}

impl std::fmt::Debug for TagContextValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TagContextValue")
            .field("tag", &self.tag)
            .field("current", &self.current)
            .field("replaced", &self.replacement.borrow().is_some())
            .finish()
    }
}

impl Clone for TagContextValue {
    fn clone(&self) -> Self {
        TagContextValue {
            tag: self.tag.clone(),
            current: self.current.clone(),
            replacement: RefCell::new(self.replacement.borrow().clone()),
        }
    }
}

impl TagContextValue {
    /// 构造标签上下文。
    pub fn new(tag: Object, current: Object) -> Self {
        TagContextValue {
            tag,
            current,
            replacement: RefCell::new(None),
        }
    }

    /// 标签模型实例（对应 Go 的 `上下文.标签`）。
    pub fn tag(&self) -> Object {
        self.tag.clone()
    }

    /// 替换前的当前实现（对应 Go 的 `上下文.当前实现`）。
    pub fn current(&self) -> Object {
        self.current.clone()
    }

    /// 写入替换实现（对应 Go 的 `上下文.替换实现(函数)`）。
    pub fn replace(&self, func: Rc<Function>) {
        *self.replacement.borrow_mut() = Some(func);
    }

    /// 读取处理器写入的替换实现；未替换时为 `None`。
    pub fn replacement(&self) -> Option<Rc<Function>> {
        self.replacement.borrow().clone()
    }
}
