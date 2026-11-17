//! 项目清单 `凡.toml` 与锁文件 `凡.lock` 的数据模型与读写。
//!
//! 一期只支持 git 源：每个依赖以内联表声明仓库地址与版本引用（分支、tag 或
//! commit）。锁文件把每个依赖固定到具体 commit，保证可复现安装。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 项目清单文件名（位于项目根）。
pub const MANIFEST_NAME: &str = "凡.toml";
/// 锁文件名（位于项目根，由工具生成，应提交到版本库）。
pub const LOCK_NAME: &str = "凡.lock";
/// 项目内第三方依赖的物化目录名。
pub const DEPS_DIR: &str = "依赖";

/// `凡.toml` 的 `[项目]` 段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectMeta {
    #[serde(default)]
    pub 名称: String,
    #[serde(default)]
    pub 版本: String,
    #[serde(default)]
    pub 描述: String,
}

/// 单个 git 依赖的声明。
///
/// `引用` 可以是分支名、tag 名或 commit 哈希；缺省时由 git 取远端默认分支。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitDep {
    pub 仓库: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub 引用: Option<String>,
}

/// 项目清单。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Manifest {
    pub 项目: ProjectMeta,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub 依赖: BTreeMap<String, GitDep>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub 开发依赖: BTreeMap<String, GitDep>,
}

/// 锁文件中单个包的解析结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedPackage {
    pub 名称: String,
    pub 仓库: String,
    /// 锁定时的引用（分支/tag/commit），仅用于展示与重新解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub 引用: Option<String>,
    /// 固定到的 commit 哈希。
    pub 提交: String,
}

/// 锁文件 `凡.lock`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Lockfile {
    #[serde(default, rename = "包")]
    pub packages: Vec<LockedPackage>,
}

impl Manifest {
    /// 从项目根读取并解析清单。
    pub fn load(project_root: &Path) -> Result<Manifest, String> {
        let path = project_root.join(MANIFEST_NAME);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取清单 {}：{}", path.display(), e))?;
        toml::from_str(&text).map_err(|e| format!("清单 {} 解析失败：{}", path.display(), e))
    }

    /// 序列化为 TOML 文本。
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| format!("清单序列化失败：{e}"))
    }

    /// 写入项目根的清单文件。
    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = project_root.join(MANIFEST_NAME);
        std::fs::write(&path, self.to_toml()?)
            .map_err(|e| format!("无法写入清单 {}：{}", path.display(), e))
    }

    /// 合并开发依赖后，返回全部依赖（名称 → 声明）。
    pub fn all_deps(&self) -> BTreeMap<String, GitDep> {
        let mut merged = self.依赖.clone();
        for (name, dep) in &self.开发依赖 {
            merged.insert(name.clone(), dep.clone());
        }
        merged
    }
}

impl Lockfile {
    /// 从项目根读取锁文件；不存在时返回空锁（等价于尚未安装）。
    pub fn load(project_root: &Path) -> Result<Lockfile, String> {
        let path = project_root.join(LOCK_NAME);
        if !path.exists() {
            return Ok(Lockfile::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取锁文件 {}：{}", path.display(), e))?;
        toml::from_str(&text).map_err(|e| format!("锁文件 {} 解析失败：{}", path.display(), e))
    }

    /// 序列化并写入项目根。
    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = project_root.join(LOCK_NAME);
        let text = toml::to_string_pretty(self).map_err(|e| format!("锁文件序列化失败：{e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("无法写入锁文件 {}：{}", path.display(), e))
    }

    /// 按名称查找锁定项。
    pub fn find(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.名称 == name)
    }

    /// 用新的解析结果替换同名锁定项（不存在则追加），保持按名称排序。
    pub fn upsert(&mut self, pkg: LockedPackage) {
        if let Some(slot) = self.packages.iter_mut().find(|p| p.名称 == pkg.名称) {
            *slot = pkg;
        } else {
            self.packages.push(pkg);
        }
        self.packages.sort_by(|a, b| a.名称.cmp(&b.名称));
    }

    /// 移除指定名称的锁定项。
    pub fn remove(&mut self, name: &str) {
        self.packages.retain(|p| p.名称 != name);
    }
}

/// 向上逐级查找含 `凡.toml` 的目录，作为项目根；找不到返回 `None`。
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::path::absolute(start).ok()?
    };
    if current.is_file() {
        current = current.parent()?.to_path_buf();
    }
    loop {
        if current.join(MANIFEST_NAME).is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}
