//! 网络组默认实现：用标准库的 `TcpStream` 手写 HTTP/1.1。
//!
//! 这是 `@内建模块("网络")` 在**没有**扩展动态库时的回落实现。它只支持
//! 明文 HTTP（不支持 HTTPS/TLS），能力受限但无需任何第三方依赖；提供
//! `fan_ext_http` 扩展库后即可自动切换到完整的客户端实现。
//!
//! 请求结果以字典返回，键与标准库约定一致：`success`、`statusCode`、`body`、
//! `headers`。失败时**同时**给出错误值与 `success: 假` 的字典，便于凡语言层
//! 用 `如果 !结果["success"]` 判断。

use fan_object::{ErrorValue, FanDict, NativeSpec, Object};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::rc::Rc;
use std::time::Duration;

/// 解析出的 URL 各部分。
#[derive(Debug)]
struct Url {
    host: String,
    port: u16,
    path: String,
}

/// 解析 `http://host[:port]/path`；不支持的协议返回错误。
fn parse_url(raw: &str) -> Result<Url, String> {
    let rest = raw
        .strip_prefix("http://")
        .ok_or_else(|| format!("仅支持 http 协议，实际地址：{}", raw))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| format!("端口非法：{}", p))?,
        ),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err(format!("地址缺少主机名：{}", raw));
    }
    Ok(Url { host, port, path })
}

/// 从选项字典取值。
fn opt(options: Option<&FanDict>, key: &str) -> Option<Object> {
    options.and_then(|d| d.get(&Object::string(key)))
}

/// 组装并发送一次 HTTP 请求，返回响应。
fn request(method: &str, url: &str, options: Option<&FanDict>) -> Result<FanDict, String> {
    let parsed = parse_url(url)?;
    let timeout = opt(options, "timeout")
        .and_then(|v| v.as_float())
        .unwrap_or(10.0);
    let timeout = if timeout > 0.0 {
        Duration::from_secs_f64(timeout)
    } else {
        Duration::from_secs(10)
    };

    let addr = format!("{}:{}", parsed.host, parsed.port);
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("连接 {} 失败：{}", addr, e))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    // 请求行与必要头
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: fan/1.0\r\n",
        method.to_uppercase(),
        parsed.path,
        parsed.host
    );
    let body = opt(options, "body")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    // 自定义请求头
    if let Some(Object::Dict(headers)) = opt(options, "headers") {
        let inner = headers.0.borrow();
        for key in &inner.keys {
            let name = match key {
                Object::Str(s) => s.to_string(),
                other => other.inspect(),
            };
            if let Some(value) = headers.get(key) {
                head.push_str(&format!("{}: {}\r\n", name, value.inspect()));
            }
        }
    }
    head.push_str("\r\n");

    stream
        .write_all(head.as_bytes())
        .map_err(|e| format!("发送请求失败：{}", e))?;
    if !body.is_empty() {
        stream
            .write_all(body.as_bytes())
            .map_err(|e| format!("发送请求体失败：{}", e))?;
    }
    let _ = stream.flush();

    // 读状态行
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("读取响应失败：{}", e))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| format!("响应状态行无法解析：{}", status_line.trim()))?;

    // 读响应头
    let mut headers = FanDict::new();
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .map_err(|e| format!("读取响应头失败：{}", e))?
            == 0
        {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = value.parse::<usize>().ok();
            }
            headers.set(Object::string(name.to_string()), Object::string(value));
        }
    }

    // 读响应体：按 Content-Length，缺失则读到连接关闭
    let mut body_bytes = Vec::new();
    match content_length {
        Some(n) => {
            let mut limited = reader.take(n as u64);
            limited
                .read_to_end(&mut body_bytes)
                .map_err(|e| format!("读取响应体失败：{}", e))?;
        }
        None => {
            reader
                .read_to_end(&mut body_bytes)
                .map_err(|e| format!("读取响应体失败：{}", e))?;
        }
    }
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();

    let mut result = FanDict::new();
    result.set(
        Object::string("success"),
        Object::Bool((200..300).contains(&status_code)),
    );
    result.set(Object::string("statusCode"), Object::int(status_code));
    result.set(Object::string("body"), Object::string(body_text));
    result.set(Object::string("headers"), Object::Dict(headers));
    Ok(result)
}

