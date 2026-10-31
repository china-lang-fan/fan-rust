//! 词法分析器。
//!
//! 逐字符扫描 UTF-8 源码，产出记号流。行为与 Go 版 `internal/lexer` 保持一致：
//! 非法输入不抛异常，而是产出 `TokenType::Illegal` 记号，由解析器统一报错。

use crate::token::{lookup_keyword, Token, TokenType};
use std::collections::VecDeque;

/// 词法分析器。
pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    prev_tok: TokenType,
    space_before: bool,
    /// 插值字符串会一次产出多个记号，先入队后逐个吐出。
    queue: VecDeque<Token>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            src: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            prev_tok: TokenType::Newline,
            space_before: false,
            queue: VecDeque::new(),
        }
    }

    /// 切出全部记号，直到文件末尾（含 EOF 记号）。
    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut out = Vec::new();
        loop {
            let t = self.next_token();
            let is_eof = t.type_ == TokenType::Eof;
            out.push(t);
            if is_eof {
                break;
            }
        }
        out
    }

    /// 切出下一个记号。
    pub fn next_token(&mut self) -> Token {
        if let Some(tok) = self.queue.pop_front() {
            if tok.type_ != TokenType::Eof {
                self.prev_tok = tok.type_;
            }
            return tok;
        }
        self.space_before = self.skip_spaces();
        if self.pos >= self.src.len() {
            return self.emit(TokenType::Eof, "");
        }

        let r = self.src[self.pos];

        if r == '\n' {
            let tok = self.emit(TokenType::Newline, "\n");
            self.advance();
            self.line += 1;
            self.col = 1;
            return tok;
        }
        if r == '#' {
            self.consume_line_comment();
            return self.next_token();
        }
        if r == '"' {
            return self.read_string();
        }
        if r == '\'' || r == '‘' {
            return self.read_char();
        }
        if is_digit(r) {
            return self.read_number();
        }
        if is_ident_start(r) {
            return self.read_identifier_or_keyword();
        }
        self.read_operator()
    }

    // ------------------------------------------------------------ 基础设施

    fn advance(&mut self) {
        if self.pos < self.src.len() {
            self.pos += 1;
            self.col += 1;
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.src.get(self.pos + offset).copied()
    }

    fn skip_spaces(&mut self) -> bool {
        let mut saw = false;
        while self.pos < self.src.len() {
            let r = self.src[self.pos];
            if r == ' ' || r == '\t' || r == '\r' {
                self.advance();
                saw = true;
                continue;
            }
            break;
        }
        saw
    }

    fn consume_line_comment(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos] != '\n' {
            self.advance();
        }
    }

    fn emit(&mut self, t: TokenType, literal: &str) -> Token {
        let tok = Token {
            type_: t,
            literal: literal.to_string(),
            line: self.line,
            column: self.col,
            leading_space: self.space_before,
        };
        if t != TokenType::Eof {
            self.prev_tok = t;
        }
        tok
    }

    fn emit_advance(&mut self, t: TokenType, literal: &str) -> Token {
        let tok = self.emit(t, literal);
        self.advance();
        tok
    }

    fn emit_advance_n(&mut self, t: TokenType, literal: &str, n: usize) -> Token {
        let tok = self.emit(t, literal);
        for _ in 0..n {
            self.advance();
        }
        tok
    }

    /// 直接构造记号（不更新 `prev_tok`），用于数字、字符串、字符与错误路径。
    /// 这些路径在 Go 版中同样不带 `LeadingSpace`。
    fn raw_token(&self, t: TokenType, literal: &str, line: usize, column: usize) -> Token {
        Token {
            type_: t,
            literal: literal.to_string(),
            line,
            column,
            leading_space: false,
        }
    }

    /// 直接构造记号并显式指定前导空白，用于标识符与关键字。
    fn spaced_token(
        &self,
        t: TokenType,
        literal: &str,
        line: usize,
        column: usize,
        leading_space: bool,
    ) -> Token {
        Token {
            type_: t,
            literal: literal.to_string(),
            line,
            column,
            leading_space,
        }
    }

    // ------------------------------------------------------------ 运算符

    fn read_operator(&mut self) -> Token {
        let r = self.src[self.pos];
        let next = self.peek(1);
        match r {
            '(' | '（' => self.emit_advance(TokenType::LParen, &r.to_string()),
            ')' | '）' => self.emit_advance(TokenType::RParen, &r.to_string()),
            '[' => self.emit_advance(TokenType::LBrack, "["),
            ']' => self.emit_advance(TokenType::RBrack, "]"),
            '{' => self.emit_advance(TokenType::LBrace, "{"),
            '}' => self.emit_advance(TokenType::RBrace, "}"),
            ',' => self.emit_advance(TokenType::Comma, ","),
            '，' => self.emit_advance(TokenType::Comma, "，"),
            '、' => self.emit_advance(TokenType::Comma, "、"),
            '@' | '＠' => self.emit_advance(TokenType::Tag, &r.to_string()),
            '?' | '？' => self.emit_advance(TokenType::Quest, &r.to_string()),
            '.' => {
                if next == Some('.') && self.peek(2) == Some('.') {
                    self.emit_advance_n(TokenType::Ellipsis, "...", 3)
                } else {
                    self.emit_advance(TokenType::Dot, ".")
                }
            }
            ':' => self.emit_advance(TokenType::Colon, ":"),
            '：' => self.emit_advance(TokenType::Colon, "："),
            '+' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::PlusEq, "+=", 2)
                } else if next == Some('+') {
                    self.emit_advance_n(TokenType::Inc, "++", 2)
                } else {
                    self.emit_advance(TokenType::Plus, "+")
                }
            }
            '-' => {
                if next == Some('>') {
                    self.emit_advance_n(TokenType::Arrow, "->", 2)
                } else if next == Some('=') {
                    self.emit_advance_n(TokenType::MinusEq, "-=", 2)
                } else if next == Some('-') {
                    self.emit_advance_n(TokenType::Dec, "--", 2)
                } else {
                    self.emit_advance(TokenType::Minus, "-")
                }
            }
            '→' => self.emit_advance(TokenType::Arrow, "→"),
            '*' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::StarEq, "*=", 2)
                } else {
                    self.emit_advance(TokenType::Star, "*")
                }
            }
            '/' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::SlashEq, "/=", 2)
                } else {
                    self.emit_advance(TokenType::Slash, "/")
                }
            }
            '%' => self.emit_advance(TokenType::Percent, "%"),
            '=' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::Eq, "==", 2)
                } else {
                    self.emit_advance(TokenType::Assign, "=")
                }
            }
            '!' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::Neq, "!=", 2)
                } else {
                    self.emit_advance(TokenType::Not, "!")
                }
            }
            '<' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::Lte, "<=", 2)
                } else if next == Some('<') {
                    self.emit_advance_n(TokenType::Shl, "<<", 2)
                } else {
                    self.emit_advance(TokenType::Lt, "<")
                }
            }
            '>' => {
                if next == Some('=') {
                    self.emit_advance_n(TokenType::Gte, ">=", 2)
                } else if next == Some('>') {
                    self.emit_advance_n(TokenType::Shr, ">>", 2)
                } else {
                    self.emit_advance(TokenType::Gt, ">")
                }
            }
            '&' => {
                if next == Some('&') {
                    self.emit_advance_n(TokenType::And, "&&", 2)
                } else {
                    self.emit_advance(TokenType::BAnd, "&")
                }
            }
            '|' => {
                if next == Some('|') {
                    self.emit_advance_n(TokenType::Or, "||", 2)
                } else {
                    self.emit_advance(TokenType::BOr, "|")
                }
            }
            '^' => self.emit_advance(TokenType::BXor, "^"),
            '~' => self.emit_advance(TokenType::BNot, "~"),
            _ => self.emit_advance(TokenType::Illegal, &r.to_string()),
        }
    }

    // ------------------------------------------------------------ 数字

    fn read_number(&mut self) -> Token {
        let start = self.pos;
        let start_col = self.col;

        // 二进制、八进制、十六进制前缀：0b/0B、0o/0O、0x/0X。
        if self.src[self.pos] == '0' && self.pos + 1 < self.src.len() {
            let (valid, base_name): (fn(char) -> bool, &str) = match self.src[self.pos + 1] {
                'b' | 'B' => (is_binary_digit, "二进制"),
                'o' | 'O' => (is_octal_digit, "八进制"),
                'x' | 'X' => (is_hex_digit, "十六进制"),
                _ => return self.read_decimal_number(start, start_col),
            };
            self.advance();
            self.advance();
            let digit_start = self.pos;
            while self.pos < self.src.len() && valid(self.src[self.pos]) {
                self.advance();
            }
            let followed_by_ident = self.pos < self.src.len() && is_ident_part(self.src[self.pos]);
            if self.pos == digit_start || followed_by_ident {
                let msg = format!("{base_name}整数格式错误");
                return self.raw_token(TokenType::Illegal, &msg, self.line, start_col);
            }
            let literal: String = self.src[start..self.pos].iter().collect();
            return self.raw_token(TokenType::Int, &literal, self.line, start_col);
        }
        self.read_decimal_number(start, start_col)
    }

    fn read_decimal_number(&mut self, start: usize, start_col: usize) -> Token {
        while self.pos < self.src.len() && is_digit(self.src[self.pos]) {
            self.advance();
        }
        let mut is_float = false;
        if self.pos < self.src.len()
            && self.src[self.pos] == '.'
            && self.pos + 1 < self.src.len()
            && is_digit(self.src[self.pos + 1])
        {
            is_float = true;
            self.advance();
            while self.pos < self.src.len() && is_digit(self.src[self.pos]) {
                self.advance();
            }
        }
        let literal: String = self.src[start..self.pos].iter().collect();
        let t = if is_float {
            TokenType::Float
        } else {
            TokenType::Int
        };
        self.raw_token(t, &literal, self.line, start_col)
    }

    // ------------------------------------------------------------ 字符串

    fn read_string(&mut self) -> Token {
        let start_col = self.col;
        let line = self.line;
        self.advance(); // 跳过开头引号
        let mut buf = String::new();
        let mut out: Vec<Token> = Vec::new();
        let mut has_interp = false;

        macro_rules! flush {
            () => {
                if !buf.is_empty() {
                    out.push(self.raw_token(TokenType::StringPart, &buf, line, start_col));
                    buf.clear();
                }
            };
        }

        loop {
            if self.pos >= self.src.len() {
                return self.raw_token(TokenType::Illegal, "字符串未闭合", line, start_col);
            }
            let r = self.src[self.pos];
            if r == '"' {
                self.advance();
                if !has_interp {
                    return self.raw_token(TokenType::Str, &buf, line, start_col);
                }
                flush!();
                out.push(self.raw_token(TokenType::InterpEnd, "", line, start_col));
                let first = out.remove(0);
                for t in out {
                    self.queue.push_back(t);
                }
                return first;
            }
            if r == '\n' {
                return self.raw_token(TokenType::Illegal, "字符串未闭合", self.line, start_col);
            }
            if r == '\\' {
                let next = self.peek(1);
                if next == Some('$') {
                    buf.push('$');
                    self.advance();
                    self.advance();
                    continue;
                }
                match next.and_then(escape_rune) {
                    Some(e) => {
                        buf.push(e);
                        self.advance();
                        self.advance();
                        continue;
                    }
                    // 未知转义按原样保留（连同反斜杠一起）。凡语言自身不解释
                    // `\d`、`\w` 这类序列，但正则、格式串等消费者需要它们，
                    // 这样凡代码可直接写 "\d+" 而不必写成 "\\d+"。
                    None => {
                        buf.push('\\');
                        self.advance();
                        if let Some(c) = next {
                            buf.push(c);
                            self.advance();
                        }
                        continue;
                    }
                }
            }
            if r == '$' {
                let after = self.peek(1);
                if after == Some('{') {
                    has_interp = true;
                    flush!();
                    out.push(self.raw_token(TokenType::InterpStart, "", self.line, self.col));
                    self.advance(); // $
                    self.advance(); // {
                    let mut depth = 1;
                    while depth > 0 {
                        if self.pos >= self.src.len() {
                            return self.raw_token(
                                TokenType::Illegal,
                                "插值表达式未闭合",
                                line,
                                start_col,
                            );
                        }
                        let c = self.src[self.pos];
                        if c == '{' {
                            depth += 1;
                            let tok = self.emit_advance(TokenType::LBrace, "{");
                            out.push(tok);
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                self.advance();
                                out.push(self.raw_token(
                                    TokenType::InterpEnd,
                                    "",
                                    self.line,
                                    self.col,
                                ));
                            } else {
                                out.push(self.emit_advance(TokenType::RBrace, "}"));
                            }
                        } else if c == '"' {
                            out.push(self.read_string_inline());
                        } else {
                            self.space_before = self.skip_spaces();
                            if self.pos >= self.src.len() {
                                return self.raw_token(
                                    TokenType::Illegal,
                                    "插值表达式未闭合",
                                    line,
                                    start_col,
                                );
                            }
                            let mut tok = self.next_raw_token();
                            tok.leading_space = self.space_before;
                            out.push(tok);
                        }
                    }
                } else if after.is_some_and(is_ident_start) {
                    has_interp = true;
                    flush!();
                    self.advance(); // $
                    let id_start = self.pos;
                    while self.pos < self.src.len() && is_ident_part(self.src[self.pos]) {
                        self.advance();
                    }
                    let name: String = self.src[id_start..self.pos].iter().collect();
                    out.push(self.raw_token(TokenType::InterpIdent, &name, self.line, start_col));
                } else {
                    // $ 后不是标识符或 {，按普通字符处理（常见于正则等）。
                    buf.push('$');
                    self.advance();
                }
                continue;
            }
            buf.push(r);
            self.advance();
        }
    }

    /// 读取插值表达式内部的嵌套普通字符串，返回单个 STRING 记号。
    fn read_string_inline(&mut self) -> Token {
        let start_col = self.col;
        let start_line = self.line;
        self.advance(); // 引号
        let mut buf = String::new();
        loop {
            if self.pos >= self.src.len() {
                return self.raw_token(TokenType::Illegal, "字符串未闭合", start_line, start_col);
            }
            let r = self.src[self.pos];
            if r == '"' {
                self.advance();
                return self.raw_token(TokenType::Str, &buf, start_line, start_col);
            }
            if r == '\\' {
                let next = self.peek(1);
                if next == Some('$') {
                    buf.push('$');
                    self.advance();
                    self.advance();
                    continue;
                }
                if let Some(e) = next.and_then(escape_rune) {
                    buf.push(e);
                    self.advance();
                    self.advance();
                    continue;
                }
            }
            buf.push(r);
            self.advance();
        }
    }

    /// 在不触发字符串插值队列的情况下切下一个记号。
    fn next_raw_token(&mut self) -> Token {
        if self.pos >= self.src.len() {
            return self.emit(TokenType::Eof, "");
        }
        let r = self.src[self.pos];
        if r == '\n' {
            let tok = self.emit(TokenType::Newline, "\n");
            self.advance();
            self.line += 1;
            self.col = 1;
            return tok;
        }
        if r == '#' {
            self.consume_line_comment();
            self.space_before = self.skip_spaces();
            return self.next_raw_token();
        }
        if r == '\'' || r == '‘' {
            return self.read_char();
        }
        if is_digit(r) {
            return self.read_number();
        }
        if is_ident_start(r) {
            return self.read_identifier_or_keyword();
        }
        self.read_operator()
    }

    // ------------------------------------------------------------ 字符字面量

    fn read_char(&mut self) -> Token {
        let start_col = self.col;
        let open = self.src[self.pos];
        let close = if open == '‘' { '’' } else { '\'' };
        self.advance();
        let mut value = '\0';
        let mut count = 0;
        loop {
            if self.pos >= self.src.len() {
                return self.raw_token(
                    TokenType::Illegal,
                    "字符字面量未闭合",
                    self.line,
                    start_col,
                );
            }
            let r = self.src[self.pos];
            if r == close {
                self.advance();
                if count != 1 {
                    return self.raw_token(
                        TokenType::Illegal,
                        "字符字面量必须包含恰好一个字符",
                        self.line,
                        start_col,
                    );
                }
                return self.raw_token(TokenType::Char, &value.to_string(), self.line, start_col);
            }
            if r == '\n' {
                return self.raw_token(
                    TokenType::Illegal,
                    "字符字面量未闭合",
                    self.line,
                    start_col,
                );
            }
            if r == '\\' {
                let next = self.peek(1);
                match next.and_then(escape_rune) {
                    Some(e) => {
                        value = e;
                        self.advance();
                        self.advance();
                    }
                    None => {
                        let msg = format!("未知转义 \\{}", next.unwrap_or('\0'));
                        return self.raw_token(TokenType::Illegal, &msg, self.line, self.col);
                    }
                }
            } else {
                value = r;
                self.advance();
            }
            count += 1;
            if count > 1 {
                return self.raw_token(
                    TokenType::Illegal,
                    "字符字面量必须包含恰好一个字符",
                    self.line,
                    start_col,
                );
            }
        }
    }

    // ------------------------------------------------------------ 标识符

    fn read_identifier_or_keyword(&mut self) -> Token {
        let start = self.pos;
        let start_col = self.col;
        while self.pos < self.src.len() && is_ident_part(self.src[self.pos]) {
            self.advance();
        }
        let literal: String = self.src[start..self.pos].iter().collect();

        // 右括号后紧跟“的xxx”：多段函数调用的粘连写法，拆出“的”作为成员连接词。
        if self.prev_tok == TokenType::RParen
            && literal.starts_with('的')
            && literal.chars().count() > 1
        {
            return self.split_member_after_paren(start, start_col, !self.space_before);
        }

        // “注释”是保留字，但只有后跟空白/冒号/行尾时才作为注释起始。
        if literal == "注释" {
            let next = self.peek(0);
            if matches!(
                next,
                None | Some('\n') | Some(' ') | Some('\t') | Some(':') | Some('：')
            ) {
                self.consume_line_comment();
                return self.next_token();
            }
        }

        match lookup_keyword(&literal) {
            Some(t) => self.spaced_token(t, &literal, self.line, start_col, self.space_before),
            None => self.spaced_token(
                TokenType::Ident,
                &literal,
                self.line,
                start_col,
                self.space_before,
            ),
        }
    }

    /// 处理右括号后的“的成员”粘连写法。
    /// `no_space` 表示右括号与“的”之间没有空白（属于同一条多段调用）。
    fn split_member_after_paren(
        &mut self,
        start: usize,
        start_col: usize,
        no_space: bool,
    ) -> Token {
        let end = start + 1; // “的”的长度
        if end > self.src.len() || (end < self.src.len() && self.src[end] == '\n') {
            let literal: String = self.src[start..self.pos].iter().collect();
            return self.raw_token(TokenType::Ident, &literal, self.line, start_col);
        }
        self.pos = end;
        self.spaced_token(TokenType::Member, "的", self.line, start_col, !no_space)
    }
}

