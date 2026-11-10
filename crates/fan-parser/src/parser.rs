//! 语法分析器主体。
//!
//! 采用 Pratt（优先级爬升）解析，与 Go 版 `internal/parser` 的算法与错误消息保持一致。
//! 解析错误不中断解析，全部收集到 `Parser::errors` 中。

use crate::error::Errors;
use crate::precedence::{precedence_of, LOWEST, PREC_POSTFIX, PREC_PREFIX};
use fan_ast::{self as ast, DeclType, Position, Program, Stmt, Visibility};
use fan_lexer::{Lexer, Token, TokenType};

/// 优先级数值，越大结合越紧。
pub type Precedence = u8;

/// 语法分析器。
pub struct Parser {
    lexer: Lexer,
    cur: Token,
    peek: Token,
    peek2: Token,
    peek3: Token,
    errors: Errors,
    /// 括号/方括号嵌套深度，大于 0 时忽略换行（允许折行）。
    paren_depth: usize,
    /// 是否允许“无括号调用”：此标志只在表达式语句内打开。
    implicit_call_ok: bool,
}

impl Parser {
    pub fn new(source: &str) -> Self {
        let mut lexer = Lexer::new(source);
        let cur = lexer.next_token();
        let peek = lexer.next_token();
        let peek2 = lexer.next_token();
        let peek3 = lexer.next_token();
        Parser {
            lexer,
            cur,
            peek,
            peek2,
            peek3,
            errors: Errors::default(),
            paren_depth: 0,
            implicit_call_ok: false,
        }
    }

    pub fn errors(&self) -> &Errors {
        &self.errors
    }

    /// 前进一个记号。`paren_depth > 0` 时跳过换行，以支持括号内折行。
    fn next(&mut self) {
        self.cur = std::mem::replace(
            &mut self.peek,
            std::mem::replace(
                &mut self.peek2,
                std::mem::replace(&mut self.peek3, self.lexer.next_token()),
            ),
        );
        if self.paren_depth > 0 {
            while self.cur.type_ == TokenType::Newline {
                self.cur = std::mem::replace(
                    &mut self.peek,
                    std::mem::replace(
                        &mut self.peek2,
                        std::mem::replace(&mut self.peek3, self.lexer.next_token()),
                    ),
                );
            }
        }
    }

    fn add_error(&mut self, t: &Token, message: impl std::fmt::Display) {
        self.errors.0.push(crate::error::ParseError::new(
            t.line,
            t.column,
            message.to_string(),
        ));
    }

    fn add_error_at(&mut self, pos: Position, message: impl std::fmt::Display) {
        self.errors.0.push(crate::error::ParseError::new(
            pos.line,
            pos.column,
            message.to_string(),
        ));
    }

    fn cur_pos(&self) -> Position {
        Position::new(self.cur.line, self.cur.column)
    }

    fn skip_statement_end(&mut self) {
        while self.cur.type_ == TokenType::Newline {
            self.next();
        }
    }

    fn current_precedence(&self) -> Precedence {
        precedence_of(self.cur.type_)
    }

