//! 运行实例。
//!
//! `Runtime` 是一次独立凡程序运行所承载的全部可变状态：标准输出、进程上下文、
//! 调用栈、模型/接口/命名类型/原生函数注册表。不同 `Runtime` 之间完全隔离，
//! 可在同一进程中并发或重入地执行多个凡程序（与 Go 版 `Runtime` 的设计一致）。

use crate::kind::Kind;
use crate::runtime_entity::{Builtin, Class, Function, Interface, NamedType, NativeSpec};
use crate::{Environment, Object};
use fan_ast::Position;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

/// 扩展动态库的 ABI 版本。
///
/// 解释器与扩展库必须链接同一份 `fan-object`，否则 `Runtime` 的内存布局可能
/// 不一致。扩展库导出 `fan_ext_abi_version` 供加载前校验，常量放在这里是为
/// 了让两侧引用同一个定义，不会各自漂移。
pub const FAN_EXT_ABI_VERSION: u32 = 1;

/// 调用栈中的一帧（凡函数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFrame {
    pub name: Rc<str>,
    pub pos: Position,
}

/// 标准输出的可替换目标。测试时可替换为 `Vec<u8>` 以捕获输出。
pub type Output = Rc<RefCell<dyn Write>>;

/// 一个独立的凡程序运行实例。
pub struct Runtime {
    /// 标准输出；默认指向进程标准输出。
    pub stdout: Output,
    pub script_path: String,
    pub args: Vec<String>,
    /// 函数调用栈（最外层在前）。
    pub fn_stack: Vec<FnFrame>,

