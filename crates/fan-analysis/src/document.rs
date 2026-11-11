//! 单文件静态分析视图：解析源码并收集顶层符号、模型/接口成员、
//! 文档注释与函数体行范围。对应 Go 版 `analysis.Document`。

use std::collections::HashMap;

use fan_ast::expr::{Expr, FunctionLit};
use fan_ast::stmt::{InterfaceMethodSig, MethodDef, Stmt};
use fan_ast::{Position, Program};
use fan_parser::parse_program;

use crate::symbol::{Symbol, SymbolKind};
use crate::walk::{walk, NodeRef};

/// 导入信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportInfo {
    pub path: String,
    pub name_pos: Position,
}

/// 函数体在文件中的行范围（1-based，闭区间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnRange {
    pub start_line: usize,
    pub end_line: usize,
}

/// 单个凡语言源文件的静态视图。
#[derive(Debug)]
pub struct Document {
    pub uri: String,
    pub source: String,
    lines: Vec<String>,
    pub prog: Program,
    /// 按声明出现顺序保存的顶层符号。
    pub top_levels: Vec<Symbol>,
    /// 名字 → top_levels 下标（函数可重载，保留全部）。
    pub by_name: HashMap<String, Vec<usize>>,
    pub classes: HashMap<String, usize>,
    pub interfaces: HashMap<String, usize>,
    pub named_types: HashMap<String, usize>,
    pub imports: HashMap<String, ImportInfo>,
    fn_ranges: Vec<FnRange>,
}

