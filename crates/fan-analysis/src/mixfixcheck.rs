//! 分段（mixfix）函数调用检查：段间空白与签名匹配。
//!
//! 对应 Go 版 `analysis.go` 中的 `checkMixfixSpacing` 等。只对最外层调用链
//! 诊断一次，避免对嵌套的成员/调用节点重复报错。

use std::collections::HashMap;

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;
use fan_ast::{Position, Program};

use crate::diagnostic::{Diagnostic, Severity};
use crate::walk::{walk_with_parents, NodeRef};

/// 一个分段函数定义的静态形状。
#[derive(Debug, Clone)]
struct MixfixDef {
    segments: Vec<String>,
    connectors: Vec<String>,
}

/// 以首段名为键的分段函数注册表。
#[derive(Debug, Default)]
struct MixfixRegistry {
    roots: HashMap<String, Vec<MixfixDef>>,
    /// 所有分段定义的完整拼接名（如 `修改的状态为有效`），用于扁平调用比对。
    full_names: std::collections::HashSet<String>,
}

impl MixfixRegistry {
    fn has_root(&self, name: &str) -> bool {
        self.roots.contains_key(name)
    }

    fn has_full_name(&self, name: &str) -> bool {
        self.full_names.contains(name)
    }
}

/// 还原出来的一条调用链。
#[derive(Debug, Clone)]
struct Chain {
    root: String,
    segments: Vec<String>,
    connectors: Vec<String>,
    spaced: Vec<bool>,
    root_pos: Position,
    connector_pos: Vec<Position>,
}

/// 从顶层语句收集分段函数定义。
fn collect_mixfix_defs(prog: &Program) -> MixfixRegistry {
    let mut registry = MixfixRegistry::default();
    for stmt in &prog.statements {
        let inner = match stmt {
            Stmt::Visibility(v) => &*v.inner,
            other => other,
        };
        let Stmt::VarDecl(v) = inner else { continue };
        let Some(Expr::Function(f)) = &v.value else {
            continue;
        };
        if !f.is_segmented() {
            continue;
        }
        let root = f.name_segments[0].clone();
        registry.full_names.insert(joined_mixfix_name(f));
        registry.roots.entry(root).or_default().push(MixfixDef {
            segments: f.name_segments.clone(),
            connectors: f.connectors.clone(),
        });
    }
    registry
}

/// 按 `连接词+段名` 顺序拼出分段函数的完整名字（首段连接词为空）。
fn joined_mixfix_name(f: &fan_ast::expr::FunctionLit) -> String {
    let mut name = String::new();
    for (i, seg) in f.name_segments.iter().enumerate() {
        name.push_str(&f.connectors[i]);
        name.push_str(seg);
    }
    name
}

/// 从表达式还原分段调用链；非链式结构返回 `None`。
fn collect_chain(mut current: &Expr) -> Option<Chain> {
    struct OutSeg {
        connector: String,
        name: String,
        spaced: bool,
        connector_pos: Position,
    }
    let mut outward: Vec<OutSeg> = Vec::new();
    loop {
        match current {
            Expr::Call(call) => match &*call.callee {
                // 根调用 `名字(...)`：取其 Ident 后结束向内展开。
                Expr::Ident(_) => {
                    current = &*call.callee;
                    break;
                }
                Expr::Member(m) => {
                    outward.push(OutSeg {
                        connector: m.connector.clone(),
                        name: m.name.clone(),
                        spaced: m.spaced,
                        connector_pos: m.pos,
                    });
                    current = &m.object;
                }
                _ => return None,
            },
            Expr::ImplicitReceiverCall(irc) => {
                outward.push(OutSeg {
                    connector: irc.connector.clone(),
                    name: irc.name.clone(),
                    spaced: irc.spaced,
                    connector_pos: irc.pos,
                });
                current = &irc.receiver;
            }
            Expr::Member(m) => {
                outward.push(OutSeg {
                    connector: m.connector.clone(),
                    name: m.name.clone(),
                    spaced: m.spaced,
                    connector_pos: m.pos,
                });
                current = &m.object;
            }
            Expr::MaybeCall(mc) => current = &mc.callee,
            _ => break,
        }
    }
    let Expr::Ident(id) = current else {
        return None;
    };
    if outward.is_empty() {
        return None;
    }
    let mut chain = Chain {
        root: id.name.clone(),
        segments: vec![id.name.clone()],
        connectors: vec![String::new()],
        spaced: vec![false],
        root_pos: id.pos,
        connector_pos: vec![Position::default()],
    };
    for part in outward.into_iter().rev() {
        chain.connectors.push(part.connector);
        chain.segments.push(part.name);
        chain.spaced.push(part.spaced);
        chain.connector_pos.push(part.connector_pos);
    }
    Some(chain)
}

