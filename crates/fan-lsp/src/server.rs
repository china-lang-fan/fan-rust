//! LSP 服务器主循环、文档生命周期与防抖诊断推送。
//!
//! 零异步依赖：读循环跑在调用线程，防抖计时器跑在独立线程，两者通过
//! `std::sync::mpsc` 通信；所有输出经共享 `Mutex` 串行化，任何 panic 都被
//! 捕获并转成 JSON-RPC `-32603` 错误，避免杀死整个语言服务器。

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Map, Value};

use fan_analysis::symbol::SymbolKind;
use fan_analysis::utf16::utf16_column;
use fan_analysis::{analyze_index, Diagnostic, Index, Severity};

use crate::protocol::{completion_kind, error_code, severity, PUBLISH_DEBOUNCE_MS};
use crate::protocol::{JsonRpcError, JsonRpcMessage};
use crate::transport::{message_to_json, read_message, write_message};

/// 可在多线程间共享的输出句柄。
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// 一个已打开的文档。
struct OpenDocument {
    #[allow(dead_code)]
    uri: String,
    version: i64,
    text: String,
    index: Index,
}

/// 防抖线程的指令。
enum Debounce {
    /// 安排一次推送（uri、version）；连续安排会重置计时器。
    Schedule(String, i64),
    /// 立即停止防抖线程。
    Stop,
}

/// LSP 服务器。
pub struct Server {
    docs: Arc<Mutex<HashMap<String, OpenDocument>>>,
    writer: Option<SharedWriter>,
    schedule_tx: Sender<Debounce>,
    schedule_rx: Option<Receiver<Debounce>>,
    debouncer: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// 创建服务器（不启动读循环）。
    pub fn new() -> Self {
        let docs = Arc::new(Mutex::new(HashMap::new()));
        let (schedule_tx, schedule_rx) = channel();
        Server {
            docs,
            writer: None,
            schedule_tx,
            schedule_rx: Some(schedule_rx),
            debouncer: None,
        }
    }

    /// 从 `reader` 读取并处理消息，直到 `exit` 或 EOF。
    pub fn run<R: BufRead, W: Write + Send + 'static>(
        &mut self,
        reader: &mut R,
        writer: W,
    ) -> std::io::Result<()> {
        let writer: SharedWriter = Arc::new(Mutex::new(Box::new(writer)));
        self.writer = Some(writer.clone());
        if let Some(rx) = self.schedule_rx.take() {
            self.debouncer = Some(spawn_debouncer(self.docs.clone(), writer.clone(), rx));
        }
        loop {
            let Some(msg) = read_message(reader)? else {
                return Ok(());
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            if method == "exit" {
                return Ok(());
            }
            self.handle_safe(msg);
        }
    }

    /// 保证任何带 id 的请求都有响应：panic 时回 `-32603`。
    fn handle_safe(&self, raw: Value) {
        let id = raw.get("id").cloned();
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(&raw)));
        if let Err(payload) = outcome {
            if let Some(id) = id {
                let message = panic_message(&payload);
                let err = JsonRpcMessage {
                    id: Some(id),
                    method: None,
                    params: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: error_code::INTERNAL_ERROR,
                        message: format!("内部错误：{message}"),
                    }),
                };
                let _ = self.send(&err);
            }
        }
    }

    /// 串行化写出一条 JSON-RPC 消息。
    fn send(&self, msg: &JsonRpcMessage) -> std::io::Result<()> {
        let Some(writer) = &self.writer else {
            return Ok(());
        };
        let value = message_to_json(msg);
        let mut out = writer.lock().unwrap();
        write_message(&mut *out, &value)
    }

    /// 安排一次防抖诊断推送。
    fn schedule_publish(&self, uri: &str, version: i64) {
        let _ = self
            .schedule_tx
            .send(Debounce::Schedule(uri.to_string(), version));
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.schedule_tx.send(Debounce::Stop);
        if let Some(handle) = self.debouncer.take() {
            let _ = handle.join();
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "未知 panic".to_string()
}

/// 拉起防抖线程：收到 Schedule 后等待 250ms，期间无新安排则推送诊断。
fn spawn_debouncer(
    docs: Arc<Mutex<HashMap<String, OpenDocument>>>,
    writer: SharedWriter,
    rx: Receiver<Debounce>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut pending: Option<(String, i64)> = None;
        loop {
            let cmd = match &pending {
                Some(_) => rx.recv_timeout(Duration::from_millis(PUBLISH_DEBOUNCE_MS)),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match cmd {
                Ok(Debounce::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Debounce::Schedule(uri, version)) => {
                    pending = Some((uri, version));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((uri, version)) = pending.take() {
                        publish_diagnostics(&docs, &writer, &uri, version);
                    }
                }
            }
        }
    })
}

