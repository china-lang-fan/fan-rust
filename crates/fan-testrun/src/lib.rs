//! 凡语言测试运行器：发现 `*_测试.凡` 并执行其中的测试函数。
//!
//! 对应 Go 版 `internal/testrun`。判定规则与之一致：
//!
//! - 文件：名字以 `_测试.凡` / `_test.凡` / `_测试.fan` / `_test.fan` 结尾
//! - 用例：顶层声明、值是一个函数字面量，且名字以 `测试`/`Test` 开头，
//!   或函数上带 `@测试`/`@test` 标签
//! - 执行：逐个无参调用；抛出错误即记为失败
//!
//! 测试文件先整体求值一遍（顶层 `导入`、辅助函数等），再逐个调用用例，
//! 因此用例之间共享同一份环境。

use fan_ast::{Expr, Ident, Program, Stmt, TagExpr};
use fan_eval::{default_modules_dirs, FanError, Interpreter};
use fan_object::{Object, Runtime};
use fan_parser::parse_program;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// 一个测试用例。
#[derive(Debug, Clone)]
pub struct TestCase {
    pub name: String,
    pub line: usize,
    pub column: usize,
}

/// 一次失败。
#[derive(Debug, Clone)]
pub struct Failure {
    pub name: String,
    pub line: usize,
    pub column: usize,
    pub reason: String,
}

/// 单个测试文件的执行结果。
#[derive(Debug, Clone)]
pub struct FileResult {
    pub path: String,
    pub tests: Vec<TestCase>,
    pub failures: Vec<Failure>,
    /// 文件级错误（解析失败、求值失败等），非空则整个文件无法继续。
    pub setup_error: String,
}

impl FileResult {
    /// 该文件是否有失败。
    pub fn failed(&self) -> bool {
        !self.setup_error.is_empty() || !self.failures.is_empty()
    }
}

/// 一批结果中是否存在失败。
pub fn has_failure(results: &[FileResult]) -> bool {
    results.iter().any(|r| r.failed())
}

/// 统计信息。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub passed: usize,
    pub failed: usize,
}

/// 测试运行器。
pub struct Runner;

impl Runner {
    /// 在 `target`（文件或目录）下发现并执行测试。
    pub fn run(target: &str) -> Result<Vec<FileResult>, String> {
        let root = std::path::absolute(Path::new(target))
            .map_err(|e| format!("路径 {} 无法解析：{}", target, e))?;
        if !root.exists() {
            return Err(format!("目标不存在：{}", target));
        }
        let paths = discover_files(&root);
        let results: Vec<FileResult> = paths.iter().map(|p| run_file(p)).collect();
        Ok(results)
    }

    /// 汇总统计。
    pub fn summary(results: &[FileResult]) -> Summary {
        let mut summary = Summary {
            files: results.len(),
            ..Summary::default()
        };
        for r in results {
            let failed = r.failures.len() + if r.setup_error.is_empty() { 0 } else { 1 };
            summary.failed += failed;
            summary.passed += r.tests.len().saturating_sub(r.failures.len());
        }
        summary
    }
}

/// 找出目标下的全部测试文件，按路径排序。
fn discover_files(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if root.is_file() {
        if is_test_file(root) {
            paths.push(root.to_path_buf());
        }
        return paths;
    }
    collect_dir(root, &mut paths);
    paths.sort();
    paths
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // 跳过隐藏目录与依赖目录
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "node_modules" || name == "vendor" {
                continue;
            }
            collect_dir(&path, out);
        } else if is_test_file(&path) {
            out.push(path);
        }
    }
}

/// 是否为测试文件。
pub fn is_test_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    ["_测试.凡", "_test.凡", "_测试.fan", "_test.fan"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// 执行单个测试文件。
pub fn run_file(path: &Path) -> FileResult {
    let display = path.to_string_lossy().to_string();
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return FileResult {
                path: display,
                tests: Vec::new(),
                failures: Vec::new(),
                setup_error: format!("无法读取：{}", e),
            }
        }
    };
    let (program, errors) = parse_program(&source);
    if !errors.is_empty() {
        let messages: Vec<String> = errors.0.iter().map(|e| e.to_string()).collect();
        return FileResult {
            path: display,
            tests: Vec::new(),
            failures: Vec::new(),
            setup_error: messages.join("\n"),
        };
    }

    let dir = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let rt = Rc::new(RefCell::new(Runtime::new()));
    rt.borrow_mut()
        .set_process_context(display.clone(), Vec::new());
    let mut interp = Interpreter::for_script(rt, default_modules_dirs(&dir), &dir, &display);

    if let Err(e) = interp.run(&program) {
        return FileResult {
            path: display,
            tests: Vec::new(),
            failures: Vec::new(),
            setup_error: e.display(),
        };
    }

    let tests = collect_tests(&program);
    let mut failures = Vec::new();
    for case in &tests {
        if let Err(e) = call_test(&mut interp, case) {
            failures.push(Failure {
                name: case.name.clone(),
                line: case.line,
                column: case.column,
                reason: e.display(),
            });
        }
    }
    FileResult {
        path: display,
        tests: tests.clone(),
        failures,
        setup_error: String::new(),
    }
}

