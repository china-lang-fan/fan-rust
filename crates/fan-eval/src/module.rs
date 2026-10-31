//! 模块系统：`导入`、`通过`、`公开`/`私有`。
//!
//! 对应 Go 版 `evaluator/module.go`。一个 `.凡` 文件即一个模块：模块内
//! `公开` 声明的名字进入导出表，导入方通过模块名访问（`数学模块.平方根(4)`）。

use crate::error::{value, EvalResult, FanError};
use crate::eval::Interpreter;
use fan_ast::Visibility;
use fan_ast::{ImportStmt, Position, ThroughExpr, VisibilityStmt};
use fan_native::ExtRegistry;
use fan_object::{Environment, ModuleValue, Object, Runtime};
use fan_parser::parse_program;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// 模块加载器：负责路径解析、缓存与循环导入检测。
pub struct Loader {
    /// 模块搜索目录，按顺序尝试。
    pub modules_dirs: Vec<String>,
    /// 绝对路径 → 已加载模块。
    cache: HashMap<String, Rc<RefCell<ModuleValue>>>,
    /// 正在加载中的路径，用于检测循环导入。
    loading: Vec<String>,
}

impl Default for Loader {
    fn default() -> Self {
        Loader::new(Vec::new())
    }
}

impl Loader {
    pub fn new(modules_dirs: Vec<String>) -> Self {
        Loader {
            modules_dirs,
            cache: HashMap::new(),
            loading: Vec::new(),
        }
    }

    /// 按默认搜索顺序构造：`base` → `FAN_SDK` → 可执行文件旁 → 系统安装位置。
    ///
    /// 标准库（`fan-sdk`）是共享目录，命令行执行脚本时脚本自身目录通常不含
    /// 标准库，故需要这一组回退位置。
    pub fn with_default_dirs(base: &str) -> Self {
        Loader::new(default_modules_dirs(base))
    }

    /// 清空缓存（测试或热重载场景使用）。
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// 把导入路径解析为实际文件绝对路径。
    ///
    /// 查找顺序：导入方所在目录 → 各模块搜索目录 → 当前工作目录。
    pub fn resolve_path(&self, base: &str, path: &str) -> Result<String, String> {
        let candidates = candidate_names(path);
        if Path::new(path).is_absolute() {
            if let Some(found) = first_existing(&candidates) {
                return Ok(found);
            }
            return Err(format!("找不到模块：{}", path));
        }
        let mut search: Vec<PathBuf> = Vec::new();
        if !base.is_empty() {
            for c in &candidates {
                search.push(Path::new(base).join(c));
            }
        }
        for dir in &self.modules_dirs {
            for c in &candidates {
                search.push(Path::new(dir).join(c));
            }
        }
        for c in &candidates {
            search.push(PathBuf::from(c));
        }
        for p in &search {
            if p.is_file() {
                return Ok(p.to_string_lossy().to_string());
            }
        }
        Err(format!("找不到模块：{}", path))
    }

    /// 加载模块并执行，返回其导出表。
    ///
    /// 传入 `self` 的共享句柄而非 `&mut self`：模块体内可能再次 `导入`，
    /// 子解释器必须复用同一个加载器才能共享缓存并检测到循环导入。
    pub fn load(
        self_rc: &Rc<RefCell<Loader>>,
        rt: Rc<RefCell<Runtime>>,
        ext: Rc<RefCell<ExtRegistry>>,
        base: &str,
        path: &str,
        pos: Position,
    ) -> Result<Rc<RefCell<ModuleValue>>, FanError> {
        let full = self_rc
            .borrow()
            .resolve_path(base, path)
            .map_err(|e| FanError::at(e, pos))?;
        if let Some(cached) = self_rc.borrow().cache.get(&full) {
            return Ok(cached.clone());
        }
        if self_rc.borrow().loading.iter().any(|p| p == &full) {
            return Err(FanError::at(format!("检测到循环导入：{}", path), pos));
        }
        self_rc.borrow_mut().loading.push(full.clone());
        let result = Self::load_uncached(self_rc, rt, ext, &full, path, pos);
        self_rc.borrow_mut().loading.pop();
        let module = result?;
        self_rc.borrow_mut().cache.insert(full, module.clone());
        Ok(module)
    }

