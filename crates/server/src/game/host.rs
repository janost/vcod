//! The CoD side of `vcod_gsc::Host`: builtin dispatch and entity field
//! routing. A field read or write is routed through the retail field
//! tables: engine-backed goes to the entity's typed slot, client-tagged
//! goes to the entity's own `client` store when it has one and errors
//! otherwise, everything else goes to the entity's own struct in the VM
//! heap.

use crate::configstrings::{Allocators, CsRange};
use crate::game::builtins;
use crate::game::damage::DamageEvent;
use crate::game::entity::{ObjectTable, FIRST_HUD_ELEM};
use crate::game::fields::{self, FieldType, Route};
use crate::server::MAX_CLIENTS;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, Host, Target, Value};

/// The builtins `GameHost::builtin` answers from its own match, folded: the
/// env and io names, which have no family module of their own. Every other
/// builtin comes from a family's `NAMES`, and `is_builtin` walks both so the
/// load-time pre-scan sees the same set dispatch does.
/// `every_listed_builtin_dispatches` keeps this list in step with the match.
pub const BUILTINS: &[&str] = &[
    "setcullfog",
    "ambientplay",
    "println",
    "iprintln",
    "logprint",
];

/// Every builtin family's `NAMES`, walked in the same order `builtin`
/// dispatches them, so the pre-scan and dispatch cannot drift apart by
/// construction.
pub fn is_builtin(name: &str) -> bool {
    builtins::entity::lookup(name).is_some()
        || builtins::math::lookup(name).is_some()
        || builtins::fx::lookup(name).is_some()
        || builtins::hud::lookup(name).is_some()
        || builtins::sound::lookup(name).is_some()
        || builtins::attach::lookup(name).is_some()
        || builtins::client::lookup(name).is_some()
        || builtins::combat::lookup(name).is_some()
        || builtins::mover::lookup(name).is_some()
        || builtins::cvar::lookup(name).is_some()
        || builtins::precache::lookup(name).is_some()
        || BUILTINS.contains(&name)
}

/// One client lifecycle step the netcode raised, by client slot. `Connect`
/// and `Begin` are two events on purpose: `Callback_PlayerConnect` blocks on
/// `waittill("begin")` as its second statement, so the thread has to be
/// running and parked on that wait before the notify goes out.
pub enum ClientEvent {
    /// The client was admitted a slot; allocates its entity, fills `.name`
    /// in from the userinfo the netcode already sanitized, and runs
    /// `CodeCallback_PlayerConnect` on it. The name travels with the event
    /// because the callback reads it (`dm.gsc`'s `logPrint("J;" + ... +
    /// self.name)`), and the object table has nowhere else to get it.
    Connect { slot: usize, name: String },
    /// `ClientBegin`, raised when the client enters the world; notifies
    /// the parked callback.
    Begin(usize),
    /// The client is gone; runs `CodeCallback_PlayerDisconnect` and frees
    /// the entity.
    Disconnect(usize),
}

/// One `self spawn(origin, angles)` the script performed, by client slot.
/// The other direction from `ClientEvent`, and queued for the same reason:
/// the client's sim lives in `Server`, which a builtin cannot reach.
pub struct SpawnRequest {
    pub slot: usize,
    pub origin: [f32; 3],
    pub yaw_deg: f32,
    /// `sessionstate == "playing"`, which is what decides the sim's mode.
    pub player: bool,
}

/// One edge a weapon builtin made in a client's playerstate, queued for the
/// same reason `SpawnRequest` is: the sim lives in `Server`, out of a
/// builtin's reach. `ps.weapons`, `ps.weaponslots` and the viewmodel are not
/// here; those are state the host owns and `Server` mirrors every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponOp {
    SetClip {
        clip_index: usize,
        rounds: i16,
    },
    SetAmmo {
        ammo_index: usize,
        rounds: i16,
    },
    /// `takeAllWeapons`, whose host half (clearing `client_weapons`) is the
    /// mirror's; this is the playerstate half, both arrays emptied. No
    /// builtin pushes it yet.
    TakeAll,
    /// `setSpawnWeapon`: `ps.weapon` set outright, with the machine reset to
    /// ready, which is what a spawn hands a player (object model doc,
    /// section 20).
    SetCurrent(u8),
    /// `switchToWeapon`: the same change a usercmd's weapon byte asks for, so
    /// it goes through the putaway and the raise rather than teleporting the
    /// weapon into the player's hands (combat doc, section 1.8).
    SwitchTo(u8),
}

