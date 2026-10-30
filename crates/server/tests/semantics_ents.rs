//! `probe_ents` measures `getEntArray`'s return order, which needs real map
//! entities, so it runs here rather than in `crates/gsc`: the object model
//! lives in this crate and the entity lump comes from `vcod-common`.
//! `crates/gsc/tests/semantics_ab.rs` runs every other probe and asserts
//! this one is claimed here.
//!
//! `probe_delete` is claimed here too, for the same reason: it measures the
//! deferred-free window with `delete()` and `getEntArray()` on real spawned
//! entities, which needs this crate's object model. Naming it here also
//! satisfies `semantics_ab.rs`'s `every_probe_file_and_capture_section_are_paired`
//! guard, which checks this file's text for the name.
//!
//! `probe_cvar` and `probe_not_string` are claimed here for a different
//! reason: they call `getCvar`/`setCvar`/`getCvarInt`/`getCvarFloat`/
//! `getTime`/`randomInt`, which need the real `Cvars` table this crate
//! owns; `crates/gsc`'s `ProbeHost` answers only `logPrint`/`isDefined` and
//! must not grow a fake cvar table just to run these two. Unlike
//! `probe_ents`/`probe_delete`, they need no map data, so they run against
//! a bare `GameHost::new`.
//!
//! `probe_bootstrap` is claimed here for a third reason: it orders the two
//! `main()`s by reading `game["allies"]`, which only the real
//! `mp_pavlov.gsc` sets, so its `ScriptSource` has to be the pak-backed one
//! with the probe overlaid on the gametype path. `crates/gsc`'s
//! `ProbeSource` stubs every other file out and cannot supply that.
//!
//! The seven `probe_stale_*` probes are claimed here for `probe_delete`'s
//! reason without its map: what a handle reads once `delete()` or
//! `destroy()` has freed its object needs this crate's object table and its
//! deferred free, and nothing in them depends on the map's own entities.
//!
//! `probe_placespawn` is claimed here: it drops the spawnpoints of all
//! twelve stock maps, which `placespawn_ab.rs` gates to the bit through
//! `client-probes/probe_placebits` instead.
//!
//! `probe_loadfx_wait` is claimed here for `probe_bootstrap`'s reason: it
//! loads effects on both sides of `level.initializing`, which only the real
//! bootstrap sets and clears.
//!
//! The `probe_precache_*` probes are claimed here for the same reason, and
//! so are the `probe_late_*` probes: `probe_late_attach`,
//! `probe_late_index`, `probe_late_label`, `probe_late_objective`,
//! `probe_late_player`, `probe_late_setmodel`, `probe_late_setshader`,
//! `probe_late_settext`, `probe_late_shellshock`, `probe_late_shockspan`,
//! `probe_late_statusicon` and `probe_late_viewmodel`.
//!
//! The `probe_attach_*` probes and `probe_detach_missing` are claimed here
//! for the object table's attachment slots, and `probe_testclient` for the
//! client `addTestClient` connects, which `testclient.rs` runs on a whole
//! server: its spawn reads the map's collision.
//!
//! `probe_ents`, `probe_delete`, `probe_bootstrap`, `probe_loadfx_wait`,
//! the `probe_precache_*` and the `probe_late_*` probes need `COD_DIR`;
//! without the paks they return early, like every other game-data test in
//! the workspace. `probe_cvar` and `probe_not_string` need no game data and
//! always run.

use std::rc::Rc;
use vcod_common::asset_source::{AssetSource, RawFile};

use vcod_gsc::{FuncRef, Loader, ScriptSource, Value, Vm};
use vcod_server::cvars::Cvars;
use vcod_server::game::host::{ClientEvent, GameHost};
use vcod_server::game::script::{Carry, ScriptRuntime};
use vcod_server::game::spawn::spawn_entities_from_string;

/// A probe's file plus a stub for the one script it calls across files.
/// Copy of `semantics_ab.rs`'s `ProbeSource`: a test in `crates/gsc` cannot
/// be imported from `crates/server`.
struct ProbeSource {
    path: String,
    text: String,
}

