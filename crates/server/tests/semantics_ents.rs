//! `probe_ents` measures `getEntArray`'s return order, which needs real map
//! entities, so it runs here rather than in `crates/gsc`: the object model
//! lives in this crate and the entity lump comes from `vcod-common`.
//! `crates/gsc/tests/semantics_ab.rs` runs every other probe and asserts
//! this one is claimed here.
//!
//! Needs `COD_DIR`; without the paks it returns early, like every other
//! game-data test in the workspace.

use vcod_gsc::{Loader, ScriptSource, Value, Vm};
use vcod_server::game::host::GameHost;
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

/// The `# probe_ents` section of `retail-captures.txt`, parsed with the same
/// semantics `semantics_ab.rs::captures()` uses (`# name` starts a section,
/// `PROBE ` lines collect). `probe_ents` never dies on retail, so there is
/// no `PROBE_FATAL` case to reproduce here.
fn retail_probe_ents_lines() -> Vec<String> {
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/retail-captures.txt")
        .expect("read retail-captures.txt");
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            in_section = rest.trim() == "probe_ents";
        } else if in_section && line.starts_with("PROBE ") {
            lines.push(line.to_string());
        }
    }
    lines
}

/// Retail ran this on mp_pavlov with sv_maxclients 8. It spawns three
/// `script_origin` entities from `Callback_StartGameType`, names them, and
/// prints `getEntArray("script_origin", "classname")` in order and then each
/// entity's `getEntityNumber()`. The map's own four come first because their
/// entity numbers are lower (docs/research/cod11-gsc-object-model.md section
/// 10).
///
/// The order matches retail; the numbers do not, and this test fails on
/// them. vcod allocates an entity for every block with a classname, where
/// retail's `SP_misc_model` and `SP_light` free theirs, which on mp_pavlov
/// is 117 entities and puts the fourth `script_origin` at 415 here against
/// 298 on retail. The expectation is retail's capture and stays that way:
/// section 13 of the object-model doc has the measurement, and running the
/// `spawns` table is what closes it.
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

    assert_eq!(host.script_log, retail_probe_ents_lines());
}
