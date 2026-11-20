//! 模块加载策略的运行期注入点。
//!
//! fan-vm 的生产依赖不包含 fan-parser / 文件系统，无法自行把模块路径解析为
//! 可执行模块。具体加载（读文件、解析、递归编译、缓存与环检测）由装配层
//! （如 fan-cli）实现本 trait 后注入 VM，保持 VM 纯粹的「取指-执行」职责。
//!
//! 加载结果统一是 fan-object 的 [`Object::Module`]，其 `exports` 已填充为
//! 模块内所有 `公开` 绑定；VM 只在 `LoadModule` 时按路径查询。

use fan_object::Object;

use crate::error::VmResult;

/// 模块加载器：把源码中的导入路径解析为模块对象。
///
/// 实现方需自行保证：
/// - 同一模块路径只加载一次（缓存），与树遍历求值器的 Loader 行为一致；
/// - 循环导入被检测并以错误返回；
/// - 路径解析遵循 SDK / 依赖目录 / 相对路径的搜索顺序。
pub trait ModuleLoader {
    /// 按导入路径加载模块，返回 `Object::Module(..)`。
    fn load(&mut self, path: &str) -> VmResult<Object>;
}
