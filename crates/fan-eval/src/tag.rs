//! 标签系统框架。
//!
//! 凡语言的标签写在声明之前，用于给函数或模型附加元信息。本模块提供统一的
//! 处理器框架：每种标签名对应一个 [`TagHandler`]，通过 [`TagRegistry`] 分发。
//! 新增标签只需实现 `TagHandler` 并注册，无需改动求值器。
//!
//! 当前内置一种标签：
//!
//! | 标签 | 作用 |
//! |---|---|
//! | `@内建("名字")` | 把函数绑定到原生实现，名字可带模块前缀，见 [`BuiltinTag`] |
//!
//! 对应 Go 版 `evaluator/tag.go`。Go 版把标签建模为「标签模型 + 处理器 +
//! 可替换调用目标」的通用机制，而凡语言标准库实际只用这一种用途，因此这里
//! 保留同样的扩展点，但只落地需要的两个处理器。

use crate::error::FanError;
use crate::eval::Interpreter;
use fan_ast::{Expr, TagExpr};
use fan_object::{Function, Object};
use std::collections::HashMap;
use std::rc::Rc;

/// `@内建` 标签名。
pub const 内建标签: &str = "内建";

/// 标签应用的上下文：解释器、被标注的函数、标签本身。
pub struct TagContext<'a> {
    pub interp: &'a mut Interpreter,
    pub func: &'a mut Function,
    pub tag: &'a TagExpr,
}

/// 标签处理器。每种标签名对应一个实现。
pub trait TagHandler {
    /// 该处理器响应的标签名。
    fn name(&self) -> &str;

    /// 应用标签。返回错误即中断当前声明的求值。
    fn apply(&self, ctx: &mut TagContext<'_>) -> Result<(), FanError>;
}

/// 标签注册表：标签名 → 处理器。
#[derive(Clone)]
pub struct TagRegistry {
    handlers: HashMap<String, Rc<dyn TagHandler>>,
}

impl Default for TagRegistry {
    fn default() -> Self {
        TagRegistry::with_builtins()
    }
}

impl TagRegistry {
    /// 创建只含解释器内置标签的注册表。
    pub fn with_builtins() -> Self {
        let mut registry = TagRegistry {
            handlers: HashMap::new(),
        };
        registry.register(Rc::new(BuiltinTag));
        registry
    }

    /// 注册一个标签处理器；同名会被覆盖。
    pub fn register(&mut self, handler: Rc<dyn TagHandler>) {
        self.handlers.insert(handler.name().to_string(), handler);
    }

    /// 已注册的标签名。
    pub fn names(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    /// 按标签名分派。
    pub fn dispatch(&self, ctx: &mut TagContext<'_>) -> Result<(), FanError> {
        let name = ctx.tag.type_name.clone();
        match self.handlers.get(&name) {
            Some(handler) => handler.apply(ctx),
            None => Err(FanError::at(
                format!("未定义标签处理器：{}", name),
                ctx.tag.pos,
            )),
        }
    }
}

/// `@内建("名字")`：把函数绑定到 `core` 组的原生实现。
///
/// 绑定后调用该函数将转调原生实现，函数体不再执行。
struct BuiltinTag;

impl TagHandler for BuiltinTag {
    fn name(&self) -> &str {
        内建标签
    }