impl ScriptSource for ProbeSource {
    fn read(&self, canonical: &str) -> Option<String> {
        if canonical == self.path {
            return Some(self.text.clone());
        }
        if canonical == "maps/mp/gametypes/_callbacksetup" {
            return Some("SetupCallbacks() {}\n".to_string());
        }
        None
    }
}

/// The `# <name>` section of `retail-captures.txt`, parsed with the same
/// semantics `semantics_ab.rs::captures()` uses (`# name` starts a section,
/// `PROBE ` lines collect). `retail_probe_died` reads the `PROBE_FATAL`
/// half.
fn retail_probe_lines(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/retail-captures.txt")
        .expect("read retail-captures.txt");
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            in_section = rest.trim() == name;
        } else if in_section && line.starts_with("PROBE ") {
            lines.push(line.to_string());
        }
    }
    lines
}

/// `tools/run_probe.sh`'s `developer 1`, under which retail's captures
/// were taken: a runtime error ends its thread.
fn probe_cvars() -> Cvars {
    let mut c = Cvars::new();
    c.set("developer", "1");
    c
}

/// Loads `name`'s probe fresh, mirroring `semantics_ab.rs::install`; a
/// test in `crates/gsc` cannot be imported from `crates/server`.
fn install(name: &str) -> (Vm, FuncRef) {
    let path = format!("maps/mp/gametypes/{name}");
    let text = std::fs::read_to_string(format!("../gsc/tests/fixtures/semantics/{name}.gsc"))
        .unwrap_or_else(|e| panic!("read {name}.gsc: {e}"));
    let mut vm = Vm::new();
    let mut loader = Loader::new(Box::new(ProbeSource {
        path: path.clone(),
        text,
    }));
    loader
        .load(&mut vm, &path)
        .unwrap_or_else(|e| panic!("{name} does not load: {e:?}"));
    let main = vm.func_ref(&path, "main");
    (vm, main)
}

/// Retail ran this on mp_pavlov with sv_maxclients 8. It spawns three
/// `script_origin` entities from `Callback_StartGameType`, names them, and
/// prints `getEntArray("script_origin", "classname")` in order and then each
/// entity's `getEntityNumber()`. The map's own four come first because their
/// entity numbers are lower (docs/research/cod11-gsc-object-model.md section
/// 10).
///
/// The jump from 75 to 298 is what makes this a gate rather than a
/// formality: mp_pavlov's 117 `light` and `misc_model` blocks sit between
/// those two `script_origin`s and consume no entity number, because their
/// `SP_` function frees the entity and `G_Spawn` hands the slot back out
/// (docs/research/cod11-gsc-object-model.md sections 13 and 14).
#[test]
fn probe_ents_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };

    let path = "maps/mp/gametypes/probe_ents".to_string();
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/probe_ents.gsc")
        .expect("read probe_ents.gsc");
    let mut vm = Vm::new();
    let mut loader = Loader::new(Box::new(ProbeSource {
        path: path.clone(),
        text,
    }));
    loader
        .load(&mut vm, &path)
        .unwrap_or_else(|e| panic!("probe_ents does not load: {e:?}"));

    let mut host = GameHost::new(
        &vcod_server::game::profile::COD1_11,
        vec![String::new(); 2048],
    );
    let bsp_path = fs
        .resolve_map("mp_pavlov")
        .expect("mp_pavlov.bsp in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read mp_pavlov.bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse mp_pavlov.bsp");
    vm.with_cx(|cx| spawn_entities_from_string(&mut host, cx, &bsp.entities))
        .expect("spawn mp_pavlov's entities");

    // `main` only stores the callbacks; the engine calls
    // `level.callbackStartGameType` itself, so the test does too.
    let main = vm.func_ref(&path, "main");
    vm.call_now(&mut host, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_ents main errored: {e:?}"));

    let level = vm.level_id();
    let callback = vm.with_cx(|cx| {
        let field = cx.intern_folded("callbackstartgametype");
        cx.get_field(level, field)
    });
    let Value::Function(callback) = callback else {
        panic!("level.callbackStartGameType is {callback:?}, not a function");
    };
    vm.call_now(&mut host, callback, None, Vec::new())
        .unwrap_or_else(|e| panic!("Callback_StartGameType errored: {e:?}"));

    assert_eq!(host.script_log, retail_probe_lines("probe_ents"));
}

