//! 虚拟机调用帧：一次函数/模块执行的指令游标与局部槽位。

/// 一帧对应一个可执行块（模块入口或函数体）。
#[derive(Debug)]
pub struct Frame {
    /// 所属代码块在模块函数表中的索引；模块入口用专用值 [`Frame::ENTRY`]。
    pub function_index: u16,
    /// 指令指针：下一条待取指令在代码块 `code` 中的字节偏移。
    pub ip: usize,
    /// 本帧局部槽位在 VM 槽位数组中的基址。
    pub slot_base: usize,
}

impl Frame {
    /// 标记当前帧执行的是模块入口块（而非函数表中的函数）。
    pub const ENTRY: u16 = u16::MAX;

    pub fn entry(slot_base: usize) -> Self {
        Frame {
            function_index: Frame::ENTRY,
            ip: 0,
            slot_base,
        }
    }

    /// 函数帧构造器，函数调用里程碑（M1）启用。
    #[allow(dead_code)]
    pub fn function(function_index: u16, slot_base: usize) -> Self {
        Frame {
            function_index,
            ip: 0,
            slot_base,
        }
    }
}
