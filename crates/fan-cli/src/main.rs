//! 凡语言命令行工具。
//!
//! ```text
//! fan 运行 <脚本.凡> [参数...]   执行脚本
//! fan 测试 [目录或文件]          执行 *_测试.凡 中的测试函数
//! fan 检查 [目录或文件]          静态检查，不运行即可发现未定义名、坏导入等问题
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

mod vm_loader;
use vm_loader::FsModuleLoader;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 执行引擎：树遍历求值器（默认）或栈式字节码虚拟机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Engine {
    #[default]
    Tree,
    Vm,
}

impl Engine {
    fn parse(value: &str) -> Result<Engine, String> {
        match value {
            "tree" | "树" => Ok(Engine::Tree),
            "vm" | "虚拟机" => Ok(Engine::Vm),
            other => Err(format!("未知引擎：{other}（可选 tree/树 或 vm/虚拟机）")),
        }
    }
}

/// 从 `运行` 的参数中剥离 `--engine=<引擎>` / `--engine <引擎>`（含中文
/// `--引擎`），返回所选引擎与剩余的位置参数。默认树遍历引擎。
fn split_engine_args(args: &[String]) -> Result<(Engine, Vec<String>), String> {
    let mut engine = Engine::Tree;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(rest) = arg
            .strip_prefix("--engine=")
            .or_else(|| arg.strip_prefix("--引擎="))
        {
            engine = Engine::parse(rest)?;
            i += 1;
        } else if arg == "--engine" || arg == "--引擎" {
            let value = args
                .get(i + 1)
                .ok_or_else(|| "--engine 需要一个值（tree 或 vm）".to_string())?;
            engine = Engine::parse(value)?;
            i += 2;
        } else {
            positional.push(arg.clone());
            i += 1;
        }
    }
    Ok((engine, positional))
}

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
                return Err("用法：fan 运行 <脚本.凡> [--engine=tree|vm] [参数...]".to_string());
            }
            let path = &args[1];
            let (engine, script_args) = split_engine_args(&args[2..])?;
            run_file(path, &script_args, engine)
        }
        "测试" | "test" => {
            let target = args.get(1).map(|s| s.as_str()).unwrap_or(".");
            run_tests(target)
        }
        "检查" | "check" => {
            let target = args.get(1).map(|s| s.as_str()).unwrap_or(".");
            run_check(target)
        }
        "交互" | "repl" => run_repl(BufReader::new(std::io::stdin()), std::io::stdout()),
        "lsp" => run_lsp(),
        "模块" | "mod" => run_mod(&args[1..]),
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
          \x20 fan 运行 <脚本.凡> [--engine=tree|vm] [参数...]   执行脚本（默认 tree 引擎，vm 为字节码虚拟机）\n\
          \x20 fan 测试 [目录或文件]          执行 *_测试.凡 中的测试函数\n\
          \x20 fan 检查 [目录或文件]          静态检查（不运行），报告未定义名、坏导入等\n\
          \x20 fan 交互                      进入交互式求值（REPL）\n\
          \x20 fan 模块 <动作> [参数]         管理 git 源依赖（init/add/install/remove/tidy）\n\
          \x20 fan lsp                       启动语言服务器（LSP/stdio）\n\
          \x20 fan 版本                      输出版本\n\
          \x20 fan 帮助                      输出本帮助\n\
         \n\
         模块搜索目录：脚本所在目录 → 环境变量 FAN_SDK → 可执行文件旁 → 系统安装位置。\n",
        VERSION
    )
}

/// 执行一个脚本文件。
fn run_file(path: &str, script_args: &[String], engine: Engine) -> Result<(), String> {
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
    match engine {
        Engine::Tree => {
            let mut interp = Interpreter::for_script(rt, default_modules_dirs(&dir), &dir, &abs);
            interp.run(&program).map_err(|e| e.display())?;
        }
        Engine::Vm => run_vm(&program, rt, &dir)?,
    }
    Ok(())
}

