//! The frame scheduler. A thread suspends by being left unstepped, which is
//! the whole reason the compiler emits bytecode rather than walking the AST.

use crate::atom::Atom;
use crate::vm::{Frame, Target};

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

        let menu = vm.interner_mut().intern("team_americangerman");
        let resp = vm.interner_mut().intern("allies");
        let ev = vm.interner_mut().intern("menuresponse");
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

        let ev = vm.interner_mut().intern("e");
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

        let ev = vm.interner_mut().intern("death");
        vm.notify(EntId(7), ev, &[]);
        vm.run_frame(&mut host, 2000);
        assert_eq!(vm.thread_count(), 0, "killed, not resumed");
        assert!(!host.calls.iter().any(|(n, _)| n == "done"));
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
        let ev = vm.interner_mut().intern("e");
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
    /// endon, then threads `killer`, which runs immediately (this round's
    /// item 1) and notifies the very event `main` just registered against,
    /// all before `main`'s own step ever reaches `wait 5`. If the endon
    /// were only visible once `main` suspends (the bug this test pins),
    /// the notify would find no waiter, and `main` would wake up 5 "ms"
    /// later and run `done()` instead of having been killed immediately.
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

    /// `start_thread`'s immediate run can error just as easily as
    /// `run_frame`-driven one can; that path (the thread is removed, not
    /// left dangling or panicking) had no direct coverage until now, since
    /// the round-1 error tests were rewritten to route their error through
    /// `run_frame` instead (their leading `wait 0;`), for the unrelated
    /// reason that `start_thread` has nowhere to return the error to.
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

    /// The `now_ms = 0` every other `start_thread` test in this suite
    /// passes can't distinguish "the fix is present" from "the fix was
    /// reverted": `Vm::now_ms` already defaults to `0`, so a reverted
    /// `self.now_ms = now_ms;` and a working one behave identically at
    /// that value. This test uses a clock that isn't the default: `main`
    /// threads `f`, whose own `wait 1` (1000 ms) must deadline against
    /// `5_000`, not `0` -- `run_frame(&mut host, 5_000)` (right after
    /// `start_thread`, same clock reading) must not fire it, only
    /// `run_frame(&mut host, 6_000)` should.
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
}
