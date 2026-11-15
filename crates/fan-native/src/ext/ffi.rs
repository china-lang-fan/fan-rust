//! 动态库 FFI 内建：让凡程序能加载并调用外部 C 动态库。
//!
//! 本模块是整个 FFI 特性的 **unsafe 集中地**。凡语言只见安全的句柄值
//! （[`HandleValue`]，仅含种类标记与唯一 id），真正的宿主资源——`dlopen`
//! 句柄、函数符号、原生缓冲——都登记在本模块的进程内注册表里，按 id 索引。
//!
//! 设计约束：
//!
//! - 句柄不透明：不提供指针算术，不与整数互转，凡语言只能经本模块内建操作。
//! - 不自动释放：句柄离开作用域不回收原生内存，调用方必须显式 `ffi.free`/关闭，
//!   避免与 C 库的所有权约定冲突造成 double-free。
//! - 最小可用：仅支持标量（整数/小数/字节）、C 字符串、裸内存与不透明句柄；
//!   结构体布局与回调函数指针留待二期。
//!
//! 内建以 `ffi.` 为模块前缀注册全名（如 `ffi.open`），经
//! `定义 本地("ffi.open")` 绑定，运行时由 `bind_native` 按完整名查表。

use fan_object::{new_error, HandleKind, HandleValue, NativeSpec, Object};
use libloading::Library;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::rc::Rc;

/// 本模块名，与 `动态库.凡` 的本地绑定前缀、静态诊断目录一致。
pub const MODULE: &str = "ffi";

// ===========================================================================
// 类型签名：白名单内的 C 标量类型，用凡语言类型名字符串描述
// ===========================================================================

/// 一个受支持的 C 参数/返回类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CType {
    /// C 指针宽度整数，凡侧 `i64`。
    Int,
    /// C `double`，凡侧 `f64`。
    Float,
    /// C `unsigned char`，凡侧 `u8`。
    Byte,
    /// 不透明指针（句柄/缓冲/`NULL`），凡侧句柄或空。
    Ptr,
    /// `void` 返回。
    Void,
}

impl CType {
    /// 把凡语言类型名（白名单）解析为 C 类型；非法名报错。
    fn parse(name: &str) -> Result<CType, String> {
        match name.trim() {
            "整数" | "int" | "long" => Ok(CType::Int),
            "小数" | "double" | "float" => Ok(CType::Float),
            "字节" | "byte" => Ok(CType::Byte),
            "句柄" | "指针" | "ptr" | "handle" => Ok(CType::Ptr),
            "空" | "void" => Ok(CType::Void),
            other => Err(format!("不支持的 FFI 类型：{other}")),
        }
    }
}

// ===========================================================================
// 进程内资源注册表
// ===========================================================================

/// 函数符号登记：保留库的强引用以防卸载，并缓存类型签名与裸函数地址。
struct SymEntry {
    func: *mut c_void,
    // 延长库生命周期至符号移除；库在登记期间不会被卸载。
    _lib: Rc<Library>,
    ret: CType,
    params: Vec<CType>,
}

/// 原生内存登记：一块由 `malloc`/C 字符串分配的缓冲及其容量。
struct MemEntry {
    ptr: *mut c_void,
    len: usize,
    /// C 字符串缓冲（含结尾 `\0`）。
    cstring: bool,
}

/// 已加载动态库登记。
struct LibEntry {
    lib: Rc<Library>,
}

