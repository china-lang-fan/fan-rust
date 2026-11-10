//! 模型、接口与类型运算。
//!
//! 对应 Go 版 `evaluator/class.go` 与 `interface.go`。凡语言的模型是**结构化**的：
//! 接口无需声明实现，拥有全部方法即自动满足；嵌入（组合）通过字段与方法的
//! **提升**实现，同层多个嵌入提供同名成员时报歧义。

use crate::error::{value, EvalResult, FanError, Flow};
use crate::eval::Interpreter;
use fan_ast::{
    ClassStmt, DeclType, InterfaceStmt, MethodDef, Position, TypeCastExpr, TypeTestExpr, Visibility,
};
use fan_object::{
    primitive_kind_from_name, runtime_type_name, Class, Environment, FanArray, FanDict, Function,
    Instance, Interface, NamedType, Object,
};
use std::cell::RefCell;
use std::rc::Rc;

impl Interpreter {
    // ------------------------------------------------------------ 模型

    /// 求值 `模型 X … 结束`，把模型注册到当前环境。
    pub fn eval_class_stmt(&mut self, stmt: &ClassStmt) -> EvalResult {
        let mut cls = Class::new(stmt.name.clone());
        cls.fields = stmt.fields.clone();
        cls.parent = Some(self.env.clone());
        cls.visibility = stmt.visibility;

        for embed_name in &stmt.embeds {
            let existing = self.env.borrow().get(embed_name);
            let embed_cls = match existing {
                Some(Object::Class(c)) => c,
                Some(other) => {
                    return Err(FanError::at(
                        format!("{} 不是模型，实际是 {}", embed_name, other.kind()),
                        stmt.pos,
                    ))
                }
                None => {
                    return Err(FanError::at(
                        format!("嵌入的模型 {} 未定义", embed_name),
                        stmt.pos,
                    ))
                }
            };
            cls.embeds.push(embed_cls);
        }

        // 重复定义同名模型时（多文件合并场景）保留既有方法，只刷新字段与嵌入。
        let existing = self.env.borrow().get(&stmt.name);
        if let Some(Object::Class(c)) = existing {
            let mut merged = c.as_ref().clone();
            merged.fields = cls.fields;
            merged.embeds = cls.embeds;
            merged.parent = cls.parent;
            self.env
                .borrow_mut()
                .force_set(&stmt.name, Object::Class(Rc::new(merged)));
            return value(Object::Nil);
        }

        let cls = Rc::new(cls);
        let env = self.env.clone();
        env.borrow_mut()
            .declare(
                &stmt.name,
                Object::Class(cls),
                false,
                DeclType::any(),
                stmt.visibility,
            )
            .map_err(|e| FanError::at(e, stmt.pos))?;
        value(Object::Nil)
    }

    /// 求值 `类型名.方法名(…)` 形式的方法定义。
    ///
    /// 宿主类型名（字符串、数组、整数…）并非模型对象，其方法登记在运行实例的
    /// `primitive_methods` 中，调用时按接收者的类型标识查找。
    pub fn eval_method_def(&mut self, stmt: &MethodDef) -> EvalResult {
        let owner = self.env.borrow().get(&stmt.class_name);
        let Some(owner) = owner else {
            // 环境中没有该名字：若它是基本类型名，则为基本类型定义方法
            return self.define_primitive_method(stmt);
        };
        match owner {
            Object::Class(cls) => {
                let mut cls = cls.as_ref().clone();
                let mut func = Function {
                    name: stmt.method_name.clone(),
                    params: stmt.function.params.clone(),
                    body: stmt.function.body.clone(),
                    env: Some(self.env.clone()),
                    pos: stmt.pos,
                    return_types: stmt.function.return_types.clone(),
                    tags: stmt.function.tags.clone(),
                    visibility: stmt.visibility,
                    receiver_name: "自己".to_string(),
                    class_name: cls.name.clone(),
                    is_method: true,
                    owner: Some(Rc::new(cls.clone())),
                    ..Function::new(
                        &stmt.method_name,
                        Vec::new(),
                        Default::default(),
                        None,
                        stmt.pos,
                    )
                };
                // 方法同样支持 `@内建`：原生实现会收到接收者作为首个实参。
                self.process_function_tags(&mut func)?;
                cls.methods.insert(stmt.method_name.clone(), Rc::new(func));
                cls.method_vis
                    .insert(stmt.method_name.clone(), stmt.visibility);
                self.env
                    .borrow_mut()
                    .force_set(&stmt.class_name, Object::Class(Rc::new(cls)));
                value(Object::Nil)
            }
            other => Err(FanError::at(
                format!("{} 不是模型，实际是 {}", stmt.class_name, other.kind()),
                stmt.pos,
            )),
        }
    }

    /// 为基本类型或自定义类型定义方法：`定义 字符串 的 方法 截取(…)`。
    ///
    /// 自定义类型（`类型 姓名 字符串`）的方法登记在它自己的名下，
    /// 查找时优先于底层类型的方法，未命中再回退到底层类型。
    fn define_primitive_method(&mut self, stmt: &MethodDef) -> EvalResult {
        let type_name = match fan_object::primitive_kind_from_name(&stmt.class_name) {
            Some(kind) => kind.to_string(),
            None => {
                let rt = self.env.borrow().runtime();
                if rt.borrow().lookup_named_type(&stmt.class_name).is_some() {
                    stmt.class_name.clone()
                } else {
                    return Err(FanError::at(
                        format!("模型 {} 未定义", stmt.class_name),
                        stmt.pos,
                    ));
                }
            }
        };
        let mut func = Function {
            name: stmt.method_name.clone(),
            params: stmt.function.params.clone(),
            body: stmt.function.body.clone(),
            env: Some(self.env.clone()),
            pos: stmt.pos,
            return_types: stmt.function.return_types.clone(),
            tags: stmt.function.tags.clone(),
            visibility: stmt.visibility,
            receiver_name: "自己".to_string(),
            class_name: stmt.class_name.clone(),
            is_method: true,
            is_primitive_method: true,
            ..Function::new(
                &stmt.method_name,
                Vec::new(),
                Default::default(),
                None,
                stmt.pos,
            )
        };
        // 基本类型方法同样支持 `@内建`：原生实现会收到接收者作为首个实参。
        self.process_function_tags(&mut func)?;
        let rt = self.env.borrow().runtime();
        rt.borrow_mut()
            .register_primitive_method(&type_name, &stmt.method_name, Rc::new(func));
        value(Object::Nil)
    }

