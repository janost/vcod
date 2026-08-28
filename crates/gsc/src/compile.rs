//! Lowers a parsed `ast::File` to per-function bytecode.
//!
//! Calling convention for `Op::Call`/`Op::CallBuiltin`/`Op::CallPtr`: the
//! receiver (if `has_recv`) is pushed first, then each argument in order,
//! then (for `CallPtr` only) the function-pointer value itself, on top.
//! Every call op leaves exactly one value on the stack, including
//! `waittill`/`notify`/`endon`, which push a synthetic `undefined` after
//! their dedicated op so statement-level code can pop it uniformly.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    self, ArmLabel, BinOp, CallTarget, Cast, Expr, FuncDef, Stmt, StmtKind, SwitchArm, UnOp,
};
use crate::atom::{Atom, Interner};
use crate::bytecode::{Function, Op};
use crate::value::{FuncRef, Value};

#[derive(Debug)]
pub struct CompileError {
    pub line: u32,
    pub msg: String,
}

/// Compiles every function in `file`. `path` is this file's canonical
/// script path, interned once and used both as `Function::file` and to
/// resolve calls and `::name` pointers back into this file.
pub fn compile_file(
    file: &ast::File,
    path: &str,
    interner: &mut Interner,
) -> Result<Vec<Function>, CompileError> {
    let file_atom = interner.intern(path);
    // The AST does not track which `#using_animtree` was active at each
    // function's lexical position, so a bare `#animtree` resolves against
    // the last directive in the file. No stock MP script reads it back.
    let animtree = file.animtrees.last().map(|s| interner.intern(s));

    let mut local_funcs = HashSet::new();
    for f in &file.funcs {
        local_funcs.insert(interner.intern(&f.name));
    }

    let mut out = Vec::with_capacity(file.funcs.len());
    for f in &file.funcs {
        let c = Compiler {
            interner: &mut *interner,
            file_atom,
            local_funcs: &local_funcs,
            animtree,
            locals: HashMap::new(),
            code: Vec::new(),
            consts: Vec::new(),
            lines: Vec::new(),
            // Seeded from the function's declaration line; compile_stmt
            // overwrites it with each statement's own line before that
            // statement emits anything.
            cur_line: f.line,
            break_stack: Vec::new(),
            continue_stack: Vec::new(),
            switch_counter: 0,
        };
        out.push(c.compile_function(f)?);
    }
    Ok(out)
}

struct Compiler<'a> {
    interner: &'a mut Interner,
    file_atom: Atom,
    local_funcs: &'a HashSet<Atom>,
    animtree: Option<Atom>,
    locals: HashMap<String, u16>,
    code: Vec<Op>,
    consts: Vec<Value>,
    lines: Vec<u32>,
    cur_line: u32,
    /// Patch sites (indices into `code`) for `break`, one frame per
    /// enclosing loop or switch.
    break_stack: Vec<Vec<usize>>,
    /// Patch sites for `continue`, one frame per enclosing loop. A switch
    /// does not push a frame, so `continue` inside one reaches the loop
    /// around it.
    continue_stack: Vec<Vec<usize>>,
    /// Numbers each switch's hidden subject local (`$switch0`, `$switch1`,
    /// ...) so nested or sequential switches in one function get distinct
    /// slots. `$` cannot start a source identifier, so these never collide.
    switch_counter: u32,
}

