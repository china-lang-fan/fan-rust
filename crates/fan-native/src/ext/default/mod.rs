//! 扩展组内建的**默认实现**：纯 Rust 标准库，无扩展动态库时回落使用。
//!
//! 设计意图：标准库声明 `@内建模块("json")` 之后，
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

/// 已知模块名 → 默认实现的注册入口。
///
/// 模块名大小写不敏感，便于凡代码书写。
pub fn has_default(module: &str) -> bool {
    matches!(
        module.to_lowercase().as_str(),
        "json" | "正则" | "regex" | "网络" | "http"
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

/// 内建名 → 所属模块。用于 `@内建` 找不到实现时自动补加载。
pub fn module_of(native_name: &str) -> Option<&'static str> {
    match native_name {
        "jsonEncode" | "jsonDecode" => Some("json"),
        name if name.starts_with("regex") => Some("正则"),
        "httpRequest" => Some("网络"),
        _ => None,
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
    fn 按内建名自动补加载正则() {
        let mut rt = Runtime::new();
        let mut reg = crate::ext::ExtRegistry::new();
        // 具体来源取决于运行环境是否部署了 fan_ext_regex：有则「扩展库」，
        // 无则「默认实现」。这里只要求补加载成功且内建可用。
        let source = reg.ensure_for_native(&mut rt, "regexMatch").unwrap();
        assert!(matches!(
            source,
            crate::ext::ExtSource::Default | crate::ext::ExtSource::Library
        ));
        assert!(rt.lookup_native("regexFindAll").is_some());
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

    #[test]
    fn 内建名可反查模块() {
        assert_eq!(module_of("jsonEncode"), Some("json"));
        assert_eq!(module_of("jsonDecode"), Some("json"));
        assert_eq!(module_of("regexMatch"), Some("正则"));
        assert_eq!(module_of("regexQuote"), Some("正则"));
        assert_eq!(module_of("print"), None);
    }
}
