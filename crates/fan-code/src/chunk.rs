//! 字节码的数据结构：常量、代码块、函数块与模块块。
//!
//! 这些类型是编译期（fan-code）与运行期（fan-vm）之间的稳定契约，只承载可
//! 序列化的纯数据，不引用任何 AST 或运行时对象。运行时对象（`Object`）由
//! fan-vm 在加载产物时把 [`ConstValue`] 装箱得到。

use crate::opcode::Opcode;

/// 魔数：凡语言编译产物（`.fanc` / `.凡码`）的文件头标识。
pub const MAGIC: [u8; 4] = *b"FANC";
/// 字节码格式版本。破坏性变更时 +1，旧产物将被拒绝并回退源码重编译。
pub const BYTECODE_VERSION: u16 = 6;
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
    /// 被捕获变量的源码名，仅用于编译期去重，运行期按索引访问。
    pub name: String,
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
    /// 各参数的声明类型名（空串表示未标注 any），与 params 对齐；用于接口签名比对。
    pub param_types: Vec<String>,
    /// 返回值声明类型名（空串表示未标注），用于接口签名比对。
    pub return_types: Vec<String>,
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
    /// 模块内定义的全部模型（类），`BindClass` 以索引引用。
    pub classes: Vec<ClassChunk>,
    /// 模块内定义的模型方法，`BindClass` 之后由 VM 装配到字节码方法表。
    pub methods: Vec<MethodChunk>,
    /// 模块内声明的接口（纯元数据），VM 链接模块时注册到 Runtime 并展平嵌入。
    pub interfaces: Vec<InterfaceChunk>,
    /// 模块内声明的自定义命名类型，VM 链接模块时注册到 Runtime 并支持类型名构造调用。
    pub named_types: Vec<NamedTypeChunk>,
    /// 模块内函数声明上出现的标签（装饰器），`ApplyTag` 以索引引用。
    /// 仅承载可序列化的纯数据：标签名、至多一个位置参数表达式、命名参数表达式，
    /// 表达式在所属入口块的常量/指令中以独立小代码块编译，运行时求值后构造标签实例。
    pub tags: Vec<TagChunk>,
    /// 公开导出的原生别名：(凡语函数名, Runtime 原生 spec 名)。
    /// `定义 公开 本地(...)` 的别名不占运行期槽值，加载器据此导出真实 Builtin。
    pub native_exports: Vec<(String, String)>,
    /// 导出的顶层绑定名。
    pub exports: Vec<String>,
    /// 静态记录的导入模块路径（实际加载策略由运行时决定）。
    pub imports: Vec<String>,
    /// 模块入口顶层「名字 → 入口局部槽位」映射，供装配层执行入口后回收导出值。
    pub top_slots: Vec<(String, u16)>,
}

/// 模型（类）编译后的静态元数据。
///
/// 阶段一仅记录字段名（含嵌入模型字段留待后续里程碑），运行期由 VM 据此构造
/// `fan_object::Class` 与零值实例；方法表在方法定义里程碑补充。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClassChunk {
    /// 模型名，用于报错与运行期类对象标识。
    pub name: String,
    /// 自身声明的字段名，按声明顺序；实例化时先据此置零值。
    pub fields: Vec<String>,
    /// 嵌入模型名，按声明顺序；BindClass 时由 VM 解析为对应类对象。
    pub embeds: Vec<String>,
}

/// 模型方法编译产物：方法体作为普通函数进入模块函数表，这里记录定位信息。
///
/// 方法的隐式接收者 `自己` 由编译器在参数表前置一个槽位，VM 调用绑定方法时
/// 把接收者作为首参注入。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MethodChunk {
    /// 所属模型名；基本类型方法为凡语言类型名（如 `字符串`）。
    pub class_name: String,
    /// 方法名。
    pub method_name: String,
    /// 方法体在模块 `functions` 表中的索引。
    pub function_index: u16,
    /// 是否为基本类型（字符串/数组等）扩展方法；VM 按接收者运行时类型登记。
    pub primitive: bool,
}

/// 接口方法签名（无函数体）的可序列化形态。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InterfaceMethodChunk {
    /// 方法名。
    pub name: String,
    /// 参数声明类型名（空串表示未标注），按声明顺序。
    pub param_types: Vec<String>,
    /// 返回值声明类型名（空串表示未标注）。
    pub return_types: Vec<String>,
}