/// Retail ran this on mp_pavlov with sv_maxclients 8. `Callback_StartGameType`
/// spawns two `script_origin`s, deletes one immediately and checks
/// `getEntArray`/`getEntityNumber` right away, then again after a `wait
/// 0.15`; a second delete-then-wait pair follows. Two `wait`s means the
/// callback suspends, so it needs `start_thread` plus stepped frames rather
/// than `probe_ents`' single `call_now` — the same fallback
/// `crates/gsc/tests/semantics_ab.rs::run_probe` uses for a probe built
/// around `wait`. Each stepped frame runs the VM step before the entity
/// think pass, the order `ScriptRuntime::run_frame` uses, so a deferred
/// `delete()` frees on the same schedule production code would.
///
/// This only bounds `DELETE_DEFER_MS`, it does not pin it: the 50 ms frame
/// step and the 150 ms `wait`s mean a range of defers frees each deleted
/// entity in time to match retail's post-wait counts. `probe_entnum` pins
/// 100 (`a_delete_s_free_runs_after_the_threads_of_its_frame` in
/// `crate::game::script`).
#[test]
fn probe_delete_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };

    let path = "maps/mp/gametypes/probe_delete".to_string();
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/probe_delete.gsc")
        .expect("read probe_delete.gsc");
    let mut vm = Vm::new();
    let mut loader = Loader::new(Box::new(ProbeSource {
        path: path.clone(),
        text,
    }));
    loader
        .load(&mut vm, &path)
        .unwrap_or_else(|e| panic!("probe_delete does not load: {e:?}"));

    let mut host = GameHost::new(
        &vcod_server::game::profile::COD1_11,
        vec![String::new(); 2048],
    );
    let bsp_path = fs
        .resolve_map("mp_pavlov")
        .expect("mp_pavlov.bsp in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read mp_pavlov.bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse mp_pavlov.bsp");
    vm.with_cx(|cx| spawn_entities_from_string(&mut host, cx, &bsp.entities))
        .expect("spawn mp_pavlov's entities");

    // `main` only stores the callbacks; the engine calls
    // `level.callbackStartGameType` itself, so the test does too.
    let main = vm.func_ref(&path, "main");
    vm.call_now(&mut host, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_delete main errored: {e:?}"));

    let level = vm.level_id();
    let callback = vm.with_cx(|cx| {
        let field = cx.intern_folded("callbackstartgametype");
        cx.get_field(level, field)
    });
    let Value::Function(callback) = callback else {
        panic!("level.callbackStartGameType is {callback:?}, not a function");
    };

    vm.start_thread(&mut host, callback, None, Vec::new());
    for frame in 1..=12 {
        let now_ms = frame * 50;
        host.level_time_ms = now_ms;
        if let Some(e) = vm.run_frame(&mut host, now_ms).into_iter().next() {
            panic!("probe_delete Callback_StartGameType errored: {e:?}");
        }
        vm.with_cx(|cx| host.run_entity_thinks(cx, now_ms));
    }

    assert_eq!(host.script_log, retail_probe_lines("probe_delete"));
}

