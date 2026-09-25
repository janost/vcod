//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::rc::Rc;

use crate::game::host::{ClientEvent, GameHost, SpawnRequest};
use crate::game::item::Activate;
use crate::game::spawn::spawn_entities_from_string;
use crate::game::turret::{TurretOp, TurretShot};
use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{EntId, Loader, ScriptSource, Target, Value, Vm};

/// Every gametype script includes this file, and it is where the engine's
/// entry points into script live, `CodeCallback_StartGameType` among them.
pub(crate) const CALLBACK_SETUP: &str = "maps/mp/gametypes/_callbacksetup";

/// The event `Callback_PlayerConnect` parks on before it lets the client
/// into the world. A misspelt notify does not error, it hangs the thread,
/// so the literal lives here with a test on it.
const BEGIN_NOTIFY: &str = "begin";

/// The event every gametype's team-join loop parks on, once the connect
/// callback has opened the first menu. Same silent failure as
/// `BEGIN_NOTIFY`, same test.
const MENURESPONSE_NOTIFY: &str = "menuresponse";

/// Reads `.gsc` out of the mounted paks. `Loader` hands `read` a canonical
/// path (lowercase, forward slashes, no extension; see `vcod_gsc::canonical`);
/// the pak stores e.g. `maps/MP/_load.gsc`, and `Pk3Fs::read` already
/// lowercases its lookup key, so appending `.gsc` is the only work here.
pub(crate) struct PakScripts {
    fs: Rc<Pk3Fs>,
    /// One canonical path answered from memory instead of the paks, for a
    /// test whose gametype script does not ship in one
    /// (`Server::overlay_script`).
    overlay: Option<(String, String)>,
}

impl PakScripts {
    pub(crate) fn new(fs: Rc<Pk3Fs>, overlay: Option<(String, String)>) -> PakScripts {
        PakScripts { fs, overlay }
    }
}