pub struct GameHost {
    pub configstrings: Vec<String>,
    pub ents: ObjectTable,
    /// Client lifecycle events the netcode raised, drained by `run_frame`
    /// before the think pass. Queued rather than called inline for the same
    /// reason `damage` is: a builtin must never reenter the VM.
    pub client_events: Vec<ClientEvent>,
    /// Per-client server commands the script asked for, by client slot,
    /// drained by `Server` after `run_frame`. A builtin cannot reach the
    /// netchan, so it queues, the same reason `damage` does.
    pub client_commands: Vec<(usize, String)>,
    /// Spawns the script performed this frame, drained by `Server` after
    /// `run_frame` the way `client_commands` is.
    pub client_spawns: Vec<SpawnRequest>,
    /// Each client's weapons, by slot. `giveWeapon` and `setSpawnWeapon`
    /// write it, `spawn` clears it, and `Server` copies it into the client's
    /// sim every frame — the same re-read-it-each-frame arrangement the
    /// configstring table has, because a thread past a `wait` can still
    /// change it.
    pub client_weapons: Vec<crate::weapons::PlayerWeapons>,
    /// Each client's viewmodel, as the model configstring index retail's
    /// `setViewmodel` stores on the `gclient_t` (object model doc, section
    /// 20). Mirrored into the sim every frame the way `client_weapons` is.
    pub client_viewmodel: Vec<i32>,
    /// What the weapon builtins did to a client's ammo and current weapon,
    /// by slot, in call order. Unlike `client_weapons` these are edges rather
    /// than state -- `ps.ammoclip` is spent by firing, so re-applying a full
    /// clip every frame would make the weapon bottomless -- so `Server`
    /// drains them after `run_frame` and applies each once.
    pub client_weapon_ops: Vec<(usize, WeaponOp)>,
    /// The map's weapon table, for the fields the builtins need: the ammo and
    /// clip indexes an op addresses, and the rounds it hands out.
    pub weapons: std::rc::Rc<crate::weapons::WeaponTable>,
    /// Damage the script asked for, drained after `run_frame` by stage 6.
    /// A builtin must never reenter the VM, so a callback becomes a queued
    /// event (the design's "callbacks cannot run inline").
    pub damage: Vec<DamageEvent>,
    /// Runtime configstring slot allocators, one per engine indexer
    /// (`G_ModelIndex` and its siblings).
    pub allocators: Allocators,
    /// The cvar table. `getCvar` and friends read it, `setCvar` and
    /// `makeCvarServerInfo` write it, and the 140/204 configstring mirror is
    /// rebuilt from it. `Server` seeds it at load and reads it back every
    /// frame, the same single-owner arrangement the configstring table has.
    pub cvars: crate::cvars::Cvars,
    /// The map's collision, once one exists. `None` in every unit test and
    /// on a fresh `GameHost`; `bulletTrace` traces against it when present
    /// and reports a clean miss when it is not. Stage 10 is what sets it.
    pub world: Option<std::rc::Rc<crate::world::World>>,
    /// `randomFloat`'s generator state: xorshift64*, so a draw is a real
    /// uniform value and reproducible from a seed. Retail seeds from the
    /// level clock; nothing here needs to match its sequence, only to be a
    /// real draw.
    pub rng: u64,
    /// Every `logPrint` line, in order. Retail's `logPrint` writes to the
    /// server's `games_mp.log`; this is where that will come from, and it is
    /// what lets a test replay a retail capture.
    pub script_log: Vec<String>,
    /// The level clock in milliseconds: `getTime()` reads it, and it is set
    /// by `ScriptRuntime::load` at map load and by `run_frame` every frame
    /// after. `getTime`'s real units are unmeasured (`probe_cvar` only
    /// established non-negative); this is the reading chosen. A later task's
    /// deferred entity free also needs the level clock rather than a
    /// per-call timestamp, which is why this lives on the host instead of
    /// being threaded through `get_time`'s own call.
    pub level_time_ms: i32,
    /// Set by `exitLevel()`, drained and logged once per frame by
    /// `run_frame`. No stage in this sub-project acts on it; stage 6 ("the
    /// score limit ends the map") is where it does.
    pub exit_level: bool,
    /// Who owns a client's name, from `setClientNameMode`. Nothing reads it
    /// until clients exist: both of retail's readers are client code.
    pub client_name_mode: builtins::cvar::ClientNameMode,
    /// The registered-item bitset. `precacheItem` writes it and mirrors it
    /// into configstring 8 (`crate::items`); nothing else reads or writes
    /// it directly.
    pub items: crate::items::Items,
    /// The mounted paks, once there are any. `register_item` reads a
    /// weapon's models out of its weapon file through this; `None` on a
    /// fresh host, so a unit test that mounts nothing registers the bit and
    /// precaches no weapon model.
    pub fs: Option<std::rc::Rc<vcod_common::pk3::Pk3Fs>>,
    /// Each turret's settled barrel pitch, by entity, from the sweep
    /// `crate::game::spawn::settle_turret_pitch` runs at map load. `wire.rs`
    /// puts it on the wire as `angles2[0]`.
    pub turret_pitch: std::collections::HashMap<EntId, f32>,
}