    // ------------------------------------------------------------ 实例化

    /// 创建模型实例：先构造零值实例，再调用 `初始化` 方法（若存在）。
    pub fn instantiate(
        &mut self,
        cls: &Rc<Class>,
        args: Vec<Object>,
        pos: Position,
    ) -> Result<Object, FanError> {
        let inst = new_zero_instance(cls);
        let init = cls.lookup_method("初始化");
        let Some(init_fn) = init else {
            if !args.is_empty() {
                return Err(FanError::at(
                    format!("模型 {} 没有 初始化 方法，不应传参", cls.name),
                    pos,
                ));
            }
            return Ok(Object::Instance(inst));
        };
        let bound = bind_method(&init_fn, inst.clone(), self.env.clone());
        self.call_bound(Rc::new(bound), args, pos)?;
        Ok(Object::Instance(inst))
    }

    /// 调用一个已绑定接收者的函数（用于构造与方法调用）。
    fn call_bound(
        &mut self,
        func: Rc<Function>,
        args: Vec<Object>,
        pos: Position,
    ) -> Result<Object, FanError> {
        let callee = Object::Func(func);
        match self.apply_function(&callee, args, pos)? {
            Flow::Value(v) => Ok(v),
            Flow::Return(values) => Ok(values.into_iter().next().unwrap_or(Object::Nil)),
            Flow::Throw(v) => Err(FanError::new(format!("未捕获的抛出：{}", v.inspect()))),
            _ => Err(FanError::at("函数调用中断", pos)),
        }
    }

    // ------------------------------------------------------------ 成员访问

