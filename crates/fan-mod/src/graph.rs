//! 依赖图解析：从根清单出发，递归读取每个依赖包工作树内自带的 `fan.toml`，
//! 解析出包含传递依赖的完整扁平依赖集合。
//!
//! 二期不引入版本求解器，采用确定性规则：
//! - 依赖以「名称」作为唯一键，同名声明必须指向同一仓库；
//! - 同一引用（分支/tag/commit）经多条路径到达时去重，只解析一次；
//! - 同仓库的不同引用若解析到不同 commit，视为冲突并报错（暂不多版本并存）；
//! - 依赖链路成环时报错，并给出完整循环链路。
//!
//! 依赖包可以不带 `fan.toml`（纯代码仓库），此时按叶子包处理。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use crate::git::GitCache;
use crate::manifest::{GitDep, Lockfile, Manifest, MANIFEST_NAME};

/// 解析后的单个包：声明信息加固定到的 commit。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub 名称: String,
    pub 仓库: String,
    pub 引用: Option<String>,
    pub 提交: String,
}

/// 解析模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveMode {
    /// 优先复用锁文件中的 commit，仅在缺失或声明变更时重新解析。
    Normal,
    /// 忽略锁文件，对所有包重新解析（全量 `fan 模块 update`）。
    ForceUpdate,
    /// 仅对集合中的包强制联网重解析，其余包仍优先复用锁（单包 `update <名称>`）。
    ForceOnly(BTreeSet<String>),
}

impl ResolveMode {
    /// 该包是否需要忽略锁、强制重新解析。
    fn forces(&self, name: &str) -> bool {
        match self {
            ResolveMode::Normal => false,
            ResolveMode::ForceUpdate => true,
            ResolveMode::ForceOnly(names) => names.contains(name),
        }
    }

    /// 强制解析时是否必须联网取远端最新（全量/单包强制都需要）。
    fn is_forced(&self) -> bool {
        !matches!(self, ResolveMode::Normal)
    }
}

/// 待解析队列项：包声明加上到达它的祖先链路（用于循环检测与冲突报错）。
struct Pending {
    name: String,
    dep: GitDep,
    chain: Vec<String>,
}

/// 解析根清单声明的全部依赖（含传递依赖），返回按名称排序的扁平结果。
pub fn resolve(
    cache: &GitCache,
    root_deps: &BTreeMap<String, GitDep>,
    lock: &Lockfile,
    mode: &ResolveMode,
) -> Result<Vec<ResolvedPackage>, String> {
    let mut queue = VecDeque::new();
    for (name, dep) in root_deps {
        queue.push_back(Pending {
            name: name.clone(),
            dep: dep.clone(),
            chain: Vec::new(),
        });
    }

    let mut resolved: BTreeMap<String, ResolvedPackage> = BTreeMap::new();
    // 记录每个包首次被发现时的链路，仅用于冲突诊断。
    let mut discovered: BTreeMap<String, Vec<String>> = BTreeMap::new();

    while let Some(Pending { name, dep, chain }) = queue.pop_front() {
        if chain.contains(&name) {
            let mut loop_chain = chain.clone();
            loop_chain.push(name);
            return Err(format!("检测到循环依赖：{}", loop_chain.join(" → ")));
        }

        if let Some(existing) = resolved.get(&name) {
            if existing.仓库 != dep.仓库 {
                return Err(format!(
                    "依赖 {name} 存在仓库冲突：{}（{}）与 {}（{}）。当前仅支持同名依赖指向同一仓库",
                    existing.仓库,
                    chain_text(discovered.get(&name).map(|v| v.as_slice())),
                    dep.仓库,
                    chain_text(Some(&chain))
                ));
            }
            if existing.引用 == dep.引用 {
                // 同仓库同引用：内容寻址下结果必然一致，直接去重。
                continue;
            }
            let commit = resolve_commit(cache, lock, mode, &name, &dep)?;
            if commit != existing.提交 {
                return Err(format!(
                    "依赖 {name} 的不同引用解析到不同提交（{} 与 {}），暂不支持多版本并存",
                    existing.提交, commit
                ));
            }
            continue;
        }

        let commit = resolve_commit(cache, lock, mode, &name, &dep)?;
        let worktree = cache.prepare_worktree(&dep.仓库, &commit)?;
        discovered.insert(name.clone(), chain.clone());
        resolved.insert(
            name.clone(),
            ResolvedPackage {
                名称: name.clone(),
                仓库: dep.仓库.clone(),
                引用: dep.引用.clone(),
                提交: commit.clone(),
            },
        );

        // 读取该包自带清单，把它的依赖（不含开发依赖）加入队列。
        if let Some(package_manifest) = load_package_manifest(&worktree)? {
            for (child_name, child_dep) in &package_manifest.依赖 {
                let mut child_chain = chain.clone();
                child_chain.push(name.clone());
                queue.push_back(Pending {
                    name: child_name.clone(),
                    dep: child_dep.clone(),
                    chain: child_chain,
                });
            }
        }
    }

    Ok(resolved.into_values().collect())
}

/// 按模式决定 commit 来源：非强制且锁匹配时复用锁；否则查 git，强制模式联网取
/// 远端最新，普通解析走离线优先的本地对象。
fn resolve_commit(
    cache: &GitCache,
    lock: &Lockfile,
    mode: &ResolveMode,
    name: &str,
    dep: &GitDep,
) -> Result<String, String> {
    if !mode.forces(name) {
        if let Some(locked) = lock.find(name) {
            if locked.仓库 == dep.仓库 && locked.引用 == dep.引用 {
                return Ok(locked.提交.clone());
            }
        }
    }
    if mode.is_forced() {
        cache.resolve_commit_fresh(&dep.仓库, dep.引用.as_deref())
    } else {
        cache.resolve_commit(&dep.仓库, dep.引用.as_deref())
    }
}

