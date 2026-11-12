//! 凡语言求值器。
//!
//! 把抽象语法树求值成运行时对象，位于流水线中段：
//!
//! ```text
//! fan-ast → fan-lexer → fan-parser → fan-object → fan-eval → fan-native → fan-cli
//! ```

pub mod error;
pub mod eval;
pub mod module;
pub mod oop;
pub mod operators;
pub mod tag;

pub use error::{EvalResult, FanError, Flow};
pub use eval::Interpreter;
pub use module::{default_modules_dirs, module_name_from_path, Loader};
pub use oop::{convert_to_named, convert_to_primitive_kind, value_is_type};

use fan_ast::Program;
use fan_object::Object;

/// 便捷入口：在全新环境中执行已解析好的程序。
pub fn run_program(program: &Program) -> Result<Object, FanError> {
    Interpreter::new().run(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;

    #[test]
    fn 便捷入口可执行程序() {
        let (program, errors) = parse_program("变量 甲 = 1 加 2\n甲");
        assert!(errors.is_empty());
        assert_eq!(run_program(&program).unwrap().inspect(), "3");
    }

    #[test]
    fn 空程序返回空值() {
        let (program, _) = parse_program("");
        assert_eq!(run_program(&program).unwrap(), Object::Nil);
    }
}
