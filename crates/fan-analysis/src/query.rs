//! 编辑器查询：悬停（Hover）、定义跳转（Definition）、补全（Completion）。
//!
//! 对应 Go 版 `query.go`。对外结果的行列均为 LSP 约定的 0-based、UTF-16
//! 码元列；内部分析用 1-based rune 列。

use crate::document::Document;
use crate::index::Index;
use crate::symbol::{Symbol, SymbolKind};
use crate::utf16::{is_letter, utf16_column, utf16_to_rune_col, word_at, WordMatch};

/// 0-based、UTF-16 列的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverResult {
    pub content: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub doc: String,
}

/// 补全列表里的关键字（Go 版 CompletionKeywords 对应物）。
pub const COMPLETION_KEYWORDS: &[&str] = &[
    "函数", "变量", "常量", "返回", "如果", "那么", "否则如果", "否则", "结束",
    "当", "循环", "重复", "直到", "遍历", "中的", "跳出", "继续",
    "判断", "其他", "模型", "接口", "类型", "嵌入", "方法", "自己",
    "导入", "公开", "私有", "通过", "作为", "稍后", "抛出", "检查", "捕获",
    "是", "转为", "位与", "位或", "异或", "位取反", "左移", "右移",
    "整数", "小数", "字符串", "布尔", "数组", "字典", "字节", "字符", "真", "假", "空",
];

/// 关键字补全使用的占位种类。
const KEYWORD_KIND: SymbolKind = SymbolKind::Module;

impl Index {
    /// 悬停查询。
    pub fn hover(&self, line0: usize, char16: usize) -> Option<HoverResult> {
        let doc = self.root();
        let line = doc.line_text(line0 + 1);
        let word = word_at(line, char16);
        if word.text.is_empty() {
            return None;
        }
        let range = word_range(line, line0, &word);
        if let Some(sym) = self.lookup_root(&word.text) {
            return Some(HoverResult { content: format_hover(sym), range });
        }
        if let Some(sym) = self.mixfix_symbol_at(line, &word.text) {
            return Some(HoverResult { content: format_hover(sym), range });
        }
        if let Some((owner, member)) = self.member_at(doc, line0, word.start_col) {
            if let Some((_uri, sym)) = self.lookup_member(&owner, &member) {
                return Some(HoverResult { content: format_hover(sym), range });
            }
        }
        None
    }

    /// 定义跳转。
    pub fn definition(&self, line0: usize, char16: usize) -> Option<Location> {
        let doc = self.root();
        let line = doc.line_text(line0 + 1);
        let word = word_at(line, char16);
        if word.text.is_empty() {
            return None;
        }
        if let Some(sym) = self.lookup_root(&word.text) {
            return Some(Location { uri: doc.uri.clone(), range: name_range(doc, &sym.name, sym.pos) });
        }
        if let Some(sym) = self.mixfix_symbol_at(line, &word.text) {
            return Some(Location { uri: doc.uri.clone(), range: name_range(doc, &sym.name, sym.pos) });
        }
        if let Some((owner, member)) = self.member_at(doc, line0, word.start_col) {
            return self.member_definition(&owner, &member);
        }
        None
    }

    /// 补全查询。
    pub fn completion(&self, line0: usize, char16: usize) -> Vec<CompletionItem> {
        let doc = self.root();
        let line = doc.line_text(line0 + 1);
        if let Some(owner_expr) = completion_owner(line, char16) {
            let owner = self.resolve_owner_type(&owner_expr);
            return self.member_completions(&owner);
        }
        if let Some(model) = composite_field_completion_model(line, char16) {
            return self.model_field_completions(&model);
        }
        self.root_completions()
    }

    fn lookup_root(&self, name: &str) -> Option<&Symbol> {
        self.root().find_by_name(name).last().copied()
    }

    /// 查找光标行内分段函数的段名对应的分段函数符号。
    fn mixfix_symbol_at(&self, line: &str, segment_word: &str) -> Option<&Symbol> {
        if segment_word.is_empty() {
            return None;
        }
        let root = mixfix_root_on_line(line)?;
        self.root()
            .top_levels
            .iter()
            .filter(|s| s.kind == SymbolKind::MixfixFunction)
            .find(|s| s.name.starts_with(&root) && s.name.contains(segment_word))
    }