    fn apply(&self, ctx: &mut TagContext<'_>) -> Result<(), FanError> {
        let spec = string_arg(ctx.tag)?;
        let pos = ctx.tag.pos;
        // 名字可写成 `模块名.函数名`：模块名用于加载对应的扩展库（找不到
        // 扩展库时回落到内置默认实现），函数名指定其中的原生函数。
        // 不写模块名时，取系统已注册的默认函数。
        let (module, native_name) = match spec.split_once('.') {
            Some((m, f)) if !m.is_empty() && !f.is_empty() => (Some(m.to_string()), f.to_string()),
            _ => (None, spec.clone()),
        };

        let rt = ctx.interp.env.borrow().runtime();
        if let Some(module) = &module {
            let loaded = {
                let mut rt_mut = rt.borrow_mut();
                ctx.interp.ext.borrow_mut().ensure(&mut rt_mut, module)
            };
            loaded.map_err(|e| FanError::at(e, pos))?;
        }

        let found = rt.borrow().lookup_native(&native_name);
        let builtin = match found {
            Some(b) => b,
            // 未写模块名：按函数名反查所属模块并加载，兼容只写函数名的写法
            None if module.is_none() => {
                let source = {
                    let mut rt_mut = rt.borrow_mut();
                    ctx.interp
                        .ext
                        .borrow_mut()
                        .ensure_for_native(&mut rt_mut, &native_name)
                };
                if source.is_err() {
                    return Err(FanError::at(
                        format!("标签 @{} 引用的原生函数 {} 未注册", 内建标签, native_name),
                        pos,
                    ));
                }
                rt.borrow().lookup_native(&native_name).ok_or_else(|| {
                    FanError::at(
                        format!("标签 @{} 引用的原生函数 {} 未注册", 内建标签, native_name),
                        pos,
                    )
                })?
            }
            None => {
                return Err(FanError::at(
                    format!(
                        "模块 {} 中没有原生函数 {}",
                        module.unwrap_or_default(),
                        native_name
                    ),
                    pos,
                ))
            }
        };
        ctx.func.native_impl = Some(builtin);
        Ok(())
    }
}

/// 取标签的第一个字符串位置参数。
pub fn string_arg(tag: &TagExpr) -> Result<String, FanError> {
    if let Some(Expr::Str(s)) = tag.positional.first() {
        return Ok(s.value.to_string());
    }
    if let Some(Expr::InterpStr(s)) = tag.positional.first() {
        // 插值串退化为连接其字面分段
        return Ok(s.parts.concat());
    }
    Err(FanError::at(
        format!("@{} 需要一个字符串参数", tag.type_name),
        tag.pos,
    ))
}

impl Interpreter {
    /// 处理函数声明上的全部标签，按书写顺序依次分派。
    pub fn process_function_tags(&mut self, func: &mut Function) -> Result<(), FanError> {
        if func.tags.is_empty() {
            return Ok(());
        }
        // 先把标签取出，避免分派期间对 func 的重复借用
        let tags = func.tags.clone();
        // 取出注册表副本分派，避免与 `&mut self` 的借用冲突
        let registry = self.tags.clone();
        for tag in &tags {
            let mut ctx = TagContext {
                interp: self,
                func,
                tag,
            };
            registry.dispatch(&mut ctx)?;
        }
        Ok(())
    }