/// 以栈式字节码虚拟机执行已解析的程序：先 AST→ModuleChunk 编译，再取指执行。
///
/// 注入基于文件系统的模块加载器，使 `导入` 语句可在运行期解析外部纯凡模块；
/// 遇到 fan-code 尚未覆盖的语法会返回明确错误，由调用方提示回退树遍历引擎。
fn run_vm(
    program: &fan_ast::Program,
    rt: Rc<RefCell<Runtime>>,
    script_dir: &str,
) -> Result<(), String> {
    fan_eval::register_builtins(&mut rt.borrow_mut());
    let module = fan_code::compiler::compile_program(program)?;
    // 先建主 VM 取得共享闭包注册表与模块表，再让加载器与最终主 VM 复用同一实例，
    // 保证被导入模块在子 VM 中登记的字节码闭包/模块块能在主 VM 调用时被识别。
    let seed = fan_vm::Vm::new(&module, Rc::clone(&rt));
    let registry = seed.shared_registry();
    let modules = seed.shared_modules();
    let loader = FsModuleLoader::new(
        script_dir,
        Rc::clone(&rt),
        Rc::clone(&registry),
        Rc::clone(&modules),
    );
    let mut vm = fan_vm::Vm::new(&module, rt)
        .with_shared_registry(registry)
        .with_shared_modules(modules)
        .with_loader(Box::new(loader));
    vm.run().map_err(|e| e.to_string())?;
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

/// 静态检查：不运行程序，扫描目标下全部 `.凡`/`.fan` 文件并报告静态诊断。
///
/// 复用编辑器（LSP）同源的 `fan-analysis`：未定义名、坏导入、语法错误、
/// 复合字面量与嵌入歧义等。出现 Error 级诊断时以非零码退出，便于接入 CI 门禁；
/// Warning 级只提示、不影响退出码。
fn run_check(target: &str) -> Result<(), String> {
    let root = std::path::absolute(Path::new(target))
        .map_err(|e| format!("路径 {} 无法解析：{}", target, e))?;
    if !root.exists() {
        return Err(format!("目标不存在：{}", target));
    }
    let paths = discover_fan_files(&root);
    if paths.is_empty() {
        println!("未找到可检查的 .凡/.fan 文件：{}", target);
        return Ok(());
    }

    // 模块导入路径相对「工程根」解析（与运行时 FAN_SDK 语义对齐）：目录模式以
    // 目标目录为根，单文件模式以其所在目录为根。整工程共享同一模块搜索基目录。
    let base: &Path = if root.is_dir() {
        &root
    } else {
        root.parent().unwrap_or(Path::new("."))
    };

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    for path in &paths {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取 {}: {}", path.display(), e))?;
        let uri = fan_analysis::paths::path_to_uri(&path.to_string_lossy());
        let index = fan_analysis::Index::build_with_base(base, &uri, &source);
        let diagnostics = fan_analysis::analyze_index(&index);
        if diagnostics.is_empty() {
            continue;
        }
        println!("{}", path.display());
        for d in &diagnostics {
            match d.severity {
                fan_analysis::Severity::Error => {
                    error_count += 1;
                    println!("  ✗ 第{}行第{}列：{}", d.line, d.column, d.message);
                }
                _ => {
                    warning_count += 1;
                    println!("  · 第{}行第{}列：{}", d.line, d.column, d.message);
                }
            }
        }
    }

    println!(
        "\n扫描 {} 个文件，{} 个问题（错误 {}，警告 {}）",
        paths.len(),
        error_count + warning_count,
        error_count,
        warning_count
    );
    if error_count > 0 {
        return Err("静态检查未通过".to_string());
    }
    Ok(())
}

/// 发现目标下的全部凡语言源文件（含库与测试），按路径排序。
fn discover_fan_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if root.is_file() {
        if is_fan_file(root) {
            paths.push(root.to_path_buf());
        }
        return paths;
    }
    collect_fan_dir(root, &mut paths);
    paths.sort();
    paths
}

