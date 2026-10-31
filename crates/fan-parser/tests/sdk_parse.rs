//! 共享标准库解析回归测试。
//!
//! `fan-sdk` 是 Go 版与 Rust 版共用的标准库（凡语言源码）。
//! 本测试确保 Rust 解析器能无错解析其中每一个文件——这是“语法不变”的硬约束。

use std::path::{Path, PathBuf};

use fan_parser::parse_program;

/// 定位仓库根的 fan-sdk 目录。
fn sdk_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fan-sdk")
}

/// 收集目录下所有 `.凡` 文件，按名字排序。
fn fan_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("fan-sdk 目录应可读")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("凡"))
        .collect();
    files.sort();
    files
}

#[test]
fn 标准库目录存在且非空() {
    let dir = sdk_dir();
    assert!(dir.is_dir(), "fan-sdk 目录应存在：{dir:?}");
    let files = fan_files(&dir);
    assert!(!files.is_empty(), "fan-sdk 目录应有 .凡 文件");
}

#[test]
fn 标准库全部文件可无错解析() {
    let dir = sdk_dir();
    let mut failed: Vec<String> = Vec::new();
    for path in fan_files(&dir) {
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                failed.push(format!("{}：读取失败 {e}", path.display()));
                continue;
            }
        };
        let (_prog, errors) = parse_program(&source);
        if !errors.is_empty() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            for e in errors.0.iter().take(3) {
                failed.push(format!("{name}：{e}"));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "以下标准库文件解析失败（{} 处）：\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn 标准库每个文件都能解析出语句() {
    let dir = sdk_dir();
    for path in fan_files(&dir) {
        // 跳过纯注释/空文件。
        let source = std::fs::read_to_string(&path).expect("可读");
        let (prog, errors) = parse_program(&source);
        assert!(errors.is_empty(), "{} 解析出错", path.display());
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let is_comment_only = source.lines().all(|l| {
            let t = l.trim();
            t.is_empty() || t.starts_with('#') || t.starts_with("注释")
        });
        if !is_comment_only && !name.contains("_测试") {
            assert!(!prog.is_empty(), "{name} 应解析出语句");
        }
    }
}
