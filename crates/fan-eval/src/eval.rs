//! 求值器核心。
//!
//! 把 AST 求值成 `Object`。控制流用 `Flow` 显式表达而非塞进错误通道，
//! 这是与 Go 版的**刻意差异**：Rust 的 `match` 穷尽性检查会让遗漏的
//! 控制流分支在编译期暴露，比运行期类型断言更安全。

use crate::error::{value, EvalResult, FanError, Flow};
use crate::operators;
use fan_ast::{self as ast, BlockStmt, DeclType, Expr, Position, Program, Stmt, Visibility};
use fan_object::{
    check_decl_type, kind_display_name, runtime_type_name, Builtin, Environment, FanArray, FanDict,
    Object, Runtime,
};
use std::cell::RefCell;
use std::rc::Rc;

/// 解释器：持有一个根环境与执行状态。
pub struct Interpreter {
    /// 当前作用域。函数调用时会被替换为更内层的作用域。
    pub env: Rc<RefCell<Environment>>,
    /// 模块加载器。模块内再次 `导入` 时共享同一份缓存。
    pub loader: Rc<RefCell<crate::module::Loader>>,
    /// 标签处理器注册表。
    pub tags: crate::tag::TagRegistry,
    /// 扩展动态库登记表：仅在 `定义 本地("模块.函数")` 引用了该模块的原生函数时才加载。
    ///
    /// 与主程序、各子模块共享同一份：动态库一旦加载就无法卸载，登记表是
    /// 进程级状态，若各自持有会导致同一库被重复登记。
    pub ext: Rc<RefCell<fan_native::ExtRegistry>>,
    /// 循环嵌套深度，用于校验 `跳出`/`继续` 的合法性。
    loop_depth: usize,
    /// 函数嵌套深度，用于校验 `返回` 的合法性。
    fn_depth: usize,
    /// 每次函数调用的 `稍后` 队列：返回前按**后进先出**执行。
    ///
    /// 用栈是因为调用可嵌套——内层函数返回时只能执行它自己登记的稍后项。
    defers: Vec<Vec<Deferred>>,
    /// 稍后块正在处理的抛出值，块内的 `捕获()` 读取它。
    pending_throw: Option<Object>,
    /// 本次抛出是否已被某个 `捕获()` 取走；取走后不再向外传播。
    throw_caught: bool,
    /// 当前正在执行的方法所属模型名（调用栈），普通函数调用记为 `None`。
    ///
    /// 用于判定 `私有` 成员的访问是否合法：私有字段与方法只允许在本模型
    /// 的方法体内访问。用栈是因为调用可嵌套，内层返回后要还原外层；普通
    /// 函数记为 `None` 是为了遮蔽外层——方法里调用普通函数后，该函数体内
    /// 不应还能访问本模型的私有成员。
    self_class: Vec<Option<String>>,
}

/// 一条 `稍后` 登记。
///
/// 连同登记时的作用域一起保存：稍后执行发生在函数返回前，此时的当前
/// 作用域已切回调用方，若不留存就看不见函数内的局部变量。
struct Deferred {
    /// `函数调用` 或 `函数` 字面量。
    expr: ast::Expr,
    /// 登记时就求好值的实参：`稍后 f(x)` 的 x 取**登记时刻**的值，
    /// 而不是执行时刻。被调用者仍在执行时求值，以免把 `a.f()` 里的
    /// 无参方法提前调用掉。
    eager_args: Option<Vec<Object>>,
    env: Rc<RefCell<Environment>>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Interpreter::new()
    }
}

impl Interpreter {
    /// 创建解释器，并预置最基础的原生函数（`打印`/`打印行`）。
    pub fn new() -> Self {
        let env = Rc::new(RefCell::new(Environment::new()));
        let loader = Rc::new(RefCell::new(crate::module::Loader::new(Vec::new())));
        let ext = Rc::new(RefCell::new(fan_native::ExtRegistry::new()));
        let interp = Interpreter::with_parts(env, loader, ext);
        interp.register_core_natives();
        interp
    }

    /// 创建把输出写入指定缓冲区的解释器（供测试捕获输出）。
    pub fn with_output(sink: fan_object::Output) -> Self {
        let rt = Rc::new(RefCell::new(Runtime::with_output(sink)));
        let env = Rc::new(RefCell::new(Environment::for_runtime(rt)));
        let loader = Rc::new(RefCell::new(crate::module::Loader::new(Vec::new())));
        let ext = Rc::new(RefCell::new(fan_native::ExtRegistry::new()));
        let interp = Interpreter::with_parts(env, loader, ext);
        interp.register_core_natives();
        interp
    }

    /// 在已有环境上创建一个辅助解释器，共享模块加载器。
    ///
    /// 用于求值子表达式（如切片上下界、参数默认值）：这些表达式不应受
    /// 当前循环/函数深度约束，故深度计数归零。
    fn with_env(
        &self,
        env: Rc<RefCell<Environment>>,
        loader: Rc<RefCell<crate::module::Loader>>,
    ) -> Self {
        Interpreter::with_parts(env, loader, self.ext.clone())
    }

    /// 全部构造路径的唯一出口：集中装配标签注册表与扩展登记表。
    fn with_parts(
        env: Rc<RefCell<Environment>>,
        loader: Rc<RefCell<crate::module::Loader>>,
        ext: Rc<RefCell<fan_native::ExtRegistry>>,
    ) -> Self {
        Interpreter {
            env,
            loader,
            tags: crate::tag::TagRegistry::with_builtins(),
            ext,
            loop_depth: 0,
            fn_depth: 0,
            defers: Vec::new(),
            pending_throw: None,
            throw_caught: false,
            self_class: Vec::new(),
        }
    }

    /// 为执行脚本文件创建解释器。
    ///
    /// 命令行 `运行` 子命令使用：以指定运行实例求值脚本，`base_dir` 决定
    /// `导入` 的相对解析起点，`source_file` 供错误定位与 `进程.脚本()` 使用。
    pub fn for_script(
        rt: Rc<RefCell<Runtime>>,
        modules_dirs: Vec<String>,
        base_dir: &str,
        source_file: &str,
    ) -> Self {
        let env = Rc::new(RefCell::new(Environment::for_runtime(rt)));
        {
            let mut e = env.borrow_mut();
            e.set_base_dir(base_dir);
            e.source_file = source_file.to_string();
        }
        let loader = Rc::new(RefCell::new(crate::module::Loader::new(modules_dirs)));
        let ext = Rc::new(RefCell::new(fan_native::ExtRegistry::new()));
        let interp = Interpreter::with_parts(env, loader, ext);
        interp.register_core_natives();
        interp
    }

    /// 为指定运行实例创建解释器，并复用已有的模块加载器。
    ///
    /// 模块加载时使用：子模块与主程序共享同一个运行实例（原生函数表、
    /// 类型注册表等），从而能在模块里直接调用 `打印行` 等内建函数。
    pub fn for_module(
        rt: Rc<RefCell<Runtime>>,
        loader: Rc<RefCell<crate::module::Loader>>,
        base_dir: &str,
        source_file: &str,
        module_name: &str,
        ext: Rc<RefCell<fan_native::ExtRegistry>>,
    ) -> Self {
        let env = Rc::new(RefCell::new(Environment::for_runtime(rt)));
        {
            let mut e = env.borrow_mut();
            e.set_base_dir(base_dir);
            e.source_file = source_file.to_string();
            e.module_name = module_name.to_string();
            e.module_export = true;
        }
        let interp = Interpreter::with_parts(env, loader, ext);
        interp.register_core_natives();
        interp
    }

    /// 注册 `core` 组内建（纯标准库实现），并补上 `打印`/`打印行`。
    ///
    /// `打印`/`打印行` 是语言内置输出能力，无需导入模块即可调用；标准库
    /// `系统.凡` 的 `打印` 则通过 `定义 本地("print") 函数 ...` 绑定到同一份实现。
    fn register_core_natives(&self) {
        let rt = self.env.borrow().runtime();
        let mut rt = rt.borrow_mut();
        fan_native::register_core(&mut rt);
        // 复用 core 的 print，避免同一能力出现两份实现
        let print = rt.lookup_native("print").expect("core 组应提供 print");
        rt.register_native(Rc::new(Builtin::new("打印", None, print.func.clone())));
        rt.register_native(Rc::new(Builtin::new(
            "打印行",
            None,
            Rc::new(|rt, args| {
                rt.print_line(&args);
                Ok(vec![])
            }),
        )));
        // 常用内建的中文名：不必导入 系统 模块就能直接使用。
        // 这些名字在标准库里也有同名导出（系统.追加 等），二者互不冲突。
        for (chinese, english) in [("长度", "len"), ("追加", "append"), ("错误", "error")] {
            let src = rt
                .lookup_native(english)
                .unwrap_or_else(|| panic!("core 组应提供 {english}"));
            rt.register_native(Rc::new(Builtin::new(chinese, None, src.func.clone())));
        }
    }

    /// 执行整个程序，返回最后一条语句的值。
    pub fn run(&mut self, program: &Program) -> Result<Object, FanError> {
        let mut last = Object::Nil;
        for stmt in &program.statements {
            match self.eval_stmt(stmt)? {
                Flow::Value(v) => last = v,
                Flow::Return(values) => {
                    return Ok(single_or_tuple(values));
                }
                Flow::Break => return Err(FanError::new("跳出 不能出现在循环外")),
                Flow::Continue => return Err(FanError::new("继续 不能出现在循环外")),
                Flow::Throw(v) => {
                    return Err(FanError::new(format!("未捕获的抛出：{}", v.inspect())))
                }
                Flow::Check(v, p) => {
                    return Err(FanError::at(
                        format!("未捕获的检查错误：{}", v.inspect()),
                        p,
                    ))
                }
            }
        }
        Ok(last)
    }

    // ------------------------------------------------------------ 语句

    pub fn eval_stmt(&mut self, stmt: &Stmt) -> EvalResult {
        match stmt {
            Stmt::VarDecl(d) => self.eval_var_decl(d),
            Stmt::Assign(a) => self.eval_assign(a),
            Stmt::CompoundAssign(a) => self.eval_compound_assign(a),
            Stmt::MultiDecl(d) => self.eval_multi_decl(d),
            Stmt::MultiAssign(a) => self.eval_multi_assign(a),
            Stmt::Expr(e) => self.eval_expr(&e.expr),
            Stmt::Block(b) => self.eval_block(b),
            Stmt::If(i) => self.eval_if(i),
            Stmt::Switch(s) => self.eval_switch(s),
            Stmt::While(w) => self.eval_while(w),
            Stmt::Repeat(r) => self.eval_repeat(r),
            Stmt::Times(t) => self.eval_times(t),
            Stmt::ForEach(f) => self.eval_for_each(f),
            Stmt::Break(_) => {
                if self.loop_depth == 0 {
                    return Err(FanError::new("跳出 不能出现在循环外"));
                }
                Ok(Flow::Break)
            }
            Stmt::Continue(_) => {
                if self.loop_depth == 0 {
                    return Err(FanError::new("继续 不能出现在循环外"));
                }
                Ok(Flow::Continue)
            }
            Stmt::Return(r) => self.eval_return(r),
            Stmt::IndexAssign(a) => self.eval_index_assign(a),
            Stmt::FieldAssign(a) => self.eval_field_assign(a),
            Stmt::Throw(t) => {
                let v = self.eval_expr(&t.value)?.into_value()?;
                Ok(Flow::Throw(v))
            }
            Stmt::TypeDef(t) => self.eval_type_def(t),
            Stmt::Import(i) => self.eval_import(i),
            Stmt::Class(c) => self.eval_class_stmt(c),
            Stmt::Interface(i) => self.eval_interface_stmt(i),
            Stmt::MethodDef(m) => self.eval_method_def(m),
            Stmt::Visibility(v) => self.eval_visibility(v),
            // `稍后` 语句在后续里程碑实现；此处给出明确提示而非静默通过。
            Stmt::Defer(n) => {
                // 只登记、不执行：真正的执行发生在函数返回前。
                // 实参此刻就求值并保存，执行时原样使用。
                let eager_args = match &n.call {
                    ast::Expr::Call(call) => {
                        let mut vals = Vec::with_capacity(call.args.len());
                        for a in &call.args {
                            match a {
                                ast::Expr::Spread(s) => vals.extend(self.spread_elements(s)?),
                                _ => vals.push(self.eval_expr(a)?.into_value()?),
                            }
                        }
                        Some(vals)
                    }
                    _ => None,
                };
                match self.defers.last_mut() {
                    Some(frame) => frame.push(Deferred {
                        expr: n.call.clone(),
                        eager_args,
                        env: self.env.clone(),
                    }),
                    None => {
                        return Err(FanError::at("稍后 只能在函数内使用", n.pos));
                    }
                }
                value(Object::Nil)
            }
        }
    }

