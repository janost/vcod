//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use vcod_common::asset_source::{AssetSource, RawFile};

use crate::game::builtins::attach::ATTACH_SLOTS;
use crate::game::builtins::set::BuiltinSet;
use crate::game::host::{ClientEvent, GameHost, SpawnRequest};
use crate::game::item::Activate;
use crate::game::profile::{HostProfile, WeaponNumbering};
use crate::game::spawn::spawn_entities_from_string;
use crate::game::turret::{TurretOp, TurretShot};
use vcod_common::mapworld::MapWorld;
use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{EntId, Loader, ScriptSource, Target, Value, Vm};

mod clients;
mod damage;
mod frame;
mod load;
mod movers;
mod objectives;
mod testing;
mod touch;
mod turret;
pub use frame::{EntityPass, PassStop};
pub(crate) use load::PakScripts;
pub use load::{Carry, CompiledScripts, GametypeScripts, LevelMap};
pub use objectives::{
    BasBase, Bounds, CtfFlag, DomFlag, HqRadio, ReObjective, ReObjectives, SdObjectives,
    UoObjectives,
};

/// Every gametype script includes this file, and it is where the engine's
/// entry points into script live, `CodeCallback_StartGameType` among them.
pub(crate) const CALLBACK_SETUP: &str = "maps/mp/gametypes/_callbacksetup";

/// The event `Callback_PlayerConnect` parks on before it lets the client
/// into the world. A misspelt notify does not error, it hangs the thread,
/// so the literal lives here with a test on it.
pub(crate) const BEGIN_NOTIFY: &str = "begin";

/// The event every gametype's team-join loop parks on, once the connect
/// callback has opened the first menu. Same silent failure as
/// `BEGIN_NOTIFY`, same test.
const MENURESPONSE_NOTIFY: &str = "menuresponse";

/// The menu a `menuresponse` names: a slot of `CsRange::Menu`, read back at
/// the notify, or text `Cmd_MenuResponse_f` passes as it is.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuRef {
    Index(usize),
    Name(String),
}

/// A script value as text, through the same `%g` rendering string
/// concatenation uses. A value with no rendering (`undefined`, an array, an
/// entity) comes back debug-rendered, so a failed assertion says what the
/// field actually held rather than an empty string.
fn render(cx: &vcod_gsc::Cx, v: Value) -> String {
    cx.format_number(v).unwrap_or_else(|| format!("{v:?}"))
}

/// What a client entity wears, by name: `.model` and the attachments with
/// their ignoreCollision bits. `None` without a model.
pub(crate) fn wear(
    host: &mut GameHost,
    cx: &mut vcod_gsc::Cx,
    ent: EntId,
) -> Option<crate::game::hitrig::Assembly> {
    use crate::game::hitrig::{Assembly, model_name};
    use vcod_gsc::Host;
    let atom = cx.intern_folded("model");
    let v = host.get_field(cx, ent, atom);
    let body = model_name(&render(cx, v));
    if body.is_empty() {
        return None;
    }
    let e = host.ents.get(ent)?;
    let attachments = e
        .attachments
        .iter()
        .take(ATTACH_SLOTS)
        .map(|(m, t)| {
            let tag = cx.resolve(*t).to_string();
            (model_name(cx.resolve(*m)), (!tag.is_empty()).then_some(tag))
        })
        .collect();
    Some(Assembly {
        body,
        attachments,
        ignore_collision: e.attach_ignore_collision,
    })
}

/// `clientState.team`, the roster's 2-bit team field. The four values are
/// what retail's `sessionteam` setter writes for the four names script may
/// assign it (`docs/research/clientstate-wire-format.md`).
pub const TEAM_NONE: i32 = 0;
pub const TEAM_AXIS: i32 = 1;
pub const TEAM_ALLIES: i32 = 2;
pub const TEAM_SPECTATOR: i32 = 3;

