//! 冒烟：用真实标准库文件验证内建绑定链路。
//!
//! 这些用例直接加载 `fan-sdk` 下的源文件，是「内建框架 + 标准库」整条链路
//! 的验收测试，而非单纯的单元测试。

use fan_eval::Interpreter;
use fan_parser::parse_program;
use std::path::PathBuf;

/// 标准库目录。
///
/// 标准库由 Go 版与 Rust 版共用、不在任一实现仓库内，故不能写死路径：
/// 先认 `FAN_SDK` 环境变量，再按工作区布局逐级向上找 `fan-sdk`。
fn sdk_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("FAN_SDK") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..4 {
        let candidate = dir.join("fan-sdk");
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir.pop();
    }
    None
}

/// 标准库目录；拿不到时给出可操作的提示，而不是含糊的「找不到模块」。
fn sdk() -> String {
    sdk_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            panic!("未找到 fan-sdk：请用 FAN_SDK 环境变量指向它，或放在仓库同级目录")
        })
}

fn run(src: &str) -> Result<fan_object::Object, String> {
    let (program, errors) = parse_program(src);
    if !errors.is_empty() {
        return Err(format!("解析错误：{errors}"));
    }
    let mut interp = Interpreter::new();
    interp.env.borrow_mut().set_base_dir(sdk());
    interp.run(&program).map_err(|e| e.display())
}

// ---------------------------------------------------------------- 系统

#[test]
fn 系统模块类型名() {
    let out = run("导入 \"系统\" 作为 系统模块\n系统模块.类型名(1)").unwrap();
    assert_eq!(out.inspect(), "integer");
}

#[test]
fn 系统模块长度与追加() {
    let out = run("导入 \"系统\" 作为 系统模块\n系统模块.长度([1, 2, 3])").unwrap();
    assert_eq!(out.inspect(), "3");
    let out = run("导入 \"系统\" 作为 系统模块\n系统模块.追加([1], 2)").unwrap();
    assert_eq!(out.inspect(), "[1, 2]");
}

// ---------------------------------------------------------------- 转换

#[test]
fn 转换模块解析整数() {
    let out = run("导入 \"转换\" 作为 转换模块\n转换模块.转整数(\"42\")").unwrap();
    assert_eq!(out.inspect(), "42");
}

#[test]
fn 转换模块转字符串() {
    let out = run("导入 \"转换\" 作为 转换模块\n转换模块.转字符串(42)").unwrap();
    assert_eq!(out.inspect(), "42");
    let out = run("导入 \"转换\" 作为 转换模块\n转换模块.转字符串(1.5)").unwrap();
    assert_eq!(out.inspect(), "1.5");
}

// ---------------------------------------------------------------- 数学

#[test]
fn 数学模块平方根与幂() {
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.平方根(9.0)").unwrap();
    assert_eq!(out.inspect(), "3");
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.幂(2.0, 10.0)").unwrap();
    assert_eq!(out.inspect(), "1024");
}

#[test]
fn 数学模块纯凡语言实现的函数() {
    // 绝对值/四舍五入/总和/斜边长 全部用凡语言写成，验证其可用
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.绝对值(-7)").unwrap();
    assert_eq!(out.inspect(), "7");
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.四舍五入(3.6)").unwrap();
    assert_eq!(out.inspect(), "4");
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.总和([1, 2, 3, 4])").unwrap();
    assert_eq!(out.inspect(), "10");
    let out = run("导入 \"数学\" 作为 数学模块\n数学模块.斜边长(3.0, 4.0)").unwrap();
    assert_eq!(out.inspect(), "5");
}

// ---------------------------------------------------------------- 编码
//
// 编码.凡 顶部声明 @内建("jsonEncode")。该内建属 ext 组，无 json 扩展库时
// 自动回落到纯标准库实现，因此模块可直接加载——不部署任何扩展也能运行。

#[test]
fn 编码模块base64往返() {
    // 生成/解析类函数返回 (值, 错误) 多元值，故显示形如 `(凡, 空)`
    let encoded = run("导入 \"编码\" 作为 编码模块\n编码模块.生成Base64(\"凡\")")
        .unwrap()
        .inspect();
    assert_eq!(encoded, "5Yeh", "Base64 编码结果：{encoded}");
    let back = run(&format!(
        "导入 \"编码\" 作为 编码模块\n编码模块.解析Base64(\"{}\")",
        encoded
    ))
    .unwrap();
    assert_eq!(back.inspect(), "(凡, 空)");
}

#[test]
fn 编码模块json往返() {
    let out = run("导入 \"编码\" 作为 编码模块\n编码模块.生成JSON([1, 2, 3])").unwrap();
    assert_eq!(out.inspect(), "([1,2,3], 空)");
    let back = run("导入 \"编码\" 作为 编码模块\n编码模块.解析JSON(\"[1,2,3]\")").unwrap();
    assert_eq!(back.inspect(), "([1, 2, 3], 空)");
}