impl<'a> Compiler<'a> {
    fn err(&self, msg: impl Into<String>) -> CompileError {
        CompileError {
            line: self.cur_line,
            msg: msg.into(),
        }
    }

    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.lines.push(self.cur_line);
        self.code.len() - 1
    }

    fn add_const(&mut self, v: Value) -> Result<u16, CompileError> {
        self.consts.push(v);
        u16::try_from(self.consts.len() - 1).map_err(|_| self.err("too many constants"))
    }

    fn push_const(&mut self, v: Value) -> Result<(), CompileError> {
        let idx = self.add_const(v)?;
        self.emit(Op::Const(idx));
        Ok(())
    }

    /// Gets or allocates a stable slot for a local, case-folded like every
    /// other identifier the engine matches. Re-mentioning an existing name
    /// (including a parameter) reuses its slot.
    fn local_slot(&mut self, name: &str) -> Result<u16, CompileError> {
        let folded = name.to_ascii_lowercase();
        if let Some(&slot) = self.locals.get(&folded) {
            return Ok(slot);
        }
        let slot = u16::try_from(self.locals.len()).map_err(|_| self.err("too many locals"))?;
        self.locals.insert(folded, slot);
        Ok(slot)
    }

    /// Allocates a parameter's slot. Unlike `local_slot`, this always
    /// allocates a fresh slot rather than reusing one for a repeated name:
    /// `make_frame` binds call arguments to `locals[0..params]` purely by
    /// position, so every formal parameter needs its own slot regardless of
    /// its name, or the argument count and the slot count disagree.
    ///
    /// A repeated name (`m(nTarget, eMG42, nGunner, nTarget)`, shipped in
    /// `maps/redsquare.gsc`) still gets a slot per occurrence; re-inserting
    /// the name maps every later body reference to the *last* parameter
    /// that declared it, so an earlier same-named parameter's slot is
    /// filled but unreachable by name. This was rejected as a compile error
    /// until the census found this file, which the retail engine plainly
    /// accepts.
    fn declare_param(&mut self, name: &str) -> Result<(), CompileError> {
        let folded = name.to_ascii_lowercase();
        let slot = u16::try_from(self.locals.len()).map_err(|_| self.err("too many locals"))?;
        self.locals.insert(folded, slot);
        Ok(())
    }

    fn patch(&mut self, idx: usize, target: u32) {
        match &mut self.code[idx] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => *t = target,
            other => unreachable!("patch target {other:?} is not a jump"),
        }
    }

    fn compile_function(mut self, def: &FuncDef) -> Result<Function, CompileError> {
        for p in &def.params {
            self.declare_param(p)?;
        }
        let params = u8::try_from(def.params.len()).map_err(|_| self.err("too many parameters"))?;
        self.compile_block(&def.body)?;
        // Every function returns. A jump already patched to land just past
        // the body (e.g. an `if` with no `else`, ending in `return`) relies
        // on this instruction actually being there, so the rule has to be
        // syntactic — the AST's last statement, not whichever op happened
        // to be emitted last — or such a jump lands one past the end of
        // `code`.
        let last_is_return = matches!(
            def.body.last(),
            Some(Stmt {
                kind: StmtKind::Return(_),
                ..
            })
        );
        if !last_is_return {
            self.emit(Op::ReturnUndef);
        }
        let name = self.interner.intern(&def.name);
        let locals = u16::try_from(self.locals.len()).map_err(|_| self.err("too many locals"))?;
        Ok(Function {
            file: self.file_atom,
            name,
            params,
            locals,
            code: self.code,
            consts: self.consts,
            lines: self.lines,
        })
    }

    fn compile_block(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        for s in stmts {
            self.compile_stmt(s)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, s: &Stmt) -> Result<(), CompileError> {
        // The AST attributes a line to the statement, not the instruction;
        // every op and error this statement produces is attributed to it.
        self.cur_line = s.line;
        match &s.kind {
            StmtKind::Expr(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Pop);
            }
            StmtKind::Assign { target, value } => self.compile_assign(target, value)?,
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => self.compile_if(cond, then, otherwise.as_deref())?,
            StmtKind::While { cond, body } => self.compile_while(cond, body)?,
            StmtKind::DoWhile { body, cond } => self.compile_do_while(body, cond)?,
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => self.compile_for(init.as_deref(), cond.as_ref(), step.as_deref(), body)?,
            StmtKind::Switch { subject, arms } => self.compile_switch(subject, arms)?,
            StmtKind::Return(e) => match e {
                Some(e) => {
                    self.compile_expr(e)?;
                    self.emit(Op::Return);
                }
                None => {
                    self.emit(Op::ReturnUndef);
                }
            },
            StmtKind::Break => self.compile_break()?,
            StmtKind::Continue => self.compile_continue()?,
            StmtKind::Wait(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Wait);
            }
        }
        Ok(())
    }

    fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<(), CompileError> {
        match target {
            Expr::Local(name) => {
                self.compile_expr(value)?;
                let slot = self.local_slot(name)?;
                self.emit(Op::StoreLocal(slot));
            }
            Expr::Field(obj, name) => {
                self.compile_expr(obj)?;
                self.compile_expr(value)?;
                let a = self.interner.intern(name);
                self.emit(Op::StoreField(a));
            }
            Expr::Index(obj, idx) => {
                self.compile_expr(obj)?;
                self.compile_expr(idx)?;
                self.compile_expr(value)?;
                self.emit(Op::StoreIndex);
            }
            _ => return Err(self.err("assignment target must be a variable, field or index")),
        }
        Ok(())
    }

    fn compile_if(
        &mut self,
        cond: &Expr,
        then: &[Stmt],
        otherwise: Option<&[Stmt]>,
    ) -> Result<(), CompileError> {
        self.compile_expr(cond)?;
        let jf = self.emit(Op::JumpIfFalse(0));
        self.compile_block(then)?;
        match otherwise {
            Some(else_body) => {
                let j_end = self.emit(Op::Jump(0));
                let else_addr = self.code.len() as u32;
                self.patch(jf, else_addr);
                self.compile_block(else_body)?;
                let end = self.code.len() as u32;
                self.patch(j_end, end);
            }
            None => {
                let end = self.code.len() as u32;
                self.patch(jf, end);
            }
        }
        Ok(())
    }

    fn compile_while(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), CompileError> {
        let cond_start = self.code.len() as u32;
        self.compile_expr(cond)?;
        let jf = self.emit(Op::JumpIfFalse(0));
        self.break_stack.push(Vec::new());
        self.continue_stack.push(Vec::new());
        self.compile_block(body)?;
        for idx in self.continue_stack.pop().unwrap() {
            self.patch(idx, cond_start);
        }
        self.emit(Op::Jump(cond_start));
        let epilogue = self.code.len() as u32;
        self.patch(jf, epilogue);
        for idx in self.break_stack.pop().unwrap() {
            self.patch(idx, epilogue);
        }
        Ok(())
    }

    // Body first, then the condition, then a backward `JumpIfTrue` to the
    // body's start, so the body always runs once. `continue` targets the
    // condition, not the top of the body.
    fn compile_do_while(&mut self, body: &[Stmt], cond: &Expr) -> Result<(), CompileError> {
        let body_start = self.code.len() as u32;
        self.break_stack.push(Vec::new());
        self.continue_stack.push(Vec::new());
        self.compile_block(body)?;
        let cond_start = self.code.len() as u32;
        for idx in self.continue_stack.pop().unwrap() {
            self.patch(idx, cond_start);
        }
        self.compile_expr(cond)?;
        self.emit(Op::JumpIfTrue(body_start));
        let epilogue = self.code.len() as u32;
        for idx in self.break_stack.pop().unwrap() {
            self.patch(idx, epilogue);
        }
        Ok(())
    }

    fn compile_for(
        &mut self,
        init: Option<&Stmt>,
        cond: Option<&Expr>,
        step: Option<&Stmt>,
        body: &[Stmt],
    ) -> Result<(), CompileError> {
        if let Some(init) = init {
            self.compile_stmt(init)?;
        }
        let cond_start = self.code.len() as u32;
        let jf = match cond {
            Some(cond) => {
                self.compile_expr(cond)?;
                Some(self.emit(Op::JumpIfFalse(0)))
            }
            None => None,
        };
        self.break_stack.push(Vec::new());
        self.continue_stack.push(Vec::new());
        self.compile_block(body)?;
        // `continue` targets the step, not the condition: it must still run
        // the step before looping back.
        let step_start = self.code.len() as u32;
        for idx in self.continue_stack.pop().unwrap() {
            self.patch(idx, step_start);
        }
        if let Some(step) = step {
            self.compile_stmt(step)?;
        }
        self.emit(Op::Jump(cond_start));
        let epilogue = self.code.len() as u32;
        if let Some(jf) = jf {
            self.patch(jf, epilogue);
        }
        for idx in self.break_stack.pop().unwrap() {
            self.patch(idx, epilogue);
        }
        Ok(())
    }

    // The subject goes into a hidden local rather than staying `Dup`'d on
    // the operand stack: bodies fall through into each other with no `Pop`
    // between them, so the stack depth carried into a body can't depend on
    // which label jumped there, and a body that runs off its end (an
    // idiomatic final case with no `break`) needs no matching pop either.
    //
    // Arms are walked in source order, `default` included among them: a
    // `case` tests the hidden local and jumps to this arm's landing
    // address; `default` isn't tested, it's the address the label chain
    // falls back to when nothing matches. Either way, an arm with an empty
    // body has no landing address of its own — its jump site (a `case`'s)
    // or its role as the fallback (`default`'s) is queued and resolved
    // against whichever body comes next, which is exactly the fallthrough
    // the corpus's stacked labels rely on.
    fn compile_switch(&mut self, subject: &Expr, arms: &[SwitchArm]) -> Result<(), CompileError> {
        self.compile_expr(subject)?;
        let tmp_name = format!("$switch{}", self.switch_counter);
        self.switch_counter += 1;
        let tmp = self.local_slot(&tmp_name)?;
        self.emit(Op::StoreLocal(tmp));

        let mut case_sites = Vec::with_capacity(arms.len());
        for arm in arms {
            if let ArmLabel::Case(label) = &arm.label {
                self.emit(Op::LoadLocal(tmp));
                self.compile_expr(label)?;
                self.emit(Op::Eq);
                case_sites.push(self.emit(Op::JumpIfTrue(0)));
            }
        }
        let chain_end = self.emit(Op::Jump(0));

        self.break_stack.push(Vec::new());
        let mut case_site = case_sites.into_iter();
        let mut pending: Vec<usize> = Vec::new();
        let mut default_pending = false;
        let mut default_addr: Option<u32> = None;

        for arm in arms {
            let is_default = matches!(arm.label, ArmLabel::Default);
            let case_jidx = if is_default {
                None
            } else {
                Some(case_site.next().expect("one jump site per case arm"))
            };
            if arm.body.is_empty() {
                if let Some(jidx) = case_jidx {
                    pending.push(jidx);
                }
                default_pending |= is_default;
                continue;
            }
            let addr = self.code.len() as u32;
            if let Some(jidx) = case_jidx {
                self.patch(jidx, addr);
            }
            for p in pending.drain(..) {
                self.patch(p, addr);
            }
            if default_pending || is_default {
                default_addr = Some(addr);
                default_pending = false;
            }
            self.compile_block(&arm.body)?;
        }

        // Whatever's left over — trailing empty cases, a trailing empty
        // `default`, or no `default` at all — lands at the switch's end.
        let epilogue = self.code.len() as u32;
        for p in pending.drain(..) {
            self.patch(p, epilogue);
        }
        if default_pending {
            default_addr = Some(epilogue);
        }
        self.patch(chain_end, default_addr.unwrap_or(epilogue));

        for idx in self.break_stack.pop().unwrap() {
            self.patch(idx, epilogue);
        }
        Ok(())
    }

    fn compile_break(&mut self) -> Result<(), CompileError> {
        if self.break_stack.is_empty() {
            return Err(self.err("break outside a loop or switch"));
        }
        let idx = self.emit(Op::Jump(0));
        self.break_stack.last_mut().unwrap().push(idx);
        Ok(())
    }

    fn compile_continue(&mut self) -> Result<(), CompileError> {
        if self.continue_stack.is_empty() {
            return Err(self.err("continue outside a loop"));
        }
        let idx = self.emit(Op::Jump(0));
        self.continue_stack.last_mut().unwrap().push(idx);
        Ok(())
    }

    fn compile_expr(&mut self, e: &Expr) -> Result<(), CompileError> {
        match e {
            Expr::Undefined => self.push_const(Value::Undefined)?,
            Expr::Int(n) => self.push_const(Value::Int(*n))?,
            Expr::Float(f) => self.push_const(Value::Float(*f))?,
            Expr::Str(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::String(a))?;
            }
            Expr::Localized(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::Localized(a))?;
            }
            Expr::Anim(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::Anim(a))?;
            }
            Expr::AnimtreeRef => match self.animtree {
                Some(a) => self.push_const(Value::Anim(a))?,
                None => self.push_const(Value::Undefined)?,
            },
            Expr::VectorLit(x, y, z) => {
                self.compile_expr(x)?;
                self.compile_expr(y)?;
                self.compile_expr(z)?;
                self.emit(Op::MakeVector);
            }
            Expr::EmptyArray => {
                self.emit(Op::NewArray);
            }
            Expr::Local(name) => {
                let slot = self.local_slot(name)?;
                self.emit(Op::LoadLocal(slot));
            }
            Expr::SelfRef => {
                self.emit(Op::LoadSelf);
            }
            Expr::LevelRef => {
                self.emit(Op::LoadLevel);
            }
            Expr::GameRef => {
                self.emit(Op::LoadGame);
            }
            Expr::AnimRef => {
                self.emit(Op::LoadAnim);
            }
            Expr::Field(obj, name) => {
                self.compile_expr(obj)?;
                let a = self.interner.intern(name);
                self.emit(Op::LoadField(a));
            }
            Expr::Index(obj, idx) => {
                self.compile_expr(obj)?;
                self.compile_expr(idx)?;
                self.emit(Op::LoadIndex);
            }
            // The bare `::name` form: an empty file means this file, not
            // literally the file named "".
            Expr::FuncRef { file, name } => {
                let file_atom = if file.is_empty() {
                    self.file_atom
                } else {
                    self.interner.intern(file)
                };
                let name_atom = self.interner.intern(name);
                self.push_const(Value::Function(FuncRef {
                    file: file_atom,
                    name: name_atom,
                }))?;
            }
            Expr::Call {
                recv,
                target,
                args,
                threaded,
            } => self.compile_call(recv, target, args, *threaded)?,
            Expr::Bin(BinOp::And, l, r) => self.compile_and(l, r)?,
            Expr::Bin(BinOp::Or, l, r) => self.compile_or(l, r)?,
            Expr::Bin(op, l, r) => {
                self.compile_expr(l)?;
                self.compile_expr(r)?;
                self.emit(bin_op(*op));
            }
            Expr::Un(UnOp::Neg, inner) => {
                self.compile_expr(inner)?;
                self.emit(Op::Neg);
            }
            Expr::Un(UnOp::Not, inner) => {
                self.compile_expr(inner)?;
                self.emit(Op::Not);
            }
            Expr::Cast(kind, inner) => {
                self.compile_expr(inner)?;
                self.emit(match kind {
                    Cast::Int => Op::CastInt,
                    Cast::Float => Op::CastFloat,
                    Cast::Vector => Op::CastVector,
                });
            }
        }
        Ok(())
    }

    // `left, Dup, JumpIfFalse(end), Pop, right, end:`. The surviving copy
    // of `left` on the false path is the short-circuit result.
    fn compile_and(&mut self, l: &Expr, r: &Expr) -> Result<(), CompileError> {
        self.compile_expr(l)?;
        self.emit(Op::Dup);
        let jf = self.emit(Op::JumpIfFalse(0));
        self.emit(Op::Pop);
        self.compile_expr(r)?;
        let end = self.code.len() as u32;
        self.patch(jf, end);
        Ok(())
    }

    fn compile_or(&mut self, l: &Expr, r: &Expr) -> Result<(), CompileError> {
        self.compile_expr(l)?;
        self.emit(Op::Dup);
        let jt = self.emit(Op::JumpIfTrue(0));
        self.emit(Op::Pop);
        self.compile_expr(r)?;
        let end = self.code.len() as u32;
        self.patch(jt, end);
        Ok(())
    }

    fn compile_call(
        &mut self,
        recv: &Option<Box<Expr>>,
        target: &CallTarget,
        args: &[Expr],
        threaded: bool,
    ) -> Result<(), CompileError> {
        // `waittill`, `notify` and `endon` are recognised by name at the
        // call site and compile to their dedicated ops instead of a normal
        // call, no matter what they resolve to as a function.
        if let CallTarget::Name(name) = target {
            match name.to_ascii_lowercase().as_str() {
                "waittill" => return self.compile_waittill(recv, args),
                "notify" => return self.compile_notify(recv, args),
                "endon" => return self.compile_endon(recv, args),
                _ => {}
            }
        }

        if let Some(r) = recv {
            self.compile_expr(r)?;
        }
        for a in args {
            self.compile_expr(a)?;
        }
        let argc = u8::try_from(args.len()).map_err(|_| self.err("too many arguments"))?;
        let has_recv = recv.is_some();
        match target {
            CallTarget::Name(name) => {
                let atom = self.interner.intern(name);
                if self.local_funcs.contains(&atom) {
                    self.emit(Op::Call {
                        func: FuncRef {
                            file: self.file_atom,
                            name: atom,
                        },
                        argc,
                        has_recv,
                        threaded,
                    });
                } else {
                    self.emit(Op::CallBuiltin {
                        name: atom,
                        argc,
                        has_recv,
                    });
                }
            }
            CallTarget::Path { file, name } => {
                let file_atom = self.interner.intern(file);
                let name_atom = self.interner.intern(name);
                self.emit(Op::Call {
                    func: FuncRef {
                        file: file_atom,
                        name: name_atom,
                    },
                    argc,
                    has_recv,
                    threaded,
                });
            }
            CallTarget::Deref(inner) => {
                self.compile_expr(inner)?;
                self.emit(Op::CallPtr {
                    argc,
                    has_recv,
                    threaded,
                });
            }
        }
        Ok(())
    }

    fn compile_waittill(
        &mut self,
        recv: &Option<Box<Expr>>,
        args: &[Expr],
    ) -> Result<(), CompileError> {
        let Some(r) = recv else {
            return Err(self.err("waittill needs a receiver"));
        };
        self.compile_expr(r)?;
        let Some((name_expr, bind_exprs)) = args.split_first() else {
            return Err(self.err("waittill needs an event name"));
        };
        self.compile_expr(name_expr)?;
        let mut binds = Vec::with_capacity(bind_exprs.len());
        for b in bind_exprs {
            let Expr::Local(n) = b else {
                return Err(self.err("waittill bind target must be a local variable"));
            };
            binds.push(self.local_slot(n)?);
        }
        self.emit(Op::WaitTill {
            binds: binds.into_boxed_slice(),
        });
        self.push_const(Value::Undefined)?;
        Ok(())
    }

    fn compile_notify(
        &mut self,
        recv: &Option<Box<Expr>>,
        args: &[Expr],
    ) -> Result<(), CompileError> {
        let Some(r) = recv else {
            return Err(self.err("notify needs a receiver"));
        };
        self.compile_expr(r)?;
        let Some((name_expr, extra)) = args.split_first() else {
            return Err(self.err("notify needs an event name"));
        };
        self.compile_expr(name_expr)?;
        for a in extra {
            self.compile_expr(a)?;
        }
        let argc = u8::try_from(extra.len()).map_err(|_| self.err("too many notify arguments"))?;
        self.emit(Op::Notify { argc });
        self.push_const(Value::Undefined)?;
        Ok(())
    }

    fn compile_endon(
        &mut self,
        recv: &Option<Box<Expr>>,
        args: &[Expr],
    ) -> Result<(), CompileError> {
        let Some(r) = recv else {
            return Err(self.err("endon needs a receiver"));
        };
        self.compile_expr(r)?;
        let Some(name_expr) = args.first() else {
            return Err(self.err("endon needs an event name"));
        };
        self.compile_expr(name_expr)?;
        self.emit(Op::EndOn);
        self.push_const(Value::Undefined)?;
        Ok(())
    }
}

