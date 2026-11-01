//! 正则组默认实现：纯 Rust 标准库的回溯匹配引擎。
//!
//! 这是 `正则` 组在**没有**扩展动态库时的回落实现（`编码`、`网络` 两组同理，它支持日常
//! 所需的语法子集：字面量、`.`、字符类 `[a-z]`、`\d`/`\w`/`\s` 及其大写形式、
//! 分组与捕获、交替 `|`、锚点 `^`/`$`、重复 `*`/`+`/`?`/`{n,m}`。
//! 不支持回溯引用、前后断言、非贪婪量词等高级特性——需要这些能力时提供
//! `fan_ext_regex` 扩展库即可自动切换过去。
//!
//! 参数顺序沿用凡语言标准库约定：**模式在前，文本在后**。

use fan_object::{ErrorValue, NativeSpec, Object};
use std::rc::Rc;

// ---------------------------------------------------------------- 语法树

/// 字符类中的一项。
#[derive(Debug, Clone)]
pub enum ClassItem {
    /// 单个字符。
    Char(char),
    /// 闭区间字符范围。
    Range(char, char),
    /// 预定义类：`\d`、`\w`、`\s`（`negated` 表示大写形式）。
    Named(NamedClass, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedClass {
    Digit,
    Word,
    Space,
}

impl NamedClass {
    fn contains(self, c: char) -> bool {
        match self {
            NamedClass::Digit => c.is_ascii_digit(),
            NamedClass::Word => c.is_alphanumeric() || c == '_',
            NamedClass::Space => matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C'),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Char(char),
    /// `.`：任意字符（不含换行）。
    Any,
    /// `[...]`：字符类，`negated` 表示 `[^...]`。
    Class(Vec<ClassItem>, bool),
    /// 捕获分组，编号为分组序号。
    Group(usize, Vec<Vec<Node>>),
    /// 重复：最少 `min` 次，最多 `max`（None 表示不限）。
    Repeat(Box<Node>, usize, Option<usize>),
    /// 交替：各分支是节点序列。
    Alt(Vec<Vec<Node>>),
    AnchorStart,
    AnchorEnd,
}

// ---------------------------------------------------------------- 解析

/// 正则解析器。
struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// 已分配的捕获组数量。
    group_count: usize,
}

type ParseResult<T> = Result<T, String>;

impl Parser {
    fn new(pattern: &str) -> Self {
        Parser {
            chars: pattern.chars().collect(),
            pos: 0,
            group_count: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// 解析整个模式。
    fn parse(&mut self) -> ParseResult<Vec<Node>> {
        let seq = self.parse_alt()?;
        if self.pos < self.chars.len() {
            return Err(format!("模式第 {} 个字符处存在多余内容", self.pos));
        }
        Ok(seq)
    }

    fn parse_alt(&mut self) -> ParseResult<Vec<Node>> {
        let mut branches = vec![self.parse_seq()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.parse_seq()?);
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap())
        } else {
            Ok(vec![Node::Alt(branches)])
        }
    }

    fn parse_seq(&mut self) -> ParseResult<Vec<Node>> {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                '|' => break,
                ')' => break,
                '^' => {
                    self.pos += 1;
                    nodes.push(Node::AnchorStart);
                }
                '$' => {
                    self.pos += 1;
                    nodes.push(Node::AnchorEnd);
                }
                // 分组与字符类同样可以后接量词，如 `[a-z]+`、`(ab)*`
                '(' => {
                    let group = self.parse_group()?;
                    nodes.push(self.parse_quantifier(group)?);
                }
                '[' => {
                    let class = self.parse_class()?;
                    nodes.push(self.parse_quantifier(class)?);
                }
                '*' | '+' | '?' => {
                    return Err(format!("第 {} 个字符处的量词缺少作用对象", self.pos))
                }
                _ => {
                    let atom = self.parse_atom()?;
                    nodes.push(self.parse_quantifier(atom)?);
                }
            }
        }
        Ok(nodes)
    }

    fn parse_group(&mut self) -> ParseResult<Node> {
        self.pos += 1; // '('
        self.group_count += 1;
        let index = self.group_count;
        let body = self.parse_alt()?;
        if self.next() != Some(')') {
            return Err("分组缺少右括号".to_string());
        }
        // 分组内部可能含交替，统一包成 Alt
        let branches = match body.as_slice() {
            [Node::Alt(_)] => vec![body],
            _ => vec![body],
        };
        Ok(Node::Group(index, branches))
    }

    fn parse_class(&mut self) -> ParseResult<Node> {
        self.pos += 1; // '['
        let mut negated = false;
        if self.peek() == Some('^') {
            negated = true;
            self.pos += 1;
        }
        let mut items = Vec::new();
        loop {
            match self.next() {
                None => return Err("字符类未闭合".to_string()),
                Some(']') => break,
                Some('\\') => {
                    let (named, neg) = self.parse_named_class()?;
                    items.push(ClassItem::Named(named, neg));
                }
                Some(c) => {
                    // 形如 a-z 的范围
                    if self.peek() == Some('-') && self.chars.get(self.pos + 1) != Some(&']') {
                        self.pos += 1;
                        let to = self.next().ok_or("字符范围不完整")?;
                        items.push(ClassItem::Range(c, to));
                    } else {
                        items.push(ClassItem::Char(c));
                    }
                }
            }
        }
        Ok(Node::Class(items, negated))
    }

    /// 解析 `\d`、`\w`、`\s` 及其大写形式；其余转义按字面字符处理。
    fn parse_named_class(&mut self) -> ParseResult<(NamedClass, bool)> {
        match self.next() {
            Some('d') => Ok((NamedClass::Digit, false)),
            Some('D') => Ok((NamedClass::Digit, true)),
            Some('w') => Ok((NamedClass::Word, false)),
            Some('W') => Ok((NamedClass::Word, true)),
            Some('s') => Ok((NamedClass::Space, false)),
            Some('S') => Ok((NamedClass::Space, true)),
            Some(other) => Err(format!("不支持的转义 \\{}", other)),
            None => Err("转义序列不完整".to_string()),
        }
    }

    fn parse_atom(&mut self) -> ParseResult<Node> {
        match self.next() {
            None => Err("模式意外结束".to_string()),
            Some('.') => Ok(Node::Any),
            Some('\\') => {
                // 既可能是预定义类，也可能只是转义的字面字符
                match self.peek() {
                    Some(c @ ('d' | 'D' | 'w' | 'W' | 's' | 'S')) => {
                        let _ = c;
                        let (named, neg) = self.parse_named_class()?;
                        Ok(Node::Class(vec![ClassItem::Named(named, neg)], false))
                    }
                    Some(c) => {
                        self.pos += 1;
                        Ok(Node::Char(c))
                    }
                    None => Err("转义序列不完整".to_string()),
                }
            }
            Some(c) => Ok(Node::Char(c)),
        }
    }

    fn parse_quantifier(&mut self, atom: Node) -> ParseResult<Node> {
        match self.peek() {
            Some('*') => {
                self.pos += 1;
                Ok(Node::Repeat(Box::new(atom), 0, None))
            }
            Some('+') => {
                self.pos += 1;
                Ok(Node::Repeat(Box::new(atom), 1, None))
            }
            Some('?') => {
                self.pos += 1;
                Ok(Node::Repeat(Box::new(atom), 0, Some(1)))
            }
            Some('{') => {
                self.pos += 1;
                let min = self.parse_number()?;
                let max = if self.peek() == Some(',') {
                    self.pos += 1;
                    if self.peek() == Some('}') {
                        None
                    } else {
                        Some(self.parse_number()?)
                    }
                } else {
                    Some(min)
                };
                if self.next() != Some('}') {
                    return Err("量词缺少右花括号".to_string());
                }
                if let Some(m) = max {
                    if m < min {
                        return Err("量词上界小于下界".to_string());
                    }
                }
                Ok(Node::Repeat(Box::new(atom), min, max))
            }
            _ => Ok(atom),
        }
    }

    fn parse_number(&mut self) -> ParseResult<usize> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err("量词缺少数字".to_string());
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse::<usize>()
            .map_err(|_| "量词数字过大".to_string())
    }
}

