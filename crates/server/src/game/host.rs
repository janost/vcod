//! The CoD side of `vcod_gsc::Host`: builtin dispatch and entity field
//! routing. A field read or write is routed through the retail field
//! tables: engine-backed goes to the entity's typed slot, client-tagged
//! goes to the entity's own `client` store when it has one and errors
//! otherwise, everything else goes to the entity's own struct in the VM
//! heap.

use crate::configstrings::{Allocators, CsRange};
use crate::game::builtins;
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
    /// `takeAllWeapons`, whose host half (clearing `client_weapons`) reaches
    /// the sim through the frame mirror; this is the playerstate half, the
    /// ammo and clip arrays emptied.
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

/// A client's health as the script sees it: `self.health`, `self.maxhealth`
/// and whether `finishPlayerDamage` has killed it since its last spawn. The
/// host is the owner; `Server` mirrors it into the sim every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vitals {
    pub health: i32,
    pub max_health: i32,
    pub dead: bool,
}

impl Default for Vitals {
    /// A connecting client: retail's `gclient_t` is zeroed, and the lone
    /// spectator capture reads `stats[2]` 0 until the gametype's spawn
    /// writes `self.maxhealth` (docs/protocol-1.1.md, "Block 1").
    fn default() -> Self {
        Vitals {
            health: 0,
            max_health: 0,
            dead: false,
        }
    }
}

/// One thing `finishPlayerDamage` did to a client that its sim has to act
/// on, queued for the same reason `WeaponOp` is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimOp {
    Damaged {
        damage: i32,
        point: [f32; 3],
        /// The damage direction, normalised; all zero when the callback was
        /// handed none.
        dir: [f32; 3],
        knockback: bool,
        attacker: Option<usize>,
        /// Where the attacker's entity stood, for the dead yaw.
        attacker_origin: Option<[f32; 3]>,
        fatal: bool,
    },
}

pub struct GameHost {
    pub configstrings: Vec<String>,
    pub ents: ObjectTable,
    /// Client lifecycle events the netcode raised, drained by `run_frame`
    /// before the think pass: a callback run inline from `SV_ClientCommand`
    /// would reenter the VM mid-frame.
    pub client_events: Vec<ClientEvent>,
    /// Per-client server commands the script asked for, by client slot,
    /// drained by `Server` after `run_frame`. A builtin cannot reach the
    /// netchan, so it queues, the same reason `client_events` does.
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
    /// Each client's health, authoritative here: the `health` and
    /// `maxhealth` accessors on a client entity read and write it, and
    /// `Server` mirrors it into the sim every frame.
    pub client_vitals: Vec<Vitals>,
    /// Each client's last usercmd buttons, mirrored in by `Server` before
    /// the frame, for `useButtonPressed`.
    pub client_buttons: Vec<u8>,
    /// Each client's entity state as the tick's moves left it, mirrored in by
    /// `Server::replay_moves` before the script frame. `cloneplayer` copies
    /// the slot's entry into the body queue; nothing else reads it.
    pub client_entity_states: Vec<Option<vcod_common::net::msg::EntityState>>,
    /// What `finishPlayerDamage` did to a client this frame, drained by
    /// `Server` after `run_frame` and applied to the sim once each.
    pub client_sim_ops: Vec<(usize, SimOp)>,
    /// The map's weapon table, for the fields the builtins need: the ammo and
    /// clip indexes an op addresses, and the rounds it hands out.
    pub weapons: std::rc::Rc<crate::weapons::WeaponTable>,
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
    /// The events raised this frame, put on the wire as temp entities and
    /// dropped by the snapshot build: retail frees a `G_TempEntity` the
    /// frame after it is sent.
    pub temp_entities: Vec<crate::game::temp_entity::TempEntity>,
    /// The eight corpse entities `cloneplayer` fills
    /// (`crate::game::bodies`).
    pub bodies: crate::game::bodies::BodyQueue,
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
            client_vitals: vec![Vitals::default(); MAX_CLIENTS],
            client_buttons: vec![0; MAX_CLIENTS],
            client_entity_states: vec![None; MAX_CLIENTS],
            client_sim_ops: Vec::new(),
            weapons: std::rc::Rc::new(crate::weapons::WeaponTable::empty()),
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
            temp_entities: Vec::new(),
            bodies: crate::game::bodies::BodyQueue::new(crate::game::bodies::BODY_QUEUE_SIZE),
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

