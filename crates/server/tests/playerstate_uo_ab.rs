//! The playerstate a protocol 22 server spawns a player into, diffed field
//! by field against the retail United Offensive 1.51b capture
//! `fixtures/playerstate/mp_cassino-dm-uo.txt` (`--save-playerstate
//! --capture-tag uo` beside `COD_VERSION=uo tools/run_server.sh`; the header
//! names the join). UO's 114 fields include the sprint and fatigue ones
//! (docs/research/coduo-diff.md 4, 20).
//!
//! Needs `COD_DIR` at a UO install; without one it returns early.

use crate::common;

use common::{Queues, header_value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;
use vcod_common::net::msg::PlayerState;
use vcod_common::net::protocol::PROTOCOL_V22;

const FIXTURE: &str = "tests/fixtures/playerstate/mp_cassino-dm-uo.txt";

/// Where the spawn lands (`_spawnlogic` picks at random) and the clock: the
/// spawn point is checked for shape below instead.
const SPAWN_SHAPE: &[&str] = &[
    "commandTime",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "viewangles[0]",
    "viewangles[1]",
    "viewangles[2]",
];

/// Fields vcod knowingly differs on, each with the reason.
const GAPS: &[(&str, &str)] = &[];

#[test]
fn a_spawned_uo_player_matches_retail() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let text = std::fs::read_to_string(FIXTURE).unwrap();
    let team = header_value(&text, "joined", FIXTURE).to_string();
    let weapon = header_value(&text, "weapon", FIXTURE).to_string();
    let mut section = "";
    let mut retail: Vec<(String, i32)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line;
            continue;
        }
        if section == "[playerstate]" {
            let (n, v) = line.split_once(' ').unwrap();
            retail.push((n.to_string(), v.parse().unwrap()));
        }
    }
    let p = &PROTOCOL_V22;
    let names: Vec<&str> = retail.iter().map(|(n, _)| n.as_str()).collect();
    let want: Vec<&str> = p.player_fields.iter().map(|f| f.name).collect();
    assert_eq!(
        names, want,
        "the capture is not in protocol 22's field order"
    );

    let fs = Rc::new(fs);
    let bsp_path = fs
        .resolve_map("mp_cassino")
        .expect("mp_cassino in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_cassino", "dm"), now);
    sv.set_protocol(&PROTOCOL_V22);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&*fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (cl, join) = common::join(&mut sv, &q, &mut now, &team, &weapon);
    assert!(join.findings().is_empty(), "{:?}", join.findings());
    let ours: PlayerState = cl
        .snapshots()
        .newest()
        .map(|s| s.ps.clone())
        .expect("a snapshot");

    let mut diffs = Vec::new();
    for (i, (name, r)) in retail.iter().enumerate() {
        if SPAWN_SHAPE.contains(&name.as_str()) || GAPS.iter().any(|(g, _)| g == name) {
            continue;
        }
        // Bit 512 is the anim restart toggle, not a fact about the pose.
        let mask = if name.ends_with("Anim") { 511 } else { !0 };
        if r & mask != ours.fields[i] & mask {
            diffs.push(format!("{name} retail {r} ours {}", ours.fields[i]));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} field(s) differ\n{}",
        diffs.len(),
        diffs.join("\n")
    );

    // The spawn is one of the map's DM spawn points, dropped to the floor by
    // `placeSpawnpoint` (which may nudge it under a unit sideways out of a
    // wall, as -688 -1160's is here).
    let origin = ours.origin(p);
    let spawns: Vec<[f32; 3]> = vcod_common::bsp::entity_blocks(&bsp.entities)
        .iter()
        .filter(|b| b.get("classname").map(String::as_str) == Some("mp_deathmatch_spawn"))
        .filter_map(|b| {
            b.get("origin")
                .and_then(|o| vcod_common::bsp::parse_vec3(o))
        })
        .collect();
    assert!(
        spawns.iter().any(|s| (s[0] - origin[0]).abs() < 1.0
            && (s[1] - origin[1]).abs() < 1.0
            && (s[2] - origin[2]).abs() < 64.0),
        "origin {origin:?} is at none of the {} spawn points",
        spawns.len()
    );
    assert_eq!((ours.health(), ours.max_health()), (100, 100));
    for (name, why) in GAPS {
        let i = PlayerState::field_index(p, name).unwrap();
        let r = retail[i].1;
        assert_ne!(
            r, ours.fields[i],
            "{name} now matches retail; drop it from GAPS ({why})"
        );
    }
}
