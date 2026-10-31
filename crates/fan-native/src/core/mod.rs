//! `core` 组内建：只依赖 Rust 标准库，编译进解释器本体。
//!
//! 每个文件对应标准库中的一个模块文件，命名与 `fan-sdk` 保持一致，
//! 便于对照凡语言层还剩下哪些能力必须由宿主提供。

mod codec;
mod convert;
mod file;
mod math;
mod path;
mod process;
mod random;
mod system;
mod time;

use fan_object::NativeSpec;

/// 收集 `core` 组全部内建描述符。
///
/// 新增内建时，在对应子模块的 `specs()` 中追加一项即可，无需改动此处之外
/// 的代码。尚未纳入的宿主能力组（文件/路径/时间/进程）在后续里程碑补齐。
pub fn specs() -> Vec<NativeSpec> {
    let mut all = Vec::new();
    all.extend(system::specs());
    all.extend(convert::specs());
    all.extend(file::specs());
    all.extend(path::specs());
    all.extend(math::specs());
    all.extend(process::specs());
    all.extend(time::specs());
    all.extend(random::specs());
    all.extend(codec::specs());
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 核心组内建均标记为标准库实现() {
        for spec in specs() {
            assert_eq!(
                spec.group,
                fan_object::NativeGroup::Core,
                "{} 分组错误",
                spec.name
            );
            assert!(spec.ext_module.is_none(), "{} 不应属于动态库", spec.name);
        }
    }

    #[test]
    fn 核心组内建覆盖标准库所需() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        // 这些是凡语言无法表达的原子能力，必须提供
        for required in ["print", "type", "len", "append", "error", "fail"] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
