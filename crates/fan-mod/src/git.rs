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

    /// 确保本地有该仓库的裸镜像，返回裸仓库路径。
    ///
    /// 离线优先：已镜像时不主动联网，仅在后续操作证明本地缺对象时才由调用方
    /// 触发 `fetch_bare`；首次镜像仍需 clone（联网）。
    fn ensure_bare_offline(&self, url: &str) -> Result<PathBuf, String> {
        let bare = self.bare_dir(url);
        if bare.join("HEAD").exists() {
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

    /// 显式增量拉取所有分支与 tag（联网）。
    fn fetch_bare(&self, url: &str) -> Result<PathBuf, String> {
        let bare = self.ensure_bare_offline(url)?;
        run_git(&bare, &["fetch", "--all", "--tags", "--prune"])?;
        Ok(bare)
    }

    /// 把引用（分支/tag/commit）解析为完整 commit 哈希；引用为空时取本地 HEAD。
    ///
    /// 离线优先：优先用本地镜像对象解析，缺对象时才 fetch 后重试。
    pub fn resolve_commit(&self, url: &str, reference: Option<&str>) -> Result<String, String> {
        let bare = self.ensure_bare_offline(url)?;
        let args: Vec<&str> = match reference {
            Some(r) if !r.is_empty() => vec!["rev-parse", r],
            _ => vec!["rev-parse", "HEAD"],
        };
        match run_git(&bare, &args) {
            Ok(commit) => Ok(commit),
            Err(_) => {
                // 本地可能缺少该引用对应的对象，联网拉取后再试一次。
                let bare = self.fetch_bare(url)?;
                run_git(&bare, &args)
            }
        }
    }

    /// 强制联网刷新后再解析，用于 `update`：浮动引用（分支）取远端最新提交。
    pub fn resolve_commit_fresh(
        &self,
        url: &str,
        reference: Option<&str>,
    ) -> Result<String, String> {
        let bare = self.fetch_bare(url)?;
        match reference {
            Some(r) if !r.is_empty() => run_git(&bare, &["rev-parse", r]),
            _ => run_git(&bare, &["rev-parse", "HEAD"]),
        }
    }

    /// 列出仓库的全部 tag，按版本号语义排序（`v2.0` 排在 `v10.0` 之前）。
    ///
    /// tag 是凡包推荐的发布版本载体：不可变、语义清晰，并由锁文件固定到对应
    /// commit。先尝试联网刷新，断网时回退到本地已缓存的标签。
    pub fn list_tags(&self, url: &str) -> Result<Vec<String>, String> {
        let bare = match self.fetch_bare(url) {
            Ok(bare) => bare,
            Err(_) => self.ensure_bare_offline(url)?,
        };
        let output = run_git(&bare, &["tag", "--list", "--sort=version:refname"])?;
        Ok(output
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect())
    }

    /// 准备某 commit 的工作树目录（幂等），返回其绝对路径。
    ///
    /// 离线优先：目标工作树已存在直接返回；新建时先用本地裸库对象，缺对象才
    /// fetch 重试，因此只要缓存完整，断网也能复现安装。
    pub fn prepare_worktree(&self, url: &str, commit: &str) -> Result<PathBuf, String> {
        let bare = self.ensure_bare_offline(url)?;
        let short = &commit[..commit.len().min(12)];
        let target = self.worktree_base(url).join(format!("@{short}"));
        if target.is_dir() {
            return Ok(target);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建工作树目录失败：{e}"))?;
        }
        // --detach：固定到具体 commit，不跟踪任何分支。
        let args = [
            "worktree",
            "add",
            "--detach",
            &target.to_string_lossy(),
            commit,
        ];
        match run_git(&bare, &args) {
            Ok(_) => Ok(target),
            Err(_) => {
                let bare = self.fetch_bare(url)?;
                run_git(&bare, &args)?;
                Ok(target)
            }
        }
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

    #[test]
    fn 列出仓库标签并按版本排序() {
        let temp = std::env::temp_dir().join(format!(
            "fan-mod-tags-{}-{}",
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
        std::fs::write(source.join("fan.toml"), "[项目]\n名称 = \"x\"\n").unwrap();
        run_git(&source, &["add", "."]).unwrap();
        run_git(&source, &["commit", "-m", "init"]).unwrap();
        // 故意乱序打 tag，验证 version 排序而非字典序（v10 不排在 v2 前）。
        run_git(&source, &["tag", "v10.0.0"]).unwrap();
        run_git(&source, &["tag", "v2.0.0"]).unwrap();
        run_git(&source, &["tag", "v1.0.0"]).unwrap();

        let cache = GitCache::with_root(temp.join("缓存"));
        let url = source.to_string_lossy().to_string();
        let tags = cache.list_tags(&url).unwrap();
        assert_eq!(tags, vec!["v1.0.0", "v2.0.0", "v10.0.0"]);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn 缓存完整时离线可复现且缺对象才联网() {
        let temp = std::env::temp_dir().join(format!(
            "fan-mod-offline-{}-{}",
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
        std::fs::write(source.join("fan.toml"), "[项目]\n名称 = \"x\"\n").unwrap();
        run_git(&source, &["add", "."]).unwrap();
        run_git(&source, &["commit", "-m", "init"]).unwrap();
        let head = run_git(&source, &["rev-parse", "HEAD"]).unwrap();

        let cache = GitCache::with_root(temp.join("缓存"));
        let url = source.to_string_lossy().to_string();
        // 先正常解析并检出，填充裸库与工作树缓存。
        assert_eq!(cache.resolve_commit(&url, None).unwrap(), head);
        let work = cache.prepare_worktree(&url, &head).unwrap();
        assert!(work.join("fan.toml").is_file());

        // 把裸库远端改成不存在的路径，模拟断网/远端失效。
        let bare = cache.bare_dir(&url);
        let poisoned = temp.join("不存在的远端.git").to_string_lossy().to_string();
        run_git(&bare, &["remote", "set-url", "origin", &poisoned]).unwrap();

        // 已缓存的引用与工作树：离线解析/复用都应成功，零网络。
        assert_eq!(cache.resolve_commit(&url, None).unwrap(), head);
        assert_eq!(cache.prepare_worktree(&url, &head).unwrap(), work);
        assert!(cache.list_tags(&url).is_ok());

        // 强制刷新必然联网失败；本地缺失的引用也会尝试联网并失败。
        assert!(cache.resolve_commit_fresh(&url, None).is_err());
        assert!(cache.resolve_commit(&url, Some("不存在的引用xyz")).is_err());

        let _ = std::fs::remove_dir_all(&temp);
    }
}
