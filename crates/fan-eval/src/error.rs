//! 求值错误与控制流信号。
//!
//! Go 版把 `跳出/继续/返回/抛出` 四种信号塞进 `error` 通道，靠类型断言分发。
//! Rust 里这属于反模式，这里改为**显式的控制流枚举**：
//! 只有真正的运行时错误才走 `Err`，控制流用 `Flow` 表达，编译期即可穷尽检查。

use fan_ast::Position;
use fan_object::StackFrame;
use std::fmt;

/// 运行时错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanError {
    /// 中文错误消息（与 Go 版措辞保持一致）。
    pub message: String,
    /// 出错位置。
    pub pos: Option<Position>,
    /// 出错时的调用栈。
    pub stack: Vec<StackFrame>,
}

impl FanError {
    pub fn new(message: impl Into<String>) -> Self {
        FanError {
            message: message.into(),
            pos: None,
            stack: Vec::new(),
        }
    }

    pub fn at(message: impl Into<String>, pos: Position) -> Self {
        FanError {
            message: message.into(),
            pos: Some(pos),
            stack: Vec::new(),
        }
    }

    pub fn with_stack(mut self, stack: Vec<StackFrame>) -> Self {
        if self.stack.is_empty() {
            self.stack = stack;
        }
        self
    }

    /// 带位置前缀的完整消息，形如 `第3行第7列：xxx`。
    pub fn display(&self) -> String {
        match self.pos {
            Some(p) => format!("第{}行第{}列：{}", p.line, p.column, self.message),
            None => self.message.clone(),
        }
    }
}

impl fmt::Display for FanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl std::error::Error for FanError {}

/// 控制流信号。
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    /// 正常求值结果。
    Value(fan_object::Object),
    /// `返回`，携带全部返回值。
    Return(Vec<fan_object::Object>),
    /// `跳出`。
    Break,
    /// `继续`。
    Continue,
    /// `抛出`，携带被抛出的值。
    Throw(fan_object::Object),
    /// `检查 调用` 命中的错误，携带错误值与检查位置，立即向当前函数边界冒泡。
    Check(fan_object::Object, Position),
}

impl Flow {
    /// 取出正常值；控制流信号转为错误。
    pub fn into_value(self) -> Result<fan_object::Object, FanError> {
        match self {
            Flow::Value(v) => Ok(v),
            Flow::Return(_) => Err(FanError::new("返回 不能出现在函数外")),
            Flow::Break => Err(FanError::new("跳出 不能出现在循环外")),
            Flow::Continue => Err(FanError::new("继续 不能出现在循环外")),
            Flow::Throw(v) => Err(FanError::new(format!("未捕获的抛出：{}", v.inspect()))),
            Flow::Check(v, _) => Err(FanError::new(format!("未捕获的检查错误：{}", v.inspect()))),
        }
    }

    /// 是否为需要立即向外传播的控制流信号。
    pub fn is_signal(&self) -> bool {
        !matches!(self, Flow::Value(_))
    }
}

/// 求值结果：控制流信号或运行时错误。
pub type EvalResult = Result<Flow, FanError>;

/// 构造一个正常的求值结果。
pub fn value(o: fan_object::Object) -> EvalResult {
    Ok(Flow::Value(o))
}

/// 构造一个错误结果。
pub fn error(message: impl Into<String>) -> EvalResult {
    Err(FanError::new(message))
}

/// 构造一个带位置的错误结果。
pub fn error_at(message: impl Into<String>, pos: Position) -> EvalResult {
    Err(FanError::at(message, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误消息带位置前缀() {
        let e = FanError::at("类型错误", Position::new(3, 7));
        assert_eq!(e.display(), "第3行第7列：类型错误");
        assert_eq!(e.to_string(), "第3行第7列：类型错误");
    }

    #[test]
    fn 无位置错误只有消息() {
        let e = FanError::new("类型错误");
        assert_eq!(e.display(), "类型错误");
        assert_eq!(e.pos, None);
    }

    #[test]
    fn 调用栈只填充一次() {
        let frame = StackFrame {
            function: "甲".to_string(),
            file: "a.凡".to_string(),
            line: 1,
            column: 1,
        };
        let e = FanError::new("错").with_stack(vec![frame]);
        assert_eq!(e.stack.len(), 1);
        // 已有栈时不再覆盖
        let e2 = e.clone().with_stack(vec![]);
        assert_eq!(e2.stack.len(), 1);
    }

    #[test]
    fn 控制流信号识别() {
        assert!(!Flow::Value(fan_object::Object::nil()).is_signal());
        assert!(Flow::Break.is_signal());
        assert!(Flow::Continue.is_signal());
        assert!(Flow::Return(vec![]).is_signal());
        assert!(Flow::Throw(fan_object::Object::nil()).is_signal());
    }

    #[test]
    fn 正常值可取出() {
        let f = Flow::Value(fan_object::Object::int(1));
        assert_eq!(f.into_value().unwrap(), fan_object::Object::int(1));
    }

    #[test]
    fn 循环外跳出报错() {
        assert_eq!(
            Flow::Break.into_value().unwrap_err().message,
            "跳出 不能出现在循环外"
        );
        assert_eq!(
            Flow::Continue.into_value().unwrap_err().message,
            "继续 不能出现在循环外"
        );
        assert_eq!(
            Flow::Return(vec![]).into_value().unwrap_err().message,
            "返回 不能出现在函数外"
        );
    }

    #[test]
    fn 未捕获抛出含值文本() {
        let f = Flow::Throw(fan_object::Object::string("崩溃"));
        assert_eq!(f.into_value().unwrap_err().message, "未捕获的抛出：崩溃");
    }

    #[test]
    fn 便捷构造结果() {
        assert!(value(fan_object::Object::int(1)).is_ok());
        assert!(error("失败").is_err());
        assert!(error_at("失败", Position::new(1, 1)).is_err());
    }
}
