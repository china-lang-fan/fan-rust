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

    #[test]
    fn 数组字面量与下标读取() {
        let src = "变量 组 = [10, 20, 30]\n组[1]";
        assert_eq!(run_source(src).unwrap(), Object::Int(20));
    }

    #[test]
    fn 数组下标赋值就地修改() {
        let src = "变量 组 = [1, 2, 3]\n组[1] = 99\n组[1]";
        assert_eq!(run_source(src).unwrap(), Object::Int(99));
    }

    #[test]
    fn 数组越界读取报错() {
        assert!(run_source("变量 组 = [1]\n组[5]").is_err());
    }

    #[test]
    fn 字典成员访问与下标等价() {
        let src = "变量 表 = {\"宽\": 3, \"高\": 4}\n表.宽";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
        let src2 = "变量 表 = {\"宽\": 3, \"高\": 4}\n表 的 高";
        assert_eq!(run_source(src2).unwrap(), Object::Int(4));
    }

    #[test]
    fn 字典成员赋值() {
        let src = "变量 表 = {\"宽\": 3}\n表.宽 = 30\n表[\"宽\"]";
        assert_eq!(run_source(src).unwrap(), Object::Int(30));
        let src2 = "变量 表 = {}\n表.新键 = 7\n表.新键";
        assert_eq!(run_source(src2).unwrap(), Object::Int(7));
    }

    #[test]
    fn 字典缺失成员报错() {
        assert!(run_source("变量 表 = {}\n表.无").is_err());
    }

    #[test]
    fn 成员访问参与运算与函数() {
        let src = "变量 表 = {\"宽\": 3, \"高\": 4}\n表.宽 乘 表.高";
        assert_eq!(run_source(src).unwrap(), Object::Int(12));
        let src2 = "函数 面积(任意 表)\n  返回 表.宽 乘 表.高\n结束\n面积({\"宽\": 5, \"高\": 6})";
        assert_eq!(run_source(src2).unwrap(), Object::Int(30));
    }

    #[test]
    fn 非容器成员访问报错() {
        assert!(run_source("变量 甲 = 1\n甲.字段").is_err());
    }

    #[test]
    fn 数组与字符串切片() {
        let src = "变量 组 = [1, 2, 3, 4]\n组[1:3]";
        match run_source(src).unwrap() {
            Object::Array(a) => assert_eq!(*a.0.borrow(), vec![Object::Int(2), Object::Int(3)]),
            other => panic!("应为数组，实际 {other:?}"),
        }
        assert_eq!(
            run_source("\"凡语言编程\"[1:3]").unwrap(),
            Object::string("语言")
        );
        // 省略边界。
        let src2 = "变量 组 = [1, 2, 3]\n组[:2]";
        assert!(matches!(run_source(src2).unwrap(), Object::Array(_)));
    }

    #[test]
    fn 字符串下标产出字符() {
        assert_eq!(run_source("\"凡语言\"[1]").unwrap(), Object::Char('语'));
    }

    #[test]
    fn 字典字面量与按键读取() {
        let src = "变量 表 = {\"甲\": 1, \"乙\": 2}\n表[\"乙\"]";
        assert_eq!(run_source(src).unwrap(), Object::Int(2));
    }

    #[test]
    fn 字典按键写入与缺失键返回空() {
        let src = "变量 表 = {\"甲\": 1}\n表[\"乙\"] = 2\n表[\"乙\"]";
        assert_eq!(run_source(src).unwrap(), Object::Int(2));
        assert_eq!(
            run_source("变量 表 = {\"甲\": 1}\n表[\"无\"]").unwrap(),
            Object::Nil
        );
    }

    #[test]
    fn 遍历数组带序号() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3, 4] 中的 元素 序号\n  和 = 和 加 元素\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(10));
    }

    #[test]
    fn 遍历数组单变量() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3] 中的 元素\n  和 = 和 加 元素\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(6));
    }

    #[test]
    fn 遍历字符串产出单字符字符串() {
        let src = "变量 拼接 = \"\"\n遍历 \"凡语\" 中的 字\n  拼接 = 拼接 加 字\n结束\n拼接";
        assert_eq!(run_source(src).unwrap(), Object::string("凡语"));
    }

    #[test]
    fn 遍历字典键值() {
        let src = "变量 总和 = 0\n遍历 {\"甲\": 1, \"乙\": 2} 中的 键 值\n  总和 = 总和 加 值\n结束\n总和";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
    }

    #[test]
    fn 跳出提前结束遍历() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3, 4] 中的 元素\n  如果 元素 大于 2 那么\n    跳出\n  结束\n  和 = 和 加 元素\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
    }

    #[test]
    fn 继续跳过本轮() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3, 4] 中的 元素\n  如果 元素 等于 2 那么\n    继续\n  结束\n  和 = 和 加 元素\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(8));
    }

    #[test]
    fn while循环内跳出与继续() {
        let src = "变量 和 = 0\n变量 序号 = 0\n当 序号 小于 10 循环\n  序号 = 序号 加 1\n  如果 序号 等于 5 那么\n    继续\n  结束\n  如果 序号 大于 7 那么\n    跳出\n  结束\n  和 = 和 加 序号\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(23));
    }

    #[test]
    fn 跳出只能出现在循环内() {
        let (program, _) = parse_program("跳出");
        let result = compile_program(&program);
        assert!(result.is_err_and(|msg| msg.contains("跳出")));
    }

    #[test]
    fn 继续只能出现在循环内() {
        let (program, _) = parse_program("继续");
        let result = compile_program(&program);
        assert!(result.is_err_and(|msg| msg.contains("继续")));
    }
}
