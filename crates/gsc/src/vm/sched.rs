//! The frame scheduler. A thread suspends by being left unstepped, which is
//! the whole reason the compiler emits bytecode rather than walking the AST.

use std::rc::Rc;

use crate::atom::Atom;
use crate::bytecode::Function;
use crate::value::{FuncRef, Value};

use super::interp::{Frame, Step, Suspend};
use super::{ErrorKind, Host, ScriptError, Target, Vm};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ThreadId(pub u32);

// `Thread`/`ThreadState` are `pub(crate)`, not `pub`: `Vm` never hands one
// out (only a `ThreadId`, to name a thread without exposing its innards),
// so nothing outside this crate can obtain one to act on.
#[derive(Debug)]
pub(crate) enum ThreadState {
    Runnable,
    /// Server milliseconds at which this thread becomes runnable again.
    WaitingUntil(i32),
    WaitingNotify {
        target: Target,
        event: Atom,
        binds: Box<[u16]>,
    },
}

#[derive(Debug)]
pub(crate) struct Thread {
    pub id: ThreadId,
    pub frames: Vec<Frame>,
    pub state: ThreadState,
    pub endons: Vec<(Target, Atom)>,
}

impl Vm {
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
        let target = target.into();
        let event = self.interner.fold_atom(event);
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
        let mut steps = 0;
        // `self.threads` stays sorted by id -- `spawn` only ever appends
        // (`next_thread` is monotonic), and `notify`'s kill pass only
        // removes, which preserves order -- so the next id above the
        // watermark is a binary search, not a linear scan: O(n log n)
        // total for a frame instead of the O(n²) the old `filter` +
        // `min_by_key` walk cost once a frame holds many live threads.
        while steps < Self::MAX_THREADS_PER_FRAME {
            let idx = self
                .threads
                .partition_point(|t| last_id.is_some_and(|l| t.id.0 <= l));
            let Some(t) = self.threads.get(idx) else {
                break;
            };
            let tid = t.id;
            last_id = Some(tid.0);
            self.step_thread(host, tid, &mut errors);
            steps += 1;
        }
        if steps == Self::MAX_THREADS_PER_FRAME
            && self
                .threads
                .partition_point(|t| last_id.is_some_and(|l| t.id.0 <= l))
                < self.threads.len()
        {
            // Was silent before: a VM past this many concurrently live
            // threads just stopped simulating the rest with nothing in the
            // log to say why. Still doesn't step them -- reaping starved
            // threads is a design question for the next project -- but a
            // triage now has something to search for.
            log::warn!(
                "gsc: MAX_THREADS_PER_FRAME ({}) reached in one run_frame call; threads above the watermark are starved this frame",
                Self::MAX_THREADS_PER_FRAME
            );
        }
        errors
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
    use crate::value::{EntId, Value};
    use crate::vm::tests::TestHost;
    use crate::vm::{Target, Vm};

    fn vm_with(src: &str) -> Vm {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        vm
    }

