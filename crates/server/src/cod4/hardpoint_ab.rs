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
//! [`GAPS`]. `helicopter_collision_and_pose_match_retail` replays a second
//! capture the same way: the posed tags, a trace through the turned gun
//! and the move trace's notifies; `helicopter_idle_matches_retail` a
//! third: a helicopter with no goal and its gun on a still body. Needs
//! `COD4_DIR`.
//!
//! `HARDPOINT_AB_REPORT=1` prints both runs' rows.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use vcod_cod4::net::delta::EntityState;

use super::wire_ab::{Probe, Side, retail, side};
use super::*;
use crate::game::cod4::vehicle::{ET_HELICOPTER, ET_PLANE};

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
    ("lerp.u.vehicle.gunPitch", 0.13, AIM),
    ("lerp.u.vehicle.gunYaw", 0.13, AIM),
];

/// Why the gun is tolerated in flight: `VEH_UpdateAim` aims from the posed
/// `tag_barrel` in the body's frame, so the gun parts as the flown body
/// does ([`FLIGHT`]). On a body that holds still it matches to
/// [`STILL_GAPS`] (`helicopter_idle_matches_retail`, cod4-gsc.md 3.2, "The
/// turret").
const AIM: &str = "the gun in the flown body's frame (cod4-gsc.md 3.2)";

/// The idle capture's tolerance before its goal, on a body that matches
/// word for word: `VEH_UpdateAim`'s arithmetic matches retail's step for
/// step given retail's barrel, but the posed `tag_barrel` lands an ulp off
/// now and then ([`POSE`]), which the tracking carries a few ulps on.
const STILL_GAPS: &[(&str, f32, &str)] = &[
    ("lerp.u.vehicle.gunPitch", 5e-5, POSE),
    ("lerp.u.vehicle.gunYaw", 5e-5, POSE),
];

/// Why a still gun is tolerated: ours poses the tags off the zone's
/// bind-pose `baseMat`, 1.7's DObj skeleton composes the model's local
/// bones (cod4-gsc.md 3.2, "The turret"; gdb on 0x80a011c read retail's
/// barrel an ulp off ours where the gun parted).
const POSE: &str = "the posed tag_barrel's place at an ulp (cod4-gsc.md 3.2)";

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

/// The owner on ours under the probe gsc in `gsc` and the game seed
/// `seed`: joins on retail's clock, axis and the rifle class at its twin's
/// times, spawned on its twin's spot.
fn ours(
    install: &vcod_cod4::install::Install,
    r: &Side,
    gsc: &str,
    seed: u32,
) -> (Side, Vec<String>) {
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
    sv.pin_game_seed(seed);
    sv.set_script_overrides(gsc.into());
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
    // To the capture's last snapshot and a frame.
    let run_ms = r.snaps.last().expect("snapshots").server_time + 50;
    while sv.core.sv_time_ms < run_ms {
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

/// The largest difference of each [`FIELDS`] field between retail's
/// vehicles and ours at the same time past spawn, and the rows ours lacks.
/// `tol` names the difference tolerated in a field at a time; every row
/// past it is listed.
fn compare(
    rt: &Tracks,
    ot: &Tracks,
    tol: impl Fn(&str, i32) -> f32,
    report: bool,
) -> (Vec<String>, Vec<String>) {
    assert_eq!(
        rt.keys().collect::<Vec<_>>(),
        ot.keys().collect::<Vec<_>>(),
        "the vehicles"
    );
    if report {
        for (key, rows) in rt {
            for (tag, rows) in [("retail", rows), ("ours", &ot[key])] {
                for (t, e) in rows.iter().filter(|(t, _)| [0, 50, 3000].contains(t)) {
                    eprintln!("{tag} {key:?} +{t}: {}", nonzero(e));
                }
            }
        }
    }
    let mut worst: BTreeMap<&str, (f32, String)> = BTreeMap::new();
    let mut missing = Vec::new();
    let mut unexplained = Vec::new();
    for (key, rows) in rt {
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
                if word(re, name) == word(oe, name) {
                    continue;
                }
                let d = if name.contains("apos") || name.contains("gun") {
                    // An angle: the short way round.
                    (a - b).rem_euclid(360.0).min((b - a).rem_euclid(360.0))
                } else {
                    (a - b).abs()
                }
                .max(f32::MIN_POSITIVE);
                if report {
                    eprintln!("{key:?} +{t} {name}: retail {a} ours {b}");
                }
                if d > tol(name, *t) {
                    unexplained.push(format!("{key:?} +{t} {name}: retail {a} ours {b}"));
                }
                let w = worst.entry(name).or_insert((0.0, String::new()));
                if d > w.0 {
                    *w = (d, format!("{key:?} +{t}: retail {a} ours {b}"));
                }
            }
        }
    }
    for (name, (d, at)) in &worst {
        eprintln!("{name}: worst {d} at {at}");
    }
    eprintln!("rows missing on ours: {}", missing.len());
    (missing, unexplained)
}

/// Retail's vehicles and ours, row by row: every [`FIELDS`] difference
/// past its [`GAPS`] tolerance, and the largest of each field.
#[test]
fn hardpoint_flights_match_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("hardpoint"));
    let (o, aborts) = ours(&install, &r, GSC, SEED);
    assert!(aborts.is_empty(), "{aborts:?}");
    let report = std::env::var_os("HARDPOINT_AB_REPORT").is_some();
    let gap = |name: &str, _| GAPS.iter().find(|g| g.0 == name).map_or(0.0, |g| g.1);
    let (missing, unexplained) = compare(&tracks(&r), &tracks(&o), gap, report);
    assert!(missing.is_empty(), "{missing:?}");
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// The idle probe gsc (`heli_idle`), read over the zones' scripts.
const IDLE_GSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/gsc/heli_idle"
);

