//! 凡语言**正则扩展库**：基于 `regex` crate，以动态库形式按需加载。
//!
//! 这是 core/ext 分工中 `ext` 侧的一个真实实现样例。解释器本体零第三方
//! 依赖，只有凡代码出现 `@内建模块("正则")` 或用到 `regex*` 内建时才会
//! `dlopen` 本库；找不到本库时，解释器回落到内置的纯标准库引擎。
//!
//! # 导出符号
//!
//! - `fan_ext_abi_version() -> u32`
//! - `fan_ext_register(rt: *mut Runtime)`
//!
//! 两者必须与 `fan-native` 的加载约定一致，ABI 版本取自 `fan-object` 中的
//! 公共常量，不会两侧漂移。

use fan_object::{ErrorValue, NativeSpec, Object, Runtime};
use std::cell::RefCell;
use std::rc::Rc;

/// 本库提供的模块名。
pub const MODULE: &str = "正则";

/// 把编译错误转成凡语言错误值。
fn error_of(message: impl Into<String>) -> Object {
    Object::Error(Rc::new(RefCell::new(ErrorValue::new(message.into()))))
}

/// 统一的返回形式：成功 `(值, 空)`，失败 `(空值, 错误)`。
fn result_of(value: Result<Object, String>, empty: Object) -> Vec<Object> {
    match value {
        Ok(v) => vec![v, Object::Nil],
        Err(e) => vec![empty, error_of(e)],
    }
}

/// 取出字符串参数。
fn text_of(args: &[Object], index: usize) -> String {
    args.get(index).and_then(|v| v.as_str()).unwrap_or_default()
}

/// 编译模式；失败信息带上模式本身便于定位。
fn compile(pattern: &str) -> Result<regex::Regex, String> {
    regex::Regex::new(pattern).map_err(|e| format!("正则模式 {} 无法编译：{}", pattern, e))
}

/// 把一次捕获展开为 [整体, 分组1, 分组2, …]。
fn capture_to_array(caps: &regex::Captures) -> Vec<Object> {
    caps.iter()
        .map(|m| Object::string(m.map(|m| m.as_str().to_string()).unwrap_or_default()))
        .collect()
}

/// 数量限制参数：`-1`（或缺失）表示不限，`0` 表示不取。
///
/// `正则.凡` 的 `替换(…, 数量 = 1)` 与 `全部替换(…)` 分别传 1 与 -1。
fn count_of(args: &[Object], index: usize) -> Option<usize> {
    match args.get(index).and_then(|v| v.as_int()) {
        Some(n) if n < 0 => None,
        Some(n) => Some(n as usize),
        None => None,
    }
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::ext(
            "regexMatch",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| Object::Bool(re.is_match(&text))),
                    Object::Bool(false),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexFind",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| {
                        Object::string(
                            re.find(&text)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                        )
                    }),
                    Object::string(String::new()),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexFindAll",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| {
                        let found = re
                            .find_iter(&text)
                            .map(|m| Object::string(m.as_str().to_string()));
                        Object::array(match count_of(&args, 2) {
                            Some(n) => found.take(n).collect(),
                            None => found.collect(),
                        })
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexFindGroups",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| match re.captures(&text) {
                        Some(caps) => Object::array(capture_to_array(&caps)),
                        None => Object::array(vec![]),
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexFindAllGroups",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| {
                        let found = re
                            .captures_iter(&text)
                            .map(|caps| Object::array(capture_to_array(&caps)));
                        Object::array(match count_of(&args, 2) {
                            Some(n) => found.take(n).collect(),
                            None => found.collect(),
                        })
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexReplace",
            Some(3),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                let template = text_of(&args, 2);
                let limit = count_of(&args, 3);
                Ok(result_of(
                    compile(&pattern).map(|re| {
                        // 数量为 -1 时全部替换，否则只替换前 n 处
                        let replaced = match limit {
                            Some(n) => re.replacen(&text, n, template.as_str()).into_owned(),
                            None => re.replace_all(&text, template.as_str()).into_owned(),
                        };
                        Object::string(replaced)
                    }),
                    Object::string(String::new()),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexSplit",
            Some(2),
            MODULE,
            Rc::new(|_rt, args| {
                let pattern = text_of(&args, 0);
                let text = text_of(&args, 1);
                Ok(result_of(
                    compile(&pattern).map(|re| {
                        // 数量为 0 得空数组；其余用 splitn（末段为剩余全部）
                        match count_of(&args, 2) {
                            Some(0) => Object::array(Vec::new()),
                            Some(n) => {
                                Object::array(re.splitn(&text, n).map(Object::string).collect())
                            }
                            None => Object::array(re.split(&text).map(Object::string).collect()),
                        }
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::ext(
            "regexQuote",
            Some(1),
            MODULE,
            Rc::new(|_rt, args| Ok(vec![Object::string(regex::escape(&text_of(&args, 0)))])),
        ),
    ]
}

/// ABI 版本，供解释器在加载前校验。
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

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {}", name));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    fn first(name: &str, args: Vec<Object>) -> String {
        call(name, args).unwrap()[0].inspect()
    }

    #[test]
    fn 匹配与查找() {
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string(r"\d+"), Object::string("a123b")]
            ),
            "真"
        );
        assert_eq!(
            first(
                "regexFind",
                vec![Object::string(r"\d+"), Object::string("a12b34")]
            ),
            "12"
        );
        assert_eq!(
            first(
                "regexFindAll",
                vec![Object::string(r"\d+"), Object::string("a12b34")]
            ),
            "[12, 34]"
        );
    }

    #[test]
    fn 分组与替换() {
        let out = first(
            "regexFindGroups",
            vec![Object::string(r"([a-z]+)(\d+)"), Object::string("abc123")],
        );
        assert_eq!(out, "[abc123, abc, 123]");
        assert_eq!(
            first(
                "regexReplace",
                vec![
                    Object::string(r"(\w+)"),
                    Object::string("a b"),
                    Object::string("[$1]"),
                ]
            ),
            "[a] [b]"
        );
    }

    #[test]
    fn 拆分与转义() {
        assert_eq!(
            first(
                "regexSplit",
                vec![Object::string("[,;]"), Object::string("a,b;c")]
            ),
            "[a, b, c]"
        );
        assert_eq!(first("regexQuote", vec![Object::string("a.b?")]), r"a\.b\?");
    }

    #[test]
    fn 非法模式返回错误而非中断() {
        let out = call(
            "regexFind",
            vec![Object::string("("), Object::string("abc")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "");
        assert!(
            out[1].inspect().contains("无法编译"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 扩展库声明的模块名正确() {
        for spec in specs() {
            assert_eq!(spec.ext_module, Some(MODULE));
            assert!(spec.name.starts_with("regex"));
        }
    }

    #[test]
    fn 导出的abi版本与解释器一致() {
        assert_eq!(fan_ext_abi_version(), fan_object::FAN_EXT_ABI_VERSION);
    }

    #[test]
    fn 通过c接口注册可用() {
        let mut rt = Runtime::new();
        unsafe { fan_ext_register(&mut rt as *mut Runtime) };
        for name in ["regexMatch", "regexFind", "regexReplace", "regexQuote"] {
            assert!(rt.lookup_native(name).is_some(), "内建 {} 未注册", name);
        }
    }
}