/// 编译模式，返回节点序列与捕获组数量。
pub fn compile(pattern: &str) -> ParseResult<(Vec<Node>, usize)> {
    let mut parser = Parser::new(pattern);
    let nodes = parser.parse()?;
    Ok((nodes, parser.group_count))
}

// ---------------------------------------------------------------- 匹配

/// 一次匹配的结果：起止位置与捕获组（下标 0 为整体匹配）。
#[derive(Debug, Clone)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
}

/// 在文本中查找首个匹配。
pub fn find(pattern: &str, text: &str) -> ParseResult<Option<Match>> {
    let (nodes, group_count) = compile(pattern)?;
    let chars: Vec<char> = text.chars().collect();
    for start in 0..=chars.len() {
        let mut groups = vec![None; group_count + 1];
        if let Some(end) = match_seq(&nodes, &chars, start, &mut groups) {
            groups[0] = Some((start, end));
            return Ok(Some(Match { start, end, groups }));
        }
    }
    Ok(None)
}

/// 匹配节点序列，成功返回结束位置。捕获组就地写入 `groups`。
fn match_seq(
    nodes: &[Node],
    chars: &[char],
    pos: usize,
    groups: &mut Vec<Option<(usize, usize)>>,
) -> Option<usize> {
    // 空序列即匹配成功，这是递归的终止条件
    let Some((first, rest)) = nodes.split_first() else {
        return Some(pos);
    };
    match first {
        Node::AnchorStart => {
            if pos != 0 {
                return None;
            }
            match_seq(rest, chars, pos, groups)
        }
        Node::AnchorEnd => {
            if pos != chars.len() {
                return None;
            }
            match_seq(rest, chars, pos, groups)
        }
        Node::Char(c) => {
            if chars.get(pos) == Some(c) {
                match_seq(rest, chars, pos + 1, groups)
            } else {
                None
            }
        }
        Node::Any => {
            if pos < chars.len() && chars[pos] != '\n' {
                match_seq(rest, chars, pos + 1, groups)
            } else {
                None
            }
        }
        Node::Class(items, negated) => {
            let c = chars.get(pos)?;
            let hit = items.iter().any(|item| match item {
                ClassItem::Char(ch) => *ch == *c,
                ClassItem::Range(from, to) => *c >= *from && *c <= *to,
                ClassItem::Named(named, neg) => named.contains(*c) != *neg,
            });
            if hit != *negated && pos < chars.len() {
                match_seq(rest, chars, pos + 1, groups)
            } else {
                None
            }
        }
        Node::Group(index, branches) => {
            for branch in branches {
                // 先尝试内层匹配，再继续后续节点
                if let Some(end) = match_seq(branch, chars, pos, groups) {
                    let saved = groups[*index];
                    groups[*index] = Some((pos, end));
                    if let Some(final_end) = match_seq(rest, chars, end, groups) {
                        return Some(final_end);
                    }
                    groups[*index] = saved;
                }
            }
            None
        }
        Node::Repeat(inner, min, max) => {
            let limit = max.unwrap_or(chars.len() - pos);
            // 贪心：从最大次数向下回溯
            for count in (*min..=limit).rev() {
                if let Some(end) = match_repeat(inner, chars, pos, count, groups) {
                    if let Some(final_end) = match_seq(rest, chars, end, groups) {
                        return Some(final_end);
                    }
                }
            }
            None
        }
        Node::Alt(branches) => {
            for branch in branches {
                if let Some(end) = match_seq(branch, chars, pos, groups) {
                    if let Some(final_end) = match_seq(rest, chars, end, groups) {
                        return Some(final_end);
                    }
                }
            }
            None
        }
    }
}

