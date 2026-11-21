//! AST → 字节码的代码生成。
//!
//! 编译器只依赖 fan-ast 与本 crate 的字节码类型，不包含任何执行逻辑。M1 阶段
//! 覆盖最小命令式子集：字面量、变量、运算、if/while、普通函数与原生调用；
//! 闭包、对象系统、标签、defer 等在后续里程碑逐步加入。

mod scope;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fan_ast::expr::Expr;
use fan_ast::stmt::Stmt;
use fan_ast::Program;

use crate::chunk::{
    Arity, ChunkBuilder, ClassChunk, FunctionChunk, InterfaceChunk, InterfaceMethodChunk,
    MethodChunk, ModuleChunk, NamedTypeChunk, TagChunk, UpvalueDesc,
};
use crate::opcode::Opcode;
use crate::ConstValue;

use scope::ScopeTracker;

/// 编译错误。
pub type CompileResult<T> = Result<T, String>;

/// 提取参数声明类型名列表（未标注为空串），供接口签名比对序列化。
fn param_type_names(params: &[fan_ast::Parameter]) -> Vec<String> {
    params.iter().map(|p| p.type_.0.clone()).collect()
}

/// 提取返回值声明类型名列表（未标注为空串）。
fn return_type_names(types: &[fan_ast::DeclType]) -> Vec<String> {
    types.iter().map(|t| t.0.clone()).collect()
}

/// 把一份程序（一个源文件）编译为模块块。
pub fn compile_program(program: &Program) -> CompileResult<ModuleChunk> {
    let functions = Rc::new(RefCell::new(Vec::new()));
    let classes = Rc::new(RefCell::new(Vec::new()));
    let methods = Rc::new(RefCell::new(Vec::new()));
    let interfaces = Rc::new(RefCell::new(Vec::new()));
    let named_types = Rc::new(RefCell::new(Vec::new()));
    let tags = Rc::new(RefCell::new(Vec::new()));
    let native_exports = Rc::new(RefCell::new(Vec::new()));
    let exports = Rc::new(RefCell::new(Vec::new()));
    let imports = Rc::new(RefCell::new(Vec::new()));
    let native_aliases = Rc::new(RefCell::new(HashMap::new()));
    let mut compiler = Compiler::new(
        functions,
        classes,
        methods,
        interfaces,
        named_types,
        tags,
        native_exports,
        exports,
        imports,
        native_aliases,
    );
    // 函数提升：预扫描顶层函数声明，先在根作用域占槽，使前置引用（后定义的函数
    // 在更早的函数体内被调用）能解析到同一局部槽，与 tree 求值器的模块作用域对齐。
    compiler.predeclare_top_level_functions(&program.statements);
    compiler.compile_statements(&program.statements)?;
    // 入口块以 Halt 收尾，停机时值栈顶即顶层最后一个表达式结果（若有）。
    compiler.builder.emit(Opcode::Halt);
    let entry = compiler.builder.build();

    let functions = Rc::try_unwrap(compiler.functions)
        .expect("函数表无其他引用")
        .into_inner();
    let classes = Rc::try_unwrap(compiler.classes)
        .expect("类表无其他引用")
        .into_inner();
    let methods = Rc::try_unwrap(compiler.methods)
        .expect("方法表无其他引用")
        .into_inner();
    let interfaces = Rc::try_unwrap(compiler.interfaces)
        .expect("接口表无其他引用")
        .into_inner();
    let named_types = Rc::try_unwrap(compiler.named_types)
        .expect("命名类型表无其他引用")
        .into_inner();
    let tags = Rc::try_unwrap(compiler.tags)
        .expect("标签表无其他引用")
        .into_inner();
    let native_exports = Rc::try_unwrap(compiler.native_exports)
        .expect("原生导出表无其他引用")
        .into_inner();
    let exports = Rc::try_unwrap(compiler.exports)
        .expect("导出表无其他引用")
        .into_inner();
    let imports = Rc::try_unwrap(compiler.imports)
        .expect("导入表无其他引用")
        .into_inner();
    Ok(ModuleChunk {
        entry,
        functions,
        classes,
        methods,
        interfaces,
        named_types,
        tags,
        native_exports,
        exports,
        imports,
        top_slots: compiler.scope.root_slots(),
    })
}

struct Compiler<'p> {
    builder: ChunkBuilder,
    /// 模块内共享的函数表：嵌套函数按追加顺序登记，MakeClosure 以索引引用。
    functions: Rc<RefCell<Vec<FunctionChunk>>>,
    /// 模块内共享的模型（类）表，BindClass 以索引引用。
    classes: Rc<RefCell<Vec<ClassChunk>>>,
    /// 模块内共享的模型方法表。
    methods: Rc<RefCell<Vec<MethodChunk>>>,
    /// 模块内共享的接口元数据表。
    interfaces: Rc<RefCell<Vec<InterfaceChunk>>>,
    /// 模块内共享的自定义命名类型表。
    named_types: Rc<RefCell<Vec<NamedTypeChunk>>>,
    /// 模块内共享的标签（装饰器）表，ApplyTag 以索引引用。
    tags: Rc<RefCell<Vec<TagChunk>>>,
    /// 公开导出的原生别名（凡语函数名 → Runtime spec 名），供加载器导出真实 Builtin。
    native_exports: Rc<RefCell<Vec<(String, String)>>>,
    /// 模块导出/导入名，模块内所有函数编译器共享。
    exports: Rc<RefCell<Vec<String>>>,
    imports: Rc<RefCell<Vec<String>>>,
    /// `定义 本地` 建立的「凡语函数名 → 原生 spec 名」别名，模块内共享。
    /// 调用命中别名时发 CallNative 直接走 Runtime 原生表，不生成字节码函数体。
    native_aliases: Rc<RefCell<HashMap<String, String>>>,
    scope: ScopeTracker,
    /// 当前函数登记的 upvalue 描述，顺序即运行期 upvalue 槽位顺序。
    upvalues: Vec<UpvalueDesc>,
    /// 外层函数编译器；模块入口为 None。裸指针仅在递归编译期间使用，
    /// 父编译器在整个子编译器生命周期内都存活于调用栈上。
    parent: Option<*mut Compiler<'p>>,
    /// 当前所在函数的参数名（空表示模块入口），用于把参数映射到前置槽位。
    params: Vec<String>,
    /// 是否正在编译模块入口（决定顶层表达式语句是否保留值）。
    in_module: bool,
    /// 嵌套循环的跳转信息栈：`(break 目标待回填偏移, continue 目标待回填偏移)`。
    loops: Vec<LoopCtx>,
    /// `通过 模块 表达式` 的模块别名栈：编译内部表达式期间，裸标识符改走该模块成员。
    through_modules: Vec<String>,
}

/// 一个循环的控制流回填点。
struct LoopCtx {
    /// 循环内 `跳出` 的跳转指令偏移列表，循环结束时统一回填到出口。
    breaks: Vec<usize>,
    /// `继续` 的跳转指令偏移列表，回填到循环的条件/步进起点。
    continues: Vec<usize>,
}

impl<'p> Compiler<'p> {
    /// 构造编译器实例；模块入口与子函数编译器共用同一组共享表。
    #[allow(clippy::too_many_arguments)]
    fn new(
        functions: Rc<RefCell<Vec<FunctionChunk>>>,
        classes: Rc<RefCell<Vec<ClassChunk>>>,
        methods: Rc<RefCell<Vec<MethodChunk>>>,
        interfaces: Rc<RefCell<Vec<InterfaceChunk>>>,
        named_types: Rc<RefCell<Vec<NamedTypeChunk>>>,
        tags: Rc<RefCell<Vec<TagChunk>>>,
        native_exports: Rc<RefCell<Vec<(String, String)>>>,
        exports: Rc<RefCell<Vec<String>>>,
        imports: Rc<RefCell<Vec<String>>>,
        native_aliases: Rc<RefCell<HashMap<String, String>>>,
    ) -> Self {
        Compiler {
            builder: ChunkBuilder::new(),
            functions,
            classes,
            methods,
            interfaces,
            named_types,
            tags,
            native_exports,
            exports,
            imports,
            native_aliases,
            scope: ScopeTracker::new(),
            upvalues: Vec::new(),
            parent: None,
            params: Vec::new(),
            in_module: true,
            loops: Vec::new(),
            through_modules: Vec::new(),
        }
    }

