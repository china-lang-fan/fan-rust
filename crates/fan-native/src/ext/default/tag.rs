//! 标签扩展模块的**默认实现**：无 `fan_ext_tag` 动态库时回落使用。
//!
//! 与动态库实现行为一致——把 `(标签名, 凡函数)` 写入
//! `Runtime::fan_tag_handlers`，真正的分派由 `fan-eval` 在函数声明期完成。
//! 放在默认实现里保证：不部署任何扩展动态库，`注册标签处理` 依然可用。

use fan_object::{NativeSpec, Object};
use std::rc::Rc;

/// 注册内建名（与扩展库、`fan-sdk/库/标签工具.凡` 一致）。
pub const REGISTER_NAME: &str = "tagRegisterHandler";

/// 默认实现的内建描述符。
pub fn specs() -> Vec<NativeSpec> {
    vec![NativeSpec::core(
        REGISTER_NAME,
        Some(2),
        Rc::new(|rt, args| {
            if args.len() != 2 {
                return Err("注册标签处理 需要 2 个参数".to_string());
            }
            let tag_name = args[0]
                .as_str()
                .ok_or_else(|| "标签名必须是字符串".to_string())?;
            let Object::Func(handler) = &args[1] else {
                return Err("标签处理器必须是函数".to_string());
            };
            rt.register_fan_tag_handler(&tag_name, handler.clone());
            Ok(vec![Object::Nil])
        }),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::{Function, Runtime};

    #[test]
    fn 默认实现可登记标签处理器() {
        let mut rt = Runtime::new();
        let spec = specs().remove(0);
        let func = Rc::new(Function::new(
            "处理",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        ));
        (spec.func)(
            &mut rt,
            vec![Object::string("自定义"), Object::Func(func.clone())],
        )
        .unwrap();
        let found = rt.lookup_fan_tag_handler("自定义").expect("应已登记");
        assert!(Rc::ptr_eq(&found, &func));
    }
}
