//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::rc::Rc;

use crate::game::host::{ClientEvent, GameHost, SpawnRequest};
use crate::game::spawn::spawn_entities_from_string;
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

pub struct ScriptRuntime {
    vm: Vm,
    pub(crate) host: GameHost,
    entry: String,
    gametype_entry: String,
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
        let id = EntId(u32::try_from(slot).ok()?);
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
        // Thinks before threads: `G_RunFrame` runs the entity pass first, so
        // a script reading `getEntArray` in the same frame sees the freed
        // entity already gone. Whether retail really orders it this way is
        // what `probe_delete`'s post-wait count measures.
        self.host.ents.run_thinks(now_ms);
        // The body queue is not in the object table, so its own think -- the
        // 250 ms `eFlags` 0x800 clear -- runs beside the table's.
        self.host
            .bodies
            .run_thinks(now_ms, &vcod_common::net::protocol::PROTOCOL_V1);
        for e in self.vm.run_frame(&mut self.host, now_ms) {
            log::warn!("script error: {e:?}");
        }
    }
}

#[cfg(test)]
impl ScriptRuntime {
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
        };
        let main = rt.vm.func_ref(&rt.entry, "main");
        rt.vm.start_thread(&mut rt.host, 0, main, None, vec![]);
        rt
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
            Carry::default(),
        );
        assert!(rt.is_ok(), "{:?}", rt.err());
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
}