// 裸指针仅在本模块的 unsafe 块内解引用；注册表运行在单线程解释器内，
// 不跨线程发送（句柄值本身也是非 Send 的 Rc）。
#[derive(Default)]
struct Registry {
    libs: HashMap<u64, LibEntry>,
    syms: HashMap<u64, SymEntry>,
    mems: HashMap<u64, MemEntry>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

// ===========================================================================
// 句柄与注册表互转辅助
// ===========================================================================

fn as_handle(o: &Object) -> Result<Rc<HandleValue>, String> {
    match o {
        Object::Handle(h) => Ok(h.clone()),
        other => Err(format!("参数必须是句柄，实际为 {}", other.kind())),
    }
}

/// 登记一块原生内存并返回内存句柄。
fn register_mem(ptr: *mut c_void, len: usize, cstring: bool) -> Object {
    let handle = HandleValue::new(HandleKind::Memory);
    REGISTRY.with(|r| {
        r.borrow_mut()
            .mems
            .insert(handle.id(), MemEntry { ptr, len, cstring })
    });
    Object::Handle(Rc::new(handle))
}

/// 取回内存登记项；句柄无效或已释放时报错。
fn lookup_mem(id: u64) -> Result<*mut c_void, String> {
    REGISTRY.with(|r| {
        r.borrow()
            .mems
            .get(&id)
            .map(|e| e.ptr)
            .ok_or_else(|| "内存句柄无效或已释放".to_string())
    })
}

/// 句柄转 C 指针参数：内存句柄取登记指针，空值转 NULL。
fn handle_to_ptr(o: &Object) -> Result<usize, String> {
    match o {
        Object::Nil => Ok(0),
        Object::Handle(h) if h.kind() == HandleKind::Memory => {
            lookup_mem(h.id()).map(|p| p as usize)
        }
        Object::Handle(_) => Err("只有内存句柄能作为指针参数传递".to_string()),
        other => Err(format!("指针参数必须是句柄或空，实际为 {}", other.kind())),
    }
}

// ===========================================================================
// 参数封送与调用分派
// ===========================================================================

/// 按声明的 C 类型把单个凡值封送为指针宽度的整数载体（usize）。
/// 浮点参数不在此处理，由分派宏按 f64 传入。
fn marshal_int(o: &Object, ty: CType) -> Result<usize, String> {
    let o = o.unwrap_named();
    match ty {
        CType::Int => o
            .as_int()
            .map(|v| v as usize)
            .ok_or_else(|| format!("整数参数实际为 {}", o.kind())),
        CType::Byte => match &o {
            Object::Byte(b) => Ok(*b as usize),
            Object::Int(v) if (0..=255).contains(v) => Ok(*v as usize),
            _ => Err(format!("字节参数实际为 {}", o.kind())),
        },
        CType::Ptr => handle_to_ptr(&o),
        CType::Float => Err("浮点参数应走 f64 分派".to_string()),
        CType::Void => Err("void 不能作为参数类型".to_string()),
    }
}

/// 取浮点参数。
fn marshal_float(o: &Object) -> Result<f64, String> {
    o.unwrap_named()
        .as_float()
        .ok_or_else(|| format!("小数参数实际为 {}", o.kind()))
}

/// 按返回类型把整数/浮点载体解包回凡值。
fn unmarshal_ret(ty: CType, int_ret: isize, float_ret: f64) -> Object {
    match ty {
        CType::Int => Object::Int(int_ret as i64),
        CType::Float => Object::Float(float_ret),
        CType::Byte => Object::Byte((int_ret as u64 & 0xff) as u8),
        CType::Ptr => {
            if int_ret == 0 {
                Object::Nil
            } else {
                // 返回的裸指针登记为内存句柄（长度未知，读写边界由调用方负责）。
                register_mem(int_ret as *mut c_void, 0, false)
            }
        }
        CType::Void => Object::Nil,
    }
}

// ---------------------------------------------------------------------------
// 类型化调用分派：嵌套 match 树（每个参数位在两个具体 ABI 类型间选择）
//
// 难点：参数位是 f64 还是 usize 由运行时 CType 决定，而 extern "C" 函数指针类型
// 必须编译期固定，stable Rust 不允许类型位置的运行时分支，也无法运行时部分应用
// C 函数。方案：参数先封送进 [`Slot`]，再用声明宏按字面量参数索引递归展开一棵
// match 树——每个参数位在 usize/f64 两个**具体类型**间分支，类型/实参按序累积，
// 叶层得到完全具体的 extern 签名后单次调用。所有签名编译期单态化，C ABI 传参
// 寄存器正确，外部函数只调用一次。
// ---------------------------------------------------------------------------

/// 已封送的参数槽：整数/字节/指针为 usize，小数为 f64。
#[derive(Clone, Copy)]
enum Slot {
    Int(usize),
    Float(f64),
}

fn slot_of(o: &Object, ty: CType) -> Result<Slot, String> {
    if ty == CType::Float {
        marshal_float(o).map(Slot::Float)
    } else {
        marshal_int(o, ty).map(Slot::Int)
    }
}

/// 叶层：类型与实参已全部具体化，转成 extern 指针调用一次。
macro_rules! ffi_leaf {
    ($func:expr, $ret:ty, [$($ty:ty),*], [$($val:expr),*]) => {{
        let f: extern "C" fn($($ty),*) -> $ret = std::mem::transmute($func);
        f($($val),*)
    }};
}

/// 递归步：处理下一个字面量索引位，按 Slot 选具体类型并继续累积。
macro_rules! ffi_step {
    // 终止：无剩余索引
    ($func:expr,$slots:expr,$ret:ty; $($ty:ty),*; $($val:expr),* ;) => {
        ffi_leaf!($func,$ret,[$($ty),*],[$($val),*])
    };
    // 递归：当前位 $idx
    ($func:expr,$slots:expr,$ret:ty; $($ty:ty),*; $($val:expr),* ; $idx:tt $($rest:tt)*) => {
        match $slots[$idx] {
            Slot::Int(v) => ffi_step!($func,$slots,$ret;
                $($ty,)* usize; $($val,)* v ; $($rest)*),
            Slot::Float(v) => ffi_step!($func,$slots,$ret;
                $($ty,)* f64; $($val,)* v ; $($rest)*),
        }
    };
}

/// 实际调用入口，unsafe 集中于此。按参数个数选择对应索引序列（一期最多 6 参）。
unsafe fn invoke(entry: &SymEntry, args: &[Object]) -> Result<Object, String> {
    if args.len() != entry.params.len() {
        return Err(format!(
            "参数数量不符：函数需要 {} 个，实际 {} 个",
            entry.params.len(),
            args.len()
        ));
    }
    // 先把全部实参按声明类型封送成槽位。
    let mut slots: Vec<Slot> = Vec::with_capacity(args.len());
    for (o, ty) in args.iter().zip(entry.params.iter()) {
        slots.push(slot_of(o, *ty)?);
    }
    let func = entry.func;
    let slots = &slots[..];

    // 整数/字节/指针/void 返回走 isize；小数返回走 f64。两路径都只调用一次。
    if entry.ret == CType::Float {
        let r: f64 = match args.len() {
            0 => {
                let f: extern "C" fn() -> f64 = std::mem::transmute(func);
                f()
            }
            1 => ffi_step!(func, slots, f64; ; ; 0),
            2 => ffi_step!(func, slots, f64; ; ; 0 1),
            3 => ffi_step!(func, slots, f64; ; ; 0 1 2),
            4 => ffi_step!(func, slots, f64; ; ; 0 1 2 3),
            5 => ffi_step!(func, slots, f64; ; ; 0 1 2 3 4),
            6 => ffi_step!(func, slots, f64; ; ; 0 1 2 3 4 5),
            n => return Err(format!("FFI 一期最多支持 6 个参数，实际 {n} 个")),
        };
        Ok(unmarshal_ret(entry.ret, 0, r))
    } else {
        let r: isize = match args.len() {
            0 => {
                let f: extern "C" fn() -> isize = std::mem::transmute(func);
                f()
            }
            1 => ffi_step!(func, slots, isize; ; ; 0),
            2 => ffi_step!(func, slots, isize; ; ; 0 1),
            3 => ffi_step!(func, slots, isize; ; ; 0 1 2),
            4 => ffi_step!(func, slots, isize; ; ; 0 1 2 3),
            5 => ffi_step!(func, slots, isize; ; ; 0 1 2 3 4),
            6 => ffi_step!(func, slots, isize; ; ; 0 1 2 3 4 5),
            n => return Err(format!("FFI 一期最多支持 6 个参数，实际 {n} 个")),
        };
        Ok(unmarshal_ret(entry.ret, r, 0.0))
    }
}

// ===========================================================================
// 内建实现：统一 (值, 错误) 双返回，成功错误位为空，失败值位为空
// ===========================================================================

fn ok(value: Object) -> Vec<Object> {
    vec![value, Object::Nil]
}

fn fail(message: String) -> Vec<Object> {
    vec![Object::Nil, new_error(message)]
}

/// ffi.open(路径) -> (库句柄, 错误)
fn builtin_open(args: Vec<Object>) -> Vec<Object> {
    let Some(path) = args.first().and_then(Object::as_str) else {
        return fail("ffi.open 需要字符串路径参数".to_string());
    };
    // SAFETY: 仅调用 dlopen/LoadLibrary 加载库，不在此处解引用任何符号。
    let lib = match unsafe { Library::new(&path) } {
        Ok(l) => Rc::new(l),
        Err(e) => return fail(format!("加载动态库失败：{e}")),
    };
    let handle = HandleValue::new(HandleKind::Library);
    REGISTRY.with(|r| {
        r.borrow_mut()
            .libs
            .insert(handle.id(), LibEntry { lib: lib.clone() });
    });
    ok(Object::Handle(Rc::new(handle)))
}

/// ffi.symbol(库句柄, 符号名, 返回类型, 参数类型...) -> (函数句柄, 错误)
fn builtin_symbol(args: Vec<Object>) -> Vec<Object> {
    if args.len() < 3 {
        return fail("ffi.symbol 至少需要 库、符号名、返回类型 三个参数".to_string());
    }
    let lib_handle = match as_handle(&args[0]) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    if lib_handle.kind() != HandleKind::Library {
        return fail("ffi.symbol 第一个参数必须是库句柄".to_string());
    }
    let Some(name) = args[1].as_str() else {
        return fail("符号名必须是字符串".to_string());
    };
    let ret = match args[2].as_str() {
        Some(s) => match CType::parse(&s) {
            Ok(t) => t,
            Err(e) => return fail(e),
        },
        None => return fail("返回类型必须是字符串".to_string()),
    };
    let mut params = Vec::with_capacity(args.len() - 3);
    for a in &args[3..] {
        match a.as_str() {
            Some(s) => match CType::parse(&s) {
                Ok(CType::Void) => return fail("void 不能作为参数类型".to_string()),
                Ok(t) => params.push(t),
                Err(e) => return fail(e),
            },
            None => return fail("参数类型必须是字符串".to_string()),
        }
    }
    if params.len() > 6 {
        return fail("FFI 一期最多支持 6 个参数".to_string());
    }

    let lib = REGISTRY.with(|r| r.borrow().libs.get(&lib_handle.id()).map(|e| e.lib.clone()));
    let Some(lib) = lib else {
        return fail("库句柄无效或已关闭".to_string());
    };
    // SAFETY: 取符号地址但不调用；库由注册表强引用保活，地址在库关闭前有效。
    // 先以零参函数指针类型取符号，转裸地址存入注册表，实际签名在 invoke 还原。
    let func: *mut c_void = unsafe {
        let sym: Result<libloading::Symbol<unsafe extern "C" fn()>, _> = lib.get(name.as_bytes());
        match sym {
            Ok(s) => *s as usize as *mut c_void,
            Err(e) => return fail(format!("找不到符号 {name}：{e}")),
        }
    };
    let handle = HandleValue::new(HandleKind::Symbol);
    REGISTRY.with(|r| {
        r.borrow_mut().syms.insert(
            handle.id(),
            SymEntry {
                func,
                _lib: lib,
                ret,
                params,
            },
        )
    });
    ok(Object::Handle(Rc::new(handle)))
}

/// ffi.call(函数句柄, 任意... 参数) -> (返回值, 错误)
fn builtin_call(args: Vec<Object>) -> Vec<Object> {
    let Some(first) = args.first() else {
        return fail("ffi.call 需要函数句柄参数".to_string());
    };
    let sym_handle = match as_handle(first) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    if sym_handle.kind() != HandleKind::Symbol {
        return fail("ffi.call 第一个参数必须是函数句柄".to_string());
    }
    let call_args = &args[1..];
    let outcome = REGISTRY.with(|r| {
        let registry = r.borrow();
        let entry = registry
            .syms
            .get(&sym_handle.id())
            .ok_or_else(|| "函数句柄无效或已释放".to_string())?;
        // SAFETY: 指针与签名来自 ffi.symbol，库被强引用保活；封送与 ABI
        // 正确性由 invoke 的类型化 transmute 保证。
        unsafe { invoke(entry, call_args) }
    });
    match outcome {
        Ok(v) => ok(v),
        Err(e) => fail(e),
    }
}

/// ffi.alloc(字节数) -> (内存句柄, 错误)
fn builtin_alloc(args: Vec<Object>) -> Vec<Object> {
    let Some(n) = args.first().and_then(Object::as_int) else {
        return fail("ffi.alloc 需要整数字节数".to_string());
    };
    if n <= 0 {
        return fail("分配字节数必须为正数".to_string());
    }
    let layout = match std::alloc::Layout::from_size_align(n as usize, 1) {
        Ok(l) => l,
        Err(e) => return fail(format!("内存布局无效：{e}")),
    };
    // SAFETY: 尺寸来自上面校验过的非零正整数，对齐为 1。
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return fail("内存分配失败".to_string());
    }
    ok(register_mem(ptr as *mut c_void, n as usize, false))
}

