//! `fan 模块` 各子命令的编排逻辑。
//!
//! 依赖以软链形式物化到项目根的 `依赖/<名称>`，指向全局缓存中的 git 工作树；
//! 不支持软链的平台回退为递归复制。运行时 Loader 把每个依赖包根加入模块搜索路径。

use std::path::{Path, PathBuf};

use crate::git::GitCache;
use crate::manifest::{find_project_root, GitDep, LockedPackage, Lockfile, Manifest, DEPS_DIR};

/// 模块管理命令的执行结果，输出给 CLI 打印。
#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
}

impl Report {
    fn say(&mut self, msg: impl Into<String>) {
        self.lines.push(msg.into());
    }
}

/// `fan 模块 init [名称]`：在当前目录生成清单。
pub fn init(dir: &Path, name: Option<String>) -> Result<Report, String> {
    let mut report = Report::default();
    let manifest_path = dir.join(crate::manifest::MANIFEST_NAME);
    if manifest_path.exists() {
        return Err(format!("清单已存在：{}", manifest_path.display()));
    }
    let default_name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "凡项目".to_string())
    });
    let manifest = Manifest {
        项目: crate::manifest::ProjectMeta {
            名称: default_name,
            版本: "0.1.0".to_string(),
            描述: String::new(),
        },
        ..Default::default()
    };
    manifest.save(dir)?;
    report.say(format!("已创建清单 {}", manifest_path.display()));
    Ok(report)
}

/// `fan 模块 add <名称> <仓库> [引用]`：声明依赖、解析 commit、安装并更新锁。
pub fn add(
    dir: &Path,
    name: &str,
    repo: &str,
    reference: Option<String>,
) -> Result<Report, String> {
    let root = require_project(dir)?;
    let mut report = Report::default();
    let mut manifest = Manifest::load(&root)?;
    let dep = GitDep {
        仓库: repo.to_string(),
        引用: reference.clone(),
    };
    manifest.依赖.insert(name.to_string(), dep);
    manifest.save(&root)?;

    let cache = GitCache::new()?;
    install_one(&root, &cache, name, repo, reference.as_deref(), &mut report)?;

    report.say(format!("已添加依赖 {name}"));
    Ok(report)
}

/// `fan 模块 install`：按清单与锁文件安装全部依赖。
pub fn install(dir: &Path) -> Result<Report, String> {
    let root = require_project(dir)?;
    let mut report = Report::default();
    let manifest = Manifest::load(&root)?;
    let mut lock = Lockfile::load(&root)?;
    let cache = GitCache::new()?;

    for (name, dep) in manifest.all_deps() {
        let reference = dep.引用.as_deref();
        // 锁里已有同一引用的 commit 时直接复用，避免无谓的重新解析。
        let commit = match lock.find(&name) {
            Some(locked) if locked.仓库 == dep.仓库 && locked.引用 == dep.引用 => {
                locked.提交.clone()
            }
            _ => {
                let commit = cache.resolve_commit(&dep.仓库, reference)?;
                lock.upsert(LockedPackage {
                    名称: name.clone(),
                    仓库: dep.仓库.clone(),
                    引用: dep.引用.clone(),
                    提交: commit.clone(),
                });
                commit
            }
        };
        materialize(&root, &cache, &dep.仓库, &commit, &name, &mut report)?;
    }

    // 清理锁中已不在清单里的条目。
    let names: Vec<String> = lock.packages.iter().map(|p| p.名称.clone()).collect();
    for name in names {
        if !manifest.all_deps().contains_key(&name) {
            lock.remove(&name);
            remove_link(&root, &name);
        }
    }
    lock.save(&root)?;
    if report.lines.is_empty() {
        report.say("依赖已是最新".to_string());
    }
    Ok(report)
}

/// `fan 模块 remove <名称>`：从清单、锁与依赖目录移除。
pub fn remove(dir: &Path, name: &str) -> Result<Report, String> {
    let root = require_project(dir)?;
    let mut report = Report::default();
    let mut manifest = Manifest::load(&root)?;
    if manifest.依赖.remove(name).is_none() && manifest.开发依赖.remove(name).is_none() {
        return Err(format!("清单中没有依赖 {name}"));
    }
    manifest.save(&root)?;
    let mut lock = Lockfile::load(&root)?;
    lock.remove(name);
    lock.save(&root)?;
    remove_link(&root, name);
    report.say(format!("已移除依赖 {name}"));
    Ok(report)
}

/// `fan 模块 tidy`：扫描源码导入，报告清单中未使用、以及导入了但未声明的依赖。
///
/// 一期只做只读诊断，不自动改清单（删除依赖有破坏性），供使用者据报告手动调整。
pub fn tidy(dir: &Path) -> Result<Report, String> {
    let root = require_project(dir)?;
    let mut report = Report::default();
    let manifest = Manifest::load(&root)?;
    let imported = scan_dependency_imports(&root, &manifest)?;

    for name in manifest.all_deps().keys() {
        if !imported.contains(name) {
            report.say(format!(
                "未使用的依赖：{name}（可执行 fan 模块 remove {name}）"
            ));
        }
    }
    for name in &imported {
        if !manifest.all_deps().contains_key(name) {
            report.say(format!("已导入但未声明：{name}（可执行 fan 模块 add ...）"));
        }
    }
    if report.lines.is_empty() {
        report.say("依赖与导入一致".to_string());
    }
    Ok(report)
}

