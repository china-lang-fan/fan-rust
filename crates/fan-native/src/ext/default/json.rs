//! JSON 默认实现：纯 Rust 标准库，不依赖 `serde_json`。
//!
//! 这是 `@内建模块("json")` 在**没有**扩展动态库时使用的回落实现。它只覆盖
//! JSON 的核心语法（对象/数组/字符串/数值/布尔/空），足以支撑标准库
//! `编码.凡` 的日常用法；若需要更强的能力（如自定义数字精度、大文档性能），
//! 提供 `fan_ext_json` 扩展库即可自动切换过去。

use fan_ast::format_f64;
use fan_object::{NativeSpec, Object};
use std::rc::Rc;

/// 把对象序列化为 JSON 文本。
pub fn encode(value: &Object) -> Result<String, String> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

fn write_value(value: &Object, out: &mut String) -> Result<(), String> {
    match value {
        Object::Int(i) => out.push_str(&i.to_string()),
        Object::Float(f) => out.push_str(&format_f64(*f)),
        Object::Byte(b) => out.push_str(&b.to_string()),
        Object::Char(c) => write_string(&c.to_string(), out),
        Object::Str(s) => write_string(s, out),
        Object::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Object::Nil => out.push_str("null"),
        Object::Array(a) => {
            out.push('[');
            for (i, item) in a.elements().iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Object::Dict(d) => {
            out.push('{');
            let inner = d.0.borrow();
            for (i, key) in inner.keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let key_text = match key {
                    Object::Str(s) => s.to_string(),
                    other => other.inspect(),
                };
                write_string(&key_text, out);
                out.push(':');
                // 内部键含类型前缀，须经 `get` 取回，不能直查 entries
                write_value(&d.get(key).unwrap_or(Object::Nil), out)?;
            }
            out.push('}');
        }
        other => return Err(format!("{} 不能序列化为 JSON", other.kind())),
    }
    Ok(())
}

