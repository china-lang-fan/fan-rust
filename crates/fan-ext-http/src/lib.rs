//! 凡语言**网络扩展库**：基于 `ureq`，以动态库形式按需加载。
//!
//! 这是 core/ext 分工中 `ext` 侧的实现。凡代码用到 `httpRequest`（见
//! `fan-sdk/网络.凡`）时才会 `dlopen` 本库；找不到本库时回落到内置默认实现。
//!
//! # 与回落实现的差别
//!
//! 回落实现用标准库 `TcpStream` 手写 HTTP，**只支持明文 HTTP**。本库补齐
//! HTTPS/TLS 与重定向等能力——这正是把它放进扩展库的理由：TLS 依赖链不该
//! 进入解释器本体。
//!
//! # 与回落实现的契约
//!
//! 扩展库加载后会**替换**回落实现，故对外行为必须与之一致：
//!
//! - `httpRequest(方法, 地址, 选项?)` → `(结果字典, 错误值)`
//! - 结果字典含 `success`、`statusCode`、`body`、`headers`
//! - 失败时**同时**给出错误值与 `success: 假` 的字典，便于凡语言层
//!   用 `如果 !结果["success"]` 判断
//!
//! 选项键沿用回落实现：`timeout`（秒）、`body`（字符串）、`headers`（字典）。
//!
//! # 导出符号
//!
//! - `fan_ext_abi_version() -> u32`
//! - `fan_ext_register(rt: *mut Runtime)`

use fan_object::{ErrorValue, FanDict, NativeSpec, Object, Runtime};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// 本库提供的模块名。
pub const MODULE: &str = "网络";

/// 从选项字典取值。
fn opt(options: Option<&FanDict>, key: &str) -> Option<Object> {
    options.and_then(|d| d.get(&Object::string(key)))
}

/// 超时时间，缺省 10 秒；非正值同样按缺省处理。
fn timeout_of(options: Option<&FanDict>) -> Duration {
    match opt(options, "timeout").and_then(|v| v.as_float()) {
        Some(secs) if secs > 0.0 => Duration::from_secs_f64(secs),
        _ => Duration::from_secs(10),
    }
}

/// 组装并发送一次请求，返回结果字典。
pub fn request(method: &str, url: &str, options: Option<&FanDict>) -> Result<FanDict, String> {
    if url.is_empty() {
        return Err("请求地址为空".to_string());
    }
    let method_text = method.to_uppercase();
    let http_method = ureq::http::Method::from_bytes(method_text.as_bytes())
        .map_err(|_| format!("请求方法 {} 非法", method_text))?;

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout_of(options)))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut builder = ureq::http::Request::builder()
        .method(http_method)
        .uri(url)
        .header("User-Agent", "fan/1.0");

    // 自定义请求头
    if let Some(Object::Dict(headers)) = opt(options, "headers") {
        let names: Vec<Object> = headers.0.borrow().keys.clone();
        for key in names {
            let name = match &key {
                Object::Str(s) => s.to_string(),
                other => other.inspect(),
            };
            let Some(value) = headers.get(&key) else {
                continue;
            };
            // 用 as_str 取值而非 inspect：后者会给字符串加上引号，
            // 那样 Content-Type 之类的头值就不对了
            let text = value.as_str().unwrap_or_else(|| value.inspect());
            // 头名与头值须是合法 HTTP 标记，不合法的**跳过**而非让整个请求
            // 失败——回落实现是照发，这里退一步更接近它的「尽力而为」，
            // 也避免用户因一个头名写错就发不出请求
            let Ok(name) = ureq::http::HeaderName::try_from(name) else {
                continue;
            };
            let Ok(text) = ureq::http::HeaderValue::try_from(text) else {
                continue;
            };
            builder = builder.header(name, text);
        }
    }

    let body = opt(options, "body")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // 空请求体用 `()`，避免给 GET 之类的方法挂上实体
    let mut response = if body.is_empty() {
        let req = builder.body(()).map_err(|e| format!("构造请求失败：{e}"))?;
        agent.run(req).map_err(|e| format!("请求失败：{e}"))?
    } else {
        let req = builder
            .body(body)
            .map_err(|e| format!("构造请求失败：{e}"))?;
        agent.run(req).map_err(|e| format!("请求失败：{e}"))?
    };

    let status = response.status().as_u16() as i64;
    let mut out_headers = FanDict::new();
    for (name, value) in response.headers() {
        out_headers.set(
            Object::string(name.as_str()),
            Object::string(value.to_str().unwrap_or_default()),
        );
    }
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("读取响应失败：{e}"))?;

    let mut result = FanDict::new();
    result.set(
        Object::string("success"),
        Object::Bool((200..300).contains(&status)),
    );
    result.set(Object::string("statusCode"), Object::int(status));
    result.set(Object::string("body"), Object::string(text));
    result.set(Object::string("headers"), Object::Dict(out_headers));
    Ok(result)
}

