//! 栈式虚拟机的取指-译码-执行循环。
//!
//! M0 只实现最小指令子集（常量、栈操作、局部槽、跳转、返回），足以手编模块块
//! 跑通值的产生与控制流；其余指令随里程碑逐步实现，未实现者显式报错而非静默
//! 误执行。值统一使用 fan-object 的 [`Object`]。

use fan_code::chunk::{Chunk, ModuleChunk};
use fan_code::opcode::Opcode;
use fan_object::Object;

use crate::error::{VmError, VmResult};
use crate::frame::Frame;
use crate::value::const_to_object;

/// 单线程栈式虚拟机。
pub struct Vm<'m> {
    module: &'m ModuleChunk,
    frames: Vec<Frame>,
    stack: Vec<Object>,
    slots: Vec<Object>,
}

impl<'m> Vm<'m> {
    pub fn new(module: &'m ModuleChunk) -> Self {
        Vm {
            module,
            frames: Vec::new(),
            stack: Vec::new(),
            slots: Vec::new(),
        }
    }

    /// 执行模块入口块，返回入口的最终值（无结果时为空）。
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

    fn execute(&mut self) -> VmResult<Object> {
        loop {
            let chunk = self.current_chunk().clone();
            let frame = self.frames.last_mut().expect("执行时必须存在调用帧");
            let offset = frame.ip;
            let byte = *chunk
                .code
                .get(offset)
                .ok_or_else(|| VmError::new("指令流意外结束，缺少 Halt/Return"))?;
            let template = Opcode::from_code(byte)
                .ok_or_else(|| VmError::new(format!("未知操作码 {byte}")).at(offset, None))?;
            frame.ip += 1;

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
                    let frame = self.frames.last().expect("帧存在");
                    let value = self
                        .slots
                        .get(frame.slot_base + idx)
                        .cloned()
                        .ok_or_else(|| VmError::new(format!("局部槽位 {idx} 越界")))?;
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
                other => {
                    return Err(VmError::new(format!(
                        "指令 {other:?} 尚未在 VM 中实现（M0 子集）"
                    ))
                    .at(offset, chunk.lines.first().copied()));
                }
            }
        }
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
}

/// 真值规则与树遍历求值器一致：只有 `假` 与 `空` 为假，其余皆真。
fn is_truthy(value: &Object) -> bool {
    !matches!(value, Object::Nil | Object::Bool(false))
}
