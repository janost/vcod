//! `func_door`, `func_door_rotating` and `func_rotating` against retail:
//! mp_carentan with the stock blocks of six submodels replaced by
//! `fixtures/doors/mp_carentan-doors.ent`, under
//! `client-probes/probe_doors` with one `--probe-uo-weapon direct` client,
//! on 1.1d, 1.5b and UO (`fixtures/doors/mp_carentan-dm-doors*.txt`,
//! recipes in their headers). Every frame's door origins and angles, the
//! client's origin, the use key's `"trigger"`, the crush damage and the
//! rotating slave's turn must be retail's, and so must what the client is
//! sent of each mover off the capture pair `net/*-doors-<v>.bin`: when it
//! enters and leaves the snapshot, its sound events, `loopSound` and
//! `constantLight` (cod11-movers.md 26). The replay spawns the client on
//! the capture's spawn point.
//!
//! `DOORS_REPORT=1` prints the differing `PROBE` lines, `DOORS_WIRE=1` the
//! differing wire lines, `DOORS_DUMP=<dir>` writes both sides of each.
//! Needs `COD_DIR` at the capture's install; without it the test returns
//! early.

use crate::common;
use std::collections::BTreeMap;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const BULLET_HITS: [i32; 3] = [
    vcod_common::net::event_ids::EV_BULLET_HIT_SMALL,
    vcod_common::net::event_ids::EV_BULLET_HIT_LARGE,
    vcod_common::net::event_ids::EV_UO_BULLET_HIT,
];
const MELEE: [i32; 2] = [
    vcod_common::net::event_ids::EV_MELEE_HIT,
    vcod_common::net::event_ids::EV_MELEE_MISS,
];
const BLOCKS: &str = include_str!("fixtures/doors/mp_carentan-doors.ent");

/// `fs` with the doors lump in place of mp_carentan's.
fn with_doors(fs: Option<Pk3Fs>) -> Option<Pk3Fs> {
    let mut fs = fs?;
    let path = fs.resolve_map(MAP)?;
    let bytes = fs.read(&path)?;
    fs.overlay(
        &path,
        vcod_common::testing::bsp_replacing_models(&bytes, BLOCKS),
    );
    Some(fs)
}

fn fs_11() -> Option<Pk3Fs> {
    with_doors(vcod_common::testing::game_fs_11())
}

fn fs_15() -> Option<Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = PROTOCOL_V6.transport.version;
    let fs = (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs);
    with_doors(fs)
}

fn fs_uo() -> Option<Pk3Fs> {
    with_doors(vcod_common::testing::uo_fs())
}

/// The lines up to `PROBE done`: the frame logger runs on after it, on
/// retail until the server quits, on ours until the replay stops.
fn to_done(mut lines: Vec<String>) -> Vec<String> {
    if let Some(i) = lines.iter().position(|l| l == "PROBE done") {
        lines.truncate(i + 1);
    }
    lines
}

fn check(fs: fn() -> Option<Pk3Fs>, proto: &'static Cod1Wire, fixture: &str, tag: &str) {
    if fs().is_none() {
        eprintln!("COD_DIR unset or not the capture's install: skipping");
        return;
    }
    let text = std::fs::read_to_string(fixture).unwrap();
    let retail = to_done(
        text.lines()
            .filter(|l| l.starts_with("PROBE"))
            .map(str::to_string)
            .collect(),
    );
    let (lines, frames) =
        common::direct_replay_frames(fs, proto, MAP, "probe_doors", "kar98k_mp", &text);
    let ours = to_done(lines);
    diff(fixture, &retail, &ours, "DOORS_REPORT");
    // The wire up to the last frame both ran.
    let retail_wire = retail_wire(proto, tag);
    let end = frames.last().map_or(0, |f| f.0);
    let retail_wire: Vec<String> = retail_wire
        .into_iter()
        .filter(|l| time_of(l) <= end)
        .collect();
    let ours_wire = wire_lines(proto, frames.iter().map(|(t, e)| (*t, e)));
    diff(tag, &retail_wire, &ours_wire, "DOORS_WIRE");
}

fn time_of(line: &str) -> i32 {
    line.split(' ')
        .nth(1)
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
}