/// 失败时仍给出结果字典，便于凡语言层读取 `success`。
fn failure_of(message: String) -> Vec<Object> {
    let mut result = FanDict::new();
    result.set(Object::string("success"), Object::Bool(false));
    result.set(Object::string("statusCode"), Object::int(0));
    result.set(Object::string("body"), Object::string(String::new()));
    vec![
        Object::Dict(result),
        Object::Error(Rc::new(RefCell::new(ErrorValue::new(message)))),
    ]
}

/// 本库提供的内建。
pub fn specs() -> Vec<NativeSpec> {
    vec![NativeSpec::ext(
        "httpRequest",
        Some(2),
        MODULE,
        Rc::new(|_rt, args| {
            let method = args[0].as_str().unwrap_or_else(|| "GET".to_string());
            let url = args[1].as_str().unwrap_or_default();
            let options = match args.get(2) {
                Some(Object::Dict(d)) => Some(d.clone()),
                _ => None,
            };
            match request(&method, &url, options.as_ref()) {
                Ok(result) => Ok(vec![Object::Dict(result), Object::Nil]),
                Err(e) => Ok(failure_of(e)),
            }
        }),
    )]
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
    use std::io::{Read, Write};

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    fn get(dict: &FanDict, key: &str) -> Option<Object> {
        dict.get(&Object::string(key))
    }

    /// 起一个只回固定内容的 HTTP 服务器，返回其地址与等待句柄。
    ///
    /// 用它替代真实外网请求：单测不该依赖网络可达性。
    fn spawn_server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("起不了本机监听");
        let port = listener.local_addr().expect("取不到端口").port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

    #[test]
    fn 能对本机发起请求() {
        let (url, handle) = spawn_server("你好");
        let result = request("GET", &url, None).expect("本机请求应成功");
        assert_eq!(get(&result, "statusCode"), Some(Object::int(200)));
        assert_eq!(get(&result, "success"), Some(Object::Bool(true)));
        assert_eq!(get(&result, "body"), Some(Object::string("你好")));
        handle.join().expect("服务线程异常");
    }

    #[test]
    fn 响应头会被收集进字典() {
        let (url, handle) = spawn_server("x");
        let result = request("GET", &url, None).expect("本机请求应成功");
        let headers = match get(&result, "headers") {
            Some(Object::Dict(d)) => d,
            other => panic!("headers 应为字典，实际 {other:?}"),
        };
        assert_eq!(
            get(&headers, "content-type"),
            Some(Object::string("text/plain")),
            "响应头应含 content-type"
        );
        handle.join().expect("服务线程异常");
    }

    #[test]
    fn 自定义请求头一并发出() {
        let (url, handle) = spawn_server("x");
        let mut headers = FanDict::new();
        headers.set(Object::string("X-Test"), Object::string("fan"));
        let mut options = FanDict::new();
        options.set(Object::string("headers"), Object::Dict(headers));

        let result = request("GET", &url, Some(&options)).expect("带请求头的请求应成功");
        assert_eq!(get(&result, "statusCode"), Some(Object::int(200)));
        handle.join().expect("服务线程异常");
    }

    #[test]
    fn 非法请求头被跳过而非使请求失败() {
        let (url, handle) = spawn_server("x");
        let mut headers = FanDict::new();
        // 中文头名不是合法 HTTP 标记：跳过它，请求本身仍应成功
        headers.set(Object::string("X-测试"), Object::string("甲"));
        let mut options = FanDict::new();
        options.set(Object::string("headers"), Object::Dict(headers));

        let result = request("GET", &url, Some(&options)).expect("非法头应被跳过，请求仍成功");
        assert_eq!(get(&result, "statusCode"), Some(Object::int(200)));
        handle.join().expect("服务线程异常");
    }

    #[test]
    fn 空地址报错() {
        assert!(request("GET", "", None).unwrap_err().contains("地址为空"));
    }

    #[test]
    fn 非法请求方法报错() {
        assert!(request("不是一个方法", "http://x/", None).is_err());
    }

    #[test]
    fn 连不上的地址报错而非中断() {
        // 端口 1 通常无服务：应返回错误值 + success 为假的字典
        let out = call(
            "httpRequest",
            vec![Object::string("GET"), Object::string("http://127.0.0.1:1/")],
        )
        .expect("不应以异常形式失败");
        let result = match &out[0] {
            Object::Dict(d) => d.clone(),
            other => panic!("应返回结果字典，实际 {}", other.kind()),
        };
        assert_eq!(get(&result, "success"), Some(Object::Bool(false)));
        assert!(matches!(out[1], Object::Error(_)), "第二项应是错误值");
    }

    #[test]
    fn 内建名与模块名符合加载约定() {
        for spec in specs() {
            assert_eq!(spec.ext_module, Some(MODULE));
            assert_eq!(spec.name, "httpRequest");
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
        assert!(rt.lookup_native("httpRequest").is_some());
    }
}
