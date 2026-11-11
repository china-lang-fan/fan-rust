//! 内建名字与类型关键字集合，用于未定义名诊断时排除。
//!
//! 对应 Go 版 `builtins.go`。内建函数不提供悬停说明（无源码注释）。

use std::collections::HashSet;

/// 内建函数与常见全局名字。
pub fn builtin_names() -> HashSet<&'static str> {
    [
        "长度",
        "追加",
        "打印",
        "错误",
        "注册标签处理",
        "捕获",
        "type",
        "len",
        "append",
        "error",
    ]
    .into_iter()
    .collect()
}

/// 类型关键字，出现在表达式位置时不作为未定义名字。
pub fn type_keywords() -> HashSet<&'static str> {
    [
        "整数",
        "小数",
        "字符串",
        "布尔",
        "数组",
        "字典",
        "错误",
        "字节",
        "字符",
        "任意",
        "空",
    ]
    .into_iter()
    .collect()
}

/// 软关键字：作为独立标识符出现时透明忽略，不算未定义。
pub fn soft_keywords() -> HashSet<&'static str> {
    ["通过", "从"].into_iter().collect()
}