#[test]
fn 编码模块json支持中文() {
    let back = run("导入 \"编码\" 作为 编码模块\n编码模块.解析JSON(\"{\\\"名字\\\":\\\"凡\\\"}\")")
        .unwrap();
    assert!(back.inspect().contains("凡"), "实际：{}", back.inspect());
}

// ------------------------------------------------- 数组 / 字符串 / 随机
//
// 这几个模块大量使用「为基本类型定义方法」（`定义 字符串 的 方法 …`）。

#[test]
fn 数组模块纯凡语言实现() {
    let out = run("导入 \"数组\" 作为 数组模块\n数组模块.反转([1, 2, 3])").unwrap();
    assert_eq!(out.inspect(), "[3, 2, 1]");
}

#[test]
fn 字符串模块纯凡语言实现() {
    let out = run("导入 \"字符串\" 作为 字符串模块\n字符串模块.包含(\"甲乙\", \"乙\")").unwrap();
    assert_eq!(out.inspect(), "真");
}

#[test]
fn 随机模块() {
    let out = run("导入 \"随机\" 作为 随机模块\n随机模块.随机整数(10)").unwrap();
    let n: i64 = out.inspect().parse().unwrap();
    assert!((0..10).contains(&n), "越界：{}", n);
}

// ---------------------------------------------------------------- 文件

#[test]
fn 文件模块临时目录() {
    let out = run("导入 \"文件\" 作为 文件模块\n文件模块.临时目录(\"fan-sdk-\")").unwrap();
    // 返回 (路径, 错误)，错误应为空
    assert!(out.inspect().ends_with("空)"), "实际：{}", out.inspect());
}

#[test]
fn 文件模块写入读取往返() {
    let out = run("导入 \"文件\" 作为 文件模块\n\
         变量 目录, 错误值 = 文件模块.临时目录(\"fan-sdk-\")\n\
         变量 路径 = 文件模块.拼路径(目录, \"a.txt\")\n\
         文件模块.写入(路径, \"甲\")\n\
         变量 内容, 读取错误 = 文件模块.读取(路径)\n\
         文件模块.删除全部(目录)\n\
         内容")
    .unwrap();
    assert_eq!(out.inspect(), "甲");
}

#[test]
fn 文件模块追加() {
    let out = run("导入 \"文件\" 作为 文件模块\n\
         变量 目录, 错误值 = 文件模块.临时目录(\"fan-sdk-\")\n\
         变量 路径 = 文件模块.拼路径(目录, \"b.txt\")\n\
         文件模块.写入(路径, \"甲\")\n\
         文件模块.追加(路径, \"乙\")\n\
         变量 内容, 读取错误 = 文件模块.读取(路径)\n\
         文件模块.删除全部(目录)\n\
         内容")
    .unwrap();
    assert_eq!(out.inspect(), "甲乙");
}

#[test]
fn 文件模块存在性与路径() {
    let out = run("导入 \"文件\" 作为 文件模块\n文件模块.路径分隔符()").unwrap();
    assert!(!out.inspect().is_empty(), "路径分隔符为空");
    let out = run("导入 \"文件\" 作为 文件模块\n文件模块.文件名(\"甲/乙.txt\")").unwrap();
    assert_eq!(out.inspect(), "乙.txt");
}

// ---------------------------------------------------------------- 时间

#[test]
fn 时间模块格式化() {
    let out = run("导入 \"时间\" 作为 时间模块\n时间模块.格式化(0, \"2006-01-02\")").unwrap();
    assert_eq!(out.inspect(), "1970-01-01");
}

#[test]
fn 时间模块解析日期往返() {
    let out = run("导入 \"时间\" 作为 时间模块\n\
         变量 时间戳, 错误值 = 时间模块.解析日期(\"2020-01-01\")\n\
         时间模块.格式化(时间戳, 时间模块.日期格式)")
    .unwrap();
    assert_eq!(out.inspect(), "2020-01-01");
}

#[test]
fn 时间模块解析失败返回错误() {
    let out = run("导入 \"时间\" 作为 时间模块\n时间模块.解析日期(\"bad\")").unwrap();
    assert!(out.inspect().starts_with("(0,"), "实际：{}", out.inspect());
}

#[test]
fn 时间模块时间分量() {
    let out = run("导入 \"时间\" 作为 时间模块\n\
         变量 时间戳, 错误值 = 时间模块.解析(时间模块.默认格式, \"2020-06-15 10:20:30\")\n\
         变量 分量 = 时间模块.时间分量(时间戳)\n\
         分量[\"year\"]")
    .unwrap();
    assert_eq!(out.inspect(), "2020");
}

