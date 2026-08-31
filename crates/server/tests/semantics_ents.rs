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
//! `probe_ents`, `probe_delete` and `probe_bootstrap` need `COD_DIR`;
//! without the paks they return early, like every other game-data test in
//! the workspace. `probe_cvar` and `probe_not_string` need no game data and
//! always run.

use std::rc::Rc;

use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{FuncRef, Loader, ScriptSource, Value, Vm};
use vcod_server::cvars::Cvars;
use vcod_server::game::host::GameHost;
use vcod_server::game::script::ScriptRuntime;
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
/// `PROBE ` lines collect). None of the three probes claimed here die on
/// retail, so there is no `PROBE_FATAL` case to reproduce.
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
    let Some(fs) = vcod_common::testing::game_fs() else {
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

    let mut host = GameHost::new(vec![String::new(); 2048]);
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
    vm.call_now(&mut host, 0, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_ents main errored: {e:?}"));

    let level = vm.level_id();
    let callback = vm.with_cx(|cx| {
        let field = cx.intern_folded("callbackstartgametype");
        cx.get_field(level, field)
    });
    let Value::Function(callback) = callback else {
        panic!("level.callbackStartGameType is {callback:?}, not a function");
    };
    vm.call_now(&mut host, 0, callback, None, Vec::new())
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
/// around `wait`. Each stepped frame runs the entity think pass before the
/// VM step, the order `ScriptRuntime::run_frame` uses, so a deferred
/// `delete()` frees on the same schedule production code would.
///
/// This only bounds `DELETE_DEFER_MS`, it does not pin it: the 50 ms frame
/// step and the 150 ms `wait`s mean any defer in (0, 150] ms frees each
/// deleted entity in time to match retail's post-wait counts, so this test
/// would pass unchanged at 1, 50 or 150 too. The exact 100 ms figure rests
/// on the disassembly citation in `DELETE_DEFER_MS`'s own comment, not on
/// this test.
#[test]
fn probe_delete_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
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

    let mut host = GameHost::new(vec![String::new(); 2048]);
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
    vm.call_now(&mut host, 0, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_delete main errored: {e:?}"));

    let level = vm.level_id();
    let callback = vm.with_cx(|cx| {
        let field = cx.intern_folded("callbackstartgametype");
        cx.get_field(level, field)
    });
    let Value::Function(callback) = callback else {
        panic!("level.callbackStartGameType is {callback:?}, not a function");
    };

    vm.start_thread(&mut host, 0, callback, None, Vec::new());
    for frame in 1..=12 {
        let now_ms = frame * 50;
        host.level_time_ms = now_ms;
        host.ents.run_thinks(now_ms);
        if let Some(e) = vm.run_frame(&mut host, now_ms).into_iter().next() {
            panic!("probe_delete Callback_StartGameType errored: {e:?}");
        }
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
    let mut host = GameHost::new(vec![String::new(); 2048]);
    vm.call_now(&mut host, 0, main, None, Vec::new())
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
    let mut host = GameHost::new(vec![String::new(); 2048]);
    host.cvars.set("scr_allow_fg42", "0");
    vm.call_now(&mut host, 0, main, None, Vec::new())
        .unwrap_or_else(|e| panic!("probe_not_string main errored: {e:?}"));

    assert_eq!(host.script_log, retail_probe_lines("probe_not_string"));
}

/// The pak-backed source with one probe overlaid on its gametype path, so
/// the probe's own `main` runs beside the real `mp_pavlov.gsc` closure
/// rather than beside stubs. `PakScripts` in `crates/server/src/game/
/// script.rs` is the half this borrows; a private type there cannot be
/// imported into an integration test.
struct PakWithProbe {
    fs: Rc<Pk3Fs>,
    path: String,
    text: String,
}

impl ScriptSource for PakWithProbe {
    fn read(&self, canonical: &str) -> Option<String> {
        if canonical == self.path {
            return Some(self.text.clone());
        }
        let bytes = self.fs.read(&format!("{canonical}.gsc"))?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
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
    let Some(fs) = vcod_common::testing::game_fs() else {
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
        Cvars::new(),
        None,
        0,
    )
    .expect("load mp_pavlov on probe_bootstrap");

    assert_eq!(rt.script_log(), retail_probe_lines("probe_bootstrap"));
}
