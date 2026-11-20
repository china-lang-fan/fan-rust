//! 字节码闭包值的登记与识别。
//!
//! `fan-object` 的 `Object::Func` 承载的是树遍历函数（带 AST 函数体），无法干净
//! 地存放字节码函数表索引与 upvalue。VM 因此维护一张身份表：`MakeClosure` 时
//! 构造一个标记用的 `Object::Func`，以其 `Rc` 指针为键登记对应的字节码闭包
//! （函数表索引 + upvalue 句柄）；调用时据此判断该值是「字节码闭包」还是普通
//! 原生/树遍历函数。

use std::collections::HashMap;
use std::rc::Rc;

use fan_ast::{BlockStmt, Parameter, Position};
use fan_object::{Function, Object};

use crate::upvalue::{Closure, Upvalue};

/// 字节码函数在 `ModuleChunk.functions` 中的索引。
pub type FnIndex = u16;

/// 标记函数名前缀，便于调试与栈追踪识别。
const VM_FN_PREFIX: &str = "__vm_fn__";

/// 维护字节码闭包值身份到闭包数据的映射。
#[derive(Default)]
pub struct VmFunctionRegistry {
    closure_by_ptr: HashMap<usize, Rc<Closure>>,
}

impl VmFunctionRegistry {
    pub fn new() -> Self {
        VmFunctionRegistry::default()
    }

    /// 构造代表函数表 `index` 项、携带 `upvalues` 的可调用 `Object::Func` 并登记。
    pub fn make_closure(
        &mut self,
        index: FnIndex,
        display_name: &str,
        upvalues: Vec<Upvalue>,
    ) -> Object {
        let function = Function::new(
            format!("{VM_FN_PREFIX}{index}:{display_name}"),
            Vec::<Parameter>::new(),
            BlockStmt::default(),
            None,
            Position::new(0, 0),
        );
        let rc = Rc::new(function);
        let key = Rc::as_ptr(&rc) as usize;
        self.closure_by_ptr.insert(
            key,
            Rc::new(Closure {
                function_index: index,
                upvalues,
            }),
        );
        Object::Func(rc)
    }

    /// 若该函数值是 VM 字节码闭包，返回其闭包数据（函数索引 + upvalue）。
    pub fn lookup(&self, value: &Object) -> Option<Rc<Closure>> {
        if let Object::Func(rc) = value {
            let key = Rc::as_ptr(rc) as usize;
            self.closure_by_ptr.get(&key).cloned()
        } else {
            None
        }
    }
}