    /// 解析整个程序。
    pub fn parse_program(&mut self) -> Program {
        let mut program = Program::default();
        while self.cur.type_ != TokenType::Eof {
            if self.cur.type_ == TokenType::Newline {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::Illegal {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("非法字符：{}", t.literal));
                self.next();
                continue;
            }
            match self.parse_statement() {
                Some(stmt) => program.statements.push(stmt),
                None => {
                    while self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
                        self.next();
                    }
                }
            }
            self.skip_statement_end();
        }
        program
    }

    // ------------------------------------------------------------ 语句分派

    fn parse_statement(&mut self) -> Option<Stmt> {
        let t = self.cur.type_;
        match t {
            TokenType::Tag => self.parse_tagged_statement(),
            TokenType::Switch => self.parse_switch_statement(),
            TokenType::If => self.parse_if_statement(),
            TokenType::While => self.parse_while_statement(),
            TokenType::Repeat => self.parse_repeat_statement(),
            TokenType::ForIn => self.parse_for_each_statement(),
            TokenType::Defer => self.parse_defer_statement(),
            TokenType::Throw => self.parse_throw_statement(),
            TokenType::Import => self.parse_import_statement(),
            TokenType::Public => self.parse_visibility_statement(Visibility::Public),
            TokenType::Private => self.parse_visibility_statement(Visibility::Private),
            TokenType::Define => {
                self.next();
                match self.cur.type_ {
                    TokenType::Public => self.parse_visibility_statement(Visibility::Public),
                    TokenType::Private => self.parse_visibility_statement(Visibility::Private),
                    TokenType::Function => self.parse_function_statement(),
                    TokenType::Class => self.parse_class_statement(),
                    TokenType::Interface => self.parse_interface_statement(),
                    TokenType::TypeDef => self.parse_type_def_statement(),
                    TokenType::Ident if self.peek.type_ == TokenType::Member => {
                        self.parse_ident_member_statement()
                    }
                    TokenType::Ident
                        if is_type_token(self.peek.type_)
                            && self.peek2.type_ == TokenType::Member =>
                    {
                        self.parse_method_definition()
                    }
                    _ if is_type_token(self.cur.type_) && self.peek.type_ == TokenType::Member => {
                        self.parse_method_definition()
                    }
                    _ => self.parse_var_decl(),
                }
            }
            TokenType::Function => self.parse_function_statement(),
            TokenType::Return => self.parse_return_statement(),
            TokenType::Class => self.parse_class_statement(),
            TokenType::Interface => self.parse_interface_statement(),
            TokenType::TypeDef => self.parse_type_def_statement(),
            TokenType::Ident if self.peek.type_ == TokenType::Member => {
                self.parse_ident_member_statement()
            }
            TokenType::Ident
                if is_type_token(self.peek.type_) && self.peek2.type_ == TokenType::Member =>
            {
                self.parse_method_definition()
            }
            _ if is_type_token(self.cur.type_) && self.peek.type_ == TokenType::Member => {
                self.parse_method_definition()
            }
            TokenType::Break => {
                let stmt = ast::BreakStmt {
                    pos: self.cur_pos(),
                };
                self.next();
                Some(Stmt::Break(stmt))
            }
            TokenType::Continue => {
                let stmt = ast::ContinueStmt {
                    pos: self.cur_pos(),
                };
                self.next();
                Some(Stmt::Continue(stmt))
            }
            TokenType::Var | TokenType::Const => self.parse_var_decl(),
            _ if is_type_token(self.cur.type_) => {
                if self.peek.type_ == TokenType::Ident {
                    self.parse_var_decl()
                } else {
                    self.parse_expression_statement()
                }
            }
            TokenType::Ident if self.peek.type_ == TokenType::Assign => self.parse_bare_decl(),
            TokenType::Ident if self.peek.type_ == TokenType::Comma => self.parse_bare_decl(),
            TokenType::Ident if self.peek.type_ == TokenType::LBrack => {
                self.parse_index_assign_or_expr()
            }
            _ => self.parse_expression_statement(),
        }
    }

    fn parse_block(&mut self) -> Option<ast::BlockStmt> {
        let pos = self.cur_pos();
        let mut block = ast::BlockStmt {
            pos,
            statements: Vec::new(),
        };
        loop {
            self.skip_statement_end();
            match self.cur.type_ {
                TokenType::End
                | TokenType::Else
                | TokenType::ElseIf
                | TokenType::Until
                | TokenType::Eof => return Some(block),
                TokenType::Illegal => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("非法字符：{}", t.literal));
                    return None;
                }
                _ => {}
            }
            let stmt = self.parse_statement()?;
            block.statements.push(stmt);
            if !matches!(
                self.cur.type_,
                TokenType::Newline
                    | TokenType::End
                    | TokenType::Else
                    | TokenType::ElseIf
                    | TokenType::Until
                    | TokenType::Eof
            ) {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("语句后应换行，实际是 {:?}", t.literal));
                return None;
            }
        }
    }

    // ------------------------------------------------------------ 条件与判断

    fn parse_if_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        let branch = self.parse_if_branch()?;
        let mut stmt = ast::IfStmt {
            pos,
            branches: vec![branch],
            else_: None,
        };
        loop {
            match self.cur.type_ {
                TokenType::ElseIf => {
                    self.next();
                    stmt.branches.push(self.parse_if_branch()?);
                }
                TokenType::Else => {
                    self.next();
                    if self.cur.type_ == TokenType::If {
                        self.next();
                        stmt.branches.push(self.parse_if_branch()?);
                        continue;
                    }
                    if self.cur.type_ == TokenType::Then {
                        let t = self.cur.clone();
                        self.add_error(&t, "否则 后不能跟 那么");
                        return None;
                    }
                    stmt.else_ = Some(self.parse_block()?);
                    if self.cur.type_ != TokenType::End {
                        let t = self.cur.clone();
                        self.add_error(&t, "条件语句缺少 结束");
                        return None;
                    }
                    self.next();
                    return Some(Stmt::If(stmt));
                }
                TokenType::End => {
                    self.next();
                    return Some(Stmt::If(stmt));
                }
                TokenType::Eof => {
                    let t = self.cur.clone();
                    self.add_error(&t, "条件语句缺少 结束");
                    return None;
                }
                _ => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("条件语句中出现意外内容：{:?}", t.literal));
                    return None;
                }
            }
        }
    }

    fn parse_if_branch(&mut self) -> Option<ast::IfBranch> {
        let condition = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::Then {
            let t = self.cur.clone();
            self.add_error(&t, "如果 条件后缺少 那么");
            return None;
        }
        self.next();
        let body = self.parse_block()?;
        Some(ast::IfBranch { condition, body })
    }

    fn parse_switch_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        let subject = if self.cur.type_ != TokenType::Newline {
            Some(self.parse_expression(LOWEST)?)
        } else {
            None
        };
        self.skip_statement_end();
        let mut stmt = ast::SwitchStmt {
            pos,
            subject: subject.clone(),
            branches: Vec::new(),
        };
        loop {
            let branch = self.parse_switch_branch()?;
            if subject.is_none() && !branch.guard && !branch.default {
                let t = self.cur.clone();
                self.add_error(&t, "无匹配对象时，判断分支应使用 当");
                return None;
            }
            let is_default = branch.default;
            stmt.branches.push(branch);
            if self.cur.type_ == TokenType::End {
                self.next();
                return Some(Stmt::Switch(stmt));
            }
            if is_default && self.cur.type_ != TokenType::End {
                let t = self.cur.clone();
                self.add_error(&t, "其他 分支必须在最后");
                return None;
            }
            if self.cur.type_ == TokenType::Eof {
                let t = self.cur.clone();
                self.add_error(&t, "判断 语句缺少 结束");
                return None;
            }
        }
    }

    fn parse_switch_branch(&mut self) -> Option<ast::SwitchBranch> {
        let pos = self.cur_pos();
        let mut branch = ast::SwitchBranch {
            pos,
            guard: false,
            values: Vec::new(),
            body: ast::BlockStmt::default(),
            default: false,
        };
        match self.cur.type_ {
            TokenType::Assign => {
                self.next();
                loop {
                    branch.values.push(self.parse_expression(LOWEST)?);
                    if self.cur.type_ != TokenType::Comma {
                        break;
                    }
                    self.next();
                }
            }
            TokenType::While => {
                self.next();
                let condition = self.parse_expression(LOWEST)?;
                branch.guard = true;
                branch.values = vec![condition];
            }
            TokenType::Default => {
                self.next();
                branch.default = true;
            }
            TokenType::End | TokenType::Eof => {
                let t = self.cur.clone();
                self.add_error(&t, "判断 语句缺少 结束");
                return None;
            }
            _ => {
                let t = self.cur.clone();
                self.add_error(&t, "判断 分支应以 为、当 或 其他 开头");
                return None;
            }
        }
        self.skip_statement_end();
        branch.body = self.parse_switch_body()?;
        Some(branch)
    }

    fn parse_switch_body(&mut self) -> Option<ast::BlockStmt> {
        let pos = self.cur_pos();
        let mut block = ast::BlockStmt {
            pos,
            statements: Vec::new(),
        };
        loop {
            self.skip_statement_end();
            match self.cur.type_ {
                TokenType::Assign
                | TokenType::While
                | TokenType::Default
                | TokenType::End
                | TokenType::Eof => return Some(block),
                TokenType::Illegal => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("非法字符：{}", t.literal));
                    return None;
                }
                _ => {}
            }
            let stmt = self.parse_statement()?;
            block.statements.push(stmt);
            if !matches!(
                self.cur.type_,
                TokenType::Newline
                    | TokenType::Assign
                    | TokenType::While
                    | TokenType::Default
                    | TokenType::End
                    | TokenType::Eof
            ) {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("语句后应换行，实际是 {:?}", t.literal));
                return None;
            }
        }
    }

    // ------------------------------------------------------------ 循环

    fn parse_while_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        let condition = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::Loop {
            let t = self.cur.clone();
            self.add_error(&t, "当 条件后缺少 循环");
            return None;
        }
        self.next();
        let body = self.parse_block()?;
        if self.cur.type_ != TokenType::End {
            let t = self.cur.clone();
            self.add_error(&t, "循环缺少 结束");
            return None;
        }
        self.next();
        Some(Stmt::While(ast::WhileStmt {
            pos,
            condition,
            body,
        }))
    }

    fn parse_repeat_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        if self.peek.type_ != TokenType::Newline {
            return self.parse_times_statement(pos);
        }
        self.next();
        let body = self.parse_block()?;
        if self.cur.type_ != TokenType::Until {
            let t = self.cur.clone();
            self.add_error(&t, "重复 循环缺少 直到 条件");
            return None;
        }
        self.next();
        let until = self.parse_expression(LOWEST)?;
        self.skip_statement_end();
        if self.cur.type_ != TokenType::End {
            let t = self.cur.clone();
            self.add_error(&t, "重复 循环缺少 结束");
            return None;
        }
        self.next();
        Some(Stmt::Repeat(ast::RepeatStmt { pos, body, until }))
    }

    fn parse_times_statement(&mut self, pos: Position) -> Option<Stmt> {
        self.next();
        let count = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::Times {
            let t = self.cur.clone();
            self.add_error(&t, "重复 次数后缺少 次");
            return None;
        }
        self.next();
        let body = self.parse_block()?;
        if self.cur.type_ == TokenType::Until {
            let t = self.cur.clone();
            self.add_error(&t, "重复 N 次 循环中不能使用 直到");
            return None;
        }
        if self.cur.type_ != TokenType::End {
            let t = self.cur.clone();
            self.add_error(&t, "重复 N 次 循环缺少 结束");
            return None;
        }
        self.next();
        Some(Stmt::Times(ast::TimesStmt { pos, count, body }))
    }

    fn parse_for_each_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        let iterable = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::InOf {
            let t = self.cur.clone();
            self.add_error(&t, "遍历 语句缺少 中的");
            return None;
        }
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "遍历 需要元素变量名");
            return None;
        }
        let first_name = self.cur.literal.clone();
        self.next();
        let mut second_name = None;
        if self.cur.type_ == TokenType::Ident {
            second_name = Some(self.cur.literal.clone());
            self.next();
        }
        let body = self.parse_block()?;
        if self.cur.type_ != TokenType::End {
            let t = self.cur.clone();
            self.add_error(&t, "遍历 语句缺少 结束");
            return None;
        }
        self.next();
        Some(Stmt::ForEach(ast::ForEachStmt {
            pos,
            iterable,
            first_name,
            second_name,
            body,
        }))
    }

    // ------------------------------------------------------------ 声明与赋值

    fn parse_var_decl(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        let mut is_const = false;
        let mut decl_type = DeclType::any();
        let mut saw_keyword = false;

        if self.cur.type_ == TokenType::Define {
            saw_keyword = true;
            self.next();
        }
        match self.cur.type_ {
            TokenType::Var => {
                saw_keyword = true;
                self.next();
            }
            TokenType::Const => {
                saw_keyword = true;
                is_const = true;
                self.next();
            }
            _ => {}
        }

        if is_type_token(self.cur.type_) {
            saw_keyword = true;
            decl_type = self.parse_type_union();
        } else if (self.cur.type_ == TokenType::Class || self.cur.type_ == TokenType::Ident)
            && (self.peek.type_ == TokenType::Ident || is_union_sep_token(self.peek.type_))
        {
            // 自定义类型标注：类型名 后接变量名，或后接 或/| 构成联合类型。
            saw_keyword = true;
            decl_type = self.parse_type_union();
        }

        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "声明语句缺少变量名");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();

        if self.cur.type_ == TokenType::Comma {
            let mut names = vec![name];
            while self.cur.type_ == TokenType::Comma {
                self.next();
                if self.cur.type_ != TokenType::Ident {
                    let t = self.cur.clone();
                    self.add_error(&t, "多变量声明应为 变量 名1、名2 = 值");
                    return None;
                }
                names.push(self.cur.literal.clone());
                self.next();
            }
            if self.cur.type_ != TokenType::Assign {
                if self.cur.type_ == TokenType::Newline || self.cur.type_ == TokenType::Eof {
                    if is_const {
                        let t = self.cur.clone();
                        self.add_error(&t, "常量声明必须赋初值（使用 为 或 =）");
                        return None;
                    }
                    return Some(Stmt::MultiDecl(ast::MultiDecl {
                        pos,
                        is_const: false,
                        is_explicit: saw_keyword,
                        names,
                        value: ast::Expr::Tuple(ast::TupleExpr {
                            pos,
                            elements: Vec::new(),
                        }),
                        visibility: Visibility::File,
                    }));
                }
                let t = self.cur.clone();
                self.add_error(&t, "多变量声明必须使用 为 或 =");
                return None;
            }
            self.next();
            let value = self.parse_rhs_list(pos)?;
            return Some(Stmt::MultiDecl(ast::MultiDecl {
                pos,
                is_const,
                is_explicit: saw_keyword,
                names,
                value,
                visibility: Visibility::File,
            }));
        }

        if self.cur.type_ != TokenType::Assign {
            if self.cur.type_ == TokenType::Newline || self.cur.type_ == TokenType::Eof {
                if is_const {
                    let t = self.cur.clone();
                    self.add_error(&t, "常量声明必须赋初值（使用 为 或 =）");
                    return None;
                }
                if !saw_keyword {
                    let t = self.cur.clone();
                    self.add_error(&t, "声明必须赋初值（使用 为 或 =）");
                    return None;
                }
                return Some(Stmt::VarDecl(ast::VarDecl {
                    pos,
                    is_const: false,
                    is_explicit: saw_keyword,
                    decl_type,
                    name,
                    value: None,
                    visibility: Visibility::File,
                }));
            }
            let t = self.cur.clone();
            self.add_error(&t, "声明必须赋初值（使用 为 或 =）");
            return None;
        }
        self.next();
        let value = self.parse_expression(LOWEST)?;
        Some(Stmt::VarDecl(ast::VarDecl {
            pos,
            is_const,
            is_explicit: saw_keyword,
            decl_type,
            name,
            value: Some(value),
            visibility: Visibility::File,
        }))
    }

    fn parse_bare_decl(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        let name = self.cur.literal.clone();
        self.next();
        if self.cur.type_ == TokenType::Comma {
            let mut names = vec![name];
            while self.cur.type_ == TokenType::Comma {
                self.next();
                if self.cur.type_ != TokenType::Ident {
                    let t = self.cur.clone();
                    self.add_error(&t, "多目标赋值应为 名1、名2 = 值");
                    return None;
                }
                names.push(self.cur.literal.clone());
                self.next();
            }
            if self.cur.type_ != TokenType::Assign {
                let t = self.cur.clone();
                self.add_error(&t, "多目标赋值应使用 为 或 =");
                return None;
            }
            self.next();
            let value = self.parse_rhs_list(pos)?;
            return Some(Stmt::MultiAssign(ast::MultiAssign { pos, names, value }));
        }
        self.next();
        let value = self.parse_expression(LOWEST)?;
        Some(Stmt::VarDecl(ast::VarDecl {
            pos,
            is_const: false,
            is_explicit: false,
            decl_type: DeclType::any(),
            name,
            value: Some(value),
            visibility: Visibility::File,
        }))
    }

    fn parse_expression_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.implicit_call_ok = true;
        let expr = self.parse_expression(LOWEST);
        self.implicit_call_ok = false;
        let expr = expr?;

        if self.cur.type_ == TokenType::Assign {
            if let ast::Expr::Member(member) = expr {
                self.next();
                let value = self.parse_expression(LOWEST)?;
                return Some(Stmt::FieldAssign(ast::FieldAssignExpr {
                    pos,
                    object: *member.object,
                    name: member.name,
                    value,
                }));
            }
        }
        if let Some(op) = compound_assign_op(self.cur.type_) {
            if !assignable_expr(&expr) {
                let t = self.cur.clone();
                self.add_error(&t, "复合赋值左侧必须是变量、下标或字段");
                return None;
            }
            self.next();
            let value = self.parse_expression(LOWEST)?;
            return Some(Stmt::CompoundAssign(ast::CompoundAssignStmt {
                pos,
                target: expr,
                op: op.to_string(),
                value,
            }));
        }
        Some(Stmt::Expr(ast::ExpressionStmt { pos, expr }))
    }

    fn parse_index_assign_or_expr(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        let expr = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::Assign {
            if let Some(op) = compound_assign_op(self.cur.type_) {
                self.next();
                let value = self.parse_expression(LOWEST)?;
                return Some(Stmt::CompoundAssign(ast::CompoundAssignStmt {
                    pos,
                    target: expr,
                    op: op.to_string(),
                    value,
                }));
            }
            return Some(Stmt::Expr(ast::ExpressionStmt { pos, expr }));
        }
        self.next();
        let value = self.parse_expression(LOWEST)?;
        match expr {
            ast::Expr::Index(idx) => Some(Stmt::IndexAssign(ast::IndexAssignStmt {
                pos,
                target: *idx.target,
                index: *idx.index,
                value,
            })),
            other => Some(Stmt::Expr(ast::ExpressionStmt { pos, expr: other })),
        }
    }

    fn parse_return_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if matches!(
            self.cur.type_,
            TokenType::Newline | TokenType::Eof | TokenType::End
        ) {
            return Some(Stmt::Return(ast::ReturnStmt {
                pos,
                values: Vec::new(),
            }));
        }
        let values = self.parse_expression_list()?;
        Some(Stmt::Return(ast::ReturnStmt { pos, values }))
    }

    fn parse_defer_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        let expr = self.parse_expression(LOWEST).or_else(|| {
            let t = self.cur.clone();
            self.add_error(&t, "稍后 后需要函数调用或无参函数");
            None
        })?;
        match expr {
            ast::Expr::Call(_) | ast::Expr::Function(_) => {
                Some(Stmt::Defer(ast::DeferStmt { pos, call: expr }))
            }
            _ => {
                let t = self.cur.clone();
                self.add_error(&t, "稍后 后必须是函数调用或无参函数");
                None
            }
        }
    }

    fn parse_throw_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ == TokenType::Newline || self.cur.type_ == TokenType::Eof {
            let t = self.cur.clone();
            self.add_error(&t, "抛出 后需要一个值");
            return None;
        }
        let value = self.parse_expression(LOWEST).or_else(|| {
            let t = self.cur.clone();
            self.add_error(&t, "抛出 后需要一个值");
            None
        })?;
        Some(Stmt::Throw(ast::ThrowStmt { pos, value }))
    }

    fn parse_import_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Str {
            let t = self.cur.clone();
            self.add_error(&t, "导入 后应为字符串路径");
            return None;
        }
        let path = self.cur.literal.clone();
        self.next();
        let mut name = String::new();
        if self.cur.type_ == TokenType::Ident && self.cur.literal == "作为" {
            self.next();
            if self.cur.type_ != TokenType::Ident {
                let t = self.cur.clone();
                self.add_error(&t, "作为 后应为模块名");
                return None;
            }
            name = self.cur.literal.clone();
            self.next();
        }
        if name.is_empty() {
            name = module_base_name(&path).to_string();
        }
        Some(Stmt::Import(ast::ImportStmt { pos, path, name }))
    }

    fn parse_visibility_statement(&mut self, vis: Visibility) -> Option<Stmt> {
        let pos = self.cur_pos();
        let kw = self.cur.literal.clone();
        self.next();
        let mut inner = match self.cur.type_ {
            TokenType::Function => self.parse_function_statement(),
            TokenType::Class => self.parse_class_statement(),
            TokenType::Interface => self.parse_interface_statement(),
            TokenType::TypeDef => self.parse_type_def_statement(),
            TokenType::Ident if self.peek.type_ == TokenType::Member => {
                self.parse_method_definition()
            }
            TokenType::Define => {
                self.next();
                match self.cur.type_ {
                    TokenType::Function => self.parse_function_statement(),
                    TokenType::Class => self.parse_class_statement(),
                    TokenType::Interface => self.parse_interface_statement(),
                    TokenType::TypeDef => self.parse_type_def_statement(),
                    TokenType::Ident if self.peek.type_ == TokenType::Member => {
                        self.parse_method_definition()
                    }
                    _ => self.parse_var_decl(),
                }
            }
            TokenType::Var | TokenType::Const => self.parse_var_decl(),
            _ => {
                let t = self.cur.clone();
                self.add_error(
                    &t,
                    format_args!("{kw} 后应为 变量/常量/函数/模型/接口/类型/方法定义"),
                );
                return None;
            }
        }?;
        apply_visibility(&mut inner, vis);
        let name = declared_name(&inner);
        if name.is_empty() {
            let t = self.cur.clone();
            self.add_error(&t, format_args!("{kw} 语句必须带名字"));
            return None;
        }
        Some(Stmt::Visibility(ast::VisibilityStmt {
            pos,
            name,
            visibility: vis,
            inner: Box::new(inner),
        }))
    }

    // ------------------------------------------------------------ 函数

    fn parse_function_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if !is_function_name_token(self.cur.type_) {
            let t = self.cur.clone();
            self.add_error(&t, "函数声明需要名字");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();
        let mut params: Vec<ast::Parameter> = Vec::new();
        let mut segments = vec![name.clone()];
        let mut connectors = vec![String::new()];
        let mut group_sizes: Vec<usize> = if self.cur.type_ == TokenType::LParen {
            params = self.parse_param_list()?;
            vec![params.len()]
        } else {
            vec![0]
        };

        // 分段函数：根(a)的段(b)，各段紧邻。
        while (self.cur.type_ == TokenType::Member && self.peek.type_ == TokenType::Ident)
            || self.cur.type_ == TokenType::Ident
        {
            let mut connector = String::new();
            if self.cur.type_ == TokenType::Member {
                connector = self.cur.literal.clone();
                self.next();
            }
            let segment_name = self.cur.literal.clone();
            self.next();
            let mut next_params: Vec<ast::Parameter> = Vec::new();
            if self.cur.type_ == TokenType::LParen {
                next_params = self.parse_param_list()?;
            } else if !is_function_segment_boundary(self.cur.type_) {
                let t = self.cur.clone();
                self.add_error(&t, "分段函数每段后应有参数列表或结束");
                return None;
            }
            segments.push(segment_name);
            connectors.push(connector);
            group_sizes.push(next_params.len());
            params.extend(next_params);
        }

        let mut full_name = String::new();
        for (i, segment) in segments.iter().enumerate() {
            full_name.push_str(&connectors[i]);
            full_name.push_str(segment);
        }
        if segments.len() > 1 {
            if let Err(reason) = validate_mixfix_params(&segments, &group_sizes, &params) {
                self.add_error_at(pos, reason);
                return None;
            }
        }
        let return_types = self.parse_return_types()?;
        let body = self.parse_function_body()?;
        let mut literal = ast::FunctionLit {
            pos,
            params,
            body,
            name: full_name.clone(),
            name_segments: Vec::new(),
            connectors: Vec::new(),
            group_sizes: Vec::new(),
            signature: String::new(),
            return_types,
            tags: Vec::new(),
        };
        if segments.len() > 1 {
            literal.name_segments = segments.clone();
            literal.connectors = connectors.clone();
            literal.group_sizes = group_sizes.clone();
            literal.signature =
                build_mixfix_signature(&segments, &connectors, &group_sizes, &literal.params);
        }
        Some(Stmt::VarDecl(ast::VarDecl {
            pos,
            is_const: false,
            is_explicit: false,
            decl_type: DeclType::any(),
            name: full_name,
            value: Some(ast::Expr::Function(literal)),
            visibility: Visibility::File,
        }))
    }

    fn parse_function_literal(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        self.next();
        let params = self.parse_param_list()?;
        let return_types = self.parse_return_types()?;
        let body = self.parse_function_body()?;
        Some(ast::Expr::Function(ast::FunctionLit {
            pos,
            params,
            body,
            name: String::new(),
            name_segments: Vec::new(),
            connectors: Vec::new(),
            group_sizes: Vec::new(),
            signature: String::new(),
            return_types,
            tags: Vec::new(),
        }))
    }

    fn parse_param_list(&mut self) -> Option<Vec<ast::Parameter>> {
        let mut params: Vec<ast::Parameter> = Vec::new();
        if self.cur.type_ != TokenType::LParen {
            let t = self.cur.clone();
            self.add_error(&t, "参数列表应以 ( 开头");
            return None;
        }
        self.paren_depth += 1;
        self.next();
        let mut saw_default = false;
        loop {
            if self.cur.type_ == TokenType::RParen {
                self.paren_depth -= 1;
                self.next();
                return Some(params);
            }
            // 新语序：类型在前、名字在后，且类型必须显式声明。
            if !is_type_position_token(self.cur.type_) {
                let t = self.cur.clone();
                if t.type_ == TokenType::Ident {
                    self.add_error(
                        &t,
                        "参数须写作「类型 名字」（类型在前、名字在后），且必须显式声明类型",
                    );
                } else {
                    self.add_error(&t, "参数应以类型名开头（类型在前、名字在后）");
                }
                self.paren_depth -= 1;
                return None;
            }
            let dt = self.parse_type_union();
            if dt.is_union() {
                let t = self.cur.clone();
                self.add_error(
                    &t,
                    "参数不允许匿名联合类型，请先用「定义 类型」为联合起别名",
                );
                self.paren_depth -= 1;
                return None;
            }
            // 可变参数：类型后紧跟 ...，再写名字，如 `整数... 数字们`。
            let mut variadic = false;
            if self.cur.type_ == TokenType::Ellipsis {
                variadic = true;
                self.next();
            }
            if self.cur.type_ != TokenType::Ident && self.cur.type_ != TokenType::Self_ {
                let t = self.cur.clone();
                self.add_error(&t, "类型后应为参数名（标识符）");
                self.paren_depth -= 1;
                return None;
            }
            let name = self.cur.literal.clone();
            self.next();
            let mut default = None;
            if !variadic && self.cur.type_ == TokenType::Assign {
                saw_default = true;
                self.next();
                let d = match self.parse_expression(LOWEST) {
                    Some(d) => d,
                    None => {
                        self.paren_depth -= 1;
                        return None;
                    }
                };
                if let Err(reason) = check_default_inferable(&dt, &d) {
                    let t = self.cur.clone();
                    self.add_error(&t, reason);
                    self.paren_depth -= 1;
                    return None;
                }
                default = Some(d);
            } else if saw_default && !variadic {
                let t = self.cur.clone();
                self.add_error(&t, "默认参数后面不能再出现普通参数");
                self.paren_depth -= 1;
                return None;
            }
            params.push(ast::Parameter {
                name,
                type_: dt,
                variadic,
                default,
            });
            if variadic {
                if self.cur.type_ == TokenType::Comma {
                    let t = self.cur.clone();
                    self.add_error(&t, "可变参数必须是最后一个参数");
                    self.paren_depth -= 1;
                    return None;
                }
                if self.cur.type_ != TokenType::RParen {
                    let t = self.cur.clone();
                    self.add_error(&t, "可变参数必须是最后一个参数");
                    self.paren_depth -= 1;
                    return None;
                }
                self.paren_depth -= 1;
                self.next();
                return Some(params);
            }
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::RParen {
                self.paren_depth -= 1;
                self.next();
                return Some(params);
            }
            let t = self.cur.clone();
            self.add_error(&t, "参数列表应以 , 分隔或以 ) 结束");
            self.paren_depth -= 1;
            return None;
        }
    }

    fn parse_return_types(&mut self) -> Option<Vec<DeclType>> {
        if self.cur.type_ != TokenType::Arrow {
            return Some(Vec::new());
        }
        self.next();
        if self.cur.type_ == TokenType::LParen {
            self.paren_depth += 1;
            self.next();
            let mut types: Vec<DeclType> = Vec::new();
            loop {
                if self.cur.type_ == TokenType::RParen {
                    self.paren_depth -= 1;
                    self.next();
                    return Some(types);
                }
                if !is_type_position_token(self.cur.type_) {
                    let t = self.cur.clone();
                    self.add_error(&t, "括号内应为返回类型");
                    self.paren_depth -= 1;
                    return None;
                }
                types.push(self.parse_type_union());
                if self.cur.type_ == TokenType::Comma {
                    self.next();
                    continue;
                }
                if self.cur.type_ == TokenType::RParen {
                    self.paren_depth -= 1;
                    self.next();
                    return Some(types);
                }
                let t = self.cur.clone();
                self.add_error(&t, "返回类型应以 , 分隔或以 ) 结束");
                self.paren_depth -= 1;
                return None;
            }
        }
        if !is_type_position_token(self.cur.type_) {
            let t = self.cur.clone();
            self.add_error(&t, "-> 后应为返回类型");
            return None;
        }
        Some(vec![self.parse_type_union()])
    }

    fn parse_function_body(&mut self) -> Option<ast::BlockStmt> {
        let pos = self.cur_pos();
        if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
            let t = self.cur.clone();
            self.add_error(&t, "函数体应另起一行");
            return None;
        }
        self.skip_statement_end();
        let mut block = ast::BlockStmt {
            pos,
            statements: Vec::new(),
        };
        loop {
            match self.cur.type_ {
                TokenType::End => {
                    self.next();
                    return Some(block);
                }
                TokenType::Eof => {
                    let t = self.cur.clone();
                    self.add_error(&t, "函数缺少 结束");
                    return None;
                }
                TokenType::Illegal => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("非法字符：{}", t.literal));
                    return None;
                }
                _ => {}
            }
            let stmt = self.parse_statement()?;
            block.statements.push(stmt);
            if !matches!(
                self.cur.type_,
                TokenType::Newline | TokenType::End | TokenType::Eof
            ) {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("语句后应换行，实际是 {:?}", t.literal));
                return None;
            }
            self.skip_statement_end();
        }
    }

    // ------------------------------------------------------------ 模型 / 接口 / 类型

    fn parse_class_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "模型 需要名字");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();
        let mut cls = ast::ClassStmt {
            pos,
            name,
            fields: Vec::new(),
            embeds: Vec::new(),
            tags: Vec::new(),
            visibility: Visibility::File,
        };
        if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
            let t = self.cur.clone();
            self.add_error(&t, "模型名后应换行");
            return None;
        }
        self.skip_statement_end();
        loop {
            match self.cur.type_ {
                TokenType::End => {
                    self.next();
                    return Some(Stmt::Class(cls));
                }
                TokenType::Eof => {
                    let t = self.cur.clone();
                    self.add_error(&t, "模型缺少 结束");
                    return None;
                }
                TokenType::Illegal => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("非法字符：{}", t.literal));
                    return None;
                }
                TokenType::Property | TokenType::Public | TokenType::Private => {
                    let mut field_vis = Visibility::File;
                    if self.cur.type_ == TokenType::Public || self.cur.type_ == TokenType::Private {
                        field_vis = if self.cur.type_ == TokenType::Public {
                            Visibility::Public
                        } else {
                            Visibility::Private
                        };
                        self.next();
                        if self.cur.type_ != TokenType::Property {
                            let t = self.cur.clone();
                            self.add_error(&t, "可见性修饰后应为 属性 字段声明");
                            return None;
                        }
                    }
                    self.next();
                    let dt = if is_type_token(self.cur.type_)
                        || ((self.cur.type_ == TokenType::Class
                            || self.cur.type_ == TokenType::Ident)
                            && (self.peek.type_ == TokenType::Ident
                                || is_union_sep_token(self.peek.type_)))
                    {
                        self.parse_type_union()
                    } else {
                        DeclType::any()
                    };
                    if self.cur.type_ != TokenType::Ident {
                        let t = self.cur.clone();
                        self.add_error(&t, "字段名应为标识符");
                        return None;
                    }
                    let name_tok = self.cur.clone();
                    self.next();
                    cls.fields.push(ast::FieldDecl {
                        pos: Position::new(name_tok.line, name_tok.column),
                        name: name_tok.literal,
                        type_: dt,
                        visibility: field_vis,
                    });
                    if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
                        let t = self.cur.clone();
                        self.add_error(&t, "字段声明应换行结束");
                        return None;
                    }
                    self.skip_statement_end();
                }
                TokenType::Embed => {
                    self.next();
                    if self.cur.type_ != TokenType::Ident {
                        let t = self.cur.clone();
                        self.add_error(&t, "嵌入需要类型名");
                        return None;
                    }
                    cls.embeds.push(self.cur.literal.clone());
                    self.next();
                    if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
                        let t = self.cur.clone();
                        self.add_error(&t, "嵌入声明应换行结束");
                        return None;
                    }
                    self.skip_statement_end();
                }
                TokenType::Var => {
                    let t = self.cur.clone();
                    self.add_error(
                        &t,
                        "模型体内字段须用 属性 声明（变量 关键字已移除，无兼容期）",
                    );
                    return None;
                }
                _ => {
                    let t = self.cur.clone();
                    self.add_error(
                        &t,
                        format_args!("模型内只能包含 字段/嵌入，实际：{:?}", t.literal),
                    );
                    return None;
                }
            }
        }
    }

    fn parse_type_def_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "类型 声明需要名字");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();
        if self.cur.type_ == TokenType::Assign {
            self.next();
        }
        if !is_type_position_token(self.cur.type_) {
            let t = self.cur.clone();
            self.add_error(
                &t,
                format_args!("类型 {name} 需要底层类型（基本类型或其联合）"),
            );
            return None;
        }
        let base_type = self.parse_type_union();
        // 类型别名的底层只允许内置基础类型或其联合；模型名、接口名、其他
        // 自定义类型名一律拒绝，避免别名与模型/接口的名义类型产生歧义。
        let illegal: Vec<&str> = base_type
            .members()
            .into_iter()
            .filter(|m| !DeclType::named(*m).is_builtin())
            .collect();
        if !illegal.is_empty() {
            let t = self.cur.clone();
            self.add_error(
                &t,
                format_args!(
                    "类型 {name} 的底层类型只能是基本类型或其联合，不允许模型/接口名或其他自定义类型：{}",
                    illegal.join("、")
                ),
            );
            return None;
        }
        if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
            let t = self.cur.clone();
            self.add_error(&t, "类型声明后应换行");
            return None;
        }
        Some(Stmt::TypeDef(ast::TypeDefStmt {
            pos,
            name,
            base_type,
            visibility: Visibility::File,
        }))
    }

    fn parse_interface_statement(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "接口 需要名字");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();
        let mut iface = ast::InterfaceStmt {
            pos,
            name,
            embeds: Vec::new(),
            methods: Vec::new(),
            visibility: Visibility::File,
        };
        if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
            let t = self.cur.clone();
            self.add_error(&t, "接口名后应换行");
            return None;
        }
        self.skip_statement_end();
        loop {
            match self.cur.type_ {
                TokenType::End => {
                    self.next();
                    return Some(Stmt::Interface(iface));
                }
                TokenType::Eof => {
                    let t = self.cur.clone();
                    self.add_error(&t, "接口缺少 结束");
                    return None;
                }
                TokenType::Illegal => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("非法字符：{}", t.literal));
                    return None;
                }
                TokenType::Embed => {
                    self.next();
                    if self.cur.type_ != TokenType::Ident {
                        let t = self.cur.clone();
                        self.add_error(&t, "嵌入需要接口名");
                        return None;
                    }
                    iface.embeds.push(self.cur.literal.clone());
                    self.next();
                    if self.cur.type_ != TokenType::Newline && self.cur.type_ != TokenType::Eof {
                        let t = self.cur.clone();
                        self.add_error(&t, "嵌入声明应换行结束");
                        return None;
                    }
                    self.skip_statement_end();
                }
                TokenType::Method => {
                    let sig = self.parse_interface_method()?;
                    iface.methods.push(sig);
                    self.skip_statement_end();
                }
                _ => {
                    let t = self.cur.clone();
                    self.add_error(
                        &t,
                        format_args!("接口内只能包含 方法签名/嵌入，实际：{:?}", t.literal),
                    );
                    return None;
                }
            }
        }
    }

    fn parse_interface_method(&mut self) -> Option<ast::InterfaceMethodSig> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "方法需要名字");
            return None;
        }
        let name = self.cur.literal.clone();
        self.next();
        let mut params = Vec::new();
        if self.cur.type_ == TokenType::LParen {
            params = self.parse_param_list()?;
        }
        let return_types = self.parse_return_types()?;
        if !matches!(
            self.cur.type_,
            TokenType::Newline | TokenType::Eof | TokenType::End
        ) {
            let t = self.cur.clone();
            self.add_error(&t, "接口方法签名后应换行");
            return None;
        }
        Some(ast::InterfaceMethodSig {
            name,
            params,
            return_types,
            pos,
        })
    }

    fn parse_ident_member_statement(&mut self) -> Option<Stmt> {
        if self.peek2.type_ == TokenType::Method
            || (matches!(self.peek2.type_, TokenType::Public | TokenType::Private)
                && self.peek3.type_ == TokenType::Method)
        {
            return self.parse_method_definition();
        }
        self.parse_expression_statement()
    }

    fn parse_method_definition(&mut self) -> Option<Stmt> {
        let pos = self.cur_pos();
        let mut receiver_name = "自己".to_string();
        let mut class_name = self.cur.literal.clone();
        let mut primitive = is_type_token(self.cur.type_);
        if primitive && self.peek.type_ != TokenType::Member {
            return None;
        }
        if !primitive && is_type_token(self.peek.type_) && self.peek2.type_ == TokenType::Member {
            receiver_name = self.cur.literal.clone();
            self.next();
            class_name = self.cur.literal.clone();
            primitive = true;
        }
        self.next();
        if self.cur.type_ != TokenType::Member {
            let t = self.cur.clone();
            self.add_error(&t, "方法定义应为：定义 类型 的 方法 …");
            return None;
        }
        self.next();
        let mut vis = Visibility::File;
        if matches!(self.cur.type_, TokenType::Public | TokenType::Private) {
            vis = if self.cur.type_ == TokenType::Public {
                Visibility::Public
            } else {
                Visibility::Private
            };
            self.next();
        }
        if self.cur.type_ != TokenType::Method {
            let t = self.cur.clone();
            self.add_error(&t, "类型成员定义应使用 方法 关键字");
            return None;
        }
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "方法需要名字");
            return None;
        }
        let method_name = self.cur.literal.clone();
        self.next();
        let params = self.parse_param_list()?;
        let return_types = self.parse_return_types()?;
        let body = self.parse_function_body()?;
        Some(Stmt::MethodDef(ast::MethodDef {
            pos,
            class_name,
            method_name: method_name.clone(),
            function: ast::FunctionLit {
                pos,
                params,
                body,
                name: method_name,
                name_segments: Vec::new(),
                connectors: Vec::new(),
                group_sizes: Vec::new(),
                signature: String::new(),
                return_types,
                tags: Vec::new(),
            },
            primitive,
            receiver_name,
            visibility: vis,
        }))
    }

    // ------------------------------------------------------------ 标签

    fn parse_tagged_statement(&mut self) -> Option<Stmt> {
        let mut tags: Vec<ast::TagExpr> = Vec::new();
        while self.cur.type_ == TokenType::Tag {
            if let Some(tag) = self.parse_tag_expr() {
                tags.push(tag);
            }
            // 标签独占一行时，跳到被修饰的声明处。
            self.skip_statement_end();
        }
        let inner = match self.cur.type_ {
            TokenType::Function => self.parse_function_statement(),
            TokenType::Class => self.parse_class_statement(),
            TokenType::Public => self.parse_visibility_statement(Visibility::Public),
            TokenType::Private => self.parse_visibility_statement(Visibility::Private),
            TokenType::Define => {
                self.next();
                match self.cur.type_ {
                    TokenType::Function => self.parse_function_statement(),
                    TokenType::Class => self.parse_class_statement(),
                    TokenType::Interface => self.parse_interface_statement(),
                    TokenType::TypeDef => self.parse_type_def_statement(),
                    TokenType::Public => self.parse_visibility_statement(Visibility::Public),
                    TokenType::Private => self.parse_visibility_statement(Visibility::Private),
                    _ => self.parse_var_decl(),
                }
            }
            TokenType::Var | TokenType::Const => self.parse_var_decl(),
            _ => {
                let t = self.cur.clone();
                self.add_error(&t, "标签后应为 函数/模型/变量 声明");
                return None;
            }
        }?;
        Some(attach_tags(inner, tags))
    }

    /// 解析 `@标记(位置参数, 命名 = 值)`。
    fn parse_tag_expr(&mut self) -> Option<ast::TagExpr> {
        let pos = self.cur_pos();
        self.next(); // 消费 @
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "标签需要类型名");
            return None;
        }
        let type_name = self.cur.literal.clone();
        self.next();
        let mut positional: Vec<ast::Expr> = Vec::new();
        let mut named: Vec<ast::NamedTagArg> = Vec::new();
        if self.cur.type_ == TokenType::LParen {
            self.paren_depth += 1;
            self.next();
            loop {
                if self.cur.type_ == TokenType::RParen {
                    self.paren_depth -= 1;
                    self.next();
                    break;
                }
                // 命名参数：标识符 = 表达式
                if self.cur.type_ == TokenType::Ident && self.peek.type_ == TokenType::Assign {
                    let arg_name = self.cur.literal.clone();
                    self.next();
                    self.next();
                    let value = match self.parse_expression(LOWEST) {
                        Some(v) => v,
                        None => {
                            self.paren_depth -= 1;
                            return None;
                        }
                    };
                    named.push(ast::NamedTagArg {
                        name: arg_name,
                        value,
                    });
                } else {
                    let value = match self.parse_expression(LOWEST) {
                        Some(v) => v,
                        None => {
                            self.paren_depth -= 1;
                            return None;
                        }
                    };
                    positional.push(value);
                }
                if self.cur.type_ == TokenType::Comma {
                    self.next();
                    continue;
                }
                if self.cur.type_ == TokenType::RParen {
                    self.paren_depth -= 1;
                    self.next();
                    break;
                }
                let t = self.cur.clone();
                self.add_error(&t, "标签参数应以 , 分隔或以 ) 结束");
                self.paren_depth -= 1;
                return None;
            }
        }
        Some(ast::TagExpr {
            pos,
            type_name,
            positional,
            named,
        })
    }

    // ------------------------------------------------------------ 表达式

    fn parse_expression(&mut self, prec: Precedence) -> Option<ast::Expr> {
        let mut left = self.parse_prefix()?;
        left = self.parse_postfix(left)?;
        loop {
            if matches!(
                self.cur.type_,
                TokenType::Newline
                    | TokenType::Eof
                    | TokenType::Comma
                    | TokenType::RParen
                    | TokenType::RBrack
                    | TokenType::End
                    | TokenType::Then
                    | TokenType::Else
                    | TokenType::ElseIf
                    | TokenType::Loop
                    | TokenType::Times
                    | TokenType::Until
                    | TokenType::InOf
                    | TokenType::Colon
            ) {
                if self.implicit_call_ok && is_implicit_call_end(self.cur.type_) {
                    let allow_identifier =
                        self.cur.type_ == TokenType::Newline || self.cur.type_ == TokenType::Eof;
                    match &left {
                        ast::Expr::Ident(_) if allow_identifier => {
                            return Some(ast::Expr::MaybeCall(ast::MaybeCallExpr {
                                pos: left.pos(),
                                callee: Box::new(left),
                                auto_call: true,
                            }));
                        }
                        ast::Expr::Member(_) => {
                            return Some(ast::Expr::MaybeCall(ast::MaybeCallExpr {
                                pos: left.pos(),
                                callee: Box::new(left),
                                auto_call: true,
                            }));
                        }
                        _ => {}
                    }
                }
                return Some(left);
            }
            if self.cur.type_ == TokenType::Member || self.cur.type_ == TokenType::Dot {
                left = self.parse_member(left)?;
                continue;
            }
            if self.implicit_call_ok
                && (self.cur.type_ == TokenType::Ident
                    || is_implicit_call_continuation(self.cur.type_))
                && self.current_precedence() == LOWEST
            {
                left = self.parse_implicit_call(left)?;
                continue;
            }
            if prec >= self.current_precedence() {
                return Some(left);
            }
            left = self.parse_infix(left)?;
        }
    }

    fn parse_implicit_call(&mut self, head: ast::Expr) -> Option<ast::Expr> {
        if self.cur.type_ != TokenType::Ident {
            return self.parse_implicit_function_args(head);
        }
        if self.peek.type_ == TokenType::Member || self.peek.type_ == TokenType::Dot {
            return self.parse_implicit_function_args(head);
        }
        let name = self.cur.literal.clone();
        let pos = self.cur_pos();
        let spaced = self.cur.leading_space;
        self.next();
        let args = if self.cur.type_ == TokenType::LParen {
            self.parse_args()?
        } else {
            self.parse_implicit_args()?
        };
        self.parse_postfix(ast::Expr::ImplicitReceiverCall(
            ast::ImplicitReceiverCallExpr {
                pos,
                receiver: Box::new(head),
                name,
                args,
                connector: String::new(),
                spaced,
            },
        ))
    }

    fn parse_implicit_function_args(&mut self, head: ast::Expr) -> Option<ast::Expr> {
        let args = self.parse_implicit_args()?;
        if args.is_empty() {
            return Some(head);
        }
        let pos = head.pos();
        match head {
            ast::Expr::Ident(callee) => Some(ast::Expr::Call(ast::CallExpr {
                pos,
                callee: Box::new(ast::Expr::Ident(callee)),
                args,
                mixfix_root: None,
            })),
            _ => {
                let t = self.cur.clone();
                self.add_error(&t, "无括号调用必须以函数名开头");
                None
            }
        }
    }

    fn parse_implicit_args(&mut self) -> Option<Vec<ast::Expr>> {
        let mut args: Vec<ast::Expr> = Vec::new();
        self.implicit_call_ok = false;
        loop {
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if matches!(
                self.cur.type_,
                TokenType::Newline
                    | TokenType::Eof
                    | TokenType::RParen
                    | TokenType::RBrack
                    | TokenType::End
                    | TokenType::Then
                    | TokenType::Else
                    | TokenType::ElseIf
                    | TokenType::InOf
                    | TokenType::Loop
                    | TokenType::Times
                    | TokenType::Until
            ) {
                self.implicit_call_ok = true;
                return Some(args);
            }
            let arg = match self.parse_expression(LOWEST) {
                Some(a) => a,
                None => {
                    self.implicit_call_ok = true;
                    return None;
                }
            };
            args.push(arg);
        }
    }

    fn parse_postfix(&mut self, mut left: ast::Expr) -> Option<ast::Expr> {
        loop {
            match self.cur.type_ {
                TokenType::LBrack => {
                    let pos = self.cur_pos();
                    self.paren_depth += 1;
                    self.next();
                    if self.cur.type_ == TokenType::Colon {
                        let colon_pos = self.cur_pos();
                        self.next();
                        let mut high = None;
                        if self.cur.type_ != TokenType::RBrack {
                            high = Some(Box::new(match self.parse_expression(LOWEST) {
                                Some(e) => e,
                                None => {
                                    self.paren_depth -= 1;
                                    return None;
                                }
                            }));
                        }
                        if self.cur.type_ != TokenType::RBrack {
                            let t = self.cur.clone();
                            self.add_error(&t, "切片缺少右方括号 ]");
                            self.paren_depth -= 1;
                            return None;
                        }
                        self.paren_depth -= 1;
                        self.next();
                        left = ast::Expr::Slice(ast::SliceExpr {
                            pos,
                            target: Box::new(left),
                            low: None,
                            high,
                            colon_pos,
                        });
                        continue;
                    }
                    let idx = match self.parse_expression(LOWEST) {
                        Some(e) => e,
                        None => {
                            self.paren_depth -= 1;
                            return None;
                        }
                    };
                    if self.cur.type_ == TokenType::Colon {
                        let colon_pos = self.cur_pos();
                        self.next();
                        let mut high = None;
                        if self.cur.type_ != TokenType::RBrack {
                            high = Some(Box::new(match self.parse_expression(LOWEST) {
                                Some(e) => e,
                                None => {
                                    self.paren_depth -= 1;
                                    return None;
                                }
                            }));
                        }
                        if self.cur.type_ != TokenType::RBrack {
                            let t = self.cur.clone();
                            self.add_error(&t, "切片缺少右方括号 ]");
                            self.paren_depth -= 1;
                            return None;
                        }
                        self.paren_depth -= 1;
                        self.next();
                        left = ast::Expr::Slice(ast::SliceExpr {
                            pos,
                            target: Box::new(left),
                            low: Some(Box::new(idx)),
                            high,
                            colon_pos,
                        });
                        continue;
                    }
                    if self.cur.type_ != TokenType::RBrack {
                        let t = self.cur.clone();
                        self.add_error(&t, "缺少右方括号 ]");
                        self.paren_depth -= 1;
                        return None;
                    }
                    self.paren_depth -= 1;
                    self.next();
                    left = ast::Expr::Index(ast::IndexExpr {
                        pos,
                        target: Box::new(left),
                        index: Box::new(idx),
                    });
                }
                TokenType::LParen => {
                    let pos = self.cur_pos();
                    let args = self.parse_args()?;
                    let call = ast::Expr::Call(ast::CallExpr {
                        pos,
                        callee: Box::new(left),
                        args,
                        mixfix_root: None,
                    });
                    left = self.parse_segmented_tail(call, pos)?;
                }
                TokenType::LBrace => {
                    // 模型复合字面量：{ 必须紧邻模型名，带空白时按字典字面量处理。
                    if !self.cur.leading_space && is_composite_model(&left) {
                        let node = self.parse_composite_literal(left)?;
                        left = node;
                        continue;
                    }
                    return Some(left);
                }
                TokenType::Inc => {
                    let pos = self.cur_pos();
                    left = ast::Expr::Update(ast::UpdateExpr {
                        pos,
                        target: Box::new(left),
                        op: "加".to_string(),
                        prefix: false,
                    });
                    self.next();
                }
                TokenType::Dec => {
                    let pos = self.cur_pos();
                    left = ast::Expr::Update(ast::UpdateExpr {
                        pos,
                        target: Box::new(left),
                        op: "减".to_string(),
                        prefix: false,
                    });
                    self.next();
                }
                _ => return Some(left),
            }
        }
    }

    fn parse_composite_literal(&mut self, model: ast::Expr) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        self.paren_depth += 1;
        self.next();
        let mut fields: Vec<ast::CompositeField> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        loop {
            if self.cur.type_ == TokenType::RBrace {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Composite(ast::CompositeLit {
                    pos,
                    model: Box::new(model),
                    fields,
                }));
            }
            if self.cur.type_ != TokenType::Ident {
                let t = self.cur.clone();
                self.add_error(&t, "复合字面量字段名应为裸标识符，形如 矩形{宽: 3}");
                self.paren_depth -= 1;
                return None;
            }
            let name_tok = self.cur.clone();
            if seen.contains(&name_tok.literal) {
                self.add_error(
                    &name_tok,
                    format_args!("复合字面量中字段 {} 重复指定", name_tok.literal),
                );
                self.paren_depth -= 1;
                return None;
            }
            seen.push(name_tok.literal.clone());
            self.next();
            if self.cur.type_ != TokenType::Colon {
                let t = self.cur.clone();
                self.add_error(&t, "复合字面量字段名后应为 :");
                self.paren_depth -= 1;
                return None;
            }
            self.next();
            let value = match self.parse_expression(LOWEST) {
                Some(v) => v,
                None => {
                    self.paren_depth -= 1;
                    return None;
                }
            };
            fields.push(ast::CompositeField {
                name: name_tok.literal,
                name_pos: Position::new(name_tok.line, name_tok.column),
                value: Some(value),
            });
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::RBrace {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Composite(ast::CompositeLit {
                    pos,
                    model: Box::new(model),
                    fields,
                }));
            }
            let t = self.cur.clone();
            self.add_error(&t, "复合字面量字段应以 , 分隔或以 } 结束");
            self.paren_depth -= 1;
            return None;
        }
    }

    fn parse_args(&mut self) -> Option<Vec<ast::Expr>> {
        self.paren_depth += 1;
        self.next();
        let mut args: Vec<ast::Expr> = Vec::new();
        loop {
            if self.cur.type_ == TokenType::RParen {
                self.paren_depth -= 1;
                self.next();
                return Some(args);
            }
            let arg = match self.parse_expression(LOWEST) {
                Some(a) => a,
                None => {
                    self.paren_depth -= 1;
                    return None;
                }
            };
            args.push(arg);
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::RParen {
                self.paren_depth -= 1;
                self.next();
                return Some(args);
            }
            let t = self.cur.clone();
            self.add_error(&t, "参数列表应以 , 分隔或以 ) 结束");
            self.paren_depth -= 1;
            return None;
        }
    }

    fn parse_member(&mut self, left: ast::Expr) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        let connector = self.cur.literal.clone();
        let spaced = self.cur.leading_space;
        self.next();
        if self.cur.type_ != TokenType::Ident
            && !is_type_token(self.cur.type_)
            && self.cur.type_ != TokenType::Self_
        {
            let t = self.cur.clone();
            self.add_error(&t, "的 后面需要成员名");
            return None;
        }
        let name = self.cur.literal.clone();
        let node = ast::Expr::Member(ast::MemberExpr {
            pos,
            object: Box::new(left),
            name,
            connector,
            spaced,
        });
        self.next();
        if self.cur.type_ == TokenType::LParen {
            let args = self.parse_args()?;
            let call = self.parse_segmented_tail(
                ast::Expr::Call(ast::CallExpr {
                    pos,
                    callee: Box::new(node),
                    args,
                    mixfix_root: None,
                }),
                pos,
            )?;
            return self.parse_postfix(call);
        }
        self.parse_postfix(node)
    }

    /// 续接分段调用的后续各段：`根(a)的段(b)`。
    ///
    /// 声明侧已把各段名拼成一个名字，这里要拼出同样的名字才能查到函数。
    /// 两种写法必须区分开：
    ///
    /// - `标记("b")的状态为无效` —— 段间无空白，是分段调用
    /// - `取结果("s") 的状态为无效` —— 段间有空白，是「先调用再取字段」
    ///
    /// 另有点号 `.` 恒为字段/方法访问，不构成新段，故 `造().说话()` 不受影响。
    fn parse_segmented_tail(&mut self, call: ast::Expr, pos: Position) -> Option<ast::Expr> {
        let ast::Expr::Call(mut node) = call else {
            return Some(call);
        };
        // 首段名来自被调用者：裸名字（`修改(...)`）或成员名（`工具.求(...)`）
        let root_name = match node.callee.as_ref() {
            ast::Expr::Ident(i) => i.name.clone(),
            ast::Expr::Member(m) => m.name.clone(),
            _ => return Some(ast::Expr::Call(node)),
        };
        let mut full = root_name.clone();
        let mut extra_args: Vec<ast::Expr> = Vec::new();
        let mut segments = 1usize;

        loop {
            let connector = if self.cur.type_ == TokenType::Member && !self.cur.leading_space {
                if self.peek.type_ != TokenType::Ident {
                    break;
                }
                let c = self.cur.literal.clone();
                self.next();
                c
            } else if self.cur.type_ == TokenType::Ident && !self.cur.leading_space {
                String::new()
            } else {
                break;
            };
            let name = self.cur.literal.clone();
            self.next();
            let has_paren = self.cur.type_ == TokenType::LParen && !self.cur.leading_space;
            let args = if has_paren {
                self.parse_args()?
            } else {
                Vec::new()
            };
            full.push_str(&connector);
            full.push_str(&name);
            extra_args.extend(args);
            segments += 1;
            // 空参数段只能是末段，其后不再接续
            if !has_paren {
                break;
            }
        }

        if segments == 1 {
            return Some(ast::Expr::Call(node));
        }
        let root = root_name.clone();
        node.args.extend(extra_args);
        match node.callee.as_mut() {
            ast::Expr::Ident(i) => i.name = full,
            ast::Expr::Member(m) => m.name = full,
            _ => unreachable!("首段名只可能来自标识符或成员"),
        }
        node.mixfix_root = Some(root);
        let _ = (pos, segments);
        Some(ast::Expr::Call(node))
    }

    fn parse_prefix(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        match self.cur.type_ {
            TokenType::Int => {
                let lit = self.cur.literal.clone();
                let value = if lit.len() >= 2 && lit.starts_with('0') {
                    let base = match lit.as_bytes()[1] {
                        b'b' | b'B' | b'o' | b'O' | b'x' | b'X' => 0,
                        _ => 10,
                    };
                    parse_int_with_base(&lit, base)
                } else {
                    lit.parse::<i64>().ok()
                };
                match value {
                    Some(v) => {
                        self.next();
                        Some(ast::Expr::Integer(ast::IntegerLit { pos, value: v }))
                    }
                    None => {
                        let t = self.cur.clone();
                        self.add_error(&t, format_args!("整数超出范围或格式错误：{lit}"));
                        None
                    }
                }
            }
            TokenType::Float => {
                let lit = self.cur.literal.clone();
                match lit.parse::<f64>() {
                    Ok(v) => {
                        self.next();
                        Some(ast::Expr::Float(ast::FloatLit { pos, value: v }))
                    }
                    Err(_) => {
                        let t = self.cur.clone();
                        self.add_error(&t, format_args!("小数格式错误：{lit}"));
                        None
                    }
                }
            }
            TokenType::Str => {
                let v = self.cur.literal.clone();
                self.next();
                Some(ast::Expr::Str(ast::StringLit { pos, value: v }))
            }
            TokenType::StringPart | TokenType::InterpIdent | TokenType::InterpStart => {
                self.parse_interp_string()
            }
            TokenType::Char => {
                let lit = self.cur.literal.clone();
                let chars: Vec<char> = lit.chars().collect();
                if chars.len() != 1 {
                    let t = self.cur.clone();
                    self.add_error(&t, "字符字面量必须包含恰好一个字符");
                    return None;
                }
                self.next();
                Some(ast::Expr::Char(ast::CharLit {
                    pos,
                    value: chars[0],
                }))
            }
            TokenType::True => {
                self.next();
                Some(ast::Expr::Bool(ast::BoolLit { pos, value: true }))
            }
            TokenType::False => {
                self.next();
                Some(ast::Expr::Bool(ast::BoolLit { pos, value: false }))
            }
            TokenType::Nil => {
                self.next();
                Some(ast::Expr::Nil(ast::NilLit { pos }))
            }
            TokenType::Check => self.parse_check_expression(),
            TokenType::Through => self.parse_through_expression(),
            TokenType::Ident | TokenType::Self_ => {
                if self.is_readability_prefix() {
                    self.next();
                    return self.parse_expression(LOWEST);
                }
                let name = self.cur.literal.clone();
                self.next();
                Some(ast::Expr::Ident(ast::Ident { pos, name }))
            }
            TokenType::TypeError | TokenType::TypeByte | TokenType::TypeChar => {
                let name = self.cur.literal.clone();
                self.next();
                Some(ast::Expr::Ident(ast::Ident { pos, name }))
            }
            TokenType::Inc | TokenType::Dec => {
                let op = if self.cur.type_ == TokenType::Inc {
                    "加"
                } else {
                    "减"
                };
                self.next();
                let implicit = self.implicit_call_ok;
                self.implicit_call_ok = false;
                let target = self.parse_expression(PREC_POSTFIX);
                self.implicit_call_ok = implicit;
                let target = target?;
                Some(ast::Expr::Update(ast::UpdateExpr {
                    pos,
                    target: Box::new(target),
                    op: op.to_string(),
                    prefix: true,
                }))
            }
            TokenType::Minus => {
                self.next();
                let right = self.parse_expression(PREC_PREFIX)?;
                Some(ast::Expr::Unary(ast::UnaryExpr {
                    pos,
                    op: "-".to_string(),
                    right: Box::new(right),
                }))
            }
            TokenType::Not | TokenType::BNot => {
                let op = self.cur.literal.clone();
                self.next();
                let right = self.parse_expression(PREC_PREFIX)?;
                Some(ast::Expr::Unary(ast::UnaryExpr {
                    pos,
                    op,
                    right: Box::new(right),
                }))
            }
            TokenType::LBrack => self.parse_array_literal(),
            TokenType::LBrace => self.parse_dict_literal(),
            TokenType::Ellipsis => {
                self.next();
                let value = match self.parse_expression(PREC_PREFIX) {
                    Some(v) => v,
                    None => {
                        let t = self.cur.clone();
                        self.add_error(&t, "展开 ... 后应为数组或字典表达式");
                        return None;
                    }
                };
                Some(ast::Expr::Spread(ast::SpreadExpr {
                    pos,
                    value: Box::new(value),
                }))
            }
            TokenType::Function => self.parse_function_literal(),
            TokenType::LParen => {
                self.paren_depth += 1;
                self.next();
                let expr = match self.parse_expression(LOWEST) {
                    Some(e) => e,
                    None => {
                        self.paren_depth -= 1;
                        return None;
                    }
                };
                if self.cur.type_ != TokenType::RParen {
                    let t = self.cur.clone();
                    self.add_error(&t, "缺少右括号 )");
                    self.paren_depth -= 1;
                    return None;
                }
                self.paren_depth -= 1;
                self.next();
                Some(expr)
            }
            _ => {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("此处不期望出现：{:?}", t.literal));
                None
            }
        }
    }

    fn parse_infix(&mut self, left: ast::Expr) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        if self.cur.type_ == TokenType::Quest {
            self.next();
            let then = self.parse_expression(LOWEST)?;
            if self.cur.type_ != TokenType::Colon {
                let t = self.cur.clone();
                self.add_error(&t, "三元表达式缺少 :");
                return None;
            }
            self.next();
            let else_ = self.parse_expression(LOWEST)?;
            return Some(ast::Expr::Ternary(ast::TernaryExpr {
                pos,
                cond: Box::new(left),
                then: Box::new(then),
                else_: Box::new(else_),
            }));
        }
        let op_tok = self.cur.clone();
        let pr = self.current_precedence();
        self.next();
        if op_tok.type_ == TokenType::Is || op_tok.type_ == TokenType::As {
            if !is_type_position_token(self.cur.type_) {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("{} 后面应为类型名", op_tok.literal));
                return None;
            }
            let type_name = self.cur.literal.clone();
            self.next();
            return Some(if op_tok.type_ == TokenType::Is {
                ast::Expr::TypeTest(ast::TypeTestExpr {
                    pos,
                    object: Box::new(left),
                    type_name,
                })
            } else {
                ast::Expr::TypeCast(ast::TypeCastExpr {
                    pos,
                    object: Box::new(left),
                    type_name,
                })
            });
        }
        let right = self.parse_expression(pr)?;
        Some(ast::Expr::Binary(ast::BinaryExpr {
            pos,
            op: op_tok.literal,
            left: Box::new(left),
            right: Box::new(right),
        }))
    }

    fn parse_check_expression(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        let check_tok = self.cur.clone();
        self.next();
        let call = match self.parse_expression(LOWEST) {
            Some(c) => c,
            None => {
                self.add_error(&check_tok, "检查 后应为函数调用表达式");
                return None;
            }
        };
        Some(ast::Expr::Check(ast::CheckExpr {
            pos,
            call: Box::new(call),
        }))
    }

    fn parse_through_expression(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        self.next();
        if self.cur.type_ != TokenType::Ident {
            let t = self.cur.clone();
            self.add_error(&t, "通过 后应为模块名");
            return None;
        }
        let module_name = self.cur.literal.clone();
        self.next();
        let expr = match self.parse_expression(LOWEST) {
            Some(e) => e,
            None => {
                let t = self.cur.clone();
                self.add_error(&t, format_args!("通过 {module_name} 后应为表达式"));
                return None;
            }
        };
        Some(ast::Expr::Through(ast::ThroughExpr {
            pos,
            module_name,
            expr: Box::new(expr),
        }))
    }

    fn parse_array_literal(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        self.paren_depth += 1;
        self.next();
        let mut elements: Vec<ast::Expr> = Vec::new();
        loop {
            if self.cur.type_ == TokenType::RBrack {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Array(ast::ArrayLit { pos, elements }));
            }
            let el = match self.parse_expression(LOWEST) {
                Some(e) => e,
                None => {
                    self.paren_depth -= 1;
                    return None;
                }
            };
            elements.push(el);
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::RBrack {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Array(ast::ArrayLit { pos, elements }));
            }
            let t = self.cur.clone();
            self.add_error(&t, "数组元素应以 , 分隔或以 ] 结束");
            self.paren_depth -= 1;
            return None;
        }
    }

    fn parse_dict_literal(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        self.paren_depth += 1;
        self.next();
        let mut pairs: Vec<ast::DictPair> = Vec::new();
        loop {
            if self.cur.type_ == TokenType::RBrace {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Dict(ast::DictLit { pos, pairs }));
            }
            if self.cur.type_ == TokenType::Ellipsis {
                self.next();
                let value = match self.parse_expression(LOWEST) {
                    Some(v) => v,
                    None => {
                        self.paren_depth -= 1;
                        return None;
                    }
                };
                pairs.push(ast::DictPair {
                    key: None,
                    value,
                    spread: true,
                });
            } else {
                let key = match self.parse_expression(LOWEST) {
                    Some(k) => k,
                    None => {
                        self.paren_depth -= 1;
                        return None;
                    }
                };
                if self.cur.type_ != TokenType::Colon {
                    let t = self.cur.clone();
                    self.add_error(&t, "字典项应以 : 分隔键和值");
                    self.paren_depth -= 1;
                    return None;
                }
                self.next();
                let value = match self.parse_expression(LOWEST) {
                    Some(v) => v,
                    None => {
                        self.paren_depth -= 1;
                        return None;
                    }
                };
                pairs.push(ast::DictPair {
                    key: Some(key),
                    value,
                    spread: false,
                });
            }
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            if self.cur.type_ == TokenType::RBrace {
                self.paren_depth -= 1;
                self.next();
                return Some(ast::Expr::Dict(ast::DictLit { pos, pairs }));
            }
            let t = self.cur.clone();
            self.add_error(&t, "字典项应以 , 分隔或以 } 结束");
            self.paren_depth -= 1;
            return None;
        }
    }

    /// 解析词法器切好的插值字符串记号序列。
    fn parse_interp_string(&mut self) -> Option<ast::Expr> {
        let pos = self.cur_pos();
        let mut parts: Vec<String> = Vec::new();
        let mut exprs: Vec<ast::Expr> = Vec::new();
        if self.cur.type_ == TokenType::StringPart {
            parts.push(self.cur.literal.clone());
            self.next();
        } else {
            parts.push(String::new());
        }
        loop {
            match self.cur.type_ {
                TokenType::StringPart => {
                    parts.push(self.cur.literal.clone());
                    self.next();
                }
                TokenType::InterpIdent => {
                    let p = self.cur_pos();
                    let name = self.cur.literal.clone();
                    exprs.push(ast::Expr::Ident(ast::Ident { pos: p, name }));
                    self.next();
                    if matches!(
                        self.cur.type_,
                        TokenType::InterpIdent | TokenType::InterpStart | TokenType::InterpEnd
                    ) {
                        parts.push(String::new());
                    }
                }
                TokenType::InterpStart => {
                    self.next();
                    let expr = match self.parse_expression(LOWEST) {
                        Some(e) => e,
                        None => {
                            let t = self.cur.clone();
                            self.add_error(&t, "插值表达式为空");
                            return None;
                        }
                    };
                    exprs.push(expr);
                    if self.cur.type_ != TokenType::InterpEnd {
                        let t = self.cur.clone();
                        self.add_error(&t, "插值表达式缺少右花括号 }");
                        return None;
                    }
                    self.next();
                    if matches!(
                        self.cur.type_,
                        TokenType::InterpIdent | TokenType::InterpStart | TokenType::InterpEnd
                    ) {
                        parts.push(String::new());
                    }
                }
                TokenType::InterpEnd => {
                    self.next();
                    if parts.len() == exprs.len() {
                        parts.push(String::new());
                    }
                    return Some(ast::Expr::InterpStr(ast::InterpString {
                        pos,
                        parts,
                        exprs,
                    }));
                }
                _ => {
                    let t = self.cur.clone();
                    self.add_error(&t, format_args!("插值字符串格式错误，意外的 {}", t.type_));
                    return None;
                }
            }
        }
    }

    /// 多目标声明/赋值右侧：逗号分隔时包装为元组，单个表达式直接返回。
    fn parse_rhs_list(&mut self, pos: Position) -> Option<ast::Expr> {
        let first = self.parse_expression(LOWEST)?;
        if self.cur.type_ != TokenType::Comma {
            return Some(first);
        }
        let mut elements = vec![first];
        while self.cur.type_ == TokenType::Comma {
            self.next();
            if self.cur.type_ == TokenType::Newline || self.cur.type_ == TokenType::Eof {
                let t = self.cur.clone();
                self.add_error(&t, "多目标赋值右侧逗号后缺少表达式");
                return None;
            }
            elements.push(self.parse_expression(LOWEST)?);
        }
        Some(ast::Expr::Tuple(ast::TupleExpr { pos, elements }))
    }

    fn parse_expression_list(&mut self) -> Option<Vec<ast::Expr>> {
        let mut exprs: Vec<ast::Expr> = Vec::new();
        loop {
            exprs.push(self.parse_expression(LOWEST)?);
            if self.cur.type_ == TokenType::Comma {
                self.next();
                continue;
            }
            return Some(exprs);
        }
    }

    /// “从 …” 可读性前缀：遇到 `从` 且后跟表达式起始记号时跳过该词。
    fn is_readability_prefix(&self) -> bool {
        if self.cur.type_ != TokenType::Ident || self.cur.literal != "从" {
            return false;
        }
        if matches!(
            self.peek.type_,
            TokenType::LParen
                | TokenType::Assign
                | TokenType::Comma
                | TokenType::Newline
                | TokenType::Eof
                | TokenType::RParen
                | TokenType::RBrack
                | TokenType::LBrack
        ) {
            return false;
        }
        matches!(
            self.peek.type_,
            TokenType::Ident
                | TokenType::Self_
                | TokenType::Int
                | TokenType::Float
                | TokenType::Str
                | TokenType::True
                | TokenType::False
                | TokenType::Nil
                | TokenType::LBrace
                | TokenType::Minus
                | TokenType::Not
                | TokenType::Function
                | TokenType::Member
                | TokenType::Dot
        )
    }

    // ------------------------------------------------------------ 类型辅助

    fn parse_single_type_name(&mut self) -> DeclType {
        match self.cur.type_ {
            _ if is_type_token(self.cur.type_) => {
                let dt = parse_decl_type(self.cur.type_);
                self.next();
                dt
            }
            TokenType::Nil => {
                self.next();
                DeclType::named("空")
            }
            TokenType::Class | TokenType::Ident => {
                let name = self.cur.literal.clone();
                self.next();
                // `任意` 是显式的动态类型占位（与名称修饰规则一致），解析为未标注类型。
                if name == "任意" {
                    DeclType::any()
                } else {
                    DeclType::named(name)
                }
            }
            _ => DeclType::any(),
        }
    }

    /// 在类型位置读取 “类型 (或/| 类型)*”，内部用 `|` 连接。
    fn parse_type_union(&mut self) -> DeclType {
        let first = self.parse_single_type_name();
        if first.is_any() {
            return DeclType::any();
        }
        let mut parts = vec![first.0.clone()];
        while is_union_sep_token(self.cur.type_) {
            self.next();
            if !is_type_position_token(self.cur.type_) {
                let t = self.cur.clone();
                self.add_error(&t, "联合类型 或 后应为类型名");
                return DeclType::union(&parts);
            }
            parts.push(self.parse_single_type_name().0);
        }
        DeclType::union(&parts)
    }
}

