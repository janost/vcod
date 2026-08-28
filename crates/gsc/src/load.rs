//! Turning script paths into compiled functions.
//!
//! Scripts write `maps\mp\_load::main()` while the paks store
//! `maps/MP/_load.gsc`, so every path is canonicalized before lookup.

use std::collections::{BTreeSet, HashSet};

use crate::bytecode::Op;
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

/// Reads, compiles and installs scripts on demand, following every
/// cross-file reference a loaded file names until the transitive closure
/// is installed.
pub struct Loader {
    source: Box<dyn ScriptSource>,
    /// Canonical paths already loaded (or in the middle of loading), so a
    /// reference cycle terminates and a file shared by two callers
    /// compiles once.
    loaded: HashSet<String>,
}

impl Loader {
    pub fn new(source: Box<dyn ScriptSource>) -> Self {
        Loader {
            source,
            loaded: HashSet::new(),
        }
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
        self.load_one(vm, &canonical(path), None, &mut installed, &mut visited)
            .inspect_err(|_| {
                vm.uninstall(&installed);
                for p in &visited {
                    self.loaded.remove(p);
                }
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

        let text = self.source.read(path).ok_or_else(|| LoadError {
            path: path.to_string(),
            // No source line exists for "the file itself is missing".
            line: 0,
            msg: match referrer {
                Some(r) => format!("script not found (referenced from {r})"),
                None => "script not found".to_string(),
            },
        })?;

        let ast = crate::parse::parse_file(&text).map_err(|e| LoadError {
            path: path.to_string(),
            line: e.line,
            msg: e.msg,
        })?;

        let fns =
            crate::compile::compile_file(&ast, path, vm.interner_mut()).map_err(|e| LoadError {
                path: path.to_string(),
                line: e.line,
                msg: e.msg,
            })?;

        // Collected before `install` consumes `fns`. A cross-file call is
        // an `Op::Call` whose target names another file; a bare `::name`
        // pointer lowers to a `Value::Function` constant instead, since
        // `Expr::FuncRef` is an AST node compilation resolves away. Both
        // already carry a canonical file, resolved at compile time.
        let mut refs: HashSet<String> = HashSet::new();
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
        vm.install(fns).map_err(|msg| LoadError {
            path: path.to_string(),
            line: 0,
            msg,
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
    use crate::vm::tests::TestHost;
    use crate::vm::Vm;
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
        vm.call_now(&mut host, 0, f, None, vec![]).unwrap();
        assert!(host.calls.iter().any(|(n, _)| n == "loaded"));
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
        assert!(vm.call_now(&mut host, 0, f, None, vec![]).is_ok());
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
        assert!(vm
            .call_now(&mut TestHost::default(), 0, f, None, vec![])
            .is_ok());
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
}
