//! The instruction loop: walks `bytecode::Op` over a frame stack until it
//! suspends, returns, or the caller-supplied budget runs out.
//!
//! `run_ops` runs the top frame's ops with its ip, stack depth and budget
//! in locals, over `Prepared::fast` (the fused runs) and a stack sized to
//! the function's deepest; a call or return leaves it for `step_ops` to
//! change the frame stack (`Exit`). A call's arguments and a builtin's are
//! read in place off the caller's stack, and a returned frame's buffers go
//! back to `Vm::frame_pool`. Two numbers of one type take an inline arm in
//! the arithmetic and compare ops ahead of the general `eval_*`
//! (docs/research/cod4-server-frame.md 9, "The spawn wave", and 9.2;
//! `tests/bench.rs` is the microbench).

use std::rc::Rc;

use crate::atom::{Atom, Interner};
use crate::bytecode::{Function, Op};
use crate::heap::ArrayKey;
use crate::value::{FuncRef, Value, format_number};

use super::sched::ThreadId;
use super::{Cx, ErrorKind, Host, QueuedSpawn, ScriptError, Target, Vm};

/// Interpreter-internal: not part of the public API, since nothing outside
/// this crate drives `step_frames` directly, and doing so needs the
/// non-empty-`frames` precondition its doc comment states, one an
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
    /// stays alongside it, unresolved, for error attribution: a
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
    /// Test-only: what this frame's `stack` length must be once the call
    /// currently running on top of it returns, per `stack_effect`'s
    /// declared `(pop, push)` for that `Call`/`CallPtr`. Lives on the frame,
    /// not a local in `step_frames`, because a call can suspend (`wait`)
    /// several layers deep and resume in a later `step_frames` invocation.
    /// This has to survive that.
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

/// The most frames in flight, a thread's and those of every thread under it
/// on the native stack waiting on a spawn's immediate run. A call or a
/// thread call past it is `script stack overflow (too many embedded
/// function calls)`. Retail's CoD2 VM keeps one 32-entry frame stack for
/// all of them and refuses a call once 31 are in use (cod2-gsc-diff.md
/// 3.7); stock scripts reach 12, every CoD4 map's load included.
pub(crate) const MAX_FRAMES: usize = 31;

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

/// Two numbers as the ordering comparisons read them: two ints stay ints,
/// any other numeric pair compares as floats. Retail casts the pair to one
/// type, then compares ints with `cmp`/`setl` and floats with `fcomps`
/// (CoDMP.exe 1.1 0x478b98..0x478cd8, cod2_lnxded 0x807dd52).
#[derive(Clone, Copy)]
enum NumPair {
    Int(i32, i32),
    Float(f32, f32),
}

fn num_pair(a: Value, b: Value) -> Option<NumPair> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(NumPair::Int(x, y)),
        _ => Some(NumPair::Float(to_f32(a)?, to_f32(b)?)),
    }
}

/// `x op y` on a [`NumPair`].
macro_rules! pair_cmp {
    ($p:expr, $cmp:tt) => {
        match $p {
            NumPair::Int(x, y) => x $cmp y,
            NumPair::Float(x, y) => x $cmp y,
        }
    };
}

/// `a op b` for one of the four ordering comparisons. Retail has no
/// ordering outside numbers: `"a" < "b"` is fatal there (`# probe_cmp_order`).
macro_rules! numeric_cmp {
    ($a:expr, $b:expr, $cmp:tt) => {
        match num_pair($a, $b) {
            Some(p) => Ok(Value::Int(pair_cmp!(p, $cmp) as i32)),
            None => Err(ErrorKind::BadType("< > <= >= need numbers")),
        }
    };
}

/// The longest string a concatenation may build. No retail limit is on
/// record; stock scripts build at most 138 bytes and anything sent to a
/// client stops at 1024, while the interner keeps every string it is given
/// for the level, so `s = s + s` would otherwise reach gigabytes in about
/// 30 turns.
pub(crate) const MAX_STRING_LEN: usize = 8192;

