//! The CoD side of `vcod_gsc::Host`: builtin dispatch and entity field
//! routing. There is no entity object model yet, so a field read is
//! `Undefined` and a write is refused: a script that reaches one fails by
//! name instead of silently working.

use crate::game::builtins;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, Host, Target, Value};

/// Every builtin `GameHost::builtin` dispatches, folded, so the load-time
/// pre-scan can list what a map script calls that the host does not answer
/// yet. `every_listed_builtin_dispatches` keeps this in step with the match.
pub const BUILTINS: &[&str] = &[
    "setcullfog",
    "ambientplay",
    "println",
    "iprintln",
    "logprint",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

pub struct GameHost {
    pub configstrings: Vec<String>,
}

impl GameHost {
    pub fn new(configstrings: Vec<String>) -> GameHost {
        GameHost { configstrings }
    }
}

impl Host for GameHost {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        _recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind> {
        // `resolve_folded`, not `resolve`: the atom carries the spelling the
        // script used (`setCullFog`), and dispatch matches the folded form
        // without allocating.
        match cx.resolve_folded(name) {
            "setcullfog" => builtins::env::set_cull_fog(&mut self.configstrings, cx, args),
            "ambientplay" => builtins::env::ambient_play(&mut self.configstrings, cx, args),
            "println" | "iprintln" | "logprint" => builtins::io::print_line(cx, args),
            _ => Err(ErrorKind::MissingBuiltin(name)),
        }
    }

    fn get_field(&mut self, _cx: &mut Cx, _ent: EntId, _field: Atom) -> Value {
        Value::Undefined
    }

    fn set_field(
        &mut self,
        _cx: &mut Cx,
        _ent: EntId,
        _field: Atom,
        _value: Value,
    ) -> Result<(), ErrorKind> {
        Err(ErrorKind::BadType(
            "entity fields arrive with the object model",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mp_pavlov.gsc` opens with exactly these two calls, and they are the
    /// only configstrings the script writes so far: slot 12 from setCullFog
    /// (docs/protocol-1.1.md, "Configstrings") and slot 3 from ambientPlay.
    #[test]
    fn setcullfog_and_ambientplay_write_their_configstring_slots() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = GameHost::new(vec![String::new(); 2048]);
        let src = r#"main() {
            setCullFog(0, 6000, 0.8, 0.8, 0.8, 0);
            ambientPlay("ambient_mp_pavlov");
        }"#;
        let ast = vcod_gsc::parse::parse_file(src).unwrap();
        let fns = vcod_gsc::compile::compile_file(&ast, "test", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let f = vm.func_ref("test", "main");
        vm.call_now(&mut host, 0, f, None, Vec::new()).unwrap();

        assert_eq!(host.configstrings[12], "0 6000 1 0.8 0.8 0.8 0");
        assert_eq!(host.configstrings[3], "n\\ambient_mp_pavlov\\t\\0");
    }

    /// `BUILTINS` drives the load-time pre-scan while the match below drives
    /// dispatch; a name dropped from one and not the other would make the
    /// pre-scan lie. Argument errors are fine here, `MissingBuiltin` is not.
    #[test]
    fn every_listed_builtin_dispatches() {
        for name in BUILTINS {
            let mut vm = vcod_gsc::Vm::new();
            let mut host = GameHost::new(vec![String::new(); 2048]);
            let src = format!("main() {{ {name}(); }}");
            let ast = vcod_gsc::parse::parse_file(&src).unwrap();
            let fns = vcod_gsc::compile::compile_file(&ast, "test", vm.interner_mut()).unwrap();
            vm.install(fns).unwrap();
            let f = vm.func_ref("test", "main");
            let err = vm.call_now(&mut host, 0, f, None, Vec::new()).err();
            assert!(
                !matches!(
                    err.map(|e| e.kind),
                    Some(vcod_gsc::ErrorKind::MissingBuiltin(_))
                ),
                "{name} is listed but does not dispatch"
            );
        }
    }
}
