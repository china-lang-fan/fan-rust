//! AST → 字节码的代码生成。
//!
//! 编译器只依赖 fan-ast 与本 crate 的字节码类型，不包含任何执行逻辑。M1 阶段
//! 覆盖最小命令式子集：字面量、变量、运算、if/while、普通函数与原生调用；
//! 闭包、对象系统、标签、defer 等在后续里程碑逐步加入。

mod scope;

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;
use fan_ast::Program;

use crate::chunk::{Arity, ChunkBuilder, FunctionChunk, ModuleChunk};
use crate::opcode::Opcode;
use crate::ConstValue;

use scope::ScopeTracker;

/// 编译错误。
pub type CompileResult<T> = Result<T, String>;

/// 把一份程序（一个源文件）编译为模块块。
pub fn compile_program(program: &Program) -> CompileResult<ModuleChunk> {
    let mut compiler = Compiler::new();
    compiler.compile_statements(&program.statements)?;
    // 入口块以 Halt 收尾，停机时值栈顶即顶层最后一个表达式结果（若有）。
    compiler.builder.emit(Opcode::Halt);
    let entry = compiler.builder.build();

    Ok(ModuleChunk {
        entry,
        functions: std::mem::take(&mut compiler.functions),
        exports: compiler.exports,
        imports: compiler.imports,
    })
}