    fn eval_block(&mut self, block: &BlockStmt) -> EvalResult {
        let outer = self.env.clone();
        self.env = Rc::new(RefCell::new(Environment::enclosed(outer.clone())));
        let result = self.eval_block_no_scope(block);
        self.env = outer;
        result
    }

    /// 在当前作用域内顺序执行块内语句（不新建作用域）。
    fn eval_block_no_scope(&mut self, block: &BlockStmt) -> EvalResult {
        let mut last = Object::Nil;
        for stmt in &block.statements {
            match self.eval_stmt(stmt)? {
                Flow::Value(v) => last = v,
                signal => return Ok(signal),
            }
        }
        value(last)
    }

    fn eval_if(&mut self, stmt: &ast::IfStmt) -> EvalResult {
        for branch in &stmt.branches {
            let cond = self.eval_expr(&branch.condition)?.into_value()?;
            let Object::Bool(v) = cond.unwrap_named() else {
                return Err(FanError::at(
                    format!("如果 的条件必须是布尔，实际是 {}", cond.kind()),
                    branch.condition.pos(),
                ));
            };
            if v {
                return self.eval_block(&branch.body);
            }
        }
        match &stmt.else_ {
            Some(body) => self.eval_block(body),
            None => value(Object::Nil),
        }
    }

    fn eval_switch(&mut self, stmt: &ast::SwitchStmt) -> EvalResult {
        let subject = match &stmt.subject {
            Some(e) => Some(self.eval_expr(e)?.into_value()?),
            None => None,
        };

        // 条件守卫分支按顺序求值；`其他` 默认分支留到最后。
        let mut default_branch: Option<&ast::SwitchBranch> = None;
        for branch in &stmt.branches {
            if branch.default {
                default_branch = Some(branch);
                continue;
            }
            let matched = if branch.guard {
                // 守卫条件必须是布尔，与 `当` 循环、`如果` 保持一致
                let v = self.eval_expr(&branch.values[0])?.into_value()?;
                let Object::Bool(v) = v.unwrap_named() else {
                    return Err(FanError::at(
                        format!("当 的条件必须是布尔，实际是 {}", v.kind()),
                        branch.pos,
                    ));
                };
                v
            } else {
                let Some(target) = &subject else {
                    return Err(FanError::at("判断 分支缺少匹配对象", branch.pos));
                };
                let mut hit = false;
                for candidate in &branch.values {
                    let val = self.eval_expr(candidate)?.into_value()?;
                    if operators::eq(branch.pos, target, &val)? == Object::Bool(true) {
                        hit = true;
                        break;
                    }
                }
                hit
            };
            if matched {
                return self.eval_block(&branch.body);
            }
        }
        match default_branch {
            Some(branch) => self.eval_block(&branch.body),
            None => value(Object::Nil),
        }
    }

    fn eval_while(&mut self, stmt: &ast::WhileStmt) -> EvalResult {
        self.loop_depth += 1;
        let result = self.loop_guard(|s| {
            loop {
                let cond = s.eval_expr(&stmt.condition)?.into_value()?;
                let Object::Bool(v) = cond.unwrap_named() else {
                    return Err(FanError::at(
                        format!("当 的条件必须是布尔，实际是 {}", cond.kind()),
                        stmt.condition.pos(),
                    ));
                };
                if !v {
                    break;
                }
                match s.eval_block(&stmt.body)? {
                    Flow::Break => break,
                    Flow::Continue => continue,
                    other => {
                        if other.is_signal() {
                            return Ok(other);
                        }
                    }
                }
            }
            value(Object::Nil)
        });
        self.loop_depth -= 1;
        result
    }

    fn eval_repeat(&mut self, stmt: &ast::RepeatStmt) -> EvalResult {
        self.loop_depth += 1;
        let result = self.loop_guard(|s| {
            loop {
                match s.eval_block(&stmt.body)? {
                    Flow::Break => break,
                    Flow::Continue => {}
                    other => {
                        if other.is_signal() {
                            return Ok(other);
                        }
                    }
                }
                let cond = s.eval_expr(&stmt.until)?.into_value()?;
                let Object::Bool(v) = cond.unwrap_named() else {
                    return Err(FanError::at(
                        format!("直到 的条件必须是布尔，实际是 {}", cond.kind()),
                        stmt.until.pos(),
                    ));
                };
                if v {
                    break;
                }
            }
            value(Object::Nil)
        });
        self.loop_depth -= 1;
        result
    }

    fn eval_times(&mut self, stmt: &ast::TimesStmt) -> EvalResult {
        let count = self.eval_expr(&stmt.count)?.into_value()?;
        let n = match count.unwrap_named() {
            Object::Int(v) => v,
            other => {
                return Err(FanError::at(
                    format!("重复 次数必须是整数，实际是 {}", other.kind()),
                    stmt.count.pos(),
                ))
            }
        };
        self.loop_depth += 1;
        let result = self.loop_guard(|s| {
            for _ in 0..n.max(0) {
                match s.eval_block(&stmt.body)? {
                    Flow::Break => break,
                    Flow::Continue => continue,
                    other => {
                        if other.is_signal() {
                            return Ok(other);
                        }
                    }
                }
            }
            value(Object::Nil)
        });
        self.loop_depth -= 1;
        result
    }

    fn eval_for_each(&mut self, stmt: &ast::ForEachStmt) -> EvalResult {
        let iterable = self.eval_expr(&stmt.iterable)?.into_value()?;
        self.loop_depth += 1;
        let result = self.loop_guard(|s| match iterable.unwrap_named() {
            Object::Array(arr) => {
                let elements = arr.elements();
                for (idx, item) in elements.into_iter().enumerate() {
                    if let Err(flow) = s.bind_iteration(
                        &stmt.first_name,
                        item,
                        &stmt.second_name,
                        Object::Int(idx as i64),
                    ) {
                        return flow;
                    }
                    match s.eval_block(&stmt.body)? {
                        Flow::Break => break,
                        Flow::Continue => continue,
                        other => {
                            if other.is_signal() {
                                return Ok(other);
                            }
                        }
                    }
                }
                value(Object::Nil)
            }
            Object::Str(text) => {
                // 遍历字符串产出的是**单字符字符串**而非 `字符`：凡语言的
                // `字符` 是独立类型，若在此产出它，标准库里 `取字符(...)` 之类
                // 把它当字符串用的代码会在比较时报「不同类型不可比较」。
                let chars: Vec<String> = text.chars().map(|c| c.to_string()).collect();
                for (idx, ch) in chars.into_iter().enumerate() {
                    if let Err(flow) = s.bind_iteration(
                        &stmt.first_name,
                        Object::string(ch),
                        &stmt.second_name,
                        Object::Int(idx as i64),
                    ) {
                        return flow;
                    }
                    match s.eval_block(&stmt.body)? {
                        Flow::Break => break,
                        Flow::Continue => continue,
                        other => {
                            if other.is_signal() {
                                return Ok(other);
                            }
                        }
                    }
                }
                value(Object::Nil)
            }
            Object::Dict(dict) => {
                let keys = dict.keys();
                for key in keys {
                    let val = dict.get(&key).unwrap_or(Object::Nil);
                    // 只给一个名字时绑定值，给两个名字时绑定 (键, 值)
                    if let Err(flow) = s.bind_iteration(
                        &stmt.first_name,
                        if stmt.second_name.is_none() {
                            val.clone()
                        } else {
                            key.clone()
                        },
                        &stmt.second_name,
                        val,
                    ) {
                        return flow;
                    }
                    match s.eval_block(&stmt.body)? {
                        Flow::Break => break,
                        Flow::Continue => continue,
                        other => {
                            if other.is_signal() {
                                return Ok(other);
                            }
                        }
                    }
                }
                value(Object::Nil)
            }
            other => Err(FanError::at(
                format!("遍历 不支持 {}，仅支持 数组/字符串/字典", other.kind()),
                stmt.iterable.pos(),
            )),
        });
        self.loop_depth -= 1;
        result
    }

    /// 在循环体内绑定迭代变量（在当前作用域内直接写入）。
    fn bind_iteration(
        &mut self,
        first: &str,
        first_value: Object,
        second: &Option<String>,
        second_value: Object,
    ) -> Result<(), EvalResult> {
        let env = self.env.clone();
        let mut e = env.borrow_mut();
        if e.has_local(first) {
            let _ = e.assign(first, first_value);
        } else {
            e.set_local(first.to_string(), first_value, DeclType::any());
        }
        if let Some(name) = second {
            if e.has_local(name) {
                let _ = e.assign(name, second_value);
            } else {
                e.set_local(name.clone(), second_value, DeclType::any());
            }
        }
        Ok(())
    }

    /// 统一处理循环体返回的控制流：`跳出` 终止，`继续` 继续下一轮。
    fn loop_guard<F>(&mut self, f: F) -> EvalResult
    where
        F: FnOnce(&mut Interpreter) -> EvalResult,
    {
        f(self)
    }

    fn eval_return(&mut self, stmt: &ast::ReturnStmt) -> EvalResult {
        if self.fn_depth == 0 {
            return Err(FanError::new("返回 不能出现在函数外"));
        }
        let mut values = Vec::new();
        for e in &stmt.values {
            // `返回 检查 调用` 命中错误时，检查信号要原样冒泡到函数边界，
            // 不能经 into_value 转成运行时错误。
            match self.eval_expr(e)? {
                Flow::Value(v) => values.push(v),
                signal => return Ok(signal),
            }
        }
        if values.is_empty() {
            values.push(Object::Nil);
        }
        Ok(Flow::Return(values))
    }

    // ------------------------------------------------------------ 声明与赋值

    fn eval_var_decl(&mut self, decl: &ast::VarDecl) -> EvalResult {
        // 无初值声明：统一初始化为 空。
        if decl.value.is_none() {
            let env = self.env.clone();
            env.borrow_mut()
                .declare_zero(&decl.name, decl.decl_type.clone(), decl.visibility)
                .map_err(|e| FanError::at(e, decl.pos))?;
            return value(Object::Nil);
        }
        let val = match self.eval_expr(decl.value.as_ref().unwrap())? {
            Flow::Value(v) => v,
            // `变量 x = 检查 调用` 命中错误时，检查信号向函数边界冒泡
            signal => return Ok(signal),
        };
        let env = self.env.clone();
        // 分段函数：除按全名绑定外，额外登记到 mixfix 注册表，支持按参数类型重载。
        if let Object::Func(f) = &val {
            if f.is_segmented() {
                let root = f.name_segments[0].clone();
                let signature = f.signature.clone();
                let rt = env.borrow().runtime();
                rt.borrow_mut()
                    .register_mixfix(&root, &signature, f.clone())
                    .map_err(|reason| FanError::at(reason, decl.pos))?;
            }
        }
        let mut e = env.borrow_mut();

        if decl.is_explicit {
            e.declare(
                &decl.name,
                val.clone(),
                decl.is_const,
                decl.decl_type.clone(),
                decl.visibility,
            )
            .map_err(|err| FanError::at(err, decl.pos))?;
            return value(val);
        }
        // 隐式声明（`甲 = 1`）：已存在则赋值，否则新建。
        // 硬规则：非分段普通函数同名重复定义直接报错，禁止后定义覆盖。
        // 分段函数按参数类型签名重载，同名不同签名合法，其同签名重复已由
        // register_mixfix 拦截；普通变量的重新赋值也不受影响。
        if let Object::Func(f) = &val {
            if !f.is_segmented() {
                if let Some(existing) = e.get(&decl.name) {
                    if matches!(existing, Object::Func(_)) {
                        return Err(FanError::at(
                            format!("函数 {} 重复定义", decl.name),
                            decl.pos,
                        ));
                    }
                }
            }
        }
        if e.has_local(&decl.name) || e.get(&decl.name).is_some() {
            e.assign(&decl.name, val.clone())
                .map_err(|err| FanError::at(err, decl.pos))?;
            return value(val);
        }
        e.declare(
            &decl.name,
            val.clone(),
            false,
            DeclType::any(),
            decl.visibility,
        )
        .map_err(|err| FanError::at(err, decl.pos))?;
        value(val)
    }

