//! `cod4_hardpoint_ab`: the hardpoint flight capture
//! (`play-1.7-hardpoint.txt`) on ours. Retail ran `cod4_lnxded` 1.7 under
//! `+set fs_game mods/vcodhp +devmap mp_crash`, `war`, with the probe gsc
//! `crates/cod4/tests/fixtures/gsc/hardpoint_flight/maps/mp/mp_crash_fx.gsc`
//! packed in the mod's iwd (retail reads a mod's scripts first), and one
//! `cod4_probe` joining axis and idling (`--team axis --script idle --name
//! owner --qport 4021 --secs 75`, sv_pure 0). A second after the probe
//! spawns the gsc flies two planes (`spawnPlane` and `moveTo`) and a
//! helicopter (`spawnHelicopter` and the `CMD_VEH_*` methods) on fixed
//! paths with no script random draw. Ours loads the same file over the
//! zones' (`Cod4Server::set_script_overrides`), the probe joins in process
//! on retail's level clock as `wire_ab`'s do, and each plane's and the
//! helicopter's wire state is compared with retail's at the same time past
//! its spawn (docs/research/cod4-gsc.md 3.2). What still differs is
//! [`GAPS`]. Needs `COD4_DIR`.
//!
//! `HARDPOINT_AB_REPORT=1` prints both runs' rows.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use vcod_cod4::net::delta::EntityState;

use super::wire_ab::{Probe, Side, retail, side};
use super::*;
use crate::game::cod4::vehicle::{ET_HELICOPTER, ET_PLANE};

/// The capture's last snapshot (78300) and a frame.
const RUN_MS: i32 = 78_350;

/// The run's `srand` seed (`G_InitGame`), recovered off the probe's
/// challenge by `wire_ab::recover_game_seed` (cod4-combat.md 10.9).
const SEED: u32 = 1_936_999_990;

/// The probe gsc's directory, read over the zones' scripts.
const GSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/gsc/hardpoint_flight"
);

/// The fields compared, each with the largest difference tolerated (0:
/// bit for bit) and why. A field not listed is not compared.
const GAPS: &[(&str, f32, &str)] = &[
    ("lerp.pos.trBase[0]", 10.0, FLIGHT),
    ("lerp.pos.trBase[1]", 10.0, FLIGHT),
    ("lerp.pos.trBase[2]", 2.0, FLIGHT),
    ("lerp.apos.trBase[0]", 2.5, FLIGHT),
    ("lerp.apos.trBase[1]", 2.5, FLIGHT),
    ("lerp.apos.trBase[2]", 2.5, FLIGHT),
    ("lerp.u.vehicle.gunPitch", 2.0, FLIGHT),
    ("lerp.u.vehicle.gunYaw", 2.0, FLIGHT),
];

/// Why the helicopter's flight is tolerated: the first 1.5 s match bit
/// for bit, then the turn through the first node parts by a few hundredths
/// of a degree (cod4-gsc.md 3.2, "The flight against retail"): the think's
/// float model, retail's x87 intermediates against ours in `f32`, steering
/// along a goal vector that shrinks to nothing at the node. The planes
/// match bit for bit.
const FLIGHT: &str = "the helicopter think's float model at the nodes (cod4-gsc.md 3.2)";

/// The fields of a plane or helicopter the comparison reads.
const FIELDS: &[&str] = &[
    "lerp.pos.trType",
    "lerp.pos.trBase[0]",
    "lerp.pos.trBase[1]",
    "lerp.pos.trBase[2]",
    "lerp.pos.trDelta[0]",
    "lerp.pos.trDelta[1]",
    "lerp.pos.trDelta[2]",
    "lerp.pos.trDuration",
    "lerp.apos.trType",
    "lerp.apos.trBase[0]",
    "lerp.apos.trBase[1]",
    "lerp.apos.trBase[2]",
    "lerp.u.vehicle.teamAndOwnerIndex",
    "lerp.u.vehicle.gunPitch",
    "lerp.u.vehicle.gunYaw",
    "lerp.u.vehicle.bodyPitch",
    "lerp.u.vehicle.bodyRoll",
    "un1.helicopterStage",
    "index",
    "un2",
    "weapon",
    "time2",
];

fn word(e: &EntityState, name: &str) -> Option<u32> {
    EntityState::table(e.etype())
        .iter()
        .find(|f| f.name == name)
        .map(|f| e.0[f.word()])
}

/// A field as a number: a float field's value, else the word.
fn value(e: &EntityState, name: &str) -> Option<f32> {
    let f = EntityState::table(e.etype())
        .iter()
        .find(|f| f.name == name)?;
    let w = e.0[f.word()];
    Some(if f.bits <= 0 {
        f32::from_bits(w)
    } else {
        w as f32
    })
}

