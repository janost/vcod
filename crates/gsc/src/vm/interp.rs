//! The instruction loop: walks `bytecode::Op` over a frame stack until it
//! suspends, returns, or the caller-supplied budget runs out.
//!
//! The loop matches each op by reference and holds the top frame across
//! it; a call or return changes the frame stack after the op (`After`). A
//! call's arguments and a builtin's are read in place off the caller's
//! stack, and a returned frame's buffers go back to `Vm::frame_pool`. Two
//! numbers of one type take an inline arm in the arithmetic and compare ops
//! ahead of the general `eval_*` (docs/research/cod4-server-frame.md 9,
//! "The spawn wave"; `tests/bench.rs` is the microbench).

use std::rc::Rc;

use crate::atom::{Atom, Interner};
use crate::bytecode::{Function, Op};
use crate::heap::ArrayKey;
use crate::value::{FuncRef, Value, format_number};

use super::sched::ThreadId;
use super::{Cx, ErrorKind, Host, QueuedSpawn, ScriptError, Target, Vm};

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
    /// The operand stack's buffer: its first `sp` values are the stack,
    /// the rest stale values a returned frame's run left, overwritten as
    /// the stack grows back over them.
    pub stack: Vec<Value>,
    pub sp: u32,
    pub recv: Option<Target>,
    /// The operand stack's depth before the instruction now running, so a
    /// runtime error the VM recovers from (`Vm::set_recover_errors`) can
    /// put the stack back as that instruction's `stack_effect` leaves it.
    pub op_base: u32,
    /// Test-only: what this frame's `stack` length must be once the call
    /// currently running on top of it returns, per `stack_effect`'s
    /// declared `(pop, push)` for that `Call`/`CallPtr`. Lives on the frame,
    /// not a local in `step_frames`, because a call can suspend (`wait`)
    /// several layers deep and resume in a later `step_frames` invocation —
    /// this has to survive that.
    #[cfg(test)]
    stack_effect_check: Option<i32>,
}

impl Frame {
    /// Pushes onto the operand stack from outside the op loop: a call's
    /// value landing on its caller.
    pub(crate) fn push(&mut self, v: Value) {
        let sp = self.sp as usize;
        if sp < self.stack.len() {
            self.stack[sp] = v;
        } else {
            self.stack.push(v);
        }
        self.sp += 1;
    }
}

/// How [`Vm::run_ops`] left its frame.
enum Exit {
    /// A call's new frame, for the frame stack's top.
    Call(Frame),
    /// The frame returned this.
    Return(Value),
    Step(Step),
    /// The op at the frame's `ip - 1` failed.
    Err(ErrorKind),
}

/// The spawns a builtin queues (`Cx::spawn`), drained by `Op::CallBuiltin`
/// the moment it returns, and a field hook's queue, asserted empty at each
/// hook rather than routed anywhere: `Op::CallBuiltin` is the only drain
/// point, so a spawn from `get_field`/`set_field` has no defined moment to
/// run at.
#[derive(Default)]
struct Spawns {
    pending: Vec<QueuedSpawn>,
    field: Vec<QueuedSpawn>,
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
    /// `waittillframeend`.
    FrameEnd,
    WaitTill {
        target: Target,
        event: Atom,
        binds: Box<[u16]>,
        /// What the notify's first arguments must equal (`waittillmatch`);
        /// empty for `waittill`.
        matches: Box<[Value]>,
    },
}

/// The first `wait` retail refuses as too long (`cod_lnxded` 0x80d7fe4): a
/// longer one would overrun the 24-bit script clock.
const MAX_WAIT_SECONDS: f32 = 16777.0;

/// The most returned frames' buffers [`Vm::recycle_frame`] keeps.
const FRAME_POOL_MAX: usize = 64;

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
pub(crate) fn values_equal(interner: &Interner, a: Value, b: Value) -> Result<bool, ErrorKind> {
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

/// CoD4's `*` (`div` false) and `/`: a vector on either side casts the
/// other side up to one and the pair goes per component, a zero divisor
/// component an error; numbers as CoD2's (`MulNumber`, `DivFloat`).
fn eval_vector_op(a: Value, b: Value, div: bool) -> Result<Value, ErrorKind> {
    let as_vec = |v: Value| match v {
        Value::Vector(v) => Some(v),
        n => to_f32(n).map(|f| [f; 3]),
    };
    if matches!(a, Value::Vector(_)) || matches!(b, Value::Vector(_)) {
        let (Some(x), Some(y)) = (as_vec(a), as_vec(b)) else {
            return Err(ErrorKind::BadType("pair has unmatching types"));
        };
        if div {
            if y.contains(&0.0) {
                return Err(ErrorKind::BadType("divide by 0"));
            }
            return Ok(Value::Vector([x[0] / y[0], x[1] / y[1], x[2] / y[2]]));
        }
        return Ok(Value::Vector([x[0] * y[0], x[1] * y[1], x[2] * y[2]]));
    }
    if !div {
        return eval_mul(a, b);
    }
    let (a, b) = match (a, b) {
        (Value::Int(x), Value::Int(y)) => (Value::Float(x as f32), Value::Float(y as f32)),
        ab => ab,
    };
    if matches!(b, Value::Float(y) if y == 0.0) {
        return Err(ErrorKind::BadType("division by zero"));
    }
    eval_div(a, b)
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

fn eval_bitxor(a: Value, b: Value) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x ^ y)),
        _ => Err(ErrorKind::BadType("^ needs integers")),
    }
}

/// `<<` and `>>` on ints; the count wraps as x86's shift does.
fn eval_shift(a: Value, b: Value, left: bool) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) if left => Ok(Value::Int(x.wrapping_shl(y as u32))),
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_shr(y as u32))),
        _ => Err(ErrorKind::BadType("<< and >> need integers")),
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

/// Retail's unary `!` and a condition both coerce a string numerically:
/// `!"1"` is false and `!"0"` true, `if ("1")` takes the branch and
/// `if ("0")` does not, while `""` and `"a"` are the fatal `cannot cast
/// ... to bool` (`# probe_not_string`, `# probe_not_empty_string`,
/// `# probe_truthy`, `# probe_truthy_empty`, `# probe_truthy_numstr`,
/// tests/fixtures/semantics/retail-captures.txt). UO's stock `tdm.gsc`
/// tests `if (getCvar("scr_teamscorepenalty"))` on every suicide.
///
/// `Localized` (a `&"KEY"` literal) folds into the same arm as `String`,
/// as it does everywhere else in this crate (`Value::as_bool`'s own error
/// arm, `format_number`): not measured against retail either way (no probe
/// or stock script negates one), but excluding it would be a gap against
/// the rest of the file's own convention, not a considered divergence.
fn not_of(v: Value, interner: &Interner) -> Result<bool, ErrorKind> {
    match v {
        Value::String(s) | Value::Localized(s) => {
            let text = interner.resolve(s);
            let n = string_int(text)
                .ok_or_else(|| ErrorKind::Custom(format!("cannot cast \"{text}\" to bool")))?;
            Ok(n == 0)
        }
        other => other.as_bool().map(|b| !b),
    }
}