/// 写入带转义的 JSON 字符串。控制字符一律用短转义或 `\u00XX`。
fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JSON 递归下降解析器。
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Parser {
            bytes: text.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// 消费期望的字符。
    fn expect(&mut self, ch: u8) -> Result<(), String> {
        match self.peek() {
            Some(c) if c == ch => {
                self.pos += 1;
                Ok(())
            }
            Some(c) => Err(format!(
                "第 {} 个字符处期望 '{}'，实际 '{}'",
                self.pos, ch as char, c as char
            )),
            None => Err("JSON 文本意外结束".to_string()),
        }
    }

    /// 解析完整文档：允许顶层是任意值（JSON 文本序列的现代惯例）。
    fn parse_document(&mut self) -> Result<Object, String> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.skip_ws();
        if self.pos < self.bytes.len() {
            return Err(format!("第 {} 个字符处存在多余内容", self.pos));
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<Object, String> {
        self.skip_ws();
        match self.peek().ok_or_else(|| "JSON 文本为空".to_string())? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => {
                let text = self.parse_string()?;
                Ok(Object::string(text))
            }
            b't' => {
                self.expect_literal("true")?;
                Ok(Object::Bool(true))
            }
            b'f' => {
                self.expect_literal("false")?;
                Ok(Object::Bool(false))
            }
            b'n' => {
                self.expect_literal("null")?;
                Ok(Object::Nil)
            }
            b'-' | b'0'..=b'9' => self.parse_number(),
            c => Err(format!(
                "第 {} 个字符处无法识别的内容 '{}'",
                self.pos, c as char
            )),
        }
    }

    fn expect_literal(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(format!("第 {} 个字符处期望 {}", self.pos, word))
        }
    }

    fn parse_object(&mut self) -> Result<Object, String> {
        self.expect(b'{')?;
        let mut dict = fan_object::FanDict::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Object::Dict(dict));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value()?;
            dict.set(Object::string(key), value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Object::Dict(dict));
                }
                _ => return Err(format!("第 {} 个字符处期望 ',' 或 '}}'", self.pos)),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Object, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Object::array(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Object::array(items));
                }
                _ => return Err(format!("第 {} 个字符处期望 ',' 或 ']'", self.pos)),
            }
        }
    }

    /// 解析字符串，处理全部转义形式（含 `\uXXXX` 代理对）。
    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        // 收集字节而非字符：非 ASCII 需按 UTF-8 整体还原，逐字节转 char 会乱码
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = self.bytes.get(self.pos).copied().ok_or("字符串未闭合")?;
            self.pos += 1;
            match b {
                b'"' => {
                    return String::from_utf8(out).map_err(|_| "字符串不是合法 UTF-8".to_string());
                }
                b'\\' => {
                    let esc = self.bytes.get(self.pos).copied().ok_or("转义序列不完整")?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            // 高位代理后紧跟低位代理时合并为一个码位
                            let scalar = if (0xD800..0xDC00).contains(&code)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                let save = self.pos;
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)
                                } else {
                                    self.pos = save;
                                    code
                                }
                            } else {
                                code
                            };
                            let ch = char::from_u32(scalar)
                                .ok_or_else(|| format!("无效的转义码位 U+{:04X}", scalar))?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => return Err(format!("无法识别的转义 \\{}", other as char)),
                    }
                }
                // 其余字节原样收集，最终统一按 UTF-8 校验
                _ => out.push(b),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err("\\u 转义需要四个十六进制位".to_string());
        }
        let mut code = 0u32;
        for _ in 0..4 {
            let c = self.bytes[self.pos];
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return Err(format!("非法的十六进制位 '{}'", c as char)),
            };
            code = code * 16 + digit as u32;
            self.pos += 1;
        }
        Ok(code)
    }

    /// 解析数值：含小数点或指数判为小数，否则为整数。
    fn parse_number(&mut self) -> Result<Object, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let mut is_float = false;
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' => {
                    is_float = true;
                    self.pos += 1;
                }
                b'+' | b'-' => self.pos += 1,
                _ => break,
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "数值含非法字节".to_string())?;
        if text.is_empty() || text == "-" {
            return Err(format!("第 {} 个字符处不是合法数值", start));
        }
        if is_float {
            text.parse::<f64>()
                .map(Object::Float)
                .map_err(|_| format!("无法解析小数：{}", text))
        } else {
            text.parse::<i64>()
                .map(Object::int)
                .map_err(|_| format!("无法解析整数：{}", text))
        }
    }
}

/// 把 JSON 文本解析为对象。
pub fn decode(text: &str) -> Result<Object, String> {
    Parser::new(text).parse_document()
}