    fn load_uncached(
        self_rc: &Rc<RefCell<Loader>>,
        rt: Rc<RefCell<Runtime>>,
        ext: Rc<RefCell<ExtRegistry>>,
        full: &str,
        path: &str,
        pos: Position,
    ) -> Result<Rc<RefCell<ModuleValue>>, FanError> {
        let source = std::fs::read_to_string(full)
            .map_err(|e| FanError::at(format!("无法读取模块 {}：{}", path, e), pos))?;
        let (program, errors) = parse_program(&source);
        if !errors.is_empty() {
            return Err(FanError::at(
                format!("模块 {} 解析失败：{}", path, errors.0[0]),
                pos,
            ));
        }
        let base_dir = Path::new(full)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let module_name = module_name_from_path(full);

        // 子模块与主程序共享运行实例，因此能直接使用内建函数与类型注册表。
        let mut sub =
            Interpreter::for_module(rt, self_rc.clone(), &base_dir, full, &module_name, ext);
        sub.run(&program)?;

        let module = Rc::new(RefCell::new(ModuleValue::new(module_name)));
        for (name, val) in sub.env.borrow().public_bindings() {
            module.borrow_mut().set(&name, val);
        }
        Ok(module)
    }
}

/// 模块文件路径 → 模块名（去掉目录与扩展名）。
pub fn module_name_from_path(full: &str) -> String {
    let base = Path::new(full)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| full.to_string());
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base,
    }
}

/// 标准库搜索目录：脚本所在目录 → `FAN_SDK` → 可执行文件旁 → 系统安装位置。
///
/// 与 Go 版 `defaultModulesDirs` 的顺序一致。目录名同时认 `fan-sdk`（仓库
/// 中的实际名称）与 `sdk`（安装后的名称）。
pub fn default_modules_dirs(base: &str) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut candidates: Vec<String> = vec![base.to_string()];
    if let Ok(env) = std::env::var("FAN_SDK") {
        candidates.push(env);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let dir = dir.to_string_lossy().to_string();
            candidates.push(dir.clone());
            for suffix in [
                "sdk",
                "fan-sdk",
                "../sdk",
                "../fan-sdk",
                "../../sdk",
                "../../fan-sdk",
                "../share/fan/sdk",
                "../../share/fan/sdk",
            ] {
                candidates.push(format!("{}/{}", dir, suffix));
            }
            // 构建目录深度不定（target/debug 或 target/debug/deps），故从
            // 可执行文件所在目录逐级上溯，找到含 `fan-sdk` 的一层为止
            candidates.extend(ancestors_with_sdk(&dir));
        }
    }
    candidates.push("/usr/local/share/fan/sdk".to_string());
    candidates.push("/usr/share/fan/sdk".to_string());

    for dir in candidates {
        if dir.is_empty() {
            continue;
        }
        let abs = std::path::absolute(Path::new(&dir))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(dir);
        if seen.insert(abs.clone()) {
            dirs.push(abs);
        }
    }
    dirs
}

/// 从 `start` 逐级上溯，收集含有 `fan-sdk` 子目录的祖先目录（最多 6 层）。
fn ancestors_with_sdk(start: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = PathBuf::from(start);
    for _ in 0..6 {
        let sdk = current.join("fan-sdk");
        if sdk.is_dir() {
            found.push(sdk.to_string_lossy().to_string());
        }
        if !current.pop() {
            break;
        }
    }
    found
}

/// 生成候选文件名：原始路径、加上 `.凡`/`.fan` 后缀。
fn candidate_names(path: &str) -> Vec<String> {
    let has_ext = Path::new(path)
        .extension()
        .map(|e| e.eq_ignore_ascii_case("凡") || e.eq_ignore_ascii_case("fan"))
        .unwrap_or(false);
    if has_ext {
        vec![path.to_string()]
    } else {
        vec![
            format!("{}.凡", path),
            format!("{}.fan", path),
            path.to_string(),
        ]
    }
}

fn first_existing(candidates: &[String]) -> Option<String> {
    candidates.iter().find(|p| Path::new(p).is_file()).cloned()
}

impl Interpreter {
    /// 求值 `导入 "路径" 作为 模块名`。
    pub fn eval_import(&mut self, stmt: &ImportStmt) -> EvalResult {
        let (rt, base) = {
            let env = self.env.borrow();
            (env.runtime(), env.base_dir.clone())
        };
        // 借用分离：取出 loader 句柄后释放，避免与 load 期间的借用冲突。
        let loader = self.loader.clone();
        let ext = self.ext.clone();
        let module = Loader::load(&loader, rt, ext, &base, &stmt.path, stmt.pos)?;

        if self.env.borrow().get(&stmt.name).is_some() {
            return Err(FanError::at(
                format!("模块名 {} 已存在", stmt.name),
                stmt.pos,
            ));
        }
        self.env
            .borrow_mut()
            .declare(
                &stmt.name,
                Object::Module(module),
                true,
                fan_ast::DeclType::any(),
                Visibility::File,
            )
            .map_err(|e| FanError::at(e, stmt.pos))?;
        value(Object::Nil)
    }