/// Fixed non-zero xorshift64* seed. Any non-zero constant works; a zero
/// state is the one xorshift degenerates on.
const RNG_SEED: u64 = 0x9e37_79b9_7f4a_7c15;

impl GameHost {
    pub fn new(configstrings: Vec<String>) -> GameHost {
        GameHost {
            configstrings,
            ents: ObjectTable::new(),
            client_events: Vec::new(),
            client_commands: Vec::new(),
            client_spawns: Vec::new(),
            client_weapons: vec![crate::weapons::PlayerWeapons::default(); MAX_CLIENTS],
            client_viewmodel: vec![0; MAX_CLIENTS],
            client_weapon_ops: Vec::new(),
            weapons: std::rc::Rc::new(crate::weapons::WeaponTable::empty()),
            damage: Vec::new(),
            allocators: Allocators::new(),
            cvars: crate::cvars::Cvars::new(),
            world: None,
            rng: RNG_SEED,
            script_log: Vec::new(),
            level_time_ms: 0,
            exit_level: false,
            items: crate::items::Items::new(),
            client_name_mode: builtins::cvar::ClientNameMode::default(),
            fs: None,
            turret_pitch: std::collections::HashMap::new(),
        }
    }

    /// `RegisterItem` (0x4e504): the registration bit, the alt-fire link,
    /// the item's own two model fields and configstring 8. Every caller
    /// goes through here rather than `Items::register`, so an item takes
    /// its model slots wherever it is registered -- at spawn for a placed
    /// weapon, in `precacheItem` for the script's own list.
    ///
    /// A chained alt mode gets the same model pair as the item itself,
    /// which retail may not: it reads a different struct there (object
    /// model doc, section 15, M3). No stock alt-fire file carries a model
    /// the base does not, so the two readings cannot be told apart.
    pub fn register_item(&mut self, name: &str) {
        for item in self.items.register(name) {
            for model in crate::items::item_models(self.fs.as_deref(), item) {
                if let Err(e) =
                    self.allocators
                        .index(&mut self.configstrings, CsRange::Model, &model)
                {
                    log::warn!("gsc: item model {model:?} not indexed: {e:?}");
                }
            }
        }
        self.configstrings[8] = self.items.bitstring();
    }

    /// A uniform draw in `[0, 1)`. xorshift64*, same shape as `Server`'s own
    /// `rand()` (`server.rs`) minus its glibc `rand()`-compatible masking,
    /// which `randomFloat` has no reason to match.
    pub fn rand_unit(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let draw = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        draw as f32 / u64::MAX as f32
    }
}

