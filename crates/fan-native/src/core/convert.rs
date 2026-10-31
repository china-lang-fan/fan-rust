//! 转换组内建：文本与数值的互转。
//!
//! 对应 `fan-sdk/转换.凡`。凡语言层已用 `判断 系统.类型名(值)` 完成分派，
//! 内建只提供两类凡语言难以正确实现的能力：浮点数的文本格式化（需与字面量
//! 显示一致）以及字符串的容错解析（需返回 `(值, 错误)` 而非抛异常）。

use fan_ast::format_f64;
use fan_object::{NativeSpec, Object};
use std::rc::Rc;

/// 解析布尔文本，与 `转为 布尔` 的取值表保持一致。
pub(crate) fn parse_bool_text(text: &str) -> Option<bool> {
    match text.trim().to_lowercase().as_str() {
        "真" | "true" | "1" | "yes" | "on" => Some(true),
        "假" | "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "intToString",
            Some(1),
            Rc::new(|_rt, args| {
                let n = args[0].as_int().ok_or("整数转文本 需要整数")?;
                Ok(vec![Object::string(n.to_string())])
            }),
        ),
        // 小数文本化复用字面量格式化，保证 `1.5` 显示与解析同源
        NativeSpec::core(
            "floatToString",
            Some(1),
            Rc::new(|_rt, args| {
                let f = args[0].as_float().ok_or("小数转文本 需要小数")?;
                Ok(vec![Object::string(format_f64(f))])
            }),
        ),
        NativeSpec::core(
            "parseInt",
            Some(1),
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                match text.trim().parse::<i64>() {
                    Ok(n) => Ok(vec![Object::int(n), Object::Nil]),
                    Err(_) => Ok(vec![
                        Object::int(0),
                        Object::string(format!("无法解析整数：{}", text)),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "parseFloat",
            Some(1),
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                match text.trim().parse::<f64>() {
                    Ok(f) => Ok(vec![Object::Float(f), Object::Nil]),
                    Err(_) => Ok(vec![
                        Object::Float(0.0),
                        Object::string(format!("无法解析小数：{}", text)),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "parseBool",
            Some(1),
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                match parse_bool_text(&text) {
                    Some(b) => Ok(vec![Object::Bool(b), Object::Nil]),
                    None => Ok(vec![
                        Object::Bool(false),
                        Object::string(format!("无法解析布尔：{}", text)),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "toByte",
            Some(1),
            Rc::new(|_rt, args| {
                let n = args[0].as_int().ok_or("转字节 需要整数")?;
                if (0..=255).contains(&n) {
                    Ok(vec![Object::Byte(n as u8), Object::Nil])
                } else {
                    Ok(vec![
                        Object::Byte(0),
                        Object::string(format!("{} 超出字节范围 0-255", n)),
                    ])
                }
            }),
        ),
        NativeSpec::core(
            "toChar",
            Some(1),
            Rc::new(|_rt, args| {
                let arg = &args[0];
                let c = match arg {
                    // 整数按编码点，单字符字符串/字符按字形——标准库
                    // 两种用法都有（转字符(65) 与 转字符("凡")）
                    Object::Str(s) => {
                        let mut chars = s.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) => c,
                            _ => {
                                return Ok(vec![
                                    Object::Char('\0'),
                                    Object::string(format!(
                                        "转字符 需要恰好一个字符，实际是 {}",
                                        s
                                    )),
                                ])
                            }
                        }
                    }
                    Object::Char(c) => *c,
                    _ => {
                        let n = arg.as_int().ok_or("转字符 需要整数或单字符字符串")?;
                        match char::from_u32(n as u32) {
                            Some(c) => c,
                            None => {
                                return Ok(vec![
                                    Object::Char('\0'),
                                    Object::string(format!("{} 不是有效的编码点", n)),
                                ])
                            }
                        }
                    }
                };
                Ok(vec![Object::Char(c), Object::Nil])
            }),
        ),
    ]
}

// 测试中 3.14 只是普通小数，并非要近似 π
#[cfg_attr(test, allow(clippy::approx_constant))]
#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::Object;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = fan_object::Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn 整数与文本互转() {
        assert_eq!(
            call("intToString", vec![Object::int(42)]).unwrap()[0],
            Object::string("42")
        );
        assert_eq!(
            call("intToString", vec![Object::int(-7)]).unwrap()[0],
            Object::string("-7")
        );
        assert_eq!(
            call("parseInt", vec![Object::string("42")]).unwrap()[0],
            Object::int(42)
        );
        assert_eq!(
            call("parseInt", vec![Object::string(" -13 ")]).unwrap()[0],
            Object::int(-13)
        );
    }

    #[test]
    fn 解析整数失败返回错误而非中断() {
        let out = call("parseInt", vec![Object::string("甲乙")]).unwrap();
        assert_eq!(out[0], Object::int(0));
        match &out[1] {
            Object::Str(s) => assert!(s.contains("无法解析整数")),
            other => panic!("期望错误消息，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 小数文本化与字面量显示一致() {
        assert_eq!(
            call("floatToString", vec![Object::Float(1.5)]).unwrap()[0],
            Object::string("1.5")
        );
        assert_eq!(
            call("floatToString", vec![Object::Float(3.0)]).unwrap()[0],
            Object::string("3")
        );
        // 与 AST 字面量格式化同源
        assert_eq!(
            format_f64(0.1 + 0.2),
            call("floatToString", vec![Object::Float(0.1 + 0.2)]).unwrap()[0].inspect()
        );
    }

    #[test]
    fn 解析小数与布尔() {
        assert_eq!(
            call("parseFloat", vec![Object::string("3.14")]).unwrap()[0],
            Object::Float(3.14)
        );
        assert_eq!(
            call("parseBool", vec![Object::string("真")]).unwrap()[0],
            Object::Bool(true)
        );
        assert_eq!(
            call("parseBool", vec![Object::string("false")]).unwrap()[0],
            Object::Bool(false)
        );
    }

    #[test]
    fn 解析布尔失败返回错误() {
        let out = call("parseBool", vec![Object::string("也许")]).unwrap();
        assert_eq!(out[0], Object::Bool(false));
        assert!(out[1].inspect().contains("无法解析布尔"));
    }

    #[test]
    fn 转字节校验范围() {
        assert_eq!(
            call("toByte", vec![Object::int(255)]).unwrap()[0],
            Object::Byte(255)
        );
        let out = call("toByte", vec![Object::int(256)]).unwrap();
        assert!(out[1].inspect().contains("超出字节范围"));
    }

    #[test]
    fn 转字符校验编码点() {
        assert_eq!(
            call("toChar", vec![Object::int(65)]).unwrap()[0],
            Object::Char('A')
        );
        assert!(call("toChar", vec![Object::int(-1)]).unwrap()[1]
            .inspect()
            .contains("编码点"));
    }

    #[test]
    fn 布尔取值表与转为运算一致() {
        for (text, want) in [
            ("真", true),
            ("true", true),
            ("1", true),
            ("YES", true),
            ("on", true),
            ("假", false),
            ("false", false),
            ("0", false),
            ("no", false),
            ("off", false),
        ] {
            assert_eq!(parse_bool_text(text), Some(want), "{text}");
        }
        assert_eq!(parse_bool_text("也许"), None);
    }

    #[test]
    fn 字符编码点取值() {
        assert_eq!('A' as i64, 65);
    }

    #[test]
    fn 转换组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "intToString",
            "floatToString",
            "parseInt",
            "parseFloat",
            "parseBool",
            "toByte",
            "toChar",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
