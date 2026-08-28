//! Turning script paths into compiled functions.
//!
//! Scripts write `maps\mp\_load::main()` while the paks store
//! `maps/MP/_load.gsc`, so every path is canonicalized before lookup.

use std::collections::{BTreeSet, HashSet};

use crate::bytecode::Op;
use crate::value::Value;
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
    pub fn load(&mut self, vm: &mut Vm, path: &str) -> Result<(), LoadError> {
        self.load_one(vm, &canonical(path))
    }

    fn load_one(&mut self, vm: &mut Vm, path: &str) -> Result<(), LoadError> {
        // Marked loaded before compiling, not after: two stock scripts
        // referencing each other is ordinary, and marking after would
        // recurse forever on the cycle.
        if !self.loaded.insert(path.to_string()) {
            return Ok(());
        }

        let text = self.source.read(path).ok_or_else(|| LoadError {
            path: path.to_string(),
            line: 0,
            msg: "script not found".to_string(),
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

        vm.install(fns).map_err(|msg| LoadError {
            path: path.to_string(),
            line: 0,
            msg,
        })?;

        for r in refs {
            self.load_one(vm, &r)?;
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