fn collect_fan_dir(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.')
                || name == "node_modules"
                || name == "vendor"
                || name == "target"
            {
                continue;
            }
            collect_fan_dir(&path, out);
        } else if is_fan_file(&path) {
            out.push(path);
        }
    }
}

fn is_fan_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.ends_with(".凡") || name.ends_with(".fan")
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

/// 模块管理子命令：`fan 模块 <动作> [参数]`（英文别名 `fan mod`）。
///
/// 委托 fan-mod crate，git 源依赖的声明、锁定与安装。动作：
/// init/add/install/remove/tidy。
fn run_mod(rest: &[String]) -> Result<(), String> {
    let (action, params) = rest
        .split_first()
        .ok_or_else(|| format!("缺少模块管理动作\n\n{}", mod_usage_text()))?;
    let cwd = std::env::current_dir().map_err(|e| format!("无法获取当前目录：{e}"))?;
    let report = match action.as_str() {
        "init" => {
            let name = params.first().cloned();
            fan_mod::init(&cwd, name)?
        }
        "add" => {
            let (name, rest) = params
                .split_first()
                .ok_or("用法：fan 模块 add <名称> <git仓库> [引用]")?;
            let repo = rest
                .first()
                .ok_or("用法：fan 模块 add <名称> <git仓库> [引用]")?;
            let reference = rest.get(1).cloned();
            fan_mod::add(&cwd, name, repo, reference)?
        }
        "install" => fan_mod::install(&cwd)?,
        "update" => fan_mod::update(&cwd, params.to_vec())?,
        "标签" | "tags" => {
            let name = params.first().ok_or("用法：fan 模块 标签 <名称>")?;
            fan_mod::list_tags(&cwd, name)?
        }
        "升级" | "upgrade" => {
            let name = params.first().ok_or("用法：fan 模块 升级 <名称> <tag>")?;
            let tag = params.get(1).ok_or("用法：fan 模块 升级 <名称> <tag>")?;
            fan_mod::upgrade(&cwd, name, tag)?
        }
        "remove" => {
            let name = params.first().ok_or("用法：fan 模块 remove <名称>")?;
            fan_mod::remove(&cwd, name)?
        }
        "tidy" => fan_mod::tidy(&cwd)?,
        other => {
            return Err(format!(
                "未知模块管理动作：{}\n\n{}",
                other,
                mod_usage_text()
            ))
        }
    };
    for line in report.lines {
        println!("{line}");
    }
    Ok(())
}

