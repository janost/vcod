//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::collections::HashSet;
use std::rc::Rc;
use vcod_common::asset_source::{AssetSource, RawFile};

use crate::game::builtins::set::BuiltinSet;
use crate::game::host::{ClientEvent, GameHost, SpawnRequest};
use crate::game::item::Activate;
use crate::game::profile::{HostProfile, WeaponNumbering};
use crate::game::spawn::spawn_entities_from_string;
use crate::game::turret::{TurretOp, TurretShot};
use vcod_common::mapworld::MapWorld;
use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{EntId, Loader, ScriptSource, Target, Value, Vm};

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

/// The `script_struct` pair CoD2's engine binds (cod2_lnxded 0x811041e).
const COD2_STRUCT: &str = "codescripts/struct";

/// Reads `.gsc` off the asset source as raw files. `Loader` hands `read`
/// a canonical path (lowercase, forward slashes, no extension; see
/// `vcod_gsc::canonical`); the pak stores e.g. `maps/MP/_load.gsc`, and
/// the source's lookup is case-blind, so appending `.gsc` is the only work.
pub(crate) struct PakScripts {
    fs: Rc<dyn AssetSource>,
    /// One canonical path answered from memory instead of the paks, for a
    /// test whose gametype script does not ship in one
    /// (`Server::overlay_script`).
    overlay: Option<(String, String)>,
}

impl PakScripts {
    pub(crate) fn new(fs: Rc<dyn AssetSource>, overlay: Option<(String, String)>) -> PakScripts {
        PakScripts { fs, overlay }
    }
}

impl ScriptSource for PakScripts {
    fn read(&self, canonical: &str) -> Option<String> {
        if let Some((path, text)) = self.overlay.as_ref()
            && path == canonical
        {
            return Some(text.clone());
        }
        let raw = self.fs.load::<RawFile>(&format!("{canonical}.gsc")).ok()?;
        Some(raw.text().into_owned())
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
    /// The entity area tree, also the engine's: a `map_restart` keeps its
    /// nodes and drops its entities (combat doc 14.7).
    pub area: Option<crate::area::AreaTree>,
    /// What the level read off the BSP. A restart keeps the map, so it
    /// skips the read and the parse, which were most of what made it slower
    /// than retail's (map-cycle doc, 4.7).
    pub map: Option<Rc<LevelMap>>,
    /// The compiled gametype and map closures. A restart loads the same
    /// files, so it reuses them instead of compiling again (map-cycle doc,
    /// 4.7).
    pub scripts: Option<Rc<CompiledScripts>>,
    /// The last level's gametype closure, compiled, which a load of any
    /// map under the same gametype reuses while its files read the same.
    pub gametype: Option<Rc<GametypeScripts>>,
    /// `G_InitGame`'s `Rand_Init` seed for the script's draws, a new one
    /// every level, restarts included (map-cycle doc 4.8); `None` (a test
    /// building a runtime by hand) takes [`crate::game::host::RNG_SEED`].
    /// CoD2's `srand` takes it as well.
    pub seed: Option<u64>,
    /// A test's replay of another server's spawn picks
    /// ([`crate::game::host::SpawnPins`]), handed to every level.
    pub spawn_pins: Option<crate::game::host::SpawnPins>,
    /// The xmodels the level loaded for traces and links. A restart keeps
    /// the map and the paks, so it reads the same files (map-cycle doc 4.9).
    pub models: Option<crate::game::host::ModelCaches>,
}

/// The two closures a level loaded, compiled, before any of it ran.
pub struct CompiledScripts {
    gametype_entry: String,
    entry: String,
    program: vcod_gsc::Program,
    /// The `#using_animtree` names the closure declares.
    animtrees: Vec<String>,
    /// The compile as it stood after the gametype's closure, which the
    /// next map's load starts from (`Carry::gametype`).
    gametype: Rc<GametypeScripts>,
}

/// The gametype's closure, compiled first on every load and the same
/// whatever the map: a map change resumes the compile from here instead
/// of starting over, once every file it read still reads the same
/// (`LoaderState::current`). Retail compiles every load; the resumed
/// compile is the one a fresh compile would have reached.
pub struct GametypeScripts {
    gametype_entry: String,
    /// The builtin set the compile resolved calls against, one per game
    /// and protocol (`HostProfile::builtins`).
    set: &'static BuiltinSet,
    program: vcod_gsc::Program,
    loader: vcod_gsc::LoaderState,
}

impl CompiledScripts {
    /// The animtrees the level's scripts name, whose `.atr` the engine
    /// reads at the load (cod11-server-handshake.md, "The four cvars").
    pub fn animtrees(&self) -> &[String] {
        &self.animtrees
    }

    /// The gametype closure this compile started from or recorded.
    #[cfg(test)]
    pub(crate) fn gametype(&self) -> &Rc<GametypeScripts> {
        &self.gametype
    }

    /// Whether `other` compiled to the same thing: every atom in the same
    /// place, the same functions with the same code, the same animtrees.
    #[cfg(test)]
    pub(crate) fn same_compile(&self, other: &CompiledScripts) -> bool {
        let (a, b) = (
            Vm::from_program(&self.program),
            Vm::from_program(&other.program),
        );
        let funcs = |vm: &Vm| {
            let mut v: Vec<_> = vm
                .functions()
                .map(|f| {
                    (
                        f.file,
                        f.name,
                        format!("{:?} {:?} {:?}", f.code, f.consts, f.lines),
                    )
                })
                .collect();
            v.sort();
            v
        };
        a.interner().texts().eq(b.interner().texts())
            && funcs(&a) == funcs(&b)
            && self.animtrees == other.animtrees
            && self.entry == other.entry
            && self.gametype_entry == other.gametype_entry
    }
}

/// The BSP's share of a level load: the entity string the spawn pass
/// reads and the two brush-model tables the host keeps.
pub struct LevelMap {
    entities: String,
    model_bounds: Vec<([f32; 3], [f32; 3])>,
    model_brushes: Vec<Vec<crate::game::trigger::BrushHull>>,
}

impl LevelMap {
    /// A level whose map gives its entity string and the brush model
    /// tables apart (CoD4's clip map, `game::cod4`).
    pub(crate) fn of_parts(
        entities: String,
        model_bounds: Vec<([f32; 3], [f32; 3])>,
        model_brushes: Vec<Vec<crate::game::trigger::BrushHull>>,
    ) -> LevelMap {
        LevelMap {
            entities,
            model_bounds,
            model_brushes,
        }
    }

    fn of(bsp: &MapWorld) -> LevelMap {
        LevelMap {
            model_bounds: bsp.models.iter().map(|m| m.clip_bounds()).collect(),
            model_brushes: crate::game::trigger::model_brush_hulls(bsp),
            entities: bsp.entities.clone(),
        }
    }
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

/// Where `G_RunFrame`'s entity loop stands this frame
/// ([`ScriptRuntime::run_entity_pass`]).
pub struct EntityPass {
    next: u32,
    /// Every entity that has had its `G_RunEntity` this frame (0x502cb).
    ran: HashSet<EntId>,
    /// Each linked client's slot and parent, for `G_RunClient`'s turn.
    client_links: Vec<(usize, EntId)>,
}

/// A turn of the entity pass that needs the client sims, which
/// `Cod1Module::run_frame_cod1` holds: the pass returns it and is called again for the
/// next turn.
pub enum PassStop {
    /// A grenade went off on its turn (`G_RunMissile`); walk the blast.
    Blast(crate::game::missile::Explosion),
    /// A brush model mover moved on its turn (`G_MoverTeam`): push what it
    /// listed ([`ScriptRuntime::push_mover`]).
    Push(crate::game::mover::Step),
    /// `G_RunClient` (0x40660) for a linked client, its parent already run:
    /// re-anchor it.
    Client(usize),
}

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
        let profile = HostProfile::of_protocol_cvar(cvars.get("protocol"));
        Self::load_game(
            profile,
            source,
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

    /// A CoD2 level: `world` is the map's `.d3dbsp` as a [`MapWorld`]
    /// (`vcod_cod2::world`), whose entity string the spawn pass reads, and
    /// the scripts compile under `Dialect::Cod2` against CoD2's builtin
    /// tables. Groundwork for hosting CoD2: the host still runs 1.1's
    /// configstring layout and no weapon table.
    #[allow(clippy::too_many_arguments)]
    pub fn load_cod2(
        source: Box<dyn ScriptSource>,
        fs: Rc<Pk3Fs>,
        world: &MapWorld,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
    ) -> anyhow::Result<ScriptRuntime> {
        Self::load_cod2_in(
            source,
            fs,
            world,
            None,
            map,
            gametype,
            configstrings,
            cvars,
            now_ms,
        )
    }

    /// [`ScriptRuntime::load_cod2`] with the level's collision
    /// (`crate::game::cod2::level_world`), which the traces, the spawn
    /// pass's links and the triggers read.
    #[allow(clippy::too_many_arguments)]
    pub fn load_cod2_in(
        source: Box<dyn ScriptSource>,
        fs: Rc<Pk3Fs>,
        world: &MapWorld,
        collision: Option<Rc<crate::world::World>>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
    ) -> anyhow::Result<ScriptRuntime> {
        Self::load_cod2_carry(
            source,
            fs,
            world,
            collision,
            map,
            gametype,
            configstrings,
            cvars,
            now_ms,
            Carry::default(),
        )
    }

    /// [`ScriptRuntime::load_cod2_in`] with what the last level handed on
    /// (`take_carry`): `game[]` and `pers[]` under `savePersist`, the
    /// compiled closures on a restart of the same map. Its map and area
    /// tree are rebuilt off `world`.
    #[allow(clippy::too_many_arguments)]
    pub fn load_cod2_carry(
        source: Box<dyn ScriptSource>,
        fs: Rc<Pk3Fs>,
        world: &MapWorld,
        collision: Option<Rc<crate::world::World>>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
        carry: Carry,
    ) -> anyhow::Result<ScriptRuntime> {
        // No `misc_model` goes in the area tree: CoD2's xmodels are not
        // CoD1's, and nothing the bootstrap runs walks the tree.
        let area = match world.models.first() {
            Some(m) => crate::area::AreaTree::new(
                m.mins,
                m.maxs,
                crate::game::entity::MAX_GENTITIES as usize,
            ),
            None => crate::area::AreaTree::unbounded(),
        };
        let carry = Carry {
            map: Some(Rc::new(LevelMap::of(world))),
            area: Some(area),
            ..carry
        };
        Self::load_game(
            &crate::game::profile::COD2,
            source,
            fs,
            map,
            gametype,
            configstrings,
            cvars,
            collision,
            Rc::new(crate::weapons::WeaponTable::empty()),
            now_ms,
            carry,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn load_game(
        profile: &'static HostProfile,
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
        Self::load_game_with(
            profile,
            source,
            fs,
            map,
            gametype,
            configstrings,
            cvars,
            world,
            weapons,
            now_ms,
            carry,
            |_| {},
        )
    }

    /// [`Self::load_game`] with `setup` run on the fresh host ahead of the
    /// spawn pass: a game's own state the profile cannot hold
    /// (`game::cod4::load`'s zones).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load_game_with(
        profile: &'static HostProfile,
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
        setup: impl FnOnce(&mut GameHost),
    ) -> anyhow::Result<ScriptRuntime> {
        let proto = profile.defs.proto;
        let entry = format!("maps/mp/{map}");
        let gametype_entry = format!("maps/mp/gametypes/{gametype}");
        let cached = carry
            .scripts
            .filter(|c| c.gametype_entry == gametype_entry && c.entry == entry);
        let compile = crate::tick_profile::phase("load/compile");
        let (mut vm, scripts) = match cached {
            Some(c) => (Vm::from_program(&c.program), c),
            None => {
                let set = (profile.builtins)();
                let dialect = profile.vm.dialect;
                let resumed = carry.gametype.filter(|g| {
                    g.gametype_entry == gametype_entry
                        && std::ptr::eq(g.set, set)
                        && g.loader.current(source.as_ref())
                });
                // One `Loader`, not two: its `loaded` set dedupes the files
                // both closures share, and `Vm::install` rejects a duplicate
                // `FuncRef`, so a second `Loader` would fail on the first
                // shared file.
                let (mut vm, mut loader, gametype) = match resumed {
                    Some(g) => {
                        let mut loader = Loader::resume(source, dialect, g.loader.clone());
                        if profile.compile_against_builtins {
                            loader.set_builtins(set.table());
                        }
                        (Vm::from_program(&g.program), loader, g)
                    }
                    None => {
                        let mut vm = Vm::new();
                        let mut loader = Loader::with_dialect(source, dialect);
                        if profile.compile_against_builtins {
                            loader.set_builtins(set.table());
                        }
                        loader
                            .load(&mut vm, &gametype_entry)
                            .map_err(|e| anyhow::anyhow!("loading {gametype_entry}: {e:?}"))?;
                        let g = Rc::new(GametypeScripts {
                            gametype_entry: gametype_entry.clone(),
                            set,
                            program: vm.program(),
                            loader: loader.state(),
                        });
                        (vm, loader, g)
                    }
                };
                loader
                    .load(&mut vm, &entry)
                    .map_err(|e| anyhow::anyhow!("loading {entry}: {e:?}"))?;
                // CoD2's engine binds `codescripts/struct`'s pair for the
                // spawn pass's `script_struct` (cod2-gsc-diff.md 2.4).
                if profile.script_structs {
                    loader
                        .load(&mut vm, COD2_STRUCT)
                        .map_err(|e| anyhow::anyhow!("loading {COD2_STRUCT}: {e:?}"))?;
                }
                // Retail's compiler refuses a call outside its version's
                // tables: a script compile error, which drops the level
                // (`builtins::set`).
                let unknown = set.unknown_calls(&vm);
                if !unknown.is_empty() {
                    anyhow::bail!(
                        "script compile error in {entry} or {gametype_entry}: unknown (builtin) {}",
                        unknown.join(", ")
                    );
                }
                // A name retail lists that vcod does not answer yet must not
                // stop the map serving: a thread that reaches one dies, the
                // rest of the script runs. List them once here.
                let missing = set.unimplemented_calls(&vm);
                if !missing.is_empty() {
                    log::info!(
                        "gsc: {entry} and {gametype_entry} call {} builtins the host does not implement: {}",
                        missing.len(),
                        missing.join(" ")
                    );
                }
                let scripts = Rc::new(CompiledScripts {
                    gametype_entry: gametype_entry.clone(),
                    entry: entry.clone(),
                    program: vm.program(),
                    animtrees: loader.animtrees().map(str::to_string).collect(),
                    gametype,
                });
                (vm, scripts)
            }
        };
        drop(compile);
        vm.apply_profile(&profile.vm);
        // `game[]` before anything runs: the gametype's `main` reads it in
        // its first statements (map-cycle doc, section 1). After the
        // compile, so a cached closure and a fresh one intern alike.
        if let Some(game) = carry.game.as_ref() {
            vm.install_game(game);
        }

        let mut host = GameHost::new(profile, configstrings);
        host.cvars = cvars;
        // A frame-counted `wait` (CoD4) counts `sv_fps`'s frames from the
        // bootstrap's first.
        if profile.vm.frame_waits {
            vm.set_frame_ms(crate::game::cod4::frame_ms(&host.cvars));
        }
        // `Scr_Init` takes `developer` at the load (coduo_lnxded 0x80714f1):
        // below 1 a runtime error does not end its thread.
        vm.set_recover_errors(
            host.cvars
                .get("developer")
                .trim()
                .parse::<i32>()
                .unwrap_or(0)
                == 0,
        );
        host.patch_1_5 = matches!(host.cvars.get("protocol"), "6" | "22");
        host.missiles.uo = proto.is_uo();
        host.missiles.falls = !host.speaks_1_1();
        host.movers.stored = host.patch_1_5;
        host.allocators = crate::configstrings::Allocators::seeded(&host.configstrings, &proto.cs);
        host.items = crate::items::Items::new(host.defs);
        host.timeout = crate::game::timeout::MatchTimeout::new(crate::game::builtins::cvar::atoi(
            host.cvars.get("g_timeoutBank"),
        ));
        host.world = world;
        // The level's entities go, and the world copies with them.
        if let Some(w) = &host.world {
            w.collision.clear_world_copies();
        }
        host.weapons = weapons;
        host.fs = Some(fs.clone());
        setup(&mut host);
        host.level_time_ms = now_ms;
        host.dobj_time_ms = now_ms;
        host.ents.set_clock(now_ms);
        host.pers_carry = carry.pers;
        if let Some(seed) = carry.seed {
            host.rng = seed | 1;
            host.holdrand = vcod_common::rng::HoldRand(seed as u32);
            // CoD2's `G_InitGame` hands the one seed to `srand` too
            // (cod2-combat.md 9.6).
            if let crate::game::host::HostExt::Cod2(c) = &mut host.ext {
                c.rand.reseed(seed as u32);
                c.game_seed = Some(seed as u32);
            }
        }
        host.spawn_pins = carry.spawn_pins;
        if let Some(items) = carry.items {
            host.items = items;
        }
        if let Some(m) = carry.models {
            host.xmodel_collision = m.collision;
            host.xmodel_bounds = m.bounds;
            host.vehicles.rigs = m.rigs;
        }

        // `_load.gsc::main`, in mp_pavlov's own closure, calls `getEntArray`
        // in its first statements, so the object table must hold every map
        // entity before `main` runs: the entity lump goes in first, then the
        // pre-scan (which only reads the loader, not the table), and only
        // then does `main` start.
        let (level_map, area) = match (carry.map, carry.area) {
            (Some(level_map), Some(mut area)) => {
                area.unlink_all();
                (level_map, area)
            }
            _ => {
                let bsp = vcod_common::mapworld::find(&*fs, map)?;
                let area = crate::area::AreaTree::for_map(&bsp, &fs);
                (Rc::new(LevelMap::of(&bsp)), area)
            }
        };
        // `GScr_AddFieldsForRadiant` (cod2_lnxded 0x811ac96) reads
        // `radiant/keys.txt` at the load (cod2-gsc-diff.md 2.1).
        if profile.reads_radiant_keys {
            // CoD4's lives in `common_mp.ff`, read through the zones.
            let text = match &host.assets {
                Some(a) => a.read("radiant/keys.txt"),
                None => fs.read("radiant/keys.txt"),
            }
            .unwrap_or_default();
            host.radiant_keys = Some(crate::game::fields::parse_radiant_keys(
                &String::from_utf8_lossy(&text),
            ));
        }
        host.model_bounds = level_map.model_bounds.clone();
        host.model_brushes = level_map.model_brushes.clone();
        host.area = area;
        // `G_SetupWeaponDef` (0x8120d74), at the head of `G_InitGame`
        // (0x8109096): `defaultweapon_mp` is weapon 1, and its world model
        // the first model (cod2-protocol.md 15.3).
        if profile.weapons == WeaponNumbering::Registered {
            crate::game::builtins::cod2::register_weapon(&mut host, "defaultweapon_mp");
        }
        // `G_InitGame` then sets the `winner` slot (CoD2's 22) to 0.
        if profile.init_winner {
            let winner = host.allocators.layout().winner;
            host.configstrings.set(winner, "\\winner\\0");
        }
        let spawn = crate::tick_profile::phase("load/spawn");
        vm.with_cx(|cx| spawn_entities_from_string(&mut host, cx, &level_map.entities))
            .map_err(|e| anyhow::anyhow!("spawning {map}'s entities: {e:?}"))?;
        drop(spawn);
        if profile.script_structs {
            crate::game::cod2::create_structs(&mut vm, &mut host);
        }
        // `G_InitGame`'s `G_setfog("0")` past the spawn (0x4fe2e), ahead of
        // the scripts' own fog.
        let fog = host.allocators.layout().fog;
        host.configstrings.set(fog, "0");

        let mut rt = ScriptRuntime {
            vm,
            host,
            entry,
            gametype_entry,
            map: Some(level_map),
            scripts: Some(scripts),
            script_lag_ms: 0,
        };
        // `G_InitGame` hands `Scr_InitSystem` the level's own `level.time`
        // (docs/research/cod11-gsc-language.md, "The script clock").
        rt.vm.set_time(now_ms);
        let _p = crate::tick_profile::phase("load/bootstrap");
        rt.start_bootstrap()?;
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
    ///
    /// All three run under `level.initializing` (`GameHost::initializing`).
    fn start_bootstrap(&mut self) -> anyhow::Result<()> {
        self.host.initializing = true;
        let gametype_entry = self.gametype_entry.clone();
        self.start(&gametype_entry, "main", None)?;

        let entry = self.entry.clone();
        self.start(&entry, "main", None)?;

        // `CodeCallback_StartGameType` is defined in `_callbacksetup`, not in
        // the gametype script; it guards on `level.gametypestarted` and then
        // calls `level.callbackStartGameType`.
        self.start(CALLBACK_SETUP, "CodeCallback_StartGameType", None)?;
        self.host.initializing = false;
        if self.host.profile.weapons == WeaponNumbering::Registered {
            crate::game::builtins::cod2::write_weapon_configstrings(&mut self.host);
        }
        Ok(())
    }

    /// Starts one entry point as a thread with `recv` as its `self`, checking
    /// it is installed first. `Vm::start_thread` panics on a `FuncRef` that is
    /// not, and `--gametype` is user input, so a gametype script that loads
    /// but defines no `main` (or never pulls in `_callbacksetup`) has to fail
    /// the way a missing file does: an error `main.rs` exits on, not a panic.
    fn start(&mut self, path: &str, name: &str, recv: Option<Target>) -> anyhow::Result<()> {
        self.start_with_args(path, name, recv, vec![])
    }

    /// `start` with the arguments the entry point takes.
    pub(crate) fn start_with_args(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<Target>,
        args: Vec<Value>,
    ) -> anyhow::Result<()> {
        let f = self.vm.func_ref(path, name);
        if !self.vm.has_function(f) {
            anyhow::bail!("{path}.gsc defines no {name}()");
        }
        let args = if path == CALLBACK_SETUP {
            self.host.callback_args(name, args)
        } else {
            args
        };
        self.vm.start_thread(&mut self.host, f, recv, args);
        Ok(())
    }

    /// `Scr_PlayerDamage` (combat doc, section 4.4) for every hit this
    /// frame's attacks made: `CodeCallback_PlayerDamage` on the victim's
    /// entity with the nine arguments. The inflictor is the attacker himself
    /// unless the hit names one, which a blast does: there the missile that
    /// went off is handed over, still alive for as long as its explode event
    /// rides the wire. A hit on a slot with no entity, or from one, is
    /// dropped: there is nobody to call and nobody to name. An attacker of
    /// `ENTITYNUM_NONE` is a missile's whose thrower left, delivered with an
    /// undefined attacker (and on UO an undefined inflictor). So is one on a
    /// victim without `takedamage`, `G_Damage`'s first test (4.2 step 1),
    /// and one a vehicle seat shuts out ([`crate::game::vehicle::Vehicles::occupant_damage`]).
    pub fn deliver_hits(&mut self, hits: Vec<crate::game::combat::Hit>) {
        for mut hit in hits {
            if !self.client_vitals(hit.victim).takedamage {
                continue;
            }
            let Some(victim) = self.client_entity(hit.victim) else {
                continue;
            };
            // A missile whose thrower left hands `G_Damage` a NULL attacker,
            // which `Scr_PlayerDamage` passes as undefined (combat doc 13.7).
            // UO's drops the inflictor too, which `GScr_AddEntity` pushes as
            // undefined.
            let (attacker, inflictor) = match (self.attacker_entity(hit.attacker), hit.inflictor) {
                (Some(a), i) => (Value::Entity(a), Some(i.unwrap_or(a))),
                // A zeroed attacker with no entity 0 in use: retail pushes
                // the free entity's object, which vcod has none of.
                (None, i)
                    if hit.attacker == crate::game::host::ENTITYNUM_NONE as usize
                        || hit.attacker == crate::game::host::ZEROED_ENTITY =>
                {
                    (Value::Undefined, i)
                }
                _ => continue,
            };
            let Some(damage) = self
                .host
                .vehicles
                .occupant_damage(hit.victim, inflictor, hit.damage, hit.dflags)
            else {
                continue;
            };
            hit.damage = damage;
            let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
                (
                    cx.intern_exact(hit.mod_),
                    cx.intern_exact(&hit.weapon),
                    cx.intern_exact(hit.hitloc),
                )
            });
            let args = vec![
                inflictor.map_or(Value::Undefined, Value::Entity),
                attacker,
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
            ) {
                log::error!("gsc: {e:#}");
            }
        }
    }

    /// A round's `G_Damage` on an entity with no client (combat doc 2.4,
    /// step 4): a UO vehicle's branch ([`crate::game::vehicle::vehicle_damage`])
    /// with the shot's forward as the direction, any other entity's generic
    /// arm. The notifies wake their waiters with the frame's threads.
    /// A round's, a swing's or a shell's `G_CheckHitTriggerDamage`
    /// (`crate::game::damage_trigger::sweep`), charged to the attacker's
    /// client entity, or the world when it has none.
    pub fn trigger_sweep(&mut self, t: &crate::game::combat::TriggerSweep) {
        let attacker = self.attacker_entity(t.attacker);
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let attacker = attacker.unwrap_or_else(|| host.ents.world(cx));
            let mod_ = host.defs.mod_index(t.mod_).unwrap_or(0);
            crate::game::damage_trigger::sweep(
                host, cx, attacker, t.start, t.end, t.damage, mod_, false,
            );
        });
    }

