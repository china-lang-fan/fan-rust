//! 字节码的数据结构：常量、代码块、函数块与模块块。
//!
//! 这些类型是编译期（fan-code）与运行期（fan-vm）之间的稳定契约，只承载可
//! 序列化的纯数据，不引用任何 AST 或运行时对象。运行时对象（`Object`）由
//! fan-vm 在加载产物时把 [`ConstValue`] 装箱得到。

use crate::opcode::Opcode;

/// 魔数：凡语言编译产物（`.fanc` / `.凡码`）的文件头标识。
pub const MAGIC: [u8; 4] = *b"FANC";
/// 字节码格式版本。破坏性变更时 +1，旧产物将被拒绝并回退源码重编译。
pub const BYTECODE_VERSION: u16 = 1;
/// 项目清单文件名，用于上溯定位项目根（与 fan-mod 的约定保持一致）。
pub const MANIFEST_MARKER: &str = "fan.toml";

/// 常量池中可出现的字面量。仅包含可被静态序列化的纯值。
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Byte(u8),
    Char(char),
    Str(String),
}

/// 函数参数形状。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Arity {
    /// 固定位置参数的个数。
    pub fixed: usize,
    /// 是否带可变参数（收束为数组）。
    pub variadic: bool,
    /// 带默认值的参数个数（默认值表达式在函数体内以专门初始化指令实现）。
    pub optional: usize,
}

/// 一个可执行代码块：扁平指令流 + 常量池 + 每条指令对应的源码行。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chunk {
    /// 操作码与操作数拼接而成的字节流。
    pub code: Vec<u8>,
    /// 常量池，指令以索引引用。
    pub constants: Vec<ConstValue>,
    /// 与 `code` 中每条指令起始位置平行的源码行（长度等于指令条数而非字节数）。
    pub lines: Vec<u32>,
}

/// 闭包捕获的上层变量描述。
#[derive(Debug, Clone, PartialEq)]
pub struct UpvalueDesc {
    /// true 表示捕获自直接外层帧的局部槽位；false 表示捕获自外层的 upvalue。
    pub from_local: bool,
    /// 局部槽位索引或外层 upvalue 索引。
    pub index: u16,
}

/// 函数（含方法、匿名函数、分段函数）编译后的块。
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionChunk {
    /// 函数名（分段函数为拼接全名），用于报错与栈追踪。
    pub name: String,
    pub arity: Arity,
    /// 参数名，顺序与固定参数一致，供调试与局部槽位分配参考。
    pub params: Vec<String>,
    pub body: Chunk,
    pub upvalues: Vec<UpvalueDesc>,
}

/// 一个源文件（模块）编译后的完整产物。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleChunk {
    /// 模块顶层语句对应的入口块。
    pub entry: Chunk,
    /// 模块内定义的全部函数，`MakeClosure` 以索引引用。
    pub functions: Vec<FunctionChunk>,
    /// 导出的顶层绑定名。
    pub exports: Vec<String>,
    /// 静态记录的导入模块路径（实际加载策略由运行时决定）。
    pub imports: Vec<String>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk::default()
    }
}

/// 代码生成期的辅助构造器：负责写指令、追加常量、维护行号。
pub struct ChunkBuilder {
    chunk: Chunk,
    current_line: u32,
}

impl ChunkBuilder {
    pub fn new() -> Self {
        ChunkBuilder {
            chunk: Chunk::new(),
            current_line: 0,
        }
    }

    /// 设置后续写入指令关联的源码行。
    pub fn set_line(&mut self, line: u32) {
        self.current_line = line;
    }

    /// 写入一条指令及其定宽操作数。
    pub fn emit(&mut self, op: Opcode) -> usize {
        let offset = self.chunk.code.len();
        self.chunk.code.push(op.code());
        self.chunk.lines.push(self.current_line);
        match &op {
            Opcode::Const(v)
            | Opcode::LoadLocal(v)
            | Opcode::StoreLocal(v)
            | Opcode::LoadGlobal(v)
            | Opcode::StoreGlobal(v)
            | Opcode::LoadUpvalue(v)
            | Opcode::StoreUpvalue(v)
            | Opcode::MakeClosure(v)
            | Opcode::GetMember(v)
            | Opcode::SetMember(v)
            | Opcode::BuildArray(v)
            | Opcode::BuildDict(v)
            | Opcode::BuildTuple(v)
            | Opcode::LoadModule(v) => self.chunk.code.extend_from_slice(&v.to_le_bytes()),
            Opcode::Jump(v) | Opcode::JumpIfFalse(v) | Opcode::JumpIfTrue(v) => {
                self.chunk.code.extend_from_slice(&v.to_le_bytes())
            }
            Opcode::Call(argc) => self.chunk.code.push(*argc),
            Opcode::CallNative(idx, argc)
            | Opcode::MixfixCall(idx, argc)
            | Opcode::InvokeMethod(idx, argc) => {
                self.chunk.code.extend_from_slice(&idx.to_le_bytes());
                self.chunk.code.push(*argc);
            }
            Opcode::Halt
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::GetIndex
            | Opcode::SetIndex
            | Opcode::Return
            | Opcode::Throw
            | Opcode::Recover => {}
        }
        offset
    }

    /// 追加常量并返回其在常量池中的索引。
    pub fn add_constant(&mut self, value: ConstValue) -> u16 {
        if let Some(idx) = self.chunk.constants.iter().position(|c| c == &value) {
            return idx as u16;
        }
        let idx = self.chunk.constants.len();
        assert!(idx <= u16::MAX as usize, "常量池超出 u16 上限");
        self.chunk.constants.push(value);
        idx as u16
    }

    /// 把跳转指令在 `offset` 处的操作数回填为当前代码长度。
    pub fn patch_jump(&mut self, offset: usize) {
        let target = self.chunk.code.len() as u32;
        let op = self.chunk.code[offset];
        debug_assert!(
            matches!(
                Opcode::from_code(op),
                Some(Opcode::Jump(_)) | Some(Opcode::JumpIfFalse(_)) | Some(Opcode::JumpIfTrue(_))
            ),
            "回填目标必须是跳转指令"
        );
        let bytes = target.to_le_bytes();
        self.chunk.code[offset + 1..offset + 5].copy_from_slice(&bytes);
    }

    /// 当前指令流字节长度，用于计算跳转目标。
    pub fn code_len(&self) -> usize {
        self.chunk.code.len()
    }

    pub fn build(self) -> Chunk {
        self.chunk
    }
}

impl Default for ChunkBuilder {
    fn default() -> Self {
        Self::new()
    }
}
