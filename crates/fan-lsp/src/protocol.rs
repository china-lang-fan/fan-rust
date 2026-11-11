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

/// LSP CompletionItemKind 数值（仅列本服务器用到的）。
pub mod completion_kind {
    pub const TEXT: i64 = 1;
    pub const METHOD: i64 = 2;
    pub const FUNCTION: i64 = 3;
    pub const FIELD: i64 = 5;
    pub const VARIABLE: i64 = 6;
    pub const CLASS: i64 = 7;
    pub const INTERFACE: i64 = 8;
    pub const MODULE: i64 = 9;
    pub const PARAMETER: i64 = 14;
    pub const CONSTANT: i64 = 21;
}

/// JSON-RPC 预定义错误码。
pub mod error_code {
    /// 方法不存在或未实现。
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// 内部错误（panic 兜底）。
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// 诊断防抖间隔（毫秒）。
pub const PUBLISH_DEBOUNCE_MS: u64 = 250;