/// ffi.free(内存句柄) -> 错误
fn builtin_free(args: Vec<Object>) -> Vec<Object> {
    let handle = match args.first().map(as_handle) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return fail(e),
        None => return fail("ffi.free 需要内存句柄".to_string()),
    };
    if handle.kind() != HandleKind::Memory {
        return fail("ffi.free 只能释放内存句柄".to_string());
    }
    let removed = REGISTRY.with(|r| r.borrow_mut().mems.remove(&handle.id()));
    let Some(entry) = removed else {
        return fail("内存句柄无效或已释放".to_string());
    };
    // SAFETY: 指针与容量来自 ffi.alloc/ffi.cstring，同一分配器，仅释放一次。
    unsafe {
        if entry.cstring {
            drop(CString::from_raw(entry.ptr as *mut std::os::raw::c_char));
        } else {
            let layout = std::alloc::Layout::from_size_align_unchecked(entry.len.max(1), 1);
            std::alloc::dealloc(entry.ptr as *mut u8, layout);
        }
    }
    vec![Object::Nil]
}

/// 按类型读单值。
/// SAFETY: 调用方保证 ptr 指向至少对应类型宽度的有效内存。
unsafe fn read_typed(ptr: *mut c_void, ty: CType) -> Object {
    match ty {
        CType::Int => Object::Int(std::ptr::read(ptr as *const i64)),
        CType::Float => Object::Float(std::ptr::read(ptr as *const f64)),
        CType::Byte => Object::Byte(std::ptr::read(ptr as *const u8)),
        CType::Ptr => {
            let p = std::ptr::read(ptr as *const *mut c_void);
            if p.is_null() {
                Object::Nil
            } else {
                register_mem(p, 0, false)
            }
        }
        CType::Void => Object::Nil,
    }
}

