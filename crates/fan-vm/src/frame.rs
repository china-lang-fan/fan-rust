//! 虚拟机调用帧：一次函数/模块执行的指令游标与局部槽位。

use fan_object::Object;

use crate::upvalue::{ModuleId, Upvalue};

/// 一条 `稍后` 登记：零参延迟闭包（已闭合登记时刻的实参与外层变量）。
pub struct Deferred {
    /// 延迟执行的零参字节码闭包值（在闭包身份表中可查）。
    pub closure: Object,
}

/// 一帧对应一个可执行块（模块入口或函数体）。
#[derive(Default)]
pub struct Frame {
    /// 本帧所属模块：入口帧对应当前执行的模块，函数帧指向函数体所在模块
    /// （跨模块调用时可能与调用方不同）。
    pub module_id: ModuleId,
    /// 所属代码块在该模块函数表中的索引；模块入口用专用值 [`Frame::ENTRY`]。
    pub function_index: u16,
    /// 指令指针：下一条待取指令在代码块 `code` 中的字节偏移。
    pub ip: usize,
    /// 本帧局部槽位在 VM 槽位数组中的基址。
    pub slot_base: usize,
    /// 实际收到的实参个数（模块入口为 0），供默认参数缺省判定读取。
    pub argc: usize,
    /// 本帧创建的 open upvalue，按本帧局部槽的绝对位置登记。
    /// 帧返回时统一关闭（把槽位当前值搬进 cell），使闭包在帧销毁后仍可读写。
    pub open_upvalues: Vec<(usize, Upvalue)>,
    /// 本帧登记的 `稍后` 闭包，按登记顺序存放，帧退出时后进先出执行。
    pub defers: Vec<Deferred>,
    /// `返回 标识符` 记录的返回值来源局部槽（绝对位置）；defer 执行后据此重读，
    /// 使 defer 对该局部变量的写入能反映到返回值（对齐树遍历求值器语义）。
    /// 复杂返回表达式无法静态定位到单一槽位时为 None，沿用值栈上已求好的结果。
    pub return_slot: Option<usize>,
}

impl Frame {
    /// 标记当前帧执行的是模块入口块（而非函数表中的函数）。
    pub const ENTRY: u16 = u16::MAX;

    pub fn entry(module_id: ModuleId, slot_base: usize) -> Self {
        Frame {
            module_id,
            function_index: Frame::ENTRY,
            ip: 0,
            slot_base,
            argc: 0,
            open_upvalues: Vec::new(),
            defers: Vec::new(),
            return_slot: None,
        }
    }

    /// 函数帧构造器，函数调用里程碑（M1）启用。
    pub fn function(
        module_id: ModuleId,
        function_index: u16,
        slot_base: usize,
        argc: usize,
    ) -> Self {
        Frame {
            module_id,
            function_index,
            ip: 0,
            slot_base,
            argc,
            open_upvalues: Vec::new(),
            defers: Vec::new(),
            return_slot: None,
        }
    }
}
