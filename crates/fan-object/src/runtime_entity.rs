//! 运行时实体：函数、模型、实例、接口、命名类型、内建函数。
//!
//! Go 版把这些放在 `internal/evaluator` 包，靠 `object.Object` 接口实现跨包扩展。
//! Rust 的 `enum` 无法跨 crate 增补变体，因此统一放到 `fan-object`，
//! 让 `Object` 能直接持有它们，避免 `fan-object` 与 `fan-eval` 循环依赖。

use crate::{Environment, Object};
use fan_ast::{
    BlockStmt, DeclType, FieldDecl, InterfaceMethodSig, Parameter, Position, Visibility,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

// ---------------------------------------------------------------- 函数

/// 一个凡语言函数（含方法、闭包、分段函数）。
#[derive(Debug, Clone)]
pub struct Function {
    /// 函数名；匿名函数为空串。
    pub name: String,
    pub params: Vec<Parameter>,
    pub body: BlockStmt,
    /// 定义时的环境，用于闭包捕获。
    pub env: Option<Rc<RefCell<Environment>>>,
    /// 定义位置，用于错误调用栈。
    pub pos: Position,
    pub return_types: Vec<DeclType>,
    pub tags: Vec<fan_ast::TagExpr>,
    pub visibility: Visibility,

    /// 作为方法时的接收者名（默认 `自己`）。
    pub receiver_name: String,
    /// 作为方法时所属的模型名。
    pub class_name: String,
    /// 是否为原生类型的方法扩展。
    pub is_primitive_method: bool,
    /// 是否为方法（决定调用时是否注入接收者）。
    pub is_method: bool,

    /// 方法所属的接收者（调用方法时绑定）。
    /// 已绑定的接收者。模型方法为实例；为基本类型定义的方法则直接是那个值
    /// （字符串、数组等），因此这里不限制为 `Instance`。
    pub self_value: Option<Object>,
    /// `定义 本地(...)` 绑定的原生实现；存在时调用函数即转调原生函数。
    pub native_impl: Option<Rc<Builtin>>,
    /// 作为方法时所归属的模型。
    pub owner: Option<Rc<Class>>,

    /// 分段函数名各段。
    pub name_segments: Vec<String>,
    /// 分段函数段间连接词。
    pub connectors: Vec<String>,
    /// 分段函数每段参数个数。
    pub group_sizes: Vec<usize>,
    pub signature: String,
}

impl Function {
    pub fn new(
        name: impl Into<String>,
        params: Vec<Parameter>,
        body: BlockStmt,
        env: Option<Rc<RefCell<Environment>>>,
        pos: Position,
    ) -> Self {
        Function {
            name: name.into(),
            params,
            body,
            env,
            pos,
            return_types: Vec::new(),
            tags: Vec::new(),
            visibility: Visibility::File,
            receiver_name: "自己".to_string(),
            class_name: String::new(),
            is_primitive_method: false,
            is_method: false,
            self_value: None,
            native_impl: None,
            owner: None,
            name_segments: Vec::new(),
            connectors: Vec::new(),
            group_sizes: Vec::new(),
            signature: String::new(),
        }
    }

    /// 是否为分段函数（函数名由多段构成）。
    pub fn is_segmented(&self) -> bool {
        self.name_segments.len() > 1
    }

    /// 必需参数个数（不含默认值参数与可变参数）。
    pub fn required_arity(&self) -> usize {
        self.params
            .iter()
            .filter(|p| p.default.is_none() && !p.variadic)
            .count()
    }

    /// 是否接受可变参数。
    pub fn is_variadic(&self) -> bool {
        self.params.last().map(|p| p.variadic).unwrap_or(false)
    }
}

// ---------------------------------------------------------------- 模型

/// 模型（类）定义。
#[derive(Debug, Clone, Default)]
pub struct Class {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// 方法名 → 函数。
    pub methods: HashMap<String, Rc<Function>>,
    /// 方法名 → 可见性。
    pub method_vis: HashMap<String, Visibility>,
    /// 被嵌入的模型，按声明顺序。
    pub embeds: Vec<Rc<Class>>,
    /// 定义该模型时的环境，供方法体求值解析名字。
    pub parent: Option<Rc<RefCell<Environment>>>,
    pub visibility: Visibility,
}

/// 一次方法查找的结果（沿嵌入链提升）。
pub struct MethodResolution {
    pub func: Option<Rc<Function>>,
    /// 方法实际所属的模型（可能是被嵌入的模型）。
    pub owner_class: Option<Rc<Class>>,
    /// 同层多个嵌入提供同名方法。
    pub ambiguous: bool,
    pub found: bool,
}

impl Class {
    pub fn new(name: impl Into<String>) -> Self {
        Class {
            name: name.into(),
            fields: Vec::new(),
            methods: HashMap::new(),
            method_vis: HashMap::new(),
            embeds: Vec::new(),
            parent: None,
            visibility: Visibility::File,
        }
    }

    /// 按名字查找字段声明。
    pub fn field(&self, name: &str) -> Option<&FieldDecl> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// 查看自身方法（不含嵌入提升）。
    pub fn method(&self, name: &str) -> Option<Rc<Function>> {
        self.methods.get(name).cloned()
    }

    /// 沿嵌入链查找方法：自身方法优先；同层多个嵌入提供同名方法时报歧义。
    pub fn resolve_method(&self, name: &str, this: &Rc<Class>) -> MethodResolution {
        if let Some(f) = self.methods.get(name) {
            return MethodResolution {
                func: Some(f.clone()),
                owner_class: Some(this.clone()),
                ambiguous: false,
                found: true,
            };
        }
        let mut matches: Vec<MethodResolution> = Vec::new();
        for embed in &self.embeds {
            let r = embed.resolve_method(name, embed);
            if r.found && !r.ambiguous {
                matches.push(r);
            }
        }
        match matches.len() {
            1 => {
                let m = matches.remove(0);
                MethodResolution {
                    func: m.func,
                    owner_class: m.owner_class,
                    ambiguous: false,
                    found: true,
                }
            }
            n if n > 1 => MethodResolution {
                func: None,
                owner_class: None,
                ambiguous: true,
                found: true,
            },
            _ => MethodResolution {
                func: None,
                owner_class: None,
                ambiguous: false,
                found: false,
            },
        }
    }

    /// 查找方法；歧义时返回 `None`。
    pub fn lookup_method(&self, name: &str) -> Option<Rc<Function>> {
        self.resolve_method(name, &Rc::new(self.clone()))
            .func
            .filter(|_| !self.resolve_method(name, &Rc::new(self.clone())).ambiguous)
    }

    /// 嵌入链中是否包含指定模型名。
    pub fn has_embed(&self, name: &str) -> bool {
        let mut seen: Vec<String> = Vec::new();
        self.has_embed_inner(name, &mut seen)
    }

    fn has_embed_inner(&self, name: &str, seen: &mut Vec<String>) -> bool {
        if seen.iter().any(|s| s == &self.name) {
            return false;
        }
        seen.push(self.name.clone());
        self.embeds
            .iter()
            .any(|e| e.name == name || e.has_embed_inner(name, seen))
    }
}

/// 模型实例。
#[derive(Debug, Clone, Default)]
pub struct Instance {
    pub class: Rc<Class>,
    /// 字段名 → 值。
    pub fields: RefCell<HashMap<String, Object>>,
    /// 被嵌入模型的子实例，按声明顺序。
    pub embeds: RefCell<Vec<Rc<Instance>>>,
}

/// 一次字段查找的结果（沿嵌入链提升）。
pub struct FieldResolution {
    pub value: Option<Object>,
    /// 持有该字段的实例（可能是嵌入子实例）。
    pub instance: Option<Rc<Instance>>,
    /// 同层多个嵌入提供同名字段。
    pub ambiguous: bool,
    pub found: bool,
}

impl Instance {
    pub fn new(class: Rc<Class>) -> Self {
        Instance {
            class,
            fields: RefCell::new(HashMap::new()),
            embeds: RefCell::new(Vec::new()),
        }
    }

    /// 读取自身字段（不含嵌入提升）。
    pub fn get(&self, name: &str) -> Option<Object> {
        self.fields.borrow().get(name).cloned()
    }

    /// 写入自身字段（不含嵌入提升）。
    pub fn set(&self, name: impl Into<String>, value: Object) {
        self.fields.borrow_mut().insert(name.into(), value);
    }

    /// 按嵌入模型名查找子实例。
    pub fn embed_by_name(&self, name: &str) -> Option<Rc<Instance>> {
        self.embeds
            .borrow()
            .iter()
            .find(|e| e.class.name == name)
            .cloned()
    }

    /// 沿嵌入链查找字段：自身字段优先；同层多个嵌入提供同名字段时报歧义。
    pub fn resolve_field(&self, name: &str, this: &Rc<Instance>) -> FieldResolution {
        if let Some(v) = self.fields.borrow().get(name) {
            return FieldResolution {
                value: Some(v.clone()),
                instance: Some(this.clone()),
                ambiguous: false,
                found: true,
            };
        }
        let mut matches: Vec<FieldResolution> = Vec::new();
        for e in self.embeds.borrow().iter() {
            let r = e.resolve_field(name, e);
            if r.found && !r.ambiguous {
                matches.push(r);
            }
        }
        match matches.len() {
            1 => {
                let m = matches.remove(0);
                FieldResolution {
                    value: m.value,
                    instance: m.instance,
                    ambiguous: false,
                    found: true,
                }
            }
            n if n > 1 => FieldResolution {
                value: None,
                instance: None,
                ambiguous: true,
                found: true,
            },
            _ => FieldResolution {
                value: None,
                instance: None,
                ambiguous: false,
                found: false,
            },
        }
    }

    /// 沿嵌入链写字段；同层歧义时拒绝写入。
    pub fn set_field_value(&self, name: &str, val: Object) -> Result<(), String> {
        {
            let mut fields = self.fields.borrow_mut();
            if fields.contains_key(name) {
                fields.insert(name.to_string(), val);
                return Ok(());
            }
        }
        // 按嵌入模型名整体替换：`狗窝{动物: 动物{名字: "旺财"}}`
        let embed_idx = self
            .embeds
            .borrow()
            .iter()
            .position(|e| e.class.name == name);
        if let Some(idx) = embed_idx {
            let val = val.unwrap_named();
            match &val {
                Object::Instance(inst) if inst.class.name == name => {
                    self.embeds.borrow_mut()[idx] = inst.clone();
                    return Ok(());
                }
                _ => {
                    return Err(format!(
                        "嵌入字段 {} 需要一个 {} 实例，实际是 {}",
                        name,
                        name,
                        val.kind()
                    ))
                }
            }
        }

        let mut targets: Vec<Rc<Instance>> = Vec::new();
        for e in self.embeds.borrow().iter() {
            if e.resolve_field(name, e).found {
                targets.push(e.clone());
            }
        }
        match targets.len() {
            1 => targets[0].set_field_value(name, val),
            n if n > 1 => Err(format!(
                "{} 的字段 {} 在嵌入链中有歧义，需通过嵌入名限定访问",
                self.class.name, name
            )),
            _ => Err(format!("{} 没有字段 {}", self.class.name, name)),
        }
    }
}

// ---------------------------------------------------------------- 接口

/// 接口定义。模型无需声明实现，拥有全部方法即自动满足。
#[derive(Debug, Clone, Default)]
pub struct Interface {
    pub name: String,
    /// 方法名 → 签名。
    pub methods: HashMap<String, InterfaceMethodSig>,
    pub embeds: Vec<String>,
    pub visibility: Visibility,
}

impl Interface {
    pub fn new(name: impl Into<String>) -> Self {
        Interface {
            name: name.into(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------- 命名类型

/// 用户用 `定义 类型 名 底层类型` 定义的类型身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    pub name: String,
    pub base_type: DeclType,
    /// 底层是否为联合类型。
    pub is_union: bool,
}

impl NamedType {
    pub fn new(name: impl Into<String>, base_type: DeclType) -> Self {
        let is_union = base_type.is_union();
        NamedType {
            name: name.into(),
            base_type,
            is_union,
        }
    }
}

// ---------------------------------------------------------------- 内建函数

/// 原生函数的实现签名。
///
/// 与 Go 版 `func(rt *Runtime, args []object.Object) ([]object.Object, error)` 对应：
/// 返回多个值，失败时返回中文错误消息。
pub type NativeImpl = Rc<dyn Fn(&mut crate::Runtime, Vec<Object>) -> Result<Vec<Object>, String>>;

/// 内建函数分组。
///
/// 解释器只内置 `Core` 组；`Ext` 组位于独立动态库，由 `定义 本地("模块名.函数名") 函数 ...`
/// 按需加载，因此不使用扩展能力的程序无需加载任何第三方依赖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NativeGroup {
    /// 仅依赖 Rust 标准库，编译进解释器本体。
    #[default]
    Core,
    /// 依赖第三方库，交付为独立动态库。
    Ext,
}

impl std::fmt::Display for NativeGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeGroup::Core => write!(f, "core"),
            NativeGroup::Ext => write!(f, "ext"),
        }
    }
}

