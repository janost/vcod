//! The `Host` trait, the heap-backed struct/entity/array data model at
//! runtime, and the `Vm` handle around them. The instruction loop that walks
//! `bytecode::Op` lives in `interp`; thread scheduling (`start_thread`,
//! `notify`, `run_frame`) lives in `sched`. Both are child modules, not
//! siblings, because they read `Vm`'s private fields directly.

use std::collections::HashMap;
use std::rc::Rc;

use crate::atom::{Atom, Interner};
use crate::bytecode::Function;
use crate::heap::{ArrayKey, Heap};
use crate::value::{ArrayId, EntId, FuncRef, StructId, Value};
use sched::Thread;

mod interp;
pub mod sched;

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

/// Everything a `Host` callback may reach inside the VM. Borrows disjoint
/// fields of `Vm`, which is what makes a builtin able to allocate: a single
/// `&mut Vm` cannot be handed out while `run_frame` holds one.
///
/// `notify` queues rather than resolving: a builtin can no more reenter the
/// VM than `Op::Notify` can (see `step_thread`).
pub struct Cx<'a> {
    interner: &'a mut Interner,
    heap: &'a mut Heap,
    level: StructId,
    game: StructId,
    notifies: &'a mut Vec<(Target, Atom, Vec<Value>)>,
}

impl Cx<'_> {
    pub fn intern(&mut self, s: &str) -> Atom {
        self.interner.intern(s)
    }

    pub fn resolve(&self, a: Atom) -> &str {
        self.interner.resolve(a)
    }

    pub fn new_struct(&mut self) -> StructId {
        self.heap.new_struct()
    }

    pub fn new_array(&mut self) -> ArrayId {
        self.heap.new_array()
    }

    pub fn get_field(&self, s: StructId, f: Atom) -> Value {
        self.heap.get_field(s, f)
    }

    pub fn set_field(&mut self, s: StructId, f: Atom, v: Value) {
        self.heap.set_field(s, f, v);
    }

    pub fn get_index(&self, a: ArrayId, k: ArrayKey) -> Value {
        self.heap.get_index(a, k)
    }

    pub fn set_index(&mut self, a: ArrayId, k: ArrayKey, v: Value) {
        self.heap.set_index(a, k, v);
    }

    pub fn array_len(&self, a: ArrayId) -> usize {
        self.heap.array_len(a)
    }

    /// The `%g` rendering task 7 measures against retail, so a builtin
    /// formatting a number for a configstring cannot drift from what the
    /// VM does for string concatenation. `None` for a value that has no
    /// rendering.
    pub fn format_number(&self, v: Value) -> Option<String> {
        match v {
            Value::Int(i) => Some(i.to_string()),
            // Placeholder: task 7 replaces this with the measured %g
            // rendering (docs/research/cod11-gsc-language.md).
            Value::Float(f) => Some(format!("{f}")),
            Value::String(a) | Value::Localized(a) => Some(self.interner.resolve(a).to_string()),
            Value::Vector([x, y, z]) => Some(format!("({x:.2}, {y:.2}, {z:.2})")),
            _ => None,
        }
    }

    pub fn level(&self) -> Target {
        Target::Struct(self.level)
    }

    pub fn game(&self) -> Target {
        Target::Struct(self.game)
    }

    pub fn notify(&mut self, target: Target, event: Atom, args: Vec<Value>) {
        self.notifies.push((target, event, args));
    }
}

/// Everything CoD-specific the VM can reach.
pub trait Host {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind>;

    /// Reading an unset field yields `Undefined` in gsc, so there is no
    /// error to report.
    fn get_field(&mut self, cx: &mut Cx, ent: EntId, field: Atom) -> Value;

    fn set_field(
        &mut self,
        cx: &mut Cx,
        ent: EntId,
        field: Atom,
        value: Value,
    ) -> Result<(), ErrorKind>;
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

    fn log_script_error(&self, e: &ScriptError) {
        log::warn!(
            "gsc: thread aborted in {}::{}:{}: {:?}",
            self.interner.resolve(e.file),
            self.interner.resolve(e.func),
            e.line,
            e.kind
        );
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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
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
            cx: &mut Cx,
            name: Atom,
            _recv: Option<Target>,
            args: &[Value],
        ) -> Result<Value, ErrorKind> {
            // Builtin dispatch matches against a fixed lowercase literal, so
            // it resolves folded: `IPrintLn(...)` must still reach the
            // "iprintln" arm below.
            let n = cx.resolve(name).to_ascii_lowercase();
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

        fn get_field(&mut self, cx: &mut Cx, e: EntId, f: Atom) -> Value {
            let k = (e.0, cx.resolve(f).to_string());
            self.fields.get(&k).copied().unwrap_or(Value::Undefined)
        }

        fn set_field(&mut self, cx: &mut Cx, e: EntId, f: Atom, v: Value) -> Result<(), ErrorKind> {
            self.fields.insert((e.0, cx.resolve(f).to_string()), v);
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

    /// No lexed integer literal spells `i32::MIN`: `2147483648` overflows
    /// `read_number`'s `i32` parse and falls back to a float, so
    /// `-2147483648` compiles as `Neg` on `Float(2147483648.0)`. It is
    /// still reachable through `(int)` of that float -- a saturating cast
    /// since Rust 1.45 -- because `2147483648.0` is exact in `f32` (a power
    /// of two) and already in range, so the cast lands exactly on
    /// `i32::MIN` rather than clamping short of it.
    /// (docs/research/cod11-gsc-language.md §9)
    #[test]
    fn int_min_is_reachable_only_through_a_float_cast_not_a_direct_literal() {
        let (v, _, _) = run("main() { return -2147483648; }");
        assert_eq!(v, Value::Float(-2147483648.0), "no direct int literal");
        let (v, _, _) = run("main() { return (int)-2147483648; }");
        assert_eq!(v, Value::Int(i32::MIN), "reachable via a saturating cast");
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

    /// The G1 blocker: a builtin needs to allocate and populate a heap array
    /// during the call, which `&Interner` could not express (E0499 against the
    /// `&mut Vm` `run_frame` holds).
    #[test]
    fn a_builtin_can_mint_and_populate_an_array_and_the_script_reads_it_back() {
        struct ArrayHost;
        impl Host for ArrayHost {
            fn builtin(
                &mut self,
                cx: &mut Cx,
                _name: Atom,
                _recv: Option<Target>,
                _args: &[Value],
            ) -> Result<Value, ErrorKind> {
                let a = cx.new_array();
                let v = cx.intern("hello");
                cx.set_index(a, ArrayKey::Int(0), Value::String(v));
                cx.set_index(a, ArrayKey::Int(1), Value::Int(7));
                Ok(Value::Array(a))
            }
            fn get_field(&mut self, _cx: &mut Cx, _e: EntId, _f: Atom) -> Value {
                Value::Undefined
            }
            fn set_field(
                &mut self,
                _cx: &mut Cx,
                _e: EntId,
                _f: Atom,
                _v: Value,
            ) -> Result<(), ErrorKind> {
                Ok(())
            }
        }

        let src = "main() { a = getentarray(\"x\", \"classname\"); return a[1]; }";
        let mut vm = Vm::new();
        let ast = crate::parse::parse_file(src).unwrap();
        let fns = crate::compile::compile_file(&ast, "test", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = ArrayHost;
        let f = vm.func_ref("test", "main");
        let out = vm.call_now(&mut host, 0, f, None, Vec::new()).unwrap();
        assert_eq!(out, Value::Int(7));
    }
}
