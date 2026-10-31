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
//! - 已有成熟实现且自行实现代价过高的（正则引擎、JSON 解析）
//!
//! **2. 内建函数分两组。**
//!
//! | 组 | 依赖 | 交付形态 | 加载时机 |
//! |---|---|---|---|
//! | [`core`] | 仅 Rust 标准库 | 编译进解释器 | 启动时注册，始终可用 |
//! | [`ext`] | 第三方 crate | 独立动态库 | `@内建模块` 标签触发，按需加载 |
//!
//! 未使用 `@内建模块` 的程序**不会**加载任何扩展依赖，因此解释器本体保持
//! 零第三方依赖。
//!
//! **3. 所有内建通过统一描述符注册。** 新增一个内建只需在对应组的
//! `specs()` 里追加一项，无需改动注册框架。

pub mod core;
pub mod ext;

pub use ext::ExtRegistry;
pub use fan_object::NativeImpl;

use fan_object::{Builtin, NativeSpec, Runtime};
use std::rc::Rc;

/// 把 `core` 组（纯 Rust 标准库）的全部内建注册到运行实例。
///
/// `ext` 组不在此注册——它们由 `@内建模块` 标签在首次使用时动态加载。
pub fn register_core(rt: &mut Runtime) {
    for spec in core::specs() {
        rt.register_native(Rc::new(Builtin::new(
            spec.name,
            spec.arity,
            spec.func.clone(),
        )));
    }
}

/// 列出 `core` 组全部内建的名字。
pub fn core_names() -> Vec<&'static str> {
    core::specs().iter().map(|s| s.name).collect()
}

/// 按分组列出内建名字，供诊断与文档生成使用。
pub fn specs_by_group() -> Vec<(NativeSpec, &'static str)> {
    core::specs().into_iter().map(|s| (s, "core")).collect()
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
    fn 注册后可按名字查到() {
        let rt = Runtime::new();
        let mut rt = rt;
        register_core(&mut rt);
        for name in core_names() {
            assert!(rt.lookup_native(name).is_some(), "内建 {} 未注册", name);
        }
    }

    #[test]
    fn 未注册的扩展内建查不到() {
        // ext 组默认不加载，验证解释器本体确实零扩展依赖
        let mut rt = Runtime::new();
        register_core(&mut rt);
        assert!(rt.lookup_native("regexMatch").is_none());
        assert!(rt.lookup_native("jsonDecode").is_none());
    }
}