    fn eval_assign(&mut self, stmt: &ast::AssignStmt) -> EvalResult {
        let val = match self.eval_expr(&stmt.value)? {
            Flow::Value(v) => v,
            signal => return Ok(signal),
        };
        let env = self.env.clone();
        env.borrow_mut()
            .assign(&stmt.name, val.clone())
            .map_err(|e| FanError::at(e, stmt.pos))?;
        value(val)
    }

    fn eval_compound_assign(&mut self, stmt: &ast::CompoundAssignStmt) -> EvalResult {
        let current = self.eval_expr(&stmt.target)?.into_value()?;
        let right = self.eval_expr(&stmt.value)?.into_value()?;
        let pos = stmt.pos;
        let computed = match stmt.op.as_str() {
            "加" => operators::add(pos, &current, &right)?,
            "减" => operators::sub(pos, &current, &right)?,
            "乘" => operators::mul(pos, &current, &right)?,
            "除" => operators::div(pos, &current, &right)?,
            other => return Err(FanError::at(format!("未知复合赋值运算符：{}", other), pos)),
        };
        self.assign_to(&stmt.target, computed)
    }

    fn eval_multi_decl(&mut self, decl: &ast::MultiDecl) -> EvalResult {
        // 无初值声明（`变量 m, n`）：逐个初始化为 空。
        // 解析器用空元组表示「没有初值」，与 `= ()` 同形，故一并处理。
        if matches!(&decl.value, Expr::Tuple(t) if t.elements.is_empty()) {
            let env = self.env.clone();
            for name in &decl.names {
                env.borrow_mut()
                    .declare(
                        name,
                        Object::Nil,
                        decl.is_const,
                        DeclType::any(),
                        decl.visibility,
                    )
                    .map_err(|err| FanError::at(err, decl.pos))?;
            }
            return value(Object::Nil);
        }
        let val = match self.eval_expr(&decl.value)? {
            Flow::Value(v) => v,
            signal => return Ok(signal),
        };
        let values = require_tuple(decl.pos, val, decl.names.len())?;
        let env = self.env.clone();
        let mut e = env.borrow_mut();
        for (name, v) in decl.names.iter().zip(values) {
            e.declare(name, v, decl.is_const, DeclType::any(), decl.visibility)
                .map_err(|err| FanError::at(err, decl.pos))?;
        }
        value(Object::Nil)
    }

    fn eval_multi_assign(&mut self, stmt: &ast::MultiAssign) -> EvalResult {
        let val = match self.eval_expr(&stmt.value)? {
            Flow::Value(v) => v,
            signal => return Ok(signal),
        };
        let values = require_tuple(stmt.pos, val, stmt.names.len())?;
        let env = self.env.clone();
        let mut e = env.borrow_mut();
        for (name, v) in stmt.names.iter().zip(values) {
            e.assign(name, v)
                .map_err(|err| FanError::at(err, stmt.pos))?;
        }
        value(Object::Nil)
    }

    /// 把值写回可赋值的目标（标识符、下标、字段）。
    fn assign_to(&mut self, target: &Expr, val: Object) -> EvalResult {
        match target {
            Expr::Ident(id) => {
                let env = self.env.clone();
                env.borrow_mut()
                    .assign(&id.name, val.clone())
                    .map_err(|e| FanError::at(e, id.pos))?;
                value(val)
            }
            Expr::Index(idx) => {
                let container = self.eval_expr(&idx.target)?.into_value()?;
                let index = self.eval_expr(&idx.index)?.into_value()?;
                self.set_index(&container, &index, val.clone(), idx.pos)?;
                value(val)
            }
            Expr::Member(m) => {
                let object = self.eval_expr(&m.object)?.into_value()?;
                self.set_member(&object, &m.name, val.clone(), m.pos)?;
                value(val)
            }
            other => Err(FanError::at(
                format!("不可赋值的目标：{}", other),
                target.pos(),
            )),
        }
    }

    fn eval_index_assign(&mut self, stmt: &ast::IndexAssignStmt) -> EvalResult {
        let container = self.eval_expr(&stmt.target)?.into_value()?;
        let index = self.eval_expr(&stmt.index)?.into_value()?;
        let val = self.eval_expr(&stmt.value)?.into_value()?;
        self.set_index(&container, &index, val.clone(), stmt.pos)?;
        value(val)
    }

    fn set_index(
        &self,
        container: &Object,
        index: &Object,
        val: Object,
        pos: Position,
    ) -> Result<(), FanError> {
        match container.unwrap_named() {
            Object::Array(arr) => {
                let Some(i) = index.as_int() else {
                    return Err(FanError::at(
                        format!("数组下标必须是整数，实际是 {}", index.kind()),
                        pos,
                    ));
                };
                let len = arr.len();
                if i < 0 || i as usize >= len {
                    return Err(FanError::at(
                        format!("数组下标越界：长度 {}，下标 {}", len, i),
                        pos,
                    ));
                }
                arr.set(i as usize, val);
                Ok(())
            }
            Object::Dict(mut dict) => {
                if !index.is_dict_key() {
                    return Err(FanError::at(
                        format!("不支持的字典键类型：{}", index.kind()),
                        pos,
                    ));
                }
                dict.set(index.clone(), val);
                Ok(())
            }
            other => Err(FanError::at(
                format!("{} 不支持下标赋值", other.kind()),
                pos,
            )),
        }
    }

    fn eval_field_assign(&mut self, stmt: &ast::FieldAssignExpr) -> EvalResult {
        let object = self.eval_expr(&stmt.object)?.into_value()?;
        let val = self.eval_expr(&stmt.value)?.into_value()?;
        self.set_member(&object, &stmt.name, val.clone(), stmt.pos)?;
        value(val)
    }

    fn eval_type_def(&mut self, stmt: &ast::TypeDefStmt) -> EvalResult {
        let nt = fan_object::NamedType::new(stmt.name.clone(), stmt.base_type.clone());
        let env = self.env.clone();
        let rt = env.borrow().runtime();
        let mut rt = rt.borrow_mut();
        rt.register_named_type(nt);
        // 类型名本身可当转换函数用：`常量 状态 无效 = 状态(0)`
        let type_name = stmt.name.clone();
        rt.register_native(Rc::new(Builtin::new(
            type_name.clone(),
            Some(1),
            Rc::new(move |rt, args| {
                let Some(nt) = rt.lookup_named_type(&type_name).cloned() else {
                    return Err(format!("未定义的类型 {}", type_name));
                };
                let v = args.into_iter().next().unwrap_or(Object::Nil);
                crate::oop::convert_to_named(&nt, &v).map(|o| vec![o])
            }),
        )));
        value(Object::Nil)
    }

    // ------------------------------------------------------------ 表达式

    pub fn eval_expr(&mut self, expr: &Expr) -> EvalResult {
        match expr {
            Expr::Integer(n) => value(Object::Int(n.value)),
            Expr::Float(n) => value(Object::Float(n.value)),
            Expr::Str(n) => value(Object::string(n.value.clone())),
            Expr::Char(n) => value(Object::Char(n.value)),
            Expr::Bool(n) => value(Object::Bool(n.value)),
            Expr::Nil(_) => value(Object::Nil),
            Expr::Ident(n) => self.eval_ident(&n.name, n.pos),
            Expr::Unary(n) => {
                let right = self.eval_expr(&n.right)?.into_value()?;
                value(operators::unary(n.pos, &n.op, &right)?)
            }
            Expr::Binary(n) => self.eval_binary(n),
            Expr::Ternary(n) => {
                let cond = self.eval_expr(&n.cond)?.into_value()?;
                if cond.is_truthy() {
                    self.eval_expr(&n.then)
                } else {
                    self.eval_expr(&n.else_)
                }
            }
            Expr::Array(n) => {
                let mut elements = Vec::with_capacity(n.elements.len());
                for e in &n.elements {
                    // `...值` 就地摊开成若干元素
                    match e {
                        Expr::Spread(s) => elements.extend(self.spread_elements(s)?),
                        _ => elements.push(self.eval_expr(e)?.into_value()?),
                    }
                }
                value(Object::Array(FanArray::new(elements)))
            }
            Expr::Dict(n) => {
                let mut dict = FanDict::new();
                for pair in &n.pairs {
                    if pair.spread {
                        let src = self.eval_expr(&pair.value)?.into_value()?;
                        if let Object::Dict(d) = src.unwrap_named() {
                            for k in d.keys() {
                                if let Some(v) = d.get(&k) {
                                    dict.set(k, v);
                                }
                            }
                        }
                    } else {
                        let key = self.eval_expr(pair.key.as_ref().unwrap())?.into_value()?;
                        let v = self.eval_expr(&pair.value)?.into_value()?;
                        if !key.is_dict_key() {
                            return Err(FanError::at(
                                format!("不支持的字典键类型：{}", key.kind()),
                                pair.value.pos(),
                            ));
                        }
                        dict.set(key, v);
                    }
                }
                value(Object::Dict(dict))
            }
            Expr::Tuple(n) => {
                let mut values = Vec::with_capacity(n.elements.len());
                for e in &n.elements {
                    values.push(self.eval_expr(e)?.into_value()?);
                }
                value(Object::tuple(values))
            }
            Expr::Index(n) => {
                let target = self.eval_expr(&n.target)?.into_value()?;
                let index = self.eval_expr(&n.index)?.into_value()?;
                value(self.get_index(&target, &index, n.pos)?)
            }
            Expr::Slice(n) => {
                let target = self.eval_expr(&n.target)?.into_value()?;
                value(self.get_slice(&target, n)?)
            }
            Expr::InterpStr(n) => self.eval_interp_string(n),
            Expr::Call(n) => self.eval_call(n),
            Expr::Function(n) => self.eval_function_literal(n),
            Expr::Member(n) => {
                let object = self.eval_expr(&n.object)?.into_value()?;
                value(self.get_member(&object, &n.name, n.pos)?)
            }
            Expr::Update(n) => self.eval_update(n),
            Expr::MaybeCall(n) => {
                let callee = self.eval_expr(&n.callee)?.into_value()?;
                // 无参函数/方法在语句位置自动调用
                if n.auto_call && matches!(callee.kind(), fan_object::Kind::Func) {
                    return self.apply_function(&callee, Vec::new(), n.pos);
                }
                value(callee)
            }
            Expr::TypeTest(n) => self.eval_type_test(n),
            Expr::TypeCast(n) => self.eval_type_cast(n),
            Expr::Composite(n) => {
                let object = self.eval_expr(&n.model)?.into_value()?;
                let mut fields = fan_object::FanDict::new();
                for f in &n.fields {
                    let v = match &f.value {
                        Some(e) => self.eval_expr(e)?.into_value()?,
                        None => Object::Nil,
                    };
                    fields.set(Object::string(f.name.clone()), v);
                }
                match object.unwrap_named() {
                    Object::Class(c) => {
                        // 复合字面量按字段名填充，不调用 初始化
                        let inst = crate::oop::new_zero_instance(&c);
                        for key in fields.keys() {
                            let Some(name) = key.as_str() else { continue };
                            let Some(v) = fields.get(&key) else { continue };
                            inst.set_field_value(&name, v)
                                .map_err(|e| FanError::at(e, n.pos))?;
                        }
                        value(Object::Instance(inst))
                    }
                    _ => {
                        drop(fields);
                        Err(FanError::at("复合字面量暂仅支持模型", n.pos))
                    }
                }
            }
            Expr::Through(t) => self.eval_through(t),
            Expr::Check(n) => self.eval_check(n),
            Expr::ImplicitReceiverCall(n) => self.eval_implicit_receiver_call(n),
            _ => Err(FanError::new(format!("暂未支持的表达式：{}", expr))),
        }
    }

    /// 名字是否已在作用域或内建表中绑定。
    fn is_bound(&self, name: &str) -> bool {
        let env = self.env.borrow();
        if env.get(name).is_some() {
            return true;
        }
        env.runtime().borrow().lookup_native(name).is_some()
    }