// ---------------------------------------------------------------- 自由函数

/// 解析带进制前缀的整数。`base` 为 0 时按 `0b/0o/0x` 自动识别。
fn parse_int_with_base(lit: &str, base: u32) -> Option<i64> {
    if base == 0 {
        let bytes = lit.as_bytes();
        if bytes.len() > 2 {
            return match bytes[1] {
                b'b' | b'B' => i64::from_str_radix(&lit[2..], 2).ok(),
                b'o' | b'O' => i64::from_str_radix(&lit[2..], 8).ok(),
                b'x' | b'X' => i64::from_str_radix(&lit[2..], 16).ok(),
                _ => lit.parse::<i64>().ok(),
            };
        }
        return lit.parse::<i64>().ok();
    }
    lit.parse::<i64>().ok()
}

/// 复合赋值运算符映射（与 Go 版一致，返回中文动词）。
fn compound_assign_op(t: TokenType) -> Option<&'static str> {
    match t {
        TokenType::PlusEq => Some("加"),
        TokenType::MinusEq => Some("减"),
        TokenType::StarEq => Some("乘"),
        TokenType::SlashEq => Some("除"),
        _ => None,
    }
}

fn assignable_expr(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::Ident(_) | ast::Expr::Index(_) | ast::Expr::Member(_)
    )
}