fn bin_op(op: BinOp) -> Op {
    match op {
        BinOp::Add => Op::Add,
        BinOp::Sub => Op::Sub,
        BinOp::Mul => Op::Mul,
        BinOp::Div => Op::Div,
        BinOp::Mod => Op::Mod,
        BinOp::Eq => Op::Eq,
        BinOp::Ne => Op::Ne,
        BinOp::Lt => Op::Lt,
        BinOp::Gt => Op::Gt,
        BinOp::Le => Op::Le,
        BinOp::Ge => Op::Ge,
        BinOp::BitAnd => Op::BitAnd,
        BinOp::BitOr => Op::BitOr,
        BinOp::And | BinOp::Or => unreachable!("short-circuit ops are compiled separately"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::Interner;
    use crate::bytecode::Op;

    fn compile(src: &str) -> (Interner, Vec<crate::bytecode::Function>) {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut i = Interner::default();
        let fns = compile_file(&ast, "test/script", &mut i).unwrap();
        // Every function this suite compiles must pass the abstract stack
        // walk: a bad jump target or an inconsistent stack depth on some
        // path is exactly the class of bug a test asserting on isolated
        // ops (e.g. "one `go` and one `no`") can pass right over.
        for f in &fns {
            crate::bytecode::stack_depth(f)
                .unwrap_or_else(|e| panic!("stack_depth on {:?}: {e}", i.resolve(f.name)));
        }
        (i, fns)
    }

    #[test]
    fn locals_get_stable_slots_and_params_come_first() {
        let (_, f) = compile("m(a, b) { c = a; }");
        assert_eq!(f[0].params, 2);
        // a=0, b=1, c=2
        assert_eq!(f[0].locals, 3);
        assert!(f[0].code.contains(&Op::LoadLocal(0)));
        assert!(f[0].code.contains(&Op::StoreLocal(2)));
    }

    #[test]
    fn every_jump_lands_inside_the_code() {
        let (_, f) = compile("m() { if(a) { b(); } else { c(); } while(d) { e(); break; } }");
        let len = f[0].code.len() as u32;
        for op in &f[0].code {
            if let Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) = op {
                assert!(*t < len, "jump to {t} outside 0..{len}");
            }
        }
    }

    /// A function's last statement is an `if` with no `else`, whose only
    /// branch ends in `return`. The `if`'s `JumpIfFalse` was patched to
    /// land right after the then-branch, at the code length measured
    /// *then* — a real instruction only if the compiler still appends the
    /// implicit `ReturnUndef` despite the then-branch's `Op::Return` being
    /// the last op emitted. Checking the AST's last statement rather than
    /// the last op is what keeps that patch in bounds.
    #[test]
    fn a_trailing_if_with_no_else_ending_in_return_still_terminates_in_bounds() {
        let (_, f) = compile("m() { if(a) { return 1; } }");
        assert_eq!(f[0].code.last(), Some(&Op::ReturnUndef));
        let len = f[0].code.len() as u32;
        for op in &f[0].code {
            if let Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) = op {
                assert!(*t < len, "jump to {t} outside 0..{len}");
            }
        }
    }

    /// A case with no trailing `break` is idiomatic and must fall through
    /// into the next body without leaving a stray value behind — the
    /// `compile()` helper's `stack_depth` check is the real assertion
    /// here; a build without the fix panics inside `compile()` itself.
    #[test]
    fn a_case_with_no_trailing_break_falls_through_cleanly() {
        compile(r#"m() { switch(r) { case "a": go(); case "b": no(); break; } }"#);
    }

    /// `default` in the middle of a switch, with the following case falling
    /// into it exactly as it falls into any other case: `cover_stand.gsc`'s
    /// shape (`default:` sets state and falls into the case that acts on
    /// it). Checked by op order — the default's `StoreField` must precede
    /// the matched case's call, proving the default body physically
    /// precedes and falls into it rather than being appended at the end.
    #[test]
    fn a_non_trailing_default_falls_into_the_following_case() {
        let (_, f) = compile(
            r#"m() { switch(x) { default: self.pose = "y"; case "z": playanim(); break; } }"#,
        );
        let store_at = f[0]
            .code
            .iter()
            .position(|o| matches!(o, Op::StoreField(_)))
            .expect("default's assignment compiles to StoreField");
        let call_at = f[0]
            .code
            .iter()
            .position(|o| matches!(o, Op::CallBuiltin { .. }))
            .expect("the case's call");
        assert!(
            store_at < call_at,
            "default's body must fall into the following case, not follow it"
        );
    }

    /// `maps/redsquare.gsc` ships `mg42_target(nTarget, eMG42, nGunner,
    /// nTarget)` (`nTarget` repeated); retail accepts it, so this compiles
    /// rather than errors. Reduced here to the same shape with two names:
    /// `a` still gets one slot per occurrence (three formal parameters,
    /// three call-argument slots), but a body reference to `a` resolves to
    /// the *last* parameter that declared it, not the first.
    #[test]
    fn a_duplicate_parameter_name_compiles_and_the_last_one_wins() {
        let (_, f) = compile("m(a, b, a) { x = a; }");
        assert_eq!(f[0].params, 3);
        // `a`'s body reference must load the third parameter's slot (2),
        // the one the second `a` declared, not the first.
        assert!(f[0].code.contains(&Op::LoadLocal(2)));
    }

    #[test]
    fn short_circuit_and_emits_a_conditional_jump_not_a_call() {
        let (_, f) = compile("m() { x = a && b; }");
        assert!(f[0].code.iter().any(|o| matches!(o, Op::JumpIfFalse(_))));
    }

    /// An empty case must jump to the next arm's body, not to the epilogue.
    #[test]
    fn switch_fallthrough_shares_one_body() {
        let (_, f) =
            compile(r#"m() { switch(r) { case "a": case "b": go(); break; default: no(); } }"#);
        let calls = f[0]
            .code
            .iter()
            .filter(|o| matches!(o, Op::CallBuiltin { .. } | Op::Call { .. }))
            .count();
        assert_eq!(calls, 2, "one `go` and one `no`, shared by both labels");
    }

    #[test]
    fn a_call_to_a_function_in_this_file_resolves_to_call_not_builtin() {
        let (i, f) = compile("m() { helper(); } helper() { }");
        // Both functions take no arguments, so find `m` by name, not by shape.
        let want = i.get("m").unwrap();
        let m = f.iter().find(|f| f.name == want).unwrap();
        assert!(m.code.iter().any(|o| matches!(o, Op::Call { .. })));
        assert!(!m.code.iter().any(|o| matches!(o, Op::CallBuiltin { .. })));
    }

    #[test]
    fn an_unknown_bare_name_compiles_to_a_builtin_call() {
        let (i, f) = compile(r#"m() { iprintln("hi"); }"#);
        let name = i.get("iprintln").unwrap();
        assert!(f[0]
            .code
            .iter()
            .any(|o| matches!(o, Op::CallBuiltin { name: n, argc: 1, .. } if *n == name)));
    }

    #[test]
    fn wait_compiles_to_its_own_op() {
        let (_, f) = compile("m() { wait 0.05; }");
        assert!(f[0].code.contains(&Op::Wait));
    }

    #[test]
    fn waittill_records_the_slots_it_binds() {
        let (_, f) = compile(r#"m() { self waittill("menuresponse", menu, response); }"#);
        let Some(Op::WaitTill { binds, .. }) =
            f[0].code.iter().find(|o| matches!(o, Op::WaitTill { .. }))
        else {
            panic!("no WaitTill emitted")
        };
        assert_eq!(binds.len(), 2, "menu and response");
    }

    /// Mirrors `brecourt.gsc`'s shape: a `notify` and a `waittill` on the
    /// same event whose literal spelling differs only in case. The engine
    /// matches event names case-insensitively, same as any other
    /// identifier, so both event-name literals must intern to the same
    /// atom — otherwise the `waittill` would wait forever on a `notify`
    /// that, textually, never fires.
    #[test]
    fn event_names_differing_only_by_case_intern_to_the_same_atom() {
        let (_, f) = compile(r#"m() { level notify("Explode"); level waittill("explode"); }"#);
        let mut string_consts = f[0].consts.iter().filter_map(|c| match c {
            Value::String(a) => Some(*a),
            _ => None,
        });
        let notify_event = string_consts.next().expect("notify's event string");
        let waittill_event = string_consts.next().expect("waittill's event string");
        assert_eq!(notify_event, waittill_event);
    }

    #[test]
    fn a_function_without_an_explicit_return_still_returns() {
        let (_, f) = compile("m() { a(); }");
        assert_eq!(f[0].code.last(), Some(&Op::ReturnUndef));
    }

    #[test]
    fn break_outside_a_loop_is_a_compile_error() {
        let ast = crate::parse::parse_file("m() { break; }").unwrap();
        let mut i = Interner::default();
        assert!(compile_file(&ast, "test/script", &mut i).is_err());
    }
}
