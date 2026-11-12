//! 凡语言命令行工具。
//!
//! ```text
//! fan 运行 <脚本.凡> [参数...]   执行脚本
//! fan 测试 [目录或文件]          执行 *_测试.凡 中的测试函数
//! fan 交互                      进入交互式求值（REPL）
//! fan lsp                       启动语言服务器（LSP/stdio）
//! fan 版本                      输出版本
//! fan 帮助                      输出用法
//! ```

use fan_eval::{default_modules_dirs, Interpreter};
use fan_object::{Object, Runtime};
use fan_parser::parse_program;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::rc::Rc;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => {}
        Err(msg) => {
            eprintln!("错误：{}", msg);
            std::process::exit(1);
        }
    }
}

/// 命令分发。返回 `Err` 即打印错误并以非零码退出。
fn run(args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        print_usage();
        return Ok(());
    }
    match args[0].as_str() {
        "运行" | "run" => {
            if args.len() < 2 {
                return Err("用法：fan 运行 <脚本.凡> [参数...]".to_string());
            }
            run_file(&args[1], &args[2..])
        }
        "测试" | "test" => {
            let target = args.get(1).map(|s| s.as_str()).unwrap_or(".");
            run_tests(target)
        }
        "交互" | "repl" => run_repl(BufReader::new(std::io::stdin()), std::io::stdout()),
        "lsp" => run_lsp(),
        "版本" | "version" | "-v" | "--version" => {
            println!("凡语言 {}（命令名：fan）", VERSION);
            Ok(())
        }
        "帮助" | "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("未知子命令：{}\n\n{}", other, usage_text())),
    }
}

fn print_usage() {
    print!("{}", usage_text());
}

fn usage_text() -> String {
    format!(
        "凡语言 {}（命令名：fan）\n\
         \n\
          用法：\n\
          \x20 fan 运行 <脚本.凡> [参数...]   执行脚本\n\
          \x20 fan 测试 [目录或文件]          执行 *_测试.凡 中的测试函数\n\
          \x20 fan 交互                      进入交互式求值（REPL）\n\
          \x20 fan lsp                       启动语言服务器（LSP/stdio）\n\
          \x20 fan 版本                      输出版本\n\
          \x20 fan 帮助                      输出本帮助\n\
         \n\
         模块搜索目录：脚本所在目录 → 环境变量 FAN_SDK → 可执行文件旁 → 系统安装位置。\n",
        VERSION
    )
}

/// 执行一个脚本文件。
fn run_file(path: &str, script_args: &[String]) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("无法读取 {}: {}", path, e))?;
    let (program, errors) = parse_program(&source);
    if !errors.is_empty() {
        for e in &errors.0 {
            eprintln!("语法错误：{}", e);
        }
        return Err("解析失败".to_string());
    }

    let abs = abs_path(path);
    let dir = Path::new(&abs)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let rt = Rc::new(RefCell::new(Runtime::new()));
    rt.borrow_mut()
        .set_process_context(abs.clone(), script_args.to_vec());
    let mut interp = Interpreter::for_script(rt, default_modules_dirs(&dir), &dir, &abs);
    interp.run(&program).map_err(|e| e.display())?;
    Ok(())
}

/// 执行目录下的测试文件。
fn run_tests(target: &str) -> Result<(), String> {
    let results = fan_testrun::Runner::run(target)?;
    for r in &results {
        println!("{}", r.path);
        if !r.setup_error.is_empty() {
            println!("  文件错误：{}", r.setup_error);
            continue;
        }
        for case in &r.tests {
            let failed = r.failures.iter().find(|f| f.name == case.name);
            match failed {
                Some(f) => println!("  ✗ {}（第{}行）：{}", f.name, f.line, f.reason),
                None => println!("  ✓ {}", case.name),
            }
        }
    }
    let summary = fan_testrun::Runner::summary(&results);
    println!(
        "\n文件 {} 个，通过 {} 项，失败 {} 项",
        summary.files, summary.passed, summary.failed
    );
    if fan_testrun::has_failure(&results) {
        return Err("测试失败".to_string());
    }
    Ok(())
}

/// 交互式求值：逐行读取、在同一解释器上累积状态。
///
/// 与 Go 版 `runREPL` 对齐：空行跳过，`:退出`/`:quit`/`:q` 结束，
/// 语法或运行错误只打印不中断会话。为便于测试，输入/输出均以参数注入。
fn run_repl<I: BufRead, O: Write>(input: I, mut output: O) -> Result<(), String> {
    let mut interp = Interpreter::new();
    let reader = input;
    writeln!(output, "凡语言 REPL，输入 :退出 结束").map_err(|e| e.to_string())?;
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, ":退出" | ":quit" | ":q") {
            return Ok(());
        }
        let (program, errors) = parse_program(trimmed);
        if !errors.is_empty() {
            for e in &errors.0 {
                writeln!(output, "语法错误：{e}").map_err(|e| e.to_string())?;
            }
            continue;
        }
        match interp.run(&program) {
            Ok(Object::Nil) => {}
            Ok(v) => writeln!(output, "{}", Object::format(&v)).map_err(|e| e.to_string())?,
            Err(e) => writeln!(output, "运行错误：{}", e.display()).map_err(|e| e.to_string())?,
        }
    }
    Ok(())
}

