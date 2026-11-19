//! 编译产物（`.fanc` / `.凡码`）的路径约定。
//!
//! 与多数编译型语言一致，产物不污染源码目录，统一写到项目根的 `target/` 下，
//! 镜像源文件的相对路径结构。项目根定义为沿目录上溯、首个含 `target/` 目录或
//! `fan.toml` 清单的目录；找不到时退化为源文件所在目录下的 `target/`。

use std::path::{Path, PathBuf};

use crate::chunk::MANIFEST_MARKER;

/// 源文件是否为凡语言源码（`.fan` 或 `.凡`）。
pub fn is_fan_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("fan") | Some("凡")
    )
}

/// 源码扩展名对应的编译产物扩展名。
pub fn compiled_extension(source: &Path) -> Option<&'static str> {
    match source.extension().and_then(|e| e.to_str()) {
        Some("fan") => Some("fanc"),
        Some("凡") => Some("凡码"),
        _ => None,
    }
}

/// 从源文件路径推导编译产物路径：`<项目根>/target/字节码/<相对路径><新扩展名>`。
pub fn artifact_path(source: &Path) -> Result<PathBuf, String> {
    let ext = compiled_extension(source)
        .ok_or_else(|| format!("不是凡语言源码文件：{}", source.display()))?;
    let project_root = find_project_root(source);
    let rel = source
        .strip_prefix(&project_root)
        .unwrap_or(source)
        .with_extension(ext);
    Ok(project_root.join("target").join("字节码").join(rel))
}

/// 上溯定位项目根：含 `target/` 目录或 `fan.toml` 清单的最近祖先。
fn find_project_root(source: &Path) -> PathBuf {
    let mut current = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    loop {
        if current.join("target").is_dir() || current.join(MANIFEST_MARKER).is_file() {
            return current;
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return source.parent().map(Path::to_path_buf).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 扩展名识别与产物后缀() {
        assert!(is_fan_source(Path::new("主.fan")));
        assert!(is_fan_source(Path::new("主.凡")));
        assert!(!is_fan_source(Path::new("主.txt")));
        assert_eq!(compiled_extension(Path::new("a.fan")), Some("fanc"));
        assert_eq!(compiled_extension(Path::new("a.凡")), Some("凡码"));
        assert_eq!(compiled_extension(Path::new("a.txt")), None);
    }
}