    /// 构造编译某个嵌套函数体的子编译器，挂接父链以支持 upvalue 递归解析。
    fn child(&self) -> Compiler<'p> {
        let mut child = Compiler::new(
            Rc::clone(&self.functions),
            Rc::clone(&self.classes),
            Rc::clone(&self.methods),
            Rc::clone(&self.interfaces),
            Rc::clone(&self.named_types),
            Rc::clone(&self.tags),
            Rc::clone(&self.native_exports),
            Rc::clone(&self.exports),
            Rc::clone(&self.imports),
            Rc::clone(&self.native_aliases),
        );
        child.in_module = false;
        child.parent = Some(self as *const _ as *mut Compiler<'p>);
        child
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

    /// 预扫描顶层语句，把函数声明（含 `公开` 可见性包裹、`定义 本地` 原生绑定）
    /// 的名字提前在根作用域占槽，实现模块级函数提升。
    fn predeclare_top_level_functions(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::VarDecl(d) => {
                    if matches!(&d.value, Some(Expr::Function(_))) {
                        self.scope.declare_or_reuse_root(&d.name);
                    }
                }
                // 公开 等可见性包裹内部声明。
                Stmt::Visibility(v) => {
                    self.predeclare_top_level_functions(std::slice::from_ref(&v.inner))
                }
                _ => {}
            }
        }
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> CompileResult<()> {
        match stmt {
            Stmt::VarDecl(d) => {
                self.line(d.pos.line as u32);
                // `定义 本地(..) 函数 名(..)`：原生别名在 compile_function_literal
                // 内登记，此处只声明名字槽位，不压值也不 StoreLocal（栈上无函数值）。
                let is_native = matches!(
                    d.value.as_ref(),
                    Some(Expr::Function(f)) if f.native.is_some()
                );
                if is_native {
                    if let Some(Expr::Function(f)) = &d.value {
                        self.compile_expr(&Expr::Function(f.clone()))?;
                        // 公开的原生别名没有运行期槽值，记录 (凡语名, spec) 供加载器导出 Builtin。
                        if matches!(d.visibility, fan_ast::stmt::Visibility::Public) {
                            if let Some(decl) = &f.native {
                                self.native_exports
                                    .borrow_mut()
                                    .push((d.name.clone(), decl.spec.clone()));
                            }
                        }
                    }
                    if self.scope.resolve(&d.name).is_none() {
                        self.scope.declare(&d.name);
                    }
                    if matches!(d.visibility, fan_ast::stmt::Visibility::Public) {
                        let mut exports = self.exports.borrow_mut();
                        if !exports.contains(&d.name) {
                            exports.push(d.name.clone());
                        }
                    }
                    return Ok(());
                }
                if let Some(value) = &d.value {
                    self.compile_expr(value)?;
                } else {
                    let nil = self.builder.add_constant(ConstValue::Nil);
                    self.builder.emit(Opcode::Const(nil));
                }
                // 函数声明携带标签（装饰器）时，记录绑定槽位以便绑定后应用标签。
                let tag_func = match &d.value {
                    Some(Expr::Function(f)) if !f.tags.is_empty() => Some(f.clone()),
                    _ => None,
                };
                // 隐式声明（无 `变量` 关键字的 `甲 = 值`）在已有同名变量时是赋值，
                // 与树遍历求值器一致：依次解析局部槽、闭包 upvalue，命中即原地写入，
                // 都未命中才声明新局部槽。
                if !d.is_explicit {
                    if let Some(existing) = self.scope.resolve(&d.name) {
                        self.builder.emit(Opcode::StoreLocal(existing));
                        if let Some(f) = &tag_func {
                            self.emit_apply_tags(f, existing)?;
                        }
                        return Ok(());
                    }
                    if let Some(up) = self.resolve_upvalue(&d.name)? {
                        self.builder.emit(Opcode::StoreUpvalue(up));
                        return Ok(());
                    }
                    let slot = self.scope.declare_or_reuse_root(&d.name);
                    self.builder.emit(Opcode::StoreLocal(slot));
                    if let Some(f) = &tag_func {
                        self.emit_apply_tags(f, slot)?;
                    }
                } else {
                    // 函数提升可能已在根作用域预占同名槽，此处复用而非重新分配。
                    let slot = self.scope.declare_or_reuse_root(&d.name);
                    self.builder.emit(Opcode::StoreLocal(slot));
                    if let Some(f) = &tag_func {
                        self.emit_apply_tags(f, slot)?;
                    }
                }
                if matches!(d.visibility, fan_ast::stmt::Visibility::Public) {
                    self.exports.borrow_mut().push(d.name.clone());
                }
                Ok(())
            }
            Stmt::Assign(a) => {
                self.line(a.pos.line as u32);
                self.compile_expr(&a.value)?;
                self.emit_store_name(&a.name)
            }
            Stmt::CompoundAssign(c) => self.compile_compound_assign(c),
            Stmt::MultiDecl(d) => {
                self.line(d.pos.line as u32);
                // 无初值多变量声明（`变量 甲、乙`）解析为空元组：为每个名字压入空值，
                // 不走 UnpackTuple（没有可解构的元素）。
                let is_empty_tuple = matches!(&d.value, Expr::Tuple(t) if t.elements.is_empty());
                if is_empty_tuple {
                    let nil = self.builder.add_constant(ConstValue::Nil);
                    for name in d.names.iter().rev() {
                        let slot = self.scope.declare(name);
                        self.builder.emit(Opcode::Const(nil));
                        self.builder.emit(Opcode::StoreLocal(slot));
                    }
                    return Ok(());
                }
                self.compile_expr(&d.value)?;
                self.builder.emit(Opcode::UnpackTuple(d.names.len() as u16));
                // 栈上现为按序的 n 个值，逆序弹出写入各名字槽位。
                for name in d.names.iter().rev() {
                    let slot = self.scope.declare(name);
                    self.builder.emit(Opcode::StoreLocal(slot));
                }
                Ok(())
            }
            Stmt::MultiAssign(a) => {
                self.line(a.pos.line as u32);
                self.compile_expr(&a.value)?;
                self.builder.emit(Opcode::UnpackTuple(a.names.len() as u16));
                for name in a.names.iter().rev() {
                    self.emit_store_name(name)?;
                }
                Ok(())
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
            Stmt::Switch(s) => self.compile_switch(s),
            Stmt::While(w) => self.compile_while(w),
            Stmt::Repeat(r) => self.compile_repeat(r),
            Stmt::Times(t) => self.compile_times(t),
            Stmt::ForEach(f) => self.compile_foreach(f),
            Stmt::Return(r) => {
                self.line(r.pos.line as u32);
                match r.values.as_slice() {
                    [] => {
                        let nil = self.builder.add_constant(ConstValue::Nil);
                        self.builder.emit(Opcode::Const(nil));
                        self.builder.emit(Opcode::Return);
                    }
                    // `返回 局部标识符`：发 ReturnLocal 记录槽位，使帧退出时执行的
                    // 稍后 若改写该局部变量，返回值能反映最新值（对齐 tree 语义）。
                    [Expr::Ident(id)] if self.scope.resolve(&id.name).is_some() => {
                        let slot = self.scope.resolve(&id.name).expect("已确认命中局部");
                        self.builder.emit(Opcode::ReturnLocal(slot));
                    }
                    [single] => {
                        self.compile_expr(single)?;
                        self.builder.emit(Opcode::Return);
                    }
                    many => {
                        for e in many {
                            self.compile_expr(e)?;
                        }
                        self.builder.emit(Opcode::BuildTuple(many.len() as u16));
                        self.builder.emit(Opcode::Return);
                    }
                }
                Ok(())
            }
            Stmt::Break(_) => {
                let ctx = self
                    .loops
                    .last_mut()
                    .ok_or_else(|| "跳出 只能出现在循环内".to_string())?;
                ctx.breaks.push(self.builder.emit(Opcode::Jump(0)));
                Ok(())
            }
            Stmt::Continue(_) => {
                let ctx = self
                    .loops
                    .last_mut()
                    .ok_or_else(|| "继续 只能出现在循环内".to_string())?;
                ctx.continues.push(self.builder.emit(Opcode::Jump(0)));
                Ok(())
            }
            Stmt::IndexAssign(a) => {
                self.line(a.pos.line as u32);
                // 栈布局：容器、下标、值依次压栈，SetIndex 无操作数依次弹出。
                self.compile_expr(&a.target)?;
                self.compile_expr(&a.index)?;
                self.compile_expr(&a.value)?;
                self.builder.emit(Opcode::SetIndex);
                Ok(())
            }
            Stmt::FieldAssign(a) => {
                self.line(a.pos.line as u32);
                // 栈布局：对象、值依次压栈，成员名取常量池。
                self.compile_expr(&a.object)?;
                self.compile_expr(&a.value)?;
                let name_idx = self.add_string_const(&a.name);
                self.builder.emit(Opcode::SetMember(name_idx));
                Ok(())
            }
            Stmt::Import(import) => {
                self.line(import.pos.line as u32);
                // 记录静态路径供装配层预编译/校验；运行期由注入的加载器解析。
                if !self.imports.borrow().contains(&import.path) {
                    self.imports.borrow_mut().push(import.path.clone());
                }
                let path_idx = self.add_string_const(&import.path);
                self.builder.emit(Opcode::LoadModule(path_idx));
                // 模块对象绑定到别名（默认名取路径末段），作为当前作用域局部。
                let slot = self.scope.declare(&import.name);
                self.builder.emit(Opcode::StoreLocal(slot));
                Ok(())
            }
            Stmt::Visibility(vis) => {
                self.line(vis.pos.line as u32);
                // 先编译内部声明（函数/变量等），再按可见性登记导出名。
                self.compile_stmt(&vis.inner)?;
                if vis.visibility == fan_ast::stmt::Visibility::Public {
                    let mut exports = self.exports.borrow_mut();
                    if !exports.contains(&vis.name) {
                        exports.push(vis.name.clone());
                    }
                }
                Ok(())
            }
            Stmt::Class(c) => self.compile_class(c),
            Stmt::Interface(i) => self.compile_interface(i),
            Stmt::TypeDef(t) => self.compile_type_def(t),
            Stmt::MethodDef(m) => self.compile_method_def(m),
            Stmt::Defer(d) => self.compile_defer(d),
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

    /// 把 `判断` 降级为线性分支链，语义对齐 tree eval_switch：
    /// 守卫分支按顺序求值布尔条件；值匹配分支拿 subject 与候选值逐个「等于」比较，
    /// 命中即执行分支体并跳过其余分支；`其他` 作为默认分支放最后；无命中且无默认时落空。
    fn compile_switch(&mut self, s: &fan_ast::stmt::SwitchStmt) -> CompileResult<()> {
        self.line(s.pos.line as u32);
        // subject 只求值一次，存入编译器生成的局部槽（独立作用域，名字对用户不可见）。
        let subject_name = format!("$判断目标@{:x}", self.builder.code_len());
        self.scope.push();
        if let Some(subject) = &s.subject {
            let slot = self.scope.declare(&subject_name);
            self.compile_expr(subject)?;
            self.builder.emit(Opcode::StoreLocal(slot));
        }
        let has_default = s.branches.iter().any(|b| b.default);
        let mut end_jumps = Vec::new();
        for branch in &s.branches {
            if branch.default {
                continue;
            }
            if branch.guard {
                self.compile_expr(&branch.values[0])?;
            } else {
                if s.subject.is_none() {
                    return Err("判断 值匹配分支缺少匹配对象".to_string());
                }
                let eq_op = self.add_string_const("等于");
                let mut short_circuit = Vec::new();
                for (i, candidate) in branch.values.iter().enumerate() {
                    if i > 0 {
                        // 前一个比较为假时弹出假值，继续算下一个候选；为真则跳过后续比较。
                        short_circuit.push(self.builder.emit(Opcode::JumpIfTrue(0)));
                        self.builder.emit(Opcode::Pop);
                    }
                    self.emit_load_name(&subject_name)?;
                    self.compile_expr(candidate)?;
                    self.builder.emit(Opcode::BinOp(eq_op));
                }
                // 短路真跳转全部落到栈顶这个最终布尔上。
                for j in short_circuit {
                    self.builder.patch_jump(j);
                }
            }
            let skip_body = self.builder.emit(Opcode::JumpIfFalse(0));
            self.builder.emit(Opcode::Pop);
            self.compile_block(&branch.body.statements)?;
            end_jumps.push(self.builder.emit(Opcode::Jump(0)));
            self.builder.patch_jump(skip_body);
            self.builder.emit(Opcode::Pop);
        }
        if has_default {
            let default = s.branches.iter().find(|b| b.default).unwrap();
            self.compile_block(&default.body.statements)?;
        }
        for j in end_jumps {
            self.builder.patch_jump(j);
        }
        self.scope.pop();
        Ok(())
    }

    fn compile_while(&mut self, w: &fan_ast::stmt::WhileStmt) -> CompileResult<()> {
        self.line(w.pos.line as u32);
        let loop_start = self.builder.code_len();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        self.compile_expr(&w.condition)?;
        let exit = self.builder.emit(Opcode::JumpIfFalse(0));
        self.builder.emit(Opcode::Pop);
        self.compile_block(&w.body.statements)?;
        self.builder.emit(Opcode::Jump(loop_start as u32));
        self.builder.patch_jump(exit);
        self.builder.emit(Opcode::Pop); // 退出时弹出条件假值
        let after_pop = self.builder.code_len();
        let ctx = self.loops.pop().expect("循环上下文存在");
        for jump in ctx.continues {
            self.builder.patch_jump_to(jump, loop_start);
        }
        for jump in ctx.breaks {
            self.builder.patch_jump_to(jump, after_pop);
        }
        Ok(())
    }

    /// `重复 ... 直到 条件`：后判断循环，至少执行一次，条件为真时退出。
    fn compile_repeat(&mut self, r: &fan_ast::stmt::RepeatStmt) -> CompileResult<()> {
        self.line(r.pos.line as u32);
        let loop_start = self.builder.code_len();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continues: Vec::new(),
            // 条件在循环体之后才求值，体执行（含 break）时栈上尚无布尔残留。
        });
        self.compile_block(&r.body.statements)?;
        let cond_start = self.builder.code_len();
        self.compile_expr(&r.until)?;
        let exit = self.builder.emit(Opcode::JumpIfTrue(0));
        self.builder.emit(Opcode::Pop);
        self.builder.emit(Opcode::Jump(loop_start as u32));
        self.builder.patch_jump(exit);
        self.builder.emit(Opcode::Pop);
        let after_pop = self.builder.code_len();
        let ctx = self.loops.pop().expect("循环上下文存在");
        for jump in ctx.continues {
            self.builder.patch_jump_to(jump, cond_start);
        }
        for jump in ctx.breaks {
            self.builder.patch_jump_to(jump, after_pop);
        }
        Ok(())
    }

    /// `重复 N 次`：把计数存入隐藏槽位，每轮递减，归零退出。
    fn compile_times(&mut self, t: &fan_ast::stmt::TimesStmt) -> CompileResult<()> {
        self.line(t.pos.line as u32);
        self.compile_expr(&t.count)?;
        let counter = self.scope.declare("__times_counter__");
        self.builder.emit(Opcode::StoreLocal(counter));

        let loop_start = self.builder.code_len();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        self.builder.emit(Opcode::LoadLocal(counter));
        let zero = self.builder.add_constant(ConstValue::Int(0));
        self.builder.emit(Opcode::Const(zero));
        let op = self.add_string_const("小于等于");
        self.builder.emit(Opcode::BinOp(op));
        let exit = self.builder.emit(Opcode::JumpIfTrue(0));
        self.builder.emit(Opcode::Pop);

        self.compile_block(&t.body.statements)?;

        // counter = counter - 1
        self.builder.emit(Opcode::LoadLocal(counter));
        let one = self.builder.add_constant(ConstValue::Int(1));
        self.builder.emit(Opcode::Const(one));
        let sub = self.add_string_const("减");
        self.builder.emit(Opcode::BinOp(sub));
        self.builder.emit(Opcode::StoreLocal(counter));
        self.builder.emit(Opcode::Jump(loop_start as u32));

        self.builder.patch_jump(exit);
        self.builder.emit(Opcode::Pop);
        let after_pop = self.builder.code_len();
        let ctx = self.loops.pop().expect("循环上下文存在");
        for jump in ctx.continues {
            self.builder.patch_jump_to(jump, loop_start);
        }
        for jump in ctx.breaks {
            self.builder.patch_jump_to(jump, after_pop);
        }
        Ok(())
    }

    /// `遍历 可迭代 中的 元素[, 序号]`：把可迭代对象与计数器存隐藏槽位，
    /// 用 `ForStep` 在运行期按类型推进并绑定迭代变量，条件为假时退出。
    fn compile_foreach(&mut self, f: &fan_ast::stmt::ForEachStmt) -> CompileResult<()> {
        self.line(f.pos.line as u32);
        use fan_ast::expr::Expr as E;
        // 数组、字符串、字典字面量在编译期即可确定标签；其余复杂表达式
        // （函数调用、成员取值等）用动态标签，由 VM 运行期按实际类型分派。
        let tag = match &f.iterable {
            E::Array(_) => crate::opcode::FOR_TAG_ARRAY,
            E::Str(_) | E::Char(_) => crate::opcode::FOR_TAG_STRING,
            E::Dict(_) => crate::opcode::FOR_TAG_DICT,
            _ => crate::opcode::FOR_TAG_DYNAMIC,
        };

        // 求值可迭代对象并保存到隐藏槽。
        self.compile_expr(&f.iterable)?;
        let iter_slot = self.scope.declare("__foreach_iter__");
        self.builder.emit(Opcode::StoreLocal(iter_slot));
        let counter_slot = self.scope.declare("__foreach_counter__");
        let zero = self.builder.add_constant(ConstValue::Int(0));
        self.builder.emit(Opcode::Const(zero));
        self.builder.emit(Opcode::StoreLocal(counter_slot));

        // 先在当前作用域分配迭代变量槽位，再推进循环体作用域让名字在体内可见，
        // 退出循环后这些名字不可见（与树遍历求值器的作用域一致）。
        let elem_slot = self.scope.declare(&f.first_name);
        let index_slot = match &f.second_name {
            Some(name) => self.scope.declare(name),
            None => crate::opcode::FOR_NO_INDEX_SLOT,
        };
        self.scope.push();
        self.scope.declare_at(&f.first_name, elem_slot);
        if let Some(name) = &f.second_name {
            self.scope.declare_at(name, index_slot);
        }

        let loop_start = self.builder.code_len();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continues: Vec::new(),
            // ForStep 布尔在进入循环体前已由 JumpIfFalse 后的 Pop 消费，
            // 故 break 时值栈无残留布尔，跳到出口 Pop 之后即可。
        });
        self.builder.emit(Opcode::ForStep(
            tag,
            iter_slot,
            counter_slot,
            elem_slot,
            index_slot,
        ));
        let exit = self.builder.emit(Opcode::JumpIfFalse(0));
        self.builder.emit(Opcode::Pop); // 弹出“还有下一轮”的真值
        for stmt in &f.body.statements {
            self.compile_stmt(stmt)?;
        }
        self.scope.pop();
        self.builder.emit(Opcode::Jump(loop_start as u32));