impl ScriptSource for PakScripts {
    fn read(&self, canonical: &str) -> Option<String> {
        if let Some((path, text)) = self.overlay.as_ref() {
            if path == canonical {
                return Some(text.clone());
            }
        }
        let bytes = self.fs.read(&format!("{canonical}.gsc"))?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// What `savePersist` preserves across a level boundary
/// (docs/research/cod11-map-cycle.md section 1): the `game[]` table and one
/// `pers[]` per client slot. Both empty on a first load and on a boundary
/// the outgoing level did not ask to persist.
#[derive(Default)]
pub struct Carry {
    pub game: Option<vcod_gsc::GameCarry>,
    pub pers: Vec<Option<vcod_gsc::ArrayCarry>>,
    /// The item registry, which is engine state rather than script state:
    /// a `map_restart` keeps it whether or not the level asked to persist,
    /// the way it keeps the configstring table (map-cycle doc, 4.6).
    pub items: Option<crate::items::Items>,
}

/// A script value as text, through the same `%g` rendering string
/// concatenation uses. A value with no rendering (`undefined`, an array, an
/// entity) comes back debug-rendered, so a failed assertion says what the
/// field actually held rather than an empty string.
fn render(cx: &vcod_gsc::Cx, v: Value) -> String {
    cx.format_number(v).unwrap_or_else(|| format!("{v:?}"))
}

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
/// a new `ScriptRuntime` and drops the old one; the heap goes with it,
/// which is also what the VM's no-garbage-collection design assumes.
/// `clientState` carries six attachment pairs
/// (`docs/research/clientstate-wire-format.md`).
pub const ATTACH_SLOTS: usize = 6;

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

/// `serverCursorHint` for a usable turret, slot 6 of `hintStrings`
/// (`docs/research/cod11-turrets.md` 4.3).
const HINT_MG42: i32 = 6;
/// A manned gun's shot and its cooldown alias, both on the gun's own ring
/// (`docs/research/cod11-events-and-fx.md` section 1, turrets doc 6.3, 6.4).
const EV_FIRE_WEAPON_MG42: i32 = 168;
pub(crate) const EV_SOUND_ALIAS: i32 = 172;

pub struct ScriptRuntime {
    vm: Vm,
    pub(crate) host: GameHost,
    entry: String,
    gametype_entry: String,
    /// Draws the `wait`/`random` gate's random half (`Triggers::fire`), one
    /// xorshift64* state per map load so a rerun of the same seed reproduces
    /// the same firing pattern.
    rng: u64,
}

impl ScriptRuntime {
    /// Loads the gametype and map script closures, spawns the map's entities
    /// and runs the bootstrap. `map` and `gametype` are bare names, e.g.
    /// "mp_pavlov" and "dm". `configstrings` and `cvars` seed the host;
    /// `world`, once the caller has one, lets `bulletTrace` trace against the
    /// real map geometry, and `weapons` is what the weapon builtins read.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        world: Option<Rc<crate::world::World>>,
        weapons: Rc<crate::weapons::WeaponTable>,
        now_ms: i32,
        rng_seed: u64,
        carry: Carry,
    ) -> anyhow::Result<ScriptRuntime> {
        Self::load_from(
            Box::new(PakScripts::new(fs.clone(), None)),
            fs,
            map,
            gametype,
            configstrings,
            cvars,
            world,
            weapons,
            now_ms,
            rng_seed,
            carry,
        )
    }

    /// `load` with the script source supplied. `fs` still resolves the map's
    /// BSP, so `source` only decides where `.gsc` text comes from: a test
    /// overlays one file on the paks and gets the real bootstrap rather than
    /// a hand-written copy of it that a change here would not reach.
    #[allow(clippy::too_many_arguments)]
    pub fn load_from(
        source: Box<dyn ScriptSource>,
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        world: Option<Rc<crate::world::World>>,
        weapons: Rc<crate::weapons::WeaponTable>,
        now_ms: i32,
        rng_seed: u64,
        carry: Carry,
    ) -> anyhow::Result<ScriptRuntime> {
        let entry = format!("maps/mp/{map}");
        let gametype_entry = format!("maps/mp/gametypes/{gametype}");
        let mut vm = Vm::new();
        // `game[]` before anything runs: the gametype's `main` reads it in
        // its first statements (map-cycle doc, section 1).
        if let Some(game) = carry.game.as_ref() {
            vm.install_game(game);
        }
        // One `Loader`, not two: its `loaded` set dedupes the files both
        // closures share, and `Vm::install` rejects a duplicate `FuncRef`, so
        // a second `Loader` would fail on the first shared file.
        let mut loader = Loader::new(source);
        loader
            .load(&mut vm, &gametype_entry)
            .map_err(|e| anyhow::anyhow!("loading {gametype_entry}: {e:?}"))?;
        loader
            .load(&mut vm, &entry)
            .map_err(|e| anyhow::anyhow!("loading {entry}: {e:?}"))?;

        let mut host = GameHost::new(configstrings);
        host.cvars = cvars;
        host.world = world;
        host.weapons = weapons;
        host.fs = Some(fs.clone());
        host.level_time_ms = now_ms;
        host.pers_carry = carry.pers;
        if let Some(items) = carry.items {
            host.items = items;
        }

        // `_load.gsc::main`, in mp_pavlov's own closure, calls `getEntArray`
        // in its first statements, so the object table must hold every map
        // entity before `main` runs: the entity lump goes in first, then the
        // pre-scan (which only reads the loader, not the table), and only
        // then does `main` start.
        let bsp_path = fs
            .resolve_map(map)
            .ok_or_else(|| anyhow::anyhow!("map {map} not found in the mounted paks"))?;
        let bsp_bytes = fs
            .read(&bsp_path)
            .ok_or_else(|| anyhow::anyhow!("reading {bsp_path}"))?;
        let bsp = vcod_common::bsp::parse(&bsp_bytes)?;
        host.model_bounds = bsp.models.iter().map(|m| (m.mins, m.maxs)).collect();
        host.model_brushes = crate::game::trigger::model_brush_hulls(&bsp);
        vm.with_cx(|cx| spawn_entities_from_string(&mut host, cx, &bsp.entities))
            .map_err(|e| anyhow::anyhow!("spawning {map}'s entities: {e:?}"))?;

        // The closure calls far more builtins than the host answers yet.
        // Listing them once here beats discovering them one aborted thread
        // at a time, and a missing builtin must not stop the map serving:
        // a thread that reaches one dies, the rest of the script runs.
        let missing = loader.missing_builtins(&vm, &|n| crate::game::host::is_builtin(n));
        if !missing.is_empty() {
            log::info!(
                "gsc: {entry} and {gametype_entry} call {} builtins the host does not implement: {}",
                missing.len(),
                missing.join(" ")
            );
        }

        let mut rt = ScriptRuntime {
            vm,
            host,
            entry,
            gametype_entry,
            rng: rng_seed,
        };
        rt.start_bootstrap(now_ms)?;
        Ok(rt)
    }

    /// The gametype's `main`, then the map's, then
    /// `CodeCallback_StartGameType`, whose own header in `_callbacksetup.gsc`
    /// reads "Called by code after the level's main script function has run".
    ///
    /// The order of the two `main`s is `probe_bootstrap`'s measurement, not an
    /// inference: `mp_pavlov.gsc` sets `game["allies"] = "russian"`, and the
    /// probe reads that key back as undefined from the gametype's `main` and
    /// as "russian" from `Callback_StartGameType`.
    ///
    /// `start_thread`, not `call_now`: retail runs these as script threads,
    /// and `call_now` turns any `wait` into `SuspendedInImmediateCall`.
    /// `start_thread` steps the new thread to its first suspend before
    /// returning, which the same probe measured, so the tables are complete
    /// when `load` returns.
    fn start_bootstrap(&mut self, now_ms: i32) -> anyhow::Result<()> {
        let gametype_entry = self.gametype_entry.clone();
        self.start(&gametype_entry, "main", None, now_ms)?;

        let entry = self.entry.clone();
        self.start(&entry, "main", None, now_ms)?;

        // `CodeCallback_StartGameType` is defined in `_callbacksetup`, not in
        // the gametype script; it guards on `level.gametypestarted` and then
        // calls `level.callbackStartGameType`.
        self.start(CALLBACK_SETUP, "CodeCallback_StartGameType", None, now_ms)
    }

    /// Starts one entry point as a thread with `recv` as its `self`, checking
    /// it is installed first. `Vm::start_thread` panics on a `FuncRef` that is
    /// not, and `--gametype` is user input, so a gametype script that loads
    /// but defines no `main` (or never pulls in `_callbacksetup`) has to fail
    /// the way a missing file does: an error `main.rs` exits on, not a panic.
    fn start(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<Target>,
        now_ms: i32,
    ) -> anyhow::Result<()> {
        self.start_with_args(path, name, recv, vec![], now_ms)
    }

    /// `start` with the arguments the entry point takes.
    fn start_with_args(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<Target>,
        args: Vec<Value>,
        now_ms: i32,
    ) -> anyhow::Result<()> {
        let f = self.vm.func_ref(path, name);
        if !self.vm.has_function(f) {
            anyhow::bail!("{path}.gsc defines no {name}()");
        }
        // The thread runs here, before the caller continues, so the level
        // clock has to be this frame's already: the damage callbacks start
        // ahead of `run_frame`, and a `cloneplayer` or a think they schedule
        // on the previous frame's clock lands a frame in the past.
        self.host.level_time_ms = now_ms;
        self.vm.start_thread(&mut self.host, now_ms, f, recv, args);
        Ok(())
    }

    /// `Scr_PlayerDamage` (combat doc, section 4.4) for every hit this
    /// frame's attacks made: `CodeCallback_PlayerDamage` on the victim's
    /// entity with the nine arguments. The inflictor is the attacker himself
    /// unless the hit names one, which a blast does: there the missile that
    /// went off is handed over, still alive for as long as its explode event
    /// rides the wire. A hit on a slot with no entity, or from one, is
    /// dropped: there is nobody to call and nobody to name.
    pub fn deliver_hits(&mut self, hits: Vec<crate::game::combat::Hit>, now_ms: i32) {
        for hit in hits {
            let (Some(victim), Some(attacker)) = (
                self.client_entity(hit.victim),
                self.client_entity(hit.attacker),
            ) else {
                continue;
            };
            let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
                (
                    cx.intern_exact(hit.mod_),
                    cx.intern_exact(&hit.weapon),
                    cx.intern_exact(hit.hitloc),
                )
            });
            let args = vec![
                Value::Entity(hit.inflictor.unwrap_or(attacker)),
                Value::Entity(attacker),
                Value::Int(hit.damage),
                Value::Int(hit.dflags),
                Value::String(mod_),
                Value::String(weapon),
                Value::Vector(hit.point),
                Value::Vector(hit.dir),
                Value::String(hitloc),
            ];
            if let Err(e) = self.start_with_args(
                CALLBACK_SETUP,
                "CodeCallback_PlayerDamage",
                Some(Target::Entity(victim)),
                args,
                now_ms,
            ) {
                log::error!("gsc: {e:#}");
            }
        }
    }

    /// Damage whose attacker is not a player. `hurt_touch` (0x64dc4) hands
    /// `G_Damage` the trigger entity for both the inflictor and the attacker
    /// and zero for the point and the direction, which the immediates it
    /// pushes show directly (VERIFIED); the stock callback tests
    /// `isPlayer(attacker)`, so a non-player attacker is a shape the corpus
    /// already handles.
    pub fn deliver_world_hit(
        &mut self,
        victim_slot: usize,
        inflictor: EntId,
        damage: i32,
        dflags: i32,
        mod_: &str,
        now_ms: i32,
    ) {
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
            (
                cx.intern_exact(mod_),
                cx.intern_exact("none"),
                cx.intern_exact("none"),
            )
        });
        let args = vec![
            Value::Entity(inflictor),
            Value::Entity(inflictor),
            Value::Int(damage),
            Value::Int(dflags),
            Value::String(mod_),
            Value::String(weapon),
            Value::Vector([0.0; 3]),
            Value::Vector([0.0; 3]),
            Value::String(hitloc),
        ];
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
            Some(Target::Entity(victim)),
            args,
            now_ms,
        ) {
            log::error!("gsc: {e:#}");
        }
    }

    /// `G_TouchTriggers` for one client, which retail runs once per usercmd
    /// from `ClientThink_real` (0x405b3). The notify only marks the threads
    /// parked in `waittill("trigger", other)` runnable; they run in this
    /// tick's script frame.
    ///
    /// Only a client at `ps.pm_type <= 1` touches anything: the function's
    /// own second guard, which takes the dead, a spectator and the
    /// intermission camera out before the pass runs
    /// (docs/research/cod11-gsc-object-model.md 8.2).
    ///
    /// `buttons` are the cmd's own rather than the host's mirrored copy, which
    /// is only written after the move pass this runs inside.
    pub fn touch_triggers_with_buttons(&mut self, slot: usize, now_ms: i32, buttons: u8) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        let hits = self
            .vm
            .with_cx(|cx| crate::game::trigger::touched(host, cx, client));
        let triggers = &mut self.host.triggers;
        let rng = &mut self.rng;
        // Drawn under the `rng` borrow and acted on after it: each entry is a
        // trigger that fired, carrying its damage and flags if it hurts.
        let fired: Vec<(EntId, Option<(i32, i32)>)> = hits
            .into_iter()
            .filter_map(|id| {
                // A `trigger_use` answers the use key rather than contact
                // (docs/superpowers/specs/2026-09-08-movers-triggers-sd-design.md
                // 3.6). Ahead of `fire`, so a keyless touch leaves the `wait`
                // window unarmed.
                if triggers.get(id).map(|t| t.kind) == Some(crate::game::trigger::TriggerKind::Use)
                    && buttons & vcod_common::net::msg::BUTTON_USE == 0
                {
                    return None;
                }
                if !triggers.fire(id, now_ms, &mut |n| {
                    if n <= 0 {
                        0
                    } else {
                        crate::game::host::rand_int(rng) % n
                    }
                }) {
                    return None;
                }
                let hurt = triggers
                    .get(id)
                    .filter(|t| t.kind == crate::game::trigger::TriggerKind::Hurt)
                    .map(|t| (t.damage, t.dflags));
                Some((id, hurt))
            })
            .collect();
        for (id, hurt) in fired {
            self.host.trigger_fires.push((id, client));
            if let Some((damage, dflags)) = hurt {
                self.deliver_world_hit(
                    slot,
                    id,
                    damage,
                    dflags,
                    crate::game::trigger::MOD_TRIGGER_HURT,
                    now_ms,
                );
            }
        }
    }

    /// The buttons of the last cmd `item_pass` saw from `slot`, which its
    /// use edge is taken against.
    pub fn client_old_buttons(&self, slot: usize) -> u8 {
        self.host.client_old_buttons.get(slot).copied().unwrap_or(0)
    }

    /// The item half of `G_TouchTriggers` and then `Cmd_Activate_f`, for one
    /// cmd (docs/research/cod11-items.md, sections 1 and 2): every item in
    /// the touch box, then, on the use key's rising edge, the one the aim
    /// picks. Both gated as the trigger half is; the use half also needs the
    /// player alive, `G_CheckForCursorHints`' own gate.
    pub fn item_pass(&mut self, slot: usize, buttons: u8, eye: [f32; 3], view: [f32; 3]) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        let old = std::mem::replace(&mut self.host.client_old_buttons[slot], buttons);
        let pressed = buttons & !old & vcod_common::net::msg::BUTTON_USE != 0;
        // `Cmd_Activate_f`'s busy byte (0x4848e), which has no `pm_type`
        // gate: a gunner's press asks for the release and does nothing else
        // (turrets doc 4.1).
        let mut release_asked = false;
        if pressed {
            if let Some(rec) = self
                .host
                .turrets
                .values_mut()
                .find(|r| r.owner == Some(slot))
            {
                rec.busy = 2;
                release_asked = true;
            }
        }
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            use vcod_gsc::Host;
            let origin_atom = cx.intern_folded("origin");
            let Value::Vector(origin) = host.get_field(cx, client, origin_atom) else {
                return;
            };
            for id in crate::game::item::touching(host, cx, origin) {
                host.item_notifies
                    .push((id, "touch", vec![Value::Entity(client)]));
                host.item_notifies
                    .push((client, "touch", vec![Value::Entity(id)]));
                crate::game::item::touch(host, cx, id, slot, true);
            }
            if !pressed || release_asked {
                return;
            }
            let v = host.client_vitals[slot];
            if v.health <= 0 || v.dead {
                return;
            }
            match crate::game::item::activate_ent(host, cx, slot, eye, view) {
                Some(Activate::Item(id)) => {
                    host.item_notifies
                        .push((id, "touch", vec![Value::Entity(client)]));
                    crate::game::item::touch(host, cx, id, slot, false);
                }
                Some(Activate::Turret(turret)) => {
                    host.turret_ops.push(TurretOp::Mount { slot, turret });
                }
                None => {}
            }
        });
    }

    /// `turret_use` (0x52a9c)'s record half for the mount `item_pass` just
    /// queued for `slot`, off the state of the cmd that pressed use. The
    /// record is manned at once, so a second client's press in the same
    /// frame finds the gun busy. Returns what the sim half needs: the
    /// turret's entity number, its stance and the view to snap to.
    pub fn take_turret_mounts(
        &mut self,
        slot: usize,
        origin: [f32; 3],
        stance: vcod_common::pmove::Stance,
        view: [f32; 3],
    ) -> Vec<(u32, crate::game::turret::TurretStance, [f32; 3])> {
        let ops = std::mem::take(&mut self.host.turret_ops);
        let mut out = Vec::new();
        for op in ops {
            let TurretOp::Mount { slot: s, turret } = op else {
                self.host.turret_ops.push(op);
                continue;
            };
            debug_assert_eq!(s, slot, "a mount is drained after its own cmd's pass");
            let host = &mut self.host;
            let angles = self
                .vm
                .with_cx(|cx| crate::game::item::angles_of(host, cx, turret));
            let Some(rec) = self.host.turrets.get_mut(&turret) else {
                continue;
            };
            let view = crate::game::turret::mount(rec, s, origin, stance, view, angles);
            out.push((turret.0, rec.def.stance, view));
        }
        out
    }

    /// `turret_think_client` (0x52340, turrets doc 6) for the gun `slot`
    /// mans, once per server frame in `ClientEndFrame`: the aim, the fire
    /// and the loop sound, on the record, the gun's entity and the gunner's
    /// sim. `attack_held` is the frame's last cmd's attack bit. `body` is
    /// what the body placement reads the gunner's anims from; without it the
    /// body stays where it is. Returns the round the gun fired, for the
    /// server to trace.
    pub fn turret_think_client(
        &mut self,
        slot: usize,
        sim: &mut crate::spectate::ClientSim,
        attack_held: bool,
        body: Option<(
            &vcod_common::animtree::PlayerAnims,
            &mut crate::game::hitrig::HitRigs,
        )>,
    ) -> Option<TurretShot> {
        use crate::game::turret::{aim, fire_tick, loop_tick, muzzle};
        let id = *self
            .host
            .turrets
            .iter()
            .find(|(_, r)| r.owner == Some(slot))?
            .0;
        // A gun asked to let go, or a gunner no longer playing (dead,
        // spectating), is released instead (0x5235c, 0x5236b).
        let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
        if self.host.turrets[&id].busy != 1 || !playing {
            for te in self.release_turret(slot, sim) {
                self.push_temp_entity(te);
            }
            return None;
        }
        let host = &mut self.host;
        let (origin, angles) = self.vm.with_cx(|cx| {
            (
                crate::game::item::origin_of(host, cx, id),
                crate::game::item::angles_of(host, cx, id),
            )
        });
        let rec = self.host.turrets.get_mut(&id)?;
        let (loop_index, stop_index) =
            crate::game::turret::sound_indices(rec, &self.host.configstrings);

        sim.viewlocked = 1;
        sim.viewlocked_ent = id.0;
        sim.gunfx = 0;
        if let Some(view) = aim(rec, sim.view_angles(), angles) {
            sim.set_view_angle(view);
        }
        let placed = match (body, &rec.tags, self.host.fs.as_deref()) {
            (Some((anims, rigs)), Some(tags), Some(fs)) => {
                let tag_weapon = crate::game::turret::tag_weapon_local(tags, rec.angles2);
                let turret = (
                    glam::Vec3::from(origin),
                    vcod_common::turretpose::angles_quat(angles),
                );
                vcod_common::turretpose::place_gunner(
                    anims,
                    |name| rigs.clip(fs, name),
                    sim.legs_anim(),
                    tag_weapon,
                    turret,
                    sim.ps.origin,
                    rec.def.anim_hor_rotate_inc,
                )
            }
            _ => None,
        };
        if let Some((mut at, _)) = placed {
            if let Some(world) = &self.host.world {
                at = crate::game::turret::lift_onto_floor(&world.collision, at, origin[2]);
            }
            sim.ps.origin = at;
        }
        sim.firing = fire_tick(rec, attack_held);
        let mut shot = None;
        if sim.firing {
            sim.viewlocked = 2;
            let forward = crate::game::spawn::angle_forward(sim.view_angles());
            // A missing tag skips the round and its event, not the flags.
            if let Some(tags) = &rec.tags {
                shot = Some(TurretShot {
                    slot,
                    muzzle: muzzle(tags, origin, angles, rec.angles2, forward).into(),
                    dir: forward.into(),
                    damage: rec.dmg,
                    rifle_bullet: rec.def.rifle_bullet,
                });
            }
        }
        let (loop_sound, stop) = loop_tick(rec, loop_index);
        let fired = shot.is_some();
        if let Some(ent) = self.host.ents.get_mut(id) {
            if fired {
                ent.events.add(EV_FIRE_WEAPON_MG42, 0);
            }
            ent.loop_sound = loop_sound;
            if stop {
                ent.events.add(EV_SOUND_ALIAS, stop_index);
            }
        }
        if let Some((_, yaw)) = placed {
            self.set_client_origin(slot, sim.origin());
            self.set_client_yaw(slot, yaw);
        }
        shot
    }

    /// `G_ClientStopUsingTurret` (0x53054, turrets doc 8) on the gun `slot`
    /// mans, if any: the record freed, the gun's loop cut and the gunner put
    /// back where it mounted, its new origin mirrored to script. Returns
    /// `TeleportPlayer`'s temp entities for the caller to queue.
    pub fn release_turret(
        &mut self,
        slot: usize,
        sim: &mut crate::spectate::ClientSim,
    ) -> Vec<crate::game::temp_entity::TempEntity> {
        let Some((id, rec)) = self
            .host
            .turrets
            .iter_mut()
            .find(|(_, r)| r.owner == Some(slot))
        else {
            return Vec::new();
        };
        let id = *id;
        let Some((_, origin, stance)) = crate::game::turret::release(rec) else {
            return Vec::new();
        };
        if let Some(ent) = self.host.ents.get_mut(id) {
            ent.loop_sound = 0;
        }
        let temps = crate::game::turret::release_sim(sim, slot, origin, stance);
        self.set_client_origin(slot, sim.origin());
        temps
    }

    /// The sim half of every release `GameHost::free_entity` queued for
    /// `slot` (`G_FreeTurret`, turrets doc 8), each temp entity queued.
    pub fn apply_turret_releases(&mut self, slot: usize, sim: &mut crate::spectate::ClientSim) {
        let ops = std::mem::take(&mut self.host.turret_ops);
        for op in ops {
            match op {
                TurretOp::Release {
                    slot: s,
                    origin,
                    stance,
                } if s == slot => {
                    for te in crate::game::turret::release_sim(sim, slot, origin, stance) {
                        self.push_temp_entity(te);
                    }
                    self.set_client_origin(slot, sim.origin());
                }
                op => self.host.turret_ops.push(op),
            }
        }
    }

    /// Drops the releases queued for a slot with no sim to apply them to.
    pub fn drop_turret_releases(&mut self) {
        self.host
            .turret_ops
            .retain(|op| !matches!(op, TurretOp::Release { .. }));
    }

    /// `Cmd_Kill_f`: the `kill` client command, which is the `suicide` builtin
    /// reached from outside the VM. Same three effects -- the vitals, the
    /// `Damaged` op the sim reads, and `CodeCallback_PlayerKilled` with
    /// `MOD_SUICIDE` -- but started as a thread rather than spawned, since
    /// nothing is waiting on it. A dead or spectating client is ignored, the
    /// way the builtin ignores a dead one. Returns whether the kill ran.
    pub fn kill_client(&mut self, slot: usize, now_ms: i32) -> bool {
        let Some(id) = self.client_entity(slot) else {
            return false;
        };
        // A client that never spawned has no health to take.
        if self
            .host
            .client_vitals
            .get(slot)
            .is_none_or(|v| v.max_health <= 0)
        {
            return false;
        }
        let host = &mut self.host;
        let Some(args) = self
            .vm
            .with_cx(|cx| crate::game::builtins::combat::suicide_effects(host, cx, slot))
        else {
            return false;
        };
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerKilled",
            Some(Target::Entity(id)),
            args,
            now_ms,
        ) {
            log::error!("gsc: {e:#}");
        }
        true
    }

    /// What `finishPlayerDamage` did to the sims this frame, drained: each
    /// op is applied once.
    pub fn take_sim_ops(&mut self) -> Vec<(usize, crate::game::host::SimOp)> {
        std::mem::take(&mut self.host.client_sim_ops)
    }

    /// What `linkTo` and `unlink` did this frame, drained the same way.
    pub fn take_link_ops(&mut self) -> Vec<(usize, crate::game::host::LinkOp)> {
        std::mem::take(&mut self.host.client_link_ops)
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

    /// One client's health as the script left it, read every frame the way
    /// `client_weapons` is.
    pub fn client_vitals(&self, slot: usize) -> crate::game::host::Vitals {
        self.host
            .client_vitals
            .get(slot)
            .copied()
            .unwrap_or_default()
    }

    /// The buttons of a client's last usercmd, for `useButtonPressed`.
    pub fn set_client_buttons(&mut self, slot: usize, buttons: u8) {
        if let Some(b) = self.host.client_buttons.get_mut(slot) {
            *b = buttons;
        }
    }

    /// A client's wire `ps.pm_type` as the tick's moves left it, for the
    /// touch pass's gate.
    pub fn set_client_pm_type(&mut self, slot: usize, pm_type: i32) {
        if let Some(t) = self.host.client_pm_type.get_mut(slot) {
            *t = pm_type;
        }
    }

    /// A client's `ps.on_ground` as the tick's moves left it, for
    /// `isOnGround` (docs/research/cod11-gsc-object-model.md, 23.5).
    pub fn set_client_on_ground(&mut self, slot: usize, on_ground: bool) {
        if let Some(g) = self.host.client_on_ground.get_mut(slot) {
            *g = on_ground;
        }
    }

    /// A client's eye (lean included) and `[pitch, yaw]` aim as the tick left
    /// them, for `aim_lookat`.
    pub fn set_client_aim(&mut self, slot: usize, eye: [f32; 3], aim: [f32; 2]) {
        if let Some(a) = self.host.client_aim.get_mut(slot) {
            *a = (eye, aim);
        }
    }

    /// `ClientEndFrame`'s aim trace (`G_CheckForPreventFriendlyFire`,
    /// 0x4f88c): once per frame per playing client, after the script frame,
    /// the lookat the aim enters first is stored for `isLookingAt` and fired
    /// through the same notify the touch pass raises, whose waiters run on
    /// the next frame (docs/research/cod11-gsc-object-model.md 23.1). Every
    /// frame it is aimed at, since `G_Trigger` gates nothing.
    ///
    /// The `pm_type` gate skips a spectator and the intermission camera, as
    /// `ClientEndFrame`'s own arms do. It also clears a dead client (`pm_type`
    /// 6/7), where retail's sessionstate 1 reaches the trace subject to the
    /// unread `ent+0x172` byte (23.1); every stock `isLookingAt` caller also
    /// tests `isAlive`.
    pub fn aim_lookat(&mut self, slot: usize, now_ms: i32) {
        let Some(client) = self.client_entity(slot) else {
            if let Some(l) = self.host.client_lookat.get_mut(slot) {
                *l = None;
            }
            return;
        };
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            self.host.client_lookat[slot] = None;
            return;
        }
        let (eye, aim) = self.host.client_aim[slot];
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::trigger::aim_trace(host, cx, eye, aim));
        self.host.client_lookat[slot] = hit;
        if let Some(id) = hit {
            if self.host.triggers.fire(id, now_ms, &mut |_| 0) {
                self.host.trigger_fires.push((id, client));
            }
        }
    }

    /// `G_CheckForCursorHints` (0x4f59c) as `ClientEndFrame` runs it every
    /// frame: the hint for what the use key would pick now, 0 for none or for
    /// a player not alive (docs/research/cod11-items.md, section 2.3), and
    /// `serverCursorHintString`: `None` leaves it as it was, which a dead
    /// player and a gunner do (turrets doc 4.3), `Some(-1)` is no string.
    pub fn cursor_hint_pass(
        &mut self,
        slot: usize,
        eye: [f32; 3],
        view: [f32; 3],
    ) -> (i32, Option<i32>) {
        let v = self
            .host
            .client_vitals
            .get(slot)
            .copied()
            .unwrap_or_default();
        if self.client_entity(slot).is_none() || v.health <= 0 || v.dead {
            return (0, None);
        }
        if self.host.turrets.values().any(|r| r.owner == Some(slot)) {
            return (0, None);
        }
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return (0, Some(-1));
        }
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::item::activate_ent(host, cx, slot, eye, view));
        let id = match hit {
            None => return (0, Some(-1)),
            Some(Activate::Turret(id)) => {
                let string = self.host.turrets[&id]
                    .def
                    .use_hint_string
                    .as_deref()
                    .and_then(|name| {
                        crate::configstrings::hint_string_index(&self.host.configstrings, name)
                    });
                return (HINT_MG42, Some(string.unwrap_or(-1)));
            }
            Some(Activate::Item(id)) => id,
        };
        let Some(item) = self.host.ents.get(id).and_then(|e| e.item) else {
            return (0, Some(-1));
        };
        let Some(kind) = crate::game::pickup::item_kind(item.index as usize) else {
            return (0, Some(-1));
        };
        let owned = self.host.client_weapons[slot].holds(item.index as usize);
        (crate::game::pickup::cursor_hint(kind, owned), Some(-1))
    }

    /// A client's entity state as the tick's moves left it, for
    /// `cloneplayer`. `None` for a slot with no sim.
    pub fn set_client_entity_state(
        &mut self,
        slot: usize,
        state: Option<vcod_common::net::msg::EntityState>,
    ) {
        if let Some(s) = self.host.client_entity_states.get_mut(slot) {
            *s = state;
        }
    }

    /// A client's `ps.grenadeTimeLeft` as the tick's moves left it, for the
    /// death drop (`docs/research/cod11-combat.md` 5.1 step 5). 0 for a slot
    /// with no sim.
    pub fn set_client_grenade_ms(&mut self, slot: usize, ms: i32) {
        if let Some(g) = self.host.client_grenade_ms.get_mut(slot) {
            *g = ms;
        }
    }

    /// A client's box height as the tick's moves left it, for `dropItem`.
    pub fn set_client_height(&mut self, slot: usize, height: f32) {
        if let Some(h) = self.host.client_height.get_mut(slot) {
            *h = height;
        }
    }

    /// A client's ammo arrays as the tick's moves left them, for the item
    /// pass and `dropItem`. Ops still queued for that client's sim are
    /// re-applied on top, since the sim has not seen them yet.
    pub fn set_client_ammo(
        &mut self,
        slot: usize,
        ammo: [i16; vcod_common::pmove::weapon::NUM_AMMO],
        clip: [i16; vcod_common::pmove::weapon::NUM_AMMO],
    ) {
        let host = &mut self.host;
        let Some(a) = host.client_ammo.get_mut(slot) else {
            return;
        };
        *a = crate::game::host::AmmoArrays { ammo, clip };
        for (_, op) in host.client_weapon_ops.iter().filter(|(s, _)| *s == slot) {
            a.apply(*op);
        }
    }

    /// `Cmd_MenuResponse_f` (0x486d8): notify the client's entity with the
    /// menu's **name** and the response. The name, not the index the client
    /// sent -- retail reads configstring `CsRange::Menu.start + index` back
    /// and passes that string, which is why `dm.gsc` can both compare it
    /// against `game["menu_team"]` and hand it straight back to `openMenu`.
    ///
    /// The event name folds, the response does not: one is an event name,
    /// the other a string value the script compares against `"allies"` and
    /// weapon names.
    pub fn menu_response(&mut self, slot: usize, index: i32, response: &str) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let menu = crate::configstrings::script_menu_name(&self.host.configstrings, index as usize)
            .to_string();
        let (event, menu, response) = self.vm.with_cx(|cx| {
            (
                cx.intern_folded(MENURESPONSE_NOTIFY),
                cx.intern_exact(&menu),
                cx.intern_exact(response),
            )
        });
        self.vm
            .notify(id, event, &[Value::String(menu), Value::String(response)]);
    }

    /// The per-client server commands the script queued this frame, in call
    /// order. `Server` sends them; nothing here can reach a netchan.
    pub fn take_client_commands(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.host.client_commands)
    }

    /// The spawns the script performed this frame, in call order. `Server`
    /// applies them to the client sims; nothing here can reach one.
    pub fn take_client_spawns(&mut self) -> Vec<SpawnRequest> {
        std::mem::take(&mut self.host.client_spawns)
    }

    /// One client's weapons as the script left them. Read every frame for the
    /// same reason the configstrings are: any thread can have changed them.
    pub fn client_weapons(&self, slot: usize) -> crate::weapons::PlayerWeapons {
        self.host
            .client_weapons
            .get(slot)
            .copied()
            .unwrap_or_default()
    }

    /// Writes back the weapon the sim's own machine switched to. The script
    /// host's copy is what `Server` mirrors into `ps.weapon` every frame, so
    /// a switch the usercmd's weapon byte asked for has to move it: without
    /// this the mirror puts the old weapon back the next frame and
    /// `PM_Weapon` starts the identical putaway again, once every
    /// `dropTime`, for as long as the client keeps asking.
    pub fn set_client_weapon(&mut self, slot: usize, weapon: u8) {
        if let Some(w) = self.host.client_weapons.get_mut(slot) {
            w.current = weapon;
        }
    }

    /// `BG_TakePlayerWeapon`, which the last round of a `clipOnly` weapon
    /// with no reserve runs (`docs/research/cod11-combat.md` 1.5 step 9).
    /// `ps.weapon` is left alone: the switch path takes it to 0 on its own
    /// once the player no longer holds it (1.8).
    pub fn take_client_weapon(&mut self, slot: usize, weapon: u8) {
        if let Some(w) = self.host.client_weapons.get_mut(slot) {
            w.take(weapon as usize);
        }
    }

    /// The ammo and current-weapon edges the weapon builtins made this frame,
    /// in call order. Drained rather than read, unlike `client_weapons`: they
    /// are edges, and applying one twice would refill a spent clip.
    pub fn take_weapon_ops(&mut self) -> Vec<(usize, crate::game::host::WeaponOp)> {
        std::mem::take(&mut self.host.client_weapon_ops)
    }

    /// One client's viewmodel index, read every frame for the same reason
    /// `client_weapons` is.
    pub fn client_viewmodel(&self, slot: usize) -> i32 {
        self.host.client_viewmodel.get(slot).copied().unwrap_or(0)
    }

    /// Reads a field off an entity through the same routing script uses,
    /// rendered as text by `render`.
    pub fn field_str(&mut self, ent: EntId, name: &str) -> String {
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            let v = host.get_field(cx, ent, atom);
            render(cx, v)
        })
    }

    /// One field off a client's entity, rendered the way `field_str` renders
    /// one. `None` when the slot holds no client entity.
    pub fn client_field(&mut self, slot: usize, name: &str) -> Option<String> {
        let ent = self.client_entity(slot)?;
        Some(self.field_str(ent, name))
    }

    /// One key out of a client's `.pers`, rendered the same way. Array keys
    /// are exact-cased, unlike the field name (`docs/research/
    /// cod11-gsc-language.md`), so the key is interned as written.
    pub fn client_pers(&mut self, slot: usize, key: &str) -> Option<String> {
        use vcod_gsc::Host;
        let ent = self.client_entity(slot)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded("pers");
            let Value::Array(a) = host.get_field(cx, ent, atom) else {
                return None;
            };
            let k = vcod_gsc::ArrayKey::Str(cx.intern_exact(key));
            Some(render(cx, cx.get_index(a, k)))
        })
    }

    /// Queues one client lifecycle event for the next `run_frame`. The
    /// netcode's only way into script: a callback that ran inline from
    /// `SV_ClientCommand` would reenter the VM mid-frame.
    pub fn push_client_event(&mut self, ev: ClientEvent) {
        self.host.client_events.push(ev);
    }

    /// The client's entity, once `Connect` has been drained. The object
    /// table is the single owner of that fact: an entity at the slot's own
    /// number with a `client` store is one `spawn_client` made.
    /// Writes a client's simulated origin onto its script entity. The sim
    /// owns a player's position and the script reads it: `spawn` used to be
    /// the only writer, so `positionWouldTelefrag` tested where each client
    /// last spawned rather than where it now stands.
    pub fn set_client_origin(&mut self, slot: usize, origin: [f32; 3]) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            let _ = host.set_field(cx, ent, field, Value::Vector(origin));
        });
    }

    /// Writes an entity's `origin` and `angles` as script would. Test-facing
    /// (`Server::test_place_entity`).
    pub fn place_entity(&mut self, num: u32, origin: [f32; 3], angles: [f32; 3]) {
        use vcod_gsc::Host;
        let Some(id) = self.host.ents.handle(num) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            for (name, v) in [("origin", origin), ("angles", angles)] {
                let field = cx.intern_folded(name);
                let _ = host.set_field(cx, id, field, Value::Vector(v));
            }
        });
    }

    /// Writes a player's `angles` the way `ClientThink_real` does after every
    /// cmd it runs: pitch and roll 0, yaw the view's
    /// (docs/research/cod11-gsc-object-model.md 23.6).
    pub fn set_client_yaw(&mut self, slot: usize, yaw: f32) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("angles");
            let _ = host.set_field(cx, ent, field, Value::Vector([0.0, yaw, 0.0]));
        });
    }

    /// A client's body model as the roster's `modelindex`: the model number
    /// `configstring 268 + modelindex` resolves
    /// (`docs/research/clientstate-wire-format.md`). 0 when the client has no
    /// model yet, which is what a client still on the menus has.
    ///
    /// The model itself comes from the stock character scripts --
    /// `character/mp_american_airborne01.gsc` does
    /// `self setModel("xmodel/playerbody_american_airborne")` -- through
    /// `_teams::model()` on spawn.
    pub fn client_model_index(&mut self, slot: usize) -> i32 {
        let Some(name) = self.client_field(slot, "model") else {
            return 0;
        };
        let (first, last) = crate::configstrings::CsRange::Model.bounds();
        self.host.configstrings[first..=last]
            .iter()
            .position(|cs| *cs == name)
            .map_or(0, |i| (i + 1) as i32)
    }

    /// A client's team as the roster carries it: `clientState.team`, the
    /// 2-bit field the client colours names and picks friend from foe with.
    /// Script owns it through `.sessionteam`, which `_teams.gsc` sets on a
    /// menu join and `dm.gsc` clears back to `"none"` on every spawn; the
    /// four names and the values they map to are in
    /// `docs/research/clientstate-wire-format.md`.
    ///
    /// A client whose `.sessionteam` is unset reads `TEAM_SPECTATOR`, which
    /// is what retail leaves in the field between `ClientConnect` and the
    /// first script write.
    pub fn client_team(&mut self, slot: usize) -> i32 {
        match self.client_field(slot, "sessionteam").as_deref() {
            Some("none") => TEAM_NONE,
            Some("axis") => TEAM_AXIS,
            Some("allies") => TEAM_ALLIES,
            _ => TEAM_SPECTATOR,
        }
    }

    /// A client's attachments as the roster carries them: up to six
    /// `(attachModelIndex, attachTagIndex)` pairs, the model resolving through
    /// `configstring 268 + index` and the tag through `108 + index`
    /// (`docs/research/clientstate-wire-format.md`). A head and a helmet are
    /// attachments, not part of the body: the stock character script does
    /// `attachFromArray(xmodelalias\head_allied::main())` and
    /// `self attach(self.hatModel)`, so a client sent none is headless.
    pub fn client_attachments(&mut self, slot: usize) -> Vec<(i32, i32)> {
        let Some(ent) = self.client_entity(slot) else {
            return Vec::new();
        };
        let Some(e) = self.host.ents.get(ent) else {
            return Vec::new();
        };
        let pairs: Vec<(String, String)> = {
            self.vm.with_cx(|cx| {
                e.attachments
                    .iter()
                    .map(|(m, t)| (cx.resolve(*m).to_string(), cx.resolve(*t).to_string()))
                    .collect()
            })
        };
        let index_in = |range: crate::configstrings::CsRange, name: &str, base: usize| {
            if name.is_empty() {
                return 0;
            }
            let (first, last) = range.bounds();
            self.host.configstrings[first..=last]
                .iter()
                .position(|cs| cs == name)
                .map_or(0, |i| (first + i - base) as i32)
        };
        pairs
            .iter()
            .take(ATTACH_SLOTS)
            .map(|(m, t)| {
                (
                    index_in(crate::configstrings::CsRange::Model, m, 268),
                    index_in(crate::configstrings::CsRange::Tag, t, 108),
                )
            })
            .collect()
    }

    /// The models a client wears, by name rather than by configstring index:
    /// what the locational trace poses. The body is `.model` and the head and
    /// helmet are attachments, the same pair `client_model_index` and
    /// `client_attachments` put on the roster.
    pub fn client_assembly(&mut self, slot: usize) -> Option<crate::game::hitrig::Assembly> {
        use crate::game::hitrig::{model_name, Assembly};
        let body = model_name(&self.client_field(slot, "model")?);
        if body.is_empty() {
            return None;
        }
        let ent = self.client_entity(slot)?;
        let e = self.host.ents.get(ent)?;
        let attachments = self.vm.with_cx(|cx| {
            e.attachments
                .iter()
                .take(ATTACH_SLOTS)
                .map(|(m, t)| {
                    let tag = cx.resolve(*t).to_string();
                    (model_name(cx.resolve(*m)), (!tag.is_empty()).then_some(tag))
                })
                .collect()
        });
        Some(Assembly { body, attachments })
    }

    /// A client entity's `.origin` as the scripts read it.
    pub fn client_origin(&mut self, slot: usize) -> [f32; 3] {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return [0.0; 3];
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            match host.get_field(cx, ent, field) {
                Value::Vector(v) => v,
                _ => [0.0; 3],
            }
        })
    }

    pub fn client_entity(&self, slot: usize) -> Option<EntId> {
        let id = self.host.ents.handle(u32::try_from(slot).ok()?)?;
        self.host
            .ents
            .get(id)
            .filter(|e| e.client.is_some())
            .map(|_| id)
    }

    /// `SV_SpawnServer`'s client pass (docs/research/cod11-map-cycle.md,
    /// section 3 step 22): the incoming level's `ClientConnect` for a client
    /// that is already on the netchan. The object table is the new level's,
    /// so this is a first connect as far as script is concerned. It runs
    /// rather than queues, because retail runs it inside the spawn, after the
    /// settle frames of step 20.
    pub fn reconnect_client(&mut self, slot: usize, name: String, now_ms: i32) {
        self.dispatch_client_event(ClientEvent::Connect { slot, name }, now_ms);
    }

    /// `game[]` lifted for the next level, and every client's `pers[]` with
    /// it (docs/research/cod11-map-cycle.md section 1). Both leave this
    /// runtime intact; the caller drops it right after.
    pub fn take_carry(&self) -> Carry {
        let pers = (0..crate::server::MAX_CLIENTS)
            .map(|slot| {
                self.client_pers_array(slot)
                    .map(|id| self.vm.take_array(id))
            })
            .collect();
        Carry {
            game: Some(self.vm.take_game()),
            pers,
            items: Some(self.host.items.clone()),
        }
    }

    /// A client's `.pers` array id, `None` for a slot with no client entity
    /// or one whose field is not an array.
    fn client_pers_array(&self, slot: usize) -> Option<vcod_gsc::ArrayId> {
        let id = self.client_entity(slot)?;
        let e = self.host.ents.get(id)?;
        match e.client.as_ref()?.get(crate::game::fields::pers_index())? {
            Value::Array(a) => Some(*a),
            _ => None,
        }
    }

    /// The console lines the level's builtins queued
    /// (`trap_SendConsoleCommand`), drained in call order. `Server` runs
    /// them at the top of its next tick, which is retail's `EXEC_APPEND`.
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.host.console)
    }

    /// `level+0x20c`, taken and cleared: whether a score moved this frame,
    /// which is what arms the intermission scoreboard drain (map-cycle doc,
    /// 6.3). Retail's drain clears the flag whether or not any client was in
    /// intermission to receive one, so this takes rather than reads.
    pub fn take_ranks_dirty(&mut self) -> bool {
        std::mem::take(&mut self.host.ranks_dirty)
    }

    /// One queued client event. A callback the closure does not define is
    /// logged and skipped: a gametype without one is still a serving map,
    /// the same reading `load`'s missing-builtin pre-scan takes.
    fn dispatch_client_event(&mut self, ev: ClientEvent, now_ms: i32) {
        match ev {
            ClientEvent::Connect { slot, name } => {
                // The slot's `gclient_t` starts clean, so a reconnect into a
                // slot cannot inherit the previous occupant's weapons in the
                // frames before its first `spawn`.
                if let Some(w) = self.host.client_weapons.get_mut(slot) {
                    *w = crate::weapons::PlayerWeapons::default();
                }
                if let Some(v) = self.host.client_viewmodel.get_mut(slot) {
                    *v = 0;
                }
                if let Some(b) = self.host.client_old_buttons.get_mut(slot) {
                    *b = 0;
                }
                self.host.reset_client_objectives(slot);
                // The carried `pers`, if the boundary this client crossed
                // kept one; taken, so a later reconnect starts empty as
                // retail's does.
                let pers = self.host.pers_carry.get_mut(slot).and_then(Option::take);
                let host = &mut self.host;
                let id = match self
                    .vm
                    .with_cx(|cx| host.ents.spawn_client(cx, slot, pers.as_ref()))
                {
                    Ok(id) => id,
                    Err(e) => {
                        log::error!("client {slot}: no entity: {e:?}");
                        return;
                    }
                };
                // `.name` is `CLIENT_FIELDS[0]`; retail fills it in
                // `ClientUserinfoChanged`, before the callback runs.
                use vcod_gsc::Host;
                let host = &mut self.host;
                let set = self.vm.with_cx(|cx| {
                    let field = cx.intern_folded("name");
                    let value = Value::String(cx.intern_exact(&name));
                    host.set_field(cx, id, field, value)
                });
                if let Err(e) = set {
                    log::error!("client {slot}: name not set: {e:?}");
                }
                self.start_callback("CodeCallback_PlayerConnect", id, now_ms);
            }
            ClientEvent::Begin(slot) => {
                let Some(id) = self.client_entity(slot) else {
                    log::warn!("client {slot}: begin with no entity, connect never ran");
                    return;
                };
                let event = self.vm.with_cx(|cx| cx.intern_folded(BEGIN_NOTIFY));
                self.vm.notify(id, event, &[]);
            }
            ClientEvent::Disconnect(slot) => {
                // Whatever the last level boundary lifted for this slot dies
                // with the client that owned it: the next occupant of the
                // slot is a stranger and must not connect into its `pers[]`.
                if let Some(c) = self.host.pers_carry.get_mut(slot) {
                    *c = None;
                }
                // The callback runs first: it reads `self`, and freeing the
                // slot ahead of it would hand it a dead entity.
                if let Some(id) = self.client_entity(slot) {
                    self.start_callback("CodeCallback_PlayerDisconnect", id, now_ms);
                    // Whatever was still running on the player dies with
                    // it: a dead player's `waitRespawnButton` polled a
                    // freed entity otherwise (`Vm::kill_threads_of`).
                    self.vm.kill_threads_of(Target::Entity(id));
                }
                // `G_FreeEntity` on the player (turrets doc 8): a gun it
                // manned is unowned, its loop left to the unmanned think.
                for rec in self.host.turrets.values_mut() {
                    if rec.owner == Some(slot) {
                        rec.owner = None;
                        rec.busy = 0;
                    }
                }
                self.host.ents.free_client(slot);
            }
        }
    }

    /// One `_callbacksetup` entry point on a client's entity.
    fn start_callback(&mut self, name: &str, id: EntId, now_ms: i32) {
        if let Err(e) = self.start(CALLBACK_SETUP, name, Some(Target::Entity(id)), now_ms) {
            log::error!("gsc: {e:#}");
        }
    }

    /// The configstrings the script wrote (e.g. `setCullFog`, `ambientPlay`)
    /// by the end of `load`. `Server::load_scripts` copies these back into
    /// its own table once, before the first gamestate goes out.
    /// The object table as packet entities: what a snapshot carries before
    /// any per-client culling (`crates/server/src/game/wire.rs`).
    pub fn packet_entities(
        &mut self,
        p: &vcod_common::net::protocol::Protocol,
    ) -> std::collections::BTreeMap<u32, vcod_common::net::msg::EntityState> {
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::wire::packet_entities(host, cx, p))
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

    pub fn configstrings(&self) -> &[String] {
        &self.host.configstrings
    }

    /// The events raised this frame, still queued.
    pub fn temp_entities(&self) -> &[crate::game::temp_entity::TempEntity] {
        &self.host.temp_entities
    }

    /// Queues one event for the next snapshot build.
    pub fn push_temp_entity(&mut self, te: crate::game::temp_entity::TempEntity) {
        self.host.temp_entities.push(te);
    }

    /// The same list, drained. A temp entity lives for one frame, so the
    /// snapshot build takes them rather than reading them
    /// (`crate::game::temp_entity`).
    pub fn take_temp_entities(&mut self) -> Vec<crate::game::temp_entity::TempEntity> {
        std::mem::take(&mut self.host.temp_entities)
    }

    /// `Attack::Throw`: the grenade a release put in the air. `now_ms` is
    /// the `level.time` the throw ran at, which is a frame behind the one
    /// this is called on (`crate::game::missile::Missiles::fire_grenade`).
    #[allow(clippy::too_many_arguments)]
    pub fn fire_grenade(
        &mut self,
        owner: usize,
        weapon: u8,
        model: i32,
        origin: glam::Vec3,
        velocity: glam::Vec3,
        fuse_left_ms: i32,
        now_ms: i32,
    ) {
        let host = &mut self.host;
        let spawned = self.vm.with_cx(|cx| {
            host.missiles.fire_grenade(
                &mut host.ents,
                cx,
                model,
                owner,
                weapon,
                origin,
                velocity,
                fuse_left_ms,
                now_ms,
            )
        });
        if let Err(e) = spawned {
            log::warn!("the grenade client {owner} threw was not spawned: {e:?}");
        }
    }

    /// One frame of the missile pass (`docs/research/cod11-combat.md` 12).
    pub fn run_missiles(
        &mut self,
        world: Option<&vcod_common::collision::CollisionWorld>,
        sims: &[(usize, &crate::spectate::ClientSim)],
        now_ms: i32,
    ) -> crate::game::missile::MissileFrame {
        let host = &mut self.host;
        let frame = host.missiles.run(world, sims, now_ms);
        for id in &frame.freed {
            host.free_entity(*id);
        }
        frame
    }

    /// The missiles on the wire this frame. They are `SVF_BROADCAST`, so the
    /// caller adds them past its own PVS cull.
    pub fn missiles(&self) -> &crate::game::missile::Missiles {
        &self.host.missiles
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

    /// The cvar table as the script left it. `Server::tick` reads it back
    /// every frame for the same reason it re-reads the configstrings: a
    /// thread past a `wait` can still call `setCvar`.
    pub fn cvars(&self) -> &crate::cvars::Cvars {
        &self.host.cvars
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

    /// One server frame of script.
    pub fn run_frame(&mut self, now_ms: i32) {
        // The packet pass, on the clock the netcode raised its events with:
        // retail runs `ClientBegin`, `ClientCommand` and `ClientDisconnect`
        // from `SV_ExecuteClientMessage` (map-cycle doc, 4.4), before
        // `G_RunFrame` advances `level.time`. So a thread one of them wakes
        // runs to its next suspend here, and a `wait 0` it takes is due in
        // *this* frame's thread pass rather than the next one. That is what
        // parks `sd.gsc`'s connect callback on its `waittill("menuresponse")`
        // before the `openMenu` it just queued can be answered; one frame
        // later and the answer notifies nothing and the client never leaves
        // the team menu.
        //
        // `run_runnable`, not `run_frame`: this pass is the callbacks and
        // whatever they notified, never a second deadline wake. Retail's is
        // one `VM_Call` per event, not a `Scr_RunThreads`.
        //
        // Client events in the order the netcode raised them: a `Begin`
        // drained ahead of its own `Connect` finds no thread parked on the
        // notify and strands the client silently.
        let packet_ms = self.host.level_time_ms;
        for ev in std::mem::take(&mut self.host.client_events) {
            self.dispatch_client_event(ev, packet_ms);
        }
        for e in self.vm.run_runnable(&mut self.host, packet_ms) {
            log::warn!("script error: {e:?}");
        }
        self.host.level_time_ms = now_ms;
        // The item pass's notifies, on this frame's clock, and their waiters
        // run here, ahead of every `wait` this frame brings due
        // (docs/research/cod11-items.md, 13.1). Any other thread already
        // `Runnable` runs here with them; a frame with no item notify skips
        // the pass.
        let item_notifies = std::mem::take(&mut self.host.item_notifies);
        if !item_notifies.is_empty() {
            for (id, event, args) in item_notifies {
                if self.host.ents.get(id).is_some() {
                    let event = self.vm.with_cx(|cx| cx.intern_folded(event));
                    self.vm.notify(Target::Entity(id), event, &args);
                }
            }
            for e in self.vm.run_runnable(&mut self.host, now_ms) {
                log::warn!("script error: {e:?}");
            }
        }
        // A trigger's thread runs on the clock of the frame after the touch:
        // retail's plant bar carries the `scaleStartTime` of the first
        // snapshot it is on, and `G_RunFrame` drains the lookat queue after
        // writing `level.time` (docs/research/cod11-gsc-object-model.md 23.1).
        let event = self.vm.with_cx(|cx| cx.intern_folded("trigger"));
        for (id, other) in std::mem::take(&mut self.host.trigger_fires) {
            if self.host.ents.get(id).is_some() && self.host.ents.get(other).is_some() {
                self.vm
                    .notify(Target::Entity(id), event, &[Value::Entity(other)]);
            }
        }
        // Thinks before threads: `G_RunFrame` runs the entity pass first, so
        // a script reading `getEntArray` in the same frame sees the freed
        // entity already gone. Whether retail really orders it this way is
        // what `probe_delete`'s post-wait count measures.
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.run_entity_thinks(cx, now_ms));
        self.host.run_turret_thinks();
        // The body queue is not in the object table, so its own think -- the
        // 250 ms `eFlags` 0x800 clear -- runs beside the table's.
        self.host
            .bodies
            .run_thinks(now_ms, &vcod_common::net::protocol::PROTOCOL_V1);
        // The movers belong to the same entity pass: retail integrates them
        // in `G_RunFrame` ahead of the thread pass, so a thread parked on
        // `movedone` wakes on the frame the motion ended rather than the one
        // after (`crate::game::mover`).
        let host = &mut self.host;
        let done = self.vm.with_cx(|cx| crate::game::mover::run(host, cx));
        for d in done {
            let event = self.vm.with_cx(|cx| cx.intern_folded(d.event));
            self.vm.notify(Target::Entity(d.ent), event, &[]);
        }
        for e in self.vm.run_frame(&mut self.host, now_ms) {
            log::warn!("script error: {e:?}");
        }
    }
}