impl Host for GameHost {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind> {
        // `resolve_folded`, not `resolve`: the atom carries the spelling the
        // script used (`setCullFog`), and dispatch matches the folded form.
        // Owned rather than borrowed: the entity family needs `cx` mutably,
        // which a borrow still held from `resolve_folded` would block.
        let folded = cx.resolve_folded(name).to_string();
        if let Some(f) = builtins::entity::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::math::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::fx::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::hud::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::sound::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::attach::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::client::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::combat::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::mover::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::cvar::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        if let Some(f) = builtins::precache::lookup(&folded) {
            return f(self, cx, recv, args);
        }
        match folded.as_str() {
            "setcullfog" => builtins::env::set_cull_fog(&mut self.configstrings, cx, args),
            "ambientplay" => builtins::env::ambient_play(&mut self.configstrings, cx, args),
            "println" | "iprintln" | "logprint" => builtins::io::print_line(self, cx, args),
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
            Route::Engine {
                slot,
                ty: FieldType::Enum(names),
            } => match e.engine[slot] {
                Value::Int(i) => match names.get(i as usize) {
                    Some(n) => Value::String(cx.intern_exact(n)),
                    None => Value::Undefined,
                },
                other => other,
            },
            Route::Engine { slot, .. } => e.engine[slot],
            // `e.client` is `Some` only for an entity `spawn_client` made:
            // that is the real test, not a number range that would merely
            // coincide with it. Retail errors on a null `ent->client`; a
            // read has no error channel, so a client-less entity reads
            // undefined and the write path carries the error.
            Route::Client(i) => match &e.client {
                Some(c) => c[i],
                None => Value::Undefined,
            },
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
            Route::Engine {
                slot,
                ty: FieldType::Enum(names),
            } => {
                e.engine[slot] = enum_index(cx, names, value)?;
                Ok(())
            }
            Route::Engine { slot, ty } => {
                if !type_accepts(ty, value) {
                    return Err(ErrorKind::BadType("wrong type for an engine field"));
                }
                e.engine[slot] = value;
                Ok(())
            }
            // Retail's null `ent->client` check: an entity `spawn_client`
            // never touched has no `client` store to write into.
            Route::Client(i) => {
                let Some(c) = e.client.as_mut() else {
                    return Err(ErrorKind::BadType("that entity has no client"));
                };
                let ty = fields::CLIENT_FIELDS[i].ty;
                if !type_accepts(ty, value) {
                    return Err(ErrorKind::BadType("wrong type for a client field"));
                }
                c[i] = value;
                Ok(())
            }
            Route::Script => {
                let s = e.script;
                cx.set_field(s, field, value);
                Ok(())
            }
        }
    }
}

/// The index a `FieldType::Enum` field stores for the string script wrote.
/// Retail's shared setter (0x4af80) raises a script error listing the whole
/// table when the name is not in it, and takes nothing but a string.
fn enum_index(cx: &Cx, names: &[&str], value: Value) -> Result<Value, ErrorKind> {
    let Value::String(s) = value else {
        return Err(ErrorKind::BadType(
            "that field takes one of a fixed set of names",
        ));
    };
    match names.iter().position(|n| *n == cx.resolve(s)) {
        Some(i) => Ok(Value::Int(i as i32)),
        None => Err(ErrorKind::BadType("not one of that field's names")),
    }
}