    pub fn deliver_entity_hit(&mut self, hit: &crate::game::combat::EntityHit) {
        let id = hit.target;
        if self.host.ents.get(id).is_none() {
            return;
        }
        let attacker = self.attacker_entity(hit.attacker);
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            let world = host.ents.world(cx);
            let attacker = attacker.unwrap_or(world);
            let inflictor = hit.inflictor.unwrap_or(attacker);
            let out = if host.vehicles.live.contains_key(&id) {
                crate::game::vehicle::vehicle_damage(
                    host,
                    cx,
                    id,
                    Some(inflictor),
                    Some(attacker),
                    hit.dir,
                    hit.point,
                    hit.damage,
                    hit.dflags,
                    hit.mod_,
                )
            } else {
                let entity_hit = crate::game::host::EntityHit {
                    inflictor,
                    mod_: hit.mod_.to_string(),
                    dir: hit.dir,
                    point: hit.point,
                };
                host.damage_nonclient(cx, id, hit.damage, attacker, &entity_hit)
            };
            out.into_iter()
                .map(|(event, args)| (cx.intern_folded(event), args))
                .collect::<Vec<_>>()
        });
        for (event, args) in notifies {
            self.vm.notify(Target::Entity(id), event, &args);
        }
        self.vehicle_deaths();
    }

    /// `Scr_Vehicle_Die` for every vehicle death queued outside a builtin
    /// (a round in a cmd, a blast in the entity pass, a collision in the
    /// think): each rider's probe callback runs to its first `wait` before
    /// the next rider's ([`crate::game::vehicle::die`]).
    pub fn vehicle_deaths(&mut self) {
        loop {
            let host = &mut self.host;
            let next = self
                .vm
                .with_cx(|cx| crate::game::vehicle::die::drain(host, cx));
            for (on, event, args) in std::mem::take(&mut self.host.vehicles.die_notifies) {
                let event = self.vm.with_cx(|cx| cx.intern_folded(event));
                self.vm.notify(Target::Entity(on), event, &args);
            }
            match next {
                Some((victim, args)) => {
                    if let Err(e) = self.start_with_args(
                        CALLBACK_SETUP,
                        "CodeCallback_PlayerDamage",
                        Some(Target::Entity(victim)),
                        args,
                    ) {
                        log::error!("gsc: {e:#}");
                    }
                }
                None if self.host.vehicles.deaths.is_empty()
                    && self.host.vehicles.dying.is_empty() =>
                {
                    return;
                }
                None => {}
            }
        }
    }

    /// Damage whose attacker is not a player. `hurt_touch` (0x64dc4) hands
    /// `G_Damage` the trigger entity for both the inflictor and the attacker
    /// and zero for the point and the direction, which the immediates it
    /// pushes show directly (VERIFIED), and `Scr_PlayerDamage` passes a null
    /// point or direction as undefined; the stock callback tests
    /// `isPlayer(attacker)`, so a non-player attacker is a shape the corpus
    /// already handles. A blocked mover's crush is the same call.
    pub fn deliver_world_hit(
        &mut self,
        victim_slot: usize,
        inflictor: EntId,
        damage: i32,
        dflags: i32,
        mod_: &str,
    ) {
        self.deliver_unowned_hit(
            victim_slot,
            [inflictor, inflictor],
            damage,
            dflags,
            mod_,
            "none",
            None,
            None,
        );
    }

    /// An entity's int field, 0 when it holds none.
    fn int_field(&mut self, id: EntId, name: &str) -> i32 {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            match vcod_gsc::Host::get_field(host, cx, id, atom) {
                Value::Int(i) => i,
                _ => 0,
            }
        })
    }

    /// `G_PlaySoundAliasAtPoint`: an `EV_SOUND_ALIAS` temp entity at `at`,
    /// culled by PVS (`G_Spawn` and no `svFlags` on 1.1d 0x67a2c,
    /// `G_TempEntity` on 1.5b and UO).
    fn add_sound_at(&mut self, at: [f32; 3], parm: i32) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            host.add_temp_entity(
                cx,
                crate::game::temp_entity::TempEntity {
                    event: EV_SOUND_ALIAS,
                    parm,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: at,
                    scope: crate::game::temp_entity::Scope::Pvs,
                    quake: None,
                    origin2: None,
                },
            )
        });
    }

    /// Damage from `[inflictor, attacker]`, neither a player:
    /// [`Self::deliver_world_hit`]'s shape with a weapon, a point and a
    /// direction, as a driverless vehicle's `VEH_PlayerDamage` passes them
    /// (a zero direction, coduo-vehicles.md 11.6). `None` reaches the
    /// callback undefined (`Scr_PlayerDamage`, game.mp 0x5ca18), and an
    /// undefined direction makes the stock gametypes add
    /// `DAMAGE_NO_KNOCKBACK` (cod11-movers.md 21, "The sine crush").
    #[allow(clippy::too_many_arguments)]
    pub fn deliver_unowned_hit(
        &mut self,
        victim_slot: usize,
        [inflictor, attacker]: [EntId; 2],
        damage: i32,
        dflags: i32,
        mod_: &str,
        weapon: &str,
        point: Option<[f32; 3]>,
        dir: Option<[f32; 3]>,
    ) {
        if !self.client_vitals(victim_slot).takedamage {
            return;
        }
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let Some(damage) =
            self.host
                .vehicles
                .occupant_damage(victim_slot, Some(inflictor), damage, dflags)
        else {
            return;
        };
        let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
            (
                cx.intern_exact(mod_),
                cx.intern_exact(weapon),
                cx.intern_exact("none"),
            )
        });
        let args = vec![
            Value::Entity(inflictor),
            Value::Entity(attacker),
            Value::Int(damage),
            Value::Int(dflags),
            Value::String(mod_),
            Value::String(weapon),
            point.map_or(Value::Undefined, Value::Vector),
            dir.map_or(Value::Undefined, Value::Vector),
            Value::String(hitloc),
        ];
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
            Some(Target::Entity(victim)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
    }

    /// `ClientEvents`' fall damage (player-clip doc 8.8): `G_Damage` with no
    /// inflictor, attacker, direction or point, means `MOD_FALLING` and hit
    /// location 0, which the damage callback reads as four undefined
    /// arguments, weapon `"none"` and hit location `"none"` (8.10).
    pub fn deliver_fall(&mut self, victim_slot: usize, damage: i32) {
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let (mod_, none) = self
            .vm
            .with_cx(|cx| (cx.intern_exact("MOD_FALLING"), cx.intern_exact("none")));
        let args = vec![
            Value::Undefined,
            Value::Undefined,
            Value::Int(damage),
            Value::Int(0),
            Value::String(mod_),
            Value::String(none),
            Value::Undefined,
            Value::Undefined,
            Value::String(none),
        ];
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
            Some(Target::Entity(victim)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
    }

    /// `G_TouchTriggers` for one client, which retail runs once per usercmd
    /// from `ClientThink_real` (0x405b3): the triggers and the items it
    /// touches, in the area tree's order (`trigger::touched`). The notifies
    /// only mark the threads parked on them runnable; they run in this
    /// tick's script frame.
    ///
    /// Only a client at `ps.pm_type <= 1` touches anything: the function's
    /// own second guard, which takes the dead, a spectator and the
    /// intermission camera out before the pass runs
    /// (docs/research/cod11-gsc-object-model.md 8.2).
    ///
    /// The touch is taken at the client's `origin` field;
    /// `touch_triggers_at` takes it at an explicit `ps.origin` instead.
    pub fn touch_triggers(&mut self, slot: usize, now_ms: i32) {
        let origin = self.client_origin(slot);
        self.touch_triggers_at(slot, now_ms, origin);
    }

    /// `ClientImpacts` for `slot`'s cmd: [`GameHost::client_impacts`].
    pub fn client_impacts(&mut self, slot: usize, touched: &[u32]) {
        self.host.client_impacts(slot, touched);
    }

    /// `G_TouchTriggers` with its boxes on `origin`, `ps.origin`, while the
    /// client's `origin` field holds the snapped `r.currentOrigin` a hurt's
    /// callbacks read (`docs/research/cod11-combat.md` 5.5).
    pub fn touch_triggers_at(&mut self, slot: usize, now_ms: i32, origin: [f32; 3]) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        let hits = self
            .vm
            .with_cx(|cx| crate::game::trigger::touched(host, cx, slot, origin));
        log::trace!(target: "touch_pass", "slot {slot} at {origin:?}: {hits:?}");
        // One walk, in the tree's order: each entity's two `"touch"`
        // notifies (0x3fa42, 0x3fa61), then its touch function, an item's
        // `Touch_Item` or a trigger's fire.
        for id in hits {
            let Some(e) = self.host.ents.get(id) else {
                continue;
            };
            let item = e.item.is_some();
            self.host
                .engine_notifies
                .push((id, "touch", vec![Value::Entity(client)]));
            self.host
                .engine_notifies
                .push((client, "touch", vec![Value::Entity(id)]));
            if item {
                let host = &mut self.host;
                self.vm
                    .with_cx(|cx| crate::game::item::touch(host, cx, id, slot, true));
                continue;
            }
            // `Touch_Multi` skips its wait arm on a trigger whose think is
            // `enableLinkTo`'s `Think_GeneralLink` (0x65aa2).
            let linked = self.host.links.is_enabled(id);
            // `hurt_touch` reads the trigger's `dmg` and `spawnflags` at
            // every touch (cod11-combat.md 5.9).
            let is_hurt = self
                .host
                .triggers
                .get(id)
                .is_some_and(|t| t.kind == crate::game::trigger::TriggerKind::Hurt);
            let hurt = is_hurt.then(|| {
                let (dmg, flags) = (self.int_field(id, "dmg"), self.int_field(id, "spawnflags"));
                (dmg, self.host.triggers.arm_hurt(id, flags), flags)
            });
            let fire = self.host.triggers.fire(id, now_ms, linked);
            if !fire.fired() {
                continue;
            }
            // `Touch_Multi` arms the free off `level.time`, which inside a
            // cmd is still the last frame's.
            if fire == crate::game::trigger::Fire::Spent {
                let at = self.host.level_time_ms + crate::game::trigger::SPENT_FREE_MS;
                self.host
                    .ents
                    .schedule(id, crate::game::entity::ThinkFn::Free, at);
            }
            if !self.host.queue_trigger(id, client) {
                self.host
                    .engine_notifies
                    .push((id, "trigger", vec![Value::Entity(client)]));
            }
            if let Some((damage, dflags, flags)) = hurt {
                // `G_PlaySoundAliasAtPoint` at the toucher's origin, a temp
                // entity taken ahead of the damage callback's.
                let sound = self.host.triggers.get(id).map_or(0, |t| t.sound);
                if flags & crate::game::trigger::HURT_SILENT == 0 && sound != 0 {
                    let at = self.client_origin(slot);
                    self.add_sound_at(at, sound);
                }
                self.deliver_world_hit(
                    slot,
                    id,
                    damage,
                    dflags,
                    crate::game::trigger::MOD_TRIGGER_HURT,
                );
            }
        }
    }

    /// Where the `radiusDamage` calls since the last ask went off.
    pub fn take_blast_noises(&mut self) -> Vec<[f32; 3]> {
        std::mem::take(&mut self.host.blast_noises)
    }

    /// Engine code that needs the host and the VM's context together,
    /// outside a builtin: a vehicle's seat change or step. A thread it
    /// spawns is dropped (`Cx::spawn`), so it raises notifies instead.
    pub fn with_host<R>(&mut self, f: impl FnOnce(&mut GameHost, &mut vcod_gsc::Cx) -> R) -> R {
        let host = &mut self.host;
        self.vm.with_cx(|cx| f(host, cx))
    }

    /// The buttons of the last cmd `item_pass` saw from `slot`, which its
    /// use edge is taken against.
    pub fn client_old_buttons(&self, slot: usize) -> u8 {
        self.host.client_old_buttons.get(slot).copied().unwrap_or(0)
    }

    /// `Cmd_Activate_f` for one cmd (docs/research/cod11-items.md, section
    /// 2): on the use key's rising edge, the item, turret or `trigger_use`
    /// the aim picks. Gated as the touch pass is, and on the player being
    /// alive, `G_CheckForCursorHints`' own gate. The items the player walks
    /// onto are touched in `touch_triggers_at`, in the area tree's order
    /// among the triggers.
    pub fn item_pass(&mut self, slot: usize, buttons: u8, eye: [f32; 3], view: [f32; 3]) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        let old = std::mem::replace(&mut self.host.client_old_buttons[slot], buttons);
        let pressed = buttons & !old & vcod_common::net::msg::BUTTON_USE != 0;
        // `G_IsVehicleUnusable` first: a rider's press is the vehicle's use,
        // the exit (game.mp.uo 0x4dffb).
        if pressed
            && let Some((veh, _)) = self.host.vehicles.seat_of(slot)
            && self.host.vehicles.live[&veh].contents & crate::game::vehicle::CONTENTS_USABLE != 0
        {
            let host = &mut self.host;
            self.vm.with_cx(|cx| {
                // `ps.origin`, which the cmd's touch pass left in the
                // entity's `origin`.
                let from = crate::game::item::origin_of(host, cx, client);
                crate::game::vehicle::seat::unlink_player(
                    host,
                    cx,
                    slot,
                    true,
                    from,
                    crate::game::vehicle::seat::PLAYER_BOX,
                )
            });
            return;
        }
        // `Cmd_Activate_f`'s busy byte (0x4848e), which has no `pm_type`
        // gate: a gunner's press asks for the release and does nothing else
        // (turrets doc 4.1).
        let mut release_asked = false;
        if pressed
            && let Some(rec) = self
                .host
                .turrets
                .values_mut()
                .find(|r| r.owner == Some(slot))
        {
            rec.busy = 2;
            release_asked = true;
        }
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if !pressed || release_asked {
                return;
            }
            let v = host.client_vitals[slot];
            if v.health <= 0 || v.dead {
                return;
            }
            match crate::game::item::activate_ent(host, cx, slot, eye, view) {
                Some(Activate::Item(id)) => {
                    host.engine_notifies
                        .push((id, "touch", vec![Value::Entity(client)]));
                    crate::game::item::touch(host, cx, id, slot, false);
                }
                Some(Activate::Turret(turret)) => {
                    host.turret_ops.push(TurretOp::Mount { slot, turret });
                }
                // `Cmd_Activate_f` notifies `trigger` on the entity the hint
                // pass chose on the spot (0x48547), not through the trigger
                // queue, and with no `wait` gate
                // (docs/research/cod11-gametypes-re-bel.md 4).
                Some(Activate::Trigger(id)) => {
                    host.engine_notifies
                        .push((id, "trigger", vec![Value::Entity(client)]))
                }
                Some(Activate::Vehicle(veh)) => {
                    let on_ground = host.client_on_ground.get(slot).copied().unwrap_or(false);
                    crate::game::vehicle::seat::board(host, cx, veh, slot, on_ground, 0.0);
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
        use crate::game::turret::{AimRule, aim, fire_tick, loop_tick, muzzle};
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
            for te in self.release_turret(slot, sim).unwrap_or_default() {
                self.push_temp_entity(te);
            }
            self.teleport_link(slot, sim);
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
        let (loop_index, stop_index) = crate::game::turret::sound_indices(
            rec,
            &self.host.configstrings,
            self.host.allocators.layout(),
        );

        // 1.5's aim and gunner placement (0x62305) wait out a match timeout
        // with the fire.
        let rule = AimRule::of(self.host.defs.proto);
        let held = rule == AimRule::V15 && self.host.frozen;
        if !held {
            if rule.locks_view() {
                sim.viewlocked = 1;
                sim.gunfx = 0;
            }
            sim.viewlocked_ent = id.0;
            if let Some(view) = aim(rec, rule, sim.view_angles(), angles) {
                sim.set_view_angle(view);
            }
        }
        let placed = match (body, &rec.tags, self.host.fs.as_deref()) {
            (Some((anims, rigs)), Some(tags), Some(fs)) if !held => {
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
        // Held, the blend 0x61389 last set stands.
        if !held {
            sim.set_gunner_blend(placed.as_ref(), self.host.level_time_ms);
        }
        if let Some(mut at) = placed.as_ref().map(|g| g.origin) {
            if let Some(world) = &self.host.world {
                at = crate::game::turret::lift_onto_floor(&world.collision, at, origin[2]);
            }
            sim.ps.origin = at;
        }
        // 1.5's fire block (0x52305) runs only off a match timeout, its
        // cooldown left standing.
        sim.firing = !self.host.frozen && fire_tick(rec, attack_held);
        let mut shot = None;
        if sim.firing {
            if rule.locks_view() {
                sim.viewlocked = 2;
            }
            let forward = crate::game::spawn::angle_forward(rule.fire_angles(
                sim.view_angles(),
                rec.angles2,
                angles,
            ));
            // A missing tag skips the round and its event, not the flags.
            if let Some(tags) = &rec.tags {
                shot = Some(TurretShot {
                    slot,
                    gun: id.0,
                    muzzle: muzzle(tags, origin, angles, rec.angles2, forward).into(),
                    dir: forward.into(),
                    damage: rec.dmg,
                    rifle_bullet: rec.def.rifle_bullet,
                });
            }
        }
        let (loop_sound, stop) = loop_tick(rec, loop_index, self.host.frozen);
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
        if let Some(g) = &placed {
            self.set_client_origin(slot, sim.origin());
            self.set_client_yaw(slot, g.yaw());
        }
        shot
    }

    /// The UO half of a turret round `slot` fired from `muzzle`
    /// (`Bullet_Fire_Extended`, game.mp.uo 0x7a920): with no player hit the
    /// gun's fire event takes parm 0x80 (the client traces the round
    /// itself); each player hit may draw an `EV_BULLET_TRACER` from the
    /// muzzle to it, `g_tracerChanceLMG` for an `lmg`/`hmg`/`umg` gun, else
    /// `g_tracerChance`.
    pub fn uo_turret_round(&mut self, slot: usize, muzzle: [f32; 3], hits: &[[f32; 3]]) {
        let Some((&id, rec)) = self
            .host
            .turrets
            .iter()
            .find(|(_, r)| r.owner == Some(slot))
        else {
            return;
        };
        let weapon = self.host.defs.weapon_index(&rec.weapon).unwrap_or(0) as i32;
        if hits.is_empty() {
            if let Some(ent) = self.host.ents.get_mut(id) {
                let i = (ent.events.seq.wrapping_sub(1) & 3) as usize;
                ent.events.parms[i] |= TURRET_ROUND_UNTRACED;
            }
            return;
        }
        let heavy = self
            .host
            .weapons
            .get(weapon as usize)
            .is_some_and(|d| matches!(d.ammo_type.as_str(), "lmg" | "hmg" | "umg"));
        let chance = crate::game::builtins::cvar::atof(self.host.cvars.get(if heavy {
            "g_tracerChanceLMG"
        } else {
            "g_tracerChance"
        }));
        for &at in hits {
            // A pinned outcome still takes its draw, so later draws stay put.
            let drawn = self.host.rand_unit() < chance;
            if self.host.tracer_pins.pop_front().unwrap_or(drawn) {
                self.push_temp_entity(crate::game::temp_entity::TempEntity {
                    event: vcod_common::net::event_ids::EV_BULLET_TRACER,
                    parm: weapon,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: muzzle,
                    scope: crate::game::temp_entity::Scope::Pvs,
                    quake: None,
                    origin2: Some(at),
                });
            }
        }
    }

    /// `G_ClientStopUsingTurret` (0x53054, turrets doc 8) on the gun `slot`
    /// mans, if any: the record freed, the gun's loop cut and the gunner put
    /// back where it mounted, its new origin mirrored to script. Returns
    /// `TeleportPlayer`'s temp entities for the caller to queue, `None` when
    /// `slot` mans no gun. The area tree's link is the caller's
    /// ([`ScriptRuntime::teleport_link`]).
    pub fn release_turret(
        &mut self,
        slot: usize,
        sim: &mut crate::spectate::ClientSim,
    ) -> Option<Vec<crate::game::temp_entity::TempEntity>> {
        let (id, rec) = self
            .host
            .turrets
            .iter_mut()
            .find(|(_, r)| r.owner == Some(slot))?;
        let id = *id;
        let (_, origin, stance) = crate::game::turret::release(rec)?;
        if let Some(ent) = self.host.ents.get_mut(id) {
            ent.loop_sound = 0;
        }
        let temps = crate::game::turret::release_sim(sim, slot, origin, stance);
        self.set_client_origin(slot, sim.origin());
        Some(temps)
    }

    /// `TeleportPlayer`'s link (1.1d 0x513dc..0x51477, 1.5b 0x50dc8,
    /// UO 0x5a3f8): a client the tree held is unlinked and linked again at
    /// the released origin, unsnapped, so it heads its node's list there
    /// until its next cmd links it (combat doc 14.7). One it did not hold
    /// stays out. `ClientSpawn`'s release skips this: its own unlink and
    /// link at the spawn point follow, which the `spawn` builtin made.
    pub fn teleport_link(&mut self, slot: usize, sim: &crate::spectate::ClientSim) {
        if self.host.area.last_link(slot as u32).is_none() {
            return;
        }
        let contents = self.host.area.contents(slot as u32);
        self.host.area.unlink(slot as u32);
        let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
        let origin = sim.link_origin().to_array();
        let bounds = (sim.ps.mins().to_array(), sim.ps.maxs().to_array());
        self.host
            .link_client(slot, origin, bounds, contents, playing);
        self.set_client_link_origin(slot, origin);
    }

    /// The sim half of every release `GameHost::free_entity` queued for
    /// `slot` (`G_FreeTurret`, turrets doc 8), each temp entity queued. The
    /// link was the freeing thread's.
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
    pub fn kill_client(&mut self, slot: usize) -> bool {
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
        // `player_die`'s "death" ahead of the callback, as the builtin's.
        let death = self.vm.with_cx(|cx| cx.intern_folded("death"));
        self.vm.notify(Target::Entity(id), death, &args[1..2]);
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerKilled",
            Some(Target::Entity(id)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.player_die_walk(cx, slot));
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

    /// `G_ExplodeMissile`'s tail, after its blast walk
    /// (`GameHost::spawn_concussive`).
    pub fn spawn_concussive(&mut self) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.spawn_concussive(cx));
    }

    /// `G_MoverPush` over the players in `sims` and the items `step`
    /// listed (`crate::push::push`); a blocked push holds the mover a frame.
    /// A player a sine mover crushes takes `G_Damage(player, mover, mover,
    /// NULL, NULL, 99999, 0, MOD_CRUSH)` (game.mp 0x55617) before the
    /// mover's end-of-period test.
    pub fn push_mover(
        &mut self,
        step: &crate::game::mover::Step,
        sims: &mut [(usize, &mut crate::spectate::ClientSim)],
        world: &vcod_common::collision::CollisionWorld,
    ) {
        let host = &mut self.host;
        let crushed = self
            .vm
            .with_cx(|cx| crate::push::push(host, cx, step, sims, world));
        let Some(crushed) = crushed else {
            let host = &mut self.host;
            self.vm
                .with_cx(|cx| crate::game::mover::stall(host, cx, step));
            return;
        };
        for slot in crushed {
            self.deliver_world_hit(slot, step.ent, CRUSH_DAMAGE, 0, "MOD_CRUSH");
        }
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::mover::reached(host, cx, step.ent));
    }

    /// Where a link parent is this frame, origin and angles, `None` once it
    /// has been freed. `G_RunFrame` runs a linked entity's parent ahead of it
    /// (0x50939-0x50955), so a mover parent is read on its own clock, the
    /// level time, a frame ahead of the `.origin` script reads
    /// (docs/research/cod11-movers.md, section 13).
    pub fn link_anchor(&mut self, id: EntId) -> Option<([f32; 3], [f32; 3])> {
        use vcod_gsc::Host;
        let origin = self.entity_origin_of(id)?;
        let host = &mut self.host;
        let angles = self.vm.with_cx(|cx| {
            let field = cx.intern_folded("angles");
            match host.get_field(cx, id, field) {
                Value::Vector(v) => v,
                _ => [0.0; 3],
            }
        });
        let (pos, apos) = self
            .host
            .movers
            .pose_at(id, self.host.level_time_ms)
            .unwrap_or((None, None));
        Some((
            pos.map_or(origin, |v| v.to_array()),
            apos.map_or(angles, |v| v.to_array()),
        ))
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

    /// Every weapon item lying in the world to be picked up, as `(weapon
    /// index, origin, empty)`: placed `mpweapon_*` entities while shown and
    /// drops. Empty is no round in the clip or the reserve: a drop writes
    /// -1 into each count it found empty (`cod11-items.md` 8), and 0 is
    /// unset, which the pickup fills.
    /// A taken one waits on its respawn off every snapshot and is left out.
    pub fn weapon_items(&mut self) -> Vec<(u8, [f32; 3], bool)> {
        use vcod_gsc::Host;
        let defs = self.host.defs;
        let items: Vec<(EntId, u8, i32)> = self
            .host
            .ents
            .iter_inuse()
            .filter_map(|(id, e)| {
                let item = e.item?;
                let weapon = !item.taken && defs.weapon_name(item.index as usize).is_some();
                weapon.then_some((id, item.index, item.clip))
            })
            .collect();
        let host = &mut self.host;
        let counts: Vec<i32> = self.vm.with_cx(|cx| {
            let count = cx.intern_folded("count");
            items
                .iter()
                .map(|&(id, ..)| match host.get_field(cx, id, count) {
                    Value::Int(n) => n,
                    _ => 0,
                })
                .collect()
        });
        items
            .into_iter()
            .zip(counts)
            .filter_map(|((id, w, clip), count)| {
                Some((w, self.entity_origin_of(id)?, clip < 0 && count < 0))
            })
            .collect()
    }

    /// Every health item lying in the world to be picked up (`item_health*`,
    /// a death's `dropHealth`), not taken: where a hurt bot heals
    /// (`bots/retreat.rs`).
    pub fn health_items(&mut self) -> Vec<[f32; 3]> {
        let defs = self.host.defs;
        let health: Vec<usize> = defs
            .static_items()
            .filter(|(_, n, _)| n.starts_with("item_health"))
            .map(|(i, ..)| i)
            .collect();
        let ids: Vec<EntId> = self
            .host
            .ents
            .iter_inuse()
            .filter_map(|(id, e)| {
                let item = e.item?;
                (!item.taken && health.contains(&(item.index as usize))).then_some(id)
            })
            .collect();
        ids.into_iter()
            .filter_map(|id| self.entity_origin_of(id))
            .collect()
    }

    /// One client's health as the script left it, read every frame the way
    /// `client_weapons` is.
    /// `ClientEndFrame`'s `takedamage` write.
    pub fn set_client_takedamage(&mut self, slot: usize, on: bool) {
        if let Some(v) = self.host.client_vitals.get_mut(slot) {
            v.takedamage = on;
        }
    }

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

    /// A client's last usercmd, for the `spawn` builtin's think.
    pub fn set_client_last_cmd(&mut self, slot: usize, cmd: vcod_common::net::msg::UserCmd) {
        if let Some(c) = self.host.client_last_cmd.get_mut(slot) {
            *c = cmd;
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

    /// A client's stance as the tick's moves left it, for `getStance`.
    pub fn set_client_stance(&mut self, slot: usize, stance: vcod_common::pmove::Stance) {
        if let Some(s) = self.host.client_stance.get_mut(slot) {
            *s = stance;
        }
    }

    /// A client's prone cone as the tick's moves left it, for
    /// `setPlayerAngles`.
    pub fn set_client_prone_cone(
        &mut self,
        slot: usize,
        cone: Option<vcod_common::pmove::ProneCone>,
    ) {
        if let Some(c) = self.host.client_prone_cone.get_mut(slot) {
            *c = cone;
        }
    }

    /// A client's eye (lean included) and `[pitch, yaw]` aim as the tick left
    /// them, for `aim_lookat`, and whether the held weapon is a `rifleBullet`
    /// one, which picks the priority map the trace ranks bones with.
    pub fn set_client_aim(&mut self, slot: usize, eye: [f32; 3], aim: [f32; 2], rifle: bool) {
        if let Some(a) = self.host.client_aim.get_mut(slot) {
            *a = (eye, aim, rifle);
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
        let (eye, aim, rifle) = self.host.client_aim[slot];
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::trigger::aim_trace(host, cx, slot, eye, aim, rifle, now_ms));
        self.host.client_lookat[slot] = hit;
        if let Some(id) = hit
            && self.host.triggers.fire(id, now_ms, false).fired()
            && !self.host.queue_trigger(id, client)
        {
            self.host
                .engine_notifies
                .push((id, "trigger", vec![Value::Entity(client)]));
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
        let h = self.cursor_hints(slot, eye, view, None);
        (h.hint, h.string)
    }

    /// [`crate::game::trigger::mount_boxes`]: what the server's point
    /// contents add to pmove's and the hint's.
    pub fn mount_boxes(&mut self) -> Vec<vcod_common::movetrace::ContentsBox> {
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::trigger::mount_boxes(host, cx))
    }

    /// [`Self::cursor_hint_pass`] with United Offensive's LMG arm
    /// (game.mp.uo 0x67aa0): a sighted LMG, the bipod down, skips the pass
    /// and reads 0; one held otherwise reads hint 7 (`HINT_LMG`) with its
    /// [`crate::game::trigger::lmg_mount`] value and the weapon's
    /// `useHintString` where a mount is ahead, unless an activate entity's
    /// hint replaces the 7, which keeps the value
    /// (docs/research/coduo-movement.md 11).
    pub fn cursor_hints(
        &mut self,
        slot: usize,
        eye: [f32; 3],
        view: [f32; 3],
        lmg: Option<LmgHint<'_>>,
    ) -> CursorHint {
        let hint = |hint, string| CursorHint {
            hint,
            val: 0,
            string,
        };
        let v = self
            .host
            .client_vitals
            .get(slot)
            .copied()
            .unwrap_or_default();
        if self.client_entity(slot).is_none() || v.health <= 0 || v.dead {
            return hint(0, None);
        }
        if self.host.turrets.values().any(|r| r.owner == Some(slot)) {
            return hint(0, None);
        }
        // `G_CheckForCursorHints` skips everything for a rider (0x575bb).
        if self.host.vehicles.seat_of(slot).is_some() {
            return hint(0, Some(-1));
        }
        if self.host.client_pm_type.get(slot).copied().unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return hint(0, Some(-1));
        }
        let mut miss = hint(0, Some(-1));
        if let Some(l) = lmg {
            if l.sighted {
                return miss;
            }
            let (sin, cos) = l.yaw_deg.to_radians().sin_cos();
            let point = glam::Vec3::from(l.origin)
                + glam::Vec3::new(cos, sin, 0.0) * LMG_PROBE_AHEAD
                + glam::Vec3::Z;
            if let Some(val) = crate::game::trigger::lmg_mount(&self.host, l.boxes, point) {
                let string = l
                    .hint_string
                    .and_then(|name| {
                        crate::configstrings::hint_string_index(
                            &self.host.configstrings,
                            self.host.allocators.layout(),
                            name,
                        )
                    })
                    .unwrap_or(-1);
                miss = CursorHint {
                    hint: HINT_LMG,
                    val,
                    string: Some(string),
                };
            }
        }
        let val = miss.val;
        let found = |h: i32, string: i32| CursorHint {
            hint: h,
            val,
            string: Some(string),
        };
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::item::activate_ent(host, cx, slot, eye, view));
        let id = match hit {
            None => return miss,
            Some(Activate::Turret(id)) => {
                let string = self.host.turrets[&id]
                    .def
                    .use_hint_string
                    .as_deref()
                    .and_then(|name| {
                        crate::configstrings::hint_string_index(
                            &self.host.configstrings,
                            self.host.allocators.layout(),
                            name,
                        )
                    });
                return found(HINT_MG42, string.unwrap_or(-1));
            }
            Some(Activate::Trigger(id)) => {
                let Some(t) = self.host.triggers.get(id) else {
                    return miss;
                };
                return found(t.cursor_hint, t.hint_string);
            }
            Some(Activate::Item(id)) => id,
            // `hintActivate` with the vehicle file's `useHintString`.
            Some(Activate::Vehicle(id)) => {
                let string = self
                    .host
                    .vehicles
                    .live
                    .get(&id)
                    .map_or(-1, |v| v.hint_string);
                return found(HINT_ACTIVATE, string);
            }
        };
        let Some(item) = self.host.ents.get(id).and_then(|e| e.item) else {
            return miss;
        };
        let Some(kind) = crate::game::pickup::item_kind(self.host.defs, item.index as usize) else {
            return miss;
        };
        let owned = self.host.client_weapons[slot].holds(item.index as usize);
        CursorHint {
            hint: crate::game::pickup::cursor_hint(self.host.defs.proto, kind, owned),
            ..miss
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

    /// A client's body as the tick's moves left it, for a scripted blast.
    pub fn set_client_body(&mut self, slot: usize, body: Option<crate::game::combat::HitBody>) {
        if let Some(b) = self.host.client_bodies.get_mut(slot) {
            *b = body;
        }
    }

    /// A client's posed model, live or dead, for `dropItem`'s tag.
    pub fn set_client_dobj(&mut self, slot: usize, body: Option<crate::game::combat::HitBody>) {
        if let Some(b) = self.host.client_dobjs.get_mut(slot) {
            *b = body;
        }
    }

    /// The controls the slot's last skeleton build eased to
    /// (`GameHost::dobj_posed`).
    pub fn dobj_record(&self, slot: usize) -> Option<vcod_common::playerpose::Controllers> {
        self.host.dobj_posed.get(slot)?.record()
    }

    /// Starts a new skeleton stamp, as each client packet does.
    pub fn clear_dobj_stamps(&mut self) {
        self.host.clear_dobj_stamps();
    }

    /// The animtree a builtin poses a body with.
    pub fn set_player_anims(&mut self, anims: Option<Rc<vcod_common::animtree::PlayerAnims>>) {
        self.host.anims = anims;
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

    /// A client's origin at its last link, for the `radiusDamage` walk.
    pub fn set_client_link_origin(&mut self, slot: usize, origin: [f32; 3]) {
        if let Some(o) = self.host.client_link_origin.get_mut(slot) {
            *o = origin;
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
    pub fn menu_response(&mut self, slot: usize, menu: &MenuRef, response: &str) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let menu = match menu {
            MenuRef::Index(i) => crate::configstrings::script_menu_name(
                &self.host.configstrings,
                self.host.allocators.layout(),
                *i,
            )
            .to_string(),
            MenuRef::Name(name) => name.clone(),
        };
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

    /// An engine notify on client `slot`'s entity with string arguments,
    /// for the ones a game module raises outside a builtin (CoD2's
    /// binocular and vote notifies, `crate::game::cod2`). Its waiters run in
    /// the next thread pass.
    pub fn notify_client(&mut self, slot: usize, event: &str, args: &[&str]) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let (event, args) = self.vm.with_cx(|cx| {
            let args: Vec<Value> = args
                .iter()
                .map(|a| Value::String(cx.intern_exact(a)))
                .collect();
            (cx.intern_folded(event), args)
        });
        self.vm.notify(id, event, &args);
    }

    /// [`Self::notify_client`] with one value argument (CoD4's
    /// `confirm_location` vector).
    pub fn notify_client_value(&mut self, slot: usize, event: &str, arg: Option<Value>) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let event = self.vm.with_cx(|cx| cx.intern_folded(event));
        let args: Vec<Value> = arg.into_iter().collect();
        self.vm.notify(id, event, &args);
    }

    /// The per-client server commands the script queued this frame, in call
    /// order. `Server` sends them; nothing here can reach a netchan.
    pub fn take_client_commands(&mut self) -> Vec<(usize, String)> {
        self.host.client_commands.take()
    }

    /// [`Self::take_client_commands`] with each command's write stamp, the
    /// order CoD2's engine sends them in among the configstrings.
    pub fn take_client_commands_stamped(&mut self) -> Vec<(u64, usize, String)> {
        self.host.client_commands.take_stamped()
    }

    /// `GameHost::scoreboard` over the roster last mirrored in.
    pub fn scoreboard(&mut self) -> String {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.scoreboard(cx))
    }

    /// `G_Say` for a client's own `say`, `say_team` or `tell`; the lines it
    /// queued, in order.
    pub fn say(
        &mut self,
        slot: usize,
        target: Option<usize>,
        mode: crate::game::say::SayMode,
        text: &str,
    ) -> Vec<(usize, String)> {
        let from = self.host.client_commands.len();
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.say(cx, slot, target, mode, text));
        self.host.client_commands.split_off(from)
    }

    /// 1.5's `matchtimeout` (`timein` false) or `matchtimein` from `slot`:
    /// the server commands it sends, `None` addressing everyone
    /// ([`GameHost::match_timeout`]).
    pub fn match_timeout(
        &mut self,
        slot: usize,
        timein: bool,
        now_ms: i32,
    ) -> Vec<(Option<usize>, String)> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if timein {
                host.match_timein(cx, slot, now_ms)
            } else {
                host.match_timeout(cx, slot, now_ms)
            }
        })
    }

    /// `Cmd_CallVote_f` (`callvote`) or `Cmd_Vote_f` (`vote`) for `slot`;
    /// `argv` is the whole command.
    pub fn vote_command(
        &mut self,
        slot: usize,
        argv: &[String],
        svs_ms: i32,
    ) -> crate::game::vote::Sends {
        if let Some(sends) = self.cod2_script_vote(slot, argv) {
            return sends;
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if argv[0].eq_ignore_ascii_case("vote") {
                host.cast_vote(cx, slot, argv.get(1).map_or("", String::as_str))
            } else {
                host.call_vote(cx, slot, argv, svs_ms)
            }
        })
    }

    /// CoD2's votes under `g_oldVoting` 0 (default 1): `Cmd_CallVote_f`
    /// (cod2_lnxded 0x80ffbb2) refuses under `g_allowVote` 0 or on a `;` in
    /// its three arguments and otherwise notifies `call_vote` with them;
    /// `Cmd_Vote_f` (0x8100960) notifies `vote` with "yes" when the first
    /// letter is `y` or the second `Y` or `1` (Q3's test, as read), else
    /// "no". VERIFIED, both functions and the dvar defaults. `None` where
    /// the engine's own vote runs.
    fn cod2_script_vote(
        &mut self,
        slot: usize,
        argv: &[String],
    ) -> Option<crate::game::vote::Sends> {
        use crate::game::builtins::cvar::atoi;
        let flag = |v: &str| v.is_empty() || atoi(v) != 0;
        if !self.host.profile.votes.script_votes || flag(self.host.cvars.get("g_oldVoting")) {
            return None;
        }
        let arg = |i: usize| argv.get(i).map_or("", String::as_str);
        if argv[0].eq_ignore_ascii_case("vote") {
            let b = arg(1).as_bytes();
            let yes = b.first() == Some(&b'y') || matches!(b.get(1), Some(b'Y' | b'1'));
            self.notify_client(slot, "vote", &[if yes { "yes" } else { "no" }]);
            return Some(Vec::new());
        }
        if !flag(self.host.cvars.get("g_allowVote")) {
            return Some(vec![(Some(slot), "e \"GAME_VOTINGNOTENABLED\"".into())]);
        }
        // CoD4's two-client floor comes ahead of the script's call too
        // (0x80adc1d).
        if self.host.client_roster.iter().flatten().count() < self.host.profile.votes.min_connected
        {
            return Some(vec![(
                Some(slot),
                "e \"GAME_VOTINGNOTENOUGHPLAYERS\"".into(),
            )]);
        }
        let (a, b, c) = (arg(1), arg(2), arg(3));
        if [a, b, c].iter().any(|s| s.contains(';')) {
            return Some(vec![(Some(slot), "e \"GAME_INVALIDVOTESTRING\"".into())]);
        }
        self.notify_client(slot, "call_vote", &[a, b, c]);
        Some(Vec::new())
    }

    /// `CheckVote`: the lines for everyone and whether configstring 15
    /// changed.
    pub fn check_vote(&mut self, svs_ms: i32) -> (Vec<String>, bool) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.check_vote(cx, svs_ms))
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

    /// Writes back the slots a select-fire switch moved (the putaway's alt
    /// arm, `docs/research/cod11-combat.md` 1.8), which the mirror would
    /// otherwise put back the next frame.
    pub fn set_client_slots(&mut self, slot: usize, slots: [u8; crate::weapons::NUM_SLOTS]) {
        if let Some(w) = self.host.client_weapons.get_mut(slot) {
            w.slots = slots;
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

    /// Whether [`Self::take_ops_of`] would hand anything back for `slot`.
    pub fn has_ops_of(&self, slot: usize) -> bool {
        self.host.client_weapon_ops.iter().any(|(s, _)| *s == slot)
            || self.host.client_sim_ops.iter().any(|(s, _)| *s == slot)
    }

    /// `slot`'s share of [`Self::take_weapon_ops`] and
    /// [`Self::take_sim_ops`], for what the move pass lands before the
    /// client's next cmd; every other client's stay queued.
    pub fn take_ops_of(
        &mut self,
        slot: usize,
    ) -> (
        Vec<crate::game::host::WeaponOp>,
        Vec<crate::game::host::SimOp>,
    ) {
        fn split<T>(all: &mut Vec<(usize, T)>, slot: usize) -> Vec<T> {
            let (mine, rest) = std::mem::take(all)
                .into_iter()
                .partition::<Vec<_>, _>(|(s, _)| *s == slot);
            *all = rest;
            mine.into_iter().map(|(_, op)| op).collect()
        }
        (
            split(&mut self.host.client_weapon_ops, slot),
            split(&mut self.host.client_sim_ops, slot),
        )
    }

    /// One client's viewmodel index, read every frame for the same reason
    /// `client_weapons` is.
    pub fn client_viewmodel(&self, slot: usize) -> i32 {
        self.host.client_viewmodel.get(slot).copied().unwrap_or(0)
    }

    /// The slot's shellshock fields: index, start ms, duration ms.
    pub fn client_shellshock(&self, slot: usize) -> [i32; 3] {
        self.host
            .client_shellshock
            .get(slot)
            .copied()
            .unwrap_or([0; 3])
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

    /// A client's `(iHeadIcon, iHeadIconTeam)` (`GameHost::head_icon`).
    pub fn head_icon(&self, slot: usize) -> (i32, i32) {
        self.host.head_icon(slot)
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
        self.host.vehicles.lifted.remove(&slot);
        self.host.vehicles.stale_seated.remove(&slot);
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            let _ = host.set_field(cx, ent, field, Value::Vector(origin));
        });
    }

    /// Writes a client's `angles` field (`r.currentAngles`), no link.
    pub fn set_client_angles(&mut self, slot: usize, angles: [f32; 3]) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("angles");
            let _ = host.set_field(cx, ent, field, Value::Vector(angles));
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

    /// Raises entity `num`'s `origin` by `dz` through the field write, and
    /// returns where it then stands.
    pub fn raise_entity(&mut self, num: u32, dz: f32) -> Option<[f32; 3]> {
        use vcod_gsc::Host;
        let id = self.host.ents.handle(num)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            let Value::Vector(mut o) = host.get_field(cx, id, field) else {
                return None;
            };
            o[2] += dz;
            if dz != 0.0 {
                let _ = host.set_field(cx, id, field, Value::Vector(o));
            }
            Some(o)
        })
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
        if let Some(s) = host.dobj_posed.get_mut(slot) {
            s.set_angles(None);
        }
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
        // A client that has never had `setModel` reads model configstring
        // 0, "", which must not match the first unused model slot: retail
        // sends 0 for a spectator who never spawned.
        if name.is_empty() {
            return 0;
        }
        let (first, last) = self
            .host
            .allocators
            .layout()
            .bounds(crate::configstrings::CsRange::Model);
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

    /// What the follow pass reads off a client's script fields, `None` for a
    /// slot with no client entity. A `sessionstate` outside the four legal
    /// strings reads as playing, the arm `ClientEndFrame` falls through to.
    pub fn client_session(&mut self, slot: usize) -> Option<crate::follow::Session> {
        use crate::follow::{Session, SessionState};
        let state = match self.client_field(slot, "sessionstate")?.as_str() {
            "spectator" => SessionState::Spectator,
            "dead" => SessionState::Dead,
            "intermission" => SessionState::Intermission,
            _ => SessionState::Playing,
        };
        let spectator_client = self
            .client_field(slot, "spectatorclient")
            .and_then(|v| v.parse().ok())
            .unwrap_or(-1);
        let archive_ms = self.client_archive_ms(slot);
        let no_spectate = self.host.client_no_spectate.get(slot).copied().unwrap_or(0);
        Some(Session {
            state,
            spectator_client,
            archive_ms,
            no_spectate,
        })
    }

    /// A client's `archivetime` in the milliseconds the engine reads, 0 for
    /// a slot with no client entity.
    pub fn client_archive_ms(&mut self, slot: usize) -> i32 {
        let i = crate::game::fields::archive_time_index();
        self.client_entity(slot)
            .and_then(|ent| self.host.ents.get(ent))
            .and_then(|e| e.client.as_ref())
            .map_or(0, |c| crate::game::host::archive_time_ms(c[i]))
    }

    /// The engine's own write to `archivetime`: the archive's trim and
    /// `SpectatorClientEndFrame`'s retry store milliseconds straight into the
    /// field (`crate::archive`).
    pub fn set_client_archive_ms(&mut self, slot: usize, ms: i32) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let i = crate::game::fields::archive_time_index();
        if let Some(c) = self.host.ents.get_mut(ent).and_then(|e| e.client.as_mut()) {
            c[i] = Value::Int(ms);
        }
    }

    /// `ClientEndFrame`'s own `ClientSpawn` (0x40f82): the memset takes the
    /// weapons and the ammo with the rest of the playerstate, and the
    /// entity is at the new spot. Unlike the `spawn` builtin it leaves the
    /// health, which the dead end of a killcam reads 0 on.
    pub fn engine_client_spawn(&mut self, slot: usize, origin: [f32; 3], yaw_deg: f32) {
        if let Some(w) = self.host.client_weapons.get_mut(slot) {
            *w = crate::weapons::PlayerWeapons::default();
        }
        if let Some(a) = self.host.client_ammo.get_mut(slot) {
            *a = crate::game::host::AmmoArrays::default();
        }
        if let Some(s) = self.host.client_shellshock.get_mut(slot) {
            *s = [0; 3];
        }
        self.set_client_origin(slot, origin);
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("angles");
            let _ = host.set_field(cx, ent, field, Value::Vector([0.0, yaw_deg, 0.0]));
        });
    }

    /// Whether script has turned the frame archive on (`setarchive`).
    pub fn archive_on(&self) -> bool {
        self.host.archive_on
    }

    /// The engine's own write to `spectatorclient`: `SpectatorClientEndFrame`
    /// and `StopFollowing` put -1 there without going through script.
    pub fn set_client_spectator_client(&mut self, slot: usize, v: i32) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let i = crate::game::fields::spectator_client_index();
        if let Some(c) = self.host.ents.get_mut(ent).and_then(|e| e.client.as_mut()) {
            c[i] = Value::Int(v);
        }
    }

    /// A client's attachments as the roster carries them: up to six
    /// `(attachModelIndex, attachTagIndex)` pairs, the model resolving through
    /// `configstring 268 + index` and the tag through `108 + index`
    /// (`docs/research/clientstate-wire-format.md`). A head and a helmet are
    /// attachments, not part of the body: the stock character script does
    /// `attachFromArray(xmodelalias\head_allied::main())` and
    /// `self attach(self.hatModel)`, so a client sent none is headless.
    /// The tag takes its configstring here, not at `attach`: `ClientEndFrame`'s
    /// own-body arm calls `G_TagIndex` on each (1.1 0x413f6, UO
    /// `G_UpdateClientInfo` 0x52c81), so only a playing or dead client
    /// allocates one (docs/research/cod11-gsc-object-model.md, "attach and
    /// detach").
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
        // 1.7's `ClientEndFrame` (0x80a68b8) runs its inlined
        // `G_UpdateClientInfo` ahead of the session checks, as CoD2's
        // (0x80f7516) calls 0x80f72f8, so every connected client's tags
        // take their index (cod4-player-anims.md 5, cod2-player-anims.md 1).
        let own_body = matches!(
            self.host.profile.game,
            vcod_common::game::Game::Cod4 | vcod_common::game::Game::Cod2
        ) || self.client_session(slot).is_some_and(|s| {
            matches!(
                s.state,
                crate::follow::SessionState::Playing | crate::follow::SessionState::Dead
            )
        });
        let layout = self.host.allocators.layout();
        let mut out = Vec::with_capacity(pairs.len());
        for (m, t) in pairs.iter().take(ATTACH_SLOTS) {
            let tag = if t.is_empty() {
                None
            } else if own_body {
                self.host
                    .allocators
                    .index(
                        &mut self.host.configstrings,
                        crate::configstrings::CsRange::Tag,
                        t,
                    )
                    .ok()
            } else {
                self.host.allocators.find_folded(
                    &self.host.configstrings,
                    crate::configstrings::CsRange::Tag,
                    t,
                )
            };
            let model = if m.is_empty() {
                None
            } else {
                self.host.allocators.find_folded(
                    &self.host.configstrings,
                    crate::configstrings::CsRange::Model,
                    m,
                )
            };
            let rel =
                |slot: Option<usize>, range| slot.map_or(0, |s| (s - layout.base(range)) as i32);
            out.push((
                rel(model, crate::configstrings::CsRange::Model),
                rel(tag, crate::configstrings::CsRange::Tag),
            ));
        }
        out
    }

    /// The models a client wears, by name rather than by configstring index:
    /// what the locational trace poses. The body is `.model` and the head and
    /// helmet are attachments, the same pair `client_model_index` and
    /// `client_attachments` put on the roster.
    pub fn client_assembly(&mut self, slot: usize) -> Option<crate::game::hitrig::Assembly> {
        use crate::game::hitrig::{Assembly, model_name};
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
        Some(Assembly {
            body,
            attachments,
            ignore_collision: e.attach_ignore_collision,
        })
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

    /// The frame a frame-counted `wait` counts (`Vm::set_frame_ms`).
    pub fn set_frame_ms(&mut self, ms: i32) {
        self.vm.set_frame_ms(ms);
    }

    /// The network clock (`svs.time`) `addTestClient`'s reconnect check
    /// reads; `Server` mirrors it in before the threads run.
    pub fn set_network_time(&mut self, svs_ms: i32) {
        self.host.test_clients.svs_time_ms = svs_ms;
    }

    /// The entity `Scr_AddEntity` pushes for a damage's attacker: the
    /// slot's client, a UO vehicle by its number (a shell's fuse blast), or
    /// entity 0 for a [`crate::game::host::ZEROED_ENTITY`] attacker (combat
    /// doc 13.7).
    pub fn attacker_entity(&self, slot: usize) -> Option<EntId> {
        if slot == crate::game::host::ZEROED_ENTITY {
            return self.host.ents.handle(0);
        }
        self.attacker_vehicle(slot)
            .or_else(|| self.client_entity(slot))
    }

    fn attacker_vehicle(&self, n: usize) -> Option<EntId> {
        let id = self.host.ents.handle(u32::try_from(n).ok()?)?;
        self.host.vehicles.live.contains_key(&id).then_some(id)
    }

    /// A blast's owner as `G_Damage` is handed it: a slot with no client
    /// left (a UO vehicle shell keeps its driver's `+0x330` after he left)
    /// is the freed entity there, whose number reads 0
    /// ([`crate::game::host::ZEROED_ENTITY`], combat doc 13.7). A live
    /// vehicle's number, a shell's fuse blast, stays.
    pub fn charged_owner(&self, owner: Option<usize>) -> Option<usize> {
        owner.map(|slot| match self.attacker_entity(slot) {
            Some(_) => slot,
            None => crate::game::host::ZEROED_ENTITY,
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

    /// `SV_DropClient`'s `ClientDisconnect`, run inside the drop as retail
    /// runs it, not queued for the next frame's packet pass. The events the
    /// netcode queued ahead of it go first, in order, so a slot's `Connect`
    /// never lands after its own drop. The threads its notifies woke wait
    /// for the next frame's first pass.
    pub fn client_disconnect(&mut self, slot: usize) {
        for ev in std::mem::take(&mut self.host.client_events) {
            self.dispatch_client_event(ev);
        }
        self.dispatch_client_event(ClientEvent::Disconnect(slot));
    }

    /// `SV_SpawnServer`'s client pass (docs/research/cod11-map-cycle.md,
    /// section 3 step 22): the incoming level's `ClientConnect` for a client
    /// that is already on the netchan. The object table is the new level's,
    /// so this is a first connect as far as script is concerned. It runs
    /// rather than queues, because retail runs it inside the spawn, after the
    /// settle frames of step 20.
    pub fn reconnect_client(&mut self, slot: usize, name: String) {
        self.dispatch_client_event(ClientEvent::Connect { slot, name });
    }

    /// The animtrees the level's scripts name in `#using_animtree`.
    pub fn animtrees(&self) -> &[String] {
        self.scripts.as_ref().map_or(&[], |c| c.animtrees())
    }

    #[cfg(test)]
    pub(crate) fn compiled(&self) -> Option<&Rc<CompiledScripts>> {
        self.scripts.as_ref()
    }

    /// Drops the compiled closure a restart would reuse.
    pub(crate) fn forget_compiled(&mut self) {
        self.scripts = None;
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
            area: Some(self.host.area.clone()),
            map: self.map.clone(),
            scripts: self.scripts.clone(),
            gametype: self.scripts.as_ref().map(|s| s.gametype.clone()),
            seed: None,
            spawn_pins: None,
            models: Some(crate::game::host::ModelCaches {
                collision: self.host.xmodel_collision.clone(),
                bounds: self.host.xmodel_bounds.clone(),
                rigs: self.host.vehicles.rigs.clone(),
            }),
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
    fn dispatch_client_event(&mut self, ev: ClientEvent) {
        match ev {
            ClientEvent::Connect { slot, name } => {
                let host = &mut self.host;
                let Some(id) = self.vm.with_cx(|cx| host.connect_client(cx, slot, &name)) else {
                    return;
                };
                self.start_callback("CodeCallback_PlayerConnect", id);
                // `ClientConnect` closes with `CalculateRanks` (1.1d 0x42677).
                self.host.ranks_dirty = true;
            }
            ClientEvent::Begin(slot) => {
                let Some(id) = self.client_entity(slot) else {
                    log::warn!("client {slot}: begin with no entity, connect never ran");
                    return;
                };
                // `ClientBegin` (1.1d 0x42f88): `CalculateRanks`, then the
                // notify, whose waiters run in the next frame's thread pass
                // on its clock (cod11-server-handshake.md, "When the game
                // hears a drop").
                self.host.ranks_dirty = true;
                let event = self.vm.with_cx(|cx| cx.intern_folded(BEGIN_NOTIFY));
                self.vm.notify(id, event, &[]);
            }
            ClientEvent::Disconnect(slot) => {
                // `ClientDisconnect` opens with `menuresponse` ("-1",
                // "disconnect") on the player (1.1d 0x42ad9, 1.5b 0x4cf5f,
                // UO 0x55433). Its waiters run on the next frame's clock;
                // the player's own die with it below
                // (cod11-server-handshake.md, "When the game hears a drop").
                if let Some(id) = self.client_entity(slot) {
                    let (event, menu, response) = self.vm.with_cx(|cx| {
                        (
                            cx.intern_folded(MENURESPONSE_NOTIFY),
                            cx.intern_exact("-1"),
                            cx.intern_exact("disconnect"),
                        )
                    });
                    self.vm
                        .notify(id, event, &[Value::String(menu), Value::String(response)]);
                }
                self.host.complaints_about_gone(slot);
                // `ClientDisconnect` lets go of a vehicle seat first (0x45433).
                self.with_host(|host, cx| {
                    crate::game::vehicle::seat::unlink_player(
                        host,
                        cx,
                        slot,
                        false,
                        [0.0; 3],
                        ([0.0; 3], [0.0; 3]),
                    )
                });
                self.host.vehicles.seat_ops.clear();
                self.host.vehicles.riders.remove(&slot);
                // Whatever the last level boundary lifted for this slot dies
                // with the client that owned it: the next occupant of the
                // slot is a stranger and must not connect into its `pers[]`.
                if let Some(c) = self.host.pers_carry.get_mut(slot) {
                    *c = None;
                }
                // The client's own HUD elements go before the callback, as
                // in retail's `ClientDisconnect` (cod11-hud-protocol.md, "A
                // client's elements die with it"): the thread that would
                // `destroy` them is killed below, and the next client into
                // the slot has the owner's number.
                self.host.ents.free_client_hud_elems(slot);
                // The callback runs first: it reads `self`, and freeing the
                // slot ahead of it would hand it a dead entity.
                if let Some(id) = self.client_entity(slot) {
                    self.start_callback("CodeCallback_PlayerDisconnect", id);
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
                // `G_FreeEntity`'s unlink, owner and ground sweeps
                // (`ClientDisconnect` 0x42bfc).
                self.host.area.unlink(slot as u32);
                self.host.missiles.free_owner_refs(slot);
                for b in &mut self.host.barrages {
                    if b.owner == Some(slot) {
                        b.owner = None;
                    }
                }
                self.host.free_ground_refs(slot as u32);
                self.host.ents.free_client(slot);
                // `CalculateRanks` closes it on all three (1.1d 0x42c1f).
                self.host.ranks_dirty = true;
            }
        }
    }

    /// One `_callbacksetup` entry point on a client's entity.
    fn start_callback(&mut self, name: &str, id: EntId) {
        if let Err(e) = self.start(CALLBACK_SETUP, name, Some(Target::Entity(id))) {
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

    /// The entities `playSound` made `SVF_BROADCAST`, which every snapshot
    /// carries past the cull, and on 1.5 and UO those whose
    /// `r.broadcastTime` the network clock `svs_time` has not passed.
    pub fn broadcast_entities(&self, svs_time: i32) -> impl Iterator<Item = u32> + '_ {
        self.host
            .ents
            .iter_inuse()
            .filter(move |(_, e)| {
                let t = e.broadcast_time;
                e.broadcast || t < 0 || (t != 0 && t.wrapping_sub(svs_time) >= 0)
            })
            .map(|(id, _)| id.0)
    }

    /// The snapshot builder's clear of a passed `r.broadcastTime`
    /// (coduo_lnxded 0x8096d53, 1.5b 0x8095dc6), which is what
    /// `SV_inSnapshot` reads until then ([`Self::in_snapshot_broadcast`]).
    pub fn expire_broadcast_times(&mut self, svs_time: i32) {
        for e in self.host.ents.iter_inuse_mut() {
            if e.broadcast_time > 0 && e.broadcast_time.wrapping_sub(svs_time) < 0 {
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
        p: &vcod_common::net::protocol::Protocol,
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

    /// `GameHost::damage_entity` for a grenade's blast, `attacker` the
    /// thrower's slot; a thrower with no entity left reads as the world,
    /// `G_Damage`'s stand-in (combat doc, 4.2). The waiters run with the
    /// frame's threads.
    pub fn damage_entity(
        &mut self,
        victim: &crate::game::combat::EntityVictim,
        blast: &crate::game::combat::Blast,
        damage: i32,
        attacker: usize,
    ) {
        let id = victim.id;
        let attacker = self.attacker_entity(attacker);
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            let attacker = attacker.unwrap_or_else(|| host.ents.world(cx));
            let hit = victim.hit(blast, blast.inflictor.unwrap_or(attacker));
            host.damage_nonclient(cx, id, damage, attacker, &hit)
        });
        // Raised inside the pass, run by the next frame's threads with the
        // trigger queue's round (combat doc 14.8).
        for (event, args) in notifies {
            self.host.engine_notifies.push((id, event, args));
        }
    }

    /// A blast's `G_Damage` on a UO vehicle (`crate::game::vehicle`):
    /// `attacker` the owner's slot, `inflictor` the missile or the vehicle
    /// that fired it, `at` the blast's origin.
    pub fn damage_vehicle(
        &mut self,
        id: EntId,
        damage: i32,
        attacker: usize,
        inflictor: EntId,
        at: [f32; 3],
        mod_: &str,
    ) {
        let attacker = self.attacker_entity(attacker);
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            // `G_RadiusDamage` hands over `r.currentOrigin` less the blast,
            // lifted 24, and `G_Damage` normalizes it (game.mp.uo 0x5098b,
            // 0x4fedb).
            let origin = crate::game::item::origin_of(host, cx, id);
            let dir = (glam::Vec3::from(origin) - glam::Vec3::from(at) + glam::Vec3::Z * 24.0)
                .normalize_or_zero()
                .to_array();
            crate::game::vehicle::vehicle_damage(
                host,
                cx,
                id,
                Some(inflictor),
                attacker,
                dir,
                at,
                damage,
                crate::game::combat::DFLAG_RADIUS,
                mod_,
            )
            .into_iter()
            .map(|(event, args)| (cx.intern_folded(event), args))
            .collect::<Vec<_>>()
        });
        for (event, args) in notifies {
            self.vm.notify(Target::Entity(id), event, &args);
        }
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
    #[allow(clippy::too_many_arguments)]
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

    /// The corpse queue, whose entities every client's snapshot carries
    /// until their slots are reused (`crate::game::bodies`).
    /// The queue's corpses as the snapshot sends them: each with the event
    /// ring of the entity `cloneplayer` handed to script, which `playSound`
    /// appends to (`G_PlaySoundAlias` 0x67b30 writes `s.events` on any
    /// entity with no client).
    pub fn body_entities<'a>(
        &'a self,
        p: &'a vcod_common::net::protocol::Protocol,
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
    /// the live threads, then the entities and HUD elements with the fields
    /// script holds on each.
    pub fn script_usage(&mut self) -> String {
        let threads = self.vm.thread_usage();
        let ents = &self.host.ents;
        let (entity, hudelem) = self.vm.with_cx(|cx| {
            let tally = |it: &mut dyn Iterator<Item = vcod_gsc::StructId>| {
                it.fold((0, 0), |(n, v), s| (n + 1, v + cx.field_count(s)))
            };
            (
                tally(&mut ents.iter_inuse().map(|(_, e)| e.script)),
                tally(&mut ents.iter_hud_elems().map(|(_, e)| e.script)),
            )
        });
        crate::game::usage::script_usage(
            &threads,
            &[
                ("entity", entity.0, entity.1),
                ("hudelem", hudelem.0, hudelem.1),
            ],
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

    /// One server frame of script with no clients to trace a missile
    /// against: [`ScriptRuntime::run_threads`], then the whole entity pass.
    /// The blasts it sets off go unwalked and its movers push no one;
    /// `Cod1Module::run_frame_cod1` runs the halves itself.
    pub fn run_frame(&mut self, now_ms: i32) {
        self.run_threads(now_ms);
        let world = self.host.world.clone();
        let collision = world.as_deref().map(|w| &w.collision);
        let mut pass = self.begin_entity_pass(Vec::new());
        while let Some(stop) = self.run_entity_pass(&mut pass, collision, &[], now_ms) {
            match stop {
                // No bodies to block a mover: its push always goes through.
                PassStop::Push(step) => {
                    if let Some(c) = collision {
                        self.push_mover(&step, &mut [], c);
                    }
                }
                PassStop::Blast(_) => self.spawn_concussive(),
                PassStop::Client(_) => {}
            }
        }
    }

    /// `G_RunFrame` up to its entity loop: the packet pass, `level.time`,
    /// the touch, item and mover notifies, and the thread pass.
    pub fn run_threads(&mut self, now_ms: i32) {
        // The packet pass, on the clock the netcode raised its events with:
        // retail runs `ClientBegin`, `ClientCommand` and `ClientDisconnect`
        // from `SV_ExecuteClientMessage` (map-cycle doc, 4.4), before
        // `G_RunFrame` advances `level.time`. A callback runs to its first
        // suspend inside its event; what it or a command woke, and a `wait 0`
        // it took, run in this frame's first pass below, picked with the
        // threads due on the clock (`client-probes/probe_threadorder`,
        // cod11-gsc-language.md, "The packet pass"). That is what parks
        // `sd.gsc`'s connect callback on its `waittill("menuresponse")`
        // before the `openMenu` it just queued can be answered; one frame
        // later and the answer notifies nothing and the client never leaves
        // the team menu.
        //
        // Client events in the order the netcode raised them: a `Begin`
        // drained ahead of its own `Connect` finds no thread parked on the
        // notify and strands the client silently.
        for ev in std::mem::take(&mut self.host.client_events) {
            self.dispatch_client_event(ev);
        }
        // `G_RunFrame` starts under a new skeleton stamp, with the DObjs
        // where the last frame left them.
        self.host.dobj_time_ms = self.host.level_time_ms;
        self.host.clear_dobj_stamps();
        self.host.level_time_ms = now_ms;
        self.host.ents.set_clock(now_ms);
        // The notifies retail raises where they happen: the last entity
        // pass's movers and the cmds' touches, use key and item takes. They
        // only wake their waiters, which run in the first round below with
        // the trigger queue's (docs/research/cod11-gsc-language.md, "Engine
        // notifies and the trigger queue").
        for (id, event, args) in std::mem::take(&mut self.host.engine_notifies) {
            if self.host.ents.get(id).is_some() {
                let event = self.vm.with_cx(|cx| cx.intern_folded(event));
                self.vm.notify(Target::Entity(id), event, &args);
            }
        }
        self.trigger_rounds();
        // 1.1's second thread pass follows `G_DObjUpdateServerTime`; 1.5's
        // and UO's one pass runs before `G_XAnimUpdate` (1.5b 0x50821,
        // 0x50840; UO 0x59e4d, 0x59e6c), on the last frame's DObjs.
        if !self.host.patch_1_5 {
            self.host.dobj_time_ms = now_ms;
        }
        // The VM logs each error itself (`Vm::record_error`).
        self.vm
            .run_frame(&mut self.host, now_ms - self.script_lag_ms);
        self.host.dobj_time_ms = now_ms;
    }

    /// `G_RunFrame`'s trigger queue (0x50540..0x50665), on the clock of the
    /// frame after the touch: retail's plant bar carries the
    /// `scaleStartTime` of the first snapshot it is on
    /// (docs/research/cod11-gsc-object-model.md 23.1). Each round notifies
    /// `"trigger"` once per entity, in queue order with a sent entry
    /// swapped for the last, keeps a repeat for the next round, and runs
    /// `Scr_RunCurrentThreads` (0x50653); the rounds go on while an entry
    /// was kept. An entry whose entity or activator is gone is dropped.
    /// What a round's threads queue joins the queue, and is dropped with it
    /// once a round keeps nothing (0x50665).
    fn trigger_rounds(&mut self) {
        let event = self.vm.with_cx(|cx| cx.intern_folded("trigger"));
        loop {
            let mut sent = HashSet::new();
            let mut kept = false;
            let mut i = 0;
            while i < self.host.trigger_fires.len() {
                let (id, other) = self.host.trigger_fires[i];
                if self.host.ents.get(id).is_some() && self.host.ents.get(other).is_some() {
                    if !sent.insert(id) {
                        kept = true;
                        i += 1;
                        continue;
                    }
                    self.vm
                        .notify(Target::Entity(id), event, &[Value::Entity(other)]);
                }
                self.host.trigger_fires.swap_remove(i);
            }
            self.vm.run_current(&mut self.host);
            if !kept {
                break;
            }
        }
        self.host.trigger_fires.clear();
    }

    /// The start of `G_RunFrame`'s entity loop (0x50912), after the threads
    /// and every client write they made. `client_links` is each linked
    /// client's slot and parent.
    pub fn begin_entity_pass(&mut self, client_links: Vec<(usize, EntId)>) -> EntityPass {
        crate::game::link::prune(&mut self.host);
        EntityPass {
            next: 0,
            ran: HashSet::new(),
            client_links,
        }
    }

    /// `G_RunFrame`'s entity loop (0x50930..0x50975): `G_RunEntity` once per
    /// in-use entity by number, its link parent first, so items, links,
    /// movers, missiles, thinks and linked clients interleave the way
    /// retail's do (combat doc 14.7). Returns at each turn that needs the
    /// client sims ([`PassStop`]), which the caller handles before calling
    /// again; `None` once the loop is past the last entity. An entity spawned
    /// on the way runs this frame when its number is still ahead of the loop.
    pub fn run_entity_pass(
        &mut self,
        pass: &mut EntityPass,
        world: Option<&vcod_common::collision::CollisionWorld>,
        sims: &[(usize, &crate::spectate::ClientSim)],
        now_ms: i32,
    ) -> Option<PassStop> {
        use crate::game::bodies::{BODY_FIRST, BODY_QUEUE_SIZE};
        // A `G_Error` longjmps out of the frame.
        while pass.next < self.host.ents.num_entities() && self.host.game_error.is_none() {
            let n = pass.next;
            // `G_RunClient`'s arm (0x50422), for the one case it does work
            // in: a linked client, re-anchored by the caller.
            if let Some(&(slot, parent)) = pass.client_links.iter().find(|(s, _)| *s == n as usize)
            {
                if pass.ran.insert(parent)
                    && let Some(x) = self.run_entity(&pass.ran, parent, world, sims, now_ms)
                {
                    return Some(x);
                }
                pass.next += 1;
                return Some(PassStop::Client(slot));
            }
            // The body queue is not in the object table; a corpse's only
            // work is its think.
            if (BODY_FIRST..BODY_FIRST + BODY_QUEUE_SIZE as u32).contains(&n) {
                self.host
                    .bodies
                    .run_think(n, now_ms, &vcod_common::net::protocol::PROTOCOL_V1);
                pass.next += 1;
                continue;
            }
            let Some(id) = self.host.ents.handle(n) else {
                pass.next += 1;
                continue;
            };
            // 0x50939..0x50949. The parent's turn stamps it, so its own
            // turn later in the loop is a no-op (0x502cb).
            if let Some(parent) = self.host.links.get(id).map(|l| l.parent)
                && pass.ran.insert(parent)
                && let Some(x) = self.run_entity(&pass.ran, parent, world, sims, now_ms)
            {
                // `pass.next` stays, so the next call runs the child.
                return Some(x);
            }
            pass.next += 1;
            if pass.ran.insert(id)
                && let Some(x) = self.run_entity(&pass.ran, id, world, sims, now_ms)
            {
                return Some(x);
            }
        }
        // The end frames' traces stay under `G_RunFrame`'s stamp; the next
        // client packet starts its own.
        None
    }

    /// One `G_RunEntity` (0x502bc): a temp entity's free once its event has
    /// aged out (0x502f9..0x5031d), then the first arm the entity takes: a
    /// missile (0x50375), a link (0x50392, and `G_RunMover`'s 0x57623), a
    /// mover (0x5040e) or an item (0x503f5), and its think after.
    fn run_entity(
        &mut self,
        ran: &HashSet<EntId>,
        id: EntId,
        world: Option<&vcod_common::collision::CollisionWorld>,
        sims: &[(usize, &crate::spectate::ClientSim)],
        now_ms: i32,
    ) -> Option<PassStop> {
        use crate::game::missile::Turn;
        // A vehicle's turn is its think (`G_RunThink`): an undriven one's
        // 50 ms step runs here, by entity number among the rest
        // (coduo-vehicles.md 12, "The think's turn").
        if self.host.vehicles.live.contains_key(&id) {
            // A think script set (a `delete`'s free) replaces the
            // vehicle's own.
            let _ = self.host.run_entity_think(id, now_ms);
            if !self.host.vehicles.live.contains_key(&id) {
                return None;
            }
            let inputs = crate::game::vehicle::think::rider_inputs(&self.host, sims);
            let host = &mut self.host;
            let ran = self
                .vm
                .with_cx(|cx| crate::game::vehicle::think::run_undriven(host, cx, id, &inputs));
            if ran {
                self.vehicle_deaths();
            }
            return None;
        }
        let host = &mut self.host;
        // A missile meets the script models and UO's vehicles where they
        // stand on its turn.
        let models = if host.missiles.contains(id) {
            self.vm.with_cx(|cx| host.shot_models(cx))
        } else {
            Vec::new()
        };
        let (fs, anims) = (host.fs.clone(), host.anims.clone());
        let mut bones = crate::game::combat::BoneTraceCtx::of(
            fs.as_deref(),
            anims.as_deref(),
            &mut host.hit_rigs,
            now_ms,
            &mut host.dobj_posed,
            vcod_common::patch::Arith::for_protocol(host.defs.proto),
        );
        // `G_RunMissile` ends in `G_RunThink`: a free set on the missile's
        // entity replaces its fuse (combat doc 13.8).
        if host.missiles.contains(id)
            && let Some(at) = host.ents.take_free_think(id)
        {
            host.missiles.schedule_free(id, at);
        }
        let grenade = host.missiles.grenade_at(id);
        let turn = host
            .missiles
            .run_one(id, world, sims, &models, bones.as_mut(), now_ms);
        // `G_GrenadeTouchTriggerDamage` over the move, after its link and
        // ahead of the fuse's think, charged to the grenade with its
        // `explosionInnerDamage` (combat doc 12.2, 14.8).
        if let Some((from, weapon)) = grenade
            && matches!(turn, Some(Turn::Flew | Turn::Exploded(_)))
            && let Some(to) = host.missiles.origin_of(id)
        {
            let damage = host
                .weapons
                .get(weapon as usize)
                .map_or(0, |d| d.explosion_inner_damage);
            let grenade_mod = host.defs.mod_index("MOD_GRENADE").unwrap_or(3);
            self.vm.with_cx(|cx| {
                crate::game::damage_trigger::sweep(
                    host,
                    cx,
                    id,
                    from,
                    to,
                    damage,
                    grenade_mod,
                    true,
                )
            });
        }
        // A water entry splash is `G_TempEntity`'d inside the move (12.1).
        for te in host.missiles.take_splashes() {
            self.vm.with_cx(|cx| host.add_temp_entity(cx, te));
        }
        match turn {
            Some(Turn::Flew) => {
                host.missiles.sync_origin(&mut host.ents, id);
                host.link_shootable(id);
                return None;
            }
            Some(Turn::Exploded(x)) => {
                host.missiles.sync_origin(&mut host.ents, id);
                if x.struck
                    .and_then(|n| host.ents.handle(n))
                    .is_some_and(|v| host.vehicles.live.contains_key(&v))
                {
                    host.missiles.struck_vehicle(id);
                }
                return Some(PassStop::Blast(x));
            }
            Some(Turn::Freed) => {
                host.free_entity(id);
                return None;
            }
            None => {}
        }
        if host.run_temp_entity(id, now_ms) {
            return None;
        }
        if self.vm.with_cx(|cx| host.run_barrage(cx, id, now_ms)) {
            return None;
        }
        // A parent that has not had its turn is where script left it.
        let fresh = host.links.get(id).is_some_and(|l| ran.contains(&l.parent));
        let (done, step) = self.vm.with_cx(|cx| {
            if crate::game::link::run_linked(host, cx, id, fresh) {
                return (Vec::new(), None);
            }
            if crate::game::mover::is_mover(host, id) {
                return crate::game::mover::run_one(host, cx, id);
            }
            crate::game::item::run_item(host, cx, id, now_ms);
            (Vec::new(), None)
        });
        // `G_RunThink`, which every arm past the missile's ends on.
        if host.run_entity_think(id, now_ms).is_some() {
            self.vm
                .with_cx(|cx| crate::game::item::misc_spawner_think(host, cx, id));
        }
        host.run_turret_think(id);
        // Raised inside the pass, run by the next frame's threads.
        for d in done {
            host.engine_notifies.push((d.ent, d.event, Vec::new()));
        }
        step.map(PassStop::Push)
    }

    /// What the bots read off stock `sd.gsc` this frame
    /// (docs/research/bot-objectives.md): `game["attackers"]` and
    /// `game["defenders"]`, the live `bombzone_A` / `bombzone_B` triggers'
    /// bounds in that order, and once `level.bombplanted` the defuse
    /// trigger's origin and bounds. Entity handles do not survive a
    /// `map_restart`, so the caller asks again every frame.
    /// The stock `re.gsc` objectives as they stand this frame
    /// (docs/research/bot-objectives.md, "Retrieval"): the sides from
    /// `game["re_attackers"]` and `game["re_defenders"]`, and each entity of
    /// `level.retrieval_objective` the script has not deleted, through its
    /// `trigger`, `goal` and `objnum` fields and the compass record `objnum`
    /// names. `clients` bounds the carrier search.
    pub fn re_objectives(&mut self, clients: usize) -> ReObjectives {
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::{ArrayKey, Host};
        let level = self.vm.level_id();
        let client_ents: Vec<(usize, EntId)> = (0..clients)
            .filter_map(|slot| Some((slot, self.client_entity(slot)?)))
            .collect();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let game = cx.game();
            let mut team = |key: &str| {
                let k = ArrayKey::Str(cx.intern_exact(key));
                match cx.get_index(game, k) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    _ => String::new(),
                }
            };
            let attackers = team("re_attackers");
            let defenders = team("re_defenders");
            let [list, trigger, goal, objnum] =
                ["retrieval_objective", "trigger", "goal", "objnum"].map(|f| cx.intern_folded(f));
            let Value::Array(list) = cx.get_field(level, list) else {
                return ReObjectives {
                    attackers,
                    defenders,
                    objectives: Vec::new(),
                };
            };
            let mut objectives = Vec::new();
            for i in 0..cx.array_len(list) {
                let Value::Entity(obj) = cx.get_index(list, ArrayKey::Int(i as i32)) else {
                    continue;
                };
                if host.ents.get(obj).is_none() {
                    continue;
                }
                let (Value::Entity(t), Value::Entity(g)) = (
                    host.get_field(cx, obj, trigger),
                    host.get_field(cx, obj, goal),
                ) else {
                    continue;
                };
                let pickup = Some(entity_abs_bounds(host, cx, t)).filter(|b| b.1[2] > -5000.0);
                let goal = entity_abs_bounds(host, cx, g);
                // The compass record follows its carrier (`objective_onEntity`,
                // `re.gsc` 2062) until a drop pins it again (`retrieval_think`,
                // 1983). `hasobj` is no guide: a death drop never clears it
                // (`drop_objective_on_disconnect_or_death`, 2617).
                let record = match host.get_field(cx, obj, objnum) {
                    Value::Int(n) => host.objectives.get(n as usize).filter(|o| o.state != 0),
                    _ => None,
                };
                let carrier = record.and_then(|o| {
                    client_ents
                        .iter()
                        .find(|(_, ent)| ent.0 as i32 == o.ent_num)
                        .map(|(slot, _)| *slot)
                });
                objectives.push(ReObjective {
                    pickup,
                    goal,
                    carrier,
                    shown_to: record.map_or(0, |o| o.team_num),
                    laid: record.map(|o| o.origin_f32()),
                });
            }
            ReObjectives {
                attackers,
                defenders,
                objectives,
            }
        })
    }

    pub fn sd_objectives(&mut self) -> SdObjectives {
        use crate::game::builtins::entity::get_ent;
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::{ArrayKey, Host};
        let level = self.vm.level_id();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let game = cx.game();
            let mut team = |key: &str| {
                let k = ArrayKey::Str(cx.intern_exact(key));
                match cx.get_index(game, k) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    _ => String::new(),
                }
            };
            let attackers = team("attackers");
            let defenders = team("defenders");
            let mut find = |cx: &mut vcod_gsc::Cx, name: &str| {
                let args = [
                    Value::String(cx.intern_exact(name)),
                    Value::String(cx.intern_exact("targetname")),
                ];
                match get_ent(host, cx, None, &args) {
                    Ok(Value::Entity(id)) => Some(id),
                    _ => None,
                }
            };
            let zones: Vec<EntId> = ["bombzone_A", "bombzone_B"]
                .iter()
                .filter_map(|n| find(cx, n))
                .collect();
            let trigger = find(cx, "bombtrigger");
            let planted = cx.intern_folded("bombplanted");
            let planted = !matches!(
                cx.get_field(level, planted),
                Value::Undefined | Value::Int(0)
            );
            let sites = zones
                .into_iter()
                .map(|id| entity_abs_bounds(host, cx, id))
                .collect();
            let bomb = trigger.filter(|_| planted).map(|id| {
                let origin = cx.intern_folded("origin");
                let at = match host.get_field(cx, id, origin) {
                    Value::Vector(v) => v,
                    _ => [0.0; 3],
                };
                (at, entity_abs_bounds(host, cx, id))
            });
            SdObjectives {
                attackers,
                defenders,
                sites,
                bomb,
            }
        })
    }
}

impl ScriptRuntime {
    /// What the bots read off United Offensive's `ctf`, `dom`, `hq` and
    /// `bas` this frame (docs/research/bot-objectives.md section 7);
    /// `None` on any other gametype or before the script has set its
    /// objectives up. Read fresh every frame: a `map_restart` respawns
    /// them, and the scripts move their triggers about.
    pub fn uo_objectives(&mut self, gametype: &str, clients: usize) -> Option<UoObjectives> {
        use crate::game::builtins::entity::get_ent;
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::Host;
        let level = self.vm.level_id();
        let client_ents: Vec<(usize, EntId)> = (0..clients)
            .filter_map(|slot| Some((slot, self.client_entity(slot)?)))
            .collect();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let team_of = |cx: &mut vcod_gsc::Cx, v: Value| match v {
                Value::String(a) => match cx.resolve(a) {
                    "axis" => TEAM_AXIS,
                    "allies" | "allied" => TEAM_ALLIES,
                    _ => TEAM_NONE,
                },
                _ => TEAM_NONE,
            };
            let truthy = |v: Value| match v {
                Value::Int(i) => i != 0,
                Value::Float(f) => f != 0.0,
                _ => false,
            };
            let number = |v: Value| match v {
                Value::Int(i) => i as f32,
                Value::Float(f) => f,
                _ => 0.0,
            };
            let ent = |v: Value| match v {
                Value::Entity(e) => Some(e),
                _ => None,
            };
            // A trigger the script sent 10000 units down (`triggerOff`) is
            // not there to touch.
            let live = |b: Bounds| (b.1[2] > -5000.0).then_some(b);
            let f = |cx: &mut vcod_gsc::Cx, name: &str| cx.intern_folded(name);
            let origin = f(cx, "origin");
            match gametype {
                "ctf" => {
                    let [team, trigger, goal, mobile, moved, carried] = [
                        "team",
                        "trigger",
                        "goal",
                        "mobile_trigger",
                        "moved",
                        "carried_by",
                    ]
                    .map(|n| f(cx, n));
                    let mut flags = Vec::new();
                    for name in ["allies_flag", "axis_flag"] {
                        let at = f(cx, name);
                        let flag = ent(cx.get_field(level, at))?;
                        host.ents.get(flag)?;
                        let (Some(t), Some(g), Some(m)) = (
                            ent(host.get_field(cx, flag, trigger)),
                            ent(host.get_field(cx, flag, goal)),
                            ent(host.get_field(cx, flag, mobile)),
                        ) else {
                            return None;
                        };
                        let carrier = ent(host.get_field(cx, flag, carried)).and_then(|c| {
                            client_ents.iter().find(|(_, e)| *e == c).map(|(s, _)| *s)
                        });
                        let moved = truthy(host.get_field(cx, flag, moved));
                        let team = host.get_field(cx, flag, team);
                        flags.push(CtfFlag {
                            team: team_of(cx, team),
                            home: (!moved)
                                .then(|| live(entity_abs_bounds(host, cx, t)))
                                .flatten(),
                            dropped: (moved && carrier.is_none())
                                .then(|| live(entity_abs_bounds(host, cx, m)))
                                .flatten(),
                            goal: entity_abs_bounds(host, cx, g),
                            carrier,
                        });
                    }
                    Some(UoObjectives::Ctf(flags))
                }
                "dom" => {
                    let team = f(cx, "team");
                    let mut flags = Vec::new();
                    // `Flag_InitTriggers` looks for `flag1` .. `flag14`.
                    for q in 1..15 {
                        let args = [
                            Value::String(cx.intern_exact(&format!("flag{q}"))),
                            Value::String(cx.intern_exact("targetname")),
                        ];
                        let Ok(Value::Entity(id)) = get_ent(host, cx, None, &args) else {
                            continue;
                        };
                        let t = host.get_field(cx, id, team);
                        flags.push(DomFlag {
                            team: team_of(cx, t),
                            bounds: entity_abs_bounds(host, cx, id),
                        });
                    }
                    (!flags.is_empty()).then_some(UoObjectives::Dom(flags))
                }
                "hq" => {
                    let [list, hidden, team, radius] =
                        ["radio", "hidden", "team", "radius"].map(|n| f(cx, n));
                    let Value::Array(list) = cx.get_field(level, list) else {
                        return None;
                    };
                    let mut radios = Vec::new();
                    for i in 0..cx.array_len(list) {
                        let Some(r) = ent(cx.get_index(list, vcod_gsc::ArrayKey::Int(i as i32)))
                        else {
                            continue;
                        };
                        if host.ents.get(r).is_none() || truthy(host.get_field(cx, r, hidden)) {
                            continue;
                        }
                        let at = match host.get_field(cx, r, origin) {
                            Value::Vector(v) => v,
                            _ => continue,
                        };
                        let t = host.get_field(cx, r, team);
                        radios.push(HqRadio {
                            at,
                            team: team_of(cx, t),
                            radius: number(host.get_field(cx, r, radius)),
                        });
                    }
                    // The next radio's prespawn marker (`hq_obj_think`, 1792).
                    let next = host
                        .objectives
                        .first()
                        .filter(|o| o.state != 0 && radios.is_empty())
                        .map(|o| o.origin_f32());
                    Some(UoObjectives::Hq { radios, next })
                }
                "bas" => {
                    let [
                        list,
                        team,
                        health,
                        alive,
                        status,
                        objective,
                        model,
                        countdown,
                    ] = [
                        "bases",
                        "script_team",
                        "health",
                        "alive",
                        "status",
                        "objective",
                        "objectivemodel",
                        "countdowntime",
                    ]
                    .map(|n| f(cx, n));
                    let Value::Array(list) = cx.get_field(level, list) else {
                        return None;
                    };
                    let mut bases = Vec::new();
                    for i in 0..cx.array_len(list) {
                        let Some(b) = ent(cx.get_index(list, vcod_gsc::ArrayKey::Int(i as i32)))
                        else {
                            continue;
                        };
                        if host.ents.get(b).is_none() || !truthy(host.get_field(cx, b, alive)) {
                            continue;
                        }
                        let at = match host.get_field(cx, b, origin) {
                            Value::Vector(v) => v,
                            _ => continue,
                        };
                        let status = match host.get_field(cx, b, status) {
                            Value::String(a) => cx.resolve(a).to_string(),
                            _ => String::new(),
                        };
                        let trigger = ent(host.get_field(cx, b, objective))
                            .filter(|t| host.ents.get(*t).is_some());
                        let model = ent(host.get_field(cx, b, model))
                            .filter(|m| host.ents.get(*m).is_some())
                            .and_then(|m| match host.get_field(cx, m, origin) {
                                Value::Vector(v) => Some(v),
                                _ => None,
                            });
                        let t = host.get_field(cx, b, team);
                        let planted = status == "bomb_planted" || status == "bomb_defusing";
                        bases.push(BasBase {
                            team: team_of(cx, t),
                            at,
                            breached: number(host.get_field(cx, b, health)) <= 0.0,
                            planted,
                            blows_in: planted.then(|| number(host.get_field(cx, b, countdown))),
                            charge: trigger
                                .map(|t| entity_abs_bounds(host, cx, t))
                                .and_then(live)
                                .zip(model),
                        });
                    }
                    Some(UoObjectives::Bas(bases))
                }
                _ => None,
            }
        })
    }
}