/// 安装单个依赖：解析 commit、写锁、物化目录。
fn install_one(
    root: &Path,
    cache: &GitCache,
    name: &str,
    repo: &str,
    reference: Option<&str>,
    report: &mut Report,
) -> Result<(), String> {
    let commit = cache.resolve_commit(repo, reference)?;
    let mut lock = Lockfile::load(root)?;
    lock.upsert(LockedPackage {
        名称: name.to_string(),
        仓库: repo.to_string(),
        引用: reference.map(|s| s.to_string()),
        提交: commit.clone(),
    });
    lock.save(root)?;
    materialize(root, cache, repo, &commit, name, report)
}

/// 检出工作树并在项目 `依赖/` 下建立指向它的链接或副本。
fn materialize(
    root: &Path,
    cache: &GitCache,
    repo: &str,
    commit: &str,
    name: &str,
    report: &mut Report,
) -> Result<(), String> {
    let worktree = cache.prepare_worktree(repo, commit)?;
    let link = root.join(DEPS_DIR).join(name);
    if link.exists() || link.is_symlink() {
        std::fs::remove_dir_all(&link)
            .or_else(|_| std::fs::remove_file(&link))
            .map_err(|e| format!("清理旧依赖失败：{e}"))?;
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建依赖目录失败：{e}"))?;
    }
    link_or_copy(&worktree, &link)?;
    report.say(format!("已安装 {name} → {}", link.display()));
    Ok(())
}

fn remove_link(root: &Path, name: &str) {
    let link = root.join(DEPS_DIR).join(name);
    let _ = std::fs::remove_dir_all(&link).or_else(|_| std::fs::remove_file(&link));
}

/// 优先软链（省空间、与缓存共享），平台不支持时回退递归复制。
fn link_or_copy(src: &Path, dst: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(src, dst).is_ok() {
            return Ok(());
        }
    }
    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取目录失败：{e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("复制文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 扫描项目源码中的根相对导入，提取引用到的第三方依赖名。
///
/// 约定：第三方包以 `依赖/<名称>/...` 的根相对路径导入；标准库 `库/...` 与
/// 相对导入（`./`、`../`）不计入。这里用轻量文本规则，避免引入 AST 依赖。
fn scan_dependency_imports(root: &Path, _manifest: &Manifest) -> Result<Vec<String>, String> {
    let mut found = std::collections::BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("读取目录失败：{e}"))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if file_name.starts_with('.')
                    || file_name == DEPS_DIR
                    || file_name == "target"
                    || file_name == "node_modules"
                {
                    continue;
                }
                stack.push(path);
            } else if file_name.ends_with(".凡") || file_name.ends_with(".fan") {
                extract_imports(&path, &mut found)?;
            }
        }
    }
    Ok(found.into_iter().collect())
}

fn extract_imports(
    path: &Path,
    out: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取文件失败：{e}"))?;
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("导入") else {
            continue;
        };
        let quote_start = rest.find('"');
        let quote_end = rest.rfind('"');
        if let (Some(a), Some(b)) = (quote_start, quote_end) {
            if b > a {
                let import_path = &rest[a + 1..b];
                if let Some(name) = dependency_name(import_path) {
                    out.insert(name);
                }
            }
        }
    }
    Ok(())
}

/// 从导入路径提取第三方依赖名；非 `依赖/` 前缀返回 None。
fn dependency_name(import_path: &str) -> Option<String> {
    let normalized = import_path.replace('\\', "/");
    let mut parts = normalized.split('/');
    if parts.next()? != DEPS_DIR {
        return None;
    }
    parts.next().map(|s| s.to_string())
}

fn require_project(dir: &Path) -> Result<PathBuf, String> {
    find_project_root(dir).ok_or_else(|| {
        format!(
            "未找到清单 {}，请先在项目根执行 fan 模块 init",
            crate::manifest::MANIFEST_NAME
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 依赖名从导入路径提取() {
        assert_eq!(
            dependency_name("依赖/工具包/库/字符串").as_deref(),
            Some("工具包")
        );
        assert_eq!(dependency_name("库/系统"), None);
        assert_eq!(dependency_name("./本地"), None);
    }

    #[test]
    fn init生成清单且重复初始化报错() {
        let temp = std::env::temp_dir().join(format!(
            "fan-mod-init-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        assert!(init(&temp, Some("演示".to_string())).is_ok());
        let manifest = Manifest::load(&temp).unwrap();
        assert_eq!(manifest.项目.名称, "演示");
        assert!(init(&temp, None).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