    /// A uniform draw in `[0, 1)` off the host's own state.
    pub fn rand_unit(&mut self) -> f32 {
        rand_unit(&mut self.rng)
    }

    /// The three HUD fields whose retail setter is not a plain store, and
    /// `None` for every other name so the caller falls through to the
    /// generic path. `color` (0x4b03c) and `alpha` (0x4c1fc) write byte
    /// lanes of one packed word; `label` (0x4c400) takes a localized string
    /// and stores what `G_LocalizedStringIndex` answers.
    fn set_hud_field(
        &mut self,
        cx: &Cx,
        ent: EntId,
        field: Atom,
        value: Value,
    ) -> Option<Result<(), ErrorKind>> {
        let lane = |v: Value| match v {
            Value::Int(i) => Some(fields::hud_color_byte(i as f32)),
            Value::Float(f) => Some(fields::hud_color_byte(f)),
            _ => None,
        };
        let word = match cx.resolve_folded(field) {
            "color" => {
                let Value::Vector(v) = value else {
                    return Some(Err(ErrorKind::BadType("color takes a vector")));
                };
                let old = self.ents.get(ent).map_or(0, hud_color_word);
                (old & 0xff00_0000)
                    | fields::hud_color_byte(v[0])
                    | (fields::hud_color_byte(v[1]) << 8)
                    | (fields::hud_color_byte(v[2]) << 16)
            }
            "alpha" => {
                let Some(a) = lane(value) else {
                    return Some(Err(ErrorKind::BadType("alpha takes a number")));
                };
                let old = self.ents.get(ent).map_or(0, hud_color_word);
                (old & 0x00ff_ffff) | (a << 24)
            }
            "label" => {
                let (Value::String(a) | Value::Localized(a)) = value else {
                    return Some(Err(ErrorKind::BadType("label takes a string")));
                };
                let name = cx.resolve(a).to_string();
                let index = match self
                    .allocators
                    .localized_index(&mut self.configstrings, &name)
                {
                    Ok(i) => i,
                    Err(e) => return Some(Err(e)),
                };
                return Some(self.store_hud_slot(ent, "label", Value::Int(index)));
            }
            _ => return None,
        };
        Some(self.store_hud_slot(ent, "color", Value::Int(word as i32)))
    }

    /// One HUD engine slot by name, for the setters that compute their own
    /// stored value.
    fn store_hud_slot(&mut self, ent: EntId, name: &str, v: Value) -> Result<(), ErrorKind> {
        let Route::Engine { slot, .. } = fields::route_hud(name) else {
            return Err(ErrorKind::BadType("no such HUD field"));
        };
        let Some(e) = self.ents.get_mut(ent) else {
            return Err(ErrorKind::BadType("no such entity"));
        };
        e.engine[slot] = v;
        Ok(())
    }
}

/// The packed RGBA word behind `color` and `alpha`, both of which share one
/// engine slot because they share `hudelem_t+0x1c`.
fn hud_color_word(e: &crate::game::entity::GEntity) -> u32 {
    match fields::route_hud("color") {
        Route::Engine { slot, .. } => match e.engine[slot] {
            Value::Int(i) => i as u32,
            _ => 0,
        },
        _ => 0,
    }
}