/// 内建函数描述符：注册框架的统一入口。
///
/// 新增内建只需构造一个 `NativeSpec` 并加入所属组的 `specs()` 列表。
#[derive(Clone)]
pub struct NativeSpec {
    /// 原生名，与 `定义 本地("名字") ...` 的绑定名一致。
    pub name: &'static str,
    /// 最少参数个数；`None` 表示不校验。
    pub arity: Option<usize>,
    /// 所属分组，决定交付与加载方式。
    pub group: NativeGroup,
    /// 所属动态库模块名（`Ext` 组有效）。
    pub ext_module: Option<&'static str>,
    /// 实现体。
    pub func: NativeImpl,
}

impl NativeSpec {
    /// 声明一个仅依赖标准库的内建。
    pub fn core(name: &'static str, arity: Option<usize>, func: NativeImpl) -> Self {
        NativeSpec {
            name,
            arity,
            group: NativeGroup::Core,
            ext_module: None,
            func,
        }
    }

    /// 声明一个位于动态库中的内建。
    pub fn ext(
        name: &'static str,
        arity: Option<usize>,
        module: &'static str,
        func: NativeImpl,
    ) -> Self {
        NativeSpec {
            name,
            arity,
            group: NativeGroup::Ext,
            ext_module: Some(module),
            func,
        }
    }
}

