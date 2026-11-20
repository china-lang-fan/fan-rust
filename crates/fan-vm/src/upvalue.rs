//! 闭包 upvalue 的运行期表示。
//!
//! 采用 Lua 式 open/closed upvalue：
//! - 函数帧存活时，upvalue 以「open」状态直接引用该帧槽位（共享同一存储，
//!   帧内对局部变量的赋值对所有捕获它的闭包立即可见）；
//! - 帧返回时，被闭包捕获的槽位被「关闭」：值搬进 upvalue 自身持有的存储，
//!   使闭包在帧销毁后仍能读写该变量。
//!
//! 两种状态统一用 [`std::cell::RefCell`] 承载：open 时与槽位背后的 `RefCell`
//! 是同一个 `Rc`，closed 时 `Rc` 独立持有值。

use std::cell::RefCell;
use std::rc::Rc;

use fan_object::Object;

/// 一个 upvalue 槽的共享句柄。多个闭包捕获同一外层变量时持有同一个 `Rc`，
/// 因此写入对所有闭包可见，符合树遍历求值器基于环境的共享语义。
pub type Upvalue = Rc<RefCell<Object>>;

/// 模块在 VM 模块表中的标识（主模块为 0，被导入模块按加载顺序编号）。
pub type ModuleId = u16;

/// 闭包值：所属模块、字节码函数表索引 + 已解析的 upvalue 句柄。
#[derive(Clone)]
pub struct Closure {
    /// 函数体所在模块（跨模块调用时据此定位 ModuleChunk）。
    pub module_id: ModuleId,
    /// 对应所属模块 `ModuleChunk.functions` 的索引。
    pub function_index: u16,
    /// 顺序与该函数 `FunctionChunk.upvalues` 描述一致。
    pub upvalues: Vec<Upvalue>,
    /// 绑定的接收者（方法闭包）：调用时作为隐式首参 `自己` 传入；普通函数为 None。
    pub receiver: Option<Object>,
}
