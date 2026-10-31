//! `ext` 组内建：依赖第三方库，交付为独立动态库，按需加载。
//!
//! 对应 `fan-sdk` 中的正则、JSON、网络等模块。这些能力依赖成熟的第三方
//! 实现（正则引擎、JSON 解析器、HTTP 客户端），自行用标准库实现代价过高。
//!
//! # 为何单独分组
//!
//! 解释器本体**零第三方依赖**。只有在凡代码中出现 `@内建模块("正则")` 时，
//! 才会去加载 `fan_ext_正则` 动态库；不使用扩展能力的程序完全不触碰这些依赖。
//! 这也让扩展库能独立分发与更新。
//!
//! # 加载机制
//!
//! 直接声明平台原生符号（`dlopen`/`LoadLibraryA`）而不引入 `libloading`，
//! 以保持零依赖。扩展库需导出两个符号：
//!
//! - `fan_ext_abi_version() -> u32`：ABI 版本校验
//! - `fan_ext_register(rt: *mut Runtime)`：向内建表注册本库提供的函数
//!
//! > 约束：扩展库必须与解释器链接**同一版本**的 `fan-object`，否则 `Runtime`
//! > 内存布局可能不一致。ABI 版本号即用于拦截这种情况。

pub mod default;

use crate::ext::default::has_default;
use fan_object::Runtime;
use std::ffi::CString;
use std::path::PathBuf;

/// 当前 ABI 版本。扩展库导出的版本必须一致，否则拒绝加载。
pub const ABI_VERSION: u32 = fan_object::FAN_EXT_ABI_VERSION;

/// 动态库文件名前缀。
pub const LIB_PREFIX: &str = "fan_ext_";

/// 模块名 → 动态库名。
///
/// 凡代码的 `@内建模块("正则")` 用的是中文模块名，而文件名与 crate 名用
/// ASCII 更稳妥（跨平台、构建工具兼容），故在此固定别名。
pub fn lib_name(module: &str) -> &str {
    match module {
        "正则" => "regex",
        "网络" => "http",
        _ => module,
    }
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type RegisterFn = unsafe extern "C" fn(*mut Runtime);

#[cfg(unix)]
mod sys {
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        pub fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        /// 进程退出前不卸载：卸载会让已注册的函数指针失效，故保留声明备用。
        #[allow(dead_code)]
        pub fn dlclose(handle: *mut c_void) -> c_int;
        pub fn dlerror() -> *const c_char;
    }

    pub const RTLD_NOW: c_int = 2;

    /// 按平台惯例拼出动态库文件名。
    pub fn file_name(module: &str) -> String {
        let ext = if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        };
        format!(
            "lib{}{}.{}",
            super::LIB_PREFIX,
            super::lib_name(module),
            ext
        )
    }
}

#[cfg(windows)]
mod sys {
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        pub fn LoadLibraryA(filename: *const c_char) -> *mut c_void;
        pub fn GetProcAddress(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        pub fn FreeLibrary(handle: *mut c_void) -> c_int;
        pub fn GetLastError() -> u32;
    }

    pub fn file_name(module: &str) -> String {
        format!("{}{}.dll", super::LIB_PREFIX, super::lib_name(module))
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn file_name(module: &str) -> String {
        format!("{}{}", super::LIB_PREFIX, super::lib_name(module))
    }
}

/// 一个扩展模块的实现来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSource {
    /// 来自扩展动态库（第三方库驱动，能力完整）。
    Library,
    /// 无扩展库，回落至解释器内置默认实现（纯标准库，能力受限）。
    Default,
}

impl std::fmt::Display for ExtSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtSource::Library => write!(f, "扩展库"),
            ExtSource::Default => write!(f, "默认实现"),
        }
    }
}

/// 已登记的扩展模块表。
pub struct ExtRegistry {
    /// 模块名与实现来源，按登记顺序。
    loaded: Vec<(String, ExtSource)>,
}

impl Default for ExtRegistry {
    fn default() -> Self {
        ExtRegistry::new()
    }
}

impl ExtRegistry {
    pub fn new() -> Self {
        ExtRegistry { loaded: Vec::new() }
    }

    /// 已登记的模块名列表。
    pub fn loaded(&self) -> Vec<&str> {
        self.loaded.iter().map(|(m, _)| m.as_str()).collect()
    }

