//! `定义 本地("模块.函数")` 的静态诊断。
//!
//! 运行时语义是「native 优先、找不到则静默回退函数体」，因此 native 名拼错
//! 不会产生任何运行时信号。这里在**不加载扩展动态库**的前提下，静态读取
//! fan-native 的 core/ext 默认实现名单，对三类「找不到」给出 Warning 级提示，
//! 不阻断运行（回退仍会发生）：
//!
//! 1. 无模块前缀且名字不在 core 内建里；
//! 2. 模块名没有任何扩展实现（既无动态库也无默认实现）；
//! 3. 模块存在但其中没有该函数名。

use std::collections::HashSet;

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;
use fan_ast::{NativeDecl, Program};
use fan_native::ext::default;

use crate::diagnostic::{Diagnostic, Severity};

/// 静态 native 目录：core 名集合 + 各扩展模块的函数名集合。
///
/// 全部来自编译期内建描述符，构造过程不触碰文件系统、不 dlopen。
#[derive(Debug, Default)]
struct NativeCatalog {
    /// core 组内建名（无模块前缀时查这里）。
    core: HashSet<&'static str>,
    /// 已知扩展模块（小写别名归一）→ 该模块默认实现提供的函数名。
    ext_modules: std::collections::HashMap<String, HashSet<&'static str>>,
}

impl NativeCatalog {
    fn build() -> Self {
        let mut catalog = NativeCatalog::default();
        for spec in fan_native::core::specs() {
            catalog.core.insert(spec.name);
        }
        for (alias, specs) in [
            ("json", default::json::specs()),
            ("regex", default::regex::specs()),
            ("http", default::http::specs()),
        ] {
            let names = specs.iter().map(|s| s.name).collect();
            catalog.ext_modules.insert(alias.to_string(), names);
        }
        catalog
    }

    /// 把源码里的模块名归一到目录键（小写；中文别名映射到 ASCII 库名）。
    fn normalize_module(module: &str) -> String {
        match module.to_lowercase().as_str() {
            "正则" => "regex".to_string(),
            "网络" => "http".to_string(),
            other => other.to_string(),
        }
    }

    /// 校验一条本地声明，返回 0 或 1 条 Warning。
    fn check_decl(&self, decl: &NativeDecl) -> Option<Diagnostic> {
        let (line, column) = (decl.pos.line, decl.pos.column);
        let warn = |msg: String| Diagnostic::new(line, column, Severity::Warning, "native", msg);

        match &decl.module {
            // 无模块前缀：只能命中 core 已注册内建。
            None => {
                if self.core.contains(decl.name.as_str()) {
                    None
                } else {
                    Some(warn(format!(
                        "本地绑定 {} 未在 core 内建中注册，运行时将回退执行函数体",
                        decl.spec
                    )))
                }
            }
            // 带模块前缀：先看模块是否存在，再看模块里有没有该函数。
            Some(module) => {
                let key = Self::normalize_module(module);
                match self.ext_modules.get(&key) {
                    None => {
                        if default::has_default(module) {
                            // 别名已归一却仍走到这里，属于目录登记遗漏：不报为妙。
                            None
                        } else {
                            Some(warn(format!(
                                "本地绑定模块 {} 没有扩展实现，运行时将回退执行函数体",
                                module
                            )))
                        }
                    }
                    Some(names) => {
                        if names.contains(decl.name.as_str()) {
                            None
                        } else {
                            Some(warn(format!(
                                "模块 {} 中没有原生函数 {}，运行时将回退执行函数体",
                                module, decl.name
                            )))
                        }
                    }
                }
            }
        }
    }
}

/// 遍历程序里所有带 `本地` 声明的函数，产出 native 诊断。
pub fn check_native_bindings(prog: &Program) -> Vec<Diagnostic> {
    let catalog = NativeCatalog::build();
    let mut diags = Vec::new();
    for stmt in &prog.statements {
        let inner = match stmt {
            Stmt::Visibility(v) => &*v.inner,
            other => other,
        };
        let Stmt::VarDecl(v) = inner else { continue };
        let Some(Expr::Function(f)) = &v.value else {
            continue;
        };
        if let Some(decl) = &f.native {
            if let Some(diag) = catalog.check_decl(decl) {
                diags.push(diag);
            }
        }
    }
    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;

    fn diag_codes(src: &str) -> Vec<(String, Severity)> {
        let (prog, errors) = parse_program(src);
        assert!(errors.is_empty(), "解析错误：{errors}");
        check_native_bindings(&prog)
            .into_iter()
            .map(|d| (d.code, d.severity))
            .collect()
    }

    fn warnings(src: &str) -> Vec<Diagnostic> {
        let (prog, errors) = parse_program(src);
        assert!(errors.is_empty(), "解析错误：{errors}");
        check_native_bindings(&prog)
    }

    #[test]
    fn core已注册名无诊断() {
        // print 是 core 内建。
        let src = "定义 本地(\"print\") 函数 输出(任意... 内容)\n    返回 空\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 未注册core名告警() {
        let src = "定义 本地(\"不存在的函数\") 函数 甲()\n    返回 空\n结束\n";
        let diags = warnings(src);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].code, "native");
        assert!(diags[0].message.contains("core 内建"));
    }

    #[test]
    fn 扩展模块命中默认实现无诊断() {
        let src = "定义 本地(\"json.jsonEncode\") 函数 生成(任意 值)\n    返回 空\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 中文模块别名可识别() {
        let src =
            "定义 本地(\"正则.regexMatch\") 函数 匹配(任意 模式, 任意 文本)\n    返回 空\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 模块中无此函数告警() {
        let src = "定义 本地(\"json.不存在\") 函数 甲()\n    返回 空\n结束\n";
        let diags = warnings(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("没有原生函数"));
    }

    #[test]
    fn 未知模块告警() {
        let src = "定义 本地(\"不存在的模块.某某\") 函数 甲()\n    返回 空\n结束\n";
        let diags = warnings(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("没有扩展实现"));
    }

    #[test]
    fn 普通函数不产生native诊断() {
        let src = "函数 甲()\n    返回 1\n结束\n";
        assert!(warnings(src).is_empty());
    }

    #[test]
    fn 诊断级别为warning不阻断() {
        let src = "定义 本地(\"nope\") 函数 甲()\n    返回 1\n结束\n";
        assert_eq!(
            diag_codes(src),
            vec![("native".to_string(), Severity::Warning)]
        );
    }
}
