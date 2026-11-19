//! 字节码反汇编：把 [`Chunk`] 的指令流解码为可读文本，供调试与测试。

use crate::chunk::Chunk;
use crate::opcode::Opcode;

/// 一条反汇编后的指令（偏移、源码行、文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisasmLine {
    pub offset: usize,
    pub line: u32,
    pub text: String,
}

/// 读取 `code[start..]` 开头的完整指令，返回指令与它占用的总字节数。
fn decode_instruction(code: &[u8], start: usize) -> Result<(Opcode, usize), String> {
    let byte = *code
        .get(start)
        .ok_or_else(|| format!("偏移 {start} 处缺少操作码"))?;
    let template =
        Opcode::from_code(byte).ok_or_else(|| format!("偏移 {start} 处存在未知操作码 {byte}"))?;
    let operand_size = template.operand_size();
    let operand = code
        .get(start + 1..start + 1 + operand_size)
        .ok_or_else(|| format!("偏移 {start} 处指令操作数不完整"))?;

    let u16_at = |i: usize| u16::from_le_bytes([operand[i], operand[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([operand[i], operand[i + 1], operand[i + 2], operand[i + 3]]);

    let op = match template {
        Opcode::Halt => Opcode::Halt,
        Opcode::Const(_) => Opcode::Const(u16_at(0)),
        Opcode::Pop => Opcode::Pop,
        Opcode::Dup => Opcode::Dup,
        Opcode::LoadLocal(_) => Opcode::LoadLocal(u16_at(0)),
        Opcode::StoreLocal(_) => Opcode::StoreLocal(u16_at(0)),
        Opcode::LoadGlobal(_) => Opcode::LoadGlobal(u16_at(0)),
        Opcode::StoreGlobal(_) => Opcode::StoreGlobal(u16_at(0)),
        Opcode::LoadUpvalue(_) => Opcode::LoadUpvalue(u16_at(0)),
        Opcode::StoreUpvalue(_) => Opcode::StoreUpvalue(u16_at(0)),
        Opcode::Jump(_) => Opcode::Jump(u32_at(0)),
        Opcode::JumpIfFalse(_) => Opcode::JumpIfFalse(u32_at(0)),
        Opcode::JumpIfTrue(_) => Opcode::JumpIfTrue(u32_at(0)),
        Opcode::Call(_) => Opcode::Call(operand[0]),
        Opcode::CallNative(_, _) => Opcode::CallNative(u16_at(0), operand[2]),
        Opcode::Return => Opcode::Return,
        Opcode::MakeClosure(_) => Opcode::MakeClosure(u16_at(0)),
        Opcode::GetMember(_) => Opcode::GetMember(u16_at(0)),
        Opcode::SetMember(_) => Opcode::SetMember(u16_at(0)),
        Opcode::GetIndex => Opcode::GetIndex,
        Opcode::SetIndex => Opcode::SetIndex,
        Opcode::BuildArray(_) => Opcode::BuildArray(u16_at(0)),
        Opcode::BuildDict(_) => Opcode::BuildDict(u16_at(0)),
        Opcode::BuildTuple(_) => Opcode::BuildTuple(u16_at(0)),
        Opcode::MixfixCall(_, _) => Opcode::MixfixCall(u16_at(0), operand[2]),
        Opcode::InvokeMethod(_, _) => Opcode::InvokeMethod(u16_at(0), operand[2]),
        Opcode::Throw => Opcode::Throw,
        Opcode::Recover => Opcode::Recover,
        Opcode::LoadModule(_) => Opcode::LoadModule(u16_at(0)),
    };
    Ok((op, 1 + operand_size))
}

/// 反汇编整个代码块。
pub fn disassemble(chunk: &Chunk) -> Result<Vec<DisasmLine>, String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    let mut instr_index = 0usize;
    while offset < chunk.code.len() {
        let (op, size) = decode_instruction(&chunk.code, offset)?;
        let line = chunk.lines.get(instr_index).copied().unwrap_or_default();
        lines.push(DisasmLine {
            offset,
            line,
            text: render(&op, chunk),
        });
        offset += size;
        instr_index += 1;
    }
    Ok(lines)
}

/// 渲染单条指令，索引类指令附带常量池内容便于阅读。
fn render(op: &Opcode, chunk: &Chunk) -> String {
    let const_str = |idx: u16| match chunk.constants.get(idx as usize) {
        Some(c) => format!("{c:?}"),
        None => "<越界常量>".to_string(),
    };
    match op {
        Opcode::Const(i) => format!("CONST {i} ；{}", const_str(*i)),
        Opcode::LoadLocal(i) => format!("LOAD_LOCAL {i}"),
        Opcode::StoreLocal(i) => format!("STORE_LOCAL {i}"),
        Opcode::LoadGlobal(i) => format!("LOAD_GLOBAL {i} ；{}", const_str(*i)),
        Opcode::StoreGlobal(i) => format!("STORE_GLOBAL {i} ；{}", const_str(*i)),
        Opcode::LoadUpvalue(i) => format!("LOAD_UPVALUE {i}"),
        Opcode::StoreUpvalue(i) => format!("STORE_UPVALUE {i}"),
        Opcode::Jump(t) => format!("JUMP -> {t}"),
        Opcode::JumpIfFalse(t) => format!("JUMP_IF_FALSE -> {t}"),
        Opcode::JumpIfTrue(t) => format!("JUMP_IF_TRUE -> {t}"),
        Opcode::Call(n) => format!("CALL argc={n}"),
        Opcode::CallNative(i, n) => format!("CALL_NATIVE {i} argc={n} ；{}", const_str(*i)),
        Opcode::MakeClosure(i) => format!("MAKE_CLOSURE fn[{i}]"),
        Opcode::GetMember(i) => format!("GET_MEMBER {i} ；{}", const_str(*i)),
        Opcode::SetMember(i) => format!("SET_MEMBER {i} ；{}", const_str(*i)),
        Opcode::BuildArray(n) => format!("BUILD_ARRAY {n}"),
        Opcode::BuildDict(n) => format!("BUILD_DICT {n}"),
        Opcode::BuildTuple(n) => format!("BUILD_TUPLE {n}"),
        Opcode::MixfixCall(i, n) => format!("MIXFIX_CALL {i} argc={n} ；{}", const_str(*i)),
        Opcode::InvokeMethod(i, n) => format!("INVOKE_METHOD {i} argc={n} ；{}", const_str(*i)),
        Opcode::LoadModule(i) => format!("LOAD_MODULE {i} ；{}", const_str(*i)),
        other => format!("{other:?}"),
    }
}
