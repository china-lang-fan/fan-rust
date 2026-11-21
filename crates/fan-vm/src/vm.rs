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
use fan_object::{Builtin, Object, Runtime, TagContextValue};

use crate::error::{VmError, VmResult};
use crate::frame::Frame;
use crate::loader::ModuleLoader;
use crate::upvalue::{ModuleId, Upvalue};
use crate::value::const_to_object;
use crate::vm_function::{SharedRegistry, VmFunctionRegistry};

/// 主模块在模块表中的固定标识。
pub const MAIN_MODULE: ModuleId = 0;

/// 栈展开哨兵消息：`崩溃了` 触发的控制流用它与真实 VM 错误区分，
/// 沿调用帧上溯时每帧先执行自己的 `稍后` 队列。
const THROW_SENTINEL: &str = "__vm_throw__";

/// 跨 VM 共享的模块表：模块 ID → 模块块（持有所有权）。
pub type SharedModules = Rc<RefCell<HashMap<ModuleId, Rc<ModuleChunk>>>>;

/// 单线程栈式虚拟机。
pub struct Vm<'m> {
    /// 已链接的模块表，主模块固定为 [`MAIN_MODULE`]；主/子 VM 与加载器共享。
    modules: SharedModules,
    /// 当前执行的模块 ID（帧自身也记录所属模块，这里仅为入口运行提供默认值）。
    current_module: ModuleId,
    runtime: Rc<RefCell<Runtime>>,
    frames: Vec<Frame>,
    stack: Vec<Object>,
    /// 展开 `...` 的边界标记：记录压入标记时刻的值栈长度，供 Marked 系列指令结算。
    marks: Vec<usize>,
    /// 每帧独立的局部槽位区，按帧槽位基址切片。
    slots: Vec<Object>,
    globals: HashMap<String, Object>,
    /// 跨模块共享的字节码闭包登记表。
    functions: SharedRegistry,
    /// 已创建的模型对象，按类名登记，供 BindClass 解析嵌入模型与构造期查找。
    class_registry: HashMap<String, Rc<fan_object::Class>>,
    /// 字节码方法表：(类名, 方法名) → (所属模块, 函数表索引)，GetMember 命中时构造绑定闭包。
    method_registry: HashMap<(String, String), (ModuleId, u16)>,
    /// 基本类型扩展方法表：(运行时类型标识, 方法名) → (所属模块, 函数表索引)。
    /// 运行时类型标识取 fan_object::runtime_type_name（如 string/array），命名类型穿透到底层。
    primitive_method_registry: HashMap<(String, String), (ModuleId, u16)>,
    /// 自定义命名类型扩展方法表：(命名类型名, 方法名) → (所属模块, 函数表索引)。
    /// 命名类型方法在成员访问时优先于底层基本类型的同名方法（对齐 tree 查找顺序）。
    named_method_registry: HashMap<(String, String), (ModuleId, u16)>,
    /// 模块加载器；遇到 LoadModule 时按路径解析。未注入则导入语句报错。
    loader: Option<Box<dyn ModuleLoader>>,
    /// 当前沿调用栈传播的崩溃值（`崩溃了` 抛出的规范错误对象）。
    /// 栈展开期间一直保留，直到某帧 `稍后` 中的 `恢复()` 取走（置回 None）。
    pending_throw: Option<Object>,
    /// 主模块借用的生命周期标记（兼容既有 `Vm::new(&module, ..)` 签名）。
    _marker: std::marker::PhantomData<&'m ()>,
}

impl<'m> Vm<'m> {
    pub fn new(module: &'m ModuleChunk, runtime: Rc<RefCell<Runtime>>) -> Self {
        let mut table = HashMap::new();
        let main = Rc::new(module.clone());
        table.insert(MAIN_MODULE, Rc::clone(&main));
        let mut vm = Vm {
            modules: Rc::new(RefCell::new(table)),
            current_module: MAIN_MODULE,
            runtime,
            frames: Vec::new(),
            stack: Vec::new(),
            marks: Vec::new(),
            slots: Vec::new(),
            globals: HashMap::new(),
            functions: VmFunctionRegistry::shared(),
            class_registry: HashMap::new(),
            method_registry: HashMap::new(),
            primitive_method_registry: HashMap::new(),
            named_method_registry: HashMap::new(),
            loader: None,
            pending_throw: None,
            _marker: std::marker::PhantomData,
        };
        // 先登记命名类型，扩展方法登记需据此区分命名类型方法与基本类型方法。
        vm.register_named_types(&main);
        vm.register_primitive_methods(MAIN_MODULE, &main);
        vm.register_interfaces(&main);
        vm
    }

    /// 登记模块内声明的自定义命名类型到 Runtime，供 `是`/`转为` 与类型名构造使用。
    fn register_named_types(&mut self, module: &ModuleChunk) {
        for chunk in &module.named_types {
            let base = fan_ast::DeclType(chunk.base_members.join("|"));
            let nt = fan_object::NamedType::new(chunk.name.clone(), base);
            self.runtime.borrow_mut().register_named_type(nt);
        }
    }

    /// 登记模块内声明的接口到 Runtime，并递归展平嵌入接口的方法集。
    ///
    /// 接口先按原样注册以支持同文件前向引用，再展平嵌入（带环检测、同名签名冲突报错），
    /// 与 tree 求值器 `eval_interface_stmt`/`flatten_interface` 的两阶段注册一致。
    fn register_interfaces(&mut self, module: &ModuleChunk) {
        // 第一阶段：原样注册，使展平能按名解析同模块内前向定义的接口。
        for chunk in &module.interfaces {
            let mut iface = fan_object::Interface::new(chunk.name.clone());
            for m in &chunk.methods {
                iface.methods.insert(
                    m.name.clone(),
                    fan_ast::InterfaceMethodSig {
                        name: m.name.clone(),
                        params: Self::sig_params(m),
                        return_types: Self::sig_types(&m.return_types),
                        pos: fan_ast::Position::default(),
                    },
                );
            }
            iface.embeds = chunk.embeds.clone();
            self.runtime.borrow_mut().register_interface(Rc::new(iface));
        }
        // 第二阶段：逐个展平嵌入并覆盖注册。
        for chunk in &module.interfaces {
            let mut iface = match self.runtime.borrow().lookup_interface(&chunk.name) {
                Some(iface) => (*iface).clone(),
                None => continue,
            };
            if self.flatten_interface(&mut iface, &mut Vec::new()).is_ok() {
                self.runtime.borrow_mut().register_interface(Rc::new(iface));
            }
        }
    }

    /// 把接口方法签名里的参数类型还原为 AST 参数（仅类型参与比对，名字留空）。
    fn sig_params(m: &fan_code::chunk::InterfaceMethodChunk) -> Vec<fan_ast::Parameter> {
        m.param_types
            .iter()
            .map(|t| fan_ast::Parameter {
                name: String::new(),
                type_: fan_ast::DeclType(t.clone()),
                variadic: false,
                default: None,
            })
            .collect()
    }

