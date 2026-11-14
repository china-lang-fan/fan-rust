//! 凡语言原生函数库（内建函数）。
//!
//! # 设计原则
//!
//! **1. 能用凡语言实现的，一律用凡语言实现。**
//! 标准库（`fan-sdk/*.凡`）中，`字典.凡`、`字符串.凡`、`数组.凡` 全部是纯凡
//! 语言实现；`数学.凡` 也只用两个内建（`平方根`、`幂`），其余 `绝对值`、
//! `向下取整`、`四舍五入`、`总和`、`平均值` 等全部用凡语言写成。
//! 内建函数只提供凡语言**无法表达**的原子能力：
//!
//! - 宿主环境交互（文件系统、进程、时间、网络）
//! - 运行时反射（取类型名）
//! - 解释器内部能力（输出、构造带调用栈的错误）
//! - 已有成熟实现且自行实现代价过高的（正则引擎、JSON 解析、HTTP 客户端）
//!
//! **2. 内建函数分两组，但都静态编入解释器。**
//!
//! | 组 | 依赖 | 注册时机 |
//! |---|---|---|
//! | [`core`] | 仅 Rust 标准库 | 启动时注册，始终可用 |
//! | [`ext`] | 第三方 crate（`regex`/`serde_json`/`ureq`） | 同样在启动时注册 |
//!
//! 两组没有加载方式差别，更没有 FFI 边界与动态库：`ext` 只是「依赖第三方
//! crate 的内建」的来源标记。凡代码统一以 `定义 本地("名字")`（core 裸名）
//! 或 `定义 本地("模块.函数")`（ext 模块前缀）绑定，运行时查同一张原生函数表。
//!
//! **3. 所有内建通过统一描述符注册。** 新增一个内建只需在对应组的
//! `specs()` 里追加一项，无需改动注册框架。

pub mod core;
pub mod ext;

pub use fan_object::NativeImpl;

use fan_object::{Builtin, NativeSpec, Runtime};
use std::rc::Rc;

/// 把一组描述符登记为内建。
fn register_specs(rt: &mut Runtime, specs: &[NativeSpec]) {
    for spec in specs {
        rt.register_native(Rc::new(Builtin::new(
            spec.name,
            spec.arity,
            spec.func.clone(),
        )));
    }
}

/// 把全部内建（`core` + `ext`）注册到运行实例。
///
/// 解释器启动时调用一次；之后凡代码的 `定义 本地(...)` 仅在这张已注册的
/// 原生函数表里查表，查不到则静默回退执行凡函数体。
pub fn register_natives(rt: &mut Runtime) {
    register_specs(rt, &core::specs());
    register_specs(rt, &ext::ext_specs());
}

/// 列出 `core` 组全部内建的名字。
pub fn core_names() -> Vec<&'static str> {
    core::specs().iter().map(|s| s.name).collect()
}

/// 按分组列出内建名字，供诊断与文档生成使用。
pub fn specs_by_group() -> Vec<(NativeSpec, &'static str)> {
    core::specs()
        .into_iter()
        .map(|s| (s, "core"))
        .chain(ext::ext_specs().into_iter().map(|s| (s, "ext")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core组内建名字互不重复() {
        let mut names = core_names();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "存在重名的内建函数");
    }

    #[test]
    fn 注册后core与ext内建都可按名字查到() {
        let mut rt = Runtime::new();
        register_natives(&mut rt);
        // core 裸名
        for name in core_names() {
            assert!(
                rt.lookup_native(name).is_some(),
                "core 内建 {} 未注册",
                name
            );
        }
        // ext 模块前缀名
        for name in [
            "regexMatch",
            "jsonEncode",
            "jsonDecode",
            "httpRequest",
            "tagRegisterHandler",
        ] {
            assert!(rt.lookup_native(name).is_some(), "ext 内建 {} 未注册", name);
        }
    }

    #[test]
    fn 全部内建名字互不重复() {
        let mut names: Vec<&str> = specs_by_group().iter().map(|(s, _)| s.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "core 与 ext 之间存在重名内建");
    }
}