/// 启动 LSP 语言服务器：在标准输入/输出上按 JSON-RPC、`Content-Length`
/// 分帧与编辑器通信，直到收到 `exit` 或输入结束。
fn run_lsp() -> Result<(), String> {
    let stdin = std::io::stdin();
    // Stdout 本身是 Send + Write（内部自带锁），而本目标的 StdoutLock
    // 不是 Send，无法移入防抖线程，因此直接传 Stdout 而非其锁守卫。
    let stdout = std::io::stdout();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut server = fan_lsp::Server::new();
    server
        .run(&mut reader, stdout)
        .map_err(|e| format!("LSP 服务器异常：{e}"))
}

fn abs_path(path: &str) -> String {
    std::path::absolute(Path::new(path))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 把 &str 参数表转成 run 需要的 Vec<String>。
    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fan-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn 运行脚本并输出结果() {
        let dir = temp_dir("run");
        let path = write(&dir, "甲.凡", "打印行(\"你好\")\n");
        assert!(run(args(&["运行", &path])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 脚本可读取进程参数() {
        let dir = temp_dir("args");
        // 参数写入文件以便断言（打印行 输出到标准输出，测试不便捕获）
        let out = dir.join("参数.txt");
        // 直接用 本地 绑定原生函数，避免依赖共享 fan-sdk 的 .凡 模块
        let source = format!(
            "定义 本地(\"processArgs\") 函数 取参数(任意... 占位) -> 数组\n    返回 空\n结束\n\
             定义 本地(\"writeFile\") 函数 写文件(任意... 占位)\n    返回 空\n结束\n\
             写文件(\"{}\", 取参数()[0])\n",
            out.to_string_lossy().replace('\\', "/")
        );
        let path = write(&dir, "参数.凡", &source);
        assert!(run(args(&["运行", &path, "甲参数"])).is_ok());
        let written = std::fs::read_to_string(&out).unwrap();
        assert_eq!(written, "甲参数");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 运行不存在的文件报错() {
        let err = run(args(&["运行", "绝不存在的文件.凡"])).unwrap_err();
        assert!(err.contains("无法读取"), "实际：{}", err);
    }

    #[test]
    fn 语法错误时报错() {
        let dir = temp_dir("syntax");
        let path = write(&dir, "错.凡", "变量 = = =\n");
        let err = run(args(&["运行", &path])).unwrap_err();
        assert_eq!(err, "解析失败");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 缺少文件名时给出用法() {
        let err = run(args(&["运行"])).unwrap_err();
        assert!(err.contains("用法"), "实际：{}", err);
    }

    #[test]
    fn 未知子命令报错() {
        let err = run(args(&["不存在的命令"])).unwrap_err();
        assert!(err.contains("未知子命令"), "实际：{}", err);
    }

    #[test]
    fn 版本与帮助不报错() {
        assert!(run(args(&["版本"])).is_ok());
        assert!(run(args(&["帮助"])).is_ok());
    }

    #[test]
    fn 测试子命令运行通过的文件() {
        let dir = temp_dir("test-ok");
        write(&dir, "甲_测试.凡", "函数 测试甲()\n    变量 值 = 1\n结束\n");
        assert!(run(args(&["测试", &dir.to_string_lossy()])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 测试子命令失败时返回错误() {
        let dir = temp_dir("test-fail");
        write(
            &dir,
            "乙_测试.凡",
            "函数 测试乙()\n    崩溃了(\"故意失败\")\n结束\n",
        );
        let err = run(args(&["测试", &dir.to_string_lossy()])).unwrap_err();
        assert_eq!(err, "测试失败");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repl逐行求值并保持状态() {
        let input = "变量 甲 = 3\n甲 加 4\n:退出\n".as_bytes();
        let mut out = Vec::new();
        run_repl(input, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("凡语言 REPL"), "实际：{text}");
        assert!(text.contains("7"), "应输出表达式结果 7：{text}");
        // 状态在行间保持：后续表达式能读到先前声明的变量
        assert!(text.contains("3"), "应保留先前变量值：{text}");
    }

    #[test]
    fn repl错误不中断后续会话() {
        let input = "变量 = = =\n1 加 2\n:q\n".as_bytes();
        let mut out = Vec::new();
        run_repl(input, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("语法错误"), "实际：{text}");
        assert!(text.contains("3"), "错误后仍应继续求值：{text}");
    }

    #[test]
    fn repl运行错误可恢复() {
        let input = "打印(未定义名字)\n2 乘 3\n:quit\n".as_bytes();
        let mut out = Vec::new();
        run_repl(input, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("运行错误"), "实际：{text}");
        assert!(text.contains("6"), "运行错误后应继续：{text}");
    }

    #[test]
    fn lsp服务器在空输入上干净退出() {
        // stdin 立即 EOF（无消息）时服务器应正常返回而非报错。
        let result =
            fan_lsp::Server::new().run(&mut std::io::Cursor::new(Vec::new()), Vec::<u8>::new());
        assert!(result.is_ok(), "空输入应正常结束：{result:?}");
    }
}
