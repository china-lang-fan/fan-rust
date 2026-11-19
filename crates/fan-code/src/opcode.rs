//! 字节码指令集。
//!
//! 采用栈式虚拟机：绝大多数指令从值栈弹出操作数、把结果压回值栈。每条指令占
//! 一个字节的操作码，后接定宽操作数（小端序）。操作码编号一经发布即固定，
//! 新增指令只能追加在末尾、不得重排，以保证 `.fanc`/`.凡码` 产物的前向兼容。
//!
//! 操作数宽度约定：
//! - `u8`：1 字节，常用于参数个数等小计数；
//! - `u16`：2 字节，常用于常量池/槽位/函数表索引；
//! - `u32`：4 字节，常用于指令流内的绝对跳转偏移。

/// 单条字节码指令。变体上的字段即该指令的定宽操作数。
#[derive(Debug, Clone, PartialEq)]
#[repr(u8)]
pub enum Opcode {
    /// 停止执行，值栈顶即模块/帧的结果。
    Halt = 0,
    /// 把常量池 `[u16]` 处的常量压栈。
    Const(u16) = 1,
    /// 弹出栈顶并丢弃（表达式语句的结果）。
    Pop = 2,
    /// 复制栈顶再压一份。
    Dup = 3,

    /// 压入局部槽位 `[u16]` 的值。
    LoadLocal(u16) = 4,
    /// 弹出栈顶写入局部槽位 `[u16]`。
    StoreLocal(u16) = 5,
    /// 按常量池 `[u16]` 中的名字读取全局/模块绑定。
    LoadGlobal(u16) = 6,
    /// 弹出栈顶，按常量池 `[u16]` 中的名字写入全局/模块绑定。
    StoreGlobal(u16) = 7,
    /// 压入闭包捕获变量（upvalue）`[u16]`。
    LoadUpvalue(u16) = 8,
    /// 弹出栈顶写入 upvalue `[u16]`。
    StoreUpvalue(u16) = 9,

    /// 无条件跳转到绝对偏移 `[u32]`。
    Jump(u32) = 10,
    /// 栈顶为假（假/空）时跳转到 `[u32]`，不弹栈。
    JumpIfFalse(u32) = 11,
    /// 栈顶为真时跳转到 `[u32]`，不弹栈。
    JumpIfTrue(u32) = 12,

    /// 调用栈顶可调用对象，参数个数 `[u8]`（参数已在栈上，顺序入栈）。
    Call(u8) = 13,
    /// 调用原生内建：名字取常量池 `[u16]`，参数个数 `[u8]`。
    CallNative(u16, u8) = 14,
    /// 从当前帧返回，返回值已在栈顶。
    Return = 15,
    /// 用模块函数表 `[u16]` 处的函数体配合 upvalue 构造闭包并压栈。
    MakeClosure(u16) = 16,

    /// 弹出对象，按常量池 `[u16]` 中的成员名取值并压栈。
    GetMember(u16) = 17,
    /// 弹出值与对象，按成员名常量 `[u16]` 写入对象成员。
    SetMember(u16) = 18,
    /// 弹出下标与对象，压入 `对象[下标]`。
    GetIndex = 19,
    /// 弹出值、下标、对象，执行 `对象[下标] = 值`。
    SetIndex = 20,

    /// 弹出 `[u16]` 个元素，构造数组压栈。
    BuildArray(u16) = 21,
    /// 弹出 `[u16]` 对键值，构造字典压栈。
    BuildDict(u16) = 22,
    /// 弹出 `[u16]` 个元素，构造元组压栈。
    BuildTuple(u16) = 23,

    /// 分段（mixfix）函数调用：根段名取常量池 `[u16]`，参数个数 `[u8]`。
    ///
    /// 语法消歧已由 parser 完成；此指令只负责运行时按参数动态类型重建签名、
    /// 在 `mixfix_functions` 表中做重载分派，与树遍历求值器的行为对齐。
    MixfixCall(u16, u8) = 24,
    /// 方法调用：方法名取常量池 `[u16]`，参数个数 `[u8]`（接收者在参数之下）。
    InvokeMethod(u16, u8) = 25,

