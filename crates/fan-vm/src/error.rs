//! 虚拟机运行错误。
//!
//! M0 阶段使用独立的错误类型，仅描述字节码执行层面的问题；后续与树遍历
//! 求值器统一对外报错口径时，再映射到 fan-eval 的 `FanError`。

use std::fmt;

/// VM 执行结果。
pub type VmResult<T> = Result<T, VmError>;

#[derive(Debug, Clone, PartialEq)]
pub struct VmError {
    pub message: String,
    /// 出错指令在模块块中的字节偏移（若可定位）。
    pub offset: Option<usize>,
    /// 出错指令对应的源码行（若行号表可定位）。
    pub line: Option<u32>,
}

impl VmError {
    pub fn new(message: impl Into<String>) -> Self {
        VmError {
            message: message.into(),
            offset: None,
            line: None,
        }
    }

    pub fn at(mut self, offset: usize, line: Option<u32>) -> Self {
        self.offset = Some(offset);
        self.line = line;
        self
    }
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.offset, self.line) {
            (Some(off), Some(line)) => write!(
                f,
                "字节码错误（偏移 {off}，第 {line} 行）：{}",
                self.message
            ),
            (Some(off), None) => write!(f, "字节码错误（偏移 {off}）：{}", self.message),
            _ => write!(f, "字节码错误：{}", self.message),
        }
    }
}

impl std::error::Error for VmError {}
