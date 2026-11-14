//! `ext` 组内建：依赖第三方 crate（正则引擎、JSON、HTTP 客户端）的能力。
//!
//! 与 `core` 组的区别只在**依赖来源**，不在加载方式——两组都在解释器启动时
//! 静态注册进同一张原生函数表，没有动态库、没有 FFI 边界、没有运行时加载与
//! 「找不到库就回落」的弱实现。凡代码仍以 `定义 本地("模块.函数") 函数 ...`
//! 绑定，模块段（如 `正则`、`网络`）经 [`normalize_module`] 归一后用于静态诊断。
//!
//! | 模块 | 实现支撑 | 内建前缀 |
//! |---|---|---|
//! | [`regex`] | `regex` crate | `regex*` |
//! | [`json`] | `serde_json` | `json*` |
//! | [`http`] | `ureq`（含 TLS） | `http*` |
//! | [`tag`] | 纯标准库 | `tag*` |

pub mod http;
pub mod json;
pub mod regex;
pub mod tag;

use fan_object::NativeSpec;
use std::collections::{HashMap, HashSet};

/// 收集 `ext` 组全部内建描述符，启动时与 core 一起注册。
pub fn ext_specs() -> Vec<NativeSpec> {
    let mut all = Vec::new();
    all.extend(regex::specs());
    all.extend(json::specs());
    all.extend(http::specs());
    all.extend(tag::specs());
    all
}

/// 把源码里的模块名归一到 ASCII 目录键（小写；中文别名映射）。
///
/// 与绑定字符串里的模块段对应：`正则` → `regex`、`网络` → `http`，
/// 其余 ASCII 名转小写自映射（`json`、`tag`）。
pub fn normalize_module(module: &str) -> String {
    match module.to_lowercase().as_str() {
        "正则" => "regex".to_string(),
        "网络" => "http".to_string(),
        other => other.to_string(),
    }
}

/// 静态 native 目录：归一模块名 → 该模块提供的内建名集合。
///
/// 全部来自编译期内建描述符，不触碰文件系统，供 `fan-analysis` 对
/// `定义 本地("模块.函数")` 做「模块/函数是否存在」的静态诊断。
pub fn ext_catalog() -> HashMap<String, HashSet<&'static str>> {
    let mut catalog = HashMap::new();
    for (key, specs) in [
        ("regex", regex::specs()),
        ("json", json::specs()),
        ("http", http::specs()),
        ("tag", tag::specs()),
    ] {
        catalog.insert(
            key.to_string(),
            specs.iter().map(|s| s.name).collect::<HashSet<_>>(),
        );
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext组内建均标记为ext来源() {
        for spec in ext_specs() {
            assert_eq!(
                spec.group,
                fan_object::NativeGroup::Ext,
                "{} 分组错误",
                spec.name
            );
            assert!(spec.ext_module.is_some(), "{} 缺少模块标记", spec.name);
        }
    }

    #[test]
    fn 内建名在模块内不重复() {
        let catalog = ext_catalog();
        let mut seen = HashSet::new();
        for specs in [regex::specs(), json::specs(), http::specs(), tag::specs()] {
            for spec in specs {
                assert!(seen.insert(spec.name), "内建名 {} 重复", spec.name);
            }
        }
        assert_eq!(catalog.len(), 4);
    }

    #[test]
    fn 中文模块名归一到ascii键() {
        assert_eq!(normalize_module("正则"), "regex");
        assert_eq!(normalize_module("网络"), "http");
        assert_eq!(normalize_module("JSON"), "json");
        assert_eq!(normalize_module("tag"), "tag");
        assert_eq!(normalize_module("自定义"), "自定义");
    }

    #[test]
    fn 目录包含各模块的关键内建() {
        let catalog = ext_catalog();
        assert!(catalog["regex"].contains("regexMatch"));
        assert!(catalog["json"].contains("jsonEncode"));
        assert!(catalog["http"].contains("httpRequest"));
        assert!(catalog["tag"].contains("tagRegisterHandler"));
    }
}