    /// 内建模型（字符串、数组等原生类型的扩展方法宿主）。
    pub builtin_classes: HashMap<String, Rc<Class>>,
    /// 接口名 → 接口定义。
    pub interfaces: HashMap<String, Rc<Interface>>,
    /// 命名类型名 → 类型身份。
    pub named_types: HashMap<String, NamedType>,
    /// 原生函数名 → 实现。
    pub native_functions: HashMap<String, Rc<Builtin>>,
    /// 为基本类型定义的方法：类型标识（"string"/"array"…）→ 方法名 → 实现。
    ///
    /// 凡代码用 `定义 字符串 的 方法 截取(…)` 声明，宿主类型本身没有类对象，
    /// 故单独建表。
    pub primitive_methods: HashMap<String, HashMap<String, Rc<Function>>>,
    /// 分段（mixfix）函数注册表：根段名 → 运行时签名 → 实现。
    ///
    /// 同一根段名下允许按参数类型重载，签名由调用实参的运行时类型重建，
    /// 与 Go 版 `mixfixFunctionRegistry` 一致。
    pub mixfix_functions: HashMap<String, HashMap<String, Rc<Function>>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("script_path", &self.script_path)
            .field("args", &self.args)
            .field("fn_stack", &self.fn_stack)
            .field("interfaces", &self.interfaces.keys().collect::<Vec<_>>())
            .field("named_types", &self.named_types.keys().collect::<Vec<_>>())
            .field(
                "native_functions",
                &self.native_functions.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            stdout: Rc::new(RefCell::new(std::io::stdout())),
            script_path: String::new(),
            args: Vec::new(),
            fn_stack: Vec::new(),
            builtin_classes: HashMap::new(),
            interfaces: HashMap::new(),
            named_types: HashMap::new(),
            native_functions: HashMap::new(),
            primitive_methods: HashMap::new(),
            mixfix_functions: HashMap::new(),
        }
    }

    /// 创建一个把输出写入指定缓冲区的运行实例（供测试使用）。
    pub fn with_output(sink: Output) -> Self {
        Runtime {
            stdout: sink,
            ..Runtime::new()
        }
    }

    /// 设置脚本路径与命令行参数。
    pub fn set_process_context(&mut self, script_path: impl Into<String>, args: Vec<String>) {
        self.script_path = script_path.into();
        self.args = args;
    }

    // ------------------------------------------------------------ 调用栈

    pub fn push_fn(&mut self, name: impl Into<String>, pos: Position) {
        self.fn_stack.push(FnFrame {
            name: Rc::from(name.into()),
            pos,
        });
    }

    pub fn pop_fn(&mut self) {
        self.fn_stack.pop();
    }

    /// 当前调用栈快照（最内层在前），用于错误追踪。
    pub fn stack(&self) -> Vec<crate::collection::StackFrame> {
        self.fn_stack
            .iter()
            .rev()
            .map(|f| crate::collection::StackFrame {
                function: f.name.to_string(),
                file: self.script_path.clone(),
                line: f.pos.line,
                column: f.pos.column,
            })
            .collect()
    }

    pub fn stack_depth(&self) -> usize {
        self.fn_stack.len()
    }

    // ------------------------------------------------------------ 注册表

    /// 注册或替换一个内建模型。
    pub fn register_builtin_class(&mut self, class: Rc<Class>) {
        self.builtin_classes.insert(class.name.clone(), class);
    }

    pub fn register_interface(&mut self, iface: Rc<Interface>) {
        self.interfaces.insert(iface.name.clone(), iface);
    }

    pub fn lookup_interface(&self, name: &str) -> Option<Rc<Interface>> {
        self.interfaces.get(name).cloned()
    }

    pub fn register_named_type(&mut self, nt: NamedType) {
        self.named_types.insert(nt.name.clone(), nt);
    }

    pub fn lookup_named_type(&self, name: &str) -> Option<&NamedType> {
        self.named_types.get(name)
    }

    pub fn register_native(&mut self, builtin: Rc<Builtin>) {
        self.native_functions.insert(builtin.name.clone(), builtin);
    }

    /// 按描述符登记内建。解释器与扩展库都走这里，保证登记方式一致。
    pub fn register_spec(&mut self, spec: &NativeSpec) {
        self.register_native(Rc::new(Builtin::new(
            spec.name,
            spec.arity,
            spec.func.clone(),
        )));
    }

    /// 批量登记一组描述符。
    pub fn register_specs(&mut self, specs: &[NativeSpec]) {
        for spec in specs {
            self.register_spec(spec);
        }
    }

    pub fn lookup_native(&self, name: &str) -> Option<Rc<Builtin>> {
        self.native_functions.get(name).cloned()
    }

    /// 登记一个为基本类型定义的方法。`type_name` 为类型标识，如 `"string"`。
    pub fn register_primitive_method(&mut self, type_name: &str, method: &str, func: Rc<Function>) {
        self.primitive_methods
            .entry(type_name.to_string())
            .or_default()
            .insert(method.to_string(), func);
    }

    /// 查找为基本类型定义的方法。
    pub fn lookup_primitive_method(&self, type_name: &str, method: &str) -> Option<Rc<Function>> {
        self.primitive_methods
            .get(type_name)
            .and_then(|m| m.get(method))
            .cloned()
    }

    /// 列出某基本类型已定义的方法名（按名排序）。
    pub fn primitive_method_names(&self, type_name: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .primitive_methods
            .get(type_name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// 登记一个分段函数。`root` 为根段名，`signature` 为声明期类型签名。
    /// 同一签名重复登记时报错（与 Go 版“重复的分段函数签名”一致）。
    pub fn register_mixfix(
        &mut self,
        root: &str,
        signature: &str,
        func: Rc<Function>,
    ) -> Result<(), String> {
        let table = self.mixfix_functions.entry(root.to_string()).or_default();
        if table.contains_key(signature) {
            return Err(format!("重复的分段函数签名：{signature}"));
        }
        table.insert(signature.to_string(), func);
        Ok(())
    }

    /// 判断根段名是否登记过分段函数。
    pub fn has_mixfix_root(&self, root: &str) -> bool {
        self.mixfix_functions.contains_key(root)
    }

    /// 按根段名与运行时签名查找分段函数。
    pub fn lookup_mixfix(&self, root: &str, signature: &str) -> Option<Rc<Function>> {
        self.mixfix_functions
            .get(root)?
            .get(signature)
            .cloned()
    }

    /// 在指定环境内查找模型（内建模型优先，其次是环境中的模型值）。
    pub fn resolve_class(&self, name: &str, env: &Environment) -> Option<Rc<Class>> {
        if let Some(cls) = self.builtin_classes.get(name) {
            return Some(cls.clone());
        }
        match env.get(name) {
            Some(Object::Class(c)) => Some(c),
            _ => None,
        }
    }

    // ------------------------------------------------------------ 输出

    /// 打印若干值。单参数为数组时展开其元素（与 Go 版 `printObjects` 一致）。
    pub fn print_objects(&mut self, args: &[Object]) {
        // 打印的是**一个数组**：展开元素，之间加空格，便于阅读。
        // 打印的是**多个实参**：原样逐个拼接，不插分隔——要不要空格由调用
        // 方自己写进实参，语言不替调用方决定。
        let out = match args {
            [Object::Array(a)] => a
                .elements()
                .iter()
                .map(Object::inspect)
                .collect::<Vec<_>>()
                .join(" "),
            _ => {
                let mut out = String::new();
                for v in args {
                    out.push_str(&v.inspect());
                }
                out
            }
        };
        let sink = self.stdout.clone();
        let _ = sink.borrow_mut().write_all(out.as_bytes());
        let _ = sink.borrow_mut().flush();
    }

    /// 打印并换行。
    pub fn print_line(&mut self, args: &[Object]) {
        self.print_objects(args);
        let sink = self.stdout.clone();
        let _ = sink.borrow_mut().write_all(b"\n");
    }
}

/// 判断某个种类名是否为内置基础类型名。
pub fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "整数" | "小数" | "字符串" | "布尔" | "数组" | "字典" | "错误" | "字节" | "字符" | "空"
    )
}

