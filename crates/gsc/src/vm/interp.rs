//! The instruction loop: walks `bytecode::Op` over a frame stack until it
//! suspends, returns, or the caller-supplied budget runs out.

use std::rc::Rc;

use crate::atom::{Atom, Interner};
use crate::bytecode::{Function, Op};
use crate::heap::ArrayKey;
use crate::value::{format_number, FuncRef, Value};

use super::sched::ThreadId;
use super::{Cx, ErrorKind, Host, ScriptError, Target, Vm};

/// Interpreter-internal: not part of the public API, since nothing outside
/// this crate drives `step_frames` directly, and doing so needs the
/// non-empty-`frames` precondition its doc comment states -- one an
/// embedder has no way to satisfy correctly from outside the scheduler.
#[derive(Debug)]
pub(crate) struct Frame {
    pub func: FuncRef,
    /// The installed function `func` resolves to, cached at frame creation
    /// so `step_frames`'s hot loop reads it straight off the frame instead
    /// of hitting `Vm::functions` on every single instruction. Every path
    /// that produces a `Frame` goes through `make_frame`, which sets this;
    /// a suspended frame just carries it along in `Thread::frames` across
    /// the wait, unchanged, rather than being rebuilt on resume. `func`
    /// stays alongside it, unresolved, for error attribution -- a
    /// `ScriptError` names the file/function by `Atom`, not by the `Rc`.
    pub func_rc: Rc<Function>,
    pub ip: u32,
    pub locals: Vec<Value>,
    pub stack: Vec<Value>,
    pub recv: Option<Target>,
    /// Test-only: what this frame's `stack` length must be once the call
    /// currently running on top of it returns, per `stack_effect`'s
    /// declared `(pop, push)` for that `Call`/`CallPtr`. Lives on the frame,
    /// not a local in `step_frames`, because a call can suspend (`wait`)
    /// several layers deep and resume in a later `step_frames` invocation —
    /// this has to survive that.
    #[cfg(test)]
    stack_effect_check: Option<i32>,
}

/// What one step of the interpreter can produce. Interpreter-internal, same
/// as `Frame`.
pub(crate) enum Step {
    Running,
    Returned(Value),
    Suspend(Suspend),
}

pub(crate) enum Suspend {
    Wait {
        seconds: f32,
    },
    WaitTill {
        target: Target,
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

/// Numeric equality promotes across Int/Float. A number against a string
/// is rendered and compared textually, and `undefined` against anything
/// else is an error, both measured
/// (tests/fixtures/semantics/retail-captures.txt, `# probe_cmp*`).
fn values_equal(interner: &Interner, a: Value, b: Value) -> Result<bool, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x == y),
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            Ok(to_f32(a) == to_f32(b))
        }
        // `"5" == 5` holds, `"5.0" == 5` does not.
        (Value::String(s), n @ (Value::Int(_) | Value::Float(_)))
        | (n @ (Value::Int(_) | Value::Float(_)), Value::String(s)) => {
            Ok(interner.resolve(s) == format_number(n, interner).expect("a number renders"))
        }
        // Atoms, so this is case-sensitive: `"ABC" == "abc"` is false.
        (Value::String(x), Value::String(y)) => Ok(x == y),
        // Not measured; retail's message names a "pair has unmatching
        // types", which two `undefined`s are not (§9 of the research doc).
        (Value::Undefined, Value::Undefined) => Ok(true),
        (Value::Undefined, _) | (_, Value::Undefined) => Err(ErrorKind::BadType(
            "cannot compare undefined against a value",
        )),
        // Not measured. `Value`'s derived `PartialEq`, so a mixed pair
        // (vector against a string) reads false rather than the fatal
        // "pair has unmatching types" retail's error text suggests, and two
        // vectors, entities, arrays or function pointers compare by value
        // and can read true. No probe has driven this arm (§9 of the
        // research doc).
        _ => Ok(a == b),
    }
}

/// `x op y` for one of the four ordering comparisons, promoting Int/Float
/// like every other arithmetic op. Retail has no ordering outside numbers:
/// `"a" < "b"` is fatal there (`# probe_cmp_order`).
fn numeric_cmp(a: Value, b: Value, f: impl Fn(f32, f32) -> bool) -> Result<Value, ErrorKind> {
    match (to_f32(a), to_f32(b)) {
        (Some(x), Some(y)) => Ok(Value::Int(f(x, y) as i32)),
        _ => Err(ErrorKind::BadType("< > <= >= need numbers")),
    }
}

