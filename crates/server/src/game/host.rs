//! The CoD side of `vcod_gsc::Host`: builtin dispatch and entity field
//! routing. A field read or write is routed through the retail field
//! tables: engine-backed goes to the entity's typed slot, client-tagged
//! errors until stage 4 brings clients, everything else goes to the
//! entity's own struct in the VM heap.

use crate::game::builtins;
use crate::game::damage::DamageEvent;
use crate::game::entity::{ObjectTable, FIRST_HUD_ELEM};
use crate::game::fields::{self, FieldType, Route};
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
    pub ents: ObjectTable,
    /// Damage the script asked for, drained after `run_frame` by stage 6.
    /// A builtin must never reenter the VM, so a callback becomes a queued
    /// event (the design's "callbacks cannot run inline").
    pub damage: Vec<DamageEvent>,
}

impl GameHost {
    pub fn new(configstrings: Vec<String>) -> GameHost {
        GameHost {
            configstrings,
            ents: ObjectTable::new(),
            damage: Vec::new(),
        }
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

    fn get_field(&mut self, cx: &mut Cx, ent: EntId, field: Atom) -> Value {
        let Some(e) = self.ents.get(ent) else {
            return Value::Undefined;
        };
        let route = if ent.0 >= FIRST_HUD_ELEM {
            fields::route_hud(cx.resolve_folded(field))
        } else {
            fields::route_entity(cx.resolve_folded(field))
        };
        match route {
            Route::Engine { slot, .. } => e.engine[slot],
            // Retail errors here; a read has no error channel, so an entity
            // with no client reads undefined and the write path carries the
            // error.
            Route::Client(_) => Value::Undefined,
            Route::Script => cx.get_field(e.script, field),
        }
    }

    fn set_field(
        &mut self,
        cx: &mut Cx,
        ent: EntId,
        field: Atom,
        value: Value,
    ) -> Result<(), ErrorKind> {
        let route = if ent.0 >= FIRST_HUD_ELEM {
            fields::route_hud(cx.resolve_folded(field))
        } else {
            fields::route_entity(cx.resolve_folded(field))
        };
        let Some(e) = self.ents.get_mut(ent) else {
            return Err(ErrorKind::BadType("no such entity"));
        };
        match route {
            Route::Engine { slot, ty } => {
                if !type_accepts(ty, value) {
                    return Err(ErrorKind::BadType("wrong type for an engine field"));
                }
                e.engine[slot] = value;
                Ok(())
            }
            Route::Client(_) => Err(ErrorKind::BadType(
                "that field lives on the client, which arrives in stage 4",
            )),
            Route::Script => {
                let s = e.script;
                cx.set_field(s, field, value);
                Ok(())
            }
        }
    }
}

/// Which `Value` shapes each field type accepts. Retail converts in
/// `Scr_SetGenericField`; we refuse a mismatch instead, so a script bug
/// surfaces where retail would silently store a zero.
fn type_accepts(ty: FieldType, v: Value) -> bool {
    use FieldType::*;
    match ty {
        Int => matches!(v, Value::Int(_) | Value::Undefined),
        Float => matches!(v, Value::Int(_) | Value::Float(_) | Value::Undefined),
        CString | IString | ModelIndex => {
            matches!(v, Value::String(_) | Value::Localized(_) | Value::Undefined)
        }
        Vector | YawVector => matches!(v, Value::Vector(_) | Value::Undefined),
        Entity => matches!(v, Value::Entity(_) | Value::Undefined),
        Object => matches!(v, Value::Struct(_) | Value::Array(_) | Value::Undefined),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

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

    /// An engine field round-trips through its typed slot; a script-defined
    /// name round-trips through the entity's own struct in the VM heap. One
    /// object, two stores, chosen by the field table.
    #[test]
    fn engine_and_script_fields_use_their_own_stores() {
        let (mut vm, mut host) = fixture();
        let e = vm.with_cx(|cx| host.ents.spawn(cx).unwrap());
        vm.with_cx(|cx| {
            let tn = cx.intern_folded("targetname");
            let sx = cx.intern_folded("script_exploder");
            let v = cx.intern_exact("exploder");
            host.set_field(cx, e, tn, Value::String(v)).unwrap();
            host.set_field(cx, e, sx, Value::Int(3)).unwrap();
            assert_eq!(host.get_field(cx, e, tn), Value::String(v));
            assert_eq!(host.get_field(cx, e, sx), Value::Int(3));
            // The script store is the heap struct, not a second map.
            let s = host.ents.get(e).unwrap().script;
            assert_eq!(cx.get_field(s, sx), Value::Int(3));
            assert_eq!(cx.get_field(s, tn), Value::Undefined);
        });
    }

    /// Reading a field nothing has written is `undefined`, which is what makes
    /// `isDefined(ents[i].script_exploder)` in `_load.gsc` the filter it is.
    #[test]
    fn an_unwritten_field_reads_undefined() {
        let (mut vm, mut host) = fixture();
        let e = vm.with_cx(|cx| host.ents.spawn(cx).unwrap());
        vm.with_cx(|cx| {
            let f = cx.intern_folded("script_exploder");
            assert_eq!(host.get_field(cx, e, f), Value::Undefined);
            let g = cx.intern_folded("health");
            assert_eq!(host.get_field(cx, e, g), Value::Undefined);
        });
    }

    /// Field names fold case: `.TargetName` and `.targetname` are one field.
    #[test]
    fn field_access_folds_case() {
        let (mut vm, mut host) = fixture();
        let e = vm.with_cx(|cx| host.ents.spawn(cx).unwrap());
        vm.with_cx(|cx| {
            let lower = cx.intern_folded("targetname");
            let mixed = cx.intern_folded("TargetName");
            let v = cx.intern_exact("a");
            host.set_field(cx, e, mixed, Value::String(v)).unwrap();
            assert_eq!(host.get_field(cx, e, lower), Value::String(v));
        });
    }

    /// A client-routed field on an entity with no client is the error retail
    /// raises for a null `ent->client`, not a silent undefined
    /// (docs/research/cod11-gsc-object-model.md section 5).
    #[test]
    fn a_client_field_without_a_client_is_an_error() {
        let (mut vm, mut host) = fixture();
        let e = vm.with_cx(|cx| host.ents.spawn(cx).unwrap());
        vm.with_cx(|cx| {
            let f = cx.intern_folded("sessionteam");
            let v = cx.intern_exact("allies");
            assert!(host.set_field(cx, e, f, Value::String(v)).is_err());
        });
    }

    /// Writing the wrong type into a typed engine slot is refused. Retail
    /// converts per field type in `Scr_SetGenericField`; we refuse rather than
    /// silently coerce, so a script bug surfaces.
    #[test]
    fn an_engine_slot_refuses_the_wrong_type() {
        let (mut vm, mut host) = fixture();
        let e = vm.with_cx(|cx| host.ents.spawn(cx).unwrap());
        vm.with_cx(|cx| {
            let origin = cx.intern_folded("origin");
            assert!(host.set_field(cx, e, origin, Value::Int(1)).is_err());
            assert!(host
                .set_field(cx, e, origin, Value::Vector([1.0, 2.0, 3.0]))
                .is_ok());
        });
    }
}
