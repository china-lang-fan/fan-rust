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
    use fan_object::{Object, Runtime};
    use fan_parser::parse_program;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn run_source(source: &str) -> Result<Object, VmError> {
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "测试源码不应有解析错误：{errors:?}");
        let module = compile_program(&program).expect("编译应成功");
        let runtime = Rc::new(RefCell::new(Runtime::new()));
        let mut vm = Vm::new(&module, runtime);
        vm.run()
    }

    fn run_with_natives(source: &str) -> Result<Object, VmError> {
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "测试源码不应有解析错误：{errors:?}");
        let module = compile_program(&program).expect("编译应成功");
        let runtime = Rc::new(RefCell::new(Runtime::new()));
        fan_native::register_natives(&mut runtime.borrow_mut());
        let mut vm = Vm::new(&module, runtime);
        vm.run()
    }

    #[test]
    fn 字面量与算术运算() {
        assert_eq!(run_source("1 加 2 乘 3").unwrap(), Object::Int(7));
    }

    #[test]
    fn 变量声明赋值与读取() {
        let src = "变量 甲 = 10\n甲 = 甲 加 5\n甲";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 条件语句选择分支() {
        let src =
            "变量 甲 = 3\n变量 乙 = 0\n如果 甲 大于 2 那么\n  乙 = 100\n否则\n  乙 = 1\n结束\n乙";
        assert_eq!(run_source(src).unwrap(), Object::Int(100));
    }

    #[test]
    fn while循环累加() {
        let src = "变量 和 = 0\n变量 序号 = 1\n当 序号 小于等于 5 循环\n  和 = 和 加 序号\n  序号 = 序号 加 1\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 自定义函数调用与返回() {
        let src = "函数 加倍(整数 n) -> 整数\n  返回 n 乘 2\n结束\n加倍(21)";
        assert_eq!(run_source(src).unwrap(), Object::Int(42));
    }

    #[test]
    fn 函数多语句修改局部变量() {
        let src =
            "函数 计算(整数 n) -> 整数\n  变量 甲 = n\n  甲 = 甲 加 1\n  返回 甲\n结束\n计算(41)";
        assert_eq!(run_source(src).unwrap(), Object::Int(42));
    }

    #[test]
    fn 短路逻辑不求值失效分支() {
        // 触发() 被调用即除零报错；短路时该函数不应被调用。
        let prelude = "函数 触发() -> 整数\n  变量 甲 = 1\n  变量 乙 = 0\n  返回 甲 / 乙\n结束\n";
        let src_false = format!("{prelude}假 且 触发()");
        assert_eq!(run_source(&src_false).unwrap(), Object::Bool(false));
        let src_true = format!("{prelude}真 或 触发()");
        assert_eq!(run_source(&src_true).unwrap(), Object::Bool(true));
        // 非短路时右侧确实被求值（除零报错）。
        let src_eval = format!("{prelude}真 且 触发()");
        assert!(run_source(&src_eval).is_err());
    }

    #[test]
    fn repeat后判断循环至少执行一次() {
        let src = "变量 甲 = 0\n重复\n  甲 = 甲 加 1\n直到 甲 大于等于 3 结束\n甲";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
    }

    #[test]
    fn times计数循环执行固定次数() {
        let src = "变量 和 = 0\n重复 5 次\n  和 = 和 加 2\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(10));
    }

    #[test]
    fn 函数多返回值解构() {
        let src =
            "函数 交换(整数 a, 整数 b)\n  返回 b、a\n结束\n变量 甲、乙 = 交换(1, 2)\n甲 加 乙";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
    }

    #[test]
    fn 多目标赋值解构元组() {
        let src = "变量 甲 = 0\n变量 乙 = 0\n甲、乙 = 7、8\n甲 乘 10 加 乙";
        assert_eq!(run_source(src).unwrap(), Object::Int(78));
    }

    #[test]
    fn 默认参数缺省时取默认值() {
        let src = "函数 加法(整数 a, 整数 b = 10)\n  返回 a 加 b\n结束\n加法(5)";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 默认参数显式传参覆盖默认值() {
        let src = "函数 加法(整数 a, 整数 b = 10)\n  返回 a 加 b\n结束\n加法(5, 6)";
        assert_eq!(run_source(src).unwrap(), Object::Int(11));
    }

    #[test]
    fn 多个默认参数逐个缺省() {
        let src =
            "函数 合计(整数 a, 整数 b = 100, 整数 c = 1000)\n  返回 a 加 b 加 c\n结束\n合计(1, 2)";
        assert_eq!(run_source(src).unwrap(), Object::Int(1003));
    }

    #[test]
    fn 可变参数收束为数组() {
        let src = "函数 收集(任意 头, 任意... 尾)\n  返回 尾\n结束\n收集(0, 1, 2, 3)";
        match run_source(src).unwrap() {
            Object::Array(items) => {
                assert_eq!(
                    *items.0.borrow(),
                    vec![Object::Int(1), Object::Int(2), Object::Int(3)]
                );
            }
            other => panic!("应为数组，实际为 {other:?}"),
        }
    }

    #[test]
    fn 可变参数无多余实参时为空数组() {
        let src = "函数 收集(任意 头, 任意... 尾)\n  返回 尾\n结束\n收集(9)";
        assert!(matches!(run_source(src).unwrap(), Object::Array(a) if a.0.borrow().is_empty()));
    }

    #[test]
    fn 参数个数不足且无默认值时报错() {
        let src = "函数 甲(任意 a, 任意 b)\n  返回 a\n结束\n甲(1)";
        assert!(run_source(src).is_err());
    }

    #[test]
    fn 非可变函数实参过多时报错() {
        let src = "函数 甲(任意 a)\n  返回 a\n结束\n甲(1, 2)";
        assert!(run_source(src).is_err());
    }

    #[test]
    fn 原生len求字符串与数组长度() {
        assert_eq!(run_with_natives("len(\"你好凡\")").unwrap(), Object::Int(3));
        let src = "函数 包装(任意... 尾)\n  返回 len(尾)\n结束\n包装(1, 2, 3, 4)";
        assert_eq!(run_with_natives(src).unwrap(), Object::Int(4));
    }

    #[test]
    fn 原生type返回运行时类型名() {
        assert_eq!(
            run_with_natives("type(42)").unwrap(),
            Object::string("integer")
        );
        assert_eq!(
            run_with_natives("type(\"x\")").unwrap(),
            Object::string("string")
        );
    }

    #[test]
    fn 原生函数参与凡语函数组合() {
        let src = "函数 双倍长度(任意 文本)\n  返回 len(文本) 乘 2\n结束\n双倍长度(\"abcd\")";
        assert_eq!(run_with_natives(src).unwrap(), Object::Int(8));
    }
}
