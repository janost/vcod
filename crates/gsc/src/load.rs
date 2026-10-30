//! Turning script paths into compiled functions.
//!
//! Scripts write `maps\mp\_load::main()` while the paks store
//! `maps/MP/_load.gsc`, so every path is canonicalized before lookup.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::Dialect;
use crate::bytecode::Op;
use crate::compile::CompileOptions;
use crate::value::{FuncRef, Value};
use crate::vm::Vm;

/// Lowercase, backslashes to forward slashes, `.gsc` suffix dropped.
pub fn canonical(path: &str) -> String {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.strip_suffix(".gsc").unwrap_or(&p).to_string()
}

/// Reads script source by canonical path. A trait so this crate never
/// learns what a pk3 is; the host implements it over its own asset store.
pub trait ScriptSource {
    /// `canonical` is already normalized; the implementation appends `.gsc`
    /// and whatever case folding its own storage needs.
    fn read(&self, canonical: &str) -> Option<String>;
}

#[derive(Debug)]
pub struct LoadError {
    pub path: String,
    pub line: u32,
    pub msg: String,
}

/// One file as the source answered it, and its parse.
struct SourceFile {
    text: String,
    parsed: Result<crate::ast::File, crate::parse::ParseError>,
}

/// What a [`Loader`] has done so far, without its source or builtin
/// table: [`Loader::state`] takes one after some loads and
/// [`Loader::resume`] carries on from it, as the loader that took it
/// would have. A host keeps one beside the matching [`crate::Program`]
/// to skip compiling a closure it compiled before.
#[derive(Clone, Default)]
pub struct LoaderState {
    loaded: HashSet<String>,
    animtrees: BTreeSet<String>,
    /// Every path the source was asked for, with what it answered.
    files: HashMap<String, Option<Rc<SourceFile>>>,
}

impl LoaderState {
    /// Whether `source` still answers every path this state read with the
    /// same text (a missing file still missing): if so, loading again from
    /// `source` repeats exactly what built this state. Reads every one.
    pub fn current(&self, source: &dyn ScriptSource) -> bool {
        // Sorted, so the reads (and whatever they touch in the source) go
        // in one order run to run.
        let mut paths: Vec<_> = self.files.iter().collect();
        paths.sort_by(|a, b| a.0.cmp(b.0));
        paths
            .into_iter()
            .all(|(path, file)| match (source.read(path), file) {
                (Some(text), Some(f)) => text == f.text,
                (None, None) => true,
                _ => false,
            })
    }
}

/// Reads, compiles and installs scripts on demand, following every
/// cross-file reference a loaded file names until the transitive closure
/// is installed.
pub struct Loader {
    source: Box<dyn ScriptSource>,
    /// Canonical paths already loaded (or in the middle of loading), so a
    /// reference cycle terminates and a file shared by two callers
    /// compiles once.
    loaded: HashSet<String>,
    /// Every `#using_animtree` name the loaded files declare, folded.
    animtrees: BTreeSet<String>,
    /// Each path read once and parsed once: a CoD2 file's includes are
    /// looked up by every file that includes them.
    files: RefCell<HashMap<String, Option<Rc<SourceFile>>>>,
    dialect: Dialect,
    /// The host's builtin names, which beat an included function (CoD2).
    is_builtin: Option<BuiltinTable>,
}

/// Answers whether a folded name is a builtin.
pub type BuiltinTable = Box<dyn Fn(&str) -> bool>;

impl Loader {
    /// A loader compiling CoD 1.1 script.
    pub fn new(source: Box<dyn ScriptSource>) -> Self {
        Self::with_dialect(source, Dialect::Cod1)
    }

    pub fn with_dialect(source: Box<dyn ScriptSource>, dialect: Dialect) -> Self {
        Self::resume(source, dialect, LoaderState::default())
    }