pub fn specs() -> Vec<NativeSpec> {
    vec![NativeSpec::core(
        "httpRequest",
        Some(2),
        Rc::new(|_rt, args| {
            let method = args[0].as_str().unwrap_or_else(|| "GET".to_string());
            let url = args[1].as_str().unwrap_or_default();
            let options = match args.get(2) {
                Some(Object::Dict(d)) => Some(d.clone()),
                _ => None,
            };
            match request(&method, &url, options.as_ref()) {
                Ok(result) => Ok(vec![Object::Dict(result), Object::Nil]),
                Err(e) => {
                    // 失败时仍给出结果字典，便于凡语言层读取 success
                    let mut result = FanDict::new();
                    result.set(Object::string("success"), Object::Bool(false));
                    result.set(Object::string("statusCode"), Object::int(0));
                    result.set(Object::string("body"), Object::string(String::new()));
                    Ok(vec![
                        Object::Dict(result),
                        Object::Error(Rc::new(std::cell::RefCell::new(ErrorValue::new(e)))),
                    ])
                }
            }
        }),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::Runtime;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn 解析地址() {
        let u = parse_url("http://example.com/a/b").unwrap();
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 80);
        assert_eq!(u.path, "/a/b");
        let u = parse_url("http://127.0.0.1:8080/x").unwrap();
        assert_eq!(u.port, 8080);
    }

    #[test]
    fn 拒绝非http协议() {
        assert!(parse_url("https://example.com")
            .unwrap_err()
            .contains("仅支持 http"));
        assert!(parse_url("bad-url").unwrap_err().contains("仅支持 http"));
    }

    #[test]
    fn 非法端口报错() {
        assert!(parse_url("http://host:abc/")
            .unwrap_err()
            .contains("端口非法"));
    }

    #[test]
    fn 请求失败时给出成功为假的结果字典() {
        let out = call(
            "httpRequest",
            vec![
                Object::string("GET"),
                Object::string("bad-url"),
                Object::Nil,
            ],
        )
        .unwrap();
        let dict = match &out[0] {
            Object::Dict(d) => d.clone(),
            other => panic!("期望字典，实际 {}", other.inspect()),
        };
        assert_eq!(
            dict.get(&Object::string("success")),
            Some(Object::Bool(false))
        );
        assert!(
            out[1].inspect().contains("仅支持 http"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 连接不可达端口返回错误() {
        let mut options = FanDict::new();
        options.set(Object::string("timeout"), Object::Float(1.0));
        let out = call(
            "httpRequest",
            vec![
                Object::string("GET"),
                Object::string("http://127.0.0.1:1"),
                Object::Dict(options),
            ],
        )
        .unwrap();
        assert!(
            out[1].inspect().contains("连接"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 与本地服务器完成一次往返() {
        // 起一个极简 HTTP 服务器，验证请求组装与响应解析
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = socket.read(&mut buf).unwrap_or(0);
            let received = String::from_utf8_lossy(&buf[..n]).to_string();
            // 回显收到的请求行，便于断言方法正确
            let first_line = received.lines().next().unwrap_or("").to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                first_line.len(),
                first_line
            );
            let _ = socket.write_all(response.as_bytes());
        });

        let out = call(
            "httpRequest",
            vec![
                Object::string("GET"),
                Object::string(format!("http://127.0.0.1:{}/hello", port)),
                Object::Nil,
            ],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil, "错误：{}", out[1].inspect());
        let dict = match &out[0] {
            Object::Dict(d) => d.clone(),
            other => panic!("期望字典，实际 {}", other.inspect()),
        };
        assert_eq!(
            dict.get(&Object::string("success")),
            Some(Object::Bool(true))
        );
        assert_eq!(
            dict.get(&Object::string("statusCode")),
            Some(Object::int(200))
        );
        assert_eq!(
            dict.get(&Object::string("body")),
            Some(Object::string("GET /hello HTTP/1.1"))
        );
        handle.join().unwrap();
    }

    #[test]
    fn 网络组内建具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        assert!(names.contains(&"httpRequest"));
    }
}