/// How long past its spawn the idle capture's helicopter has no goal.
const IDLE_MS: i32 = 4000;

/// `cod4_hardpoint_ab`'s idle capture (`play-1.7-heli-idle.txt`): retail
/// 1.7 under `+set fs_game mods/vcodidle +devmap mp_crash`, `war`, sv_pure
/// 0, the gsc `crates/cod4/tests/fixtures/gsc/heli_idle/maps/mp/
/// mp_crash_fx.gsc` packed in the mod's iwd, one `cod4_probe` (`--team axis
/// --script idle --name owner --qport 4021 --secs 30`) under `unshare -rnm`.
/// A cobra spawns pitched 8, yawed 250 and rolled -6 with no goal for 4 s
/// while its gun tracks a point ahead and below, then one behind; then it
/// flies a pass-through goal. Until the goal every word matches retail's
/// but the gun's few ulps ([`STILL_GAPS`]): the no-goal think (0x808ba22)
/// and the turret (`VEH_UpdateAim`) on a body that holds still
/// (cod4-gsc.md 3.2). Past it the flight's [`GAPS`] hold, the gun's under
/// the body's. No random draw reaches it (no hover radius, no stop), so
/// the seed is the flight capture's.
#[test]
fn helicopter_idle_matches_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("heli-idle"));
    let (o, aborts) = ours(&install, &r, IDLE_GSC, SEED);
    assert!(aborts.is_empty(), "{aborts:?}");
    let report = std::env::var_os("HARDPOINT_AB_REPORT").is_some();
    let (rt, ot) = (tracks(&r), tracks(&o));
    assert!(
        rt.get(&(ET_HELICOPTER, 0))
            .is_some_and(|rows| rows.len() > 100),
        "the capture's helicopter"
    );
    let gap = |name: &str, t: i32| {
        let gaps = if t <= IDLE_MS { STILL_GAPS } else { GAPS };
        gaps.iter().find(|g| g.0 == name).map_or(0.0, |g| g.1)
    };
    let (missing, unexplained) = compare(&rt, &ot, gap, report);
    assert!(missing.is_empty(), "{missing:?}");
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// The collision probe gsc (`heli_collide`), read over the zones' scripts.
const COLLIDE_GSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/gsc/heli_collide"
);

/// A capture's `PROBE` lines (the probe gsc's `iPrintLn`s): the words and
/// the numbers of each, the time since the spawn first.
fn probe_lines(s: &Side) -> Vec<(Vec<String>, Vec<f32>)> {
    s.commands
        .iter()
        .filter_map(|(_, c)| Some(&c[c.find("PROBE ")? + 6..]))
        .map(|line| {
            let mut words = Vec::new();
            let mut nums = Vec::new();
            for tok in line.split(|c: char| c.is_whitespace() || "(),\"".contains(c)) {
                match tok.parse::<f32>() {
                    Ok(v) => nums.push(v),
                    Err(_) if !tok.is_empty() => words.push(tok.to_string()),
                    Err(_) => {}
                }
            }
            (words, nums)
        })
        .collect()
}

/// `cod4_hardpoint_ab`'s collision capture (`play-1.7-heli-collide.txt`):
/// retail 1.7 under `+set fs_game mods/vcodcoll +devmap mp_crash`, `war`,
/// sv_pure 0, the gsc `crates/cod4/tests/fixtures/gsc/heli_collide/maps/mp/
/// mp_crash_fx.gsc` packed in the mod's iwd, one `cod4_probe` (`--team axis
/// --script idle --name owner --qport 4021 --secs 45`) under `unshare -rnm`.
/// A cobra holds over the first loop node with its gun turned on a point
/// off its left side, then flies straight down into the ground. Ours
/// prints the same lines: the posed `tag_barrel` and `tag_flash` and a
/// bullet trace down through their middle (its point and the struck
/// triangle's normal, which 1.7 leaves in the bind pose's space), then the
/// think's `script_vehicle_collision` notifies (the first three, their
/// normals and places, and the count by 16 s) at the same times. No
/// random draw reaches them (hover radius 0), so the seed is the flight
/// capture's. The posed places part by the printed thousandth, the
/// descent's last place by its float model (cod4-gsc.md 3.2).
#[test]
fn helicopter_collision_and_pose_match_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("heli-collide"));
    let (o, aborts) = ours(&install, &r, COLLIDE_GSC, SEED);
    assert!(aborts.is_empty(), "{aborts:?}");
    let (rl, ol) = (probe_lines(&r), probe_lines(&o));
    assert_eq!(rl.len(), 8, "{rl:?}");
    assert_eq!(
        rl.iter().map(|l| &l.0).collect::<Vec<_>>(),
        ol.iter().map(|l| &l.0).collect::<Vec<_>>(),
        "the lines"
    );
    let report = std::env::var_os("HARDPOINT_AB_REPORT").is_some();
    for ((words, a), (_, b)) in rl.iter().zip(&ol) {
        if report {
            eprintln!("{words:?}: retail {a:?} ours {b:?}");
        }
        assert_eq!(a.len(), b.len(), "{words:?} {a:?} {b:?}");
        // The time and the count are exact; the posed places within the
        // print's last digit, the descent's last place within its float
        // model.
        let tol = match words.first().map(String::as_str) {
            Some("tags" | "trace") => 0.002,
            Some("count") => 0.2,
            _ => 0.0,
        };
        assert_eq!(a[0], b[0], "{words:?} time");
        for (x, y) in a.iter().zip(b).skip(1) {
            assert!((x - y).abs() <= tol, "{words:?}: retail {a:?} ours {b:?}");
        }
    }
}
