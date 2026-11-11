//! 凡语言语言服务器。
//!
//! 实现 Language Server Protocol（3.17 子集），通过 stdio 使用 JSON-RPC
//! 与编辑器通信。对应 Go 版 `internal/lsp`；静态分析能力委托给
//! [`fan_analysis`]，本 crate 只负责协议、分帧与文档生命周期。

/// LSP 协议结构体（serde 序列化）。
pub mod protocol;
/// stdio 上的 `Content-Length` 分帧读写。
pub mod transport;

pub use transport::{read_message, write_message};

/// 服务器名称，用于 `initialize` 响应的 `serverInfo`。
pub const SERVER_NAME: &str = "fan-lsp";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_可编译() {
        assert_eq!(super::SERVER_NAME, "fan-lsp");
    }
}
