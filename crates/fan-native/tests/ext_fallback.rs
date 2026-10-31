//! 端到端：找不到扩展库时回落到内置默认实现。
//!
//! 与 `ext_library.rs` 分属两个测试可执行文件，避免它们对
//! `FAN_EXT_DIR` 环境变量的相反要求相互干扰。
//!
//! 注意：扩展库会被构建到 `target/debug/deps`，而那里正是测试可执行文件
//! 所在目录，属于**预期的部署位置**，因此无法在本进程内把 `正则` 的库
//! “藏起来”。回落路径改用确实没有库的模块（如 `json`）来验证。

use fan_native::ext::default;
use fan_native::ext::{resolve_library, ExtRegistry, ExtSource};
use fan_object::{Object, Runtime};

/// 让扩展库**搜不到**（清除环境变量）。
fn hide_library() {
    unsafe { std::env::remove_var("FAN_EXT_DIR") };
}

#[test]
fn 未部署扩展库时定位不到() {
    hide_library();
    // json 没有对应的扩展库 crate，故任何搜索目录里都不该有
    assert!(resolve_library("json").is_none(), "不该找到 json 扩展库");
    assert!(
        resolve_library("绝不存在的模块").is_none(),
        "不该找到不存在的扩展库"
    );
}

#[test]
fn 无扩展库时回落到默认实现() {
    hide_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    let source = reg.ensure(&mut rt, "json").expect("应回落到默认实现");
    assert_eq!(source, ExtSource::Default);
    assert_eq!(reg.source_of("json"), Some(ExtSource::Default));
}

#[test]
fn 默认实现注册的内建可用() {
    hide_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "json").unwrap();

    let builtin = rt.lookup_native("jsonEncode").expect("jsonEncode 未注册");
    let out = (builtin.func)(&mut rt, vec![Object::string("凡")]).unwrap();
    assert_eq!(out[0].inspect(), "\"凡\"");
}

#[test]
fn 正则的默认实现同样可用() {
    // 直接走回落路径登记，验证无库时 正则 也有可用实现
    let mut rt = Runtime::new();
    default::register(&mut rt, "正则").unwrap();

    let builtin = rt.lookup_native("regexMatch").expect("regexMatch 未注册");
    let out = (builtin.func)(
        &mut rt,
        vec![Object::string(r"\d+"), Object::string("a123b")],
    )
    .unwrap();
    assert_eq!(out[0].inspect(), "真");
}

#[test]
fn 既无扩展库也无默认实现的模块报错() {
    hide_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    let err = reg.ensure(&mut rt, "不存在的模块").unwrap_err();
    assert!(err.contains("既没有扩展库"), "实际：{}", err);
}
