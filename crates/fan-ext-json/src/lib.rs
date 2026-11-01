//! 凡语言**JSON 扩展库**：基于 `serde_json`，以动态库形式按需加载。
//!
//! 这是 core/ext 分工中 `ext` 侧的实现。解释器本体零第三方依赖，只有凡代码
//! 用到 `jsonEncode`/`jsonDecode`（见 `fan-sdk/编码.凡`）时才会 `dlopen` 本库；
//! 找不到本库时，解释器回落到内置的纯标准库实现。
//!
//! # 与回落实现的契约
//!
//! 扩展库加载后会**替换**回落实现，故对外行为必须与之一致，否则切换实现
//! 就会改变程序语义：
//!
//! - `jsonEncode(值)` → `(文本, 错误)`；失败时文本为空串
//! - `jsonDecode(文本)` → `(值, 错误)`；失败时值为 空
//! - 整数一律解码为 整数 而非浮点——`编码_测试.凡` 断言 `值["items"][1]` 等于 2
//!
//! 唯一有意的差异：非有限浮点（`NaN`/`±Inf`）在回落实现里会写出 `NaN` 这类
//! 非法 JSON 文本，本库改为明确报错。
//!
//! # 导出符号
//!
//! - `fan_ext_abi_version() -> u32`
//! - `fan_ext_register(rt: *mut Runtime)`
//!
//! 两者必须与 `fan-native` 的加载约定一致，ABI 版本取自 `fan-object` 中的
//! 公共常量，不会两侧漂移。

use fan_object::{FanDict, NativeSpec, Object, Runtime};
use std::rc::Rc;

/// 本库提供的模块名。
pub const MODULE: &str = "json";

/// 统一的返回形式：成功 `(值, 空)`，失败 `(空值, 错误文本)`。
///
/// 失败用**字符串**而非错误对象，是为了与回落实现逐字一致——切换实现
/// 不应改变凡语言层可观测的行为。
fn result_of(value: Result<Object, String>, empty: Object) -> Vec<Object> {
    match value {
        Ok(v) => vec![v, Object::Nil],
        Err(e) => vec![empty, Object::string(e)],
    }
}

/// 凡语言对象 → JSON 值。
fn to_json(value: &Object) -> Result<serde_json::Value, String> {
    let out = match value {
        Object::Int(i) => serde_json::Value::from(*i),
        Object::Byte(b) => serde_json::Value::from(*b),
        Object::Float(f) => {
            // 走与回落实现同一个格式化函数，再按 JSON 数字重新解析：
            // 这样 1.0 写出「1」而不是「1.0」，切换实现不会改变程序输出
            let text = fan_ast::format_f64(*f);
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Number(n)) => serde_json::Value::Number(n),
                // format_f64 对 NaN / ±Inf 给出的是非法 JSON 文本，这里改为报错
                _ => return Err(format!("浮点数 {f} 不能序列化为 JSON")),
            }
        }
        Object::Str(s) => serde_json::Value::String(s.to_string()),
        Object::Char(c) => serde_json::Value::String(c.to_string()),
        Object::Bool(b) => serde_json::Value::Bool(*b),
        Object::Nil => serde_json::Value::Null,
        Object::Array(a) => {
            let mut items = Vec::new();
            for item in a.elements() {
                items.push(to_json(&item)?);
            }
            serde_json::Value::Array(items)
        }
        Object::Dict(d) => {
            let mut map = serde_json::Map::new();
            // 先取键的快照再取值：遍历期间不持有借用，`get` 才不会撞上 `RefCell`
            let keys: Vec<Object> = d.0.borrow().keys.clone();
            for key in keys {
                let key_text = match &key {
                    Object::Str(s) => s.to_string(),
                    other => other.inspect(),
                };
                map.insert(key_text, to_json(&d.get(&key).unwrap_or(Object::Nil))?);
            }
            serde_json::Value::Object(map)
        }
        other => return Err(format!("{} 不能序列化为 JSON", other.kind())),
    };
    Ok(out)
}

