//! 栈式虚拟机的取指-译码-执行循环。
//!
//! 值统一使用 fan-object 的 [`Object`]；全局绑定与原生内建通过 [`Runtime`]
//! 访问，VM 不重复实现宿主能力。函数帧承载局部槽位，模块全局名落到 VM 自己的
//! 全局表（后续与模块导出统一）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fan_code::chunk::{Chunk, ModuleChunk};
use fan_code::opcode::Opcode;
use fan_object::{Builtin, Object, Runtime};

use crate::error::{VmError, VmResult};
use crate::frame::Frame;
use crate::value::const_to_object;
use crate::vm_function::VmFunctionRegistry;

/// 单线程栈式虚拟机。
pub struct Vm<'m> {
    module: &'m ModuleChunk,
    runtime: Rc<RefCell<Runtime>>,
    frames: Vec<Frame>,
    stack: Vec<Object>,
    /// 每帧独立的局部槽位区，按帧槽位基址切片。
    slots: Vec<Object>,
    globals: HashMap<String, Object>,
    functions: VmFunctionRegistry,
}

impl<'m> Vm<'m> {
    pub fn new(module: &'m ModuleChunk, runtime: Rc<RefCell<Runtime>>) -> Self {
        Vm {
            module,
            runtime,
            frames: Vec::new(),
            stack: Vec::new(),
            slots: Vec::new(),
            globals: HashMap::new(),
            functions: VmFunctionRegistry::new(),
        }
    }

    /// 执行模块入口块，返回入口的最终值。
    pub fn run(&mut self) -> VmResult<Object> {
        let base = self.slots.len();
        self.frames.push(Frame::entry(base));
        let result = self.execute();
        self.frames.pop();
        result
    }

    fn current_chunk(&self) -> &Chunk {
        let frame = self.frames.last().expect("执行时必须存在调用帧");
        if frame.function_index == Frame::ENTRY {
            &self.module.entry
        } else {
            &self.module.functions[frame.function_index as usize].body
        }
    }

    fn line_at(&self, chunk: &Chunk, frame: &Frame) -> Option<u32> {
        // 指令序号难以由字节偏移直接得到，行号表仅作最佳努力：VM 在出错时
        // 主要依赖偏移；这里保留接口，后续可建偏移→行号映射。
        let _ = (chunk, frame);
        None
    }