    /// 当前是否正身处指定模型的方法内——`私有` 成员的访问判定依据。
    pub(crate) fn inside_class(&self, class_name: &str) -> bool {
        self.self_class.last().and_then(|c| c.as_deref()) == Some(class_name)
    }

    fn eval_ident(&mut self, name: &str, pos: Position) -> EvalResult {
        let env = self.env.clone();
        if let Some(v) = env.borrow().get(name) {
            return value(v);
        }
        // 未声明时回退到原生函数表，使 `打印` 等内建可直接按名字使用。
        let native = env.borrow().runtime().borrow().lookup_native(name);
        if let Some(b) = native {
            return value(Object::Builtin(b));
        }
        Err(FanError::at(format!("未定义的标识符：{}", name), pos))
    }

    fn eval_binary(&mut self, n: &ast::BinaryExpr) -> EvalResult {
        // 且/或 走短路求值
        if n.op == "且" || n.op == "&&" {
            let left = self.eval_expr(&n.left)?.into_value()?;
            return value(operators::logical_and(
                n.pos,
                &left,
                &self.eval_expr(&n.right)?.into_value()?,
            )?);
        }
        if n.op == "或" || n.op == "||" {
            let left = self.eval_expr(&n.left)?.into_value()?;
            return value(operators::logical_or(
                n.pos,
                &left,
                &self.eval_expr(&n.right)?.into_value()?,
            )?);
        }
        let left = self.eval_expr(&n.left)?.into_value()?;
        let right = self.eval_expr(&n.right)?.into_value()?;
        let out = match n.op.as_str() {
            "加" | "+" => operators::add(n.pos, &left, &right)?,
            "减" | "-" => operators::sub(n.pos, &left, &right)?,
            "乘" | "*" => operators::mul(n.pos, &left, &right)?,
            "除" | "/" => operators::div(n.pos, &left, &right)?,
            "取余" | "%" => operators::modulus(n.pos, &left, &right)?,
            "等于" | "==" => operators::eq(n.pos, &left, &right)?,
            "不等于" | "!=" => match operators::eq(n.pos, &left, &right)? {
                Object::Bool(v) => Object::Bool(!v),
                other => other,
            },
            "小于" | "<" => operators::compare(n.pos, &left, &right, &[-1])?,
            "小于等于" | "<=" => operators::compare(n.pos, &left, &right, &[0, -1])?,
            "大于" | ">" => operators::compare(n.pos, &left, &right, &[1])?,
            "大于等于" | ">=" => operators::compare(n.pos, &left, &right, &[0, 1])?,
            "位与" | "&" => operators::bitwise(n.pos, &left, &right, "位与")?,
            "位或" | "|" => operators::bitwise(n.pos, &left, &right, "位或")?,
            "异或" | "^" => operators::bitwise(n.pos, &left, &right, "异或")?,
            "左移" | "<<" => operators::bitwise(n.pos, &left, &right, "左移")?,
            "右移" | ">>" => operators::bitwise(n.pos, &left, &right, "右移")?,
            other => return Err(FanError::at(format!("未知运算符：{}", other), n.pos)),
        };
        value(out)
    }

    fn eval_update(&mut self, n: &ast::UpdateExpr) -> EvalResult {
        let current = self.eval_expr(&n.target)?.into_value()?;
        let one = Object::Int(1);
        let next = match n.op.as_str() {
            "加" => operators::add(n.pos, &current, &one)?,
            "减" => operators::sub(n.pos, &current, &one)?,
            other => return Err(FanError::at(format!("未知自增运算符：{}", other), n.pos)),
        };
        if n.prefix {
            self.assign_to(&n.target, next.clone())?;
            value(next)
        } else {
            self.assign_to(&n.target, next)?;
            value(current)
        }
    }

    fn get_index(
        &self,
        target: &Object,
        index: &Object,
        pos: Position,
    ) -> Result<Object, FanError> {
        match target.unwrap_named() {
            Object::Array(arr) => {
                let Some(i) = index.as_int() else {
                    return Err(FanError::at(
                        format!("数组下标必须是整数，实际是 {}", index.kind()),
                        pos,
                    ));
                };
                let len = arr.len();
                if i < 0 || i as usize >= len {
                    return Err(FanError::at(
                        format!("数组下标越界：长度 {}，下标 {}", len, i),
                        pos,
                    ));
                }
                Ok(arr.get(i as usize).unwrap_or(Object::Nil))
            }
            Object::Str(text) => {
                let Some(i) = index.as_int() else {
                    return Err(FanError::at(
                        format!("字符串下标必须是整数，实际是 {}", index.kind()),
                        pos,
                    ));
                };
                let chars: Vec<char> = text.chars().collect();
                if i < 0 || i as usize >= chars.len() {
                    return Err(FanError::at(
                        format!("字符串下标越界：长度 {}，下标 {}", chars.len(), i),
                        pos,
                    ));
                }
                Ok(Object::Char(chars[i as usize]))
            }
            Object::Dict(dict) => {
                if !index.is_dict_key() {
                    return Err(FanError::at(
                        format!("不支持的字典键类型：{}", index.kind()),
                        pos,
                    ));
                }
                Ok(dict.get(index).unwrap_or(Object::Nil))
            }
            other => Err(FanError::at(
                format!("{} 不支持下标访问", other.kind()),
                pos,
            )),
        }
    }

    fn get_slice(&self, target: &Object, n: &ast::SliceExpr) -> Result<Object, FanError> {
        let len = match target.unwrap_named() {
            Object::Array(a) => a.len() as i64,
            Object::Str(s) => s.chars().count() as i64,
            other => return Err(FanError::at(format!("{} 不支持切片", other.kind()), n.pos)),
        };
        let low = match &n.low {
            Some(e) => {
                let mut interp = Interpreter::with_env(self, self.env.clone(), self.loader.clone());
                interp
                    .eval_expr(e)?
                    .into_value()?
                    .as_int()
                    .ok_or_else(|| FanError::at("切片下界必须是整数", n.pos))?
            }
            None => 0,
        };
        let high = match &n.high {
            Some(e) => {
                let mut interp = Interpreter::with_env(self, self.env.clone(), self.loader.clone());
                interp
                    .eval_expr(e)?
                    .into_value()?
                    .as_int()
                    .ok_or_else(|| FanError::at("切片上界必须是整数", n.pos))?
            }
            None => len,
        };
        let low = low.clamp(0, len);
        let high = high.clamp(low, len);
        match target.unwrap_named() {
            Object::Array(a) => {
                let mut out = Vec::new();
                for i in low..high {
                    if let Some(v) = a.get(i as usize) {
                        out.push(v);
                    }
                }
                Ok(Object::Array(FanArray::new(out)))
            }
            Object::Str(s) => {
                let out: String = s
                    .chars()
                    .skip(low as usize)
                    .take((high - low) as usize)
                    .collect();
                Ok(Object::string(out))
            }
            _ => unreachable!(),
        }
    }

    fn eval_interp_string(&mut self, n: &ast::InterpString) -> EvalResult {
        let mut out = String::new();
        for (i, part) in n.parts.iter().enumerate() {
            out.push_str(part);
            if let Some(e) = n.exprs.get(i) {
                let v = self.eval_expr(e)?.into_value()?;
                out.push_str(&v.inspect());
            }
        }
        value(Object::string(out))
    }

    // ------------------------------------------------------------ 函数

    fn eval_function_literal(&mut self, n: &ast::FunctionLit) -> EvalResult {
        use fan_object::Function;
        let mut fn_obj = Function {
            name: n.name.clone(),
            params: n.params.clone(),
            body: n.body.clone(),
            env: Some(self.env.clone()),
            pos: n.pos,
            return_types: n.return_types.clone(),
            tags: n.tags.clone(),
            visibility: Visibility::File,
            receiver_name: "自己".to_string(),
            class_name: String::new(),
            is_primitive_method: false,
            is_method: false,
            self_value: None,
            native_impl: None,
            owner: None,
            name_segments: n.name_segments.clone(),
            connectors: n.connectors.clone(),
            group_sizes: n.group_sizes.clone(),
            signature: n.signature.clone(),
        };
        // 应用自定义标签处理器（未注册标签静默忽略）。
        self.process_function_tags(&mut fn_obj)?;
        // `定义 本地(...)`：native 优先；找不到原生实现时保持 None，
        // 调用即回退执行函数体（见 apply_function 的 native_impl 分派）。
        if let Some(decl) = &n.native {
            if let Some(builtin) = self.bind_native(decl) {
                fn_obj.native_impl = Some(builtin);
            }
        }
        value(Object::Func(Rc::new(fn_obj)))
    }

    /// 按 `定义 本地("模块.函数")` 声明解析原生实现。
    ///
    /// 与旧 `@内建` 标签的关键差异是**静默回退**：以下三种「找不到」都返回
    /// `None` 而非报错，由调用方回退执行函数体：
    ///
    /// 1. 无模块前缀且名字不在已注册的 core 内建里；
    /// 2. 扩展模块的动态库加载失败（`ensure` 已内置默认实现回落，仍失败）；
    /// 3. 模块加载成功但其中没有该函数名。
    fn bind_native(&mut self, decl: &ast::NativeDecl) -> Option<Rc<Builtin>> {
        if let Some(module) = &decl.module {
            let rt = self.env.borrow().runtime();
            let loaded = {
                let mut rt_mut = rt.borrow_mut();
                self.ext.borrow_mut().ensure(&mut rt_mut, module)
            };
            // 库加载失败：静默回退函数体，不产生运行时信号。
            if loaded.is_err() {
                return None;
            }
        }
        // 无模块前缀：只查 core 已注册内建；有模块前缀：ensure 后再查。
        // 查无此名同样静默回退。
        self.env
            .borrow()
            .runtime()
            .borrow()
            .lookup_native(&decl.name)
    }

    /// 求 `...值` 并摊平成元素列表：数组字面量与调用实参共用。
    fn spread_elements(&mut self, s: &ast::SpreadExpr) -> Result<Vec<Object>, FanError> {
        let v = self.eval_expr(&s.value)?.into_value()?;
        match v.unwrap_named() {
            Object::Array(arr) => Ok(arr.elements()),
            other => Err(FanError::at(
                format!("展开 ... 仅支持数组，实际是 {}", other.kind()),
                s.pos,
            )),
        }
    }

    /// 无点调用 `"a,b,c" 拆分(",")`：以左侧值作接收者调用同名方法。
    ///
    /// 与 `"a,b,c".拆分(",")` 完全等价，只是省掉了连接符。
    fn eval_implicit_receiver_call(&mut self, n: &ast::ImplicitReceiverCallExpr) -> EvalResult {
        let receiver = self.eval_expr(&n.receiver)?.into_value()?;
        let method = self.get_member(&receiver, &n.name, n.pos)?;
        let mut args = Vec::with_capacity(n.args.len());
        for a in &n.args {
            args.push(self.eval_expr(a)?.into_value()?);
        }
        self.apply_function(&method, args, n.pos)
    }

    fn eval_call(&mut self, n: &ast::CallExpr) -> EvalResult {
        // `捕获()`：取走稍后块正在处理的抛出值。
        // 仅当作用域里没有同名定义时生效，用户自定义的同名函数始终优先。
        if let Expr::Ident(id) = &*n.callee {
            if id.name == "捕获" && n.args.is_empty() && !self.is_bound(&id.name) {
                self.throw_caught = true;
                return value(self.pending_throw.clone().unwrap_or(Object::Nil));
            }
        }

        // 分段函数：先求值实参，再按运行时类型重建签名做重载分派。
        if let Some(root) = n.mixfix_root.clone() {
            let mut args = Vec::with_capacity(n.args.len());
            for a in &n.args {
                match a {
                    Expr::Spread(s) => args.extend(self.spread_elements(s)?),
                    other => args.push(self.eval_expr(other)?.into_value()?),
                }
            }
            let signature = {
                let env_ref = self.env.borrow();
                build_runtime_mixfix_signature(&root, &args, &env_ref)
            };
            let func = {
                let rt = self.env.borrow().runtime();
                let rt_ref = rt.borrow();
                rt_ref.lookup_mixfix(&root, &signature)
            };
            if let Some(func) = func {
                let callee = Object::Func(func);
                return self.apply_function(&callee, args, n.pos);
            }
            return Err(FanError::at(
                format!("找不到匹配的分段函数：{signature}"),
                n.pos,
            ));
        }

        let callee = self.eval_expr(&n.callee)?.into_value()?;
        let mut args = Vec::with_capacity(n.args.len());
        for a in &n.args {
            match a {
                Expr::Spread(s) => args.extend(self.spread_elements(s)?),
                other => args.push(self.eval_expr(other)?.into_value()?),
            }
        }
        match callee.unwrap_named() {
            Object::Func(_) => self.apply_function(&callee, args, n.pos),
            Object::Builtin(b) => {
                let rt = self.env.borrow().runtime();
                let out =
                    (b.func)(&mut rt.borrow_mut(), args).map_err(|e| FanError::at(e, n.pos))?;
                value(single_or_tuple(out))
            }
            Object::Class(c) => value(self.instantiate(&c, args, n.pos)?),
            other => Err(FanError::at(
                format!("{} 不是可调用对象", other.kind()),
                n.pos,
            )),
        }
    }