    /// 取字段声明的可见性；未声明的字段按默认（文件级）处理。
    fn field_visibility(class: &Class, name: &str) -> Visibility {
        class
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.visibility)
            .unwrap_or_default()
    }

    /// 校验 `私有` 成员：只允许在所属模型自己的方法体内访问。
    fn check_private(
        &self,
        owner: &Rc<Class>,
        name: &str,
        vis: Visibility,
        what: &str,
        pos: Position,
    ) -> Result<(), FanError> {
        if vis != Visibility::Private {
            return Ok(());
        }
        if self.inside_class(&owner.name) {
            return Ok(());
        }
        Err(FanError::at(
            format!(
                "{} 的{} {} 是私有的，只能在 {} 的方法内访问",
                owner.name, what, name, owner.name
            ),
            pos,
        ))
    }

    /// 读取成员：模型字段/方法（含嵌入提升）、字典键、模块导出。
    pub fn get_member(
        &mut self,
        obj: &Object,
        name: &str,
        pos: Position,
    ) -> Result<Object, FanError> {
        // 自定义类型的方法优先：`类型 姓名 字符串` 上定义的 `问候`
        // 要比底层 字符串 的同名方法先命中
        if let Object::Named(named) = obj {
            if let Some(found) = self.bind_method_for_type(&named.type_name, obj, name) {
                return Ok(found);
            }
        }
        match obj.unwrap_named() {
            Object::Instance(inst) => {
                if let Some(embed) = inst.embed_by_name(name) {
                    return Ok(Object::Instance(embed));
                }
                let r = inst.resolve_field(name, &inst);
                if r.ambiguous {
                    return Err(FanError::at(
                        format!("{} 的字段 {} 在嵌入链中有歧义", inst.class.name, name),
                        pos,
                    ));
                }
                if let Some(v) = r.value {
                    // 私有字段只允许在所属模型自己的方法内读
                    if let Some(owner) = &r.instance {
                        self.check_private(
                            &owner.class,
                            name,
                            Self::field_visibility(&owner.class, name),
                            "字段",
                            pos,
                        )?;
                    }
                    return Ok(v);
                }
                // 字段未命中，尝试方法
                let m = inst.class.resolve_method(name, &inst.class);
                if m.ambiguous {
                    return Err(FanError::at(
                        format!(
                            "{} 的方法 {} 在嵌入链中有歧义，需通过嵌入名限定",
                            inst.class.name, name
                        ),
                        pos,
                    ));
                }
                let Some(func) = m.func else {
                    return Err(FanError::at(
                        format!("{} 没有字段或方法 {}", inst.class.name, name),
                        pos,
                    ));
                };
                // 私有方法只允许在所属模型自己的方法内调用
                if let Some(owner) = &m.owner_class {
                    let vis = owner.method_vis.get(name).copied().unwrap_or_default();
                    self.check_private(owner, name, vis, "方法", pos)?;
                }
                Ok(Object::Func(Rc::new(bind_method(
                    &func,
                    inst,
                    self.env.clone(),
                ))))
            }
            Object::Dict(dict) => {
                // 键优先：用户数据不应被内建成员遮蔽
                if let Some(v) = dict.get(&Object::string(name)) {
                    return Ok(v);
                }
                match Self::dict_builtin(&dict, name) {
                    Some(v) => Ok(v),
                    None => self.bind_primitive_method(obj, name, pos),
                }
            }
            Object::Module(m) => m.borrow().exports.get(name).cloned().ok_or_else(|| {
                FanError::at(
                    format!("模块 {} 没有公开成员 {}", m.borrow().name, name),
                    pos,
                )
            }),
            // 基本类型：先查内建成员，再查 `定义 字符串 的 方法 X()` 登记的方法
            Object::Str(_)
            | Object::Int(_)
            | Object::Float(_)
            | Object::Bool(_)
            | Object::Byte(_)
            | Object::Char(_) => self.bind_primitive_method(obj, name, pos),
            Object::Array(arr) => match Self::array_builtin(&arr, name) {
                Some(v) => Ok(v),
                None => self.bind_primitive_method(obj, name, pos),
            },
            other => Err(FanError::at(
                format!("{} 不支持成员访问", other.kind()),
                pos,
            )),
        }
    }

    /// 字典的内建成员：`长度`、`键`、`值`。
    ///
    /// 键查找未命中时才走到这里，故用户数据不会遮蔽内建成员。
    fn dict_builtin(dict: &FanDict, name: &str) -> Option<Object> {
        match name {
            "长度" => Some(Object::int(dict.len() as i64)),
            "键" => Some(Object::array(dict.keys())),
            "值" => Some(Object::array(
                dict.keys().iter().filter_map(|k| dict.get(k)).collect(),
            )),
            _ => None,
        }
    }

    /// 数组的内建成员：`长度` 与 `反转`（后者直接给出倒序后的新数组）。
    fn array_builtin(arr: &FanArray, name: &str) -> Option<Object> {
        match name {
            "长度" => Some(Object::int(arr.len() as i64)),
            "反转" => Some(Object::array(
                arr.elements().into_iter().rev().collect::<Vec<_>>(),
            )),
            _ => None,
        }
    }

    /// 为指定类型名查找方法并绑定接收者；未定义时返回 `None`（供回退用）。
    fn bind_method_for_type(&self, type_name: &str, obj: &Object, name: &str) -> Option<Object> {
        let rt = self.env.borrow().runtime();
        let func = rt.borrow().lookup_primitive_method(type_name, name)?;
        let mut bound = func.as_ref().clone();
        bound.self_value = Some(obj.clone());
        Some(Object::Func(Rc::new(bound)))
    }

    /// 把为基本类型定义的方法绑定当前接收者，返回可调用的函数。
    fn bind_primitive_method(
        &self,
        obj: &Object,
        name: &str,
        pos: Position,
    ) -> Result<Object, FanError> {
        let type_name = fan_object::runtime_type_name(obj);
        self.bind_method_for_type(type_name, obj, name)
            .ok_or_else(|| {
                FanError::at(
                    format!(
                        "{} 没有方法 {}",
                        fan_object::kind_display_name(type_name),
                        name
                    ),
                    pos,
                )
            })
    }

    /// 写入成员：模型字段（含嵌入提升）、字典键、模块导出。
    pub fn set_member(
        &mut self,
        obj: &Object,
        name: &str,
        val: Object,
        pos: Position,
    ) -> Result<(), FanError> {
        match obj.unwrap_named() {
            Object::Instance(inst) => {
                // 私有字段只允许在所属模型自己的方法内写
                let r = inst.resolve_field(name, &inst);
                if let Some(owner) = &r.instance {
                    self.check_private(
                        &owner.class,
                        name,
                        Self::field_visibility(&owner.class, name),
                        "字段",
                        pos,
                    )?;
                }
                inst.set_field_value(name, val)
                    .map_err(|e| FanError::at(e, pos))
            }
            Object::Dict(mut dict) => {
                dict.set(Object::string(name), val);
                Ok(())
            }
            Object::Module(m) => {
                m.borrow_mut().set(name, val);
                Ok(())
            }
            other => Err(FanError::at(
                format!("{} 不支持字段赋值", other.kind()),
                pos,
            )),
        }
    }

    // ------------------------------------------------------------ 接口

    /// 求值 `接口 X … 结束`。
    ///
    /// 先把接口（含嵌入名）原样注册到运行实例，再展平嵌入接口：
    /// 预先注册是为了支持前向引用与相互可见，展平时把嵌入方法并入本接口，
    /// 带环检测，同名方法按签名比对，冲突报错。与 Go 版 `registerInterface` 一致。
    pub fn eval_interface_stmt(&mut self, stmt: &InterfaceStmt) -> EvalResult {
        let mut iface = Interface::new(stmt.name.clone());
        iface.visibility = stmt.visibility;
        for m in &stmt.methods {
            if iface.methods.contains_key(&m.name) {
                return Err(FanError::at(
                    format!("接口 {} 的方法 {} 重复声明", stmt.name, m.name),
                    m.pos,
                ));
            }
            iface.methods.insert(m.name.clone(), m.clone());
        }
        iface.embeds = stmt.embeds.clone();

        // 先注册，使后续展平可按名找到（含同文件中定义在前向位置的接口）。
        {
            let env = self.env.clone();
            env.borrow()
                .runtime()
                .borrow_mut()
                .register_interface(Rc::new(iface.clone()));
        }

        self.flatten_interface(&mut iface, &mut Vec::new())
            .map_err(|reason| FanError::at(reason, stmt.pos))?;

        // 展平后用完整方法集覆盖注册。
        let env = self.env.clone();
        env.borrow()
            .runtime()
            .borrow_mut()
            .register_interface(Rc::new(iface));
        env.borrow_mut()
            .declare(
                &stmt.name,
                Object::Nil,
                false,
                DeclType::any(),
                stmt.visibility,
            )
            .map_err(|e| FanError::at(e, stmt.pos))?;
        value(Object::Nil)
    }

    /// 递归展平接口的嵌入方法集。
    ///
    /// `visiting` 是当前展开链上的接口名，用于检测循环嵌入；
    /// 同名但签名不同的方法报错，签名一致（含都未标注）则合并。
    fn flatten_interface(
        &self,
        iface: &mut Interface,
        visiting: &mut Vec<String>,
    ) -> Result<(), String> {
        let embed_names = iface.embeds.clone();
        for embed_name in &embed_names {
            if visiting.iter().any(|n| n == embed_name) {
                return Err(format!("检测到接口循环嵌入：{embed_name}"));
            }
            let embed = {
                let env = self.env.borrow();
                let rt = env.runtime();
                let rt_ref = rt.borrow();
                match rt_ref.lookup_interface(embed_name) {
                    Some(embed) => embed,
                    None => {
                        return Err(format!(
                            "接口 {} 嵌入了未定义的接口 {}",
                            iface.name, embed_name
                        ))
                    }
                }
            };

            visiting.push(embed_name.clone());
            // 先展平被嵌入接口自身的嵌入，保证拿到完整方法集。
            let mut embed_flat = (*embed).clone();
            self.flatten_interface(&mut embed_flat, visiting)?;
            visiting.pop();

            for (name, method) in embed_flat.methods {
                if let Some(existing) = iface.methods.get(&name) {
                    let conflict = fan_object::interface_sig_conflict(existing, &method);
                    if !conflict.is_empty() {
                        return Err(format!(
                            "接口 {} 中方法 {} 的签名冲突（{}）",
                            iface.name, name, conflict
                        ));
                    }
                    continue;
                }
                iface.methods.insert(name, method);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ 类型运算

    /// 求值 `X 是 类型`，返回布尔。
    pub fn eval_type_test(&mut self, expr: &TypeTestExpr) -> EvalResult {
        let v = self.eval_expr(&expr.object)?.into_value()?;
        let env = self.env.clone();
        let env = env.borrow();
        value(Object::Bool(value_is_type(&v, &expr.type_name, &env)))
    }

    /// 求值 `X 转为 类型`，返回 `(值, 是否成功)` 多元值。
    pub fn eval_type_cast(&mut self, expr: &TypeCastExpr) -> EvalResult {
        let v = self.eval_expr(&expr.object)?.into_value()?;
        let env = self.env.clone();
        let env_ref = env.borrow();

        // 目标是自定义命名类型：先尝试身份一致，再按底层类型包装。
        if let Some(nt) = env_ref
            .runtime()
            .borrow()
            .lookup_named_type(&expr.type_name)
            .cloned()
        {
            if v.type_name().as_deref() == Some(expr.type_name.as_str()) {
                return value(Object::tuple(vec![v, Object::Bool(true)]));
            }
            return match convert_to_named(&nt, &v) {
                Ok(converted) => value(Object::tuple(vec![converted, Object::Bool(true)])),
                Err(_) => value(Object::tuple(vec![Object::Nil, Object::Bool(false)])),
            };
        }
        if value_is_type(&v, &expr.type_name, &env_ref) {
            return value(Object::tuple(vec![v, Object::Bool(true)]));
        }
        // 基本类型按值转换；失败返回 (空, 假)。
        if let Some(kind) = primitive_kind_from_name(&expr.type_name) {
            let inner = v.unwrap_named();
            if let Some(converted) = convert_to_primitive_kind(&inner, kind) {
                return value(Object::tuple(vec![converted, Object::Bool(true)]));
            }
        }
        value(Object::tuple(vec![Object::Nil, Object::Bool(false)]))
    }
}

// ---------------------------------------------------------------- 辅助

/// 递归构造模型及其嵌入链的零值实例（字段初值为 空）。
pub fn new_zero_instance(cls: &Rc<Class>) -> Rc<Instance> {
    let inst = Rc::new(Instance::new(cls.clone()));
    for field in &cls.fields {
        inst.set(field.name.clone(), Object::Nil);
    }
    for embed in &cls.embeds {
        inst.embeds.borrow_mut().push(new_zero_instance(embed));
    }
    inst
}

/// 把函数绑定到接收者：新建一层环境并声明 `自己`。
pub fn bind_method(
    f: &Function,
    inst: Rc<Instance>,
    fallback_env: Rc<RefCell<Environment>>,
) -> Function {
    let base = f.env.clone().unwrap_or(fallback_env);
    let env = Rc::new(RefCell::new(Environment::enclosed(base)));
    env.borrow_mut().set_local(
        "自己".to_string(),
        Object::Instance(inst.clone()),
        DeclType::any(),
    );
    Function {
        name: f.name.clone(),
        params: f.params.clone(),
        body: f.body.clone(),
        env: Some(env),
        pos: f.pos,
        return_types: f.return_types.clone(),
        tags: f.tags.clone(),
        visibility: f.visibility,
        receiver_name: f.receiver_name.clone(),
        class_name: f.class_name.clone(),
        is_primitive_method: f.is_primitive_method,
        is_method: true,
        self_value: Some(Object::Instance(inst)),
        native_impl: f.native_impl.clone(),
        owner: f.owner.clone(),
        name_segments: f.name_segments.clone(),
        connectors: f.connectors.clone(),
        group_sizes: f.group_sizes.clone(),
        signature: f.signature.clone(),
    }
}

/// 判断值是否属于指定类型名。规则与 Go 版 `valueIsType` 一致。
pub fn value_is_type(value: &Object, type_name: &str, env: &Environment) -> bool {
    if let Some(kind) = primitive_kind_from_name(type_name) {
        return runtime_type_name(value) == kind;
    }
    if value.kind() == fan_object::Kind::Nil {
        if matches!(type_name, "空" | "错误") {
            return true;
        }
        return match env.runtime().borrow().lookup_interface(type_name) {
            Some(iface) => interface_missing_method(&iface, value, env).is_none(),
            None => false,
        };
    }
    // 自定义命名类型：身份严格一致
    if value.type_name().as_deref() == Some(type_name) {
        return true;
    }
    let rt = env.runtime();
    if let Some(nt) = rt.borrow().lookup_named_type(type_name).cloned() {
        if nt.is_union {
            // 联合命名类型：内部值满足任一成员即可
            let inner = value.unwrap_named();
            return nt.base_type.0.split('|').any(|m| {
                primitive_kind_from_name(m)
                    .map(|k| runtime_type_name(&inner) == k)
                    .unwrap_or_else(|| value_is_type(&inner, m, env))
            });
        }
        return false;
    }
    if let Object::Instance(inst) = value {
        if inst.class.name == type_name || inst.class.has_embed(type_name) {
            return true;
        }
    }
    let iface = rt.borrow().lookup_interface(type_name);
    match iface {
        Some(iface) => interface_missing_method(&iface, value, env).is_none(),
        None => false,
    }
}

/// 返回第一个不满足的接口方法名及原因；全部满足返回 `None`。
pub fn interface_missing_method(
    iface: &Interface,
    value: &Object,
    env: &Environment,
) -> Option<(String, String)> {
    fan_object::interface_missing_method(iface, value, env)
}

/// 把值包装为命名类型；底层类型不满足时失败。
pub fn convert_to_named(nt: &NamedType, value: &Object) -> Result<Object, String> {
    let inner = value.unwrap_named();
    if nt.is_union {
        for member in nt.base_type.0.split('|') {
            if let Some(kind) = primitive_kind_from_name(member) {
                if runtime_type_name(&inner) == kind {
                    return Ok(Object::named(nt.name.clone(), inner));
                }
            }
        }
        return Err(format!("无法转换为 {}", nt.name));
    }
    let base = nt.base_type.0.as_str();
    match primitive_kind_from_name(base) {
        Some(kind) if runtime_type_name(&inner) == kind => {
            Ok(Object::named(nt.name.clone(), inner))
        }
        _ => Err(format!("无法转换为 {}", nt.name)),
    }
}

/// 基本类型间的值转换（供 `转为` 使用）；失败返回 `None`。
pub fn convert_to_primitive_kind(value: &Object, kind: &str) -> Option<Object> {
    if runtime_type_name(value) == kind {
        return Some(value.clone());
    }
    match kind {
        "integer" => match value {
            Object::Float(v) => Some(Object::Int(*v as i64)),
            Object::Bool(v) => Some(Object::Int(if *v { 1 } else { 0 })),
            Object::Byte(v) => Some(Object::Int(*v as i64)),
            Object::Char(v) => Some(Object::Int(*v as i64)),
            Object::Str(s) => s.trim().parse::<i64>().ok().map(Object::Int),
            _ => None,
        },
        "float" => match value {
            Object::Int(v) => Some(Object::Float(*v as f64)),
            Object::Bool(v) => Some(Object::Float(if *v { 1.0 } else { 0.0 })),
            Object::Byte(v) => Some(Object::Float(*v as f64)),
            Object::Char(v) => Some(Object::Float(*v as i64 as f64)),
            Object::Str(s) => s.trim().parse::<f64>().ok().map(Object::Float),
            _ => None,
        },
        "string" => Some(Object::string(value.inspect())),
        "byte" => integer_like(value)
            .filter(|n| (0..=255).contains(n))
            .map(|n| Object::Byte(n as u8)),
        "char" => match value {
            Object::Char(v) => Some(Object::Char(*v)),
            Object::Str(s) => {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => Some(Object::Char(c)),
                    _ => None,
                }
            }
            Object::Int(v) => char::from_u32(*v as u32).map(Object::Char),
            Object::Byte(v) => Some(Object::Char(*v as char)),
            _ => None,
        },
        "boolean" => match value {
            Object::Int(v) => Some(Object::Bool(*v != 0)),
            Object::Float(v) => Some(Object::Bool(*v != 0.0)),
            Object::Str(s) => match s.trim().to_lowercase().as_str() {
                "真" | "true" | "1" | "yes" | "on" => Some(Object::Bool(true)),
                "假" | "false" | "0" | "no" | "off" => Some(Object::Bool(false)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// 取整数族的值（整数/字节/字符）。
fn integer_like(value: &Object) -> Option<i64> {
    match value {
        Object::Int(v) => Some(*v),
        Object::Byte(v) => Some(*v as i64),
        Object::Char(v) => Some(*v as i64),
        Object::Float(v) => Some(*v as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::Interpreter;
    use fan_ast::{DeclType, FieldDecl, InterfaceMethodSig, Visibility};
    use fan_parser::parse_program;

    fn run(src: &str) -> Result<Object, String> {
        let (program, errors) = parse_program(src);
        if !errors.is_empty() {
            return Err(format!("解析错误：{errors}"));
        }
        Interpreter::new().run(&program).map_err(|e| e.display())
    }

    fn check(src: &str, expected: &str) {
        match run(src) {
            Ok(v) => assert_eq!(v.inspect(), expected, "源码：{src}"),
            Err(e) => panic!("执行失败：{e}\n源码：{src}"),
        }
    }

    fn class_with_fields(name: &str, fields: &[&str]) -> Rc<Class> {
        let mut cls = Class::new(name);
        for f in fields {
            cls.fields.push(FieldDecl {
                pos: Position::START,
                name: f.to_string(),
                type_: DeclType::any(),
                visibility: Visibility::File,
            });
        }
        Rc::new(cls)
    }

    fn method_fn(name: &str) -> Rc<Function> {
        Rc::new(Function::new(
            name,
            Vec::new(),
            Default::default(),
            None,
            Position::START,
        ))
    }

    // ---------------------------------------------------------- 单元测试

    #[test]
    fn 零值实例字段初值为空() {
        let cls = class_with_fields("矩形", &["宽", "高"]);
        let inst = new_zero_instance(&cls);
        assert_eq!(inst.get("宽"), Some(Object::Nil));
        assert_eq!(inst.get("高"), Some(Object::Nil));
        assert_eq!(inst.get("不存在"), None);
        assert!(inst.embeds.borrow().is_empty());
    }

    #[test]
    fn 零值实例递归构造嵌入链() {
        let inner = class_with_fields("动物", &["名"]);
        let mut outer = Class::new("狗");
        outer.embeds.push(inner);
        let outer = Rc::new(outer);

        let inst = new_zero_instance(&outer);
        assert_eq!(inst.embeds.borrow().len(), 1);
        assert!(inst.embed_by_name("动物").is_some());
    }

    #[test]
    fn 可按嵌入模型名整体替换子实例() {
        // 复合字面量 `狗窝{动物: 动物{名: "旺财"}}` 走的就是这条路径
        let inner = class_with_fields("动物", &["名"]);
        let mut outer = Class::new("狗窝");
        outer.embeds.push(inner.clone());
        let outer = Rc::new(outer);

        let inst = new_zero_instance(&outer);
        let replacement = new_zero_instance(&inner);
        replacement.set("名", Object::string("旺财"));
        inst.set_field_value("动物", Object::Instance(replacement))
            .expect("嵌入名应当可写");

        let embedded = inst.embed_by_name("动物").expect("应取到替换后的实例");
        assert_eq!(embedded.get("名"), Some(Object::string("旺财")));
    }

    #[test]
    fn 嵌入名写入类型不符时报错() {
        let inner = class_with_fields("动物", &["名"]);
        let mut outer = Class::new("狗窝");
        outer.embeds.push(inner);
        let inst = new_zero_instance(&Rc::new(outer));
        assert!(inst
            .set_field_value("动物", Object::string("不是实例"))
            .is_err());
    }

    #[test]
    fn 嵌入字段提升查找() {
        let inner = class_with_fields("动物", &["名"]);
        let mut outer = Class::new("狗");
        outer.embeds.push(inner.clone());
        let outer = Rc::new(outer);

        let inst = new_zero_instance(&outer);
        inst.set_field_value("名", Object::string("旺财")).unwrap();
        let r = inst.resolve_field("名", &inst);
        assert!(r.found);
        assert_eq!(r.value, Some(Object::string("旺财")));
    }

    #[test]
    fn 写入不存在的字段报错() {
        let cls = class_with_fields("矩形", &["宽"]);
        let inst = new_zero_instance(&cls);
        assert_eq!(
            inst.set_field_value("不存在", Object::int(1)),
            Err("矩形 没有字段 不存在".to_string())
        );
    }

    #[test]
    fn 同层多嵌入同名字段报歧义() {
        let mut outer = Class::new("外层");
        outer.embeds.push(class_with_fields("甲", &["名"]));
        outer.embeds.push(class_with_fields("乙", &["名"]));
        let outer = Rc::new(outer);

        let inst = new_zero_instance(&outer);
        assert!(inst.resolve_field("名", &inst).ambiguous);
        assert!(inst
            .set_field_value("名", Object::Nil)
            .unwrap_err()
            .contains("有歧义"));
    }

    // ------------------------------------------------ 为基本类型定义方法

    #[test]
    fn 为字符串定义方法并调用() {
        check(
            "定义 字符串 的 方法 翻倍(整数 次数) -> 字符串\n    返回 自己\n结束\n\"甲\".翻倍(2)",
            "甲",
        );
    }

    #[test]
    fn 基本类型方法体内可用自己() {
        check(
            "定义 字符串 的 方法 首尾() -> 字符串\n    返回 自己\n结束\n\"凡\".首尾()",
            "凡",
        );
    }

    #[test]
    fn 为数组定义方法() {
        check(
            "定义 数组 的 方法 元素个数() -> 整数\n    返回 0\n结束\n[1, 2].元素个数()",
            "0",
        );
    }

    #[test]
    fn 为整数定义方法() {
        check(
            "定义 整数 的 方法 加倍() -> 整数\n    返回 自己 乘 2\n结束\n21.加倍()",
            "42",
        );
    }

    #[test]
    fn 未定义的类型名仍报模型未定义() {
        let err = run("定义 甲 的 方法 乙() -> 整数\n    返回 1\n结束\n").unwrap_err();
        assert!(err.contains("模型 甲 未定义"), "实际：{}", err);
    }

    #[test]
    fn 基本类型上不存在的方法报错() {
        let err = run("\"甲\".不存在的方法()").unwrap_err();
        assert!(err.contains("没有方法"), "实际：{}", err);
    }

    #[test]
    fn 字典键优先于基本类型方法() {
        // 字典成员访问沿用键语义，键缺失时才回落方法查找
        check(
            "定义 字典 的 方法 甲() -> 整数\n    返回 7\n结束\n变量 表 = {\"甲\": 1}\n表.甲",
            "1",
        );
    }

    #[test]
    fn 基本类型方法可跨类型隔离() {
        // 同名方法分别定义在字符串与整数上，互不覆盖
        check(
            "定义 字符串 的 方法 标记() -> 字符串\n    返回 \"文\"\n结束\n定义 整数 的 方法 标记() -> 字符串\n    返回 \"数\"\n结束\n\"甲\".标记()",
            "文",
        );
        check(
            "定义 字符串 的 方法 标记() -> 字符串\n    返回 \"文\"\n结束\n定义 整数 的 方法 标记() -> 字符串\n    返回 \"数\"\n结束\n1.标记()",
            "数",
        );
    }

    #[test]
    fn 方法绑定声明自己变量() {
        let mut inner = Class::new("动物");
        inner.methods.insert("说话".to_string(), method_fn("说话"));
        let inner = Rc::new(inner);
        let mut outer = Class::new("狗");
        outer.embeds.push(inner);
        let outer = Rc::new(outer);

        assert!(outer.lookup_method("说话").is_some());
        assert!(outer.lookup_method("跑").is_none());
    }

    #[test]
    fn 同层多嵌入同名方法报歧义() {
        let mut a = Class::new("甲");
        a.methods.insert("说话".to_string(), method_fn("说话"));
        let mut b = Class::new("乙");
        b.methods.insert("说话".to_string(), method_fn("说话"));
        let mut outer = Class::new("外层");
        outer.embeds.push(Rc::new(a));
        outer.embeds.push(Rc::new(b));
        let outer = Rc::new(outer);

        // 歧义时 lookup_method 返回 None
        assert!(outer.lookup_method("说话").is_none());
        assert!(outer.resolve_method("说话", &outer).ambiguous);
    }

    #[test]
    fn 嵌入链查询包含深层模型() {
        let leaf = Rc::new(Class::new("叶"));
        let mut mid = Class::new("中");
        mid.embeds.push(leaf);
        let mid = Rc::new(mid);
        let mut top = Class::new("顶");
        top.embeds.push(mid);
        let top = Rc::new(top);

        assert!(top.has_embed("叶"));
        assert!(top.has_embed("中"));
        assert!(!top.has_embed("不存在"));
    }

    #[test]
    fn 接口缺失方法检测() {
        let mut iface = Interface::new("说话者");
        iface.methods.insert(
            "说话".to_string(),
            InterfaceMethodSig {
                name: "说话".to_string(),
                params: Vec::new(),
                return_types: Vec::new(),
                pos: Position::START,
            },
        );
        let empty_cls = Rc::new(Class::new("石头"));
        let inst = Object::Instance(new_zero_instance(&empty_cls));
        let env = Environment::new();
        assert_eq!(
            interface_missing_method(&iface, &inst, &env),
            Some(("说话".to_string(), "缺少".to_string()))
        );
    }

    #[test]
    fn 接口满足时无缺失方法() {
        let mut cls = Class::new("狗");
        cls.methods.insert("说话".to_string(), method_fn("说话"));
        let cls = Rc::new(cls);
        let mut iface = Interface::new("说话者");
        iface.methods.insert(
            "说话".to_string(),
            InterfaceMethodSig {
                name: "说话".to_string(),
                params: Vec::new(),
                return_types: Vec::new(),
                pos: Position::START,
            },
        );
        let inst = Object::Instance(new_zero_instance(&cls));
        let env = Environment::new();
        assert_eq!(interface_missing_method(&iface, &inst, &env), None);
    }

    #[test]
    fn 基本类型互转() {
        assert_eq!(
            convert_to_primitive_kind(&Object::Float(3.7), "integer"),
            Some(Object::Int(3))
        );
        assert_eq!(
            convert_to_primitive_kind(&Object::Int(3), "float"),
            Some(Object::Float(3.0))
        );
        assert_eq!(
            convert_to_primitive_kind(&Object::string("42"), "integer"),
            Some(Object::Int(42))
        );
        assert_eq!(
            convert_to_primitive_kind(&Object::int(1), "string"),
            Some(Object::string("1"))
        );
        assert_eq!(
            convert_to_primitive_kind(&Object::Int(65), "char"),
            Some(Object::Char('A'))
        );
        assert_eq!(
            convert_to_primitive_kind(&Object::Int(255), "byte"),
            Some(Object::Byte(255))
        );
        assert_eq!(convert_to_primitive_kind(&Object::Int(256), "byte"), None);
        assert_eq!(
            convert_to_primitive_kind(&Object::Bool(true), "integer"),
            Some(Object::Int(1))
        );
    }

    #[test]
    fn 字符串转布尔识别中英文() {
        for s in ["真", "true", "1", "YES", "on"] {
            assert_eq!(
                convert_to_primitive_kind(&Object::string(s), "boolean"),
                Some(Object::Bool(true)),
                "{s}"
            );
        }
        for s in ["假", "false", "0", "no"] {
            assert_eq!(
                convert_to_primitive_kind(&Object::string(s), "boolean"),
                Some(Object::Bool(false)),
                "{s}"
            );
        }
        assert_eq!(
            convert_to_primitive_kind(&Object::string("也许"), "boolean"),
            None
        );
    }

    #[test]
    fn 同类型转换直接返回原值() {
        assert_eq!(
            convert_to_primitive_kind(&Object::Int(5), "integer"),
            Some(Object::Int(5))
        );
    }

    #[test]
    fn 命名类型转换包装身份() {
        let nt = NamedType::new("状态", DeclType::named("整数"));
        assert_eq!(
            convert_to_named(&nt, &Object::int(1)),
            Ok(Object::named("状态", Object::int(1)))
        );
        assert!(convert_to_named(&nt, &Object::string("x")).is_err());
    }

    #[test]
    fn 联合命名类型转换按成员匹配() {
        let nt = NamedType::new(
            "编号",
            DeclType::union(&["整数".to_string(), "字符串".to_string()]),
        );
        assert_eq!(
            convert_to_named(&nt, &Object::int(7)),
            Ok(Object::named("编号", Object::int(7)))
        );
        assert_eq!(
            convert_to_named(&nt, &Object::string("甲")),
            Ok(Object::named("编号", Object::string("甲")))
        );
        assert!(convert_to_named(&nt, &Object::bool_of(true)).is_err());
    }

    #[test]
    fn 值类型判定基础类型() {
        let env = Environment::new();
        assert!(value_is_type(&Object::int(1), "整数", &env));
        assert!(!value_is_type(&Object::int(1), "字符串", &env));
        assert!(value_is_type(&Object::nil(), "空", &env));
        assert!(value_is_type(&Object::nil(), "错误", &env));
        assert!(!value_is_type(&Object::nil(), "整数", &env));
    }

    #[test]
    fn 命名类型身份判定() {
        let env = Environment::new();
        env.runtime()
            .borrow_mut()
            .register_named_type(NamedType::new("状态", DeclType::named("整数")));
        assert!(value_is_type(
            &Object::named("状态", Object::int(1)),
            "状态",
            &env
        ));
        assert!(!value_is_type(&Object::int(1), "状态", &env));
    }

    #[test]
    fn 模型与嵌入的类型判定() {
        let env = Environment::new();
        let mut dog = Class::new("狗");
        dog.embeds.push(Rc::new(Class::new("动物")));
        let dog = Rc::new(dog);
        let inst = Object::Instance(new_zero_instance(&dog));
        assert!(value_is_type(&inst, "狗", &env));
        assert!(value_is_type(&inst, "动物", &env));
        assert!(!value_is_type(&inst, "猫", &env));
    }

    // ---------------------------------------------------------- 端到端

    const 矩形模型: &str = "定义 模型 矩形
    属性 整数 宽
    属性 整数 高
结束
";

    #[test]
    fn 定义模型并构造零值实例() {
        check(&format!("{矩形模型}变量 甲 = 矩形{{}}\n甲.宽"), "空");
    }

    #[test]
    fn 初始化方法设置字段() {
        check(
            &format!(
                "{矩形模型}定义 矩形 的 方法 初始化(整数 宽, 整数 高)
    自己.宽 = 宽
    自己.高 = 高
结束
变量 甲 = 矩形(3, 4)
甲.宽"
            ),
            "3",
        );
    }

    #[test]
    fn 模型方法调用可访问自己() {
        check(
            &format!(
                "{矩形模型}定义 矩形 的 方法 初始化(整数 宽, 整数 高)
    自己.宽 = 宽
    自己.高 = 高
结束
定义 矩形 的 方法 面积() -> 整数
    返回 自己.宽 乘 自己.高
结束
变量 甲 = 矩形(3, 4)
甲.面积()"
            ),
            "12",
        );
    }

    #[test]
    fn 无初始化方法时不接受参数() {
        assert!(run(&format!("{矩形模型}矩形(1)"))
            .unwrap_err()
            .contains("没有 初始化 方法"));
    }

    #[test]
    fn 访问不存在的成员报错() {
        assert!(run(&format!("{矩形模型}变量 乙 = 矩形{{}}\n乙.不存在"))
            .unwrap_err()
            .contains("没有字段或方法"));
    }

    #[test]
    fn 嵌入字段提升读写() {
        check(
            "定义 模型 动物
    属性 字符串 名
结束
定义 模型 狗
    嵌入 动物
    属性 整数 年龄
结束
变量 甲 = 狗{}
甲.名 = \"旺财\"
甲.名",
            "旺财",
        );
    }

    #[test]
    fn 嵌入名可取子对象() {
        check(
            "定义 模型 动物
    属性 字符串 名
结束
定义 模型 狗
    嵌入 动物
结束
变量 甲 = 狗{}
甲.动物.名 = \"小黑\"
甲.动物.名",
            "小黑",
        );
    }

    #[test]
    fn 嵌入模型未定义时报错() {
        assert!(run("定义 模型 狗\n    嵌入 不存在\n结束")
            .unwrap_err()
            .contains("未定义"));
    }

    #[test]
    fn 是运算符判断基础类型() {
        check("1 是 整数", "真");
        check("1 是 字符串", "假");
        check("\"甲\" 是 字符串", "真");
    }

    #[test]
    fn 是运算符判断模型与嵌入() {
        check(
            "定义 模型 动物
    属性 字符串 名
结束
定义 模型 狗
    嵌入 动物
结束
变量 甲 = 狗{}
甲 是 狗",
            "真",
        );
    }

    #[test]
    fn 转为运算符成功时返回值与真() {
        check("变量 结果、成功 = 3.7 转为 整数\n结果", "3");
        check("变量 结果、成功 = 3.7 转为 整数\n成功", "真");
    }

    #[test]
    fn 转为运算符失败时返回空与假() {
        check("变量 结果、成功 = \"文本\" 转为 整数\n成功", "假");
        check("变量 结果、成功 = \"文本\" 转为 整数\n结果", "空");
    }

    #[test]
    fn 转为字符串使用显示文本() {
        check("变量 结果、成功 = 42 转为 字符串\n结果", "42");
    }

    #[test]
    fn 自定义类型的转为包装身份() {
        check(
            "定义 类型 状态 = 整数\n变量 结果、成功 = 1 转为 状态\n成功",
            "真",
        );
    }

    #[test]
    fn 接口结构化满足() {
        check(
            "接口 说话者
    方法 说话() -> 字符串
结束
定义 模型 狗
结束
定义 狗 的 方法 说话() -> 字符串
    返回 \"汪\"
结束
变量 甲 = 狗{}
甲 是 说话者",
            "真",
        );
    }

    #[test]
    fn 接口未满足时判定为假() {
        check(
            "接口 说话者
    方法 说话()
结束
定义 模型 石头
    属性 整数 重量
结束
变量 甲 = 石头{}
甲 是 说话者",
            "假",
        );
    }

    #[test]
    fn 模型可按接口类型传入函数() {
        check(
            "接口 说话者
    方法 说话()
结束
定义 模型 狗
结束
定义 狗 的 方法 说话() -> 字符串
    返回 \"汪\"
结束
函数 让说话(说话者 甲)
    返回 甲.说话()
结束
让说话(狗{})",
            "汪",
        );
    }

    #[test]
    fn 嵌入接口方法被展平() {
        check(
            "接口 命名者
    方法 取名() -> 字符串
结束
接口 说话者
    嵌入 命名者
    方法 说话() -> 字符串
结束
定义 模型 鹦鹉
结束
定义 鹦鹉 的 方法 取名() -> 字符串
    返回 \"小绿\"
结束
定义 鹦鹉 的 方法 说话() -> 字符串
    返回 \"你好\"
结束
变量 甲 = 鹦鹉{}
甲 是 说话者",
            "真",
        );
    }

    #[test]
    fn 展平后只实现部分嵌入方法则不满足() {
        check(
            "接口 命名者
    方法 取名() -> 字符串
结束
接口 说话者
    嵌入 命名者
    方法 说话() -> 字符串
结束
定义 模型 哑巴
结束
定义 哑巴 的 方法 说话() -> 字符串
    返回 \"嗯\"
结束
变量 甲 = 哑巴{}
甲 是 说话者",
            "假",
        );
    }

    #[test]
    fn 接口循环嵌入报错() {
        // 先定义 乙（嵌入稍后定义的 丙 会先报未定义），因此用自环与互环两种合法顺序：
        // 互环：甲 先定义并嵌入 乙；乙 后定义且嵌入 甲（甲此时已注册）。
        let err = run(
            "接口 甲
    嵌入 乙
结束
接口 乙
    嵌入 甲
结束",
        )
        .unwrap_err();
        // 甲 在前向引用 乙：要求嵌入接口先定义，故报未定义而非环。
        assert!(
            err.contains("循环嵌入") || err.contains("未定义的接口"),
            "实际错误：{err}"
        );

        // 自环：接口嵌入自身，定义时自身已注册，应明确报循环嵌入。
        let err_self = run(
            "接口 甲
    嵌入 甲
结束",
        )
        .unwrap_err();
        assert!(err_self.contains("循环嵌入"), "实际错误：{err_self}");
    }

    #[test]
    fn 嵌入未定义接口报错() {
        let err = run(
            "接口 甲
    嵌入 不存在
结束",
        )
        .unwrap_err();
        assert!(err.contains("未定义的接口"), "实际错误：{err}");
    }

    #[test]
    fn 嵌入同名方法签名冲突报错() {
        let err = run(
            "接口 甲
    方法 处理(字符串 内容) -> 字符串
结束
接口 乙
    嵌入 甲
    方法 处理(整数 内容) -> 字符串
结束",
        )
        .unwrap_err();
        assert!(err.contains("签名冲突"), "实际错误：{err}");
    }

    #[test]
    fn 嵌入同名方法签名一致时合并成功() {
        check(
            "接口 甲
    方法 处理(字符串 内容) -> 字符串
结束
接口 乙
    嵌入 甲
结束
定义 模型 实现者
结束
定义 实现者 的 方法 处理(字符串 内容) -> 字符串
    返回 内容
结束
实现者() 是 乙",
            "真",
        );
    }
}