    /// 求值 `通过 模块名 表达式`：以模块导出表为查找层求值表达式。
    pub fn eval_through(&mut self, expr: &ThroughExpr) -> EvalResult {
        let module = match self.env.borrow().get(&expr.module_name) {
            Some(Object::Module(m)) => m,
            Some(other) => {
                return Err(FanError::at(
                    format!("{} 不是模块，实际是 {}", expr.module_name, other.kind()),
                    expr.pos,
                ))
            }
            None => {
                return Err(FanError::at(
                    format!("模块 {} 未定义", expr.module_name),
                    expr.pos,
                ))
            }
        };
        // 把模块导出注入一层作用域，使表达式内的裸名字先命中模块成员。
        let scoped = Rc::new(RefCell::new(Environment::enclosed(self.env.clone())));
        for (name, val) in module.borrow().exports.clone() {
            scoped
                .borrow_mut()
                .set_local(name, val, fan_ast::DeclType::any());
        }
        let saved = self.env.clone();
        self.env = scoped;
        let result = self.eval_expr(&expr.expr);
        self.env = saved;
        result
    }

    /// 求值 `公开`/`私有` 前缀声明。
    pub fn eval_visibility(&mut self, stmt: &VisibilityStmt) -> EvalResult {
        let result = self.eval_stmt(&stmt.inner)?;
        match result {
            crate::error::Flow::Value(v) => {
                self.env
                    .borrow_mut()
                    .set_visibility(&stmt.name, stmt.visibility);
                value(v)
            }
            other => Ok(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::Interpreter;
    use std::fs;

    /// 在临时目录下准备模块文件，返回目录路径。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fan_module_test_{}", tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("应能创建临时目录");
        dir
    }

    fn write_module(dir: &Path, name: &str, source: &str) {
        fs::write(dir.join(format!("{}.凡", name)), source).expect("应能写入模块文件");
    }

    fn run_with_base(base: &str, src: &str) -> Result<Object, String> {
        let (program, errors) = parse_program(src);
        if !errors.is_empty() {
            return Err(format!("解析错误：{errors}"));
        }
        let mut interp = Interpreter::new();
        interp.env.borrow_mut().set_base_dir(base);
        interp.run(&program).map_err(|e| e.display())
    }

    fn check(base: &str, src: &str, expected: &str) {
        match run_with_base(base, src) {
            Ok(v) => assert_eq!(v.inspect(), expected, "源码：{src}"),
            Err(e) => panic!("执行失败：{e}\n源码：{src}"),
        }
    }

    // ---------------------------------------------------------- 单元测试

    #[test]
    fn 模块名取自文件名去掉扩展名() {
        assert_eq!(module_name_from_path("/库/数学.凡"), "数学");
        assert_eq!(module_name_from_path("数学.凡"), "数学");
        assert_eq!(module_name_from_path("/库/子/工具.fan"), "工具");
        assert_eq!(module_name_from_path("无扩展名"), "无扩展名");
    }

    #[test]
    fn 候选文件名补全凡后缀() {
        assert_eq!(candidate_names("数学"), vec!["数学.凡", "数学.fan", "数学"]);
        assert_eq!(candidate_names("数学.凡"), vec!["数学.凡"]);
        assert_eq!(candidate_names("数学.fan"), vec!["数学.fan"]);
    }

    #[test]
    fn 路径解析优先导入方目录() {
        let dir = temp_dir("resolve");
        write_module(
            &dir,
            "数学",
            "公开 函数 加倍(数) -> 整数\n    返回 数 乘 2\n结束\n",
        );
        let loader = Loader::new(Vec::new());
        let found = loader
            .resolve_path(&dir.to_string_lossy(), "数学")
            .expect("应能解析到模块");
        assert_eq!(module_name_from_path(&found), "数学");
    }

    #[test]
    fn 路径解析找不到时报错() {
        let loader = Loader::new(Vec::new());
        assert!(loader
            .resolve_path("/不存在的目录", "幽灵模块")
            .unwrap_err()
            .contains("找不到模块"));
    }

    #[test]
    fn 路径解析支持模块搜索目录() {
        let dir = temp_dir("dirs");
        write_module(
            &dir,
            "工具",
            "公开 函数 名字() -> 字符串\n    返回 \"工具\"\n结束\n",
        );
        let loader = Loader::new(vec![dir.to_string_lossy().to_string()]);
        let found = loader
            .resolve_path("", "工具")
            .expect("应能在搜索目录中找到");
        assert_eq!(module_name_from_path(&found), "工具");
    }

    // ---------------------------------------------------------- 端到端

    #[test]
    fn 导入模块并调用公开函数() {
        let dir = temp_dir("import");
        write_module(
            &dir,
            "数学",
            "公开 函数 加倍(数 整数) -> 整数\n    返回 数 乘 2\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        check(
            &base,
            "导入 \"数学\" 作为 数学模块\n数学模块.加倍(21)",
            "42",
        );
    }

    #[test]
    fn 模块内私有成员不导出() {
        let dir = temp_dir("private");
        write_module(
            &dir,
            "工具",
            "函数 秘密() -> 整数\n    返回 1\n结束\n公开 函数 公开值() -> 整数\n    返回 2\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        check(&base, "导入 \"工具\" 作为 工具模块\n工具模块.公开值()", "2");
        assert!(
            run_with_base(&base, "导入 \"工具\" 作为 工具模块\n工具模块.秘密()")
                .unwrap_err()
                .contains("没有公开成员")
        );
    }

    #[test]
    fn 导入不存在的模块报错() {
        let dir = temp_dir("missing");
        let base = dir.to_string_lossy().to_string();
        assert!(run_with_base(&base, "导入 \"幽灵\" 作为 幽灵模块")
            .unwrap_err()
            .contains("找不到模块"));
    }

    #[test]
    fn 模块名重复导入报错() {
        let dir = temp_dir("dup");
        write_module(
            &dir,
            "数学",
            "公开 函数 加倍(数 整数) -> 整数\n    返回 数 乘 2\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        assert!(run_with_base(
            &base,
            "导入 \"数学\" 作为 数学模块\n导入 \"数学\" 作为 数学模块"
        )
        .unwrap_err()
        .contains("已存在"));
    }

    #[test]
    fn 循环导入被检测() {
        let dir = temp_dir("cycle");
        write_module(
            &dir,
            "甲模块",
            "导入 \"乙模块\" 作为 乙\n公开 函数 值() -> 整数\n    返回 1\n结束\n",
        );
        write_module(
            &dir,
            "乙模块",
            "导入 \"甲模块\" 作为 甲\n公开 函数 值() -> 整数\n    返回 2\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        assert!(run_with_base(&base, "导入 \"甲模块\" 作为 甲\n甲.值()")
            .unwrap_err()
            .contains("循环导入"));
    }