        self.builder.patch_jump(exit);
        self.builder.emit(Opcode::Pop); // 退出时弹出假值
        let after_pop = self.builder.code_len();
        let ctx = self.loops.pop().expect("循环上下文存在");
        for jump in ctx.continues {
            self.builder.patch_jump_to(jump, loop_start);
        }
        for jump in ctx.breaks {
            self.builder.patch_jump_to(jump, after_pop);
        }
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
            Expr::InterpStr(interp) => self.compile_interp_string(interp),
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
            Expr::Update(u) => {
                self.line(u.pos.line as u32);
                self.compile_update(u)?;
                Ok(())
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
                // 短路逻辑：左值保留在栈上，满足条件时直接作为结果，不计算右值。
                if b.op == "且" || b.op == "&&" {
                    self.compile_expr(&b.left)?;
                    let skip = self.builder.emit(Opcode::JumpIfFalse(0));
                    self.builder.emit(Opcode::Pop); // 左值为真：弹出后求右值
                    self.compile_expr(&b.right)?;
                    self.builder.patch_jump(skip);
                    Ok(())
                } else if b.op == "或" || b.op == "||" {
                    self.compile_expr(&b.left)?;
                    let skip = self.builder.emit(Opcode::JumpIfTrue(0));
                    self.builder.emit(Opcode::Pop); // 左值为假：弹出后求右值
                    self.compile_expr(&b.right)?;
                    self.builder.patch_jump(skip);
                    Ok(())
                } else {
                    self.compile_expr(&b.left)?;
                    self.compile_expr(&b.right)?;
                    let op = self.add_string_const(&b.op);
                    self.builder.emit(Opcode::BinOp(op));
                    Ok(())
                }
            }
            Expr::Call(call) => self.compile_call(call),
            Expr::Through(t) => self.compile_through(t),
            Expr::Composite(lit) => self.compile_composite(lit),
            Expr::TypeTest(t) => {
                self.line(t.pos.line as u32);
                self.compile_expr(&t.object)?;
                let name_idx = self.add_string_const(&t.type_name);
                self.builder.emit(Opcode::TypeTest(name_idx));
                Ok(())
            }
            Expr::TypeCast(t) => {
                self.line(t.pos.line as u32);
                self.compile_expr(&t.object)?;
                let name_idx = self.add_string_const(&t.type_name);
                self.builder.emit(Opcode::TypeCast(name_idx));
                Ok(())
            }
            Expr::ImplicitReceiverCall(c) => self.compile_implicit_receiver_call(c),
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
            Expr::Member(m) => {
                self.line(m.pos.line as u32);
                self.compile_expr(&m.object)?;
                let name_idx = self.add_string_const(&m.name);
                self.builder.emit(Opcode::GetMember(name_idx));
                Ok(())
            }
            Expr::Tuple(t) => {
                self.line(t.pos.line as u32);
                for element in &t.elements {
                    self.compile_expr(element)?;
                }
                self.builder
                    .emit(Opcode::BuildTuple(t.elements.len() as u16));
                Ok(())
            }
            Expr::Array(a) => {
                self.line(a.pos.line as u32);
                let has_spread = a
                    .elements
                    .iter()
                    .any(|e| matches!(e, fan_ast::expr::Expr::Spread(_)));
                if has_spread {
                    // 含展开：压标记后逐项编译，展开项发 Spread 摊平，最后按标记结算数组。
                    self.builder.emit(Opcode::PushMark);
                    for element in &a.elements {
                        if let fan_ast::expr::Expr::Spread(s) = element {
                            self.compile_expr(&s.value)?;
                            self.builder.emit(Opcode::Spread);
                        } else {
                            self.compile_expr(element)?;
                        }
                    }
                    self.builder.emit(Opcode::BuildArrayMarked);
                } else {
                    for element in &a.elements {
                        self.compile_expr(element)?;
                    }
                    self.builder
                        .emit(Opcode::BuildArray(a.elements.len() as u16));
                }
                Ok(())
            }
            Expr::Dict(d) => {
                self.line(d.pos.line as u32);
                let has_spread = d.pairs.iter().any(|p| p.spread);
                if has_spread {
                    // 含字典展开：压标记，普通项按键值压栈，展开项发 SpreadDict 摊平。
                    self.builder.emit(Opcode::PushMark);
                    for pair in &d.pairs {
                        if pair.spread {
                            self.compile_expr(&pair.value)?;
                            self.builder.emit(Opcode::SpreadDict);
                        } else {
                            let key = pair.key.as_ref().expect("非展开字典项必须有键表达式");
                            self.compile_expr(key)?;
                            self.compile_expr(&pair.value)?;
                        }
                    }
                    self.builder.emit(Opcode::BuildDictMarked);
                } else {
                    for pair in &d.pairs {
                        let key = pair.key.as_ref().expect("非展开字典项必须有键表达式");
                        self.compile_expr(key)?;
                        self.compile_expr(&pair.value)?;
                    }
                    self.builder.emit(Opcode::BuildDict(d.pairs.len() as u16));
                }
                Ok(())
            }
            Expr::Index(idx) => {
                self.line(idx.pos.line as u32);
                self.compile_expr(&idx.target)?;
                self.compile_expr(&idx.index)?;
                self.builder.emit(Opcode::GetIndex);
                Ok(())
            }
            Expr::Slice(s) => {
                self.line(s.pos.line as u32);
                // 栈布局：target、low、high；省略的边界压 Nil 哨兵。
                self.compile_expr(&s.target)?;
                match &s.low {
                    Some(low) => self.compile_expr(low)?,
                    None => {
                        let nil = self.builder.add_constant(ConstValue::Nil);
                        self.builder.emit(Opcode::Const(nil));
                    }
                }
                match &s.high {
                    Some(high) => self.compile_expr(high)?,
                    None => {
                        let nil = self.builder.add_constant(ConstValue::Nil);
                        self.builder.emit(Opcode::Const(nil));
                    }
                }
                self.builder.emit(Opcode::GetSlice);
                Ok(())
            }
            other => Err(format!(
                "字节码编译器暂不支持的表达式：{}",
                expr_kind_name(other)
            )),
        }
    }

    fn compile_call(&mut self, call: &fan_ast::expr::CallExpr) -> CompileResult<()> {
        self.line(call.pos.line as u32);
        // 异常控制流关键字：恢复() 取当前待恢复错误；崩溃了(描述) 抛规范错误对象。
        // 仅在名字未被局部/upvalue 绑定时生效（用户自定义同名函数优先），与 tree 对齐。
        if let fan_ast::expr::Expr::Ident(id) = &*call.callee {
            let bound =
                self.scope.resolve(&id.name).is_some() || self.resolve_upvalue(&id.name)?.is_some();
            if !bound && id.name == "恢复" {
                if !call.args.is_empty() {
                    return Err("恢复 不接受参数".to_string());
                }
                self.builder.emit(Opcode::Recover);
                return Ok(());
            }
            if !bound && id.name == "崩溃了" {
                if call.args.len() != 1 {
                    return Err(format!(
                        "崩溃了 需要 1 个参数（错误描述），实际 {} 个",
                        call.args.len()
                    ));
                }
                self.compile_expr(&call.args[0])?;
                self.builder.emit(Opcode::Throw);
                return Ok(());
            }
            // 命名类型构造：`状态(1)`、`姓名("凡")`，类型名不占变量槽，
            // 仅当未被局部/upvalue 绑定时按命名类型包装处理（用户同名绑定优先）。
            if !bound && self.is_named_type(&id.name) {
                if call.args.len() != 1 {
                    return Err(format!(
                        "命名类型 {} 构造需要恰好 1 个参数，实际 {} 个",
                        id.name,
                        call.args.len()
                    ));
                }
                if matches!(&call.args[0], fan_ast::expr::Expr::Spread(_)) {
                    return Err("命名类型构造暂不支持展开实参 ...".to_string());
                }
                self.compile_expr(&call.args[0])?;
                let name_idx = self.add_string_const(&id.name);
                self.builder.emit(Opcode::NewNamed(name_idx));
                return Ok(());
            }
        }
        // 原生别名：`定义 本地` 声明的凡语函数名直接映射到 Runtime 原生 spec。
        let native_spec = if let fan_ast::expr::Expr::Ident(id) = &*call.callee {
            self.native_aliases.borrow().get(&id.name).cloned()
        } else {
            None
        };
        if let Some(spec) = native_spec {
            let has_spread = call
                .args
                .iter()
                .any(|a| matches!(a, fan_ast::expr::Expr::Spread(_)));
            if has_spread {
                self.builder.emit(Opcode::PushMark);
                for arg in &call.args {
                    match arg {
                        fan_ast::expr::Expr::Spread(s) => {
                            self.compile_expr(&s.value)?;
                            self.builder.emit(Opcode::Spread);
                        }
                        other => self.compile_expr(other)?,
                    }
                }
                let spec_idx = self.add_string_const(&spec);
                self.builder.emit(Opcode::CallNativeMarked(spec_idx));
            } else {
                for arg in &call.args {
                    self.compile_expr(arg)?;
                }
                let spec_idx = self.add_string_const(&spec);
                self.builder
                    .emit(Opcode::CallNative(spec_idx, call.args.len() as u8));
            }
            return Ok(());
        }
        let has_spread = call
            .args
            .iter()
            .any(|a| matches!(a, fan_ast::expr::Expr::Spread(_)));
        if has_spread {
            self.builder.emit(Opcode::PushMark);
            for arg in &call.args {
                match arg {
                    fan_ast::expr::Expr::Spread(s) => {
                        self.compile_expr(&s.value)?;
                        self.builder.emit(Opcode::Spread);
                    }
                    other => self.compile_expr(other)?,
                }
            }
            self.compile_expr(&call.callee)?;
            self.builder.emit(Opcode::CallMarked);
        } else {
            for arg in &call.args {
                self.compile_expr(arg)?;
            }
            self.compile_expr(&call.callee)?;
            let argc = call.args.len() as u8;
            self.builder.emit(Opcode::Call(argc));
        }
        Ok(())
    }

    /// 编译隐式接收者调用 `名单 追加 "张三"`，语义等价于 `名单.追加("张三")`：
    /// 先取接收者的同名成员作为可调用值，再以显式实参（不含接收者）调用。
    fn compile_implicit_receiver_call(
        &mut self,
        c: &fan_ast::expr::ImplicitReceiverCallExpr,
    ) -> CompileResult<()> {
        self.line(c.pos.line as u32);
        let has_spread = c
            .args
            .iter()
            .any(|a| matches!(a, fan_ast::expr::Expr::Spread(_)));
        if has_spread {
            self.builder.emit(Opcode::PushMark);
            for arg in &c.args {
                match arg {
                    fan_ast::expr::Expr::Spread(s) => {
                        self.compile_expr(&s.value)?;
                        self.builder.emit(Opcode::Spread);
                    }
                    other => self.compile_expr(other)?,
                }
            }
            self.compile_expr(&c.receiver)?;
            let name_idx = self.add_string_const(&c.name);
            self.builder.emit(Opcode::GetMember(name_idx));
            self.builder.emit(Opcode::CallMarked);
        } else {
            for arg in &c.args {
                self.compile_expr(arg)?;
            }
            self.compile_expr(&c.receiver)?;
            let name_idx = self.add_string_const(&c.name);
            self.builder.emit(Opcode::GetMember(name_idx));
            self.builder.emit(Opcode::Call(c.args.len() as u8));
        }
        Ok(())
    }

    /// 编译 `通过 模块 表达式`：在内部表达式求值期间，把裸标识符解析为该模块的导出
    /// 成员（模块对象 + GetMember），对齐 tree eval_through 注入模块作用域的语义。
    fn compile_through(&mut self, t: &fan_ast::expr::ThroughExpr) -> CompileResult<()> {
        self.line(t.pos.line as u32);
        self.through_modules.push(t.module_name.clone());
        let result = self.compile_expr(&t.expr);
        self.through_modules.pop();
        result
    }

    /// 编译 `模型 X … 结束`：登记类元数据并发出 BindClass 创建运行期类对象，
    /// 再像普通变量一样绑定到类名局部槽，使 `X{...}` 与 `X(...)` 能按名取到类。
    fn compile_class(&mut self, c: &fan_ast::stmt::ClassStmt) -> CompileResult<()> {
        self.line(c.pos.line as u32);
        // 模型上的标签与 tree 求值器一样暂不产生任何效果，静默忽略。
        let field_names: Vec<String> = c.fields.iter().map(|f| f.name.clone()).collect();
        let index = self.classes.borrow().len() as u16;
        self.classes.borrow_mut().push(ClassChunk {
            name: c.name.clone(),
            fields: field_names,
            embeds: c.embeds.clone(),
        });
        self.builder.emit(Opcode::BindClass(index));
        // 类名作为当前作用域局部绑定（含函数提升预声明的槽位复用）。
        let slot = self.scope.declare_or_reuse_root(&c.name);
        self.builder.emit(Opcode::StoreLocal(slot));
        if c.visibility == fan_ast::stmt::Visibility::Public {
            let mut exports = self.exports.borrow_mut();
            if !exports.contains(&c.name) {
                exports.push(c.name.clone());
            }
        }
        Ok(())
    }

    /// 编译 `接口 X … 结束`：接口是纯结构化类型契约，不产生运行期指令，
    /// 仅登记元数据供 VM 注册到 Runtime 做 `是`/`转为` 的满足判定；接口名占一个
    /// 根槽位（值为空），与 tree 求值器声明同名变量保持一致。
    fn compile_interface(&mut self, i: &fan_ast::stmt::InterfaceStmt) -> CompileResult<()> {
        self.line(i.pos.line as u32);
        let methods = i
            .methods
            .iter()
            .map(|m| InterfaceMethodChunk {
                name: m.name.clone(),
                param_types: param_type_names(&m.params),
                return_types: return_type_names(&m.return_types),
            })
            .collect();
        self.interfaces.borrow_mut().push(InterfaceChunk {
            name: i.name.clone(),
            embeds: i.embeds.clone(),
            methods,
        });
        let nil = self.builder.add_constant(ConstValue::Nil);
        self.builder.emit(Opcode::Const(nil));
        let slot = self.scope.declare_or_reuse_root(&i.name);
        self.builder.emit(Opcode::StoreLocal(slot));
        if i.visibility == fan_ast::stmt::Visibility::Public {
            let mut exports = self.exports.borrow_mut();
            if !exports.contains(&i.name) {
                exports.push(i.name.clone());
            }
        }
        Ok(())
    }

    /// 判断名字是否为本模块声明的自定义命名类型。
    fn is_named_type(&self, name: &str) -> bool {
        self.named_types.borrow().iter().any(|t| t.name == name)
    }

    /// 编译 `定义 类型 名 = 底层类型`：命名类型是纯类型身份，不产生运行期指令，
    /// 仅登记元数据供 VM 注册到 Runtime；类型名不占变量槽（它作为构造调用被特判）。
    fn compile_type_def(&mut self, t: &fan_ast::stmt::TypeDefStmt) -> CompileResult<()> {
        self.line(t.pos.line as u32);
        let base_members: Vec<String> = t
            .base_type
            .members()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let is_union = t.base_type.is_union();
        self.named_types.borrow_mut().push(NamedTypeChunk {
            name: t.name.clone(),
            base_members,
            is_union,
        });
        if t.visibility == fan_ast::stmt::Visibility::Public {
            let mut exports = self.exports.borrow_mut();
            if !exports.contains(&t.name) {
                exports.push(t.name.clone());
            }
        }
        Ok(())
    }

    /// 编译复合字面量 `模型{字段: 值, …}`：依次压入字段名常量与字段值，再压入类对象，
    /// 由 NewInstance 在运行期构造零值实例并按名填字段（不调用 初始化，对齐 tree）。
    fn compile_composite(&mut self, lit: &fan_ast::expr::CompositeLit) -> CompileResult<()> {
        self.line(lit.pos.line as u32);
        for field in &lit.fields {
            let name_idx = self.add_string_const(&field.name);
            self.builder.emit(Opcode::Const(name_idx));
            match &field.value {
                Some(value) => self.compile_expr(value)?,
                None => {
                    let nil = self.builder.add_constant(ConstValue::Nil);
                    self.builder.emit(Opcode::Const(nil));
                }
            }
        }
        self.compile_expr(&lit.model)?;
        let class_name = match lit.model.as_ref() {
            Expr::Ident(id) => id.name.clone(),
            other => {
                return Err(format!(
                    "复合字面量暂仅支持标识符模型，实际是 {}",
                    expr_kind_name(other)
                ))
            }
        };
        let name_idx = self.add_string_const(&class_name);
        self.builder
            .emit(Opcode::NewInstance(name_idx, lit.fields.len() as u8));
        Ok(())
    }

    /// 编译 `定义 模型 的 方法 名(…)`：方法体编译为带隐式首参 `自己` 的函数字节码，
    /// 登记到模块函数表并在方法表记录 (类名, 方法名) → 函数索引；不产生运行期栈值。
    /// 基本类型（字符串/数组等）的方法扩展留待后续里程碑。
    fn compile_method_def(&mut self, m: &fan_ast::stmt::MethodDef) -> CompileResult<()> {
        self.line(m.pos.line as u32);
        let f = &m.function;
        // 方法可把接收者显式写成首个形参（`问候(任意 自己)`）：它复用槽 0 且不占实参位，
        // 与 tree 求值器一致；未显式写接收者时仍由隐式 `自己` 占槽 0。
        let explicit_self = f
            .params
            .first()
            .map(|p| p.name == m.receiver_name)
            .unwrap_or(false);
        let body_params: Vec<&fan_ast::Parameter> = if explicit_self {
            f.params.iter().skip(1).collect()
        } else {
            f.params.iter().collect()
        };
        let mut child = self.child();
        // 接收者固定占据槽 0（显式 自己 或隐式 自己 同名）。
        child.scope.declare_at(&m.receiver_name, 0);
        let mut fixed_index: u16 = 1;
        let has_variadic = body_params.iter().any(|p| p.variadic);
        for param in &body_params {
            if param.variadic {
                let slot = body_params.len() as u16; // 接收者(1 槽) + 显式参数序号
                child.scope.declare_at(&param.name, slot);
            } else {
                child.scope.declare_at(&param.name, fixed_index);
                fixed_index += 1;
            }
        }
        // 显式参数的默认值序言（槽位整体后移一位以容纳接收者）。
        for (offset, param) in body_params.iter().enumerate() {
            if let Some(default) = &param.default {
                let slot = (offset + 1) as u16;
                child.line(f.pos.line as u32);
                child.builder.emit(Opcode::ArgCount);
                // 接收者占一个实参，故默认值判定比较的是 argc 与 offset+1。
                let const_idx = child
                    .builder
                    .add_constant(ConstValue::Int((offset + 1) as i64));
                child.builder.emit(Opcode::Const(const_idx));
                let op = child.add_string_const("小于等于");
                child.builder.emit(Opcode::BinOp(op));
                child.builder.emit(Opcode::Dup);
                let skip_default = child.builder.emit(Opcode::JumpIfFalse(0));
                child.builder.emit(Opcode::Pop);
                child.compile_expr(default)?;
                child.builder.emit(Opcode::StoreLocal(slot));
                child.builder.patch_jump(skip_default);
                child.builder.emit(Opcode::Pop);
            }
        }
        child.compile_statements(&f.body.statements)?;
        let nil = child.builder.add_constant(ConstValue::Nil);
        child.builder.emit(Opcode::Const(nil));
        child.builder.emit(Opcode::Return);

        let fixed = body_params.iter().filter(|p| !p.variadic).count() + 1;
        let arity = Arity {
            fixed,
            variadic: has_variadic,
            optional: body_params.iter().filter(|p| p.default.is_some()).count(),
        };
        let chunk = FunctionChunk {
            name: format!("{}#{}", m.class_name, m.method_name),
            arity,
            params: {
                let mut names = vec![m.receiver_name.clone()];
                names.extend(body_params.iter().map(|p| p.name.clone()));
                names
            },
            // 接收者未标注类型（any），与接口签名比对时接收者不计入参数。
            param_types: {
                let mut types = vec![String::new()];
                types.extend(body_params.iter().map(|p| p.type_.0.clone()));
                types
            },
            return_types: return_type_names(&f.return_types),
            body: child.builder.build(),
            upvalues: child.upvalues,
        };
        let function_index = self.functions.borrow().len() as u16;
        self.functions.borrow_mut().push(chunk);
        self.methods.borrow_mut().push(MethodChunk {
            class_name: m.class_name.clone(),
            method_name: m.method_name.clone(),
            function_index,
            primitive: m.primitive,
        });
        Ok(())
    }

    /// 编译 `稍后 表达式`：把延迟工作包成一个零参闭包登记到当前函数帧。
    ///
    /// 两种形态与树遍历求值器一致：
    /// - `稍后 f(实参…)`：实参在**登记时刻**就求值（保留此刻的值），被调用者
    ///   仍在执行时才解析，避免把无参方法提前调用。实现上把每个实参值提升为
    ///   外层局部槽，延迟闭包经 upvalue 读取后再调用。
    /// - `稍后 函数() … 结束`：直接把函数字面量编译为延迟闭包，执行时零参调用。
    fn compile_defer(&mut self, d: &fan_ast::stmt::DeferStmt) -> CompileResult<()> {
        self.line(d.pos.line as u32);
        // 调用形态的实参在登记时刻求值并提升为外层合成临时槽（按名声明，
        // 供子闭包经 upvalue 按名解析），执行时刻取回此刻保存的值。
        let mut arg_temp_names: Vec<String> = Vec::new();
        if let Expr::Call(call) = &d.call {
            for (idx, arg) in call.args.iter().enumerate() {
                if matches!(arg, Expr::Spread(_)) {
                    return Err("稍后 调用暂不支持展开实参 ...".to_string());
                }
                self.compile_expr(arg)?;
                let temp = format!("__稍后实参{}_{}__", d.pos.line, idx);
                let slot = self.scope.declare(&temp);
                self.builder.emit(Opcode::StoreLocal(slot));
                arg_temp_names.push(temp);
            }
        }
        let mut child = self.child();
        match &d.call {
            Expr::Function(_) => {
                // 函数字面量：在子编译器里直接编译该表达式并零参调用。
                child.compile_expr(&d.call)?;
                child.builder.emit(Opcode::Call(0));
                child.builder.emit(Opcode::Pop);
            }
            Expr::Call(call) => {
                // 执行时刻：经 upvalue 取回登记时刻的实参值，再解析被调用者并调用。
                for temp in &arg_temp_names {
                    let up = child
                        .resolve_upvalue(temp)?
                        .ok_or_else(|| "稍后 实参提升为 upvalue 失败".to_string())?;
                    child.builder.emit(Opcode::LoadUpvalue(up));
                }
                // 原生别名与 compile_call 入口对齐：命中则直接发 CallNative，
                // 避免把别名当普通标识符经 upvalue/全局解析成空值。
                let native_spec = if let Expr::Ident(id) = &*call.callee {
                    self.native_aliases.borrow().get(&id.name).cloned()
                } else {
                    None
                };
                if let Some(spec) = native_spec {
                    let spec_idx = child.add_string_const(&spec);
                    child
                        .builder
                        .emit(Opcode::CallNative(spec_idx, call.args.len() as u8));
                } else {
                    child.compile_expr(&call.callee)?;
                    child.builder.emit(Opcode::Call(call.args.len() as u8));
                }
                child.builder.emit(Opcode::Pop);
            }
            other => {
                return Err(format!(
                    "稍后 只支持函数调用或函数字面量，实际是 {}",
                    expr_kind_name(other)
                ));
            }
        }
        let nil = child.builder.add_constant(ConstValue::Nil);
        child.builder.emit(Opcode::Const(nil));
        child.builder.emit(Opcode::Return);

        let chunk = FunctionChunk {
            name: "<稍后>".to_string(),
            arity: Arity {
                fixed: 0,
                variadic: false,
                optional: 0,
            },
            params: Vec::new(),
            param_types: Vec::new(),
            return_types: Vec::new(),
            body: child.builder.build(),
            upvalues: child.upvalues,
        };
        let index = self.functions.borrow().len() as u16;
        self.functions.borrow_mut().push(chunk);
        self.builder.emit(Opcode::MakeClosure(index));
        self.builder.emit(Opcode::RegisterDefer);
        Ok(())
    }

    /// 把函数声明上的全部标签按书写顺序登记并应用：先把每个标签的参数表达式内联
    /// 求值到值栈（位置参数至多一个在前、命名参数值在后），再发 `ApplyTag` 作用于
    /// 绑定槽位。运行时由 VM 构造标签模型实例并回调处理器；未注册标签静默忽略。
    fn emit_apply_tags(&mut self, f: &fan_ast::expr::FunctionLit, slot: u16) -> CompileResult<()> {
        for tag in &f.tags {
            if tag.positional.len() > 1 {
                return Err("标签未命名参数最多一个".to_string());
            }
            let mut argc: u8 = 0;
            if let Some(first) = tag.positional.first() {
                self.compile_expr(first)?;
                argc += 1;
            }
            for arg in &tag.named {
                self.compile_expr(&arg.value)?;
                argc += 1;
            }
            let tag_chunk = TagChunk {
                name: tag.type_name.clone(),
                named_names: tag.named.iter().map(|a| a.name.clone()).collect(),
                has_positional: !tag.positional.is_empty(),
            };
            let index = self.tags.borrow().len() as u16;
            self.tags.borrow_mut().push(tag_chunk);
            self.builder.emit(Opcode::ApplyTag(index, slot, argc));
        }
        Ok(())
    }

    fn compile_function_literal(&mut self, f: &fan_ast::expr::FunctionLit) -> CompileResult<()> {
        self.line(f.pos.line as u32);
        // `定义 本地("spec") 函数 名(...)`：不生成字节码函数体，把凡语函数名登记为
        // Runtime 原生 spec 的别名；调用该名时改发 CallNative。与树遍历求值器的
        // bind_native 对齐：带模块前缀优先按完整 spec，否则按点号后的短名。
        if let Some(decl) = &f.native {
            if !f.name.is_empty() {
                self.native_aliases
                    .borrow_mut()
                    .insert(f.name.clone(), decl.spec.clone());
            }
            return Ok(());
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
        let mut child = self.child();
        child.params = f.params.iter().map(|p| p.name.clone()).collect();
        let has_variadic = f.params.iter().any(|p| p.variadic);
        // 固定参数（非可变参数）按出现顺序占据 0..fixed；可变参数固定落在末槽，
        // 由 VM 把多余实参收束为数组写入。
        let mut fixed_index: u16 = 0;
        for param in &f.params {
            if param.variadic {
                let slot = f.params.len() as u16 - 1;
                child.scope.declare_at(&param.name, slot);
            } else {
                child.scope.declare_at(&param.name, fixed_index);
                fixed_index += 1;
            }
        }
        // 函数体序言：为每个带默认值的参数在“实参不足”时求值默认表达式并写回槽位。
        // 默认值表达式可引用更早的参数与外层闭包，与树遍历求值器的调用期求值一致。
        let fixed_count = f.params.iter().filter(|p| !p.variadic).count();
        for (idx, param) in f.params.iter().enumerate() {
            if let Some(default) = &param.default {
                child.line(f.pos.line as u32);
                child.builder.emit(Opcode::ArgCount);
                let const_idx = child.builder.add_constant(ConstValue::Int(idx as i64));
                child.builder.emit(Opcode::Const(const_idx));
                // argc <= idx 表示实参未覆盖到该参数位置，需要使用默认值。
                let op = child.add_string_const("小于等于");
                child.builder.emit(Opcode::BinOp(op));
                // 复制一份比较值：缺省路径弹出副本后求值默认表达式；
                // 已传参路径跳过默认值块，在块尾弹出原始比较值。两条路径栈平衡。
                child.builder.emit(Opcode::Dup);
                let skip_default = child.builder.emit(Opcode::JumpIfFalse(0));
                child.builder.emit(Opcode::Pop); // 缺省路径：弹出比较值副本
                child.compile_expr(default)?;
                let slot = if param.variadic {
                    f.params.len() as u16 - 1
                } else {
                    idx as u16
                };
                child.builder.emit(Opcode::StoreLocal(slot));
                child.builder.patch_jump(skip_default);
                child.builder.emit(Opcode::Pop); // 已传参路径：弹出原始比较值
            }
        }
        child.compile_statements(&f.body.statements)?;
        let nil = child.builder.add_constant(ConstValue::Nil);
        child.builder.emit(Opcode::Const(nil));
        child.builder.emit(Opcode::Return);

        let arity = Arity {
            fixed: fixed_count,
            variadic: has_variadic,
            optional: f.params.iter().filter(|p| p.default.is_some()).count(),
        };
        let chunk = FunctionChunk {
            name: name.clone(),
            arity,
            params: child.params.clone(),
            param_types: param_type_names(&f.params),
            return_types: return_type_names(&f.return_types),
            body: child.builder.build(),
            upvalues: child.upvalues,
        };
        let index = self.functions.borrow().len() as u16;
        self.functions.borrow_mut().push(chunk);
        self.builder.emit(Opcode::MakeClosure(index));

        // 具名函数同时绑定到当前作用域名，匿名函数仅压栈。
        if !f.name.is_empty() {
            // 顶层函数已在预扫描阶段于根作用域占槽，这里复用同一槽位，避免重复分配
            // 导致前置引用（函数提升）解析到空槽。
            let slot = self.scope.declare_or_reuse_root(&f.name);
            self.builder.emit(Opcode::StoreLocal(slot));
            // 绑定后再把函数值留在栈上，保持“声明表达式有值”的直觉（非必需）。
            self.builder.emit(Opcode::LoadLocal(slot));
        }
        Ok(())
    }

    /// 编译复合赋值 `甲 += 乙`：读取当前值与右侧表达式做二元运算后写回，
    /// 运算符与 tree eval_compound_assign 对齐（加/减/乘/除）。当前仅支持标识符目标。
    fn compile_compound_assign(
        &mut self,
        c: &fan_ast::stmt::CompoundAssignStmt,
    ) -> CompileResult<()> {
        self.line(c.pos.line as u32);
        let fan_ast::expr::Expr::Ident(id) = &c.target else {
            return Err("字节码编译器暂不支持对非标识符复合赋值".to_string());
        };
        match c.op.as_str() {
            "加" | "减" | "乘" | "除" => {}
            other => return Err(format!("未知复合赋值运算符：{other}")),
        }
        self.emit_load_name(&id.name)?;
        self.compile_expr(&c.value)?;
        let op_idx = self.add_string_const(&c.op);
        self.builder.emit(Opcode::BinOp(op_idx));
        self.emit_store_name(&id.name)
    }

    /// 编译插值字符串：parts 与 exprs 交错，表达式经 ToStr 文本化后用字符串拼接累积。
    fn compile_interp_string(&mut self, interp: &fan_ast::expr::InterpString) -> CompileResult<()> {
        let add_op = self.add_string_const("加");
        let mut emitted = false;
        for (i, part) in interp.parts.iter().enumerate() {
            if !part.is_empty() {
                let c = self.add_string_const(part);
                self.builder.emit(Opcode::Const(c));
                if emitted {
                    self.builder.emit(Opcode::BinOp(add_op));
                }
                emitted = true;
            }
            if let Some(e) = interp.exprs.get(i) {
                self.compile_expr(e)?;
                self.builder.emit(Opcode::ToStr);
                if emitted {
                    self.builder.emit(Opcode::BinOp(add_op));
                }
                emitted = true;
            }
        }
        // 全空（理论上不会出现）时兜底为空字符串，避免栈上无值。
        if !emitted {
            let empty = self.add_string_const("");
            self.builder.emit(Opcode::Const(empty));
        }
        Ok(())
    }

    /// 编译自增自减（`甲++`/`甲--` 及前缀形式），仅支持标识符目标。
    ///
    /// 栈效果：读取当前值（Dup 保留一份作为表达式结果），加/减 1 后写回目标；
    /// 后缀返回旧值，前缀返回新值，与树遍历求值器 eval_update 对齐。
    fn compile_update(&mut self, u: &fan_ast::expr::UpdateExpr) -> CompileResult<()> {
        let fan_ast::expr::Expr::Ident(id) = &*u.target else {
            return Err("字节码编译器暂不支持对非标识符自增自减".to_string());
        };
        let delta = match u.op.as_str() {
            "加" => 1i64,
            "减" => -1,
            other => return Err(format!("未知自增运算符：{other}")),
        };
        // 前缀结果是新值，后缀结果是旧值；两者都需要先取当前值并写回新值。
        self.emit_load_name(&id.name)?; // 栈：当前值
        self.builder.emit(Opcode::Dup); // 栈：当前值、当前值
        let one = self.builder.add_constant(ConstValue::Int(1));
        self.builder.emit(Opcode::Const(one)); // 栈：当前值、当前值、1
        let bin_op = if delta > 0 { "加" } else { "减" };
        let op_idx = self.add_string_const(bin_op);
        self.builder.emit(Opcode::BinOp(op_idx)); // 栈：当前值、新值
        self.emit_store_name(&id.name)?; // 栈：当前值（写回弹出新值）
        if u.prefix {
            // 前缀：结果应为新值。当前栈顶是旧值，改为重新读取写回后的目标。
            self.builder.emit(Opcode::Pop);
            self.emit_load_name(&id.name)?;
        }
        Ok(())
    }

    /// 按名读取：`通过 模块` 覆盖 → 局部槽 → upvalue → 全局/原生名。
    fn emit_load_name(&mut self, name: &str) -> CompileResult<()> {
        // 通过表达式内部：裸标识符优先解析为模块导出成员。
        if let Some(module_name) = self.through_modules.last().cloned() {
            // 模块别名自身是当前作用域可见的局部（import 绑定），先取出模块对象。
            self.emit_load_name_unscoped(&module_name)?;
            let name_idx = self.add_string_const(name);
            self.builder.emit(Opcode::GetMember(name_idx));
            return Ok(());
        }
        self.emit_load_name_unscoped(name)
    }

    /// 不经过 `通过` 模块覆盖的按名读取：局部槽 → upvalue → 全局/原生名。
    fn emit_load_name_unscoped(&mut self, name: &str) -> CompileResult<()> {
        if let Some(slot) = self.scope.resolve(name) {
            self.builder.emit(Opcode::LoadLocal(slot));
        } else if let Some(up) = self.resolve_upvalue(name)? {
            self.builder.emit(Opcode::LoadUpvalue(up));
        } else {
            let idx = self.add_string_const(name);
            self.builder.emit(Opcode::LoadGlobal(idx));
        }
        Ok(())
    }

    fn emit_store_name(&mut self, name: &str) -> CompileResult<()> {
        if let Some(slot) = self.scope.resolve(name) {
            self.builder.emit(Opcode::StoreLocal(slot));
        } else if let Some(up) = self.resolve_upvalue(name)? {
            self.builder.emit(Opcode::StoreUpvalue(up));
        } else {
            let idx = self.add_string_const(name);
            self.builder.emit(Opcode::StoreGlobal(idx));
        }
        Ok(())
    }

    /// 把一个外层名字登记为本函数的 upvalue，返回其在本函数 upvalue 表中的槽位。
    ///
    /// 先查本函数是否已登记同名 upvalue（去重）；否则向父编译器递归解析：
    /// 命中父的局部槽 → `from_local=true`；命中父的 upvalue → `from_local=false`，
    /// 从而支持跨多层嵌套的捕获。模块入口没有父链，未命中返回 None（按全局处理）。
    fn resolve_upvalue(&mut self, name: &str) -> CompileResult<Option<u16>> {
        if let Some(existing) = self.upvalues.iter().position(|u| u.name == name) {
            return Ok(Some(existing as u16));
        }
        let Some(parent_ptr) = self.parent else {
            return Ok(None);
        };
        // 安全：父编译器存活于调用栈，且此处不与自身借用重叠。
        let parent = unsafe { &mut *parent_ptr };
        if let Some(slot) = parent.scope.resolve(name) {
            let index = self.upvalues.len() as u16;
            self.upvalues.push(UpvalueDesc {
                name: name.to_string(),
                from_local: true,
                index: slot,
            });
            Ok(Some(index))
        } else if let Some(parent_up) = parent.resolve_upvalue(name)? {
            let index = self.upvalues.len() as u16;
            self.upvalues.push(UpvalueDesc {
                name: name.to_string(),
                from_local: false,
                index: parent_up,
            });
            Ok(Some(index))
        } else {
            Ok(None)
        }
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