    /// 抛出栈顶的错误值（对应 `崩溃了`）。
    Throw = 26,
    /// 压入当前待恢复的错误值（对应 `恢复`），无则压入空。
    Recover = 27,
    /// 导入模块：模块路径取常量池 `[u16]`，把模块对象压栈。
    LoadModule(u16) = 28,
    /// 二元运算：运算符文本取常量池 `[u16]`，弹出右、左操作数后压入结果。
    ///
    /// 运算符语义（含中文运算符与字符串/数组拼接等）由 fan-vm 集中实现，避免
    /// 为每个运算符分配一条指令。
    BinOp(u16) = 29,
    /// 一元运算：运算符文本取常量池 `[u16]`，弹出操作数后压入结果。
    UnaryOp(u16) = 30,
    /// 栈顶为函数时零参调用，否则原样保留（对应无括号 `MaybeCall` 的运行期判定）。
    CallIfFunc = 31,
}

impl Opcode {
    /// 该指令（不含操作码本身）的操作数字节数。
    pub fn operand_size(&self) -> usize {
        match self {
            Opcode::Halt
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::GetIndex
            | Opcode::SetIndex
            | Opcode::Return
            | Opcode::Throw
            | Opcode::Recover
            | Opcode::CallIfFunc => 0,

            Opcode::Const(_)
            | Opcode::LoadLocal(_)
            | Opcode::StoreLocal(_)
            | Opcode::LoadGlobal(_)
            | Opcode::StoreGlobal(_)
            | Opcode::LoadUpvalue(_)
            | Opcode::StoreUpvalue(_)
            | Opcode::MakeClosure(_)
            | Opcode::GetMember(_)
            | Opcode::SetMember(_)
            | Opcode::BuildArray(_)
            | Opcode::BuildDict(_)
            | Opcode::BuildTuple(_)
            | Opcode::LoadModule(_)
            | Opcode::BinOp(_)
            | Opcode::UnaryOp(_) => 2,

            Opcode::Jump(_) | Opcode::JumpIfFalse(_) | Opcode::JumpIfTrue(_) => 4,

            Opcode::Call(_) => 1,
            Opcode::CallNative(_, argc)
            | Opcode::MixfixCall(_, argc)
            | Opcode::InvokeMethod(_, argc) => {
                let _ = argc;
                3
            }
        }
    }

    /// 操作码的固定字节编号。
    pub fn code(&self) -> u8 {
        // 安全：枚举带显式 `#[repr(u8)]` 且所有变体都有 discriminant。
        unsafe { *<*const _>::from(self).cast::<u8>() }
    }

    /// 按编号读取操作码，再由调用方按 `operand_size` 读取后续操作数。
    pub fn from_code(code: u8) -> Option<Opcode> {
        match code {
            0 => Some(Opcode::Halt),
            1 => Some(Opcode::Const(0)),
            2 => Some(Opcode::Pop),
            3 => Some(Opcode::Dup),
            4 => Some(Opcode::LoadLocal(0)),
            5 => Some(Opcode::StoreLocal(0)),
            6 => Some(Opcode::LoadGlobal(0)),
            7 => Some(Opcode::StoreGlobal(0)),
            8 => Some(Opcode::LoadUpvalue(0)),
            9 => Some(Opcode::StoreUpvalue(0)),
            10 => Some(Opcode::Jump(0)),
            11 => Some(Opcode::JumpIfFalse(0)),
            12 => Some(Opcode::JumpIfTrue(0)),
            13 => Some(Opcode::Call(0)),
            14 => Some(Opcode::CallNative(0, 0)),
            15 => Some(Opcode::Return),
            16 => Some(Opcode::MakeClosure(0)),
            17 => Some(Opcode::GetMember(0)),
            18 => Some(Opcode::SetMember(0)),
            19 => Some(Opcode::GetIndex),
            20 => Some(Opcode::SetIndex),
            21 => Some(Opcode::BuildArray(0)),
            22 => Some(Opcode::BuildDict(0)),
            23 => Some(Opcode::BuildTuple(0)),
            24 => Some(Opcode::MixfixCall(0, 0)),
            25 => Some(Opcode::InvokeMethod(0, 0)),
            26 => Some(Opcode::Throw),
            27 => Some(Opcode::Recover),
            28 => Some(Opcode::LoadModule(0)),
            29 => Some(Opcode::BinOp(0)),
            30 => Some(Opcode::UnaryOp(0)),
            31 => Some(Opcode::CallIfFunc),
            _ => None,
        }
    }
}
