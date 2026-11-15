//! 句柄值：凡语言与底层动态库沟通的不透明引用。
//!
//! 句柄是一等基本类型（关键字 `句柄`，见 [`crate::Kind::Handle`]），用于承载
//! 动态库、函数符号、原生内存等宿主资源。它在 object 层**只保存种类标记与
//! 唯一 id，不持有任何裸指针**——真正的 OS 资源（`dlopen` 句柄、`malloc`
//! 缓冲等）由 fan-native 的 FFI 模块按 id 登记管理，从而让本 crate 保持零
//! 平台/第三方依赖。
//!
//! 设计取舍：
//!
//! - **不透明**：凡语言不能对句柄做指针算术，也不能与整数互转，只能通过
//!   `动态库.凡` 提供的内建操作，杜绝野指针与越界访问从语言层流入。
//! - **按 id 判等**：两个句柄相等当且仅当指向同一登记项，绝不比较地址数值。
//! - **不自动释放**：句柄离开作用域不会自动回收原生内存，调用方必须显式
//!   释放（`ffi.free`/关闭库），避免与 C 库的所有权约定冲突造成 double-free。

use std::cell::Cell;

/// 句柄指向的资源种类，仅用于分类展示与 FFI 模块的分派校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    /// `dlopen` 得到的动态库。
    Library,
    /// `dlsym` 得到的函数符号。
    Symbol,
    /// `malloc`/C 字符串等原生内存缓冲。
    Memory,
}

impl HandleKind {
    /// 种类的中文短名，用于 inspect 展示。
    pub fn label(self) -> &'static str {
        match self {
            HandleKind::Library => "库",
            HandleKind::Symbol => "符号",
            HandleKind::Memory => "内存",
        }
    }
}

// 进程内单调递增的句柄 id 来源。
//
// 句柄可能被 `Clone`（浅拷贝共享同一登记项），故 id 在构造时一次性分配，
// 克隆体保留相同 id，与 FFI 注册表里的同一项对应。
thread_local! {
    static HANDLE_SEQ: Cell<u64> = const { Cell::new(1) };
}

fn next_id() -> u64 {
    HANDLE_SEQ.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    })
}

/// 一个不透明句柄值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleValue {
    /// 资源种类。
    kind: HandleKind,
    /// 进程内唯一 id，FFI 模块据此在注册表中取出实际资源。
    id: u64,
}

impl HandleValue {
    /// 构造指定种类的新句柄，自动分配唯一 id。
    pub fn new(kind: HandleKind) -> Self {
        HandleValue {
            kind,
            id: next_id(),
        }
    }

    /// 资源种类。
    pub fn kind(&self) -> HandleKind {
        self.kind
    }

    /// 句柄的唯一 id（供 FFI 注册表做键）。
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl std::fmt::Display for HandleValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不暴露可参与运算的地址，只给种类与 id，便于排查又无法被误用为指针。
        write!(f, "句柄({}#{})", self.kind.label(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 每个句柄获得唯一编号() {
        let a = HandleValue::new(HandleKind::Memory);
        let b = HandleValue::new(HandleKind::Memory);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn 克隆共享同一编号() {
        let a = HandleValue::new(HandleKind::Library);
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn 种类标签正确() {
        assert_eq!(HandleKind::Library.label(), "库");
        assert_eq!(HandleKind::Symbol.label(), "符号");
        assert_eq!(HandleKind::Memory.label(), "内存");
    }

    #[test]
    fn 展示文本含种类与编号且不暴露地址() {
        let h = HandleValue::new(HandleKind::Memory);
        let text = h.to_string();
        assert!(text.starts_with("句柄(内存#"), "实际：{text}");
        assert!(text.ends_with(')'));
    }
}
