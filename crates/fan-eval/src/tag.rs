//! 标签系统框架。
//!
//! 凡语言的标签写在声明之前，用于给函数或模型附加元信息。本模块提供统一的
//! 处理器框架：每种标签名对应一个 [`TagHandler`]，通过 [`TagRegistry`] 分发。
//! 新增标签只需实现 `TagHandler` 并注册，无需改动求值器。
//!
//! 旧的 `@内建("名字")` 标签已移除：原生函数改由
//! `定义 本地("模块.函数") 函数 ...` 声明绑定（见 `eval::Interpreter::bind_native`），
//! 因此默认注册表不再内置任何处理器。标签框架本身保留，供 `@测试`（testrun 在
//! AST 层静态识别）、切面等自定义标签使用；遇到未注册标签时静默忽略，由 LSP
//! 静态分析负责告警。
//!
//! 对应 Go 版 `evaluator/tag.go`。

use crate::error::FanError;
use crate::eval::Interpreter;
use fan_ast::TagExpr;
use fan_object::{Function, Object};
use std::collections::HashMap;
use std::rc::Rc;

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
#[derive(Clone, Default)]
pub struct TagRegistry {
    handlers: HashMap<String, Rc<dyn TagHandler>>,
}

impl TagRegistry {
    /// 创建空注册表。
    ///
    /// 原生函数已改由 `定义 本地(...)` 绑定，框架不再内置任何标签处理器；
    /// 名称保留以表达「解释器自带的注册表」这一语义。
    pub fn with_builtins() -> Self {
        TagRegistry {
            handlers: HashMap::new(),
        }
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
    ///
    /// 未注册的标签静默忽略（返回 `Ok(())`），不影响声明求值；对未知标签的
    /// 提示由 LSP 静态分析负责。
    pub fn dispatch(&self, ctx: &mut TagContext<'_>) -> Result<(), FanError> {
        let name = ctx.tag.type_name.clone();
        match self.handlers.get(&name) {
            Some(handler) => handler.apply(ctx),
            None => Ok(()),
        }
    }
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

    /// 调用带原生绑定（`定义 本地(...)`）的函数：转调原生实现。
    ///
    /// 方法形式会把接收者作为第一个实参传入。
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
    use fan_ast::Position;

    /// 构造一个带指定标签名的标签表达式。
    fn tag_named(type_name: &str) -> TagExpr {
        TagExpr {
            pos: Position::START,
            type_name: type_name.to_string(),
            positional: vec![],
            named: vec![],
        }
    }

    // ------------------------------------------------------ 框架单元测试

    #[test]
    fn 默认注册表为空() {
        let registry = TagRegistry::with_builtins();
        assert!(registry.names().is_empty());
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
        let mut interp = Interpreter::new();
        let mut func = Function::new("甲", vec![], Default::default(), None, Position::START);
        let tag = tag_named("标记");
        let mut ctx = TagContext {
            interp: &mut interp,
            func: &mut func,
            tag: &tag,
        };
        registry.dispatch(&mut ctx).unwrap();
        assert_eq!(func.name, "已标记_甲");
    }

    #[test]
    fn 未注册的标签名静默忽略() {
        let registry = TagRegistry::with_builtins();
        let mut interp = Interpreter::new();
        let mut func = Function::new("甲", vec![], Default::default(), None, Position::START);
        let tag = tag_named("不存在");
        let mut ctx = TagContext {
            interp: &mut interp,
            func: &mut func,
            tag: &tag,
        };
        // 不再报错：未注册标签静默忽略
        registry.dispatch(&mut ctx).unwrap();
        assert_eq!(func.name, "甲");
    }

    #[test]
    fn 解释器的标签注册表默认为空() {
        let interp = Interpreter::new();
        assert!(interp.tags.names().is_empty());
    }
}