/// 重复匹配 `count` 次。
fn match_repeat(
    inner: &Node,
    chars: &[char],
    pos: usize,
    count: usize,
    groups: &mut Vec<Option<(usize, usize)>>,
) -> Option<usize> {
    let mut current = pos;
    for _ in 0..count {
        current = match_seq(std::slice::from_ref(inner), chars, current, groups)?;
    }
    Some(current)
}

/// 取出捕获组文本；未参与匹配时为空文本。
fn group_texts(m: &Match, text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    m.groups
        .iter()
        .map(|g| match g {
            Some((s, e)) => chars[*s..*e].iter().collect(),
            None => String::new(),
        })
        .collect()
}

/// 找出全部匹配（跳过空匹配，避免死循环）。
pub fn find_all(pattern: &str, text: &str) -> ParseResult<Vec<Match>> {
    let (nodes, group_count) = compile(pattern)?;
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut from = 0;
    while from <= chars.len() {
        let mut found = None;
        for start in from..=chars.len() {
            let mut groups = vec![None; group_count + 1];
            if let Some(end) = match_seq(&nodes, &chars, start, &mut groups) {
                if end == start {
                    continue; // 空匹配：继续找下一个起点
                }
                groups[0] = Some((start, end));
                found = Some(Match { start, end, groups });
                break;
            }
        }
        match found {
            Some(m) => {
                from = if m.end > m.start { m.end } else { m.start + 1 };
                out.push(m);
            }
            None => break,
        }
    }
    Ok(out)
}