/// An entity's non-zero fields by its type's table.
fn nonzero(e: &EntityState) -> String {
    EntityState::table(e.etype())
        .iter()
        .filter(|f| e.0[f.word()] != 0)
        .map(|f| format!("{}={:#x}", f.name, e.0[f.word()]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Each vehicle of the run by its spawn order within its type (planes 0
/// and 1, helicopter 0): (time past its first snapshot, its state).
type Tracks = BTreeMap<(u32, usize), Vec<(i32, EntityState)>>;

fn tracks(s: &Side) -> Tracks {
    let mut order: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut first: BTreeMap<u32, i32> = BTreeMap::new();
    let mut out = Tracks::new();
    for f in &s.snaps {
        for e in f
            .entities
            .iter()
            .filter(|e| matches!(e.etype(), ET_PLANE | ET_HELICOPTER))
        {
            let nums = order.entry(e.etype()).or_default();
            if !nums.contains(&e.number()) {
                nums.push(e.number());
            }
            let k = nums.iter().position(|&n| n == e.number()).unwrap();
            let t0 = *first.entry(e.number()).or_insert(f.server_time);
            out.entry((e.etype(), k))
                .or_default()
                .push((f.server_time - t0, *e));
        }
    }
    out
}

/// The owner on ours: joins on retail's clock, axis and the rifle class
/// at its twin's times, spawned on its twin's spot.
fn ours(install: &vcod_cod4::install::Install, r: &Side) -> (Side, Vec<String>) {
    let cs0 = String::from_utf8_lossy(r.gs.configstring(0)).into_owned();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    for (k, v) in vcod_common::net::connectionless::info_pairs(&cs0) {
        if !matches!(k, "protocol" | "shortversion" | "fs_game") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_cvar("sv_cheats", "1");
    sv.pin_game_seed(SEED);
    sv.set_script_overrides(GSC.into());
    sv.load(Some(install)).unwrap();
    let spawn = r
        .snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(0))
        .expect("the owner spawns");
    let yaw = spawn.ps.get_f32("viewangles[1]").unwrap_or(0.0);
    let origin = ["origin[0]", "origin[1]", "origin[2]"].map(|n| spawn.ps.get_f32(n).unwrap());
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(Some((origin, [0.0, yaw, 0.0])));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    let first_snap = r.snaps[0].server_time;
    let mut probe = Probe::new(
        "owner",
        0,
        28961,
        first_snap,
        &[
            (1050, "mr {sid} 8 axis"),
            (2550, "mr {sid} 15 assault_mp,0"),
        ],
    );
    let mut now = Instant::now();
    while sv.core.sv_time_ms < RUN_MS {
        let svs = sv.core.sv_time_ms;
        if !probe.joined && svs >= probe.connect_at {
            probe.join(&mut sv, now);
        }
        if probe.joined {
            let pkt = probe.packet(svs);
            probe.log("c2s", svs, &pkt);
            sv.handle_packet(probe.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if to == probe.addr {
                probe.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (side(&probe.lines), aborts)
}

/// Retail's vehicles and ours, row by row: every [`FIELDS`] difference
/// past its [`GAPS`] tolerance, and the largest of each field.
#[test]
fn hardpoint_flights_match_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("hardpoint"));
    let (o, aborts) = ours(&install, &r);
    assert!(aborts.is_empty(), "{aborts:?}");
    let (rt, ot) = (tracks(&r), tracks(&o));
    let report = std::env::var_os("HARDPOINT_AB_REPORT").is_some();
    assert_eq!(
        rt.keys().collect::<Vec<_>>(),
        ot.keys().collect::<Vec<_>>(),
        "the vehicles"
    );
    if report {
        for (key, rows) in &rt {
            for (tag, rows) in [("retail", rows), ("ours", &ot[key])] {
                for (t, e) in rows.iter().filter(|(t, _)| [0, 50, 3000].contains(t)) {
                    eprintln!("{tag} {key:?} +{t}: {}", nonzero(e));
                }
            }
        }
    }
    let mut worst: BTreeMap<&str, (f32, String)> = BTreeMap::new();
    let mut missing = Vec::new();
    for (key, rows) in &rt {
        let ours: BTreeMap<i32, &EntityState> = ot[key].iter().map(|(t, e)| (*t, e)).collect();
        for (t, re) in rows {
            let Some(oe) = ours.get(t) else {
                missing.push(format!("{key:?} +{t}"));
                continue;
            };
            for name in FIELDS {
                let (Some(a), Some(b)) = (value(re, name), value(oe, name)) else {
                    continue;
                };
                let d = if word(re, name) == word(oe, name) {
                    0.0
                } else if name.contains("apos") || name.contains("gun") {
                    // An angle: the short way round.
                    (a - b).rem_euclid(360.0).min((b - a).rem_euclid(360.0))
                } else {
                    (a - b).abs()
                }
                .max(if word(re, name) == word(oe, name) {
                    0.0
                } else {
                    f32::MIN_POSITIVE
                });
                if report && d > 0.0 {
                    eprintln!("{key:?} +{t} {name}: retail {a} ours {b}");
                }
                let w = worst.entry(name).or_insert((0.0, String::new()));
                if d > w.0 {
                    *w = (d, format!("{key:?} +{t}: retail {a} ours {b}"));
                }
            }
        }
    }
    let mut unexplained = Vec::new();
    for (name, (d, at)) in &worst {
        let tol = GAPS.iter().find(|g| g.0 == *name).map_or(0.0, |g| g.1);
        eprintln!("{name}: worst {d} at {at} (tolerated {tol})");
        if *d > tol {
            unexplained.push(format!("{name}: {d} at {at}"));
        }
    }
    eprintln!("rows missing on ours: {}", missing.len());
    assert!(missing.is_empty(), "{missing:?}");
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}
