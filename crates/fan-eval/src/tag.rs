//! 标签系统框架与凡语言标签处理器分派。
//!
//! 凡语言的标签写在声明之前，用于给函数或模型附加元信息。本模块提供统一的
//! 处理器框架：每种标签名对应一个 [`TagHandler`]，通过 [`TagRegistry`] 分发。
//! 新增 Rust 侧标签只需实现 `TagHandler` 并注册，无需改动求值器。
//!
//! 此外，凡语言代码可用 `注册标签处理(标签名, 处理函数)`（见 `fan-sdk/库/标签工具.凡`，
//! 由静态编入的原生内建 `tagRegisterHandler` 写入 `Runtime::fan_tag_handlers`）登记一个**凡语言处理器**。
//! 函数声明期遇到带处理器的标签时，求值器构造标签上下文并回调该凡函数，
//! 处理器可通过 `上下文.替换实现(新实现)` 改写目标函数。对应 Go 版
//! `evaluator/tag.go` 的 `processTags`/`registerFanTagHandler`。
//!
//! 旧的 `@内建("名字")` 标签已移除：原生函数改由
//! `定义 本地("模块.函数") 函数 ...` 声明绑定（见 `eval::Interpreter::bind_native`）。
//! 遇到未注册标签时静默忽略，由 LSP 静态分析负责告警。

use crate::error::FanError;
use crate::eval::Interpreter;
use crate::oop::new_zero_instance;
use fan_ast::TagExpr;
use fan_object::{Class, Function, Instance, Object, TagContextValue};
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
    ///
    /// 分派顺序：先查 Rust 侧 [`TagRegistry`]（默认空），再查 `Runtime` 中的
    /// 凡语言处理器（`注册标签处理` 登记）。两者都没有命中的标签静默忽略。
    pub fn process_function_tags(&mut self, func: &mut Function) -> Result<(), FanError> {
        if func.tags.is_empty() {
            return Ok(());
        }
        let tags = func.tags.clone();
        let registry = self.tags.clone();
        for tag in &tags {
            // Rust 侧处理器优先。
            if self.has_rust_handler(tag) {
                let mut ctx = TagContext {
                    interp: self,
                    func,
                    tag,
                };
                registry.dispatch(&mut ctx)?;
                continue;
            }
            // 凡语言处理器：在 Runtime 表中按标签名查找。
            self.apply_fan_tag_handler(func, tag)?;
        }
        Ok(())
    }

    fn has_rust_handler(&self, tag: &TagExpr) -> bool {
        self.tags.names().contains(&tag.type_name.as_str())
    }

    /// 构造标签模型实例；解析不到模型时返回 `空`（未注册标签不阻断求值）。
    fn build_tag_instance(&mut self, tag: &TagExpr) -> Result<Object, FanError> {
        let class = {
            let rt = self.env.borrow().runtime();
            let env = self.env.clone();
            let found = rt.borrow().resolve_class(&tag.type_name, &env.borrow());
            found
        };
        let Some(class) = class else {
            return Ok(Object::Nil);
        };
        if tag.positional.len() > 1 {
            return Err(FanError::at("标签未命名参数最多一个", tag.pos));
        }
        let inst = new_zero_instance(&class);
        if let Some(first) = tag.positional.first() {
            let v = self.eval_expr(first)?.into_value()?;
            self.write_tag_field(&inst, &class, "值", v, tag.pos)?;
        }
        for arg in &tag.named {
            let v = self.eval_expr(&arg.value)?.into_value()?;
            self.write_tag_field(&inst, &class, &arg.name, v, tag.pos)?;
        }
        Ok(Object::Instance(inst))
    }

    /// 写入标签实例字段；模型缺该字段时报错（与 Go 版 createTagInstance 一致）。
    fn write_tag_field(
        &mut self,
        inst: &Rc<Instance>,
        class: &Rc<Class>,
        name: &str,
        val: Object,
        pos: fan_ast::Position,
    ) -> Result<(), FanError> {
        inst.set_field_value(name, val)
            .map_err(|_| FanError::at(format!("标签模型 {} 没有字段 {}", class.name, name), pos))
    }

    /// 查 Runtime 中的凡语言标签处理器并回调，应用其替换实现。
    fn apply_fan_tag_handler(
        &mut self,
        func: &mut Function,
        tag: &TagExpr,
    ) -> Result<(), FanError> {
        let handler = {
            let rt = self.env.borrow().runtime();
            let found = rt.borrow().lookup_fan_tag_handler(&tag.type_name);
            found
        };
        let Some(handler) = handler else {
            // 未注册标签：静默忽略。
            return Ok(());
        };
        let tag_instance = self.build_tag_instance(tag)?;
        // 当前实现即被标注函数自身（尚未替换前的可调用对象）。
        let current = Object::Func(Rc::new(func.clone()));
        let ctx_value = Rc::new(TagContextValue::new(tag_instance, current));
        let context = Object::TagContext(ctx_value.clone());

        // 回调凡语言处理器：处理函数(上下文)。
        let callee = Object::Func(handler);
        self.apply_function(&callee, vec![context], tag.pos)?;

        // 处理器通过 上下文.替换实现(...) 写入的新实现。
        if let Some(replacement) = ctx_value.replacement() {
            func.tag_replacement = Some(replacement);
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
