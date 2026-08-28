//! Lowers a parsed `ast::File` to per-function bytecode.
//!
//! Calling convention for `Op::Call`/`Op::CallBuiltin`/`Op::CallPtr`: the
//! receiver (if `has_recv`) is pushed first, then each argument in order,
//! then (for `CallPtr` only) the function-pointer value itself, on top.
//! Every call op leaves exactly one value on the stack, including
//! `waittill`/`notify`/`endon`, which push a synthetic `undefined` after
//! their dedicated op so statement-level code can pop it uniformly.

use std::collections::{HashMap, HashSet};

use crate::ast::{self, BinOp, CallTarget, Cast, Expr, FuncDef, Stmt, SwitchArm, UnOp};
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
            // Statements carry no line of their own in this AST; every
            // instruction and error in a function is attributed to the
            // function's declaration line.
            cur_line: f.line,
            break_stack: Vec::new(),
            continue_stack: Vec::new(),
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

    fn add_const(&mut self, v: Value) -> u16 {
        self.consts.push(v);
        (self.consts.len() - 1) as u16
    }

    fn push_const(&mut self, v: Value) {
        let idx = self.add_const(v);
        self.emit(Op::Const(idx));
    }

    /// Gets or allocates a stable slot for a local, case-folded like every
    /// other identifier the engine matches.
    fn local_slot(&mut self, name: &str) -> u16 {
        let folded = name.to_ascii_lowercase();
        let next = self.locals.len() as u16;
        *self.locals.entry(folded).or_insert(next)
    }

    fn patch(&mut self, idx: usize, target: u32) {
        match &mut self.code[idx] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => *t = target,
            other => unreachable!("patch target {other:?} is not a jump"),
        }
    }

    fn compile_function(mut self, def: &FuncDef) -> Result<Function, CompileError> {
        for p in &def.params {
            self.local_slot(p);
        }
        let params = def.params.len() as u8;
        self.compile_block(&def.body)?;
        // Every function returns; append the implicit undefined return
        // unless the body's last emitted instruction is already one.
        match self.code.last() {
            Some(Op::Return) | Some(Op::ReturnUndef) => {}
            _ => {
                self.emit(Op::ReturnUndef);
            }
        }
        let name = self.interner.intern(&def.name);
        Ok(Function {
            file: self.file_atom,
            name,
            params,
            locals: self.locals.len() as u16,
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
        match s {
            Stmt::Expr(e) => {
                self.compile_expr(e)?;
                self.emit(Op::Pop);
            }
            Stmt::Assign { target, value } => self.compile_assign(target, value)?,
            Stmt::If {
                cond,
                then,
                otherwise,
            } => self.compile_if(cond, then, otherwise.as_deref())?,
            Stmt::While { cond, body } => self.compile_while(cond, body)?,
            Stmt::DoWhile { body, cond } => self.compile_do_while(body, cond)?,
            Stmt::For {
                init,
                cond,
                step,
                body,
            } => self.compile_for(init.as_deref(), cond.as_ref(), step.as_deref(), body)?,
            Stmt::Switch {
                subject,
                arms,
                default,
            } => self.compile_switch(subject, arms, default.as_deref())?,
            Stmt::Return(e) => match e {
                Some(e) => {
                    self.compile_expr(e)?;
                    self.emit(Op::Return);
                }
                None => {
                    self.emit(Op::ReturnUndef);
                }
            },
            Stmt::Break => self.compile_break()?,
            Stmt::Continue => self.compile_continue()?,
            Stmt::Wait(e) => {
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
                let slot = self.local_slot(name);
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

    // Emits the label-test chain, then the bodies in order so an empty case
    // falls into the next one. Every possible entry point into the body
    // region (each arm whose body isn't merged into a later one, plus the
    // default) starts with its own `Pop` of the duplicated subject; arms
    // with an empty body are folded into whichever body comes next by
    // patching their `JumpIfTrue` to that body's `Pop`, so the subject is
    // popped exactly once no matter which label matched.
    fn compile_switch(
        &mut self,
        subject: &Expr,
        arms: &[SwitchArm],
        default: Option<&[Stmt]>,
    ) -> Result<(), CompileError> {
        self.compile_expr(subject)?;

        let mut jump_sites = Vec::with_capacity(arms.len());
        for arm in arms {
            self.emit(Op::Dup);
            self.compile_expr(&arm.label)?;
            self.emit(Op::Eq);
            jump_sites.push(self.emit(Op::JumpIfTrue(0)));
        }
        let chain_end = self.emit(Op::Jump(0));

        self.break_stack.push(Vec::new());
        let mut pending: Vec<usize> = Vec::new();
        for (arm, &jidx) in arms.iter().zip(jump_sites.iter()) {
            if arm.body.is_empty() {
                pending.push(jidx);
                continue;
            }
            let addr = self.code.len() as u32;
            self.patch(jidx, addr);
            for p in pending.drain(..) {
                self.patch(p, addr);
            }
            self.emit(Op::Pop);
            self.compile_block(&arm.body)?;
        }

        let default_addr = self.code.len() as u32;
        self.patch(chain_end, default_addr);
        for p in pending.drain(..) {
            self.patch(p, default_addr);
        }
        self.emit(Op::Pop);
        if let Some(default_body) = default {
            self.compile_block(default_body)?;
        }

        let epilogue = self.code.len() as u32;
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
            Expr::Undefined => self.push_const(Value::Undefined),
            Expr::Int(n) => self.push_const(Value::Int(*n)),
            Expr::Float(f) => self.push_const(Value::Float(*f)),
            Expr::Str(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::String(a));
            }
            Expr::Localized(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::Localized(a));
            }
            Expr::Anim(s) => {
                let a = self.interner.intern(s);
                self.push_const(Value::Anim(a));
            }
            Expr::AnimtreeRef => match self.animtree {
                Some(a) => self.push_const(Value::Anim(a)),
                None => self.push_const(Value::Undefined),
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
                let slot = self.local_slot(name);
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
                }));
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
        let argc = args.len() as u8;
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
            binds.push(self.local_slot(n));
        }
        self.emit(Op::WaitTill {
            binds: binds.into_boxed_slice(),
        });
        self.push_const(Value::Undefined);
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
        self.emit(Op::Notify {
            argc: extra.len() as u8,
        });
        self.push_const(Value::Undefined);
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
        self.push_const(Value::Undefined);
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
                assert!(*t <= len, "jump to {t} outside 0..={len}");
            }
        }
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