/// Which `Value` shapes each field type accepts. Retail converts in
/// `Scr_SetGenericField`; we refuse a mismatch instead, so a script bug
/// surfaces where retail would silently store a zero.
fn type_accepts(ty: FieldType, v: Value) -> bool {
    use FieldType::*;
    match ty {
        // `Enum` never reaches here; `set_field` converts it first.
        Int | Enum(_) => matches!(v, Value::Int(_) | Value::Undefined),
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

    /// `BUILTINS` plus every family's `NAMES` drives the load-time pre-scan
    /// (`is_builtin`) while `builtin`'s dispatch chain answers the same
    /// names; a name dropped from one and not the other would make the
    /// pre-scan lie. Argument errors are fine here, `MissingBuiltin` is not.
    #[test]
    fn every_listed_builtin_dispatches() {
        let names: Vec<&str> = BUILTINS
            .iter()
            .copied()
            .chain(builtins::entity::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::math::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::fx::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::hud::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::sound::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::attach::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::client::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::combat::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::mover::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::cvar::NAMES.iter().map(|(n, _)| *n))
            .chain(builtins::precache::NAMES.iter().map(|(n, _)| *n))
            .collect();
        for name in names {
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

    /// The 37 builtins `mp_pavlov`'s closure calls, from the server's own
    /// load-time census. The stage gate is a clean pre-scan on the map
    /// path, so a name dropped here is a regression the map load would hit
    /// at runtime.
    #[test]
    fn every_builtin_mp_pavlovs_closure_calls_is_answered() {
        const CENSUS: &[&str] = &[
            "anglestoforward",
            "attach",
            "bullettrace",
            "delete",
            "detachall",
            "distance",
            "getattachmodelname",
            "getattachsize",
            "getattachtagname",
            "getcvar",
            "getent",
            "getentarray",
            "getorigin",
            "getviewmodel",
            "hide",
            "isdefined",
            "isplayer",
            "istouching",
            "length",
            "loadfx",
            "movegravity",
            "notsolid",
            "playfx",
            "playfxontag",
            "playloopsound",
            "playsound",
            "radiusdamage",
            "randomfloat",
            "rotatevelocity",
            "setmodel",
            "setviewmodel",
            "show",
            "solid",
            "spawn",
            "spawnstruct",
            "vectornormalize",
            "vectortoangles",
        ];
        assert_eq!(CENSUS.len(), 37);
        let missing: Vec<_> = CENSUS.iter().filter(|n| !is_builtin(n)).collect();
        assert!(missing.is_empty(), "not answered: {missing:?}");
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

    /// A HUD enum field stores retail's index and reads back the name, the
    /// set/get hook pair retail hangs on the field record. The stock
    /// `startGame` writes `level.clock.alignX = "center"`, so a string has
    /// to be what the field takes -- but `"center"` alone proves nothing
    /// about the order, since it is index 1 of three either way, so every
    /// name in all three tables is checked against its index here. Nothing
    /// else in the tree reads the stored index yet.
    #[test]
    fn the_hud_enum_fields_store_retails_indices_and_read_back_names() {
        const TABLES: &[(&str, [&str; 3])] = &[
            ("font", ["default", "bigfixed", "smallfixed"]),
            ("alignx", ["left", "center", "right"]),
            ("aligny", ["top", "middle", "bottom"]),
        ];
        let (mut vm, mut host) = fixture();
        let h = vm.with_cx(|cx| host.ents.spawn_hud_elem(cx).unwrap());
        vm.with_cx(|cx| {
            for (field, names) in TABLES {
                let f = cx.intern_folded(field);
                for (i, name) in names.iter().enumerate() {
                    let v = cx.intern_exact(name);
                    host.set_field(cx, h, f, Value::String(v)).unwrap();
                    assert_eq!(
                        host.ents.get(h).unwrap().engine[hud_slot(field)],
                        Value::Int(i as i32),
                        "{field} = {name:?} is retail's index {i}"
                    );
                    let Value::String(back) = host.get_field(cx, h, f) else {
                        panic!("{field} reads back as a name");
                    };
                    assert_eq!(cx.resolve(back), *name);
                }
                // A name from another one of the three tables is still not
                // one of this field's, which is retail's error too.
                let wrong = cx.intern_exact("middle");
                assert!(
                    *field == "aligny" || host.set_field(cx, h, f, Value::String(wrong)).is_err(),
                    "{field} took a name that is not in its table"
                );
            }
        });
    }

    /// One HUD field's dense slot, for the assertion above.
    fn hud_slot(name: &str) -> usize {
        match fields::route_hud(name) {
            Route::Engine { slot, .. } => slot,
            _ => panic!("{name} is an engine field"),
        }
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

    /// A client field reads back what was written, and the same field on a
    /// map entity is still an error, because retail has no `gclient_t` there.
    /// The map-entity refusal is pinned to its exact message, not just
    /// `is_err()`: `set_field` has two distinct refusals now (a nonexistent
    /// entity's "no such entity" and a client-less entity's "that entity has
    /// no client"), and this test exists to exercise the second, not either.
    #[test]
    fn a_client_field_round_trips_and_a_map_entity_still_refuses_one() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let f = cx.intern_folded("sessionteam");
            let v = Value::String(cx.intern_exact("allies"));
            host.set_field(cx, c, f, v).unwrap();
            assert_eq!(host.get_field(cx, c, f), v);

            let m = host.ents.spawn(cx).unwrap();
            assert_eq!(
                host.set_field(cx, m, f, v).unwrap_err(),
                ErrorKind::BadType("that entity has no client")
            );
        });
    }

    /// `Route::Client(i)` and `Route::Engine{slot}` (from `ENTITY_FIELDS`)
    /// used to share one `GEntity.engine` array on a client entity, with two
    /// unrelated index spaces landing in the same cells: client index 1
    /// (`sessionteam`) and engine slot 1 (`origin`) collided. `client` is
    /// now a separate store, so setting both on one client entity must not
    /// let either overwrite the other, in either write order.
    #[test]
    fn a_client_field_and_an_entity_field_do_not_alias_on_a_client_entity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let sessionteam = cx.intern_folded("sessionteam");
            let origin = cx.intern_folded("origin");
            let team = Value::String(cx.intern_exact("allies"));
            let pos = Value::Vector([1.0, 2.0, 3.0]);

            let a = host.ents.spawn_client(cx, 0).unwrap();
            host.set_field(cx, a, sessionteam, team).unwrap();
            host.set_field(cx, a, origin, pos).unwrap();
            assert_eq!(host.get_field(cx, a, sessionteam), team);
            assert_eq!(host.get_field(cx, a, origin), pos);

            let b = host.ents.spawn_client(cx, 1).unwrap();
            host.set_field(cx, b, origin, pos).unwrap();
            host.set_field(cx, b, sessionteam, team).unwrap();
            assert_eq!(host.get_field(cx, b, origin), pos);
            assert_eq!(host.get_field(cx, b, sessionteam), team);
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
