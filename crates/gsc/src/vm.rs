//! The `Host` trait, the heap-backed struct/entity/array data model at
//! runtime, and the instruction loop that walks `bytecode::Op`.
//!
//! `call_now` runs one function to completion on its own frame stack and
//! turns any `wait`/`waittill` it hits into an error: there is no scheduler
//! here to resume it later. Task 8 adds that scheduler on top of
//! `step_frames`, which already suspends cleanly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::atom::{Atom, Interner, StrAtom};
use crate::bytecode::{Function, Op};
use crate::heap::{ArrayKey, Heap};
use crate::value::{EntId, FuncRef, StructId, Value};

/// `self`/a call's receiver, once resolved to something field access and
/// notify/waittill/endon can key on. `level` and `game` are heap structs
/// and are receivers throughout the corpus (`level notify`, `level
/// waittill`, `level thread`, `level endon` number in the thousands), so
/// this cannot be `EntId` alone. Two `u32` newtypes rather than `Option
/// <Value>`: `Value` carries an `f32` and is neither `Eq` nor `Hash`, and
/// Task 8 uses `Target` as a map key and as `Thread::endons`'s element
/// type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Target {
    Entity(EntId),
    Struct(StructId),
}

#[derive(Clone, PartialEq, Debug)]
pub enum ErrorKind {
    /// The host has no such builtin. The pre-scan in Task 9 exists so this
    /// is reached only for a name that appeared dynamically.
    MissingBuiltin(Atom),
    BadType(&'static str),
    /// Per-frame instruction cap; see Task 8.
    Budget,
    /// A `wait` or `waittill` reached inside `call_now`.
    SuspendedInImmediateCall,
    Custom(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct ScriptError {
    pub file: Atom,
    pub func: Atom,
    pub line: u32,
    pub kind: ErrorKind,
}

/// Everything CoD-specific the VM can reach.
pub trait Host {
    fn builtin(
        &mut self,
        interner: &Interner,
        name: Atom,
        recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind>;

    /// Reading an unset field yields `Undefined` in gsc, so there is no
    /// error to report.
    fn get_field(&mut self, interner: &Interner, ent: EntId, field: Atom) -> Value;

    fn set_field(
        &mut self,
        interner: &Interner,
        ent: EntId,
        field: Atom,
        value: Value,
    ) -> Result<(), ErrorKind>;
}

pub struct Frame {
    pub func: FuncRef,
    pub ip: u32,
    pub locals: Vec<Value>,
    pub stack: Vec<Value>,
    pub recv: Option<Target>,
}

/// What one step of the interpreter can produce.
pub enum Step {
    Running,
    Returned(Value),
    Suspend(Suspend),
}

pub enum Suspend {
    Wait {
        seconds: f32,
    },
    WaitTill {
        target: Target,
        event: StrAtom,
        binds: Box<[u16]>,
    },
}

/// Converts an Int or Float value to `f32`; any other value has no numeric
/// reading.
fn to_f32(v: Value) -> Option<f32> {
    match v {
        Value::Int(i) => Some(i as f32),
        Value::Float(f) => Some(f),
        _ => None,
    }
}

/// A value used as an array subscript. A float key truncates toward zero,
/// matching the `(int)` cast rather than rejecting it outright, since a
/// loop counter that drifted to a float is more likely than a script that
/// meant to key by fractional value.
fn array_key(v: Value) -> Option<ArrayKey> {
    match v {
        Value::String(a) => Some(ArrayKey::Str(a)),
        Value::Int(i) => Some(ArrayKey::Int(i)),
        Value::Float(f) => Some(ArrayKey::Int(f as i32)),
        _ => None,
    }
}

/// Numeric equality promotes across Int/Float; anything else (including a
/// type mismatch) falls back to `Value`'s derived structural equality,
/// which is `false` for two different variants. This is what lets scripts
/// compare a possibly-`undefined` value with `==` and never error.
fn values_equal(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            to_f32(a) == to_f32(b)
        }
        _ => a == b,
    }
}

/// `x op y` for one of the four ordering comparisons, promoting Int/Float
/// like every other arithmetic op. A non-numeric operand is not an error
/// here either: it reads as `false`, the same rule as `Eq`/`Ne`.
fn numeric_cmp(a: Value, b: Value, f: impl Fn(f32, f32) -> bool) -> Value {
    match (to_f32(a), to_f32(b)) {
        (Some(x), Some(y)) => Value::Int(f(x, y) as i32),
        _ => Value::Int(0),
    }
}

fn eval_add(interner: &mut Interner, a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_add(y))),
        (Value::String(x), Value::String(y)) => {
            let s = format!("{}{}", interner.resolve_str(x), interner.resolve_str(y));
            Ok(Value::String(interner.intern_str(&s)))
        }
        (Value::Vector(x), Value::Vector(y)) => {
            Ok(Value::Vector([x[0] + y[0], x[1] + y[1], x[2] + y[2]]))
        }
        (a, b) => match (to_f32(a), to_f32(b)) {
            (Some(x), Some(y)) => Ok(Value::Float(x + y)),
            _ => Err(ErrorKind::BadType("+ needs numbers, strings or vectors")),
        },
    }
}