#[cfg(test)]
impl ScriptRuntime {
    /// A placed item at `at`, the way the map load makes one.
    pub fn place_item(&mut self, classname: &str, at: [f32; 3], count: i32) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        let id = self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            for (f, v) in [
                ("classname", Value::String(cx.intern_exact(classname))),
                ("origin", Value::Vector(at)),
                ("count", Value::Int(count)),
            ] {
                let a = cx.intern_folded(f);
                host.set_field(cx, id, a, v).unwrap();
            }
            id
        });
        crate::game::item::attach(
            &mut self.host,
            id,
            crate::items::classname_index(classname).unwrap(),
        );
        id
    }

    /// Compiles `src` as `maps/mp/test`, builds a host with an empty object
    /// table (no paks, no bsp, no missing-builtin pre-scan), and starts
    /// `main`. For this module's own tests.
    pub fn for_test(src: &str) -> ScriptRuntime {
        Self::for_test_at("maps/mp/test", src)
    }

    /// `for_test` with the file path spelled out, for a test whose script has
    /// to answer to a path the runtime dispatches into by name --
    /// `CALLBACK_SETUP` and its `CodeCallback_*` entry points.
    pub fn for_test_at(path: &str, src: &str) -> ScriptRuntime {
        let mut vm = Vm::new();
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, path, vm.interner_mut())
            .expect("test script compiles");
        vm.install(fns).expect("test script installs");
        let host = GameHost::new(vec![String::new(); 2048]);
        let mut rt = ScriptRuntime {
            vm,
            host,
            entry: path.to_string(),
            gametype_entry: String::new(),
            // Fixed, not drawn: a test's `fire` gate must reproduce the same
            // draw on every run.
            rng: 0x5eed_5eed_5eed_5eed,
        };
        let main = rt.vm.func_ref(&rt.entry, "main");
        rt.vm.start_thread(&mut rt.host, 0, main, None, vec![]);
        rt
    }

    /// Compile and install another file's worth of functions into a test
    /// runtime, so a test can add a thread body after `for_test`.
    pub fn install_for_test(&mut self, src: &str) {
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, &self.entry, self.vm.interner_mut())
            .expect("test script compiles");
        self.vm.install(fns).expect("test script installs");
    }

    /// A map entity at `origin`, the way the entity lump makes one.
    pub fn spawn_map_entity_for_test(&mut self, origin: [f32; 3]) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            let at = cx.intern_folded("origin");
            host.set_field(cx, id, at, Value::Vector(origin)).unwrap();
            id
        })
    }

    /// A client entity in `slot` at `origin`.
    pub fn spawn_client_for_test(&mut self, slot: usize, origin: [f32; 3]) -> EntId {
        let host = &mut self.host;
        let id = self
            .vm
            .with_cx(|cx| host.ents.spawn_client(cx, slot, None).unwrap());
        self.set_client_origin(slot, origin);
        id
    }

    /// A client's `sessionstate`, which `spawn_client` leaves at
    /// `"spectator"`; the four legal strings are in
    /// docs/research/cod11-map-cycle.md 6.1.
    pub fn set_client_state_for_test(&mut self, slot: usize, state: &str) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("sessionstate");
            let v = Value::String(cx.intern_exact(state));
            host.set_field(cx, ent, field, v).unwrap();
        });
    }

    /// Start `name` as a thread on `ent`, the way a script's `thread` does.
    pub fn start_thread_for_test(&mut self, ent: EntId, name: &str, now_ms: i32) {
        let f = self.vm.func_ref(&self.entry, name);
        self.vm
            .start_thread(&mut self.host, now_ms, f, Some(Target::Entity(ent)), vec![]);
    }

    /// Set a test client's health and max health, the way a spawn does.
    pub fn set_client_health_for_test(&mut self, slot: usize, health: i32) {
        if let Some(v) = self.host.client_vitals.get_mut(slot) {
            v.health = health;
            v.max_health = health;
        }
    }

    /// Writes a folded field on `level`.
    pub fn set_level_field_for_test(&mut self, name: &str, v: Value) {
        let level = self.vm.level_id();
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            cx.set_field(level, atom, v);
        });
    }

    /// Reads a folded field off `level`.
    pub fn level_field(&mut self, name: &str) -> vcod_gsc::Value {
        let level = self.vm.level_id();
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            cx.get_field(level, atom)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-tick copy from the sim keeps an op the sim has not applied
    /// yet: the copy predates it, the mirror must not.
    #[test]
    fn the_per_tick_ammo_copy_keeps_a_queued_op() {
        use crate::game::host::WeaponOp;
        use vcod_common::pmove::weapon::NUM_AMMO;
        let mut rt = ScriptRuntime::for_test("main() {}");
        rt.host.weapon_op(
            0,
            WeaponOp::SetAmmo {
                ammo_index: 10,
                rounds: 400,
            },
        );
        rt.host.weapon_op(
            1,
            WeaponOp::SetClip {
                clip_index: 3,
                rounds: 7,
            },
        );
        let mut clip = [0; NUM_AMMO];
        clip[3] = 2;
        rt.set_client_ammo(0, [0; NUM_AMMO], clip);
        assert_eq!(rt.host.client_ammo[0].ammo[10], 400);
        assert_eq!(rt.host.client_ammo[0].clip[3], 2, "slot 1's op leaked");
    }

    /// Any nonzero value works (`xorshift`'s only constraint); these tests
    /// never touch a trigger, so the draw itself is never observed.
    const TEST_RNG_SEED: u64 = 1;

    /// The packet pass runs the threads the netcode's events woke and
    /// nothing else. It carries no deadline wake of its own, so a thread
    /// looping on `wait 0` advances exactly one iteration per server frame;
    /// waking deadlines there stepped it twice, once at the previous frame's
    /// clock and again at this one's.
    #[test]
    fn a_wait_zero_loop_advances_once_per_server_frame() {
        let mut rt = ScriptRuntime::for_test(
            "main() { level.n = 0; for(;;) { wait 0; level.n = level.n + 1; } }",
        );
        for frame in 1..=5 {
            rt.run_frame(frame * 50);
            assert_eq!(
                rt.level_field("n"),
                Value::Int(frame),
                "after {frame} frames"
            );
        }
    }

    /// `run_thinks` still runs ahead of the frame's thread pass, which is
    /// `probe_delete`'s measurement: an entity deleted in one frame is out of
    /// `getEntArray` for a thread that wakes past the deferred free, and
    /// still in it for one that looks before. The packet pass ahead of both
    /// must not step that thread early.
    #[test]
    fn a_deleted_entity_is_gone_by_the_thread_pass_that_looks_past_the_defer() {
        let mut rt = ScriptRuntime::for_test(
            "main() { e = spawn(\"script_origin\", (0, 0, 0)); e delete(); \
             level.now = getentarray(\"script_origin\", \"classname\").size; \
             wait 0.5; \
             level.later = getentarray(\"script_origin\", \"classname\").size; }",
        );
        assert_eq!(
            rt.level_field("now"),
            Value::Int(1),
            "delete() freed at once"
        );
        for frame in 1..=12 {
            rt.run_frame(frame * 50);
        }
        assert_eq!(rt.level_field("later"), Value::Int(0));
    }

    /// Both closures load into one `Vm` through one `Loader`, which is what
    /// keeps `Vm::install`'s duplicate rejection from firing on the first
    /// file the two share (`_utility`, `_teams` and the rest).
    #[test]
    fn the_map_and_gametype_closures_both_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_pavlov",
            "dm",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            TEST_RNG_SEED,
            Carry::default(),
        );
        assert!(rt.is_ok(), "{:?}", rt.err());
    }

    /// mp_depot's `*1` is a `script_brushmodel` carrying `script_exploder`
    /// and `targetname "exploder"`, which is the arm of `_load.gsc::main`
    /// that calls `notsolid()`. The bootstrap therefore has to leave those
    /// brushes out of the clip; `*2` carries `script_exploder` with no
    /// targetname, so no arm reaches it and it stays solid. mp_powcamp
    /// (`*3`, `*9`) and mp_rocket (`*3`) are the other two stock maps with
    /// one. The gametype is `sd` because `*1` is also a `bombzone`, which
    /// `_gameobjects::main` deletes -- unlinking it for another reason --
    /// under every other gametype.
    #[test]
    fn the_bootstrap_unlinks_mp_depots_exploder_brush_model() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs.read("maps/mp/mp_depot.bsp").expect("mp_depot.bsp");
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        // No paks for the props: only the submodel brushes matter here.
        let world = Rc::new(crate::world::World::from_bsp(&bsp, None));
        ScriptRuntime::load(
            Rc::new(fs),
            "mp_depot",
            "sd",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            Some(world.clone()),
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            TEST_RNG_SEED,
            Carry::default(),
        )
        .expect("load mp_depot on sd");
        assert!(!world.collision.model_linked(1), "exploder stays solid");
        assert!(world.collision.model_linked(2));
    }

    /// Stock `_utility::getPlant` from where retail's planter stood in the
    /// plant capture, facing its view yaw: neither 18-unit trace meets the
    /// clip-only floor, the fallback's `(+16, +16)` trace lands on bombzone_A's
    /// flak88 `script_model`, and `objective_add` truncates that to the
    /// `-176, 2473, -22` retail's slot 0 reads at 91800
    /// (docs/research/cod11-gsc-object-model.md 23.3, 23.6).
    #[test]
    fn get_plant_puts_the_charge_where_retail_did_on_mp_carentan() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan.bsp");
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        let world = Rc::new(crate::world::World::from_bsp(&bsp, Some(&fs)));
        let mut rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_carentan",
            "sd",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            Some(world),
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            TEST_RNG_SEED,
            Carry::default(),
        )
        .expect("load mp_carentan on sd");
        rt.install_for_test(
            "plant() { self.angles = (0, 29.6, 0); p = self maps\\mp\\_utility::getPlant(); \
             level.charge = p.origin; objective_add(0, \"current\", p.origin); }",
        );
        let planter = rt.spawn_map_entity_for_test([-192.8, 2457.1, -21.9]);
        rt.start_thread_for_test(planter, "plant", 0);
        rt.run_frame(50);
        assert_eq!(rt.aborts(), Vec::<String>::new());
        let Value::Vector(charge) = rt.level_field("charge") else {
            panic!("getPlant returned no origin");
        };
        // The slot is all retail measured of the charge, and it is truncated,
        // so each component is within a unit of it rather than the point.
        let retail = glam::Vec3::new(-176.0, 2473.0, -22.0);
        assert!(
            (glam::Vec3::from(charge) - retail).abs().max_element() < 1.0,
            "charge at {charge:?}"
        );
        assert_eq!(rt.host.objectives[0].origin_f32(), retail.to_array());
    }

    /// `mp_pavlov.gsc` sets `game["allies"] = "russian"`, and dm's
    /// `Callback_StartGameType` turns that into `team_russiangerman`, so
    /// this pins that the map's `main` ran before the code callback and that
    /// all three precache families reached the table. It does **not** pin
    /// the order of the two `main`s: `dm.gsc`'s own `main` never reads
    /// `game["allies"]` and its `Callback_StartGameType` defaults the key
    /// when unset, so 1180 reads the same either way. That order is pinned
    /// by `probe_bootstrap_matches_retail` in
    /// `crates/server/tests/semantics_ents.rs`, which drives this same
    /// bootstrap with the probe as its gametype.
    #[test]
    fn the_bootstrap_precaches_the_russian_team_menu_on_mp_pavlov() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_pavlov",
            "dm",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            TEST_RNG_SEED,
            Carry::default(),
        )
        .expect("load mp_pavlov on dm");
        assert_eq!(rt.configstrings()[1180], "team_russiangerman");
        assert_eq!(rt.configstrings()[1181], "weapon_russian");
        assert_eq!(rt.configstrings()[21], "gfx/hud/hud@status_dead.tga");
        assert_eq!(
            rt.configstrings()[1501],
            "levelshots/layouts/hud@layout_mp_pavlov"
        );
    }

    /// A gametype script that resolves but defines no `main` reached
    /// `Vm::start_thread`'s "target must be installed" panic. `--gametype` is
    /// user input, so it has to come back as an error `main.rs` exits on,
    /// same as a gametype whose file does not exist at all.
    #[test]
    fn a_gametype_without_a_main_is_an_error_not_a_panic() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        /// The paks with one gametype path answered by a script that has
        /// every part of a gametype except the entry point.
        struct NoMain(Rc<Pk3Fs>);
        impl ScriptSource for NoMain {
            fn read(&self, canonical: &str) -> Option<String> {
                if canonical == "maps/mp/gametypes/nomain" {
                    return Some("notMain() {}\n".to_string());
                }
                PakScripts::new(self.0.clone(), None).read(canonical)
            }
        }
        let fs = Rc::new(fs);
        let err = ScriptRuntime::load_from(
            Box::new(NoMain(fs.clone())),
            fs,
            "mp_pavlov",
            "nomain",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            Rc::new(crate::weapons::WeaponTable::empty()),
            0,
            TEST_RNG_SEED,
            Carry::default(),
        );
        let Err(err) = err else {
            panic!("a gametype with no main must not load");
        };
        assert!(
            err.to_string().contains("defines no main()"),
            "unexpected error: {err:#}"
        );
    }

    /// The engine's side of the connect handshake is one notify and its exact
    /// spelling. A miss does not error, it hangs the thread, so this test
    /// exists to fail loudly if the constant is ever edited.
    #[test]
    fn the_begin_notify_is_spelled_begin() {
        assert_eq!(BEGIN_NOTIFY, "begin");
    }

    /// The whole team-join state machine wakes on this one notify, and a
    /// misspelling hangs the loop with nothing logged, so the spelling is
    /// pinned the same way `begin` is.
    #[test]
    fn the_menu_notify_is_spelled_menuresponse() {
        assert_eq!(MENURESPONSE_NOTIFY, "menuresponse");
    }

    /// What `savePersist` keeps for a client across a restart
    /// (docs/research/cod11-map-cycle.md section 1): the `pers[]` table, and
    /// not the rest of the entity. The second level is a second
    /// `ScriptRuntime` with its own `Vm`, so this only passes if the copy
    /// travelled as text through the carry and was re-interned on the far
    /// side. The two levels default the key differently, so the value read
    /// after says which one wrote it.
    #[test]
    fn a_carried_pers_is_back_on_the_client_the_next_level_connects() {
        fn src(default_team: &str) -> String {
            format!(
                "main() {{ level.callbackPlayerConnect = ::c; }}\n\
                 CodeCallback_PlayerConnect() {{ [[level.callbackPlayerConnect]](); }}\n\
                 c() {{ if (!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"{default_team}\"; \
                       self.statusicon = self.pers[\"team\"]; }}\n"
            )
        }

        let mut before = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("axis"));
        before.reconnect_client(0, "vcod".into(), 50);
        assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
        // Something only the first level wrote, to show what does not carry.
        before.host.client_vitals[0].health = 42;
        let carry = before.take_carry();

        let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
        after.host.pers_carry = carry.pers;
        after.reconnect_client(0, "vcod".into(), 50);
        assert_eq!(
            after.client_pers(0, "team").as_deref(),
            Some("axis"),
            "the carried pers did not reach the reconnected client"
        );
        // The `gclient_t` itself does not carry: retail bzeroes it in
        // `ClientConnect` and the script-side `pers` is the whole of what
        // survives.
        assert_eq!(after.host.client_vitals[0].health, 0);

        // The carry is spent by the connect that took it, so the next one
        // starts empty -- which is every connect but a persisting
        // boundary's.
        after.reconnect_client(0, "vcod".into(), 100);
        assert_eq!(after.client_pers(0, "team").as_deref(), Some("allies"));
    }

    /// The carry a level boundary lifted is spent by a disconnect as well as
    /// by a connect. `map_restart` sends its `n` through the reliable guard,
    /// which can drop the client whose `pers[]` was just lifted, and the next
    /// client to take that slot must not inherit the old one's team, score or
    /// weapons.
    #[test]
    fn a_disconnect_spends_the_slot_s_carried_pers() {
        fn src(default_team: &str) -> String {
            format!(
                "main() {{ level.callbackPlayerConnect = ::c; \
                          level.callbackPlayerDisconnect = ::d; }}\n\
                 CodeCallback_PlayerConnect() {{ [[level.callbackPlayerConnect]](); }}\n\
                 CodeCallback_PlayerDisconnect() {{ [[level.callbackPlayerDisconnect]](); }}\n\
                 c() {{ if (!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"{default_team}\"; }}\n\
                 d() {{ level.gone = 1; }}\n"
            )
        }

        let mut before = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("axis"));
        before.reconnect_client(0, "vcod".into(), 50);
        assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
        let carry = before.take_carry();

        let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
        after.host.pers_carry = carry.pers;
        // The drop the restart's `n` can cause: the slot has no entity in
        // this runtime yet, its `ClientConnect` has not run.
        after.push_client_event(ClientEvent::Disconnect(0));
        after.run_frame(50);

        after.reconnect_client(0, "stranger".into(), 100);
        assert_eq!(
            after.client_pers(0, "team").as_deref(),
            Some("allies"),
            "the dropped client's carried pers reached the next occupant"
        );
        assert!(after.aborts().is_empty(), "{:?}", after.aborts());
    }

    /// Connect arms the callback's `waittill("begin")` and Begin releases it.
    /// Order matters and the failure is silent: `Callback_PlayerConnect`
    /// blocks on that notify as its second statement, so a Begin drained
    /// before its Connect leaves the thread parked forever with nothing
    /// logged.
    #[test]
    fn connect_arms_the_wait_and_begin_releases_it() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerConnect = ::c; }\n\
             CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
             c() { self.statusicon = \"connecting\"; self waittill(\"begin\"); \
                   self.statusicon = \"begun\"; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "statusicon"), "connecting");

        rt.push_client_event(ClientEvent::Begin(0));
        rt.run_frame(100);
        assert_eq!(rt.field_str(ent, "statusicon"), "begun");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// Both events in one frame, which is what a client whose first usercmd
    /// lands before the next `run_frame` produces. It holds because `start_thread`
    /// steps a new thread to its first suspend before returning, so the
    /// `waittill` is armed by the time the Begin behind it in the queue is
    /// drained; nothing else pins that.
    #[test]
    fn a_connect_and_a_begin_in_one_frame_still_release_the_wait() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerConnect = ::c; }\n\
             CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
             c() { self.statusicon = \"connecting\"; self waittill(\"begin\"); \
                   self.statusicon = \"begun\"; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.push_client_event(ClientEvent::Begin(0));
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "statusicon"), "begun");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// The two things a client entity carries before any script touches it:
    /// `.name` from the userinfo, and a `.pers` the gametype can index. Both
    /// are measured on the retail 1.1d server -- a probe gametype logged
    /// `pers` defined and `self.name` correct inside
    /// `Callback_PlayerConnect`, before its `waittill("begin")`. Without
    /// either, `dm.gsc`'s connect callback dies before it opens a menu.
    #[test]
    fn a_fresh_client_entity_carries_its_name_and_an_indexable_pers() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerConnect = ::c; }\n\
             CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
             c() { if(!isdefined(self.pers[\"team\"])) self.pers[\"team\"] = \"spectator\"; \
                   self.statusicon = self.name + \":\" + self.pers[\"team\"]; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "janost".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "statusicon"), "janost:spectator");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// The `menuresponse` notify carries the menu's *name*, read back out of
    /// its configstring slot, not the index the client sent. Every
    /// gametype's join loop compares that first argument against
    /// `game["menu_team"]` and hands it back to `openMenu`, so an index
    /// would match nothing and the loop would spin forever.
    #[test]
    fn a_menu_response_notifies_the_menu_name_and_the_response() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerConnect = ::c; }\n\
             CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
             c() { self waittill(\"menuresponse\", menu, response); \
                   self.statusicon = menu + \":\" + response; }\n",
        );
        let (lo, _) = crate::configstrings::CsRange::Menu.bounds();
        rt.host.configstrings[lo + 1] = "weapon_russian".to_string();
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");

        rt.menu_response(0, 1, "mosin_nagant_mp");
        rt.run_frame(100);
        assert_eq!(
            rt.field_str(ent, "statusicon"),
            "weapon_russian:mosin_nagant_mp"
        );
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// Disconnect runs its callback on the entity and only then frees the
    /// slot, so the callback still has a `self` to read and the next frame
    /// finds the slot empty.
    #[test]
    fn disconnect_runs_the_callback_before_it_frees_the_slot() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerDisconnect = ::d; }\n\
             CodeCallback_PlayerConnect() {}\n\
             CodeCallback_PlayerDisconnect() { [[level.callbackPlayerDisconnect]](); }\n\
             d() { level.gone = self getEntityNumber(); }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 3,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        assert!(rt.client_entity(3).is_some());

        rt.push_client_event(ClientEvent::Disconnect(3));
        rt.run_frame(100);
        assert_eq!(rt.level_field("gone"), vcod_gsc::Value::Int(3));
        assert!(rt.client_entity(3).is_none());
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// A thread still polling the player when it disconnects dies with the
    /// entity instead of erroring on a freed `self`, which is what the
    /// stock `waitRespawnButton` did to a dead player who left.
    #[test]
    fn a_disconnect_kills_the_threads_running_on_the_player() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() { self thread poll(); }\n\
             CodeCallback_PlayerDisconnect() {}\n\
             poll() { for(;;) { level.polls = self useButtonPressed(); wait .05; } }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 3,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        rt.run_frame(100);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());

        rt.push_client_event(ClientEvent::Disconnect(3));
        for f in 3..8 {
            rt.run_frame(f * 50);
        }
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// The object table has to survive past map load, so the runtime owns the
    /// host. A thread that spawns an entity in one frame must find it in the
    /// next.
    #[test]
    fn an_entity_spawned_in_one_frame_survives_into_the_next() {
        let mut rt = ScriptRuntime::for_test(
            "main() { level.e = spawn(\"script_origin\", (0,0,0)); wait 0.05; \
             level.n = level.e getEntityNumber(); }",
        );
        rt.run_frame(0);
        rt.run_frame(100);
        let n = rt.level_field("n");
        assert_eq!(n, vcod_gsc::Value::Int(72));
    }

    /// A client standing in a trigger wakes the thread parked on it, with the
    /// toucher as the notify's argument. Retail raises this inside
    /// `G_TouchTriggers` (0x3f88c) with `Scr_AddEntity` supplying `other`.
    #[test]
    fn touching_a_trigger_notifies_the_parked_thread() {
        let mut rt = ScriptRuntime::for_test(
            "main() { level.hits = 0; level thread watch(); }\n\
             watch() { for(;;) { level waittill(\"go\"); } }\n",
        );
        // The trigger's own thread, started on the entity the test registers.
        let src = "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
                   level.hits = level.hits + 1; level.who = other; } }";
        rt.install_for_test(src);

        let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::Multiple,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.run_frame(0);

        let player = rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        assert_eq!(rt.client_entity(0), Some(player));
        rt.set_client_state_for_test(0, "playing");

        rt.touch_triggers_with_buttons(0, 50, 0);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
        assert_eq!(rt.level_field("who"), Value::Entity(player));

        // Out of the box: no second notify.
        rt.set_client_origin(0, [500.0, 0.0, 0.0]);
        rt.touch_triggers_with_buttons(0, 100, 0);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
    }

    /// A touch lands between frames, and the thread it wakes runs on the
    /// next frame's clock: retail's plant bar carries the `scaleStartTime` of
    /// the first snapshot it is on (object-model doc 23.1).
    #[test]
    fn a_triggered_thread_runs_on_the_next_frame_s_clock() {
        let mut rt = ScriptRuntime::for_test("main() {}");
        rt.install_for_test(
            "trigger_think() { self waittill(\"trigger\", other); level.at = getTime(); }",
        );
        let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::Multiple,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.run_frame(0);
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.touch_triggers_with_buttons(0, 0, 0);
        rt.run_frame(50);
        assert_eq!(rt.level_field("at"), Value::Int(50));
    }

    /// The aim trace starts at the eye truncated toward zero, the way
    /// `CalcMuzzlePoints` snaps the muzzle point (object-model doc 23.1): an
    /// eye at z 60.9 aims level from z 60, under a box whose top is 60.5.
    #[test]
    fn the_aim_trace_starts_at_the_truncated_eye() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } }",
        );
        let zone = rt.spawn_map_entity_for_test([100.0, 0.0, 44.5]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::LookAt,
            crate::game::trigger::TriggerShape::boxed([-8.0, -8.0, 0.0], [8.0, 8.0, 16.0]),
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.run_frame(0);
        rt.spawn_client_for_test(0, [0.9, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);
        rt.set_client_aim(0, [0.9, 0.0, 60.9], [0.0, 0.0]);
        rt.aim_lookat(0, 50);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
    }

    /// The aim trace fires a `trigger_lookat` the client's view enters, with
    /// the aimer as `other`, and `isLookingAt` answers off the same trace
    /// (docs/research/cod11-gsc-object-model.md 23.1). Aimed away, neither.
    #[test]
    fn aiming_at_a_lookat_trigger_notifies_it_and_backs_islookingat() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; level.who = other; } }\n\
             check() { level.looking = self islookingat(level.zone); }",
        );
        let zone = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::LookAt,
            crate::game::trigger::TriggerShape::boxed([-20.0, -20.0, 40.0], [20.0, 20.0, 80.0]),
            0,
            0,
        );
        rt.set_level_field_for_test("zone", Value::Entity(zone));
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.run_frame(0);

        let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);

        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0]);
        rt.aim_lookat(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
        assert_eq!(rt.level_field("who"), Value::Entity(player));
        rt.start_thread_for_test(player, "check", 50);
        rt.run_frame(100);
        assert_eq!(rt.level_field("looking"), Value::Int(1));

        // Aimed away: no fire, and the answer drops.
        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 90.0]);
        rt.aim_lookat(0, 150);
        rt.run_frame(150);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
        rt.start_thread_for_test(player, "check", 150);
        rt.run_frame(200);
        assert_eq!(rt.level_field("looking"), Value::Int(0));

        // Every frame it is aimed at, since `G_Trigger` gates nothing.
        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0]);
        rt.aim_lookat(0, 250);
        rt.run_frame(250);
        rt.aim_lookat(0, 300);
        rt.run_frame(300);
        assert_eq!(rt.level_field("hits"), Value::Int(3));
    }

    /// A script `delete()` takes the trigger row with the entity, and the
    /// entity number it hands back carries no box into its next tenant.
    /// `sd.gsc` deletes both bombzones at the plant, so a row that outlived
    /// its entity would keep firing at the old brush -- and, once the number
    /// is reused, fire on a spawn that was never a trigger.
    #[test]
    fn a_deleted_trigger_takes_its_row_and_leaves_the_reused_number_clean() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } }\n\
             remove() { self delete(); }",
        );
        let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::Multiple,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.touch_triggers_with_buttons(0, 0, 0);
        rt.run_frame(0);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "the row fires first");

        // `delete()` defers the free by `DELETE_DEFER_MS`; 200 is past it.
        rt.start_thread_for_test(zone, "remove", 0);
        rt.run_frame(0);
        rt.run_frame(200);

        assert!(rt.host.ents.get(zone).is_none(), "the entity is gone");
        assert!(rt.host.triggers.get(zone).is_none(), "the row is gone");

        // The free list hands the number straight back out, and its new
        // tenant is a plain entity: a point box at its own origin, not the
        // dead trigger's brush.
        let reused = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
        assert_eq!(reused.0, zone.0, "the number is reused");
        let host = &mut rt.host;
        let bounds = rt
            .vm
            .with_cx(|cx| crate::game::trigger::entity_abs_bounds(host, cx, reused));
        assert_eq!(bounds, ([300.0, 0.0, 0.0], [300.0, 0.0, 0.0]));

        // And standing back in the old box notifies nobody.
        rt.set_client_origin(0, [10.0, 0.0, 0.0]);
        rt.touch_triggers_with_buttons(0, 250, 0);
        rt.run_frame(250);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "no further notify");
    }

    /// `G_TouchTriggers` runs its pass only for `ps.pm_type <= 1`
    /// (docs/research/cod11-gsc-object-model.md 8.2), so of the four values a
    /// client can carry on our wire only a living player's 0 gets through:
    /// spectator 4, intermission 5 and dead 6 all fail the compare. The dead
    /// case is the minefield bug -- a corpse lying in the trigger re-armed
    /// `minefield_kill` on every pass.
    #[test]
    fn only_a_living_client_touches_a_trigger() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } }",
        );
        let zone = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::Multiple,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            0,
            0,
        );
        rt.start_thread_for_test(zone, "trigger_think", 0);
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.run_frame(0);

        // A spectator, at `pm_type` 4. The old gate excluded it off
        // `sessionstate`; the measured rule excludes it off the compare, so
        // the behaviour is unchanged.
        rt.set_client_state_for_test(0, "spectator");
        rt.set_client_pm_type(0, crate::spectate::PM_SPECTATOR);
        rt.touch_triggers_with_buttons(0, 50, 0);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "a spectator touched");

        rt.set_client_state_for_test(0, "intermission");
        rt.set_client_pm_type(0, crate::spectate::PM_INTERMISSION);
        rt.touch_triggers_with_buttons(0, 75, 0);
        rt.run_frame(75);
        assert_eq!(
            rt.level_field("hits"),
            Value::Int(0),
            "an intermission camera touched"
        );

        rt.set_client_state_for_test(0, "dead");
        rt.set_client_pm_type(0, crate::spectate::PM_DEAD);
        rt.touch_triggers_with_buttons(0, 100, 0);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "a corpse touched");

        // And the living player the gate is there to let through.
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);
        rt.touch_triggers_with_buttons(0, 125, 0);
        rt.run_frame(125);
        assert_eq!(
            rt.level_field("hits"),
            Value::Int(1),
            "a live player did not"
        );
    }

    /// A `trigger_hurt` damages the player standing in it through the stock
    /// damage callback, so the victim's health drops on the path a bullet's
    /// hit already takes, and the 100 ms cadence `register_hurt` arms gates
    /// the second helping.
    #[test]
    fn a_trigger_hurt_damages_the_player_in_it() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
             dir, hitloc) { level.damage = damage; level.mod = mod; \
             self.health = self.health - damage; }\n",
        );
        rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_health_for_test(0, 100);

        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register_hurt(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            5,
            0,
        );

        rt.touch_triggers_with_buttons(0, 100, 0);
        rt.run_frame(100);
        let expected_mod = rt
            .vm
            .with_cx(|cx| cx.intern_exact(crate::game::trigger::MOD_TRIGGER_HURT));
        assert_eq!(rt.level_field("damage"), Value::Int(5));
        assert_eq!(rt.level_field("mod"), Value::String(expected_mod));
        assert_eq!(rt.client_vitals(0).health, 95);

        rt.touch_triggers_with_buttons(0, 150, 0);
        rt.run_frame(150);
        assert_eq!(rt.client_vitals(0).health, 95, "inside the 100 ms window");

        rt.touch_triggers_with_buttons(0, 200, 0);
        rt.run_frame(200);
        assert_eq!(rt.client_vitals(0).health, 90);
    }

    /// A `trigger_use` answers the use key: standing in one raises nothing
    /// until the bit is down. The `auto1`/`auto2` MG42 mount pairs on the
    /// stock maps are what this serves. The keyless touch also leaves the
    /// `wait` window unarmed, so the next keyed touch still fires.
    #[test]
    fn a_trigger_use_needs_the_use_key() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } }",
        );
        let mount = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            mount,
            crate::game::trigger::TriggerKind::Use,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            1000,
            0,
        );
        rt.start_thread_for_test(mount, "trigger_think", 0);
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.run_frame(0);

        rt.touch_triggers_with_buttons(0, 50, 0);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "no use key");

        rt.touch_triggers_with_buttons(0, 100, vcod_common::net::msg::BUTTON_USE);
        rt.run_frame(100);
        assert_eq!(
            rt.level_field("hits"),
            Value::Int(1),
            "the window was armed"
        );

        rt.touch_triggers_with_buttons(0, 150, vcod_common::net::msg::BUTTON_USE);
        rt.run_frame(150);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "inside the window");
    }

    /// The link re-anchor reads the parent through its handle, so a parent
    /// freed and replaced in its slot between two frames releases the link
    /// rather than handing the planter to the new occupant.
    #[test]
    fn a_freed_parent_has_no_origin_once_its_slot_is_reused() {
        let mut rt = ScriptRuntime::for_test("main() {}");
        let parent = rt.spawn_map_entity_for_test([1.0, 2.0, 3.0]);
        assert_eq!(rt.entity_origin_of(parent), Some([1.0, 2.0, 3.0]));
        rt.host.free_entity(parent);
        let next = rt.spawn_map_entity_for_test([9.0, 9.0, 9.0]);
        assert_eq!(next.0, parent.0, "the slot was not reused");
        assert_eq!(rt.entity_origin_of(parent), None);
    }

    /// `sd.gsc`'s `bombzone_think` with two attackers: the first aborts and
    /// destroys its bar, the second starts at the other zone and its bar
    /// takes the freed record, the first retries. Its
    /// `isdefined(other.progressbackground)` has to read 0, as a destroyed
    /// hudelem does on retail (`# probe_stale_handle`), so it makes a bar of
    /// its own and its `setShader` lands there rather than on the second's.
    #[test]
    fn a_retried_plant_gets_its_own_bar_after_another_took_the_freed_one() {
        let mut rt = ScriptRuntime::for_test("main() {}");
        rt.install_for_test(
            "plant() { \
               if(!isdefined(self.progressbackground)) \
                 self.progressbackground = newClientHudElem(self); \
               self.progressbackground setShader(\"black\", 124, 12); } \
             abort() { self.progressbackground destroy(); }",
        );
        let a = rt.spawn_client_for_test(0, [0.0; 3]);
        let b = rt.spawn_client_for_test(1, [0.0; 3]);
        let bar = |rt: &mut ScriptRuntime, p: EntId| {
            use vcod_gsc::Host;
            let host = &mut rt.host;
            rt.vm.with_cx(|cx| {
                let f = cx.intern_folded("progressbackground");
                host.get_field(cx, p, f)
            })
        };
        rt.start_thread_for_test(a, "plant", 0);
        let Value::Entity(first) = bar(&mut rt, a) else {
            panic!("no bar for the first attempt");
        };
        rt.start_thread_for_test(a, "abort", 0);
        rt.start_thread_for_test(b, "plant", 0);
        let Value::Entity(b_bar) = bar(&mut rt, b) else {
            panic!("no bar for the second attacker");
        };
        assert_eq!(b_bar.0, first.0, "the second bar did not reuse the record");
        rt.start_thread_for_test(a, "plant", 0);
        assert_eq!(rt.aborts(), Vec::<String>::new());

        let Value::Entity(a_bar) = bar(&mut rt, a) else {
            panic!("the retry made no bar");
        };
        assert_ne!(a_bar, b_bar, "the retry reused the other attacker's bar");
        let owner =
            |rt: &ScriptRuntime, id| rt.host.ents.get(id).and_then(|e| e.hud).map(|h| h.owner);
        assert_eq!(owner(&rt, a_bar), Some(a.0));
        assert_eq!(owner(&rt, b_bar), Some(b.0));
    }

    /// A connect callback that returns at once, so a test client gets its
    /// entity without the stock menus.
    const PICKUP_CALLBACKS: &str = "main() {}\nCodeCallback_PlayerConnect() {}\n";

    /// One client at the origin with 100/100 health, the allied loadout on
    /// the host mirrors, and a table built from the stock numbers.
    fn pickup_rig() -> ScriptRuntime {
        pickup_rig_with(PICKUP_CALLBACKS)
    }

    fn pickup_rig_with(callbacks: &str) -> ScriptRuntime {
        use crate::game::host::Vitals;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, callbacks);
        rt.host.weapons = Rc::new(crate::game::pickup::tests_table());
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "p".into(),
        });
        rt.run_frame(0);
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
        };
        for (name, s) in [("m1carbine_mp", 1), ("colt_mp", 3), ("fraggrenade_mp", 4)] {
            let w = crate::configstrings::weapon_index(name).unwrap();
            let d = rt.host.weapons.get(w).unwrap().clone();
            rt.host.client_weapons[0].give(w, s);
            rt.host.client_ammo[0].clip[d.clip_index] = d.clip_size as i16;
            if !d.clip_only {
                rt.host.client_ammo[0].ammo[d.ammo_index] = d.max_ammo as i16;
            }
        }
        rt.host.client_weapons[0].current =
            crate::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
        rt.set_client_origin(0, [0.0; 3]);
        rt
    }

    const DOWN: [f32; 3] = [87.9, 0.0, 0.0];

    #[test]
    fn a_health_pack_underfoot_heals_and_leaves_the_wire() {
        let mut rt = pickup_rig();
        rt.host.client_vitals[0].health = 50;
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        assert_eq!(rt.host.client_vitals[0].health, 75);
        assert!(rt.host.ents.get(id).unwrap().item.unwrap().taken);
        assert_eq!(
            rt.take_sim_ops(),
            vec![(
                0,
                crate::game::host::SimOp::Event {
                    event: 146,
                    parm: 68
                }
            )]
        );
        assert_eq!(
            rt.take_client_commands(),
            vec![(0, "f \"GAME_PICKUP_HEALTH\u{15}25\"".to_string())]
        );
        assert!(rt.script_log().iter().any(|l| l == "Item: 0 item_health"));
    }

    /// A grabbed drop is freed 100 ms on; a grabbed placed item stays
    /// allocated, off the wire (section 7, removal).
    #[test]
    fn a_grabbed_drop_is_freed_and_a_grabbed_placed_item_stays_hidden() {
        let mut rt = pickup_rig();
        rt.host.client_vitals[0].health = 10;
        let placed = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        let drop = rt.place_item("item_health", [4.0, 0.0, 1.0], 0);
        rt.host
            .ents
            .get_mut(drop)
            .unwrap()
            .item
            .as_mut()
            .unwrap()
            .dropped = true;
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        rt.run_frame(50);
        assert!(rt.host.ents.get(drop).is_some(), "freed before 100 ms");
        rt.run_frame(100);
        assert!(rt.host.ents.get(drop).is_none(), "the drop was not freed");
        rt.run_frame(1000);
        assert!(rt.host.ents.get(placed).unwrap().item.unwrap().taken);
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        assert!(!rt.packet_entities(p).contains_key(&placed.0));
    }

    #[test]
    fn a_health_pack_at_full_health_stays() {
        let mut rt = pickup_rig();
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
        assert!(rt.take_sim_ops().is_empty());
    }

    #[test]
    fn a_dead_or_spectating_player_takes_nothing() {
        use crate::game::host::Vitals;
        let mut rt = pickup_rig();
        rt.host.client_vitals[0].health = 50;
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        rt.set_client_pm_type(0, 4);
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, [0.0, 0.0, 60.0], DOWN);
        rt.set_client_pm_type(0, 0);
        rt.host.client_vitals[0] = Vitals {
            health: 0,
            max_health: 100,
            dead: true,
        };
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
    }

    /// The use key acts on its rising edge only: a use held across ten cmds
    /// in front of two health packs, both out of the touch box and each
    /// still grabbable after the other, takes one.
    #[test]
    fn a_held_use_key_grabs_once() {
        let mut rt = pickup_rig();
        rt.host.client_vitals[0].health = 10;
        let a = rt.place_item("item_health", [40.0, 0.0, 30.0], 0);
        let b = rt.place_item("item_health", [40.0, 4.0, 30.0], 0);
        for _ in 0..10 {
            rt.item_pass(
                0,
                vcod_common::net::msg::BUTTON_USE,
                [0.0, 0.0, 60.0],
                [36.0, 0.0, 0.0],
            );
        }
        let taken = [a, b]
            .iter()
            .filter(|id| rt.host.ents.get(**id).unwrap().item.unwrap().taken)
            .count();
        assert_eq!(taken, 1);
        assert_eq!(rt.host.client_vitals[0].health, 35);
    }

    /// Two fg42s underfoot on one cmd, the player holding one short of full:
    /// the first grab fills the reserve and the second, reading the mirror
    /// the first moved, finds nothing to take.
    #[test]
    fn two_items_in_reach_on_one_cmd_see_each_others_ammo() {
        let mut rt = pickup_rig();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        let d = rt.host.weapons.get(fg).unwrap().clone();
        rt.host.client_weapons[0].give(fg, 2);
        rt.host.client_ammo[0].clip[d.clip_index] = 20;
        rt.host.client_ammo[0].ammo[d.ammo_index] = 300;
        let a = rt.place_item("mpweapon_fg42", [0.0, 0.0, 1.0], 90);
        let b = rt.place_item("mpweapon_fg42", [4.0, 0.0, 1.0], 90);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        assert!(rt.host.ents.get(a).unwrap().item.unwrap().taken);
        assert!(!rt.host.ents.get(b).unwrap().item.unwrap().taken);
        assert_eq!(rt.host.client_ammo[0].ammo[d.ammo_index], 320);
    }

    /// The swap with the use key: the carbine lands on the panzerfaust's own
    /// origin, carries its rounds and its dropper, and the `"trigger"` notify
    /// names the player and the carbine, in that order.
    #[test]
    fn the_use_key_swaps_and_the_trigger_notify_names_the_drop() {
        let mut rt = pickup_rig();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        rt.host.client_weapons[0].give(fg, 2);
        let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 30.0], 0);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], [36.0, 0.0, 0.0]);
        rt.item_pass(
            0,
            vcod_common::net::msg::BUTTON_USE,
            [0.0, 0.0, 60.0],
            [36.0, 0.0, 0.0],
        );
        assert!(rt.host.ents.get(pf).unwrap().item.unwrap().taken);
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap();
        let (drop, state) = rt
            .host
            .ents
            .iter_inuse()
            .find_map(|(id, e)| {
                e.item
                    .filter(|i| i.index as usize == carbine)
                    .map(|i| (id, i))
            })
            .expect("the carbine was dropped");
        assert_eq!(state.owner, Some(0));
        assert_eq!(state.clip, 15);
        assert_eq!(rt.entity_origin_of(drop), Some([40.0, 0.0, 30.0]));
        let client = rt.client_entity(0).unwrap();
        assert!(rt.host.item_notifies.iter().any(|(id, ev, args)| *id == pf
            && *ev == "trigger"
            && args == &vec![Value::Entity(client), Value::Entity(drop)]));
        // `Cmd_Activate_f` notifies the item alone, never the player.
        assert!(!rt
            .host
            .item_notifies
            .iter()
            .any(|(id, ev, _)| *id == client && *ev == "touch"));
        let panzerfaust = crate::configstrings::weapon_index("panzerfaust_mp").unwrap();
        assert!(rt
            .take_client_commands()
            .contains(&(0, format!("a {panzerfaust}"))));
    }

    /// A stock stand turret at (40, 0, 0) facing +x, and `pickup_rig`'s
    /// client plus a second one, both on the ground behind it.
    fn turret_rig() -> (ScriptRuntime, EntId) {
        use vcod_gsc::Host;
        let mut rt = pickup_rig();
        rt.push_client_event(ClientEvent::Connect {
            slot: 1,
            name: "q".into(),
        });
        rt.run_frame(0);
        rt.host.client_vitals[1] = rt.host.client_vitals[0];
        rt.set_client_origin(1, [0.0, 4.0, 0.0]);
        rt.set_client_on_ground(0, true);
        rt.set_client_on_ground(1, true);
        rt.host.configstrings[1212] = "CGAME_USEMG42".into();
        let host = &mut rt.host;
        let gun = rt.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            let a = cx.intern_folded("origin");
            host.set_field(cx, id, a, Value::Vector([40.0, 0.0, 0.0]))
                .unwrap();
            id
        });
        let def = crate::game::turret::TurretDef::parse(
            "WEAPONFILE\\weaponClass\\turret\\leftArc\\45\\rightArc\\45\\topArc\\40\\bottomArc\\40\\useHintString\\CGAME_USEMG42",
        )
        .unwrap();
        rt.host.turrets.insert(
            gun,
            crate::game::turret::TurretRecord::new(
                "mg42_bipod_stand_mp",
                def,
                Default::default(),
                0.0,
            ),
        );
        (rt, gun)
    }

    const AT_GUN: [f32; 3] = [30.0, 0.0, 0.0];

    /// How many mounts the drain after `slot`'s pass applied.
    fn mount_queued(rt: &mut ScriptRuntime, slot: usize) -> usize {
        rt.take_turret_mounts(slot, [0.0; 3], vcod_common::pmove::Stance::Stand, AT_GUN)
            .len()
    }

    #[test]
    fn a_usable_turret_hints_mg42_with_its_hint_string_slot() {
        let (mut rt, _) = turret_rig();
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_GUN), (6, Some(0)));
    }

    /// The server drains each cmd's mount before the next cmd's pass, so the
    /// second client's press in the same frame finds the gun manned.
    #[test]
    fn two_presses_on_one_gun_mount_only_the_first() {
        let (mut rt, gun) = turret_rig();
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(mount_queued(&mut rt, 0), 1);
        rt.item_pass(1, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(mount_queued(&mut rt, 1), 0);
        assert_eq!(rt.host.turrets[&gun].owner, Some(0));
    }

    /// A gunner's press is the release request and nothing else, and a
    /// gunner reads hint 0 with the string left where the mount left it.
    #[test]
    fn a_gunners_use_press_asks_for_the_release_and_mounts_nothing() {
        let (mut rt, gun) = turret_rig();
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        mount_queued(&mut rt, 0);
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_GUN), (0, None));
        let fg42 = rt.place_item("mpweapon_fg42", [40.0, 0.0, 40.0], 90);
        rt.item_pass(0, 0, EYE, AT_GUN);
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert!(rt.host.turret_ops.is_empty());
        assert_eq!(rt.host.turrets[&gun].busy, 2);
        assert!(!rt.host.ents.get(fg42).unwrap().item.unwrap().taken);
    }

    /// `Cmd_Activate_f` has no `pm_type` gate (turrets doc 4.1): a gunner
    /// past the touch pass's gate still asks for the release.
    #[test]
    fn a_gunners_press_asks_for_the_release_whatever_its_pm_type() {
        let (mut rt, gun) = turret_rig();
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        mount_queued(&mut rt, 0);
        rt.item_pass(0, 0, EYE, AT_GUN);
        rt.set_client_pm_type(0, 6);
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(rt.host.turrets[&gun].busy, 2);
    }

    const EYE: [f32; 3] = [0.0, 0.0, 60.0];
    const AT_ITEM: [f32; 3] = [36.0, 0.0, 0.0];

    #[test]
    fn aiming_at_an_unowned_fg42_hints_nine_past_its_index() {
        let mut rt = pickup_rig();
        rt.place_item("mpweapon_fg42", [40.0, 0.0, 30.0], 90);
        // fg42 is index 6: the capture's 15 (docs/research/cod11-items.md 12.3).
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 15);
        assert_eq!(rt.cursor_hint_pass(0, EYE, [-36.0, 0.0, 0.0]).0, 0);
        rt.host.client_vitals[0] = crate::game::host::Vitals {
            health: 0,
            max_health: 100,
            dead: true,
        };
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
    }

    /// An owned fg42 short of full hints 73 past its index, the capture's
    /// 79, and once taken the same aim hints nothing.
    #[test]
    fn an_owned_weapon_hints_73_past_its_index_until_taken() {
        let mut rt = pickup_rig();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        let d = rt.host.weapons.get(fg).unwrap().clone();
        rt.host.client_weapons[0].give(fg, 2);
        rt.host.client_ammo[0].ammo[d.ammo_index] = 100;
        rt.place_item("mpweapon_fg42", [40.0, 0.0, 30.0], 90);
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 79);
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
    }

    /// The capture's swap: 32 on the panzerfaust, 0 on the carbine it
    /// dropped while its dropper is locked out, 21 once the lock clears.
    #[test]
    fn an_owner_locked_drop_hints_nothing_until_its_lock_clears() {
        let mut rt = pickup_rig();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        rt.host.client_weapons[0].give(fg, 2);
        rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 30.0], 0);
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 32);
        rt.item_pass(0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap();
        let drop = rt
            .host
            .ents
            .iter_inuse()
            .find_map(|(id, e)| e.item.filter(|i| i.index as usize == carbine).map(|_| id))
            .expect("the carbine was dropped");
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 0);
        rt.host
            .ents
            .get_mut(drop)
            .unwrap()
            .item
            .as_mut()
            .unwrap()
            .owner = None;
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM).0, 21);
    }

    /// The item pass's notifies reach script at the next frame: a player
    /// parked on `"touch"` wakes with the item, before the grab it refused.
    #[test]
    fn a_touch_notify_wakes_a_waiting_player_thread() {
        let mut rt = pickup_rig_with(
            "main() {}\nCodeCallback_PlayerConnect() { self waittill(\"touch\", item); logPrint(\"touched \" + item.classname); }\n",
        );
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        assert!(!rt.host.ents.get(id).unwrap().item.unwrap().taken);
        rt.run_frame(50);
        assert!(rt.script_log().iter().any(|l| l == "touched item_health"));
    }

    /// A thread an item notify wakes runs before the frame's `wait`s come
    /// due, whatever the two threads' ages: an older loop polling a flag the
    /// `"trigger"` waiter sets sees it on the grab's own frame, the way
    /// retail's `probe_pickup` teleport did (docs/research/cod11-items.md,
    /// 13.1).
    #[test]
    fn an_item_notified_thread_runs_before_the_frames_waits() {
        let mut rt = pickup_rig_with(
            "main() { level.flag = 0; thread poll(); }\n\
             poll() { for (;;) { wait 0.05; if (level.flag) { logPrint(\"seen \" + getTime()); return; } } }\n\
             watch() { self waittill(\"trigger\", player); level.flag = 1; }\n\
             CodeCallback_PlayerConnect() {}\n",
        );
        rt.host.client_vitals[0].health = 50;
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        rt.start_thread_for_test(id, "watch", 0);
        rt.item_pass(0, 0, [0.0, 0.0, 60.0], DOWN);
        rt.run_frame(50);
        rt.run_frame(100);
        let seen: Vec<&String> = rt
            .script_log()
            .iter()
            .filter(|l| l.starts_with("seen"))
            .collect();
        assert_eq!(seen, vec!["seen 50"]);
    }
}