/// `probe_cvar` measures `getCvar`/`setCvar`/`getCvarInt`/`getCvarFloat`/
/// `getTime`/`randomInt` against retail. It spawns no entities and reads no
/// map data — every cvar it touches it also registers itself — so a bare
/// `GameHost` reproduces it.
#[test]
fn probe_cvar_matches_retail() {
    let (mut vm, main) = install("probe_cvar");
    let mut host = GameHost::new(
        &vcod_server::game::profile::COD1_11,
        vec![String::new(); 2048],
    );
    vm.call_now(&mut host, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_cvar main errored: {e:?}"));

    assert_eq!(host.script_log, retail_probe_lines("probe_cvar"));
}

/// `probe_not_string` measures unary `!` on a `getCvar` read.
/// `not_getcvar_allow_fg42` came back `1` on retail, which needs
/// `scr_allow_fg42` to read `"0"` there, not unset. The retail capture has
/// it at slot 164/228 as `"0"` while every other `scr_allow_*` is `"1"`
/// (`crates/server/src/cvars.rs`'s `STOCK_SCRIPT_CVARS`), and Activision's
/// own `_teams::initGlobalCvars()` passes `"1"` as fg42's default too, so
/// that call is not what produced the `"0"`: `makeCvarServerInfo` never
/// overwrites an existing value. What did is `default_mp.cfg`, which the
/// engine execs at startup and whose line 95 is `set scr_allow_fg42 0`
/// (docs/research/cod11-gsc-object-model.md section 18). This isolated
/// probe runs no bootstrap and mounts no paks, so the test seeds the
/// measured value directly to reproduce retail's state.
#[test]
fn probe_not_string_matches_retail() {
    let (mut vm, main) = install("probe_not_string");
    let mut host = GameHost::new(
        &vcod_server::game::profile::COD1_11,
        vec![String::new(); 2048],
    );
    host.cvars.set("scr_allow_fg42", "0");
    vm.call_now(&mut host, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_not_string main errored: {e:?}"));

    assert_eq!(host.script_log, retail_probe_lines("probe_not_string"));
}

/// Runs `name`'s `main` and then its `Callback_StartGameType` on a bare
/// `GameHost`, stepping 50 ms frames with the entity think pass after
/// each, the order `probe_delete_matches_retail` uses. The callback goes
/// through `call_now` first, since that is the path that returns an error
/// the probe dies on before its first `wait`, and is rerun fresh through
/// `start_thread` if it suspends. Returns the log and whether a thread died.
fn run_stale_probe(name: &str) -> (Vec<String>, bool) {
    let run = |threaded: bool| {
        let (mut vm, main) = install(name);
        let mut host = GameHost::new(
            &vcod_server::game::profile::COD1_11,
            vec![String::new(); 2048],
        );
        vm.call_now(&mut host, main, None, Vec::new())
            .unwrap_or_else(|e| panic!("{name} main errored: {e:?}"));
        let level = vm.level_id();
        let callback = vm.with_cx(|cx| {
            let field = cx.intern_folded("callbackstartgametype");
            cx.get_field(level, field)
        });
        let Value::Function(callback) = callback else {
            panic!("level.callbackStartGameType is {callback:?}, not a function");
        };
        if !threaded {
            let r = vm.call_now(&mut host, callback, None, Vec::new());
            return (host.script_log, r.err().map(|e| e.kind));
        }
        let mut died = None;
        vm.start_thread(&mut host, callback, None, Vec::new());
        for frame in 1..=12 {
            let now_ms = frame * 50;
            host.level_time_ms = now_ms;
            if let Some(e) = vm.run_frame(&mut host, now_ms).into_iter().next() {
                died.get_or_insert(e.kind);
            }
            vm.with_cx(|cx| host.run_entity_thinks(cx, now_ms));
        }
        (host.script_log, died)
    };
    let (log, err) = match run(false) {
        (_, Some(vcod_gsc::ErrorKind::SuspendedInImmediateCall)) => run(true),
        r => r,
    };
    (log, err.is_some())
}

/// Whether retail's section for `name` ends in a `PROBE_FATAL` line.
fn retail_probe_died(name: &str) -> bool {
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/retail-captures.txt")
        .expect("read retail-captures.txt");
    let mut in_section = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            in_section = rest.trim() == name;
        } else if in_section && line.starts_with("PROBE_FATAL") {
            return true;
        }
    }
    false
}

/// A handle outlives its object on retail as a dead entity: live through
/// `delete()`'s 100 ms window, then `isDefined` 0 and unequal to whatever
/// takes the slot, equal only to its own copies, and fatal to a field read,
/// a field write or a method call. A destroyed hudelem is dead at once. The
/// six fatal cases are one probe each because each kills the retail server.
#[test]
fn probe_stale_handles_match_retail() {
    for name in [
        "probe_stale_handle",
        "probe_stale_ent_method",
        "probe_stale_ent_read",
        "probe_stale_ent_write",
        "probe_stale_hud_method",
        "probe_stale_hud_read",
        "probe_stale_hud_write",
        "probe_stale_self",
        "probe_stale_self_hud",
    ] {
        let (ours, died) = run_stale_probe(name);
        assert_eq!(ours, retail_probe_lines(name), "{name}");
        assert_eq!(died, retail_probe_died(name), "{name}: died");
    }
}

