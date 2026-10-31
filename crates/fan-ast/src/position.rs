//! 源码位置信息。
//!
//! 与 Go 版 `ast.Position` 一一对应，行与列都从 1 开始计数。

/// 源码中的一个位置（行、列均从 1 开始）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    /// 起始位置（第 1 行第 1 列）。
    pub const START: Position = Position { line: 1, column: 1 };

    pub fn new(line: usize, column: usize) -> Self {
        Position { line, column }
    }
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 起始位置是第一行第一列() {
        assert_eq!(Position::START, Position::new(1, 1));
    }

    #[test]
    fn 显示格式为行列号() {
        assert_eq!(Position::new(3, 7).to_string(), "3:7");
    }

    #[test]
    fn 默认位置是零值() {
        // 与 Go 版 int 零值一致：未记录位置时行列为 0。
        assert_eq!(Position::default(), Position::new(0, 0));
        assert_ne!(Position::default(), Position::START);
    }

    #[test]
    fn 位置可比较且可拷贝() {
        let a = Position::new(2, 5);
        let b = a;
        assert_eq!(a, b);
        assert_ne!(a, Position::new(2, 6));
    }
}
