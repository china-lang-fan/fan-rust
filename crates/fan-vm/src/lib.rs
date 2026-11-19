//! 凡语言栈式字节码虚拟机。
//!
//! 输入是 fan-code 编译产出的 [`ModuleChunk`]，本 crate 不依赖 parser/AST，只做
//! 取指-译码-执行。运行时值复用 fan-object 的 `Object`，原生内建与 FFI 通过
//! `Runtime` 的原生表接入，VM 不重复实现宿主能力。
//!
//! ```text
//! fan-code（编译）→ ModuleChunk → fan-vm（本 crate，执行）
//! ```

mod error;
mod frame;
mod ops;
mod value;
mod vm;
mod vm_function;

pub use error::{VmError, VmResult};
pub use vm::Vm;

#[cfg(test)]
mod tests {
    use super::*;
    use fan_code::compiler::compile_program;
    use fan_object::Runtime;
    use fan_parser::parse_program;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn run_source(source: &str) -> Result<fan_object::Object, VmError> {
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "测试源码不应有解析错误：{errors:?}");
        let module = compile_program(&program).expect("编译应成功");
        let runtime = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = Vm::new(&module, runtime);
        vm.run()
    }

    #[test]
    fn 字面量与算术运算() {
        assert_eq!(
            run_source("1 加 2 乘 3").unwrap(),
            fan_object::Object::Int(7)
        );
    }

    #[test]
    fn 变量声明赋值与读取() {
        let src = "变量 甲 = 10\n甲 = 甲 加 5\n甲";
        assert_eq!(run_source(src).unwrap(), fan_object::Object::Int(15));
    }

    #[test]
    fn 条件语句选择分支() {
        let src =
            "变量 甲 = 3\n变量 乙 = 0\n如果 甲 大于 2 那么\n  乙 = 100\n否则\n  乙 = 1\n结束\n乙";
        assert_eq!(run_source(src).unwrap(), fan_object::Object::Int(100));
    }

    #[test]
    fn while循环累加() {
        let src = "变量 和 = 0\n变量 序号 = 1\n当 序号 小于等于 5 循环\n  和 = 和 加 序号\n  序号 = 序号 加 1\n结束\n和";
        assert_eq!(run_source(src).unwrap(), fan_object::Object::Int(15));
    }

    #[test]
    fn 自定义函数调用与返回() {
        let src = "函数 加倍(整数 n) -> 整数\n  返回 n 乘 2\n结束\n加倍(21)";
        assert_eq!(run_source(src).unwrap(), fan_object::Object::Int(42));
    }

    #[test]
    fn 函数多语句修改局部变量() {
        let src =
            "函数 计算(整数 n) -> 整数\n  变量 甲 = n\n  甲 = 甲 加 1\n  返回 甲\n结束\n计算(41)";
        assert_eq!(run_source(src).unwrap(), fan_object::Object::Int(42));
    }
}
