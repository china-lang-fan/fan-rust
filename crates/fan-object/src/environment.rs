//! 作用域环境。
//!
//! `Environment` 是一条由内向外的作用域链：查找沿链上溯，声明只写入当前层。
//! 与 Go 版 `evaluator.Environment` 一一对应，包含常量保护、可见性与声明类型检查。

use crate::kind::Kind;
use crate::{Class, Object, Runtime};
use fan_ast::{DeclType, Visibility};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// 环境中的一个绑定。
#[derive(Debug, Clone)]
pub struct Binding {
    pub value: Object,
    pub is_const: bool,
    pub visibility: Visibility,
    pub decl_type: DeclType,
}

/// 作用域环境。
#[derive(Debug)]
pub struct Environment {
    store: HashMap<String, Binding>,
    outer: Option<Rc<RefCell<Environment>>>,
    rt: Rc<RefCell<Runtime>>,
    /// 脚本所在目录，用于解析模块路径。
    pub base_dir: String,
    /// 当前源文件名，用于错误定位。
    pub source_file: String,
    /// 当前模块名。
    pub module_name: String,
    /// 是否处于模块导出模式：为 true 时声明会记录可见性以供导出。
    pub module_export: bool,
}

impl Environment {
    /// 创建根环境（自带一个新的运行实例）。
    pub fn new() -> Self {
        Environment::for_runtime(Rc::new(RefCell::new(Runtime::new())))
    }

    /// 创建绑定到指定运行实例的根环境。
    pub fn for_runtime(rt: Rc<RefCell<Runtime>>) -> Self {
        Environment {
            store: HashMap::new(),
            outer: None,
            rt,
            base_dir: String::new(),
            source_file: String::new(),
            module_name: String::new(),
            module_export: false,
        }
    }

    /// 创建内嵌于 `outer` 的新作用域。
    pub fn enclosed(outer: Rc<RefCell<Environment>>) -> Self {
        let (rt, base_dir, source_file, module_name) = {
            let o = outer.borrow();
            (
                o.rt.clone(),
                o.base_dir.clone(),
                o.source_file.clone(),
                o.module_name.clone(),
            )
        };
        Environment {
            store: HashMap::new(),
            outer: Some(outer),
            rt,
            base_dir,
            source_file,
            module_name,
            module_export: false,
        }
    }

    pub fn runtime(&self) -> Rc<RefCell<Runtime>> {
        self.rt.clone()
    }

    pub fn set_base_dir(&mut self, dir: impl Into<String>) {
        self.base_dir = dir.into();
    }

    // ------------------------------------------------------------ 查找与声明

    /// 沿作用域链查找绑定。
    pub fn find(&self, name: &str) -> Option<Binding> {
        if let Some(b) = self.store.get(name) {
            return Some(b.clone());
        }
        match &self.outer {
            Some(outer) => outer.borrow().find(name),
            None => None,
        }
    }

    /// 取值；未声明返回 `None`。
    pub fn get(&self, name: &str) -> Option<Object> {
        self.find(name).map(|b| b.value)
    }

    /// 当前作用域是否已声明该名字。
    pub fn has_local(&self, name: &str) -> bool {
        self.store.contains_key(name)
    }

    /// 声明一个新绑定，并做声明类型校验与 coercion。
    pub fn declare(
        &mut self,
        name: impl Into<String>,
        value: Object,
        is_const: bool,
        decl_type: DeclType,
        visibility: Visibility,
    ) -> Result<(), String> {
        let name = name.into();
        if self.store.contains_key(&name) {
            return Err(format!("变量 {} 已声明", name));
        }
        let value = coerce_decl_type(&decl_type, value, self)?;
        self.store.insert(
            name,
            Binding {
                value,
                is_const,
                visibility,
                decl_type,
            },
        );
        Ok(())
    }

    /// 声明无初值变量：初值为 `空`，跳过初始类型校验；后续赋值仍受约束。
    pub fn declare_zero(
        &mut self,
        name: impl Into<String>,
        decl_type: DeclType,
        visibility: Visibility,
    ) -> Result<(), String> {
        let name = name.into();
        if self.store.contains_key(&name) {
            return Err(format!("变量 {} 已声明", name));
        }
        self.store.insert(
            name,
            Binding {
                value: Object::Nil,
                is_const: false,
                visibility,
                decl_type,
            },
        );
        Ok(())
    }