/// The highest `ps.pm_type` `G_TouchTriggers` runs its pass for
/// (docs/research/cod11-gsc-object-model.md 8.2).
const TOUCH_MAX_PM_TYPE: i32 = 1;
/// What `G_MoverPush`'s crush arm hands `G_Damage` (game.mp 0x55617).
const CRUSH_DAMAGE: i32 = 99999;

/// `serverCursorHint` for a usable turret, slot 6 of `hintStrings`
/// (`docs/research/cod11-turrets.md` 4.3).
const HINT_MG42: i32 = 6;
/// `hintActivate`, the cursor hint a usable vehicle shows (coduo-vehicles.md 1).
const HINT_ACTIVATE: i32 = 2;
/// United Offensive's `HINT_LMG`, a mount ahead of a held LMG
/// (coduo-movement.md 11).
pub const HINT_LMG: i32 = 7;
/// How far ahead of the feet the LMG arm probes (game.mp.uo 0xa1098).
const LMG_PROBE_AHEAD: f32 = 15.0;

/// What [`ScriptRuntime::cursor_hints`] wrote: `serverCursorHint`,
/// `serverCursorHintVal` and `serverCursorHintString` (`None` leaves the
/// string as it was, `Some(-1)` is none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorHint {
    pub hint: i32,
    pub val: i32,
    pub string: Option<i32>,
}

/// The held weapon when it is a United Offensive LMG, for the hint pass's
/// mount probe.
#[derive(Clone, Copy)]
pub struct LmgHint<'a> {
    /// `r.currentOrigin`, the feet.
    pub origin: [f32; 3],
    pub yaw_deg: f32,
    /// `pm_flags` 0x20: the bipod is down.
    pub sighted: bool,
    /// The weapon file's `useHintString`, empty on every stock LMG.
    pub hint_string: Option<&'a str>,
    /// [`crate::game::trigger::mount_boxes`].
    pub boxes: &'a [vcod_common::movetrace::ContentsBox],
}
/// A manned gun's shot and its cooldown alias, both on the gun's own ring
/// (`docs/research/cod11-events-and-fx.md` section 1, turrets doc 6.3, 6.4).
use vcod_common::net::event_ids::EV_FIRE_WEAPON_MG42;
/// UO's fire-event bit for a turret round the server traced into no player.
const TURRET_ROUND_UNTRACED: i32 = 0x80;
pub(crate) use vcod_common::net::event_ids::EV_SOUND_ALIAS;

/// Owns one map's loaded script closure, the `Vm` it lives in, and the
/// `GameHost` the object table and configstrings live in. The host has to
/// outlive map load: a thread can spawn an entity in one frame and read it
/// back in the next, so a host that died at the end of `load` would throw
/// the object table away under it.
///
/// Always a fresh `Vm`/`GameHost` per map, never a reload into a live one:
/// `Vm::install` rejects a duplicate `FuncRef` rather than silently
/// overwriting, so re-loading a shared gametype script into a `Vm` that
/// already has it would fail loudly. Every stock gametype references the
/// same shared scripts, so a map change that reused one long-lived `Vm`
/// would hit this on the first shared file. A map change therefore builds
/// a new `ScriptRuntime` and drops the old one; the heap and the interner
/// go with it.
pub struct ScriptRuntime {
    vm: Vm,
    pub(crate) host: GameHost,
    entry: String,
    gametype_entry: String,
    /// `None` in a test runtime that mounts no map.
    map: Option<Rc<LevelMap>>,
    scripts: Option<Rc<CompiledScripts>>,
    /// How far the script clock runs behind `level.time`: CoD2's VM steps
    /// one 50 ms tick a `G_RunFrame` (`Scr_IncTime` 0x80855e2) whatever
    /// the frame's length, so its 100 ms load frames leave it behind
    /// (cod2-protocol.md 15.8). 0 on CoD1, whose `Scr_SetTime` follows
    /// `level.time`.
    pub(crate) script_lag_ms: i32,
    /// `developer` at the load, as `Scr_Init` took it: whether
    /// `scriptUsage` prints source lines or offsets.
    developer: bool,
    /// Per slot, the bytes of new script strings its `mr` answers and vote
    /// arguments added this level, and whether its refusal was logged
    /// ([`Self::charge_client_strings`]).
    client_strings: HashMap<usize, (usize, bool)>,
}