fn mod_usage_text() -> String {
    "用法：\n\
     \x20 fan 模块 init [名称]                 生成 fan.toml 清单\n\
     \x20 fan 模块 add <名称> <git仓库> [引用]  添加 git 依赖并安装\n\
     \x20 fan 模块 install                     按清单与锁文件安装全部依赖（含传递依赖）\n\
     \x20 fan 模块 update [名称...]            重新解析到最新提交（不带名称=全部）\n\
     \x20 fan 模块 标签 <名称>                 列出该依赖仓库的全部 tag\n\
     \x20 fan 模块 升级 <名称> <tag>           把依赖切换到指定 tag（支持升级/降级）\n\
     \x20 fan 模块 remove <名称>               移除依赖\n\
     \x20 fan 模块 tidy                        检查依赖与导入是否一致"
        .to_string()
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
    fn 解析引擎开关支持中英文与等号写法() {
        let (engine, rest) = split_engine_args(&args(&["--engine=vm", "a", "b"])).unwrap();
        assert_eq!(engine, Engine::Vm);
        assert_eq!(rest, args(&["a", "b"]));

        let (engine, rest) = split_engine_args(&args(&["--engine", "tree", "脚本.凡"])).unwrap();
        assert_eq!(engine, Engine::Tree);
        assert_eq!(rest, args(&["脚本.凡"]));

        let (engine, _) = split_engine_args(&args(&["--引擎=虚拟机"])).unwrap();
        assert_eq!(engine, Engine::Vm);

        let (engine, _) = split_engine_args(&args(&["--引擎", "树"])).unwrap();
        assert_eq!(engine, Engine::Tree);

        // 不带开关时默认树引擎。
        let (engine, rest) = split_engine_args(&args(&["脚本.凡"])).unwrap();
        assert_eq!(engine, Engine::Tree);
        assert_eq!(rest, args(&["脚本.凡"]));
    }

    #[test]
    fn 未知引擎名报错() {
        let err = split_engine_args(&args(&["--engine=llvm"])).unwrap_err();
        assert!(err.contains("未知引擎"), "实际：{err}");
        let err = split_engine_args(&args(&["--engine"])).unwrap_err();
        assert!(err.contains("需要一个值"), "实际：{err}");
    }

    /// 用独立内存输出缓冲跑一个引擎，返回捕获到的 stdout 文本。
    fn run_engine_captured(source: &str, engine: Engine) -> Result<String, String> {
        use std::io::Cursor;
        let (program, errors) = parse_program(source);
        assert!(errors.is_empty(), "测试源码不应有解析错误：{:?}", errors.0);
        let sink: Rc<RefCell<Cursor<Vec<u8>>>> = Rc::new(RefCell::new(Cursor::new(Vec::new())));
        let rt = Rc::new(RefCell::new(Runtime::with_output(sink.clone())));
        match engine {
            Engine::Tree => {
                let mut interp = Interpreter::new_with_runtime(rt);
                interp.run(&program).map_err(|e| e.display())?;
            }
            Engine::Vm => {
                fan_eval::register_builtins(&mut rt.borrow_mut());
                let module = fan_code::compiler::compile_program(&program)?;
                let mut vm = fan_vm::Vm::new(&module, rt);
                vm.run().map_err(|e| e.to_string())?;
            }
        }
        let bytes = sink.borrow().get_ref().clone();
        String::from_utf8(bytes).map_err(|e| format!("输出不是合法 UTF-8：{e}"))
    }

    /// 双引擎对拍：同一源码在 tree 与 vm 下的 stdout 必须逐字节一致。
    /// 仅覆盖 VM 当前支持的 M1 子集；不支持的语法（下标/对象/导入等）不在此列。
    fn assert_engines_agree(source: &str) {
        let tree = run_engine_captured(source, Engine::Tree)
            .unwrap_or_else(|e| panic!("tree 引擎失败：{e}"));
        let vm =
            run_engine_captured(source, Engine::Vm).unwrap_or_else(|e| panic!("vm 引擎失败：{e}"));
        assert_eq!(tree, vm, "双引擎输出不一致\n源码：\n{source}");
    }

    #[test]
    fn 双引擎对拍算术与变量() {
        assert_engines_agree("打印行(1 加 2 乘 3)\n变量 甲 = 10\n打印行(甲 减 4)\n");
    }

    #[test]
    fn 双引擎对拍条件与短路() {
        let src =
            "变量 甲 = 7\n如果 甲 大于 5 那么\n  打印行(\"大\")\n否则\n  打印行(\"小\")\n结束\n\
                   打印行(假 或 真)\n打印行(真 且 假)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍循环() {
        let src = "变量 和 = 0\n变量 序号 = 1\n当 序号 小于等于 5 循环\n  和 = 和 加 序号\n  序号 = 序号 加 1\n结束\n打印行(和)\n\
                   变量 计数 = 0\n重复 4 次\n  计数 = 计数 加 1\n结束\n打印行(计数)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍函数与默认可变参数() {
        let src = "函数 加法(整数 a, 整数 b = 10)\n  返回 a 加 b\n结束\n\
                   打印行(加法(5))\n打印行(加法(5, 6))\n\
                   函数 收集(任意 头, 任意... 尾)\n  打印行(长度(尾))\n结束\n收集(1, 2, 3)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍多返回值解构() {
        let src = "函数 交换(整数 a, 整数 b)\n  返回 b、a\n结束\n\
                   变量 甲、乙 = 交换(1, 2)\n打印行(甲)\n打印行(乙)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍数组下标与切片() {
        let src = "变量 组 = [10, 20, 30, 40]\n\
                   打印行(组[2])\n组[1] = 99\n打印行(组[1])\n\
                   打印行(组[0:2])\n打印行(\"凡语言编程\"[1:3])\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍字典读写() {
        let src = "变量 表 = {\"甲\": 1, \"乙\": 2}\n\
                   打印行(表[\"甲\"])\n表[\"丙\"] = 3\n打印行(表[\"丙\"])\n打印行(表[\"无\"])\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍遍历数组与字符串() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3, 4] 中的 元素 序号\n  和 = 和 加 元素\n结束\n打印行(和)\n\
                   变量 拼接 = \"\"\n遍历 \"凡语\" 中的 字\n  拼接 = 拼接 加 字\n结束\n打印行(拼接)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍遍历字典() {
        let src = "变量 总和 = 0\n遍历 {\"甲\": 1, \"乙\": 2, \"丙\": 3} 中的 键 值\n  总和 = 总和 加 值\n结束\n打印行(总和)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍跳出与继续() {
        let src = "变量 和 = 0\n遍历 [1, 2, 3, 4] 中的 元素\n  如果 元素 等于 2 那么\n    继续\n  结束\n  如果 元素 大于 3 那么\n    跳出\n  结束\n  和 = 和 加 元素\n结束\n打印行(和)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍字典成员访问与赋值() {
        let src = "变量 表 = {\"宽\": 3, \"高\": 4}\n\
                   打印行(表.宽)\n打印行(表 的 高)\n\
                   表.宽 = 30\n打印行(表[\"宽\"])\n\
                   表.新键 = 7\n打印行(表.新键)\n打印行(表.宽 乘 表.高)\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍成员访问跨函数传递() {
        let src = "函数 面积(任意 表)\n  返回 表.宽 乘 表.高\n结束\n\
                   打印行(面积({\"宽\": 5, \"高\": 6}))\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍闭包计数器状态保持() {
        let src = "函数 造计数器()\n  变量 数 = 0\n  函数 增加()\n    数 = 数 加 1\n    返回 数\n  结束\n  返回 增加\n结束\n\
                   变量 计数 = 造计数器()\n打印行(计数())\n打印行(计数())\n打印行(计数())\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍两闭包实例相互独立() {
        let src = "函数 造计数器()\n  变量 数 = 0\n  函数 增加()\n    数 = 数 加 1\n    返回 数\n  结束\n  返回 增加\n结束\n\
                   变量 甲 = 造计数器()\n变量 乙 = 造计数器()\n\
                   打印行(甲())\n打印行(甲())\n打印行(乙())\n打印行(甲())\n";
        assert_engines_agree(src);
    }

    #[test]
    fn 双引擎对拍共享upvalue与多层捕获() {
        let src = "函数 造对()\n  变量 共享 = 10\n  函数 读()\n    返回 共享\n  结束\n\
                   函数 写(整数 值)\n    共享 = 值\n  结束\n  返回 读, 写\n结束\n\
                   变量 读、写 = 造对()\n打印行(读())\n写(99)\n打印行(读())\n";
        assert_engines_agree(src);
        let nested = "函数 外层(整数 基数)\n  函数 中层()\n    函数 内层(整数 增量)\n      返回 基数 加 增量\n    结束\n    返回 内层\n  结束\n  返回 中层()\n结束\n\
                      变量 加五 = 外层(5)\n打印行(加五(3))\n打印行(加五(10))\n";
        assert_engines_agree(nested);
    }

    #[test]
    fn vm数组字面量展开已支持并与tree一致() {
        // 数组字面量展开（...）已在字节码编译器支持，双引擎结果应一致。
        assert_engines_agree("变量 甲 = [1, 2]\n变量 乙 = [0, ...甲, 3]\n打印行(乙)\n");
    }

    #[test]
    fn vm遇到仍不支持的语法返回明确错误() {
        // 接口定义尚未在字节码编译器支持，应给“暂不支持”错误而非 panic。
        let err = run_engine_captured("接口 说话者\n 方法 说话() -> 字符串\n结束\n", Engine::Vm)
            .unwrap_err();
        assert!(err.contains("暂不支持"), "实际：{err}");
    }

    /// 文件级双引擎对拍：在临时目录运行主模块（可含导入），用内存输出 sink
    /// 捕获结果并写入指定文件，供 tree/vm 两次运行后比对。
    fn run_file_engine_captured(path: &str, engine: Engine, out_file: &str) {
        let source = std::fs::read_to_string(path).unwrap();
        let (program, errors) = parse_program(&source);
        assert!(errors.is_empty(), "解析错误：{:?}", errors.0);
        let sink: Rc<RefCell<std::io::Cursor<Vec<u8>>>> =
            Rc::new(RefCell::new(std::io::Cursor::new(Vec::new())));
        let rt = Rc::new(RefCell::new(Runtime::with_output(sink.clone())));
        let abs = abs_path(path);
        let dir = Path::new(&abs)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        match engine {
            Engine::Tree => {
                let mut interp =
                    Interpreter::for_script(rt, default_modules_dirs(&dir), &dir, &abs);
                interp.run(&program).unwrap();
            }
            Engine::Vm => run_vm(&program, rt, &dir).unwrap(),
        }
        let captured = String::from_utf8(sink.borrow().get_ref().clone()).unwrap();
        std::fs::write(out_file, captured).unwrap();
    }

    #[test]
    fn 双引擎对拍文件模块导入与嵌套导入() {
        let dir = temp_dir("import");
        let lib = dir.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        write(
            &lib,
            "运算.凡",
            "公开 变量 版本 = \"1.0\"\n\
             公开 函数 加倍(整数 n) -> 整数\n  返回 n 乘 2\n结束\n\
             公开 函数 相加(整数 a, 整数 b) -> 整数\n  返回 a 加 b\n结束\n",
        );
        let main_path = write(
            &dir,
            "主.凡",
            "导入 \"lib/运算\"\n\
             打印行(运算.加倍(21))\n打印行(运算.版本)\n打印行(运算.相加(3, 4))\n",
        );
        let tree_out = dir.join("tree.txt").to_string_lossy().to_string();
        let vm_out = dir.join("vm.txt").to_string_lossy().to_string();
        run_file_engine_captured(&main_path, Engine::Tree, &tree_out);
        run_file_engine_captured(&main_path, Engine::Vm, &vm_out);
        assert_eq!(
            std::fs::read_to_string(&tree_out).unwrap(),
            std::fs::read_to_string(&vm_out).unwrap(),
            "模块导入双引擎输出不一致"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 双引擎对拍模块别名与嵌套导入() {
        let dir = temp_dir("nested-import");
        let lib = dir.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        write(
            &lib,
            "运算.凡",
            "公开 函数 相加(整数 a, 整数 b) -> 整数\n  返回 a 加 b\n结束\n",
        );
        let lib2 = dir.join("lib2");
        std::fs::create_dir_all(&lib2).unwrap();
        write(
            &lib2,
            "几何.凡",
            "导入 \"lib/运算\" 作为 运\n\
             公开 函数 面积(整数 宽, 整数 高) -> 整数\n  返回 运.相加(宽, 高) 乘 2\n结束\n",
        );
        let main_path = write(
            &dir,
            "主.凡",
            "导入 \"lib2/几何\" 作为 几\n打印行(几.面积(3, 4))\n",
        );
        let tree_out = dir.join("tree.txt").to_string_lossy().to_string();
        let vm_out = dir.join("vm.txt").to_string_lossy().to_string();
        run_file_engine_captured(&main_path, Engine::Tree, &tree_out);
        run_file_engine_captured(&main_path, Engine::Vm, &vm_out);
        assert_eq!(
            std::fs::read_to_string(&tree_out).unwrap(),
            std::fs::read_to_string(&vm_out).unwrap(),
            "嵌套导入双引擎输出不一致"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn 检查干净工程通过且零退出() {
        let dir = temp_dir("check-ok");
        write(
            &dir,
            "好.凡",
            "函数 加一(整数 x) -> 整数\n    返回 x 加 1\n结束\n变量 y = 加一(1)\n",
        );
        assert!(run(args(&["检查", &dir.to_string_lossy()])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 检查坏导入报错且非零退出() {
        let dir = temp_dir("check-import");
        write(
            &dir,
            "主.凡",
            "导入 \"不存在的模块\" 作为 幽灵\n变量 x = 1\n",
        );
        let err = run(args(&["检查", &dir.to_string_lossy()])).unwrap_err();
        assert_eq!(err, "静态检查未通过");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 检查语法错误报错且非零退出() {
        let dir = temp_dir("check-syntax");
        write(&dir, "坏.凡", "变量 x = =\n");
        let err = run(args(&["检查", &dir.to_string_lossy()])).unwrap_err();
        assert_eq!(err, "静态检查未通过");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 检查单文件模式() {
        let dir = temp_dir("check-file");
        let path = write(&dir, "好.凡", "变量 y = 1\n");
        assert!(run(args(&["检查", &path])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 检查不存在的目标报错() {
        let err = run(args(&["检查", "/tmp/opencode/肯定不存在的 fan 目录"])).unwrap_err();
        assert!(err.contains("目标不存在"), "实际：{err}");
    }

    #[test]
    fn 检查空目录正常通过() {
        let dir = temp_dir("check-empty");
        assert!(run(args(&["检查", &dir.to_string_lossy()])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 检查工程根相对导入能解析() {
        // 工程根下 库/子目录里的模块用「库/xxx」根相对导入，应以工程根为搜索基目录，
        // 不应误报找不到模块。
        let dir = temp_dir("check-imports-root");
        std::fs::create_dir_all(dir.join("库")).unwrap();
        write(
            &dir,
            "库/系统.凡",
            "函数 版本号() -> 字符串\n    返回 \"1\"\n结束\n",
        );
        write(
            &dir,
            "库/应用.凡",
            "导入 \"库/系统\" 作为 系统\n变量 v = 系统.版本号()\n",
        );
        assert!(run(args(&["检查", &dir.to_string_lossy()])).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 单文件双引擎执行结果分类。
    #[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
    enum SdkOutcome {
        /// 两引擎都成功且捕获输出逐字节一致。
        Agree,
        /// VM 编译/运行不支持当前语法（含模块加载链上的不支持），按跳过处理。
        VmUnsupported,
        /// 两引擎都成功但输出不一致（真实语义差异，需排查）。
        Diverge,
        /// tree 成功但 VM 返回错误且非“不支持”类（运行期缺陷）。
        VmError,
        /// tree 自身失败（环境/基线问题），不计入 VM 覆盖率。
        TreeError,
    }

    /// 用内存 sink 以指定引擎执行单个源文件，返回 (是否成功, 输出或错误消息)。
    fn run_sdk_file(path: &str, engine: Engine) -> (bool, String) {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => return (false, format!("读取失败：{e}")),
        };
        let (program, errors) = parse_program(&source);
        if !errors.is_empty() {
            return (false, format!("解析失败：{}", errors.0.first().unwrap()));
        }
        let sink: Rc<RefCell<std::io::Cursor<Vec<u8>>>> =
            Rc::new(RefCell::new(std::io::Cursor::new(Vec::new())));
        let rt = Rc::new(RefCell::new(Runtime::with_output(sink.clone())));
        let abs = abs_path(path);
        let dir = Path::new(&abs)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let run_result: Result<(), String> = match engine {
            Engine::Tree => {
                let mut interp =
                    Interpreter::for_script(rt, default_modules_dirs(&dir), &dir, &abs);
                interp.run(&program).map(|_| ()).map_err(|e| e.display())
            }
            Engine::Vm => run_vm(&program, rt, &dir),
        };
        match run_result {
            Ok(()) => (
                true,
                String::from_utf8(sink.borrow().get_ref().clone()).unwrap(),
            ),
            Err(e) => (false, e),
        }
    }

    /// 判断 VM 错误是否属于“语法/能力暂不支持”类（区别于真实运行期缺陷）。
    fn is_unsupported(msg: &str) -> bool {
        const MARKERS: [&str; 7] = [
            "暂不支持",
            "不支持的语句",
            "不支持的表达式",
            "本地原生绑定",
            "尚未在 VM 中实现",
            "VM 暂不支持",
            "未注入模块加载器",
        ];
        MARKERS.iter().any(|m| msg.contains(m))
    }

    fn classify_sdk_file(path: &str) -> SdkOutcome {
        let (tree_ok, tree_out) = run_sdk_file(path, Engine::Tree);
        if !tree_ok {
            return SdkOutcome::TreeError;
        }
        let (vm_ok, vm_out) = run_sdk_file(path, Engine::Vm);
        match vm_ok {
            true if vm_out == tree_out => SdkOutcome::Agree,
            true => SdkOutcome::Diverge,
            false if is_unsupported(&vm_out) => SdkOutcome::VmUnsupported,
            false => SdkOutcome::VmError,
        }
    }

    /// 定位 SDK 测试目录：优先 FAN_SDK，否则跳过（返回 None）。
    fn sdk_test_dir() -> Option<std::path::PathBuf> {
        let root = std::env::var("FAN_SDK").ok()?;
        let dir = std::path::PathBuf::from(root).join("测试");
        dir.is_dir().then_some(dir)
    }

    #[test]
    fn sdk双引擎对拍并量化vm覆盖率() {
        let Some(dir) = sdk_test_dir() else {
            eprintln!("跳过 SDK 对拍：未设置 FAN_SDK 或目录不存在");
            return;
        };
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                n.ends_with("_测试.凡") || n.ends_with("_test.凡")
            })
            .collect();
        files.sort();
        assert!(!files.is_empty(), "SDK 测试目录应至少含一个测试文件");

        let mut counts = std::collections::HashMap::new();
        let mut details: Vec<(String, SdkOutcome, String)> = Vec::new();
        let mut agreed_files: Vec<String> = Vec::new();
        for path in &files {
            let p = path.to_string_lossy().to_string();
            let outcome = classify_sdk_file(&p);
            if matches!(outcome, SdkOutcome::Agree) {
                agreed_files.push(
                    path.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone()),
                );
            }
            if !matches!(outcome, SdkOutcome::Agree | SdkOutcome::VmUnsupported) {
                let (_, msg) = run_sdk_file(&p, Engine::Vm);
                details.push((p.clone(), SdkOutcome::VmError, msg));
            }
            *counts.entry(outcome).or_insert(0u32) += 1;
        }

        let total = files.len() as u32;
        let agree = counts.get(&SdkOutcome::Agree).copied().unwrap_or(0);
        let unsupported = counts.get(&SdkOutcome::VmUnsupported).copied().unwrap_or(0);
        let diverge = counts.get(&SdkOutcome::Diverge).copied().unwrap_or(0);
        let vm_error = counts.get(&SdkOutcome::VmError).copied().unwrap_or(0);
        let tree_error = counts.get(&SdkOutcome::TreeError).copied().unwrap_or(0);
        eprintln!(
            "SDK 双引擎对拍（共 {total} 文件）：一致 {agree}，VM暂不支持 {unsupported}，\
             输出分歧 {diverge}，VM运行错误 {vm_error}，tree基线错误 {tree_error}"
        );
        for (p, _, msg) in &details {
            eprintln!("  [VM错误] {p}：{msg}");
        }
        for name in &agreed_files {
            eprintln!("  [一致] {name}");
        }
        // 硬断言：不允许输出分歧或 VM 运行期错误（暂不支持属预期，不计失败）。
        assert_eq!(diverge, 0, "存在双引擎输出分歧，见上方明细");
        assert_eq!(vm_error, 0, "存在 VM 运行期错误，见上方明细");
        assert_eq!(tree_error, 0, "tree 基线自身失败，检查 FAN_SDK 环境");
    }
}
