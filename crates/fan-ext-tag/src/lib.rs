//! 凡语言**标签扩展库**：以动态库形式提供 `注册标签处理` 的注册入口。
//!
//! 这是 core/ext 分工中 `ext` 侧的实现。解释器本体保留标签分派框架
//! （`fan-eval` 的 `process_function_tags`），而「把凡函数登记为标签处理器」
//! 这一注册动作通过本扩展库的原生内建 `tagRegisterHandler` 完成：
//!
//! - 凡代码经 `fan-sdk/库/标签工具.凡` 的 `定义 本地("tag.tagRegisterHandler")`
//!   引用本库，触发 `dlopen`；
//! - 找不到本库时，解释器回落到 `fan-native/ext/default/tag.rs` 的等价默认实现，
//!   因此标签能力在未部署扩展库时依然可用。
//!
//! 注意：真正的标签分派与凡函数回调必须在解释器（`fan-eval`）内完成——
//! object/native 层无法调用凡函数。本库只负责把 `(标签名, 处理函数)` 写入
//! `Runtime::fan_tag_handlers`，求值器在函数声明期读取并回调。
//!
//! # 导出符号
//!
//! - `fan_ext_abi_version() -> u32`
//! - `fan_ext_register(rt: *mut Runtime)`

use fan_object::{NativeSpec, Object, Runtime};
use std::rc::Rc;

/// 本库提供的模块名。
pub const MODULE: &str = "tag";

/// 注册内建名（与 `fan-sdk/库/标签工具.凡` 的本地绑定名一致）。
pub const REGISTER_NAME: &str = "tagRegisterHandler";

/// 本库提供的内建描述符。
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

#[no_mangle]
pub extern "C" fn fan_ext_abi_version() -> u32 {
    fan_object::FAN_EXT_ABI_VERSION
}

/// 向运行实例注册本库的全部内建。
///
/// # Safety
///
/// `rt` 必须是解释器传入的有效 `Runtime` 指针；由加载方保证。
#[no_mangle]
pub unsafe extern "C" fn fan_ext_register(rt: *mut Runtime) {
    let rt = &mut *rt;
    rt.register_specs(&specs());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::Function;

    #[test]
    fn 内建名与模块名符合加载约定() {
        for spec in specs() {
            assert_eq!(spec.ext_module, Some(MODULE));
        }
        assert!(specs().iter().any(|s| s.name == REGISTER_NAME));
    }

    #[test]
    fn 导出的abi版本与解释器一致() {
        assert_eq!(fan_ext_abi_version(), fan_object::FAN_EXT_ABI_VERSION);
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

    #[test]
    fn 通过c接口注册可用() {
        let mut rt = Runtime::new();
        unsafe { fan_ext_register(&mut rt as *mut Runtime) };
        assert!(rt.lookup_native(REGISTER_NAME).is_some());
    }
}
