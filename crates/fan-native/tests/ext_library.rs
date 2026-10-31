//! 端到端：动态加载真实扩展库 `fan_ext_regex`。
//!
//! 与 `ext_fallback.rs` 分属两个测试可执行文件，避免它们对
//! `FAN_EXT_DIR` 环境变量的相反要求相互干扰。

use fan_native::ext::{resolve_library, ExtRegistry, ExtSource};
use fan_object::{Object, Runtime};

/// 扩展库所在目录，即 `target/<profile>`。
fn target_dir() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("取不到当前可执行文件路径");
    // <target>/<profile>/deps/<测试名>-<hash>
    exe.parent()
        .and_then(|p| p.parent())
        .expect("推导不出构建输出目录")
        .to_path_buf()
}

/// 让扩展库可被搜到。
fn expose_library() {
    unsafe { std::env::set_var("FAN_EXT_DIR", target_dir()) };
}

#[test]
fn 能定位到扩展库文件() {
    expose_library();
    let path = resolve_library("正则").expect("找不到正则扩展库");
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("libfan_ext_regex"),
        "实际：{}",
        path.display()
    );
}

#[test]
fn 有扩展库时走扩展库() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    let source = reg.ensure(&mut rt, "正则").expect("应加载扩展库");
    assert_eq!(source, ExtSource::Library);
    assert_eq!(reg.source_of("正则"), Some(ExtSource::Library));
}

#[test]
fn 扩展库注册的内建可用() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "正则").unwrap();

    let builtin = rt.lookup_native("regexMatch").expect("regexMatch 未注册");
    let out = (builtin.func)(
        &mut rt,
        vec![Object::string(r"\d+"), Object::string("a123b")],
    )
    .unwrap();
    assert_eq!(out[0].inspect(), "真");
    assert_eq!(out[1], Object::Nil);
}

#[test]
fn 扩展库与默认实现行为一致() {
    // 同一样式在两处实现下应给出相同结果，便于互相替换
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "正则").unwrap();

    let cases: Vec<(&str, &str, &str)> = vec![
        (r"\d+", "a12b34", "12"),
        (r"[a-z]+", "abc", "abc"),
        (r"猫|狗", "养了狗", "狗"),
    ];
    for (pattern, text, expected) in cases {
        let builtin = rt.lookup_native("regexFind").unwrap();
        let out =
            (builtin.func)(&mut rt, vec![Object::string(pattern), Object::string(text)]).unwrap();
        assert_eq!(
            out[0].inspect(),
            expected,
            "模式 {} 于文本 {}",
            pattern,
            text
        );
    }
}

#[test]
fn 按内建名也能触发库加载() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    assert_eq!(
        reg.ensure_for_native(&mut rt, "regexQuote").unwrap(),
        ExtSource::Library
    );
    assert!(rt.lookup_native("regexFindAllGroups").is_some());
}