    /// `检查 调用`：调用结果末尾携带错误时，立即向当前函数边界冒泡。
    ///
    /// 与 Go 版 `evalCheckExpr` 对齐：
    /// - 调用本身求值出错（含内层 `检查` 冒泡上来的信号）原样向上传播；
    /// - 返回值是错误，或多元值最后一个元素是错误时，产生 `Flow::Check`；
    /// - 多元值末尾是 `错误/空` 约定位时剥掉该位，只返回前面的真实结果。
    fn eval_check(&mut self, n: &ast::CheckExpr) -> EvalResult {
        let val = self.eval_expr(&n.call)?;
        let val = match val {
            // 内层检查/抛出等控制流信号直接向上冒泡
            signal @ (Flow::Return(_)
            | Flow::Break
            | Flow::Continue
            | Flow::Throw(_)
            | Flow::Check(_, _)) => return Ok(signal),
            Flow::Value(v) => v,
        };
        if let Some(err) = error_from_value(&val) {
            return Ok(Flow::Check(err, n.pos));
        }
        value(strip_error_tail(val))
    }

    /// 调用凡函数：新建作用域、绑定实参、执行函数体。
    pub fn apply_function(
        &mut self,
        callee: &Object,
        args: Vec<Object>,
        pos: Position,
    ) -> EvalResult {
        let Object::Func(f) = callee.unwrap_named() else {
            return Err(FanError::at("不是函数", pos));
        };
        let fn_obj = f.clone();

        // `定义 本地(...)` 绑定了原生实现的函数直接转调，不再执行函数体。
        if fn_obj.native_impl.is_some() {
            return self.call_native_impl(&fn_obj, args, pos);
        }

        // 参数个数校验。方法若把接收者显式写成首个形参（`问候(自己)`），
        // 它由 `自己` 绑定提供，不占实参位
        let param_list: Vec<_> = match fn_obj.params.first() {
            Some(p) if fn_obj.self_value.is_some() && p.name == fn_obj.receiver_name => {
                fn_obj.params.iter().skip(1).collect()
            }
            _ => fn_obj.params.iter().collect(),
        };
        let required = param_list
            .iter()
            .filter(|p| p.default.is_none() && !p.variadic)
            .count();
        if args.len() < required {
            return Err(FanError::at(
                format!(
                    "{} 需要至少 {} 个参数，实际 {} 个",
                    display_fn_name(&fn_obj),
                    required,
                    args.len()
                ),
                pos,
            ));
        }
        if !fn_obj.is_variadic() && args.len() > param_list.len() {
            return Err(FanError::at(
                format!(
                    "{} 最多接受 {} 个参数，实际 {} 个",
                    display_fn_name(&fn_obj),
                    param_list.len(),
                    args.len()
                ),
                pos,
            ));
        }

        let fn_env = Rc::new(RefCell::new(match &fn_obj.env {
            Some(e) => Environment::enclosed(e.clone()),
            None => Environment::enclosed(self.env.clone()),
        }));
        // 先求值默认值并完成参数类型校验。每次校验单独借用，避免默认值
        // 求值触发嵌套调用时与长期借用冲突。
        let mut bound: Vec<(String, Object, fan_ast::DeclType)> = Vec::new();
        {
            let mut idx = 0usize;
            for param in &param_list {
                if param.variadic {
                    let rest = args.get(idx..).unwrap_or(&[]).to_vec();
                    for val in &rest {
                        check_decl_type(&param.type_, val, &fn_env.borrow()).map_err(|reason| {
                            FanError::at(format!("参数 {} 类型不匹配：{}", param.name, reason), pos)
                        })?;
                    }
                    bound.push((param.name.clone(), Object::array(rest), param.type_.clone()));
                    break;
                }
                let value = match args.get(idx) {
                    Some(v) => {
                        idx += 1;
                        v.clone()
                    }
                    None => match &param.default {
                        Some(expr) => {
                            let mut sub =
                                Interpreter::with_env(self, fn_env.clone(), self.loader.clone());
                            sub.eval_expr(expr)?.into_value()?
                        }
                        None => Object::Nil,
                    },
                };
                check_decl_type(&param.type_, &value, &fn_env.borrow()).map_err(|reason| {
                    FanError::at(format!("参数 {} 类型不匹配：{}", param.name, reason), pos)
                })?;
                bound.push((param.name.clone(), value, param.type_.clone()));
            }
        }
        {
            let mut e = fn_env.borrow_mut();
            // 绑定接收者：方法体内的 `自己`
            if let Some(receiver) = &fn_obj.self_value {
                e.set_local(
                    fn_obj.receiver_name.clone(),
                    receiver.clone(),
                    fan_ast::DeclType::any(),
                );
            }
            for (name, value, dt) in bound {
                e.set_local(name, value, dt);
            }
        }

        // 切换作用域并执行
        let outer = self.env.clone();
        self.env = fn_env;
        self.fn_depth += 1;
        self.defers.push(Vec::new());
        // 记下本次调用是否身处某模型的方法内，供 `私有` 成员的访问判定
        self.self_class.push(match fn_obj.self_value.as_ref() {
            Some(Object::Instance(inst)) => Some(inst.class.name.clone()),
            _ => None,
        });
        let rt = self.env.borrow().runtime();
        rt.borrow_mut()
            .push_fn(display_fn_name(&fn_obj), fn_obj.pos);
        let result = self.eval_block_no_scope(&fn_obj.body);
        // `检查` 冒泡到本函数边界：在弹出当前栈帧之前先为错误补调用栈，
        // 这样沿调用链每一层都能把自己的帧累积进去（与 Go 版 cs.err.Stack 一致）。
        if let Ok(Flow::Check(Object::Error(cell), _)) = &result {
            let mut ev = cell.borrow_mut();
            if ev.stack.is_empty() {
                ev.stack = rt.borrow().stack();
            }
        }
        rt.borrow_mut().pop_fn();
        self.fn_depth -= 1;
        self.env = outer;

        let result = self.run_defers(result)?;
        // 稍后块仍属本次调用，故等它执行完再还原上下文
        self.self_class.pop();

        let return_types = fn_obj.return_types.clone();
        let check_env = self.env.clone();
        let coerce = |values: Vec<Object>| -> Result<Vec<Object>, FanError> {
            coerce_return_values(pos, &return_types, values, &check_env.borrow())
        };

        match result {
            Flow::Return(values) => {
                let values = coerce(values)?;
                value(single_or_tuple(values))
            }
            Flow::Value(v) => {
                let values = coerce(vec![v])?;
                value(single_or_tuple(values))
            }
            Flow::Break => Err(FanError::at("跳出 不能出现在函数外", pos)),
            Flow::Continue => Err(FanError::at("继续 不能出现在函数外", pos)),
            // 抛出保持为控制流信号继续上行：本层无人接管时，要由外层函数的
            // 稍后/捕获 处理。一路都没人管，才由顶层 run 报「未捕获的抛出」。
            Flow::Throw(v) => Ok(Flow::Throw(v)),
            // `检查` 冒泡到当前函数边界（对齐 Go checkReturn）：
            // 仅当函数声明了以 `错误` 收尾的返回类型时，才把错误作为正常返回值
            // 交回调用方（其余位补零值）；否则继续向更外层冒泡。
            Flow::Check(err, check_pos) => {
                let catches = matches!(return_types.last(), Some(dt) if dt.0 == "错误");
                if !catches {
                    return Ok(Flow::Check(err, check_pos));
                }
                let mut values: Vec<Object> = return_types
                    .iter()
                    .take(return_types.len() - 1)
                    .map(|dt| zero_value_for_type(dt, &check_env.borrow()))
                    .collect();
                values.push(err);
                let values = coerce(values)?;
                value(single_or_tuple(values))
            }
        }
    }