struct Compiler {
    builder: ChunkBuilder,
    functions: Vec<FunctionChunk>,
    exports: Vec<String>,
    imports: Vec<String>,
    scope: ScopeTracker,
    /// 当前所在函数的参数名（空表示模块入口），用于把参数映射到前置槽位。
    params: Vec<String>,
    /// 是否正在编译模块入口（决定顶层表达式语句是否保留值）。
    in_module: bool,
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            builder: ChunkBuilder::new(),
            functions: Vec::new(),
            exports: Vec::new(),
            imports: Vec::new(),
            scope: ScopeTracker::new(),
            params: Vec::new(),
            in_module: true,
        }
    }

    fn line(&mut self, line: u32) {
        self.builder.set_line(line);
    }

    fn add_string_const(&mut self, s: &str) -> u16 {
        self.builder.add_constant(ConstValue::Str(s.to_string()))
    }

    fn compile_statements(&mut self, stmts: &[Stmt]) -> CompileResult<()> {
        for stmt in stmts {
            self.compile_stmt(stmt)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> CompileResult<()> {
        match stmt {
            Stmt::VarDecl(d) => {
                self.line(d.pos.line as u32);
                if let Some(value) = &d.value {
                    self.compile_expr(value)?;
                } else {
                    let nil = self.builder.add_constant(ConstValue::Nil);
                    self.builder.emit(Opcode::Const(nil));
                }
                // 隐式声明（无 `变量` 关键字的 `甲 = 值`）在已有同名变量时是赋值，
                // 与树遍历求值器一致：解析到既有槽位就原地写入，否则才声明新槽。
                let slot = if !d.is_explicit {
                    if let Some(existing) = self.scope.resolve(&d.name) {
                        existing
                    } else {
                        self.scope.declare(&d.name)
                    }
                } else {
                    self.scope.declare(&d.name)
                };
                self.builder.emit(Opcode::StoreLocal(slot));
                if matches!(d.visibility, fan_ast::stmt::Visibility::Public) {
                    self.exports.push(d.name.clone());
                }
                Ok(())
            }
            Stmt::Assign(a) => {
                self.line(a.pos.line as u32);
                self.compile_expr(&a.value)?;
                self.emit_store_name(&a.name)
            }
            Stmt::Expr(e) => {
                self.line(e.pos.line as u32);
                self.compile_expr(&e.expr)?;
                // 函数体内语句的值被丢弃；模块入口保留顶层表达式值作为运行结果。
                if !self.in_module {
                    self.builder.emit(Opcode::Pop);
                }
                Ok(())
            }
            Stmt::Block(b) => self.compile_block(&b.statements),
            Stmt::If(if_stmt) => self.compile_if(if_stmt),
            Stmt::While(w) => self.compile_while(w),
            Stmt::Return(r) => {
                self.line(r.pos.line as u32);
                match r.values.as_slice() {
                    [] => {
                        let nil = self.builder.add_constant(ConstValue::Nil);
                        self.builder.emit(Opcode::Const(nil));
                    }
                    [single] => self.compile_expr(single)?,
                    many => {
                        for e in many {
                            self.compile_expr(e)?;
                        }
                        self.builder.emit(Opcode::BuildTuple(many.len() as u16));
                    }
                }
                self.builder.emit(Opcode::Return);
                Ok(())
            }
            Stmt::Break(_) | Stmt::Continue(_) => {
                Err("跳出/继续 将在循环里程碑接入字节码".to_string())
            }
            other => Err(format!(
                "字节码编译器暂不支持的语句：{}",
                stmt_kind_name(other)
            )),
        }
    }

    fn compile_block(&mut self, stmts: &[Stmt]) -> CompileResult<()> {
        self.scope.push();
        for s in stmts {
            self.compile_stmt(s)?;
        }
        self.scope.pop();
        Ok(())
    }

    fn compile_if(&mut self, if_stmt: &fan_ast::stmt::IfStmt) -> CompileResult<()> {
        self.line(if_stmt.pos.line as u32);
        let mut end_jumps = Vec::new();
        let branch_count = if_stmt.branches.len();
        for (idx, branch) in if_stmt.branches.iter().enumerate() {
            self.compile_expr(&branch.condition)?;
            let jump_if_false = self.builder.emit(Opcode::JumpIfFalse(0));
            self.builder.emit(Opcode::Pop); // 弹出条件值
            self.compile_block(&branch.body.statements)?;
            if idx + 1 < branch_count || if_stmt.else_.is_some() {
                end_jumps.push(self.builder.emit(Opcode::Jump(0)));
            }
            self.builder.patch_jump(jump_if_false);
        }
        if let Some(else_block) = &if_stmt.else_ {
            self.compile_block(&else_block.statements)?;
        }
        for jump in end_jumps {
            self.builder.patch_jump(jump);
        }
        Ok(())
    }

    fn compile_while(&mut self, w: &fan_ast::stmt::WhileStmt) -> CompileResult<()> {
        self.line(w.pos.line as u32);
        let loop_start = self.builder.code_len() as u32;
        self.compile_expr(&w.condition)?;
        let exit = self.builder.emit(Opcode::JumpIfFalse(0));
        self.builder.emit(Opcode::Pop);
        self.compile_block(&w.body.statements)?;
        self.builder.emit(Opcode::Jump(loop_start));
        self.builder.patch_jump(exit);
        self.builder.emit(Opcode::Pop); // 退出时弹出条件假值
        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> CompileResult<()> {
        match expr {
            Expr::Integer(n) => {
                let c = self.builder.add_constant(ConstValue::Int(n.value));
                self.line(n.pos.line as u32);
                self.builder.emit(Opcode::Const(c));
                Ok(())
            }
            Expr::Float(n) => {
                let c = self.builder.add_constant(ConstValue::Float(n.value));
                self.line(n.pos.line as u32);
                self.builder.emit(Opcode::Const(c));
                Ok(())
            }
            Expr::Str(s) => {
                let c = self.builder.add_constant(ConstValue::Str(s.value.clone()));
                self.line(s.pos.line as u32);
                self.builder.emit(Opcode::Const(c));
                Ok(())
            }
            Expr::Char(c) => {
                let k = self.builder.add_constant(ConstValue::Char(c.value));
                self.line(c.pos.line as u32);
                self.builder.emit(Opcode::Const(k));
                Ok(())
            }
            Expr::Bool(b) => {
                let c = self.builder.add_constant(ConstValue::Bool(b.value));
                self.line(b.pos.line as u32);
                self.builder.emit(Opcode::Const(c));
                Ok(())
            }
            Expr::Nil(n) => {
                let c = self.builder.add_constant(ConstValue::Nil);
                self.line(n.pos.line as u32);
                self.builder.emit(Opcode::Const(c));
                Ok(())
            }
            Expr::Ident(id) => {
                self.line(id.pos.line as u32);
                self.emit_load_name(&id.name)
            }
            Expr::Unary(u) => {
                self.line(u.pos.line as u32);
                self.compile_expr(&u.right)?;
                let op = self.add_string_const(&u.op);
                self.builder.emit(Opcode::UnaryOp(op));
                Ok(())
            }
            Expr::Binary(b) => {
                self.line(b.pos.line as u32);
                self.compile_expr(&b.left)?;
                self.compile_expr(&b.right)?;
                let op = self.add_string_const(&b.op);
                self.builder.emit(Opcode::BinOp(op));
                Ok(())
            }
            Expr::Call(call) => self.compile_call(call),
            Expr::MaybeCall(maybe) => {
                self.line(maybe.pos.line as u32);
                self.compile_expr(&maybe.callee)?;
                if maybe.auto_call {
                    // 运行期仅当值为函数时才零参调用，否则原样保留。
                    self.builder.emit(Opcode::CallIfFunc);
                }
                Ok(())
            }
            Expr::Function(f) => self.compile_function_literal(f),
            other => Err(format!(
                "字节码编译器暂不支持的表达式：{}",
                expr_kind_name(other)
            )),
        }
    }

    fn compile_call(&mut self, call: &fan_ast::expr::CallExpr) -> CompileResult<()> {
        self.line(call.pos.line as u32);
        for arg in &call.args {
            match arg {
                fan_ast::expr::Expr::Spread(s) => {
                    self.compile_expr(&s.value)?;
                    // M1 子集暂不展开实参，展开在数组里程碑处理。
                    return Err("字节码编译器暂不支持调用实参展开（...）".to_string());
                }
                other => self.compile_expr(other)?,
            }
        }
        self.compile_expr(&call.callee)?;
        let argc = call.args.len() as u8;
        self.builder.emit(Opcode::Call(argc));
        Ok(())
    }

    fn compile_function_literal(&mut self, f: &fan_ast::expr::FunctionLit) -> CompileResult<()> {
        self.line(f.pos.line as u32);
        if f.native.is_some() {
            return Err("本地原生绑定（定义 本地）由装配层处理，不进字节码函数表".to_string());
        }
        let name = if f.name_segments.is_empty() {
            if f.name.is_empty() {
                "<匿名>".to_string()
            } else {
                f.name.clone()
            }
        } else {
            f.name_segments.join("")
        };

        // 在子编译器中生成函数体，局部槽位从参数开始顺序分配。
        let mut child = Compiler::new();
        child.in_module = false;
        child.params = f.params.iter().map(|p| p.name.clone()).collect();
        for (idx, param) in f.params.iter().enumerate() {
            child.scope.declare_at(&param.name, idx as u16);
        }
        child.compile_statements(&f.body.statements)?;
        let nil = child.builder.add_constant(ConstValue::Nil);
        child.builder.emit(Opcode::Const(nil));
        child.builder.emit(Opcode::Return);

        let arity = Arity {
            fixed: f.params.len(),
            variadic: f.params.iter().any(|p| p.variadic),
            optional: f.params.iter().filter(|p| p.default.is_some()).count(),
        };
        let chunk = FunctionChunk {
            name: name.clone(),
            arity,
            params: child.params.clone(),
            body: child.builder.build(),
            upvalues: Vec::new(),
        };
        let index = self.functions.len() as u16;
        self.functions.push(chunk);
        self.builder.emit(Opcode::MakeClosure(index));

        // 具名函数同时绑定到当前作用域名，匿名函数仅压栈。
        if !f.name.is_empty() {
            let slot = self.scope.declare(&f.name);
            self.builder.emit(Opcode::StoreLocal(slot));
            // 绑定后再把函数值留在栈上，保持“声明表达式有值”的直觉（非必需）。
            self.builder.emit(Opcode::LoadLocal(slot));
        }
        Ok(())
    }

    /// 按名读取：局部槽优先，未命中则作为全局/原生名。
    fn emit_load_name(&mut self, name: &str) -> CompileResult<()> {
        if let Some(slot) = self.scope.resolve(name) {
            self.builder.emit(Opcode::LoadLocal(slot));
        } else {
            let idx = self.add_string_const(name);
            self.builder.emit(Opcode::LoadGlobal(idx));
        }
        Ok(())
    }

    fn emit_store_name(&mut self, name: &str) -> CompileResult<()> {
        if let Some(slot) = self.scope.resolve(name) {
            self.builder.emit(Opcode::StoreLocal(slot));
        } else {
            let idx = self.add_string_const(name);
            self.builder.emit(Opcode::StoreGlobal(idx));
        }
        Ok(())
    }
}

fn stmt_kind_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::VarDecl(_) => "变量声明",
        Stmt::Assign(_) => "赋值",
        Stmt::CompoundAssign(_) => "复合赋值",
        Stmt::MultiDecl(_) => "多目标声明",
        Stmt::MultiAssign(_) => "多目标赋值",
        Stmt::Expr(_) => "表达式语句",
        Stmt::Block(_) => "语句块",
        Stmt::If(_) => "条件",
        Stmt::Switch(_) => "判断",
        Stmt::While(_) => "当循环",
        Stmt::Repeat(_) => "重复直到循环",
        Stmt::Times(_) => "重复N次循环",
        Stmt::ForEach(_) => "遍历循环",
        Stmt::Break(_) => "跳出",
        Stmt::Continue(_) => "继续",
        Stmt::Return(_) => "返回",
        Stmt::Defer(_) => "稍后",
        Stmt::Import(_) => "导入",
        Stmt::Visibility(_) => "可见性修饰",
        Stmt::Class(_) => "模型",
        Stmt::MethodDef(_) => "方法定义",
        Stmt::Interface(_) => "接口",
        Stmt::TypeDef(_) => "类型定义",
        Stmt::FieldAssign(_) => "字段赋值",
        Stmt::IndexAssign(_) => "下标赋值",
    }
}