    /// 调用带 `@内建` 绑定的函数：转调原生实现。
    ///
    /// 方法形式的内建会把接收者作为第一个实参传入。
    pub(crate) fn call_native_impl(
        &mut self,
        func: &Function,
        args: Vec<Object>,
        pos: fan_ast::Position,
    ) -> crate::error::EvalResult {
        use crate::error::value;
        let Some(builtin) = func.native_impl.clone() else {
            return Err(FanError::at("函数没有原生实现", pos));
        };
        let mut call_args = args;
        if let Some(receiver) = &func.self_value {
            call_args.insert(0, receiver.clone());
        }
        let rt = self.env.borrow().runtime();
        let out = {
            let mut rt_mut = rt.borrow_mut();
            (builtin.func)(&mut rt_mut, call_args)
        };
        let out = out.map_err(|e| FanError::at(e, pos))?;
        value(crate::eval::single_or_tuple(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::Interpreter;
    use fan_object::{Builtin, Runtime};
    use fan_parser::parse_program;

    /// 注册一个把整数加倍的原生函数 `double`，模拟 core 组内建。
    fn interp_with_native() -> Interpreter {
        let interp = Interpreter::new();
        let rt = interp.env.borrow().runtime();
        rt.borrow_mut().register_native(Rc::new(Builtin::new(
            "double",
            Some(1),
            Rc::new(|_rt, args| {
                let n = args.first().and_then(|a| a.as_int()).unwrap_or(0);
                Ok(vec![Object::int(n * 2)])
            }),
        )));
        interp
    }

    fn check(src: &str, expected: &str) {
        let (program, errors) = parse_program(src);
        assert!(errors.is_empty(), "解析错误：{errors}");
        let mut interp = interp_with_native();
        match interp.run(&program) {
            Ok(v) => assert_eq!(v.inspect(), expected, "源码：{src}"),
            Err(e) => panic!("执行失败：{}\n源码：{src}", e.display()),
        }
    }

    // ------------------------------------------------------ 框架单元测试

    #[test]
    fn 注册表默认含内置标签() {
        let registry = TagRegistry::with_builtins();
        let mut names = registry.names();
        names.sort_unstable();
        assert_eq!(names, vec![内建标签]);
    }

    #[test]
    fn 可注册自定义标签处理器() {
        struct 标记标签;
        impl TagHandler for 标记标签 {
            fn name(&self) -> &str {
                "标记"
            }
            fn apply(&self, ctx: &mut TagContext<'_>) -> Result<(), FanError> {
                ctx.func.name = format!("已标记_{}", ctx.func.name);
                Ok(())
            }
        }

        let mut registry = TagRegistry::with_builtins();
        registry.register(Rc::new(标记标签));
        assert!(registry.names().contains(&"标记"));

        // 验证自定义处理器确实生效
        let mut interp = interp_with_native();
        let mut func = Function::new(
            "甲",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        );
        let tag = TagExpr {
            pos: fan_ast::Position::START,
            type_name: "标记".to_string(),
            positional: vec![],
            named: vec![],
        };
        let mut ctx = TagContext {
            interp: &mut interp,
            func: &mut func,
            tag: &tag,
        };
        registry.dispatch(&mut ctx).unwrap();
        assert_eq!(func.name, "已标记_甲");
    }

    #[test]
    fn 未注册的标签名报错() {
        let registry = TagRegistry::with_builtins();
        let mut interp = interp_with_native();
        let mut func = Function::new(
            "甲",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        );
        let tag = TagExpr {
            pos: fan_ast::Position::START,
            type_name: "不存在".to_string(),
            positional: vec![],
            named: vec![],
        };
        let mut ctx = TagContext {
            interp: &mut interp,
            func: &mut func,
            tag: &tag,
        };
        assert!(registry
            .dispatch(&mut ctx)
            .unwrap_err()
            .display()
            .contains("未定义标签处理器"));
    }

    #[test]
    fn 字符串参数解析支持字面量() {
        let tag = TagExpr {
            pos: fan_ast::Position::START,
            type_name: 内建标签.to_string(),
            positional: vec![Expr::Str(fan_ast::StringLit {
                pos: fan_ast::Position::START,
                value: "double".to_string(),
            })],
            named: vec![],
        };
        assert_eq!(string_arg(&tag).unwrap(), "double");
    }

    #[test]
    fn 缺少字符串参数报错() {
        let tag = TagExpr {
            pos: fan_ast::Position::START,
            type_name: 内建标签.to_string(),
            positional: vec![Expr::Integer(fan_ast::IntegerLit {
                pos: fan_ast::Position::START,
                value: 42,
            })],
            named: vec![],
        };
        assert!(string_arg(&tag)
            .unwrap_err()
            .display()
            .contains("需要一个字符串参数"));
    }

    // ------------------------------------------------------ 端到端

    #[test]
    fn 内建绑定后不执行函数体() {
        check(
            "@内建(\"double\")\n函数 加倍(数) -> 整数\n    返回 0\n结束\n加倍(5)",
            "10",
        );
    }

    #[test]
    fn 未注册的原生名报错() {
        let (program, _) = parse_program("@内建(\"nope\")\n函数 加倍(数)\n    返回 0\n结束\n");
        let mut interp = interp_with_native();
        assert!(interp
            .run(&program)
            .unwrap_err()
            .display()
            .contains("未注册"));
    }

    // ------------------------------------------------------ @内建 的模块名

    #[test]
    fn 内建标签可写模块名前缀() {
        // `模块名.函数名`：模块名用于加载扩展库，没有库时回落内置默认实现
        check(
            "@内建(\"json.jsonEncode\")\n函数 生成(值)\n    返回 空\n结束\n生成([1, 2])",
            "([1,2], 空)",
        );
    }

    #[test]
    fn 内建标签不写模块名时取默认实现() {
        // 不写模块名：沿用按函数名反查所属模块的兼容路径
        check(
            "@内建(\"jsonEncode\")\n函数 生成(值)\n    返回 空\n结束\n生成([1, 2])",
            "([1,2], 空)",
        );
    }

    #[test]
    fn 内建标签的模块名不存在时报错() {
        let (program, _) =
            parse_program("@内建(\"不存在的模块.某某\")\n函数 甲()\n    返回 0\n结束\n");
        let mut interp = interp_with_native();
        let err = interp.run(&program).unwrap_err().display();
        assert!(err.contains("既没有扩展库"), "实际：{}", err);
    }

    #[test]
    fn 内建标签的函数名不在模块中时报错() {
        let (program, _) = parse_program("@内建(\"json.不存在\")\n函数 甲()\n    返回 0\n结束\n");
        let mut interp = interp_with_native();
        let err = interp.run(&program).unwrap_err().display();
        assert!(err.contains("没有原生函数"), "实际：{}", err);
    }

    #[test]
    fn 未知标签名报错() {
        let (program, _) = parse_program("@自定义\n函数 加倍(数)\n    返回 0\n结束\n");
        let mut interp = interp_with_native();
        assert!(interp
            .run(&program)
            .unwrap_err()
            .display()
            .contains("未定义标签处理器"));
    }

    #[test]
    fn 无标签的函数照常执行函数体() {
        check(
            "函数 加倍(数) -> 整数\n    返回 数 乘 2\n结束\n加倍(4)",
            "8",
        );
    }

    #[test]
    fn 解释器的标签注册表可用() {
        let interp = Interpreter::new();
        assert!(interp.tags.names().contains(&内建标签));
    }

    #[test]
    fn 原生实现可由运行实例共享() {
        let rt = Rc::new(std::cell::RefCell::new(Runtime::new()));
        rt.borrow_mut().register_native(Rc::new(Builtin::new(
            "triple",
            Some(1),
            Rc::new(|_rt, args| {
                let n = args.first().and_then(|a| a.as_int()).unwrap_or(0);
                Ok(vec![Object::int(n * 3)])
            }),
        )));
        assert!(rt.borrow().lookup_native("triple").is_some());
        assert!(rt.borrow().lookup_native("missing").is_none());
    }
}