    #[test]
    fn wait_resumes_only_after_its_deadline() {
        let mut vm = vm_with(r#"main() { wait 0.1; done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, 0, f, None, vec![]);

        vm.run_frame(&mut host, 0);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"), "not yet");
        vm.run_frame(&mut host, 50);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "still not yet"
        );
        vm.run_frame(&mut host, 100);
        assert!(
            host.calls.iter().any(|(n, _)| n == "done"),
            "resumed at 100 ms"
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
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(4))), vec![]);
        vm.run_frame(&mut host, 0);

        let menu = vm.interner_mut().intern_exact("team_americangerman");
        let resp = vm.interner_mut().intern_exact("allies");
        let ev = vm.interner_mut().intern_folded("menuresponse");
        vm.notify(EntId(4), ev, &[Value::String(menu), Value::String(resp)]);
        vm.run_frame(&mut host, 50);

        let (_, args) = host.calls.iter().find(|(n, _)| n == "got").unwrap();
        assert_eq!(args, &[Value::String(menu), Value::String(resp)]);
    }

    #[test]
    fn a_notify_on_another_entity_does_not_wake_the_waiter() {
        let mut vm = vm_with(r#"main() { self waittill("e"); done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(1))), vec![]);
        vm.run_frame(&mut host, 0);

        let ev = vm.interner_mut().intern_folded("e");
        vm.notify(EntId(2), ev, &[]);
        vm.run_frame(&mut host, 50);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
        assert_eq!(vm.thread_count(), 1, "still waiting");
    }

    #[test]
    fn endon_kills_a_waiting_thread() {
        let mut vm = vm_with(r#"main() { self endon("death"); wait 1; done(); }"#);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(7))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1);

        let ev = vm.interner_mut().intern_folded("death");
        vm.notify(EntId(7), ev, &[]);
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
        vm.start_thread(&mut host, 0, waiter, Some(Target::Entity(EntId(3))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1, "waiting");

        vm.start_thread(&mut host, 50, pump, Some(Target::Entity(EntId(3))), vec![]);
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
        vm.start_thread(&mut host, 0, waiter, Some(Target::Entity(EntId(5))), vec![]);
        vm.start_thread(&mut host, 0, dier, Some(Target::Entity(EntId(5))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 2);

        let ev = vm.interner_mut().intern_exact("playerCONNECT");
        vm.notify(EntId(5), ev, &[]);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "parent_done"),
            "caller did not block"
        );
        assert!(!host.calls.iter().any(|(n, _)| n == "child_done"));
        vm.run_frame(&mut host, 100);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
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
        vm.start_thread(&mut host, 0, fa, Some(Target::Entity(EntId(1))), vec![]);
        vm.start_thread(&mut host, 0, fb, Some(Target::Entity(EntId(1))), vec![]);
        vm.run_frame(&mut host, 0);
        let ev = vm.interner_mut().intern_folded("e");
        vm.notify(EntId(1), ev, &[]);
        vm.run_frame(&mut host, 50);
        let order: Vec<&str> = host.calls.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            order.iter().position(|n| *n == "first"),
            order.iter().position(|n| *n == "second").map(|i| i - 1)
        );
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
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(1))), vec![]);
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
        vm.start_thread(&mut host, 0, victim, None, vec![]);
        vm.start_thread(&mut host, 0, killer, None, vec![]);
        vm.start_thread(&mut host, 0, witness, None, vec![]);

        vm.run_frame(&mut host, 0);
        assert!(
            host.calls.iter().any(|(n, _)| n == "seen"),
            "witness still ran"
        );
        assert_eq!(vm.thread_count(), 0);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
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
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(1))), vec![]);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(!host.calls.iter().any(|(n, _)| n == "done"), "not yet");
        vm.run_frame(&mut host, 1000);
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
        vm.start_thread(&mut host, 0, f, None, vec![]);
        vm.run_frame(&mut host, 0);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "seen"),
            "still waiting"
        );

        vm.run_frame(&mut host, 1000);
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
        vm.start_thread(&mut host, 0, waiter, None, vec![Value::Int(1)]);
        vm.start_thread(&mut host, 0, waiter, None, vec![Value::Int(2)]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 2, "both waiting");

        vm.start_thread(&mut host, 0, pump, None, vec![]);
        vm.run_frame(&mut host, 0);

        let seen: Vec<Value> = host
            .calls
            .iter()
            .filter(|(n, _)| n == "got")
            .map(|(_, args)| args[0])
            .collect();
        assert_eq!(seen, vec![Value::Int(1), Value::Int(2)]);
    }

    // --- Divergences kept as documentation
    // (docs/research/cod11-gsc-language.md §10), pinned so a later change to
    // notify/kill ordering is deliberate rather than accidental. ---

    /// Two `notify`s of the same event on the same target queued within one
    /// step coalesce: `notify` only flips a waiter's state (`Vm::notify`),
    /// it never re-checks whether a later notify in the same flush still has
    /// a waiter, so the first wakes it and the second, finding it no longer
    /// `WaitingNotify`, is dropped. `pump`'s two `notify`s both queue during
    /// its own single step and flush together after it (`step_thread`); by
    /// the time the second flushes, `waiter` has already been marked
    /// `Runnable` by the first, so it never sees `2` -- and never runs
    /// again to look, since its second `waittill` is now waiting on a tick
    /// that already happened.
    #[test]
    fn two_notifies_of_the_same_event_in_one_step_coalesce_and_the_second_is_lost() {
        let mut vm = vm_with(
            r#"waiter() { self waittill("tick", v); got(v); self waittill("tick", v); got(v); }
            pump() { self notify("tick", 1); self notify("tick", 2); }"#,
        );
        let mut host = TestHost::default();
        let waiter = vm.func_ref("test/script", "waiter");
        let pump = vm.func_ref("test/script", "pump");
        vm.start_thread(&mut host, 0, waiter, Some(Target::Entity(EntId(1))), vec![]);
        vm.run_frame(&mut host, 0);
        assert_eq!(vm.thread_count(), 1, "waiting on the first tick");

        vm.start_thread(&mut host, 0, pump, Some(Target::Entity(EntId(1))), vec![]);
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
        vm.start_thread(&mut host, 0, f, Some(Target::Entity(EntId(1))), vec![]);

        assert_eq!(vm.thread_count(), 0, "killed by its own endon mid-step");
        assert!(host.calls.iter().any(|(n, _)| n == "sideeffecta"));
        assert!(host.calls.iter().any(|(n, _)| n == "sideeffectb"));
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "never resumed past its own wait to reach this"
        );
    }

    /// Past `MAX_THREADS_PER_FRAME` (1,000) live threads, the ones above
    /// the id watermark are starved for the frame -- `run_frame`'s walk
    /// still only visits the lowest 1,000 ids, whether that walk is the old
    /// linear scan or the `partition_point` binary search it was replaced
    /// with. This pins the behavior (and, incidentally, that the
    /// replacement didn't change it) rather than the `log::warn!` that now
    /// also fires when it happens, which nothing in this suite captures.
    #[test]
    fn starving_past_max_threads_per_frame_leaves_the_excess_unstepped_this_frame() {
        let mut vm = vm_with("main() { wait 1; done(); }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        for _ in 0..1002 {
            vm.start_thread(&mut host, 0, f, None, vec![]);
        }
        assert_eq!(
            vm.thread_count(),
            1002,
            "all spawned, all waiting on their own wait"
        );

        vm.run_frame(&mut host, 2000);
        assert_eq!(
            vm.thread_count(),
            2,
            "the two threads past the watermark were starved this frame"
        );
    }

    /// The `now_ms = 0` every other `start_thread` test in this suite
    /// passes can't distinguish "the fix is present" from "the fix was
    /// reverted": `Vm::now_ms` already defaults to `0`, so a reverted
    /// `self.now_ms = now_ms;` and a working one behave identically at
    /// that value. This test uses a clock that isn't the default: `main`
    /// threads `f`, whose own `wait 1` (1000 ms) must deadline against
    /// `5_000`, not `0` -- `run_frame(&mut host, 5_000)` (right after
    /// `start_thread`, same clock reading) must not fire it, only
    /// `run_frame(&mut host, 6_000)` should.
    #[test]
    fn start_thread_threads_a_wait_against_its_own_now_ms_not_zero() {
        let mut vm = vm_with("main() { thread f(); } f() { wait 1; done(); }");
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(&mut host, 5_000, f, None, vec![]);
        assert_eq!(vm.thread_count(), 1, "f is alive, waiting on its own wait");

        vm.run_frame(&mut host, 5_000);
        assert!(
            !host.calls.iter().any(|(n, _)| n == "done"),
            "not due yet -- 6_000, not 1_000"
        );
        vm.run_frame(&mut host, 6_000);
        assert!(host.calls.iter().any(|(n, _)| n == "done"), "due now");
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
}
