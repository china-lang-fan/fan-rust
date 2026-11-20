//! 字节码闭包值的登记与识别。
//!
//! `fan-object` 的 `Object::Func` 承载的是树遍历函数（带 AST 函数体），无法干净
//! 地存放字节码函数表索引与 upvalue。VM 因此维护一张身份表：`MakeClosure` 时
//! 构造一个标记用的 `Object::Func`，以其 `Rc` 指针为键登记对应的字节码闭包
//! （所属模块 + 函数表索引 + upvalue 句柄）；调用时据此判断该值是「字节码闭包」
//! 还是普通原生/树遍历函数。
//!
//! 注册表本身以 `Rc<RefCell<..>>` 在主 VM 与加载子模块的子 VM 间共享，使被导入
//! 模块产出的闭包能在主模块中被识别和调用。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fan_ast::{BlockStmt, Parameter, Position};
use fan_object::{Function, Object};

use crate::upvalue::{Closure, ModuleId, Upvalue};

/// 字节码函数在所属模块 `ModuleChunk.functions` 中的索引。
pub type FnIndex = u16;

/// 标记函数名前缀，便于调试与栈追踪识别。
const VM_FN_PREFIX: &str = "__vm_fn__";

/// 维护字节码闭包值身份到闭包数据的映射，可在多个 VM 间共享。
#[derive(Default)]
pub struct VmFunctionRegistry {
    closure_by_ptr: HashMap<usize, Rc<Closure>>,
}

/// 可共享的注册表单柄。
pub type SharedRegistry = Rc<RefCell<VmFunctionRegistry>>;

impl VmFunctionRegistry {
    pub fn new() -> Self {
        VmFunctionRegistry::default()
    }

    pub fn shared() -> SharedRegistry {
        Rc::new(RefCell::new(VmFunctionRegistry::new()))
    }

    /// 构造代表 `module` 模块函数表 `index` 项、携带 `upvalues` 的可调用
    /// `Object::Func` 并登记。
    pub fn make_closure(
        &mut self,
        module: ModuleId,
        index: FnIndex,
        display_name: &str,
        upvalues: Vec<Upvalue>,
    ) -> Object {
        let function = Function::new(
            format!("{VM_FN_PREFIX}{module}:{index}:{display_name}"),
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
                module_id: module,
                function_index: index,
                upvalues,
            }),
        );
        Object::Func(rc)
    }

    /// 若该函数值是 VM 字节码闭包，返回其闭包数据（模块 + 函数索引 + upvalue）。
    pub fn lookup(&self, value: &Object) -> Option<Rc<Closure>> {
        if let Object::Func(rc) = value {
            let key = Rc::as_ptr(rc) as usize;
            self.closure_by_ptr.get(&key).cloned()
        } else {
            None
        }
    }
}
