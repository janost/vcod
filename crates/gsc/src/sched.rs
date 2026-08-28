//! The frame scheduler. A thread suspends by being left unstepped, which is
//! the whole reason the compiler emits bytecode rather than walking the AST.

use crate::atom::Atom;
use crate::vm::{Frame, Target};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ThreadId(pub u32);

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
    use crate::vm::Vm;

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
        vm.start_thread(f, None, vec![]);

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
        vm.start_thread(f, Some(EntId(4)), vec![]);
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
        vm.start_thread(f, Some(EntId(1)), vec![]);
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
        vm.start_thread(f, Some(EntId(7)), vec![]);
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
        vm.start_thread(f, None, vec![]);
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
        let mut vm = vm_with("main() { while(1) { spin(); } }");
        vm.set_budget(10_000);
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(f, None, vec![]);
        let errs = vm.run_frame(&mut host, 0);
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].kind, crate::vm::ErrorKind::Budget));
        assert_eq!(vm.thread_count(), 0, "aborted, not left runnable");
    }

    #[test]
    fn an_erroring_thread_does_not_stop_its_sibling() {
        let mut vm = vm_with(
            r#"main() { thread bad(); thread good(); } bad() { double("no"); } good() { ok(); }"#,
        );
        let mut host = TestHost::default();
        let f = vm.func_ref("test/script", "main");
        vm.start_thread(f, None, vec![]);
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
        vm.start_thread(fa, Some(EntId(1)), vec![]);
        vm.start_thread(fb, Some(EntId(1)), vec![]);
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
}