/// The pak-backed source with one probe overlaid on its gametype path, so
/// the probe's own `main` runs beside the real `mp_pavlov.gsc` closure
/// rather than beside stubs. `PakScripts` in `crates/server/src/game/
/// script.rs` is the half this borrows; a private type there cannot be
/// imported into an integration test.
struct PakWithProbe {
    fs: Rc<dyn AssetSource>,
    path: String,
    text: String,
}

impl ScriptSource for PakWithProbe {
    fn read(&self, canonical: &str) -> Option<String> {
        if canonical == self.path {
            return Some(self.text.clone());
        }
        let raw = self.fs.load::<RawFile>(&format!("{canonical}.gsc")).ok()?;
        Some(raw.text().into_owned())
    }
}

/// Retail ran this on mp_pavlov as the gametype script. It reads
/// `game["allies"]` from its own `main` and again from
/// `Callback_StartGameType`; only `mp_pavlov.gsc`'s `main` sets that key, so
/// the pair orders the two `main()`s, and `bootstrap_thread_ran_inline` says
/// a bare `thread f()` runs its target to the first suspend before the
/// caller's next statement.
///
/// This runs the production bootstrap rather than a copy of it: the probe is
/// the gametype `ScriptRuntime::load_from` loads, so swapping the two
/// `start_thread` calls in `start_bootstrap` fails here. Nothing else pins
/// the order — `dm.gsc`'s `main` never reads `game["allies"]` and its
/// `Callback_StartGameType` defaults the key when unset, so the configstring
/// assertions in `script.rs` come out the same either way.
#[test]
fn probe_bootstrap_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fs = Rc::new(fs);

    let path = "maps/mp/gametypes/probe_bootstrap".to_string();
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/probe_bootstrap.gsc")
        .expect("read probe_bootstrap.gsc");
    let rt = ScriptRuntime::load_from(
        Box::new(PakWithProbe {
            fs: fs.clone(),
            path,
            text,
        }),
        fs,
        "mp_pavlov",
        "probe_bootstrap",
        vec![String::new(); 2048],
        probe_cvars(),
        None,
        Rc::new(vcod_server::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    )
    .expect("load mp_pavlov on probe_bootstrap");

    assert_eq!(rt.script_log(), retail_probe_lines("probe_bootstrap"));
}