    fn execute(&mut self) -> VmResult<Object> {
        loop {
            let chunk = self.current_chunk().clone();
            let offset = self.frames.last().expect("帧存在").ip;
            let byte = *chunk
                .code
                .get(offset)
                .ok_or_else(|| VmError::new("指令流意外结束，缺少 Halt/Return"))?;
            let template = Opcode::from_code(byte)
                .ok_or_else(|| VmError::new(format!("未知操作码 {byte}")).at(offset, None))?;
            self.frames.last_mut().expect("帧存在").ip += 1;

            match template {
                Opcode::Halt => {
                    return Ok(self.stack.last().cloned().unwrap_or(Object::Nil));
                }
                Opcode::Const(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let value = chunk
                        .constants
                        .get(idx)
                        .map(const_to_object)
                        .ok_or_else(|| VmError::new(format!("常量索引 {idx} 越界")))?;
                    self.stack.push(value);
                }
                Opcode::Pop => {
                    self.stack
                        .pop()
                        .ok_or_else(|| VmError::new("Pop 时值栈为空"))?;
                }
                Opcode::Dup => {
                    let top = self
                        .stack
                        .last()
                        .cloned()
                        .ok_or_else(|| VmError::new("Dup 时值栈为空"))?;
                    self.stack.push(top);
                }
                Opcode::LoadLocal(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let base = self.frames.last().expect("帧存在").slot_base;
                    let value = self.slots.get(base + idx).cloned().unwrap_or(Object::Nil);
                    self.stack.push(value);
                }
                Opcode::StoreLocal(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let value = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("StoreLocal 时值栈为空"))?;
                    let base = self.frames.last().expect("帧存在").slot_base;
                    let target = base + idx;
                    if target >= self.slots.len() {
                        self.slots.resize(target + 1, Object::Nil);
                    }
                    self.slots[target] = value;
                }
                Opcode::LoadGlobal(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    if let Some(value) = self.globals.get(&name).cloned() {
                        self.stack.push(value);
                    } else if let Some(builtin) = self.runtime.borrow().lookup_native(&name) {
                        self.stack.push(Object::Builtin(builtin));
                    } else {
                        return Err(VmError::new(format!("未定义的名字：{name}")));
                    }
                }
                Opcode::StoreGlobal(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    let value = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("StoreGlobal 时值栈为空"))?;
                    self.globals.insert(name, value);
                }
                Opcode::Jump(_) => {
                    let target = self.read_u32(&chunk)? as usize;
                    self.frames.last_mut().expect("帧存在").ip = target;
                }
                Opcode::JumpIfFalse(_) | Opcode::JumpIfTrue(_) => {
                    let target = self.read_u32(&chunk)? as usize;
                    let cond = self
                        .stack
                        .last()
                        .cloned()
                        .ok_or_else(|| VmError::new("条件跳转时值栈为空"))?;
                    let truthy = is_truthy(&cond);
                    let take = matches!(template, Opcode::JumpIfTrue(_)) == truthy
                        || matches!(template, Opcode::JumpIfFalse(_)) && !truthy;
                    if take {
                        self.frames.last_mut().expect("帧存在").ip = target;
                    }
                }
                Opcode::Return => {
                    return Ok(self.stack.last().cloned().unwrap_or(Object::Nil));
                }
                Opcode::MakeClosure(_) => {
                    let index = self.read_u16(&chunk)?;
                    let name = self
                        .module
                        .functions
                        .get(index as usize)
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| "<未知函数>".to_string());
                    let value = self.functions.make_closure(index, &name);
                    self.stack.push(value);
                }
                Opcode::Call(_) => {
                    let argc = self.read_u8(&chunk)? as usize;
                    self.call_value(argc)?;
                }
                Opcode::CallIfFunc => {
                    let callee = self
                        .stack
                        .last()
                        .cloned()
                        .ok_or_else(|| VmError::new("CallIfFunc 时值栈为空"))?;
                    if is_callable(&callee) {
                        // 弹出再以零参调用，结果压栈。
                        self.stack.pop();
                        self.call_value(0)?;
                    }
                }
                Opcode::CallNative(_, _) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let argc = self.read_u8(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    self.call_native(&name, argc)?;
                }
                Opcode::BinOp(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let op = self.string_const(&chunk, idx)?;
                    let right = self.pop_operand()?;
                    let left = self.pop_operand()?;
                    let result = crate::ops::apply_binary(&op, left, right)
                        .map_err(|e| self.decorate(&chunk, e))?;
                    self.stack.push(result);
                }
                Opcode::UnaryOp(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let op = self.string_const(&chunk, idx)?;
                    let operand = self.pop_operand()?;
                    let result = crate::ops::apply_unary(&op, operand)
                        .map_err(|e| self.decorate(&chunk, e))?;
                    self.stack.push(result);
                }
                other => {
                    let line = self.line_at(&chunk, self.frames.last().expect("帧存在"));
                    return Err(
                        VmError::new(format!("指令 {other:?} 尚未在 VM 中实现")).at(offset, line)
                    );
                }
            }
        }
    }

    fn call_value(&mut self, argc: usize) -> VmResult<()> {
        let callee = self
            .stack
            .pop()
            .ok_or_else(|| VmError::new("Call 时缺少可调用对象"))?;
        let args = self.pop_args(argc)?;
        if let Some(index) = self.functions.lookup(&callee) {
            self.invoke_bytecode_function(index, args)?;
            return Ok(());
        }
        match callee {
            Object::Builtin(builtin) => self.invoke_builtin(&builtin, args),
            other => Err(VmError::new(format!("值不可调用：{}", other.kind()))),
        }
    }

    fn call_native(&mut self, name: &str, argc: usize) -> VmResult<()> {
        let args = self.pop_args(argc)?;
        let builtin = self
            .runtime
            .borrow()
            .lookup_native(name)
            .ok_or_else(|| VmError::new(format!("未定义的内建函数：{name}")))?;
        self.invoke_builtin(&builtin, args)
    }

    fn invoke_builtin(&mut self, builtin: &Rc<Builtin>, args: Vec<Object>) -> VmResult<()> {
        let func = builtin.func.clone();
        let values = func(&mut self.runtime.borrow_mut(), args)
            .map_err(|e| VmError::new(format!("内建 {} 调用失败：{e}", builtin.name)))?;
        self.push_call_result(values);
        Ok(())
    }

    fn invoke_bytecode_function(&mut self, index: u16, args: Vec<Object>) -> VmResult<()> {
        let arity = self.module.functions[index as usize].arity.fixed;
        if args.len() != arity {
            return Err(VmError::new(format!(
                "函数 {} 参数个数不符：期望 {arity}，实得 {}",
                self.module.functions[index as usize].name,
                args.len()
            )));
        }
        let base = self.slots.len();
        self.slots.extend(args);
        self.frames.push(Frame::function(index, base));
        let result = self.execute()?;
        self.frames.pop();
        // 回收本帧槽位。
        self.slots.truncate(base);
        self.stack.push(result);
        Ok(())
    }

    fn push_call_result(&mut self, values: Vec<Object>) {
        match values.len() {
            0 => self.stack.push(Object::Nil),
            1 => self.stack.push(values.into_iter().next().unwrap()),
            _ => self.stack.push(Object::tuple(values)),
        }
    }

    fn pop_args(&mut self, argc: usize) -> VmResult<Vec<Object>> {
        if self.stack.len() < argc {
            return Err(VmError::new("实参个数超过值栈深度"));
        }
        let at = self.stack.len() - argc;
        Ok(self.stack.split_off(at))
    }

    fn pop_operand(&mut self) -> VmResult<Object> {
        self.stack
            .pop()
            .ok_or_else(|| VmError::new("运算时值栈操作数不足"))
    }

    fn string_const(&self, chunk: &Chunk, idx: usize) -> VmResult<String> {
        match chunk.constants.get(idx) {
            Some(fan_code::ConstValue::Str(s)) => Ok(s.clone()),
            other => Err(VmError::new(format!(
                "常量 {idx} 不是字符串，实际为 {other:?}"
            ))),
        }
    }

    fn read_u8(&mut self, chunk: &Chunk) -> VmResult<u8> {
        let frame = self.frames.last_mut().expect("帧存在");
        let value = *chunk
            .code
            .get(frame.ip)
            .ok_or_else(|| VmError::new("读取 u8 操作数时指令流结束"))?;
        frame.ip += 1;
        Ok(value)
    }

    fn read_u16(&mut self, chunk: &Chunk) -> VmResult<u16> {
        let frame = self.frames.last_mut().expect("帧存在");
        let bytes = chunk
            .code
            .get(frame.ip..frame.ip + 2)
            .ok_or_else(|| VmError::new("读取 u16 操作数时指令流结束"))?;
        frame.ip += 2;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self, chunk: &Chunk) -> VmResult<u32> {
        let frame = self.frames.last_mut().expect("帧存在");
        let bytes = chunk
            .code
            .get(frame.ip..frame.ip + 4)
            .ok_or_else(|| VmError::new("读取 u32 操作数时指令流结束"))?;
        frame.ip += 4;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn decorate(&self, _chunk: &Chunk, err: VmError) -> VmError {
        err
    }
}

/// 真值规则与树遍历求值器一致：只有 `假` 与 `空` 为假，其余皆真。
fn is_truthy(value: &Object) -> bool {
    !matches!(value, Object::Nil | Object::Bool(false))
}

/// 是否为可调用值：字节码函数（Object::Func）或原生内建。
fn is_callable(value: &Object) -> bool {
    matches!(value, Object::Func(_) | Object::Builtin(_))
}
