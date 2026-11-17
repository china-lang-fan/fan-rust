//! 基于系统 `git` 命令行的依赖缓存层。
//!
//! 布局（`$HOME/.fan/cache` 下）：
//! - `裸库/<主机>/<路径>.git`：每个远程仓库的裸镜像，`git fetch` 增量更新；
//! - `工作树/<主机>/<路径>@<短哈希>`：从裸镜像检出的只读工作树，供项目软链引用。
//!
//! 完整性由 git 的内容寻址保证：锁文件固定 commit 哈希，无需额外校验和。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 模块管理的全局缓存根：`$HOME/.fan/cache`（Windows 用 `USERPROFILE`）。
pub fn cache_root() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "无法定位用户主目录：未设置 HOME/USERPROFILE 环境变量".to_string())?;
    Ok(PathBuf::from(home).join(".fan").join("cache"))
}

/// 把仓库 URL 映射为文件系统安全的相对路径（去掉协议、保留主机与路径）。
///
/// 例：`https://github.com/owner/repo.git` → `github.com/owner/repo`。
fn repo_slug(url: &str) -> String {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git@"))
        .unwrap_or(url);
    let normalized = stripped.replace(':', "/");
    normalized
        .strip_suffix(".git")
        .unwrap_or(&normalized)
        .trim_matches('/')
        .to_string()
}

/// 执行一条 git 命令，成功返回 stdout，失败返回带 stderr 的错误。
fn run_git(work: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(work)
        .args(args)
        .output()
        .map_err(|e| format!("无法启动 git（是否已安装并加入 PATH）：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} 失败：{stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// git 依赖缓存：负责镜像仓库与工作树的准备。
pub struct GitCache {
    root: PathBuf,
}

impl GitCache {
    pub fn new() -> Result<Self, String> {
        Ok(GitCache {
            root: cache_root()?,
        })
    }

    /// 仅供测试：指定缓存根构造。
    #[cfg(test)]
    pub fn with_root(root: PathBuf) -> Self {
        GitCache { root }
    }

    fn bare_dir(&self, url: &str) -> PathBuf {
        self.root
            .join("裸库")
            .join(repo_slug(url))
            .with_extension("git")
    }

    fn worktree_base(&self, url: &str) -> PathBuf {
        self.root.join("工作树").join(repo_slug(url))
    }

    /// 确保本地有该仓库的最新裸镜像，返回裸仓库路径。
    fn ensure_bare(&self, url: &str) -> Result<PathBuf, String> {
        let bare = self.bare_dir(url);
        if bare.join("HEAD").exists() {
            // 已镜像：增量拉取所有分支与 tag。
            run_git(&bare, &["fetch", "--all", "--tags", "--prune"])?;
            return Ok(bare);
        }
        if let Some(parent) = bare.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建缓存目录失败：{e}"))?;
        }
        run_git(
            self.root.as_path(),
            &["clone", "--mirror", url, &bare.to_string_lossy()],
        )?;
        Ok(bare)
    }

    /// 把引用（分支/tag/commit）解析为完整 commit 哈希；引用为空时取默认分支 HEAD。
    pub fn resolve_commit(&self, url: &str, reference: Option<&str>) -> Result<String, String> {
        let bare = self.ensure_bare(url)?;
        match reference {
            Some(r) if !r.is_empty() => {
                let full = run_git(&bare, &["rev-parse", r])?;
                Ok(full)
            }
            _ => run_git(&bare, &["rev-parse", "HEAD"]),
        }
    }

    /// 准备某 commit 的工作树目录（幂等），返回其绝对路径。
    pub fn prepare_worktree(&self, url: &str, commit: &str) -> Result<PathBuf, String> {
        let bare = self.ensure_bare(url)?;
        let short = &commit[..commit.len().min(12)];
        let target = self.worktree_base(url).join(format!("@{short}"));
        if target.is_dir() {
            return Ok(target);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建工作树目录失败：{e}"))?;
        }
        // --detach：固定到具体 commit，不跟踪任何分支。
        run_git(
            &bare,
            &[
                "worktree",
                "add",
                "--detach",
                &target.to_string_lossy(),
                commit,
            ],
        )?;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 仓库地址映射为安全相对路径() {
        assert_eq!(
            repo_slug("https://github.com/owner/repo.git"),
            "github.com/owner/repo"
        );
        assert_eq!(
            repo_slug("git@github.com:owner/repo.git"),
            "github.com/owner/repo"
        );
        assert_eq!(repo_slug("https://gitea.cn/a/b"), "gitea.cn/a/b");
    }

    #[test]
    fn 本地仓库可解析提交并检出工作树() {
        let temp = std::env::temp_dir().join(format!(
            "fan-mod-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let source = temp.join("源仓库");
        std::fs::create_dir_all(&source).unwrap();
        run_git(&source, &["init"]).unwrap();
        run_git(&source, &["config", "user.email", "t@fan"]).unwrap();
        run_git(&source, &["config", "user.name", "测试"]).unwrap();
        std::fs::write(
            source.join("fan.toml"),
            "[项目]\n名称 = \"x\"\n版本 = \"0.1.0\"\n",
        )
        .unwrap();
        run_git(&source, &["add", "."]).unwrap();
        run_git(&source, &["commit", "-m", "init"]).unwrap();
        let head = run_git(&source, &["rev-parse", "HEAD"]).unwrap();

        let cache = GitCache::with_root(temp.join("缓存"));
        let url = source.to_string_lossy().to_string();
        let resolved = cache.resolve_commit(&url, None).unwrap();
        assert_eq!(resolved, head);
        let work = cache.prepare_worktree(&url, &resolved).unwrap();
        assert!(work.join("fan.toml").is_file());
        // 幂等：再次准备应返回同一目录且不报错。
        let again = cache.prepare_worktree(&url, &resolved).unwrap();
        assert_eq!(work, again);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
