//! 拼音补全的配置 DTO 与引擎抽象（不含任何实现）。
//!
//! 本模块只定义数据结构与 [`PinyinEngine`] trait，保持 `fan-analysis` 对
//! 读音数据、文件系统与动态库零依赖。具体实现由独立 crate 提供：
//! - 内置引擎 `BuiltinEngine`（全拼/首字母/双拼）位于 `fan-pinyin`；
//! - 可选的 librime 后端在 `fan-lsp` 组合根以动态加载方式注入。
//!
//! ## 后续演进：librime 与自由中文输入
//!
//! 本期只对“语言关键字 + 项目符号”这些有限、可枚举的词做匹配，不打包通用
//! 大词库。若将来需要注释/字符串里的自由整句输入，推荐接入
//! [librime](https://github.com/rime/librime)（可内嵌的跨平台纯引擎库，不
//! 依赖系统 IME 守护进程），在 `fan-lsp` 内实现同一 trait 并通过运行时
//! dlopen 加载；不直接调用各平台系统输入法接口（IBus/Fcitx/IMK/TSF），
//! 因为 LSP 是无窗口的 stdio 子进程，拿不到编辑器的输入上下文。

/// 双拼键位方案。键位对齐 Rime `rime-double-pinyin` 仓库的同名方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DoublePinyinScheme {
    /// 自然码双拼（Rime `double_pinyin`），作为默认方案。
    #[default]
    Ziranma,
    /// 小鹤双拼（Rime `double_pinyin_flypy`）。
    Xiaohe,
}

impl DoublePinyinScheme {
    /// 从配置字符串解析方案，无法识别时回退到自然码。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "xiaohe" | "flypy" | "小鹤" => Self::Xiaohe,
            _ => Self::Ziranma,
        }
    }
}

/// 启用哪些输入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSchemes {
    /// 全拼前缀匹配，如 `dazhaohu`。
    pub full: bool,
    /// 首字母前缀匹配，如 `dzh`。
    pub initial: bool,
    /// 双拼前缀匹配。
    pub double: bool,
}

impl Default for InputSchemes {
    fn default() -> Self {
        // 默认三种模式全开，并行前缀匹配，用户无需切换输入法状态。
        Self {
            full: true,
            initial: true,
            double: true,
        }
    }
}

/// 拼音补全的运行配置（由 LSP `initializationOptions` 映射而来）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PinyinOptions {
    /// 是否启用拼音补全；关闭后完全走原有符号补全。
    pub enabled: bool,
    /// 候选上限。
    pub max_suggest: usize,
    /// 启用的输入模式。
    pub schemes: InputSchemes,
    /// 双拼键位方案。
    pub double_scheme: DoublePinyinScheme,
    /// 引擎选择：`auto`/`builtin` 用内置引擎，`rime` 尝试外部引擎（缺失则回退）。
    pub engine: String,
    /// librime 动态库的显式路径；为空时按平台默认库名探测。
    pub rime_lib_path: String,
    /// Rime shared data 目录（方案 yaml 等）；为空时交给 librime 默认值。
    pub rime_shared_data_dir: String,
    /// Rime user data 目录（用户词库/构建产物）；为空时交给 librime 默认值。
    pub rime_user_data_dir: String,
    /// 启动后切换的 schema id（如 `double_pinyin`）；为空则用 librime 当前方案。
    pub rime_schema_id: String,
}

impl PinyinOptions {
    /// 一套合理的默认配置：启用、上限 50、三模式全开、自然码、自动引擎。
    pub fn sensible_default() -> Self {
        Self {
            enabled: true,
            max_suggest: 50,
            schemes: InputSchemes::default(),
            double_scheme: DoublePinyinScheme::default(),
            engine: "auto".to_string(),
            rime_lib_path: String::new(),
            rime_shared_data_dir: String::new(),
            rime_user_data_dir: String::new(),
            rime_schema_id: String::new(),
        }
    }
}

/// 单个已知词（关键字/项目符号）对当前输入的匹配结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinyinMatch {
    /// 匹配精度分，数值越小越优先（具体量纲由引擎定义，仅用于同引擎内排序）。
    pub rank: usize,
    /// 客户端过滤文本：填入拉丁编码，避免输入字母时中文 label 被编辑器过滤。
    pub filter_text: String,
}

/// 引擎自有自由词候选（内置引擎不产生，由 librime 等外部引擎返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCandidate {
    /// 候选词文本。
    pub word: String,
    /// 来源标记，展示在补全 detail（如 `Rime`）。
    pub detail: String,
    /// 引擎侧词频/权重，数值越大越靠前（仅用于引擎候选内部排序）。
    pub weight: u32,
}

/// 拼音补全引擎抽象。实现位于 `fan-pinyin`（内置）或 `fan-lsp`（外部动态后端）。
///
/// 实现必须是 `Send + Sync`，以便 LSP 在防抖线程间共享。
pub trait PinyinEngine: Send + Sync {
    /// 对候选池里的一个已知词判定是否命中当前拉丁输入，命中则给精度分与过滤文本。
    fn rank(&self, input: &str, word: &str, opts: &PinyinOptions) -> Option<PinyinMatch>;
    /// 返回引擎自有的自由词候选；不支持自由词的引擎返回空数组。
    fn suggest(&self, input: &str, opts: &PinyinOptions) -> Vec<EngineCandidate>;
}

/// 空引擎：两个方法分别返回 None/空，供无实现注入时（如纯逻辑单测、旧入口）使用。
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEngine;

impl PinyinEngine for NullEngine {
    fn rank(&self, _input: &str, _word: &str, _opts: &PinyinOptions) -> Option<PinyinMatch> {
        None
    }

    fn suggest(&self, _input: &str, _opts: &PinyinOptions) -> Vec<EngineCandidate> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 方案解析回退() {
        assert_eq!(
            DoublePinyinScheme::parse("xiaohe"),
            DoublePinyinScheme::Xiaohe
        );
        assert_eq!(
            DoublePinyinScheme::parse("小鹤"),
            DoublePinyinScheme::Xiaohe
        );
        assert_eq!(
            DoublePinyinScheme::parse("anything"),
            DoublePinyinScheme::Ziranma
        );
    }

    #[test]
    fn 空引擎不产候选() {
        let engine = NullEngine;
        let opts = PinyinOptions::sensible_default();
        assert!(engine.rank("dzh", "打招呼", &opts).is_none());
        assert!(engine.suggest("dzh", &opts).is_empty());
    }
}