impl std::fmt::Debug for NativeSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSpec")
            .field("name", &self.name)
            .field("group", &self.group)
            .field("ext_module", &self.ext_module)
            .finish()
    }
}

/// 内建（原生）函数。
#[derive(Clone)]
pub struct Builtin {
    pub name: String,
    /// 最少参数个数，用于快速校验。
    pub arity: Option<usize>,
    pub func: NativeImpl,
}

impl Builtin {
    pub fn new(name: impl Into<String>, arity: Option<usize>, func: NativeImpl) -> Self {
        Builtin {
            name: name.into(),
            arity,
            func,
        }
    }
}

impl std::fmt::Debug for Builtin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Builtin")
            .field("name", &self.name)
            .field("arity", &self.arity)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fn(name: &str) -> Function {
        Function::new(
            name,
            Vec::new(),
            BlockStmt::default(),
            None,
            Position::START,
        )
    }

    #[test]
    fn 函数默认不是方法也不是分段() {
        let f = empty_fn("甲");
        assert!(!f.is_method);
        assert!(!f.is_segmented());
        assert!(!f.is_variadic());
        assert_eq!(f.required_arity(), 0);
        assert_eq!(f.receiver_name, "自己");
        assert_eq!(f.visibility, Visibility::File);
    }

    #[test]
    fn 函数区分必需参数与默认参数() {
        let mut f = empty_fn("甲");
        f.params.push(Parameter {
            name: "必填".to_string(),
            type_: DeclType::any(),
            variadic: false,
            default: None,
        });
        f.params.push(Parameter {
            name: "可选".to_string(),
            type_: DeclType::any(),
            variadic: false,
            default: Some(fan_ast::Expr::Nil(fan_ast::NilLit {
                pos: Position::START,
            })),
        });
        assert_eq!(f.required_arity(), 1);
        assert!(!f.is_variadic());
    }

    #[test]
    fn 可变参数识别() {
        let mut f = empty_fn("甲");
        f.params.push(Parameter {
            name: "其余".to_string(),
            type_: DeclType::named("整数"),
            variadic: true,
            default: None,
        });
        assert!(f.is_variadic());
        assert_eq!(f.required_arity(), 0);
    }

    #[test]
    fn 分段函数判定() {
        let mut f = empty_fn("根");
        f.name_segments = vec!["根".to_string(), "段".to_string()];
        assert!(f.is_segmented());
    }

    #[test]
    fn 模型字段与方法查找() {
        let mut cls = Class::new("矩形");
        cls.fields.push(FieldDecl {
            pos: Position::START,
            name: "宽".to_string(),
            type_: DeclType::named("整数"),
            visibility: Visibility::File,
        });
        let method = Rc::new(empty_fn("面积"));
        cls.methods.insert("面积".to_string(), method.clone());

        assert!(cls.field("宽").is_some());
        assert!(cls.field("高").is_none());
        assert!(cls.method("面积").is_some());
        assert!(cls.method("周长").is_none());
    }

    #[test]
    fn 实例字段读写() {
        let cls = Rc::new(Class::new("矩形"));
        let inst = Instance::new(cls);
        assert_eq!(inst.get("宽"), None);
        inst.set("宽", Object::Int(3));
        assert_eq!(inst.get("宽"), Some(Object::Int(3)));
    }

    #[test]
    fn 接口方法表() {
        let mut iface = Interface::new("说话者");
        iface.methods.insert(
            "说话".to_string(),
            InterfaceMethodSig {
                name: "说话".to_string(),
                params: Vec::new(),
                return_types: vec![DeclType::named("字符串")],
                pos: Position::START,
            },
        );
        assert!(iface.methods.contains_key("说话"));
        assert_eq!(iface.embeds.len(), 0);
    }

    #[test]
    fn 命名类型自动识别联合() {
        let single = NamedType::new("状态", DeclType::named("整数"));
        assert!(!single.is_union);

        let union = NamedType::new(
            "编号",
            DeclType::union(&["整数".to_string(), "字符串".to_string()]),
        );
        assert!(union.is_union);
    }

    #[test]
    fn 内建函数可携带参数个数() {
        let b = Builtin::new(
            "平方根",
            Some(1),
            Rc::new(|_rt, _args| Ok(vec![Object::Int(1)])),
        );
        assert_eq!(b.name, "平方根");
        assert_eq!(b.arity, Some(1));
        assert!(format!("{b:?}").contains("平方根"));
    }
}
