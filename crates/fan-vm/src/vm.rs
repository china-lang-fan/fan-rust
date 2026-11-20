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
use crate::upvalue::Upvalue;
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
                    self.set_slot(base, idx, value);
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
                    let func = self
                        .module
                        .functions
                        .get(index as usize)
                        .cloned()
                        .ok_or_else(|| VmError::new(format!("函数表索引 {index} 越界")))?;
                    // 按编译期描述逐个解析 upvalue：来自外层局部槽则取/建 open 句柄，
                    // 来自外层 upvalue 则直接复用同一句柄，保证跨层共享同一存储。
                    let mut upvalues: Vec<Upvalue> = Vec::with_capacity(func.upvalues.len());
                    for desc in &func.upvalues {
                        let upvalue = if desc.from_local {
                            self.capture_local(desc.index)?
                        } else {
                            self.capture_upvalue(desc.index)?
                        };
                        upvalues.push(upvalue);
                    }
                    let value = self.functions.make_closure(index, &func.name, upvalues);
                    self.stack.push(value);
                }
                Opcode::LoadUpvalue(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let cell = self.frame_upvalue(idx)?;
                    self.stack.push(cell.borrow().clone());
                }
                Opcode::StoreUpvalue(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let value = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("StoreUpvalue 时值栈为空"))?;
                    let cell = self.frame_upvalue(idx)?;
                    *cell.borrow_mut() = value;
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
                Opcode::UnpackTuple(_) => {
                    let count = self.read_u16(&chunk)? as usize;
                    let value = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("UnpackTuple 时值栈为空"))?;
                    let elements = match value {
                        Object::Tuple(items) => items,
                        other => {
                            return Err(VmError::new(format!(
                                "多目标解构要求元组，实际为 {}",
                                other.kind()
                            )))
                        }
                    };
                    if elements.len() < count {
                        return Err(VmError::new(format!(
                            "元组元素 {} 个，不足以解构 {count} 个目标",
                            elements.len()
                        )));
                    }
                    for element in elements.iter().take(count) {
                        self.stack.push(element.clone());
                    }
                }
                Opcode::BuildTuple(_) => {
                    let count = self.read_u16(&chunk)? as usize;
                    let elements = self.pop_args(count)?;
                    self.stack.push(Object::tuple(elements));
                }
                Opcode::ArgCount => {
                    let argc = self.frames.last().expect("帧存在").argc;
                    self.stack.push(Object::Int(argc as i64));
                }
                Opcode::BuildArray(_) => {
                    let count = self.read_u16(&chunk)? as usize;
                    let elements = self.pop_args(count)?;
                    self.stack.push(Object::array(elements));
                }
                Opcode::BuildDict(_) => {
                    let pairs = self.read_u16(&chunk)? as usize;
                    // 栈上依次为 key0, value0, key1, value1 …，pop_args 保持顺序。
                    let entries = self.pop_args(pairs * 2)?;
                    let mut dict = fan_object::FanDict::new();
                    let mut i = 0;
                    while i < entries.len() {
                        let key = entries[i].clone();
                        let value = entries[i + 1].clone();
                        if !key.is_dict_key() {
                            return Err(VmError::new(format!(
                                "不支持的字典键类型：{}",
                                key.kind()
                            )));
                        }
                        dict.set(key, value);
                        i += 2;
                    }
                    self.stack.push(Object::Dict(dict));
                }
                Opcode::GetIndex => {
                    let index = self.pop_operand()?;
                    let target = self.pop_operand()?;
                    let value = self.get_index(&target, &index)?;
                    self.stack.push(value);
                }
                Opcode::SetIndex => {
                    let value = self.pop_operand()?;
                    let index = self.pop_operand()?;
                    let target = self.pop_operand()?;
                    self.set_index(&target, &index, value)?;
                }
                Opcode::GetSlice => {
                    let high = self.pop_operand()?;
                    let low = self.pop_operand()?;
                    let target = self.pop_operand()?;
                    let result = self.get_slice(&target, &low, &high)?;
                    self.stack.push(result);
                }
                Opcode::GetMember(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    let object = self.pop_operand()?;
                    let value = self.get_member(&object, &name)?;
                    self.stack.push(value);
                }
                Opcode::SetMember(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    let value = self.pop_operand()?;
                    let object = self.pop_operand()?;
                    self.set_member(&object, &name, value)?;
                }
                Opcode::ForStep(..) => {
                    let tag = self.read_u8(&chunk)?;
                    let iter_slot = self.read_u16(&chunk)? as usize;
                    let counter_slot = self.read_u16(&chunk)? as usize;
                    let elem_slot = self.read_u16(&chunk)? as usize;
                    let index_slot = self.read_u16(&chunk)? as usize;
                    self.for_step(tag, iter_slot, counter_slot, elem_slot, index_slot)?;
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
        if let Some(closure) = self.functions.lookup(&callee) {
            let index = closure.function_index;
            let upvalues = closure.upvalues.clone();
            self.invoke_bytecode_function(index, upvalues, args)?;
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

    fn invoke_bytecode_function(
        &mut self,
        index: u16,
        upvalues: Vec<Upvalue>,
        args: Vec<Object>,
    ) -> VmResult<()> {
        let func = &self.module.functions[index as usize];
        let fixed = func.arity.fixed;
        let variadic = func.arity.variadic;
        let min_required = fixed - func.arity.optional;
        let name = func.name.clone();
        if args.len() < min_required {
            return Err(VmError::new(format!(
                "函数 {name} 需要至少 {min_required} 个参数，实际 {} 个",
                args.len()
            )));
        }
        if !variadic && args.len() > fixed {
            return Err(VmError::new(format!(
                "函数 {name} 最多接受 {fixed} 个参数，实际 {} 个",
                args.len()
            )));
        }
        let base = self.slots.len();
        // 固定参数槽：有实参取实参，缺省先填空，函数序言再按默认值回填。
        for idx in 0..fixed {
            let value = args.get(idx).cloned().unwrap_or(Object::Nil);
            self.slots.push(value);
        }
        // 可变参数末槽：把超出固定参数的实参收束为数组（无多余则空数组）。
        if variadic {
            let rest = if args.len() > fixed {
                args[fixed..].to_vec()
            } else {
                Vec::new()
            };
            self.slots.push(Object::array(rest));
        }
        let mut frame = Frame::function(index, base, args.len());
        // 闭包携带的 upvalue 作为本帧 upvalue 槽的前缀，Load/StoreUpvalue 按索引访问。
        // 这些句柄来自外层，abs_pos 用哨兵标记，帧退出时不做关闭处理。
        frame.open_upvalues = upvalues.into_iter().map(|up| (usize::MAX, up)).collect();
        self.frames.push(frame);
        let result = self.execute()?;
        let mut finished = self.frames.pop().expect("函数帧存在");
        // 关闭本帧创建的 open upvalue（abs_pos 非哨兵）：把对应局部槽当前值搬进
        // cell，使闭包在槽位回收后仍持有该变量；来自外层的入参 upvalue 原样保留。
        for (abs_pos, upvalue) in finished.open_upvalues.drain(..) {
            if abs_pos != usize::MAX {
                if let Some(value) = self.slots.get(abs_pos).cloned() {
                    *upvalue.borrow_mut() = value;
                }
            }
        }
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

    /// 读取当前帧槽位中的对象。
    fn slot(&self, frame_base: usize, slot: usize) -> Object {
        self.slots
            .get(frame_base + slot)
            .cloned()
            .unwrap_or(Object::Nil)
    }

    /// 写入当前帧槽位，槽位不足时扩展；若该槽已被闭包以 open upvalue 捕获，
    /// 同步刷新共享 cell，使帧内赋值对所有持有该 upvalue 的闭包立即可见。
    fn set_slot(&mut self, frame_base: usize, slot: usize, value: Object) {
        let target = frame_base + slot;
        if target >= self.slots.len() {
            self.slots.resize(target + 1, Object::Nil);
        }
        self.slots[target] = value.clone();
        if let Some(frame) = self.frames.last() {
            let linked: Vec<Upvalue> = frame
                .open_upvalues
                .iter()
                .filter(|(pos, _)| *pos == target)
                .map(|(_, up)| Rc::clone(up))
                .collect();
            for up in linked {
                *up.borrow_mut() = value.clone();
            }
        }
    }

    /// 取当前帧的第 `idx` 个 upvalue 句柄（含闭包入参与本帧新建的 open upvalue）。
    fn frame_upvalue(&self, idx: usize) -> VmResult<Upvalue> {
        let frame = self.frames.last().ok_or_else(|| VmError::new("无调用帧"))?;
        frame
            .open_upvalues
            .get(idx)
            .map(|(_, up)| Rc::clone(up))
            .ok_or_else(|| VmError::new(format!("upvalue 索引 {idx} 越界")))
    }

    /// 捕获当前帧的局部槽 `slot`：若已有 open 句柄则复用，否则新建并登记。
    ///
    /// 复用保证同一槽位被多个闭包捕获时共享同一个 cell，写入彼此可见。
    fn capture_local(&mut self, slot: u16) -> VmResult<Upvalue> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| VmError::new("无调用帧"))?;
        let base = frame.slot_base;
        let abs_pos = base + slot as usize;
        if let Some((_, up)) = frame.open_upvalues.iter().find(|(pos, _)| *pos == abs_pos) {
            return Ok(Rc::clone(up));
        }
        let initial = self.slots.get(abs_pos).cloned().unwrap_or(Object::Nil);
        let up: Upvalue = Rc::new(RefCell::new(initial));
        frame.open_upvalues.push((abs_pos, Rc::clone(&up)));
        Ok(up)
    }

    /// 复用当前帧已有的第 `idx` 个 upvalue 句柄（跨多层嵌套时透传）。
    fn capture_upvalue(&self, idx: u16) -> VmResult<Upvalue> {
        self.frame_upvalue(idx as usize)
    }

    /// 下标读取，语义对齐树遍历求值器：数组按整数下标、字符串产字符、
    /// 字典按键查找（缺失返回空）。
    fn get_index(&self, target: &Object, index: &Object) -> VmResult<Object> {
        match target.unwrap_named() {
            Object::Array(arr) => {
                let Some(i) = index.as_int() else {
                    return Err(VmError::new(format!(
                        "数组下标必须是整数，实际是 {}",
                        index.kind()
                    )));
                };
                let len = arr.len() as i64;
                if i < 0 || i >= len {
                    return Err(VmError::new(format!(
                        "数组下标越界：长度 {}，下标 {}",
                        len, i
                    )));
                }
                Ok(arr.get(i as usize).unwrap_or(Object::Nil))
            }
            Object::Str(text) => {
                let Some(i) = index.as_int() else {
                    return Err(VmError::new(format!(
                        "字符串下标必须是整数，实际是 {}",
                        index.kind()
                    )));
                };
                let chars: Vec<char> = text.chars().collect();
                if i < 0 || i as usize >= chars.len() {
                    return Err(VmError::new(format!(
                        "字符串下标越界：长度 {}，下标 {}",
                        chars.len(),
                        i
                    )));
                }
                Ok(Object::Char(chars[i as usize]))
            }
            Object::Dict(dict) => {
                if !index.is_dict_key() {
                    return Err(VmError::new(format!(
                        "不支持的字典键类型：{}",
                        index.kind()
                    )));
                }
                Ok(dict.get(index).unwrap_or(Object::Nil))
            }
            other => Err(VmError::new(format!("{} 不支持下标访问", other.kind()))),
        }
    }

    /// 下标写入：数组就地替换、字典按键写入。
    fn set_index(&self, target: &Object, index: &Object, value: Object) -> VmResult<()> {
        match target.unwrap_named() {
            Object::Array(arr) => {
                let Some(i) = index.as_int() else {
                    return Err(VmError::new(format!(
                        "数组下标必须是整数，实际是 {}",
                        index.kind()
                    )));
                };
                let len = arr.len() as i64;
                if i < 0 || i >= len {
                    return Err(VmError::new(format!(
                        "数组下标越界：长度 {}，下标 {}",
                        len, i
                    )));
                }
                arr.set(i as usize, value);
                Ok(())
            }
            Object::Dict(mut dict) => {
                if !index.is_dict_key() {
                    return Err(VmError::new(format!(
                        "不支持的字典键类型：{}",
                        index.kind()
                    )));
                }
                dict.set(index.clone(), value);
                Ok(())
            }
            other => Err(VmError::new(format!("{} 不支持下标赋值", other.kind()))),
        }
    }

    /// 成员读取：字典按键（键优先）、实例按自身字段。
    ///
    /// M3 仅覆盖数据成员；方法分派、嵌入提升与命名类型方法在后续里程碑接入。
    fn get_member(&self, object: &Object, name: &str) -> VmResult<Object> {
        match object.unwrap_named() {
            Object::Dict(dict) => {
                if let Some(v) = dict.get(&Object::string(name)) {
                    return Ok(v);
                }
                Err(VmError::new(format!("字典不存在键 {name}")))
            }
            Object::Instance(inst) => match inst.get(name) {
                Some(v) => Ok(v),
                None => Err(VmError::new(format!("{} 没有字段 {name}", inst.class.name))),
            },
            other => Err(VmError::new(format!("{} 不支持成员访问", other.kind()))),
        }
    }

    /// 成员写入：字典按键写入、实例写自身字段。
    fn set_member(&self, object: &Object, name: &str, value: Object) -> VmResult<()> {
        match object.unwrap_named() {
            Object::Dict(mut dict) => {
                dict.set(Object::string(name), value);
                Ok(())
            }
            Object::Instance(inst) => {
                inst.set(name, value);
                Ok(())
            }
            other => Err(VmError::new(format!("{} 不支持字段赋值", other.kind()))),
        }
    }

    /// 切片：边界为 Nil 时取默认（下界 0、上界长度），并按树遍历语义 clamp。
    fn get_slice(&self, target: &Object, low: &Object, high: &Object) -> VmResult<Object> {
        let len = match target.unwrap_named() {
            Object::Array(a) => a.len() as i64,
            Object::Str(s) => s.chars().count() as i64,
            other => return Err(VmError::new(format!("{} 不支持切片", other.kind()))),
        };
        let low = match low {
            Object::Nil => 0,
            v => v
                .as_int()
                .ok_or_else(|| VmError::new("切片下界必须是整数"))?,
        };
        let high = match high {
            Object::Nil => len,
            v => v
                .as_int()
                .ok_or_else(|| VmError::new("切片上界必须是整数"))?,
        };
        let low = low.clamp(0, len);
        let high = high.clamp(low, len);
        match target.unwrap_named() {
            Object::Array(a) => {
                let mut out = Vec::new();
                for i in low..high {
                    if let Some(v) = a.get(i as usize) {
                        out.push(v);
                    }
                }
                Ok(Object::array(out))
            }
            Object::Str(s) => {
                let out: String = s
                    .chars()
                    .skip(low as usize)
                    .take((high - low) as usize)
                    .collect();
                Ok(Object::string(out))
            }
            _ => unreachable!("切片目标已在前面校验"),
        }
    }

    /// 遍历循环步进：按类型标签推进计数器，把下一轮元素/序号绑定到槽位，
    /// 并在栈顶压入“是否还有下一轮”的布尔值。
    fn for_step(
        &mut self,
        tag: u8,
        iter_slot: usize,
        counter_slot: usize,
        elem_slot: usize,
        index_slot: usize,
    ) -> VmResult<()> {
        let base = self.frames.last().expect("帧存在").slot_base;
        let iterable = self.slot(base, iter_slot);
        let counter = self.slot(base, counter_slot).as_int().unwrap_or(0);
        let has_more = match tag {
            fan_code::opcode::FOR_TAG_ARRAY => {
                let Object::Array(arr) = iterable else {
                    return Err(VmError::new(format!(
                        "遍历目标应为数组，实际是 {}",
                        iterable.kind()
                    )));
                };
                let has_more = (counter as usize) < arr.len();
                if has_more {
                    let idx = counter as usize;
                    self.set_slot(base, elem_slot, arr.get(idx).unwrap_or(Object::Nil));
                    if (index_slot as u16) != fan_code::opcode::FOR_NO_INDEX_SLOT {
                        self.set_slot(base, index_slot, Object::Int(counter));
                    }
                    self.set_slot(base, counter_slot, Object::Int(counter + 1));
                }
                has_more
            }
            fan_code::opcode::FOR_TAG_STRING => {
                let Object::Str(text) = iterable else {
                    return Err(VmError::new(format!(
                        "遍历目标应为字符串，实际是 {}",
                        iterable.kind()
                    )));
                };
                let chars: Vec<String> = text.chars().map(|c| c.to_string()).collect();
                let has_more = (counter as usize) < chars.len();
                if has_more {
                    let idx = counter as usize;
                    self.set_slot(base, elem_slot, Object::string(chars[idx].clone()));
                    if (index_slot as u16) != fan_code::opcode::FOR_NO_INDEX_SLOT {
                        self.set_slot(base, index_slot, Object::Int(counter));
                    }
                    self.set_slot(base, counter_slot, Object::Int(counter + 1));
                }
                has_more
            }
            fan_code::opcode::FOR_TAG_DICT => {
                let Object::Dict(dict) = iterable else {
                    return Err(VmError::new(format!(
                        "遍历目标应为字典，实际是 {}",
                        iterable.kind()
                    )));
                };
                let keys = dict.keys();
                let has_more = (counter as usize) < keys.len();
                if has_more {
                    let idx = counter as usize;
                    let key = keys[idx].clone();
                    let value = dict.get(&key).unwrap_or(Object::Nil);
                    // 双变量时首槽绑定键、次槽绑定值；单变量（序号槽为哨兵）
                    // 仅元素槽绑定值。
                    if (index_slot as u16) == fan_code::opcode::FOR_NO_INDEX_SLOT {
                        self.set_slot(base, elem_slot, value);
                    } else {
                        self.set_slot(base, elem_slot, key);
                        self.set_slot(base, index_slot, value);
                    }
                    self.set_slot(base, counter_slot, Object::Int(counter + 1));
                }
                has_more
            }
            other => return Err(VmError::new(format!("未知遍历类型标签：{other}"))),
        };
        self.stack.push(Object::Bool(has_more));
        Ok(())
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