fn eval_sub(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_sub(y))),
        (Value::Vector(x), Value::Vector(y)) => {
            Ok(Value::Vector([x[0] - y[0], x[1] - y[1], x[2] - y[2]]))
        }
        (a, b) => match (to_f32(a), to_f32(b)) {
            (Some(x), Some(y)) => Ok(Value::Float(x - y)),
            _ => Err(ErrorKind::BadType("- needs numbers or vectors")),
        },
    }
}

fn eval_mul(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_mul(y))),
        (Value::Vector(v), s) | (s, Value::Vector(v)) => {
            let f = to_f32(s).ok_or(ErrorKind::BadType("vector * needs a scalar"))?;
            Ok(Value::Vector([v[0] * f, v[1] * f, v[2] * f]))
        }
        (a, b) => match (to_f32(a), to_f32(b)) {
            (Some(x), Some(y)) => Ok(Value::Float(x * y)),
            _ => Err(ErrorKind::BadType(
                "* needs numbers, or a vector and a scalar",
            )),
        },
    }
}

fn eval_div(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if y == 0 {
                return Err(ErrorKind::BadType("division by zero"));
            }
            Ok(Value::Int(x.wrapping_div(y)))
        }
        (a, b) => match (to_f32(a), to_f32(b)) {
            (Some(x), Some(y)) => Ok(Value::Float(x / y)),
            _ => Err(ErrorKind::BadType("/ needs numbers")),
        },
    }
}

fn eval_mod(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if y == 0 {
                return Err(ErrorKind::BadType("modulo by zero"));
            }
            Ok(Value::Int(x.wrapping_rem(y)))
        }
        (a, b) => match (to_f32(a), to_f32(b)) {
            (Some(x), Some(y)) => Ok(Value::Float(x % y)),
            _ => Err(ErrorKind::BadType("% needs numbers")),
        },
    }
}

fn eval_bitand(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x & y)),
        _ => Err(ErrorKind::BadType("& needs integers")),
    }
}

fn eval_bitor(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x | y)),
        _ => Err(ErrorKind::BadType("| needs integers")),
    }
}

fn eval_neg(v: Value) -> Result<Value, ErrorKind> {
    match v {
        Value::Int(i) => Ok(Value::Int(i.wrapping_neg())),
        Value::Float(f) => Ok(Value::Float(-f)),
        Value::Vector(v) => Ok(Value::Vector([-v[0], -v[1], -v[2]])),
        _ => Err(ErrorKind::BadType("unary - needs a number or a vector")),
    }
}

/// A call's receiver reads as a `Target` when it is an entity or a heap
/// struct (`level`, `game` are both, and both are common receivers in the
/// corpus); `undefined` and any other value leave the callee's `self`
/// unbound rather than erroring.
fn as_target(v: Value) -> Option<Target> {
    match v {
        Value::Entity(e) => Some(Target::Entity(e)),
        Value::Struct(s) => Some(Target::Struct(s)),
        _ => None,
    }
}

fn target_to_value(t: Target) -> Value {
    match t {
        Target::Entity(e) => Value::Entity(e),
        Target::Struct(s) => Value::Struct(s),
    }
}