fn is_implicit_call_end(t: TokenType) -> bool {
    matches!(
        t,
        TokenType::Newline
            | TokenType::Eof
            | TokenType::Comma
            | TokenType::RParen
            | TokenType::RBrack
            | TokenType::Then
            | TokenType::Else
            | TokenType::ElseIf
    )
}

fn is_implicit_call_continuation(t: TokenType) -> bool {
    matches!(
        t,
        TokenType::Int
            | TokenType::Float
            | TokenType::Str
            | TokenType::True
            | TokenType::False
            | TokenType::Nil
            | TokenType::LParen
            | TokenType::LBrack
            | TokenType::Minus
            | TokenType::Not
    )
}

/// 判断左侧表达式是否可作为模型复合字面量的模型名。
fn is_composite_model(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Ident(_) => true,
        ast::Expr::Member(m) => is_composite_model(&m.object),
        _ => false,
    }
}

fn is_type_token(t: TokenType) -> bool {
    matches!(
        t,
        TokenType::TypeInt
            | TokenType::TypeFloat
            | TokenType::TypeString
            | TokenType::TypeBool
            | TokenType::TypeArray
            | TokenType::TypeDict
            | TokenType::TypeError
            | TokenType::TypeByte
            | TokenType::TypeChar
    )
}

/// 判断记号能否作为类型位置上的一个类型名。
fn is_type_position_token(t: TokenType) -> bool {
    is_type_token(t) || t == TokenType::Class || t == TokenType::Ident || t == TokenType::Nil
}