/// vcod's ceiling on a level's script variables: every struct and array
/// the last sweep kept or allocated since, with their fields and elements
/// ([`crate::heap::Heap::vars`]), plus the live threads, so the garbage of
/// the frames since the sweep counts too. Past it the allocating thread
/// ends with `ErrorKind::TooManyVariables`, where retail's fixed pool
/// raises `exceeded maximum number of script variables`
/// (cod11-gsc-language.md, "Runtime errors below developer 1"). A stock
/// CoD4 round with 12 bots allocates about 17,000 a minute
/// (cod2-gsc-diff.md 3.7).
pub(crate) const MAX_VARIABLES: u64 = 64_000_000;

/// The same ceiling on the interned strings' bytes: the permanent ones,
/// the temporary ones the last sweep kept and those built since.
pub(crate) const MAX_STRING_BYTES: usize = 256 << 20;

/// `lhs` and `rhs` joined, or a script error past [`MAX_STRING_LEN`].
fn concat(lhs: &str, rhs: &str) -> Result<String, ErrorKind> {
    if lhs.len() + rhs.len() > MAX_STRING_LEN {
        return Err(ErrorKind::BadType("string too long"));
    }
    Ok(format!("{lhs}{rhs}"))
}

/// `s` interned as a temporary string, unless the interner already holds
/// `max_bytes`.
fn intern_within(interner: &mut Interner, max_bytes: usize, s: &str) -> Result<Value, ErrorKind> {
    if interner.bytes() + s.len() > max_bytes && interner.lookup_exact(s).is_none() {
        return Err(ErrorKind::TooManyVariables);
    }
    Ok(Value::String(interner.intern_temp(s)))
}

/// `a + b`; a concatenation past `max_bytes` of interned text fails.
fn eval_add(
    interner: &mut Interner,
    max_bytes: usize,
    a: Value,
    b: Value,
) -> Result<Value, ErrorKind> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x.wrapping_add(y))),
        (Value::Vector(x), Value::Vector(y)) => {
            Ok(Value::Vector([x[0] + y[0], x[1] + y[1], x[2] + y[2]]))
        }
        // A string on either side concatenates whatever renders
        // (`# probe_concat`); by exact text, not folded, so a built
        // display string keeps its case.
        (Value::String(x), other) => {
            let rhs = format_number(other, interner).ok_or(ErrorKind::BadType(
                "+ cannot concatenate that onto a string",
            ))?;
            let s = concat(interner.resolve(x), &rhs)?;
            intern_within(interner, max_bytes, &s)
        }
        (other, Value::String(y)) => {
            let lhs = format_number(other, interner).ok_or(ErrorKind::BadType(
                "+ cannot concatenate that onto a string",
            ))?;
            let s = concat(&lhs, interner.resolve(y))?;
            intern_within(interner, max_bytes, &s)
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
    let (start, len, neg) = numeric_prefix(text, false);
    let digits = text[start..len].trim_start_matches(['+', '-']);
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
    let (start, len, _) = numeric_prefix(text, true);
    let f = text[start..len].parse::<f64>().unwrap_or(0.0) as f32;
    (f != 0.0 || zero_spelled(text)).then_some(f)
}