/// 计算并推送一个文档的诊断（在防抖线程中执行）。
fn publish_diagnostics(
    docs: &Arc<Mutex<HashMap<String, OpenDocument>>>,
    writer: &SharedWriter,
    uri: &str,
    version: i64,
) {
    let (text, diags, actual_version) = {
        let guard = docs.lock().unwrap();
        let Some(doc) = guard.get(uri) else {
            return;
        };
        let raw = analyze_index(&doc.index);
        let version = if version == 0 { doc.version } else { version };
        (doc.text.clone(), raw, version)
    };
    let lsp_diags = to_lsp_diagnostics(&text, &diags);
    let mut params = Map::new();
    params.insert("uri".to_string(), Value::String(uri.to_string()));
    if actual_version != 0 {
        params.insert("version".to_string(), Value::from(actual_version));
    }
    params.insert("diagnostics".to_string(), lsp_diags);
    let msg = JsonRpcMessage {
        id: None,
        method: Some("textDocument/publishDiagnostics".to_string()),
        params: Value::Object(params),
        result: None,
        error: None,
    };
    let value = message_to_json(&msg);
    let mut out = writer.lock().unwrap();
    let _ = write_message(&mut *out, &value);
}

/// 把分析诊断转换为 LSP 诊断 JSON（列号转 UTF-16）。
fn to_lsp_diagnostics(source: &str, diags: &[Diagnostic]) -> Value {
    let lines: Vec<&str> = source.split('\n').collect();
    let items: Vec<Value> = diags
        .iter()
        .map(|d| {
            let line_text = lines.get(d.line.wrapping_sub(1)).copied().unwrap_or("");
            let start_char = utf16_column(line_text, d.column);
            let mut end_char = utf16_column(line_text, d.end_column);
            if end_char < start_char {
                end_char = start_char + 1;
            }
            let sev = match d.severity {
                Severity::Error => severity::ERROR,
                Severity::Warning => severity::WARNING,
                Severity::Information => severity::INFORMATION,
                Severity::Hint => severity::HINT,
            };
            json!({
                "range": {
                    "start": { "line": d.line - 1, "character": start_char },
                    "end": { "line": d.end_line - 1, "character": end_char },
                },
                "severity": sev,
                "code": d.code,
                "source": "fan",
                "message": d.message,
            })
        })
        .collect();
    Value::Array(items)
}

/// 把分析层符号种类映射为 LSP CompletionItemKind。
fn to_completion_kind(kind: SymbolKind) -> i64 {
    match kind {
        SymbolKind::Function | SymbolKind::MixfixFunction => completion_kind::FUNCTION,
        SymbolKind::Method => completion_kind::METHOD,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Type => completion_kind::INTERFACE,
        SymbolKind::Field => completion_kind::FIELD,
        SymbolKind::Variable | SymbolKind::Parameter => completion_kind::VARIABLE,
        SymbolKind::Constant => completion_kind::CONSTANT,
        SymbolKind::Module => completion_kind::MODULE,
    }
}

