//! 字节码函数值的登记与识别。
//!
//! `fan-object` 的 `Object::Func` 承载的是树遍历函数（带 AST 函数体），无法干净
//! 地存放字节码函数表索引。VM 因此维护一张身份表：`MakeClosure` 时构造一个
//! 标记用的 `Object::Func`，以其 `Rc` 指针为键登记对应的函数表索引；调用时
//! 据此判断该值是「字节码函数」还是普通原生/树遍历函数。

use std::collections::HashMap;
use std::rc::Rc;

use fan_ast::{BlockStmt, Parameter, Position};
use fan_object::{Function, Object};

/// 字节码函数在 `ModuleChunk.functions` 中的索引。
pub type FnIndex = u16;

/// 标记函数名前缀，便于调试与栈追踪识别。
const VM_FN_PREFIX: &str = "__vm_fn__";

/// 维护字节码函数值身份到函数表索引的映射。
#[derive(Default)]
pub struct VmFunctionRegistry {
    index_by_ptr: HashMap<usize, FnIndex>,
}

impl VmFunctionRegistry {
    pub fn new() -> Self {
        VmFunctionRegistry::default()
    }

    /// 构造一个代表函数表 `index` 项的可调用 `Object::Func` 并登记其身份。
    pub fn make_closure(&mut self, index: FnIndex, display_name: &str) -> Object {
        let function = Function::new(
            format!("{VM_FN_PREFIX}{index}:{display_name}"),
            Vec::<Parameter>::new(),
            BlockStmt::default(),
            None,
            Position::new(0, 0),
        );
        let rc = Rc::new(function);
        let key = Rc::as_ptr(&rc) as usize;
        self.index_by_ptr.insert(key, index);
        Object::Func(rc)
    }

    /// 若该函数值是 VM 字节码函数，返回其函数表索引。
    pub fn lookup(&self, value: &Object) -> Option<FnIndex> {
        if let Object::Func(rc) = value {
            let key = Rc::as_ptr(rc) as usize;
            self.index_by_ptr.get(&key).copied()
        } else {
            None
        }
    }
}