/// A string read as a number by the engine's casts to bool, int and float
/// (cod_lnxded 1.1d 0x080a3c30, 0x080a3de0, 0x080a3fa0; coduo_lnxded
/// 0x080aa9a0, 0x080aaae2, 0x080aac34): `strtol(s, 0, 10)` or `strtod`, its longest
/// numeric prefix ("12abc" is 12, " -7" is -7). A result of 0 stands only
/// when the first non-blank character, past a sign, is `0` or `.0`;
/// otherwise the cast fails (`# probe_cast_string`,
/// tests/fixtures/semantics/retail-captures.txt).
fn string_int(text: &str) -> Option<i32> {
    let (len, neg) = numeric_prefix(text, false);
    let digits = text[..len].trim_start_matches(|c: char| c.is_ascii_whitespace());
    let digits = digits.trim_start_matches(['+', '-']);
    // strtol saturates at a 32-bit long.
    let n = digits.bytes().fold(0i64, |n, d| {
        (n * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX) + 1)
    });
    let n = if neg { -n } else { n };
    let n = n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    (n != 0 || zero_spelled(text)).then_some(n)
}

/// [`string_int`]'s `strtod` arm.
fn string_float(text: &str) -> Option<f32> {
    let (len, _) = numeric_prefix(text, true);
    let f = text[..len].trim_start().parse::<f64>().unwrap_or(0.0) as f32;
    (f != 0.0 || zero_spelled(text)).then_some(f)
}

/// The byte length of `text`'s blanks plus its longest decimal number (an
/// integer, or with `float` a fraction and an exponent too), and its sign.
fn numeric_prefix(text: &str, float: bool) -> (usize, bool) {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || (b'\t'..=b'\r').contains(&b[i])) {
        i += 1;
    }
    let start = i;
    let neg = b.get(i) == Some(&b'-');
    if matches!(b.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let from = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > from
    };
    let mut any = digits(&mut i);
    if float {
        if b.get(i) == Some(&b'.') {
            let mut j = i + 1;
            if digits(&mut j) || any {
                any = true;
                i = j;
            }
        }
        if any && matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if digits(&mut j) {
                i = j;
            }
        }
    }
    if any { (i, neg) } else { (start, false) }
}

/// The engine's test that a cast's 0 was meant: the first character past
/// the blanks (anything under `!`) and a sign is `0`, or `.` then `0`.
fn zero_spelled(text: &str) -> bool {
    let t = text.trim_start_matches(|c: char| c < '!');
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    t.starts_with('0') || t.starts_with(".0")
}