fn escape_rune(r: char) -> Option<char> {
    match r {
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '\\' => Some('\\'),
        _ => None,
    }
}

fn is_digit(r: char) -> bool {
    r.is_ascii_digit()
}

fn is_ident_start(r: char) -> bool {
    r == '_' || r.is_alphabetic()
}

fn is_ident_part(r: char) -> bool {
    is_ident_start(r) || is_digit(r)
}

fn is_binary_digit(r: char) -> bool {
    r == '0' || r == '1'
}

fn is_octal_digit(r: char) -> bool {
    ('0'..='7').contains(&r)
}

fn is_hex_digit(r: char) -> bool {
    r.is_ascii_digit() || ('a'..='f').contains(&r) || ('A'..='F').contains(&r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 切出全部记号（不含末尾 EOF），返回 (类型, 文本) 便于断言。
    fn kinds(src: &str) -> Vec<(TokenType, String)> {
        Lexer::new(src)
            .tokenize()
            .into_iter()
            .filter(|t| t.type_ != TokenType::Eof)
            .map(|t| (t.type_, t.literal))
            .collect()
    }

    fn types(src: &str) -> Vec<TokenType> {
        kinds(src).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn 空输入只产出文件末尾记号() {
        assert_eq!(types(""), vec![]);
        assert_eq!(Lexer::new("").tokenize().len(), 1);
        assert_eq!(Lexer::new("").tokenize()[0].type_, TokenType::Eof);
    }

    #[test]
    fn 中文关键字被识别为对应记号() {
        assert_eq!(types("如果"), vec![TokenType::If]);
        assert_eq!(types("那么"), vec![TokenType::Then]);
        assert_eq!(types("结束"), vec![TokenType::End]);
        assert_eq!(types("函数"), vec![TokenType::Function]);
        assert_eq!(types("模型"), vec![TokenType::Class]);
        assert_eq!(types("返回"), vec![TokenType::Return]);
        assert_eq!(types("的"), vec![TokenType::Member]);
        assert_eq!(types("真"), vec![TokenType::True]);
        assert_eq!(types("空"), vec![TokenType::Nil]);
    }

    #[test]
    fn 非关键字中文是标识符() {
        assert_eq!(kinds("小明"), vec![(TokenType::Ident, "小明".to_string())]);
        assert_eq!(kinds("_甲1"), vec![(TokenType::Ident, "_甲1".to_string())]);
    }

    #[test]
    fn 标识符允许下划线字母数字混合() {
        assert_eq!(
            kinds("abc_123"),
            vec![(TokenType::Ident, "abc_123".to_string())]
        );
    }

    #[test]
    fn 十进制整数与小数() {
        assert_eq!(kinds("42"), vec![(TokenType::Int, "42".to_string())]);
        assert_eq!(kinds("0"), vec![(TokenType::Int, "0".to_string())]);
        assert_eq!(kinds("3.14"), vec![(TokenType::Float, "3.14".to_string())]);
        assert_eq!(kinds("0.5"), vec![(TokenType::Float, "0.5".to_string())]);
    }

    #[test]
    fn 小数点后无数字时不构成小数() {
        // “1.” 后面不是数字，按整数 1 + 点号处理。
        assert_eq!(types("1."), vec![TokenType::Int, TokenType::Dot]);
    }

    #[test]
    fn 进制字面量整数() {
        assert_eq!(
            kinds("0b1010"),
            vec![(TokenType::Int, "0b1010".to_string())]
        );
        assert_eq!(kinds("0o17"), vec![(TokenType::Int, "0o17".to_string())]);
        assert_eq!(kinds("0xFF"), vec![(TokenType::Int, "0xFF".to_string())]);
        assert_eq!(kinds("0Xab"), vec![(TokenType::Int, "0Xab".to_string())]);
    }

    #[test]
    fn 进制字面量缺数字报非法() {
        let toks = Lexer::new("0b").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
        assert_eq!(toks[0].literal, "二进制整数格式错误");

        let toks = Lexer::new("0x").tokenize();
        assert_eq!(toks[0].literal, "十六进制整数格式错误");
    }

    #[test]
    fn 进制字面量含非法数字报非法() {
        let toks = Lexer::new("0b12").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
        assert_eq!(toks[0].literal, "二进制整数格式错误");
    }

    #[test]
    fn 普通字符串读取内容() {
        assert_eq!(
            kinds("\"你好\""),
            vec![(TokenType::Str, "你好".to_string())]
        );
        assert_eq!(kinds("\"\""), vec![(TokenType::Str, "".to_string())]);
    }

    #[test]
    fn 字符串支持转义序列() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![(TokenType::Str, "a\nb".to_string())]
        );
        assert_eq!(
            kinds(r#""a\tb""#),
            vec![(TokenType::Str, "a\tb".to_string())]
        );
        assert_eq!(
            kinds(r#""a\\b""#),
            vec![(TokenType::Str, "a\\b".to_string())]
        );
        assert_eq!(
            kinds(r#""a\"b""#),
            vec![(TokenType::Str, "a\"b".to_string())]
        );
    }

    #[test]
    fn 转义美元符号得到字面美元() {
        assert_eq!(
            kinds(r#""\$100""#),
            vec![(TokenType::Str, "$100".to_string())]
        );
    }

    #[test]
    fn 未知转义保留原样供正则等消费者解释() {
        // 凡语言自身不解释 `\d`，保留给正则等消费者
        assert_eq!(
            kinds(r#""\d+""#),
            vec![(TokenType::Str, "\\d+".to_string())]
        );
        assert_eq!(
            kinds(r#""\w\s""#),
            vec![(TokenType::Str, "\\w\\s".to_string())]
        );
    }

    #[test]
    fn 双反斜杠仍解出单个反斜杠() {
        // "\\d+" 与 "\d+" 等价，兼容既有写法
        assert_eq!(
            kinds(r#""\\d+""#),
            vec![(TokenType::Str, "\\d+".to_string())]
        );
    }

    #[test]
    fn 字符字面量中的未知转义仍报非法() {
        // 字符字面量必须恰好一个字符，`\d` 会变成两个，故报错
        let toks = Lexer::new(r#"'\q'"#).tokenize();
        assert!(toks.iter().any(|t| t.type_ == TokenType::Illegal));
    }

    #[test]
    fn 未闭合字符串报非法() {
        let toks = Lexer::new("\"abc").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
        assert_eq!(toks[0].literal, "字符串未闭合");
    }

    #[test]
    fn 字符串不能跨行() {
        let toks = Lexer::new("\"ab\ncd\"").tokenize();
        assert!(toks.iter().any(|t| t.type_ == TokenType::Illegal));
    }

    #[test]
    fn 插值变量产生片段与标识符() {
        let toks = kinds("\"你好 $名称\"");
        assert_eq!(
            toks,
            vec![
                (TokenType::StringPart, "你好 ".to_string()),
                (TokenType::InterpIdent, "名称".to_string()),
                (TokenType::InterpEnd, String::new()),
            ]
        );
    }

    #[test]
    fn 插值表达式产生起始与结束记号() {
        let toks = kinds("\"共 ${数量 乘 2} 个\"");
        assert_eq!(toks[0], (TokenType::StringPart, "共 ".to_string()));
        assert_eq!(toks[1], (TokenType::InterpStart, String::new()));
        // 中间是表达式记号：数量 乘 2
        assert_eq!(toks[2], (TokenType::Ident, "数量".to_string()));
        assert_eq!(toks[3], (TokenType::Star, "乘".to_string()));
        assert_eq!(toks[4], (TokenType::Int, "2".to_string()));
        assert_eq!(toks[5], (TokenType::InterpEnd, String::new()));
        assert_eq!(toks[6], (TokenType::StringPart, " 个".to_string()));
        assert_eq!(toks[7], (TokenType::InterpEnd, String::new()));
    }

    #[test]
    fn 插值内可嵌套花括号() {
        let toks = kinds("\"${d[\"k\"]}\"");
        assert_eq!(toks[0], (TokenType::InterpStart, String::new()));
        assert!(toks.iter().any(|(t, l)| *t == TokenType::Str && l == "k"));
    }

    #[test]
    fn 美元后不是标识符或花括号时按普通字符() {
        // “!”不是标识符起始字符，因此 $ 按普通字符保留（常见于正则等场景）。
        assert_eq!(kinds("\"a$!\""), vec![(TokenType::Str, "a$!".to_string())]);
        assert_eq!(
            kinds("\"100$\""),
            vec![(TokenType::Str, "100$".to_string())]
        );
    }

    #[test]
    fn 未闭合插值表达式报非法() {
        let toks = Lexer::new("\"${abc").tokenize();
        assert!(toks.iter().any(|t| t.type_ == TokenType::Illegal));
    }

    #[test]
    fn 半角与全角字符字面量() {
        assert_eq!(kinds("'A'"), vec![(TokenType::Char, "A".to_string())]);
        assert_eq!(kinds("'中'"), vec![(TokenType::Char, "中".to_string())]);
        assert_eq!(kinds("‘中’"), vec![(TokenType::Char, "中".to_string())]);
        assert_eq!(kinds(r"'\n'"), vec![(TokenType::Char, "\n".to_string())]);
    }

    #[test]
    fn 字符字面量必须恰好一个字符() {
        let toks = Lexer::new("'ab'").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
        assert_eq!(toks[0].literal, "字符字面量必须包含恰好一个字符");

        let toks = Lexer::new("''").tokenize();
        assert_eq!(toks[0].literal, "字符字面量必须包含恰好一个字符");
    }

    #[test]
    fn 未闭合字符字面量报非法() {
        let toks = Lexer::new("'a").tokenize();
        assert_eq!(toks[0].literal, "字符字面量未闭合");
    }

    #[test]
    fn 井号注释被丢弃() {
        assert_eq!(types("# 单行注释"), vec![]);
        assert_eq!(
            kinds("甲 # 注释\n"),
            vec![
                (TokenType::Ident, "甲".to_string()),
                (TokenType::Newline, "\n".to_string())
            ]
        );
    }

    #[test]
    fn 中文注释关键字后跟空白时被丢弃() {
        assert_eq!(types("注释 这是注释"), vec![]);
        assert_eq!(types("注释:这是注释"), vec![]);
        assert_eq!(types("注释：这是注释"), vec![]);
    }

    #[test]
    fn 注释后紧跟内容时不是注释() {
        // “注释abc”不满足注释起始条件，视为标识符。
        assert_eq!(
            kinds("注释abc"),
            vec![(TokenType::Ident, "注释abc".to_string())]
        );
    }

    #[test]
    fn 字符串内的井号与注释字样是普通字符() {
        assert_eq!(
            kinds("\"价格#100\""),
            vec![(TokenType::Str, "价格#100".to_string())]
        );
        assert_eq!(
            kinds("\"含 注释 字样\""),
            vec![(TokenType::Str, "含 注释 字样".to_string())]
        );
    }

    #[test]
    fn 双字符运算符优先于单字符() {
        assert_eq!(types("=="), vec![TokenType::Eq]);
        assert_eq!(types("!="), vec![TokenType::Neq]);
        assert_eq!(types("<="), vec![TokenType::Lte]);
        assert_eq!(types(">="), vec![TokenType::Gte]);
        assert_eq!(types("&&"), vec![TokenType::And]);
        assert_eq!(types("||"), vec![TokenType::Or]);
        assert_eq!(types("<<"), vec![TokenType::Shl]);
        assert_eq!(types(">>"), vec![TokenType::Shr]);
        assert_eq!(types("->"), vec![TokenType::Arrow]);
        assert_eq!(types("++"), vec![TokenType::Inc]);
        assert_eq!(types("--"), vec![TokenType::Dec]);
        assert_eq!(types("+="), vec![TokenType::PlusEq]);
        assert_eq!(types("-="), vec![TokenType::MinusEq]);
        assert_eq!(types("*="), vec![TokenType::StarEq]);
        assert_eq!(types("/="), vec![TokenType::SlashEq]);
    }

    #[test]
    fn 单字符运算符() {
        assert_eq!(types("+"), vec![TokenType::Plus]);
        assert_eq!(types("-"), vec![TokenType::Minus]);
        assert_eq!(types("*"), vec![TokenType::Star]);
        assert_eq!(types("/"), vec![TokenType::Slash]);
        assert_eq!(types("%"), vec![TokenType::Percent]);
        assert_eq!(types("<"), vec![TokenType::Lt]);
        assert_eq!(types(">"), vec![TokenType::Gt]);
        assert_eq!(types("="), vec![TokenType::Assign]);
        assert_eq!(types("!"), vec![TokenType::Not]);
        assert_eq!(types("&"), vec![TokenType::BAnd]);
        assert_eq!(types("|"), vec![TokenType::BOr]);
        assert_eq!(types("^"), vec![TokenType::BXor]);
        assert_eq!(types("~"), vec![TokenType::BNot]);
        assert_eq!(types("@"), vec![TokenType::Tag]);
        assert_eq!(types("?"), vec![TokenType::Quest]);
        assert_eq!(types("."), vec![TokenType::Dot]);
        assert_eq!(types(":"), vec![TokenType::Colon]);
    }

    #[test]
    fn 省略号记号() {
        assert_eq!(kinds("..."), vec![(TokenType::Ellipsis, "...".to_string())]);
    }

    #[test]
    fn 全角标点等价处理() {
        assert_eq!(types("（）"), vec![TokenType::LParen, TokenType::RParen]);
        assert_eq!(types("，"), vec![TokenType::Comma]);
        assert_eq!(types("、"), vec![TokenType::Comma]);
        assert_eq!(types("："), vec![TokenType::Colon]);
        assert_eq!(types("？"), vec![TokenType::Quest]);
        assert_eq!(types("→"), vec![TokenType::Arrow]);
        assert_eq!(types("＠"), vec![TokenType::Tag]);
    }

    #[test]
    fn 不支持的全角符号报非法() {
        // 全角方括号、花括号、等号等不在兼容列表内。
        let toks = Lexer::new("［1］").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
        let toks = Lexer::new("＝").tokenize();
        assert_eq!(toks[0].type_, TokenType::Illegal);
    }

    #[test]
    fn 换行产生换行记号() {
        assert_eq!(
            types("甲\n乙"),
            vec![TokenType::Ident, TokenType::Newline, TokenType::Ident]
        );
    }

    #[test]
    fn 行列号正确递增() {
        let toks = Lexer::new("甲 乙\n丙").tokenize();
        assert_eq!((toks[0].line, toks[0].column), (1, 1));
        assert_eq!((toks[1].line, toks[1].column), (1, 3));
        assert_eq!(toks[2].type_, TokenType::Newline);
        assert_eq!((toks[2].line, toks[2].column), (1, 4));
        assert_eq!((toks[3].line, toks[3].column), (2, 1));
    }

    #[test]
    fn 记号前空白被记录() {
        let toks = Lexer::new("甲 乙").tokenize();
        assert!(!toks[0].leading_space);
        assert!(toks[1].leading_space);
    }

    #[test]
    fn 右括号后紧邻的汉字被拆出成员连接词() {
        // 模拟 根(1)的段(2)：右括号后紧跟“的段”
        let toks = Lexer::new("(1)的段").tokenize();
        assert_eq!(toks[0].type_, TokenType::LParen);
        assert_eq!(toks[1].type_, TokenType::Int);
        assert_eq!(toks[2].type_, TokenType::RParen);
        assert_eq!(toks[3].type_, TokenType::Member);
        assert_eq!(toks[3].literal, "的");
        assert_eq!(toks[4].type_, TokenType::Ident);
        assert_eq!(toks[4].literal, "段");
    }

    #[test]
    fn 变量声明整句切分() {
        assert_eq!(
            types("变量 整数 年龄 = 18"),
            vec![
                TokenType::Var,
                TokenType::TypeInt,
                TokenType::Ident,
                TokenType::Assign,
                TokenType::Int,
            ]
        );
    }

    #[test]
    fn 函数定义首行切分() {
        assert_eq!(
            types("函数 求和(甲, 乙) -> 整数"),
            vec![
                TokenType::Function,
                TokenType::Ident,
                TokenType::LParen,
                TokenType::Ident,
                TokenType::Comma,
                TokenType::Ident,
                TokenType::RParen,
                TokenType::Arrow,
                TokenType::TypeInt,
            ]
        );
    }

    #[test]
    fn 标签记号与模型名() {
        assert_eq!(
            types("@内建(\"打印\")"),
            vec![
                TokenType::Tag,
                TokenType::Ident,
                TokenType::LParen,
                TokenType::Str,
                TokenType::RParen,
            ]
        );
    }

    #[test]
    fn 遍历语句切分() {
        assert_eq!(
            types("遍历 名单 中的 人"),
            vec![
                TokenType::ForIn,
                TokenType::Ident,
                TokenType::InOf,
                TokenType::Ident
            ]
        );
    }

    #[test]
    fn 制表符与回车被视作空白() {
        assert_eq!(
            types("甲\t乙\r丙"),
            vec![TokenType::Ident, TokenType::Ident, TokenType::Ident]
        );
    }

    #[test]
    fn 连续多次取记号不越界() {
        let mut lx = Lexer::new("甲");
        let _ = lx.next_token(); // 甲
        let eof1 = lx.next_token();
        let eof2 = lx.next_token();
        assert_eq!(eof1.type_, TokenType::Eof);
        assert_eq!(eof2.type_, TokenType::Eof);
    }
}