    /// A loader that carries on from `state` (taken by [`Loader::state`]
    /// under the same dialect) over `source`, which should answer as the
    /// one that built it did ([`LoaderState::current`]). The builtin table
    /// is not part of the state: set it again.
    pub fn resume(source: Box<dyn ScriptSource>, dialect: Dialect, state: LoaderState) -> Self {
        Loader {
            source,
            loaded: state.loaded,
            animtrees: state.animtrees,
            files: RefCell::new(state.files),
            dialect,
            is_builtin: None,
        }
    }

    /// What this loader has loaded and read so far.
    pub fn state(&self) -> LoaderState {
        LoaderState {
            loaded: self.loaded.clone(),
            animtrees: self.animtrees.clone(),
            files: self.files.borrow().clone(),
        }
    }

    /// `path` read and parsed, once per loader.
    fn file(&self, path: &str) -> Option<Rc<SourceFile>> {
        if let Some(f) = self.files.borrow().get(path) {
            return f.clone();
        }
        let f = self.source.read(path).map(|text| {
            let parsed = crate::parse::parse_file_dialect(&text, self.dialect);
            Rc::new(SourceFile { text, parsed })
        });
        self.files.borrow_mut().insert(path.to_string(), f.clone());
        f
    }

    /// Gives the compiler the host's builtin table, folded names: under
    /// CoD2 an unqualified call to a name both a builtin and an included
    /// function is the builtin's. Without one the included function wins.
    pub fn set_builtins(&mut self, is_builtin: BuiltinTable) {
        self.is_builtin = Some(is_builtin);
    }