    /// 赋值给已声明的变量。
    pub fn assign(&mut self, name: &str, value: Object) -> Result<(), String> {
        // 先取出约束，释放借用后再做 coercion，避免借用冲突。
        let (is_const, decl_type) = match self.store.get(name) {
            Some(b) => (b.is_const, b.decl_type.clone()),
            None => {
                return match &self.outer {
                    Some(outer) => outer.borrow_mut().assign(name, value),
                    None => Err(format!("变量 {} 未声明", name)),
                };
            }
        };
        if is_const {
            return Err(format!("常量 {} 不可再赋值", name));
        }
        let coerced = coerce_decl_type(&decl_type, value, self)?;
        if let Some(b) = self.store.get_mut(name) {
            b.value = coerced;
        }
        Ok(())
    }

    /// 只在本层内写入（用于函数参数绑定等场景，允许覆盖）。
    /// 列出本层所有 `公开` 绑定，供模块加载后填充导出表。
    ///
    /// 顺序按名字排序，保证模块 `Inspect` 输出稳定。
    pub fn public_bindings(&self) -> Vec<(String, Object)> {
        let mut items: Vec<(String, Object)> = self
            .store
            .iter()
            .filter(|(_, b)| b.visibility == Visibility::Public)
            .map(|(k, b)| (k.clone(), b.value.clone()))
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
    }

    /// 修改已声明绑定的可见性（`公开 变量 甲 = 1` 场景）。
    pub fn set_visibility(&mut self, name: &str, vis: Visibility) -> bool {
        if let Some(b) = self.store.get_mut(name) {
            b.visibility = vis;
            return true;
        }
        match &self.outer {
            Some(outer) => outer.borrow_mut().set_visibility(name, vis),
            None => false,
        }
    }

    pub fn set_local(&mut self, name: impl Into<String>, value: Object, decl_type: DeclType) {
        self.store.insert(
            name.into(),
            Binding {
                value,
                is_const: false,
                visibility: Visibility::File,
                decl_type,
            },
        );
    }

    /// 修改已存在绑定的值，不做类型校验与常量检查（用于内部注入）。
    pub fn force_set(&mut self, name: &str, value: Object) -> bool {
        match self.store.get_mut(name) {
            Some(b) => {
                b.value = value;
                true
            }
            None => match &self.outer {
                Some(outer) => outer.borrow_mut().force_set(name, value),
                None => false,
            },
        }
    }

    /// 当前层已声明的名字列表（按字典序，便于测试与调试）。
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.store.keys().cloned().collect();
        names.sort();
        names
    }

    /// 判断名字是否绑定到模块对象。
    pub fn is_module_name(&self, name: &str) -> bool {
        matches!(self.get(name), Some(Object::Module(_)))
    }
}

impl Default for Environment {
    fn default() -> Self {
        Environment::new()
    }
}

// ---------------------------------------------------------------- 类型检查

/// 把内部联合类型表示（`|` 连接）转为可读形式（` 或 ` 连接）。
pub fn format_decl_type(dt: &DeclType) -> String {
    if !dt.0.contains('|') {
        return dt.0.clone();
    }
    dt.0.split('|').collect::<Vec<_>>().join(" 或 ")
}

/// 校验值是否满足声明类型（含联合类型）。
pub fn check_decl_type(dt: &DeclType, value: &Object, env: &Environment) -> Result<(), String> {
    if dt.is_any() {
        return Ok(());
    }
    if dt.is_union() {
        for member in dt.0.split('|') {
            if member == "空" {
                if value.kind() == Kind::Nil {
                    return Ok(());
                }
                continue;
            }
            if check_single_decl_type(&DeclType::named(member), value, env).is_ok() {
                return Ok(());
            }
        }
        return Err(format!(
            "类型不匹配：声明为 {}，实际为 {}",
            format_decl_type(dt),
            value.kind()
        ));
    }
    check_single_decl_type(dt, value, env)
}

