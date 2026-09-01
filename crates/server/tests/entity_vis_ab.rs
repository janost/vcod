//! The entity list a client is sent from one spot, diffed against the retail
//! trace sample taken at that spot. Stage 5's visibility gate.
//!
//! `entities_ab.rs` pins what the map puts on the wire; this one pins who is
//! shown it. Retail culls by the client's PVS cluster
//! (`docs/protocol-1.1.md`, "Which entities a client is sent"), so a sample is
//! a position and the set that position was sent, and the replay puts our own
//! cull at the same position.
//!
//! Exact set match, not a subset: a gate that only asked "do we send at least
//! what retail sends" would pass a server that sends everything to everybody,
//! which is the bug it exists to catch.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{header_value, Queues};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Instant;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::PROTOCOL_V1;

/// Samples stage 5 knowingly does not reproduce, as `(map, label, reason)`.
/// Empty is the goal, and the guard below fails on one that starts matching.
const GAPS: &[(&str, &str, &str)] = &[];

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

/// One station of the trace: where the probe stood, and the entity numbers it
/// had been sent there.
struct Sample {
    label: String,
    origin: [f32; 3],
    ents: BTreeSet<u32>,
}

fn retail(map: &str) -> (Vec<Sample>, String, String, String) {
    let path = format!("tests/fixtures/entities/{map}-{}.txt", cfg(map).gametype);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let head = |k: &str| header_value(&text, k, &path).to_string();
    assert_eq!(head("map"), map, "{path}: header names another map");

    let mut out: Vec<Sample> = Vec::new();
    let mut in_ent = false;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[sample ") {
            out.push(Sample {
                label: rest
                    .split_once("] ")
                    .map(|(_, l)| l)
                    .unwrap_or("")
                    .to_string(),
                origin: [0.0; 3],
                ents: BTreeSet::new(),
            });
            in_ent = false;
            continue;
        }
        let s = out.last_mut().expect("a line before the first [sample]");
        if let Some(num) = line
            .strip_prefix("[ent ")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse::<u32>().ok())
        {
            s.ents.insert(num);
            in_ent = true;
            continue;
        }
        if in_ent {
            continue;
        }
        if let Some((name, v)) = line.split_once(' ') {
            let bits: i32 = v.parse().unwrap_or_else(|e| panic!("{path}: {name}: {e}"));
            match name {
                "origin[0]" => s.origin[0] = f32::from_bits(bits as u32),
                "origin[1]" => s.origin[1] = f32::from_bits(bits as u32),
                "origin[2]" => s.origin[2] = f32::from_bits(bits as u32),
                _ => {}
            }
        }
    }
    assert!(
        out.iter().map(|s| &s.ents).collect::<BTreeSet<_>>().len() >= 2,
        "{path}: every sample carries the same entity set, so this gate cannot \
         fail a server that sends everything"
    );
    (out, head("joined"), head("weapon"), path)
}

/// Our full entity list, before any culling: what the samples are filtered out
/// of. Built the way `entities_ab.rs` builds it, through a real join.
fn ours(
    map: &str,
    team: &str,
    weapon: &str,
    fs: vcod_common::pk3::Pk3Fs,
    bsp: &vcod_common::bsp::Bsp,
) -> BTreeMap<u32, EntityState> {
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(bsp));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (cl, _join) = common::join(&mut sv, &q, &mut now, team, weapon);
    assert!(
        cl.snapshots().newest().is_some(),
        "the server sent no snapshot at all"
    );
    // Not the snapshot's list: that one is already culled to where this client
    // happens to stand.
    sv.all_entities()
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping {map}");
        return;
    };
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    let (samples, team, weapon, path) = retail(map);
    let all = ours(map, &team, &weapon, fs, &bsp);
    let vis = bsp.visibility();
    let p = &PROTOCOL_V1;

    let mut findings: Vec<String> = Vec::new();
    let mut gaps_hit: BTreeSet<&str> = BTreeSet::new();
    for s in &samples {
        let got: BTreeSet<u32> = vcod_server::world::visible_entities(&vis, s.origin, &all, p)
            .keys()
            .copied()
            .collect();
        if got == s.ents {
            continue;
        }
        if let Some((_, g, _)) = GAPS
            .iter()
            .find(|(m, g, _)| *m == map && *g == s.label.as_str())
        {
            gaps_hit.insert(g);
            continue;
        }
        let missing: Vec<u32> = s.ents.difference(&got).copied().collect();
        // Each side's entities with the cluster they sit in: a wrong cluster
        // is the first thing to suspect, and -1 means a solid leaf.
        let extra: Vec<String> = got
            .difference(&s.ents)
            .map(|n| format!("{n} (cluster {})", vis.cluster_at(all[n].origin(p))))
            .collect();
        findings.push(format!(
            "sample {:?} at [{:.0}, {:.0}, {:.0}] (cluster {}): retail sent {} entities, we send \
             {}; missing {missing:?}, extra {extra:?}",
            s.label,
            s.origin[0],
            s.origin[1],
            s.origin[2],
            vis.cluster_at(s.origin),
            s.ents.len(),
            got.len(),
        ));
    }

    for (m, g, why) in GAPS {
        if *m != map {
            continue;
        }
        assert!(
            gaps_hit.contains(g),
            "{map}: GAPS lists sample {g:?} ({why}) but it matches now; drop it"
        );
    }
    assert!(
        findings.is_empty(),
        "{map}: {} of {} samples differ from {path} ({} entities on the map)\n{}",
        findings.len(),
        samples.len(),
        all.len(),
        findings.join("\n"),
    );
}

#[test]
fn carentan_entity_visibility_matches_retail() {
    check("mp_carentan");
}

#[test]
fn pavlov_entity_visibility_matches_retail() {
    check("mp_pavlov");
}