/// 把种类转为对应的内置类型名（非内置返回 `None`）。
pub fn kind_type_name(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Int => "整数",
        Kind::Float => "小数",
        Kind::Str => "字符串",
        Kind::Bool => "布尔",
        Kind::Array => "数组",
        Kind::Dict => "字典",
        Kind::Error => "错误",
        Kind::Byte => "字节",
        Kind::Char => "字符",
        Kind::Nil => "空",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::FanArray;

    /// 构造一个把输出写入共享缓冲区的运行实例。
    fn runtime_with_buffer() -> (Runtime, Rc<RefCell<Vec<u8>>>) {
        let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let rt = Runtime::with_output(buf.clone());
        (rt, buf)
    }

    #[test]
    fn 新建运行实例状态为空() {
        let rt = Runtime::new();
        assert!(rt.args.is_empty());
        assert_eq!(rt.stack_depth(), 0);
        assert!(rt.interfaces.is_empty());
        assert!(rt.named_types.is_empty());
        assert!(rt.native_functions.is_empty());
        assert!(rt.script_path.is_empty());
    }

    #[test]
    fn 进程上下文设置与替换() {
        let mut rt = Runtime::new();
        rt.set_process_context("a.凡", vec!["x".to_string(), "y".to_string()]);
        assert_eq!(rt.script_path, "a.凡");
        assert_eq!(rt.args, vec!["x", "y"]);
    }

    #[test]
    fn 调用栈后进先出() {
        let mut rt = Runtime::new();
        rt.push_fn("甲", Position::new(1, 1));
        rt.push_fn("乙", Position::new(2, 5));
        assert_eq!(rt.stack_depth(), 2);
        rt.pop_fn();
        assert_eq!(rt.stack_depth(), 1);
        assert_eq!(rt.fn_stack[0].name.to_string(), "甲");
    }

    #[test]
    fn 栈快照最内层在前() {
        let mut rt = Runtime::new();
        rt.set_process_context("测试.凡", vec![]);
        rt.push_fn("外层", Position::new(1, 1));
        rt.push_fn("内层", Position::new(9, 3));
        let stack = rt.stack();
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0].function, "内层");
        assert_eq!(stack[0].file, "测试.凡");
        assert_eq!(stack[0].line, 9);
        assert_eq!(stack[1].function, "外层");
    }

    #[test]
    fn 打印多个实参不加分隔() {
        // 按参数打印：结果之间不插空格，要不要分隔由调用方自己写进实参
        let (mut rt, buf) = runtime_with_buffer();
        rt.print_objects(&[Object::string("你好"), Object::int(1)]);
        assert_eq!(String::from_utf8(buf.borrow().clone()).unwrap(), "你好1");
    }

    #[test]
    fn 打印单个数组时元素间加空格() {
        // 打印的是数组本身：展开元素，之间加空格
        let (mut rt, buf) = runtime_with_buffer();
        rt.print_objects(&[Object::Array(FanArray::new(vec![
            Object::int(1),
            Object::int(2),
        ]))]);
        assert_eq!(String::from_utf8(buf.borrow().clone()).unwrap(), "1 2");
    }

    #[test]
    fn 数组作实参之一时仍按原样显示() {
        // 不是「打印一个数组」，而是若干实参里的一个：保留 [1, 2] 的形式
        let (mut rt, buf) = runtime_with_buffer();
        rt.print_objects(&[
            Object::string("甲"),
            Object::Array(FanArray::new(vec![Object::int(1), Object::int(2)])),
        ]);
        assert_eq!(String::from_utf8(buf.borrow().clone()).unwrap(), "甲[1, 2]");
    }

    #[test]
    fn 打印换行为两行() {
        let (mut rt, buf) = runtime_with_buffer();
        rt.print_line(&[Object::int(1)]);
        rt.print_line(&[Object::int(2)]);
        assert_eq!(String::from_utf8(buf.borrow().clone()).unwrap(), "1\n2\n");
    }

    #[test]
    fn 模型注册与查找() {
        let mut rt = Runtime::new();
        rt.register_builtin_class(Rc::new(Class::new("字符串")));
        assert!(rt.builtin_classes.contains_key("字符串"));
        assert!(rt.builtin_classes.contains_key("字符串"));
    }

    #[test]
    fn 接口注册与查找() {
        let mut rt = Runtime::new();
        rt.register_interface(Rc::new(Interface::new("说话者")));
        assert!(rt.lookup_interface("说话者").is_some());
        assert!(rt.lookup_interface("不存在").is_none());
    }

    #[test]
    fn 命名类型注册与查找() {
        let mut rt = Runtime::new();
        rt.register_named_type(NamedType::new("状态", fan_ast::DeclType::named("整数")));
        assert!(rt.lookup_named_type("状态").is_some());
        assert!(rt.lookup_named_type("不存在").is_none());
    }

    #[test]
    fn 原生函数注册与查找() {
        let mut rt = Runtime::new();
        let b = Rc::new(Builtin::new(
            "平方根",
            Some(1),
            Rc::new(|_rt, _args| Ok(vec![Object::int(1)])),
        ));
        rt.register_native(b);
        assert!(rt.lookup_native("平方根").is_some());
        assert!(rt.lookup_native("不存在").is_none());
    }

    #[test]
    fn 模型解析优先内建再环境() {
        let mut rt = Runtime::new();
        rt.register_builtin_class(Rc::new(Class::new("字符串")));
        let env = Environment::new();
        assert!(rt.resolve_class("字符串", &env).is_some());
        assert!(rt.resolve_class("不存在", &env).is_none());
    }

    #[test]
    fn 内置类型名判定() {
        for name in [
            "整数",
            "小数",
            "字符串",
            "布尔",
            "数组",
            "字典",
            "错误",
            "字节",
            "字符",
            "空",
        ] {
            assert!(is_builtin_type_name(name), "{name} 应为内置类型");
        }
        assert!(!is_builtin_type_name("矩形"));
    }

    #[test]
    fn 种类到类型名映射() {
        assert_eq!(kind_type_name(Kind::Int), Some("整数"));
        assert_eq!(kind_type_name(Kind::Nil), Some("空"));
        assert_eq!(kind_type_name(Kind::Func), None);
        assert_eq!(kind_type_name(Kind::Instance), None);
    }

    #[test]
    fn 调试输出不含实现细节() {
        let rt = Runtime::new();
        let s = format!("{rt:?}");
        assert!(s.contains("Runtime"));
        assert!(s.contains("script_path"));
    }
}