    /// 执行本次调用登记的 `稍后` 项，并处理 `捕获()`。
    ///
    /// 后进先出：最后登记的先执行。`抛出` 的值在执行前挂到 `pending_throw`，
    /// 稍后块里的 `捕获()` 读走它并置 `throw_caught`，此时抛出被视为已处理，
    /// 函数正常返回而不再向外传播。
    fn run_defers(&mut self, result: Result<Flow, FanError>) -> Result<Flow, FanError> {
        let frame = self.defers.pop().unwrap_or_default();
        if frame.is_empty() {
            return result;
        }
        let result = result?;

        // 嵌套调用（稍后块内再调函数）不能覆盖本次的抛出状态
        let saved_throw = self.pending_throw.clone();
        let saved_caught = self.throw_caught;
        if let Flow::Throw(v) = &result {
            self.pending_throw = Some(v.clone());
            self.throw_caught = false;
        }

        let mut outcome: Result<Flow, FanError> = Ok(Flow::Value(Object::Nil));
        for item in frame.iter().rev() {
            match self.run_deferred(item) {
                Ok(_) => {}
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
        }

        let caught = self.throw_caught;
        self.pending_throw = saved_throw;
        self.throw_caught = saved_caught;
        outcome?;

        if caught && matches!(result, Flow::Throw(_)) {
            return Ok(Flow::Value(Object::Nil));
        }
        Ok(result)
    }

    /// 执行单条 `稍后`：`函数调用` 直接求值，`函数` 字面量还要再调用一次。
    fn run_deferred(&mut self, item: &Deferred) -> EvalResult {
        let saved = self.env.clone();
        self.env = item.env.clone();
        let out = match &item.expr {
            ast::Expr::Function(_) => {
                let f = self.eval_expr(&item.expr)?.into_value()?;
                self.apply_function(&f, Vec::new(), item.expr.pos())
            }
            ast::Expr::Call(call) => {
                let callee = self.eval_expr(&call.callee)?.into_value()?;
                // 实参用登记时刻求好的那一份
                let args = item.eager_args.clone().unwrap_or_default();
                self.apply_function(&callee, args, item.expr.pos())
            }
            other => self.eval_expr(other),
        };
        self.env = saved;
        out
    }
}

// ---------------------------------------------------------------- 辅助

/// 按声明类型构造零值（与 Go 版 `zeroValueForType` 对齐）：
/// 命名类型按底层零值包一层身份；未知/模型/接口统一退化为 `空`。
fn zero_value_for_type(dt: &DeclType, env: &Environment) -> Object {
    if !dt.is_any() {
        if let Some(nt) = env.runtime().borrow().lookup_named_type(&dt.0).cloned() {
            let base = zero_value_for_type(&nt.base_type, env);
            return Object::named(nt.name, base);
        }
    }
    match dt.0.as_str() {
        "整数" => Object::Int(0),
        "小数" => Object::Float(0.0),
        "字符串" => Object::string(""),
        "布尔" => Object::Bool(false),
        "数组" => Object::array(Vec::new()),
        "字典" => Object::Dict(FanDict::new()),
        "字节" => Object::Byte(0),
        "字符" => Object::Char('\0'),
        // 错误位、空、模型、接口及其余类型统一为空
        _ => Object::Nil,
    }
}

/// 多返回值合并：单个值直接返回，多个值包装为多元值。
/// 运行时实参 → 分段签名使用的中文类型标签，与 parser 声明期类型名对齐。
/// 模型实例用模型名，命名类型解包到底层后按模型/内置名给出。
fn mixfix_type_label(value: &Object) -> String {
    match value.unwrap_named() {
        Object::Instance(inst) => inst.class.name.clone(),
        other => {
            let kind = runtime_type_name(&other);
            match kind_display_name(kind) {
                "未知" => kind.to_string(),
                name => name.to_string(),
            }
        }
    }
}

/// 按调用实参的运行时类型重建分段函数签名。
///
/// 形如 `修改(整数)的状态为(字符串)`，与 parser 的 `build_mixfix_signature`
/// 产出同构，只是类型标签来自实参而非声明。
fn build_runtime_mixfix_signature(root: &str, args: &[Object], env: &Environment) -> String {
    // 从注册表中取出该 root 下任一已登记函数，借它的分段结构（段名/连接词/分组大小）。
    let rt = env.runtime();
    let rt_ref = rt.borrow();
    let Some((_, sample)) = rt_ref
        .mixfix_functions
        .get(root)
        .and_then(|table| table.iter().next())
    else {
        return root.to_string();
    };

    let mut out = String::new();
    let mut index = 0usize;
    for (seg_index, segment) in sample.name_segments.iter().enumerate() {
        out.push_str(&sample.connectors[seg_index]);
        out.push_str(segment);
        out.push('(');
        let group_size = sample.group_sizes[seg_index];
        for offset in 0..group_size {
            if offset > 0 {
                out.push_str(", ");
            }
            if let Some(v) = args.get(index + offset) {
                out.push_str(&mixfix_type_label(v));
            }
        }
        out.push(')');
        index += group_size;
    }
    out
}

pub(crate) fn single_or_tuple(values: Vec<Object>) -> Object {
    match values.len() {
        0 => Object::Nil,
        1 => values.into_iter().next().unwrap(),
        _ => Object::tuple(values),
    }
}

/// 从调用结果中取错误值：单个错误，或多元值末尾的错误（与 Go `errorFromValue` 一致）。
fn error_from_value(val: &Object) -> Option<Object> {
    match val {
        Object::Error(_) => Some(val.clone()),
        Object::Tuple(t) => t.last().filter(|last| last.is_error()).cloned(),
        _ => None,
    }
}

/// 剥掉多元值末尾约定的 `错误/空` 位；单元素则直接返回该元素（与 Go `stripErrorTail` 一致）。
fn strip_error_tail(val: Object) -> Object {
    if let Object::Tuple(t) = &val {
        if t.len() > 1 {
            if let Some(last) = t.last() {
                if matches!(last.kind(), fan_object::Kind::Error | fan_object::Kind::Nil) {
                    let values: Vec<Object> = t[..t.len() - 1].to_vec();
                    return single_or_tuple(values);
                }
            }
        }
    }
    val
}

/// 按函数声明的返回类型校验返回值；未声明返回类型时不约束。
///
/// 与 Go 版 `coerceReturnValues` 一致：先比对返回值数量，再逐项做名义类型检查；
/// 声明为 `错误` 的位置允许 `空`。
fn coerce_return_values(
    pos: Position,
    types: &[fan_ast::DeclType],
    values: Vec<Object>,
    env: &Environment,
) -> Result<Vec<Object>, FanError> {
    if types.is_empty() {
        return Ok(values);
    }
    if types.len() != values.len() {
        return Err(FanError::at(
            format!(
                "返回值数量不符：声明 {} 个，实际 {} 个",
                types.len(),
                values.len()
            ),
            pos,
        ));
    }
    for (i, dt) in types.iter().enumerate() {
        let v = values.get(i).cloned().unwrap_or(Object::Nil);
        if matches!(v, Object::Nil) && dt.0 == "错误" {
            continue;
        }
        check_decl_type(dt, &v, env)
            .map_err(|reason| FanError::at(format!("第 {} 个返回值{}", i + 1, reason), pos))?;
    }
    Ok(values)
}

/// 拆分多元值；数量不符时给出中文错误（与 Go 版 `requireTuple` 一致）。
fn require_tuple(pos: Position, val: Object, count: usize) -> Result<Vec<Object>, FanError> {
    match val.unwrap_named() {
        Object::Tuple(t) => {
            if t.len() != count {
                return Err(FanError::at(
                    format!("多目标赋值数量不符：左侧 {} 个，右侧 {} 个", count, t.len()),
                    pos,
                ));
            }
            Ok(t.as_ref().clone())
        }
        other if count == 1 => Ok(vec![other]),
        other => Err(FanError::at(
            format!(
                "多目标赋值需要 {} 个返回值，实际是单个 {}",
                count,
                other.kind()
            ),
            pos,
        )),
    }
}

fn display_fn_name(f: &fan_object::Function) -> String {
    if f.name.is_empty() {
        "匿名函数".to_string()
    } else {
        f.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_parser::parse_program;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 解析并执行源码，返回最后的值。
    fn run(src: &str) -> Result<Object, String> {
        let (program, errors) = parse_program(src);
        if !errors.is_empty() {
            return Err(format!("解析错误：{errors}"));
        }
        let mut interp = Interpreter::new();
        interp.run(&program).map_err(|e| e.display())
    }

    /// 解析并执行，断言成功且返回指定显示文本。
    fn check(src: &str, expected: &str) {
        match run(src) {
            Ok(v) => assert_eq!(v.inspect(), expected, "源码：{src}"),
            Err(e) => panic!("执行失败：{e}\n源码：{src}"),
        }
    }

    /// 执行源码并把标准输出捕获为字符串。
    fn capture(src: &str) -> String {
        let (program, errors) = parse_program(src);
        assert!(errors.is_empty(), "解析错误：{errors}");
        let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let mut interp = Interpreter::with_output(buf.clone());
        interp.run(&program).expect("执行应成功");
        // 先取出字符串再返回：避免 borrow 的临时值存活到局部变量析构之后。
        let out = String::from_utf8(buf.borrow().clone()).unwrap();
        out
    }

    #[test]
    fn 空程序返回空() {
        assert_eq!(run("").unwrap(), Object::Nil);
    }

    #[test]
    fn 整数字面量与算术() {
        check("1 加 2", "3");
        check("7 减 2", "5");
        check("3 乘 4", "12");
        check("7 除 2", "3");
        check("7 取余 3", "1");
        check("1 加 2 乘 3", "7");
    }

    #[test]
    fn 小数运算() {
        check("1.5 加 2.5", "4");
        check("1 加 0.5", "1.5");
    }

    #[test]
    fn 除零报错() {
        assert!(run("1 除 0").unwrap_err().contains("除数不能为零"));
    }

    #[test]
    fn 字符串拼接与比较() {
        check("\"你好\" 加 \"世界\"", "你好世界");
        check("\"甲\" 等于 \"甲\"", "真");
        check("\"甲\" 等于 \"乙\"", "假");
    }

    #[test]
    fn 布尔与逻辑短路() {
        check("真 且 真", "真");
        check("真 且 假", "假");
        check("假 或 真", "真");
        check("非 真", "假");
    }

    #[test]
    fn 比较运算符() {
        check("1 小于 2", "真");
        check("2 小于等于 2", "真");
        check("3 大于 2", "真");
        check("2 大于等于 2", "真");
        check("1 不等于 2", "真");
    }

    #[test]
    fn 位运算() {
        check("12 位与 10", "8");
        check("1 左移 3", "8");
        check("位取反 0", "-1");
    }

    #[test]
    fn 变量声明与读取() {
        check("变量 整数 甲 = 10\n甲", "10");
        check("甲 = 10\n甲", "10");
    }

    #[test]
    fn 变量重新赋值() {
        check("变量 甲 = 1\n甲 = 2\n甲", "2");
    }

    #[test]
    fn 常量不可重新赋值() {
        assert!(run("常量 甲 = 1\n甲 = 2")
            .unwrap_err()
            .contains("不可再赋值"));
    }

    #[test]
    fn 声明类型不匹配报错() {
        assert!(run("变量 整数 甲 = \"文本\"")
            .unwrap_err()
            .contains("类型不匹配"));
    }

    #[test]
    fn 复合赋值() {
        check("变量 甲 = 1\n甲 += 4\n甲", "5");
        check("变量 甲 = 10\n甲 -= 3\n甲", "7");
    }

    #[test]
    fn 自增自减() {
        check("变量 甲 = 1\n甲++\n甲", "2");
        check("变量 甲 = 5\n甲--\n甲", "4");
        // 后缀自增返回原值
        check("变量 甲 = 1\n变量 乙 = 甲++\n乙", "1");
    }

    #[test]
    fn 条件语句分支() {
        check("如果 真 那么\n    1\n否则\n    2\n结束", "1");
        check("如果 假 那么\n    1\n否则\n    2\n结束", "2");
        check(
            "如果 假 那么\n    1\n否则如果 真 那么\n    3\n否则\n    2\n结束",
            "3",
        );
    }

    #[test]
    fn 条件必须是布尔() {
        assert!(run("如果 1 那么\n    1\n结束")
            .unwrap_err()
            .contains("必须是布尔"));
    }

    #[test]
    fn 当循环累加() {
        check(
            "变量 甲 = 0\n当 甲 小于 5 循环\n    甲 = 甲 加 1\n结束\n甲",
            "5",
        );
    }

    #[test]
    fn 循环中的跳出() {
        check("变量 甲 = 0\n当 真 循环\n    甲 = 甲 加 1\n    如果 甲 等于 3 那么\n        跳出\n    结束\n结束\n甲", "3");
    }

    #[test]
    fn 循环中的继续() {
        // 奇数累加：遇到偶数跳过
        check(
            "变量 和 = 0\n变量 甲 = 0\n当 甲 小于 6 循环\n    甲 = 甲 加 1\n    如果 甲 取余 2 等于 0 那么\n        继续\n    结束\n    和 = 和 加 甲\n结束\n和",
            "9",
        );
    }

    #[test]
    fn 重复直到循环() {
        check(
            "变量 甲 = 0\n重复\n    甲 = 甲 加 1\n直到 甲 等于 3\n结束\n甲",
            "3",
        );
    }

    #[test]
    fn 重复次数循环() {
        check("变量 甲 = 0\n重复 4 次\n    甲 = 甲 加 1\n结束\n甲", "4");
    }

    #[test]
    fn 遍历数组() {
        check(
            "变量 和 = 0\n遍历 [1, 2, 3] 中的 数\n    和 = 和 加 数\n结束\n和",
            "6",
        );
    }

    #[test]
    fn 遍历数组带序号() {
        check(
            "变量 和 = 0\n遍历 [10, 20] 中的 数 序号\n    和 = 和 加 序号\n结束\n和",
            "1",
        );
    }

    #[test]
    fn 遍历字符串逐字符() {
        check(
            "变量 计数 = 0\n遍历 \"你好\" 中的 字\n    计数 = 计数 加 1\n结束\n计数",
            "2",
        );
    }

    #[test]
    fn 遍历字典取键值() {
        // 两个名字时依次绑定 (键, 值)
        check(
            "变量 和 = 0\n遍历 {\"甲\": 1, \"乙\": 2} 中的 键 值\n    和 = 和 加 值\n结束\n和",
            "3",
        );
        // 只给一个名字时绑定值
        check(
            "变量 和 = 0\n遍历 {\"甲\": 1, \"乙\": 2} 中的 值\n    和 = 和 加 值\n结束\n和",
            "3",
        );
    }

    #[test]
    fn 数组字面量与下标() {
        check("[1, 2, 3]", "[1, 2, 3]");
        check("变量 甲 = [1, 2, 3]\n甲[1]", "2");
        check("变量 甲 = [1, 2, 3]\n甲[0] = 9\n甲", "[9, 2, 3]");
    }

    #[test]
    fn 数组下标越界报错() {
        assert!(run("变量 甲 = [1]\n甲[5]")
            .unwrap_err()
            .contains("下标越界"));
    }

    #[test]
    fn 数组是引用语义() {
        check("变量 甲 = [1]\n变量 乙 = 甲\n乙[0] = 9\n甲", "[9]");
    }

    #[test]
    fn 数组切片() {
        check("[1, 2, 3, 4][1:3]", "[2, 3]");
        check("[1, 2, 3][1:]", "[2, 3]");
        check("[1, 2, 3][:2]", "[1, 2]");
    }

    #[test]
    fn 字符串下标与切片() {
        check("\"你好\"[0]", "你");
        check("\"你好世界\"[1:3]", "好世");
    }

    #[test]
    fn 字典字面量与读写() {
        check("{\"甲\": 1}", "{甲: 1}");
        check("变量 甲 = {\"名\": \"小明\"}\n甲[\"名\"]", "小明");
        check("变量 甲 = {}\n甲[\"键\"] = 5\n甲", "{键: 5}");
    }

    #[test]
    fn 字典展开合并() {
        check(
            "变量 基础 = {\"甲\": 1}\n变量 合并 = {...基础, \"乙\": 2}\n合并",
            "{甲: 1, 乙: 2}",
        );
    }

    #[test]
    fn 分段函数调用() {
        check(
            "函数 从(字符串 起点)到(字符串 终点)乘坐(字符串 工具) -> 字符串\n\
                 返回 起点 + \"->\" + 终点 + \":\" + 工具\n\
             结束\n\
             从(\"北京\")到(\"上海\")乘坐(\"高铁\")",
            "北京->上海:高铁",
        );
        // 末段省略括号同样成立
        check(
            "函数 标记(字符串 值)的状态为无效() -> 字符串\n\
                 返回 值 + \":无效\"\n\
             结束\n\
              标记(\"b\")的状态为无效",
            "b:无效",
        );
    }

    #[test]
    fn 分段函数按参数类型重载() {
        check(
            "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"字符串\"\n\
             结束\n\
             函数 修改(整数 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"整数\"\n\
             结束\n\
             修改(\"x\")的状态为(\"a\")",
            "字符串",
        );
        check(
            "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"字符串\"\n\
             结束\n\
             函数 修改(整数 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"整数\"\n\
             结束\n\
             修改(1)的状态为(\"a\")",
            "整数",
        );
    }

    #[test]
    fn 同签名分段函数重复定义报错() {
        let err = run("函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"一\"\n\
             结束\n\
             函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"二\"\n\
             结束\n\
             修改(\"x\")的状态为(\"a\")")
        .unwrap_err();
        assert!(err.contains("重复的分段函数签名"), "实际：{err}");
    }

    #[test]
    fn 普通函数同名重复定义报错() {
        let err = run("函数 甲() -> 整数\n    返回 1\n结束\n\
             函数 甲() -> 整数\n    返回 2\n结束\n甲()")
        .unwrap_err();
        assert!(err.contains("函数 甲 重复定义"), "实际：{err}");
    }

    #[test]
    fn 同名变量重新赋值不受函数重复规则影响() {
        check("变量 甲 = 1\n甲 = 2\n甲", "2");
    }

    #[test]
    fn 分段函数不同签名同名重载仍然合法() {
        check(
            "函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"字符串\"\n\
             结束\n\
             函数 修改(整数 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"整数\"\n\
             结束\n修改(1)的状态为(\"a\")",
            "整数",
        );
    }

    #[test]
    fn 相似根段名互不串扰() {
        // 修改 与 修改用户 是两个独立 root，不能因前缀关系误分派
        check(
            "模型 用户\n    属性 字符串 名\n结束\n\
             模型 状态\n    属性 字符串 值\n结束\n\
             函数 修改用户(用户 当前用户)的状态为(状态 新状态) -> 字符串\n\
                 返回 当前用户.名 + \":\" + 新状态.值\n\
             结束\n\
             函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"字符串\"\n\
             结束\n\
             函数 修改(整数 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"整数\"\n\
             结束\n\
             变量 u = 用户()\nu.名 = \"张三\"\n\
             变量 s = 状态()\ns.值 = \"无效\"\n\
             修改用户(u)的状态为(s)",
            "张三:无效",
        );
        check(
            "模型 用户\n    属性 字符串 名\n结束\n\
             函数 修改用户(用户 当前用户)的状态为(字符串 标记) -> 字符串\n\
                 返回 \"用户\"\n\
             结束\n\
             函数 修改(整数 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"整数\"\n\
             结束\n\
             修改(1)的状态为(\"a\")",
            "整数",
        );
    }

    #[test]
    fn 无匹配分段签名时报错() {
        let err = run("函数 修改(字符串 值)的状态为(字符串 状态) -> 字符串\n\
                 返回 \"字符串\"\n\
             结束\n\
             修改(1)的状态为(\"a\")")
        .unwrap_err();
        assert!(err.contains("找不到匹配的分段函数"), "实际：{err}");
    }

    #[test]
    fn 检查成功剥掉错误位() {
        check(
            "函数 f() -> (整数, 错误)\n\
                 返回 42、空\n\
             结束\n\
             变量 x = 检查 f()\n\
             x",
            "42",
        );
    }

    #[test]
    fn 检查命中错误由当前函数接住() {
        // 安全除法声明了 (整数, 错误)，检查 命中后补零值并把错误作为正常返回
        let res = run("函数 除法(整数 a, 整数 b) -> (整数, 错误)\n\
                 如果 b 等于 0 那么\n\
                     返回 0、错误(\"零\")\n\
                 结束\n\
                 返回 a / b、空\n\
             结束\n\
             函数 安全除法() -> (整数, 错误)\n\
                 返回 检查 除法(10, 0)\n\
             结束\n\
             变量 r、e = 安全除法()\n\
             e")
        .unwrap();
        match res {
            Object::Error(cell) => {
                assert_eq!(cell.borrow().message, "零");
            }
            other => panic!("期望错误值，实际：{}", other.inspect()),
        }
    }

    #[test]
    fn 检查错误沿调用链传播并记录栈() {
        let res = run("函数 底层() -> (字符串, 错误)\n\
                 返回 \"\"、错误(\"打不开\")\n\
             结束\n\
             函数 中层() -> (字符串, 错误)\n\
                 返回 检查 底层()\n\
             结束\n\
             函数 顶层() -> (字符串, 错误)\n\
                 返回 检查 中层()\n\
             结束\n\
             变量 结果、e = 顶层()\n\
             e")
        .unwrap();
        match res {
            Object::Error(cell) => {
                let ev = cell.borrow();
                assert_eq!(ev.message, "打不开");
                let names: Vec<&str> = ev.stack.iter().map(|f| f.function.as_str()).collect();
                assert!(names.contains(&"中层"), "调用栈应含中层：{names:?}");
                assert!(names.contains(&"顶层"), "调用栈应含顶层：{names:?}");
            }
            other => panic!("期望错误值，实际：{}", other.inspect()),
        }
    }

    #[test]
    fn 无错误返回类型时检查冒泡到顶层() {
        let err = run("函数 坏() -> 错误\n\
                 返回 错误(\"炸了\")\n\
             结束\n\
             函数 上层() -> 整数\n\
                 变量 x = 检查 坏()\n\
                 返回 x\n\
             结束\n\
             上层()")
        .unwrap_err();
        assert!(err.contains("未捕获的检查错误"), "实际：{err}");
        assert!(err.contains("炸了"), "实际：{err}");
    }

    #[test]
    fn 检查赋值语句同样冒泡() {
        let err = run("函数 坏() -> 错误\n\
                 返回 错误(\"赋值路径\")\n\
             结束\n\
             函数 上层() -> 整数\n\
                 变量 x = 0\n\
                 x = 检查 坏()\n\
                 返回 x\n\
             结束\n\
             上层()")
        .unwrap_err();
        assert!(err.contains("赋值路径"), "实际：{err}");
    }

    #[test]
    fn 段间有空白是调用后取字段() {
        // 与分段调用只差一个空格：先调用，再按字段名取值
        check(
            "模型 结果\n\
                 属性 字符串 状态为无效\n\
             结束\n\
             函数 取值(字符串 前缀) -> 结果\n\
                 变量 r = 结果()\n\
                 r.状态为无效 = 前缀 + \":字段\"\n\
                 返回 r\n\
             结束\n\
             取值(\"s\") 的状态为无效",
            "s:字段",
        );
    }

    #[test]
    fn 稍后按后进先出执行() {
        check(
            "变量 记录 = []\n\
             函数 追加记录(字符串 文本)\n\
                 记录 = 追加(记录, 文本)\n\
             结束\n\
             函数 执行()\n\
                 稍后 追加记录(\"第二\")\n\
                 稍后 追加记录(\"第一\")\n\
             结束\n\
             执行()\n\
             记录",
            "[第一, 第二]",
        );
    }

    #[test]
    fn 稍后块内捕获抛出值() {
        check(
            "变量 结果 = \"\"\n\
             函数 执行()\n\
                 稍后 函数()\n\
                     结果 = 捕获()\n\
                 结束\n\
                 抛出 \"失败\"\n\
             结束\n\
             执行()\n\
             结果",
            "失败",
        );
    }

    #[test]
    fn 捕获可取任意实例() {
        check(
            "模型 自定义异常\n\
                 属性 字符串 原因\n\
             结束\n\
             变量 结果 = \"\"\n\
             函数 执行()\n\
                 稍后 函数()\n\
                     变量 异常 = 捕获()\n\
                     结果 = 异常.原因\n\
                 结束\n\
                 变量 异常 = 自定义异常()\n\
                 异常.原因 = \"自定义\"\n\
                 抛出 异常\n\
             结束\n\
             执行()\n\
             结果",
            "自定义",
        );
    }

    #[test]
    fn 未捕获的抛出仍然报错() {
        // 稍后块没有调用 捕获() 时，抛出继续向外传播
        assert!(run("函数 执行()\n抛出 \"失败\"\n结束\n执行()")
            .unwrap_err()
            .contains("未捕获的抛出"));
    }

    #[test]
    fn 稍后只能在函数内使用() {
        assert!(run("稍后 打印行(1)").unwrap_err().contains("只能在函数内"));
    }

    #[test]
    fn 数组内建成员长度与反转() {
        check("变量 a = [1, 2, 3]\na.长度", "3");
        check("变量 a = [1, 2, 3]\na.反转", "[3, 2, 1]");
    }

    #[test]
    fn 无点调用以左侧为接收者() {
        // `"a,b,c" 拆分(",")` 与带点的写法等价；外层套一次调用是为了让
        // 无点调用出现在实参位置——它只在语句级表达式中启用
        check(
            "定义 字符串 的 方法 喊() -> 字符串\n\
                 返回 自己 + \"!\"\n\
             结束\n\
             函数 回声(字符串 值) -> 字符串\n\
                 返回 值\n\
             结束\n\
             回声(\"你好\" 喊()) + 回声(\"你好\".喊())",
            "你好!你好!",
        );
    }

    #[test]
    fn 常用内建可直接使用中文名() {
        // 长度/追加/错误 无需导入 系统 模块
        check("长度([1, 2])", "2");
        check("追加([1], 2)", "[1, 2]");
    }

    #[test]
    fn 数组字面量内展开() {
        check(
            "变量 前 = [1, 2]\n\
             变量 后 = [4, 5]\n\
             [0, ...前, 3, ...后]",
            "[0, 1, 2, 3, 4, 5]",
        );
    }

    #[test]
    fn 展开非数组时报错() {
        assert!(run("[...1]").unwrap_err().contains("仅支持数组"));
    }

    #[test]
    fn 多变量无初值声明各为空() {
        // `变量 m, n` 两个名字都先为 空，之后仍可各自赋值
        check("变量 m, n\nm", "空");
        check("变量 m, n\nn", "空");
        check("变量 m, n\nm = 3\nn = \"三\"\nm", "3");
    }

    #[test]
    fn 嵌入模型可按模型名整体写入() {
        // `狗窝{动物: 动物{…}}` 用嵌入名当字段名填整个子实例
        check(
            "模型 动物\n\
                 属性 字符串 名字\n\
             结束\n\
             模型 狗窝\n\
                 属性 字符串 地址\n\
                 嵌入 动物\n\
             结束\n\
             变量 w = 狗窝{地址: \"北京\", 动物: 动物{名字: \"旺财\"}}\n\
             w.地址 + \"/\" + w.动物.名字",
            "北京/旺财",
        );
    }

    #[test]
    fn 自定义类型可作转换函数并带方法() {
        check(
            "类型 姓名 字符串\n\
             定义 姓名 的 方法 问候(任意 自己) -> 字符串\n\
                 返回 \"你好，\" + 自己\n\
             结束\n\
             变量 姓名 n = 姓名(\"凡\")\n\
             n.问候()",
            "你好，凡",
        );
    }

    #[test]
    fn 自定义类型保留身份而运算后回归底层() {
        // 是 判断能认出自定义类型；参与运算后身份消失、变回底层类型
        check(
            "类型 姓名 字符串\n\
             变量 姓名 n = 姓名(\"凡\")\n\
             n 是 姓名",
            "真",
        );
        check(
            "类型 状态 整数\n\
             常量 状态 有效 = 状态(1)\n\
             有效 加 1",
            "2",
        );
    }

    // ------------------------------------------------------------ 判断

    #[test]
    fn 判断按值匹配且分支可列多个值() {
        check(
            "变量 x = 2\n\
             变量 结果 = \"\"\n\
             判断 x\n\
                 为 1\n\
                     结果 = \"一\"\n\
                 为 2、3\n\
                     结果 = \"多\"\n\
                 其他\n\
                     结果 = \"无\"\n\
             结束\n\
             结果",
            "多",
        );
    }

    #[test]
    fn 判断守卫分支按顺序求值() {
        check(
            "变量 分数 = 85\n\
             变量 结果 = \"\"\n\
             判断\n\
                 当 分数 >= 90\n\
                     结果 = \"优秀\"\n\
                 当 分数 >= 60\n\
                     结果 = \"及格\"\n\
                 其他\n\
                     结果 = \"不及格\"\n\
             结束\n\
             结果",
            "及格",
        );
    }

    #[test]
    fn 判断可混合值匹配与守卫() {
        check(
            "变量 x = 500\n\
             变量 结果 = \"\"\n\
             判断 x\n\
                 为 200\n\
                     结果 = \"成功\"\n\
                 当 x >= 500\n\
                     结果 = \"错误\"\n\
             结束\n\
             结果",
            "错误",
        );
    }

    #[test]
    fn 判断守卫必须是布尔() {
        // 与 `当` 循环、`如果` 一致：不接受靠真值性蒙混过关的整数
        assert!(run("判断\n    当 1\n        变量 a = 1\n结束")
            .unwrap_err()
            .contains("布尔"));
    }

    #[test]
    fn 判断分支不穿透() {
        check(
            "变量 n = 0\n\
             判断 1\n\
                 为 1\n\
                     n++\n\
                 为 1\n\
                     n++\n\
             结束\n\
             n",
            "1",
        );
    }

    // ------------------------------------------------------------ 可见性

    #[test]
    fn 私有成员只在本模型的方法内可用() {
        // 对齐 Go 版 visibility_test：私有字段/方法对外不可见，
        // 但本模型的方法体内（含公开方法转调私有方法）可以
        let model = "定义 模型 账户\n\
             公开 属性 字符串 名字\n\
             私有 属性 整数 余额\n\
         结束\n\
         定义 账户 的 方法 初始化(字符串 名字, 整数 余额)\n\
             自己.名字 = 名字\n\
             自己.余额 = 余额\n\
         结束\n\
         定义 账户 的 公开 方法 存款(整数 数额)\n\
             自己.余额 = 自己.余额 加 数额\n\
         结束\n\
         定义 账户 的 私有 方法 内部密语() -> 字符串\n\
             返回 \"密-\" + 自己.名字\n\
         结束\n\
         定义 账户 的 公开 方法 密语() -> 字符串\n\
             返回 自己.内部密语()\n\
         结束\n";

        // 方法内访问私有成员、公开方法转调私有方法，都成立
        check(
            &format!("{model}变量 a = 账户(\"张三\", 100)\na.存款(50)\na.密语()"),
            "密-张三",
        );
        check(
            &format!("{model}变量 a = 账户(\"张三\", 100)\na.名字"),
            "张三",
        );

        // 模型外：读私有字段、写私有字段、调私有方法，均被拒
        assert!(run(&format!("{model}变量 a = 账户(\"张三\", 100)\na.余额"))
            .unwrap_err()
            .contains("私有的"));
        assert!(
            run(&format!("{model}变量 a = 账户(\"张三\", 100)\na.余额 = 5"))
                .unwrap_err()
                .contains("私有的")
        );
        assert!(run(&format!(
            "{model}变量 a = 账户(\"张三\", 100)\na.内部密语()"
        ))
        .unwrap_err()
        .contains("私有的"));
    }

    #[test]
    fn 未标注可见性的字段按默认可访问() {
        // 默认（文件级）不是私有：同一文件内照常读写
        check(
            "定义 模型 矩形\n\
                 属性 整数 宽\n\
                 属性 整数 高\n\
             结束\n\
             定义 矩形 的 方法 初始化(整数 宽, 整数 高)\n\
                 自己.宽 = 宽\n\
                 自己.高 = 高\n\
             结束\n\
             变量 r = 矩形(2, 3)\n\
             r.宽",
            "2",
        );
    }

    #[test]
    fn 稍后的实参在登记时求值() {
        // 与 Go 版一致：稍后 f(x) 的 x 取登记时刻的值，而非执行时刻
        check(
            "变量 结果 = \"\"\n\
             函数 记录(字符串 文本)\n\
                 结果 = 文本\n\
             结束\n\
             函数 f()\n\
                 变量 文本 = \"旧\"\n\
                 稍后 记录(文本)\n\
                 文本 = \"新\"\n\
             结束\n\
             f()\n\
             结果",
            "旧",
        );
    }

    #[test]
    fn 跨函数抛出由外层稍后捕获() {
        // 抛出要能越过函数边界上行，交给外层函数的 稍后/捕获 处理
        check(
            "变量 全局结果 = 空\n\
             函数 内层()\n\
                 抛出 123\n\
             结束\n\
             函数 外层()\n\
                 稍后 函数()\n\
                     全局结果 = 捕获()\n\
                 结束\n\
                 内层()\n\
             结束\n\
             外层()\n\
             全局结果",
            "123",
        );
    }

    #[test]
    fn 无人捕获的抛出向外报错() {
        assert!(run("函数 f()\n    抛出 \"坏\"\n结束\nf()")
            .unwrap_err()
            .contains("未捕获的抛出"));
    }

    #[test]
    fn 非稍后中的捕获为空() {
        check("函数 f()\n    返回 捕获()\n结束\nf()", "空");
    }

    #[test]
    fn 无参函数在语句位置自动调用() {
        check("变量 n = 0\n函数 计数()\n    n++\n结束\n计数\nn", "1");
    }

    #[test]
    fn 字符字面量支持全角引号() {
        check("变量 字符 c = 'A'\nc", "A");
        check("变量 字符 c = '中'\nc", "中");
        check("变量 字符 c = ‘A’\nc", "A");
    }

    #[test]
    fn 插值字符串代入变量() {
        check("变量 名 = \"小明\"\n\"你好 $名\"", "你好 小明");
    }

    #[test]
    fn 插值字符串代入表达式() {
        check("变量 数 = 3\n\"结果 ${数 乘 2}\"", "结果 6");
    }

    #[test]
    fn 打印输出文本() {
        assert_eq!(capture("打印(\"你好\")"), "你好");
    }

    #[test]
    fn 打印多个参数直接连接() {
        assert_eq!(capture("打印(\"甲\", \"乙\")"), "甲乙");
    }

    #[test]
    fn 函数调用与返回() {
        check("函数 加倍(任意 数)\n    返回 数 乘 2\n结束\n加倍(21)", "42");
    }

    #[test]
    fn 函数默认参数() {
        check(
            "函数 问候(任意 名, 字符串 称呼 = \"你好\")\n    返回 称呼\n结束\n问候(\"小明\")",
            "你好",
        );
    }

    #[test]
    fn 函数可变参数() {
        check(
            "函数 求和(任意 开头, 整数... 其余)\n    变量 和 = 开头\n    遍历 其余 中的 数\n        和 = 和 加 数\n    结束\n    返回 和\n结束\n求和(1, 2, 3)",
            "6",
        );
    }

    #[test]
    fn 函数参数个数不足报错() {
        assert!(run("函数 甲(任意 乙, 任意 丙)\n    返回 1\n结束\n甲(1)")
            .unwrap_err()
            .contains("需要至少 2 个参数"));
    }

    #[test]
    fn 闭包捕获外层变量() {
        check(
            "变量 基数 = 10\n函数 加基数(任意 数)\n    返回 数 加 基数\n结束\n加基数(5)",
            "15",
        );
    }

    #[test]
    fn 递归函数() {
        check(
            "函数 阶乘(任意 数)\n    如果 数 小于等于 1 那么\n        返回 1\n    结束\n    返回 数 乘 阶乘(数 减 1)\n结束\n阶乘(5)",
            "120",
        );
    }

    #[test]
    fn 多返回值解构() {
        check(
            "函数 除法()\n    返回 10、3\n结束\n变量 商、余 = 除法()\n商",
            "10",
        );
    }

    #[test]
    fn 三元表达式() {
        check("真 ? 1 : 2", "1");
        check("假 ? 1 : 2", "2");
    }

    #[test]
    fn 未定义标识符报错() {
        assert!(run("未知变量").unwrap_err().contains("未定义的标识符"));
    }

    #[test]
    fn 循环外跳出报错() {
        assert!(run("跳出").unwrap_err().contains("跳出 不能出现在循环外"));
    }

    #[test]
    fn 函数外返回报错() {
        assert!(run("返回 1").unwrap_err().contains("返回 不能出现在函数外"));
    }

    #[test]
    fn 抛出未捕获报错() {
        assert!(run("抛出 \"崩溃\"").unwrap_err().contains("未捕获的抛出"));
    }

    #[test]
    fn 块级作用域隔离() {
        // 条件分支内声明的变量不泄漏到外层
        assert!(run("如果 真 那么\n    变量 内部 = 1\n结束\n内部")
            .unwrap_err()
            .contains("未定义的标识符"));
    }

    #[test]
    fn 嵌套块可读取外层变量() {
        check("变量 甲 = 5\n如果 真 那么\n    打印(甲)\n结束\n甲", "5");
    }

    #[test]
    fn 命名类型需要显式转换() {
        // 与 Go 版一致：命名类型身份独立，底层整数不能隐式赋值。
        assert!(run("定义 类型 状态 = 整数\n变量 状态 甲 = 1")
            .unwrap_err()
            .contains("需显式转换"));
    }

    #[test]
    fn 联合命名类型可自动包装() {
        check("定义 类型 编号 = 整数 或 字符串\n变量 编号 甲 = 7\n甲", "7");
    }

    #[test]
    fn 参数类型不符时报错() {
        let err =
            run("函数 加一(整数 值) -> 整数\n    返回 值 加 1\n结束\n加一(\"文本\")").unwrap_err();
        assert!(err.contains("参数 值 类型不匹配"), "实际：{err}");
    }

    #[test]
    fn 参数类型匹配时正常执行() {
        check(
            "函数 加一(整数 值) -> 整数\n    返回 值 加 1\n结束\n加一(41)",
            "42",
        );
    }

    #[test]
    fn 返回值类型不符时报错() {
        let err = run("函数 取值() -> 字符串\n    返回 42\n结束\n取值()").unwrap_err();
        assert!(err.contains("返回值类型不匹配"), "实际：{err}");
    }

    #[test]
    fn 返回值数量不符时报错() {
        let err = run("函数 取值() -> (整数, 字符串)\n    返回 1\n结束\n取值()").unwrap_err();
        assert!(err.contains("返回值数量不符"), "实际：{err}");
    }

    #[test]
    fn 未声明返回类型时不约束() {
        check("函数 随便()\n    返回 42\n结束\n随便()", "42");
    }

    #[test]
    fn 默认参数也做类型校验() {
        // 默认值类型正确时正常
        check(
            "函数 问候(字符串 名字 = \"凡\") -> 字符串\n    返回 名字\n结束\n问候()",
            "凡",
        );
    }

    #[test]
    fn 多返回值按声明逐项校验() {
        check(
            "函数 拆分() -> (整数, 字符串)\n    返回 1, \"甲\"\n结束\n变量 a、b = 拆分()\na",
            "1",
        );
        let err = run("函数 拆分() -> (整数, 字符串)\n    返回 1, 2\n结束\n拆分()").unwrap_err();
        assert!(err.contains("第 2 个返回值"), "实际：{err}");
    }

    #[test]
    fn 方法参数同样受类型约束() {
        let err = run("定义 模型 计数器
结束
定义 计数器 的 方法 增加(整数 步长) -> 整数
    返回 步长
结束
计数器().增加(\"x\")")
        .unwrap_err();
        assert!(err.contains("参数 步长 类型不匹配"), "实际：{err}");
    }
}