    /// 把字符串类型名列表还原为 AST 声明类型。
    fn sig_types(types: &[String]) -> Vec<fan_ast::DeclType> {
        types.iter().map(|t| fan_ast::DeclType(t.clone())).collect()
    }

    /// 递归展平接口嵌入：把被嵌入接口的方法并入本接口，带环检测与同名签名冲突报错。
    fn flatten_interface(
        &self,
        iface: &mut fan_object::Interface,
        visiting: &mut Vec<String>,
    ) -> VmResult<()> {
        let embed_names = iface.embeds.clone();
        for embed_name in &embed_names {
            if visiting.iter().any(|n| n == embed_name) {
                return Err(VmError::new(format!("检测到接口循环嵌入：{embed_name}")));
            }
            let embed = match self.runtime.borrow().lookup_interface(embed_name) {
                Some(embed) => embed,
                None => {
                    return Err(VmError::new(format!(
                        "接口 {} 嵌入了未定义的接口 {}",
                        iface.name, embed_name
                    )));
                }
            };
            visiting.push(embed_name.clone());
            let mut embed_flat = (*embed).clone();
            self.flatten_interface(&mut embed_flat, visiting)?;
            visiting.pop();
            for (name, method) in embed_flat.methods {
                if let Some(existing) = iface.methods.get(&name) {
                    let conflict = fan_object::interface_sig_conflict(existing, &method);
                    if !conflict.is_empty() {
                        return Err(VmError::new(format!(
                            "接口 {} 中方法 {} 的签名冲突（{}）",
                            iface.name, name, conflict
                        )));
                    }
                    continue;
                }
                iface.methods.insert(name, method);
            }
        }
        Ok(())
    }

    /// 登记一个模块中全部基本类型扩展方法，按接收者运行时类型标识建索引。
    fn register_primitive_methods(&mut self, id: ModuleId, module: &ModuleChunk) {
        for method in &module.methods {
            if method.primitive {
                if let Some(kind) = fan_object::primitive_kind_from_name(&method.class_name) {
                    self.primitive_method_registry.insert(
                        (kind.to_string(), method.method_name.clone()),
                        (id, method.function_index),
                    );
                }
            } else if self
                .runtime
                .borrow()
                .lookup_named_type(&method.class_name)
                .is_some()
            {
                // 自定义命名类型的扩展方法登记在类型自己的名下，成员访问时优先于底层类型。
                self.named_method_registry.insert(
                    (method.class_name.clone(), method.method_name.clone()),
                    (id, method.function_index),
                );
            }
        }
    }

    /// 注册一个已链接的外部模块块。供模块加载器在执行前/执行中装配。
    pub fn register_module(&mut self, id: ModuleId, module: Rc<ModuleChunk>) {
        // 先登记命名类型：扩展方法登记需据此区分命名类型方法与基本类型方法。
        self.register_named_types(&module);
        // 基本类型扩展方法不依赖 BindClass（其模块可能无任何模型），链接时即登记。
        self.register_primitive_methods(id, &module);
        self.register_interfaces(&module);
        self.modules.borrow_mut().insert(id, module);
    }

    /// 当前共享模块表句柄，供加载子模块的子 VM 与加载器复用。
    pub fn shared_modules(&self) -> SharedModules {
        Rc::clone(&self.modules)
    }

    /// 注入外部共享模块表，使子 VM 与主 VM 看到同一组已链接模块。
    pub fn with_shared_modules(mut self, modules: SharedModules) -> Self {
        self.modules = modules;
        self
    }

    /// 注入模块加载器，使 `导入` 语句可在运行期解析外部模块。
    pub fn with_loader(mut self, loader: Box<dyn ModuleLoader>) -> Self {
        self.loader = Some(loader);
        self
    }

    /// 暴露共享注册表，供加载子模块的子 VM 复用同一张闭包身份表。
    pub fn shared_registry(&self) -> SharedRegistry {
        Rc::clone(&self.functions)
    }

    /// 注入外部共享注册表，使子 VM 与主 VM 共享闭包身份表（跨模块调用）。
    pub fn with_shared_registry(mut self, registry: SharedRegistry) -> Self {
        self.functions = registry;
        self
    }

    /// 执行模块入口块，返回入口的最终值。
    pub fn run(&mut self) -> VmResult<Object> {
        self.run_module(self.current_module)
    }

    /// 以指定模块 ID 的身份执行入口块（模块加载器用子 VM 执行被导入模块时使用）。
    pub fn run_module(&mut self, module_id: ModuleId) -> VmResult<Object> {
        // 确认目标模块已链接，未链接则提前报错而非在取指时 panic。
        self.module_chunk(module_id)?;
        let base = self.slots.len();
        self.frames.push(Frame::entry(module_id, base));
        let result = self.execute();
        self.frames.pop();
        match result {
            Err(e) if e.message == THROW_SENTINEL => {
                // 一路无人 恢复：把哨兵转成对外的「未恢复的崩溃」错误。
                let thrown = self.pending_throw.clone().unwrap_or(Object::Nil);
                self.pending_throw = None;
                Err(VmError::new(format!("未恢复的崩溃：{}", thrown.inspect())))
            }
            other => other,
        }
    }

    /// 读取最近一次入口执行后、入口槽位 `slot` 中的值（供模块加载器回收导出）。
    ///
    /// 模块入口帧已弹出，但其槽位仍保留在 `slots` 中（未被后续调用覆盖时），
    /// 因此按入口帧记录的基址读取。调用方需在入口 `run` 之后、任何嵌套调用
    /// 回收槽位之前立即读取。
    pub fn entry_slot(&self, entry_base: usize, slot: u16) -> Object {
        self.slots
            .get(entry_base + slot as usize)
            .cloned()
            .unwrap_or(Object::Nil)
    }

    /// 当前 `slots` 长度，模块加载器据此记录入口帧基址。
    pub fn slots_len(&self) -> usize {
        self.slots.len()
    }

