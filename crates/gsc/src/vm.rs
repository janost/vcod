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
use crate::sched::{Thread, ThreadId, ThreadState};
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

/// Interpreter-internal: not part of the public API, since nothing outside
/// this crate drives `step_frames` directly, and doing so needs the
/// non-empty-`frames` precondition its doc comment states -- one an
/// embedder has no way to satisfy correctly from outside the scheduler.
#[derive(Debug)]
pub(crate) struct Frame {
    pub func: FuncRef,
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

impl From<EntId> for Target {
    fn from(e: EntId) -> Self {
        Target::Entity(e)
    }
}

impl From<StructId> for Target {
    fn from(s: StructId) -> Self {
        Target::Struct(s)
    }
}

pub struct Vm {
    interner: Interner,
    heap: Heap,
    functions: HashMap<FuncRef, Rc<Function>>,
    level: StructId,
    game: StructId,
    /// Kept in start order so a `run_frame` walk, and a `notify` wake, are
    /// both deterministic.
    threads: Vec<Thread>,
    next_thread: u32,
    /// Instructions a thread may run in one scheduling step before it is
    /// aborted with `ErrorKind::Budget`. Every instruction counts against
    /// it, `Call`/`CallPtr` included, so unbounded recursion is caught the
    /// same as an unbounded loop.
    budget: u32,
    /// The clock a `Suspend::Wait` hit during an immediate run resolves
    /// against: set by `run_frame` on entry, and by `start_thread` and
    /// `call_now` before their own immediate runs, so a `wait` a thread's
    /// first instructions hit -- whether stepped by `run_frame` or by a
    /// fresh `start_thread`/`call_now` call before any `run_frame` has
    /// ever run -- always sees the real clock, never the `0` this
    /// defaults to. A nested `spawn` (recursing from inside any of the
    /// three) reads whichever last set it, which is always correct since
    /// none of them changes it mid-recursion.
    now_ms: i32,
    /// How many `spawn` calls are currently nested on the native Rust
    /// stack (a thread whose immediate run itself spawns a thread, whose
    /// immediate run spawns another, ...). `budget` bounds a single
    /// thread's own instructions, including its `Call`s, but not this: a
    /// `Call` only grows `Frame`s on a `Vec`, while a nested `spawn` grows
    /// the native call stack (`spawn` -> `step_thread` -> `step_frames` ->
    /// `spawn` -> ...), which overflows long before any instruction
    /// budget would. See `spawn`'s `MAX_SPAWN_DEPTH` check.
    spawn_depth: u32,
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
            threads: Vec::new(),
            next_thread: 0,
            budget: 1_000_000,
            now_ms: 0,
            spawn_depth: 0,
        }
    }

    pub fn set_budget(&mut self, budget: u32) {
        self.budget = budget;
    }

    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Starts a new thread running `func` from its first instruction and
    /// returns its id. The function must already be installed. Runs the
    /// thread immediately, to its own suspend/return/error, before
    /// returning -- see `spawn`. `now_ms` is the caller's own clock
    /// reading (matching `run_frame`'s parameter of the same name) and
    /// sets `self.now_ms` before the immediate run, so a `wait` the new
    /// thread's first instructions hit resolves against the real clock
    /// even when called before the first `run_frame` -- e.g. a host
    /// starting level-load threads at startup, ahead of the frame loop.
    ///
    /// The returned `ThreadId` can already refer to a finished, errored,
    /// or killed thread by the time this returns: the immediate run can
    /// carry the thread all the way to completion (or a nested `spawn`
    /// during that run can get it killed by its own endon), and this API
    /// has no liveness query. A caller that needs to know only cares once
    /// it goes looking for the thread again (e.g. to `notify` something
    /// it expects still to be listening), at which point a vanished id is
    /// already handled the same way a genuinely-later removal is.
    pub fn start_thread(
        &mut self,
        host: &mut dyn Host,
        now_ms: i32,
        func: FuncRef,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> ThreadId {
        self.now_ms = now_ms;
        let f = self
            .functions
            .get(&func)
            .cloned()
            .expect("start_thread target must be installed");
        // Errors from this immediate run have nowhere to go through this
        // signature (`-> ThreadId`, matching the public API); they are
        // still logged by `step_thread` as they happen. A caller that
        // needs to observe them starts the thread suspended (e.g. behind
        // a leading `wait 0;`) and lets `run_frame` step it instead.
        let mut errors = Vec::new();
        self.spawn(host, func, &f, recv, args, &mut errors)
    }

    /// A thread whose immediate run itself spawns a thread nested this
    /// deep runs the native Rust stack out (`spawn` -> `step_thread` ->
    /// `step_frames` -> `spawn` -> ...) long before `budget` would catch
    /// it, since each level only costs one `Call`-equivalent instruction
    /// against its *own* fresh budget. No corpus script nests a `thread`
    /// statement inside another spawned thread's own immediate execution
    /// more than one or two levels deep; this is purely a backstop against
    /// a spawn bomb (a thread whose first act is spawning another), not a
    /// limit real scripts should ever approach.
    const MAX_SPAWN_DEPTH: u32 = 64;

    /// Pushes a new `Runnable` thread and immediately steps it to its own
    /// suspend, return, or error — never leaving it merely `Runnable` for
    /// a later scheduling pass to discover cold. Retail scripts pair a
    /// `thread` statement with a `notify` a line or two later expecting
    /// the spawned thread to have already reached its `waittill`
    /// (`animscripts/predict.gsc:102-108`; 246 corpus sites do this within
    /// three lines): a deferred spawn would let the notify fire before the
    /// thread registers for it, dropping the event and hanging the
    /// `waittill` forever. Spawning is therefore recursive — the spawned
    /// thread's own immediate run may itself spawn and immediately run a
    /// further thread — bounded by `MAX_SPAWN_DEPTH`, since each nesting
    /// level costs Rust native stack, not `budget` (see its doc comment).
    /// Past that depth the new thread is left merely `Runnable`, the
    /// pre-immediate-run behavior, for the next `run_frame` to pick up
    /// instead of stepping it here: still correct (nothing hangs, nothing
    /// panics), it just loses "runs before its spawner continues" at a
    /// depth no real script reaches.
    fn spawn(
        &mut self,
        host: &mut dyn Host,
        func: FuncRef,
        f: &Function,
        recv: Option<Target>,
        args: Vec<Value>,
        errors: &mut Vec<ScriptError>,
    ) -> ThreadId {
        let frame = self.make_frame(func, f, recv, args);
        let id = ThreadId(self.next_thread);
        self.next_thread += 1;
        self.threads.push(Thread {
            id,
            frames: vec![frame],
            state: ThreadState::Runnable,
            endons: Vec::new(),
        });
        if self.spawn_depth < Self::MAX_SPAWN_DEPTH {
            self.spawn_depth += 1;
            self.step_thread(host, id, errors);
            self.spawn_depth -= 1;
        } else {
            // Silent otherwise, this degrades into exactly the deadlock
            // this round's immediate-run fix removed: two threads left
            // waiting on each other with nothing to say why. A named
            // constant and the function that tripped it is enough to spot
            // "a script nests `thread` this deep" during triage.
            log::warn!(
                "gsc: MAX_SPAWN_DEPTH ({}) exceeded spawning {}::{}; thread left runnable for the next scheduling pass instead of running immediately",
                Self::MAX_SPAWN_DEPTH,
                self.interner.resolve(func.file),
                self.interner.resolve(func.name)
            );
        }
        id
    }

    /// Kills every thread endon-registered against `(target, event)`, then
    /// wakes every thread waiting on it, binding `args` into the locals the
    /// `waittill` recorded (a missing argument writes `Undefined`). Both
    /// passes walk `threads` in start order, so two waiters on the same
    /// event always wake oldest first, and every endon kill lands before
    /// any notify wake. Waking only flips a thread's state to `Runnable`;
    /// it does not run here, so a notify can never reenter the VM.
    pub fn notify(&mut self, target: impl Into<Target>, event: Atom, args: &[Value]) {
        let target = target.into();
        let mut i = 0;
        while i < self.threads.len() {
            if self.threads[i]
                .endons
                .iter()
                .any(|&(t, e)| t == target && e == event)
            {
                self.threads.remove(i);
            } else {
                i += 1;
            }
        }

        for t in &mut self.threads {
            let matches_wait = matches!(
                &t.state,
                ThreadState::WaitingNotify { target: wt, event: we, .. }
                    if *wt == target && *we == event
            );
            if !matches_wait {
                continue;
            }
            let ThreadState::WaitingNotify { binds, .. } =
                std::mem::replace(&mut t.state, ThreadState::Runnable)
            else {
                unreachable!("matches_wait only true for WaitingNotify");
            };
            let frame = t
                .frames
                .last_mut()
                .expect("a waiting thread always retains its frame stack");
            for (arg_idx, &slot) in binds.iter().enumerate() {
                frame.locals[slot as usize] =
                    args.get(arg_idx).copied().unwrap_or(Value::Undefined);
            }
        }
    }

    fn log_script_error(&self, e: &ScriptError) {
        log::warn!(
            "gsc: thread aborted in {}::{}:{}: {:?}",
            self.interner.resolve(e.file),
            self.interner.resolve(e.func),
            e.line,
            e.kind
        );
    }

    /// Steps the thread `id` -- assumed `Runnable`, a no-op otherwise or if
    /// it no longer exists (see below) -- until it suspends, returns, or
    /// errors, then updates its stored state or removes it, and flushes
    /// any notify it queued along the way (`Op::Notify` is never resolved
    /// from inside `step_frames` itself, so a notify can't reenter the
    /// VM). A threaded call reached during the step recursively spawns and
    /// immediately runs the new thread to *its* suspend/return/error
    /// before this call returns (`spawn`), so by the time control comes
    /// back here `self.threads` may have gained, lost, or reordered
    /// entries; every write-back below re-resolves `id` to its current
    /// index rather than trusting the one taken before the step, and
    /// tolerates it having vanished -- a thread killed by an endon fired
    /// from a thread it itself spawned, mid-step, is simply gone by the
    /// time we look again, and there is nothing left to write back to.
    /// Errors, whether this thread's own or a nested spawn's, are pushed
    /// into `errors` as they occur; the caller (`run_frame`, or an
    /// enclosing `step_thread` for a nested spawn) owns collecting them.
    /// `Op::EndOn` inside `step_frames` writes straight into
    /// `self.threads[..].endons` as soon as it executes (passed this
    /// thread's own id), not through an accumulate-then-write-back vec
    /// like `notifies`: an endon registered earlier in this very step has
    /// to be visible to a notify a nested spawn fires later in the same
    /// step (`main() { self endon("x"); thread killer(); ... }`, where
    /// `killer`'s immediate run notifies `"x"` before this call ever
    /// reaches its own suspend point to write anything back).
    fn step_thread(&mut self, host: &mut dyn Host, id: ThreadId, errors: &mut Vec<ScriptError>) {
        let Some(idx) = self.threads.iter().position(|t| t.id == id) else {
            return;
        };
        if !matches!(self.threads[idx].state, ThreadState::Runnable) {
            return;
        }

        let mut frames = std::mem::take(&mut self.threads[idx].frames);
        let mut notifies = Vec::new();
        let budget = self.budget;
        let result = self.step_frames(host, &mut frames, budget, Some(id), &mut notifies, errors);

        match result {
            Ok(Step::Returned(_)) => {
                if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                    self.threads.remove(idx);
                }
            }
            Ok(Step::Suspend(Suspend::Wait { seconds })) => {
                let delay_ms = (seconds.max(0.0) * 1000.0) as i32;
                let deadline = self.now_ms + delay_ms;
                if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                    self.threads[idx].frames = frames;
                    self.threads[idx].state = ThreadState::WaitingUntil(deadline);
                }
            }
            Ok(Step::Suspend(Suspend::WaitTill {
                target,
                event,
                binds,
            })) => {
                if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                    self.threads[idx].frames = frames;
                    self.threads[idx].state = ThreadState::WaitingNotify {
                        target,
                        event,
                        binds,
                    };
                }
            }
            Ok(Step::Running) => {
                let top = frames
                    .last()
                    .expect("budget exhaustion always leaves a frame on top");
                let func = self
                    .functions
                    .get(&top.func)
                    .expect("frame references an installed function");
                let line = func.lines[(top.ip as usize).saturating_sub(1)];
                let e = ScriptError {
                    file: func.file,
                    func: func.name,
                    line,
                    kind: ErrorKind::Budget,
                };
                self.log_script_error(&e);
                errors.push(e);
                if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                    self.threads.remove(idx);
                }
            }
            Err(e) => {
                self.log_script_error(&e);
                errors.push(e);
                if let Some(idx) = self.threads.iter().position(|t| t.id == id) {
                    self.threads.remove(idx);
                }
            }
        }

        for (target, event, args) in notifies {
            self.notify(target, event, &args);
        }
    }

    /// A ceiling on how many ids the id-ascending watermark walk visits in
    /// one `run_frame` call -- not how many threads it actually steps:
    /// the walk picks the next id regardless of state, and `step_thread`
    /// is the one that returns early for a non-`Runnable` thread, so a
    /// waiting thread costs an iteration here without being stepped.
    /// `spawn`'s own `MAX_SPAWN_DEPTH` bounds any *one* nested spawn
    /// chain's native-stack depth, but a spawn bomb's chain still unwinds
    /// leaving one fresh dangling thread behind for this very walk to
    /// pick straight back up -- so without a separate cap here, the walk
    /// would discover an unending sequence of "one more thread" and this
    /// call would never return, hanging the server's frame loop outright.
    /// Since the watermark restarts at `None` every call, a VM holding
    /// more live threads than this constant would walk only the same
    /// lowest-id prefix every frame and never reach the rest -- silent,
    /// permanent starvation past this many concurrently live threads.
    /// 1,000 is still far above any real frame (CoD's own entity cap is
    /// 1024) while keeping a spawn bomb's worst-case per-frame cost
    /// bounded to roughly a tenth of what 10,000 cost.
    const MAX_THREADS_PER_FRAME: u32 = 1_000;

    /// Runs one server frame: promotes every `WaitingUntil` thread whose
    /// deadline has passed to `Runnable`, then walks ids in ascending
    /// order, up to `MAX_THREADS_PER_FRAME` of them, stepping
    /// (`step_thread`) whichever are `Runnable`. A thread spawned
    /// mid-frame (a threaded call, run immediately by `spawn`) is visited
    /// too, since its id is higher than the watermark that admitted it;
    /// the watermark is a `ThreadId`, not a cached index, since a step's
    /// deferred notify can add, remove or reorder `self.threads` before
    /// the walk reaches its next entry. The errors are collected and
    /// returned rather than propagated, so one bad thread never stops the
    /// rest of the server.
    pub fn run_frame(&mut self, host: &mut dyn Host, now_ms: i32) -> Vec<ScriptError> {
        self.now_ms = now_ms;
        for t in &mut self.threads {
            if let ThreadState::WaitingUntil(deadline) = t.state {
                if deadline <= now_ms {
                    t.state = ThreadState::Runnable;
                }
            }
        }

        let mut errors = Vec::new();
        let mut last_id: Option<u32> = None;
        for _ in 0..Self::MAX_THREADS_PER_FRAME {
            let next = self
                .threads
                .iter()
                .filter(|t| last_id.is_none_or(|l| t.id.0 > l))
                .min_by_key(|t| t.id.0)
                .map(|t| t.id);
            let Some(tid) = next else {
                break;
            };
            last_id = Some(tid.0);
            self.step_thread(host, tid, &mut errors);
        }
        errors
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    /// The struct/array store, for a host that needs to allocate an array
    /// or struct (`getentarray`, `spawnstruct`) or read/write a field or
    /// element from outside the instruction loop. `Heap`'s own methods are
    /// the API — bounds-checked, so an id the host mishandles reads
    /// `Undefined` or drops a write rather than panicking.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// `level` as a call/notify/waittill/endon target, for a host driving
    /// e.g. `level notify(...)` or reading `level.callbackStartGameType`
    /// from outside a script.
    pub fn level(&self) -> Target {
        Target::Struct(self.level)
    }

    /// `game`, the other heap struct every `Vm` preallocates.
    pub fn game(&self) -> Target {
        Target::Struct(self.game)
    }

    /// Every installed function, for a loader's cross-file scan and
    /// builtin pre-scan.
    pub fn functions(&self) -> impl Iterator<Item = &Function> {
        self.functions.values().map(Rc::as_ref)
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

    /// Removes previously installed functions, for a loader that rolls back
    /// a partially loaded reference chain after a later file in it fails.
    pub fn uninstall(&mut self, refs: &[FuncRef]) {
        for r in refs {
            self.functions.remove(r);
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
                        self.spawn(host, target, &callee, recv, args, errors);
                        push!(Value::Undefined);
                    } else {
                        frames.push(self.make_frame(target, &callee, recv, args));
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
                    let v = host
                        .builtin(&self.interner, name, recv, &args)
                        .map_err(err)?;
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
                        self.spawn(host, target, &callee, recv, args, errors);
                        push!(Value::Undefined);
                    } else {
                        frames.push(self.make_frame(target, &callee, recv, args));
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
                // `notify` is queued for the caller rather than resolved
                // here -- it must not wake another thread mid-instruction.
                // `endon` is resolved immediately below instead of queued,
                // unlike `notify`: it only ever adds a registration, never
                // runs anything, so there is no reentrancy risk, and
                // queuing it would reopen exactly the visibility gap this
                // round closed (see `step_thread`'s doc comment).
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
                    let target = as_target(recv).ok_or_else(|| {
                        err(ErrorKind::BadType(
                            "endon needs an entity or struct receiver",
                        ))
                    })?;
                    // No thread to register against under `call_now`
                    // (`thread_id` is `None` there); the registration is
                    // simply dropped, same as before this round.
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

    /// Runs `func` to completion on a fresh one-frame stack. A `wait` or
    /// `waittill` it reaches on *this* frame stack is a script error, not
    /// a hang: there is no scheduler here to come back to it later. But a
    /// threaded call inside `func` spawns and immediately runs a real,
    /// independent thread (`spawn`), and that thread's own `wait` is not
    /// an error -- it's an ordinary suspend, resolved into a
    /// `WaitingUntil` deadline the same way `run_frame`/`start_thread`
    /// would. `now_ms` (matching their parameter of the same name) is
    /// what that deadline is computed against; without it (before this
    /// existed, `self.now_ms` stayed whatever the default or the last
    /// `run_frame` call left it at) a thread spawned from inside a
    /// `call_now`-driven script -- e.g. a host's `CodeCallback_
    /// PlayerConnect`-style callback threading off a waiting worker --
    /// could fire its wait on the very next `run_frame` instead of after
    /// the real delay.
    pub fn call_now(
        &mut self,
        host: &mut dyn Host,
        now_ms: i32,
        func: FuncRef,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> Result<Value, ScriptError> {
        self.now_ms = now_ms;
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
        let mut notifies = Vec::new();
        // Errors from a threaded call spawned (and immediately run) during
        // this call have nowhere to go through this function's `Result
        // <Value, ScriptError>` either; they are still logged as they
        // happen (`step_thread`), same as `start_thread`.
        let mut errors = Vec::new();
        let budget = self.budget;
        // No real thread to register an `endon` against here (`None`); a
        // one-shot `call_now` script that calls `endon` simply drops it.
        let result = self.step_frames(host, &mut frames, budget, None, &mut notifies, &mut errors);
        // A notify fired before the failure below still had its effect.
        for (target, event, args) in notifies {
            self.notify(target, event, &args);
        }
        match result? {
            Step::Returned(v) => Ok(v),
            Step::Suspend(_) => {
                let top = frames
                    .last()
                    .expect("a suspend always leaves a frame on top");
                let f = self
                    .functions
                    .get(&top.func)
                    .expect("frame references an installed function");
                Err(ScriptError {
                    file: f.file,
                    func: f.name,
                    line: f.lines[(top.ip as usize).saturating_sub(1)],
                    kind: ErrorKind::SuspendedInImmediateCall,
                })
            }
            Step::Running => {
                let top = frames
                    .last()
                    .expect("budget exhaustion always leaves a frame on top");
                let f = self
                    .functions
                    .get(&top.func)
                    .expect("frame references an installed function");
                Err(ScriptError {
                    file: f.file,
                    func: f.name,
                    line: f.lines[(top.ip as usize).saturating_sub(1)],
                    kind: ErrorKind::Budget,
                })
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::atom::Interner;
    use crate::value::{EntId, Value};
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct TestHost {
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
            // Builtin dispatch matches against a fixed lowercase literal, so
            // it resolves folded: `IPrintLn(...)` must still reach the
            // "iprintln" arm below.
            let n = interner.resolve_folded(name).to_string();
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
        let v = vm.call_now(&mut host, 0, main, None, vec![]).unwrap();
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
        let e = vm.call_now(&mut host, 0, main, None, vec![]).unwrap_err();
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
        let e = vm.call_now(&mut host, 0, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::SuspendedInImmediateCall));
    }

    /// A thread spawned by a `call_now`-driven script (a host callback
    /// like `CodeCallback_PlayerConnect`, say) is a real, independent
    /// thread, not the throwaway one-shot stack `call_now` itself runs
    /// on -- its own `wait` is an ordinary suspend, not an immediate-call
    /// error, and has to resolve against `call_now`'s caller's clock
    /// reading, not always 0. `f`'s `wait 1` (1000 ms) taken during
    /// `main`'s `call_now`-driven run at `now_ms = 5_000` must deadline at
    /// 6_000, not 1_000: a `run_frame` at 5_000 (right after) must not
    /// fire it, and one at 6_000 must.
    #[test]
    fn call_now_threads_a_wait_against_its_own_now_ms_not_zero() {
        let ast =
            crate::parse::parse_file("main() { thread f(); } f() { wait 1; done(); }").unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.call_now(&mut host, 5_000, main, None, vec![]).unwrap();
        assert_eq!(vm.thread_count(), 1, "f is alive, waiting on its own wait");

        vm.run_frame(&mut host, 5_000);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "not due yet -- 6_000, not 1_000"
        );
        vm.run_frame(&mut host, 6_000);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "due now");
    }

    // --- Fix round 1: `level`/`game` as call and notify/waittill/endon
    // receivers, and the four brief rules that previously rested only on
    // review. ---

    /// `level f()` and friends are the majority of the corpus's control
    /// flow (996 `level notify`, 956 `level waittill`, 728 `level thread`,
    /// 408 `level endon` sites). `self` inside the callee must bind to the
    /// same heap struct `level` names, not fall back to `Undefined` the way
    /// a non-entity receiver used to. A plain call, exercising the regular
    /// `Op::Call` path; `level_thread_call_binds_self_to_the_struct_
    /// receiver` below covers the same binding through the threaded-spawn
    /// path instead.
    #[test]
    fn level_can_be_a_call_receiver_and_self_binds_to_it() {
        let (v, _, _) =
            run("main() { level helper(); return level.mark; } helper() { self.mark = 7; }");
        assert_eq!(v, Value::Int(7));
    }

    /// The threaded-spawn path (`Op::Call { threaded: true, .. }` ->
    /// `spawn`) with a struct receiver: this is the shape of 728 corpus
    /// `level thread` sites, and unlike the plain-call test above it goes
    /// through `spawn`'s immediate run rather than an inline `Call`, which
    /// `level_can_be_a_call_receiver_and_self_binds_to_it` stopped
    /// covering once `thread` became async (see its own comment).
    #[test]
    fn level_thread_call_binds_self_to_the_struct_receiver() {
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
        let e = vm.call_now(&mut host, 0, main, None, vec![]).unwrap_err();
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
        let e = vm.call_now(&mut host, 0, main, None, vec![]).unwrap_err();
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
        assert_eq!(vm.interner_mut().resolve(a), "foobar");
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
        assert_eq!(vm.interner_mut().resolve(a), "Round won");
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
        let e = vm.call_now(&mut host, 0, main, None, vec![]).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::BadType(_)));
    }

    /// The interner folds atom identity case-insensitively, and array keys
    /// intern the same way field names do, so two differently-cased string
    /// literals used as a key collide on write and read alike. The corpus
    /// has three such pairs; nothing pinned this before.
    #[test]
    fn array_keys_fold_case_like_field_names() {
        let (v, _, _) = run(r#"main() { a = []; a["medFire"] = 1; return a["medfire"]; }"#);
        assert_eq!(v, Value::Int(1));
    }

    /// A host builtin (`getentarray`, `spawnstruct`) needs to mint a
    /// struct/array and populate it from outside the instruction loop, and
    /// to reach `level`/`game` to notify or read a field on them; all four
    /// go through `Vm::heap`/`heap_mut`/`level`/`game`, not through
    /// crate-internal knowledge of allocation order.
    #[test]
    fn a_host_can_allocate_and_populate_a_struct_and_array_and_reach_level_and_game() {
        let mut vm = Vm::new();
        let hp = vm.interner_mut().intern("hp");
        let s = vm.heap_mut().new_struct();
        vm.heap_mut().set_field(s, hp, Value::Int(3));
        assert_eq!(vm.heap().get_field(s, hp), Value::Int(3));

        let a = vm.heap_mut().new_array();
        vm.heap_mut()
            .set_index(a, crate::heap::ArrayKey::Int(0), Value::Int(9));
        assert_eq!(
            vm.heap().get_index(a, crate::heap::ArrayKey::Int(0)),
            Value::Int(9)
        );

        let mark = vm.interner_mut().intern("mark");
        let level_id = match vm.level() {
            Target::Struct(id) => id,
            Target::Entity(_) => unreachable!("level is always a struct"),
        };
        vm.heap_mut().set_field(level_id, mark, Value::Int(1));
        assert_ne!(vm.level(), vm.game());
    }
}