/// ffi.read(内存句柄, 类型) -> (值, 错误)
fn builtin_read(args: Vec<Object>) -> Vec<Object> {
    if args.len() < 2 {
        return fail("ffi.read 需要 内存句柄、类型 两个参数".to_string());
    }
    let handle = match as_handle(&args[0]) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    let Some(type_name) = args[1].as_str() else {
        return fail("类型必须是字符串".to_string());
    };
    let ty = match CType::parse(&type_name) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    if ty == CType::Void {
        return fail("void 不能读取".to_string());
    }
    let ptr = match lookup_mem(handle.id()) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    // SAFETY: 句柄指向注册表管理的有效缓冲，宽度由调用方保证。
    ok(unsafe { read_typed(ptr, ty) })
}

/// ffi.write(内存句柄, 类型, 值) -> 错误
fn builtin_write(args: Vec<Object>) -> Vec<Object> {
    if args.len() < 3 {
        return fail("ffi.write 需要 内存句柄、类型、值 三个参数".to_string());
    }
    let handle = match as_handle(&args[0]) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    let Some(type_name) = args[1].as_str() else {
        return fail("类型必须是字符串".to_string());
    };
    let ty = match CType::parse(&type_name) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let ptr = match lookup_mem(handle.id()) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let value = args[2].unwrap_named();
    // SAFETY: 句柄指向可写且足够宽的注册表缓冲。
    unsafe {
        match ty {
            CType::Int => {
                let Some(v) = value.as_int() else {
                    return fail(format!("整数写入值实际为 {}", value.kind()));
                };
                std::ptr::write(ptr as *mut i64, v);
            }
            CType::Float => {
                let Some(v) = value.as_float() else {
                    return fail(format!("小数写入值实际为 {}", value.kind()));
                };
                std::ptr::write(ptr as *mut f64, v);
            }
            CType::Byte => match &value {
                Object::Byte(b) => std::ptr::write(ptr as *mut u8, *b),
                Object::Int(v) if (0..=255).contains(v) => {
                    std::ptr::write(ptr as *mut u8, *v as u8)
                }
                _ => return fail(format!("字节写入值实际为 {}", value.kind())),
            },
            CType::Ptr => {
                let target = match handle_to_ptr(&value) {
                    Ok(p) => p as *mut c_void,
                    Err(e) => return fail(e),
                };
                std::ptr::write(ptr as *mut *mut c_void, target);
            }
            CType::Void => return fail("void 不能写入".to_string()),
        }
    }
    vec![Object::Nil]
}

