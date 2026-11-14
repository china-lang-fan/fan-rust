//! 标签扩展内建：提供 `注册标签处理` 的原生注册入口。
//!
//! 解释器本体保留标签分派框架（`fan-eval` 的 `process_function_tags`），而
//! 「把凡函数登记为标签处理器」这一注册动作由本模块的原生内建
//! `tagRegisterHandler` 完成：
//!
//! - 凡代码经 `fan-sdk/库/标签工具.凡` 的 `定义 本地("tag.tagRegisterHandler")`
//!   绑定到本内建；
//! - 本模块随解释器静态编入，不再有动态库与默认回落之分。
//!
//! 注意：真正的标签分派与凡函数回调必须在解释器（`fan-eval`）内完成——
//! native 层无法调用凡函数。本模块只负责把 `(标签名, 处理函数)` 写入
//! `Runtime::fan_tag_handlers`，求值器在函数声明期读取并回调。

use fan_object::{NativeSpec, Object};
use std::rc::Rc;

/// 本模块提供的模块名。
pub const MODULE: &str = "tag";

/// 注册内建名（与 `fan-sdk/库/标签工具.凡` 的本地绑定名一致）。
pub const REGISTER_NAME: &str = "tagRegisterHandler";

/// 本模块提供的内建描述符。
pub fn specs() -> Vec<NativeSpec> {
    vec![NativeSpec::ext(
        REGISTER_NAME,
        Some(2),
        MODULE,
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
    fn 内建名与模块名符合约定() {
        for spec in specs() {
            assert_eq!(spec.ext_module, Some(MODULE));
        }
        assert!(specs().iter().any(|s| s.name == REGISTER_NAME));
    }

    #[test]
    fn 注册标签处理器后可查表() {
        let mut rt = Runtime::new();
        let func = Rc::new(Function::new(
            "处理",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        ));
        let spec = specs().remove(0);
        (spec.func)(
            &mut rt,
            vec![Object::string("自定义"), Object::Func(func.clone())],
        )
        .unwrap();
        let found = rt.lookup_fan_tag_handler("自定义").expect("应已登记");
        assert!(Rc::ptr_eq(&found, &func));
    }

    #[test]
    fn 标签名非字符串报错() {
        let mut rt = Runtime::new();
        let func = Rc::new(Function::new(
            "处理",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        ));
        let spec = specs().remove(0);
        let err = (spec.func)(&mut rt, vec![Object::int(1), Object::Func(func)]).unwrap_err();
        assert!(err.contains("标签名必须是字符串"), "实际：{err}");
    }

    #[test]
    fn 处理器非函数报错() {
        let mut rt = Runtime::new();
        let spec = specs().remove(0);
        let err = (spec.func)(&mut rt, vec![Object::string("自定义"), Object::int(2)]).unwrap_err();
        assert!(err.contains("标签处理器必须是函数"), "实际：{err}");
    }
}