    fn lookup_member(&self, owner: &str, member: &str) -> Option<(String, &Symbol)> {
        if let Some(info) = self.root().imports.get(owner) {
            if let Some(full) = self.resolve(&info.path) {
                let mod_uri = crate::paths::path_to_uri(&full);
                let mod_doc = self.all_docs().find(|d| d.uri == mod_uri)?;
                if let Some(sym) = mod_doc.find_by_name(member).last().copied() {
                    return Some((mod_doc.uri.clone(), sym));
                }
            }
            return None;
        }
        if let Some(class) = self.root().class(owner) {
            if let Some(child) = class.children.iter().find(|c| c.name == member) {
                return Some((self.root().uri.clone(), child));
            }
        }
        for sym in self.root().find_by_name(member) {
            if sym.kind == SymbolKind::Method && sym.owner == owner {
                return Some((self.root().uri.clone(), sym));
            }
        }
        None
    }

    fn member_definition(&self, owner: &str, member: &str) -> Option<Location> {
        let (uri, sym) = self.lookup_member(owner, member)?;
        let doc = self.document(&uri)?;
        Some(Location { uri, range: name_range(doc, &sym.name, sym.pos) })
    }

    /// 判断光标词是否为 `对象.成员` / `对象 的 成员` 的右半部分。
    fn member_at(
        &self,
        doc: &Document,
        line0: usize,
        member_col1: usize,
    ) -> Option<(String, String)> {
        let line = doc.line_text(line0 + 1);
        let runes: Vec<char> = line.chars().collect();
        if member_col1 == 0 {
            return None;
        }
        let pos = member_col1 - 1;
        if pos > runes.len() {
            return None;
        }
        let mut i = pos as isize - 1;
        while i >= 0 && (runes[i as usize] == ' ' || runes[i as usize] == '\t') {
            i -= 1;
        }
        if i < 0 {
            return None;
        }
        let connector_len;
        if runes[i as usize] == '.' {
            connector_len = 1isize;
        } else if i >= 2
            && runes[(i - 2) as usize..=i as usize].iter().collect::<String>() == "的"
        {
            connector_len = 3;
        } else {
            return None;
        }
        i -= connector_len;
        while i >= 0 && (runes[i as usize] == ' ' || runes[i as usize] == '\t') {
            i -= 1;
        }
        let mut start = i + 1;
        while start > 0 && is_letter(runes[(start - 1) as usize]) {
            start -= 1;
        }
        if start > i {
            return None;
        }
        let owner: String = runes[start as usize..=i as usize].iter().collect();
        let word = word_at(line, utf16_column(line, member_col1));
        if word.text.is_empty() {
            return None;
        }
        Some((self.resolve_owner_type(&owner), word.text))
    }

    fn resolve_owner_type(&self, name: &str) -> String {
        if self.root().imports.contains_key(name) {
            return name.to_string();
        }
        if let Some(sym) = self.lookup_root(name) {
            if !sym.detail.is_empty() && self.lookup_class_any_doc(&sym.detail).is_some() {
                return sym.detail.clone();
            }
        }
        name.to_string()
    }