/// [`ScriptRuntime::uo_objectives`]: one United Offensive gametype's state.
pub enum UoObjectives {
    /// The allied flag, then the axis one.
    Ctf(Vec<CtfFlag>),
    /// Each `flagN` trigger, in number order.
    Dom(Vec<DomFlag>),
    /// The radios up now, and the marker of the next one while none is.
    Hq {
        radios: Vec<HqRadio>,
        next: Option<[f32; 3]>,
    },
    /// Each `gmi_base` still alive.
    Bas(Vec<BasBase>),
}

/// One `ctf.gsc` flag (`ctf_spawn_flag`, `hold_flag`, `drop_flag`).
pub struct CtfFlag {
    /// Its owners; the other side takes it.
    pub team: i32,
    /// Its trigger while it stands at home.
    pub home: Option<Bounds>,
    /// The mobile trigger while it lies where its carrier fell.
    pub dropped: Option<Bounds>,
    /// The trigger a carrier of this flag delivers it to.
    pub goal: Bounds,
    pub carrier: Option<usize>,
}

/// One `dom.gsc` flag zone (`Flag_ZoneThink`).
pub struct DomFlag {
    /// Its owners, `TEAM_NONE` while neutral.
    pub team: i32,
    pub bounds: Bounds,
}

/// One `hq.gsc` radio on show (`hq_radio_think`).
pub struct HqRadio {
    pub at: [f32; 3],
    /// Its holders, `TEAM_NONE` while neutral.
    pub team: i32,
    /// The capture's reach across; `level.zradioradius` (50) is the height.
    pub radius: f32,
}

