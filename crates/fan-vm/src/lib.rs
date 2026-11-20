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
mod loader;
mod ops;
mod upvalue;
mod value;
mod vm;
mod vm_function;

pub use error::{VmError, VmResult};
pub use loader::ModuleLoader;
pub use upvalue::ModuleId;
pub use vm::{SharedModules, Vm, MAIN_MODULE};
pub use vm_function::SharedRegistry;

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
    fn 定义本地别名映射到原生调用() {
        // `定义 本地` 不生成字节码函数体，凡语名经别名表直接转 CallNative。
        let src = "定义 本地(\"len\") 函数 长度(任意 值)\n 返回 空\n结束\n长度(\"abcd\")";
        assert_eq!(run_with_natives(src).unwrap(), Object::Int(4));
        let src2 = "定义 本地(\"type\") 函数 类型名(任意 值) -> 字符串\n 返回 空\n结束\n类型名(7)";
        assert_eq!(run_with_natives(src2).unwrap(), Object::string("integer"));
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

    #[test]
    fn 闭包捕获外层变量并在帧返回后保持() {
        let src = "函数 造计数器()\n  变量 数 = 0\n  函数 增加()\n    数 = 数 加 1\n    返回 数\n  结束\n  返回 增加\n结束\n\
                   变量 计数 = 造计数器()\n计数()";
        assert_eq!(run_source(src).unwrap(), Object::Int(1));
        // 同一闭包多次调用，upvalue 状态持续累积。
        let src2 = "函数 造计数器()\n  变量 数 = 0\n  函数 增加()\n    数 = 数 加 1\n    返回 数\n  结束\n  返回 增加\n结束\n\
                    变量 计数 = 造计数器()\n计数()\n计数()\n计数()";
        assert_eq!(run_source(src2).unwrap(), Object::Int(3));
    }

    #[test]
    fn 两个闭包实例各自独立持有upvalue() {
        let src = "函数 造计数器()\n  变量 数 = 0\n  函数 增加()\n    数 = 数 加 1\n    返回 数\n  结束\n  返回 增加\n结束\n\
                   变量 甲 = 造计数器()\n变量 乙 = 造计数器()\n甲()\n甲()\n乙()";
        assert_eq!(run_source(src).unwrap(), Object::Int(1));
    }

    #[test]
    fn 多个闭包共享同一可变upvalue() {
        let src = "函数 造对()\n  变量 共享 = 10\n  函数 读()\n    返回 共享\n  结束\n\
                   函数 写(整数 值)\n    共享 = 值\n  结束\n  返回 读, 写\n结束\n\
                   变量 读、写 = 造对()\n写(99)\n读()";
        assert_eq!(run_source(src).unwrap(), Object::Int(99));
    }

    #[test]
    fn 跨多层嵌套捕获参数与局部() {
        let src = "函数 外层(整数 基数)\n  函数 中层()\n    函数 内层(整数 增量)\n      返回 基数 加 增量\n    结束\n    返回 内层\n  结束\n  返回 中层()\n结束\n\
                   变量 加五 = 外层(5)\n加五(10)";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 闭包读取外层变量不修改时仍可访问() {
        let src = "函数 配()\n  变量 前缀 = 100\n  函数 取(整数 偏移)\n    返回 前缀 加 偏移\n  结束\n  返回 取\n结束\n\
                    变量 取 = 配()\n取(23)";
        assert_eq!(run_source(src).unwrap(), Object::Int(123));
    }

    #[test]
    fn 后缀自增自减返回旧值并写回() {
        // 后缀：表达式结果是旧值，变量写回新值。
        let src = "变量 次数 = 5\n变量 旧 = 次数++\n旧";
        assert_eq!(run_source(src).unwrap(), Object::Int(5));
        let src2 = "变量 次数 = 5\n变量 旧 = 次数++\n次数";
        assert_eq!(run_source(src2).unwrap(), Object::Int(6));
        let src3 = "变量 次数 = 5\n次数--\n次数";
        assert_eq!(run_source(src3).unwrap(), Object::Int(4));
    }

    #[test]
    fn 自增在循环中累加() {
        let src = "变量 和 = 0\n变量 i = 0\n当 i 小于 3 循环\n  i++\n  和 = 和 加 i\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(6));
    }

    #[test]
    fn 插值字符串变量与表达式() {
        let src = "变量 名称 = \"凡语言\"\n\"你好，$名称\"";
        assert_eq!(run_source(src).unwrap(), Object::string("你好，凡语言"));

        let src2 = "变量 数量 = 3\n\"共 ${数量 乘 2} 个\"";
        assert_eq!(run_source(src2).unwrap(), Object::string("共 6 个"));
    }

    #[test]
    fn 插值字符串相邻与空值() {
        let src = "变量 a = 1\n变量 b = 2\n\"$a$b\"";
        assert_eq!(run_source(src).unwrap(), Object::string("12"));

        let src2 = "变量 x = 空\n\"值：$x\"";
        assert_eq!(run_source(src2).unwrap(), Object::string("值：空"));
    }

    #[test]
    fn 插值字符串普通美元符号与转义() {
        let src = "\"价格 $100\"";
        assert_eq!(run_source(src).unwrap(), Object::string("价格 $100"));

        let src2 = "变量 x = 1\n\"\\$x 不是 $x\"";
        assert_eq!(run_source(src2).unwrap(), Object::string("$x 不是 1"));
    }

    #[test]
    fn 判断值匹配多候选与默认分支() {
        let src = "函数 归类(整数 码)\n\
                    判断 码\n\
                        为 200 则 返回 \"成功\"\n\
                        为 301、302 时 返回 \"重定向\"\n\
                        为 404\n 返回 \"未找到\"\n\
                        其他 则 返回 \"其他\"\n\
                    结束\n结束\n\
                    归类(302)";
        assert_eq!(run_source(src).unwrap(), Object::string("重定向"));
        assert_eq!(
            run_source(&src.replace("归类(302)", "归类(500)")).unwrap(),
            Object::string("其他")
        );
        assert_eq!(
            run_source(&src.replace("归类(302)", "归类(404)")).unwrap(),
            Object::string("未找到")
        );
    }

    #[test]
    fn 判断守卫分支按顺序短路() {
        let src = "函数 描述(整数 分)\n\
                    判断\n\
                        当 分 大于等于 90 则 返回 \"优\"\n\
                        当 分 大于等于 60 时 返回 \"及格\"\n\
                        其他\n 返回 \"不及格\"\n\
                    结束\n结束\n\
                    描述(70)";
        assert_eq!(run_source(src).unwrap(), Object::string("及格"));
        assert_eq!(
            run_source(&src.replace("描述(70)", "描述(30)")).unwrap(),
            Object::string("不及格")
        );
    }

    #[test]
    fn 判断无命中且无默认落空为空() {
        let src = "变量 结果 = \"初始\"\n\
                    判断 2\n 为 1 则 结果 = \"一\"\n 结束\n结果";
        assert_eq!(run_source(src).unwrap(), Object::string("初始"));
    }

    #[test]
    fn 判断分支体可含多条语句() {
        let src = "函数 累计(整数 档)\n 变量 总和 = 0\n\
                    判断 档\n\
                        为 1 则\n 总和 = 总和 加 10\n 总和 = 总和 加 5\n\
                        为 2 则\n 总和 = 总和 加 1\n\
                        其他 则 总和 = 总和 减 1\n\
                    结束\n 返回 总和\n结束\n\
                    累计(1)";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 遍历函数返回的数组用动态标签() {
        let src = "函数 造数组()\n 返回 [10, 20, 30]\n结束\n\
                    变量 和 = 0\n遍历 造数组() 中的 元素\n 和 = 和 加 元素\n结束\n和";
        assert_eq!(run_source(src).unwrap(), Object::Int(60));
    }

    #[test]
    fn 动态标签按运行期实际类型分派字符串与字典() {
        let src = "函数 取串()\n 返回 \"ab\"\n结束\n\
                    变量 拼 = \"\"\n遍历 取串() 中的 字\n 拼 = 拼 加 字\n结束\n拼";
        assert_eq!(run_source(src).unwrap(), Object::string("ab"));

        let src2 = "函数 取字典()\n 返回 {\"甲\": 1, \"乙\": 2}\n结束\n\
                    变量 和 = 0\n遍历 取字典() 中的 键 值\n 和 = 和 加 值\n结束\n和";
        assert_eq!(run_source(src2).unwrap(), Object::Int(3));
    }

    #[test]
    fn 复合赋值四则运算写回() {
        let src = "变量 x = 10\nx += 3\nx";
        assert_eq!(run_source(src).unwrap(), Object::Int(13));
        let src3 = "变量 x = 10\nx -= 4\nx";
        assert_eq!(run_source(src3).unwrap(), Object::Int(6));
        let src4 = "变量 x = 6\nx *= 7\nx";
        assert_eq!(run_source(src4).unwrap(), Object::Int(42));
        let src5 = "变量 s = \"a\"\ns += \"b\"\ns";
        assert_eq!(run_source(src5).unwrap(), Object::string("ab"));
    }

    #[test]
    fn 数组字面量展开与拼接() {
        let src = "变量 前 = [1, 2]\n变量 后 = [4, 5]\n[0, ...前, 3, ...后]";
        assert_eq!(
            run_source(src).unwrap(),
            Object::array(vec![
                Object::Int(0),
                Object::Int(1),
                Object::Int(2),
                Object::Int(3),
                Object::Int(4),
                Object::Int(5),
            ])
        );
        let src2 = "变量 数列 = [3, 1, 4]\n[...数列]";
        assert_eq!(
            run_source(src2).unwrap(),
            Object::array(vec![Object::Int(3), Object::Int(1), Object::Int(4)])
        );
    }

    #[test]
    fn 调用实参展开传入可变参数() {
        let src = "函数 求和(整数... 数)\n 变量 总 = 0\n\
                    遍历 数 中的 n\n 总 = 总 加 n\n结束\n 返回 总\n结束\n\
                    求和(...[1, 2, 3], ...[4, 5])";
        assert_eq!(run_source(src).unwrap(), Object::Int(15));
    }

    #[test]
    fn 字典字面量展开后写覆盖同名键() {
        let src = "变量 基础 = {\"a\": 1, \"b\": 2}\n\
                    变量 覆盖 = {\"b\": 9, \"c\": 3}\n\
                    变量 合并 = {...基础, ...覆盖}\n\
                    合并[\"b\"]";
        assert_eq!(run_source(src).unwrap(), Object::Int(9));
        let src2 = "变量 基础 = {\"a\": 1, \"b\": 2}\n\
                    变量 覆盖 = {\"b\": 9, \"c\": 3}\n\
                    ({...基础, ...覆盖})[\"a\"]";
        assert_eq!(run_source(src2).unwrap(), Object::Int(1));
    }

    #[test]
    fn 顶层函数提升支持前置引用() {
        // 跑测试 定义在前、求和函数 定义在后，但调用发生在所有顶层声明完成之后，
        // 函数提升应使前置引用解析到正确槽位，与 tree 模块作用域一致。
        let src = "函数 跑测试()\n 返回 求和函数(...[1, 2], ...[3, 4])\n结束\n\
                    函数 求和函数(整数... 数)\n 变量 总 = 0\n\
                    遍历 数 中的 n\n 总 = 总 加 n\n结束\n 返回 总\n结束\n\
                    跑测试()";
        assert_eq!(run_source(src).unwrap(), Object::Int(10));
    }

    #[test]
    fn 隐式接收者调用等价点成员调用() {
        // `包 算(21)` 等价于 `包.算(21)`：取接收者的同名成员函数后以显式实参调用。
        // 隐式接收者调用由 parser 在顶层表达式语句识别，故被测调用置于末行。
        let src = "函数 翻倍(整数 x)\n 返回 x 乘 2\n结束\n\
                    变量 包 = {\"算\": 翻倍}\n\
                    包 算(21)\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(42));
    }

    #[test]
    fn 通过表达式在模块作用域内解析裸名() {
        // 通过 工具 加倍(3)：裸名 加倍 解析为模块 工具 的导出成员后调用。
        let src = "变量 工具 = {}\n\
                    函数 加倍(整数 x)\n 返回 x 乘 2\n结束\n\
                    工具.加倍 = 加倍\n\
                    通过 工具 加倍(21)\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(42));
    }

    #[test]
    fn 模型复合字面量实例化并按字段填充() {
        let src = "定义 模型 坐标\n 公开 属性 整数 横向\n 公开 属性 整数 纵向\n结束\n\
                    变量 p = 坐标{横向: 3, 纵向: 4}\np.横向\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(3));
        let src2 = "定义 模型 坐标\n 公开 属性 整数 横向\n 公开 属性 整数 纵向\n结束\n\
                    变量 p = 坐标{横向: 9}\np.纵向\n";
        assert_eq!(run_source(src2).unwrap(), Object::Nil);
    }

    #[test]
    fn 模型字段可经成员赋值修改() {
        let src = "定义 模型 账户\n 属性 整数 余额\n结束\n\
                    变量 a = 账户{余额: 100}\na.余额 = 250\na.余额\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(250));
    }

    #[test]
    fn 模型嵌入字段提升与嵌套复合字面量() {
        let src = "定义 模型 动物\n 公开 属性 字符串 名字\n结束\n\
                    定义 模型 狗窝\n 公开 属性 字符串 地址\n 嵌入 动物\n结束\n\
                    变量 w = 狗窝{地址: \"北京\", 动物: 动物{名字: \"旺财\"}}\n\
                    w.动物.名字\n";
        assert_eq!(run_source(src).unwrap(), Object::string("旺财"));
        // 嵌入字段提升：直接经宿主访问被嵌入模型的字段。
        let src2 = "定义 模型 动物\n 公开 属性 字符串 名字\n结束\n\
                    定义 模型 狗窝\n 公开 属性 字符串 地址\n 嵌入 动物\n结束\n\
                    变量 w = 狗窝{地址: \"北京\", 动物: 动物{名字: \"旺财\"}}\n\
                    w.名字\n";
        assert_eq!(run_source(src2).unwrap(), Object::string("旺财"));
    }

    #[test]
    fn 模型方法访问自己并支持方法链() {
        let src = "定义 模型 坐标\n 公开 属性 整数 横向\n 公开 属性 整数 纵向\n结束\n\
                    定义 坐标 的 方法 和() -> 整数\n 返回 自己.横向 加 自己.纵向\n结束\n\
                    坐标{横向: 3, 纵向: 4}.和()\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(7));
    }

    #[test]
    fn 模型方法可修改自己的字段() {
        let src = "定义 模型 计数器\n 属性 整数 次数\n结束\n\
                    定义 计数器 的 方法 累加(整数 增量)\n 自己.次数 = 自己.次数 加 增量\n结束\n\
                    变量 c = 计数器{次数: 0}\nc.累加(5)\nc.累加(7)\nc.次数\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(12));
    }

    #[test]
    fn 类名构造触发初始化方法() {
        let src = "定义 模型 点\n 属性 整数 横\n 属性 整数 纵\n结束\n\
                    定义 点 的 方法 初始化(整数 横, 整数 纵)\n 自己.横 = 横\n 自己.纵 = 纵\n结束\n\
                    变量 p = 点(3, 4)\np.横 加 p.纵\n";
        assert_eq!(run_source(src).unwrap(), Object::Int(7));
    }

    #[test]
    fn 嵌入模型的方法可在宿主实例上提升调用() {
        let src = "定义 模型 基础\n 属性 字符串 编号\n结束\n\
                    定义 基础 的 方法 初始化(字符串 编号)\n 自己.编号 = 编号\n结束\n\
                    定义 基础 的 方法 描述() -> 字符串\n 返回 \"部件:\" 加 自己.编号\n结束\n\
                    定义 模型 整机\n 嵌入 基础\n 属性 字符串 型号\n结束\n\
                    定义 整机 的 方法 初始化(字符串 编号, 字符串 型号)\n 自己.编号 = 编号\n 自己.型号 = 型号\n结束\n\
                    变量 m = 整机(\"001\", \"X1\")\nm.描述()\n";
        assert_eq!(run_source(src).unwrap(), Object::string("部件:001"));
    }

    #[test]
    fn 基本类型扩展方法绑定接收者() {
        let src = "定义 字符串 的 方法 感叹() -> 字符串\n 返回 自己 加 \"!\"\n结束\n\
                    \"你好\".感叹()\n";
        assert_eq!(run_source(src).unwrap(), Object::string("你好!"));
    }

    #[test]
    fn 基本类型扩展方法支持默认参数与裸函数调用() {
        let src = "函数 包裹(字符串 文本, 字符串 前后 = \"[\") -> 字符串\n 返回 前后 加 文本 加 \"]\"\n结束\n\
                    定义 字符串 的 方法 包装(字符串 前后 = \"<\") -> 字符串\n 返回 包裹(自己, 前后)\n结束\n\
                    \"x\".包装()\n";
        assert_eq!(run_source(src).unwrap(), Object::string("<x]"));
        let src2 = "函数 包裹(字符串 文本, 字符串 前后 = \"[\") -> 字符串\n 返回 前后 加 文本 加 \"]\"\n结束\n\
                    定义 字符串 的 方法 包装(字符串 前后 = \"<\") -> 字符串\n 返回 包裹(自己, 前后)\n结束\n\
                    \"x\".包装(\"(\")\n";
        assert_eq!(run_source(src2).unwrap(), Object::string("(x]"));
    }
}