/// ffi.cstring(字符串) -> (内存句柄, 错误)
fn builtin_cstring(args: Vec<Object>) -> Vec<Object> {
    let Some(text) = args.first().and_then(Object::as_str) else {
        return fail("ffi.cstring 需要字符串参数".to_string());
    };
    let c = match CString::new(text) {
        Ok(c) => c,
        Err(_) => return fail("字符串不能包含 NUL 字节".to_string()),
    };
    let len = c.as_bytes_with_nul().len();
    // into_raw 后内存交注册表管理，ffi.free 时用 from_raw 回收。
    let ptr = c.into_raw() as *mut c_void;
    ok(register_mem(ptr, len, true))
}

/// ffi.toCString(内存句柄[, 长度]) -> (字符串, 错误)
/// 不给长度或长度为 0 时按 NUL 结尾读取（strlen 语义）。
fn builtin_to_cstring(args: Vec<Object>) -> Vec<Object> {
    if args.is_empty() {
        return fail("ffi.toCString 至少需要内存句柄".to_string());
    }
    let handle = match as_handle(&args[0]) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    let ptr = match lookup_mem(handle.id()) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    // SAFETY: ptr 指向注册表管理的有效缓冲。
    unsafe {
        let cstr = match args.get(1).and_then(Object::as_int) {
            Some(n) if n > 0 => {
                let n = n as usize;
                let bytes = std::slice::from_raw_parts(ptr as *const u8, n);
                match std::ffi::CStr::from_bytes_with_nul(bytes) {
                    Ok(c) => c,
                    Err(_) => std::ffi::CStr::from_ptr(ptr as *const std::os::raw::c_char),
                }
            }
            _ => std::ffi::CStr::from_ptr(ptr as *const std::os::raw::c_char),
        };
        match cstr.to_str() {
            Ok(s) => ok(Object::string(s)),
            Err(_) => fail("C 字符串不是合法 UTF-8".to_string()),
        }
    }
}

// ===========================================================================
// 内建描述符：全名带 ffi. 前缀，经 bind_native 按完整名查表
// ===========================================================================

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::ext(
            "ffi.open",
            Some(1),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_open(a))),
        ),
        NativeSpec::ext(
            "ffi.symbol",
            None,
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_symbol(a))),
        ),
        NativeSpec::ext(
            "ffi.call",
            None,
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_call(a))),
        ),
        NativeSpec::ext(
            "ffi.alloc",
            Some(1),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_alloc(a))),
        ),
        NativeSpec::ext(
            "ffi.free",
            Some(1),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_free(a))),
        ),
        NativeSpec::ext(
            "ffi.read",
            Some(2),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_read(a))),
        ),
        NativeSpec::ext(
            "ffi.write",
            Some(3),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_write(a))),
        ),
        NativeSpec::ext(
            "ffi.cstring",
            Some(1),
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_cstring(a))),
        ),
        NativeSpec::ext(
            "ffi.toCString",
            None,
            MODULE,
            Rc::new(|_rt, a| Ok(builtin_to_cstring(a))),
        ),
    ]
}