    fn root_completions(&self) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut add = |item: CompletionItem| {
            if seen.insert(item.label.clone()) {
                items.push(item);
            }
        };
        for sym in &self.root().top_levels {
            add(CompletionItem {
                label: sym.name.clone(),
                kind: sym.kind,
                detail: sym.detail.clone(),
                doc: sym.doc.clone(),
            });
        }
        for (name, imp) in &self.root().imports {
            add(CompletionItem {
                label: name.clone(),
                kind: SymbolKind::Module,
                detail: imp.path.clone(),
                doc: String::new(),
            });
            if let Some(full) = self.resolve(&imp.path) {
                let mod_uri = crate::paths::path_to_uri(&full);
                if let Some(mod_doc) = self.all_docs().find(|d| d.uri == mod_uri) {
                    for sym in &mod_doc.top_levels {
                        add(CompletionItem {
                            label: sym.name.clone(),
                            kind: sym.kind,
                            detail: sym.detail.clone(),
                            doc: sym.doc.clone(),
                        });
                    }
                }
            }
        }
        for kw in COMPLETION_KEYWORDS {
            add(CompletionItem {
                label: kw.to_string(),
                kind: KEYWORD_KIND,
                detail: "关键字".to_string(),
                doc: String::new(),
            });
        }
        items
    }

    fn member_completions(&self, owner: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut add = |item: CompletionItem| {
            if seen.insert(item.label.clone()) {
                items.push(item);
            }
        };
        if let Some(info) = self.root().imports.get(owner) {
            if let Some(full) = self.resolve(&info.path) {
                let mod_uri = crate::paths::path_to_uri(&full);
                if let Some(mod_doc) = self.all_docs().find(|d| d.uri == mod_uri) {
                    for sym in &mod_doc.top_levels {
                        add(CompletionItem {
                            label: sym.name.clone(),
                            kind: sym.kind,
                            detail: sym.detail.clone(),
                            doc: sym.doc.clone(),
                        });
                    }
                }
            }
            return items;
        }
        if let Some(class) = self.lookup_class_any_doc(owner) {
            for child in &class.children {
                add(CompletionItem {
                    label: child.name.clone(),
                    kind: child.kind,
                    detail: child.detail.clone(),
                    doc: child.doc.clone(),
                });
            }
            self.add_embedded_members(class, &mut add, &mut std::collections::HashSet::new());
        }
        if let Some(&idx) = self.root().interfaces.get(owner) {
            let iface = &self.root().top_levels[idx];
            for child in &iface.children {
                add(CompletionItem {
                    label: child.name.clone(),
                    kind: child.kind,
                    detail: child.detail.clone(),
                    doc: child.doc.clone(),
                });
            }
        }
        for doc in self.all_docs() {
            for sym in &doc.top_levels {
                if sym.kind == SymbolKind::Method && sym.owner == owner {
                    add(CompletionItem {
                        label: sym.name.clone(),
                        kind: sym.kind,
                        detail: sym.detail.clone(),
                        doc: sym.doc.clone(),
                    });
                }
            }
        }
        items
    }

    fn add_embedded_members(
        &self,
        class: &Symbol,
        add: &mut impl FnMut(CompletionItem),
        visited: &mut std::collections::HashSet<String>,
    ) {
        for embed_name in &class.embeds {
            if !visited.insert(embed_name.clone()) {
                continue;
            }
            add(CompletionItem {
                label: embed_name.clone(),
                kind: SymbolKind::Class,
                detail: "嵌入".to_string(),
                doc: String::new(),
            });
            if let Some(embed) = self.lookup_class_any_doc(embed_name) {
                for child in &embed.children {
                    add(CompletionItem {
                        label: child.name.clone(),
                        kind: child.kind,
                        detail: child.detail.clone(),
                        doc: child.doc.clone(),
                    });
                }
                self.add_embedded_members(embed, add, visited);
            }
        }
    }

    fn model_field_completions(&self, model: &str) -> Vec<CompletionItem> {
        let Some(class) = self.lookup_class_any_doc(model) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for child in &class.children {
            if child.kind != SymbolKind::Field || !seen.insert(child.name.clone()) {
                continue;
            }
            items.push(CompletionItem {
                label: child.name.clone(),
                kind: SymbolKind::Field,
                detail: child.detail.clone(),
                doc: String::new(),
            });
        }
        for embed in &class.embeds {
            if seen.insert(embed.clone()) {
                items.push(CompletionItem {
                    label: embed.clone(),
                    kind: SymbolKind::Class,
                    detail: "嵌入".to_string(),
                    doc: String::new(),
                });
            }
        }
        items
    }
}

fn word_range(line: &str, line0: usize, word: &WordMatch) -> Range {
    Range {
        start: Position { line: line0, character: utf16_column(line, word.start_col) },
        end: Position { line: line0, character: utf16_column(line, word.end_col) },
    }
}

/// 从声明起始位置定位名字的 LSP 区间，找不到名字时回退到声明列。
fn name_range(doc: &Document, name: &str, fallback: fan_ast::Position) -> Range {
    let line = doc.line_text(fallback.line);
    let col = line
        .find(name)
        .map(|byte_idx| line[..byte_idx].chars().count() + 1)
        .unwrap_or(fallback.column);
    Range {
        start: Position { line: fallback.line - 1, character: utf16_column(line, col) },
        end: Position {
            line: fallback.line - 1,
            character: utf16_column(line, col + name.chars().count()),
        },
    }
}

/// 从一行文本找出分段函数调用的首段名（第一个 `(` 前的标识符）。
fn mixfix_root_on_line(line: &str) -> Option<String> {
    let runes: Vec<char> = line.chars().collect();
    for i in 0..runes.len() {
        if runes[i] != '(' && runes[i] != '（' {
            continue;
        }
        let mut start = i as isize - 1;
        while start >= 0 && is_letter(runes[start as usize]) {
            start -= 1;
        }
        start += 1;
        if start < i as isize {
            return Some(runes[start as usize..i].iter().collect());
        }
    }
    None
}