impl Server {
    /// 按 method 分发一条消息（通知无响应，请求必回 result 或 error）。
    fn dispatch(&self, raw: &Value) {
        let method = raw.get("method").and_then(Value::as_str).unwrap_or("");
        let id = raw.get("id").cloned();
        let params = raw.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => self.respond_initialize(id),
            "initialized" | "$/setTrace" | "$/cancelRequest" => {}
            "shutdown" => {
                let _ = self.respond(id, Value::Null);
            }
            "textDocument/didOpen" => self.on_did_open(&params),
            "textDocument/didChange" => self.on_did_change(&params),
            "textDocument/didSave" => self.on_did_save(&params),
            "textDocument/didClose" => self.on_did_close(&params),
            "textDocument/hover" => self.on_hover(id, &params),
            "textDocument/definition" => self.on_definition(id, &params),
            "textDocument/completion" => self.on_completion(id, &params),
            _ => {
                if let Some(id) = id {
                    let err = JsonRpcMessage {
                        id: Some(id),
                        method: None,
                        params: Value::Null,
                        result: None,
                        error: Some(JsonRpcError {
                            code: error_code::METHOD_NOT_FOUND,
                            message: format!("方法未实现：{method}"),
                        }),
                    };
                    let _ = self.send(&err);
                }
            }
        }
    }

    fn respond(&self, id: Option<Value>, result: Value) -> std::io::Result<()> {
        let Some(id) = id else { return Ok(()) };
        self.send(&JsonRpcMessage {
            id: Some(id),
            method: None,
            params: Value::Null,
            result: Some(result),
            error: None,
        })
    }

    fn respond_initialize(&self, id: Option<Value>) {
        let result = json!({
            "capabilities": {
                "textDocumentSync": { "openClose": true, "change": 1 },
                "definitionProvider": true,
                "hoverProvider": true,
                "completionProvider": { "triggerCharacters": [".", "的"] },
            },
            "serverInfo": { "name": crate::SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        });
        let _ = self.respond(id, result);
    }

    fn on_did_open(&self, params: &Value) {
        let Some(td) = params.get("textDocument") else {
            return;
        };
        let uri = td
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let version = td.get("version").and_then(Value::as_i64).unwrap_or(0);
        let text = td
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.set_document(&uri, version, &text);
        // didOpen 同步推送一次诊断。
        if let Some(writer) = &self.writer {
            publish_diagnostics(&self.docs, writer, &uri, version);
        }
    }

    fn on_did_change(&self, params: &Value) {
        let Some(td) = params.get("textDocument") else {
            return;
        };
        let uri = td
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let version = td.get("version").and_then(Value::as_i64).unwrap_or(0);
        let Some(changes) = params.get("contentChanges").and_then(Value::as_array) else {
            return;
        };
        let Some(last) = changes.last() else { return };
        let text = last
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        self.set_document(&uri, version, &text);
        self.schedule_publish(&uri, version);
    }

    fn on_did_save(&self, params: &Value) {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !uri.is_empty() {
            self.schedule_publish(&uri, 0);
        }
    }

    fn on_did_close(&self, params: &Value) {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if uri.is_empty() {
            return;
        }
        self.docs.lock().unwrap().remove(&uri);
        let mut map = Map::new();
        map.insert("uri".to_string(), Value::String(uri));
        map.insert("diagnostics".to_string(), Value::Array(vec![]));
        let _ = self.send(&JsonRpcMessage {
            id: None,
            method: Some("textDocument/publishDiagnostics".to_string()),
            params: Value::Object(map),
            result: None,
            error: None,
        });
    }

    fn on_hover(&self, id: Option<Value>, params: &Value) {
        let result = self.with_position(params, |index, line, character| {
            index
                .hover(line, character)
                .map(|h| {
                    json!({
                        "contents": { "kind": "markdown", "value": h.content },
                        "range": range_to_json(h.range),
                    })
                })
                .unwrap_or(Value::Null)
        });
        let _ = self.respond(id, result.unwrap_or(Value::Null));
    }

    fn on_definition(&self, id: Option<Value>, params: &Value) {
        let result = self.with_position(params, |index, line, character| {
            index
                .definition(line, character)
                .map(|loc| {
                    json!({
                        "uri": loc.uri,
                        "range": range_to_json(loc.range),
                    })
                })
                .unwrap_or(Value::Null)
        });
        let _ = self.respond(id, result.unwrap_or(Value::Null));
    }

    fn on_completion(&self, id: Option<Value>, params: &Value) {
        let result = self.with_position(params, |index, line, character| {
            let items: Vec<Value> = index
                .completion(line, character)
                .into_iter()
                .map(|item| {
                    let kind = if item.detail == "关键字" {
                        completion_kind::TEXT
                    } else {
                        to_completion_kind(item.kind)
                    };
                    let mut obj = Map::new();
                    obj.insert("label".to_string(), Value::String(item.label));
                    obj.insert("kind".to_string(), Value::from(kind));
                    if !item.detail.is_empty() {
                        obj.insert("detail".to_string(), Value::String(item.detail));
                    }
                    if !item.doc.is_empty() {
                        obj.insert(
                            "documentation".to_string(),
                            json!({ "kind": "markdown", "value": item.doc }),
                        );
                    }
                    Value::Object(obj)
                })
                .collect();
            json!({ "isIncomplete": false, "items": items })
        });
        let _ = self.respond(
            id,
            result.unwrap_or_else(|| json!({ "isIncomplete": false, "items": [] })),
        );
    }

    fn set_document(&self, uri: &str, version: i64, text: &str) {
        let index = Index::build(uri, text);
        self.docs.lock().unwrap().insert(
            uri.to_string(),
            OpenDocument {
                uri: uri.to_string(),
                version,
                text: text.to_string(),
                index,
            },
        );
    }

    /// 解析 textDocument/position 参数并在对应文档索引上执行查询。
    fn with_position<F>(&self, params: &Value, f: F) -> Option<Value>
    where
        F: FnOnce(&Index, usize, usize) -> Value,
    {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)?;
        let line = params.pointer("/position/line").and_then(Value::as_i64)? as usize;
        let character = params
            .pointer("/position/character")
            .and_then(Value::as_i64)? as usize;
        let guard = self.docs.lock().unwrap();
        let doc = guard.get(uri)?;
        Some(f(&doc.index, line, character))
    }
}