/// 校验值是否满足单个（非联合）声明类型。
pub fn check_single_decl_type(
    dt: &DeclType,
    value: &Object,
    env: &Environment,
) -> Result<(), String> {
    let name = dt.0.as_str();
    if name == "空" {
        return if value.kind() == Kind::Nil {
            Ok(())
        } else {
            Err(format!("类型不匹配：声明为 空，实际为 {}", value.kind()))
        };
    }

    // 基础类型：种类必须一致。错误类型额外允许 空（表示“无错误”）。
    let want: Option<Kind> = match name {
        "整数" => Some(Kind::Int),
        "小数" => Some(Kind::Float),
        "字符串" => Some(Kind::Str),
        "布尔" => Some(Kind::Bool),
        "数组" => Some(Kind::Array),
        "字典" => Some(Kind::Dict),
        _ => None,
    };
    if let Some(want_kind) = want {
        return if value.kind() == want_kind {
            Ok(())
        } else {
            Err(format!(
                "类型不匹配：声明为 {}，实际为 {}",
                name,
                value.kind()
            ))
        };
    }

    match name {
        "错误" => {
            if value.kind() == Kind::Nil || value.kind() == Kind::Error {
                Ok(())
            } else {
                Err(format!("类型不匹配：声明为 错误，实际为 {}", value.kind()))
            }
        }
        "字节" => match value {
            Object::Byte(_) => Ok(()),
            Object::Int(v) if (0..=255).contains(v) => Ok(()),
            Object::Int(v) => Err(format!("字节超出范围：{}（允许 0–255）", v)),
            other => Err(format!("类型不匹配：声明为 字节，实际为 {}", other.kind())),
        },
        "字符" => {
            if value.kind() == Kind::Char {
                Ok(())
            } else {
                Err(format!("类型不匹配：声明为 字符，实际为 {}", value.kind()))
            }
        }
        _ => check_named_or_interface_type(name, value, env),
    }
}

/// 自定义类型：先查命名类型身份，再查接口的结构化满足。
fn check_named_or_interface_type(
    name: &str,
    value: &Object,
    env: &Environment,
) -> Result<(), String> {
    if value.kind() == Kind::Nil {
        return Ok(());
    }
    // 命名类型：身份必须严格一致（参考 Go，不允许从底层类型隐式赋值）。
    if let Object::Named(n) = value {
        if n.type_name == name {
            return Ok(());
        }
    }
    if let Object::Instance(inst) = value {
        if inst.class.name == name {
            return Ok(());
        }
    }

    let rt = env.runtime();
    let rt_ref = rt.borrow();

    if let Some(nt) = rt_ref.lookup_named_type(name) {
        let nt = nt.clone();
        drop(rt_ref);
        // 联合命名类型：内部值满足任一成员即可。
        if nt.is_union {
            let inner = value.unwrap_named();
            for member in nt.base_type.0.split('|') {
                if check_single_decl_type(&DeclType::named(member), &inner, env).is_ok() {
                    return Ok(());
                }
            }
            return Err(format!(
                "类型不匹配：声明为 {}（{}），实际为 {}",
                name,
                format_decl_type(&nt.base_type),
                value.kind()
            ));
        }
        return Err(format!(
            "类型不匹配：声明为 {}，实际为 {}（需显式转换）",
            name,
            value.kind()
        ));
    }

    // 接口：按方法集做结构化满足检查。
    if let Some(iface) = rt_ref.lookup_interface(name) {
        let missing = interface_missing_methods(&iface, value);
        return match missing {
            None => Ok(()),
            Some(method) => Err(format!(
                "类型不匹配：声明为 {}，实际为 {}（缺少方法 {}）",
                name,
                value.kind(),
                method
            )),
        };
    }

    Err(format!(
        "类型不匹配：声明为 {}，实际为 {}",
        name,
        value.kind()
    ))
}

/// 返回值缺少的第一个接口方法名；全部满足时返回 `None`。
fn interface_missing_methods(iface: &crate::Interface, value: &Object) -> Option<String> {
    if iface.methods.is_empty() {
        return None;
    }
    for name in iface.methods.keys() {
        if lookup_method(value, name).is_none() {
            return Some(name.clone());
        }
    }
    None
}

