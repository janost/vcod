//! The CoD side of `vcod_gsc::Host`: builtin dispatch and entity field
//! routing. Field routing arrives with the object model in stage 2; until
//! then an entity field read is `Undefined` and a write is refused, so a
//! script that reaches one fails by name instead of silently working.

use crate::game::builtins;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, Host, Target, Value};

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
    /// whole of stage 1's configstring gate: slot 12 from setCullFog, slot 3
    /// from ambientPlay (docs/research/clientstate-wire-format.md).
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
}