/// 把 JSON 组的默认实现注册为内建。
pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "jsonEncode",
            Some(1),
            Rc::new(|_rt, args| match encode(&args[0]) {
                Ok(text) => Ok(vec![Object::string(text), Object::Nil]),
                Err(e) => Ok(vec![Object::string(String::new()), Object::string(e)]),
            }),
        ),
        NativeSpec::core(
            "jsonDecode",
            Some(1),
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                match decode(&text) {
                    Ok(value) => Ok(vec![value, Object::Nil]),
                    Err(e) => Ok(vec![Object::Nil, Object::string(e)]),
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 编码基本类型() {
        assert_eq!(encode(&Object::int(42)).unwrap(), "42");
        assert_eq!(encode(&Object::Float(1.5)).unwrap(), "1.5");
        assert_eq!(encode(&Object::string("甲")).unwrap(), "\"甲\"");
        assert_eq!(encode(&Object::Bool(true)).unwrap(), "true");
        assert_eq!(encode(&Object::Nil).unwrap(), "null");
    }

    #[test]
    fn 编码需要转义的字符() {
        assert_eq!(encode(&Object::string("a\"b")).unwrap(), "\"a\\\"b\"");
        assert_eq!(encode(&Object::string("a\nb")).unwrap(), "\"a\\nb\"");
        assert_eq!(encode(&Object::string("a\\b")).unwrap(), "\"a\\\\b\"");
        assert_eq!(encode(&Object::string("\u{1}")).unwrap(), "\"\\u0001\"");
    }

    #[test]
    fn 编码数组与嵌套() {
        assert_eq!(
            encode(&Object::array(vec![Object::int(1), Object::int(2)])).unwrap(),
            "[1,2]"
        );
        let mut dict = fan_object::FanDict::new();
        dict.set(Object::string("甲"), Object::int(1));
        assert_eq!(encode(&Object::Dict(dict)).unwrap(), "{\"甲\":1}");
    }

    #[test]
    fn 编码函数报错() {
        let func = Object::Func(std::rc::Rc::new(fan_object::Function::new(
            "甲",
            vec![],
            Default::default(),
            None,
            fan_ast::Position::START,
        )));
        assert!(encode(&func).unwrap_err().contains("不能序列化"));
    }

    #[test]
    fn 解码基本类型() {
        assert_eq!(decode("42").unwrap(), Object::int(42));
        assert_eq!(decode("-7").unwrap(), Object::int(-7));
        assert_eq!(decode("1.5").unwrap(), Object::Float(1.5));
        assert_eq!(decode("\"甲\"").unwrap(), Object::string("甲"));
        assert_eq!(decode("true").unwrap(), Object::Bool(true));
        assert_eq!(decode("false").unwrap(), Object::Bool(false));
        assert_eq!(decode("null").unwrap(), Object::Nil);
    }

    #[test]
    fn 解码整数与小数分流() {
        assert_eq!(decode("3").unwrap(), Object::int(3));
        assert_eq!(decode("3.0").unwrap(), Object::Float(3.0));
        assert_eq!(decode("1e3").unwrap(), Object::Float(1000.0));
    }

    #[test]
    fn 解码数组与对象() {
        assert_eq!(
            decode("[1,2]").unwrap(),
            Object::array(vec![Object::int(1), Object::int(2)])
        );
        let obj = decode("{\"甲\":1}").unwrap();
        match obj {
            Object::Dict(d) => assert_eq!(d.get(&Object::string("甲")), Some(Object::int(1))),
            other => panic!("期望字典，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 解码转义与代理对() {
        assert_eq!(decode("\"a\\nb\"").unwrap(), Object::string("a\nb"));
        // 代理对 U+D83D U+DE00 → 😀
        assert_eq!(decode("\"\\ud83d\\ude00\"").unwrap(), Object::string("😀"));
    }

    #[test]
    fn 解码嵌套结构() {
        let value = decode("{\"甲\":[1,{\"乙\":true}],\"丙\":null}").unwrap();
        // 往返一致
        assert_eq!(decode(&encode(&value).unwrap()).unwrap(), value);
    }

    #[test]
    fn 解码空白容错() {
        let mut expected = fan_object::FanDict::new();
        expected.set(Object::string("甲"), Object::int(1));
        assert_eq!(
            decode("  { \"甲\" : 1 }  ").unwrap(),
            Object::Dict(expected)
        );
    }

    #[test]
    fn 解码保留中文键与值() {
        let mut expected = fan_object::FanDict::new();
        expected.set(Object::string("名字"), Object::string("凡语言"));
        assert_eq!(
            decode("{\"名字\":\"凡语言\"}").unwrap(),
            Object::Dict(expected)
        );
    }

    #[test]
    fn 非法输入返回错误() {
        assert!(decode("").is_err());
        assert!(decode("{").is_err());
        assert!(decode("{\"甲\"}").is_err());
        assert!(decode("[1,]").is_err());
        assert!(decode("甲乙").is_err());
    }

    #[test]
    fn 多余内容报错() {
        assert!(decode("1 2").unwrap_err().contains("多余内容"));
    }

    #[test]
    fn 往返一致() {
        let mut dict = fan_object::FanDict::new();
        dict.set(Object::string("名"), Object::string("凡"));
        dict.set(Object::string("版本"), Object::int(1));
        let original = Object::Dict(dict);
        let text = encode(&original).unwrap();
        assert_eq!(decode(&text).unwrap(), original);
    }

    #[test]
    fn 内建返回错误而非中断() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "jsonDecode")
            .unwrap();
        let mut rt = fan_object::Runtime::new();
        let out = (spec.func)(&mut rt, vec![Object::string("{坏")]).unwrap();
        assert_eq!(out[0], Object::Nil);
        assert!(out[1].inspect().contains("期望") || out[1].inspect().contains("结束"));
    }
}
