//! stdio 上的 LSP 分帧：`Content-Length` 头 + 空行 + JSON 体。

use std::io::{self, BufRead, Write};

use serde_json::Value;

use crate::protocol::JsonRpcMessage;

/// 从输入读取一帧，解析为原始 JSON 值。输入结束（EOF）返回 `Ok(None)`。
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        let read = reader.read_line(&mut header)?;
        if read == 0 {
            return Ok(None);
        }
        let line = header.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            );
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "缺少 Content-Length 头"))?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

/// 把一个 JSON 值按 LSP 分帧写出。
pub fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// 把消息序列化为 JSON-RPC 2.0 JSON 值。
pub fn message_to_json(msg: &JsonRpcMessage) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    if let Some(id) = &msg.id {
        map.insert("id".to_string(), id.clone());
    }
    if let Some(method) = &msg.method {
        map.insert("method".to_string(), Value::String(method.clone()));
    }
    if msg.params != Value::Null || msg.method.is_some() {
        map.insert("params".to_string(), msg.params.clone());
    }
    if let Some(result) = &msg.result {
        map.insert("result".to_string(), result.clone());
    }
    if let Some(error) = &msg.error {
        let mut e = serde_json::Map::new();
        e.insert("code".to_string(), Value::from(error.code));
        e.insert("message".to_string(), Value::String(error.message.clone()));
        map.insert("error".to_string(), Value::Object(e));
    }
    Value::Object(map)
}
