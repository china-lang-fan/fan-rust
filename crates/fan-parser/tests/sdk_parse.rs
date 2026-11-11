//! 共享标准库解析回归测试。
//!
//! `fan-sdk` 是 Go 版与 Rust 版共用的标准库（凡语言源码）。
//! 本测试确保 Rust 解析器能无错解析其中每一个文件——这是“语法不变”的硬约束。

use std::path::{Path, PathBuf};

use fan_parser::parse_program;

/// 定位 fan-sdk 目录。
///
/// 标准库由 Go 版与 Rust 版共用、不在任一实现仓库内，故不能写死相对路径：
/// 先认 `FAN_SDK` 环境变量，再按工作区布局逐级向上找 `fan-sdk`。
/// 与 `fan-eval` 的 sdk_smoke 保持一致，两处都要能同时适配
/// 「标准库在实现仓库内」和「标准库与实现仓库同级」两种布局。
fn sdk_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FAN_SDK") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return dir;
        }
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..4 {
        let candidate = dir.join("fan-sdk");
        if candidate.is_dir() {
            return candidate;
        }
        dir.pop();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fan-sdk")
}

/// 递归收集目录下所有 `.凡` 文件，按路径排序。
///
/// 标准库已按 `库/`（模块）与 `测试/`（测试）分子目录组织，故必须递归遍历，
/// 不能只读取顶层。
fn fan_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("凡") {
            out.push(path);
        }
    }
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