/// One `bas.gsc` base (`BaseAssault_base_think`).
pub struct BasBase {
    pub team: i32,
    pub at: [f32; 3],
    /// Its health is gone: a charge can be planted on it.
    pub breached: bool,
    /// A charge ticks on it.
    pub planted: bool,
    /// While planted, `countdowntime`: whole seconds before the charge
    /// blows (`BaseAssault_bomb_countdown` counts it down once a second).
    pub blows_in: Option<f32>,
    /// The plant trigger (a `trigger_lookat` or `trigger_use`) and the
    /// objective model the plant and defuse measure 150 units from.
    pub charge: Option<(Bounds, [f32; 3])>,
}

/// [`ScriptRuntime::re_objectives`]: the retrieval state a bot plays to.
pub struct ReObjectives {
    pub attackers: String,
    pub defenders: String,
    pub objectives: Vec<ReObjective>,
}

/// One of `level.retrieval_objective` still standing.
pub struct ReObjective {
    /// Its pickup `trigger_use`'s bounds, while it lies there to be taken;
    /// `None` once carried (`triggerOff` sends it 10000 units down).
    pub pickup: Option<Bounds>,
    /// The `trigger_multiple` a carrier delivers it to.
    pub goal: Bounds,
    /// The client its compass record follows.
    pub carrier: Option<usize>,
    /// The record's `teamNum`: 0 shows it to everyone, else to that team
    /// only. A carried one shows to the attackers alone unless
    /// `scr_re_showcarrier` is set (`re.gsc` `objective_think`, 1896-1911).
    pub shown_to: i32,
    /// The record's origin: where it last lay, which a carry leaves in
    /// place.
    pub laid: Option<[f32; 3]>,
}

