//! librime 动态后端：运行时 dlopen，缺失或 ABI 不符即回退内置引擎。
//!
//! 本模块是 fan-lsp 内唯一的 unsafe/动态库集中地，模式与
//! `fan-native/src/ext/ffi.rs` 一致：编译期不链接 librime，只在运行时按
//! 平台库名（或显式路径）`dlopen`/`LoadLibrary`，任何一步失败都返回
//! [`Option::None`]，由组合根回退到 [`BuiltinEngine`]，绝不影响主进程。
//!
//! ABI 依据 librime 官方 `src/rime_api.h`：
//! - 入口符号 `rime_get_api` 返回 `RimeApi*`，首字段 `int data_size`，其后是
//!   一串函数指针；`data_size = sizeof(RimeApi) - sizeof(int)`，客户端据此用
//!   `RIME_STRUCT_HAS_MEMBER` 判断某函数在当前版本是否存在。
//! - 本模块不镜像整个巨型函数表，而是按函数指针在表中的下标（见各 `IDX_*`
//!   常量，编号对齐头文件声明顺序）做带边界校验的偏移读取，避免维护数百个
//!   字段带来的错位风险。
//! - 取词走 `set_input` + `get_context`（读 `context.menu.candidates[].text`），
//!   不模拟物理键码，适合无键盘事件的补全场景。

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Mutex;

use libloading::Library;

use fan_analysis::pinyin::{EngineCandidate, PinyinEngine, PinyinMatch, PinyinOptions};
use fan_pinyin::BuiltinEngine;

/// 平台相关的 librime 默认动态库名（SONAME），按顺序探测。
fn candidate_lib_names() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        &["librime.so.1", "librime.so"]
    } else if cfg!(target_os = "macos") {
        &["librime.1.dylib", "librime.dylib"]
    } else if cfg!(target_os = "windows") {
        &["rime.dll", "rime1.dll"]
    } else {
        &["librime.so.1"]
    }
}

// RimeApi 函数指针表中的下标（0 起，对齐 rime_api.h 声明顺序，跳过首字段
// data_size）。仅收录本模块实际调用的函数；新增调用时按头文件顺序核对编号。
const IDX_SETUP: usize = 0;
const IDX_INITIALIZE: usize = 2;
const IDX_FINALIZE: usize = 3;
const IDX_CREATE_SESSION: usize = 7;
const IDX_DESTROY_SESSION: usize = 9;
const IDX_GET_CONTEXT: usize = 14;
const IDX_FREE_CONTEXT: usize = 15;
const IDX_SELECT_SCHEMA: usize = 26;
const IDX_SET_INPUT: usize = 89;

type RimeSessionId = usize;
type Bool = c_int;

// 仅声明用到的函数指针类型；参数/返回类型严格对应 rime_api.h。
type FnSetup = unsafe extern "C" fn(*mut RimeTraits);
type FnInitialize = unsafe extern "C" fn(*mut RimeTraits);
type FnFinalize = unsafe extern "C" fn();
type FnCreateSession = unsafe extern "C" fn() -> RimeSessionId;
type FnDestroySession = unsafe extern "C" fn(RimeSessionId);
type FnGetContext = unsafe extern "C" fn(RimeSessionId, *mut RimeContext) -> Bool;
type FnFreeContext = unsafe extern "C" fn(*mut RimeContext) -> Bool;
type FnSelectSchema = unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool;
type FnSetInput = unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool;
type FnGetApi = unsafe extern "C" fn() -> *mut RimeApi;

/// `RimeTraits`（rime_api.h `rime_traits_t`），仅布局需要，字段顺序不可调整。
#[repr(C)]
struct RimeTraits {
    data_size: c_int,
    shared_data_dir: *const c_char,
    user_data_dir: *const c_char,
    distribution_name: *const c_char,
    distribution_code_name: *const c_char,
    distribution_version: *const c_char,
    app_name: *const c_char,
    modules: *const *const c_char,
    min_log_level: c_int,
    log_dir: *const c_char,
    prebuilt_data_dir: *const c_char,
    staging_dir: *const c_char,
}

