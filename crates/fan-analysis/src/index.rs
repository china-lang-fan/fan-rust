//! 跨文件符号索引：以入口文件为根，静态解析其导入的本地模块。
//!
//! 对应 Go 版 `analysis.Index`。所有模块只 parse 不执行；找不到文件时
//! 仍返回仅含入口的索引，把缺失留给 `import-not-found` 诊断。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::paths::{path_to_uri, uri_to_path};

/// 跨文件索引。
#[derive(Debug)]
pub struct Index {
    /// 入口文档的 URI。
    pub root_uri: String,
    /// 规范化绝对路径（去扩展名）→ 文档。
    docs: HashMap<String, Document>,
    /// 模块搜索目录（绝对路径）。
    search_dirs: Vec<PathBuf>,
}

impl Index {
    /// 解析入口源码并静态加载它导入的模块。
    pub fn build(uri: &str, source: &str) -> Self {
        let path = uri_to_path(uri);
        let base = Path::new(&path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Self::build_with_base(&base, uri, source)
    }

    /// 指定模块搜索基目录构建索引（测试用）。
    pub fn build_with_base(base: &Path, uri: &str, source: &str) -> Self {
        let path = uri_to_path(uri);
        let search_dirs = module_search_dirs(base);
        let root = Document::parse(uri, source);
        let mut index = Index {
            root_uri: uri.to_string(),
            docs: HashMap::new(),
            search_dirs,
        };
        if !path.is_empty() {
            let key = canonical_path(Path::new(&path));
            index.docs.insert(key, root);
            let mut visited = HashMap::new();
            index.load_imports(uri, &mut visited);
        } else {
            index.docs.insert(uri.to_string(), root);
        }
        index
    }

    /// 入口文档。
    pub fn root(&self) -> &Document {
        self.docs
            .values()
            .find(|d| d.uri == self.root_uri)
            .expect("入口文档始终存在")
    }

    /// 按 URI 取文档。
    pub fn document(&self, uri: &str) -> Option<&Document> {
        self.docs.values().find(|d| d.uri == uri)
    }

    /// 遍历索引内全部文档（入口文档在前）。
    pub fn all_docs(&self) -> impl Iterator<Item = &Document> {
        self.docs.values()
    }

    /// 在任意文档中按模型名查找模型符号。
    pub fn lookup_class_any_doc(&self, name: &str) -> Option<&crate::symbol::Symbol> {
        for doc in self.docs.values() {
            if let Some(sym) = doc.class(name) {
                return Some(sym);
            }
        }
        None
    }

    /// 解析导入路径为实际文件绝对路径；找不到返回 `None`。
    pub fn resolve(&self, import_path: &str) -> Option<String> {
        let candidates = candidate_paths(import_path);
        let try_path = |p: &Path| -> Option<String> {
            let meta = std::fs::metadata(p).ok()?;
            if meta.is_dir() {
                return None;
            }
            Some(canonical_path(p))
        };
        if Path::new(import_path).is_absolute() {
            for c in &candidates {
                if let Some(full) = try_path(Path::new(c)) {
                    return Some(full);
                }
            }
            return None;
        }
        for dir in &self.search_dirs {
            for c in &candidates {
                let joined = dir.join(c);
                if let Some(full) = try_path(&joined) {
                    return Some(full);
                }
            }
        }
        for c in &candidates {
            if let Some(full) = try_path(Path::new(c)) {
                return Some(full);
            }
        }
        None
    }

    fn load_imports(&mut self, uri: &str, visited: &mut HashMap<String, bool>) {
        let imports: Vec<String> = self
            .docs
            .values()
            .find(|d| d.uri == uri)
            .map(|d| d.imports.values().map(|info| info.path.clone()).collect())
            .unwrap_or_default();

        let mut pending = imports;
        // 先收集当前文档的导入，循环中动态发现的模块继续追加。
        let mut start = 0;
        while start < pending.len() {
            let import_path = pending[start].clone();
            start += 1;
            let Some(full) = self.resolve(&import_path) else {
                continue;
            };
            if visited.contains_key(&full) {
                continue;
            }
            visited.insert(full.clone(), true);
            let Ok(data) = std::fs::read_to_string(&full) else {
                continue;
            };
            let module_uri = path_to_uri(&full);
            let module_doc = Document::parse(&module_uri, data);
            let nested: Vec<String> = module_doc
                .imports
                .values()
                .map(|info| info.path.clone())
                .collect();
            self.docs.insert(full, module_doc);
            pending.extend(nested);
        }
    }
}

fn candidate_paths(import_path: &str) -> Vec<String> {
    if import_path.ends_with(".fan") || import_path.ends_with(".凡") {
        return vec![import_path.to_string()];
    }
    vec![
        format!("{import_path}.凡"),
        format!("{import_path}.fan"),
        import_path.to_string(),
    ]
}

fn module_search_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |dir: PathBuf| {
        let abs = std::path::absolute(&dir)
            .map(|p| p.to_path_buf())
            .unwrap_or(dir);
        if seen.insert(abs.clone()) {
            dirs.push(abs);
        }
    };
    add(base.to_path_buf());
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            add(exe_dir.to_path_buf());
            add(exe_dir.join("sdk"));
            add(exe_dir.join("fan-sdk"));
            add(exe_dir.join("../share/fan/sdk"));
            add(exe_dir.join("../../share/fan/sdk"));
        }
    }
    add(PathBuf::from("/usr/local/share/fan/sdk"));
    add(PathBuf::from("/usr/share/fan/sdk"));
    dirs
}

fn canonical_path(path: &Path) -> String {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn 静态解析本地导入模块() {
        let dir = std::env::temp_dir().join("fan_analysis_index_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("工具.凡"),
            "函数 帮助() -> 整数\n    返回 1\n结束\n",
        )
        .unwrap();
        let entry = dir.join("主.凡");
        let source = "导入 \"工具\" 作为 工具模块\n变量 x = 1\n";
        let index = Index::build(&path_to_uri(&entry.to_string_lossy()), source);
        assert!(index.resolve("工具").is_some());
        assert!(index.resolve("不存在的模块").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 找不到文件时仍返回入口索引() {
        let index = Index::build("file:///only.凡", "变量 x = 1\n");
        assert_eq!(index.root().top_levels.len(), 1);
    }
}