impl Document {
    /// 解析并建立单文件符号视图。解析出错时仍返回已能构建的部分程序。
    pub fn parse(uri: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        let (prog, _errors) = parse_program(&source);
        let lines: Vec<String> = source.split('\n').map(str::to_string).collect();
        let mut doc = Document {
            uri: uri.into(),
            source,
            lines,
            prog,
            top_levels: Vec::new(),
            by_name: HashMap::new(),
            classes: HashMap::new(),
            interfaces: HashMap::new(),
            named_types: HashMap::new(),
            imports: HashMap::new(),
            fn_ranges: Vec::new(),
        };
        doc.collect();
        doc.collect_function_ranges();
        doc
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// 取 1-based 行号对应的文本，越界返回空串。
    pub fn line_text(&self, line1: usize) -> &str {
        self.lines
            .get(line1.wrapping_sub(1))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// 按名字查找顶层符号。
    pub fn find_by_name(&self, name: &str) -> Vec<&Symbol> {
        self.by_name
            .get(name)
            .map(|idxs| idxs.iter().map(|&i| &self.top_levels[i]).collect())
            .unwrap_or_default()
    }

    pub fn class(&self, name: &str) -> Option<&Symbol> {
        self.classes.get(name).map(|&i| &self.top_levels[i])
    }

    /// 返回包含指定 0-based 行的最内层函数体的行范围。
    pub fn function_at_line(&self, line0: usize) -> Option<FnRange> {
        let line1 = line0 + 1;
        let mut best: Option<FnRange> = None;
        let mut best_span = usize::MAX;
        for range in &self.fn_ranges {
            if line1 >= range.start_line && line1 <= range.end_line {
                let span = range.end_line - range.start_line;
                if span < best_span {
                    best_span = span;
                    best = Some(*range);
                }
            }
        }
        best
    }

    fn add(&mut self, sym: Symbol) {
        let name = sym.name.clone();
        let idx = self.top_levels.len();
        self.top_levels.push(sym);
        self.by_name.entry(name).or_default().push(idx);
    }

    fn collect(&mut self) {
        let statements = self.prog.statements.clone();
        for stmt in &statements {
            self.collect_statement(stmt);
        }
    }

    fn collect_statement(&mut self, stmt: &Stmt) {
        let inner = match stmt {
            Stmt::Visibility(v) => &*v.inner,
            other => other,
        };
        match inner {
            Stmt::VarDecl(v) => {
                if let Some(Expr::Function(fn_lit)) = &v.value {
                    self.collect_function(&v.name, fn_lit, v.pos);
                    return;
                }
                let kind = if v.is_const {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };
                let mut sym = Symbol::new(&v.name, kind, v.pos);
                sym.detail = v.decl_type.to_string();
                sym.doc = self.doc_before(v.pos.line);
                sym.end_pos = self.end_of(v.pos);
                self.add(sym);
            }
            Stmt::Class(c) => {
                let mut class_sym = Symbol::new(&c.name, SymbolKind::Class, c.pos);
                class_sym.doc = self.doc_before(c.pos.line);
                class_sym.end_pos = self.end_of(c.pos);
                class_sym.embeds = c.embeds.clone();
                for field in &c.fields {
                    let mut child = Symbol::new(&field.name, SymbolKind::Field, field.pos);
                    child.owner = c.name.clone();
                    child.detail = field.type_.to_string();
                    child.doc = self.doc_before(field.pos.line);
                    child.end_pos = Position::new(
                        field.pos.line,
                        field.pos.column + field.name.chars().count(),
                    );
                    class_sym.children.push(child);
                }
                self.classes.insert(c.name.clone(), self.top_levels.len());
                self.add(class_sym);
            }
            Stmt::Interface(i) => {
                let mut iface_sym = Symbol::new(&i.name, SymbolKind::Interface, i.pos);
                iface_sym.doc = self.doc_before(i.pos.line);
                iface_sym.end_pos = self.end_of(i.pos);
                for m in &i.methods {
                    let mut child = Symbol::new(&m.name, SymbolKind::Method, m.pos);
                    child.owner = i.name.clone();
                    child.detail = interface_method_signature(m);
                    child.end_pos =
                        Position::new(m.pos.line, m.pos.column + m.name.chars().count());
                    iface_sym.children.push(child);
                }
                self.interfaces
                    .insert(i.name.clone(), self.top_levels.len());
                self.add(iface_sym);
            }
            Stmt::TypeDef(t) => {
                let mut sym = Symbol::new(&t.name, SymbolKind::Type, t.pos);
                sym.detail = format_type_detail(&t.base_type.to_string());
                sym.doc = self.doc_before(t.pos.line);
                sym.end_pos = self.end_of(t.pos);
                self.named_types
                    .insert(t.name.clone(), self.top_levels.len());
                self.add(sym);
            }
            Stmt::MethodDef(m) => {
                let mut sym = Symbol::new(&m.method_name, SymbolKind::Method, m.pos);
                sym.owner = m.class_name.clone();
                sym.detail = method_signature(m);
                sym.doc = self.doc_before(m.pos.line);
                sym.end_pos = self.end_of(m.pos);
                self.add(sym);
                if let Some(idx) = self.classes.get(&m.class_name).copied() {
                    let method = self.top_levels.last().unwrap().clone();
                    self.top_levels[idx].children.push(method);
                }
            }
            Stmt::Import(imp) => {
                self.imports.insert(
                    imp.name.clone(),
                    ImportInfo {
                        path: imp.path.clone(),
                        name_pos: imp.pos,
                    },
                );
            }
            _ => {}
        }
    }

    fn collect_function(&mut self, name: &str, fn_lit: &FunctionLit, pos: Position) {
        let (kind, detail) = if fn_lit.is_segmented() {
            (SymbolKind::MixfixFunction, fn_lit.signature.clone())
        } else {
            (SymbolKind::Function, simple_signature(name, fn_lit))
        };
        let mut sym = Symbol::new(name, kind, pos);
        sym.detail = detail;
        sym.doc = self.doc_before(pos.line);
        sym.end_pos = self.end_of(pos);
        for param in &fn_lit.params {
            let mut p = Symbol::new(&param.name, SymbolKind::Parameter, pos);
            p.owner = name.to_string();
            p.detail = param.type_.to_string();
            sym.children.push(p);
        }
        self.add(sym);
    }

    fn collect_function_ranges(&mut self) {
        let prog = self.prog.clone();
        walk(NodeRef::Program(&prog), |node| {
            if let NodeRef::Expr(Expr::Function(fn_lit)) = node {
                let start = fn_lit.pos.line;
                let mut end = start;
                for stmt in &fn_lit.body.statements {
                    end = end.max(node_max_line(stmt));
                }
                self.fn_ranges.push(FnRange {
                    start_line: start,
                    end_line: end,
                });
            }
            true
        });
    }

    fn end_of(&self, pos: Position) -> Position {
        let line = self.line_text(pos.line);
        Position::new(pos.line, line.chars().count() + 1)
    }

    /// 提取紧贴指定行（1-based）上方、中间无空行的连续整行注释。
    fn doc_before(&self, decl_line: usize) -> String {
        let mut docs: Vec<String> = Vec::new();
        let mut line = decl_line;
        while line > 1 {
            line -= 1;
            let trimmed = self.line_text(line).trim();
            if trimmed.is_empty() {
                break;
            }
            match doc_comment_text(trimmed) {
                Some(text) => docs.insert(0, text),
                None => break,
            }
        }
        docs.join("\n")
    }
}

fn node_max_line(stmt: &Stmt) -> usize {
    let mut max_line = stmt.pos().line;
    walk(NodeRef::Stmt(stmt), |node| {
        max_line = max_line.max(node.pos().line);
        true
    });
    max_line
}

/// 判断一行是否为整行文档注释，返回去掉注释前缀的文本。
fn doc_comment_text(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix('#') {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("注释") {
        if rest.is_empty() {
            return Some(String::new());
        }
        let first = rest.chars().next()?;
        if first == ' ' || first == '\t' || first == ':' || first == '：' {
            return Some(rest.trim_matches([' ', '\t', ':', '：']).to_string());
        }
    }
    None
}

/// 把内部联合类型表示（`|` 连接）转为可读形式（` 或 ` 连接）。
fn format_type_detail(t: &str) -> String {
    if t.contains('|') {
        t.split('|').collect::<Vec<_>>().join(" 或 ")
    } else {
        t.to_string()
    }
}

fn simple_signature(name: &str, fn_lit: &FunctionLit) -> String {
    let mut s = String::from(name);
    s.push('(');
    for (i, param) in fn_lit.params.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        if param.variadic {
            s.push_str("...");
        }
        s.push_str(&param.name);
        if !param.type_.is_any() {
            s.push(' ');
            s.push_str(&param.type_.to_string());
        }
    }
    s.push(')');
    if !fn_lit.return_types.is_empty() {
        s.push_str(" -> ");
        let returns: Vec<String> = fn_lit.return_types.iter().map(|t| t.to_string()).collect();
        s.push_str(&returns.join("、"));
    }
    s
}

fn method_signature(m: &MethodDef) -> String {
    simple_signature(&m.method_name, &m.function)
}

fn interface_method_signature(m: &InterfaceMethodSig) -> String {
    let mut s = String::from(&m.name);
    s.push('(');
    for (i, param) in m.params.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&param.name);
        if !param.type_.is_any() {
            s.push(' ');
            s.push_str(&param.type_.to_string());
        }
    }
    s.push(')');
    if !m.return_types.is_empty() {
        s.push_str(" -> ");
        let returns: Vec<String> = m.return_types.iter().map(|t| t.to_string()).collect();
        s.push_str(&returns.join(", "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 收集顶层函数与参数() {
        let doc = Document::parse(
            "file:///t.凡",
            "# 打招呼\n函数 打招呼(字符串 名字) -> 字符串\n    返回 名字\n结束\n",
        );
        let sym = &doc.top_levels[0];
        assert_eq!(sym.name, "打招呼");
        assert_eq!(sym.kind, SymbolKind::Function);
        assert_eq!(sym.doc, "打招呼");
        assert!(sym.detail.contains("打招呼(名字 字符串)"), "实际：{}", sym.detail);
        assert_eq!(sym.children.len(), 1);
        assert_eq!(sym.children[0].kind, SymbolKind::Parameter);
    }

    #[test]
    fn 收集模型字段与嵌入() {
        let doc = Document::parse(
            "file:///t.凡",
            "模型 形状\n    属性 整数 边数\n结束\n模型 矩形\n    嵌入 形状\n    属性 整数 宽\n结束\n",
        );
        let rect = doc.class("矩形").unwrap();
        assert_eq!(rect.embeds, vec!["形状".to_string()]);
        assert_eq!(rect.children.len(), 1);
        assert_eq!(rect.children[0].name, "宽");
        assert_eq!(rect.children[0].kind, SymbolKind::Field);
    }

    #[test]
    fn 分段函数被标记为混合种类() {
        let doc = Document::parse(
            "file:///t.凡",
            "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n    返回 值\n结束\n",
        );
        assert_eq!(doc.top_levels[0].kind, SymbolKind::MixfixFunction);
    }

    #[test]
    fn 文档注释支持两种写法() {
        assert_eq!(doc_comment_text("# 说明").unwrap(), "说明");
        assert_eq!(doc_comment_text("注释 说明").unwrap(), "说明");
        assert_eq!(doc_comment_text("注释: 说明").unwrap(), "说明");
        assert!(doc_comment_text("变量 x = 1").is_none());
    }

    #[test]
    fn 按名字查找与函数行范围() {
        let src = "函数 甲() -> 整数\n    如果 真 那么\n        返回 1\n    结束\n结束\n";
        let doc = Document::parse("file:///t.凡", src);
        assert_eq!(doc.find_by_name("甲").len(), 1);
        let range = doc.function_at_line(2).expect("第3行应在函数体内");
        assert_eq!(range.start_line, 1);
        // 函数体最末语句（返回）在第 3 行；`结束` 关键字不产生 AST 节点
        assert_eq!(range.end_line, 3);
        assert!(doc.function_at_line(99).is_none());
    }

    #[test]
    fn 收集类型别名与导入() {
        let doc = Document::parse(
            "file:///t.凡",
            "定义 类型 编号 = 整数 或 字符串\n导入 \"进程\" 作为 进程模块\n",
        );
        assert_eq!(doc.top_levels[0].kind, SymbolKind::Type);
        assert!(doc.top_levels[0].detail.contains("整数 或 字符串"));
        assert!(doc.imports.contains_key("进程模块"));
    }
}