/// 接口声明编译产物：纯元数据，运行期由 VM 注册到 Runtime 做结构化满足判定。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InterfaceChunk {
    /// 接口名。
    pub name: String,
    /// 嵌入的接口名，VM 注册时递归展平。
    pub embeds: Vec<String>,
    /// 自身声明的方法签名。
    pub methods: Vec<InterfaceMethodChunk>,
}

/// 自定义命名类型（`定义 类型 名 = 底层类型`，底层为基本类型或其联合）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NamedTypeChunk {
    /// 类型名。
    pub name: String,
    /// 底层类型的成员名（基本类型名）；联合类型含多个，单类型仅一个。
    pub base_members: Vec<String>,
    /// 是否为联合命名类型（成员数 ≥ 2）。
    pub is_union: bool,
}

/// 标签（装饰器）编译产物：纯元数据，参数表达式不内嵌于此。
///
/// 参数表达式在函数绑定的同一代码块中先于 `ApplyTag` 内联编译求值，由
/// `ApplyTag` 指令按 `has_positional` 与 `named_names` 从值栈收集：位置参数至多
/// 一个（写入标签实例字段 `值`），其余按 `named_names` 同名写入。运行时据此构造
/// 标签模型实例并回调已注册的凡语言处理器；未注册标签静默忽略。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TagChunk {
    /// 标签名（`@名字` 中的名字）。
    pub name: String,
    /// 命名参数的字段名，顺序与栈上排在位置参数之后的各值对齐。
    pub named_names: Vec<String>,
    /// 是否带一个位置参数（其值位于收集序列最前，写入字段 `值`）。
    pub has_positional: bool,
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
            | Opcode::LoadModule(v)
            | Opcode::BinOp(v)
            | Opcode::UnaryOp(v)
            | Opcode::UnpackTuple(v)
            | Opcode::CallNativeMarked(v)
            | Opcode::BindClass(v)
            | Opcode::ReturnLocal(v)
            | Opcode::TypeTest(v)
            | Opcode::TypeCast(v)
            | Opcode::NewNamed(v) => self.chunk.code.extend_from_slice(&v.to_le_bytes()),
            Opcode::Jump(v) | Opcode::JumpIfFalse(v) | Opcode::JumpIfTrue(v) => {
                self.chunk.code.extend_from_slice(&v.to_le_bytes())
            }
            Opcode::Call(argc) => self.chunk.code.push(*argc),
            Opcode::CallNative(idx, argc)
            | Opcode::MixfixCall(idx, argc)
            | Opcode::InvokeMethod(idx, argc)
            | Opcode::NewInstance(idx, argc) => {
                self.chunk.code.extend_from_slice(&idx.to_le_bytes());
                self.chunk.code.push(*argc);
            }
            Opcode::ApplyTag(tag_idx, slot, argc) => {
                self.chunk.code.extend_from_slice(&tag_idx.to_le_bytes());
                self.chunk.code.extend_from_slice(&slot.to_le_bytes());
                self.chunk.code.push(*argc);
            }
            Opcode::Halt
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::GetIndex
            | Opcode::SetIndex
            | Opcode::Return
            | Opcode::Throw
            | Opcode::Recover
            | Opcode::CallIfFunc
            | Opcode::ArgCount
            | Opcode::GetSlice
            | Opcode::ToStr
            | Opcode::PushMark
            | Opcode::Spread
            | Opcode::SpreadDict
            | Opcode::BuildArrayMarked
            | Opcode::BuildDictMarked
            | Opcode::CallMarked
            | Opcode::RegisterDefer => {}
            Opcode::ForStep(tag, iterable, counter, elem, index_slot) => {
                self.chunk.code.push(*tag);
                self.chunk.code.extend_from_slice(&iterable.to_le_bytes());
                self.chunk.code.extend_from_slice(&counter.to_le_bytes());
                self.chunk.code.extend_from_slice(&elem.to_le_bytes());
                self.chunk.code.extend_from_slice(&index_slot.to_le_bytes());
            }
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
        self.patch_jump_to(offset, target as usize);
    }

    /// 把跳转指令在 `offset` 处的操作数回填为指定目标偏移。
    pub fn patch_jump_to(&mut self, offset: usize, target: usize) {
        let op = self.chunk.code[offset];
        debug_assert!(
            matches!(
                Opcode::from_code(op),
                Some(Opcode::Jump(_)) | Some(Opcode::JumpIfFalse(_)) | Some(Opcode::JumpIfTrue(_))
            ),
            "回填目标必须是跳转指令"
        );
        let bytes = (target as u32).to_le_bytes();
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