/// The three `probe_persist_*` probes measure what a level boundary leaves
/// behind: `game[]`, `level` and an entity handle held in `game[]`, across
/// `map_restart(true)`, `exitLevel(false)` and `exitLevel(true)`. Nothing
/// short of the whole server reproduces that — the two builtins queue a
/// console line, `engine::tick` drains it, and the level that comes back is
/// what prints the after-half — so they are claimed here rather than in
/// `crates/gsc`, where the VM has no host that can restart a level.
///
/// Each runs as the gametype script of mp_pavlov, overlaid on the paks the
/// way `run_probe.sh` drops it in as one. The `exitLevel` pair needs the
/// same `sv_mapRotation` the retail recipe gave them
/// (`crates/gsc/tests/fixtures/semantics/README.md`), since `ExitLevel`
/// queues `map_rotate` and the rotation is what names the map to load next;
/// `map mp_pavlov` on mp_pavlov is the restart path
/// (docs/research/cod11-map-cycle.md section 4.2), which is what retail did
/// for them too.
///
/// A new level starts a new `logPrint` log, so the collector flushes on the
/// serverId moving rather than on the log growing: the after-half is longer
/// than the before-half, and a length test would miss the boundary.
fn run_persist_probe(name: &str, rotation: Option<&str>) -> Vec<String> {
    let fs = vcod_common::testing::game_fs_11().expect("game_fs_11, checked by the caller");
    let text = std::fs::read_to_string(format!("../gsc/tests/fixtures/semantics/{name}.gsc"))
        .unwrap_or_else(|e| panic!("read {name}.gsc: {e}"));

    let mut now = std::time::Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: "mp_pavlov".into(),
        hostname: "vcod probe".into(),
        max_clients: 8,
        gametype: name.into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    sv.overlay_script(&format!("maps/mp/gametypes/{name}"), &text);
    if let Some(r) = rotation {
        sv.set_cvar("sv_mapRotation", r);
    }
    sv.load_scripts(Rc::new(fs))
        .unwrap_or_else(|e| panic!("load mp_pavlov on {name}: {e:#}"));

    // `run_probe.sh` greps `games_mp.log` for `PROBE ` lines, so the retail
    // side of this comparison carries none of the engine's own log lines and
    // ours must not either: `ExitLevel` writes one (map-cycle doc, section 2).
    let probe_lines = |log: &[String]| -> Vec<String> {
        log.iter()
            .filter(|l| l.starts_with("PROBE "))
            .cloned()
            .collect()
    };
    let mut out: Vec<String> = probe_lines(sv.script_log());
    let mut seen = sv.script_log().len();
    let mut id = sv.server_id();
    for _ in 0..300 {
        now += std::time::Duration::from_millis(50);
        sv.tick(now);
        if sv.server_id() != id {
            id = sv.server_id();
            seen = 0;
        }
        let log = sv.script_log();
        out.extend(probe_lines(&log[seen.min(log.len())..]));
        seen = log.len();
        if out.iter().any(|l| l.starts_with("PROBE after_pass")) {
            break;
        }
    }
    out
}

/// `map_restart(true)`: `game[]` survives, `level` does not, and the entity
/// handle `game["ent"]` held reads undefined on the far side.
#[test]
fn probe_persist_restart_matches_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let ours = run_persist_probe("probe_persist_restart", None);
    assert_eq!(ours, retail_probe_lines("probe_persist_restart"));
}

/// `exitLevel(false)`: `game[]` is freed with everything else.
#[test]
fn probe_persist_exit_matches_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let ours = run_persist_probe(
        "probe_persist_exit",
        Some("gametype probe_persist_exit map mp_pavlov map mp_pavlov"),
    );
    assert_eq!(ours, retail_probe_lines("probe_persist_exit"));
}

/// A script's `setCvar("sv_hostname")` outlives `map_restart(false)` and a
/// map change: nothing re-stamps the startup value at a load (map-cycle doc
/// 4.5).
#[test]
fn probe_hostname_matches_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let ours = run_persist_probe(
        "probe_hostname",
        Some("gametype probe_hostname map mp_brecourt"),
    );
    assert_eq!(ours, retail_probe_lines("probe_hostname"));
}

/// `exitLevel(true)`: the same end of level, keeping `game[]`.
#[test]
fn probe_persist_exit_save_matches_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let ours = run_persist_probe(
        "probe_persist_exit_save",
        Some("gametype probe_persist_exit_save map mp_pavlov map mp_pavlov"),
    );
    assert_eq!(ours, retail_probe_lines("probe_persist_exit_save"));
}

/// `probe_loadfx_wait`: `loadFx` runs under `level.initializing`, which
/// `G_InitGame` clears once `Callback_StartGameType` reaches its first
/// wait. Past it an effect already loaded is still found, in any case, and
/// a new one is a script error that ends the thread before its log line.
/// Retail ran it on mp_pavlov as the gametype; the map's own `main` is the
/// pak's, so this needs `COD_DIR`.
#[test]
fn probe_loadfx_wait_matches_retail() {
    let Some(ours) = run_initializing_probe("probe_loadfx_wait") else {
        return;
    };
    assert_eq!(ours, retail_probe_lines("probe_loadfx_wait"));
    assert!(retail_probe_died("probe_loadfx_wait"));
}

