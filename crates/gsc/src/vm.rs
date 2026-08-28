//! The `Host` trait, the heap-backed struct/entity/array data model at
//! runtime, and the instruction loop that walks `bytecode::Op`.
//!
//! `call_now` runs one function to completion on its own frame stack and
//! turns any `wait`/`waittill` it hits into an error: there is no scheduler
//! here to resume it later. Task 8 adds that scheduler on top of
//! `step_frames`, which already suspends cleanly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::atom::{Atom, Interner};
use crate::bytecode::{Function, Op};
use crate::heap::{ArrayKey, Heap};
use crate::value::{EntId, FuncRef, StructId, Value};

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
        recv: Option<EntId>,
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
    pub recv: Option<EntId>,
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
        ent: EntId,
        event: Atom,
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
            let s = format!("{}{}", interner.resolve(x), interner.resolve(y));
            Ok(Value::String(interner.intern(&s)))
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

/// A call's receiver reads as `EntId` when it is one; anything else
/// (`level`, `game`, `undefined`) leaves the callee's `self` unbound rather
/// than erroring, since `Frame::recv` has no way to represent it.
fn as_ent(v: Value) -> Option<EntId> {
    match v {
        Value::Entity(e) => Some(e),
        _ => None,
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

    pub fn install(&mut self, fns: Vec<Function>) {
        for f in fns {
            let key = FuncRef {
                file: f.file,
                name: f.name,
            };
            self.functions.insert(key, Rc::new(f));
        }
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
        recv: Option<EntId>,
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
                        .map(Value::Entity)
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
                    let recv = if has_recv { as_ent(pop!()) } else { None };
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
                    let recv = if has_recv { as_ent(pop!()) } else { None };
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
                    let recv = if has_recv { as_ent(pop!()) } else { None };
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
                    let Value::Entity(ent) = recv else {
                        return Err(err(ErrorKind::BadType("waittill needs an entity receiver")));
                    };
                    let Value::String(event) = event else {
                        return Err(err(ErrorKind::BadType(
                            "waittill needs a string event name",
                        )));
                    };
                    return Ok(Step::Suspend(Suspend::WaitTill { ent, event, binds }));
                }
                Op::Notify { argc } => {
                    for _ in 0..argc {
                        pop!();
                    }
                    pop!(); // event name
                    pop!(); // receiver
                }
                Op::EndOn => {
                    pop!(); // event name
                    pop!(); // receiver
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
        recv: Option<EntId>,
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
            _recv: Option<EntId>,
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
        vm.install(fns);
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
        vm.install(fns);
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
        vm.install(fns);
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let e = vm.call_now(&mut host, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::SuspendedInImmediateCall));
    }
}
