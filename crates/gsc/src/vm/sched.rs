//! The frame scheduler. A thread suspends by being left unstepped, which is
//! the whole reason the compiler emits bytecode rather than walking the AST.
//!
//! `threads` stays in start order, which is id order, so a thread is found
//! by binary search. Two indexes stand in for walks of every thread: the
//! threads registered on each notify's (target, event) (`NotifyIndex`) and
//! the scheduler's next pick (`ReadyIndex`). Each holds every thread that
//! could qualify and some that no longer do; a candidate is checked against
//! the thread itself, so the order and the result are the walk's
//! (docs/research/cod4-server-frame.md 9, "The spawn wave").

use std::cmp::Reverse;
use std::rc::Rc;

use crate::atom::Atom;
use crate::bytecode::Function;
use crate::value::{FuncRef, Value};

use super::interp::{Frame, Step, Suspend};
use super::{ErrorKind, Host, ScriptError, Target, Vm};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ThreadId(pub u32);

/// The script clock and every `wait` deadline are 24-bit: they wrap every
/// 16,777,216 ms of `level.time`, about 4.7 hours
/// (docs/research/cod11-gsc-language.md, "The script clock").
pub const CLOCK_MASK: i32 = 0x00ff_ffff;

// `Thread`/`ThreadState` are `pub(crate)`, not `pub`: `Vm` never hands one
// out (only a `ThreadId`, to name a thread without exposing its innards),
// so nothing outside this crate can obtain one to act on.
#[derive(Debug)]
pub(crate) enum ThreadState {
    Runnable,
    /// The script clock reading ([`CLOCK_MASK`]-wrapped) at which this
    /// thread becomes runnable again.
    WaitingUntil(i32),
    /// CoD2's `waittillframeend`, reached at this clock reading: due then,
    /// behind every thread a `wait` or a notify made due then, oldest
    /// first (docs/research/cod2-gsc-diff.md 3.2).
    FrameEnd(i32),
    WaitingNotify {
        target: Target,
        event: Atom,
        binds: Box<[u16]>,
        /// `waittillmatch`'s values, which the notify's first arguments
        /// must equal; empty for `waittill`.
        matches: Box<[Value]>,
        /// When it parked, for CoD2's newest-first wake.
        parked: u64,
    },
}

impl ThreadState {
    /// A parked thread's due time, and whether it parked at
    /// `waittillframeend`.
    fn due(&self) -> Option<(i32, bool)> {
        match *self {
            ThreadState::WaitingUntil(d) => Some((d, false)),
            ThreadState::FrameEnd(d) => Some((d, true)),
            _ => None,
        }
    }
}

/// A thread's place among those due at one clock reading: retail's list
/// for that time takes a `wait` or a notify's wake at its head and a
/// `waittillframeend` at its tail (cod2-gsc-diff.md 3.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Turn {
    Newest(Reverse<u64>),
    FrameEnd(u64),
}

/// [`Vm::thread_index`] over `threads`.
fn thread_index_in(threads: &[Thread], id: ThreadId) -> Option<usize> {
    threads.binary_search_by_key(&id, |t| t.id).ok()
}

/// The threads that registered an endon, or parked a `waittill`, on each
/// (target, event): every one that has, and some that since stopped, which
/// a notify checks against the thread before it acts. A notify reads its
/// own entry instead of walking every thread; the walk was most of CoD4's
/// script time (cod4-server-frame.md 9, "The spawn wave").
#[derive(Default)]
pub(crate) struct NotifyIndex {
    pub endons: crate::fxhash::FxHashMap<(Target, Atom), Vec<ThreadId>>,
    pub waits: crate::fxhash::FxHashMap<(Target, Atom), Vec<ThreadId>>,
    /// Entries pushed since the index was last rebuilt, and the count
    /// that rebuilds it.
    pushed: usize,
    rebuild_at: usize,
}

impl NotifyIndex {
    /// The entries of every thread's endons and `waittill`, which is all a
    /// notify could act on.
    fn rebuild(&mut self, threads: &[Thread]) {
        self.endons.clear();
        self.waits.clear();
        self.pushed = 0;
        for t in threads {
            for &(target, event, _) in &t.endons {
                self.endons.entry((target, event)).or_default().push(t.id);
                self.pushed += 1;
            }
            if let ThreadState::WaitingNotify { target, event, .. } = t.state {
                self.waits.entry((target, event)).or_default().push(t.id);
                self.pushed += 1;
            }
        }
        self.rebuild_at = 2 * self.pushed + 4096;
    }
}

/// The keys `step_runnable` picks by, one pushed each time a thread turns
/// runnable or parks on the clock: a runnable thread's queue stamp, a
/// parked one's due time and turn. A key whose thread has since moved on is
/// dropped when it comes up. The pick was a walk of every thread each step,
/// most of CoD4's script time once the notifies were indexed
/// (cod4-server-frame.md 9, "The spawn wave").
#[derive(Default)]
pub(crate) struct ReadyIndex {
    /// `Runnable` threads, the latest queued first.
    runnable: std::collections::BTreeSet<(Reverse<u64>, ThreadId)>,
    /// `WaitingUntil` and `FrameEnd` threads by due time, then turn.
    timed: std::collections::BTreeSet<(i32, Turn, ThreadId)>,
}

