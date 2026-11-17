//! 凡语言模块管理工具（fan mod）。
//!
//! 一期聚焦 git 源：声明依赖（`fan.toml`）、锁定 commit（`fan.lock`）、把仓库
//! 检出到全局缓存并在项目 `依赖/` 目录物化。运行时 Loader 再把这些依赖根
//! 注入模块搜索路径，使第三方包与标准库一样可用根相对路径导入。
//!
//! 本 crate 只负责依赖管理，不参与解释执行；运行时的搜索路径改造位于
//! `fan-eval`（Rust）与 fan-go 的 Loader，两端对称。

pub mod commands;
pub mod git;
pub mod graph;
pub mod manifest;

pub use commands::{add, init, install, remove, tidy, update, Report};
pub use graph::{resolve, ResolveMode, ResolvedPackage};
pub use manifest::{find_project_root, GitDep, LockedPackage, Lockfile, Manifest};