    /// The animtrees the loaded files name in `#using_animtree`, lowercase
    /// and sorted: the engine reads each one's `animtrees/<name>.atr`.
    pub fn animtrees(&self) -> impl Iterator<Item = &str> {
        self.animtrees.iter().map(String::as_str)
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    /// Loads `path` and, recursively, every file it references by a static
    /// call target or a bare `::name` function pointer. `path` need not be
    /// canonical; every path discovered from compiled output already is
    /// (the compiler canonicalizes both halves of a `FuncRef`).
    ///
    /// Atomic at this entry point: if any file reached from `path` fails to
    /// load, every function this call installed is removed again and every
    /// path it newly marked loaded is unmarked, so a failed load leaves the
    /// `Vm` exactly as it found it (a half-loaded gametype that still looks
    /// loaded is worse than a clean refusal) and a retry — e.g. once a
    /// missing file exists — does not silently no-op on a path this call
    /// already gave up on.
    pub fn load(&mut self, vm: &mut Vm, path: &str) -> Result<(), LoadError> {
        let mut installed = Vec::new();
        let mut visited = Vec::new();
        let read_before: HashSet<String> = self.files.borrow().keys().cloned().collect();
        self.load_one(vm, &canonical(path), None, &mut installed, &mut visited)
            .inspect_err(|_| {
                vm.uninstall(&installed);
                for p in &visited {
                    self.loaded.remove(p);
                }
                // A retry reads again what this call read.
                self.files
                    .borrow_mut()
                    .retain(|p, _| read_before.contains(p));
            })
    }

    /// `referrer` is the canonical path that named `path`, for the missing-
    /// file error message; `None` at the top-level call. `installed` and
    /// `visited` accumulate across the whole recursion so `load` can undo
    /// exactly what this call did, and nothing older.
    fn load_one(
        &mut self,
        vm: &mut Vm,
        path: &str,
        referrer: Option<&str>,
        installed: &mut Vec<FuncRef>,
        visited: &mut Vec<String>,
    ) -> Result<(), LoadError> {
        // Marked loaded before compiling, not after: two stock scripts
        // referencing each other is ordinary, and marking after would
        // recurse forever on the cycle.
        if !self.loaded.insert(path.to_string()) {
            return Ok(());
        }
        visited.push(path.to_string());

        let file = self.file(path).ok_or_else(|| LoadError {
            path: path.to_string(),
            // No source line exists for "the file itself is missing".
            line: 0,
            msg: match referrer {
                Some(r) => format!("script not found (referenced from {r})"),
                None => "script not found".to_string(),
            },
        })?;

        let ast = file.parsed.as_ref().map_err(|e| LoadError {
            path: path.to_string(),
            line: e.line,
            msg: e.msg.clone(),
        })?;

        self.animtrees
            .extend(ast.animtrees.iter().map(|t| t.to_ascii_lowercase()));
        // An included file's function names, read off its parse; a file
        // that does not parse is reported when it loads below.
        let include_funcs = |p: &str| {
            let file = self.file(p)?;
            let names = file
                .parsed
                .as_ref()
                .map(|f| f.funcs.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();
            Some(names)
        };
        let opts = CompileOptions {
            dialect: self.dialect,
            include_funcs: &include_funcs,
            is_builtin: self.is_builtin.as_deref(),
        };
        let fns = crate::compile::compile_file_with(ast, path, vm.interner_mut(), &opts).map_err(
            |e| LoadError {
                path: path.to_string(),
                line: e.line,
                msg: e.msg,
            },
        )?;

        // Collected before `install` consumes `fns`. A cross-file call is
        // an `Op::Call` whose target names another file; a bare `::name`
        // pointer lowers to a `Value::Function` constant instead, since
        // `Expr::FuncRef` is an AST node compilation resolves away. Both
        // already carry a canonical file, resolved at compile time.
        // A `BTreeSet`, not a `HashSet`: this drives the recursive load
        // below, so hash iteration order would make which file a broken
        // reference gets blamed on, and which spelling of a folded key
        // `resolve` keeps (`atom.rs`'s "whichever the compiler saw first"),
        // nondeterministic run to run.
        // An included file loads whether or not anything calls into it,
        // as retail's include pass loads it.
        let mut refs: BTreeSet<String> = ast.includes.iter().map(|i| i.path.clone()).collect();
        for f in &fns {
            for op in &f.code {
                if let Op::Call { func, .. } = op {
                    refs.insert(vm.interner().resolve_folded(func.file).to_string());
                }
            }
            for c in &f.consts {
                if let Value::Function(target) = c {
                    refs.insert(vm.interner().resolve_folded(target.file).to_string());
                }
            }
        }

        // Also computed before the move into `install`, but only recorded
        // as newly installed once `install` actually accepts them.
        let new_funcs: Vec<FuncRef> = fns
            .iter()
            .map(|f| FuncRef {
                file: f.file,
                name: f.name,
            })
            .collect();

        // No source line either: this rejects the whole function set from
        // one file at once, not one statement.
        vm.install(fns).map_err(|e| LoadError {
            path: path.to_string(),
            line: 0,
            msg: e.to_string(),
        })?;
        installed.extend(new_funcs);

        for r in refs {
            self.load_one(vm, &r, Some(path), installed, visited)?;
        }
        Ok(())
    }

    /// Every distinct builtin name an installed function calls, for which
    /// `known` returns false, sorted. `waittill`, `notify` and `endon`
    /// never appear here: the compiler gives them dedicated ops rather
    /// than `Op::CallBuiltin`, and a call to another script function
    /// compiles to `Op::Call`, not `Op::CallBuiltin`.
    pub fn missing_builtins(&self, vm: &Vm, known: &dyn Fn(&str) -> bool) -> Vec<String> {
        let mut missing: BTreeSet<String> = BTreeSet::new();
        for f in vm.functions() {
            for op in &f.code {
                if let Op::CallBuiltin { name, .. } = op {
                    let n = vm.interner().resolve_folded(*name);
                    if !known(n) {
                        missing.insert(n.to_string());
                    }
                }
            }
        }
        missing.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::Vm;
    use crate::vm::tests::TestHost;
    use std::collections::HashMap;

    struct MapSource(HashMap<String, String>);

    impl ScriptSource for MapSource {
        fn read(&self, canonical: &str) -> Option<String> {
            self.0.get(canonical).cloned()
        }
    }

    fn source(files: &[(&str, &str)]) -> MapSource {
        MapSource(
            files
                .iter()
                .map(|(k, v)| (canonical(k), v.to_string()))
                .collect(),
        )
    }

    /// A source whose files a test changes between loads, counting reads.
    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<(HashMap<String, String>, usize)>>);

    impl Shared {
        fn set(&self, path: &str, text: &str) {
            self.0.borrow_mut().0.insert(canonical(path), text.into());
        }
        fn reads(&self) -> usize {
            self.0.borrow().1
        }
    }

    impl ScriptSource for Shared {
        fn read(&self, canonical: &str) -> Option<String> {
            let mut s = self.0.borrow_mut();
            s.1 += 1;
            s.0.get(canonical).cloned()
        }
    }

    #[test]
    fn a_cod2_include_is_read_once_however_many_files_include_it() {
        let src = Shared::default();
        src.set("a", "#include c;\nmain() { b::f(); one(); }");
        src.set("b", "#include c;\nf() { one(); }");
        src.set("c", "one() { }");
        let mut l = Loader::with_dialect(Box::new(src.clone()), Dialect::Cod2);
        l.load(&mut Vm::new(), "a").unwrap();
        assert_eq!(src.reads(), 3);
    }

    /// A loader resumed from another's state, on a `Vm` built from that
    /// one's program, ends where the first would have: same functions,
    /// same atoms, same file order.
    #[test]
    fn a_resumed_loader_carries_on_as_the_one_it_was_taken_from() {
        let src = source(&[
            (
                "gt",
                r#"#using_animtree("multiplayer"); main() { shared::f(); }"#,
            ),
            (
                "map",
                r#"#using_animtree("flak88"); main() { shared::f(); own::g(); }"#,
            ),
            ("shared", "f() { x = \"s\"; }"),
            ("own", "g() { y = \"o\"; }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "gt").unwrap();
        let state = l.state();
        let program = vm.program();
        l.load(&mut vm, "map").unwrap();

        let src = source(&[
            (
                "gt",
                r#"#using_animtree("multiplayer"); main() { shared::f(); }"#,
            ),
            (
                "map",
                r#"#using_animtree("flak88"); main() { shared::f(); own::g(); }"#,
            ),
            ("shared", "f() { x = \"s\"; }"),
            ("own", "g() { y = \"o\"; }"),
        ]);
        assert!(state.current(&src));
        let mut vm2 = Vm::from_program(&program);
        let mut l2 = Loader::resume(Box::new(src), Dialect::Cod1, state);
        l2.load(&mut vm2, "map").unwrap();

        assert_eq!(l2.loaded_count(), l.loaded_count());
        assert_eq!(
            l2.animtrees().collect::<Vec<_>>(),
            ["flak88", "multiplayer"]
        );
        let funcs = |vm: &Vm| {
            let mut v: Vec<_> = vm
                .functions()
                .map(|f| (f.file, f.name, format!("{:?}", f.code)))
                .collect();
            v.sort();
            v
        };
        assert_eq!(funcs(&vm2), funcs(&vm));
        // Every atom in the same place.
        assert!(vm2.interner().texts().eq(vm.interner().texts()));
    }

    #[test]
    fn a_state_is_stale_once_a_file_it_read_changes_or_appears() {
        let src = Shared::default();
        src.set("a", "#include b;\nmain() { }");
        src.set("b", "f() { }");
        let mut l = Loader::with_dialect(Box::new(src.clone()), Dialect::Cod2);
        l.load(&mut Vm::new(), "a").unwrap();
        let state = l.state();
        assert!(state.current(&src));
        src.set("b", "f() { wait 1; }");
        assert!(!state.current(&src));
        src.set("b", "f() { }");
        assert!(state.current(&src));

        // A path the load found missing is part of the state too.
        let mut l = Loader::new(Box::new(src.clone()));
        assert!(l.load(&mut Vm::new(), "gone").is_err());
        assert!(l.load(&mut Vm::new(), "a/b").is_err());
        src.set("a/b", "main() { }");
        l.load(&mut Vm::new(), "a/b").unwrap();
    }

    #[test]
    fn canonical_folds_case_and_separators() {
        assert_eq!(canonical(r"maps\MP\_Load"), "maps/mp/_load");
        assert_eq!(canonical("maps/MP/_load.gsc"), "maps/mp/_load");
    }

    #[test]
    fn loading_pulls_in_referenced_files() {
        let src = source(&[
            ("maps/mp/mp_pavlov", r#"main() { maps\mp\_load::main(); }"#),
            ("maps/mp/_load", "main() { loaded(); }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "maps/mp/mp_pavlov").unwrap();

        let mut host = TestHost::default();
        let f = vm.func_ref("maps/mp/mp_pavlov", "main");
        vm.call_now(&mut host, f, None, vec![]).unwrap();
        assert!(host.calls.iter().any(|(n, _)| n == "loaded"));
    }

    #[test]
    fn the_loader_lists_every_file_s_animtree() {
        let src = source(&[
            ("a", r#"#using_animtree("Flak88"); main() { b::f(); }"#),
            ("b", r#"#using_animtree("multiplayer"); f() { }"#),
        ]);
        let mut l = Loader::new(Box::new(src));
        l.load(&mut Vm::new(), "a").unwrap();
        assert_eq!(l.animtrees().collect::<Vec<_>>(), ["flak88", "multiplayer"]);
    }

    /// A bare `::name` pointer with no call around it (`x = file::fn;`)
    /// lowers to a `Value::Function` constant, not an `Op::Call` — the shape
    /// every stock gametype uses to register its engine callbacks. Deleting
    /// the constant-pool scan in `load_one` would still pass every other
    /// test in this file; only this one would catch it.
    #[test]
    fn a_bare_function_pointer_pulls_in_its_file() {
        let src = source(&[
            ("a", r#"main() { x = maps\mp\b::cb; }"#),
            ("maps/mp/b", "cb() { }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();

        let mut host = TestHost::default();
        let f = vm.func_ref("maps/mp/b", "cb");
        assert!(vm.call_now(&mut host, f, None, vec![]).is_ok());
    }

    #[test]
    fn a_file_referenced_twice_is_compiled_once() {
        let src = source(&[
            ("a", r#"main() { b::x(); b::y(); }"#),
            ("b", "x() { } y() { }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();
        assert_eq!(l.loaded_count(), 2);
    }

    #[test]
    fn a_reference_cycle_terminates() {
        let src = source(&[
            ("a", "main() { b::main(); }"),
            ("b", "main() { a::main(); }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();
        assert_eq!(l.loaded_count(), 2);
    }

    /// `refs` (the set driving recursive loading) is a `BTreeSet`, so with
    /// several broken references from one file, the one reported is always
    /// the alphabetically first — deterministic run to run, not whichever a
    /// `HashSet`'s hash-seed-dependent iteration order happened to visit
    /// first.
    #[test]
    fn several_broken_references_always_blame_the_alphabetically_first() {
        let src = source(&[(
            "a",
            r#"main() { z\gone::main(); a\gone::main(); m\gone::main(); }"#,
        )]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        let e = l.load(&mut vm, "a").unwrap_err();
        assert_eq!(e.path, "a/gone");
    }

    #[test]
    fn a_missing_file_names_the_path_that_wanted_it() {
        let src = source(&[("a", r#"main() { maps\mp\gone::main(); }"#)]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        let e = l.load(&mut vm, "a").unwrap_err();
        assert_eq!(e.path, "maps/mp/gone");
    }

    /// A failed load must not leave the referencing file's functions
    /// installed: a half-loaded gametype that looks loaded is worse than a
    /// clean refusal a caller can retry.
    #[test]
    fn a_failed_load_leaves_no_functions_installed() {
        let src = source(&[(
            "a",
            r#"main() { helper(); maps\mp\gone::main(); } helper() { }"#,
        )]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        assert!(l.load(&mut vm, "a").is_err());
        assert_eq!(vm.functions().count(), 0);
        assert_eq!(l.loaded_count(), 0);
    }

    /// A second `load` call that fails must not undo the first call's
    /// success: `load_one`'s early return on an already-`loaded` path means
    /// the failing call never re-visits "a", so it stays both marked loaded
    /// and installed. Nothing pinned this before; it is correct only by
    /// construction of the early return above.
    #[test]
    fn a_failed_second_load_does_not_undo_an_earlier_successful_one() {
        let src = source(&[
            ("a", "main() { }"),
            ("c", r#"main() { a::main(); maps\mp\gone::main(); }"#),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();
        assert!(l.load(&mut vm, "c").is_err());
        // "a" must still be marked loaded and still installed/callable.
        let f = vm.func_ref("a", "main");
        assert!(
            vm.call_now(&mut TestHost::default(), f, None, vec![])
                .is_ok()
        );
    }

    /// In a 799-file corpus a broken reference is hard to trace from the
    /// missing path alone; the message names which file wanted it. (The
    /// brief's own test pins `e.path` to the missing path itself, so the
    /// referrer goes into `msg`, not `path`.)
    #[test]
    fn a_missing_file_error_names_the_file_that_referenced_it() {
        let src = source(&[("maps/mp/mp_pavlov", r#"main() { maps\mp\gone::main(); }"#)]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        let e = l.load(&mut vm, "maps/mp/mp_pavlov").unwrap_err();
        assert!(
            e.msg.contains("maps/mp/mp_pavlov"),
            "expected the referrer in the message, got: {}",
            e.msg
        );
    }

    #[test]
    fn the_prescan_lists_builtins_the_host_lacks() {
        let src = source(&[(
            "a",
            r#"main() { iprintln("x"); radiusdamage(); helper(); } helper() { }"#,
        )]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();

        let missing = l.missing_builtins(&vm, &|n| n == "iprintln");
        assert_eq!(missing, vec!["radiusdamage"]);
    }

    #[test]
    fn the_prescan_ignores_script_functions_and_the_suspending_forms() {
        let src = source(&[(
            "a",
            r#"main() { self endon("e"); self notify("f"); self waittill("g"); helper(); } helper() { }"#,
        )]);
        let mut vm = Vm::new();
        let mut l = Loader::new(Box::new(src));
        l.load(&mut vm, "a").unwrap();
        assert!(l.missing_builtins(&vm, &|_| false).is_empty());
    }

    /// `probe_c2b` and `probe_c2c`: an include imports its file's functions,
    /// the file's own includes work inside it, and they reach no further.
    #[test]
    fn a_cod2_include_is_not_transitive() {
        let src = source(&[
            ("a", "#include b;\nmain() { one(); two(); }"),
            ("b", "#include c;\none() { two(); }"),
            ("c", "two() { }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::with_dialect(Box::new(src), Dialect::Cod2);
        l.load(&mut vm, "a").unwrap();
        // `two` from `a` is left to the builtins; `one` and `b`'s `two`
        // are script calls.
        assert_eq!(l.missing_builtins(&vm, &|_| false), vec!["two"]);
        let mut host = TestHost::default();
        let f = vm.func_ref("b", "one");
        assert!(vm.call_now(&mut host, f, None, vec![]).is_ok());
    }

    #[test]
    fn a_cod2_include_loads_its_file_unreferenced_and_the_builtin_table_wins() {
        let src = source(&[
            ("a", "#include b;\nmain() { getcvar(); }"),
            ("b", "getcvar() { }"),
        ]);
        let mut vm = Vm::new();
        let mut l = Loader::with_dialect(Box::new(src), Dialect::Cod2);
        l.set_builtins(Box::new(|n| n == "getcvar"));
        l.load(&mut vm, "a").unwrap();
        assert_eq!(l.loaded_count(), 2);
        assert_eq!(l.missing_builtins(&vm, &|_| false), vec!["getcvar"]);
    }

    #[test]
    fn a_cod1_loader_refuses_an_include() {
        let src = source(&[("a", "#include b;\nmain() { }"), ("b", "")]);
        let e = Loader::new(Box::new(src))
            .load(&mut Vm::new(), "a")
            .unwrap_err();
        assert_eq!(e.msg, "unknown directive '#include'");
    }
}
