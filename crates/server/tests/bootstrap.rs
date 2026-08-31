//! The stock `dm` bootstrap runs to completion on both gate maps, and so does
//! everything a joining client sets running.
//!
//! A thread that hits a missing builtin (or any other script error) dies
//! where it stands and the rest of the map keeps serving, which is retail's
//! behaviour and the right one — but it means half a bootstrap looks exactly
//! like a whole one from the outside. The VM only reported an abort through
//! `log::warn!` and the harness installs no logger, so every abort in every
//! test to date was swallowed; `Server::script_aborts` is the surface that
//! ends that, and this is the test that reads it.
//!
//! Map load is the smaller half. `Callback_PlayerConnect` and everything the
//! two menu answers reach run on threads nothing else watches, and the
//! playerstate gate cannot see one die: a field it never sets written by a
//! thread that never ran reads the same as one the diff already tolerates.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;
use vcod_common::net::protocol::PROTOCOL_V1;

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

/// A server with `map` loaded on `dm`, exactly as `configstrings_ab` boots one.
fn booted(map: &str, fs: vcod_common::pk3::Pk3Fs, now: Instant) -> vcod_server::Server {
    let fs = Rc::new(fs);
    let mut sv = vcod_server::server::Server::new(cfg(map), now);
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(fs).expect("load the scripts");
    sv
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let aborts = booted(map, fs, Instant::now()).script_aborts();
    assert!(
        aborts.is_empty(),
        "{map}: {} bootstrap thread(s) aborted\n{}",
        aborts.len(),
        aborts.join("\n")
    );
}

/// The same assertion over the client path: connect, `begin`, both stock menu
/// answers and the spawn they end in. The join asks for the team and weapon
/// the retail capture was taken with, so this and the playerstate gate drive
/// the same one.
fn check_join(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let mut now = Instant::now();
    let mut sv = booted(map, fs, now);
    let q = Rc::new(RefCell::new(Queues::default()));
    let (team, weapon) = common::captured_join(map, &cfg(map).gametype);
    let (cl, join) = common::join(&mut sv, &q, &mut now, &team, &weapon);

    // Without this the assertion below could pass on a client that never
    // spawned, which is the shape the abort surface exists to catch: an
    // aborted thread and an unimplemented one look the same from here.
    assert!(
        join.findings().is_empty(),
        "{map}: {}\n{}",
        join.summary(),
        join.findings().join("\n")
    );
    let ps = cl
        .snapshots()
        .newest()
        .map(|s| s.ps.clone())
        .expect("the server sent no snapshot at all");
    assert_eq!(
        ps.field_i32(&PROTOCOL_V1, "pm_type"),
        0,
        "{map}: the client answered both menus but is not on the player \
         movement path, so the spawn did not happen"
    );

    let aborts = sv.script_aborts();
    assert!(
        aborts.is_empty(),
        "{map}: {} thread(s) aborted across the join\n{}",
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

#[test]
fn the_join_runs_to_completion_on_mp_pavlov() {
    check_join("mp_pavlov");
}

#[test]
fn the_join_runs_to_completion_on_mp_carentan() {
    check_join("mp_carentan");
}