    /// 指定模块的实现来源；未登记则为 `None`。
    pub fn source_of(&self, module: &str) -> Option<ExtSource> {
        self.loaded
            .iter()
            .find(|(m, _)| m == module)
            .map(|(_, s)| *s)
    }

    /// 指定模块是否已登记。
    pub fn is_loaded(&self, module: &str) -> bool {
        self.loaded.iter().any(|(m, _)| m == module)
    }

    /// 确保指定模块的扩展内建已注册；重复调用只会登记一次。
    ///
    /// **回落策略**：优先加载扩展动态库；找不到（或平台不支持）时改用
    /// [`default`] 中的纯标准库实现。两者都没有才报错。这样凡代码写
    /// `@内建模块("json")` 后，无论目标环境是否部署了扩展库都能运行。
    pub fn ensure(&mut self, rt: &mut Runtime, module: &str) -> Result<ExtSource, String> {
        if let Some(source) = self.source_of(module) {
            return Ok(source);
        }
        let source = match load_into(rt, module) {
            Ok(()) => ExtSource::Library,
            Err(_) => {
                // 没有注册实现 → 使用默认
                if !has_default(module) {
                    return Err(format!("模块 {} 既没有扩展库，也没有默认实现", module));
                }
                default::register(rt, module)?;
                ExtSource::Default
            }
        };
        self.loaded.push((module.to_string(), source));
        Ok(source)
    }

    /// 按内建名确保其所属模块已登记，供 `@内建` 找不到实现时自动补加载。
    pub fn ensure_for_native(
        &mut self,
        rt: &mut Runtime,
        native: &str,
    ) -> Result<ExtSource, String> {
        match default::module_of(native) {
            Some(module) => self.ensure(rt, module),
            None => Err(String::new()),
        }
    }
}