pub struct Vm {
    interner: Interner,
    heap: Heap,
    functions: HashMap<FuncRef, Rc<Function>>,
    level: StructId,
    game: StructId,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        let mut heap = Heap::new();
        let level = heap.new_struct();
        let game = heap.new_struct();
        Vm {
            interner: Interner::default(),
            heap,
            functions: HashMap::new(),
            level,
            game,
        }
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    /// Rejects a function that fails `bytecode::stack_depth`'s abstract
    /// stack walk instead of installing it. That check is the only thing
    /// standing between a compiler bug and a panic in `step_frames` (which
    /// trusts the stack discipline it proves), so it has to run here, on
    /// every function this ever accepts — not just in the compiler's own
    /// test suite, which is the only place it ran before.
    pub fn install(&mut self, fns: Vec<Function>) -> Result<(), String> {
        for f in &fns {
            crate::bytecode::stack_depth(f)?;
        }
        for f in fns {
            let key = FuncRef {
                file: f.file,
                name: f.name,
            };
            self.functions.insert(key, Rc::new(f));
        }
        Ok(())
    }

    pub fn func_ref(&mut self, path: &str, name: &str) -> FuncRef {
        FuncRef {
            file: self.interner.intern(path),
            name: self.interner.intern(name),
        }
    }