#[test]
fn 时间模块当前时间戳() {
    let out = run("导入 \"时间\" 作为 时间模块\n时间模块.当前时间戳()").unwrap();
    assert!(out.inspect().parse::<i64>().unwrap() > 0);
}

// ---------------------------------------------------------------- 进程

#[test]
fn 进程模块进程号() {
    let out = run("导入 \"进程\" 作为 进程模块\n进程模块.进程号()").unwrap();
    assert!(out.inspect().parse::<i64>().unwrap() > 0);
}

#[test]
fn 进程模块环境变量() {
    let out = run("导入 \"进程\" 作为 进程模块\n\
         进程模块.设置环境变量(\"FAN_SDK_SMOKE\", \"凡\")\n\
         进程模块.环境变量(\"FAN_SDK_SMOKE\")")
    .unwrap();
    assert_eq!(out.inspect(), "凡");
}

#[test]
fn 进程模块脚本为文本() {
    let out = run("导入 \"系统\" 作为 系统模块\n导入 \"进程\" 作为 进程模块\n系统模块.类型名(进程模块.脚本())").unwrap();
    assert_eq!(out.inspect(), "string");
}

// ---------------------------------------------------------------- 正则
//
// 正则组属 ext，无 fan_ext_regex 动态库时回落到纯标准库引擎。

#[test]
fn 正则模块匹配与查找() {
    // 匹配/查找均返回 (值, 错误) 多元值
    let out = run("导入 \"正则\" 作为 正则模块\n正则模块.匹配(\"\\d+\", \"a123b\")").unwrap();
    assert_eq!(out.inspect(), "(真, 空)");
    let out = run("导入 \"正则\" 作为 正则模块\n正则模块.查找(\"\\d+\", \"a12b34\")").unwrap();
    assert_eq!(out.inspect(), "(12, 空)");
}

#[test]
fn 正则模块分组() {
    let out = run("导入 \"正则\" 作为 正则模块\n\
         变量 分组结果, 错误值 = 正则模块.分组(\"([a-z]+)(\\d+)\", \"abc123\")\n\
         分组结果[1]")
    .unwrap();
    assert_eq!(out.inspect(), "abc");
}

#[test]
fn 正则模块替换与拆分() {
    let out = run("导入 \"正则\" 作为 正则模块\n\
         变量 结果, 错误值 = 正则模块.全部替换(\"(\\w+)\", \"a b\", \"[$1]\")\n\
         结果")
    .unwrap();
    assert_eq!(out.inspect(), "[a] [b]");
    let out = run("导入 \"正则\" 作为 正则模块\n正则模块.拆分(\"[,;]\", \"a,b;c\")").unwrap();
    assert_eq!(out.inspect(), "([a, b, c], 空)");
}

#[test]
fn 正则模块非法模式返回错误() {
    // 具体措辞取决于用的是扩展库还是默认实现，这里只要求「返回错误」
    let out = run("导入 \"正则\" 作为 正则模块\n正则模块.查找(\"(\", \"abc\")").unwrap();
    assert!(out.inspect().contains("错误"), "实际：{}", out.inspect());
}

// ---------------------------------------------------------------- 网络
//
// 网络组属 ext，无 fan_ext_http 动态库时回落为纯标准库的 HTTP/1.1 实现。

#[test]
fn 网络模块加载并处理非法地址() {
    let out = run("导入 \"网络\" 作为 网络模块\n\
         变量 结果, 错误值 = 网络模块.请求(\"GET\", \"bad-url\", 空)\n\
         结果[\"success\"]")
    .unwrap();
    assert_eq!(out.inspect(), "假");
}

// ---------------------------------------------------------------- 扩展库来源
//
// 正则既可能由扩展库提供，也可能回落到默认实现；两者给出的结果必须一致。
// 构建输出目录里部署了 libfan_ext_regex 时这里会是「扩展库」，否则是
// 「默认实现」——凡代码本身无需关心。

#[test]
fn 正则模块登记了实现来源() {
    let (program, errors) =
        parse_program("导入 \"正则\" 作为 正则模块\n正则模块.匹配(\"\\d+\", \"a123b\")");
    assert!(errors.is_empty(), "解析错误：{errors}");
    let mut interp = Interpreter::new();
    interp.env.borrow_mut().set_base_dir(sdk());
    interp.run(&program).unwrap();
    let source = interp
        .ext
        .borrow()
        .source_of("正则")
        .expect("正则 未登记实现来源");
    // 无论哪一档实现，结果都必须正确
    let out = run("导入 \"正则\" 作为 正则模块\n正则模块.匹配(\"\\d+\", \"a123b\")").unwrap();
    assert_eq!(out.inspect(), "(真, 空)", "实现来源：{}", source);
}