/// 读取依赖包工作树中的清单；没有清单文件时视为无传递依赖的叶子包。
fn load_package_manifest(worktree: &Path) -> Result<Option<Manifest>, String> {
    if !worktree.join(MANIFEST_NAME).is_file() {
        return Ok(None);
    }
    Manifest::load(worktree).map(Some)
}

/// 格式化依赖链路用于诊断；空链路表示根清单。
fn chain_text(chain: Option<&[String]>) -> String {
    match chain {
        Some(chain) if !chain.is_empty() => chain.join(" → "),
        _ => "根清单".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn unique_temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fan-mod-graph-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git(work: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(work)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} 失败");
    }

    /// 在 `base` 下创建一个带清单的本地 git 仓库，返回仓库路径。
    fn make_repo(base: &Path, dir_name: &str, manifest_toml: &str) -> PathBuf {
        let repo = base.join(dir_name);
        std::fs::create_dir_all(repo.join("库")).unwrap();
        std::fs::write(repo.join("fan.toml"), manifest_toml).unwrap();
        std::fs::write(repo.join("库/占位.凡"), "结束\n").unwrap();
        run_git(&repo, &["init"]);
        run_git(&repo, &["config", "user.email", "t@fan"]);
        run_git(&repo, &["config", "user.name", "测试"]);
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "init"]);
        repo
    }

    fn manifest_with_dep(name: &str, repo: &Path) -> String {
        format!(
            "[\"项目\"]\n\"名称\" = \"{name}\"\n\"版本\" = \"0.1.0\"\n\n[\"依赖\".\"{name}\"]\n\"仓库\" = \"{}\"\n",
            repo.display()
        )
    }

    #[test]
    fn 解析多层传递依赖并按名称排序() {
        let base = unique_temp("transitive");
        let repo_c = make_repo(
            &base,
            "C",
            "[\"项目\"]\n\"名称\" = \"C\"\n\"版本\" = \"0.1.0\"\n",
        );
        let repo_b = make_repo(&base, "B", &manifest_with_dep("C", &repo_c));
        let repo_a = make_repo(&base, "A", &manifest_with_dep("B", &repo_b));

        let cache = GitCache::with_root(base.join("缓存"));
        let mut root_deps = BTreeMap::new();
        root_deps.insert(
            "A".to_string(),
            GitDep {
                仓库: repo_a.to_string_lossy().to_string(),
                引用: None,
            },
        );

        let graph = resolve(
            &cache,
            &root_deps,
            &Lockfile::default(),
            &ResolveMode::Normal,
        )
        .unwrap();
        let names: Vec<&str> = graph.iter().map(|p| p.名称.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        assert!(graph.iter().all(|p| p.提交.len() >= 40));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 同名依赖指向不同仓库时报冲突() {
        let base = unique_temp("conflict");
        let shared_one = make_repo(
            &base,
            "共享一",
            "[\"项目\"]\n\"名称\" = \"共享一\"\n\"版本\" = \"0.1.0\"\n",
        );
        let shared_two = make_repo(
            &base,
            "共享二",
            "[\"项目\"]\n\"名称\" = \"共享二\"\n\"版本\" = \"0.1.0\"\n",
        );
        let repo_b = make_repo(&base, "B", &manifest_with_dep("共享", &shared_one));
        let repo_d = make_repo(&base, "D", &manifest_with_dep("共享", &shared_two));

        let cache = GitCache::with_root(base.join("缓存"));
        let mut root_deps = BTreeMap::new();
        for (name, repo) in [("B", &repo_b), ("D", &repo_d)] {
            root_deps.insert(
                name.to_string(),
                GitDep {
                    仓库: repo.to_string_lossy().to_string(),
                    引用: None,
                },
            );
        }

        let err = resolve(
            &cache,
            &root_deps,
            &Lockfile::default(),
            &ResolveMode::Normal,
        )
        .unwrap_err();
        assert!(err.contains("仓库冲突"), "实际报错：{err}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 检测循环依赖并给出链路() {
        let base = unique_temp("cycle");
        let repo_a = base.join("A");
        let repo_b = base.join("B");
        std::fs::create_dir_all(repo_a.join("库")).unwrap();
        std::fs::create_dir_all(repo_b.join("库")).unwrap();
        std::fs::write(repo_a.join("fan.toml"), manifest_with_dep("B", &repo_b)).unwrap();
        std::fs::write(repo_b.join("fan.toml"), manifest_with_dep("A", &repo_a)).unwrap();
        for repo in [&repo_a, &repo_b] {
            run_git(repo, &["init"]);
            run_git(repo, &["config", "user.email", "t@fan"]);
            run_git(repo, &["config", "user.name", "测试"]);
            run_git(repo, &["add", "."]);
            run_git(repo, &["commit", "-m", "init"]);
        }

        let cache = GitCache::with_root(base.join("缓存"));
        let mut root_deps = BTreeMap::new();
        root_deps.insert(
            "A".to_string(),
            GitDep {
                仓库: repo_a.to_string_lossy().to_string(),
                引用: None,
            },
        );

        let err = resolve(
            &cache,
            &root_deps,
            &Lockfile::default(),
            &ResolveMode::Normal,
        )
        .unwrap_err();
        assert!(err.contains("循环依赖"), "实际报错：{err}");
        assert!(err.contains("A → B → A"), "实际报错：{err}");

        let _ = std::fs::remove_dir_all(&base);
    }
}