    fn make_frame(
        &self,
        func: FuncRef,
        f: &Function,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> Frame {
        let mut locals = vec![Value::Undefined; f.locals as usize];
        let take = args.len().min(f.params as usize);
        locals[..take].copy_from_slice(&args[..take]);
        Frame {
            func,
            ip: 0,
            locals,
            stack: Vec::new(),
            recv,
        }
    }

    /// Runs instructions on the top frame, following calls and returns
    /// through `frames`, until the outermost frame in `frames` returns or
    /// execution reaches a `wait`/`waittill`. There is no per-step budget
    /// here yet, so `Step::Running` is not produced; Task 8's scheduler
    /// adds that and consumes it.
    ///
    /// Precondition: `frames` is non-empty. A caller (Task 8's scheduler,
    /// once it resumes a thread from its saved frame stack) must never
    /// invoke this with an empty stack; it panics rather than doing
    /// nothing, since there is no frame to attribute a `ScriptError` to.
    pub fn step_frames(
        &mut self,
        host: &mut dyn Host,
        frames: &mut Vec<Frame>,
    ) -> Result<Step, ScriptError> {
        loop {
            let top = frames.len() - 1;
            let func_ref = frames[top].func;
            let func = self
                .functions
                .get(&func_ref)
                .expect("frame references an installed function")
                .clone();
            let ip = frames[top].ip as usize;
            let op = func.code[ip].clone();
            let line = func.lines[ip];
            frames[top].ip += 1;

            let err = |kind: ErrorKind| ScriptError {
                file: func.file,
                func: func.name,
                line,
                kind,
            };

            macro_rules! pop {
                () => {
                    frames[top]
                        .stack
                        .pop()
                        .expect("stack_depth guarantees a value here")
                };
            }
            macro_rules! push {
                ($v:expr) => {
                    frames[top].stack.push($v)
                };
            }

            match op {
                Op::Const(idx) => push!(func.consts[idx as usize]),
                Op::LoadLocal(slot) => {
                    let v = frames[top].locals[slot as usize];
                    push!(v);
                }
                Op::StoreLocal(slot) => {
                    let v = pop!();
                    frames[top].locals[slot as usize] = v;
                }
                Op::LoadField(name) => {
                    let obj = pop!();
                    let v = match obj {
                        Value::Struct(id) => self.heap.get_field(id, name),
                        Value::Entity(id) => host.get_field(&self.interner, id, name),
                        _ => {
                            return Err(err(ErrorKind::BadType(
                                "field access needs a struct or entity",
                            )))
                        }
                    };
                    push!(v);
                }
                Op::StoreField(name) => {
                    let v = pop!();
                    let obj = pop!();
                    match obj {
                        Value::Struct(id) => self.heap.set_field(id, name, v),
                        Value::Entity(id) => {
                            host.set_field(&self.interner, id, name, v).map_err(err)?
                        }
                        _ => {
                            return Err(err(ErrorKind::BadType(
                                "field assignment needs a struct or entity",
                            )))
                        }
                    }
                }
                Op::LoadIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let Value::Array(id) = obj else {
                        return Err(err(ErrorKind::BadType("indexing needs an array")));
                    };
                    let k = array_key(key).ok_or_else(|| {
                        err(ErrorKind::BadType("array key must be a string or a number"))
                    })?;
                    push!(self.heap.get_index(id, k));
                }
                Op::StoreIndex => {
                    let v = pop!();
                    let key = pop!();
                    let obj = pop!();
                    let Value::Array(id) = obj else {
                        return Err(err(ErrorKind::BadType("indexing needs an array")));
                    };
                    let k = array_key(key).ok_or_else(|| {
                        err(ErrorKind::BadType("array key must be a string or a number"))
                    })?;
                    self.heap.set_index(id, k, v);
                }
                Op::LoadSelf => {
                    let v = frames[top]
                        .recv
                        .map(target_to_value)
                        .unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::LoadLevel => push!(Value::Struct(self.level)),
                Op::LoadGame => push!(Value::Struct(self.game)),
                // No stock MP script reads a bare `anim` back (every use is
                // inside a `/# #/` developer block, which the lexer drops
                // whole); `level`/`game` are the only preallocated structs.
                Op::LoadAnim => push!(Value::Undefined),
                Op::NewArray => {
                    let id = self.heap.new_array();
                    push!(Value::Array(id));
                }
                Op::MakeVector => {
                    let z = pop!();
                    let y = pop!();
                    let x = pop!();
                    let bad = || err(ErrorKind::BadType("vector component must be a number"));
                    let x = to_f32(x).ok_or_else(bad)?;
                    let y = to_f32(y).ok_or_else(bad)?;
                    let z = to_f32(z).ok_or_else(bad)?;
                    push!(Value::Vector([x, y, z]));
                }
                Op::Call {
                    func: target,
                    argc,
                    has_recv,
                    threaded: _,
                } => {
                    let mut args = Vec::with_capacity(argc as usize);
                    for _ in 0..argc {
                        args.push(pop!());
                    }
                    args.reverse();
                    let recv = if has_recv { as_target(pop!()) } else { None };
                    let callee = self.functions.get(&target).cloned().ok_or_else(|| {
                        err(ErrorKind::Custom(format!(
                            "no such function {}::{}",
                            self.interner.resolve(target.file),
                            self.interner.resolve(target.name)
                        )))
                    })?;
                    frames.push(self.make_frame(target, &callee, recv, args));
                }
                Op::CallBuiltin {
                    name,
                    argc,
                    has_recv,
                } => {
                    let mut args = Vec::with_capacity(argc as usize);
                    for _ in 0..argc {
                        args.push(pop!());
                    }
                    args.reverse();
                    let recv = if has_recv { as_target(pop!()) } else { None };
                    let v = host
                        .builtin(&self.interner, name, recv, &args)
                        .map_err(err)?;
                    push!(v);
                }
                Op::CallPtr {
                    argc,
                    has_recv,
                    threaded: _,
                } => {
                    let ptr = pop!();
                    let Value::Function(target) = ptr else {
                        return Err(err(ErrorKind::BadType(
                            "call target is not a function pointer",
                        )));
                    };
                    let mut args = Vec::with_capacity(argc as usize);
                    for _ in 0..argc {
                        args.push(pop!());
                    }
                    args.reverse();
                    let recv = if has_recv { as_target(pop!()) } else { None };
                    let callee = self.functions.get(&target).cloned().ok_or_else(|| {
                        err(ErrorKind::Custom(format!(
                            "no such function {}::{}",
                            self.interner.resolve(target.file),
                            self.interner.resolve(target.name)
                        )))
                    })?;
                    frames.push(self.make_frame(target, &callee, recv, args));
                }
                Op::Add => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_add(&mut self.interner, a, b).map_err(err)?);
                }
                Op::Sub => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_sub(a, b).map_err(err)?);
                }
                Op::Mul => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_mul(a, b).map_err(err)?);
                }
                Op::Div => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_div(a, b).map_err(err)?);
                }
                Op::Mod => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_mod(a, b).map_err(err)?);
                }
                Op::BitAnd => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_bitand(a, b).map_err(err)?);
                }
                Op::BitOr => {
                    let b = pop!();
                    let a = pop!();
                    push!(eval_bitor(a, b).map_err(err)?);
                }
                Op::Eq => {
                    let b = pop!();
                    let a = pop!();
                    push!(Value::Int(values_equal(a, b) as i32));
                }
                Op::Ne => {
                    let b = pop!();
                    let a = pop!();
                    push!(Value::Int(!values_equal(a, b) as i32));
                }
                Op::Lt => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x < y));
                }
                Op::Gt => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x > y));
                }
                Op::Le => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x <= y));
                }
                Op::Ge => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x >= y));
                }
                Op::Neg => {
                    let v = pop!();
                    push!(eval_neg(v).map_err(err)?);
                }
                Op::Not => {
                    let v = pop!();
                    push!(Value::Int(!v.is_truthy() as i32));
                }
                Op::CastInt => {
                    let v = pop!();
                    let i = match v {
                        Value::Int(i) => i,
                        Value::Float(f) => f as i32,
                        _ => return Err(err(ErrorKind::BadType("(int) needs a number"))),
                    };
                    push!(Value::Int(i));
                }
                Op::CastFloat => {
                    let v = pop!();
                    let f = match v {
                        Value::Int(i) => i as f32,
                        Value::Float(f) => f,
                        _ => return Err(err(ErrorKind::BadType("(float) needs a number"))),
                    };
                    push!(Value::Float(f));
                }
                Op::CastVector => {
                    let v = pop!();
                    match v {
                        Value::Vector(_) => push!(v),
                        _ => return Err(err(ErrorKind::BadType("(vector) needs a vector"))),
                    }
                }
                Op::Jump(t) => frames[top].ip = t,
                Op::JumpIfFalse(t) => {
                    let c = pop!();
                    if !c.is_truthy() {
                        frames[top].ip = t;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let c = pop!();
                    if c.is_truthy() {
                        frames[top].ip = t;
                    }
                }
                Op::Pop => {
                    pop!();
                }
                Op::Dup => {
                    let v = *frames[top]
                        .stack
                        .last()
                        .expect("stack_depth guarantees a value here");
                    push!(v);
                }
                Op::Return => {
                    let v = pop!();
                    frames.pop();
                    if frames.is_empty() {
                        return Ok(Step::Returned(v));
                    }
                    frames.last_mut().unwrap().stack.push(v);
                }
                Op::ReturnUndef => {
                    frames.pop();
                    if frames.is_empty() {
                        return Ok(Step::Returned(Value::Undefined));
                    }
                    frames.last_mut().unwrap().stack.push(Value::Undefined);
                }
                Op::Wait => {
                    let s = pop!();
                    let seconds = to_f32(s)
                        .ok_or_else(|| err(ErrorKind::BadType("wait needs a number of seconds")))?;
                    return Ok(Step::Suspend(Suspend::Wait { seconds }));
                }
                Op::WaitTill { binds } => {
                    let event = pop!();
                    let recv = pop!();
                    let target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "waittill needs an entity or struct receiver",
                        ))
                    })?;
                    let Value::String(event) = event else {
                        return Err(err(ErrorKind::BadType(
                            "waittill needs a string event name",
                        )));
                    };
                    return Ok(Step::Suspend(Suspend::WaitTill {
                        target,
                        event,
                        binds,
                    }));
                }
                // Neither op has a scheduler to reach yet (Task 8 owns the
                // waiting-thread registry both would signal or register
                // against), but the receiver is still type-checked and
                // resolved to a `Target` here so that wiring dispatch up
                // in Task 8 means touching this match arm, not adding a
                // new one beside code that silently discarded it.
                Op::Notify { argc } => {
                    for _ in 0..argc {
                        pop!();
                    }
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(_event) = event else {
                        return Err(err(ErrorKind::BadType("notify needs a string event name")));
                    };
                    let _target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "notify needs an entity or struct receiver",
                        ))
                    })?;
                }
                Op::EndOn => {
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(_event) = event else {
                        return Err(err(ErrorKind::BadType("endon needs a string event name")));
                    };
                    let _target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "endon needs an entity or struct receiver",
                        ))
                    })?;
                }
            }
        }
    }

    /// Runs `func` to completion on a fresh one-frame stack. A `wait` or
    /// `waittill` it reaches is a script error, not a hang: there is no
    /// scheduler here to come back to it later.
    pub fn call_now(
        &mut self,
        host: &mut dyn Host,
        func: FuncRef,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> Result<Value, ScriptError> {
        let f = self
            .functions
            .get(&func)
            .cloned()
            .ok_or_else(|| ScriptError {
                file: func.file,
                func: func.name,
                line: 0,
                kind: ErrorKind::Custom("no such function".to_string()),
            })?;
        let mut frames = vec![self.make_frame(func, &f, recv, args)];
        loop {
            match self.step_frames(host, &mut frames)? {
                Step::Running => continue,
                Step::Returned(v) => return Ok(v),
                Step::Suspend(_) => {
                    let top = frames
                        .last()
                        .expect("a suspend always leaves a frame on top");
                    let f = self
                        .functions
                        .get(&top.func)
                        .expect("frame references an installed function");
                    return Err(ScriptError {
                        file: f.file,
                        func: f.name,
                        line: f.lines[top.ip as usize - 1],
                        kind: ErrorKind::SuspendedInImmediateCall,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::Interner;
    use crate::value::{EntId, Value};
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestHost {
        pub calls: Vec<(String, Vec<Value>)>,
        pub fields: HashMap<(u32, String), Value>,
    }

    impl Host for TestHost {
        fn builtin(
            &mut self,
            interner: &Interner,
            name: Atom,
            _recv: Option<Target>,
            args: &[Value],
        ) -> Result<Value, ErrorKind> {
            let n = interner.resolve(name).to_string();
            self.calls.push((n.clone(), args.to_vec()));
            match n.as_str() {
                "double" => match args[0] {
                    Value::Int(i) => Ok(Value::Int(i * 2)),
                    _ => Err(ErrorKind::BadType("double wants an int")),
                },
                "isdefined" => Ok(Value::Int((args[0] != Value::Undefined) as i32)),
                _ => Ok(Value::Undefined),
            }
        }

        fn get_field(&mut self, interner: &Interner, e: EntId, f: Atom) -> Value {
            let k = (e.0, interner.resolve(f).to_string());
            self.fields.get(&k).copied().unwrap_or(Value::Undefined)
        }

        fn set_field(
            &mut self,
            interner: &Interner,
            e: EntId,
            f: Atom,
            v: Value,
        ) -> Result<(), ErrorKind> {
            self.fields
                .insert((e.0, interner.resolve(f).to_string()), v);
            Ok(())
        }
    }

    /// Compiles one file, then runs `main` to completion.
    fn run(src: &str) -> (Value, TestHost, Vm) {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let v = vm.call_now(&mut host, main, None, vec![]).unwrap();
        (v, host, vm)
    }

    #[test]
    fn arithmetic_and_precedence() {
        let (v, _, _) = run("main() { return 1 + 2 * 3; }");
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn int_and_float_mix_promotes_to_float() {
        let (v, _, _) = run("main() { return 1 + 0.5; }");
        assert_eq!(v, Value::Float(1.5));
    }

    #[test]
    fn integer_division_truncates_but_float_does_not() {
        let (v, _, _) = run("main() { return 7 / 2; }");
        assert_eq!(v, Value::Int(3));
        let (v, _, _) = run("main() { return 7.0 / 2; }");
        assert_eq!(v, Value::Float(3.5));
    }

    #[test]
    fn casts() {
        let (v, _, _) = run("main() { return (int)3.9; }");
        assert_eq!(v, Value::Int(3));
        let (v, _, _) = run("main() { return (float)3; }");
        assert_eq!(v, Value::Float(3.0));
    }

    #[test]
    fn vectors_add_componentwise() {
        let (v, _, _) = run("main() { return (1,2,3) + (10,20,30); }");
        assert_eq!(v, Value::Vector([11.0, 22.0, 33.0]));
    }

    #[test]
    fn short_circuit_does_not_evaluate_the_right_side() {
        let (_, host, _) = run("main() { x = 0 && double(1); }");
        assert!(!host.calls.iter().any(|(n, _)| n == "double"));
    }

    #[test]
    fn arrays_are_associative_and_grow_on_write() {
        let (v, _, _) = run(r#"main() { a = []; a["allies"] = 3; return a["allies"]; }"#);
        assert_eq!(v, Value::Int(3));
    }

    #[test]
    fn reading_an_unset_array_key_is_undefined() {
        let (v, _, _) = run(r#"main() { a = []; return a["nope"]; }"#);
        assert_eq!(v, Value::Undefined);
    }

    #[test]
    fn level_fields_persist_across_calls_within_a_run() {
        let (v, _, _) =
            run("main() { level.n = 5; return bump(); } bump() { return level.n + 1; }");
        assert_eq!(v, Value::Int(6));
    }

    #[test]
    fn builtins_receive_their_arguments_in_source_order() {
        let (_, host, _) = run(r#"main() { iprintln("a", 2); }"#);
        let (_, args) = host.calls.iter().find(|(n, _)| n == "iprintln").unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[1], Value::Int(2));
    }

    #[test]
    fn a_script_call_returns_its_value_and_recursion_works() {
        let (v, _, _) = run(
            "main() { return fact(5); } fact(n) { if(n <= 1) return 1; return n * fact(n - 1); }",
        );
        assert_eq!(v, Value::Int(120));
    }

    #[test]
    fn a_function_pointer_call_reaches_the_target() {
        let (v, _, _) = run(
            "main() { level.f = test\\script::helper; return [[level.f]](); } helper() { return 9; }",
        );
        assert_eq!(v, Value::Int(9));
    }

    #[test]
    fn a_switch_picks_the_matching_arm_and_falls_through_empty_labels() {
        let (v, _, _) = run(
            r#"main() { r = "axis"; switch(r) { case "allies": case "axis": return 1; default: return 2; } }"#,
        );
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn a_loop_runs_its_body_the_expected_number_of_times() {
        let (v, _, _) = run("main() { n = 0; for(i = 0; i < 10; i++) n += 2; return n; }");
        assert_eq!(v, Value::Int(20));
    }

    #[test]
    fn a_bad_type_names_the_line_it_failed_on() {
        let ast = crate::parse::parse_file("main() {\n  x = 1;\n  double(\"no\");\n}").unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert_eq!(e.line, 3);
    }

    #[test]
    fn suspending_inside_call_now_is_an_error_not_a_hang() {
        let ast = crate::parse::parse_file("main() { wait 1; }").unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::SuspendedInImmediateCall));
    }

    // --- Fix round 1: `level`/`game` as call and notify/waittill/endon
    // receivers, and the four brief rules that previously rested only on
    // review. ---

    /// `level thread f()` and friends are the majority of the corpus's
    /// control flow (996 `level notify`, 956 `level waittill`, 728 `level
    /// thread`, 408 `level endon` sites). `self` inside the callee must
    /// bind to the same heap struct `level` names, not fall back to
    /// `Undefined` the way a non-entity receiver used to.
    #[test]
    fn level_can_be_a_call_receiver_and_self_binds_to_it() {
        let (v, _, _) =
            run("main() { level thread helper(); return level.mark; } helper() { self.mark = 7; }");
        assert_eq!(v, Value::Int(7));
    }

    /// Before `Target`, `WaitTill` accepted only `Value::Entity` and a
    /// `level`/`game` receiver was a `BadType` script error, not a
    /// suspend — every stock gametype's round-end wait would have failed
    /// outright rather than blocking.
    #[test]
    fn level_waittill_suspends_rather_than_failing_on_receiver_type() {
        let ast = crate::parse::parse_file(r#"main() { level waittill("round_end"); }"#).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::SuspendedInImmediateCall));
    }

    /// `Notify`/`EndOn` used to pop and discard their receiver with no
    /// type check at all, so this was silently accepted either way. Now
    /// that the receiver is resolved to a `Target`, a `level`/`game`
    /// receiver must still be accepted, not newly rejected.
    #[test]
    fn level_notify_and_endon_accept_a_struct_receiver() {
        let (v, _, _) = run(r#"main() { level notify("go"); level endon("stop"); return 1; }"#);
        assert_eq!(v, Value::Int(1));
    }

    /// The other half of that same fix: a receiver that resolves to
    /// neither an entity nor a struct is now a reported `BadType`, not a
    /// silent no-op that leaves every waiting thread hanging forever.
    #[test]
    fn notify_with_an_undefined_receiver_is_a_bad_type_error_not_a_silent_no_op() {
        let ast = crate::parse::parse_file(r#"main() { x = undefined; x notify("go"); }"#).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::BadType(_)));
    }

    /// `Vm::install` now runs `bytecode::stack_depth` over every function
    /// before accepting it, so a compiler bug that leaks or underflows the
    /// stack is a rejected install, not a panic the first time
    /// `step_frames` reaches the bad instruction.
    #[test]
    fn install_rejects_a_function_that_fails_the_stack_walk() {
        let mut vm = Vm::new();
        let file = vm.interner_mut().intern("test/bad");
        let name = vm.interner_mut().intern("main");
        let bad = crate::bytecode::Function {
            file,
            name,
            params: 0,
            locals: 0,
            code: vec![crate::bytecode::Op::Pop, crate::bytecode::Op::ReturnUndef],
            consts: Vec::new(),
            lines: vec![1, 1],
        };
        assert!(vm.install(vec![bad]).is_err());
    }

    #[test]
    fn strings_concatenate_via_add() {
        let (v, _, mut vm) = run(r#"main() { return "foo" + "bar"; }"#);
        let Value::String(a) = v else {
            panic!("expected a string, got {v:?}")
        };
        assert_eq!(vm.interner_mut().resolve_str(a), "foobar");
    }

    /// The defect this task fixes: string *content* must not be case-folded.
    /// Folding here would send every script-built message to players
    /// lowercase.
    #[test]
    fn string_concatenation_preserves_case() {
        let (v, _, mut vm) = run(r#"main() { return "Round " + "won"; }"#);
        let Value::String(a) = v else {
            panic!("expected a string, got {v:?}")
        };
        assert_eq!(vm.interner_mut().resolve_str(a), "Round won");
    }

    /// Builtin dispatch still resolves the callee's name folded: a host
    /// matches on a lowercase literal like "iprintln" regardless of the
    /// case the script called it with.
    #[test]
    fn a_builtin_name_still_resolves_folded_even_when_called_with_mixed_case() {
        let (_, host, _) = run(r#"main() { IPrintLn("hi"); }"#);
        assert!(host.calls.iter().any(|(n, _)| n == "iprintln"));
    }

    /// Field names are identifiers, not content, so access stays
    /// case-insensitive: `level.Origin` and `level.origin` name the same slot.
    #[test]
    fn field_access_is_still_case_insensitive() {
        let (v, _, _) = run("main() { level.Origin = 3; return level.origin; }");
        assert_eq!(v, Value::Int(3));
    }

    #[test]
    fn a_vector_scales_by_a_trailing_scalar() {
        let (v, _, _) = run("main() { return (1,2,3) * 2; }");
        assert_eq!(v, Value::Vector([2.0, 4.0, 6.0]));
    }

    #[test]
    fn a_vector_scales_by_a_leading_scalar() {
        let (v, _, _) = run("main() { return 2 * (1,2,3); }");
        assert_eq!(v, Value::Vector([2.0, 4.0, 6.0]));
    }

    #[test]
    fn comparing_across_incompatible_types_reads_false_not_error() {
        let (v, _, _) = run(r#"main() { return "foo" > 1; }"#);
        assert_eq!(v, Value::Int(0));
    }

    #[test]
    fn integer_division_by_zero_is_a_bad_type_error_not_a_panic() {
        let ast = crate::parse::parse_file("main() { return 1 / 0; }").unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::BadType(_)));
    }
}