/// 默认参数值的静态类型推断与兼容性校验。
///
/// 规则：默认值必须能在解析期推断出确定类型，且与声明类型兼容，否则禁止定义。
/// 目前可推断的仅限字面量（含一元负号的数值字面量）、空值与空容器；
/// 涉及标识符、调用、运算等运行期才能确定类型的表达式一律拒绝。
fn check_default_inferable(decl: &DeclType, default: &ast::Expr) -> Result<(), String> {
    let inferred = infer_literal_type(default).ok_or_else(|| {
        format!(
            "参数默认值无法静态推断类型（声明为 {}），仅支持字面量、空值与空容器",
            decl.0
        )
    })?;
    // 空值可作为任意可空/引用位置的默认值，运行期再做强制检查。
    if inferred == "空" {
        return Ok(());
    }
    if inferred != decl.0 {
        return Err(format!(
            "参数默认值类型 {} 与声明类型 {} 不一致",
            inferred, decl.0
        ));
    }
    Ok(())
}

/// 推断字面量表达式的类型名；无法静态推断时返回 `None`。
fn infer_literal_type(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Integer(_) => Some("整数".to_string()),
        ast::Expr::Float(_) => Some("小数".to_string()),
        ast::Expr::Str(_) | ast::Expr::InterpStr(_) => Some("字符串".to_string()),
        ast::Expr::Char(_) => Some("字符".to_string()),
        ast::Expr::Bool(_) => Some("布尔".to_string()),
        ast::Expr::Nil(_) => Some("空".to_string()),
        ast::Expr::Array(a) if a.elements.is_empty() => Some("数组".to_string()),
        ast::Expr::Dict(d) if d.pairs.is_empty() => Some("字典".to_string()),
        // 一元负号作用于数值字面量仍是同一数值类型。
        ast::Expr::Unary(u) if matches!(u.op.as_str(), "-") => infer_literal_type(&u.right),
        _ => None,
    }
}

