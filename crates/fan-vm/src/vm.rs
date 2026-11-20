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
use crate::loader::ModuleLoader;
use crate::upvalue::{ModuleId, Upvalue};
use crate::value::const_to_object;
use crate::vm_function::{SharedRegistry, VmFunctionRegistry};

/// 主模块在模块表中的固定标识。
pub const MAIN_MODULE: ModuleId = 0;

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
    /// 模块加载器；遇到 LoadModule 时按路径解析。未注入则导入语句报错。
    loader: Option<Box<dyn ModuleLoader>>,
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
            loader: None,
            _marker: std::marker::PhantomData,
        };
        vm.register_primitive_methods(MAIN_MODULE, &main);
        vm
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
            }
        }
    }

    /// 注册一个已链接的外部模块块。供模块加载器在执行前/执行中装配。
    pub fn register_module(&mut self, id: ModuleId, module: Rc<ModuleChunk>) {
        // 基本类型扩展方法不依赖 BindClass（其模块可能无任何模型），链接时即登记。
        self.register_primitive_methods(id, &module);
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
        result
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

    fn get_member(&mut self, object: &Object, name: &str) -> VmResult<Object> {
        match object.unwrap_named() {
            Object::Module(module) => module.borrow().exports.get(name).cloned().ok_or_else(|| {
                VmError::new(format!("模块 {} 没有公开成员 {name}", module.borrow().name))
            }),
            Object::Dict(dict) => {
                if let Some(v) = dict.get(&Object::string(name)) {
                    return Ok(v);
                }
                Err(VmError::new(format!("字典不存在键 {name}")))
            }
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