/// 按替换模板展开；`$N` 或 `${N}` 引用捕获组。
fn expand(template: &str, groups: &[String]) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let (start, end) = if chars[i + 1] == '{' {
                let close = (i + 2..chars.len()).find(|&k| chars[k] == '}');
                match close {
                    Some(k) => (i + 2, k),
                    None => {
                        out.push(chars[i]);
                        i += 1;
                        continue;
                    }
                }
            } else {
                let mut k = i + 1;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    k += 1;
                }
                (i + 1, k)
            };
            if start < end {
                if let Ok(index) = chars[start..end]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                {
                    if index < groups.len() {
                        out.push_str(&groups[index]);
                        i = if chars.get(end) == Some(&'}') {
                            end + 1
                        } else {
                            end
                        };
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 把模式中的元字符转义，使其按字面量匹配。
pub fn quote(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- 内建

/// 统一的错误返回：把编译/匹配错误转成 `(空值, 错误)` 形式。
fn with_error<T: Into<Object>>(result: ParseResult<T>, empty: Object) -> Vec<Object> {
    match result {
        Ok(v) => vec![v.into(), Object::Nil],
        Err(e) => vec![
            empty,
            Object::Error(Rc::new(std::cell::RefCell::new(ErrorValue::new(e)))),
        ],
    }
}

/// 数量限制参数：`-1`（或缺失）表示不限，`0` 表示不取。
///
/// `正则.凡` 的 `替换(…, 数量 = 1)` 与 `全部替换(…)` 分别传 1 与 -1，
/// `查找全部`/`拆分` 同理。
fn count_of(args: &[Object], index: usize) -> usize {
    match args.get(index).and_then(|v| v.as_int()) {
        Some(n) if n < 0 => usize::MAX,
        Some(n) => n as usize,
        None => usize::MAX,
    }
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "regexMatch",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                match find(&pattern, &text) {
                    Ok(Some(_)) => Ok(with_error(Ok(Object::Bool(true)), Object::Bool(false))),
                    Ok(None) => Ok(vec![Object::Bool(false), Object::Nil]),
                    Err(e) => Ok(vec![
                        Object::Bool(false),
                        Object::Error(Rc::new(std::cell::RefCell::new(ErrorValue::new(e)))),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "regexFind",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                Ok(with_error(
                    find(&pattern, &text).map(|m| match m {
                        Some(m) => Object::string(
                            text.chars()
                                .skip(m.start)
                                .take(m.end - m.start)
                                .collect::<String>(),
                        ),
                        None => Object::string(String::new()),
                    }),
                    Object::string(String::new()),
                ))
            }),
        ),
        NativeSpec::core(
            "regexFindAll",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                let limit = count_of(&args, 2);
                Ok(with_error(
                    find_all(&pattern, &text).map(|ms| {
                        Object::array(
                            ms.iter()
                                .take(limit)
                                .map(|m| {
                                    Object::string(
                                        text.chars()
                                            .skip(m.start)
                                            .take(m.end - m.start)
                                            .collect::<String>(),
                                    )
                                })
                                .collect(),
                        )
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::core(
            "regexFindGroups",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                Ok(with_error(
                    find(&pattern, &text).map(|m| match m {
                        Some(m) => Object::array(
                            group_texts(&m, &text)
                                .into_iter()
                                .map(Object::string)
                                .collect(),
                        ),
                        None => Object::array(vec![]),
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::core(
            "regexFindAllGroups",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                let limit = count_of(&args, 2);
                Ok(with_error(
                    find_all(&pattern, &text).map(|ms| {
                        Object::array(
                            ms.iter()
                                .take(limit)
                                .map(|m| {
                                    Object::array(
                                        group_texts(m, &text)
                                            .into_iter()
                                            .map(Object::string)
                                            .collect(),
                                    )
                                })
                                .collect(),
                        )
                    }),
                    Object::array(vec![]),
                ))
            }),
        ),
        NativeSpec::core(
            "regexReplace",
            Some(3),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                let template = args[2].as_str().unwrap_or_default();
                let limit = count_of(&args, 3);
                let result = find_all(&pattern, &text).map(|ms| {
                    let mut out = String::new();
                    let mut last = 0;
                    for m in ms.iter().take(limit) {
                        out.push_str(
                            &text
                                .chars()
                                .skip(last)
                                .take(m.start - last)
                                .collect::<String>(),
                        );
                        out.push_str(&expand(&template, &group_texts(m, &text)));
                        last = m.end;
                    }
                    out.push_str(&text.chars().skip(last).collect::<String>());
                    Object::string(out)
                });
                Ok(with_error(result, Object::string(String::new())))
            }),
        ),
        NativeSpec::core(
            "regexSplit",
            Some(2),
            Rc::new(|_rt, args| {
                let pattern = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                let limit = count_of(&args, 2);
                let result = find_all(&pattern, &text).map(|ms| {
                    let mut parts = Vec::new();
                    if limit == 0 {
                        return Object::array(Vec::new());
                    }
                    let mut last = 0;
                    // 第 limit 段是剩余全部，故只按 limit-1 处切分
                    for m in ms.iter().take(limit - 1) {
                        parts.push(Object::string(
                            text.chars()
                                .skip(last)
                                .take(m.start - last)
                                .collect::<String>(),
                        ));
                        last = m.end;
                    }
                    parts.push(Object::string(text.chars().skip(last).collect::<String>()));
                    Object::array(parts)
                });
                Ok(with_error(result, Object::array(vec![])))
            }),
        ),
        NativeSpec::core(
            "regexQuote",
            Some(1),
            Rc::new(|_rt, args| {
                let text = args[0].as_str().unwrap_or_default();
                Ok(vec![Object::string(quote(&text))])
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::Runtime;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    /// 取首个返回值的显示文本。
    fn first(name: &str, args: Vec<Object>) -> String {
        call(name, args).unwrap()[0].inspect()
    }

    #[test]
    fn 匹配数字与锚点() {
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("\\d+"), Object::string("a123b")]
            ),
            "真"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("^\\d+$"), Object::string("a123b")]
            ),
            "假"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("^\\d+$"), Object::string("123")]
            ),
            "真"
        );
    }

    #[test]
    fn 查找首个与全部() {
        assert_eq!(
            first(
                "regexFind",
                vec![Object::string("\\d+"), Object::string("a12b34")]
            ),
            "12"
        );
        assert_eq!(
            first(
                "regexFindAll",
                vec![Object::string("\\d+"), Object::string("a12b34")]
            ),
            "[12, 34]"
        );
    }

    #[test]
    fn 分组捕获() {
        let out = call(
            "regexFindGroups",
            vec![Object::string("([a-z]+)(\\d+)"), Object::string("abc123")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "[abc123, abc, 123]");
    }

    #[test]
    fn 全部分组捕获() {
        let out = call(
            "regexFindAllGroups",
            vec![Object::string("(\\d+)"), Object::string("1和22")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "[[1, 1], [22, 22]]");
    }

    #[test]
    fn 无匹配时分组为空数组() {
        let out = call(
            "regexFindGroups",
            vec![Object::string("\\d+"), Object::string("abc")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "[]");
        assert_eq!(out[1], Object::Nil);
    }

    #[test]
    fn 替换支持分组引用() {
        let out = first(
            "regexReplace",
            vec![
                Object::string("(\\w+)"),
                Object::string("a b"),
                Object::string("[$1]"),
            ],
        );
        assert_eq!(out, "[a] [b]");
    }

    #[test]
    fn 拆分() {
        assert_eq!(
            first(
                "regexSplit",
                vec![Object::string("[,;]"), Object::string("a,b;c")]
            ),
            "[a, b, c]"
        );
    }

    #[test]
    fn 转义元字符() {
        let out = first("regexQuote", vec![Object::string("a.b?")]);
        assert_eq!(out, "a\\.b\\?");
        // 转义后的模式应能按字面量匹配原文本
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string(&out), Object::string("a.b?")]
            ),
            "真"
        );
    }

    #[test]
    fn 字符类与任意字符() {
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("[a-z]+"), Object::string("abc")]
            ),
            "真"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("[a-z]+"), Object::string("ABC")]
            ),
            "假"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("a.c"), Object::string("abc")]
            ),
            "真"
        );
    }

    #[test]
    fn 重复量词() {
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("^a{2,3}$"), Object::string("aa")]
            ),
            "真"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("^a{2,3}$"), Object::string("aaaa")]
            ),
            "假"
        );
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("^ab*c$"), Object::string("abbbc")]
            ),
            "真"
        );
    }

    #[test]
    fn 交替() {
        assert_eq!(
            first(
                "regexMatch",
                vec![Object::string("猫|狗"), Object::string("狗")]
            ),
            "真"
        );
        assert_eq!(
            first(
                "regexFind",
                vec![Object::string("猫|狗"), Object::string("养了狗")]
            ),
            "狗"
        );
    }

    #[test]
    fn 非法模式返回错误而非中断() {
        let out = call(
            "regexFind",
            vec![Object::string("("), Object::string("abc")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "");
        assert!(
            out[1].inspect().contains("分组缺少右括号"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 悬空量词报错() {
        let out = call(
            "regexMatch",
            vec![Object::string("*a"), Object::string("a")],
        )
        .unwrap();
        assert!(
            out[1].inspect().contains("量词缺少作用对象"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 正则组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "regexMatch",
            "regexFind",
            "regexFindAll",
            "regexFindGroups",
            "regexFindAllGroups",
            "regexReplace",
            "regexSplit",
            "regexQuote",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
