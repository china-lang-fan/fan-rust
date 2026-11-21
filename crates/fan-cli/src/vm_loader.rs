//! 栈式 VM 的模块加载器（装配层实现）。
//!
//! fan-vm 自身不含 parser/文件系统，本模块在 fan-cli 内补齐「按路径找文件 →
//! 解析 → fan-code 编译 → 子 VM 执行入口 → 收集公开导出」的完整链路，并通过
//! 共享的闭包注册表让被导入模块的函数能在主模块中被调用。
//!
//! 范围（本里程碑）：纯凡模块——顶层为函数与变量声明、可嵌套导入；不支持
//! `定义 本地` 原生绑定、模型/接口等 fan-code 尚未覆盖的语法。搜索顺序与
//! 树遍历求值器的 [`fan_eval::default_modules_dirs`] 保持一致。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fan_code::compiler::compile_program;
use fan_object::{ModuleValue, Object, Runtime};
use fan_parser::parse_program;
use fan_vm::{ModuleId, ModuleLoader, SharedModules, SharedRegistry, Vm, VmError, VmResult};

use fan_eval::default_modules_dirs;

/// 跨嵌套加载共享的可变核心：缓存、模块块所有权、ID 分配、环检测。
struct Core {
    search_dirs: Vec<String>,
    runtime: Rc<RefCell<Runtime>>,
    registry: SharedRegistry,
    /// 主/子 VM 共享的模块表，编译后的模块块以 Rc 登记其中。
    modules: SharedModules,
    path_to_id: HashMap<String, ModuleId>,
    loaded: HashMap<String, Object>,
    loading: Vec<String>,
    next_id: ModuleId,
}

/// 基于文件系统的 VM 模块加载器。多个代理（子 VM 各持一个）共享同一 [`Core`]。
pub struct FsModuleLoader {
    core: Rc<RefCell<Core>>,
}

impl FsModuleLoader {
    pub fn new(
        script_dir: &str,
        runtime: Rc<RefCell<Runtime>>,
        registry: SharedRegistry,
        modules: SharedModules,
    ) -> Self {
        let core = Core {
            search_dirs: default_modules_dirs(script_dir),
            runtime,
            registry,
            modules,
            path_to_id: HashMap::new(),
            loaded: HashMap::new(),
            loading: Vec::new(),
            next_id: 1,
        };
        FsModuleLoader {
            core: Rc::new(RefCell::new(core)),
        }
    }

    /// 构造一个共享同一核心的代理，供子 VM 作为其加载器使用。
    fn proxy(&self) -> FsModuleLoader {
        FsModuleLoader {
            core: Rc::clone(&self.core),
        }
    }

    fn resolve_path(search_dirs: &[String], path: &str) -> Result<String, String> {
        let candidates = candidate_names(path);
        if Path::new(path).is_absolute() {
            return candidates
                .into_iter()
                .find(|p| Path::new(p).is_file())
                .ok_or_else(|| format!("找不到模块：{path}"));
        }
        let mut search: Vec<PathBuf> = Vec::new();
        for dir in search_dirs {
            for c in &candidates {
                search.push(Path::new(dir).join(c));
            }
        }
        for c in &candidates {
            search.push(PathBuf::from(c));
        }
        search
            .into_iter()
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().to_string())
            .ok_or_else(|| format!("找不到模块：{path}"))
    }
}

impl ModuleLoader for FsModuleLoader {
    fn load(&mut self, path: &str) -> VmResult<Object> {
        // 第一阶段：在不持有 core 借用的前提下完成路径解析与环检测/缓存判定。
        let full = {
            let core = self.core.borrow();
            Self::resolve_path(&core.search_dirs, path).map_err(VmError::new)?
        };
        if let Some(cached) = self.core.borrow().loaded.get(&full).cloned() {
            return Ok(cached);
        }
        if self.core.borrow().loading.iter().any(|p| p == &full) {
            return Err(VmError::new(format!("检测到循环导入：{path}")));
        }
        self.core.borrow_mut().loading.push(full.clone());

        // 第二阶段：编译 + 执行（可能经子 VM 递归触发 load，复用同一核心）。
        let outcome = self.execute(&full);

        // 严格弹栈：仅移除本次压入的末尾项，保留嵌套加载的环检测状态。
        let mut core = self.core.borrow_mut();
        if core.loading.last() == Some(&full) {
            core.loading.pop();
        } else {
            core.loading.retain(|p| p != &full);
        }
        let module = outcome?;
        core.loaded.insert(full, module.clone());
        Ok(module)
    }
}