fn is_union_sep_token(t: TokenType) -> bool {
    t == TokenType::Or || t == TokenType::BOr
}

fn parse_decl_type(t: TokenType) -> DeclType {
    let name = match t {
        TokenType::TypeInt => "整数",
        TokenType::TypeFloat => "小数",
        TokenType::TypeString => "字符串",
        TokenType::TypeBool => "布尔",
        TokenType::TypeArray => "数组",
        TokenType::TypeDict => "字典",
        TokenType::TypeError => "错误",
        TokenType::TypeByte => "字节",
        TokenType::TypeChar => "字符",
        _ => return DeclType::any(),
    };
    DeclType::named(name)
}

/// 允许类型名（错误/字节/字符）作为函数名使用，如 SDK 中的 `系统.字符(编码点)`。
fn is_function_name_token(t: TokenType) -> bool {
    if t == TokenType::Ident || t == TokenType::Self_ {
        return true;
    }
    is_type_token(t)
}

fn is_function_segment_boundary(t: TokenType) -> bool {
    matches!(t, TokenType::Eof | TokenType::Newline | TokenType::Arrow)
}

fn validate_mixfix_params(
    segments: &[String],
    group_sizes: &[usize],
    params: &[ast::Parameter],
) -> Result<(), String> {
    for (i, size) in group_sizes[..group_sizes.len() - 1].iter().enumerate() {
        if *size == 0 {
            return Err(format!("分段函数第 {} 段至少需要一个参数", i + 1));
        }
    }
    if group_sizes[group_sizes.len() - 1] != 0 && params.is_empty() && segments.len() > 1 {
        return Err("分段函数至少需要一个参数".to_string());
    }
    for param in params {
        if param.type_.is_any() {
            return Err("分段函数参数必须显式声明类型".to_string());
        }
        if param.variadic || param.default.is_some() {
            return Err("分段函数当前不支持默认参数或可变参数".to_string());
        }
    }
    Ok(())
}

fn build_mixfix_signature(
    segments: &[String],
    connectors: &[String],
    group_sizes: &[usize],
    params: &[ast::Parameter],
) -> String {
    let mut out = String::new();
    let mut index = 0usize;
    for (segment_index, segment) in segments.iter().enumerate() {
        out.push_str(&connectors[segment_index]);
        out.push_str(segment);
        out.push('(');
        let group_size = group_sizes[segment_index];
        for offset in 0..group_size {
            if offset > 0 {
                out.push_str(", ");
            }
            out.push_str(&params[index + offset].type_.0);
        }
        out.push(')');
        index += group_size;
    }
    out
}

fn module_base_name(path: &str) -> &str {
    let base = match path.rfind(['/', '\\']) {
        Some(i) => &path[i + 1..],
        None => path,
    };
    match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    }
}

/// 把可见性写入被修饰的具体声明。
fn apply_visibility(stmt: &mut Stmt, vis: Visibility) {
    match stmt {
        Stmt::MethodDef(m) => m.visibility = vis,
        Stmt::VarDecl(v) => v.visibility = vis,
        Stmt::MultiDecl(m) => m.visibility = vis,
        Stmt::Class(c) => c.visibility = vis,
        Stmt::Interface(i) => i.visibility = vis,
        Stmt::TypeDef(t) => t.visibility = vis,
        Stmt::Visibility(inner) => apply_visibility(&mut inner.inner, vis),
        _ => {}
    }
}

fn declared_name(stmt: &Stmt) -> String {
    match stmt {
        Stmt::VarDecl(n) => n.name.clone(),
        Stmt::Class(n) => n.name.clone(),
        Stmt::Interface(n) => n.name.clone(),
        Stmt::TypeDef(n) => n.name.clone(),
        Stmt::MethodDef(n) => n.method_name.clone(),
        Stmt::Visibility(v) => v.name.clone(),
        _ => String::new(),
    }
}