fn range_to_json(range: fan_analysis::query::Range) -> Value {
    json!({
        "start": { "line": range.start.line, "character": range.start.character },
        "end": { "line": range.end.line, "character": range.end.character },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 构造一帧 LSP 消息（id 为 None 表示通知）。
    fn frame(method: &str, id: Option<Value>, params: Value) -> String {
        let mut msg = Map::new();
        msg.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        msg.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(id) = id {
            msg.insert("id".to_string(), id);
        }
        msg.insert("params".to_string(), params);
        let body = serde_json::to_vec(&Value::Object(msg)).unwrap();
        format!(
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8(body).unwrap()
        )
    }

    /// 从分帧输出中解析出全部 JSON 消息体。
    fn read_frames(data: &str) -> Vec<Value> {
        let bytes = data.as_bytes();
        let mut cursor = std::io::Cursor::new(bytes);
        let mut out = Vec::new();
        while let Ok(Some(v)) = read_message(&mut cursor) {
            out.push(v);
        }
        out
    }

    #[test]
    fn initialize_与_did_open_推送诊断() {
        let source = "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n    返回 值\n结束\n\n变量 x = 修改(\"x\") 的状态为(\"y\")\n";
        let mut input = String::new();
        input.push_str(&frame(
            "initialize",
            Some(json!(1)),
            json!({ "capabilities": {} }),
        ));
        input.push_str(&frame("initialized", None, json!({})));
        input.push_str(&frame(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///test.凡", "languageId": "fan", "version": 1, "text": source } }),
        ));
        input.push_str(&frame("exit", None, Value::Null));

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let mut reader = Cursor::new(input.into_bytes());
            let mut server = Server::new();
            server.run(&mut reader, SharedVec(output.clone())).unwrap();
        }
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let frames = read_frames(&data);

        let got_init = frames
            .iter()
            .any(|m| m.get("id") == Some(&json!(1)) && m.get("error").is_none());
        assert!(got_init, "未收到 initialize 成功响应：{data}");

        let got_mixfix = frames.iter().any(|m| {
            m.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && data.contains("mixfix-space")
        });
        assert!(got_mixfix, "未收到 mixfix-space 诊断：{data}");
    }

    #[test]
    fn hover_返回签名与文档() {
        let source = "# 打招呼函数\n函数 打招呼(字符串 名字) -> 字符串\n    返回 名字\n结束\n\n变量 x = 打招呼(\"张三\")\n";
        let mut input = String::new();
        input.push_str(&frame("initialize", Some(json!(1)), json!({})));
        input.push_str(&frame(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///t.凡", "languageId": "fan", "version": 1, "text": source } }),
        ));
        input.push_str(&frame(
            "textDocument/hover",
            Some(json!(2)),
            json!({ "textDocument": { "uri": "file:///t.凡" }, "position": { "line": 5, "character": 7 } }),
        ));
        input.push_str(&frame("exit", None, Value::Null));

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let mut reader = Cursor::new(input.into_bytes());
            let mut server = Server::new();
            server.run(&mut reader, SharedVec(output.clone())).unwrap();
        }
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let frames = read_frames(&data);
        let hover = frames
            .iter()
            .find(|m| m.get("id") == Some(&json!(2)))
            .expect("未收到 hover 响应");
        let value = hover
            .get("result")
            .and_then(|r| r.pointer("/contents/value"))
            .map(|v| v.to_string())
            .unwrap_or_default();
        assert!(
            value.contains("打招呼函数") && value.contains("打招呼"),
            "实际：{value}"
        );
    }

    #[test]
    fn 空白处悬停显式返回_null() {
        let source = "变量 x = 1\n";
        let mut input = String::new();
        input.push_str(&frame("initialize", Some(json!(1)), json!({})));
        input.push_str(&frame(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///t.凡", "languageId": "fan", "version": 1, "text": source } }),
        ));
        input.push_str(&frame(
            "textDocument/hover",
            Some(json!(2)),
            json!({ "textDocument": { "uri": "file:///t.凡" }, "position": { "line": 0, "character": 0 } }),
        ));
        input.push_str(&frame("exit", None, Value::Null));

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let mut reader = Cursor::new(input.into_bytes());
            let mut server = Server::new();
            server.run(&mut reader, SharedVec(output.clone())).unwrap();
        }
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let frames = read_frames(&data);
        let hover = frames
            .iter()
            .find(|m| m.get("id") == Some(&json!(2)))
            .expect("未收到 hover 响应");
        assert_eq!(
            hover.get("result"),
            Some(&Value::Null),
            "无符号处应返回 null"
        );
        assert!(
            data.contains("\"result\":null"),
            "响应必须显式包含 result:null：{data}"
        );
    }

    #[test]
    fn did_change_防抖后推送诊断() {
        let source = "变量 x = 1\n";
        let mut input = String::new();
        input.push_str(&frame("initialize", Some(json!(1)), json!({})));
        input.push_str(&frame(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///d.凡", "languageId": "fan", "version": 1, "text": source } }),
        ));
        let bad = "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n    返回 值\n结束\n变量 x = 修改(\"a\") 的状态为(\"b\")\n";
        input.push_str(&frame(
            "textDocument/didChange",
            None,
            json!({ "textDocument": { "uri": "file:///d.凡", "version": 2 }, "contentChanges": [ { "text": bad } ] }),
        ));

        // 用管道保持输入不 EOF，服务器得以存活到防抖计时器触发。
        let (mut pipe_w, pipe_r) = std::os::unix::net::UnixStream::pair().unwrap();
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let out = SharedVec(output.clone());
        let handle = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(pipe_r);
            let mut server = Server::new();
            server.run(&mut reader, out).unwrap();
        });
        pipe_w.write_all(input.as_bytes()).unwrap();
        // 等待防抖推送（250ms + 余量），再发 exit 让服务器干净退出。
        std::thread::sleep(Duration::from_millis(450));
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(
            data.contains("mixfix-space"),
            "didChange 防抖后应推送诊断：{data}"
        );
        pipe_w
            .write_all(frame("exit", None, Value::Null).as_bytes())
            .unwrap();
        drop(pipe_w);
        handle.join().unwrap();
    }

    #[test]
    fn completion_返回顶层符号() {
        let source = "函数 打招呼(字符串 名字) -> 字符串\n    返回 名字\n结束\n";
        let mut input = String::new();
        input.push_str(&frame("initialize", Some(json!(1)), json!({})));
        input.push_str(&frame(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///c.凡", "languageId": "fan", "version": 1, "text": source } }),
        ));
        input.push_str(&frame(
            "textDocument/completion",
            Some(json!(3)),
            json!({ "textDocument": { "uri": "file:///c.凡" }, "position": { "line": 2, "character": 0 } }),
        ));
        input.push_str(&frame("exit", None, Value::Null));

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let mut reader = Cursor::new(input.into_bytes());
            let mut server = Server::new();
            server.run(&mut reader, SharedVec(output.clone())).unwrap();
        }
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let frames = read_frames(&data);
        let comp = frames
            .iter()
            .find(|m| m.get("id") == Some(&json!(3)))
            .expect("未收到 completion 响应");
        let labels = comp
            .pointer("/result/items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.get("label").and_then(Value::as_str).map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert!(labels.contains(&"打招呼".to_string()), "实际：{labels:?}");
        assert!(
            labels.contains(&"函数".to_string()),
            "应包含关键字：{labels:?}"
        );
    }

    #[test]
    fn 未知方法回_method_not_found() {
        let mut input = String::new();
        input.push_str(&frame(
            "textDocument/doesNotExist",
            Some(json!(7)),
            json!({}),
        ));
        input.push_str(&frame("exit", None, Value::Null));

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let mut reader = Cursor::new(input.into_bytes());
            let mut server = Server::new();
            server.run(&mut reader, SharedVec(output.clone())).unwrap();
        }
        let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let frames = read_frames(&data);
        let err = frames
            .iter()
            .find(|m| m.get("id") == Some(&json!(7)))
            .expect("未收到响应");
        assert_eq!(err.pointer("/error/code"), Some(&json!(-32601)));
    }

    /// 共享输出缓冲，满足 `Write + Send + 'static`。
    struct SharedVec(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedVec {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
