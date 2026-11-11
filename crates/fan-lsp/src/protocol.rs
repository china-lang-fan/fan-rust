//! LSP 3.17 子集的协议结构体。
//!
//! 仅声明本服务器实际收发的字段；`id` 用 `serde_json::Value` 保留客户端
//! 原始形态（数字或字符串），通知没有 id 时省略该字段。

use serde_json::Value;

/// 一个 JSON-RPC 2.0 消息（请求、响应或通知共用）。
#[derive(Debug, Clone)]
pub struct JsonRpcMessage {
    /// 请求 id；通知为 `None`，序列化时省略。
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Value,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 错误对象。
#[derive(Debug, Clone)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

/// LSP 位置（0-based，character 为 UTF-16 码元）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

/// LSP 区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// 诊断严重级别（LSP 约定数值）。
pub mod severity {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
    pub const INFORMATION: u8 = 3;
    pub const HINT: u8 = 4;
}
