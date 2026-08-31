//! The stock `dm` bootstrap runs to completion on both gate maps.
//!
//! A thread that hits a missing builtin (or any other script error) dies
//! where it stands and the rest of the map keeps serving, which is retail's
//! behaviour and the right one — but it means half a bootstrap looks exactly
//! like a whole one from the outside. The VM only reported an abort through
//! `log::warn!` and the harness installs no logger, so every abort in every
//! test to date was swallowed; `Server::script_aborts` is the surface that
//! ends that, and this is the test that reads it.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use std::rc::Rc;

fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

/// Boots `map` on `dm` exactly as `configstrings_ab` does and returns the
/// aborts the bootstrap left behind.
fn aborts(map: &str, fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let fs = Rc::new(fs);
    let mut sv = vcod_server::server::Server::new(cfg(map), std::time::Instant::now());
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(fs).expect("load the scripts");
    sv.script_aborts()
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let aborts = aborts(map, fs);
    assert!(
        aborts.is_empty(),
        "{map}: {} bootstrap thread(s) aborted\n{}",
        aborts.len(),
        aborts.join("\n")
    );
}

#[test]
fn the_bootstrap_runs_to_completion_on_mp_pavlov() {
    check("mp_pavlov");
}

/// The second gate map: a different nationality branch and a much larger
/// precache set, so a builtin only one of the two closures calls still shows.
#[test]
fn the_bootstrap_runs_to_completion_on_mp_carentan() {
    check("mp_carentan");
}