    fn module_chunk(&self, id: ModuleId) -> VmResult<Rc<ModuleChunk>> {
        self.modules
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| VmError::new(format!("模块 {id} 未链接到 VM")))
    }

    fn current_chunk(&self) -> std::cell::Ref<'_, Chunk> {
        let frame = self.frames.last().expect("执行时必须存在调用帧");
        let modules = self.modules.borrow();
        // 安全：返回的 Ref 借用 modules，调用方仅在单条指令内即时使用，
        // 不会跨会触发模块表变更的语句。用 Ref::map 投影到具体 Chunk。
        std::cell::Ref::map(modules, |table| {
            let module = table.get(&frame.module_id).expect("帧模块已链接");
            if frame.function_index == Frame::ENTRY {
                &module.entry
            } else {
                &module.functions[frame.function_index as usize].body
            }
        })
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
                Opcode::ReturnLocal(_) => {
                    let slot = self.read_u16(&chunk)? as usize;
                    let frame = self.frames.last().ok_or_else(|| VmError::new("无调用帧"))?;
                    let abs = frame.slot_base + slot;
                    // 记录返回值来源槽：帧退出执行完 稍后 后据此重读，纳入 defer 改写。
                    self.frames.last_mut().expect("帧存在").return_slot = Some(abs);
                    return Ok(self.slots.get(abs).cloned().unwrap_or(Object::Nil));
                }
                Opcode::MakeClosure(_) => {
                    let index = self.read_u16(&chunk)?;
                    let module_id = self
                        .frames
                        .last()
                        .map(|f| f.module_id)
                        .unwrap_or(MAIN_MODULE);
                    let func = self
                        .module_chunk(module_id)?
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
                    let value = self
                        .functions
                        .borrow_mut()
                        .make_closure(module_id, index, &func.name, upvalues);
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
                    *cell.borrow_mut() = value.clone();
                    // 写穿透：该 cell 若同时是某外层帧的 open upvalue，则同步回
                    // 其源头局部槽，使外层函数直接读局部变量（如 返回 数）能看到
                    // 嵌套闭包经 upvalue 的写入；帧退出时槽→cell 回写也不再覆盖。
                    let cell_ptr = Rc::as_ptr(&cell) as usize;
                    for frame in self.frames.iter() {
                        if let Some((abs_pos, _)) = frame.open_upvalues.iter().find(|(pos, up)| {
                            *pos != usize::MAX && Rc::as_ptr(up) as usize == cell_ptr
                        }) {
                            if let Some(slot) = self.slots.get_mut(*abs_pos) {
                                *slot = value.clone();
                            }
                        }
                    }
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
                Opcode::ToStr => {
                    let value = self.pop_operand()?;
                    self.stack.push(Object::string(value.inspect()));
                }
                Opcode::PushMark => {
                    self.marks.push(self.stack.len());
                }
                Opcode::Spread => {
                    let value = self.pop_operand()?;
                    let Object::Array(arr) = value else {
                        return Err(VmError::new(format!(
                            "展开 ... 仅支持数组，实际是 {}",
                            value.kind()
                        )));
                    };
                    for element in arr.elements() {
                        self.stack.push(element);
                    }
                }
                Opcode::SpreadDict => {
                    let value = self.pop_operand()?;
                    let Object::Dict(dict) = value else {
                        return Err(VmError::new(format!(
                            "字典展开 ... 仅支持字典，实际是 {}",
                            value.kind()
                        )));
                    };
                    // 按 key、value 顺序压回，供 BuildDictMarked 成对消费。
                    for key in dict.keys() {
                        let val = dict.get(&key).unwrap_or(Object::Nil);
                        self.stack.push(key);
                        self.stack.push(val);
                    }
                }
                Opcode::BuildArrayMarked => {
                    let Some(mark) = self.marks.pop() else {
                        return Err(VmError::new("BuildArrayMarked 缺少配对的压入标记"));
                    };
                    if mark > self.stack.len() {
                        return Err(VmError::new("数组构建标记超出值栈深度"));
                    }
                    let elements = self.stack.split_off(mark);
                    self.stack.push(Object::array(elements));
                }
                Opcode::BuildDictMarked => {
                    let Some(mark) = self.marks.pop() else {
                        return Err(VmError::new("BuildDictMarked 缺少配对的压入标记"));
                    };
                    if mark > self.stack.len() {
                        return Err(VmError::new("字典构建标记超出值栈深度"));
                    }
                    let entries = self.stack.split_off(mark);
                    if !entries.len().is_multiple_of(2) {
                        return Err(VmError::new("字典构建的键值必须成对出现"));
                    }
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
                Opcode::CallMarked => {
                    // 标记在 callee 压栈前记录，栈布局为 [实参…, callee]，故实参数 = 标记后槽位 - 1。
                    let argc = self.take_marked_count()? - 1;
                    self.call_value(argc)?;
                }
                Opcode::CallNativeMarked(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let name = self.string_const(&chunk, idx)?;
                    let argc = self.take_marked_count()?;
                    self.call_native(&name, argc)?;
                }
                Opcode::BindClass(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let frame_module = self
                        .frames
                        .last()
                        .map(|f| f.module_id)
                        .unwrap_or(MAIN_MODULE);
                    let module = self.module_chunk(frame_module)?;
                    let class_chunk = module
                        .classes
                        .get(idx)
                        .ok_or_else(|| VmError::new(format!("类表索引 {idx} 越界")))?;
                    let mut class = fan_object::Class::new(class_chunk.name.clone());
                    // 用字段声明构造零值字段元数据（类型信息不参与运行期，复用 any）。
                    for field_name in &class_chunk.fields {
                        class.fields.push(fan_ast::FieldDecl {
                            pos: fan_ast::Position::default(),
                            name: field_name.clone(),
                            type_: fan_ast::DeclType::any(),
                            visibility: fan_ast::Visibility::File,
                        });
                    }
                    // 解析嵌入模型：嵌入类必须已在同模块更早定义并登记。
                    for embed_name in &class_chunk.embeds {
                        let embed_class =
                            self.class_registry
                                .get(embed_name)
                                .cloned()
                                .ok_or_else(|| {
                                    VmError::new(format!(
                                        "模型 {} 嵌入的模型 {} 未定义",
                                        class_chunk.name, embed_name
                                    ))
                                })?;
                        class.embeds.push(embed_class);
                    }
                    let class_rc = Rc::new(class);
                    self.class_registry
                        .insert(class_chunk.name.clone(), Rc::clone(&class_rc));
                    // 登记本模块中属于该模型的字节码方法，供 GetMember 构造绑定闭包。
                    // 基本类型扩展方法在 register_module 链接时另行登记，此处跳过。
                    for method in &module.methods {
                        if method.class_name == class_chunk.name && !method.primitive {
                            self.method_registry.insert(
                                (method.class_name.clone(), method.method_name.clone()),
                                (frame_module, method.function_index),
                            );
                        }
                    }
                    self.stack.push(Object::Class(class_rc));
                }
                Opcode::NewInstance(_, _) => {
                    let name_idx = self.read_u16(&chunk)? as usize;
                    let pair_count = self.read_u8(&chunk)? as usize;
                    let class_name = self.string_const(&chunk, name_idx)?;
                    let class_obj = self.pop_operand()?;
                    let Object::Class(class) = class_obj else {
                        return Err(VmError::new(format!(
                            "复合字面量目标不是模型，实际是 {}",
                            class_obj.kind()
                        )));
                    };
                    if class.name != class_name {
                        return Err(VmError::new(format!(
                            "复合字面量模型不匹配：期望 {class_name}，实际 {}",
                            class.name
                        )));
                    }
                    // 栈上自下而上为 字段名、字段值 交替，先整体弹出再恢复顺序。
                    let mut raw = self.pop_args(pair_count * 2)?;
                    let mut provided: HashMap<String, Object> = HashMap::new();
                    let mut i = 0;
                    while i < raw.len() {
                        let value = raw[i + 1].clone();
                        let name = match &raw[i] {
                            Object::Str(s) => s.to_string(),
                            other => {
                                return Err(VmError::new(format!(
                                    "复合字面量字段名必须是字符串，实际是 {}",
                                    other.kind()
                                )))
                            }
                        };
                        provided.insert(name, value);
                        i += 2;
                    }
                    let inst = self.new_zero_instance(&class)?;
                    // 按复合字面量给出的字段名写入：自身字段直接写，嵌入字段经
                    // set_field_value 沿嵌入链提升解析，嵌入实例则整体替换。
                    for (name, value) in provided.drain() {
                        inst.set_field_value(&name, value).map_err(VmError::new)?;
                    }
                    let _ = raw.drain(..);
                    self.stack.push(Object::Instance(inst));
                }
                Opcode::LoadModule(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let path = self.string_const(&chunk, idx)?;
                    let module = match self.loader.as_mut() {
                        Some(loader) => loader.load(&path)?,
                        None => {
                            return Err(VmError::new(format!("未注入模块加载器，无法导入 {path}")))
                        }
                    };
                    self.stack.push(module);
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
                Opcode::Throw => {
                    // `崩溃了(描述)`：弹出描述值，构造携带调用栈的规范错误对象，
                    // 记入 pending_throw 并以哨兵错误触发栈展开。
                    let raw = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("崩溃了 缺少错误描述"))?;
                    let message = match &raw {
                        Object::Str(s) => s.to_string(),
                        other => other.inspect(),
                    };
                    let mut value = fan_object::ErrorValue::new(message);
                    let stack = self.runtime.borrow().stack();
                    if !stack.is_empty() {
                        value.stack = stack;
                    }
                    self.pending_throw = Some(Object::Error(Rc::new(RefCell::new(value))));
                    return Err(VmError::new(THROW_SENTINEL));
                }
                Opcode::Recover => {
                    // `恢复()`：取走当前待恢复的崩溃值（取走即视为已处理，停止传播）。
                    let value = self.pending_throw.clone().unwrap_or(Object::Nil);
                    self.pending_throw = None;
                    self.stack.push(value);
                }
                Opcode::RegisterDefer => {
                    // 弹出栈顶零参延迟闭包，登记到当前函数帧的 稍后 队列。
                    let closure = self
                        .stack
                        .pop()
                        .ok_or_else(|| VmError::new("RegisterDefer 时缺少延迟闭包"))?;
                    let frame = self
                        .frames
                        .last_mut()
                        .ok_or_else(|| VmError::new("稍后 只能在函数内使用"))?;
                    frame.defers.push(crate::frame::Deferred { closure });
                }
                Opcode::TypeTest(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let type_name = self.string_const(&chunk, idx)?;
                    let value = self.pop_operand()?;
                    let ok = self.value_is_type(&value, &type_name);
                    self.stack.push(Object::Bool(ok));
                }
                Opcode::TypeCast(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let type_name = self.string_const(&chunk, idx)?;
                    let value = self.pop_operand()?;
                    let (result, ok) = self.type_cast(&value, &type_name);
                    self.stack
                        .push(Object::tuple(vec![result, Object::Bool(ok)]));
                }
                Opcode::NewNamed(_) => {
                    let idx = self.read_u16(&chunk)? as usize;
                    let type_name = self.string_const(&chunk, idx)?;
                    let value = self.pop_operand()?;
                    let wrapped = self.wrap_named(&type_name, &value)?;
                    self.stack.push(wrapped);
                }
                Opcode::ApplyTag(..) => {
                    let tag_idx = self.read_u16(&chunk)? as usize;
                    let slot = self.read_u16(&chunk)?;
                    let argc = self.read_u8(&chunk)? as usize;
                    let frame_module = self
                        .frames
                        .last()
                        .map(|f| f.module_id)
                        .unwrap_or(MAIN_MODULE);
                    self.apply_tag(frame_module, tag_idx, slot, argc)?;
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

    /// 应用一个标签到刚绑定到 `slot` 的函数：构造标签模型实例，查 Runtime 中已注册的
    /// 凡语言处理器并回调；处理器经标签上下文替换实现时，把新可调用值写回该槽。
    /// 未注册标签静默忽略（对齐 tree 的 apply_fan_tag_handler）。
    fn apply_tag(
        &mut self,
        module_id: ModuleId,
        tag_idx: usize,
        slot: u16,
        argc: usize,
    ) -> VmResult<()> {
        let module = self.module_chunk(module_id)?;
        let tag = module
            .tags
            .get(tag_idx)
            .cloned()
            .ok_or_else(|| VmError::new(format!("标签表索引 {tag_idx} 越界")))?;
        // 栈上参数顺序：位置参数（至多一个，写字段 `值`）在前，命名参数值按序在后。
        let values = self.pop_args(argc)?;
        let handler = self.runtime.borrow().lookup_fan_tag_handler(&tag.name);
        let Some(handler_marker) = handler else {
            return Ok(());
        };
        // 构造标签模型实例；解析不到同名模型时为 空（未注册模型不阻断）。
        let tag_instance = self.build_tag_instance(&tag, &values)?;
        let frame_base = self.frames.last().map(|f| f.slot_base).unwrap_or(0);
        let current = self.slot(frame_base, slot as usize);
        let ctx_value = Rc::new(TagContextValue::new(tag_instance, current.clone()));
        let context = Object::TagContext(Rc::clone(&ctx_value));
        // 处理器是 MakeClosure 产出的字节码闭包标记（在闭包身份表可查），按字节码回调。
        let callee = Object::Func(handler_marker);
        self.invoke_handler(callee, vec![context])?;
        if let Some(replacement) = ctx_value.replacement() {
            let new_value = Object::Func(replacement);
            let base = self.frames.last().map(|f| f.slot_base).unwrap_or(0);
            self.set_slot(base, slot as usize, new_value);
        }
        Ok(())
    }

    /// 按标签元数据与已求值参数构造标签模型实例；无同名模型时返回 空。
    fn build_tag_instance(
        &mut self,
        tag: &fan_code::chunk::TagChunk,
        values: &[Object],
    ) -> VmResult<Object> {
        let Some(class) = self.class_registry.get(&tag.name).cloned() else {
            return Ok(Object::Nil);
        };
        // 复用零值实例化，预置全部字段（含嵌入），set_field_value 才能命中同名字段。
        let inst = self.new_zero_instance(&class)?;
        let mut cursor = 0usize;
        if tag.has_positional {
            let value = values.get(cursor).cloned().unwrap_or(Object::Nil);
            self.write_tag_field(&inst, &class, "值", value)?;
            cursor += 1;
        }
        for field in &tag.named_names {
            let value = values.get(cursor).cloned().unwrap_or(Object::Nil);
            self.write_tag_field(&inst, &class, field, value)?;
            cursor += 1;
        }
        Ok(Object::Instance(inst))
    }

    /// 写入标签实例字段；模型缺该字段时报错（对齐 tree write_tag_field）。
    fn write_tag_field(
        &mut self,
        inst: &Rc<fan_object::Instance>,
        class: &Rc<fan_object::Class>,
        name: &str,
        value: Object,
    ) -> VmResult<()> {
        inst.set_field_value(name, value)
            .map_err(|_| VmError::new(format!("标签模型 {} 没有字段 {}", class.name, name)))
    }

    /// 以单参方式调用一个可调用值（标签处理器），复用字节码/内建分派，丢弃返回值。
    fn invoke_handler(&mut self, callee: Object, args: Vec<Object>) -> VmResult<()> {
        let closure = self.functions.borrow().lookup(&callee);
        if let Some(closure) = closure {
            let module_id = closure.module_id;
            let index = closure.function_index;
            let upvalues = closure.upvalues.clone();
            drop(closure);
            self.invoke_bytecode_function(module_id, index, upvalues, args)?;
            // 调用把返回值压在值栈上，处理器的返回值无意义，弹出丢弃。
            self.stack.pop();
            return Ok(());
        }
        match callee {
            Object::Builtin(builtin) => {
                self.invoke_builtin(&builtin, args)?;
                self.stack.pop();
                Ok(())
            }
            other => Err(VmError::new(format!(
                "标签处理器不可调用：{}",
                other.kind()
            ))),
        }
    }

    fn call_value(&mut self, argc: usize) -> VmResult<()> {
        let callee = self
            .stack
            .pop()
            .ok_or_else(|| VmError::new("Call 时缺少可调用对象"))?;
        let args = self.pop_args(argc)?;
        let closure = self.functions.borrow().lookup(&callee);
        if let Some(closure) = closure {
            let module_id = closure.module_id;
            let index = closure.function_index;
            let upvalues = closure.upvalues.clone();
            // 绑定方法：接收者作为隐式首参 `自己`，置于显式实参之前。
            let args = match &closure.receiver {
                Some(receiver) => {
                    let mut full = Vec::with_capacity(args.len() + 1);
                    full.push(receiver.clone());
                    full.extend(args);
                    full
                }
                None => args,
            };
            drop(closure);
            self.invoke_bytecode_function(module_id, index, upvalues, args)?;
            return Ok(());
        }
        match callee {
            Object::Class(class) => self.construct_instance(&class, args),
            Object::Builtin(builtin) => self.invoke_builtin(&builtin, args),
            other => Err(VmError::new(format!("值不可调用：{}", other.kind()))),
        }
    }

    /// `类名(实参…)` 构造：先建零值实例，若模型定义了 `初始化` 方法则绑定接收者后调用。
    fn construct_instance(
        &mut self,
        class: &Rc<fan_object::Class>,
        args: Vec<Object>,
    ) -> VmResult<()> {
        let inst = self.new_zero_instance(class)?;
        if let Some((module_id, fn_index)) = self
            .method_registry
            .get(&(class.name.clone(), "初始化".to_string()))
        {
            let (module_id, fn_index) = (*module_id, *fn_index);
            let upvalues = self.build_method_upvalues(module_id, fn_index)?;
            let bound = self.functions.borrow_mut().make_bound_method(
                module_id,
                fn_index,
                "初始化",
                Object::Instance(Rc::clone(&inst)),
                upvalues,
            );
            // 绑定方法调用：接收者已内置，这里只传显式实参。
            let closure = self
                .functions
                .borrow()
                .lookup(&bound)
                .expect("刚登记的构造绑定闭包可查");
            let upvalues = closure.upvalues.clone();
            drop(closure);
            self.invoke_bytecode_function(module_id, fn_index, upvalues, {
                let mut full = Vec::with_capacity(args.len() + 1);
                full.push(Object::Instance(Rc::clone(&inst)));
                full.extend(args);
                full
            })?;
        } else if !args.is_empty() {
            return Err(VmError::new(format!(
                "模型 {} 没有 初始化 方法，不应传参",
                class.name
            )));
        }
        self.stack.push(Object::Instance(inst));
        Ok(())
    }

    fn call_native(&mut self, name: &str, argc: usize) -> VmResult<()> {
        let args = self.pop_args(argc)?;
        let builtin = self
            .resolve_native(name)
            .ok_or_else(|| VmError::new(format!("未定义的内建函数：{name}")))?;
        self.invoke_builtin(&builtin, args)
    }

    /// 解析原生内建：先按完整 spec（如 `tag.tagRegisterHandler`），未命中再按点号后的
    /// 短名回退（对齐 tree 的 bind_native：ext 内建以短名登记）。
    fn resolve_native(&self, name: &str) -> Option<Rc<Builtin>> {
        let runtime = self.runtime.borrow();
        if let Some(found) = runtime.lookup_native(name) {
            return Some(found);
        }
        name.rsplit_once('.')
            .and_then(|(_, short)| runtime.lookup_native(short))
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
        module_id: ModuleId,
        index: u16,
        upvalues: Vec<Upvalue>,
        args: Vec<Object>,
    ) -> VmResult<()> {
        let module = self.module_chunk(module_id)?;
        let func = &module.functions[index as usize];
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
        let mut frame = Frame::function(module_id, index, base, args.len());
        // 闭包携带的 upvalue 作为本帧 upvalue 槽的前缀，Load/StoreUpvalue 按索引访问。
        // 这些句柄来自外层，abs_pos 用哨兵标记，帧退出时不做关闭处理。
        frame.open_upvalues = upvalues.into_iter().map(|up| (usize::MAX, up)).collect();
        self.frames.push(frame);
        let exec_result = self.execute();

        // 判断是否为 `崩溃了` 触发的栈展开（区别于真实 VM 错误）。
        let is_throw = matches!(&exec_result, Err(e) if e.message == THROW_SENTINEL);
        // 后进先出执行本帧登记的 稍后 闭包；仅在有抛出时才可能出现 恢复()。
        // 必须在弹出本帧之前执行：defer 闭包经 upvalue 写本帧局部槽时，写穿透
        // 逻辑要在帧栈中找到本帧的 open upvalue，提前 pop 会让更新丢失。
        let mut caught = false;
        if self
            .frames
            .last()
            .map(|f| !f.defers.is_empty())
            .unwrap_or(false)
        {
            let before = self.pending_throw.clone();
            while let Some(deferred) = self.frames.last_mut().expect("函数帧存在").defers.pop()
            {
                self.call_value_zero(deferred.closure)?;
            }
            // 恢复() 会把 pending_throw 取走：取走即本帧已接管，停止继续上抛。
            if is_throw && before.is_some() && self.pending_throw.is_none() {
                caught = true;
            }
        }

        // 返回 标识符 记录的来源槽：defer 执行后该槽已含最新值，truncate 前先取出，
        // 用它覆盖 ReturnLocal 在 defer 之前读出的旧返回值。
        let return_slot_value = self
            .frames
            .last()
            .and_then(|f| f.return_slot)
            .and_then(|abs| self.slots.get(abs).cloned());

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

        match exec_result {
            Ok(mut result) => {
                if let Some(updated) = return_slot_value {
                    result = updated;
                }
                self.stack.push(result);
                Ok(())
            }
            Err(_) if is_throw && caught => {
                // 抛出已被本帧 稍后 中的 恢复() 接管：函数正常返回空值，不再上抛。
                self.stack.push(Object::Nil);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// 以零个实参调用一个字节码闭包（供 `稍后` 队列执行），丢弃其返回值。
    fn call_value_zero(&mut self, callee: Object) -> VmResult<()> {
        let closure = self.functions.borrow().lookup(&callee);
        if let Some(closure) = closure {
            let module_id = closure.module_id;
            let index = closure.function_index;
            let upvalues = closure.upvalues.clone();
            drop(closure);
            return self.invoke_bytecode_function(module_id, index, upvalues, Vec::new());
        }
        match callee {
            Object::Builtin(builtin) => self.invoke_builtin(&builtin, Vec::new()),
            other => Err(VmError::new(format!("稍后 项不可调用：{}", other.kind()))),
        }
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

    /// 为绑定方法解析 upvalue：方法体引用的模块顶层名最终落到所属模块入口帧的
    /// 局部槽。按编译期描述链，从该入口帧逐个取/建 open 句柄（from_local）或
    /// 透传同帧上一层 upvalue（from_local=false）。
    fn build_method_upvalues(
        &mut self,
        module_id: ModuleId,
        fn_index: u16,
    ) -> VmResult<Vec<Upvalue>> {
        let module = self.module_chunk(module_id)?;
        let func = module
            .functions
            .get(fn_index as usize)
            .ok_or_else(|| VmError::new(format!("函数表索引 {fn_index} 越界")))?;
        // 方法 upvalue 的根是其所属模块的入口帧（栈中同模块最浅的帧）。
        let frame_pos = self
            .frames
            .iter()
            .position(|f| f.module_id == module_id)
            .ok_or_else(|| VmError::new("绑定方法时找不到所属模块入口帧"))?;
        let base = self.frames[frame_pos].slot_base;
        let mut captured: Vec<Upvalue> = Vec::with_capacity(func.upvalues.len());
        for desc in &func.upvalues {
            let up = if desc.from_local {
                let abs = base + desc.index as usize;
                if let Some((_, up)) = self.frames[frame_pos]
                    .open_upvalues
                    .iter()
                    .find(|(pos, _)| *pos == abs)
                {
                    Rc::clone(up)
                } else {
                    let initial = self.slots.get(abs).cloned().unwrap_or(Object::Nil);
                    let cell: Upvalue = Rc::new(RefCell::new(initial));
                    self.frames[frame_pos]
                        .open_upvalues
                        .push((abs, Rc::clone(&cell)));
                    cell
                }
            } else {
                Rc::clone(
                    captured
                        .get(desc.index as usize)
                        .ok_or_else(|| VmError::new("绑定方法 upvalue 链断裂"))?,
                )
            };
            captured.push(up);
        }
        Ok(captured)
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
    /// 构造模型的零值实例：自身字段置空，并为嵌入模型递归建立子实例（嵌入里程碑启用）。
    fn new_zero_instance(
        &self,
        class: &Rc<fan_object::Class>,
    ) -> VmResult<Rc<fan_object::Instance>> {
        let inst = Rc::new(fan_object::Instance::new(class.clone()));
        for field in &class.fields {
            inst.set(field.name.clone(), Object::Nil);
        }
        for embed in &class.embeds {
            let child = self.new_zero_instance(embed)?;
            inst.embeds.borrow_mut().push(child);
        }
        Ok(inst)
    }

    /// 沿模型自身与嵌入链查找字节码方法，返回 (模块, 函数索引)。
    /// 与字段提升一致：自身优先，再按嵌入声明顺序深度遍历；命中第一个即返回。
    fn resolve_method(&self, class: &Rc<fan_object::Class>, name: &str) -> Option<(ModuleId, u16)> {
        if let Some(entry) = self
            .method_registry
            .get(&(class.name.clone(), name.to_string()))
        {
            return Some(*entry);
        }
        for embed in &class.embeds {
            if let Some(entry) = self.resolve_method(embed, name) {
                return Some(entry);
            }
        }
        None
    }

    /// 判断值是否属于指定类型名。与 tree 求值器 `value_is_type` 规则对齐：
    /// 基本类型按运行时类型标识；空值满足 空/错误 及任意接口；模型按类名与嵌入链；
    /// 接口按结构化方法满足判定。
    fn value_is_type(&self, value: &Object, type_name: &str) -> bool {
        if let Some(kind) = fan_object::primitive_kind_from_name(type_name) {
            return fan_object::runtime_type_name(value) == kind;
        }
        if matches!(value, Object::Nil) {
            if matches!(type_name, "空" | "错误") {
                return true;
            }
            return match self.runtime.borrow().lookup_interface(type_name) {
                Some(iface) => self.interface_satisfied(&iface, value),
                None => false,
            };
        }
        // 自定义命名类型：身份严格一致。
        if value.type_name().as_deref() == Some(type_name) {
            return true;
        }
        // 命名类型（含联合）：联合按任一成员满足，单类型按底层基本类型判定。
        if let Some(nt) = self.runtime.borrow().lookup_named_type(type_name).cloned() {
            let inner = value.unwrap_named();
            return nt.base_type.0.split('|').any(|m| {
                fan_object::primitive_kind_from_name(m)
                    .map(|k| fan_object::runtime_type_name(&inner) == k)
                    .unwrap_or(false)
            });
        }
        if let Object::Instance(inst) = value {
            if inst.class.name == type_name || inst.class.has_embed(type_name) {
                return true;
            }
        }
        match self.runtime.borrow().lookup_interface(type_name) {
            Some(iface) => self.interface_satisfied(&iface, value),
            None => false,
        }
    }

    /// 结构化接口满足判定：值（穿透命名类型）拥有接口要求的全部方法且签名兼容。
    /// 空值满足任何接口（由调用方先短路处理）。
    fn interface_satisfied(&self, iface: &fan_object::Interface, value: &Object) -> bool {
        if iface.methods.is_empty() || matches!(value, Object::Nil) {
            return true;
        }
        let checked = value.unwrap_named();
        let checked = &checked;
        for (name, want) in &iface.methods {
            let Some(got) = self.method_signature(checked, name) else {
                return false;
            };
            if !self.signature_compatible(want, &got) {
                return false;
            }
        }
        true
    }

    /// 取某值上指定方法的实现签名（参数类型列表、返回类型列表），无此方法返回 None。
    /// 模型方法经方法表定位到字节码函数块；基本类型经扩展方法表定位。
    fn method_signature(&self, value: &Object, name: &str) -> Option<(Vec<String>, Vec<String>)> {
        let entry = match value {
            Object::Instance(inst) => self.resolve_method(&inst.class, name),
            other => {
                let kind = fan_object::runtime_type_name(other).to_string();
                self.primitive_method_registry
                    .get(&(kind, name.to_string()))
                    .copied()
            }
        };
        let (module_id, fn_index) = entry?;
        let module = self.modules.borrow();
        let module = module.get(&module_id)?;
        let func = module.functions.get(fn_index as usize)?;
        // 方法的 param_types 首项是隐式接收者 自己（any），接口比对不含接收者，去掉。
        let mut params = func.param_types.clone();
        if !params.is_empty() {
            params.remove(0);
        }
        Some((params, func.return_types.clone()))
    }

    /// 比对接口方法签名与实现签名是否兼容；实现方完全未标注时只比较参数数量。
    fn signature_compatible(
        &self,
        want: &fan_ast::InterfaceMethodSig,
        got: &(Vec<String>, Vec<String>),
    ) -> bool {
        let (got_params, got_returns) = got;
        let want_params = &want.params;
        let impl_fully_untyped =
            got_params.iter().all(|t| t.is_empty()) && got_returns.iter().all(String::is_empty);
        if impl_fully_untyped {
            return got_params.len() == want_params.len();
        }
        if got_params.len() != want_params.len() {
            return false;
        }
        for (wp, gp) in want_params.iter().zip(got_params.iter()) {
            if wp.type_.is_any() || gp.is_empty() {
                continue;
            }
            if wp.type_.0 != *gp {
                return false;
            }
        }
        if want.return_types.is_empty() {
            return true;
        }
        if want.return_types.len() != got_returns.len() {
            return false;
        }
        for (wr, gr) in want.return_types.iter().zip(got_returns.iter()) {
            if wr.is_any() || gr.is_empty() {
                continue;
            }
            if wr.0 != *gr {
                return false;
            }
        }
        true
    }

    /// `值 转为 类型`：成功返回 (转换值, true)，失败返回 (空, false)。
    fn type_cast(&self, value: &Object, type_name: &str) -> (Object, bool) {
        // 目标是自定义命名类型：先身份一致直接返回，再按底层类型包装，失败返回 (空,假)。
        if let Some(nt) = self.runtime.borrow().lookup_named_type(type_name).cloned() {
            if value.type_name().as_deref() == Some(type_name) {
                return (value.clone(), true);
            }
            return match self.wrap_named_with(&nt, value) {
                Some(wrapped) => (wrapped, true),
                None => (Object::Nil, false),
            };
        }
        // 身份/结构化满足：模型、接口、基本类型同名等，直接原值返回。
        if self.value_is_type(value, type_name) {
            return (value.clone(), true);
        }
        // 基本类型间按值转换。
        if let Some(kind) = fan_object::primitive_kind_from_name(type_name) {
            if let Some(converted) = self.convert_primitive(value, kind) {
                return (converted, true);
            }
        }
        (Object::Nil, false)
    }

    /// 命名类型构造 `类型名(值)`：底层类型不满足时报错（对齐 tree convert_to_named）。
    fn wrap_named(&self, type_name: &str, value: &Object) -> VmResult<Object> {
        let nt = self
            .runtime
            .borrow()
            .lookup_named_type(type_name)
            .cloned()
            .ok_or_else(|| VmError::new(format!("未定义的类型 {type_name}")))?;
        self.wrap_named_with(&nt, value)
            .ok_or_else(|| VmError::new(format!("无法转换为 {type_name}")))
    }

    /// 按命名类型的底层成员校验后包装；联合类型接受任一成员，单类型要求底层种类匹配。
    fn wrap_named_with(&self, nt: &fan_object::NamedType, value: &Object) -> Option<Object> {
        let inner = value.unwrap_named();
        let ok = nt.base_type.0.split('|').any(|m| {
            fan_object::primitive_kind_from_name(m)
                .map(|k| fan_object::runtime_type_name(&inner) == k)
                .unwrap_or(false)
        });
        ok.then(|| Object::named(nt.name.clone(), inner))
    }

    /// 基本类型间的值转换，失败返回 None；与 tree 的 convert_to_primitive_kind 对齐。
    fn convert_primitive(&self, value: &Object, kind: &str) -> Option<Object> {
        let inner = value.unwrap_named();
        let v = &inner;
        if fan_object::runtime_type_name(v) == kind {
            return Some(v.clone());
        }
        match kind {
            "integer" => match v {
                Object::Float(f) => Some(Object::Int(*f as i64)),
                Object::Bool(b) => Some(Object::Int(if *b { 1 } else { 0 })),
                Object::Byte(b) => Some(Object::Int(*b as i64)),
                Object::Char(c) => Some(Object::Int(*c as i64)),
                Object::Str(s) => s.trim().parse::<i64>().ok().map(Object::Int),
                _ => None,
            },
            "float" => match v {
                Object::Int(i) => Some(Object::Float(*i as f64)),
                Object::Bool(b) => Some(Object::Float(if *b { 1.0 } else { 0.0 })),
                Object::Byte(b) => Some(Object::Float(*b as f64)),
                Object::Char(c) => Some(Object::Float(*c as i64 as f64)),
                Object::Str(s) => s.trim().parse::<f64>().ok().map(Object::Float),
                _ => None,
            },
            "string" => Some(Object::string(v.inspect())),
            "byte" => self
                .integer_like(v)
                .filter(|n| (0..=255).contains(n))
                .map(|n| Object::Byte(n as u8)),
            "char" => match v {
                Object::Int(i) => u32::try_from(*i)
                    .ok()
                    .and_then(char::from_u32)
                    .map(Object::Char),
                Object::Byte(b) => Some(Object::Char(*b as char)),
                _ => None,
            },
            "boolean" => match v {
                Object::Int(i) => Some(Object::Bool(*i != 0)),
                Object::Bool(_) => Some(v.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// 从整数族取值为 i64，供字节转换范围检查。
    fn integer_like(&self, value: &Object) -> Option<i64> {
        match value {
            Object::Int(i) => Some(*i),
            Object::Byte(b) => Some(*b as i64),
            Object::Char(c) => Some(*c as i64),
            Object::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    fn get_member(&mut self, object: &Object, name: &str) -> VmResult<Object> {
        // 自定义命名类型的方法优先于底层基本类型同名方法（对齐 tree 查找顺序）。
        if let Object::Named(named) = object {
            if let Some((module_id, fn_index)) = self
                .named_method_registry
                .get(&(named.type_name.clone(), name.to_string()))
                .copied()
            {
                let upvalues = self.build_method_upvalues(module_id, fn_index)?;
                let bound = self.functions.borrow_mut().make_bound_method(
                    module_id,
                    fn_index,
                    name,
                    object.clone(),
                    upvalues,
                );
                return Ok(bound);
            }
        }
        match object.unwrap_named() {
            Object::TagContext(ctx) => self.tag_context_member(&ctx, name),
            Object::Module(module) => module.borrow().exports.get(name).cloned().ok_or_else(|| {
                VmError::new(format!("模块 {} 没有公开成员 {name}", module.borrow().name))
            }),
            Object::Dict(dict) => {
                if let Some(v) = dict.get(&Object::string(name)) {
                    return Ok(v);
                }
                Err(VmError::new(format!("字典不存在键 {name}")))
            }
            Object::Error(cell) => self.error_member(&cell.borrow(), name),
            Object::Instance(inst) => {
                // 嵌入子实例优先（如 w.动物），再做沿嵌入链的字段提升查找。
                if let Some(embed) = inst.embed_by_name(name) {
                    return Ok(Object::Instance(embed));
                }
                let resolution = inst.resolve_field(name, &inst);
                if resolution.ambiguous {
                    return Err(VmError::new(format!(
                        "{} 的字段 {name} 在嵌入链中有歧义",
                        inst.class.name
                    )));
                }
                if let Some(v) = resolution.value {
                    return Ok(v);
                }
                // 字段未命中，尝试字节码方法：先查自身模型，再沿嵌入链提升查找。
                if let Some((module_id, fn_index)) = self.resolve_method(&inst.class, name) {
                    let upvalues = self.build_method_upvalues(module_id, fn_index)?;
                    let bound = self.functions.borrow_mut().make_bound_method(
                        module_id,
                        fn_index,
                        name,
                        object.clone(),
                        upvalues,
                    );
                    return Ok(bound);
                }
                Err(VmError::new(format!(
                    "{} 没有字段或方法 {name}",
                    inst.class.name
                )))
            }
            other => {
                // 基本类型（字符串/数组等）：内建成员未命中后查扩展方法表，
                // 命中则构造绑定当前接收者的方法闭包。
                let kind = fan_object::runtime_type_name(&other).to_string();
                if let Some((module_id, fn_index)) = self
                    .primitive_method_registry
                    .get(&(kind, name.to_string()))
                {
                    let (module_id, fn_index) = (*module_id, *fn_index);
                    let upvalues = self.build_method_upvalues(module_id, fn_index)?;
                    let bound = self.functions.borrow_mut().make_bound_method(
                        module_id,
                        fn_index,
                        name,
                        object.clone(),
                        upvalues,
                    );
                    return Ok(bound);
                }
                Err(VmError::new(format!("{} 不支持成员访问", other.kind())))
            }
        }
    }

    /// 标签上下文成员：`标签`、`当前实现`、`替换实现`（对齐 tree tag_context_member）。
    /// `替换实现` 返回一个内建函数，处理器调用它即把新实现写回上下文，VM 回调结束后读取。
    fn tag_context_member(&mut self, ctx: &Rc<TagContextValue>, name: &str) -> VmResult<Object> {
        match name {
            "标签" | "Tag" => Ok(ctx.tag()),
            "当前实现" => Ok(ctx.current()),
            "替换实现" => {
                let ctx = Rc::clone(ctx);
                let func = Builtin::new(
                    "替换实现",
                    Some(1),
                    Rc::new(move |_rt, args| {
                        let Some(first) = args.into_iter().next() else {
                            return Err("替换实现 需要 1 个参数".to_string());
                        };
                        match first {
                            Object::Func(f) => {
                                ctx.replace(f);
                                Ok(vec![Object::Nil])
                            }
                            other => Err(format!(
                                "替换实现 的参数必须是可调用对象，实际 {}",
                                other.kind()
                            )),
                        }
                    }),
                );
                Ok(Object::Builtin(Rc::new(func)))
            }
            _ => Err(VmError::new(format!("标签上下文没有成员 {name}"))),
        }
    }

    /// 错误对象成员：与树遍历求值器 `error_member` 保持一致。
    fn error_member(&self, err: &fan_object::ErrorValue, name: &str) -> VmResult<Object> {
        match name {
            "消息" => Ok(Object::string(err.message.clone())),
            "错误码" | "码" => Ok(Object::string(err.code.clone())),
            "详情" => match &err.details {
                Some(d) => Ok(Object::Dict(d.clone())),
                None => Ok(Object::Dict(fan_object::FanDict::new())),
            },
            "原因" => match &err.cause {
                Some(cause) => Ok(Object::Error(Rc::new(RefCell::new((**cause).clone())))),
                None => Ok(Object::Nil),
            },
            "调用栈" => {
                let frames: Vec<Object> = err
                    .stack
                    .iter()
                    .map(|f| {
                        let mut d = fan_object::FanDict::new();
                        d.set(Object::string("函数"), Object::string(f.function.clone()));
                        d.set(Object::string("文件"), Object::string(f.file.clone()));
                        d.set(Object::string("行"), Object::int(f.line as i64));
                        d.set(Object::string("列"), Object::int(f.column as i64));
                        Object::Dict(d)
                    })
                    .collect();
                Ok(Object::array(frames))
            }
            "追踪" => Ok(Object::string(err.trace())),
            _ => Err(VmError::new(format!("错误 没有成员 {name}"))),
        }
    }

    /// 成员写入：字典按键写入、实例写自身字段。
    fn set_member(&self, object: &Object, name: &str, value: Object) -> VmResult<()> {
        match object.unwrap_named() {
            Object::Module(module) => {
                module.borrow_mut().set(name, value);
                Ok(())
            }
            Object::Dict(mut dict) => {
                dict.set(Object::string(name), value);
                Ok(())
            }
            Object::Instance(inst) => inst.set_field_value(name, value).map_err(VmError::new),
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
        // 动态标签：编译期未知类型，按运行期实际对象解析为具体标签。
        let tag = if tag == fan_code::opcode::FOR_TAG_DYNAMIC {
            match iterable {
                Object::Array(_) => fan_code::opcode::FOR_TAG_ARRAY,
                Object::Str(_) => fan_code::opcode::FOR_TAG_STRING,
                Object::Dict(_) => fan_code::opcode::FOR_TAG_DICT,
                _ => {
                    return Err(VmError::new(format!(
                        "对象不可遍历，实际类型是 {}",
                        iterable.kind()
                    )))
                }
            }
        } else {
            tag
        };
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

    /// 弹出标记栈顶边界，返回自该边界起压入的实参个数（用于展开调用的运行期计数）。
    fn take_marked_count(&mut self) -> VmResult<usize> {
        let mark = self
            .marks
            .pop()
            .ok_or_else(|| VmError::new("展开调用缺少配对的压入标记"))?;
        if mark > self.stack.len() {
            return Err(VmError::new("调用标记超出值栈深度"));
        }
        Ok(self.stack.len() - mark)
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
