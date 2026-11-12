//! 扩展组内建的**默认实现**：纯 Rust 标准库，无扩展动态库时回落使用。
//!
//! 设计意图：凡代码用 `定义 本地("json.jsonEncode") 函数 ...` 引用某模块的原生函数后，
//!
//! - 存在 `fan_ext_json` 动态库 → 使用其中由 `serde_json` 等第三方库驱动的实现；
//! - 不存在 → 自动回落到此处的纯标准库实现，功能可用但能力较弱。
//!
//! 因此**分发形态不影响可用性**：不安装任何扩展，标准库依然能跑。

use fan_object::{Builtin, NativeSpec, Runtime};
use std::rc::Rc;

pub mod http;
pub mod json;
pub mod regex;
pub mod tag;

/// 已知模块名 → 默认实现的注册入口。
///
/// 模块名大小写不敏感，便于凡代码书写。
pub fn has_default(module: &str) -> bool {
    matches!(
        module.to_lowercase().as_str(),
        "json" | "正则" | "regex" | "网络" | "http" | "标签" | "tag"
    )
}

/// 把指定模块的默认实现注册进运行实例。
pub fn register(rt: &mut Runtime, module: &str) -> Result<(), String> {
    match module.to_lowercase().as_str() {
        "json" => {
            register_all(rt, json::specs());
            Ok(())
        }
        "正则" | "regex" => {
            register_all(rt, regex::specs());
            Ok(())
        }
        "网络" | "http" => {
            register_all(rt, http::specs());
            Ok(())
        }
        "标签" | "tag" => {
            register_all(rt, tag::specs());
            Ok(())
        }
        other => Err(format!("模块 {} 既没有扩展库，也没有默认实现", other)),
    }
}

/// 把一组描述符登记为内建。
fn register_all(rt: &mut Runtime, specs: Vec<NativeSpec>) {
    for spec in specs {
        rt.register_native(Rc::new(Builtin::new(
            spec.name,
            spec.arity,
            spec.func.clone(),
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 已知模块具备默认实现() {
        assert!(has_default("json"));
        assert!(has_default("JSON"));
        assert!(has_default("正则"));
        assert!(has_default("regex"));
        assert!(!has_default("不存在的模块"));
    }

    #[test]
    fn 注册正则默认实现后内建可用() {
        let mut rt = Runtime::new();
        register(&mut rt, "正则").unwrap();
        for name in ["regexMatch", "regexFind", "regexReplace", "regexQuote"] {
            assert!(rt.lookup_native(name).is_some(), "内建 {} 未注册", name);
        }
    }

    #[test]
    fn 注册后内建可查到() {
        let mut rt = Runtime::new();
        register(&mut rt, "json").unwrap();
        assert!(rt.lookup_native("jsonEncode").is_some());
        assert!(rt.lookup_native("jsonDecode").is_some());
    }

    #[test]
    fn 未知模块无默认实现() {
        let mut rt = Runtime::new();
        assert!(register(&mut rt, "不存在")
            .unwrap_err()
            .contains("也没有默认实现"));
    }
}