fn diff(what: &str, retail: &[String], ours: &[String], env: &str) {
    if let Some(dir) = std::env::var_os("DOORS_DUMP") {
        let dir = std::path::Path::new(&dir);
        std::fs::write(dir.join(format!("{env}-retail.txt")), retail.join("\n")).unwrap();
        std::fs::write(dir.join(format!("{env}-ours.txt")), ours.join("\n")).unwrap();
    }
    if std::env::var(env).is_ok_and(|v| v == "1") {
        for (i, (a, b)) in retail.iter().zip(ours).enumerate() {
            if a != b {
                eprintln!("{i}\n retail {a}\n ours   {b}");
            }
        }
        if retail.len() != ours.len() {
            eprintln!("lengths: retail {} ours {}", retail.len(), ours.len());
        }
    }
    let first = retail.iter().zip(ours).position(|(a, b)| a != b);
    assert!(
        first.is_none() && retail.len() == ours.len(),
        "{what}: first difference at line {first:?} of {} / {}: retail {:?} ours {:?}",
        retail.len(),
        ours.len(),
        first.map(|i| &retail[i]),
        first.map(|i| &ours[i]),
    );
}

#[test]
fn doors_run_as_on_retail_11() {
    check(
        fs_11,
        &PROTOCOL_V1,
        "tests/fixtures/doors/mp_carentan-dm-doors.txt",
        "doors-11",
    );
}

#[test]
fn doors_run_as_on_retail_15() {
    check(
        fs_15,
        &PROTOCOL_V6,
        "tests/fixtures/doors/mp_carentan-dm-doors-1.5.txt",
        "doors-15",
    );
}

#[test]
fn doors_run_as_on_retail_uo() {
    check(
        fs_uo,
        &PROTOCOL_V22,
        "tests/fixtures/doors/mp_carentan-dm-doors-uo.txt",
        "doors-uo",
    );
}

/// What the client is sent of each mover past the clients, one line per
/// change: `W <time> <number> in <eventSequence> <loopSound>
/// <constantLight>` when it enters the snapshot, `out` when it leaves,
/// `ev <event> <parm>` for each event its ring took while sent, and
/// `loop` and `light` when they change; and `W <time> hit <event>
/// <surfType>` for each bullet impact's temp entity as it first comes
/// (the round on door_h: one on the door, one on the wall behind it).
pub fn wire_lines<'a>(
    p: &Cod1Wire,
    frames: impl IntoIterator<Item = (i32, &'a BTreeMap<u32, EntityState>)>,
) -> Vec<String> {
    let mover = vcod_common::net::cod1::flags::ET_MOVER;
    let mut out = Vec::new();
    let mut prev: BTreeMap<u32, [i32; 3]> = BTreeMap::new();
    let mut prev_hits: BTreeMap<u32, ()> = BTreeMap::new();
    for (t, ents) in frames {
        let mut now = BTreeMap::new();
        let mut hits = BTreeMap::new();
        for (&n, e) in ents {
            let etype = e.field_i32(p, "eType");
            if let Some(ev) = p.event_of(etype).filter(|ev| BULLET_HITS.contains(ev)) {
                if !prev_hits.contains_key(&n) {
                    out.push(format!("W {t} hit {ev} {}", e.field_i32(p, "surfType")));
                }
                hits.insert(n, ());
                continue;
            }
            if let Some(ev) = p.event_of(etype).filter(|ev| MELEE.contains(ev)) {
                if !prev_hits.contains_key(&n) {
                    let f = |name: &str| e.field_i32(p, name);
                    out.push(format!(
                        "W {t} melee {ev} {} {} {}",
                        f("otherEntityNum"),
                        f("eventParm"),
                        f("surfType")
                    ));
                }
                hits.insert(n, ());
                continue;
            }
            if n < 64 || etype != mover {
                continue;
            }
            let f = |name: &str| e.field_i32(p, name);
            let cur = [f("eventSequence"), f("loopSound"), f("constantLight")];
            match prev.get(&n) {
                None => out.push(format!("W {t} {n} in {} {} {:#x}", cur[0], cur[1], cur[2])),
                Some(last) => {
                    let mut s = last[0];
                    while s & 0xff != cur[0] & 0xff {
                        let k = s & 3;
                        out.push(format!(
                            "W {t} {n} ev {} {}",
                            f(&format!("events[{k}]")),
                            f(&format!("eventParms[{k}]"))
                        ));
                        s += 1;
                    }
                    if cur[1] != last[1] {
                        out.push(format!("W {t} {n} loop {}", cur[1]));
                    }
                    if cur[2] != last[2] {
                        out.push(format!("W {t} {n} light {:#x}", cur[2]));
                    }
                }
            }
            now.insert(n, cur);
        }
        for n in prev.keys().filter(|n| !now.contains_key(n)) {
            out.push(format!("W {t} {n} out"));
        }
        prev = now;
        prev_hits = hits;
    }
    out
}

/// [`wire_lines`] off a committed capture pair `net/*-<tag>.bin`.
fn retail_wire(p: &Cod1Wire, tag: &str) -> Vec<String> {
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    wire_lines(
        p,
        frames
            .iter()
            .map(|f| (f.snapshot.server_time, &f.snapshot.entities)),
    )
}