impl ReadyIndex {
    fn rebuild(&mut self, threads: &[Thread]) {
        self.runnable.clear();
        self.timed.clear();
        for t in threads {
            match t.state {
                ThreadState::Runnable => {
                    self.runnable.insert((Reverse(t.seq), t.id));
                }
                ThreadState::WaitingUntil(d) => {
                    self.timed.insert((d, Turn::Newest(Reverse(t.seq)), t.id));
                }
                ThreadState::FrameEnd(d) => {
                    self.timed.insert((d, Turn::FrameEnd(t.seq), t.id));
                }
                ThreadState::WaitingNotify { .. } => {}
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Thread {
    pub id: ThreadId,
    pub frames: Vec<Frame>,
    pub state: ThreadState,
    /// Each with the depth of the call that registered it: under
    /// [`Vm::set_endons_end_with_call`] a return drops its call's.
    pub endons: Vec<(Target, Atom, usize)>,
    /// When it was last queued: among threads due at the same time, the
    /// latest queued runs first (`Vm::step_runnable`).
    pub seq: u64,
}

impl Vm {
    /// Starts a new thread running `func` from its first instruction and
    /// returns its id. The function must already be installed. Runs the
    /// thread immediately, to its own suspend/return/error, before
    /// returning -- see `spawn`. A `wait` the new thread's first
    /// instructions hit counts from the script clock ([`Vm::time`]), which
    /// between frames is the last frame's `level.time`, not the caller's.
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
        func: FuncRef,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> ThreadId {
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
        self.spawn(host, func, f, recv, args, &mut errors)
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
    pub(crate) fn spawn(
        &mut self,
        host: &mut dyn Host,
        func: FuncRef,
        f: Rc<Function>,
        recv: Option<Target>,
        args: Vec<Value>,
        errors: &mut Vec<ScriptError>,
    ) -> ThreadId {
        let frame = self.make_frame(func, f, recv, args);
        let id = ThreadId(self.next_thread);
        self.next_thread += 1;
        let seq = self.queue_seq();
        // `thread_index` searches on it.
        debug_assert!(self.threads.last().is_none_or(|t| t.id < id));
        self.threads.push(Thread {
            id,
            frames: vec![frame],
            state: ThreadState::Runnable,
            endons: Vec::new(),
            seq,
        });
        self.queue_ready(id);
        self.thread_returned = None;
        if self.spawn_depth < Self::MAX_SPAWN_DEPTH {
            self.spawn_depth += 1;
            let returned = self.step_thread(host, id, errors);
            self.spawn_depth -= 1;
            self.thread_returned = returned;
        } else {
            // Silent otherwise, this degrades into exactly the deadlock
            // that running a spawned thread immediately (rather than
            // merely `Runnable`, see this function's doc comment) exists
            // to prevent: two threads left waiting on each other with
            // nothing to say why. A named constant and the function that
            // tripped it is enough to spot "a script nests `thread` this
            // deep" during triage.
            log::warn!(
                "gsc: MAX_SPAWN_DEPTH ({}) exceeded spawning {}::{}; thread left runnable for the next scheduling pass instead of running immediately",
                Self::MAX_SPAWN_DEPTH,
                self.interner.resolve(func.file),
                self.interner.resolve(func.name)
            );
        }
        id
    }

    /// Kills every thread whose `self` is `target`, for a host freeing that
    /// entity: a retail free ends them (`probe_stale_self`,
    /// cod11-gsc-language.md). A thread asleep over any other free is
    /// dropped when it is next picked (`step_runnable`). A thread's `self` is its first frame's receiver; a
    /// frame above it may have called a method on another entity, which
    /// changes nothing about whose thread it is.
    pub fn kill_threads_of(&mut self, target: impl Into<Target>) {
        let target = target.into();
        self.threads
            .retain(|t| t.frames.first().and_then(|f| f.recv) != Some(target));
    }

    /// Kills every thread endon-registered against `(target, event)`, then
    /// wakes every thread waiting on it, binding `args` into the locals the
    /// `waittill` recorded (a missing argument writes `Undefined`). Both
    /// passes walk `threads` in start order, so two waiters on the same
    /// event always wake oldest first, and every endon kill lands before
    /// any notify wake. Waking only flips a thread's state to `Runnable`;
    /// it does not run here, so a notify can never reenter the VM.
    ///
    /// The event folds first: `waittill`/`endon` store the folded atom, and
    /// the name here may have arrived as any spelling of it (atom.rs).
    pub fn notify(&mut self, target: impl Into<Target>, event: Atom, args: &[Value]) {
        self.notify_from(None, target, event, args);
    }

    /// [`Vm::notify`] knowing which thread executed the `notify`, which its
    /// own endons do not kill: a thread that notifies an event it is itself
    /// registered against runs on and resumes past its next `wait`
    /// (`probe_endon_self`). Stock `dm.gsc`'s `endMap` depends on it -- it
    /// runs inside a `Callback_PlayerKilled` thread that opened with
    /// `self endon("spawned")`, calls `spawnIntermission` (which notifies
    /// "spawned") on every player including that one, and only then waits
    /// ten seconds and calls `exitLevel`. An endon fired by any *other*
    /// thread still kills, which is what
    /// `a_thread_killed_by_its_own_endon_mid_step_still_runs_to_its_next_suspend`
    /// measures.
    pub(crate) fn notify_from(
        &mut self,
        from: Option<ThreadId>,
        target: impl Into<Target>,
        event: Atom,
        args: &[Value],
    ) {
        let target = target.into();
        let event = self.interner.fold_atom(event);
        let key = (target, event);
        // The threads with an endon on it, in start order; each checked
        // against its own endons, as a walk of every thread would.
        let mut cands = self.notify_index.endons.remove(&key).unwrap_or_default();
        cands.sort_unstable();
        cands.dedup();
        let mut keep = Vec::new();
        for id in cands {
            let Some(i) = self.thread_index(id) else {
                continue;
            };
            // The outermost call whose endon this is.
            let Some(d) = self.threads[i]
                .endons
                .iter()
                .filter(|&&(t, e, _)| t == target && e == event)
                .map(|e| e.2)
                .min()
            else {
                continue;
            };
            let parked = !self.threads[i].frames.is_empty();
            if Some(id) == from {
                keep.push(id);
            } else if self.endons_end_with_call && d > 1 && parked {
                // CoD4: only that call ends; its caller reads `undefined`
                // from it and runs on (`probe_endon_return`).
                let seq = self.next_seq;
                self.next_seq += 1;
                let t = &mut self.threads[i];
                t.frames.truncate(d - 1);
                t.endons.retain(|e| e.2 < d);
                if let Some(f) = t.frames.last_mut() {
                    f.stack.push(Value::Undefined);
                }
                t.state = ThreadState::Runnable;
                t.seq = seq;
                self.queue_ready(id);
            } else {
                let t = self.threads.remove(i);
                if self.kill_mid_step {
                    self.killed.push(t.id);
                }
            }
        }
        if !keep.is_empty() {
            self.notify_index.endons.insert(key, keep);
        }

        // A waiter binds the arguments into its locals.
        for a in args {
            self.heap.share(*a);
        }
        // Queued newest first, so that the pick, last in first out, resumes
        // them in start order (`probe_notify`, `probe_notify_frame`); on
        // CoD2 queued oldest `waittill` first, so the newest resumes first.
        // The waiters this notify wakes, picked before they are ordered:
        // waking one changes no other's match, and the stable sort keeps
        // their order as it would among all the threads.
        let mut cands = self.notify_index.waits.remove(&key).unwrap_or_default();
        cands.sort_unstable_by(|a, b| b.cmp(a));
        cands.dedup();
        let mut keep = Vec::new();
        let mut order: Vec<usize> = Vec::new();
        let interner = &self.interner;
        for id in cands {
            let Some(i) = thread_index_in(&self.threads, id) else {
                continue;
            };
            let ThreadState::WaitingNotify {
                target: wt,
                event: we,
                matches,
                ..
            } = &self.threads[i].state
            else {
                continue;
            };
            if *wt != target || *we != event {
                continue;
            }
            if matches.iter().enumerate().all(|(k, m)| {
                args.get(k)
                    .is_some_and(|a| super::interp::values_equal(interner, *m, *a).unwrap_or(false))
            }) {
                order.push(i);
            } else {
                keep.push(id);
            }
        }
        if !keep.is_empty() {
            self.notify_index.waits.insert(key, keep);
        }
        if self.newest_waiter_first {
            order.sort_by_key(|&i| match self.threads[i].state {
                ThreadState::WaitingNotify { parked, .. } => parked,
                _ => 0,
            });
        }
        for i in order {
            let t = &mut self.threads[i];
            let ThreadState::WaitingNotify { binds, .. } =
                std::mem::replace(&mut t.state, ThreadState::Runnable)
            else {
                unreachable!("matches_wait only true for WaitingNotify");
            };
            t.seq = self.next_seq;
            self.next_seq += 1;
            let frame = t
                .frames
                .last_mut()
                .expect("a waiting thread always retains its frame stack");
            for (arg_idx, &slot) in binds.iter().enumerate() {
                frame.locals[slot as usize] =
                    args.get(arg_idx).copied().unwrap_or(Value::Undefined);
            }
            let id = t.id;
            self.queue_ready(id);
        }
    }

    /// Pushes thread `id`'s key as its state and stamp now give it
    /// ([`ReadyIndex`]), rebuilding the index once the keys outnumber the
    /// threads well past what a frame's churn leaves behind.
    fn queue_ready(&mut self, id: ThreadId) {
        let Some(i) = self.thread_index(id) else {
            return;
        };
        let t = &self.threads[i];
        let ready = &mut self.ready;
        match t.state {
            ThreadState::Runnable => {
                ready.runnable.insert((Reverse(t.seq), id));
            }
            ThreadState::WaitingUntil(d) => {
                ready.timed.insert((d, Turn::Newest(Reverse(t.seq)), id));
            }
            ThreadState::FrameEnd(d) => {
                ready.timed.insert((d, Turn::FrameEnd(t.seq), id));
            }
            ThreadState::WaitingNotify { .. } => {}
        }
        if ready.runnable.len() + ready.timed.len() > 2 * self.threads.len() + 4096 {
            ready.rebuild(&self.threads);
        }
    }

    /// The thread `step_runnable` picks with the clock at `now` and the
    /// frame's `level.time` at `level`, its key taken out of the index: the
    /// earliest due, among those due together the latest queued, a
    /// `waittillframeend` behind them; a runnable thread counts as due now.
    /// A thread is due on the clock or between it and `level`; one due on
    /// `level` itself waits a frame, and one behind the clock waits for it
    /// to come round again.
    fn next_ready(&mut self, now: i32, level: i32) -> Option<(i32, Turn, ThreadId)> {
        let runnable = loop {
            let Some(&(Reverse(seq), id)) = self.ready.runnable.first() else {
                break None;
            };
            let live = self.thread_index(id).is_some_and(|i| {
                let t = &self.threads[i];
                matches!(t.state, ThreadState::Runnable) && t.seq == seq
            });
            if live {
                break Some((now, Turn::Newest(Reverse(seq)), id));
            }
            self.ready.runnable.pop_first();
        };
        let from = (now, Turn::Newest(Reverse(u64::MAX)), ThreadId(0));
        let timed = loop {
            let Some(&key) = self.ready.timed.range(from..).next() else {
                break None;
            };
            let (d, turn, id) = key;
            let live = self.thread_index(id).is_some_and(|i| {
                let t = &self.threads[i];
                match (t.state.due(), turn) {
                    (Some((due, false)), Turn::Newest(Reverse(seq))) => due == d && t.seq == seq,
                    (Some((due, true)), Turn::FrameEnd(seq)) => due == d && t.seq == seq,
                    _ => false,
                }
            });
            if live {
                break (d == now || d < level).then_some(key);
            }
            self.ready.timed.remove(&key);
        };
        let next = match (runnable, timed) {
            (Some(r), Some(t)) => Some(r.min(t)),
            (r, t) => r.or(t),
        }?;
        match next.1 {
            Turn::Newest(Reverse(seq)) if Some(next) == runnable => {
                self.ready.runnable.remove(&(Reverse(seq), next.2));
            }
            _ => {
                self.ready.timed.remove(&next);
            }
        }
        Some(next)
    }

    /// Counts an entry pushed onto `notify_index`, rebuilding it once the
    /// entries pushed reach twice the live ones of the last rebuild: those
    /// no notify read back (a dead thread's, a freed entity's) would
    /// otherwise pile up.
    pub(crate) fn notify_index_pushed(&mut self) {
        let index = &mut self.notify_index;
        index.pushed += 1;
        if index.pushed > index.rebuild_at.max(4096) {
            index.rebuild(&self.threads);
        }
    }

    /// The index of the thread `id` in `threads`, which holds them in start
    /// order and so in id order.
    pub(crate) fn thread_index(&self, id: ThreadId) -> Option<usize> {
        thread_index_in(&self.threads, id)
    }

    /// Steps the thread `id` -- assumed `Runnable`, a no-op otherwise or if
    /// it no longer exists (see below) -- until it suspends, returns, or
    /// errors, then updates its stored state or removes it, and flushes
    /// any notify a builtin queued along the way (`Op::Notify` resolves
    /// where it executes; neither runs a thread, so a notify can't reenter
    /// the VM). A threaded call reached during the step recursively spawns and
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
    /// The value the thread returned, when this step ran it to its end.
    fn step_thread(
        &mut self,
        host: &mut dyn Host,
        id: ThreadId,
        errors: &mut Vec<ScriptError>,
    ) -> Option<Value> {
        let idx = self.thread_index(id)?;
        if !matches!(self.threads[idx].state, ThreadState::Runnable) {
            return None;
        }

        let mut frames = std::mem::take(&mut self.threads[idx].frames);
        let mut notifies = Vec::new();
        let budget = self.budget;
        let result = self.step_frames(host, &mut frames, budget, Some(id), &mut notifies, errors);
        if self.spawn_depth == 0 {
            self.killed.clear();
        }
        let mut returned = None;

        match result {
            Ok(Step::Returned(v)) => {
                if let Some(idx) = self.thread_index(id) {
                    self.threads.remove(idx);
                }
                returned = Some(v);
            }
            Ok(Step::Suspend(Suspend::Wait { seconds })) => {
                // Rounded half up on the x87's wider product, as retail's
                // `wait` does (docs/research/cod11-gsc-language.md, "The
                // script clock"); the opcode has already refused a negative
                // wait and one past the 24-bit range.
                let delay_ms = if self.frame_waits {
                    let frames = (f64::from(seconds) * 20.0 + 0.5) as i32;
                    let frames = if frames == 0 && seconds > 0.0 {
                        1
                    } else {
                        frames
                    };
                    frames * self.frame_ms
                } else {
                    (f64::from(seconds) * 1000.0 + 0.5) as i32
                };
                let deadline = (self.now_ms + delay_ms) & CLOCK_MASK;
                let seq = self.queue_seq();
                if let Some(idx) = self.thread_index(id) {
                    self.threads[idx].frames = frames;
                    self.threads[idx].state = ThreadState::WaitingUntil(deadline);
                    self.threads[idx].seq = seq;
                    self.queue_ready(id);
                }
            }
            Ok(Step::Suspend(Suspend::FrameEnd)) => {
                let at = self.now_ms;
                let seq = self.queue_seq();
                if let Some(idx) = self.thread_index(id) {
                    self.threads[idx].frames = frames;
                    self.threads[idx].state = ThreadState::FrameEnd(at);
                    self.threads[idx].seq = seq;
                    self.queue_ready(id);
                }
            }
            Ok(Step::Suspend(Suspend::WaitTill {
                target,
                event,
                binds,
                matches,
            })) => {
                if let Some(idx) = self.thread_index(id) {
                    self.notify_index
                        .waits
                        .entry((target, event))
                        .or_default()
                        .push(id);
                    self.notify_index_pushed();
                    self.threads[idx].frames = frames;
                    self.threads[idx].state = ThreadState::WaitingNotify {
                        target,
                        event,
                        binds,
                        matches,
                        parked: self.next_wait,
                    };
                    self.next_wait += 1;
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
                self.record_abort(&e);
                errors.push(e);
                if let Some(idx) = self.thread_index(id) {
                    self.threads.remove(idx);
                }
            }
            Err(e) => {
                self.record_abort(&e);
                errors.push(e);
                if let Some(idx) = self.thread_index(id) {
                    self.threads.remove(idx);
                }
            }
        }

        // Attributed to the thread whose instructions queued them, so its
        // own endons do not kill it (`notify_from`).
        for (target, event, args) in notifies {
            self.notify_from(Some(id), target, event, &args);
        }
        returned
    }

    /// A ceiling on how many thread steps one pass takes. Retail has none: a
    /// thread looping on `wait 0` stalls its frame for good, printing
    /// `WARNING: potential infinite loop in script.` about once a second
    /// (`cod_lnxded` 0x80a5b8c; docs/research/cod11-gsc-language.md). The
    /// cap keeps a server here alive through such a script, and a spawn
    /// bomb, at the cost of leaving the rest of the frame's threads for the
    /// next one. It sits well above what a real frame steps: the stock
    /// scripts step a few hundred at most, and `probe_wait0_loop`'s
    /// thousand `wait 0` turns in one frame have to fit.
    const MAX_THREADS_PER_FRAME: u32 = 10_000;

    fn queue_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// The script clock: the `level.time` of the last frame
    /// [`Vm::run_frame`] ran, or what [`Vm::set_time`] set, wrapped to 24
    /// bits.
    pub fn time(&self) -> i32 {
        self.now_ms
    }

    /// `Scr_InitSystem`'s clock: a level starts the script clock on its own
    /// `level.time`, before the gametype's `main` runs.
    pub fn set_time(&mut self, ms: i32) {
        self.now_ms = ms & CLOCK_MASK;
    }

    /// One server frame's threads, the way `G_RunFrame` runs them
    /// (docs/research/cod11-gsc-language.md, "The script clock"): whatever
    /// is due on the clock the last frame left, then the clock walks up to
    /// `level_ms`, each thread due before it running on its own due time,
    /// so a `wait` it takes counts from there. A thread due at `level_ms`
    /// itself waits for the next frame. A thread a notify woke runs on the
    /// clock of the moment. The errors are collected and returned rather
    /// than propagated, so one bad thread never stops the rest of the
    /// server.
    ///
    /// The clock is 24-bit. On the frame `level.time` wraps it the walk's
    /// target is behind the clock, so the clock jumps there and only the
    /// current millisecond's threads run: one due in the gap waits until a
    /// later walk crosses its due time, hours on, as on retail.
    pub fn run_frame(&mut self, host: &mut dyn Host, level_ms: i32) -> Vec<ScriptError> {
        let errors = self.step_runnable(host, level_ms & CLOCK_MASK);
        // `Scr_SetTime` leaves the clock on `level.time` whether or not it
        // walked forward.
        self.now_ms = level_ms & CLOCK_MASK;
        errors
    }

    /// The first part of [`Vm::run_frame`], `Scr_RunCurrentThreads`: the
    /// threads due on the clock itself and any a notify woke, before the
    /// clock walks.
    pub fn run_current(&mut self, host: &mut dyn Host) -> Vec<ScriptError> {
        self.step_runnable(host, self.now_ms)
    }

    /// The pick both passes share, measured against retail by
    /// `probe_wait0_yield`, `probe_wait0_loop` and `probe_notify_frame`: the
    /// earliest due time first, a notify's wake counting as due now, and
    /// among threads due at the same time the one queued last. A `wait 0`
    /// is due now and queued last, so it resumes at once, ahead of every
    /// other thread due this frame; a notify's waiters are queued newest
    /// first, so they resume in start order after the notifier's step.
    /// `level` is the frame's `level.time`, the end of the clock's walk.
    ///
    /// Nothing runs between frames: a thread a client packet's callback or
    /// command woke waits, `Runnable` under its wake stamp, for the frame's
    /// first pass, and a `wait 0` the callback took is due there too, so
    /// both join that pass's pick with the threads due on the clock
    /// (`client-probes/probe_threadorder`, cod11-gsc-language.md, "The
    /// packet pass").
    fn step_runnable(&mut self, host: &mut dyn Host, level: i32) -> Vec<ScriptError> {
        let mut errors = Vec::new();
        let mut steps = 0;
        while steps < Self::MAX_THREADS_PER_FRAME {
            let now = self.now_ms;
            let next = self.next_ready(now, level);
            let Some((at, _, tid)) = next else {
                return errors;
            };
            self.now_ms = at;
            let Some(idx) = self.thread_index(tid) else {
                continue;
            };
            // Freeing an entity or a hudelem ends every thread whose `self`
            // it is (`probe_stale_self`, `probe_stale_self_hud`); one asleep
            // over the free is dropped here, before it runs a step.
            if let Some(Target::Entity(e)) = self.threads[idx].frames.first().and_then(|f| f.recv)
                && !host.is_live(e)
            {
                self.threads.remove(idx);
                continue;
            }
            self.threads[idx].state = ThreadState::Runnable;
            self.step_thread(host, tid, &mut errors);
            steps += 1;
        }
        // Was silent before: a frame past the cap just stopped simulating the
        // rest with nothing in the log to say why.
        log::warn!(
            "gsc: MAX_THREADS_PER_FRAME ({}) reached in one pass; the threads still due wait for the next",
            Self::MAX_THREADS_PER_FRAME
        );
        errors
    }

    /// Runs `func` to completion on a fresh one-frame stack. A `wait` or
    /// `waittill` it reaches on *this* frame stack is a script error, not
    /// a hang: there is no scheduler here to come back to it later. But a
    /// threaded call inside `func` spawns and immediately runs a real,
    /// independent thread (`spawn`), and that thread's own `wait` is not
    /// an error -- it's an ordinary suspend, resolved into a
    /// `WaitingUntil` deadline the same way `run_frame`/`start_thread`
    /// would, counted from the script clock ([`Vm::time`]).
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
        let mut frames = vec![self.make_frame(func, f, recv, args)];
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
mod tests {
    use crate::atom::Atom;
    use crate::value::{EntId, Value};
    use crate::vm::tests::TestHost;
    use crate::vm::{Cx, ErrorKind, Host, Target, Vm};

    fn vm_with(src: &str) -> Vm {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        vm
    }

    /// `start_thread` hands back a `ThreadId` and has no error channel, so
    /// a thread that dies during a host's map-load bootstrap reached nothing
    /// but `log::warn!`, invisible to a test harness with no logger.
    /// `Vm::aborts` is that missing surface.
    #[test]
    fn a_thread_that_dies_inside_start_thread_is_recorded() {
        let mut vm = vm_with(r#"main() { double("not an int"); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        assert_eq!(vm.aborts().len(), 1, "the dead thread is recorded");
        let e = &vm.aborts()[0];
        assert_eq!(e.kind, ErrorKind::BadType("double wants an int"));
        assert_eq!(
            vm.describe(e),
            "test/script::main:1: BadType(\"double wants an int\")"
        );
    }

    /// Below `developer 1` retail runs on past a runtime error: the failed
    /// instruction's results read undefined and a conditional jump is not
    /// taken, so `if (undefined)` and `if (!undefined)` both take their
    /// branch, and `wait undefined` does not wait
    /// (`client-probes/probe_undef_if`, 1.1d and UO 1.51b).
    #[test]
    fn below_developer_1_a_thread_runs_on_past_an_error() {
        let mut vm = vm_with(
            r#"main() {
                x = 4;
                if (level.unset) { x = 100; } else { mark("else"); }
                mark(x);
                if (!level.unset) { mark("not"); } else { mark("not_else"); }
                mark(isDefined(level.unset + 1));
                mark(isDefined(!level.unset));
                mark(isDefined(level.unset.field));
                mark(isDefined(double("a")));
                wait level.unset;
                mark("end");
            }"#,
        );
        vm.set_recover_errors(true);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        let marks: Vec<Value> = host
            .calls
            .iter()
            .filter(|(n, _)| n == "mark")
            .map(|(_, a)| a[0])
            .collect();
        let mut s = |t: &str| Value::String(vm.interner_mut().intern_exact(t));
        let want = vec![
            Value::Int(100),
            s("not"),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            Value::Int(0),
            s("end"),
        ];
        assert_eq!(marks, want);
        assert_eq!(vm.aborts().len(), 8, "every error is still recorded");
        assert_eq!(vm.thread_count(), 0, "the thread ran to its end");
    }

    /// The record is capped, so a run that blows through it must not hand
    /// back a list that passes for complete: the count keeps going.
    #[test]
    fn the_abort_record_is_capped_but_the_count_is_not() {
        let mut vm = vm_with(r#"main() { double("not an int"); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        let n = Vm::MAX_RECORDED_ABORTS + 44;
        for _ in 0..n {
            vm.start_thread(&mut host, f, None, vec![]);
        }
        assert_eq!(vm.aborts().len(), Vm::MAX_RECORDED_ABORTS);
        assert_eq!(vm.abort_count() as usize, n);
    }

    /// CoD4's frame-counted `wait` against CoD1's milliseconds, taken at
    /// 0 with 50 ms frames: the frame each resumes on, a thread due on a
    /// frame's `level.time` running the frame after. Retail 1.7 woke 0.624
    /// 12 frames on, 0.626 and 0.643 13, 0.01 one and 1.025 (a float under
    /// it) 20 (cod4-gsc.md 4.1).
    #[test]
    fn cod4_waits_count_whole_frames() {
        let resumes = |secs: &str, frames: bool| {
            let mut vm = vm_with(&format!("main() {{ wait {secs}; done(); }}"));
            vm.set_frame_waits(frames);
            let mut host = TestHost::default();
            let f = vm.func_ref("test/script", "main");
            vm.start_thread(&mut host, f, None, vec![]);
            (0..40)
                .map(|k| k * 50)
                .find(|&t| {
                    vm.run_frame(&mut host, t);
                    host.calls.iter().any(|(n, _)| n == "done")
                })
                .unwrap()
        };
        let cod4 = [
            ("0.624", 650),
            ("0.626", 700),
            ("0.643", 700),
            ("0.01", 100),
            ("1.025", 1050),
        ];
        for (secs, at) in cod4 {
            assert_eq!(resumes(secs, true), at, "{secs}");
        }
        // CoD1's milliseconds: 626 and 10 run on the next frame past them.
        assert_eq!(resumes("0.626", false), 650);
        assert_eq!(resumes("0.01", false), 50);
    }

    #[test]
    fn wait_resumes_only_after_its_deadline() {
        let mut vm = vm_with(r#"main() { wait 0.1; done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);

        vm.run_frame(&mut host, 0);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"), "not yet");
        vm.run_frame(&mut host, 100);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "due at 100, which runs the frame after"
        );
        vm.run_frame(&mut host, 150);
        assert!(
            host.calls.iter().any(|(n, _)| n == "done"),
            "resumed on the 150 frame"
        );
        assert_eq!(vm.thread_count(), 0, "and the thread finished");
    }

    #[test]
    fn waittill_binds_the_notify_arguments_into_its_locals() {
        let mut vm = vm_with(
            r#"main() { self waittill("menuresponse", menu, response); got(menu, response); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(4, 0))), vec![]);
        vm.run_frame(&mut host, 0);

        let menu = vm.interner_mut().intern_exact("team_americangerman");
        let resp = vm.interner_mut().intern_exact("allies");
        let ev = vm.interner_mut().intern_folded("menuresponse");
        vm.notify(EntId(4, 0), ev, &[Value::String(menu), Value::String(resp)]);
        vm.run_frame(&mut host, 50);

        let (_, args) = host.calls.iter().find(|(n, _)| n == "got").unwrap();
        assert_eq!(args, &[Value::String(menu), Value::String(resp)]);
    }

    #[test]
    fn a_notify_on_another_entity_does_not_wake_the_waiter() {
        let mut vm = vm_with(r#"main() { self waittill("e"); done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.run_frame(&mut host, 0);

        let ev = vm.interner_mut().intern_folded("e");
        vm.notify(EntId(2, 0), ev, &[]);
        vm.run_frame(&mut host, 50);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
        assert_eq!(vm.thread_count(), 1, "still waiting");
    }

    #[test]
    fn a_thread_started_after_a_notify_neither_dies_nor_wakes_on_it() {
        // `probe_endon_after_notify`: hq.gsc restarts its timer thread this way.
        let mut vm = vm_with(
            r#"main() { level notify("x"); level thread a(); level thread b(); }
               a() { level endon("x"); wait 1; done(); }
               b() { level waittill("x"); woke(); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        vm.run_frame(&mut host, 2000);
        assert!(host.calls.iter().any(|(n, _)| n == "done"));
        assert!(!host.calls.iter().any(|(n, _)| n == "woke"));
        assert_eq!(vm.thread_count(), 1, "b still waits");
    }

    /// An `endon` run inside a called function, notified after it
    /// returned: CoD4's caller lives on (the call's endons went with it),
    /// CoD1's and CoD2's dies. One notified while the call still waits
    /// ends that call alone on CoD4, the caller running on at once
    /// (retail 1.7's `probe_endon_return`, cod4-gsc.md 4.2).
    #[test]
    fn cod4_endons_end_with_their_call() {
        let run = |scoped: bool| {
            let mut vm = vm_with(
                r#"main() { helper(); wait 1; returned(); inner(); done(); }
                   helper() { self endon("stop"); }
                   inner() { self endon("stop2"); wait 1; }"#,
            );
            vm.set_endons_end_with_call(scoped);
            let mut host = TestHost::default();
            let f = vm.func_ref("test/script", "main");
            vm.start_thread(&mut host, f, Some(Target::Entity(EntId(7, 0))), vec![]);
            vm.run_frame(&mut host, 0);
            let stop = vm.interner_mut().intern_folded("stop");
            vm.notify(EntId(7, 0), stop, &[]);
            vm.run_frame(&mut host, 1050);
            let stop2 = vm.interner_mut().intern_folded("stop2");
            vm.notify(EntId(7, 0), stop2, &[]);
            vm.run_frame(&mut host, 1100);
            let called = |n: &str| host.calls.iter().any(|(c, _)| c == n);
            (called("returned"), called("done"), vm.thread_count())
        };
        assert_eq!(run(true), (true, true, 0));
        assert_eq!(run(false), (false, false, 0));
    }

    #[test]
    fn endon_kills_a_waiting_thread() {
        let mut vm = vm_with(r#"main() { self endon("death"); wait 1; done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(7, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1);

        let ev = vm.interner_mut().intern_folded("death");
        vm.notify(EntId(7, 0), ev, &[]);
        vm.run_frame(&mut host, 2000);
        assert_eq!(vm.thread_count(), 0, "killed, not resumed");
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
    }

    /// The failure this whole case-folding split has to not cause: string
    /// values are case-sensitive now, so the two event-name literals below
    /// are two atoms, and only `Interner::fold_atom` at the op keeps them
    /// matching. If it stopped, the waiter would hang forever and nothing
    /// would error. `brecourt.gsc` ships exactly this shape.
    ///
    /// `pump` compiles first on purpose: that makes `"explode"` the
    /// canonical atom and the `waittill`'s `"EXPLODE"` a second one. With
    /// the two the other way round the `waittill` matches whether or not
    /// `Op::WaitTill` folds, and this test cannot fail under the mutation
    /// it names.
    #[test]
    fn a_notify_wakes_a_waittill_that_spelled_the_event_differently() {
        let mut vm = vm_with(
            r#"pump() { self notify("explode"); } main() { self waittill("EXPLODE"); done(); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "main");
        let pump = vm.func_ref("test/script", "pump");
        vm.start_thread(&mut host, waiter, Some(Target::Entity(EntId(3, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1, "waiting");

        vm.start_thread(&mut host, pump, Some(Target::Entity(EntId(3, 0))), vec![]);
        vm.run_frame(&mut host, 50);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "woken");
    }

    /// The same rule from the host side: a host interning an event name
    /// case-sensitively still reaches a `waittill` and an `endon` spelled
    /// the other way. `dier`'s `"PLAYERCONNECT"` is deliberately not the
    /// canonical spelling -- `waiter` compiles first and claims it -- so the
    /// kill below lands only if `Op::EndOn` folds.
    #[test]
    fn a_host_notify_folds_the_event_name_it_was_given() {
        let mut vm = vm_with(
            r#"waiter() { self waittill("PlayerConnect"); done(); }
               dier() { self endon("PLAYERCONNECT"); wait 1; survived(); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "waiter");
        let dier = vm.func_ref("test/script", "dier");
        vm.start_thread(&mut host, waiter, Some(Target::Entity(EntId(5, 0))), vec![]);
        vm.start_thread(&mut host, dier, Some(Target::Entity(EntId(5, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 2);

        let ev = vm.interner_mut().intern_exact("playerCONNECT");
        vm.notify(EntId(5, 0), ev, &[]);
        vm.run_frame(&mut host, 2000);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "waiter woken");
        assert!(
            !host.calls.iter().any(|(n, _)| n == "survived"),
            "endon killed the other thread"
        );
        assert_eq!(vm.thread_count(), 0);
    }

    #[test]
    fn a_threaded_call_runs_independently_of_its_caller() {
        let mut vm = vm_with(
            "main() { thread child(); parent_done(); } child() { wait 0.1; child_done(); }",
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "parent_done"),
            "caller did not block"
        );
        assert!(!host.calls.iter().any(|(n, _)| n == "child_done"));
        vm.run_frame(&mut host, 150);
        assert!(host.calls.iter().any(|(n, _)| n == "child_done"));
    }

    #[test]
    fn a_runaway_loop_aborts_its_thread_instead_of_hanging() {
        // The leading `wait 0;` moves the loop off `start_thread`'s own
        // immediate run (whose budget errors have nowhere to surface,
        // since `start_thread` returns only a `ThreadId`) and onto
        // `run_frame`'s: it's `run_frame`'s returned errors this test
        // pins, so the loop has to run under `run_frame`, not at spawn
        // time.
        let mut vm = vm_with("main() { wait 0; while(1) { spin(); } }");
        vm.set_budget(10_000);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        let errs = vm.run_frame(&mut host, 0);
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].kind, crate::vm::ErrorKind::Budget));
        assert_eq!(vm.thread_count(), 0, "aborted, not left runnable");
    }

    #[test]
    fn unbounded_recursion_aborts_its_thread_instead_of_overflowing_the_stack() {
        // Plain (non-`thread`) recursion: `recurse()` calling itself pushes
        // a new `Frame` onto the same thread's frame stack on every call,
        // never the native Rust stack, so this terminates via the budget
        // rather than a stack overflow. `main`'s leading `wait 0;` is the
        // same trick as above, to get the budget error into `run_frame`'s
        // returned errors instead of a discarded `start_thread` one — the
        // wait has to be outside the recursion itself, since a `wait`
        // inside `recurse` would just suspend on the second call instead
        // of ever exhausting the budget.
        let mut vm = vm_with("main() { wait 0; recurse(); } recurse() { recurse(); }");
        vm.set_budget(10_000);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        let errs = vm.run_frame(&mut host, 0);
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].kind, crate::vm::ErrorKind::Budget));
        assert_eq!(vm.thread_count(), 0, "aborted, not left runnable");
    }

    #[test]
    fn an_erroring_thread_does_not_stop_its_sibling() {
        // Leading `wait 0;` for the same reason as the budget tests above:
        // `bad`/`good` need to be spawned while `run_frame` is stepping
        // `main`, so `bad`'s error lands in `run_frame`'s returned errors
        // rather than in a `start_thread`-local vec nobody reads.
        let mut vm = vm_with(
            r#"main() { wait 0; thread bad(); thread good(); } bad() { double("no"); } good() { ok(); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        let errs = vm.run_frame(&mut host, 0);
        assert_eq!(errs.len(), 1);
        assert!(host.calls.iter().any(|(n, _)| n == "ok"));
    }

    /// Two threads waiting on the same event both wake, oldest first.
    #[test]
    fn notify_wakes_every_waiter_in_start_order() {
        let mut vm = vm_with(
            r#"a() { self waittill("e"); first(); } b() { self waittill("e"); second(); }"#,
        );
        let mut host = TestHost::default();
        let fa = vm.func_ref("test/script", "a");
        let fb = vm.func_ref("test/script", "b");
        vm.start_thread(&mut host, fa, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.start_thread(&mut host, fb, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        let ev = vm.interner_mut().intern_folded("e");
        vm.notify(EntId(1, 0), ev, &[]);
        vm.run_frame(&mut host, 50);
        let order: Vec<&str> = host.calls.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            order.iter().position(|n| *n == "first"),
            order.iter().position(|n| *n == "second").map(|i| i - 1)
        );
    }

    /// CoD2's order (`Vm::set_newest_waiter_first`): the newest `waittill`
    /// resumes first, so two looping waiters swap turns each notify, as
    /// mp_toujane's `connecting` waiters did live (cod2-protocol.md 15.5).
    #[test]
    fn cod2_notify_wakes_the_newest_waittill_first() {
        let mut vm = vm_with(
            r#"a() { for(;;) { self waittill("e"); first(); } }
               b() { for(;;) { self waittill("e"); second(); } }"#,
        );
        vm.set_newest_waiter_first(true);
        let mut host = TestHost::default();
        let fa = vm.func_ref("test/script", "a");
        let fb = vm.func_ref("test/script", "b");
        vm.start_thread(&mut host, fa, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.start_thread(&mut host, fb, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        let ev = vm.interner_mut().intern_folded("e");
        for at in [50, 100] {
            vm.notify(EntId(1, 0), ev, &[]);
            vm.run_frame(&mut host, at);
        }
        let order: Vec<&str> = host.calls.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(order, ["second", "first", "first", "second"]);
    }

    /// CoD2's `x = thread f()` reads what `f` returned when it ran to its
    /// end without suspending, undefined once it waits (retail 1.3,
    /// cod2-protocol.md 15.7; `_fx::soundfx` writes a field on it).
    #[test]
    fn cod2_thread_call_reads_an_unsuspended_return() {
        let src = r#"main() { r = thread seven(); got(r); w = thread later(); got(w); }
seven() { return 7; }
later() { wait 0.05; return 9; }"#;
        for (on, first) in [(true, Value::Int(7)), (false, Value::Undefined)] {
            let ast = crate::parse::parse_file_dialect(src, crate::Dialect::Cod2).unwrap();
            let mut vm = Vm::new();
            let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
            vm.install(fns).unwrap();
            vm.set_thread_returns(on);
            let mut host = TestHost::default();
            let f = vm.func_ref("test/script", "main");
            vm.start_thread(&mut host, f, None, vec![]);
            vm.run_frame(&mut host, 0);
            let got: Vec<Value> = host.calls.iter().map(|(_, a)| a[0]).collect();
            assert_eq!(got, [first, Value::Undefined]);
        }
    }

    /// `animscripts/predict.gsc:102-108`'s notetrack-prediction pattern
    /// (246 corpus sites pair a `thread` statement with a `notify` within
    /// three lines): a spawned thread must reach its own `waittill` before
    /// its spawner's `notify` two lines later, or the notify is missed and
    /// the spawned thread's `waittill` — and its spawner's own later wait
    /// for the reply — hang forever. `main` spawns `getNotetrack`, which
    /// must register its `waittill("go")` before `main`'s `notify("go")`
    /// runs; `getNotetrack` then wakes and replies, and `main` must
    /// eventually see that reply too. If either registration lost the
    /// race, `got()` would never be called and this test would hang
    /// instead of completing.
    #[test]
    fn a_spawned_thread_registers_its_waittill_before_its_spawner_can_notify_past_it() {
        let mut vm = vm_with(
            r#"main() {
                self thread getNotetrack();
                self notify("go");
                self waittill("reply");
                got();
            }
            getNotetrack() {
                self waittill("go");
                self notify("reply");
            }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);
        // Neither wake runs inline (a notify never reenters the VM): one
        // frame steps `getNotetrack` past its wait to fire "reply", the
        // next steps `main` past its own wait to run `got()`. Nothing in
        // this script waits on the clock, so the `now_ms` value doesn't
        // matter here.
        vm.run_frame(&mut host, 0);
        vm.run_frame(&mut host, 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "got"),
            "resolved instead of deadlocking"
        );
        assert_eq!(vm.thread_count(), 0);
    }

    /// Pins the id-watermark rescan in `run_frame` together with kill-
    /// before-wake ordering and the notify-flush boundary: `killer`'s
    /// in-script `notify` (not a host-driven one between frames, like
    /// every other notify test here) kills `victim` — an *earlier* id
    /// than `killer`'s own — mid-`run_frame`-pass, and `witness` — a
    /// *later* id than either — must still run in that same pass rather
    /// than being skipped because `victim`'s removal shifted indices out
    /// from under a cached one.
    #[test]
    fn an_in_script_notify_kills_an_earlier_waiter_without_skipping_a_later_thread() {
        let mut vm = vm_with(
            r#"victim() { level endon("die"); wait 5; }
            killer() { wait 0; level notify("die"); }
            witness() { wait 0; seen(); }"#,
        );
        let mut host = TestHost::default();
        let victim = vm.func_ref("test/script", "victim");
        let killer = vm.func_ref("test/script", "killer");
        let witness = vm.func_ref("test/script", "witness");
        // Each immediately suspends on its own leading `wait`, so all
        // three are `WaitingUntil` before `run_frame` ever runs; the
        // interesting part (the in-script kill) happens on their *second*
        // step, driven by `run_frame`'s own watermark walk.
        vm.start_thread(&mut host, victim, None, vec![]);
        vm.start_thread(&mut host, killer, None, vec![]);
        vm.start_thread(&mut host, witness, None, vec![]);

        vm.run_frame(&mut host, 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "seen"),
            "witness still ran"
        );
        assert_eq!(vm.thread_count(), 0);
    }

    /// A notify from a later-started thread wakes an earlier-started waiter
    /// inside the same frame (`probe_notify_frame`): the stock killcam's
    /// `waittill("end_killcam")` resumes on the frame its own
    /// `waitKillcamTime` child notifies it.
    #[test]
    fn a_later_thread_s_notify_resumes_an_earlier_waiter_in_the_same_frame() {
        let mut vm = vm_with(
            r#"waiter() { level waittill("go"); seen(); }
            notifier() { wait 1; level notify("go"); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "waiter");
        let notifier = vm.func_ref("test/script", "notifier");
        vm.start_thread(&mut host, waiter, None, vec![]);
        vm.start_thread(&mut host, notifier, None, vec![]);
        vm.run_frame(&mut host, 1050);
        assert!(
            host.calls.iter().any(|(n, _)| n == "seen"),
            "the waiter was left for the next pass"
        );
        assert_eq!(vm.thread_count(), 0);
    }

    /// A `wait 0` reached inside a frame's pass resumes before the frame is
    /// over (`probe_wait0_yield`); one taken between frames waits for the next.
    #[test]
    fn a_wait_0_inside_the_frame_resumes_in_the_same_frame() {
        let mut vm = vm_with(r#"f() { wait 1; wait 0; seen(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "f");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 1050);
        assert!(host.calls.iter().any(|(n, _)| n == "seen"));

        let mut vm = vm_with(r#"g() { wait 0; seen(); }"#);
        let mut host = TestHost::default();
        let g = vm.func_ref("test/script", "g");
        vm.set_time(1000);
        vm.start_thread(&mut host, g, None, vec![]);
        assert!(host.calls.is_empty(), "the wait 0 ran between frames");
        vm.run_frame(&mut host, 1050);
        assert!(host.calls.iter().any(|(n, _)| n == "seen"));
    }

    /// A thread whose first act is spawning another, unboundedly, recurses
    /// through the native Rust stack (`spawn` -> `step_thread` ->
    /// `step_frames` -> `spawn` -> ...), not the `Vec<Frame>` a plain
    /// recursive `Call` grows, and each nested spawn gets its own fresh
    /// `budget`, so the instruction budget alone would never catch this —
    /// only `spawn`'s own `MAX_SPAWN_DEPTH` cap does. If this test doesn't
    /// crash the process outright, the cap is working; the assertions
    /// pin exactly how it degrades: the infinite nest nonetheless unwinds
    /// (everything below the cap runs to completion), leaving one
    /// dangling thread for the next scheduling pass to continue the chain
    /// from, so the live thread count never grows past that.
    #[test]
    fn a_spawn_bomb_is_capped_by_native_stack_depth_not_left_to_overflow_it() {
        let mut vm = vm_with("main() { thread main(); }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        assert_eq!(vm.thread_count(), 1);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1, "still bounded after another pass");
    }

    /// An `endon` registered earlier in a step must be visible to a notify
    /// fired by a thread spawned *later in that same step*, not just to
    /// notifies that arrive after the step returns: `main` registers its
    /// endon, then threads `killer`, which runs immediately (a threaded
    /// call always runs its callee to its own suspend/return before the
    /// caller's step continues, see `Vm::spawn`) and notifies the very
    /// event `main` just registered against, all before `main`'s own step
    /// ever reaches `wait 5`. Were the endon visible only once `main`
    /// suspends, the notify would find no waiter, and `main` would wake up
    /// 5 "ms" later and run `done()` instead of having been killed
    /// immediately.
    #[test]
    fn an_endon_registered_before_a_nested_spawn_is_visible_to_that_spawns_own_notify() {
        let mut vm = vm_with(
            r#"main() { self endon("die"); self thread killer(); wait 5; done(); }
            killer() { self notify("die"); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);
        assert_eq!(
            vm.thread_count(),
            0,
            "killed before its wait, not left alive"
        );
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
    }

    /// `start_thread`'s immediate run can error just as easily as a
    /// `run_frame`-driven one can; this pins that path directly (the
    /// thread is removed, not left dangling or panicking). The budget and
    /// recursion tests above route their error through `run_frame` instead
    /// (their leading `wait 0;`), for the unrelated reason that
    /// `start_thread` has nowhere to return the error to.
    #[test]
    fn a_thread_that_errors_during_start_threads_immediate_run_is_removed() {
        let mut vm = vm_with(r#"main() { double("no"); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        assert_eq!(
            vm.thread_count(),
            0,
            "removed, not left dangling after its own error"
        );
    }

    /// A plain (non-threaded) call whose callee suspends on `wait` leaves
    /// the caller's frame mid-call across the suspend -- exactly the case
    /// `Frame::stack_effect_check` (vm.rs, test-only) has to survive rather
    /// than lose track of, since the caller and callee frames persist
    /// through separate `step_frames` invocations on either side of the
    /// wait. This would panic on a wrong `stack_effect` entry or a bug in
    /// that check itself; it existing and passing is the regression test.
    #[test]
    fn a_plain_call_whose_callee_waits_resumes_and_returns_to_its_caller() {
        let mut vm = vm_with("main() { helper(); done(); } helper() { wait 1; }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"), "not yet");
        vm.run_frame(&mut host, 1050);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "resumed");
        assert_eq!(vm.thread_count(), 0);
    }

    /// `Frame::func_rc` is cached at frame creation and carried across a
    /// suspend rather than rebuilt on resume. The test above only ever
    /// resumes into the same two functions it suspended in; here the
    /// resumed thread calls a *third* one, with a different parameter and
    /// local count, so a frame that reused the suspended frame's cached
    /// `Rc<Function>` would run the wrong code and drop the argument.
    #[test]
    fn a_resumed_thread_calling_a_different_function_gets_that_functions_code() {
        let mut vm = vm_with(
            "main() { waiter(); other(7); } \
             waiter() { wait 1; } \
             other(n) { seen(n); }",
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "seen"),
            "still waiting"
        );

        vm.run_frame(&mut host, 1050);
        let seen = host
            .calls
            .iter()
            .find(|(n, _)| n == "seen")
            .expect("other() ran after the resume");
        assert_eq!(
            seen.1,
            vec![Value::Int(7)],
            "its own parameter, not waiter's"
        );
        assert_eq!(vm.thread_count(), 0);
    }

    /// Two threads waiting on one event on one target wake in the order
    /// they were started: retail logs `first` before `second`
    /// (tests/fixtures/semantics/retail-captures.txt, `# probe_notify`).
    /// vcod gets this from `Vm::notify` walking `threads`, which is kept in
    /// start order; pinned here so a change to that walk fails loudly.
    #[test]
    fn two_waiters_on_one_event_wake_in_start_order() {
        let mut vm = vm_with(
            r#"waiter(tag) { level waittill("probe_event"); got(tag); }
            pump() { level notify("probe_event"); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "waiter");
        let pump = vm.func_ref("test/script", "pump");
        vm.start_thread(&mut host, waiter, None, vec![Value::Int(1)]);
        vm.start_thread(&mut host, waiter, None, vec![Value::Int(2)]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 2, "both waiting");

        vm.start_thread(&mut host, pump, None, vec![]);
        vm.run_frame(&mut host, 0);

        let seen: Vec<Value> = host
            .calls
            .iter()
            .filter(|(n, _)| n == "got")
            .map(|(_, args)| args[0])
            .collect();
        assert_eq!(seen, vec![Value::Int(1), Value::Int(2)]);
    }

    /// Two `notify`s of the same event in one step: the first wakes the
    /// waiter, which does not run before the notifier's step ends, so the
    /// second finds nobody and is lost, and the waiter reads the first's
    /// value. Retail does the same (`probe_notify_twice`,
    /// docs/research/cod11-gsc-language.md section 9).
    #[test]
    fn two_notifies_of_the_same_event_in_one_step_lose_the_second() {
        let mut vm = vm_with(
            r#"waiter() { self waittill("tick", v); got(v); self waittill("tick", v); got(v); }
            pump() { self notify("tick", 1); self notify("tick", 2); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "waiter");
        let pump = vm.func_ref("test/script", "pump");
        vm.start_thread(&mut host, waiter, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1, "waiting on the first tick");

        vm.start_thread(&mut host, pump, Some(Target::Entity(EntId(1, 0))), vec![]);
        vm.run_frame(&mut host, 0);

        let seen: Vec<Value> = host
            .calls
            .iter()
            .filter(|(n, _)| n == "got")
            .map(|(_, args)| args[0])
            .collect();
        assert_eq!(seen, vec![Value::Int(1)], "only the first tick was seen");
        assert_eq!(
            vm.thread_count(),
            1,
            "waiting forever on a second tick that already happened"
        );
    }

    // --- Divergences kept as documentation
    // (docs/research/cod11-gsc-language.md §10), pinned so a later change to
    // notify/kill ordering is deliberate rather than accidental. ---

    /// A thread that notifies an event it has itself registered an `endon`
    /// for is not killed by it: it runs on and resumes past its next
    /// `wait`. Measured against retail by `probe_endon_self`, and stock
    /// `dm.gsc`'s `endMap` is the path that needs it (`notify_from`).
    #[test]
    fn a_thread_survives_notifying_its_own_endon() {
        let mut vm = vm_with(
            r#"main() {
                self endon("die");
                self notify("die");
                sideEffectA();
                wait 5;
                done();
            }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);

        assert_eq!(vm.thread_count(), 1, "its own notify killed it");
        assert!(host.calls.iter().any(|(n, _)| n == "sideeffecta"));
        for frame in 1..=200 {
            vm.run_frame(&mut host, frame * 50);
        }
        assert!(
            host.calls.iter().any(|(n, _)| n == "done"),
            "it never resumed past its own wait"
        );
    }

    /// A thread killed by its own `endon` mid-step (via a nested spawn's
    /// notify, run synchronously inside `main`'s own `Op::Call{threaded:
    /// true}`) keeps executing to its next suspend: the kill removes it from
    /// `self.threads`, but the `step_frames` call already running its
    /// `frames` has no way to notice mid-instruction, so it runs on. `main`
    /// registers its endon, threads `killer` (which notifies "die"
    /// synchronously, killing `main`'s entry in `self.threads` before
    /// `main`'s own step ever returns to the outer loop), then still runs
    /// both side effects and reaches its own `wait 5` before the outer
    /// `step_thread` looks for `main`'s entry again to write back the
    /// suspended frames -- finds it already gone -- and simply drops them:
    /// the side effects already ran for real, but `main` never resumes to
    /// call `done()`.
    #[test]
    fn a_thread_killed_by_its_own_endon_mid_step_still_runs_to_its_next_suspend() {
        let mut vm = vm_with(
            r#"main() {
                self endon("die");
                self thread killer();
                sideEffectA();
                sideEffectB();
                wait 5;
                done();
            }
            killer() { self notify("die"); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);

        assert_eq!(vm.thread_count(), 0, "killed by its own endon mid-step");
        assert!(host.calls.iter().any(|(n, _)| n == "sideeffecta"));
        assert!(host.calls.iter().any(|(n, _)| n == "sideeffectb"));
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "never resumed past its own wait to reach this"
        );
    }

    /// CoD2 (`Vm::set_kill_mid_step`): the same nested notify ends the
    /// spawner before its next instruction, as retail's stock tdm showed
    /// (cod2-protocol.md 16.6).
    #[test]
    fn cod2_a_thread_killed_by_a_nested_spawn_s_notify_stops_at_once() {
        let mut vm = vm_with(
            r#"main() {
                self endon("die");
                self thread killer();
                sideEffectA();
            }
            killer() { self notify("die"); after(); }"#,
        );
        vm.set_kill_mid_step(true);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1, 0))), vec![]);
        assert_eq!(vm.thread_count(), 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "after"),
            "the notifier runs on"
        );
        assert!(!host.calls.iter().any(|(n, _)| n == "sideeffecta"));
    }

    /// A thread looping on `wait 0` is picked again after every turn, so it
    /// takes the whole pass; retail stalls the frame on it for good, and the
    /// cap here ends the pass instead, the other threads due waiting for the
    /// next one.
    #[test]
    fn a_wait_0_spin_ends_the_pass_at_the_cap() {
        let mut vm = vm_with(
            r#"main() { wait 0.05; done(); }
            spin() { wait 0.05; for (;;) wait 0; }"#,
        );
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        let spin = vm.func_ref("test/script", "spin");
        vm.start_thread(&mut host, main, None, vec![]);
        vm.start_thread(&mut host, spin, None, vec![]);
        vm.run_frame(&mut host, 100);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
        assert_eq!(vm.thread_count(), 2);
    }

    /// A level's script clock starts on its `level.time` (`set_time`), and
    /// a thread due at a frame's `level.time` runs the frame after
    /// (`probe_startclock`: a `wait 1` from `Callback_StartGameType` at 0
    /// wakes at 1050). `f`'s `wait 1` taken at 5_000 is due at 6_000, so
    /// the 6_000 frame leaves it and the 6_050 one runs it.
    #[test]
    fn a_wait_counts_from_the_script_clock_and_wakes_after_its_due_frame() {
        let mut vm = vm_with("main() { thread f(); } f() { wait 1; done(); }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.set_time(5_000);
        vm.start_thread(&mut host, f, None, vec![]);
        assert_eq!(vm.thread_count(), 1, "f is alive, waiting on its own wait");

        vm.run_frame(&mut host, 5_000);
        vm.run_frame(&mut host, 6_000);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "due at 6_000, which runs the frame after"
        );
        vm.run_frame(&mut host, 6_050);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "due now");
    }

    /// `probe_startclock`'s settle frames: frames 100 ms apart walk the
    /// clock through every 50 ms step, and a thread resumed on its due
    /// time counts its next `wait` from there, so a `wait 0.05` loop
    /// started at 0 logs 100, 200, 200, 300, 300, then one per 50 ms frame.
    #[test]
    fn a_frame_walks_the_clock_through_each_due_time() {
        let mut vm = vm_with("f() { for (;;) { wait 0.05; tick(); } }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "f");
        vm.start_thread(&mut host, f, None, vec![]);
        let mut seen = Vec::new();
        for level in [100, 200, 300, 350, 400] {
            let before = host.calls.len();
            vm.run_frame(&mut host, level);
            seen.extend(std::iter::repeat_n(level, host.calls.len() - before));
        }
        assert_eq!(seen, [100, 200, 200, 300, 300, 350, 400]);
    }

    /// The frame `level.time` crosses 2^24 ms: the walk's target lands
    /// behind the 24-bit clock, so the clock jumps to it and a thread due in
    /// the gap (16,777,210 here, and 9, the masked 16,777,225) waits for a
    /// later walk to cross it, hours on. A `wait 0.05` due on the target
    /// itself runs the frame after, and its next one counts on the wrapped
    /// clock.
    #[test]
    fn the_wrap_frame_strands_the_threads_due_in_its_gap() {
        const WRAP: i32 = 1 << 24;
        let mut vm = vm_with(
            "a() { wait 0.01; gap(); } b() { wait 0.025; gap(); } \
             c() { wait 0.05; tick(); wait 0.05; tick(); }",
        );
        let mut host = TestHost::default();
        vm.set_time(WRAP - 16);
        assert_eq!(vm.time(), WRAP - 16);
        for name in ["a", "b", "c"] {
            let f = vm.func_ref("test/script", name);
            vm.start_thread(&mut host, f, None, vec![]);
        }
        let count = |host: &TestHost, n: &str| host.calls.iter().filter(|(c, _)| c == n).count();
        vm.run_frame(&mut host, WRAP + 34);
        assert_eq!(vm.time(), 34);
        assert_eq!((count(&host, "gap"), count(&host, "tick")), (0, 0));
        for level in [WRAP + 84, WRAP + 134, WRAP + 184] {
            vm.run_frame(&mut host, level);
        }
        assert_eq!(
            count(&host, "gap"),
            0,
            "stranded until the clock comes round"
        );
        assert_eq!(count(&host, "tick"), 2);
        // Each runs when a walk next crosses its due time: 16,777,210 at the
        // end of this turn, 9 only once the clock has wrapped again.
        vm.run_frame(&mut host, 2 * WRAP - 1);
        assert_eq!(count(&host, "gap"), 1);
        vm.run_frame(&mut host, 2 * WRAP + 5);
        assert_eq!(count(&host, "gap"), 1);
        vm.run_frame(&mut host, 2 * WRAP + 55);
        assert_eq!(count(&host, "gap"), 2);
    }

    /// `wait` refuses what retail's opcode refuses: a negative wait and one
    /// of 16777 s or more, past what the 24-bit clock holds.
    #[test]
    fn wait_refuses_negative_and_overlong_waits() {
        for (src, msg) in [
            (
                "main() { wait -0.5; }",
                "negative wait of -0.5 is not allowed",
            ),
            (
                "main() { wait 16777; }",
                "wait of 16777 seconds is too long",
            ),
        ] {
            let mut vm = vm_with(src);
            let mut host = TestHost::default();
            let f = vm.func_ref("test/script", "main");
            vm.start_thread(&mut host, f, None, vec![]);
            assert_eq!(vm.aborts().len(), 1, "{src}");
            assert_eq!(vm.aborts()[0].kind, ErrorKind::Custom(msg.into()));
        }
        let mut vm = vm_with("main() { wait 16776.999; done(); }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        assert!(vm.aborts().is_empty());
        assert_eq!(vm.thread_count(), 1);
    }

    /// A thread spawned by a `call_now`-driven script (a host callback
    /// like `CodeCallback_PlayerConnect`, say) is a real, independent
    /// thread, not the throwaway one-shot stack `call_now` itself runs
    /// on -- its own `wait` is an ordinary suspend, not an immediate-call
    /// error, and counts from the script clock. `f`'s `wait 1` taken at
    /// 5_000 is due at 6_000 and runs on the frame after it.
    #[test]
    fn call_now_threads_a_wait_against_the_script_clock() {
        let ast =
            crate::parse::parse_file("main() { thread f(); } f() { wait 1; done(); }").unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.set_time(5_000);
        vm.call_now(&mut host, main, None, vec![]).unwrap();
        assert_eq!(vm.thread_count(), 1, "f is alive, waiting on its own wait");

        vm.run_frame(&mut host, 6_000);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "not due yet -- 6_000, not 1_000"
        );
        vm.run_frame(&mut host, 6_050);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "due now");
    }

    /// A host whose builtins start threads: `spawnkiller` runs `killed` on
    /// its own receiver, `again` re-enters itself through `bomb`, and
    /// `spawnmissing` names a function nothing installed.
    #[derive(Default)]
    struct SpawnHost(TestHost);

    impl Host for SpawnHost {
        fn builtin(
            &mut self,
            cx: &mut Cx,
            name: Atom,
            recv: Option<Target>,
            _args: &[Value],
        ) -> Result<Value, ErrorKind> {
            match cx.resolve_folded(name) {
                "spawnkiller" => {
                    let f = cx.func_ref("test/script", "killed");
                    cx.spawn(f, recv, vec![]);
                    Ok(Value::Undefined)
                }
                "again" => {
                    let f = cx.func_ref("test/script", "bomb");
                    cx.spawn(f, recv, vec![]);
                    Ok(Value::Undefined)
                }
                "spawnmissing" => {
                    let f = cx.func_ref("test/script", "nosuchthing");
                    cx.spawn(f, recv, vec![]);
                    Ok(Value::Undefined)
                }
                "spawnkillerthen" => {
                    let f = cx.func_ref("test/script", "killed");
                    cx.spawn_then(f, recv, vec![], 1);
                    let f = cx.func_ref("test/script", "nosuchthing");
                    cx.spawn_then(f, recv, vec![], 2);
                    Ok(Value::Undefined)
                }
                "spawnchain" => {
                    let f = cx.func_ref("test/script", "step");
                    cx.spawn_then(f, recv, vec![Value::Int(1)], 10);
                    let f = cx.func_ref("test/script", "step");
                    cx.spawn(f, recv, vec![Value::Int(9)]);
                    Ok(Value::Undefined)
                }
                _ => Err(ErrorKind::MissingBuiltin(name)),
            }
        }

        /// Appends `<token>:<level.state>` to `level.returned`. A token of
        /// 10 or 11 chains one more `step` with the next token, up to 12.
        fn spawn_returned(&mut self, cx: &mut Cx, token: u32) {
            if (10..12).contains(&token) {
                let f = cx.func_ref("test/script", "step");
                let (n, level) = (token as i32 - 8, cx.level());
                cx.spawn_then(f, Some(level), vec![Value::Int(n)], token + 1);
            }
            let Target::Struct(level) = cx.level() else {
                unreachable!()
            };
            let (state, returned) = (cx.intern_folded("state"), cx.intern_folded("returned"));
            let render = |cx: &mut Cx, v| match v {
                Value::String(a) => cx.resolve(a).to_string(),
                _ => String::new(),
            };
            let now = cx.get_field(level, state);
            let now = render(cx, now);
            let so_far = cx.get_field(level, returned);
            let so_far = render(cx, so_far);
            let text = cx.intern_exact(&format!("{so_far}{token}:{now} "));
            cx.set_field(level, returned, Value::String(text));
        }

        fn get_field(&mut self, cx: &mut Cx, ent: EntId, field: Atom) -> Value {
            self.0.get_field(cx, ent, field)
        }

        fn set_field(
            &mut self,
            cx: &mut Cx,
            ent: EntId,
            field: Atom,
            value: Value,
        ) -> Result<(), ErrorKind> {
            self.0.set_field(cx, ent, field, value)
        }
    }

    /// Reads `level.<name>` back as a string.
    fn level_string(vm: &mut Vm, name: &str) -> String {
        let level = vm.level_id();
        vm.with_cx(|cx| {
            let f = cx.intern_folded(name);
            match cx.get_field(level, f) {
                Value::String(a) => cx.resolve(a).to_string(),
                other => format!("{other:?}"),
            }
        })
    }

    /// A builtin that spawns a thread sees that thread run before its own
    /// caller continues: `killed` sets `level.state = "dead"` and waits, and
    /// `main`'s very next read of `level.state` is "dead". That is what
    /// retail's `finishPlayerDamage` (which runs `CodeCallback_PlayerKilled`
    /// through `Scr_ExecEntThread`) followed by `Callback_PlayerDamage`'s own
    /// `self.sessionstate` read relies on.
    #[test]
    fn a_thread_a_builtin_spawns_runs_before_the_builtin_returns_to_its_caller() {
        let mut vm = vm_with(
            r#"main() { level spawnkiller(); level.seen = level.state; }
               killed() { self.state = "dead"; wait 1; self.state = "buried"; }"#,
        );
        let mut host = SpawnHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "seen"), "dead");
        assert_eq!(level_string(&mut vm, "state"), "dead");
        vm.run_frame(&mut host, 1050);
        assert_eq!(level_string(&mut vm, "state"), "buried");
    }

    /// `spawn_then`'s hook runs once its thread has run to its first
    /// suspend, before the next queued thread and before the builtin's caller
    /// continues; a function nothing installed still gets its hook.
    #[test]
    fn spawn_then_returns_to_the_host_after_its_thread_and_before_the_caller() {
        let mut vm = vm_with(
            r#"main() { level spawnkillerthen(); level.seen = level.returned; }
               killed() { self.state = "dead"; wait 1; self.state = "buried"; }"#,
        );
        let mut host = SpawnHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "seen"), "1:dead 2:dead ");
        assert_eq!(vm.aborts().len(), 1, "the missing function is recorded");
    }

    /// A thread the hook queues starts next, ahead of the builtin's other
    /// queued threads and before its caller continues: the loop shape of
    /// `G_RadiusDamage`, one damage callback per pass.
    #[test]
    fn a_spawn_from_the_hook_runs_before_the_rest_of_the_queue() {
        let mut vm = vm_with(
            r#"main() { level.state = ""; level spawnchain(); level.seen = level.state; }
               step(n) { self.state = self.state + n; wait 1; }"#,
        );
        let mut host = SpawnHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "seen"), "1239");
    }

    /// A builtin that spawns a thread whose first act is the same builtin is
    /// a spawn bomb reached through `Cx::spawn` rather than a `thread`
    /// statement; `MAX_SPAWN_DEPTH` has to bound that path too, leaving a
    /// warning and a runnable thread instead of overflowing the native
    /// stack.
    #[test]
    fn a_builtin_spawn_bomb_hits_the_depth_guard() {
        let mut vm = vm_with("main() { level again(); } bomb() { level again(); }");
        let mut host = SpawnHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert!(
            vm.thread_count() > 0,
            "the capped spawn is left runnable for the next pass"
        );
        assert_eq!(vm.abort_count(), 0, "nothing errored on the way down");
    }

    /// A `FuncRef` no installed function answers is a recorded abort naming
    /// the function, not a panic, and the thread whose builtin asked for it
    /// carries on.
    #[test]
    fn a_builtin_spawn_of_an_uninstalled_function_is_recorded_not_a_panic() {
        let mut vm = vm_with(r#"main() { level spawnmissing(); level.seen = "ran on"; }"#);
        let mut host = SpawnHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(vm.aborts().len(), 1);
        let described = vm.describe(&vm.aborts()[0]);
        assert!(described.contains("nosuchthing"), "{described}");
        assert_eq!(level_string(&mut vm, "seen"), "ran on");
    }

    fn vm_cod2(src: &str) -> Vm {
        vm_dialect(src, crate::Dialect::Cod2)
    }

    fn vm_dialect(src: &str, dialect: crate::Dialect) -> Vm {
        let ast = crate::parse::parse_file_dialect(src, dialect).unwrap();
        let mut vm = Vm::new();
        let opts = crate::compile::CompileOptions {
            dialect,
            ..Default::default()
        };
        let fns = crate::compile::compile_file_with(&ast, "test/script", vm.interner_mut(), &opts)
            .unwrap();
        vm.install(fns).unwrap();
        vm
    }

    fn marks(vm: &Vm, host: &TestHost) -> Vec<String> {
        host.calls
            .iter()
            .filter(|(n, _)| n == "mark")
            .map(|(_, a)| match a[0] {
                Value::String(s) => vm.interner().resolve(s).to_string(),
                ref v => format!("{v:?}"),
            })
            .collect()
    }

    /// CoD4's `waittillmatch` wakes only on a notify whose first arguments
    /// equal its values; a notify that differs leaves it waiting.
    #[test]
    fn cod4_waittillmatch_wakes_on_matching_arguments_only() {
        let mut vm = vm_dialect(
            r#"waiter() { self waittillmatch("anim", "end"); mark("woke"); }
            pump(v) { self notify("anim", v); }"#,
            crate::Dialect::Cod4,
        );
        let mut host = TestHost::default();
        let me = Some(Target::Entity(EntId(1, 0)));
        let waiter = vm.func_ref("test/script", "waiter");
        let pump = vm.func_ref("test/script", "pump");
        vm.start_thread(&mut host, waiter, me, vec![]);
        let start = vm.interner_mut().intern_exact("start");
        vm.start_thread(&mut host, pump, me, vec![Value::String(start)]);
        vm.run_frame(&mut host, 0);
        assert!(
            marks(&vm, &host).is_empty(),
            "a notify of start is no match"
        );
        let anim = vm.interner_mut().intern_folded("anim");
        vm.notify(Target::Entity(EntId(1, 0)), anim, &[]);
        vm.run_frame(&mut host, 50);
        assert!(marks(&vm, &host).is_empty(), "nor is one without arguments");
        let end = vm.interner_mut().intern_exact("end");
        vm.start_thread(&mut host, pump, me, vec![Value::String(end)]);
        vm.run_frame(&mut host, 100);
        assert_eq!(marks(&vm, &host), ["woke"]);
    }

    /// CoD4 scales a vector by a number either way round and divides one
    /// per component; two ints still divide as floats, a zero divisor
    /// component is an error.
    #[test]
    fn cod4_scales_vectors_both_ways() {
        let mut vm = vm_dialect(
            r#"main() { v = (2, 4, 8); level.a = 2 * v; level.b = v * 0.5; level.c = v / 2; level.d = 3 / 2; level.e = v * (1, 0, 2); }
            bad() { level.f = (1, 2, 3) / (1, 0, 1); }"#,
            crate::Dialect::Cod4,
        );
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(vm.abort_count(), 0);
        assert_eq!(level_string(&mut vm, "a"), "Vector([4.0, 8.0, 16.0])");
        assert_eq!(level_string(&mut vm, "b"), "Vector([1.0, 2.0, 4.0])");
        assert_eq!(level_string(&mut vm, "c"), "Vector([1.0, 2.0, 4.0])");
        assert_eq!(level_string(&mut vm, "d"), "Float(1.5)");
        assert_eq!(level_string(&mut vm, "e"), "Vector([2.0, 0.0, 16.0])");
        let bad = vm.func_ref("test/script", "bad");
        vm.start_thread(&mut host, bad, None, vec![]);
        assert_eq!(vm.abort_count(), 1);
    }

    /// CoD4's `<<` and `>>` bind between the relations and the sums, and
    /// `prof_begin` / `prof_end` compile to nothing.
    #[test]
    fn cod4_shifts_and_profile_statements() {
        let mut vm = vm_dialect(
            r#"main() { prof_begin("x"); level.a = 1 << 2 + 1; level.b = -16 >> 2; level.c = 1 < 1 << 1; prof_end("x"); }"#,
            crate::Dialect::Cod4,
        );
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(vm.abort_count(), 0);
        assert_eq!(level_string(&mut vm, "a"), "Int(8)");
        assert_eq!(level_string(&mut vm, "b"), "Int(-4)");
        assert_eq!(level_string(&mut vm, "c"), "Int(1)");
        assert!(host.calls.is_empty(), "{:?}", host.calls);
    }

    /// CoD2 divides two ints as floats (`Scr_EvalDivide`, Mac 0x9012c);
    /// CoD1 truncates.
    #[test]
    fn cod2_divides_ints_as_floats() {
        let src = r#"main() { level.q = 1500 / 1000; level.r = 7; level.r /= 2; }"#;
        let mut host = TestHost::default();
        let mut vm = vm_cod2(src);
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "q"), "Float(1.5)");
        assert_eq!(level_string(&mut vm, "r"), "Float(3.5)");
        let mut vm = vm_with(src);
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "q"), "Int(1)");
    }

    /// CoD2's `Scr_EvalMultiply` (lnxded 0x807e290) multiplies numbers
    /// only: retail's server died on `anglesToForward(a) * 200` with `pair
    /// '(1, 0, -0)' and '200' has unmatching types 'vector' and 'int'`
    /// (cod2-protocol.md 16.5). CoD1's VM keeps its scaling.
    #[test]
    fn cod2_refuses_a_vector_times_a_number() {
        let src = r#"main() { level.n = 2 * 1.5; level.v = (1, 2, 3) * 2; }"#;
        let mut host = TestHost::default();
        let mut vm = vm_cod2(src);
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "n"), "Float(3.0)");
        assert_ne!(level_string(&mut vm, "v"), "Vector([2.0, 4.0, 6.0])");
        assert_eq!(vm.aborts().len(), 1);
        let mut vm = vm_with(src);
        let main = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, main, None, vec![]);
        assert_eq!(level_string(&mut vm, "v"), "Vector([2.0, 4.0, 6.0])");
    }

    /// CoD2's `probe_c2a` on retail `cod2_lnxded` (cod2-gsc-diff.md 3.2):
    /// `wait 0` resumes newest first, then the frame-end waiters oldest
    /// first, and a frame-end wait reached while that list runs still runs
    /// this frame; `wait 0.05` waits for the next.
    #[test]
    fn waittillframeend_runs_behind_the_frame_s_wait_0s() {
        let mut vm = vm_cod2(
            r#"main() {
                wait 0.05;
                mark("t0");
                thread fe("fe1");
                thread w0("w0a");
                thread fe("fe2");
                thread w05("w05");
                thread waiter();
                thread w0("w0b");
                level notify("go");
                mark("main_after_spawns");
                waittillframeend;
                mark("main_after_fe");
                wait 0.05;
                mark("next_frame");
            }
            fe(n) { mark(n + "_start"); waittillframeend; mark(n + "_end"); thread fe_nested(n); }
            fe_nested(n) { waittillframeend; mark(n + "_nested_end"); }
            w0(n) { mark(n + "_start"); wait 0; mark(n + "_end"); }
            w05(n) { wait 0.05; mark(n + "_end"); }
            waiter() { mark("waiter_start"); level waittill("go"); mark("waiter_woke"); waittillframeend; mark("waiter_fe"); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 100);
        assert_eq!(
            marks(&vm, &host),
            [
                "t0",
                "fe1_start",
                "w0a_start",
                "fe2_start",
                "waiter_start",
                "w0b_start",
                "main_after_spawns",
                "waiter_woke",
                "w0b_end",
                "w0a_end",
                "fe1_end",
                "fe2_end",
                "main_after_fe",
                "waiter_fe",
                "fe1_nested_end",
                "fe2_nested_end",
            ]
        );
        host.calls.clear();
        vm.run_frame(&mut host, 150);
        assert_eq!(marks(&vm, &host), ["next_frame", "w05_end"]);
    }

    /// `probe_c2n`: a `wait 0` a frame-end waiter takes resumes ahead of
    /// the frame-end waiters still queued.
    #[test]
    fn a_wait_0_inside_the_frame_end_list_runs_before_the_rest_of_it() {
        let mut vm = vm_cod2(
            r#"main() { wait 0.05; thread fe1(); thread fe2(); mark("main_spawned"); }
            fe1() { waittillframeend; mark("fe1_end"); wait 0; mark("fe1_w0_end"); }
            fe2() { waittillframeend; mark("fe2_end"); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, f, None, vec![]);
        vm.run_frame(&mut host, 100);
        assert_eq!(
            marks(&vm, &host),
            ["main_spawned", "fe1_end", "fe1_w0_end", "fe2_end"]
        );
    }
}