/// 光标前是否为 `.` 或 `的`，若是返回左表达式名字。
fn completion_owner(line: &str, char16: usize) -> Option<String> {
    let runes: Vec<char> = line.chars().collect();
    let mut i = utf16_to_rune_col(line, char16) as isize - 1;
    while i >= 0 && (runes[i as usize] == ' ' || runes[i as usize] == '\t') {
        i -= 1;
    }
    if i < 0 {
        return None;
    }
    if runes[i as usize] == '.' {
        // 继续向左取标识符
    } else if i >= 2 && runes[(i - 2) as usize..=i as usize].iter().collect::<String>() == "的" {
        i -= 2;
    } else {
        return None;
    }
    let mut j = i - 1;
    while j >= 0 && (runes[j as usize] == ' ' || runes[j as usize] == '\t') {
        j -= 1;
    }
    let mut start = j + 1;
    while start > 0 && is_letter(runes[(start - 1) as usize]) {
        start -= 1;
    }
    if start > j {
        return None;
    }
    Some(runes[start as usize..=j as usize].iter().collect())
}

/// 判断光标是否位于 `模型{…}` 复合字面量的字段名位置，返回模型名。
fn composite_field_completion_model(line: &str, char16: usize) -> Option<String> {
    let runes: Vec<char> = line.chars().collect();
    let col = utf16_to_rune_col(line, char16);
    if col == 0 || col > runes.len() {
        return None;
    }
    let mut depth = 0usize;
    for i in (0..col).rev() {
        match runes[i] {
            '}' => depth += 1,
            '{' => {
                if depth > 0 {
                    depth -= 1;
                    continue;
                }
                let mut j = i as isize - 1;
                while j >= 0 && is_letter(runes[j as usize]) {
                    j -= 1;
                }
                if j + 1 >= i as isize {
                    return None;
                }
                let model: String = runes[(j + 1) as usize..i].iter().collect();
                let segment: String = runes[i + 1..col].iter().collect();
                if segment.contains(':') {
                    return None;
                }
                return Some(model);
            }
            ':' | ')' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

fn format_hover(sym: &Symbol) -> String {
    let mut out = String::new();
    if !sym.detail.is_empty() {
        let head = match sym.kind {
            SymbolKind::Field | SymbolKind::Variable | SymbolKind::Constant
            | SymbolKind::Parameter => format!("{}: {}", sym.name, sym.detail),
            _ => sym.detail.clone(),
        };
        out.push_str("```fan\n");
        out.push_str(&head);
        out.push_str("\n```");
    }
    if !sym.doc.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&sym.doc);
    }
    if out.is_empty() {
        out.push_str(&sym.name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
# 打招呼函数
函数 打招呼(字符串 名字) -> 字符串
    返回 名字
结束
模型 矩形
    属性 整数 宽
    属性 整数 高
结束
";

    #[test]
    fn 悬停顶层函数() {
        let index = Index::build("file:///t.凡", SRC);
        let hover = index.hover(1, 4).expect("应悬停到函数");
        assert!(hover.content.contains("打招呼"), "实际：{}", hover.content);
        assert!(hover.content.contains("打招呼函数"));
    }

    #[test]
    fn 悬停本地函数显示绑定目标() {
        let src = "定义 本地(\"json.jsonEncode\") 函数 生成(任意 值)\n    返回 空\n结束\n";
        let index = Index::build("file:///t.凡", src);
        let hover = index.hover(0, 28).expect("应悬停到函数");
        assert!(
            hover.content.contains("本地绑定 json.jsonEncode"),
            "实际：{}",
            hover.content
        );
    }

    #[test]
    fn 定义跳转命中顶层符号() {
        let index = Index::build("file:///t.凡", SRC);
        let loc = index.definition(1, 4).expect("应找到定义");
        assert_eq!(loc.uri, "file:///t.凡");
        assert_eq!(loc.range.start.line, 1);
    }

    #[test]
    fn 根补全包含符号与关键字() {
        let index = Index::build("file:///t.凡", SRC);
        let labels: Vec<String> = index.completion(0, 0).into_iter().map(|i| i.label).collect();
        assert!(labels.contains(&"打招呼".to_string()));
        assert!(labels.contains(&"矩形".to_string()));
        assert!(labels.contains(&"函数".to_string()));
    }

    #[test]
    fn 模型成员补全返回字段() {
        let index = Index::build("file:///t.凡", SRC);
        let line = "变量 r = 矩形()\n变量 w = r.";
        let _ = line;
        let items = index.member_completions("矩形");
        let labels: Vec<String> = items.into_iter().map(|i| i.label).collect();
        assert!(labels.contains(&"宽".to_string()), "实际：{labels:?}");
        assert!(labels.contains(&"高".to_string()));
    }

    #[test]
    fn 复合字面量字段补全识别模型() {
        let line = "变量 r = 矩形{宽";
        let model = composite_field_completion_model(line, line.chars().count()).expect("应识别模型");
        assert_eq!(model, "矩形");
    }
}