impl Default for RimeTraits {
    fn default() -> Self {
        let mut t = Self {
            data_size: 0,
            shared_data_dir: ptr::null(),
            user_data_dir: ptr::null(),
            distribution_name: ptr::null(),
            distribution_code_name: ptr::null(),
            distribution_version: ptr::null(),
            app_name: ptr::null(),
            modules: ptr::null(),
            min_log_level: 0,
            log_dir: ptr::null(),
            prebuilt_data_dir: ptr::null(),
            staging_dir: ptr::null(),
        };
        // RIME_STRUCT_INIT：data_size = sizeof(T) - sizeof(data_size)。
        t.data_size = (std::mem::size_of::<Self>() - std::mem::size_of::<c_int>()) as c_int;
        t
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RimeCandidate {
    text: *mut c_char,
    comment: *mut c_char,
    reserved: *mut c_void,
}

#[repr(C)]
struct RimeComposition {
    length: c_int,
    cursor_pos: c_int,
    sel_start: c_int,
    sel_end: c_int,
    preedit: *mut c_char,
}

#[repr(C)]
struct RimeMenu {
    page_size: c_int,
    page_no: c_int,
    is_last_page: Bool,
    highlighted_candidate_index: c_int,
    num_candidates: c_int,
    candidates: *mut RimeCandidate,
    select_keys: *mut c_char,
}

/// `RimeContext`（rime_api.h `rime_context_t`），布局严格对应头文件。
#[repr(C)]
struct RimeContext {
    data_size: c_int,
    composition: RimeComposition,
    menu: RimeMenu,
    commit_text_preview: *mut c_char,
    select_labels: *mut *mut c_char,
}

impl Default for RimeContext {
    fn default() -> Self {
        let mut c = Self {
            data_size: 0,
            composition: RimeComposition {
                length: 0,
                cursor_pos: 0,
                sel_start: 0,
                sel_end: 0,
                preedit: ptr::null_mut(),
            },
            menu: RimeMenu {
                page_size: 0,
                page_no: 0,
                is_last_page: 0,
                highlighted_candidate_index: 0,
                num_candidates: 0,
                candidates: ptr::null_mut(),
                select_keys: ptr::null_mut(),
            },
            commit_text_preview: ptr::null_mut(),
            select_labels: ptr::null_mut(),
        };
        c.data_size = (std::mem::size_of::<Self>() - std::mem::size_of::<c_int>()) as c_int;
        c
    }
}

/// 仅用于按偏移读取函数指针的不透明头部：`data_size` 后即函数指针数组。
#[repr(C)]
struct RimeApi {
    data_size: c_int,
}

/// 非空配置字符串转 CString；空串返回 None（对应 librime 的 NULL=用默认值）。
fn option_cstring(s: &str) -> Option<CString> {
    if s.is_empty() {
        None
    } else {
        CString::new(s).ok()
    }
}

/// 按显式路径或平台默认库名加载 librime，全部失败返回 None。
fn load_library(options: &PinyinOptions) -> Option<Library> {
    if !options.rime_lib_path.is_empty() {
        // SAFETY: 仅 dlopen，不解引用任何符号；路径无效由库调用返回错误。
        return unsafe { Library::new(&options.rime_lib_path).ok() };
    }
    for name in candidate_lib_names() {
        if let Ok(lib) = unsafe { Library::new(name) } {
            return Some(lib);
        }
    }
    None
}

/// librime 动态引擎。持有动态库保活、API 表指针与一个串行化的会话句柄。
///
/// `rank` 委托给 [`BuiltinEngine`]：即便启用 Rime，关键字/项目符号的已知词
/// 匹配仍走内置三模式，保证 tier 0–2 行为不变；Rime 只通过 `suggest` 提供
/// 引擎自由词（tier 3）。
pub struct RimeDynamicEngine {
    // SAFETY 约束：`_lib` 必须在 `api` 与所有函数指针使用期间存活；字段声明
    // 在前以保证 drop 顺序（先丢弃会话/api，最后卸载库）。
    _lib: Library,
    api: *mut RimeApi,
    session: Mutex<RimeSessionId>,
    builtin: BuiltinEngine,
    available_indices: usize,
}

// 原始指针本身非 Send/Sync；本类型把对 api/会话的访问收敛在受控方法内，
// 且 librime 会话经 Mutex 串行访问，库句柄 Library 本身是 Send+Sync。
unsafe impl Send for RimeDynamicEngine {}
unsafe impl Sync for RimeDynamicEngine {}

impl RimeDynamicEngine {
    /// 按配置尝试加载并初始化 librime；任何失败均返回 None 供上层回退。
    pub fn try_load(options: &PinyinOptions) -> Option<Self> {
        let lib = load_library(options)?;
        // SAFETY: 仅取符号地址，不解引用；符号缺失即返回 None。
        let get_api: libloading::Symbol<FnGetApi> = unsafe { lib.get(b"rime_get_api").ok()? };
        let api = unsafe { get_api() };
        if api.is_null() {
            return None;
        }
        // SAFETY: api 指向 librime 管理的静态 RimeApi，首字段 data_size 有效。
        let data_size = unsafe { (*api).data_size };
        if data_size <= 0 {
            return None;
        }
        // 函数指针槽总数 = data_size / 指针宽度；下标访问前据此做边界校验。
        let ptr_size = std::mem::size_of::<*const c_void>();
        let available_indices = (data_size as usize) / ptr_size;

        let engine = Self {
            _lib: lib,
            api,
            session: Mutex::new(0),
            builtin: BuiltinEngine,
            available_indices,
        };
        engine.initialize(options)?;
        Some(engine)
    }

    /// 读取下标 `index` 的函数指针，越界或为空返回 None。
    fn symbol<F>(&self, index: usize) -> Option<F> {
        if index >= self.available_indices {
            return None;
        }
        // 表基址跳过首字段 data_size，按指针宽度偏移。
        let base = self.api as *const u8;
        let table = unsafe { base.add(std::mem::size_of::<c_int>()) } as *const *const c_void;
        let raw = unsafe { *table.add(index) };
        if raw.is_null() {
            return None;
        }
        // SAFETY: 已通过 data_size 边界校验，调用方保证 F 与头文件签名一致。
        Some(unsafe { std::mem::transmute_copy::<*const c_void, F>(&raw) })
    }

    fn initialize(&self, options: &PinyinOptions) -> Option<()> {
        let setup: FnSetup = self.symbol(IDX_SETUP)?;
        let initialize: FnInitialize = self.symbol(IDX_INITIALIZE)?;
        let create_session: FnCreateSession = self.symbol(IDX_CREATE_SESSION)?;

        // 这些 CString 必须在 setup/initialize 调用期间保持有效，故先绑定到变量。
        let shared = option_cstring(&options.rime_shared_data_dir);
        let user = option_cstring(&options.rime_user_data_dir);
        let app = CString::new("rime.fan").ok()?;
        // 空串表示日志只写 stderr（见 rime_api.h 对 log_dir 的约定）。
        let log_dir = CString::new("").ok()?;

        let mut traits = RimeTraits {
            app_name: app.as_ptr(),
            log_dir: log_dir.as_ptr(),
            shared_data_dir: shared.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            user_data_dir: user.as_ref().map_or(ptr::null(), |u| u.as_ptr()),
            ..RimeTraits::default()
        };

        // SAFETY: 函数指针来自已校验的 librime 表，traits 已按 RIME_STRUCT_INIT 填充，
        // 且上述 CString 在整个调用期间存活。
        unsafe {
            setup(&mut traits as *mut RimeTraits);
            initialize(&mut traits as *mut RimeTraits);
        }

        // SAFETY: initialize 已完成，创建会话。
        let session = unsafe { create_session() };
        if session == 0 {
            return None;
        }
        *self.session.lock().unwrap() = session;

        if !options.rime_schema_id.is_empty() {
            if let Some(select_schema) = self.symbol::<FnSelectSchema>(IDX_SELECT_SCHEMA) {
                if let Ok(schema) = CString::new(options.rime_schema_id.as_str()) {
                    // SAFETY: schema CString 在调用期间有效。
                    unsafe {
                        select_schema(session, schema.as_ptr());
                    }
                }
            }
        }
        Some(())
    }

    fn fetch_candidates(&self, input: &str) -> Vec<String> {
        let Some(set_input): Option<FnSetInput> = self.symbol(IDX_SET_INPUT) else {
            return Vec::new();
        };
        let Some(get_context): Option<FnGetContext> = self.symbol(IDX_GET_CONTEXT) else {
            return Vec::new();
        };
        let Some(free_context): Option<FnFreeContext> = self.symbol(IDX_FREE_CONTEXT) else {
            return Vec::new();
        };
        let Ok(input_c) = CString::new(input) else {
            return Vec::new();
        };
        let session = *self.session.lock().unwrap();
        if session == 0 {
            return Vec::new();
        }

        // SAFETY: input_c 在调用期间有效；session 由本结构创建且尚未销毁。
        let ok = unsafe { set_input(session, input_c.as_ptr()) };
        if ok == 0 {
            return Vec::new();
        }
        let mut context = RimeContext::default();
        // SAFETY: context 已按版本化结构清零并设置 data_size。
        let filled = unsafe { get_context(session, &mut context as *mut RimeContext) };
        if filled == 0 {
            return Vec::new();
        }
        let mut words = Vec::new();
        if context.menu.num_candidates > 0 && !context.menu.candidates.is_null() {
            let count = context.menu.num_candidates.max(0) as usize;
            for i in 0..count {
                // SAFETY: candidates 数组由 librime 分配，长度为 num_candidates，
                // 在 free_context 之前有效。
                let cand = unsafe { &*context.menu.candidates.add(i) };
                if cand.text.is_null() {
                    continue;
                }
                // SAFETY: text 指向 librime 管理的 NUL 结尾 UTF-8 字符串。
                let text = unsafe { CStr::from_ptr(cand.text) };
                if let Ok(s) = text.to_str() {
                    if !s.is_empty() {
                        words.push(s.to_string());
                    }
                }
            }
        }
        // SAFETY: 释放 librime 填充的 context 内部内存。
        unsafe {
            free_context(&mut context as *mut RimeContext);
        }
        words
    }
}

impl PinyinEngine for RimeDynamicEngine {
    fn rank(&self, input: &str, word: &str, opts: &PinyinOptions) -> Option<PinyinMatch> {
        // 已知词匹配保持内置三模式，确保关键字/符号的优先级与行为稳定。
        self.builtin.rank(input, word, opts)
    }

    fn suggest(&self, input: &str, _opts: &PinyinOptions) -> Vec<EngineCandidate> {
        if input.is_empty() {
            return Vec::new();
        }
        self.fetch_candidates(input)
            .into_iter()
            .enumerate()
            .map(|(i, word)| EngineCandidate {
                word,
                detail: "Rime".to_string(),
                // librime 候选已按相关度排序，靠后的索引给递减权重。
                weight: (u32::MAX as usize - i) as u32,
            })
            .collect()
    }
}

impl Drop for RimeDynamicEngine {
    fn drop(&mut self) {
        let session = *self.session.lock().unwrap();
        if session == 0 {
            return;
        }
        // 尽力销毁会话与引擎；关闭阶段的失败无需处理。
        if let Some(destroy) = self.symbol::<FnDestroySession>(IDX_DESTROY_SESSION) {
            // SAFETY: session 由本结构创建，drop 时尚未失效。
            unsafe { destroy(session) };
        }
        if let Some(finalize) = self.symbol::<FnFinalize>(IDX_FINALIZE) {
            // SAFETY: 进程仍持有 librime，finalize 与其 initialize 配对。
            unsafe { finalize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 无librime时加载回退为空() {
        // CI 与开发机默认不安装 librime；指向一个不存在的库名，验证全程不 panic。
        let options = PinyinOptions {
            rime_lib_path: "/nonexistent/libfan-no-rime.so".to_string(),
            ..PinyinOptions::sensible_default()
        };
        assert!(RimeDynamicEngine::try_load(&options).is_none());
    }

    #[test]
    fn 平台默认库名非空() {
        assert!(!candidate_lib_names().is_empty());
    }

    #[test]
    fn 版本化结构的data_size按约定计算() {
        let traits = RimeTraits::default();
        assert_eq!(
            traits.data_size as usize,
            std::mem::size_of::<RimeTraits>() - std::mem::size_of::<c_int>()
        );
        let context = RimeContext::default();
        assert_eq!(
            context.data_size as usize,
            std::mem::size_of::<RimeContext>() - std::mem::size_of::<c_int>()
        );
    }

    #[test]
    fn 空配置字符串映射为none() {
        assert!(option_cstring("").is_none());
        assert!(option_cstring("data").is_some());
    }
}
