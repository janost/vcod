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

/// Why `Vm::install` rejected a batch of functions.
#[derive(Clone, PartialEq, Debug)]
pub enum InstallError {
    /// `bytecode::stack_depth`'s abstract stack walk rejected one of the
    /// functions before any of the batch was installed.
    BadStack(String),
    /// Two installed functions share a `FuncRef`.
    Duplicate(FuncRef),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::BadStack(msg) => write!(f, "{msg}"),
            InstallError::Duplicate(r) => write!(f, "duplicate function {r:?}"),
        }
    }
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
    /// For a string value a builtin returns or stores. Case-preserving; a
    /// name the engine matches case-insensitively wants `intern_folded`.
    pub fn intern(&mut self, s: &str) -> Atom {
        self.interner.intern_exact(s)
    }

    /// For a field name, function name, file path or event name (atom.rs).
    pub fn intern_folded(&mut self, s: &str) -> Atom {
        self.interner.intern_folded(s)
    }

    pub fn resolve(&self, a: Atom) -> &str {
        self.interner.resolve(a)
    }

    /// The folded spelling, for a host dispatching on a builtin's name.
    /// `resolve` returns the atom's as-written text (`logPrint`), which is
    /// what a script built for display needs; a dispatch table needs the
    /// folded form and should not allocate to get it.
    pub fn resolve_folded(&self, a: Atom) -> &str {
        self.interner.resolve_folded(a)
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

    /// The same `%g` rendering string concatenation uses, so a builtin
    /// formatting a number for a configstring cannot drift from it.
    /// `None` for a value that has no rendering.
    pub fn format_number(&self, v: Value) -> Option<String> {
        crate::value::format_number(v, self.interner)
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
    /// `.size` on an array or string, interned once here rather than per
    /// `LoadField` — `_load.gsc` reads it inside three loops.
    size_atom: Atom,
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
        let mut interner = Interner::default();
        let size_atom = interner.intern_folded("size");
        Vm {
            interner,
            heap,
            functions: HashMap::new(),
            level,
            game,
            size_atom,
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

    /// `ErrorKind`'s own `Debug` prints `MissingBuiltin`'s `Atom` as a bare
    /// index (`MissingBuiltin(Atom(43))`) -- useless as the work list this
    /// log line exists to be, since nothing else here names the builtin.
    /// `ErrorKind` keeps the `Atom` for its callers, which match on it; only
    /// the logged text resolves it.
    fn log_script_error(&self, e: &ScriptError) {
        let kind = match &e.kind {
            ErrorKind::MissingBuiltin(name) => {
                format!("MissingBuiltin({:?})", self.interner.resolve(*name))
            }
            other => format!("{other:?}"),
        };
        log::warn!(
            "gsc: thread aborted in {}::{}:{}: {kind}",
            self.interner.resolve(e.file),
            self.interner.resolve(e.func),
            e.line,
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

    /// `level`'s struct id. `level()` returns a `Target` for notify and call
    /// receivers; this is for the host's own heap reads.
    pub fn level_id(&self) -> StructId {
        self.level
    }

    /// `game`'s struct id, the `Target::Struct` counterpart to `level_id`.
    pub fn game_id(&self) -> StructId {
        self.game
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
    /// test suite, which is the only place it ran before. Also rejects a
    /// function whose `FuncRef` collides with one already installed, rather
    /// than silently overwriting it. Both checks run over the whole batch
    /// before anything is inserted, so a rejected `install` call changes
    /// nothing.
    pub fn install(&mut self, fns: Vec<Function>) -> Result<(), InstallError> {
        for f in &fns {
            crate::bytecode::stack_depth(f).map_err(InstallError::BadStack)?;
        }
        let mut seen = std::collections::HashSet::new();
        for f in &fns {
            let key = FuncRef {
                file: f.file,
                name: f.name,
            };
            if self.functions.contains_key(&key) || !seen.insert(key) {
                return Err(InstallError::Duplicate(key));
            }
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
            file: self.interner.intern_folded(path),
            name: self.interner.intern_folded(name),
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

    /// The mirror of `run` for an expression retail treats as fatal.
    fn run_err(src: &str) -> ErrorKind {
        let ast = crate::parse::parse_file(src).unwrap();
        let mut vm = Vm::new();
        let fns = crate::compile::compile_file(&ast, "test/script", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = TestHost::default();
        let main = vm.func_ref("test/script", "main");
        vm.call_now(&mut host, 0, main, None, vec![])
            .expect_err("expected a runtime error")
            .kind
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

    /// `i32::MIN` is not spellable: the magnitude saturates at `i32::MAX`
    /// before the unary minus applies, so `-2147483648` is `-2147483647`
    /// both directly and through `(int)`. Measured on retail
    /// (tests/fixtures/semantics/retail-captures.txt, `# probe_lexer`).
    #[test]
    fn an_integer_literal_past_i32_max_saturates_before_the_minus_applies() {
        let (v, _, _) = run("main() { return -2147483648; }");
        assert_eq!(v, Value::Int(-2147483647));
        let (v, _, _) = run("main() { return (int)-2147483648; }");
        assert_eq!(v, Value::Int(-2147483647));
        let (v, _, _) = run("main() { return 999999999999; }");
        assert_eq!(v, Value::Int(i32::MAX));
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

    // --- `level`/`game` as call and notify/waittill/endon receivers. ---

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
        let file = vm.interner_mut().intern_folded("test/bad");
        let name = vm.interner_mut().intern_folded("main");
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

    /// A second function sharing a `FuncRef` with one already installed is
    /// rejected rather than silently replacing it, and rejection is
    /// all-or-nothing: an earlier function in the same batch is not left
    /// installed either.
    #[test]
    fn install_rejects_a_duplicate_func_ref_and_installs_neither() {
        let mut vm = Vm::new();
        let file = vm.interner_mut().intern_folded("test/dup");
        let f = |vm: &mut Vm, name: &str| crate::bytecode::Function {
            file,
            name: vm.interner_mut().intern_folded(name),
            params: 0,
            locals: 0,
            code: vec![crate::bytecode::Op::ReturnUndef],
            consts: Vec::new(),
            lines: vec![1],
        };
        let first = f(&mut vm, "main");
        vm.install(vec![first]).unwrap();

        let other = f(&mut vm, "other");
        let dup = f(&mut vm, "main");
        let err = vm.install(vec![other, dup]).unwrap_err();
        assert!(matches!(err, InstallError::Duplicate(_)));
        assert_eq!(vm.functions().count(), 1);
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

    /// `_load.gsc:6` reads `ents.size`. Both arms measured against retail in
    /// tests/fixtures/semantics/retail-captures.txt.
    #[test]
    fn size_counts_array_keys_and_string_characters() {
        let (v, _, _) = run(r#"main() { a = []; a[0] = 1; a[1] = 2; a["k"] = 3; return a.size; }"#);
        assert_eq!(v, Value::Int(3), "every key counts, whatever its type");
        let (v, _, _) = run(r#"main() { a = []; return a.size; }"#);
        assert_eq!(v, Value::Int(0));
        let (v, _, _) = run(r#"main() { s = "abcd"; return s.size; }"#);
        assert_eq!(v, Value::Int(4));
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

    /// Retail has no ordering outside numbers: `"a" < "b"` is a fatal
    /// `pair has unmatching types 'string' and 'string'`
    /// (tests/fixtures/semantics/retail-captures.txt, `# probe_cmp_order`).
    #[test]
    fn ordering_a_non_number_is_an_error_not_a_false_reading() {
        assert!(matches!(
            run_err(r#"main() { return "a" < "b"; }"#),
            ErrorKind::BadType(_)
        ));
        assert!(matches!(
            run_err(r#"main() { return "foo" > 1; }"#),
            ErrorKind::BadType(_)
        ));
        assert!(matches!(
            run_err("main() { return undefined >= 1; }"),
            ErrorKind::BadType(_)
        ));
    }

    /// Only numbers have a boolean reading; everything else in a condition
    /// kills the retail server (`# probe_truthy`, `# probe_truthy_empty`,
    /// `# probe_truthy_vec`, `# probe_truthy_undef`). vcod raises the
    /// equivalent error and aborts the thread instead.
    #[test]
    fn a_non_numeric_condition_is_an_error() {
        let (v, _, _) = run("main() { if (0) return 1; return 0; }");
        assert_eq!(v, Value::Int(0));
        let (v, _, _) = run("main() { if (0.5) return 1; return 0; }");
        assert_eq!(v, Value::Int(1));
        for src in [
            r#"main() { if ("a") return 1; return 0; }"#,
            r#"main() { if ("") return 1; return 0; }"#,
            "main() { v = (0,0,0); if (v) return 1; return 0; }",
            "main() { if (level.nope) return 1; return 0; }",
            "main() { if (!undefined) return 1; return 0; }",
        ] {
            assert!(matches!(run_err(src), ErrorKind::BadType(_)), "{src}");
        }
    }

    /// A number concatenated onto a string renders through `%g`; anything
    /// with no rendering is fatal (`# probe_concat`, `# probe_concat_vec`).
    #[test]
    fn concatenation_renders_numbers_and_vectors_the_way_retail_does() {
        let cases = [
            (r#"main() { return "PROBE " + 5; }"#, "PROBE 5"),
            (r#"main() { return "PROBE " + (0 - 5); }"#, "PROBE -5"),
            (r#"main() { return "PROBE " + 0.5; }"#, "PROBE 0.5"),
            (r#"main() { return "PROBE " + 2.0; }"#, "PROBE 2"),
            (r#"main() { return "PROBE " + 0.8; }"#, "PROBE 0.8"),
            (
                r#"main() { return "PROBE " + (1.0 / 3); }"#,
                "PROBE 0.333333",
            ),
            (r#"main() { return "PROBE " + 1000000; }"#, "PROBE 1000000"),
            (
                r#"main() { return "PROBE " + (1, 2, 3); }"#,
                "PROBE (1.00, 2.00, 3.00)",
            ),
            // Inference, not measurement: every probe put the string first,
            // so no capture pins a number on the left. Asserted here on the
            // assumption `+` is symmetric in which side renders.
            (r#"main() { return 5 + " PROBE"; }"#, "5 PROBE"),
        ];
        for (src, want) in cases {
            let (v, _, mut vm) = run(src);
            let Value::String(a) = v else {
                panic!("{src} did not produce a string: {v:?}");
            };
            assert_eq!(vm.interner_mut().resolve(a), want, "{src}");
        }
        assert!(matches!(
            run_err(r#"main() { return "PROBE " + undefined; }"#),
            ErrorKind::BadType(_)
        ));
    }

    /// Equality is neither structural nor numeric: a number compared with a
    /// string is rendered and matched textually, and `undefined` against
    /// anything else is fatal (`# probe_cmp`, `# probe_cmp_mixed`,
    /// `# probe_cmp_coerce`).
    #[test]
    fn equality_renders_a_number_before_comparing_it_with_a_string() {
        for (src, want) in [
            ("main() { return 1 == 1.0; }", 1),
            (r#"main() { return "abc" == "abc"; }"#, 1),
            (r#"main() { return "ABC" == "abc"; }"#, 0),
            (r#"main() { return "5" == 5; }"#, 1),
            (r#"main() { return 5 == "5"; }"#, 1),
            (r#"main() { return "5.0" == 5; }"#, 0),
            (r#"main() { return "05" == 5; }"#, 0),
            (r#"main() { return "abc" == 0; }"#, 0),
            (r#"main() { return "5" != 5; }"#, 0),
        ] {
            let (v, _, _) = run(src);
            assert_eq!(v, Value::Int(want), "{src}");
        }
        assert!(matches!(
            run_err("main() { return undefined == 0; }"),
            ErrorKind::BadType(_)
        ));
        // Not measured; retail's message names a "pair has unmatching
        // types", which two `undefined`s are not (research doc §9).
        let (v, _, _) = run("main() { return undefined == undefined; }");
        assert_eq!(v, Value::Int(1));
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

    /// Array keys are case-sensitive on retail: `a["medFire"]` and
    /// `a["medfire"]` are two entries
    /// (tests/fixtures/semantics/retail-captures.txt, `# probe_arraykey_case`,
    /// which reports size 2 and reads 1 and 2 back). Field names are not; see
    /// `fields_fold_case`.
    #[test]
    fn array_keys_are_case_sensitive_unlike_field_names() {
        let (v, _, _) =
            run(r#"main() { a = []; a["medFire"] = 1; a["medfire"] = 2; return a.size; }"#);
        assert_eq!(v, Value::Int(2));
        let (v, _, _) =
            run(r#"main() { a = []; a["medFire"] = 1; a["medfire"] = 2; return a["medFire"]; }"#);
        assert_eq!(v, Value::Int(1));
    }

    /// The other half of the same rule: a field name still folds.
    #[test]
    fn fields_fold_case() {
        let (v, _, _) = run(r#"main() { level.myField = 7; return level.myfield; }"#);
        assert_eq!(v, Value::Int(7));
    }

    /// And string values do not: `# probe_cmp` has `"ABC" == "abc"` false.
    #[test]
    fn string_equality_is_case_sensitive() {
        let (v, _, _) = run(r#"main() { return "ABC" == "abc"; }"#);
        assert_eq!(v, Value::Int(0));
        let (v, _, _) = run(r#"main() { return "abc" == "abc"; }"#);
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
        let hp = vm.interner_mut().intern_folded("hp");
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

        let mark = vm.interner_mut().intern_folded("mark");
        let level_id = match vm.level() {
            Target::Struct(id) => id,
            Target::Entity(_) => unreachable!("level is always a struct"),
        };
        vm.heap_mut().set_field(level_id, mark, Value::Int(1));
        assert_ne!(vm.level(), vm.game());
    }

    /// A host reads and writes `level` constantly; without this it needs an
    /// `unreachable!` arm on the `Target` match every time.
    #[test]
    fn level_and_game_ids_are_reachable_without_matching_on_target() {
        let mut vm = Vm::new();
        let f = vm.interner_mut().intern_folded("mark");
        let id = vm.level_id();
        vm.heap_mut().set_field(id, f, Value::Int(1));
        assert_eq!(vm.heap().get_field(vm.level_id(), f), Value::Int(1));
        assert_ne!(vm.level_id(), vm.game_id());
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