/// 在值上查找方法：模型实例查自身方法与内建模型方法。
fn lookup_method(value: &Object, name: &str) -> Option<Rc<crate::Function>> {
    match value {
        Object::Instance(inst) => inst.class.method(name),
        Object::Str(_) | Object::Array(_) | Object::Dict(_) | Object::Int(_) | Object::Float(_) => {
            None // 原生类型方法由 fan-eval 的注册表处理
        }
        _ => None,
    }
}

/// 按声明类型转换值：字节会把 0–255 的整数规约成字节；
/// 联合命名类型会自动包装身份。
pub fn coerce_decl_type(dt: &DeclType, value: Object, env: &Environment) -> Result<Object, String> {
    if dt.is_any() {
        return Ok(value);
    }
    // 字节：整数在范围内自动规约。
    if dt.0 == "字节" {
        if matches!(value, Object::Byte(_)) {
            return Ok(value);
        }
        if let Object::Int(v) = value {
            return if (0..=255).contains(&v) {
                Ok(Object::Byte(v as u8))
            } else {
                Err(format!("字节超出范围：{}（允许 0–255）", v))
            };
        }
        return Err(format!("类型不匹配：声明为 字节，实际为 {}", value.kind()));
    }
    // 联合命名类型：满足任一成员即自动包装为该类型身份。
    if let Some(nt) = env.runtime().borrow().lookup_named_type(&dt.0).cloned() {
        if nt.is_union {
            let inner = value.unwrap_named();
            for member in nt.base_type.0.split('|') {
                if check_single_decl_type(&DeclType::named(member), &inner, env).is_ok() {
                    return Ok(Object::named(nt.name.clone(), inner));
                }
            }
            return Err(format!(
                "类型不匹配：声明为 {}（{}），实际为 {}",
                dt,
                format_decl_type(&nt.base_type),
                value.kind()
            ));
        }
    }
    check_decl_type(dt, &value, env)?;
    Ok(value)
}