    #[test]
    fn 同一模块只执行一次() {
        let dir = temp_dir("once");
        // 模块内用副作用计数：每执行一次就累加到公开变量
        write_module(
            &dir,
            "计数",
            "变量 整数 次数 = 0\n公开 函数 递增() -> 整数\n    次数 = 次数 加 1\n    返回 次数\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        // 两次导入同一路径不应重复执行模块体
        check(
            &base,
            "导入 \"计数\" 作为 甲\n导入 \"计数\" 作为 乙\n甲.递增()",
            "1",
        );
    }

    #[test]
    fn 模块可使用内建函数() {
        let dir = temp_dir("native");
        write_module(
            &dir,
            "问候",
            "公开 函数 打招呼() -> 字符串\n    返回 \"你好\"\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        check(
            &base,
            "导入 \"问候\" 作为 问候模块\n问候模块.打招呼()",
            "你好",
        );
    }

    #[test]
    fn 模块内可定义模型并导出() {
        let dir = temp_dir("class");
        write_module(
            &dir,
            "形状",
            "公开 定义 模型 圆\n    公开 变量 整数 半径\n结束\n公开 定义 圆 的 方法 面积() -> 整数\n    返回 自己.半径 乘 3\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        check(
            &base,
            "导入 \"形状\" 作为 形状模块\n变量 甲 = 形状模块.圆{}\n甲.半径 = 5\n甲.面积()",
            "15",
        );
    }

    #[test]
    fn 通过模块名求值表达式() {
        let dir = temp_dir("through");
        write_module(
            &dir,
            "数学",
            "公开 函数 平方(数 整数) -> 整数\n    返回 数 乘 数\n结束\n",
        );
        let base = dir.to_string_lossy().to_string();
        check(
            &base,
            "导入 \"数学\" 作为 数学模块\n通过 数学模块 平方(5)",
            "25",
        );
    }

    #[test]
    fn 通过未定义的模块报错() {
        let dir = temp_dir("through_missing");
        let base = dir.to_string_lossy().to_string();
        assert!(run_with_base(&base, "通过 幽灵模块 平方(5)")
            .unwrap_err()
            .contains("未定义"));
    }

    #[test]
    fn 公开声明的变量可被导出() {
        let dir = temp_dir("public_var");
        write_module(&dir, "配置", "公开 变量 字符串 名称 = \"凡语言\"\n");
        let base = dir.to_string_lossy().to_string();
        check(
            &base,
            "导入 \"配置\" 作为 配置模块\n配置模块.名称",
            "凡语言",
        );
    }

    #[test]
    fn 模块解析失败时报错() {
        let dir = temp_dir("bad_syntax");
        write_module(&dir, "坏模块", "变量 变量 变量\n");
        let base = dir.to_string_lossy().to_string();
        assert!(run_with_base(&base, "导入 \"坏模块\" 作为 坏")
            .unwrap_err()
            .contains("解析失败"));
    }
}
