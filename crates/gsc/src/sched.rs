//! The frame scheduler. A thread suspends by being left unstepped, which is
//! the whole reason the compiler emits bytecode rather than walking the AST.

use crate::atom::Atom;
use crate::vm::{Frame, Target};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ThreadId(pub u32);

#[derive(Debug)]
pub enum ThreadState {
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
pub struct Thread {
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
        vm.start_thread(&mut host, f, None, vec![]);

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
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(4))), vec![]);
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
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1))), vec![]);
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
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(7))), vec![]);
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
        vm.start_thread(&mut host, f, None, vec![]);
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
        vm.start_thread(&mut host, fa, Some(Target::Entity(EntId(1))), vec![]);
        vm.start_thread(&mut host, fb, Some(Target::Entity(EntId(1))), vec![]);
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
        vm.start_thread(&mut host, f, Some(Target::Entity(EntId(1))), vec![]);
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
}