fn eval_add(interner: &mut Interner, a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_add(y))),
        (Value::Vector(x), Value::Vector(y)) => {
            Ok(Value::Vector([x[0] + y[0], x[1] + y[1], x[2] + y[2]]))
        }
        // A string on either side concatenates whatever renders
        // (`# probe_concat`); `intern_exact`, not `intern_folded`, so a built
        // display string keeps its case.
        (Value::String(x), other) => {
            let rhs = format_number(other, interner).ok_or(ErrorKind::BadType(
                "+ cannot concatenate that onto a string",
            ))?;
            let s = format!("{}{rhs}", interner.resolve(x));
            Ok(Value::String(interner.intern_exact(&s)))
        }
        (other, Value::String(y)) => {
            let lhs = format_number(other, interner).ok_or(ErrorKind::BadType(
                "+ cannot concatenate that onto a string",
            ))?;
            let s = format!("{lhs}{}", interner.resolve(y));
            Ok(Value::String(interner.intern_exact(&s)))
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
/// struct (`level` is one, and a common receiver in the corpus); `game` is
/// array-typed, not a struct, so it falls through to `None` here like any
/// other non-entity, non-struct value -- no corpus script uses `game` as a
/// receiver, which is what makes that safe. `undefined` and any other
/// value leave the callee's `self` unbound rather than erroring.
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

impl Vm {
    pub(crate) fn make_frame(
        &self,
        func: FuncRef,
        func_rc: Rc<Function>,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> Frame {
        let mut locals = vec![Value::Undefined; func_rc.locals as usize];
        let take = args.len().min(func_rc.params as usize);
        locals[..take].copy_from_slice(&args[..take]);
        Frame {
            func,
            func_rc,
            ip: 0,
            locals,
            stack: Vec::new(),
            recv,
            #[cfg(test)]
            stack_effect_check: None,
        }
    }

    /// Runs instructions on the top frame, following calls and returns
    /// through `frames`, until the outermost frame in `frames` returns,
    /// execution reaches a `wait`/`waittill`, or `budget` runs out
    /// (`Step::Running`).
    ///
    /// Precondition: `frames` is non-empty. A caller (the scheduler, once
    /// it resumes a thread from its saved frame stack) must never invoke
    /// this with an empty stack; it panics rather than doing nothing,
    /// since there is no frame to attribute a `ScriptError` to.
    ///
    /// `budget` is the number of instructions this call may run before
    /// giving up with `Step::Running`; every instruction counts against it
    /// uniformly, `Call`/`CallPtr` included, so the caller decides what
    /// "budget exhausted" means (the scheduler treats it as an error).
    /// `thread_id`, when this call is running a real scheduled thread
    /// (not `call_now`'s throwaway one-shot stack), is that thread's own
    /// id: `Op::EndOn` uses it to write straight into
    /// `self.threads[..].endons` the instant it executes, so a registration
    /// earlier in this call is visible to a notify a nested spawn fires
    /// later in the very same call, not just to notifies that happen after
    /// this call returns. `notifies` collects every `Op::Notify` this call
    /// reaches, in order, for the caller to apply once this call returns: a
    /// notify is never resolved here, so a script cannot wake a thread
    /// mid-instruction. `errors` collects any `ScriptError` raised by a
    /// thread that a threaded call spawns and immediately runs during this
    /// call (`spawn` recurses into `step_thread`, which recurses back into
    /// this function on the new thread's own frames) -- a nested spawn's
    /// failure does not abort this thread's own execution, only its own.
    pub(crate) fn step_frames(
        &mut self,
        host: &mut dyn Host,
        frames: &mut Vec<Frame>,
        budget: u32,
        thread_id: Option<ThreadId>,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        errors: &mut Vec<ScriptError>,
    ) -> Result<Step, ScriptError> {
        let mut remaining = budget;
        loop {
            if remaining == 0 {
                return Ok(Step::Running);
            }
            remaining -= 1;
            let top = frames.len() - 1;
            let func = frames[top].func_rc.clone();
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

            // Test-only: captured before `op` is consumed by the match
            // below, so `stack_effect`'s table can be checked against what
            // this instruction actually does to the operand stack. A
            // non-threaded `Call`/`CallPtr` doesn't push its declared
            // return value onto *this* frame -- it pushes a new one and the
            // value lands on this frame only once the callee's `Return`
            // runs, possibly in a later `step_frames` call -- so those, and
            // `Return`/`ReturnUndef`, are checked as a pair via
            // `Frame::stack_effect_check` instead of a same-instruction
            // delta.
            #[cfg(test)]
            let (dbg_pop, dbg_push) = crate::bytecode::stack_effect(&op);
            #[cfg(test)]
            let dbg_is_call = matches!(
                op,
                Op::Call {
                    threaded: false,
                    ..
                } | Op::CallPtr {
                    threaded: false,
                    ..
                }
            );
            #[cfg(test)]
            let dbg_is_return = matches!(op, Op::Return | Op::ReturnUndef);
            #[cfg(test)]
            let dbg_stack_before = frames[top].stack.len() as i32;
            #[cfg(test)]
            let dbg_op_repr = format!("{op:?}");

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
                        Value::Entity(id) => {
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                            };
                            host.get_field(&mut cx, id, name)
                        }
                        // The only field an array or a string has. `_load.gsc` loops on it.
                        Value::Array(id) if name == self.size_atom => {
                            Value::Int(self.heap.array_len(id) as i32)
                        }
                        Value::String(a) if name == self.size_atom => {
                            Value::Int(self.interner.resolve(a).len() as i32)
                        }
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
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                            };
                            host.set_field(&mut cx, id, name, v).map_err(err)?
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
                // Retail auto-vivifies an `Undefined` index-assignment target
                // into a new empty array (measured,
                // tests/fixtures/semantics/retail-captures.txt `# probe_autoviv`);
                // `_load.gsc`'s `add_to_array` ships on every stock map
                // relying on exactly this for an uninitialised `level` field.
                Op::EnsureArrayLocal(slot) => {
                    let id = match frames[top].locals[slot as usize] {
                        Value::Array(id) => id,
                        Value::Undefined => {
                            let id = self.heap.new_array();
                            frames[top].locals[slot as usize] = Value::Array(id);
                            id
                        }
                        _ => return Err(err(ErrorKind::BadType("indexing needs an array"))),
                    };
                    push!(Value::Array(id));
                }
                Op::EnsureArrayField(name) => {
                    let obj = pop!();
                    let id = match obj {
                        Value::Struct(sid) => match self.heap.get_field(sid, name) {
                            Value::Array(id) => id,
                            Value::Undefined => {
                                let id = self.heap.new_array();
                                self.heap.set_field(sid, name, Value::Array(id));
                                id
                            }
                            _ => return Err(err(ErrorKind::BadType("indexing needs an array"))),
                        },
                        Value::Entity(eid) => {
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                            };
                            match host.get_field(&mut cx, eid, name) {
                                Value::Array(id) => id,
                                Value::Undefined => {
                                    let id = self.heap.new_array();
                                    let mut cx = Cx {
                                        interner: &mut self.interner,
                                        heap: &mut self.heap,
                                        level: self.level,
                                        game: self.game,
                                        notifies,
                                    };
                                    host.set_field(&mut cx, eid, name, Value::Array(id))
                                        .map_err(err)?;
                                    id
                                }
                                _ => {
                                    return Err(err(ErrorKind::BadType("indexing needs an array")))
                                }
                            }
                        }
                        _ => {
                            return Err(err(ErrorKind::BadType(
                                "field assignment needs a struct or entity",
                            )))
                        }
                    };
                    push!(Value::Array(id));
                }
                Op::EnsureArrayIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let Value::Array(outer) = obj else {
                        return Err(err(ErrorKind::BadType("indexing needs an array")));
                    };
                    let k = array_key(key).ok_or_else(|| {
                        err(ErrorKind::BadType("array key must be a string or a number"))
                    })?;
                    let id = match self.heap.get_index(outer, k) {
                        Value::Array(id) => id,
                        Value::Undefined => {
                            let id = self.heap.new_array();
                            self.heap.set_index(outer, k, Value::Array(id));
                            id
                        }
                        _ => return Err(err(ErrorKind::BadType("indexing needs an array"))),
                    };
                    push!(Value::Array(id));
                }
                Op::LoadSelf => {
                    let v = frames[top]
                        .recv
                        .map(target_to_value)
                        .unwrap_or(Value::Undefined);
                    push!(v);
                }
                Op::LoadLevel => push!(Value::Struct(self.level)),
                // `game` is array-typed on retail, measured
                // tests/fixtures/semantics/retail-captures.txt `# probe_game`.
                Op::LoadGame => push!(Value::Array(self.game)),
                // No stock MP script reads a bare `anim` back (every use is
                // inside a `/# #/` developer block, which the lexer drops
                // whole); `level` is the only preallocated struct (`game`
                // preallocates too, but as an array, not a struct).
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
                    threaded,
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
                    if threaded {
                        self.spawn(host, target, callee, recv, args, errors);
                        push!(Value::Undefined);
                    } else {
                        frames.push(self.make_frame(target, callee, recv, args));
                    }
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
                    let mut cx = Cx {
                        interner: &mut self.interner,
                        heap: &mut self.heap,
                        level: self.level,
                        game: self.game,
                        notifies,
                    };
                    let v = host.builtin(&mut cx, name, recv, &args).map_err(err)?;
                    push!(v);
                }
                Op::CallPtr {
                    argc,
                    has_recv,
                    threaded,
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
                    if threaded {
                        self.spawn(host, target, callee, recv, args, errors);
                        push!(Value::Undefined);
                    } else {
                        frames.push(self.make_frame(target, callee, recv, args));
                    }
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
                    let eq = values_equal(&self.interner, a, b).map_err(err)?;
                    push!(Value::Int(eq as i32));
                }
                Op::Ne => {
                    let b = pop!();
                    let a = pop!();
                    let eq = values_equal(&self.interner, a, b).map_err(err)?;
                    push!(Value::Int(!eq as i32));
                }
                Op::Lt => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x < y).map_err(err)?);
                }
                Op::Gt => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x > y).map_err(err)?);
                }
                Op::Le => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x <= y).map_err(err)?);
                }
                Op::Ge => {
                    let b = pop!();
                    let a = pop!();
                    push!(numeric_cmp(a, b, |x, y| x >= y).map_err(err)?);
                }
                Op::Neg => {
                    let v = pop!();
                    push!(eval_neg(v).map_err(err)?);
                }
                Op::Not => {
                    let v = pop!();
                    push!(Value::Int(!v.as_bool().map_err(err)? as i32));
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
                    if !c.as_bool().map_err(err)? {
                        frames[top].ip = t;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let c = pop!();
                    if c.as_bool().map_err(err)? {
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
                    // Event names fold, string values do not, so the name a
                    // literal or a concatenation produced is folded here
                    // rather than at the interner (atom.rs).
                    let event = self.interner.fold_atom(event);
                    return Ok(Step::Suspend(Suspend::WaitTill {
                        target,
                        event,
                        binds,
                    }));
                }
                // `notify` is queued for the caller rather than resolved
                // here -- it must not wake another thread mid-instruction.
                // `endon` is resolved immediately below instead of queued,
                // unlike `notify`: it only ever adds a registration, never
                // runs anything, so there is no reentrancy risk, and
                // queuing it would reopen the visibility gap `step_thread`'s
                // doc comment describes (an endon registered earlier in a
                // step must be visible to a notify a nested spawn fires
                // later in that same step).
                Op::Notify { argc } => {
                    let mut args = Vec::with_capacity(argc as usize);
                    for _ in 0..argc {
                        args.push(pop!());
                    }
                    args.reverse();
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(event) = event else {
                        return Err(err(ErrorKind::BadType("notify needs a string event name")));
                    };
                    // Redundant, not load-bearing: every path that drains
                    // `notifies` goes through `Vm::notify`, which folds
                    // again. Kept so the queued atom matches what
                    // `waittill`/`endon` stored.
                    let event = self.interner.fold_atom(event);
                    let target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "notify needs an entity or struct receiver",
                        ))
                    })?;
                    notifies.push((target, event, args));
                }
                Op::EndOn => {
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(event) = event else {
                        return Err(err(ErrorKind::BadType("endon needs a string event name")));
                    };
                    let event = self.interner.fold_atom(event);
                    let target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "endon needs an entity or struct receiver",
                        ))
                    })?;
                    // No thread to register against under `call_now`
                    // (`thread_id` is `None` there); the registration is
                    // simply dropped.
                    if let Some(id) = thread_id {
                        if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                            self.threads[idx].endons.push((target, event));
                        }
                    }
                }
            }

            // Not reached for an op that returned out of `step_frames`
            // directly (`Wait`, `WaitTill`, or a `?`-propagated error) --
            // those never got this far, and there is nothing to check for
            // them here in any case (frames unchanged, and `pop!` already
            // panics on an over-declared pop).
            #[cfg(test)]
            if dbg_is_call {
                let caller = frames.len() - 2;
                frames[caller].stack_effect_check = Some(dbg_stack_before - dbg_pop + dbg_push);
            } else if dbg_is_return {
                if let Some(caller) = frames.last_mut() {
                    let expected = caller.stack_effect_check.take().unwrap_or_else(|| {
                        panic!(
                            "{dbg_op_repr} returned into a frame with no pending call expectation"
                        )
                    });
                    assert_eq!(
                        caller.stack.len() as i32,
                        expected,
                        "stack_effect disagreement across a call/return pair at {dbg_op_repr}"
                    );
                }
            } else {
                let actual = frames[top].stack.len() as i32;
                assert_eq!(
                    actual,
                    dbg_stack_before - dbg_pop + dbg_push,
                    "stack_effect disagreement at {dbg_op_repr} (before {dbg_stack_before}, pop {dbg_pop}, push {dbg_push})"
                );
            }
        }
    }
}