/// Where `text`'s longest decimal number (an integer, or with `float` a
/// fraction and an exponent too) starts past the `isspace` blanks and
/// ends, and its sign; an empty run when there is none.
fn numeric_prefix(text: &str, float: bool) -> (usize, usize, bool) {
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
    if any {
        (start, i, neg)
    } else {
        (start, start, false)
    }
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
/// other non-entity, non-struct value: no corpus script uses `game` as a
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
    /// this function on the new thread's own frames): a nested spawn's
    /// failure does not abort this thread's own execution, only its own.
    pub(crate) fn step_frames(
        &mut self,
        host: &mut dyn Host,
        frames: &mut Vec<Frame>,
        remaining: &mut u32,
        thread_id: Option<ThreadId>,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        errors: &mut Vec<ScriptError>,
    ) -> Result<Step, ScriptError> {
        loop {
            match self.step_ops(host, frames, remaining, thread_id, notifies, errors) {
                Err(e) if self.recover_errors && e.kind.recoverable() => {
                    // Retail below `developer 1` (`Scr_Init`'s flag,
                    // coduo_lnxded 0x80aa818) leaves the failed
                    // instruction's results undefined, a conditional jump
                    // not taken, and runs on (cod11-gsc-language.md,
                    // "Runtime errors below developer 1").
                    self.record_error(&e, true);
                    errors.push(e);
                    let f = frames.last_mut().expect("an error leaves its frame on top");
                    let at = (f.ip as usize).saturating_sub(1);
                    let (pops, pushes) = crate::bytecode::stack_effect(&f.func_rc.code[at]);
                    // The failed op's depth on entry, which every path to it
                    // agrees on (`bytecode::Prepared::depths`).
                    let base = f.func_rc.prepared.depths.get(at).copied().unwrap_or(f.sp);
                    f.sp = f.sp.min((base as i32 - pops).max(0) as u32);
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
                Exit::Call(frame) if self.outer_frames as usize + frames.len() >= MAX_FRAMES => {
                    self.recycle_frame(frame);
                    let f = frames.last().expect("a call leaves its caller on top");
                    return Err(ScriptError {
                        file: f.func_rc.file,
                        func: f.func_rc.name,
                        line: f.func_rc.lines[(f.ip as usize).saturating_sub(1)],
                        kind: ErrorKind::StackOverflow,
                    });
                }
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
    #[expect(clippy::too_many_arguments)]
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
            #[cfg(test)]
            stack_effect_check,
            ..
        } = fr;
        let func: &Function = func_rc;
        let code = &func.code[..];
        // What runs: `code` with its runs fused (`bytecode::fuse`).
        let fast = if func.prepared.fast.len() == code.len() {
            &func.prepared.fast[..]
        } else {
            code
        };
        let locals = &mut locals[..];
        let self_recv = *frame_recv;
        let mut ip = *frame_ip as usize;
        let mut sp = *frame_sp as usize;
        let mut left = *remaining;
        let Spawns {
            pending: pending_spawns,
            field: field_spawns,
        } = spawns;

        // Sized for the deepest the function's code goes (`stack_depth`),
        // so a push only stores.
        let need = func.prepared.max_stack as usize;
        if stack.len() < need {
            stack.resize(need, Value::Undefined);
        }
        let stack = &mut stack[..];
        macro_rules! pop {
            () => {{
                sp -= 1;
                stack[sp]
            }};
        }
        macro_rules! push {
            ($v:expr) => {{
                stack[sp] = $v;
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
        // After an op that may allocate: past the level's ceiling the
        // thread ends (`MAX_VARIABLES`).
        macro_rules! ceiling {
            () => {
                if self.over_ceiling() {
                    fail!(ErrorKind::TooManyVariables);
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
                    Value::Entity(id) => self.entity_field(host, notifies, field_spawns, id, name),
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
        // other pair through `numeric_cmp!`, as the compare ops do.
        macro_rules! cmp_jump {
            ($to:expr, $cmp:tt) => {{
                let b = pop!();
                let a = pop!();
                let r = match (a, b) {
                    (Value::Float(x), Value::Float(y)) => x $cmp y,
                    _ => tri!(numeric_cmp!(a, b, $cmp)) == Value::Int(1),
                };
                if left > 0 {
                    left -= 1;
                    ip = if r { ip + 1 } else { $to as usize };
                } else {
                    push!(Value::Int(r as i32));
                }
            }};
        }

        // A fused `LoadLocal`/`Const`, compare and `JumpIfFalse`: two
        // numbers compare as `numeric_cmp!` does; any other pair runs the
        // first op alone, so the compare's error comes from it.
        macro_rules! operand_cmp_jump {
            ($b:expr, $to:expr, $cmp:tt) => {{
                let b = $b;
                match num_pair(stack[sp - 1], b) {
                    Some(p) if left >= 2 => {
                        left -= 2;
                        sp -= 1;
                        ip = if pair_cmp!(p, $cmp) {
                            ip + 2
                        } else {
                            $to as usize
                        };
                    }
                    _ => push!(b),
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
            #[cfg(test)]
            let base_sp = sp;
            #[cfg(test)]
            if let Some(&d) = func.prepared.depths.get(at) {
                assert_eq!(
                    sp, d as usize,
                    "the stack's depth at {at} is not the walk's"
                );
            }
            // A spawn ran another thread, which may have ended this one.
            let mut spawned = false;
            // What the threads this op spawned ran, out of `left`.
            #[cfg(test)]
            let mut nested_spent = 0u32;

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
                            match self.entity_field(host, notifies, field_spawns, id, name) {
                                Some(v) => v,
                                None => fail!(DEAD_OBJECT),
                            }
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
                    let obj = pop!();
                    let v = pop!();
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
                            cx.interner.pin(v);
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
                    ceiling!();
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
                        // `level["k"]`, `self["k"]` on `level`, `anim["k"]`
                        // compile and fail here on 1.1d and UO; 1.5b reads
                        // undefined (`probe_level_bracket_read`).
                        Value::Struct(_) if self.struct_index_reads_undefined => Value::Undefined,
                        Value::Struct(_) => fail!(ErrorKind::BadType("struct is not an array")),
                        _ => fail!(ErrorKind::BadType("indexing needs an array")),
                    };
                    push!(v);
                }
                Op::StoreIndex => {
                    let key = pop!();
                    let obj = pop!();
                    let mut v = pop!();
                    self.heap.share(v);
                    let Value::Array(id) = obj else {
                        fail!(ErrorKind::BadType("indexing needs an array"));
                    };
                    let Some(k) = array_key(key) else {
                        fail!(ErrorKind::BadType("array key must be a string or a number"));
                    };
                    // `a[0] = a`: the value read the array the target then
                    // owned in place. An array is a value; the element
                    // takes the array as it stood, not a cycle.
                    if let Value::Array(a) = v
                        && self.heap.reaches(a, id)
                    {
                        v = Value::Array(self.heap.snapshot(a));
                        self.heap.share(v);
                    }
                    self.heap.set_index(id, k, v);
                    ceiling!();
                }
                Op::OwnLocal(slot) => {
                    if let Value::Array(id) = locals[slot as usize] {
                        let own = self.heap.own(id);
                        locals[slot as usize] = Value::Array(own);
                        ceiling!();
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
                    ceiling!();
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
                    ceiling!();
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
                    ceiling!();
                    push!(Value::Array(id));
                }
                Op::LoadSelf => {
                    push!(self_recv.map(target_to_value).unwrap_or(Value::Undefined))
                }
                Op::LoadLevel => push!(Value::Struct(self.level)),
                // `game` is array-typed on retail, measured
                // tests/fixtures/semantics/retail-captures.txt `# probe_game`.
                Op::LoadGame => push!(Value::Array(self.game)),
                // A struct of its own: `anim.k = 1` reads back 1 on all
                // three servers (`probe_anim_dot`).
                Op::LoadAnim => push!(Value::Struct(self.anim)),
                Op::NewArray => {
                    let id = self.heap.new_array();
                    ceiling!();
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
                        if self.outer_frames as usize + depth >= MAX_FRAMES {
                            fail!(ErrorKind::StackOverflow);
                        }
                        ceiling!();
                        let args = stack[base..sp].to_vec();
                        sp = base - usize::from(has_recv);
                        let spent = self
                            .spawn_nested(host, target, callee, recv, args, depth, left, errors);
                        left -= spent;
                        #[cfg(test)]
                        {
                            nested_spent += spent;
                        }
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
                    let v = tri!(super::call_builtin(
                        host,
                        &mut cx,
                        name,
                        recv,
                        &stack[base..sp]
                    ));
                    sp = base - usize::from(has_recv);
                    // Retail's `Scr_ExecEntThread` from inside a builtin:
                    // each queued thread runs to its first suspend here,
                    // before the calling thread's next instruction (see
                    // `Cx::spawn`).
                    if !pending_spawns.is_empty() {
                        let (pending, field) = (&mut *pending_spawns, &mut *field_spawns);
                        self.outer_frames += depth as u32;
                        self.pool = Some(left);
                        spawned |= self
                            .run_builtin_spawns(host, func, at, notifies, errors, pending, field);
                        let spent = left - self.pool.take().unwrap_or(0);
                        self.outer_frames -= depth as u32;
                        left -= spent;
                        #[cfg(test)]
                        {
                            nested_spent += spent;
                        }
                    }
                    ceiling!();
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
                        if self.outer_frames as usize + depth >= MAX_FRAMES {
                            fail!(ErrorKind::StackOverflow);
                        }
                        ceiling!();
                        let args = args.to_vec();
                        let spent = self
                            .spawn_nested(host, target, callee, recv, args, depth, left, errors);
                        left -= spent;
                        #[cfg(test)]
                        {
                            nested_spent += spent;
                        }
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
                        _ => tri!(eval_add(&mut self.interner, self.max_string_bytes, a, b)),
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
                        _ => tri!(numeric_cmp!(a, b, <)),
                    };
                    push!(v);
                }
                Op::Gt => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x > y) as i32),
                        _ => tri!(numeric_cmp!(a, b, >)),
                    };
                    push!(v);
                }
                Op::Le => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x <= y) as i32),
                        _ => tri!(numeric_cmp!(a, b, <=)),
                    };
                    push!(v);
                }
                Op::Ge => {
                    let b = pop!();
                    let a = pop!();
                    let v = match (a, b) {
                        (Value::Float(x), Value::Float(y)) => Value::Int((x >= y) as i32),
                        _ => tri!(numeric_cmp!(a, b, >=)),
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
                Op::LevelIndexField(f, k, name) => {
                    let v = match (
                        self.heap.get_field(self.level, f),
                        array_key(locals[k as usize]),
                    ) {
                        (Value::Array(id), Some(key)) if left >= 4 => {
                            let elem = self.heap.get_index(id, key);
                            field_of!(elem, name)
                        }
                        _ => None,
                    };
                    match v {
                        Some(v) => {
                            left -= 4;
                            push!(v);
                            ip = at + 5;
                        }
                        None => push!(Value::Struct(self.level)),
                    }
                }
                Op::LtJumpIfFalse(to) => cmp_jump!(to, <),
                Op::GtJumpIfFalse(to) => cmp_jump!(to, >),
                Op::LeJumpIfFalse(to) => cmp_jump!(to, <=),
                Op::GeJumpIfFalse(to) => cmp_jump!(to, >=),
                Op::LtLocalJumpIfFalse(b, to) => operand_cmp_jump!(locals[b as usize], to, <),
                Op::GtLocalJumpIfFalse(b, to) => operand_cmp_jump!(locals[b as usize], to, >),
                Op::LtConstJumpIfFalse(c, to) => {
                    operand_cmp_jump!(func.consts[c as usize], to, <)
                }
                Op::GtConstJumpIfFalse(c, to) => {
                    operand_cmp_jump!(func.consts[c as usize], to, >)
                }
                Op::StoreLoad2(a, b, c) => {
                    let v = pop!();
                    self.heap.share(v);
                    locals[a as usize] = v;
                    if left >= 2 {
                        left -= 2;
                        push!(locals[b as usize]);
                        push!(locals[c as usize]);
                        ip = at + 3;
                    }
                }
                Op::IndexLocal(k) => match (stack[sp - 1], array_key(locals[k as usize])) {
                    (Value::Array(id), Some(key)) if left > 0 => {
                        left -= 1;
                        stack[sp - 1] = self.heap.get_index(id, key);
                        ip = at + 2;
                    }
                    _ => push!(locals[k as usize]),
                },
                Op::AddStore(a) => {
                    let y = pop!();
                    let x = pop!();
                    let v = match (x, y) {
                        (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(y)),
                        (Value::Float(x), Value::Float(y)) => Value::Float(x + y),
                        _ => tri!(eval_add(&mut self.interner, self.max_string_bytes, x, y)),
                    };
                    if left > 0 {
                        left -= 1;
                        self.heap.share(v);
                        locals[a as usize] = v;
                        ip = at + 2;
                    } else {
                        push!(v);
                    }
                }
                // `JumpIfFalse`'s reading of the left side, which a string
                // only fails as `not_of` does.
                Op::AndJump(to) | Op::OrJump(to) => {
                    let c = stack[sp - 1];
                    let not = match c {
                        Value::Int(i) => Some(i == 0),
                        _ => not_of(c, &self.interner).ok(),
                    };
                    let jump = match (op, not) {
                        (Op::AndJump(_), Some(not)) => Some(not),
                        (_, Some(not)) => Some(!not),
                        (_, None) => None,
                    };
                    match jump {
                        Some(true) if left >= 1 => {
                            left -= 1;
                            ip = to as usize;
                        }
                        Some(false) if left >= 2 => {
                            left -= 2;
                            sp -= 1;
                            ip = at + 3;
                        }
                        _ => push!(c),
                    }
                }
                Op::StepLtSize {
                    i,
                    c,
                    a,
                    body,
                    exit,
                    extra,
                } => {
                    let x = locals[i as usize];
                    let sum = match (x, func.consts[c as usize]) {
                        (Value::Int(x), Value::Int(y)) => Some(Value::Int(x.wrapping_add(y))),
                        (Value::Float(x), Value::Float(y)) => Some(Value::Float(x + y)),
                        _ => None,
                    };
                    match (sum, locals[a as usize]) {
                        // `AddConst`'s store, then `LtSizeJumpIfFalse` on it.
                        (Some(v), Value::Array(id)) if left >= u32::from(extra) => {
                            left -= u32::from(extra);
                            locals[i as usize] = v;
                            let n = Value::Int(self.heap.array_len(id) as i32);
                            let p = num_pair(v, n).expect("a number");
                            ip = if pair_cmp!(p, <) { body } else { exit } as usize;
                        }
                        _ => push!(x),
                    }
                }
                Op::LtLevelSizeJumpIfFalse { i, f, to } => {
                    let x = locals[i as usize];
                    match (x, self.heap.get_field(self.level, f)) {
                        (Value::Int(_) | Value::Float(_), Value::Array(id)) if left >= 5 => {
                            left -= 5;
                            let n = Value::Int(self.heap.array_len(id) as i32);
                            let p = num_pair(x, n).expect("a number");
                            ip = if pair_cmp!(p, <) { at + 6 } else { to as usize };
                        }
                        _ => push!(x),
                    }
                }
                Op::LtLocalConstJumpIfFalse { a, c, to } => {
                    let x = locals[a as usize];
                    match num_pair(x, func.consts[c as usize]) {
                        Some(p) if left >= 3 => {
                            left -= 3;
                            ip = if pair_cmp!(p, <) { at + 4 } else { to as usize };
                        }
                        _ => push!(x),
                    }
                }
                Op::SubLocals(a, b) => {
                    let x = locals[a as usize];
                    match eval_sub(x, locals[b as usize]) {
                        Ok(v) if left >= 2 => {
                            left -= 2;
                            push!(v);
                            ip = at + 3;
                        }
                        _ => push!(x),
                    }
                }
                Op::LtSizeJumpIfFalse { i, a, to } => {
                    let x = locals[i as usize];
                    match (x, locals[a as usize]) {
                        // `LoadField size`'s `Int`, then `Lt` through
                        // `numeric_cmp!`.
                        (Value::Int(_) | Value::Float(_), Value::Array(id)) if left >= 4 => {
                            left -= 4;
                            let n = Value::Int(self.heap.array_len(id) as i32);
                            let p = num_pair(x, n).expect("a number");
                            ip = if pair_cmp!(p, <) { at + 5 } else { to as usize };
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
                let ran = 1 + (dbg_left - left - nested_spent) as usize;
                // Along the run's path: a `Jump` inside it goes on at its
                // target.
                let (mut pc, mut effect) = (at, 0);
                for _ in 0..ran {
                    let (pop, push) = crate::bytecode::stack_effect(&code[pc]);
                    effect += push - pop;
                    pc = match code[pc] {
                        Op::Jump(t) => t as usize,
                        _ => pc + 1,
                    };
                }
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
        *remaining = left;
        exit
    }

    /// An entity's field, `None` for a dead entity, through a `Cx` whose
    /// spawns go nowhere (see [`Spawns`]).
    #[inline(never)]
    fn entity_field(
        &mut self,
        host: &mut dyn Host,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        field_spawns: &mut Vec<QueuedSpawn>,
        id: crate::value::EntId,
        name: Atom,
    ) -> Option<Value> {
        let mut cx = Cx {
            interner: &mut self.interner,
            heap: &mut self.heap,
            level: self.level,
            game: self.game,
            notifies,
            spawns: field_spawns,
        };
        let v = host.get_live_field(&mut cx, id, name);
        debug_assert!(
            field_spawns.is_empty(),
            "Cx::spawn is only honoured from a builtin"
        );
        v
    }

    /// [`Vm::spawn`] from a thread call in a step with `left` instructions
    /// and `depth` frames: the new thread runs on what the step has left and
    /// on top of its frames. The instructions it spent.
    #[expect(clippy::too_many_arguments)]
    fn spawn_nested(
        &mut self,
        host: &mut dyn Host,
        target: FuncRef,
        callee: Rc<Function>,
        recv: Option<Target>,
        args: Vec<Value>,
        depth: usize,
        left: u32,
        errors: &mut Vec<ScriptError>,
    ) -> u32 {
        self.outer_frames += depth as u32;
        self.pool = Some(left);
        self.spawn(host, target, callee, recv, args, errors);
        self.outer_frames -= depth as u32;
        left - self.pool.take().unwrap_or(0)
    }

    /// Runs the threads a builtin just queued (`Cx::spawn`), each to its
    /// first suspend, as retail's `Scr_ExecEntThread` from inside a builtin
    /// does, before the calling thread's next op; whether any ran. `func`
    /// and `at` name the call, for an unknown function's error.
    #[inline(never)]
    fn run_builtin_spawns(
        &mut self,
        host: &mut dyn Host,
        func: &Function,
        at: usize,
        notifies: &mut Vec<(Target, Atom, Vec<Value>)>,
        errors: &mut Vec<ScriptError>,
        pending_spawns: &mut Vec<QueuedSpawn>,
        field_spawns: &mut Vec<QueuedSpawn>,
    ) -> bool {
        let mut spawned = false;
        let mut queue = std::collections::VecDeque::from(std::mem::take(pending_spawns));
        while let Some(queued) = queue.pop_front() {
            let target = queued.func;
            match self.functions.get(&target).cloned() {
                Some(_) if self.outer_frames as usize >= MAX_FRAMES => {
                    let e = ScriptError {
                        file: func.file,
                        func: func.name,
                        line: func.lines[at],
                        kind: ErrorKind::StackOverflow,
                    };
                    self.record_abort(&e);
                    errors.push(e);
                }
                Some(f) => {
                    self.spawn(host, target, f, queued.recv, queued.args, errors);
                    spawned = true;
                }
                None => {
                    let e = ScriptError {
                        file: func.file,
                        func: func.name,
                        line: func.lines[at],
                        kind: self.no_such_function("spawn of unknown function", target),
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
                // The engine code after one `Scr_ExecEntThread` may start
                // the next: those threads go ahead of the rest of the
                // builtin's queue.
                for next in field_spawns.drain(..).rev() {
                    queue.push_front(next);
                }
            }
        }
        spawned
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
    /// script with `Prepared::fast` left empty.
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
    record( t, x, y, z );
    w = 3;
    while ( w >= 0 )
        w -= 1;
    level.arr = a;
    n = "s" + w;
    for ( i = 0; i < 6; i++ )
    {
        e = level.arr[i];
        q = e + 1;
        if ( e < 3 && q > 1 || e > 5 )
            t += 1;
        if ( e > 2 )
            t += e;
        if ( q < t )
            t += 1;
        if ( isdefined( e ) || n )
            t += 2;
    }
    level.arr2 = b;
    for ( i = 0; i < 3; i++ )
    {
        g = level.arr2[i].x;
        record( g );
    }
    g = 1;
    for ( i = 0; i < level.arr.size; i++ )
    {
        if ( i > 100 )
            g = 0;
        d = i - g;
        if ( d < 3 )
            continue;
        g += d;
    }
    record( g, d );
    for ( k = 0.5; k < c.size; k += 1 )
        t += k;
    record( t, k );
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
                    Rc::get_mut(f).unwrap().prepared.fast.clear();
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
