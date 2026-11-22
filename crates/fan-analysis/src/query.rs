//! 编辑器查询：悬停（Hover）、定义跳转（Definition）、补全（Completion）。
//!
//! 对应 Go 版 `query.go`。对外结果的行列均为 LSP 约定的 0-based、UTF-16
//! 码元列；内部分析用 1-based rune 列。

use crate::document::Document;
use crate::index::Index;
use crate::pinyin::{codes_of, is_latin_prefix, matches_input, InputSchemes, PinyinOptions};
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionItem {
    pub label: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub doc: String,
    /// 客户端过滤文本：拼音补全时填入拉丁编码，避免输入字母时中文 label 被过滤。
    pub filter_text: Option<String>,
    /// 接受补全时替换的区间；拼音补全用于整体替换光标前的拉丁前缀。
    pub replace_range: Option<Range>,
}

/// 补全列表里的关键字（Go 版 CompletionKeywords 对应物）。
pub const COMPLETION_KEYWORDS: &[&str] = &[
    "函数",
    "变量",
    "常量",
    "返回",
    "如果",
    "那么",
    "否则如果",
    "否则",
    "结束",
    "当",
    "循环",
    "重复",
    "直到",
    "遍历",
    "中的",
    "跳出",
    "继续",
    "判断",
    "其他",
    "则",
    "时",
    "模型",
    "接口",
    "类型",
    "嵌入",
    "方法",
    "自己",
    "导入",
    "公开",
    "私有",
    "通过",
    "作为",
    "稍后",
    "检查",
    "崩溃了",
    "恢复",
    "是",
    "转为",
    "位与",
    "位或",
    "异或",
    "位取反",
    "左移",
    "右移",
    "整数",
    "小数",
    "字符串",
    "布尔",
    "数组",
    "字典",
    "字节",
    "字符",
    "真",
    "假",
    "空",
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
            return Some(HoverResult {
                content: format_hover(sym),
                range,
            });
        }
        if let Some(sym) = self.mixfix_symbol_at(line, &word.text) {
            return Some(HoverResult {
                content: format_hover(sym),
                range,
            });
        }
        if let Some((owner, member)) = self.member_at(doc, line0, word.start_col) {
            if let Some((_uri, sym)) = self.lookup_member(&owner, &member) {
                return Some(HoverResult {
                    content: format_hover(sym),
                    range,
                });
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
            return Some(Location {
                uri: doc.uri.clone(),
                range: name_range(doc, &sym.name, sym.pos),
            });
        }
        if let Some(sym) = self.mixfix_symbol_at(line, &word.text) {
            return Some(Location {
                uri: doc.uri.clone(),
                range: name_range(doc, &sym.name, sym.pos),
            });
        }
        if let Some((owner, member)) = self.member_at(doc, line0, word.start_col) {
            return self.member_definition(&owner, &member);
        }
        None
    }

    /// 补全查询（默认关闭拼音选项，保持历史行为）。
    pub fn completion(&self, line0: usize, char16: usize) -> Vec<CompletionItem> {
        self.completion_with_options(line0, char16, &PinyinOptions::default())
    }

    /// 带拼音/双拼配置的补全查询。
    ///
    /// 成员补全（`.`/`的`）与模型字段补全优先；其余情况下若光标前是拉丁前缀
    /// 且启用了拼音补全，则在关键字与项目符号上做全拼/首字母/双拼前缀匹配。
    pub fn completion_with_options(
        &self,
        line0: usize,
        char16: usize,
        options: &PinyinOptions,
    ) -> Vec<CompletionItem> {
        let doc = self.root();
        let line = doc.line_text(line0 + 1);
        if let Some(owner_expr) = completion_owner(line, char16) {
            let owner = self.resolve_owner_type(&owner_expr);
            return self.member_completions(&owner);
        }
        if let Some(model) = composite_field_completion_model(line, char16) {
            return self.model_field_completions(&model);
        }
        if options.enabled {
            if let Some((input, range)) = latin_prefix_at(line, line0, char16) {
                let pinyin_items = self.pinyin_completions(&input, range, options);
                if !pinyin_items.is_empty() {
                    return pinyin_items;
                }
            }
        }
        self.root_completions()
    }

    /// 在关键字、本文件符号、导入模块符号与外挂词条上做拼音前缀匹配。
    fn pinyin_completions(
        &self,
        input: &str,
        replace_range: Range,
        options: &PinyinOptions,
    ) -> Vec<CompletionItem> {
        // (label, kind, detail) 去重后的候选池。
        let mut pool: Vec<(String, SymbolKind, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |label: String, kind: SymbolKind, detail: String| {
            if seen.insert(label.clone()) {
                pool.push((label, kind, detail));
            }
        };

        for kw in COMPLETION_KEYWORDS {
            push((*kw).to_string(), KEYWORD_KIND, "关键字".to_string());
        }
        for sym in &self.root().top_levels {
            push(sym.name.clone(), sym.kind, sym.detail.clone());
        }
        for (name, imp) in &self.root().imports {
            push(name.clone(), SymbolKind::Module, imp.path.clone());
            if let Some(full) = self.resolve(&imp.path) {
                let mod_uri = crate::paths::path_to_uri(&full);
                if let Some(mod_doc) = self.all_docs().find(|d| d.uri == mod_uri) {
                    for sym in &mod_doc.top_levels {
                        push(sym.name.clone(), sym.kind, sym.detail.clone());
                    }
                }
            }
        }
        for word in &options.external_words {
            push(word.clone(), SymbolKind::Variable, "外挂词库".to_string());
        }

        // 仅含汉字（或汉字为主）的 label 才计算拼音；纯英文 label 交给普通补全。
        let mut matched: Vec<(CompletionItem, usize)> = Vec::new();
        for (label, kind, detail) in pool {
            if !label
                .chars()
                .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
            {
                continue;
            }
            let codes = codes_of(&label, options.double_scheme);
            if !matches_input(input, &codes, options.schemes) {
                continue;
            }
            let rank = rank_match(input, &codes, options.schemes);
            let filter_text = format!("{} {} {}", codes.full, codes.initial, codes.double);
            matched.push((
                CompletionItem {
                    label,
                    kind,
                    detail,
                    doc: String::new(),
                    filter_text: Some(filter_text),
                    replace_range: Some(replace_range),
                },
                rank,
            ));
        }

        // 排序：匹配精度（rank 小者优先）→ label 长度 → label 字典序，保证确定性。
        matched.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| a.0.label.chars().count().cmp(&b.0.label.chars().count()))
                .then_with(|| a.0.label.cmp(&b.0.label))
        });
        let limit = if options.max_suggest == 0 {
            matched.len()
        } else {
            options.max_suggest
        };
        matched
            .into_iter()
            .take(limit)
            .map(|(item, _)| item)
            .collect()
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
        Some(Location {
            uri,
            range: name_range(doc, &sym.name, sym.pos),
        })
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
        let connector_len = if runes[i as usize] == '.' {
            1isize
        } else if i >= 2
            && runes[(i - 2) as usize..=i as usize]
                .iter()
                .collect::<String>()
                == "的"
        {
            3
        } else {
            return None;
        };
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
                filter_text: None,
                replace_range: None,
            });
        }
        for (name, imp) in &self.root().imports {
            add(CompletionItem {
                label: name.clone(),
                kind: SymbolKind::Module,
                detail: imp.path.clone(),
                doc: String::new(),
                filter_text: None,
                replace_range: None,
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
                            filter_text: None,
                            replace_range: None,
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
                filter_text: None,
                replace_range: None,
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
                            filter_text: None,
                            replace_range: None,
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
                    filter_text: None,
                    replace_range: None,
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
                    filter_text: None,
                    replace_range: None,
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
                        filter_text: None,
                        replace_range: None,
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
                filter_text: None,
                replace_range: None,
            });
            if let Some(embed) = self.lookup_class_any_doc(embed_name) {
                for child in &embed.children {
                    add(CompletionItem {
                        label: child.name.clone(),
                        kind: child.kind,
                        detail: child.detail.clone(),
                        doc: child.doc.clone(),
                        filter_text: None,
                        replace_range: None,
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
                filter_text: None,
                replace_range: None,
            });
        }
        for embed in &class.embeds {
            if seen.insert(embed.clone()) {
                items.push(CompletionItem {
                    label: embed.clone(),
                    kind: SymbolKind::Class,
                    detail: "嵌入".to_string(),
                    doc: String::new(),
                    filter_text: None,
                    replace_range: None,
                });
            }
        }
        items
    }
}