/// 检查整份程序的分段调用。
pub fn check_mixfix_spacing(prog: &Program) -> Vec<Diagnostic> {
    let registry = collect_mixfix_defs(prog);
    let mut diags = Vec::new();
    walk_with_parents(NodeRef::Program(prog), |node, parent| {
        let Some(expr) = node.as_expr() else {
            return true;
        };

        // 紧邻的分段调用在 parser 中被压成扁平 Call（callee 为拼接名 Ident，
        // 并带 mixfix_root）；此时没有空格问题，只需校验能否匹配到定义。
        if let Expr::Call(call) = expr {
            if let Some(root) = &call.mixfix_root {
                if registry.has_root(root) {
                    if let Expr::Ident(id) = &*call.callee {
                        if !registry.has_full_name(&id.name) {
                            diags.push(Diagnostic {
                                line: id.pos.line,
                                column: id.pos.column,
                                end_line: id.pos.line,
                                end_column: id.pos.column + id.name.chars().count(),
                                severity: Severity::Warning,
                                code: "mixfix-unresolved".to_string(),
                                message: format!(
                                    "此处按分段函数调用解析，但找不到名称与段数匹配的分段函数 {}；请检查各段名称。",
                                    id.name
                                ),
                            });
                        }
                    }
                    return true;
                }
            }
        }

        let Some(chain) = collect_chain(expr) else {
            return true;
        };
        if !registry.has_root(&chain.root) {
            return true;
        }
        // 父节点仍是同根更长的链时跳过，只在最外层诊断一次。
        if let Some(pexpr) = parent.and_then(|p| p.as_expr()) {
            if let Some(pchain) = collect_chain(pexpr) {
                if pchain.root == chain.root && pchain.segments.len() > chain.segments.len() {
                    return true;
                }
            }
        }
        diags.extend(diagnose_chain(&chain, &registry));
        true
    });
    diags
}

fn diagnose_chain(chain: &Chain, registry: &MixfixRegistry) -> Vec<Diagnostic> {
    for (i, spaced) in chain.spaced.iter().enumerate() {
        if !*spaced || i == 0 {
            continue;
        }
        let pos = chain.connector_pos[i];
        let connector = if chain.connectors[i].is_empty() {
            "的".to_string()
        } else {
            chain.connectors[i].clone()
        };
        return vec![Diagnostic {
            line: pos.line,
            column: pos.column,
            end_line: pos.line,
            end_column: pos.column + connector.chars().count(),
            severity: Severity::Warning,
            code: "mixfix-space".to_string(),
            message: format!(
                "分段函数各段之间不能有空格；这里会先普通调用 {}(...)，再访问返回值的成员，与分段函数是两件事。若要调用分段函数，请删除 “{}” 前的空格。",
                chain.root, connector
            ),
        }];
    }
    if !matches_any_def(chain, registry) {
        let pos = chain.root_pos;
        return vec![Diagnostic {
            line: pos.line,
            column: pos.column,
            end_line: pos.line,
            end_column: pos.column + chain.root.chars().count(),
            severity: Severity::Warning,
            code: "mixfix-unresolved".to_string(),
            message: format!(
                "此处按分段函数调用解析，但找不到名称与段数匹配的分段函数 {}；请检查各段名称。",
                chain_signature(chain)
            ),
        }];
    }
    Vec::new()
}

fn matches_any_def(chain: &Chain, registry: &MixfixRegistry) -> bool {
    let Some(defs) = registry.roots.get(&chain.root) else {
        return false;
    };
    defs.iter().any(|def| {
        if def.segments.len() != chain.segments.len() {
            return false;
        }
        (1..def.segments.len()).all(|i| {
            def.segments[i] == chain.segments[i] && def.connectors[i] == chain.connectors[i]
        })
    })
}

fn chain_signature(chain: &Chain) -> String {
    let mut builder = String::new();
    for (i, segment) in chain.segments.iter().enumerate() {
        builder.push_str(&chain.connectors[i]);
        builder.push_str(segment);
        builder.push_str("(...)");
    }
    builder
}

#[cfg(test)]
mod tests {
    use fan_parser::parse_program;

    use super::*;

    const DEF: &str = "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n    返回 值\n结束\n";

    #[test]
    fn 段间带空格报mixfix_space() {
        let src = format!("{DEF}变量 x = 修改(\"a\") 的状态为(\"b\")\n");
        let (prog, _) = parse_program(&src);
        let diags = check_mixfix_spacing(&prog);
        assert!(
            diags.iter().any(|d| d.code == "mixfix-space"),
            "应报段间空格：{diags:?}"
        );
    }

    #[test]
    fn 紧邻的合法分段调用不报错() {
        let src = format!("{DEF}变量 x = 修改(\"a\")的状态为(\"b\")\n");
        let (prog, _) = parse_program(&src);
        assert!(check_mixfix_spacing(&prog).is_empty());
    }

    #[test]
    fn 段名不匹配报未解析() {
        let src = format!("{DEF}变量 x = 修改(\"a\")的别的段(\"b\")\n");
        let (prog, _) = parse_program(&src);
        let diags = check_mixfix_spacing(&prog);
        assert!(
            diags.iter().any(|d| d.code == "mixfix-unresolved"),
            "应报未解析：{diags:?}"
        );
    }
}
