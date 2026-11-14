//! 编码组内建：Base64、十六进制、URL 百分号编码。
//!
//! 对应 `fan-sdk/编码.凡` 中的非 JSON 部分。这些都是**纯数据变换**，用标准库
//! 实现仅需数十行，因此归入 `core` 而非依赖第三方解析器的 `ext` 组——只有
//! JSON、正则、HTTP 这类才进 `ext`。
//!
//! 解码类函数统一返回 `(值, 错误)` 而非抛错，与凡语言层约定一致。

use fan_object::{NativeSpec, Object};
use std::rc::Rc;

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 编码，带 `=` 填充。
pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(BASE64_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Base64 解码；输入含非法字符时返回错误。
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for ch in input.chars() {
        if ch == '=' || ch == '\n' || ch == '\r' || ch == ' ' {
            continue;
        }
        let v = BASE64_ALPHABET
            .iter()
            .position(|&c| c == ch as u8)
            .ok_or_else(|| format!("Base64 含非法字符：{}", ch))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// 十六进制编码（小写）。
pub fn hex_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// 十六进制解码；长度为奇数或含非法字符时返回错误。
pub fn hex_decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = input.as_bytes().to_vec();
    if !bytes.len().is_multiple_of(2) {
        return Err("十六进制文本长度必须为偶数".to_string());
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi =
            hex_val(pair[0]).ok_or_else(|| format!("非法的十六进制字符：{}", pair[0] as char))?;
        let lo =
            hex_val(pair[1]).ok_or_else(|| format!("非法的十六进制字符：{}", pair[1] as char))?;
        out.push(hi << 4 | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// URL 百分号编码：保留 `A-Za-z0-9-_.~`，其余按 UTF-8 字节编码。
pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// URL 百分号解码；`+` 视为空格，非法转义返回错误。
pub fn url_decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err("URL 转义序列不完整".to_string());
                }
                let hi = hex_val(bytes[i + 1]).ok_or("URL 转义含非法字符")?;
                let lo = hex_val(bytes[i + 2]).ok_or("URL 转义含非法字符")?;
                out.push(hi << 4 | lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// 取文本的 UTF-8 字节；非文本值回退为其显示文本。
fn as_bytes(value: &Object) -> Vec<u8> {
    match value {
        Object::Str(s) => s.as_bytes().to_vec(),
        other => other.inspect().into_bytes(),
    }
}

/// 解码结果转回文本；非 UTF-8 时按 Latin-1 逐字节还原。
fn bytes_to_text(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "base64Encode",
            Some(1),
            Rc::new(|_rt, args| Ok(vec![Object::string(base64_encode(&as_bytes(&args[0])))])),
        ),
        NativeSpec::core(
            "base64Decode",
            Some(1),
            Rc::new(
                |_rt, args| match base64_decode(&args[0].as_str().unwrap_or_default()) {
                    Ok(bytes) => Ok(vec![Object::string(bytes_to_text(bytes)), Object::Nil]),
                    Err(e) => Ok(vec![Object::string(String::new()), Object::string(e)]),
                },
            ),
        ),
        NativeSpec::core(
            "hexEncode",
            Some(1),
            Rc::new(|_rt, args| Ok(vec![Object::string(hex_encode(&as_bytes(&args[0])))])),
        ),
        NativeSpec::core(
            "hexDecode",
            Some(1),
            Rc::new(
                |_rt, args| match hex_decode(&args[0].as_str().unwrap_or_default()) {
                    Ok(bytes) => Ok(vec![Object::string(bytes_to_text(bytes)), Object::Nil]),
                    Err(e) => Ok(vec![Object::string(String::new()), Object::string(e)]),
                },
            ),
        ),
        NativeSpec::core(
            "urlEncode",
            Some(1),
            Rc::new(|_rt, args| {
                Ok(vec![Object::string(url_encode(
                    &args[0].as_str().unwrap_or_default(),
                ))])
            }),
        ),
        NativeSpec::core(
            "urlDecode",
            Some(1),
            Rc::new(
                |_rt, args| match url_decode(&args[0].as_str().unwrap_or_default()) {
                    Ok(bytes) => Ok(vec![Object::string(bytes_to_text(bytes)), Object::Nil]),
                    Err(e) => Ok(vec![Object::string(String::new()), Object::string(e)]),
                },
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = fan_object::Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn base64编解码往返() {
        let encoded = call("base64Encode", vec![Object::string("凡语言")]).unwrap()[0].inspect();
        let decoded = call("base64Decode", vec![Object::string(encoded.clone())]).unwrap();
        assert_eq!(decoded[0].inspect(), "凡语言");
        assert_eq!(decoded[1], Object::Nil);
    }

    #[test]
    fn base64编码标准向量() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
        assert_eq!(base64_encode(b"H"), "SA==");
        assert_eq!(base64_encode(b"He"), "SGU=");
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64解码非法输入返回错误() {
        let out = call("base64Decode", vec![Object::string("!!!")]).unwrap();
        assert!(out[1].inspect().contains("非法字符"));
    }

    #[test]
    fn 十六进制编解码往返() {
        let encoded = call("hexEncode", vec![Object::string("AB")]).unwrap()[0].inspect();
        assert_eq!(encoded, "4142");
        let decoded = call("hexDecode", vec![Object::string(encoded)]).unwrap();
        assert_eq!(decoded[0].inspect(), "AB");
    }

    #[test]
    fn 十六进制解码校验长度与字符() {
        assert!(call("hexDecode", vec![Object::string("4")]).unwrap()[1]
            .inspect()
            .contains("偶数"));
        assert!(call("hexDecode", vec![Object::string("ZZ")]).unwrap()[1]
            .inspect()
            .contains("非法"));
    }

    #[test]
    fn url编码保留安全字符() {
        assert_eq!(url_encode("abc-_.~"), "abc-_.~");
        assert_eq!(url_encode("a b"), "a+b");
        assert_eq!(url_encode("甲"), "%E7%94%B2");
    }

    #[test]
    fn url解码还原中文与空格() {
        let out = call("urlDecode", vec![Object::string("%E7%94%B2")]).unwrap();
        assert_eq!(out[0].inspect(), "甲");
        assert_eq!(
            call("urlDecode", vec![Object::string("a+b")]).unwrap()[0].inspect(),
            "a b"
        );
    }

    #[test]
    fn url解码不完整转义报错() {
        assert!(call("urlDecode", vec![Object::string("%4")]).unwrap()[1]
            .inspect()
            .contains("不完整"));
    }

    #[test]
    fn 编码组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "base64Encode",
            "base64Decode",
            "hexEncode",
            "hexDecode",
            "urlEncode",
            "urlDecode",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
