//! Loading a level's scripts: the pak source, the compile cache and its
//! carry between levels, the bootstrap.

use super::*;

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
    /// CoD2's parsed weapon files, for a level over the same paks.
    pub weapon_files: Option<crate::game::host::WeaponFiles>,
}

/// The two closures a level loaded, compiled, before any of it ran.
pub struct CompiledScripts {
    gametype_entry: String,
    entry: String,
    pub(super) program: vcod_gsc::Program,
    /// The `#using_animtree` names the closure declares.
    animtrees: Vec<String>,
    /// Every file the closure read, for `scriptUsage`'s source lines.
    pub(super) sources: vcod_gsc::LoaderState,
    /// The compile as it stood after the gametype's closure, which the
    /// next map's load starts from (`Carry::gametype`).
    pub(super) gametype: Rc<GametypeScripts>,
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

    pub(super) fn of(bsp: &MapWorld) -> LevelMap {
        LevelMap {
            model_bounds: bsp.models.iter().map(|m| m.clip_bounds()).collect(),
            model_brushes: crate::game::trigger::model_brush_hulls(bsp),
            entities: bsp.entities.clone(),
        }
    }
}

impl ScriptRuntime {
    /// Loads the gametype and map script closures, spawns the map's entities
    /// and runs the bootstrap. `map` and `gametype` are bare names, e.g.
    /// "mp_pavlov" and "dm". `configstrings` and `cvars` seed the host;
    /// `world`, once the caller has one, lets `bulletTrace` trace against the
    /// real map geometry, and `weapons` is what the weapon builtins read.
    #[expect(clippy::too_many_arguments)]
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
    #[expect(clippy::too_many_arguments)]
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
    /// tables, on the host's CoD2 profile (`crate::game::profile::COD2`).
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
    #[expect(clippy::too_many_arguments)]
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
    #[expect(clippy::too_many_arguments)]
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

    #[expect(clippy::too_many_arguments)]
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
    #[expect(clippy::too_many_arguments)]
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
                    sources: loader.state(),
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
        // below 1 a runtime error does not end its thread, and
        // `scriptUsage` has no sources (cod_lnxded 0x81613b0).
        let developer = host
            .cvars
            .get("developer")
            .trim()
            .parse::<i32>()
            .unwrap_or(0)
            != 0;
        vm.set_recover_errors(!developer);
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
        if let (Some(w), crate::game::host::HostExt::Cod2(c)) = (carry.weapon_files, &mut host.ext)
        {
            c.weapon_files = w;
        }
        if let Some(m) = carry.models {
            let fs = host.fs.as_ref().map(Rc::as_ptr);
            match &m.reload {
                None => {
                    host.xmodel_collision = m.collision;
                    host.xmodel_bounds = m.bounds;
                    host.vehicles.rigs = m.rigs;
                    host.xmodel_reads = m.reads;
                    host.xmodel_untouched = m.untouched;
                }
                Some(paks) if fs == Some(paks.as_ptr()) => {
                    host.xmodel_collision = m.collision;
                    host.xmodel_bounds = m.bounds;
                    host.xmodel_reads = m.reads;
                    host.xmodel_untouched = m.untouched;
                }
                Some(_) => {}
            }
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
        if profile.spawn.find_teams {
            vm.with_cx(|cx| crate::game::teams::find_teams(&mut host, cx));
        }
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
            developer,
            client_strings: HashMap::new(),
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
        // Both run with `level` as `self` (cod11-gsc-language.md, "Why:
        // the compiler types the bare globals"; `probe_usage`'s `self endon`).
        let level = Some(self.vm.level());
        let gametype_entry = self.gametype_entry.clone();
        self.start(&gametype_entry, "main", level)?;

        let entry = self.entry.clone();
        self.start(&entry, "main", level)?;

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
    pub(super) fn start(
        &mut self,
        path: &str,
        name: &str,
        recv: Option<Target>,
    ) -> anyhow::Result<()> {
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
}