fn word_range(line: &str, line0: usize, word: &WordMatch) -> Range {
    Range {
        start: Position {
            line: line0,
            character: utf16_column(line, word.start_col),
        },
        end: Position {
            line: line0,
            character: utf16_column(line, word.end_col),
        },
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
        start: Position {
            line: fallback.line - 1,
            character: utf16_column(line, col),
        },
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
    } else if i >= 2
        && runes[(i - 2) as usize..=i as usize]
            .iter()
            .collect::<String>()
            == "的"
    {
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

/// 取光标前连续的拉丁字母串及其 LSP 区间（0-based、UTF-16 列）。
///
/// 仅当紧邻光标左侧是一段纯 ASCII 字母（至少一个）时返回；遇到中文、数字、
/// 标点或空白即断开，避免把普通英文标识符之外的内容当作拼音前缀。
fn latin_prefix_at(line: &str, line0: usize, char16: usize) -> Option<(String, Range)> {
    let runes: Vec<char> = line.chars().collect();
    let cursor = utf16_to_rune_col(line, char16);
    if cursor == 0 || cursor > runes.len() {
        return None;
    }
    let end = cursor;
    let mut start = end;
    while start > 0 && runes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let text: String = runes[start..end].iter().collect();
    if !is_latin_prefix(&text) {
        return None;
    }
    // rune 列（0-based）转 UTF-16 列：起始 rune 之前的码元数。
    let start_char: usize = runes[..start].iter().map(|c| c.len_utf16()).sum();
    let end_char: usize = runes[..end].iter().map(|c| c.len_utf16()).sum();
    Some((
        text,
        Range {
            start: Position {
                line: line0,
                character: start_char,
            },
            end: Position {
                line: line0,
                character: end_char,
            },
        },
    ))
}

/// 计算匹配精度排名（越小越优先）：双拼精确 < 双拼前缀 < 全拼精确 <
/// 全拼前缀 < 首字母精确 < 首字母前缀；未参与的模式排到最后。
fn rank_match(input: &str, codes: &crate::pinyin::PinyinCodes, schemes: InputSchemes) -> usize {
    let lower = input.to_ascii_lowercase();
    let exact = |code: &str| code == lower;
    let prefix = |code: &str| code.starts_with(&lower);
    if schemes.double && exact(&codes.double) {
        0
    } else if schemes.double && prefix(&codes.double) {
        1
    } else if schemes.full && exact(&codes.full) {
        2
    } else if schemes.full && prefix(&codes.full) {
        3
    } else if schemes.initial && exact(&codes.initial) {
        4
    } else if schemes.initial && prefix(&codes.initial) {
        5
    } else {
        6
    }
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
            SymbolKind::Field
            | SymbolKind::Variable
            | SymbolKind::Constant
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
        let labels: Vec<String> = index
            .completion(0, 0)
            .into_iter()
            .map(|i| i.label)
            .collect();
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
        let model =
            composite_field_completion_model(line, line.chars().count()).expect("应识别模型");
        assert_eq!(model, "矩形");
    }

    #[test]
    fn 拼音首字母补全项目符号与关键字() {
        // 最后一行输入 dzh（打招呼 的首字母），光标在串尾；该行是 0-based 第 4 行。
        let src = format!("{SRC}dzh");
        let index = Index::build("file:///t.凡", &src);
        let line0 = src.lines().count() - 1;
        let items = index.completion_with_options(line0, 3, &PinyinOptions::sensible_default());
        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        assert!(labels.contains(&"打招呼".to_string()), "实际：{labels:?}");
        let hit = items
            .iter()
            .find(|i| i.label == "打招呼")
            .expect("应有候选项");
        assert!(hit.filter_text.is_some());
        assert!(hit.replace_range.is_some());
    }

    #[test]
    fn 拼音全拼补全关键字() {
        let src = "hanshu";
        let index = Index::build("file:///t.凡", src);
        let options = PinyinOptions::sensible_default();
        let items = index.completion_with_options(0, src.chars().count(), &options);
        assert!(
            items.iter().any(|i| i.label == "函数"),
            "全拼 hanshu 应命中 函数：{:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn 拼音替换区间覆盖整个拉丁前缀() {
        let line = "变量 x = dzh";
        let (text, range) = latin_prefix_at(line, 0, line.chars().count()).expect("应取到前缀");
        assert_eq!(text, "dzh");
        // “变量 x = ” 共 7 个字符（均 BMP，UTF-16 列等于字符列）。
        assert_eq!(range.start.character, 7);
        assert_eq!(range.end.character, 10);
    }

    #[test]
    fn 中文行中的拉丁前缀列按_utf16_计算() {
        // 前缀前含两个汉字（BMP，每字 1 个 UTF-16 码元）。
        let line = "打印 dzh";
        let (text, range) = latin_prefix_at(line, 0, line.chars().count()).unwrap();
        assert_eq!(text, "dzh");
        assert_eq!(range.start.character, 3);
        assert_eq!(range.end.character, 6);
    }

    #[test]
    fn 关闭拼音后回退普通补全() {
        let src = "dzh";
        let index = Index::build("file:///t.凡", src);
        let mut options = PinyinOptions::sensible_default();
        options.enabled = false;
        let items = index.completion_with_options(0, src.chars().count(), &options);
        // 普通根补全不含 filter_text，且仍返回完整符号集。
        assert!(items.iter().all(|i| i.filter_text.is_none()));
        assert!(items.iter().any(|i| i.label == "函数"));
    }

    #[test]
    fn 非拉丁前缀不触发拼音路径() {
        assert!(latin_prefix_at("打招呼", 0, 3).is_none());
        assert!(latin_prefix_at("a1", 0, 2).is_none());
    }

    #[test]
    fn 候选数受上限约束() {
        let src = "h";
        let index = Index::build("file:///t.凡", src);
        let mut options = PinyinOptions::sensible_default();
        options.max_suggest = 1;
        let items = index.completion_with_options(0, src.chars().count(), &options);
        assert!(items.len() <= 1, "实际数量：{}", items.len());
    }
}