/// vcod's: the bytes of new script strings one client's `mr` answers and
/// vote arguments may add to the interner in a level, where the host's
/// strings stay until it ends.
/// Stock menus answer with names the scripts already hold, which cost
/// nothing.
pub const CLIENT_STRING_BUDGET: usize = 64 * 1024;

impl ScriptRuntime {
    /// What `finishPlayerDamage` did to the sims this frame, drained: each
    /// op is applied once.
    pub fn take_sim_ops(&mut self) -> Vec<(usize, crate::game::host::SimOp)> {
        std::mem::take(&mut self.host.client_sim_ops)
    }

    /// What `linkTo` and `unlink` did this frame, drained the same way.
    pub fn take_link_ops(&mut self) -> Vec<(usize, crate::game::host::LinkOp)> {
        std::mem::take(&mut self.host.client_link_ops)
    }

    /// `G_ExplodeMissile`'s tail, after its blast walk
    /// (`GameHost::spawn_concussive`).
    pub fn spawn_concussive(&mut self) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.spawn_concussive(cx));
    }

    /// Any live entity's `.origin`, `None` once it has been freed. The link
    /// re-anchor reads the parent through it, and a freed parent is what
    /// releases a successful planter: `sd.gsc` never unlinks it, the
    /// bombzone's `delete()` does (object-model doc, 23.2).
    pub fn entity_origin_of(&mut self, id: EntId) -> Option<[f32; 3]> {
        use vcod_gsc::Host;
        self.host.ents.get(id)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            match host.get_field(cx, id, field) {
                Value::Vector(v) => Some(v),
                // Freedness is the `ents.get` above and nothing else: a live
                // parent whose `origin` slot reads undefined anchors at the
                // world origin, which is what `link_to` computed its offset
                // against.
                _ => Some([0.0; 3]),
            }
        })
    }

    /// The frame a frame-counted `wait` counts (`Vm::set_frame_ms`).
    pub fn set_frame_ms(&mut self, ms: i32) {
        self.vm.set_frame_ms(ms);
    }

    /// The network clock (`svs.time`) `addTestClient`'s reconnect check
    /// reads; `Server` mirrors it in before the threads run.
    pub fn set_network_time(&mut self, svs_ms: i32) {
        self.host.test_clients.svs_time_ms = svs_ms;
    }

    /// The object table as packet entities: what a snapshot carries before
    /// any per-client culling (`crates/server/src/game/wire.rs`).
    pub fn packet_entities(
        &mut self,
        p: &vcod_common::net::protocol::Cod1Wire,
    ) -> std::collections::BTreeMap<u32, vcod_common::net::msg::EntityState> {
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::wire::packet_entities(host, cx, p))
    }

    /// The entities `playSound` made `SVF_BROADCAST`, which every snapshot
    /// carries past the cull, and on 1.5 and UO those whose
    /// `r.broadcastTime` the clock `now` has not passed (on 1.5 and UO the
    /// game clock, cod15-protocol-diff.md 15).
    pub fn broadcast_entities(&self, now: i32) -> impl Iterator<Item = u32> + '_ {
        self.host
            .ents
            .iter_inuse()
            .filter(move |(_, e)| {
                let t = e.broadcast_time;
                e.broadcast || t < 0 || (t != 0 && t.wrapping_sub(now) >= 0)
            })
            .map(|(id, _)| id.0)
    }

    /// The snapshot builder's clear of a passed `r.broadcastTime`
    /// (coduo_lnxded 0x8096d53, 1.5b 0x8095dc6), which is what
    /// `SV_inSnapshot` reads until then ([`Self::in_snapshot_broadcast`]).
    pub fn expire_broadcast_times(&mut self, now: i32) {
        for e in self.host.ents.iter_inuse_mut() {
            if e.broadcast_time > 0 && e.broadcast_time.wrapping_sub(now) < 0 {
                e.broadcast_time = 0;
            }
        }
    }

    /// What `SV_inSnapshot` reads as sent past the cull: `SVF_BROADCAST`, or
    /// any `r.broadcastTime` the builder has not cleared.
    pub fn in_snapshot_broadcast(&self, n: u32) -> bool {
        let e = self
            .host
            .ents
            .handle(n)
            .and_then(|id| self.host.ents.get(id));
        e.is_some_and(|e| e.broadcast || e.broadcast_time != 0)
    }

    /// What the baseline pass writes (`crate::game::wire::baseline_entities`).
    pub fn baseline_entities(
        &mut self,
        p: &vcod_common::net::protocol::Cod1Wire,
    ) -> std::collections::BTreeMap<u32, vcod_common::net::msg::EntityState> {
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::wire::baseline_entities(host, cx, p))
    }

    /// The HUD elements one client is sent, archived array first
    /// (`crate::game::wire::hud_elems`). `team` is that client's
    /// `clientState.team`, which is what a team element is filtered on.
    pub fn hud_elems(
        &self,
        slot: usize,
        team: i32,
    ) -> (
        Vec<vcod_common::net::msg::HudElem>,
        Vec<vcod_common::net::msg::HudElem>,
    ) {
        crate::game::wire::hud_elems(&self.host, slot, team)
    }

    /// One client's copy of the objective table, stepped for this frame
    /// ([`crate::game::host::GameHost::objectives_for`]).
    pub fn objectives_for(
        &mut self,
        slot: usize,
        team: i32,
    ) -> [vcod_common::net::msg::Objective; vcod_common::net::msg::MAX_OBJECTIVES] {
        self.host.objectives_for(slot, team)
    }

    /// The configstrings the script wrote (e.g. `setCullFog`, `ambientPlay`)
    /// by the end of `load`. `Server::load_scripts` copies these back into
    /// its own table once, before the first gamestate goes out.
    pub fn configstrings(&self) -> &[String] {
        &self.host.configstrings
    }

    /// `G_TempEntity`: raises one event on a temp entity of its own
    /// (`GameHost::add_temp_entity`).
    pub fn push_temp_entity(&mut self, te: crate::game::temp_entity::TempEntity) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.add_temp_entity(cx, te));
    }

    /// `GameHost::blast_models` for `victim`, for a blast the sim side
    /// charges.
    pub fn blast_models(&mut self, victim: EntId) -> Vec<crate::game::combat::PlacedModel> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.blast_models(cx, victim))
    }

    /// `GameHost::shot_models`, for a round the sim side traces.
    pub fn shot_models(&mut self) -> Vec<crate::game::combat::PlacedModel> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.shot_models(cx))
    }

    /// `GameHost::muzzle_boxes`, for UO's bipod muzzle trace.
    pub fn muzzle_boxes(&mut self, mask: i32) -> Vec<vcod_common::movetrace::ClipBox> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.muzzle_boxes(cx, mask))
    }

    /// `GameHost::placed_script_models`, for a blast the sim side charges.
    pub fn placed_script_models(&mut self) -> Vec<crate::game::combat::PlacedModel> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.placed_script_models(cx))
    }

    /// `GameHost::blast_entities`, for a blast the sim side charges.
    pub fn blast_entities(&mut self) -> Vec<crate::game::combat::EntityVictim> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.blast_entities(cx))
    }

    /// Every live temp entity and its number, for the snapshot build.
    pub fn temp_entities(
        &self,
    ) -> impl Iterator<Item = (u32, &crate::game::temp_entity::TempEntity)> {
        self.host.temp_entities.iter().map(|t| (t.id.0, &t.te))
    }

    /// `Attack::Throw`: the grenade a release put in the air. `now_ms` is
    /// the `level.time` the throw ran at, which is a frame behind the one
    /// this is called on (`crate::game::missile::Missiles::fire_grenade`).
    #[expect(clippy::too_many_arguments)]
    pub fn fire_grenade(
        &mut self,
        owner: usize,
        weapon: u8,
        model: i32,
        origin: glam::Vec3,
        velocity: glam::Vec3,
        carried: glam::Vec3,
        fuse_left_ms: i32,
        now_ms: i32,
    ) {
        let host = &mut self.host;
        let spawned = self.vm.with_cx(|cx| {
            host.missiles.fire_grenade(
                &mut host.ents,
                &mut host.holdrand,
                cx,
                model,
                owner,
                weapon,
                origin,
                velocity,
                carried,
                fuse_left_ms,
                now_ms,
            )
        });
        match spawned {
            Ok(id) => {
                let (kind, take_damage) =
                    host.weapons.get(weapon as usize).map_or((0, false), |d| {
                        (
                            crate::game::missile::explosion_type(&d.proj_explosion_type),
                            d.take_damage,
                        )
                    });
                if host.missiles.set_uo_kind(id, kind, take_damage, now_ms) {
                    host.link_shootable(id);
                }
            }
            Err(e) => log::warn!("the grenade client {owner} threw was not spawned: {e:?}"),
        }
    }

    /// `fire_artillery_barrage` (game.mp.uo 0x601fb) for a strike `owner`
    /// called on `at`, at the `level.time` its cmd ran under: the unsent
    /// barrage entity, then the owner's `artillery` notify with the point.
    pub fn fire_artillery_barrage(
        &mut self,
        owner: usize,
        weapon: u8,
        at: glam::Vec3,
        now_ms: i32,
    ) {
        let Some(def) = self.host.weapons.get(weapon as usize).cloned() else {
            return;
        };
        let host = &mut self.host;
        let spawned = self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx)?;
            if let crate::game::fields::Route::Engine { slot, .. } =
                crate::game::fields::route_entity("classname")
                && let Some(e) = host.ents.get_mut(id)
            {
                e.engine[slot] = Value::String(cx.intern_exact("rocket"));
            }
            if let crate::game::fields::Route::Engine { slot, .. } =
                crate::game::fields::route_entity("origin")
                && let Some(e) = host.ents.get_mut(id)
            {
                e.engine[slot] = Value::Vector(at.to_array());
            }
            Ok::<_, vcod_gsc::ErrorKind>(id)
        });
        let id = match spawned {
            Ok(id) => id,
            Err(e) => {
                log::warn!("the barrage client {owner} called was not spawned: {e:?}");
                return;
            }
        };
        self.host
            .barrages
            .push(crate::game::artillery::Barrage::new(
                id, owner, weapon, at, &def, now_ms,
            ));
        if let Some(player) = self.client_entity(owner) {
            let event = self.vm.with_cx(|cx| cx.intern_folded("artillery"));
            self.vm.notify(
                Target::Entity(player),
                event,
                &[Value::Vector(at.to_array())],
            );
        }
    }

    /// The missiles on the wire this frame. They are `SVF_BROADCAST`, so the
    /// caller adds them past its own PVS cull.
    pub fn missiles(&self) -> &crate::game::missile::Missiles {
        &self.host.missiles
    }

    /// The queue's corpses as the snapshot sends them: each with the event
    /// ring of the entity `cloneplayer` handed to script, which `playSound`
    /// appends to (`G_PlaySoundAlias` 0x67b30 writes `s.events` on any
    /// entity with no client).
    pub fn body_entities<'a>(
        &'a self,
        p: &'a vcod_common::net::protocol::Cod1Wire,
    ) -> impl Iterator<Item = (u32, vcod_common::net::msg::EntityState)> + 'a {
        self.host.bodies.entities().map(move |(n, mut e)| {
            if let Some(g) = self
                .host
                .ents
                .handle(n)
                .and_then(|id| self.host.ents.get(id))
            {
                g.events.write(&mut |name: &str, v: i32| {
                    if let Some(i) = vcod_common::net::msg::EntityState::field_index(p, name) {
                        e.fields[i] = v;
                    }
                });
            }
            (n, e)
        })
    }

    /// The corpse queue, whose entities every client's snapshot carries
    /// until their slots are reused (`crate::game::bodies`).
    pub fn bodies(&self) -> &crate::game::bodies::BodyQueue {
        &self.host.bodies
    }

    pub fn bodies_mut(&mut self) -> &mut crate::game::bodies::BodyQueue {
        &mut self.host.bodies
    }

    /// The map's triggers, for a test or a caller outside the game module.
    pub fn triggers_mut(&mut self) -> &mut crate::game::trigger::Triggers {
        &mut self.host.triggers
    }

    /// The cvar table as the script left it. `engine::tick` reads it back
    /// every frame for the same reason it re-reads the configstrings: a
    /// thread past a `wait` can still call `setCvar`.
    pub fn cvars(&self) -> &crate::cvars::Cvars {
        &self.host.cvars
    }

    /// The `scriptUsage` console command's report (`crate::game::usage`):
    /// the live threads, then the entities and HUD elements with what
    /// script holds on each (`Vm::objects_usage`).
    pub fn script_usage(&mut self) -> String {
        let threads = self.vm.thread_usage();
        let ents = &self.host.ents;
        // A radiant key is a variable only when the scripts name it
        // (`Scr_FindField`); every name a script sets or reads they do.
        let named = self
            .scripts
            .as_ref()
            .map_or(u32::MAX, |s| s.program.atom_count());
        let named = |a: vcod_gsc::Atom| a.0 < named;
        let objects = |it: &mut dyn Iterator<Item = (vcod_gsc::EntId, vcod_gsc::StructId)>| {
            let all: Vec<_> = it
                .map(|(id, s)| (s, vcod_gsc::Target::Entity(id)))
                .collect();
            (
                all.len(),
                self.vm.objects_usage(all.into_iter(), named) as usize,
            )
        };
        let entity = objects(&mut ents.iter_inuse().map(|(id, e)| (id, e.script)));
        let hudelem = objects(&mut ents.iter_hud_elems().map(|(id, e)| (id, e.script)));
        let sources = self.scripts.as_ref().map(|s| &s.sources);
        let source = |file: &str| sources.and_then(|s| s.text(file));
        let positions = match self.developer {
            true => crate::game::usage::Positions::Lines(&source),
            false => crate::game::usage::Positions::Offsets,
        };
        crate::game::usage::script_usage(
            &threads,
            &[
                ("entity", entity.0, entity.1),
                ("hudelem", hudelem.0, hudelem.1),
            ],
            &positions,
        )
    }

    /// The `stringUsage` console command's report (`crate::game::usage`).
    pub fn string_usage(&self) -> String {
        crate::game::usage::string_usage(&self.vm.string_lengths())
    }

    /// Every builtin the loaded closure calls that its game's tables list
    /// and vcod does not answer, sorted: what a thread reaching one would
    /// abort on.
    pub fn unimplemented_builtins(&self) -> Vec<String> {
        self.host.builtins().unimplemented_calls(&self.vm)
    }

    /// Every thread that has died of an error since map load, rendered
    /// `file::func:line: Kind`. The bootstrap starts its threads with
    /// `Vm::start_thread`, whose signature has no error channel, so this is
    /// the only way a caller sees an aborted bootstrap thread; `run_frame`
    /// logs the errors from its own pass and they land here too.
    ///
    /// The VM stops recording past its own cap, so a run that blew through
    /// it gets a final line saying how many are missing rather than a list
    /// that quietly claims to be complete.
    pub fn aborts(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .vm
            .aborts()
            .iter()
            .map(|e| self.vm.describe(e))
            .collect();
        let dropped = self.vm.abort_count() as usize - out.len();
        if dropped > 0 {
            out.push(format!("... and {dropped} more, past the record cap"));
        }
        out
    }

    /// Every line the script has passed to `logPrint`, in order. Retail
    /// writes these to `games_mp.log`, which is where the probe captures in
    /// `crates/gsc/tests/fixtures/semantics/` came from.
    pub fn script_log(&self) -> &[String] {
        &self.host.script_log
    }

    /// The `G_Error` the entity pass raised, taken.
    pub fn take_game_error(&mut self) -> Option<String> {
        self.host.game_error.take()
    }
}

#[cfg(test)]
mod tests;