/// Retail's two texts for a handle whose object was freed
/// (tests/fixtures/semantics/retail-captures.txt, `# probe_stale_*`): a
/// field read or write says the first, a method call the second.
pub(crate) const DEAD_OBJECT: ErrorKind = ErrorKind::BadType("dead entity is not an object");
pub(crate) const DEAD_ENTITY: ErrorKind = ErrorKind::BadType("dead entity is not an entity");

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
    /// Every argument array is shared with the caller's copy
    /// ([`crate::heap::Heap::share`]).
    pub(crate) fn make_frame(
        &mut self,
        func: FuncRef,
        func_rc: Rc<Function>,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> Frame {
        self.make_frame_from(func, func_rc, recv, &args)
    }

    /// [`Self::make_frame`] off a slice: a call's arguments read in place
    /// on the caller's stack. The locals and the operand stack reuse a
    /// returned frame's buffers ([`Vm::recycle_frame`]).
    pub(crate) fn make_frame_from(
        &mut self,
        func: FuncRef,
        func_rc: Rc<Function>,
        recv: Option<Target>,
        args: &[Value],
    ) -> Frame {
        for a in args {
            self.heap.share(*a);
        }
        let (mut locals, stack) = self.frame_pool.pop().unwrap_or_default();
        locals.resize(func_rc.locals as usize, Value::Undefined);
        let take = args.len().min(func_rc.params as usize);
        locals[..take].copy_from_slice(&args[..take]);
        Frame {
            func,
            func_rc,
            ip: 0,
            locals,
            stack,
            sp: 0,
            recv,
            op_base: 0,
            #[cfg(test)]
            stack_effect_check: None,
        }
    }

    /// A returned frame's buffers, emptied, for the next call's
    /// [`Self::make_frame_from`]. The pool keeps as many as the deepest
    /// call stack it has seen unwind, up to [`FRAME_POOL_MAX`].
    fn recycle_frame(&mut self, f: Frame) {
        if self.frame_pool.len() < FRAME_POOL_MAX {
            let mut locals = f.locals;
            locals.clear();
            self.frame_pool.push((locals, f.stack));
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
    /// this call returns. `Op::Notify` resolves on the spot too, as
    /// retail's `VM_Notify` does (`probe_endon_late`); `notifies` collects
    /// the notifies builtins queue through `Cx::notify`, for the caller to
    /// apply once this call returns. `errors` collects any `ScriptError` raised by a
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
            match self.step_ops(host, frames, &mut remaining, thread_id, notifies, errors) {
                Err(e) if self.recover_errors && e.kind.recoverable() => {
                    // Retail below `developer 1` (`Scr_Init`'s flag,
                    // coduo_lnxded 0x80aa818) leaves the failed
                    // instruction's results undefined, a conditional jump
                    // not taken, and runs on (cod11-gsc-language.md,
                    // "Runtime errors below developer 1").
                    self.record_error(&e, true);
                    errors.push(e);
                    let f = frames.last_mut().expect("an error leaves its frame on top");
                    let (pops, pushes) = crate::bytecode::stack_effect(
                        &f.func_rc.code[(f.ip as usize).saturating_sub(1)],
                    );
                    f.sp = f.sp.min((f.op_base as i32 - pops).max(0) as u32);
                    for _ in 0..pushes {
                        f.push(Value::Undefined);
                    }
                }
                r => return r,
            }
        }
    }

    /// `step_frames` without the recovery: the first error ends the call.
    /// Runs the top frame's ops in [`Self::run_ops`] and makes the frame
    /// stack's change a call or return leaves behind.
    fn step_ops(
        &mut self,
        host: &mut dyn Host,
        frames: &mut Vec<Frame>,
        remaining: &mut u32,
        thread_id: Option<ThreadId>,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        errors: &mut Vec<ScriptError>,
    ) -> Result<Step, ScriptError> {
        // CoD2: ended by a nested spawn's notify (`Vm::set_kill_mid_step`);
        // the thread's entry is gone, so its frames are dropped.
        if self.was_killed(thread_id) {
            return Ok(Step::Returned(Value::Undefined));
        }
        // What a builtin queued through `Cx::spawn`, drained by
        // `Op::CallBuiltin` the moment the builtin returns, and a field
        // hook's own queue (see `run_ops`).
        let mut spawns = Spawns::default();
        loop {
            let depth = frames.len();
            let fr = frames.last_mut().expect("a frame is running");
            let exit = self.run_ops(
                host,
                fr,
                depth,
                remaining,
                thread_id,
                notifies,
                errors,
                &mut spawns,
            );
            match exit {
                Exit::Call(frame) => frames.push(frame),
                Exit::Return(v) => {
                    let done = frames.pop().expect("a frame is running");
                    self.recycle_frame(done);
                    self.end_call_endons(thread_id, frames.len());
                    let Some(caller) = frames.last_mut() else {
                        return Ok(Step::Returned(v));
                    };
                    caller.push(v);
                    #[cfg(test)]
                    {
                        let expected = caller.stack_effect_check.take().unwrap_or_else(|| {
                            panic!("a return into a frame with no pending call expectation")
                        });
                        assert_eq!(
                            caller.sp as i32, expected,
                            "stack_effect disagreement across a call/return pair"
                        );
                    }
                }
                Exit::Step(step) => return Ok(step),
                Exit::Err(kind) => {
                    let f = frames.last().expect("an error leaves its frame on top");
                    return Err(ScriptError {
                        file: f.func_rc.file,
                        func: f.func_rc.name,
                        line: f.func_rc.lines[(f.ip as usize).saturating_sub(1)],
                        kind,
                    });
                }
            }
        }
    }

    /// Whether `thread_id` was ended by a thread a spawn ran: only a spawn
    /// runs another thread, so only after one can this one have ended.
    fn was_killed(&self, thread_id: Option<ThreadId>) -> bool {
        !self.killed.is_empty() && thread_id.is_some_and(|id| self.killed.contains(&id))
    }

    /// Runs `fr`'s ops until one calls, returns, suspends or fails, or the
    /// budget runs out. The instruction pointer, the operand stack's depth
    /// and the budget live in locals here and go back to `fr` and
    /// `remaining` on the way out. `depth` is the frame stack's depth with
    /// `fr` on top.
    #[allow(clippy::too_many_arguments)]
    fn run_ops(
        &mut self,
        host: &mut dyn Host,
        fr: &mut Frame,
        depth: usize,
        remaining: &mut u32,
        thread_id: Option<ThreadId>,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        errors: &mut Vec<ScriptError>,
        spawns: &mut Spawns,
    ) -> Exit {
        let Frame {
            func_rc,
            ip: frame_ip,
            locals,
            stack,
            sp: frame_sp,
            recv: frame_recv,
            op_base,
            #[cfg(test)]
            stack_effect_check,
            ..
        } = fr;
        let func: &Function = func_rc;
        let code = &func.code[..];
        // What runs: `code` with its runs fused (`bytecode::fuse`).
        let fast = if func.fast.len() == code.len() {
            &func.fast[..]
        } else {
            code
        };
        let locals = &mut locals[..];
        let self_recv = *frame_recv;
        let mut ip = *frame_ip as usize;
        let mut sp = *frame_sp as usize;
        let mut left = *remaining;
        // The depth before the op now running.
        let mut base_sp = sp;
        let Spawns {
            pending: pending_spawns,
            field: field_spawns,
        } = spawns;

        macro_rules! pop {
            () => {{
                sp -= 1;
                stack[sp]
            }};
        }
        macro_rules! push {
            ($v:expr) => {{
                let v = $v;
                if sp < stack.len() {
                    stack[sp] = v;
                } else {
                    stack.push(v);
                }
                sp += 1;
            }};
        }
        // Ends the run with an error raised by the op now running.
        macro_rules! fail {
            ($kind:expr) => {
                break Exit::Err($kind)
            };
        }
        macro_rules! tri {
            ($e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(kind) => fail!(kind),
                }
            };
        }

        // `LoadField`'s answer for a value it reads without failing:
        // `None` for a dead entity or a value with no such field.
        macro_rules! field_of {
            ($obj:expr, $name:expr) => {{
                let name = $name;
                match $obj {
                    Value::Struct(_) | Value::Entity(_) if name == self.size_atom => {
                        Some(Value::Int(1))
                    }
                    Value::Struct(id) => Some(self.heap.get_field(id, name)),
                    Value::Entity(id) if host.is_live(id) => {
                        let mut cx = Cx {
                            interner: &mut self.interner,
                            heap: &mut self.heap,
                            level: self.level,
                            game: self.game,
                            notifies,
                            spawns: field_spawns,
                        };
                        let v = host.get_field(&mut cx, id, name);
                        debug_assert!(
                            field_spawns.is_empty(),
                            "Cx::spawn is only honoured from a builtin"
                        );
                        Some(v)
                    }
                    Value::Array(id) if name == self.size_atom => {
                        Some(Value::Int(self.heap.array_len(id) as i32))
                    }
                    Value::String(a) if name == self.size_atom => {
                        Some(Value::Int(self.interner.resolve(a).len() as i32))
                    }
                    _ => None,
                }
            }};
        }
        // A fused compare and `JumpIfFalse`: `Lt`'s two floats inline, any
        // other pair through `numeric_cmp`, as the compare ops do.
        macro_rules! cmp_jump {
            ($to:expr, $cmp:tt) => {{
                let b = pop!();
                let a = pop!();
                let r = match (a, b) {
                    (Value::Float(x), Value::Float(y)) => x $cmp y,
                    _ => tri!(numeric_cmp(a, b, |x, y| x $cmp y)) == Value::Int(1),
                };
                if left > 0 {
                    left -= 1;
                    ip = if r { ip + 1 } else { $to as usize };
                } else {
                    push!(Value::Int(r as i32));
                }
            }};
        }

        let exit = loop {
            if left == 0 {
                break Exit::Step(Step::Running);
            }
            left -= 1;
            let at = ip;
            let op = &fast[at];
            ip += 1;
            base_sp = sp;
            // A spawn ran another thread, which may have ended this one.
            let mut spawned = false;

            // Test-only: `stack_effect`'s table checked against what this
            // op does to the operand stack. A non-threaded `Call`/`CallPtr`
            // pushes its value onto this frame only once the callee's
            // `Return` runs, so that pair is checked through
            // `Frame::stack_effect_check` instead (`step_ops`).
            #[cfg(test)]
            let (dbg_pop, dbg_push) = crate::bytecode::stack_effect(&code[at]);
            #[cfg(test)]
            let dbg_op_repr = format!("{op:?}");
            #[cfg(test)]
            let dbg_left = left;

            match *op {
                Op::Const(idx) => push!(func.consts[idx as usize]),
                Op::LoadLocal(slot) => push!(locals[slot as usize]),
                Op::StoreLocal(slot) => {
                    let v = pop!();
                    self.heap.share(v);
                    locals[slot as usize] = v;
                }
                Op::LoadField(name) => {
                    let obj = pop!();
                    let v = match obj {
                        // `.size` of any object but an array is 1 on retail
                        // (cod_lnxded 0x080a2f28; cod11-gsc-language.md §9).
                        Value::Struct(_) | Value::Entity(_) if name == self.size_atom => {
                            Value::Int(1)
                        }
                        Value::Struct(id) => self.heap.get_field(id, name),
                        Value::Entity(id) => {
                            if !host.is_live(id) {
                                fail!(DEAD_OBJECT);
                            }
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                                spawns: field_spawns,
                            };
                            let v = host.get_field(&mut cx, id, name);
                            debug_assert!(
                                field_spawns.is_empty(),
                                "Cx::spawn is only honoured from a builtin"
                            );
                            v
                        }
                        // An array's key count, a string's length. `_load.gsc` loops on it.
                        Value::Array(id) if name == self.size_atom => {
                            Value::Int(self.heap.array_len(id) as i32)
                        }
                        Value::String(a) if name == self.size_atom => {
                            Value::Int(self.interner.resolve(a).len() as i32)
                        }
                        _ => fail!(ErrorKind::BadType("field access needs a struct or entity")),
                    };
                    push!(v);
                }
                Op::StoreField(name) => {
                    let v = pop!();
                    let obj = pop!();
                    self.heap.share(v);
                    match obj {
                        Value::Struct(id) => self.heap.set_field(id, name, v),
                        Value::Entity(id) => {
                            if !host.is_live(id) {
                                fail!(DEAD_OBJECT);
                            }
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                                spawns: field_spawns,
                            };
                            tri!(host.store_field(&mut cx, id, name, v));
                            debug_assert!(
                                field_spawns.is_empty(),
                                "Cx::spawn is only honoured from a builtin"
                            );
                        }
                        _ => fail!(ErrorKind::BadType(
                            "field assignment needs a struct or entity"
                        )),
                    }
                }
                Op::LoadIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let v = match obj {
                        Value::Array(id) => {
                            let Some(k) = array_key(key) else {
                                fail!(ErrorKind::BadType("array key must be a string or a number"));
                            };
                            self.heap.get_index(id, k)
                        }
                        // A vector reads its components under 0..2 as
                        // floats: `_utility::vectorScale` does so on every
                        // stock S&D plant. What retail does with any other
                        // key is unmeasured, so it is refused rather than
                        // guessed.
                        Value::Vector(v) => {
                            let Value::Int(i @ 0..=2) = key else {
                                fail!(ErrorKind::BadType("vector index must be 0, 1 or 2"));
                            };
                            Value::Float(v[i as usize])
                        }
                        // A string reads one character under an int key;
                        // past the end is fatal (`probe_string_index`, 1.1d
                        // and UO). UO's `_gameobjects::main` splits a name
                        // this way.
                        Value::String(a) => {
                            let Value::Int(i) = key else {
                                fail!(ErrorKind::BadType("string index must be an int"));
                            };
                            let s = self.interner.resolve(a);
                            let Some(&b) =
                                usize::try_from(i).ok().and_then(|i| s.as_bytes().get(i))
                            else {
                                fail!(ErrorKind::BadType("string index out of range"));
                            };
                            let c = (b as char).to_string();
                            Value::String(self.interner.intern_exact(&c))
                        }
                        _ => fail!(ErrorKind::BadType("indexing needs an array")),
                    };
                    push!(v);
                }
                Op::StoreIndex => {
                    let v = pop!();
                    let key = pop!();
                    let obj = pop!();
                    self.heap.share(v);
                    let Value::Array(id) = obj else {
                        fail!(ErrorKind::BadType("indexing needs an array"));
                    };
                    let Some(k) = array_key(key) else {
                        fail!(ErrorKind::BadType("array key must be a string or a number"));
                    };
                    self.heap.set_index(id, k, v);
                }
                Op::OwnLocal(slot) => {
                    if let Value::Array(id) = locals[slot as usize] {
                        let own = self.heap.own(id);
                        locals[slot as usize] = Value::Array(own);
                    }
                    push!(locals[slot as usize]);
                }
                Op::ClearIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let Value::Array(id) = obj else {
                        fail!(ErrorKind::BadType("indexing needs an array"));
                    };
                    let Some(k) = array_key(key) else {
                        fail!(ErrorKind::BadType("array key must be a string or a number"));
                    };
                    self.heap.clear_index(id, k);
                }
                // Retail auto-vivifies an `Undefined` index-assignment target
                // into a new empty array (measured,
                // tests/fixtures/semantics/retail-captures.txt `# probe_autoviv`);
                // `_load.gsc`'s `add_to_array` ships on every stock map
                // relying on exactly this for an uninitialised `level` field.
                // Each also copies a shared array before the write
                // (`Heap::own`) and stores the copy back where it was read.
                Op::EnsureArrayLocal(slot) => {
                    let id = match locals[slot as usize] {
                        Value::Array(id) => {
                            let own = self.heap.own(id);
                            locals[slot as usize] = Value::Array(own);
                            own
                        }
                        Value::Undefined => {
                            let id = self.heap.new_array();
                            locals[slot as usize] = Value::Array(id);
                            id
                        }
                        _ => fail!(ErrorKind::BadType("indexing needs an array")),
                    };
                    push!(Value::Array(id));
                }
                Op::EnsureArrayField(name) => {
                    let obj = pop!();
                    let id = match obj {
                        Value::Struct(sid) => match self.heap.get_field(sid, name) {
                            Value::Array(id) => {
                                let own = self.heap.own(id);
                                if own != id {
                                    self.heap.set_field(sid, name, Value::Array(own));
                                }
                                own
                            }
                            Value::Undefined => {
                                let id = self.heap.new_array();
                                self.heap.set_field(sid, name, Value::Array(id));
                                id
                            }
                            _ => fail!(ErrorKind::BadType("indexing needs an array")),
                        },
                        Value::Entity(eid) => {
                            if !host.is_live(eid) {
                                fail!(DEAD_OBJECT);
                            }
                            let mut cx = Cx {
                                interner: &mut self.interner,
                                heap: &mut self.heap,
                                level: self.level,
                                game: self.game,
                                notifies,
                                spawns: field_spawns,
                            };
                            let cur = host.get_field(&mut cx, eid, name);
                            debug_assert!(
                                field_spawns.is_empty(),
                                "Cx::spawn is only honoured from a builtin"
                            );
                            match cur {
                                Value::Array(id) => {
                                    let own = self.heap.own(id);
                                    if own != id {
                                        let mut cx = Cx {
                                            interner: &mut self.interner,
                                            heap: &mut self.heap,
                                            level: self.level,
                                            game: self.game,
                                            notifies,
                                            spawns: field_spawns,
                                        };
                                        tri!(host.store_field(
                                            &mut cx,
                                            eid,
                                            name,
                                            Value::Array(own)
                                        ));
                                    }
                                    own
                                }
                                Value::Undefined => {
                                    let id = self.heap.new_array();
                                    let mut cx = Cx {
                                        interner: &mut self.interner,
                                        heap: &mut self.heap,
                                        level: self.level,
                                        game: self.game,
                                        notifies,
                                        spawns: field_spawns,
                                    };
                                    tri!(host.store_field(&mut cx, eid, name, Value::Array(id)));
                                    debug_assert!(
                                        field_spawns.is_empty(),
                                        "Cx::spawn is only honoured from a builtin"
                                    );
                                    id
                                }
                                _ => fail!(ErrorKind::BadType("indexing needs an array")),
                            }
                        }
                        _ => fail!(ErrorKind::BadType(
                            "field assignment needs a struct or entity"
                        )),
                    };
                    push!(Value::Array(id));
                }
                Op::EnsureArrayIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let Value::Array(outer) = obj else {
                        fail!(ErrorKind::BadType("indexing needs an array"));
                    };
                    let Some(k) = array_key(key) else {
                        fail!(ErrorKind::BadType("array key must be a string or a number"));
                    };
                    let id = match self.heap.get_index(outer, k) {
                        Value::Array(id) => {
                            let own = self.heap.own(id);
                            if own != id {
                                self.heap.set_index(outer, k, Value::Array(own));
                            }
                            own
                        }
                        Value::Undefined => {
                            let id = self.heap.new_array();
                            self.heap.set_index(outer, k, Value::Array(id));
                            id
                        }
                        _ => fail!(ErrorKind::BadType("indexing needs an array")),
                    };
                    push!(Value::Array(id));
                }
                Op::LoadSelf => {
                    push!(self_recv.map(target_to_value).unwrap_or(Value::Undefined))
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
                    let (Some(x), Some(y), Some(z)) = (to_f32(x), to_f32(y), to_f32(z)) else {
                        fail!(ErrorKind::BadType("vector component must be a number"));
                    };
                    push!(Value::Vector([x, y, z]));
                }
                Op::Call {
                    func: target,
                    argc,
                    has_recv,
                    threaded,
                } => {
                    let Some(callee) = self.functions.get(&target).cloned() else {
                        fail!(self.no_such_function("no such function", target));
                    };
                    // The arguments are the stack's top `argc` values, the
                    // receiver under them. A receiver-less call inherits the
                    // caller's `self`, which is what carries the client
                    // entity from `CodeCallback_PlayerConnect` into the
                    // gametype's own callback (probe_self).
                    let base = sp - argc as usize;
                    let recv = if has_recv {
                        as_target(stack[base - 1])
                    } else {
                        self_recv
                    };
                    if threaded {
                        let args = stack[base..sp].to_vec();
                        sp = base - usize::from(has_recv);
                        self.spawn(host, target, callee, recv, args, errors);
                        spawned = true;
                        let v = self.thread_returned.take().filter(|_| self.thread_returns);
                        push!(v.unwrap_or(Value::Undefined));
                    } else {
                        let frame = self.make_frame_from(target, callee, recv, &stack[base..sp]);
                        sp = base - usize::from(has_recv);
                        #[cfg(test)]
                        {
                            *stack_effect_check = Some(base_sp as i32 - dbg_pop + dbg_push);
                        }
                        break Exit::Call(frame);
                    }
                }
                Op::CallBuiltin {
                    name,
                    argc,
                    has_recv,
                } => {
                    // The builtin reads its arguments in place, off the
                    // stack's top `argc` values, the receiver under them.
                    let base = sp - argc as usize;
                    let recv = if has_recv {
                        as_target(stack[base - 1])
                    } else {
                        None
                    };
                    if let Some(Target::Entity(id)) = recv
                        && !host.is_live(id)
                    {
                        fail!(DEAD_ENTITY);
                    }
                    let mut cx = Cx {
                        interner: &mut self.interner,
                        heap: &mut self.heap,
                        level: self.level,
                        game: self.game,
                        notifies,
                        spawns: pending_spawns,
                    };
                    let v = tri!(host.builtin(&mut cx, name, recv, &stack[base..sp]));
                    sp = base - usize::from(has_recv);
                    // Retail's `Scr_ExecEntThread` from inside a builtin:
                    // each queued thread runs to its first suspend here,
                    // before the calling thread's next instruction (see
                    // `Cx::spawn`).
                    if !pending_spawns.is_empty() {
                        let mut queue =
                            std::collections::VecDeque::from(std::mem::take(&mut *pending_spawns));
                        while let Some(queued) = queue.pop_front() {
                            let target = queued.func;
                            match self.functions.get(&target).cloned() {
                                Some(f) => {
                                    self.spawn(host, target, f, queued.recv, queued.args, errors);
                                    spawned = true;
                                }
                                None => {
                                    let e = ScriptError {
                                        file: func.file,
                                        func: func.name,
                                        line: func.lines[at],
                                        kind: self
                                            .no_such_function("spawn of unknown function", target),
                                    };
                                    self.record_abort(&e);
                                    errors.push(e);
                                }
                            }
                            if let Some(token) = queued.then {
                                let mut cx = Cx {
                                    interner: &mut self.interner,
                                    heap: &mut self.heap,
                                    level: self.level,
                                    game: self.game,
                                    notifies,
                                    spawns: field_spawns,
                                };
                                host.spawn_returned(&mut cx, token);
                                // The engine code after one
                                // `Scr_ExecEntThread` may start the next:
                                // those threads go ahead of the rest of the
                                // builtin's queue.
                                for next in field_spawns.drain(..).rev() {
                                    queue.push_front(next);
                                }
                            }
                        }
                    }
                    push!(v);
                }
                Op::CallPtr {
                    argc,
                    has_recv,
                    threaded,
                } => {
                    let ptr = pop!();
                    let Value::Function(target) = ptr else {
                        fail!(ErrorKind::BadType("call target is not a function pointer"));
                    };
                    let base = sp - argc as usize;
                    sp = base;
                    // Same inheritance as `Op::Call`; `[[f]]()` is how
                    // `_callbacksetup` reaches every gametype callback.
                    let recv = if has_recv {
                        as_target(pop!())
                    } else {
                        self_recv
                    };
                    let Some(callee) = self.functions.get(&target).cloned() else {
                        fail!(self.no_such_function("no such function", target));
                    };
                    let args = &stack[base..base + argc as usize];
                    if threaded {
                        self.spawn(host, target, callee, recv, args.to_vec(), errors);
                        spawned = true;
                        let v = self.thread_returned.take().filter(|_| self.thread_returns);
                        push!(v.unwrap_or(Value::Undefined));
                    } else {
                        let frame = self.make_frame_from(target, callee, recv, args);
                        #[cfg(test)]
                        {
                            *stack_effect_check = Some(base_sp as i32 - dbg_pop + dbg_push);
                        }
                        break Exit::Call(frame);
                    }
                }
                // Two numbers inline, as `eval_*` answers them; any other
                // pair through `eval_*`.
                Op::Add => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(y)),
                        (Value::Float(x), Value::Float(y)) => Value::Float(x + y),
                        _ => tri!(eval_add(&mut self.interner, a, b)),
                    };
                    push!(v);
                }
                Op::Sub => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_sub(y)),
                        (Value::Float(x), Value::Float(y)) => Value::Float(x - y),
                        _ => tri!(eval_sub(a, b)),
                    };
                    push!(v);
                }
                Op::Mul => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_mul(y)),
                        (Value::Float(x), Value::Float(y)) => Value::Float(x * y),
                        _ => tri!(eval_mul(a, b)),
                    };
                    push!(v);
                }
                Op::Div => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_div(a, b)));
                }
                Op::MulNumber => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Float(x * y),
                        (Value::Vector(_), _) | (_, Value::Vector(_)) => {
                            fail!(ErrorKind::BadType("pair has unmatching types"));
                        }
                        _ => tri!(eval_mul(a, b)),
                    };
                    push!(v);
                }
                Op::MulVector => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_vector_op(a, b, false)));
                }
                Op::DivVector => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_vector_op(a, b, true)));
                }
                Op::DivFloat => {
                    let b = pop!();
                    let a = pop!();
                    let (a, b) = match (a, b) {
                        (Value::Int(x), Value::Int(y)) => {
                            (Value::Float(x as f32), Value::Float(y as f32))
                        }
                        ab => ab,
                    };
                    let v = match (a, b) {
                        (_, Value::Float(0.0)) => fail!(ErrorKind::BadType("division by zero")),
                        (Value::Float(x), Value::Float(y)) => Value::Float(x / y),
                        _ => tri!(eval_div(a, b)),
                    };
                    push!(v);
                }
                Op::Mod => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_mod(a, b)));
                }
                Op::BitAnd => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_bitand(a, b)));
                }
                Op::BitOr => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_bitor(a, b)));
                }
                Op::BitXor => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_bitxor(a, b)));
                }
                Op::Shl | Op::Shr => {
                    let b = pop!();
                    let a = pop!();
                    push!(tri!(eval_shift(a, b, matches!(op, Op::Shl))));
                }
                Op::Eq => {
                    let b = pop!();
                    let a = pop!();
                    let eq = tri!(values_equal(&self.interner, a, b));
                    push!(Value::Int(eq as i32));
                }
                Op::Ne => {
                    let b = pop!();
                    let a = pop!();
                    let eq = tri!(values_equal(&self.interner, a, b));
                    push!(Value::Int(!eq as i32));
                }
                Op::Lt => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x < y) as i32),
                        _ => tri!(numeric_cmp(a, b, |x, y| x < y)),
                    };
                    push!(v);
                }
                Op::Gt => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x > y) as i32),
                        _ => tri!(numeric_cmp(a, b, |x, y| x > y)),
                    };
                    push!(v);
                }
                Op::Le => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x <= y) as i32),
                        _ => tri!(numeric_cmp(a, b, |x, y| x <= y)),
                    };
                    push!(v);
                }
                Op::Ge => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x >= y) as i32),
                        _ => tri!(numeric_cmp(a, b, |x, y| x >= y)),
                    };
                    push!(v);
                }
                Op::Neg => {
                    let v = pop!();
                    push!(tri!(eval_neg(v)));
                }
                Op::Not => {
                    let v = pop!();
                    push!(Value::Int(tri!(not_of(v, &self.interner)) as i32));
                }
                Op::CastInt => {
                    let v = pop!();
                    let i = match v {
                        Value::Int(i) => i,
                        Value::Float(f) => f as i32,
                        Value::String(s) | Value::Localized(s) => {
                            let text = self.interner.resolve(s);
                            match string_int(text) {
                                Some(i) => i,
                                None => fail!(ErrorKind::Custom(format!(
                                    "cannot cast \"{text}\" to int"
                                ))),
                            }
                        }
                        _ => fail!(ErrorKind::BadType("(int) needs a number")),
                    };
                    push!(Value::Int(i));
                }
                Op::CastFloat => {
                    let v = pop!();
                    let f = match v {
                        Value::Int(i) => i as f32,
                        Value::Float(f) => f,
                        Value::String(s) | Value::Localized(s) => {
                            let text = self.interner.resolve(s);
                            match string_float(text) {
                                Some(f) => f,
                                None => fail!(ErrorKind::Custom(format!(
                                    "cannot cast \"{text}\" to float"
                                ))),
                            }
                        }
                        _ => fail!(ErrorKind::BadType("(float) needs a number")),
                    };
                    push!(Value::Float(f));
                }
                Op::CastVector => {
                    let v = pop!();
                    match v {
                        Value::Vector(_) => push!(v),
                        _ => fail!(ErrorKind::BadType("(vector) needs a vector")),
                    }
                }
                Op::Jump(t) => ip = t as usize,
                Op::JumpIfFalse(t) => {
                    let c = pop!();
                    let not = match c {
                        Value::Int(i) => i == 0,
                        _ => tri!(not_of(c, &self.interner)),
                    };
                    if not {
                        ip = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let c = pop!();
                    let not = match c {
                        Value::Int(i) => i == 0,
                        _ => tri!(not_of(c, &self.interner)),
                    };
                    if !not {
                        ip = t as usize;
                    }
                }
                Op::Pop => sp -= 1,
                Op::Dup => push!(stack[sp - 1]),
                Op::Return => break Exit::Return(pop!()),
                Op::ReturnUndef => break Exit::Return(Value::Undefined),
                Op::Wait => {
                    let s = pop!();
                    let Some(seconds) = to_f32(s) else {
                        fail!(ErrorKind::BadType("wait needs a number of seconds"));
                    };
                    // Retail's two refusals, in its order: a negative wait
                    // (NaN included), then one the 24-bit clock cannot
                    // hold (docs/research/cod11-gsc-language.md, "The
                    // script clock").
                    if seconds.is_nan() || seconds < 0.0 {
                        fail!(ErrorKind::Custom(format!(
                            "negative wait of {} is not allowed",
                            crate::value::format_g(seconds)
                        )));
                    }
                    if seconds >= MAX_WAIT_SECONDS {
                        fail!(ErrorKind::Custom(format!(
                            "wait of {:.0} seconds is too long",
                            seconds
                        )));
                    }
                    break Exit::Step(Step::Suspend(Suspend::Wait { seconds }));
                }
                Op::WaitTillFrameEnd => {
                    break Exit::Step(Step::Suspend(Suspend::FrameEnd));
                }
                Op::WaitTill { ref binds } => {
                    let event = pop!();
                    let recv = pop!();
                    let Some(target) = as_target(recv) else {
                        fail!(ErrorKind::BadType(
                            "waittill needs an entity or struct receiver"
                        ));
                    };
                    let Value::String(event) = event else {
                        fail!(ErrorKind::BadType("waittill needs a string event name"));
                    };
                    // Event names fold, string values do not, so the name a
                    // literal or a concatenation produced is folded here
                    // rather than at the interner (atom.rs).
                    let event = self.interner.fold_atom(event);
                    break Exit::Step(Step::Suspend(Suspend::WaitTill {
                        target,
                        event,
                        binds: binds.clone(),
                        matches: Box::new([]),
                    }));
                }
                Op::WaitTillMatch { argc } => {
                    let base = sp - argc as usize;
                    let matches: Box<[Value]> = stack[base..sp].into();
                    sp = base;
                    let event = pop!();
                    let recv = pop!();
                    let Some(target) = as_target(recv) else {
                        fail!(ErrorKind::BadType(
                            "waittillmatch needs an entity or struct receiver"
                        ));
                    };
                    let Value::String(event) = event else {
                        fail!(ErrorKind::BadType(
                            "waittillmatch needs a string event name"
                        ));
                    };
                    let event = self.interner.fold_atom(event);
                    break Exit::Step(Step::Suspend(Suspend::WaitTill {
                        target,
                        event,
                        binds: Box::new([]),
                        matches,
                    }));
                }
                // `notify` and `endon` both resolve where they execute:
                // neither runs another thread (a woken waiter only turns
                // runnable), so there is no reentrancy, and an endon
                // registered earlier in a step must be visible to a notify a
                // nested spawn fires later in that same step.
                Op::Notify { argc } => {
                    let base = sp - argc as usize;
                    sp = base;
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(event) = event else {
                        fail!(ErrorKind::BadType("notify needs a string event name"));
                    };
                    // Redundant, not load-bearing: every path that drains
                    // `notifies` goes through `Vm::notify`, which folds
                    // again. Kept so the queued atom matches what
                    // `waittill`/`endon` stored.
                    let event = self.interner.fold_atom(event);
                    let Some(target) = as_target(recv) else {
                        fail!(ErrorKind::BadType(
                            "notify needs an entity or struct receiver"
                        ));
                    };
                    // Resolved here, as retail's `VM_Notify` is: the endon
                    // kills land and the waiters turn runnable now, so a
                    // thread this step spawns after the notify neither
                    // dies by it nor wakes to it (`probe_endon_late`). A
                    // waiter only turns runnable; nothing runs here. The
                    // arguments are read where they lie, above the stack.
                    let args = &stack[base..base + argc as usize];
                    self.notify_from(thread_id, target, event, args);
                }
                // Fused runs (`bytecode::fuse`): each takes its fast path
                // only when its whole run fits the budget and its values
                // are ones it answers without failing, and otherwise runs
                // its run's first op alone, the next op then running from
                // `fast` at `at + 1`.
                Op::LoadLocal2(a, b) => {
                    push!(locals[a as usize]);
                    if left > 0 {
                        left -= 1;
                        push!(locals[b as usize]);
                        ip = at + 2;
                    }
                }
                Op::StoreLoad(a, b) => {
                    let v = pop!();
                    self.heap.share(v);
                    locals[a as usize] = v;
                    if left > 0 {
                        left -= 1;
                        push!(locals[b as usize]);
                        ip = at + 2;
                    }
                }
                Op::ReturnLocal(a) => {
                    if left > 0 {
                        left -= 1;
                        ip = at + 2;
                        break Exit::Return(locals[a as usize]);
                    }
                    push!(locals[a as usize]);
                }
                Op::LevelField(name) => {
                    if left > 0 {
                        left -= 1;
                        let v = if name == self.size_atom {
                            Value::Int(1)
                        } else {
                            self.heap.get_field(self.level, name)
                        };
                        push!(v);
                        ip = at + 2;
                    } else {
                        push!(Value::Struct(self.level));
                    }
                }
                Op::LocalField(a, name) => {
                    let obj = locals[a as usize];
                    match field_of!(obj, name) {
                        Some(v) if left > 0 => {
                            left -= 1;
                            push!(v);
                            ip = at + 2;
                        }
                        _ => push!(obj),
                    }
                }
                Op::IndexLocals(a, k) => {
                    match (locals[a as usize], array_key(locals[k as usize])) {
                        (Value::Array(id), Some(key)) if left >= 2 => {
                            left -= 2;
                            push!(self.heap.get_index(id, key));
                            ip = at + 3;
                        }
                        (obj, _) => push!(obj),
                    }
                }
                Op::IndexLocalsField(a, k, name) => {
                    let obj = locals[a as usize];
                    let v = match (obj, array_key(locals[k as usize])) {
                        (Value::Array(id), Some(key)) if left >= 3 => {
                            let elem = self.heap.get_index(id, key);
                            field_of!(elem, name)
                        }
                        _ => None,
                    };
                    match v {
                        Some(v) => {
                            left -= 3;
                            push!(v);
                            ip = at + 4;
                        }
                        None => push!(obj),
                    }
                }
                Op::LtJumpIfFalse(to) => cmp_jump!(to, <),
                Op::GtJumpIfFalse(to) => cmp_jump!(to, >),
                Op::LeJumpIfFalse(to) => cmp_jump!(to, <=),
                Op::GeJumpIfFalse(to) => cmp_jump!(to, >=),
                Op::LtSizeJumpIfFalse { i, a, to } => {
                    let x = locals[i as usize];
                    match (to_f32(x), locals[a as usize]) {
                        // `LoadField size`'s `Int`, then `Lt` through
                        // `numeric_cmp` as two floats.
                        (Some(x), Value::Array(id)) if left >= 4 => {
                            left -= 4;
                            let n = self.heap.array_len(id) as i32;
                            ip = if x < n as f32 { at + 5 } else { to as usize };
                        }
                        _ => push!(x),
                    }
                }
                Op::AddConst { dst, src, c, to } => {
                    let x = locals[src as usize];
                    let sum = match (x, func.consts[c as usize]) {
                        (Value::Int(x), Value::Int(y)) => Some(Value::Int(x.wrapping_add(y))),
                        (Value::Float(x), Value::Float(y)) => Some(Value::Float(x + y)),
                        _ => None,
                    };
                    let run = if to.is_some() { 4 } else { 3 };
                    match sum {
                        // A number: `StoreLocal`'s `share` has nothing to do.
                        Some(v) if left >= run => {
                            left -= run;
                            locals[dst as usize] = v;
                            ip = match to {
                                Some(t) => t as usize,
                                None => at + 4,
                            };
                        }
                        _ => push!(x),
                    }
                }
                Op::EndOn => {
                    let event = pop!();
                    let recv = pop!();
                    let Value::String(event) = event else {
                        fail!(ErrorKind::BadType("endon needs a string event name"));
                    };
                    let event = self.interner.fold_atom(event);
                    let Some(target) = as_target(recv) else {
                        fail!(ErrorKind::BadType(
                            "endon needs an entity or struct receiver"
                        ));
                    };
                    // No thread to register against under `call_now`
                    // (`thread_id` is `None` there); the registration is
                    // simply dropped.
                    if let Some(id) = thread_id
                        && let Some(idx) = self.thread_index(id)
                    {
                        self.threads[idx].endons.push((target, event, depth));
                        self.notify_index
                            .endons
                            .entry((target, event))
                            .or_default()
                            .push(id);
                        self.notify_index_pushed();
                    }
                }
            }

            // A fused op ran as many of `code`'s ops as it took budget.
            #[cfg(test)]
            {
                let ran = 1 + (dbg_left - left) as usize;
                let effect: i32 = code[at..at + ran]
                    .iter()
                    .map(|o| {
                        let (pop, push) = crate::bytecode::stack_effect(o);
                        push - pop
                    })
                    .sum();
                let _ = (dbg_pop, dbg_push);
                assert_eq!(
                    sp as i32,
                    base_sp as i32 + effect,
                    "stack_effect disagreement at {dbg_op_repr} (before {base_sp}, {ran} ops)"
                );
            }

            if spawned && self.was_killed(thread_id) {
                break Exit::Step(Step::Returned(Value::Undefined));
            }
        };
        *frame_ip = ip as u32;
        *frame_sp = sp as u32;
        *op_base = base_sp as u32;
        *remaining = left;
        exit
    }

    fn no_such_function(&self, what: &str, target: FuncRef) -> ErrorKind {
        ErrorKind::Custom(format!(
            "{what} {}::{}",
            self.interner.resolve(target.file),
            self.interner.resolve(target.name)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::TestHost;

    /// Compiles and runs one `main`, returning what it `return`s or the
    /// `ScriptError::kind` it dies with.
    fn run_result(src: &str) -> Result<Value, ErrorKind> {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.call_now(&mut host, main, None, vec![])
            .map_err(|e| e.kind)
    }

    /// Round-trips `expr` through a `level` field write/read (exercising
    /// the same struct-field path a script does) and reads the result back
    /// through `Value::as_bool`'s strict cast, so a passing assertion here
    /// never smuggles in a wider notion of "bool" than `if` itself accepts.
    fn eval_bool(expr: &str) -> Result<bool, ErrorKind> {
        run_result(&format!("main() {{ level.r = ({expr}); return level.r; }}"))
            .and_then(|v| v.as_bool())
    }

    /// Retail's unary `!` accepts a string where an `if` condition does
    /// not: `probe_truthy` records `if ("a")` as fatal, but
    /// `_teams::restrictPlacedWeapons` runs
    /// `if(!getCvar("scr_allow_m1carbine"))` on every stock dm server and
    /// the bootstrap completes. The answers are `probe_not_string`'s and
    /// `probe_not_empty_string`'s (retail-captures.txt).
    #[test]
    fn not_accepts_a_string_where_if_does_not() {
        assert_eq!(eval_bool(r#"!"1""#), Ok(false));
        assert_eq!(eval_bool(r#"!"0""#), Ok(true));
        assert!(eval_bool(r#"!"""#).is_err());
        // `if` stays strict: this is the divergence, not a relaxation of
        // both paths.
        assert!(run_result(r#"main() { if ("a") { } }"#).is_err());
    }

    /// A fused run answers as its ops do one at a time, whatever budget
    /// cuts it, whatever types reach it, and with or without the error
    /// recovery: every budget up to past the script's end, against the same
    /// script with `Function::fast` left empty.
    #[test]
    fn fused_runs_answer_as_their_ops_at_every_budget() {
        let src = r#"
main()
{
    a = [];
    for ( i = 0; i < 4; i++ )
        a[i] = i * 2;
    c = [];
    for ( f = 0.5; f < a.size; f += 1.0 )
        c[c.size] = f;
    a[a.size] = c[1];
    j = 0;
    j += 0.5;
    level.x = 3;
    b[0] = level;
    m["k"] = 7;
    k = "k";
    t = 0;
    for ( i = 0; i < a.size; i++ )
    {
        x = a[i];
        y = b[0].x;
        z = m[k];
        if ( x > 2 )
            t += double( z );
        if ( x <= 1.5 )
            t += level.x;
        if ( y >= 3 )
            t += pick( x, y );
    }
    w = 3;
    while ( w >= 0 )
        w -= 1;
    if ( i < "x" )
        t += 100;
    u = m[nope];
    t += b[0].nope.size;
    return t + j + w;
}

pick( p, q )
{
    r = p;
    s = q;
    if ( r < s )
        return r;
    return s;
}
"#;
        let ast = crate::parse::parse_file(src).unwrap();
        let outcome = |budget: u32, fused: bool, recover: bool| {
            let mut vm = Vm::new();
            let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
            vm.install(fns).unwrap();
            if !fused {
                for f in vm.functions.values_mut() {
                    Rc::get_mut(f).unwrap().fast.clear();
                }
            }
            vm.set_budget(budget);
            vm.set_recover_errors(recover);
            let mut host = TestHost::default();
            let main = vm.func_ref("test/script", "main");
            let r = vm.call_now(&mut host, main, None, vec![]);
            let x = vm.interner.intern_folded("x");
            format!(
                "{r:?} {:?} {:?}",
                host.calls,
                vm.heap.get_field(vm.level, x)
            )
        };
        let mut finished = false;
        for budget in 1..2000 {
            for recover in [false, true] {
                let want = outcome(budget, false, recover);
                assert_eq!(outcome(budget, true, recover), want, "budget {budget}");
                finished |= recover && !want.contains("Budget");
            }
            if finished {
                break;
            }
        }
        assert!(finished, "the script ran out of budget throughout");
    }
}
