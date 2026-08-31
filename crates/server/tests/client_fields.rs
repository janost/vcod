//! What the stock `dm` join leaves on a client's script entity.
//!
//! This is deliberately weaker than `playerstate_ab.rs`, and the difference
//! matters. That gate diffs captured retail bytes; this one asserts against
//! the shipped scripts, because retail sends a client no entity for itself
//! and a lone client sees nobody else's, so there is nothing on the wire to
//! diff the client-tagged fields against. Every expectation below is read out
//! of `pak5.pk3:maps/mp/gametypes/dm.gsc` with the line named, so it proves
//! that our VM ran those lines and stored what they say — not that a retail
//! server holds the same values. What a second client sees of the first is
//! stage 5's gate, and that is the A/B this cannot be.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

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

/// `spawnPlayer` ends with these, in `dm.gsc` line order. `.sessionteam` is
/// `"none"` and not the team joined: dm has no teams, and line 584 overwrites
/// whatever the menu answer set. `.statusicon` is cleared from the connecting
/// icon line 187 set.
const AFTER_SPAWN: &[(&str, &str, u32)] = &[
    ("sessionteam", "none", 584),
    ("statusicon", "", 598),
    ("maxhealth", "100", 599),
    ("health", "100", 600),
];

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");

    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(fs).expect("load the scripts");

    let q = Rc::new(RefCell::new(Queues::default()));
    let (team, weapon) = common::captured_join(map, &cfg(map).gametype);
    let (_cl, join) = common::join(&mut sv, &q, &mut now, &team, &weapon);
    assert!(
        join.findings().is_empty(),
        "{map}: {}\n{}",
        join.summary(),
        join.findings().join("\n")
    );

    let mut diffs = Vec::new();
    for (name, want, line) in AFTER_SPAWN {
        let got = sv
            .client_field(0, name)
            .unwrap_or_else(|| panic!("{map}: client 0 has no script entity"));
        if got != *want {
            diffs.push(format!(".{name} is {got:?}, dm.gsc:{line} sets {want:?}"));
        }
    }
    // The two the menu answers wrote, which nothing after them touches:
    // `dm.gsc:267` stores the team response and `dm.gsc:349`/`354` the weapon.
    for (key, want, line) in [
        ("team", team.as_str(), 267),
        ("weapon", weapon.as_str(), 349),
    ] {
        let got = sv
            .client_pers(0, key)
            .unwrap_or_else(|| panic!("{map}: client 0 has no script entity"));
        if got != want {
            diffs.push(format!(
                ".pers[{key:?}] is {got:?}, dm.gsc:{line} stores the menu answer {want:?}"
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{map}: {} client field(s) are not what the stock scripts set\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

#[test]
fn the_join_leaves_the_stock_client_fields_on_mp_pavlov() {
    check("mp_pavlov");
}

/// The other nationality branch, for the same reason `playerstate_ab` runs
/// both maps: `.pers["weapon"]` differs between them.
#[test]
fn the_join_leaves_the_stock_client_fields_on_mp_carentan() {
    check("mp_carentan");
}