/// [`ScriptRuntime::sd_objectives`]: the S&D state a bot plays to.
pub struct SdObjectives {
    pub attackers: String,
    pub defenders: String,
    /// `(mins, maxs)` of each bombzone still standing.
    pub sites: Vec<Bounds>,
    /// The planted bomb's defuse trigger: its origin (what `distance` reads)
    /// and its bounds (what the aim trace enters).
    pub bomb: Option<([f32; 3], Bounds)>,
}

/// Absolute `(mins, maxs)`.
pub type Bounds = ([f32; 3], [f32; 3]);

impl ScriptRuntime {
    /// A client's `sessionteam`, as a test sets it (`Server::test_set_client_team`).
    pub fn set_client_team_for_test(&mut self, slot: usize, team: &str) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("sessionteam");
            let v = Value::String(cx.intern_exact(team));
            host.set_field(cx, ent, field, v).unwrap();
        });
    }
}

#[cfg(test)]
impl ScriptRuntime {
    /// A playing client standing at `at`, linked there the way its cmd's
    /// link would: what a blast's walk finds it by.
    pub fn place_client(&mut self, slot: usize, at: [f32; 3]) {
        self.set_client_origin(slot, at);
        let bounds = self.host.client_box(slot);
        let body = vcod_common::movetrace::CONTENTS_BODY as i32;
        self.host.link_client(slot, at, bounds, body, true);
    }