/// 把标签附加到被修饰声明上（函数或模型）。
fn attach_tags(stmt: Stmt, tags: Vec<ast::TagExpr>) -> Stmt {
    if tags.is_empty() {
        return stmt;
    }
    match stmt {
        Stmt::VarDecl(mut v) => {
            if let Some(ast::Expr::Function(f)) = v.value.as_mut() {
                f.tags = tags;
            }
            Stmt::VarDecl(v)
        }
        Stmt::Class(mut c) => {
            c.tags = tags;
            Stmt::Class(c)
        }
        Stmt::Visibility(mut vis) => {
            vis.inner = Box::new(attach_tags(*vis.inner, tags));
            Stmt::Visibility(vis)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析并检查无错误，返回程序。
    fn parse_ok(src: &str) -> Program {
        let mut p = Parser::new(src);
        let prog = p.parse_program();
        assert!(
            p.errors().is_empty(),
            "解析应无错误，实际：{:?}",
            p.errors()
        );
        prog
    }

    /// 解析并断言存在错误，返回错误列表。
    fn parse_err(src: &str) -> Errors {
        let mut p = Parser::new(src);
        let _ = p.parse_program();
        assert!(!p.errors().is_empty(), "预期有语法错误，实际无错误");
        p.errors().clone()
    }

    /// 解析单条语句并返回其文本表示。
    fn stmt_str(src: &str) -> String {
        let prog = parse_ok(src);
        assert_eq!(
            prog.statements.len(),
            1,
            "应解析出一条语句，实际 {:?}",
            prog.statements
        );
        prog.statements[0].to_string()
    }

    #[test]
    fn 空源码解析出空程序() {
        let prog = parse_ok("");
        assert!(prog.is_empty());
    }

    /// 解析表达式上下文中的一段源码，并去掉语句级的自动调用包装。
    fn expr_of(src: &str) -> ast::Expr {
        let prog = parse_ok(&format!("变量 x = {src}"));
        let ast::Stmt::VarDecl(decl) = &prog.statements[0] else {
            panic!("应为变量声明，实际：{}", prog.statements[0]);
        };
        let expr = decl.value.clone().expect("应有初值");
        match expr {
            ast::Expr::MaybeCall(m) => *m.callee,
            other => other,
        }
    }

    #[test]
    fn 分段调用各段拼成完整函数名() {
        // 声明侧把各段名拼成一个名字（`从到乘坐`），调用侧必须拼出同一个
        // 名字，并把各段参数按段序摊平
        assert_eq!(
            stmt_str("从(\"北京\")到(\"上海\")乘坐(\"高铁\")"),
            "从到乘坐(\"北京\", \"上海\", \"高铁\")"
        );
        assert_eq!(
            stmt_str("修改用户(甲)的状态为(乙)"),
            "修改用户的状态为(甲, 乙)"
        );
        // 末段可以没有参数列表
        assert_eq!(
            stmt_str("标记(\"b\")的状态为无效"),
            "标记的状态为无效(\"b\")"
        );
    }

    #[test]
    fn 分段调用可带模块限定() {
        // `工具.求(...)` 的段名合入成员名，模块限定部分不受影响
        assert_eq!(
            stmt_str("工具.求([3, 9, 2])的最大值()"),
            "工具 的 求的最大值([3, 9, 2])"
        );
    }

    #[test]
    fn 段间有空白是调用后取字段() {
        // 这一条与 `标记("b")的状态为无效` 只差一个空格，语义完全不同
        let expr = expr_of("取空格结果(\"s\") 的状态为无效");
        assert!(matches!(expr, ast::Expr::Member(_)), "实际：{expr}");
    }

    #[test]
    fn 点号恒为成员访问不构成新段() {
        // 否则 `造().说话()` 会被误判成名为「造说话」的分段函数
        let expr = expr_of("造().说话()");
        let ast::Expr::Call(call) = expr else {
            panic!("应为调用，实际：{expr}");
        };
        assert!(
            matches!(&*call.callee, ast::Expr::Member(m) if m.name == "说话"),
            "被调用者应为成员 说话，实际：{}",
            call.callee
        );
    }

    #[test]
    fn 注释与空行被忽略() {
        let prog = parse_ok("# 注释\n\n注释 中文注释\n\n");
        assert!(prog.is_empty());
    }

    #[test]
    fn 变量声明带类型与初值() {
        assert_eq!(stmt_str("变量 整数 年龄 = 18"), "变量 整数 年龄 = 18");
    }

    #[test]
    fn 变量声明四种等价写法() {
        for src in [
            "名字 为 \"小明\"",
            "名字 = \"小明\"",
            "定义 名字 为 \"小明\"",
            "变量 名字 = \"小明\"",
        ] {
            let prog = parse_ok(src);
            assert_eq!(prog.statements.len(), 1, "{src} 应解析出一条声明");
        }
    }

    #[test]
    fn 无初值变量声明() {
        assert_eq!(stmt_str("变量 计数"), "变量 计数");
        assert_eq!(stmt_str("变量 字符串 名称"), "变量 字符串 名称");
    }

    #[test]
    fn 常量声明() {
        let prog = parse_ok("常量 圆周率 = 3.14");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => assert!(v.is_const),
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 常量无初值报错() {
        let errs = parse_err("常量 圆周率");
        assert!(errs.first_message().unwrap().contains("必须赋初值"));
    }

    #[test]
    fn 赋值语句() {
        assert_eq!(stmt_str("年龄 = 20"), "变量 年龄 = 20");
    }

    #[test]
    fn 复合赋值映射为中文动词() {
        assert_eq!(stmt_str("甲 += 1"), "(甲 加 1)");
        assert_eq!(stmt_str("甲 -= 1"), "(甲 减 1)");
        assert_eq!(stmt_str("甲 *= 1"), "(甲 乘 1)");
        assert_eq!(stmt_str("甲 /= 1"), "(甲 除 1)");
    }

    #[test]
    fn 多目标声明与赋值() {
        let prog = parse_ok("变量 甲、乙 = 1、2");
        assert_eq!(prog.statements[0].to_string(), "变量 甲、乙 = (1, 2)");
        let prog = parse_ok("甲、乙 = 3、4");
        assert_eq!(prog.statements[0].to_string(), "甲、乙 = (3, 4)");
    }

    #[test]
    fn 整数字面量与进制() {
        assert_eq!(stmt_str("42"), "42");
        assert_eq!(stmt_str("0b1010"), "10");
        assert_eq!(stmt_str("0o17"), "15");
        assert_eq!(stmt_str("0xFF"), "255");
    }

    #[test]
    fn 小数字面量() {
        assert_eq!(stmt_str("3.14"), "3.14");
    }

    #[test]
    fn 字符串与布尔与空值字面量() {
        assert_eq!(stmt_str("\"你好\""), "\"你好\"");
        assert_eq!(stmt_str("真"), "真");
        assert_eq!(stmt_str("假"), "假");
        assert_eq!(stmt_str("空"), "空");
    }

    #[test]
    fn 字符字面量() {
        assert_eq!(stmt_str("'中'"), "'中'");
    }

    #[test]
    fn 一元与二元运算() {
        assert_eq!(stmt_str("-5"), "(-5)");
        assert_eq!(stmt_str("非 甲"), "(非 甲)");
        assert_eq!(stmt_str("1 加 2"), "(1 加 2)");
    }

    #[test]
    fn 乘除优先级高于加减() {
        assert_eq!(stmt_str("1 加 2 乘 3"), "(1 加 (2 乘 3))");
        assert_eq!(stmt_str("1 乘 2 加 3"), "((1 乘 2) 加 3)");
    }

    #[test]
    fn 比较与逻辑运算优先级() {
        assert_eq!(
            stmt_str("甲 大于 1 且 乙 小于 2"),
            "((甲 大于 1) 且 (乙 小于 2))"
        );
    }

    #[test]
    fn 括号改变优先级() {
        assert_eq!(stmt_str("(1 加 2) 乘 3"), "((1 加 2) 乘 3)");
    }

    #[test]
    fn 三元表达式() {
        assert_eq!(stmt_str("甲 ? 1 : 2"), "(甲 ? 1 : 2)");
    }

    #[test]
    fn 三元表达式缺少冒号报错() {
        let errs = parse_err("甲 ? 1");
        assert!(errs.first_message().unwrap().contains("三元表达式缺少"));
    }

    #[test]
    fn 数组字面量与下标() {
        assert_eq!(stmt_str("[1, 2, 3]"), "[1, 2, 3]");
        assert_eq!(stmt_str("甲[0]"), "甲[0]");
        assert_eq!(stmt_str("[]"), "[]");
    }

    #[test]
    fn 数组允许尾随逗号与跨行() {
        assert_eq!(stmt_str("[1, 2,]"), "[1, 2]");
        assert_eq!(stmt_str("[\n    1,\n    2,\n]"), "[1, 2]");
    }

    #[test]
    fn 切片表达式两侧可省略() {
        assert_eq!(stmt_str("甲[1:2]"), "甲[1:2]");
        assert_eq!(stmt_str("甲[1:]"), "甲[1:]");
        assert_eq!(stmt_str("甲[:2]"), "甲[:2]");
        assert_eq!(stmt_str("甲[:]"), "甲[:]");
    }

    #[test]
    fn 字典字面量() {
        assert_eq!(stmt_str("{\"名\": \"小明\"}"), "{\"名\": \"小明\"}");
        assert_eq!(stmt_str("{}"), "{}");
    }

    #[test]
    fn 字典展开项() {
        assert_eq!(stmt_str("{...甲}"), "{...甲}");
    }

    #[test]
    fn 插值字符串变量与表达式() {
        let prog = parse_ok("\"你好 $名称\"");
        match &prog.statements[0] {
            Stmt::Expr(e) => match &e.expr {
                ast::Expr::InterpStr(s) => {
                    assert_eq!(s.parts, vec!["你好 ", ""]);
                    assert_eq!(s.exprs.len(), 1);
                }
                other => panic!("应为插值字符串，实际 {other:?}"),
            },
            other => panic!("应为表达式语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 插值字符串花括号形式() {
        let prog = parse_ok("\"共 ${数量 乘 2} 个\"");
        match &prog.statements[0] {
            Stmt::Expr(e) => match &e.expr {
                ast::Expr::InterpStr(s) => {
                    assert_eq!(s.exprs.len(), 1);
                    assert_eq!(s.parts.len(), 2);
                }
                other => panic!("应为插值字符串，实际 {other:?}"),
            },
            other => panic!("应为表达式语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 成员访问与调用() {
        assert_eq!(stmt_str("甲 的 乙"), "甲 的 乙");
        assert_eq!(stmt_str("甲.乙"), "甲 的 乙");
        assert_eq!(stmt_str("打印(1)"), "打印(1)");
        assert_eq!(stmt_str("打印(1, 2)"), "打印(1, 2)");
    }

    #[test]
    fn 是类型判断与转为类型转换() {
        let prog = parse_ok("甲 是 狗");
        match &prog.statements[0] {
            Stmt::Expr(e) => assert!(matches!(e.expr, ast::Expr::TypeTest(_))),
            other => panic!("应为类型判断，实际 {other:?}"),
        }
        let prog = parse_ok("甲 转为 整数");
        match &prog.statements[0] {
            Stmt::Expr(e) => assert!(matches!(e.expr, ast::Expr::TypeCast(_))),
            other => panic!("应为类型转换，实际 {other:?}"),
        }
    }

    #[test]
    fn 是后面不是类型名报错() {
        let errs = parse_err("甲 是 加");
        assert!(errs.first_message().unwrap().contains("后面应为类型名"));
    }

    #[test]
    fn 条件语句单分支() {
        let prog = parse_ok("如果 甲 那么\n    乙 = 1\n结束");
        assert_eq!(prog.statements.len(), 1);
        match &prog.statements[0] {
            Stmt::If(i) => {
                assert_eq!(i.branches.len(), 1);
                assert!(i.else_.is_none());
            }
            other => panic!("应为条件语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 条件语句含否则如果与否则() {
        let prog = parse_ok(
            "如果 甲 那么\n    乙 = 1\n否则如果 丙 那么\n    乙 = 2\n否则\n    乙 = 3\n结束",
        );
        match &prog.statements[0] {
            Stmt::If(i) => {
                assert_eq!(i.branches.len(), 2);
                assert!(i.else_.is_some());
            }
            other => panic!("应为条件语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 否则如果可写成否则空格如果() {
        let prog = parse_ok("如果 甲 那么\n    乙 = 1\n否则 如果 丙 那么\n    乙 = 2\n结束");
        match &prog.statements[0] {
            Stmt::If(i) => assert_eq!(i.branches.len(), 2),
            other => panic!("应为条件语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 条件语句缺少那么报错() {
        let errs = parse_err("如果 甲\n    乙 = 1\n结束");
        assert!(errs.first_message().unwrap().contains("那么"));
    }

    #[test]
    fn 条件语句缺少结束报错() {
        let errs = parse_err("如果 甲 那么\n    乙 = 1\n");
        assert!(errs.first_message().unwrap().contains("结束"));
    }

    #[test]
    fn 否则后跟那么报错() {
        let errs = parse_err("如果 甲 那么\n    乙 = 1\n否则 那么\n    乙 = 2\n结束");
        assert!(errs.first_message().unwrap().contains("否则 后不能跟 那么"));
    }

    #[test]
    fn 判断语句值匹配与默认分支() {
        let prog = parse_ok("判断 状态码\n    为 200\n        打印(\"成功\")\n    其他\n        打印(\"其他\")\n结束");
        match &prog.statements[0] {
            Stmt::Switch(s) => {
                assert!(s.subject.is_some());
                assert_eq!(s.branches.len(), 2);
                assert!(s.branches[1].default);
            }
            other => panic!("应为判断语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 判断语句条件守卫分支() {
        let prog = parse_ok("判断\n    当 甲 大于 5\n        打印(\"大\")\n结束");
        match &prog.statements[0] {
            Stmt::Switch(s) => {
                assert!(s.subject.is_none());
                assert!(s.branches[0].guard);
            }
            other => panic!("应为判断语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 判断无主题时值匹配分支报错() {
        let errs = parse_err("判断\n    为 1\n        打印(1)\n结束");
        assert!(errs.first_message().unwrap().contains("判断分支应使用 当"));
    }

    #[test]
    fn 当循环() {
        let prog = parse_ok("当 甲 小于 10 循环\n    甲 = 甲 加 1\n结束");
        assert!(matches!(prog.statements[0], Stmt::While(_)));
    }

    #[test]
    fn 当循环缺少循环报错() {
        let errs = parse_err("当 甲 小于 10\n    甲 = 1\n结束");
        assert!(errs.first_message().unwrap().contains("循环"));
    }

    #[test]
    fn 重复直到循环() {
        let prog = parse_ok("重复\n    甲 = 甲 加 1\n直到 甲 等于 10\n结束");
        assert!(matches!(prog.statements[0], Stmt::Repeat(_)));
    }

    #[test]
    fn 重复次数循环() {
        let prog = parse_ok("重复 3 次\n    打印(\"你好\")\n结束");
        match &prog.statements[0] {
            Stmt::Times(t) => assert_eq!(t.count.to_string(), "3"),
            other => panic!("应为计数循环，实际 {other:?}"),
        }
    }

    #[test]
    fn 重复次数循环内不能用直到() {
        let errs = parse_err("重复 3 次\n    打印(1)\n直到 甲\n结束");
        assert!(errs.first_message().unwrap().contains("直到"));
    }

    #[test]
    fn 遍历循环单变量与双变量() {
        let prog = parse_ok("遍历 名单 中的 人\n    打印(人)\n结束");
        match &prog.statements[0] {
            Stmt::ForEach(f) => assert!(f.second_name.is_none()),
            other => panic!("应为遍历循环，实际 {other:?}"),
        }
        let prog = parse_ok("遍历 名单 中的 人 序号\n    打印(序号)\n结束");
        match &prog.statements[0] {
            Stmt::ForEach(f) => assert_eq!(f.second_name.as_deref(), Some("序号")),
            other => panic!("应为遍历循环，实际 {other:?}"),
        }
    }

    #[test]
    fn 遍历缺少中的报错() {
        let errs = parse_err("遍历 名单\n    打印(1)\n结束");
        assert!(errs.first_message().unwrap().contains("中的"));
    }

    #[test]
    fn 跳出与继续() {
        let prog = parse_ok("当 真 循环\n    跳出\n结束");
        assert!(matches!(prog.statements[0], Stmt::While(_)));
    }

    #[test]
    fn 函数定义含参数与返回类型() {
        let prog = parse_ok("函数 求和(整数 甲, 整数 乙)\n    返回 甲 加 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => {
                assert_eq!(v.name, "求和");
                match v.value.as_ref().unwrap() {
                    ast::Expr::Function(f) => assert_eq!(f.params.len(), 2),
                    other => panic!("应为函数字面量，实际 {other:?}"),
                }
            }
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 函数返回类型声明() {
        let prog = parse_ok("定义 函数 求和(整数 甲, 整数 乙) -> 整数\n    返回 甲 加 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert_eq!(f.return_types, vec![DeclType::named("整数")]);
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 函数多返回类型() {
        let prog = parse_ok("函数 除法() -> (整数, 错误)\n    返回 1、空\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => assert_eq!(f.return_types.len(), 2),
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 函数默认参数与可变参数() {
        let prog = parse_ok("函数 问候(字符串 名字, 字符串 称呼 = \"你好\")\n    打印(称呼)\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => assert!(f.params[1].default.is_some()),
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
        let prog = parse_ok("函数 求和(整数 开头, 整数... 其余)\n    返回 开头\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => assert!(f.params[1].variadic),
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 可变参数后还有参数报错() {
        let errs = parse_err("函数 甲(整数... 乙, 整数 丙)\n    返回 1\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("可变参数必须是最后一个参数"));
    }

    #[test]
    fn 默认参数后不能再有普通参数() {
        let errs = parse_err("函数 甲(整数 乙 = 1, 整数 丙)\n    返回 1\n结束");
        assert!(errs.first_message().unwrap().contains("默认参数后面"));
    }

    #[test]
    fn 参数类型在前名字在后() {
        let prog = parse_ok("函数 求和(整数 甲, 整数 乙) -> 整数\n    返回 甲 加 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert_eq!(f.params[0].name, "甲");
                    assert_eq!(f.params[0].type_, DeclType::named("整数"));
                    assert_eq!(f.params[1].name, "乙");
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 旧参数语序直接报错() {
        let errs = parse_err("函数 甲(乙 整数)\n    返回 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("类型后应为参数名"));
    }

    #[test]
    fn 参数必须显式声明类型() {
        let errs = parse_err("函数 甲(乙)\n    返回 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("类型后应为参数名"));
    }

    #[test]
    fn 任意类型作显式动态占位() {
        let prog = parse_ok("函数 甲(任意 乙)\n    返回 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => assert!(f.params[0].type_.is_any()),
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 匿名联合不能直接作参数类型() {
        let errs = parse_err("函数 甲(整数 或 字符串 乙)\n    返回 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("不允许匿名联合类型"));
    }

    #[test]
    fn 默认值类型不符时报错() {
        let errs = parse_err("函数 甲(整数 乙 = \"x\")\n    返回 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("默认值类型 字符串 与声明类型 整数 不一致"));
    }

    #[test]
    fn 默认值无法静态推断时报错() {
        let errs = parse_err("函数 甲(整数 乙 = 取数())\n    返回 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("无法静态推断类型"));
    }

    #[test]
    fn 字面量默认值通过推断() {
        let prog = parse_ok("函数 甲(整数 乙 = 3, 字符串 丙 = \"x\")\n    返回 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert!(f.params[0].default.is_some());
                    assert!(f.params[1].default.is_some());
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 模型体内变量关键字已移除() {
        let errs = parse_err("模型 甲\n    变量 整数 乙\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("字段须用 属性 声明"));
    }

    #[test]
    fn 模型字段用属性声明() {
        let prog = parse_ok("模型 甲\n    属性 整数 乙\n    公开 属性 字符串 丙\n结束");
        match &prog.statements[0] {
            Stmt::Class(c) => {
                assert_eq!(c.fields.len(), 2);
                assert_eq!(c.fields[1].visibility, Visibility::Public);
            }
            other => panic!("应为模型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 函数缺少结束报错() {
        let errs = parse_err("函数 甲()\n    返回 1\n");
        assert!(errs.first_message().unwrap().contains("结束"));
    }

    #[test]
    fn 返回语句多值() {
        let prog = parse_ok("函数 甲()\n    返回 1、2\n结束");
        assert!(matches!(prog.statements[0], Stmt::VarDecl(_)));
    }

    #[test]
    fn 匿名函数字面量() {
        let prog = parse_ok("变量 平方 = 函数(整数 数) -> 整数\n    返回 数 乘 数\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => {
                assert!(matches!(v.value.as_ref().unwrap(), ast::Expr::Function(_)))
            }
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 模型定义含字段与嵌入() {
        let prog = parse_ok("模型 矩形\n    属性 整数 宽\n    属性 整数 高\n结束");
        match &prog.statements[0] {
            Stmt::Class(c) => {
                assert_eq!(c.name, "矩形");
                assert_eq!(c.fields.len(), 2);
            }
            other => panic!("应为模型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 模型字段可见性() {
        let prog = parse_ok("模型 账户\n    公开 属性 字符串 名字\n    私有 属性 整数 余额\n结束");
        match &prog.statements[0] {
            Stmt::Class(c) => {
                assert_eq!(c.fields[0].visibility, Visibility::Public);
                assert_eq!(c.fields[1].visibility, Visibility::Private);
            }
            other => panic!("应为模型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 模型嵌入() {
        let prog = parse_ok("模型 学生\n    嵌入 人\n    属性 学号\n结束");
        match &prog.statements[0] {
            Stmt::Class(c) => assert_eq!(c.embeds, vec!["人".to_string()]),
            other => panic!("应为模型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 模型缺少结束报错() {
        let errs = parse_err("模型 矩形\n    属性 整数 宽\n");
        assert!(errs.first_message().unwrap().contains("结束"));
    }

    #[test]
    fn 方法定义() {
        let prog = parse_ok("定义 矩形 的 方法 面积() -> 整数\n    返回 自己.宽 乘 自己.高\n结束");
        match &prog.statements[0] {
            Stmt::MethodDef(m) => {
                assert_eq!(m.class_name, "矩形");
                assert_eq!(m.method_name, "面积");
                assert_eq!(m.receiver_name, "自己");
            }
            other => panic!("应为方法定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 方法可见性() {
        let prog = parse_ok("定义 矩形 的 公开 方法 面积()\n    返回 1\n结束");
        match &prog.statements[0] {
            Stmt::MethodDef(m) => assert_eq!(m.visibility, Visibility::Public),
            other => panic!("应为方法定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 接口定义含方法签名与嵌入() {
        let prog = parse_ok("接口 说话者\n    方法 说话() -> 字符串\n结束");
        match &prog.statements[0] {
            Stmt::Interface(i) => {
                assert_eq!(i.name, "说话者");
                assert_eq!(i.methods.len(), 1);
            }
            other => panic!("应为接口定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 自定义类型定义() {
        let prog = parse_ok("定义 类型 状态 = 整数");
        match &prog.statements[0] {
            Stmt::TypeDef(t) => {
                assert_eq!(t.name, "状态");
                assert_eq!(t.base_type, DeclType::named("整数"));
            }
            other => panic!("应为类型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 类型别名底层允许基础类型联合() {
        let prog = parse_ok("定义 类型 编号 = 整数 或 字符串");
        match &prog.statements[0] {
            Stmt::TypeDef(t) => {
                assert_eq!(t.name, "编号");
                assert!(t.base_type.is_union());
                assert_eq!(t.base_type.members(), vec!["整数", "字符串"]);
            }
            other => panic!("应为类型定义，实际 {other:?}"),
        }
    }

    #[test]
    fn 类型别名底层拒绝模型名() {
        let errs = parse_err("模型 用户\n    属性 字符串 名\n结束\n定义 类型 编号 = 用户");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("底层类型只能是基本类型或其联合"));
    }

    #[test]
    fn 类型别名底层拒绝联合中的模型名() {
        let errs = parse_err("模型 用户\n    属性 字符串 名\n结束\n定义 类型 编号 = 整数 或 用户");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("不允许模型/接口名或其他自定义类型"));
        assert!(errs.first_message().unwrap().contains("用户"));
    }

    #[test]
    fn 联合类型标注() {
        let prog = parse_ok("变量 整数 或 字符串 编号 = 1");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => {
                assert_eq!(v.decl_type.to_string(), "整数|字符串");
                assert!(v.decl_type.is_union());
            }
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 导入语句默认模块名() {
        let prog = parse_ok("导入 \"路径/模块名\"");
        match &prog.statements[0] {
            Stmt::Import(i) => {
                assert_eq!(i.path, "路径/模块名");
                assert_eq!(i.name, "模块名");
            }
            other => panic!("应为导入语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 导入语句使用别名() {
        let prog = parse_ok("导入 \"路径/模块名\" 作为 m");
        match &prog.statements[0] {
            Stmt::Import(i) => assert_eq!(i.name, "m"),
            other => panic!("应为导入语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 导入后不是字符串报错() {
        let errs = parse_err("导入 模块名");
        assert!(errs.first_message().unwrap().contains("字符串路径"));
    }

    #[test]
    fn 公开修饰函数() {
        let prog = parse_ok("公开 函数 加倍(整数 n) -> 整数\n    返回 n 乘 2\n结束");
        match &prog.statements[0] {
            Stmt::Visibility(v) => {
                assert_eq!(v.visibility, Visibility::Public);
                assert_eq!(v.name, "加倍");
            }
            other => panic!("应为可见性语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 公开修饰变量与定义在前() {
        let prog = parse_ok("公开 变量 版本 = \"1.0\"");
        assert_eq!(prog.statements[0].to_string(), "公开 变量 版本 = \"1.0\"");

        let prog = parse_ok("定义 公开 变量 版本 = \"1.0\"");
        assert_eq!(prog.statements[0].to_string(), "公开 变量 版本 = \"1.0\"");
    }

    #[test]
    fn 私有修饰模型() {
        let prog = parse_ok("私有 模型 内部\n    属性 甲\n结束");
        match &prog.statements[0] {
            Stmt::Visibility(v) => assert_eq!(v.visibility, Visibility::Private),
            other => panic!("应为可见性语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 标签解析位置参数与命名参数() {
        let prog = parse_ok("@内建(\"打印\")\n函数 输出(任意... 内容)\n    返回 空\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert_eq!(f.tags.len(), 1);
                    assert_eq!(f.tags[0].type_name, "内建");
                    assert_eq!(f.tags[0].positional.len(), 1);
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 标签解析命名参数() {
        let prog = parse_ok("@标记(说明 = \"示例\")\n函数 测试()\n    返回 42\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert_eq!(f.tags[0].named.len(), 1);
                    assert_eq!(f.tags[0].named[0].name, "说明");
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 无参标签() {
        let prog = parse_ok("@测试\n函数 甲()\n    返回 1\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert_eq!(f.tags[0].type_name, "测试");
                    assert!(f.tags[0].positional.is_empty());
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 复合字面量() {
        assert_eq!(stmt_str("矩形{宽: 3, 高: 4}"), "矩形{宽: 3, 高: 4}");
    }

    #[test]
    fn 复合字面量字段重复报错() {
        let errs = parse_err("矩形{宽: 3, 宽: 4}");
        assert!(errs.first_message().unwrap().contains("重复指定"));
    }

    #[test]
    fn 稍后与抛出语句() {
        let prog = parse_ok("稍后 关闭()");
        assert!(matches!(prog.statements[0], Stmt::Defer(_)));

        let prog = parse_ok("抛出 错误(\"失败\")");
        assert!(matches!(prog.statements[0], Stmt::Throw(_)));
    }

    #[test]
    fn 稍后后不是调用报错() {
        let errs = parse_err("稍后 1");
        assert!(errs.first_message().unwrap().contains("稍后"));
    }

    #[test]
    fn 检查表达式() {
        let prog = parse_ok("变量 甲 = 检查 读取()");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => assert!(matches!(v.value.as_ref().unwrap(), ast::Expr::Check(_))),
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 通过表达式() {
        let prog = parse_ok("通过 m 根(1)");
        match &prog.statements[0] {
            Stmt::Expr(e) => assert!(matches!(e.expr, ast::Expr::Through(_))),
            other => panic!("应为表达式语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 下标赋值() {
        assert_eq!(stmt_str("甲[0] = 9"), "甲[0] = 9");
    }

    #[test]
    fn 字段赋值() {
        assert_eq!(stmt_str("自己.宽 = 3"), "自己.宽 = 3");
    }

    #[test]
    fn 自增自减前后缀() {
        assert_eq!(stmt_str("甲++"), "(甲加)");
        assert_eq!(stmt_str("++甲"), "(加甲)");
    }

    #[test]
    fn 无括号调用() {
        let prog = parse_ok("打印 \"张三\"");
        match &prog.statements[0] {
            Stmt::Expr(e) => assert!(matches!(
                e.expr,
                ast::Expr::Call(_) | ast::Expr::MaybeCall(_)
            )),
            other => panic!("应为表达式语句，实际 {other:?}"),
        }
    }

    #[test]
    fn 接收者调用形式() {
        let prog = parse_ok("名单 追加 \"张三\"");
        assert_eq!(prog.statements.len(), 1);
    }

    #[test]
    fn 分段函数解析出多段() {
        let prog = parse_ok("函数 根(整数 甲)的段(整数 乙)\n    返回 甲 加 乙\n结束");
        match &prog.statements[0] {
            Stmt::VarDecl(v) => match v.value.as_ref().unwrap() {
                ast::Expr::Function(f) => {
                    assert!(f.is_segmented());
                    assert_eq!(f.name, "根的段");
                }
                other => panic!("应为函数字面量，实际 {other:?}"),
            },
            other => panic!("应为变量声明，实际 {other:?}"),
        }
    }

    #[test]
    fn 分段函数参数必须标注类型() {
        let errs = parse_err("函数 根(甲)的段(乙)\n    返回 1\n结束");
        assert!(errs
            .first_message()
            .unwrap()
            .contains("类型后应为参数名"));
    }

    #[test]
    fn 多条语句按顺序解析() {
        let prog = parse_ok("变量 甲 = 1\n变量 乙 = 2\n变量 丙 = 3");
        assert_eq!(prog.statements.len(), 3);
    }

    #[test]
    fn 嵌套条件语句() {
        let prog = parse_ok("如果 甲 那么\n    如果 乙 那么\n        丙 = 1\n    结束\n结束");
        assert_eq!(prog.statements.len(), 1);
    }

    #[test]
    fn 非法字符被收集为错误() {
        let errs = parse_err("甲 ＝ 1");
        assert!(errs.first_message().unwrap().contains("非法字符"));
    }

    #[test]
    fn 缺少右括号报错() {
        let errs = parse_err("(1 加 2");
        assert!(errs.first_message().unwrap().contains("缺少右括号"));
    }

    #[test]
    fn 缺少右方括号报错() {
        let errs = parse_err("甲[0");
        assert!(errs.first_message().unwrap().contains("右方括号"));
    }

    #[test]
    fn 模块名取路径最后一段并去后缀() {
        assert_eq!(module_base_name("路径/模块名"), "模块名");
        assert_eq!(module_base_name("a/b/c.凡"), "c");
        assert_eq!(module_base_name("模块名"), "模块名");
    }

    #[test]
    fn 复合赋值运算符映射完整() {
        assert_eq!(compound_assign_op(TokenType::PlusEq), Some("加"));
        assert_eq!(compound_assign_op(TokenType::MinusEq), Some("减"));
        assert_eq!(compound_assign_op(TokenType::StarEq), Some("乘"));
        assert_eq!(compound_assign_op(TokenType::SlashEq), Some("除"));
        assert_eq!(compound_assign_op(TokenType::Plus), None);
    }

    #[test]
    fn 可赋值目标判断() {
        assert!(assignable_expr(&ast::Expr::Ident(ast::Ident {
            pos: Position::START,
            name: "甲".to_string()
        })));
        assert!(!assignable_expr(&ast::Expr::Integer(ast::IntegerLit {
            pos: Position::START,
            value: 1
        })));
    }

    #[test]
    fn 模型复合字面量的模型名判定() {
        let ident = ast::Expr::Ident(ast::Ident {
            pos: Position::START,
            name: "矩形".to_string(),
        });
        assert!(is_composite_model(&ident));
        let int = ast::Expr::Integer(ast::IntegerLit {
            pos: Position::START,
            value: 1,
        });
        assert!(!is_composite_model(&int));
    }

    #[test]
    fn 分段函数签名构造() {
        let segments = vec!["根".to_string(), "段".to_string()];
        let connectors = vec![String::new(), "的".to_string()];
        let group_sizes = vec![1, 1];
        let params = vec![
            ast::Parameter {
                name: "甲".to_string(),
                type_: DeclType::named("整数"),
                variadic: false,
                default: None,
            },
            ast::Parameter {
                name: "乙".to_string(),
                type_: DeclType::named("整数"),
                variadic: false,
                default: None,
            },
        ];
        assert_eq!(
            build_mixfix_signature(&segments, &connectors, &group_sizes, &params),
            "根(整数)的段(整数)"
        );
    }

    #[test]
    fn 声明名提取() {
        let stmt = Stmt::VarDecl(ast::VarDecl {
            pos: Position::START,
            is_const: false,
            is_explicit: true,
            decl_type: DeclType::any(),
            name: "甲".to_string(),
            value: None,
            visibility: Visibility::File,
        });
        assert_eq!(declared_name(&stmt), "甲");
        assert_eq!(
            declared_name(&Stmt::Break(ast::BreakStmt {
                pos: Position::START
            })),
            ""
        );
    }
}