/// 取出值所属的模型（实例或内建模型），供方法查找使用。
pub fn class_of(value: &Object) -> Option<Rc<Class>> {
    match value {
        Object::Instance(inst) => Some(inst.class.clone()),
        Object::Class(c) => Some(c.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_entity::Interface;

    fn env() -> Environment {
        Environment::new()
    }

    #[test]
    fn 声明与取值() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        assert_eq!(e.get("甲"), Some(Object::int(1)));
        assert_eq!(e.get("乙"), None);
    }

    #[test]
    fn 重复声明报错() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        let err = e.declare(
            "甲",
            Object::int(2),
            false,
            DeclType::any(),
            Visibility::File,
        );
        assert_eq!(err, Err("变量 甲 已声明".to_string()));
    }

    #[test]
    fn 未声明变量赋值报错() {
        let mut e = env();
        assert_eq!(
            e.assign("甲", Object::int(1)),
            Err("变量 甲 未声明".to_string())
        );
    }

    #[test]
    fn 常量不可再赋值() {
        let mut e = env();
        e.declare(
            "常量甲",
            Object::int(1),
            true,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        assert_eq!(
            e.assign("常量甲", Object::int(2)),
            Err("常量 常量甲 不可再赋值".to_string())
        );
    }

    #[test]
    fn 变量可再赋值() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        e.assign("甲", Object::int(2)).unwrap();
        assert_eq!(e.get("甲"), Some(Object::int(2)));
    }

    #[test]
    fn 内层可读取外层变量() {
        let outer = Rc::new(RefCell::new(env()));
        outer
            .borrow_mut()
            .declare(
                "外",
                Object::int(1),
                false,
                DeclType::any(),
                Visibility::File,
            )
            .unwrap();
        let inner = Environment::enclosed(outer.clone());
        assert_eq!(inner.get("外"), Some(Object::int(1)));
    }

    #[test]
    fn 内层赋值修改外层变量() {
        let outer = Rc::new(RefCell::new(env()));
        outer
            .borrow_mut()
            .declare(
                "外",
                Object::int(1),
                false,
                DeclType::any(),
                Visibility::File,
            )
            .unwrap();
        let mut inner = Environment::enclosed(outer.clone());
        inner.assign("外", Object::int(9)).unwrap();
        assert_eq!(outer.borrow().get("外"), Some(Object::int(9)));
    }

    #[test]
    fn 内层声明遮蔽外层同名变量() {
        let outer = Rc::new(RefCell::new(env()));
        outer
            .borrow_mut()
            .declare(
                "甲",
                Object::int(1),
                false,
                DeclType::any(),
                Visibility::File,
            )
            .unwrap();
        let mut inner = Environment::enclosed(outer.clone());
        inner
            .declare(
                "甲",
                Object::int(2),
                false,
                DeclType::any(),
                Visibility::File,
            )
            .unwrap();
        assert_eq!(inner.get("甲"), Some(Object::int(2)));
        assert_eq!(outer.borrow().get("甲"), Some(Object::int(1)));
    }

    #[test]
    fn 无初值声明初值为空() {
        let mut e = env();
        e.declare_zero("甲", DeclType::named("整数"), Visibility::File)
            .unwrap();
        assert_eq!(e.get("甲"), Some(Object::nil()));
    }

    #[test]
    fn 声明类型检查生效() {
        let mut e = env();
        assert!(e
            .declare(
                "甲",
                Object::int(1),
                false,
                DeclType::named("整数"),
                Visibility::File
            )
            .is_ok());
        let err = e.declare(
            "乙",
            Object::string("x"),
            false,
            DeclType::named("整数"),
            Visibility::File,
        );
        assert_eq!(
            err,
            Err("类型不匹配：声明为 整数，实际为 字符串".to_string())
        );
    }

    #[test]
    fn 赋值也受声明类型约束() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::named("整数"),
            Visibility::File,
        )
        .unwrap();
        assert_eq!(
            e.assign("甲", Object::string("x")),
            Err("类型不匹配：声明为 整数，实际为 字符串".to_string())
        );
    }

    #[test]
    fn 错误类型允许空值() {
        let mut e = env();
        assert!(e
            .declare(
                "错",
                Object::nil(),
                false,
                DeclType::named("错误"),
                Visibility::File
            )
            .is_ok());
    }

    #[test]
    fn 字节类型把整数规约为字节() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(200),
            false,
            DeclType::named("字节"),
            Visibility::File,
        )
        .unwrap();
        assert_eq!(e.get("甲"), Some(Object::Byte(200)));
    }

    #[test]
    fn 字节超出范围报错() {
        let mut e = env();
        let err = e.declare(
            "甲",
            Object::int(300),
            false,
            DeclType::named("字节"),
            Visibility::File,
        );
        assert_eq!(err, Err("字节超出范围：300（允许 0–255）".to_string()));
    }

    #[test]
    fn 联合类型满足任一成员即可() {
        let mut e = env();
        let dt = DeclType::union(&["整数".to_string(), "字符串".to_string()]);
        assert!(e
            .declare("甲", Object::int(1), false, dt.clone(), Visibility::File)
            .is_ok());
        assert!(e
            .declare(
                "乙",
                Object::string("x"),
                false,
                dt.clone(),
                Visibility::File
            )
            .is_ok());
        let err = e.declare("丙", Object::bool_of(true), false, dt, Visibility::File);
        assert!(err.unwrap_err().contains("类型不匹配"));
    }

    #[test]
    fn 联合类型含空时允许空值() {
        let dt = DeclType::union(&["整数".to_string(), "空".to_string()]);
        let mut e = env();
        assert!(e
            .declare("甲", Object::nil(), false, dt, Visibility::File)
            .is_ok());
    }

    #[test]
    fn 联合类型显示用或连接() {
        let dt = DeclType::union(&["整数".to_string(), "字符串".to_string()]);
        assert_eq!(format_decl_type(&dt), "整数 或 字符串");
        assert_eq!(format_decl_type(&DeclType::named("整数")), "整数");
    }

    #[test]
    fn 命名类型身份必须一致() {
        let mut e = env();
        let nt = crate::NamedType::new("状态", DeclType::named("整数"));
        e.runtime().borrow_mut().register_named_type(nt);
        assert!(e
            .declare(
                "甲",
                Object::named("状态", Object::int(1)),
                false,
                DeclType::named("状态"),
                Visibility::File
            )
            .is_ok());
        // 底层整数不能隐式当作命名类型
        let err = e.declare(
            "乙",
            Object::int(1),
            false,
            DeclType::named("状态"),
            Visibility::File,
        );
        assert!(err.unwrap_err().contains("需显式转换"));
    }

    #[test]
    fn 联合命名类型自动包装身份() {
        let mut e = env();
        let nt = crate::NamedType::new(
            "编号",
            DeclType::union(&["整数".to_string(), "字符串".to_string()]),
        );
        e.runtime().borrow_mut().register_named_type(nt);
        assert!(e
            .declare(
                "甲",
                Object::int(7),
                false,
                DeclType::named("编号"),
                Visibility::File
            )
            .is_ok());
        // 应被自动包装为命名类型
        assert_eq!(e.get("甲").unwrap().type_name(), Some("编号".to_string()));
    }

    #[test]
    fn 接口按方法集结构化满足() {
        let e = env();
        let mut iface = Interface::new("说话者");
        iface.methods.insert(
            "说话".to_string(),
            fan_ast::InterfaceMethodSig {
                name: "说话".to_string(),
                params: Vec::new(),
                return_types: Vec::new(),
                pos: fan_ast::Position::START,
            },
        );
        e.runtime().borrow_mut().register_interface(Rc::new(iface));

        // 拥有该方法的模型实例满足接口
        let mut cls = Class::new("狗");
        cls.methods.insert(
            "说话".to_string(),
            Rc::new(crate::Function::new(
                "说话",
                Vec::new(),
                fan_ast::BlockStmt::default(),
                None,
                fan_ast::Position::START,
            )),
        );
        let inst = Object::Instance(Rc::new(crate::Instance::new(Rc::new(cls))));
        assert!(check_decl_type(&DeclType::named("说话者"), &inst, &e).is_ok());

        // 缺少方法的实例不满足
        let empty = Object::Instance(Rc::new(crate::Instance::new(Rc::new(Class::new("石头")))));
        let err = check_decl_type(&DeclType::named("说话者"), &empty, &e);
        assert!(err.unwrap_err().contains("缺少方法 说话"));
    }

    #[test]
    fn 未知类型名报错() {
        let e = env();
        let err = check_decl_type(&DeclType::named("不存在"), &Object::int(1), &e);
        assert_eq!(
            err,
            Err("类型不匹配：声明为 不存在，实际为 整数".to_string())
        );
    }

    #[test]
    fn 名字列表按字典序() {
        let mut e = env();
        e.declare(
            "丙",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        assert_eq!(e.names(), vec!["丙".to_string(), "甲".to_string()]);
    }

    #[test]
    fn 类型不匹配时给出实际种类() {
        let e = env();
        assert_eq!(
            check_decl_type(&DeclType::named("字符串"), &Object::int(1), &e),
            Err("类型不匹配：声明为 字符串，实际为 整数".to_string())
        );
    }

    #[test]
    fn 空类型只接受空值() {
        let e = env();
        assert!(check_decl_type(&DeclType::named("空"), &Object::nil(), &e).is_ok());
        assert!(check_decl_type(&DeclType::named("空"), &Object::int(1), &e).is_err());
    }

    #[test]
    fn 模块名判定() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        let m = Rc::new(RefCell::new(crate::ModuleValue::new("数学")));
        e.declare(
            "数学",
            Object::Module(m),
            false,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        assert!(e.is_module_name("数学"));
        assert!(!e.is_module_name("甲"));
    }

    #[test]
    fn 强制写入绕过常量检查() {
        let mut e = env();
        e.declare(
            "甲",
            Object::int(1),
            true,
            DeclType::any(),
            Visibility::File,
        )
        .unwrap();
        assert!(e.force_set("甲", Object::int(9)));
        assert_eq!(e.get("甲"), Some(Object::int(9)));
        assert!(!e.force_set("不存在", Object::int(1)));
    }

    #[test]
    fn 取模型辅助函数() {
        let cls = Rc::new(Class::new("矩形"));
        let inst = Object::Instance(Rc::new(crate::Instance::new(cls.clone())));
        assert!(class_of(&inst).is_some());
        assert_eq!(class_of(&inst).unwrap().name, "矩形");
        assert!(class_of(&Object::int(1)).is_none());
    }
}