    /// A `misc_mg42` at `at` with `G_SpawnTurret`'s health and a record
    /// off a bare weapon file.
    pub fn place_turret(&mut self, at: [f32; 3]) -> EntId {
        use vcod_gsc::Host;
        let host = &mut self.host;
        let id = self.vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            for (f, v) in [
                ("classname", Value::String(cx.intern_exact("misc_mg42"))),
                ("origin", Value::Vector(at)),
                ("health", Value::Int(crate::game::turret::TURRET_HEALTH)),
            ] {
                let a = cx.intern_folded(f);
                host.set_field(cx, id, a, v).unwrap();
            }
            id
        });
        let def = crate::game::turret::TurretDef::parse("WEAPONFILE\\weaponClass\\turret").unwrap();
        let rec = crate::game::turret::TurretRecord::new(
            "mg42_bipod_stand_mp",
            def,
            Default::default(),
            0.0,
        );
        self.host.turrets.insert(id, rec);
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.link_turret(cx, id));
        id
    }

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
        let row = crate::items::classname_index(self.host.defs, classname).unwrap();
        crate::game::item::attach(&mut self.host, id, row);
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::item::link(host, cx, id, crate::game::item::CONTENTS_ITEM));
        id
    }

    /// A `trigger_hurt` of `shape` on `id` doing `dmg` a touch, linked as
    /// its spawn would link it.
    pub fn register_hurt_for_test(
        &mut self,
        id: EntId,
        shape: crate::game::trigger::TriggerShape,
        dmg: i32,
    ) {
        self.host.triggers.register_hurt(id, shape, 0, 0, 0.0);
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded("dmg");
            let _ = vcod_gsc::Host::set_field(host, cx, id, atom, Value::Int(dmg));
            host.link_trigger(cx, id);
        });
    }

    /// A trigger the test registered, linked as its spawn would link it.
    pub fn link_trigger_for_test(&mut self, id: EntId) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.link_trigger(cx, id));
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
        let host = GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]);
        let mut rt = ScriptRuntime {
            vm,
            host,
            entry: path.to_string(),
            gametype_entry: String::new(),
            map: None,
            scripts: None,
            script_lag_ms: 0,
        };
        let main = rt.vm.func_ref(&rt.entry, "main");
        rt.vm.start_thread(&mut rt.host, main, None, vec![]);
        rt.host.initializing = false;
        rt
    }

    /// The path [`Self::install_for_test`] installs under.
    #[cfg(test)]
    pub fn entry_for_test(&self) -> &str {
        &self.entry
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

    /// Writes a client field through the field setter, the way script does.
    pub fn set_client_field_for_test(&mut self, slot: usize, name: &str, v: Value) {
        use vcod_gsc::Host;
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded(name);
            host.set_field(cx, ent, field, v).unwrap();
        });
    }

    /// A client's `sessionstate`, which `spawn_client` leaves at
    /// `"spectator"`; the four legal strings are in
    /// docs/research/cod11-map-cycle.md 6.1. Also the `takedamage` the
    /// client's next end frame would write for it.
    pub fn set_client_state_for_test(&mut self, slot: usize, state: &str) {
        use vcod_gsc::Host;
        self.set_client_takedamage(slot, state == "playing");
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
    pub fn start_thread_for_test(&mut self, ent: EntId, name: &str) {
        let f = self.vm.func_ref(&self.entry, name);
        self.vm
            .start_thread(&mut self.host, f, Some(Target::Entity(ent)), vec![]);
    }

    /// Start `path::name` as a thread on `recv` with the arguments `args`
    /// builds, the way a script's `thread` does.
    #[cfg(test)]
    pub fn start_script_thread_for_test(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<EntId>,
        args: impl FnOnce(&mut vcod_gsc::Cx) -> Vec<Value>,
    ) {
        let f = self.vm.func_ref(path, name);
        let args = self.vm.with_cx(args);
        let recv = Some(match recv {
            Some(e) => Target::Entity(e),
            None => Target::Struct(self.vm.level_id()),
        });
        self.vm.start_thread(&mut self.host, f, recv, args);
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

    /// [`ScriptRuntime::level_field`] rendered the way `client_field`
    /// renders.
    pub fn level_field_str(&mut self, name: &str) -> String {
        let v = self.level_field(name);
        self.vm.with_cx(|cx| render(cx, v))
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

    /// `delete()` notifies `"death"` with the entity (1.1d 0x5da14, UO
    /// 0x68bda): a thread holding `point endon("death")` on a deleted
    /// spawn point ends there, and one waiting on it wakes with the entity.
    /// UO's `_spawnlogic::Player_WaitForDeath` wrote through the dead point
    /// once its player died, a script error (bot-objectives.md 7.1).
    #[test]
    fn delete_notifies_death() {
        let mut rt = ScriptRuntime::for_test(
            "hold(p) { p endon(\"death\"); level waittill(\"go\"); level.held = 1; } \
             watch(p) { p waittill(\"death\", e); level.woke = (e == p); } \
             main() { p = spawn(\"script_origin\", (0, 0, 0)); level thread hold(p); \
             level thread watch(p); wait 0.05; p delete(); wait 0.05; level notify(\"go\"); }",
        );
        for frame in 1..=4 {
            rt.run_frame(frame * 50);
        }
        assert_eq!(rt.level_field("woke"), Value::Int(1));
        assert_eq!(rt.level_field("held"), Value::Undefined);
    }

    /// `probe_entnum` on retail: the free a `delete()` schedules 100 ms out
    /// runs in that frame's entity pass, after the frame's threads, so a
    /// thread woken on it still reads the entity (combat doc 14.7, "Entity
    /// numbers").
    #[test]
    fn a_delete_s_free_runs_after_the_threads_of_its_frame() {
        let mut rt = ScriptRuntime::for_test(
            "main() { wait 0.05; e = spawn(\"script_origin\", (0, 0, 0)); e delete(); \
             wait 0.1; level.at100 = isdefined(e); \
             wait 0.05; level.at150 = isdefined(e); }",
        );
        for frame in 1..=5 {
            rt.run_frame(frame * 50);
        }
        assert_eq!(rt.level_field("at100"), Value::Int(1));
        assert_eq!(rt.level_field("at150"), Value::Int(0));
    }

    /// `probe_entnum` on retail: a `playFx` temp entity takes the next
    /// number off `G_Spawn` between two script spawns, and goes to the tail
    /// of the free list on the first entity pass more than 300 ms past its
    /// event, so the spawn on the frame after that reuses it.
    #[test]
    fn a_temp_entity_takes_a_spawn_number_and_frees_it_past_300_ms() {
        let mut rt = ScriptRuntime::for_test(
            "num(e) { return e getEntityNumber(); } \
             main() { wait 0.05; a = spawn(\"script_origin\", (0, 0, 0)); playfx(1, (0, 0, 0)); \
             b = spawn(\"script_origin\", (0, 0, 0)); level.a = num(a); level.b = num(b); \
             wait 0.3; level.c = num(spawn(\"script_origin\", (0, 0, 0))); \
             wait 0.05; level.d = num(spawn(\"script_origin\", (0, 0, 0))); \
             wait 0.05; level.e = num(spawn(\"script_origin\", (0, 0, 0))); }",
        );
        // From a frame's thread pass, as the probe's spawns were: there the
        // script clock runs a frame behind `level.time`.
        for frame in 1..=10 {
            rt.run_frame(frame * 50);
        }
        let n = |rt: &mut ScriptRuntime, k: &str| match rt.level_field(k) {
            Value::Int(n) => n,
            other => panic!("{k}: {other:?}"),
        };
        let a = n(&mut rt, "a");
        let got: Vec<i32> = ["b", "c", "d", "e"].map(|k| n(&mut rt, k) - a).to_vec();
        assert_eq!(got, [2, 3, 4, 1]);
    }

    /// `probe_concnum` on retail: `grenadeExplosionEffect` takes two numbers
    /// off `G_Spawn`, the temp entity's and `Concussive_fx`'s. The first
    /// comes back on the +350 pass and the second on the +700 one, so of
    /// one spawn a frame after the call, the +400 one and the +750 one reuse
    /// them (combat doc 13.4).
    #[test]
    fn grenade_explosion_effect_takes_a_temp_and_a_concussion_number() {
        let mut rt = ScriptRuntime::for_test(
            "n(a) { return (spawn(\"script_origin\", (0, 0, 0)) getEntityNumber()) - a; } \
             main() { wait 0.05; a = spawn(\"script_origin\", (0, 0, 0)) getEntityNumber(); \
             grenadeexplosioneffect((0, 0, 0)); level.s = \"\" + n(a); \
             for (i = 1; i <= 16; i++) { wait 0.05; level.s = level.s + \" \" + n(a); } }",
        );
        // From a frame's thread pass, as the probe's calls were (after `wait 1`).
        for frame in 1..=21 {
            rt.run_frame(frame * 50);
        }
        // Retail: the spawn after the call 3, the +400 one the temp's 1,
        // the +750 one the concussion's 2.
        assert_eq!(
            rt.level_field_str("s"),
            "3 4 5 6 7 8 9 10 1 11 12 13 14 15 16 2 17"
        );
    }

    /// Both closures load into one `Vm` through one `Loader`, which is what
    /// keeps `Vm::install`'s duplicate rejection from firing on the first
    /// file the two share (`_utility`, `_teams` and the rest).
    #[test]
    fn the_map_and_gametype_closures_both_load() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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

    /// A CoD1 load handed the last level's gametype compile resumes it and
    /// ends with the compile a load from nothing makes.
    #[test]
    fn a_cod1_load_resumes_the_gametype_compile_where_a_fresh_one_lands() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let fs = Rc::new(fs);
        let load = |map: &str, carry: Carry| {
            ScriptRuntime::load(
                fs.clone(),
                map,
                "dm",
                vec![String::new(); 2048],
                crate::cvars::Cvars::new(),
                None,
                Rc::new(crate::weapons::WeaponTable::empty()),
                0,
                carry,
            )
            .unwrap()
        };
        let first = load("mp_pavlov", Carry::default());
        let carry = Carry {
            gametype: first.take_carry().gametype,
            ..Carry::default()
        };
        let resumed = load("mp_carentan", carry);
        let fresh = load("mp_carentan", Carry::default());
        let (a, b, c) = (
            first.compiled().unwrap(),
            resumed.compiled().unwrap(),
            fresh.compiled().unwrap(),
        );
        assert!(Rc::ptr_eq(a.gametype(), b.gametype()));
        assert!(!Rc::ptr_eq(b.gametype(), c.gametype()));
        assert!(b.same_compile(c));
        assert_eq!(resumed.configstrings(), fresh.configstrings());
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            Carry::default(),
        )
        .expect("load mp_carentan on sd");
        rt.install_for_test(
            "plant() { self.angles = (0, 29.6, 0); p = self maps\\mp\\_utility::getPlant(); \
             level.charge = p.origin; objective_add(0, \"current\", p.origin); }",
        );
        let planter = rt.spawn_map_entity_for_test([-192.8, 2457.1, -21.9]);
        rt.start_thread_for_test(planter, "plant");
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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

    /// A call outside the version's builtin tables is retail's script
    /// compile error, which fails the load; the same script loads under the
    /// version that lists the name (`builtins::set`).
    #[test]
    fn a_builtin_another_version_lists_fails_the_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        struct UoCall(Rc<Pk3Fs>);
        impl ScriptSource for UoCall {
            fn read(&self, canonical: &str) -> Option<String> {
                if canonical == "maps/mp/gametypes/uocall" {
                    return Some("main() { n = getNumVehicles(); }\n".to_string());
                }
                PakScripts::new(self.0.clone(), None).read(canonical)
            }
        }
        let fs = Rc::new(fs);
        let load = |protocol: &str| {
            let mut cvars = crate::cvars::Cvars::new();
            cvars.set("protocol", protocol);
            ScriptRuntime::load_from(
                Box::new(UoCall(fs.clone())),
                fs.clone(),
                "mp_pavlov",
                "uocall",
                vec![String::new(); 2048],
                cvars,
                None,
                Rc::new(crate::weapons::WeaponTable::empty()),
                0,
                Carry::default(),
            )
        };
        let Err(err) = load("1") else {
            panic!("getNumVehicles must not compile on 1.1");
        };
        assert!(
            err.to_string()
                .contains("unknown (builtin) function 'getnumvehicles'"),
            "unexpected error: {err:#}"
        );
        if let Err(e) = load("22") {
            assert!(!e.to_string().contains("unknown (builtin)"), "{e:#}");
        }
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
    /// 1.5's `maxspeed`: `g_speed`'s integer at the connect, floored at 0
    /// by its setter, which copies it into the entity's `speed`; it is what
    /// `ClientEndFrame` stores in `ps.speed`. 1.1 stores `g_speed` itself.
    #[test]
    fn maxspeed_seeds_from_g_speed_and_feeds_the_end_frame() {
        let src = "main() { level.callbackPlayerConnect = ::c; }\n\
                   CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
                   c() { level.seeded = self.maxspeed; self.maxspeed = -5; \
                         level.floored = self.maxspeed; level.ent = self.speed; \
                         self.maxspeed = 220; }\n";
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, src);
        rt.host.set_profile(&crate::game::profile::COD1_15);
        rt.host.cvars.set("g_speed", "300.9");
        rt.reconnect_client(0, "vcod".into());
        assert_eq!(rt.level_field("seeded"), vcod_gsc::Value::Int(300));
        assert_eq!(rt.level_field("floored"), vcod_gsc::Value::Int(0));
        assert_eq!(rt.level_field("ent"), vcod_gsc::Value::Float(0.0));
        assert_eq!(rt.host.client_speed(0), 220);
        // A later `g_speed` waits for the next connect.
        rt.host.cvars.set("g_speed", "190");
        assert_eq!(rt.host.client_speed(0), 220);

        rt.host.set_profile(&crate::game::profile::COD1_11);
        assert_eq!(rt.host.client_speed(0), 190);
    }

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
        before.reconnect_client(0, "vcod".into());
        assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
        // Something only the first level wrote, to show what does not carry.
        before.host.client_vitals[0].health = 42;
        let carry = before.take_carry();

        let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
        after.host.pers_carry = carry.pers;
        after.reconnect_client(0, "vcod".into());
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
        after.reconnect_client(0, "vcod".into());
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
        before.reconnect_client(0, "vcod".into());
        assert_eq!(before.client_pers(0, "team").as_deref(), Some("axis"));
        let carry = before.take_carry();

        let mut after = ScriptRuntime::for_test_at(CALLBACK_SETUP, &src("allies"));
        after.host.pers_carry = carry.pers;
        // The drop the restart's `n` can cause: the slot has no entity in
        // this runtime yet, its `ClientConnect` has not run.
        after.push_client_event(ClientEvent::Disconnect(0));
        after.run_frame(50);

        after.reconnect_client(0, "stranger".into());
        assert_eq!(
            after.client_pers(0, "team").as_deref(),
            Some("allies"),
            "the dropped client's carried pers reached the next occupant"
        );
        assert!(after.aborts().is_empty(), "{:?}", after.aborts());
    }

    /// `ClientDisconnect`'s `menuresponse` ("-1", "disconnect") wakes a
    /// level thread waiting on the player, which runs on the next frame's
    /// clock, after the callback; the player's own waiter dies with it
    /// (`client-probes/probe_dropclient`, all three retail servers).
    #[test]
    fn a_drop_notifies_menuresponse_for_the_next_frame() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.lvl = \"none\"; level.own = \"none\"; }\n\
             CodeCallback_PlayerConnect() { level thread lvl(self); self thread own(); }\n\
             CodeCallback_PlayerDisconnect() { level.cb = getTime(); }\n\
             lvl(p) { p waittill(\"menuresponse\", menu, response); \
                      level.lvl = menu + \" \" + response + \" \" + getTime(); }\n\
             own() { self waittill(\"menuresponse\", menu, response); level.own = menu; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        rt.client_disconnect(0);
        assert_eq!(rt.level_field("cb"), Value::Int(50));
        // `CalculateRanks` closes `ClientDisconnect`.
        assert!(rt.take_ranks_dirty());
        assert_eq!(rt.level_field_str("lvl"), "none");
        rt.run_frame(100);
        assert_eq!(rt.level_field_str("lvl"), "-1 disconnect 100");
        assert_eq!(rt.level_field_str("own"), "none");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
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
             c() { self.probe_out = \"connecting\"; self waittill(\"begin\"); \
                   self.probe_out = \"begun\"; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "probe_out"), "connecting");

        rt.push_client_event(ClientEvent::Begin(0));
        rt.run_frame(100);
        assert_eq!(rt.field_str(ent, "probe_out"), "begun");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
    }

    /// The `begin` waiter runs in the frame after the packet that entered
    /// the client, on that frame's clock, as on all three retail servers
    /// (`client-probes/probe_dropclient`: `PROBE begin` reads the first
    /// snapshot's `serverTime`).
    #[test]
    fn the_begin_waiter_runs_on_the_next_frames_clock() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() { level.callbackPlayerConnect = ::c; }\n\
             CodeCallback_PlayerConnect() { [[level.callbackPlayerConnect]](); }\n\
             c() { self waittill(\"begin\"); self.probe_out = \"\" + getTime(); }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        rt.push_client_event(ClientEvent::Begin(0));
        rt.run_frame(100);
        assert_eq!(rt.field_str(ent, "probe_out"), "100");
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
             c() { self.probe_out = \"connecting\"; self waittill(\"begin\"); \
                   self.probe_out = \"begun\"; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.push_client_event(ClientEvent::Begin(0));
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "probe_out"), "begun");
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
                   self.probe_out = self.name + \":\" + self.pers[\"team\"]; }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "player".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");
        assert_eq!(rt.field_str(ent, "probe_out"), "player:spectator");
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
                   self.probe_out = menu + \":\" + response; }\n",
        );
        let (lo, _) = crate::configstrings::CsRange::Menu.bounds();
        rt.host.configstrings.set(lo + 1, "weapon_russian");
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let ent = rt.client_entity(0).expect("connect allocated no entity");

        rt.menu_response(0, &MenuRef::Index(1), "mosin_nagant_mp");
        rt.run_frame(100);
        assert_eq!(
            rt.field_str(ent, "probe_out"),
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

    /// The missile `fire_grenade` just spawned.
    fn live_missile(rt: &ScriptRuntime) -> EntId {
        rt.host
            .ents
            .iter_inuse()
            .map(|(id, _)| id)
            .find(|&id| rt.host.missiles.contains(id))
            .expect("a missile in the pool")
    }

    /// A script `delete` on a grenade in flight arms `G_FreeEntity` 100 ms
    /// out, which `G_RunMissile`'s closing `G_RunThink` runs in place of
    /// the fuse: it flies on until then and goes without exploding
    /// (combat doc 13.8).
    #[test]
    fn a_deleted_grenade_flies_on_and_goes_unexploded() {
        let mut rt = ScriptRuntime::for_test("main() {}\ndel() { self delete(); }\n");
        let v = glam::Vec3::new(100.0, 0.0, 0.0);
        rt.fire_grenade(0, 1, 0, glam::Vec3::Z * 100.0, v, glam::Vec3::ZERO, 3000, 0);
        let g = live_missile(&rt);
        rt.run_frame(50);
        rt.start_thread_for_test(g, "del");
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        rt.run_frame(100);
        assert!(rt.host.missiles.contains(g), "it flies on until the free");
        rt.run_frame(150);
        assert!(!rt.host.missiles.contains(g), "the free took it out");
        assert!(rt.host.ents.get(g).is_none());
    }

    /// 1.1's drop ring holds numbers: a grenade that took a held drop's
    /// number is what a full ring's next drop evicts, and the eviction's
    /// free a frame on replaces its fuse (items doc 8, combat doc 13.8).
    #[test]
    fn on_1_1_a_grenade_on_an_evicted_ring_number_goes_unexploded() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() {}\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let health = 68;
        rt.host.precache_items_for_test(&["item_health"]);
        let host = &mut rt.host;
        let drops: Vec<EntId> = rt.vm.with_cx(|cx| {
            (0..crate::game::item::DROP_RING)
                .map(|_| crate::game::item::drop_item(host, cx, 0, health).unwrap())
                .collect()
        });
        rt.host.free_entity(drops[0]);
        let v = glam::Vec3::new(100.0, 0.0, 0.0);
        rt.fire_grenade(
            0,
            1,
            0,
            glam::Vec3::Z * 100.0,
            v,
            glam::Vec3::ZERO,
            3000,
            50,
        );
        let g = live_missile(&rt);
        assert_eq!(
            g.0, drops[0].0,
            "the grenade took the picked-up drop's number"
        );
        let host = &mut rt.host;
        rt.vm
            .with_cx(|cx| crate::game::item::drop_item(host, cx, 0, health))
            .unwrap();
        rt.run_frame(100);
        assert!(!rt.host.missiles.contains(g), "the eviction freed it");
        assert!(rt.host.ents.get(g).is_none());
    }

    /// `G_FreeEntityRefs` sweeps a barrage's `r.ownerNum` with the rest: a
    /// strike whose caller left hangs shells nobody owns, and so charges
    /// no one (UO `fire_artillery` 0x6002d, combat doc 13.7).
    #[test]
    fn a_barrage_whose_caller_left_hangs_shells_nobody_owns() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() {}\n\
             CodeCallback_PlayerDisconnect() {}\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 3,
            name: "vcod".into(),
        });
        rt.run_frame(50);
        let def = vcod_common::weapon::WeaponDef {
            projectile_count: 4,
            projectile_radius: 100,
            projectile_delay: 100,
            projectile_spacing: (100, 100),
            projectile_speed: 10_000.0,
            damage: 2000,
            proj_explosion_type: "artillery".into(),
            ..Default::default()
        };
        let host = &mut rt.host;
        let id = rt.vm.with_cx(|cx| host.ents.spawn(cx)).unwrap();
        let strike = crate::game::artillery::Barrage::new(id, 3, 6, glam::Vec3::ZERO, &def, 50);
        rt.host.barrages.push(strike);
        rt.push_client_event(ClientEvent::Disconnect(3));
        for f in 2..12 {
            rt.run_frame(f * 50);
        }
        let shells: Vec<Option<usize>> = rt
            .host
            .ents
            .iter_inuse()
            .filter_map(|(id, _)| rt.host.missiles.owner_of(id))
            .collect();
        assert!(!shells.is_empty(), "no shell was hung");
        assert!(shells.iter().all(Option::is_none), "{shells:?}");
    }

    /// UO's `G_MissileImpact` with no owner hands `G_Damage` NULL for both
    /// inflictor and attacker, and `Scr_PlayerDamage` pushes each as
    /// undefined (combat doc 13.7).
    #[test]
    fn a_hit_with_no_inflictor_and_no_attacker_reaches_the_callback_undefined() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() {}\n\
             CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
                 self.seen = isDefined(eInflictor) + \" \" + isDefined(eAttacker) + \" \" + sWeapon;\n\
             }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(0);
        rt.host.client_vitals[0].takedamage = true;
        rt.deliver_hits(vec![crate::game::combat::Hit {
            victim: 0,
            attacker: crate::game::host::ENTITYNUM_NONE as usize,
            inflictor: None,
            damage: 300,
            dflags: 0,
            mod_: "MOD_ARTILLERY",
            weapon: "none".into(),
            point: [0.0; 3],
            dir: [0.0, 0.0, -1.0],
            hitloc: "none",
        }]);
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "seen").as_deref(), Some("0 0 none"));
    }

    /// A UO vehicle shell's fuse blast names the vehicle as `eAttacker`
    /// (`G_ExplodeMissile`'s `parent`, game.mp.uo 0x5e2fc, combat doc 13.7),
    /// and `charged_owner` keeps its number.
    #[test]
    fn a_blast_charged_to_a_vehicle_names_it() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() {}\n\
             CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
                 self.seen = eAttacker getEntityNumber() + \" \" + isPlayer(eAttacker);\n\
             }\n",
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "vcod".into(),
        });
        rt.run_frame(0);
        rt.host.client_vitals[0].takedamage = true;
        let tank = rt.vm.with_cx(|cx| rt.host.ents.spawn(cx)).unwrap();
        rt.host
            .vehicles
            .live
            .insert(tank, crate::game::vehicle::Vehicle::for_test(0));
        let attacker = rt.charged_owner(Some(tank.0 as usize)).unwrap();
        assert_eq!(attacker, tank.0 as usize);
        rt.deliver_hits(vec![crate::game::combat::Hit {
            victim: 0,
            attacker,
            inflictor: None,
            damage: 30,
            dflags: crate::game::combat::DFLAG_RADIUS,
            mod_: "MOD_PROJECTILE_SPLASH",
            weapon: "none".into(),
            point: [0.0; 3],
            dir: [0.0, 0.0, -1.0],
            hitloc: "none",
        }]);
        // The record has no info for the frame's vehicle think.
        rt.host.vehicles.live.remove(&tank);
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "seen"), Some(format!("{} 0", tank.0)));
    }

    /// A UO vehicle shell charges its driver's slot (`+0x330`) after he
    /// left, and `Scr_AddEntity` reads the freed entity's number, 0 after
    /// `G_FreeEntity`'s memset: the client in slot 0 is `eAttacker`
    /// (game.mp.uo 0x5dcff, combat doc 13.7).
    #[test]
    fn a_hit_charged_to_an_empty_slot_names_entity_0() {
        let mut rt = ScriptRuntime::for_test_at(
            CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerConnect() {}\n\
             CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {\n\
                 self.seen = eAttacker getEntityNumber();\n\
             }\n",
        );
        for slot in [0, 1] {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: "vcod".into(),
            });
        }
        rt.run_frame(0);
        rt.host.client_vitals[1].takedamage = true;
        assert_eq!(rt.charged_owner(Some(0)), Some(0));
        let attacker = rt.charged_owner(Some(5)).unwrap();
        assert_eq!(attacker, crate::game::host::ZEROED_ENTITY);
        rt.deliver_hits(vec![crate::game::combat::Hit {
            victim: 1,
            attacker,
            inflictor: None,
            damage: 30,
            dflags: 0,
            mod_: "MOD_PROJECTILE",
            weapon: "none".into(),
            point: [0.0; 3],
            dir: [0.0, 0.0, -1.0],
            hitloc: "none",
        }]);
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(1, "seen").as_deref(), Some("0"));
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
            crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
        );
        rt.link_trigger_for_test(zone);
        rt.start_thread_for_test(zone, "trigger_think");
        rt.run_frame(0);

        let player = rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        assert_eq!(rt.client_entity(0), Some(player));
        rt.set_client_state_for_test(0, "playing");

        rt.touch_triggers(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
        assert_eq!(rt.level_field("who"), Value::Entity(player));

        // Out of the box: no second notify.
        rt.set_client_origin(0, [500.0, 0.0, 0.0]);
        rt.touch_triggers(0, 100);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
    }

    /// The retrieval carrier is the client the compass record follows. A
    /// death drop leaves the dead carrier's `hasobj` naming the objective
    /// (bot-objectives.md 4.1), so a stale `hasobj` on a lower slot must not
    /// win over the record.
    #[test]
    fn the_retrieval_carrier_is_who_the_compass_follows() {
        let mut rt = ScriptRuntime::for_test(
            "main() {
                game[\"re_attackers\"] = \"allies\";
                game[\"re_defenders\"] = \"axis\";
                obj = spawn(\"script_model\", (0, 0, 0));
                obj.trigger = spawn(\"script_origin\", (0, 0, 0));
                obj.goal = spawn(\"script_origin\", (500, 0, 0));
                obj.objnum = 1;
                objective_add(1, \"current\", (100, 0, 0));
                level.retrieval_objective = [];
                level.retrieval_objective[0] = obj;
            }
            stale() {
                self.hasobj = [];
                self.hasobj[1] = level.retrieval_objective[0];
                level.stale = 1;
            }
            carry() { objective_onEntity(1, self); }",
        );
        rt.run_frame(0);
        let o = rt.re_objectives(4);
        assert_eq!(o.objectives[0].carrier, None);
        assert_eq!(o.objectives[0].laid, Some([100.0, 0.0, 0.0]));
        let dead = rt.spawn_client_for_test(0, [0.0; 3]);
        let live = rt.spawn_client_for_test(1, [0.0; 3]);
        rt.start_thread_for_test(dead, "stale");
        rt.start_thread_for_test(live, "carry");
        rt.run_frame(50);
        assert_eq!(rt.level_field("stale"), Value::Int(1));
        let o = rt.re_objectives(4);
        assert_eq!(o.objectives[0].carrier, Some(1));
        assert_eq!(o.objectives[0].shown_to, 0);
        assert_eq!(o.objectives[0].laid, Some([100.0, 0.0, 0.0]));
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
            crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
        );
        rt.link_trigger_for_test(zone);
        rt.start_thread_for_test(zone, "trigger_think");
        rt.run_frame(0);
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.touch_triggers(0, 0);
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
        );
        rt.link_trigger_for_test(zone);
        rt.start_thread_for_test(zone, "trigger_think");
        rt.run_frame(0);
        rt.spawn_client_for_test(0, [0.9, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);
        rt.set_client_aim(0, [0.9, 0.0, 60.9], [0.0, 0.0], false);
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
        );
        rt.link_trigger_for_test(zone);
        rt.set_level_field_for_test("zone", Value::Entity(zone));
        rt.start_thread_for_test(zone, "trigger_think");
        rt.run_frame(0);

        let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);

        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
        rt.aim_lookat(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "one notify");
        assert_eq!(rt.level_field("who"), Value::Entity(player));
        rt.start_thread_for_test(player, "check");
        rt.run_frame(100);
        assert_eq!(rt.level_field("looking"), Value::Int(1));

        // Aimed away: no fire, and the answer drops.
        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 90.0], false);
        rt.aim_lookat(0, 150);
        rt.run_frame(150);
        assert_eq!(rt.level_field("hits"), Value::Int(1));
        rt.start_thread_for_test(player, "check");
        rt.run_frame(200);
        assert_eq!(rt.level_field("looking"), Value::Int(0));

        // Every frame it is aimed at, since `G_Trigger` gates nothing.
        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
        rt.aim_lookat(0, 250);
        rt.run_frame(250);
        rt.aim_lookat(0, 300);
        rt.run_frame(300);
        assert_eq!(rt.level_field("hits"), Value::Int(3));
    }

    /// A live body between the eye and a `trigger_lookat` stops the aim
    /// trace's second pass (mask 0x22802001 carries BODY), so neither the
    /// fire nor `isLookingAt` happens; the aimer's own body is the pass
    /// entity and never does (object-model doc 23.1). Moved aside, it fires.
    #[test]
    fn a_body_between_the_eye_and_a_lookat_blocks_the_fire_and_islookingat() {
        let mut rt = ScriptRuntime::for_test("main() { level.hits = 0; }");
        rt.install_for_test(
            "trigger_think() { for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } }\n\
             check() { level.looking = self islookingat(level.zone); }",
        );
        let zone = rt.spawn_map_entity_for_test([300.0, 0.0, 0.0]);
        rt.triggers_mut().register(
            zone,
            crate::game::trigger::TriggerKind::LookAt,
            crate::game::trigger::TriggerShape::boxed([-20.0, -20.0, 40.0], [20.0, 20.0, 80.0]),
            0,
        );
        rt.link_trigger_for_test(zone);
        rt.set_level_field_for_test("zone", Value::Entity(zone));
        rt.start_thread_for_test(zone, "trigger_think");
        rt.run_frame(0);
        let player = rt.spawn_client_for_test(0, [0.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);
        let body = |slot: usize, x: f32, y: f32| crate::game::combat::HitBody {
            slot,
            origin: glam::Vec3::new(x, y, 0.0),
            yaw: 0.0,
            mins: crate::game::trigger::PLAYER_MINS.into(),
            maxs: crate::game::trigger::PLAYER_MAXS.into(),
            pose: Default::default(),
        };
        rt.set_client_body(0, Some(body(0, 0.0, 0.0)));
        rt.set_client_body(1, Some(body(1, 150.0, 0.0)));
        rt.set_client_aim(0, [0.0, 0.0, 60.0], [0.0, 0.0], false);
        rt.aim_lookat(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "the body blocks");
        rt.start_thread_for_test(player, "check");
        rt.run_frame(100);
        assert_eq!(rt.level_field("looking"), Value::Int(0));

        rt.set_client_body(1, Some(body(1, 150.0, 100.0)));
        rt.aim_lookat(0, 150);
        rt.run_frame(150);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "moved aside");
        rt.start_thread_for_test(player, "check");
        rt.run_frame(200);
        assert_eq!(rt.level_field("looking"), Value::Int(1));
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
            crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
        );
        rt.link_trigger_for_test(zone);
        rt.start_thread_for_test(zone, "trigger_think");
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.set_client_state_for_test(0, "playing");
        rt.touch_triggers(0, 0);
        rt.run_frame(0);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "the row fires first");

        // `delete()` defers the free by `DELETE_DEFER_MS`; 200 is past it.
        rt.start_thread_for_test(zone, "remove");
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
        rt.touch_triggers(0, 250);
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
            crate::game::trigger::MULTIPLE_DEFAULT_WAIT_MS,
        );
        rt.link_trigger_for_test(zone);
        rt.start_thread_for_test(zone, "trigger_think");
        rt.spawn_client_for_test(0, [10.0, 0.0, 0.0]);
        rt.run_frame(0);

        // A spectator, at `pm_type` 4. The old gate excluded it off
        // `sessionstate`; the measured rule excludes it off the compare, so
        // the behaviour is unchanged.
        rt.set_client_state_for_test(0, "spectator");
        rt.set_client_pm_type(0, crate::spectate::PM_SPECTATOR);
        rt.touch_triggers(0, 50);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "a spectator touched");

        rt.set_client_state_for_test(0, "intermission");
        rt.set_client_pm_type(0, crate::spectate::PM_INTERMISSION);
        rt.touch_triggers(0, 75);
        rt.run_frame(75);
        assert_eq!(
            rt.level_field("hits"),
            Value::Int(0),
            "an intermission camera touched"
        );

        rt.set_client_state_for_test(0, "dead");
        rt.set_client_pm_type(0, crate::spectate::PM_DEAD);
        rt.touch_triggers(0, 100);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "a corpse touched");

        // And the living player the gate is there to let through.
        rt.set_client_state_for_test(0, "playing");
        rt.set_client_pm_type(0, 0);
        rt.touch_triggers(0, 125);
        rt.run_frame(125);
        assert_eq!(
            rt.level_field("hits"),
            Value::Int(1),
            "a live player did not"
        );
    }

    /// A `trigger_hurt` damages the player standing in it through the stock
    /// damage callback, so the victim's health drops on the path a bullet's
    /// hit already takes, and the 100 ms cadence `arm_hurt` arms gates
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
        rt.register_hurt_for_test(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, 0.0], [64.0, 64.0, 64.0]),
            5,
        );

        rt.touch_triggers(0, 100);
        rt.run_frame(100);
        let expected_mod = rt
            .vm
            .with_cx(|cx| cx.intern_exact(crate::game::trigger::MOD_TRIGGER_HURT));
        assert_eq!(rt.level_field("damage"), Value::Int(5));
        assert_eq!(rt.level_field("mod"), Value::String(expected_mod));
        assert_eq!(rt.client_vitals(0).health, 95);

        rt.touch_triggers(0, 150);
        rt.run_frame(150);
        assert_eq!(rt.client_vitals(0).health, 95, "inside the 100 ms window");

        rt.touch_triggers(0, 200);
        rt.run_frame(200);
        assert_eq!(rt.client_vitals(0).health, 90);
    }

    /// A script `spawn("trigger_hurt", ...)` runs `SP_trigger_hurt`: it is a
    /// hurt trigger and its `dmg` reads 5 (cod11-combat.md 5.9).
    #[test]
    fn a_script_spawned_trigger_hurt_reads_dmg_5() {
        let mut rt = ScriptRuntime::for_test(
            "main() { t = spawn(\"trigger_hurt\", (0, 0, 0), 16); level.t = t; \
             level.dmg = t.dmg; }",
        );
        rt.run_frame(50);
        assert_eq!(rt.level_field("dmg"), Value::Int(5));
        let Value::Entity(t) = rt.level_field("t") else {
            panic!("no trigger");
        };
        assert_eq!(
            rt.triggers_mut().get(t).map(|t| t.kind),
            Some(crate::game::trigger::TriggerKind::Hurt)
        );
    }

    /// A `trigger_use` answers the use key through the aim pick, the way
    /// `Cmd_Activate_f` reaches it, and never through contact
    /// (docs/research/cod11-gametypes-re-bel.md 4): standing in one with the
    /// key down raises nothing, a press while aimed at it from outside fires
    /// it, and the hint pass shows `HINT_ACTIVATE` with the string
    /// `setHintString` stored.
    #[test]
    fn a_trigger_use_fires_off_the_aim_and_never_off_contact() {
        let mut rt = pickup_rig();
        rt.install_for_test(
            "trigger_think() { level.hits = 0; for(;;) { self waittill(\"trigger\", other); \
             level.hits = level.hits + 1; } } \
             hint() { self setHintString(&\"RE_PRESS_TO_PICKUP_GENERIC\"); } \
             unhint() { self setHintString(\"\"); }",
        );
        let pickup = rt.spawn_map_entity_for_test([40.0, 0.0, 30.0]);
        rt.triggers_mut().register(
            pickup,
            crate::game::trigger::TriggerKind::Use,
            crate::game::trigger::TriggerShape::boxed([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]),
            0,
        );
        rt.link_trigger_for_test(pickup);
        rt.start_thread_for_test(pickup, "trigger_think");
        rt.run_frame(0);
        let use_key = vcod_common::net::msg::BUTTON_USE;

        rt.set_client_origin(0, [40.0, 0.0, 0.0]);
        rt.item_pass(0, 0, EYE, [-36.0, 0.0, 0.0]);
        rt.touch_triggers(0, 50);
        rt.item_pass(0, use_key, EYE, [-36.0, 0.0, 0.0]);
        rt.run_frame(50);
        assert_eq!(rt.level_field("hits"), Value::Int(0), "inside, aimed away");

        rt.item_pass(0, 0, EYE, AT_ITEM);
        rt.item_pass(0, use_key, EYE, AT_ITEM);
        rt.run_frame(100);
        assert_eq!(rt.level_field("hits"), Value::Int(1), "aimed at it");

        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM), (2, Some(-1)));
        rt.start_thread_for_test(pickup, "hint");
        let slot = rt
            .cursor_hint_pass(0, EYE, AT_ITEM)
            .1
            .expect("a hint string");
        assert_eq!(
            rt.host.configstrings[1212 + slot as usize],
            "RE_PRESS_TO_PICKUP_GENERIC\u{15}"
        );
        rt.start_thread_for_test(pickup, "unhint");
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_ITEM), (2, Some(-1)));
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
        // Past the first wait `setShader` only finds a precached shader.
        rt.host
            .allocators
            .index(
                &mut rt.host.configstrings,
                crate::configstrings::CsRange::Shader,
                "black",
            )
            .unwrap();
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
        rt.start_thread_for_test(a, "plant");
        let Value::Entity(first) = bar(&mut rt, a) else {
            panic!("no bar for the first attempt");
        };
        rt.start_thread_for_test(a, "abort");
        rt.start_thread_for_test(b, "plant");
        let Value::Entity(b_bar) = bar(&mut rt, b) else {
            panic!("no bar for the second attacker");
        };
        assert_eq!(b_bar.0, first.0, "the second bar did not reuse the record");
        rt.start_thread_for_test(a, "plant");
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
            takedamage: true,
        };
        for (name, s) in [("m1carbine_mp", 1), ("colt_mp", 3), ("fraggrenade_mp", 4)] {
            rt.host.precache_items_for_test(&[name]);
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

    /// One cmd's touch pass and use key, in `ClientThink_real`'s order.
    fn cmd(rt: &mut ScriptRuntime, slot: usize, buttons: u8, eye: [f32; 3], view: [f32; 3]) {
        let now = rt.host.level_time_ms;
        rt.touch_triggers(slot, now);
        rt.item_pass(slot, buttons, eye, view);
    }

    #[test]
    fn a_health_pack_underfoot_heals_and_leaves_the_wire() {
        let mut rt = pickup_rig();
        rt.host.client_vitals[0].health = 50;
        let id = rt.place_item("item_health", [0.0, 0.0, 1.0], 0);
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
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
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
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
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
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
        cmd(
            &mut rt,
            0,
            vcod_common::net::msg::BUTTON_USE,
            [0.0, 0.0, 60.0],
            DOWN,
        );
        rt.set_client_pm_type(0, 0);
        rt.host.client_vitals[0] = Vitals {
            health: 0,
            max_health: 100,
            dead: true,
            takedamage: true,
        };
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
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
            cmd(
                &mut rt,
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
    /// the first grab, in the area tree's order, fills the reserve and the
    /// second, reading the mirror the first moved, finds nothing to take.
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
        // The later link heads the list, so b is met first, not a.
        let walk = rt.host.area.entities_in_box([-50.0; 3], [50.0; 3], -1);
        assert_eq!(walk, [b.0, a.0]);
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
        assert!(rt.host.ents.get(b).unwrap().item.unwrap().taken);
        assert!(!rt.host.ents.get(a).unwrap().item.unwrap().taken);
        assert_eq!(rt.host.client_ammo[0].ammo[d.ammo_index], 320);
    }

    /// The swap with the use key: the carbine lands on the panzerfaust's own
    /// origin, carries its rounds and its dropper, and the `"trigger"` notify
    /// names the player and the carbine, in that order.
    /// A drop with an empty clip and a reserve still has rounds to take;
    /// only a count and a clip both written empty (-1) read as empty.
    #[test]
    fn a_weapon_item_with_a_reserve_behind_an_empty_clip_is_not_empty() {
        let mut rt = pickup_rig();
        let placed = rt.place_item("mpweapon_m1carbine", [0.0, 0.0, 30.0], 0);
        let reserve = rt.place_item("mpweapon_m1carbine", [100.0, 0.0, 30.0], 30);
        let empty = rt.place_item("mpweapon_m1carbine", [200.0, 0.0, 30.0], -1);
        for id in [reserve, empty] {
            rt.host
                .ents
                .get_mut(id)
                .unwrap()
                .item
                .as_mut()
                .unwrap()
                .clip = -1;
        }
        let items = rt.weapon_items();
        let at = |x: f32| items.iter().find(|i| i.1[0] == x).unwrap().2;
        assert!(!at(0.0), "{placed:?} unset counts are filled at the pickup");
        assert!(!at(100.0), "a reserve behind an empty clip");
        assert!(at(200.0));
    }

    #[test]
    fn the_use_key_swaps_and_the_trigger_notify_names_the_drop() {
        let mut rt = pickup_rig();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        rt.host.client_weapons[0].give(fg, 2);
        let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 30.0], 0);
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], [36.0, 0.0, 0.0]);
        cmd(
            &mut rt,
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
        assert!(
            rt.host
                .engine_notifies
                .iter()
                .any(|(id, ev, args)| *id == pf
                    && *ev == "trigger"
                    && args == &vec![Value::Entity(client), Value::Entity(drop)])
        );
        // `Cmd_Activate_f` notifies the item alone, never the player.
        assert!(
            !rt.host
                .engine_notifies
                .iter()
                .any(|(id, ev, _)| *id == client && *ev == "touch")
        );
        let panzerfaust = crate::configstrings::weapon_index("panzerfaust_mp").unwrap();
        assert!(
            rt.take_client_commands()
                .contains(&(0, format!("a {panzerfaust}")))
        );
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
        rt.host.configstrings.set(1212, "CGAME_USEMG42");
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
        // `G_SpawnTurret`'s link: the use key's walk lists it off the tree.
        let host = &mut rt.host;
        rt.vm.with_cx(|cx| host.link_turret(cx, gun));
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

    /// `G_FreeTurret`'s `TeleportPlayer` runs in the freeing thread (combat
    /// doc 14.7): the gunner is linked a unit above his mount spot and the
    /// script reads him there before his end frame applies the sim half.
    #[test]
    fn freeing_a_manned_gun_links_its_gunner_at_once() {
        use vcod_gsc::Host;
        let (mut rt, gun) = turret_rig();
        let rec = rt.host.turrets.get_mut(&gun).unwrap();
        rec.owner = Some(0);
        rec.busy = 1;
        rec.mount_origin = [10.0, 20.0, 0.0];
        let gunner_box = rt.host.client_box(0);
        rt.host
            .link_client(0, [30.0, 0.0, 0.0], gunner_box, 0x0200_0000, true);
        rt.host.free_entity(gun);
        let link = rt.host.area.last_link(0).expect("linked");
        assert_eq!(
            (link.absmin, link.contents),
            ([-6.0, 4.0, 0.0], 0x0200_0000),
            "a unit up, box and contents kept"
        );
        let me = rt.client_entity(0).unwrap();
        let host = &mut rt.host;
        let origin = rt.vm.with_cx(|cx| {
            let f = cx.intern_folded("origin");
            host.get_field(cx, me, f)
        });
        assert_eq!(origin, Value::Vector([10.0, 20.0, 1.0]));
        assert_eq!(rt.host.client_link_origin[0], [10.0, 20.0, 1.0]);
    }

    /// The server drains each cmd's mount before the next cmd's pass, so the
    /// second client's press in the same frame finds the gun manned.
    #[test]
    fn two_presses_on_one_gun_mount_only_the_first() {
        let (mut rt, gun) = turret_rig();
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(mount_queued(&mut rt, 0), 1);
        cmd(&mut rt, 1, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(mount_queued(&mut rt, 1), 0);
        assert_eq!(rt.host.turrets[&gun].owner, Some(0));
    }

    /// A gunner's press is the release request and nothing else, and a
    /// gunner reads hint 0 with the string left where the mount left it.
    #[test]
    fn a_gunners_use_press_asks_for_the_release_and_mounts_nothing() {
        let (mut rt, gun) = turret_rig();
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        mount_queued(&mut rt, 0);
        assert_eq!(rt.cursor_hint_pass(0, EYE, AT_GUN), (0, None));
        let fg42 = rt.place_item("mpweapon_fg42", [40.0, 0.0, 40.0], 90);
        cmd(&mut rt, 0, 0, EYE, AT_GUN);
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert!(rt.host.turret_ops.is_empty());
        assert_eq!(rt.host.turrets[&gun].busy, 2);
        assert!(!rt.host.ents.get(fg42).unwrap().item.unwrap().taken);
    }

    /// `Cmd_Activate_f` has no `pm_type` gate (turrets doc 4.1): a gunner
    /// past the touch pass's gate still asks for the release.
    #[test]
    fn a_gunners_press_asks_for_the_release_whatever_its_pm_type() {
        let (mut rt, gun) = turret_rig();
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        mount_queued(&mut rt, 0);
        cmd(&mut rt, 0, 0, EYE, AT_GUN);
        rt.set_client_pm_type(0, 6);
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_GUN);
        assert_eq!(rt.host.turrets[&gun].busy, 2);
    }

    const EYE: [f32; 3] = [0.0, 0.0, 60.0];
    const AT_ITEM: [f32; 3] = [36.0, 0.0, 0.0];

    /// `spawnTriggerMount`'s box from two unsorted corners.
    fn spawn_mount(rt: &mut ScriptRuntime, a: [f32; 3], b: [f32; 3], crouch: bool) -> EntId {
        let args = [
            Value::Vector(a),
            Value::Vector(b),
            Value::Int(i32::from(crouch)),
        ];
        let made = rt.with_host(|host, cx| {
            crate::game::builtins::uo::spawn_trigger_mount(host, cx, None, &args)
        });
        match made {
            Ok(Value::Entity(id)) => id,
            v => panic!("{v:?}"),
        }
    }

    /// `G_TouchTriggers` finds a mount through the area query's mask
    /// 0x405c0008 alone, so `setContents` takes it out of the touch pass
    /// or puts it back: retail UO's `probe_mounttouch` fired 57 times a
    /// second spawned (0x401c4008), 2 under 0x4000 and under 0, 55 under
    /// 0x400000 and under 0x40000000 (coduo-movement.md 11).
    #[test]
    fn set_contents_moves_a_mount_in_and_out_of_the_touch_pass() {
        let mut rt = pickup_rig();
        let mount = spawn_mount(&mut rt, [-40.0, -40.0, -10.0], [40.0, 40.0, 80.0], false);
        let touched = |rt: &mut ScriptRuntime, contents: Option<i32>| {
            rt.with_host(|host, cx| {
                if let Some(c) = contents {
                    let recv = Some(Target::Entity(mount));
                    crate::game::builtins::uo::set_contents(host, cx, recv, &[Value::Int(c)])
                        .unwrap();
                }
                crate::game::trigger::touched(host, cx, 0, [0.0; 3]).contains(&mount)
            })
        };
        assert!(touched(&mut rt, None));
        assert!(!touched(&mut rt, Some(0x4000)));
        assert!(touched(&mut rt, Some(0x40_0000)));
        assert!(!touched(&mut rt, Some(0)));
        assert!(touched(&mut rt, Some(0x4000_0000)));
        assert!(touched(&mut rt, Some(0x401c_4008)));
    }

    /// A held LMG hints 7 where its probe, 15 ahead and 1 up, is inside a
    /// script mount, 0 when it faces away or has the bipod down; the
    /// value is 0 off the mount's contents and the spawnflag only off the
    /// link's margin (coduo-movement.md 11).
    #[test]
    fn a_held_lmg_hints_7_at_a_script_mount() {
        let mut rt = pickup_rig();
        let away = [0.0, 180.0, 0.0];
        let stand = spawn_mount(&mut rt, [20.0, 5.0, 10.0], [10.0, -5.0, -10.0], false);
        assert_eq!(
            rt.with_host(|host, cx| {
                let c = cx.intern_folded("classname");
                match vcod_gsc::Host::get_field(host, cx, stand, c) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => format!("{v:?}"),
                }
            }),
            "trigger_mount"
        );
        let boxes = rt.mount_boxes();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].mins, glam::Vec3::new(10.0, -5.0, -10.0));
        assert_eq!(boxes[0].contents, 0x401c_4008);
        let lmg = |yaw_deg, sighted| LmgHint {
            origin: [0.0; 3],
            yaw_deg,
            sighted,
            hint_string: None,
            boxes: &boxes,
        };
        let at = |hint, val| CursorHint {
            hint,
            val,
            string: Some(-1),
        };
        assert_eq!(
            rt.cursor_hints(0, EYE, away, Some(lmg(0.0, false))),
            at(7, 0)
        );
        assert_eq!(
            rt.cursor_hints(0, EYE, away, Some(lmg(0.0, true))),
            at(0, 0)
        );
        assert_eq!(
            rt.cursor_hints(0, EYE, away, Some(lmg(180.0, false))),
            at(0, 0)
        );
        assert_eq!(rt.cursor_hints(0, EYE, away, None), at(0, 0));

        // A crouched one whose box stops half a unit short of the probe.
        let mut rt = pickup_rig();
        spawn_mount(&mut rt, [15.5, -5.0, -10.0], [25.0, 5.0, 10.0], true);
        let boxes = rt.mount_boxes();
        assert_eq!(boxes[0].contents, 0x405c_0008);
        let lmg = |x: f32| LmgHint {
            origin: [x, 0.0, 0.0],
            yaw_deg: 0.0,
            sighted: false,
            hint_string: None,
            boxes: &boxes,
        };
        assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(0.0))), at(7, 1));
        assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(1.0))), at(7, 0));
        assert_eq!(rt.cursor_hints(0, EYE, away, Some(lmg(-2.0))), at(0, 0));
    }

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
            takedamage: true,
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
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
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
        cmd(&mut rt, 0, vcod_common::net::msg::BUTTON_USE, EYE, AT_ITEM);
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
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
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
        rt.start_thread_for_test(id, "watch");
        // The loop's first `wait` from the load falls due on the first
        // frame and runs on the second; from there it polls every frame.
        rt.run_frame(50);
        cmd(&mut rt, 0, 0, [0.0, 0.0, 60.0], DOWN);
        rt.run_frame(100);
        rt.run_frame(150);
        let seen: Vec<&String> = rt
            .script_log()
            .iter()
            .filter(|l| l.starts_with("seen"))
            .collect();
        assert_eq!(seen, vec!["seen 100"]);
    }
}