impl FsModuleLoader {
    fn execute(&mut self, full: &str) -> VmResult<Object> {
        // 编译并分配模块 ID，注册到共享模块表。
        let id = {
            let core = self.core.borrow();
            core.path_to_id.get(full).copied()
        };
        let id = match id {
            Some(id) => id,
            None => {
                let source = std::fs::read_to_string(full)
                    .map_err(|e| VmError::new(format!("读取模块 {full} 失败：{e}")))?;
                let (program, errors) = parse_program(&source);
                if !errors.is_empty() {
                    return Err(VmError::new(format!(
                        "模块 {full} 解析失败：{}",
                        errors.0.first().expect("至少一个解析错误")
                    )));
                }
                let chunk = compile_program(&program).map_err(VmError::new)?;
                let mut core = self.core.borrow_mut();
                let id = core.next_id;
                core.next_id += 1;
                core.modules.borrow_mut().insert(id, Rc::new(chunk.clone()));
                core.path_to_id.insert(full.to_string(), id);
                id
            }
        };

        // 取出执行所需的快照，避免在子 VM 运行期间持有 core 借用。
        let (chunk, runtime, registry, modules) = {
            let core = self.core.borrow();
            let chunk = Rc::clone(core.modules.borrow().get(&id).expect("模块已注册"));
            (
                chunk,
                Rc::clone(&core.runtime),
                Rc::clone(&core.registry),
                Rc::clone(&core.modules),
            )
        };
        let exports = chunk.exports.clone();
        let top_slots = chunk.top_slots.clone();
        let native_exports = chunk.native_exports.clone();
        let name = module_name(full);

        // 子 VM：共享运行时、闭包注册表、模块表，加载器用同核心代理支持嵌套导入。
        // 入口帧以真实模块 id 执行，避免误用主模块（id=0）的入口指令。
        let proxy = self.proxy();
        let mut vm = Vm::new(&chunk, runtime.clone())
            .with_shared_registry(registry)
            .with_shared_modules(modules)
            .with_loader(Box::new(proxy));
        let base = vm.slots_len();
        vm.run_module(id)?;

        // 入口帧已弹出，槽位保留；按入口基址读顶层槽，筛选公开导出。
        let mut module = ModuleValue::new(name);
        for export in &exports {
            // `定义 公开 本地(...)` 的原生别名不占运行期槽值，直接从 Runtime 取真实 Builtin。
            if let Some((_, spec)) = native_exports.iter().find(|(alias, _)| alias == export) {
                if let Some(builtin) = runtime.borrow().lookup_native(spec) {
                    module.set(export.clone(), Object::Builtin(builtin));
                    continue;
                }
            }
            if let Some((_, slot)) = top_slots.iter().find(|(n, _)| n == export) {
                let value = vm.entry_slot(base, *slot);
                module.set(export.clone(), value);
            }
        }
        Ok(Object::Module(Rc::new(RefCell::new(module))))
    }
}

fn candidate_names(path: &str) -> Vec<String> {
    let has_ext = Path::new(path)
        .extension()
        .map(|e| e.eq_ignore_ascii_case("凡") || e.eq_ignore_ascii_case("fan"))
        .unwrap_or(false);
    if has_ext {
        vec![path.to_string()]
    } else {
        vec![
            format!("{path}.凡"),
            format!("{path}.fan"),
            path.to_string(),
        ]
    }
}

fn module_name(full: &str) -> String {
    Path::new(full)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| full.to_string())
}