/// JSON 值 → 凡语言对象。
fn to_object(value: serde_json::Value) -> Object {
    match value {
        serde_json::Value::Null => Object::Nil,
        serde_json::Value::Bool(b) => Object::Bool(b),
        serde_json::Value::Number(n) => {
            // 整数保持为 整数：凡语言层按整数比较时不应退化成浮点
            if let Some(i) = n.as_i64() {
                Object::int(i)
            } else {
                Object::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Object::string(s),
        serde_json::Value::Array(items) => {
            Object::array(items.into_iter().map(to_object).collect())
        }
        serde_json::Value::Object(map) => {
            let mut dict = FanDict::new();
            for (key, value) in map {
                dict.set(Object::string(key), to_object(value));
            }
            Object::Dict(dict)
        }
    }
}

/// 把对象序列化为 JSON 文本。
pub fn encode(value: &Object) -> Result<String, String> {
    let json = to_json(value)?;
    serde_json::to_string(&json).map_err(|e| format!("JSON 序列化失败：{e}"))
}

/// 把 JSON 文本解析为对象。
pub fn decode(text: &str) -> Result<Object, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
    Ok(to_object(value))
}

/// 本库提供的内建。
pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::ext(
            "jsonEncode",
            Some(1),
            MODULE,
            Rc::new(|_rt, args| {
                Ok(result_of(
                    encode(&args[0]).map(Object::string),
                    Object::string(String::new()),
                ))
            }),
        ),
        NativeSpec::ext(
            "jsonDecode",
            Some(1),
            MODULE,
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                Ok(result_of(decode(&text), Object::Nil))
            }),
        ),
    ]
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

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn 编码基本类型() {
        assert_eq!(encode(&Object::int(42)).unwrap(), "42");
        assert_eq!(encode(&Object::Float(1.5)).unwrap(), "1.5");
        assert_eq!(encode(&Object::string("甲")).unwrap(), "\"甲\"");
        assert_eq!(encode(&Object::Bool(true)).unwrap(), "true");
        assert_eq!(encode(&Object::Nil).unwrap(), "null");
        assert_eq!(encode(&Object::Char('甲')).unwrap(), "\"甲\"");
        assert_eq!(encode(&Object::Byte(7)).unwrap(), "7");
    }

    #[test]
    fn 编码浮点与回落实现一致() {
        // 回落实现用 format_f64：1.0 写作「1」而不是「1.0」。
        // 部署本扩展库不应让程序输出发生变化，故逐字对齐
        assert_eq!(encode(&Object::Float(1.0)).unwrap(), "1");
        assert_eq!(encode(&Object::Float(0.0)).unwrap(), "0");
        assert_eq!(encode(&Object::Float(2.5)).unwrap(), "2.5");
    }

    #[test]
    fn 编码需要转义的字符() {
        assert_eq!(encode(&Object::string("a\"b")).unwrap(), "\"a\\\"b\"");
        assert_eq!(encode(&Object::string("a\nb")).unwrap(), "\"a\\nb\"");
        assert_eq!(encode(&Object::string("a\\b")).unwrap(), "\"a\\\\b\"");
        assert_eq!(encode(&Object::string("\u{1}")).unwrap(), "\"\\u0001\"");
    }

    #[test]
    fn 编码数组与字典且保持键序() {
        assert_eq!(
            encode(&Object::array(vec![Object::int(1), Object::int(2)])).unwrap(),
            "[1,2]"
        );
        let mut dict = FanDict::new();
        dict.set(Object::string("乙"), Object::int(2));
        dict.set(Object::string("甲"), Object::int(1));
        assert_eq!(encode(&Object::Dict(dict)).unwrap(), r#"{"乙":2,"甲":1}"#);
    }

    #[test]
    fn 编码嵌套结构() {
        let mut inner = FanDict::new();
        inner.set(Object::string("名"), Object::string("凡"));
        let value = Object::array(vec![Object::Dict(inner), Object::Bool(false)]);
        assert_eq!(encode(&value).unwrap(), r#"[{"名":"凡"},false]"#);
    }

    #[test]
    fn 编码不支持的类型报错() {
        let func = Object::Func(Rc::new(fan_object::Function::new(
            "甲",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        )));
        assert!(encode(&func).unwrap_err().contains("不能序列化为 JSON"));
    }

    #[test]
    fn 编码非有限浮点数报错() {
        // 回落实现会写出非法的 NaN 文本，扩展库改为报错
        assert!(encode(&Object::Float(f64::NAN)).is_err());
        assert!(encode(&Object::Float(f64::INFINITY)).is_err());
    }

    #[test]
    fn 解码基本类型() {
        assert_eq!(decode("1").unwrap(), Object::int(1));
        assert_eq!(decode("2.5").unwrap(), Object::Float(2.5));
        assert_eq!(decode("\"甲\"").unwrap(), Object::string("甲"));
        assert_eq!(decode("true").unwrap(), Object::Bool(true));
        assert_eq!(decode("null").unwrap(), Object::Nil);
    }

    #[test]
    fn 解码整数不退化成浮点() {
        // 凡语言层按整数比较（编码_测试 断言 值["items"][1] 等于 2）
        let items = match decode("[1,2]").unwrap() {
            Object::Array(a) => a.elements(),
            other => panic!("应为数组，实际 {}", other.kind()),
        };
        assert_eq!(items[1], Object::int(2));
    }

    #[test]
    fn 解码嵌套结构() {
        let value = decode(r#"{"名":"凡","项":[1,2]}"#).unwrap();
        let dict = match &value {
            Object::Dict(d) => d.clone(),
            other => panic!("应为字典，实际 {}", other.kind()),
        };
        assert_eq!(dict.get(&Object::string("名")), Some(Object::string("凡")));
        let items = match dict.get(&Object::string("项")).unwrap() {
            Object::Array(a) => a.elements(),
            other => panic!("应为数组，实际 {}", other.kind()),
        };
        assert_eq!(items[1], Object::int(2));
    }

    #[test]
    fn 非法文本返回错误而非中断() {
        let out = call("jsonDecode", vec![Object::string("{bad")]).unwrap();
        assert_eq!(out[0].inspect(), "空");
        assert!(
            out[1].inspect().contains("解析失败"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 编码失败的空值是空串() {
        let func = Object::Func(Rc::new(fan_object::Function::new(
            "甲",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        )));
        let out = call("jsonEncode", vec![func]).unwrap();
        assert_eq!(out[0].inspect(), "");
    }

    #[test]
    fn 内建名与模块名符合加载约定() {
        for spec in specs() {
            assert_eq!(spec.ext_module, Some(MODULE));
        }
        assert!(specs().iter().any(|s| s.name == "jsonEncode"));
        assert!(specs().iter().any(|s| s.name == "jsonDecode"));
    }

    #[test]
    fn 导出的abi版本与解释器一致() {
        assert_eq!(fan_ext_abi_version(), fan_object::FAN_EXT_ABI_VERSION);
    }

    #[test]
    fn 通过c接口注册可用() {
        let mut rt = Runtime::new();
        unsafe { fan_ext_register(&mut rt as *mut Runtime) };
        for name in ["jsonEncode", "jsonDecode"] {
            assert!(rt.lookup_native(name).is_some(), "内建 {name} 未注册");
        }
    }

    #[test]
    fn 往返一致() {
        let mut dict = FanDict::new();
        dict.set(Object::string("名"), Object::string("凡"));
        dict.set(Object::string("版本"), Object::int(1));
        let text = encode(&Object::Dict(dict.clone())).unwrap();
        assert_eq!(decode(&text).unwrap(), Object::Dict(dict));
    }
}