fn expr_kind_name(expr: &Expr) -> &'static str {
    match expr {
        Expr::Integer(_) => "整数",
        Expr::Float(_) => "小数",
        Expr::Str(_) => "字符串",
        Expr::InterpStr(_) => "插值字符串",
        Expr::Char(_) => "字符",
        Expr::Bool(_) => "布尔",
        Expr::Nil(_) => "空",
        Expr::Ident(_) => "标识符",
        Expr::Unary(_) => "一元运算",
        Expr::Binary(_) => "二元运算",
        Expr::Ternary(_) => "三元运算",
        Expr::Update(_) => "自增自减",
        Expr::MaybeCall(_) => "无括号调用",
        Expr::Call(_) => "调用",
        Expr::Index(_) => "下标",
        Expr::Slice(_) => "切片",
        Expr::Member(_) => "成员访问",
        Expr::ImplicitReceiverCall(_) => "隐式接收者调用",
        Expr::Array(_) => "数组",
        Expr::Dict(_) => "字典",
        Expr::Spread(_) => "展开",
        Expr::Tuple(_) => "元组",
        Expr::Function(_) => "函数",
        Expr::Composite(_) => "复合字面量",
        Expr::TypeTest(_) => "类型判断",
        Expr::TypeCast(_) => "类型转换",
        Expr::Check(_) => "检查表达式",
        Expr::Through(_) => "通过表达式",
        Expr::Tag(_) => "标签",
    }
}