/// A uniform draw in `[0, 1)` off [`vcod_common::rng::xorshift`], the step
/// every draw on the server shares, minus the glibc `rand()`-compatible
/// masking `Server::rand` puts on top of it and `randomFloat` has no reason
/// to match. A free function so the bullet spread (`crate::game::combat`)
/// draws from `Server`'s state the same way.
pub fn rand_unit(state: &mut u64) -> f32 {
    vcod_common::rng::xorshift(state) as f32 / u64::MAX as f32
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
        // A player's health lives on the host, not in the entity's engine
        // slot: retail's `health` setter (`Scr_SetHealth`) writes
        // `ps.stats[0]` and `maxhealth`'s writes `sess.maxHealth`, which is
        // what the wire and the damage path read (docs/protocol-1.1.md,
        // "Block 1"). A map entity's `health` is still its own slot.
        if e.client.is_some() {
            match cx.resolve_folded(field) {
                "health" => return Value::Int(self.client_vitals[ent.0 as usize].health),
                "maxhealth" => return Value::Int(self.client_vitals[ent.0 as usize].max_health),
                _ => {}
            }
        }
        if ent.0 >= FIRST_HUD_ELEM {
            // The two lanes of the packed colour word, each its own getter
            // in retail (0x4c1ac, 0x4c27c).
            match cx.resolve_folded(field) {
                "color" => {
                    let w = hud_color_word(e);
                    return Value::Vector([
                        fields::hud_color_float(w & 0xff),
                        fields::hud_color_float((w >> 8) & 0xff),
                        fields::hud_color_float((w >> 16) & 0xff),
                    ]);
                }
                "alpha" => {
                    return Value::Float(fields::hud_color_float(hud_color_word(e) >> 24));
                }
                _ => {}
            }
        }
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
        if ent.0 >= FIRST_HUD_ELEM {
            if let Some(r) = self.set_hud_field(cx, ent, field, value) {
                return r;
            }
        }
        let route = if ent.0 >= FIRST_HUD_ELEM {
            fields::route_hud(cx.resolve_folded(field))
        } else {
            fields::route_entity(cx.resolve_folded(field))
        };
        let Some(e) = self.ents.get_mut(ent) else {
            return Err(ErrorKind::BadType("no such entity"));
        };
        if e.client.is_some() {
            let v = &mut self.client_vitals[ent.0 as usize];
            match cx.resolve_folded(field) {
                "health" => {
                    v.health = as_health(value)?;
                    return Ok(());
                }
                // The setter clamps the health down to the new maximum
                // (docs/protocol-1.1.md, "Block 1", INFERRED there).
                "maxhealth" => {
                    v.max_health = as_health(value)?;
                    v.health = v.health.min(v.max_health);
                    return Ok(());
                }
                _ => {}
            }
        }
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
                // `archivetime` is how long a client's frames are kept for a
                // killcam to replay, which vcod does not have and will not
                // (`docs/design/2026-09-02-stage6-combat-design.md`). Retail's
                // own code trims the value down to what it can actually serve;
                // storing zero is that trim taken to its end, and it is what
                // makes every stock gametype's `if(self.archivetime <= delay)`
                // fall through to `respawn()` instead of into the killcam.
                c[i] = match fields::CLIENT_FIELDS[i].name {
                    "archivetime" => Value::Int(0),
                    _ => value,
                };
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

/// The integer a health field takes; retail's int setter truncates a float.
fn as_health(value: Value) -> Result<i32, ErrorKind> {
    match value {
        Value::Int(i) => Ok(i),
        Value::Float(f) => Ok(f as i32),
        _ => Err(ErrorKind::BadType("health takes a number")),
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

    /// `color` and `alpha` are byte lanes of one word, so each has to leave
    /// the other's lane alone, and both read back as the 0..1 numbers script
    /// wrote. The record starts opaque white, which is what the retail round
    /// clock carries on the wire.
    #[test]
    fn color_and_alpha_share_one_packed_word() {
        let (mut vm, mut host) = fixture();
        let h = vm.with_cx(|cx| host.ents.spawn_hud_elem(cx).unwrap());
        vm.with_cx(|cx| {
            let color = cx.intern_folded("color");
            let alpha = cx.intern_folded("alpha");
            let word = |host: &GameHost| host.ents.get(h).unwrap().engine[hud_slot("color")];
            assert_eq!(word(&host), Value::Int(-1), "opaque white");

            // dm.gsc's killcam bars: half alpha, colour untouched.
            host.set_field(cx, h, alpha, Value::Float(0.5)).unwrap();
            assert_eq!(word(&host), Value::Int(0x80ff_ffffu32 as i32));
            assert_eq!(host.get_field(cx, h, alpha), Value::Float(128.0 / 255.0));

            host.set_field(cx, h, color, Value::Vector([1.0, 0.0, 0.0]))
                .unwrap();
            assert_eq!(word(&host), Value::Int(0x8000_00ffu32 as i32), "red lane 0");
            assert_eq!(
                host.get_field(cx, h, color),
                Value::Vector([1.0, 0.0, 0.0]),
                "the alpha lane stays out of the vector"
            );
            assert!(host.set_field(cx, h, color, Value::Int(1)).is_err());
        });
    }

    /// `label` takes a localized string and stores the index the client
    /// resolves through configstring `1244 + n`, not the string.
    #[test]
    fn a_hud_label_stores_a_localized_index() {
        let (mut vm, mut host) = fixture();
        let h = vm.with_cx(|cx| host.ents.spawn_hud_elem(cx).unwrap());
        vm.with_cx(|cx| {
            let label = cx.intern_folded("label");
            let key = Value::Localized(cx.intern_exact("MPSCRIPT_KILLCAM"));
            host.set_field(cx, h, label, key).unwrap();
            assert_eq!(host.get_field(cx, h, label), Value::Int(1));
            assert_eq!(host.configstrings[1245], "MPSCRIPT_KILLCAM");
        });
    }

    /// A HUD enum field stores retail's index and reads back the name, the
    /// set/get hook pair retail hangs on the field record. The stock
    /// `startGame` writes `level.clock.alignX = "center"`, so a string has
    /// to be what the field takes -- but `"center"` alone proves nothing
    /// about the order, since it is index 1 of three either way, so every
    /// name in all three tables is checked against its index here.
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

    /// A numeric client field reads 0 before anything writes it, which is
    /// what retail's getter finds on the `gclient_t` `ClientConnect` zeroed.
    /// It is what lets `self.deaths++` and `attacker.score++` run on a
    /// client that has neither yet; `.pers` is still the array
    /// `spawn_client` seeded, and a string field is still undefined.
    #[test]
    fn a_numeric_client_field_starts_at_zero() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let read = |host: &mut GameHost, cx: &mut Cx, f: &str| {
                let atom = cx.intern_folded(f);
                host.get_field(cx, c, atom)
            };
            assert_eq!(read(&mut host, cx, "score"), Value::Int(0));
            assert_eq!(read(&mut host, cx, "deaths"), Value::Int(0));
            assert_eq!(read(&mut host, cx, "spectatorclient"), Value::Int(0));
            assert_eq!(read(&mut host, cx, "archivetime"), Value::Float(0.0));
            assert_eq!(read(&mut host, cx, "statusicon"), Value::Undefined);
            assert!(matches!(read(&mut host, cx, "pers"), Value::Array(_)));
        });
    }

    /// `archivetime` stores 0 whatever the script wrote: nothing archives
    /// frames for a killcam, so a gametype's `if(self.archivetime <= delay)`
    /// has to fall through to `respawn()` rather than stall waiting for a
    /// replay that never comes.
    #[test]
    fn archivetime_reads_back_zero_whatever_was_written() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let atom = cx.intern_folded("archivetime");
            host.set_field(cx, c, atom, Value::Int(9)).unwrap();
            assert_eq!(host.get_field(cx, c, atom), Value::Int(0));
            host.set_field(cx, c, atom, Value::Float(2.5)).unwrap();
            assert_eq!(host.get_field(cx, c, atom), Value::Int(0));
            // The neighbouring numeric fields still store what they are given.
            let other = cx.intern_folded("spectatorclient");
            host.set_field(cx, c, other, Value::Int(3)).unwrap();
            assert_eq!(host.get_field(cx, c, other), Value::Int(3));
        });
    }
}