/// The `probe_precache_*` family: the other precache builtins around the
/// same `level.initializing` (docs/research/cod11-gsc-language.md,
/// "Precache builtins"). Each fatal case is its own probe because each
/// kills the retail server; ours ends the thread, so the lines before the
/// error match and the ones after it are missing on both sides.
#[test]
fn probe_precache_wait_matches_retail() {
    for name in [
        "probe_precache_empty",
        "probe_precache_icon_full",
        "probe_precache_unknown_item",
        "probe_precache_wait",
        "probe_precache_wait_item",
        "probe_precache_wait_shader",
        "probe_precache_wait_shellshock",
        "probe_precache_wait_string",
    ] {
        let Some(ours) = run_initializing_probe(name) else {
            return;
        };
        assert_eq!(ours, retail_probe_lines(name), "{name}");
        assert!(retail_probe_died(name), "{name}");
    }
}

/// The `probe_late_*` family: the builtins and fields that index a
/// configstring at run time, past `level.initializing`
/// (docs/research/cod11-gsc-language.md 9, "Indexing past the first wait").
/// A name precached in another case is found and reads back in its
/// precached spelling; a new one is the indexer's script error. The
/// player-scoped ones run in a connect callback: retail's probe adds a test
/// client, ours connects one before the probe would.
#[test]
fn probe_late_index_matches_retail() {
    for (name, player) in [
        ("probe_late_index", false),
        ("probe_late_attach", false),
        ("probe_late_label", false),
        ("probe_late_objective", false),
        ("probe_late_setmodel", false),
        ("probe_late_setshader", false),
        ("probe_late_settext", false),
        ("probe_late_player", true),
        ("probe_late_shellshock", true),
        ("probe_late_shockspan", true),
        ("probe_late_statusicon", true),
        ("probe_late_viewmodel", true),
    ] {
        let Some(ours) = run_probe_with(name, player) else {
            return;
        };
        assert_eq!(ours, retail_probe_lines(name), "{name}");
        let fatal = !matches!(name, "probe_late_index" | "probe_late_player");
        assert_eq!(retail_probe_died(name), fatal, "{name}");
    }
}

/// `attach`'s six slots, the error on a pair already attached, the
/// compaction behind a `detach`, the lower-cased tag and the "bad index"
/// readers (docs/research/cod11-gsc-object-model.md 24).
#[test]
fn probe_attach_matches_retail() {
    for name in [
        "probe_attach_badindex",
        "probe_attach_badtag",
        "probe_attach_slots",
        "probe_attach_twice",
        "probe_attach_twice_notag",
        "probe_detach_missing",
    ] {
        let Some(ours) = run_initializing_probe(name) else {
            return;
        };
        assert_eq!(ours, retail_probe_lines(name), "{name}");
        assert!(retail_probe_died(name), "{name}");
    }
}

/// Runs `name` as mp_pavlov's gametype through the real bootstrap for four
/// frames and returns its `PROBE` lines; `None` without `COD_DIR`.
fn run_initializing_probe(name: &str) -> Option<Vec<String>> {
    run_probe_with(name, false)
}

/// [`run_initializing_probe`], with a client connecting on the second frame
/// when `player` is set.
fn run_probe_with(name: &str, player: bool) -> Option<Vec<String>> {
    let fs = Rc::new(vcod_common::testing::game_fs_11()?);
    let path = format!("maps/mp/gametypes/{name}");
    let text = std::fs::read_to_string(format!("../gsc/tests/fixtures/semantics/{name}.gsc"))
        .unwrap_or_else(|e| panic!("read {name}.gsc: {e}"));
    let mut rt = ScriptRuntime::load_from(
        Box::new(PakWithProbe {
            fs: fs.clone(),
            path,
            text,
        }),
        fs,
        "mp_pavlov",
        name,
        vec![String::new(); 2048],
        probe_cvars(),
        None,
        Rc::new(vcod_server::weapons::WeaponTable::empty()),
        0,
        Carry::default(),
    )
    .unwrap_or_else(|e| panic!("load mp_pavlov on {name}: {e:?}"));
    for frame in 1..=4 {
        if player && frame == 2 {
            rt.push_client_event(ClientEvent::Connect {
                slot: 0,
                name: "probe".into(),
            });
        }
        rt.run_frame(frame * 50);
    }
    Some(
        rt.script_log()
            .iter()
            .filter(|l| l.starts_with("PROBE "))
            .cloned()
            .collect(),
    )
}