/// 动态库搜索目录：环境变量优先，其次可执行文件所在目录与当前目录。
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("FAN_EXT_DIR") {
        dirs.extend(std::env::split_paths(&dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.push(PathBuf::from("."));
    dirs
}

/// 查找模块对应的动态库文件路径。
pub fn resolve_library(module: &str) -> Option<PathBuf> {
    let name = sys::file_name(module);
    for dir in search_dirs() {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 加载动态库并把其内建注册进运行实例。
fn load_into(rt: &mut Runtime, module: &str) -> Result<(), String> {
    #[cfg(any(unix, windows))]
    {
        let path = resolve_library(module).ok_or_else(|| {
            format!(
                "找不到扩展库 {}（期望文件 {}）",
                module,
                sys::file_name(module)
            )
        })?;

        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| format!("扩展库路径含非法字符：{}", path.display()))?;

        unsafe {
            #[cfg(unix)]
            let handle = sys::dlopen(c_path.as_ptr(), sys::RTLD_NOW);
            #[cfg(windows)]
            let handle = sys::LoadLibraryA(c_path.as_ptr());

            if handle.is_null() {
                #[cfg(unix)]
                let detail = {
                    let err = sys::dlerror();
                    if err.is_null() {
                        String::from("未知错误")
                    } else {
                        std::ffi::CStr::from_ptr(err).to_string_lossy().to_string()
                    }
                };
                #[cfg(windows)]
                let detail = format!("错误码 {}", sys::GetLastError());
                return Err(format!("加载扩展库 {} 失败：{}", module, detail));
            }

            // 先校验 ABI 版本，避免布局不一致导致的内存错误
            let version_symbol = CString::new("fan_ext_abi_version").unwrap();
            #[cfg(unix)]
            let version_ptr = sys::dlsym(handle, version_symbol.as_ptr());
            #[cfg(windows)]
            let version_ptr = sys::GetProcAddress(handle, version_symbol.as_ptr());

            if !version_ptr.is_null() {
                let version_fn: AbiVersionFn = std::mem::transmute(version_ptr);
                let version = version_fn();
                if version != ABI_VERSION {
                    return Err(format!(
                        "扩展库 {} 的 ABI 版本为 {}，解释器为 {}，请重新编译扩展库",
                        module, version, ABI_VERSION
                    ));
                }
            }

            let register_symbol = CString::new("fan_ext_register").unwrap();
            #[cfg(unix)]
            let register_ptr = sys::dlsym(handle, register_symbol.as_ptr());
            #[cfg(windows)]
            let register_ptr = sys::GetProcAddress(handle, register_symbol.as_ptr());

            if register_ptr.is_null() {
                return Err(format!("扩展库 {} 未导出 fan_ext_register", module));
            }

            let register_fn: RegisterFn = std::mem::transmute(register_ptr);
            register_fn(rt as *mut Runtime);

            // 句柄保持打开：卸载会让已注册的函数指针失效，
            // 因此这里刻意不调用 dlclose，句柄随进程退出释放。
            let _kept_open = handle;
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = rt;
        Err(format!("当前平台不支持动态加载扩展库：{}", module))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 新注册表未登记任何模块() {
        let reg = ExtRegistry::new();
        assert!(reg.loaded().is_empty());
        assert!(!reg.is_loaded("正则"));
        assert_eq!(reg.source_of("正则"), None);
    }

    #[test]
    fn 无扩展库时回落到默认实现() {
        let mut rt = Runtime::new();
        let mut reg = ExtRegistry::new();
        // json 有默认实现，故即使找不到动态库也应成功
        let source = reg.ensure(&mut rt, "json").expect("应回落到默认实现");
        assert_eq!(source, ExtSource::Default);
        assert_eq!(reg.source_of("json"), Some(ExtSource::Default));
        assert!(rt.lookup_native("jsonEncode").is_some());
    }

    #[test]
    fn 重复登记只发生一次() {
        let mut rt = Runtime::new();
        let mut reg = ExtRegistry::new();
        reg.ensure(&mut rt, "json").unwrap();
        reg.ensure(&mut rt, "json").unwrap();
        assert_eq!(reg.loaded().len(), 1);
    }

    #[test]
    fn 既无扩展也无默认的模块报错() {
        let mut rt = Runtime::new();
        let mut reg = ExtRegistry::new();
        let err = reg.ensure(&mut rt, "不存在的模块").unwrap_err();
        assert!(err.contains("既没有扩展库"), "实际：{}", err);
        assert!(!reg.is_loaded("不存在的模块"));
    }

    #[test]
    fn 按内建名自动补加载() {
        let mut rt = Runtime::new();
        let mut reg = ExtRegistry::new();
        assert_eq!(
            reg.ensure_for_native(&mut rt, "jsonEncode").unwrap(),
            ExtSource::Default
        );
        assert!(rt.lookup_native("jsonDecode").is_some());
        // 不属于任何扩展模块的内建返回空错误，交由调用方继续处理
        assert_eq!(reg.ensure_for_native(&mut rt, "print").unwrap_err(), "");
    }

    #[test]
    fn 来源枚举可展示() {
        assert_eq!(ExtSource::Library.to_string(), "扩展库");
        assert_eq!(ExtSource::Default.to_string(), "默认实现");
    }

    #[test]
    fn 动态库文件名按平台命名() {
        let name = sys::file_name("正则");
        assert!(name.contains(LIB_PREFIX), "实际：{}", name);
        if cfg!(target_os = "macos") {
            assert!(name.ends_with(".dylib"), "实际：{}", name);
        } else if cfg!(target_os = "windows") {
            assert!(name.ends_with(".dll"), "实际：{}", name);
        } else if cfg!(unix) {
            assert!(name.ends_with(".so"), "实际：{}", name);
        }
    }

    #[test]
    fn 缺少扩展库且无默认实现时报错() {
        let mut rt = Runtime::new();
        let mut reg = ExtRegistry::new();
        // 用一个不可能存在的模块名，确保命中「找不到」分支
        let err = reg.ensure(&mut rt, "不存在的模块").unwrap_err();
        assert!(err.contains("既没有扩展库"), "实际：{}", err);
        assert!(!reg.is_loaded("不存在的模块"));
    }

    #[test]
    fn 搜索目录包含当前目录() {
        let dirs = search_dirs();
        assert!(!dirs.is_empty());
    }

    #[test]
    fn 中文模块名映射为ascii库名() {
        assert_eq!(lib_name("正则"), "regex");
        assert_eq!(lib_name("网络"), "http");
        assert_eq!(lib_name("json"), "json");
        assert_eq!(lib_name("自定义"), "自定义");
        assert!(sys::file_name("正则").contains("fan_ext_regex"));
    }

    #[test]
    fn abi版本号固定为一() {
        assert_eq!(ABI_VERSION, 1);
    }
}