/// 从顶层声明里收集测试用例。
fn collect_tests(program: &Program) -> Vec<TestCase> {
    let mut cases = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for stmt in &program.statements {
        let Some((name, pos, _lit)) = test_from_statement(stmt) else {
            continue;
        };
        // 同名只取一次（后声明的会覆盖先声明的绑定）
        if !seen.insert(name.clone()) {
            continue;
        }
        cases.push(TestCase {
            name,
            line: pos.line,
            column: pos.column,
        });
    }
    cases
}

/// 识别一条语句是否为测试声明，返回 (名字, 位置, 函数字面量)。
fn test_from_statement(stmt: &Stmt) -> Option<(String, fan_ast::Position, &fan_ast::FunctionLit)> {
    match stmt {
        Stmt::VarDecl(v) => {
            let Expr::Function(lit) = v.value.as_ref()? else {
                return None;
            };
            if is_test_function(&v.name, lit) {
                Some((v.name.clone(), v.pos, lit))
            } else {
                None
            }
        }
        Stmt::Visibility(v) => test_from_statement(&v.inner),
        _ => None,
    }
}

/// 名字以 `测试`/`Test` 开头，或带 `@测试`/`@test` 标签。
fn is_test_function(name: &str, lit: &fan_ast::FunctionLit) -> bool {
    if name.starts_with("测试") || name.starts_with("Test") {
        return true;
    }
    lit.tags
        .iter()
        .any(|t: &TagExpr| t.type_name == "测试" || t.type_name == "test")
}

/// 构造一次对测试函数的无参调用并执行。
fn call_test(interp: &mut Interpreter, case: &TestCase) -> Result<Object, FanError> {
    let pos = fan_ast::Position {
        line: case.line,
        column: case.column,
    };
    let call = Expr::Call(fan_ast::CallExpr {
        pos,
        callee: Box::new(Expr::Ident(Ident {
            pos,
            name: case.name.clone(),
        })),
        args: Vec::new(),
        mixfix_root: None,
    });
    let program = Program::new(vec![Stmt::Expr(fan_ast::ExpressionStmt {
        pos,
        expr: call,
    })]);
    interp.run(&program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fan-testrun-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 识别测试文件名() {
        assert!(is_test_file(Path::new("a/数组_测试.凡")));
        assert!(is_test_file(Path::new("a_test.fan")));
        assert!(!is_test_file(Path::new("a/数组.凡")));
    }

    #[test]
    fn 通过的函数不计失败() {
        let dir = temp_dir("pass");
        let path = write(&dir, "甲_测试.凡", "函数 测试甲()\n    变量 值 = 1\n结束\n");
        let result = run_file(&path);
        assert_eq!(result.setup_error, "");
        assert_eq!(result.tests.len(), 1);
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert!(!result.failed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 断言失败记为失败() {
        // 刻意不依赖标准库：单元测试不应受外部 fan-sdk 部署位置影响
        let dir = temp_dir("fail");
        let path = write(
            &dir,
            "乙_测试.凡",
            "函数 测试乙()\n    崩溃了(\"故意失败\")\n结束\n",
        );
        let result = run_file(&path);
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.failures.len(), 1);
        assert!(
            result.failures[0].reason.contains("故意失败"),
            "实际：{}",
            result.failures[0].reason
        );
        assert!(result.failed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 解析失败记为文件级错误() {
        let dir = temp_dir("parse");
        let path = write(&dir, "丙_测试.凡", "变量 = = =\n");
        let result = run_file(&path);
        assert!(!result.setup_error.is_empty(), "应有文件级错误");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 非测试函数不被收集() {
        let dir = temp_dir("skip");
        let path = write(
            &dir,
            "丁_测试.凡",
            "函数 辅助()\n    返回 1\n结束\n函数 测试丁()\n    变量 值 = 辅助()\n结束\n",
        );
        let result = run_file(&path);
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].name, "测试丁");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 目录发现与统计() {
        let dir = temp_dir("dir");
        write(&dir, "甲_测试.凡", "函数 测试甲()\n    变量 值 = 1\n结束\n");
        write(&dir, "非测试.凡", "变量 值 = 1\n");
        let results = Runner::run(&dir.to_string_lossy()).unwrap();
        assert_eq!(results.len(), 1, "只应发现测试文件");
        let summary = Runner::summary(&results);
        assert_eq!(summary.files, 1);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0);
        assert!(!has_failure(&results));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 目标不存在时报错() {
        let err = Runner::run("绝不存在的目录-凡").unwrap_err();
        assert!(err.contains("目标不存在"), "实际：{}", err);
    }
}
