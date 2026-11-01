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
    let path = resolve_library("regex").expect("找不到正则扩展库");
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
    let source = reg.ensure(&mut rt, "regex").expect("应加载扩展库");
    assert_eq!(source, ExtSource::Library);
    assert_eq!(reg.source_of("regex"), Some(ExtSource::Library));
}

#[test]
fn 扩展库注册的内建可用() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "regex").unwrap();

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
    reg.ensure(&mut rt, "regex").unwrap();

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
fn 能定位到json扩展库文件() {
    expose_library();
    let path = resolve_library("json").expect("找不到 JSON 扩展库");
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("libfan_ext_json"),
        "实际：{}",
        path.display()
    );
}

#[test]
fn json有扩展库时走扩展库() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    assert_eq!(reg.ensure(&mut rt, "json").unwrap(), ExtSource::Library);
    assert_eq!(reg.source_of("json"), Some(ExtSource::Library));
    assert!(rt.lookup_native("jsonEncode").is_some());
    assert!(rt.lookup_native("jsonDecode").is_some());
}

#[test]
fn json扩展库与默认实现编码一致() {
    // 扩展库会替换回落实现，两者必须给出逐字相同的文本，
    // 否则部署扩展库就会静默改变程序输出
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "json").unwrap();

    let mut dict = fan_object::FanDict::new();
    dict.set(Object::string("名"), Object::string("凡"));
    dict.set(Object::string("版本"), Object::int(1));

    let cases = vec![
        Object::int(42),
        Object::Float(1.0),
        Object::Float(2.5),
        Object::string("甲\"乙"),
        Object::Bool(true),
        Object::Nil,
        Object::array(vec![Object::int(1), Object::string("二")]),
        Object::Dict(dict),
    ];
    for value in cases {
        let expected = fan_native::ext::default::json::encode(&value);
        let builtin = rt.lookup_native("jsonEncode").unwrap();
        let out = (builtin.func)(&mut rt, vec![value.clone()]).unwrap();
        assert_eq!(
            out[0].as_str().as_deref(),
            expected.as_deref().ok(),
            "值 {}",
            value.inspect()
        );
    }
}

#[test]
fn json扩展库与默认实现解码一致() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "json").unwrap();

    for text in [
        r#"{"名":"凡","项":[1,2]}"#,
        "[1,2.5,\"x\",true,null]",
        "42",
        "{bad",
    ] {
        let expected = fan_native::ext::default::json::decode(text);
        let builtin = rt.lookup_native("jsonDecode").unwrap();
        let out = (builtin.func)(&mut rt, vec![Object::string(text)]).unwrap();
        match expected {
            Ok(value) => assert_eq!(out[0], value, "文本 {}", text),
            // 失败时错误文案不同，但都必须是「失败」
            Err(_) => assert!(out[0] == Object::Nil, "文本 {} 本应解析失败", text),
        }
    }
}

#[test]
fn 网络有扩展库时走扩展库() {
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    assert_eq!(reg.ensure(&mut rt, "http").unwrap(), ExtSource::Library);
    assert_eq!(reg.source_of("http"), Some(ExtSource::Library));
    assert!(rt.lookup_native("httpRequest").is_some());
}

#[test]
fn 网络扩展库与默认实现的失败路径一致() {
    // 连不上的地址：两者都要给出 success 为假的字典 + 错误值，
    // 凡语言层才能用同一套 `如果 !结果["success"]` 判断。
    // 标准库 网络_测试.凡 走的正是这条路径
    expose_library();
    let mut rt = Runtime::new();
    let mut reg = ExtRegistry::new();
    reg.ensure(&mut rt, "http").unwrap();

    let builtin = rt.lookup_native("httpRequest").unwrap();
    let out = (builtin.func)(
        &mut rt,
        vec![Object::string("GET"), Object::string("http://127.0.0.1:1/")],
    )
    .unwrap();
    let result = match &out[0] {
        Object::Dict(d) => d.clone(),
        other => panic!("应返回结果字典，实际 {}", other.kind()),
    };
    assert_eq!(
        result.get(&Object::string("success")),
        Some(Object::Bool(false))
    );
    assert!(matches!(out[1], Object::Error(_)), "第二项应是错误值");
}
