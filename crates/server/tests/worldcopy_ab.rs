//! A script-spawned brush entity, a copy of the world's clip where it
//! stands, on the wire and as a pusher, against the retail capture.
//!
//! Two fixtures from one retail run (the first one's header carries the
//! recipe): `movers/mp_carentan-dm-worldcopy.txt` is the server's
//! `games_mp.log` under `client-probes/probe_worldcopy.gsc`, one `PROBE f`
//! line per server frame of a phase (the player, the health pack and the
//! copy); `-worldcopy-wire.txt` is the `--probe-ride` client's half. Ours
//! runs the same probe with one allied client standing still.
//! `docs/research/cod11-movers.md` 20 reads both. Each pack landing's
//! random lift (items doc 14.2) is retail's, held to the lift's range, so
//! the pack is compared to the same tolerance as the rest.
//!
//! Three more captures, each header with its recipe, movers doc 21:
//! `-worldcopy2.txt` turns a copy under the player and blasts three
//! `func_static`s, `-worldcopy3.txt` reruns its turns with the player
//! printed to the ten-thousandth (`-1.5.txt` and `-uo.txt` on 1.5b and
//! UO), and `-sinecrush.txt` crushes the player
//! under a `func_bobbing`. `mp_harbor-dm-turnedslab.txt` stands the player
//! beside a turned submodel as it lifts. `mp_harbor-dm-tiltslab*.txt` and
//! `mp_depot-dm-rollramp.txt` drop him onto a pitched or rolled submodel,
//! the latter onto a non-axial side (movers doc 22); `-tiltslab-1.5.txt`,
//! `-tiltslab-uo.txt`, `-rollramp-1.5.txt` and `-rollramp-uo.txt` are both
//! on 1.5b and UO.
//!
//! Each capture holds its client's cmds (`CMDS` lines), and ours replays them
//! (`ProbeHold::framed`): where a fall lands and slides rides on where its
//! cmds split it (movers doc 21, "The slide down the slope").
//!
//! `WORLDCOPY_REPORT=1` prints every compared row, `WORLDCOPY_PS=1` the
//! client's playerstate per snapshot.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

use crate::common;

use common::{ADDR, ClientEnd, FRAME_MS, Join, Queues, holding, step_at};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::net::{NetClient, NetEvent};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy.txt";
const WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy-wire.txt";
const PROBE: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_worldcopy.gsc");
const SERVER2: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy2.txt";
const PROBE2: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_worldcopy2.gsc");
const SERVER3: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy3.txt";
const SERVER3_15: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy3-1.5.txt";
const SERVER3_UO: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy3-uo.txt";
const PROBE3: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_worldcopy3.gsc");
const CRUSH: &str = "tests/fixtures/movers/mp_carentan-dm-sinecrush.txt";
const SLAB: &str = "tests/fixtures/movers/mp_harbor-dm-turnedslab.txt";
const PROBE_SLAB: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_turnedslab.gsc");
const TILT: &str = "tests/fixtures/movers/mp_harbor-dm-tiltslab.txt";
const TILT_RIDE: &str = "tests/fixtures/movers/mp_harbor-dm-tiltslab-ride.txt";
const TILT_15: &str = "tests/fixtures/movers/mp_harbor-dm-tiltslab-1.5.txt";
const TILT_UO: &str = "tests/fixtures/movers/mp_harbor-dm-tiltslab-uo.txt";
const PROBE_TILT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_tiltslab.gsc");
const ROLL: &str = "tests/fixtures/movers/mp_depot-dm-rollramp.txt";
const ROLL_15: &str = "tests/fixtures/movers/mp_depot-dm-rollramp-1.5.txt";
const ROLL_UO: &str = "tests/fixtures/movers/mp_depot-dm-rollramp-uo.txt";
const PROBE_ROLL: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_rollramp.gsc");
const PROBE_CRUSH: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_sinecrush.gsc");

/// Float formatting.
const TOL: f32 = 0.05;

/// How long before its first snapshot a capture's client starts joining.
const FIRST_SNAP_AFTER_JOIN_MS: i64 = 400;

/// The `PROBE g` print's ten-thousandth plus the carry's float error.
const PRECISE_TOL: f32 = 0.0003;

/// The most `G_BounceItem` lifts a landing item off its floor:
/// `0.5 + 0.5 * rand() * -2^-31` (items doc 14.2).
const MAX_LIFT: f32 = 0.5;

/// 1.5b's and UO's lift, `0.5 + 0.5 * rand() / 2^31` (items doc 14.2).
const LIFT_15: std::ops::RangeInclusive<f32> = 0.5..=1.0;

/// The pack per sample time, the second vector of each `PROBE f` line.
fn pack_track(text: &str) -> Vec<(i32, [f32; 3])> {
    text.lines()
        .filter_map(|l| {
            let rest = &l[l.find("PROBE f ")? + 8..];
            let time = rest.split_whitespace().nth(1)?.parse().ok()?;
            Some((time, vectors(rest).get(1).copied().flatten()?))
        })
        .collect()
}

fn report() -> bool {
    std::env::var("WORLDCOPY_REPORT").is_ok_and(|v| v == "1")
}

/// Every `(x, y, z)` on a line, and `None` for each `(none)`.
fn vectors(line: &str) -> Vec<Option<[f32; 3]>> {
    line.split('(')
        .skip(1)
        .map(|s| {
            let inner = s.split(')').next()?;
            let v: Vec<f32> = inner
                .split(',')
                .filter_map(|t| t.trim().parse().ok())
                .collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .collect()
}

/// Each phase's frames: player, pack and copy, the copy `None` when the
/// phase has none.
type Phases = BTreeMap<String, Vec<Vec<Option<[f32; 3]>>>>;

fn phases<'a>(lines: impl Iterator<Item = &'a str>) -> Phases {
    let mut out = Phases::new();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE f ").map(|i| &line[i + 8..]) else {
            continue;
        };
        let phase = rest.split_whitespace().next().unwrap().to_string();
        out.entry(phase).or_default().push(vectors(rest));
    }
    out
}

/// The copy's spawn origin, which `probe_worldcopy3`'s `PROBE g` lines
/// measure the player from.
const COPY_SPAWN: [f32; 3] = [-576.0, 2688.0, -75.875];

/// Each phase's player per frame off the `PROBE g` lines, to the
/// ten-thousandth: `[x, y, z]` in the world.
fn precise<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, Vec<[f32; 3]>> {
    let mut out: BTreeMap<String, Vec<[f32; 3]>> = BTreeMap::new();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE g ").map(|i| &line[i + 8..]) else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        let v: Vec<f32> = t[2..5].iter().map(|v| v.parse().unwrap()).collect();
        out.entry(t[0].to_string())
            .or_default()
            .push(std::array::from_fn(|i| v[i] + COPY_SPAWN[i]));
    }
    out
}

/// A `RIDE_NEW` line's `eType`, `solid`, `index`, `eFlags` and origin,
/// keyed by what the probe named the entity.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sent {
    etype: i32,
    solid: i32,
    index: i32,
    eflags: i32,
    origin: [f32; 3],
}

fn parse_new(line: &str) -> (u32, Sent) {
    let field = |k: &str| {
        line.split_whitespace()
            .find_map(|t| t.strip_prefix(k))
            .unwrap()
    };
    let int = |k: &str| {
        let v = field(k);
        match v.strip_prefix("0x") {
            Some(h) => i64::from_str_radix(h, 16).unwrap() as i32,
            None => v.parse().unwrap(),
        }
    };
    let o: Vec<f32> = field("origin=")
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    (
        int("num=") as u32,
        Sent {
            etype: int("eType="),
            solid: int("solid="),
            index: int("index="),
            eflags: int("eFlags="),
            origin: [o[0], o[1], o[2]],
        },
    )
}

/// The probe's `PROBE <tag> <num>` lines: which entity is the copy and
/// which the `func_static`.
fn tagged(lines: &[String], tag: &str) -> Option<u32> {
    let key = format!("PROBE {tag} ");
    lines.iter().find_map(|l| {
        l.split(&key)
            .nth(1)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

struct Ours {
    log: Vec<String>,
    /// Every entity that entered a snapshot after the first, as first sent.
    new: BTreeMap<u32, Sent>,
    /// Every pack landing: retail's level time, ours' rest with no lift,
    /// retail's lift above it.
    lifts: Vec<(i32, f32, f32)>,
    /// Every pack landing's lift by ours' level time, for [`run_once`]'s
    /// second pass, and whether any of them was corrected after its frame.
    landings: (BTreeMap<i32, f32>, bool),
    /// Whether the level dropped.
    killed: bool,
    /// The client's first snapshot's time.
    first_snap: Option<i32>,
}

fn ours() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours(MAP, "probe_worldcopy", PROBE, SERVER, false))
        .as_ref()
}

fn ours2() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours(MAP, "probe_worldcopy2", PROBE2, SERVER2, true))
        .as_ref()
}

fn ours3() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours(MAP, "probe_worldcopy3", PROBE3, SERVER3, true))
        .as_ref()
}

fn ours_slab() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours("mp_harbor", "probe_turnedslab", PROBE_SLAB, SLAB, true))
        .as_ref()
}

fn ours_tilt() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours("mp_harbor", "probe_tiltslab", PROBE_TILT, TILT, true))
        .as_ref()
}

fn ours_tilt_ride() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours("mp_harbor", "probe_tiltslab", PROBE_TILT, TILT_RIDE, true))
        .as_ref()
}

/// The capture on retail 1.5b, `None` unless `COD_DIR`'s `main/` is 1.5's
/// (a UO install's is).
fn ours_tilt_15() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        let (map, name) = ("mp_harbor", "probe_tiltslab");
        run_ours_on(fs_15, map, name, PROBE_TILT, TILT_15, true)
    })
    .as_ref()
}

/// The capture on retail United Offensive 1.51b, `None` off a UO install.
fn ours_tilt_uo() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        let (map, name) = ("mp_harbor", "probe_tiltslab");
        let paks = vcod_common::testing::uo_fs;
        run_ours_on(paks, map, name, PROBE_TILT, TILT_UO, true)
    })
    .as_ref()
}

fn ours_roll() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours("mp_depot", "probe_rollramp", PROBE_ROLL, ROLL, true))
        .as_ref()
}

/// `probe_rollramp` on retail 1.5b, `None` unless `COD_DIR`'s `main/` is 1.5's.
fn ours_roll_15() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        let (map, name) = ("mp_depot", "probe_rollramp");
        run_ours_on(fs_15, map, name, PROBE_ROLL, ROLL_15, true)
    })
    .as_ref()
}

/// `probe_rollramp` on retail United Offensive 1.51b, `None` off a UO install.
fn ours_roll_uo() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        let (map, name) = ("mp_depot", "probe_rollramp");
        let paks = vcod_common::testing::uo_fs;
        run_ours_on(paks, map, name, PROBE_ROLL, ROLL_UO, true)
    })
    .as_ref()
}

fn ours_crush() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| run_ours(MAP, "probe_sinecrush", PROBE_CRUSH, CRUSH, true))
        .as_ref()
}

/// [`run_once`], run again once if the client's first snapshot missed
/// retail's: how long ours takes to join differs by map.
fn run_ours(map: &str, name: &str, probe: &str, fixture: &str, probe_rate: bool) -> Option<Ours> {
    run_ours_on(
        vcod_common::testing::game_fs_11,
        map,
        name,
        probe,
        fixture,
        probe_rate,
    )
}

/// [`run_ours`] on `fs`'s install, under its protocol.
fn run_ours_on(
    fs: fn() -> Option<Pk3Fs>,
    map: &str,
    name: &str,
    probe: &str,
    fixture: &str,
    probe_rate: bool,
) -> Option<Ours> {
    let none = BTreeMap::new();
    let mut ours = run_once(fs()?, map, name, probe, fixture, probe_rate, 0, &none)?;
    let text = std::fs::read_to_string(fixture).unwrap();
    let mut late = 0;
    if let (Some(&(first, _)), Some(t)) = (common::cmds_ran(&text, "").first(), ours.first_snap)
        && t != first
    {
        late = (t - first) as i64;
        ours = run_once(fs()?, map, name, probe, fixture, probe_rate, late, &none)?;
    }
    // A landing whose lift was corrected after its frame ran that frame's
    // push at the wrong height: run again with every lift drawn in place.
    if !ours.landings.1 {
        return Some(ours);
    }
    let known = ours.landings.0;
    run_once(fs()?, map, name, probe, fixture, probe_rate, late, &known)
}

/// Runs `probe` on `map` on ours with one allied client standing still, to `PROBE
/// done` or the level's drop. `fixture` is the retail capture whose pack
/// lifts it borrows, when it has a pack, and whose client's cmds it replays,
/// when it logs them. `late_ms` starts the client that much earlier.
/// `known` is a first pass's lift for each landing by ours' level time.
/// `probe_rate` sends the probe client's three cmds a frame instead of one.
#[allow(clippy::too_many_arguments)]
fn run_once(
    fs: Pk3Fs,
    map: &str,
    name: &str,
    probe: &str,
    fixture: &str,
    probe_rate: bool,
    late_ms: i64,
    known: &BTreeMap<i32, f32>,
) -> Option<Ours> {
    let p = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, name), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{name}"), probe);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    // Each landing's lift is a fresh `rand()` draw, which ours cannot
    // replay. Before the samples start, a landing where the pack rests takes
    // the lift up to retail's first rest. After, a landing takes the lift up
    // to retail's rest a frame later (a thread reads the last frame's entity
    // pass), the clocks lined up on the first `rest` sample, and the pack is
    // raised to that rest after the frame: a mover that carried it in the
    // frame put it off by its step. That corrected lift is the draw, which
    // a second pass (`known`) gives the landing itself, so the frame's push
    // tests the pack where retail's stood. Each lift is kept for the test to
    // hold to the lift's range.
    let retail_text = std::fs::read_to_string(fixture).unwrap();
    let track = Rc::new(pack_track(&retail_text));
    let rest = track.first().map_or([0.0; 3], |s| s.1);
    let offset = Rc::new(std::cell::Cell::new(None::<i32>));
    let landed = Rc::new(RefCell::new(Vec::new()));
    let lifts = Rc::new(RefCell::new(Vec::new()));
    let (seen, early, off_seen, landing_track) =
        (landed.clone(), lifts.clone(), offset.clone(), track.clone());
    let known = known.clone();
    let has_pack = !track.is_empty();
    sv.test_item_lifts(Box::new(move |time, floor| {
        if !has_pack {
            return 0.25;
        }
        let under = |at: [f32; 3]| (floor[0] - at[0]).abs() < 0.5 && (floor[1] - at[1]).abs() < 0.5;
        let Some(off) = off_seen.get() else {
            if !under(rest) {
                return 0.0;
            }
            early
                .borrow_mut()
                .push((time, floor[2], rest[2] - floor[2]));
            return rest[2] - floor[2];
        };
        let at = time + off + FRAME_MS as i32;
        let lift = match (known.get(&time), landing_track.iter().find(|s| s.0 >= at)) {
            (Some(&lift), _) => lift,
            (None, Some(&(_, r))) if under(r) => r[2] - floor[2],
            _ => return 0.0,
        };
        seen.borrow_mut().push((time, floor, lift));
        lift
    }));
    let mut corrected = false;
    let mut landings = BTreeMap::new();
    let mut pack = None;
    // A capture that logs its client's cmds (`CMDS` lines) is replayed on
    // them: the client joins so its first snapshot falls on retail's, and
    // each cmd runs on retail's side of a frame (`ProbeHold::framed`).
    let mut hold = common::ProbeHold {
        base: Some([0.0; 3]),
        framed: true,
        times: common::cmds_times(&retail_text, ""),
        ran: common::cmds_ran(&retail_text, ""),
        ..Default::default()
    };
    let replay = !hold.ran.is_empty();
    if let Some(&(first, _)) = hold.ran.first() {
        while (sv.game_time_ms() as i64) < first as i64 - FIRST_SNAP_AFTER_JOIN_MS - late_ms {
            now += Duration::from_millis(FRAME_MS as u64);
            sv.tick(now);
            sv.take_outgoing();
        }
    }
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    // mp_harbor's allies are Russian, mp_depot's British.
    let weapon = match map {
        "mp_harbor" => "mosin_nagant_mp",
        "mp_depot" => "enfield_mp",
        _ => "m1carbine_mp",
    };
    let mut join = Join::new("allies", weapon);
    let mut present: Option<std::collections::BTreeSet<u32>> = None;
    let mut new = BTreeMap::new();
    let mut first_snap = None;
    for _ in 0..3000 {
        now += Duration::from_millis(FRAME_MS as u64);
        // Playing, linked (`pm_type` 1) or dead (6): a corpse falls on the
        // cmds too.
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| matches!(s.ps.field_i32(p, "pm_type"), 0 | 1 | 6));
        if replay && live {
            let cmds = hold.cmds(&cl, 0);
            cl.send_cmds(&cmds);
        } else if replay {
            if let Some(c) = cl.send_frame(&holding(&cl)) {
                hold.last = c.server_time;
            }
        } else if probe_rate {
            send_at_probe_rate(&mut cl);
        } else {
            cl.send_frame(&holding(&cl));
        }
        for e in step_at(&mut sv, ADDR, &q, &mut cl, now) {
            match e {
                NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, &mut cl, now),
                NetEvent::Dropped(r) => assert!(sv.is_killed(), "dropped: {r}"),
                _ => {}
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            first_snap.get_or_insert(s.server_time);
            if let Some(present) = &present {
                for (&n, e) in s.entities.iter().filter(|(n, _)| !present.contains(n)) {
                    new.entry(n).or_insert(Sent {
                        etype: e.field_i32(p, "eType"),
                        solid: e.field_i32(p, "solid"),
                        index: e.field_i32(p, "index"),
                        eflags: e.field_i32(p, "eFlags"),
                        origin: e.origin(p),
                    });
                }
            }
            present = Some(s.entities.keys().copied().collect());
            if std::env::var("WORLDCOPY_PS").is_ok() {
                println!(
                    "PS t={} origin={:?} vel={:.0},{:.0},{:.0} ground={}",
                    s.server_time,
                    s.ps.origin(p),
                    s.ps.field_f32(p, "velocity[0]"),
                    s.ps.field_f32(p, "velocity[1]"),
                    s.ps.field_f32(p, "velocity[2]"),
                    s.ps.field_i32(p, "groundEntityNum"),
                );
            }
        }
        if offset.get().is_none()
            && has_pack
            && let Some(t) = pack_track(&sv.script_log().join("\n")).first()
        {
            offset.set(Some(track[0].0 - t.0));
            pack = sv.script_log().iter().find_map(|l| {
                let rest = l.split("PROBE player ").nth(1)?;
                rest.split(" pack ").nth(1)?.trim().parse().ok()
            });
        }
        if let (Some(off), Some(pack)) = (offset.get(), pack) {
            for (time, floor, lift) in landed.take() {
                let Some(o) = sv.test_raise_entity(pack, 0.0) else {
                    continue;
                };
                if (o[0] - floor[0]).abs() > 0.5 || (o[1] - floor[1]).abs() > 0.5 {
                    continue;
                }
                let at = time + off + FRAME_MS as i32;
                let Some(&(_, r)) = track.iter().find(|s| s.0 >= at) else {
                    continue;
                };
                // Past the print's two decimals and a float step.
                let dz = r[2] - o[2];
                if dz.abs() > 1e-4 {
                    sv.test_raise_entity(pack, dz);
                    corrected = true;
                }
                landings.insert(time, lift + dz);
                lifts.borrow_mut().push((time + off, floor[2], lift + dz));
            }
        }
        if sv.is_killed() || sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    Some(Ours {
        log: sv.script_log().to_vec(),
        new,
        lifts: lifts.take(),
        landings: (landings, corrected),
        killed: sv.is_killed(),
        first_snap,
    })
}

/// One frame of the probe client's cmds, one every 16 or 17 ms, each with
/// `send_frame`'s stamp and `delta_angles` rebase. A corpse falls a cmd's
/// gravity on every cmd and keeps the sink of the first that ends above the
/// floor, which a 50 ms cmd never does; a fall onto a slope slides down it
/// only from a cmd that starts within the ground trace's reach, which a
/// 50 ms cmd rarely does (cod11-movers.md 21).
fn send_at_probe_rate(cl: &mut NetClient<ClientEnd>) {
    let now = cl.server_clock_ms();
    let input = holding(cl);
    let da = cl
        .snapshots()
        .newest()
        .map(|s| vcod_common::net::DELTA_ANGLE_FIELDS.map(|n| s.ps.field_i32(cl.protocol(), n)));
    let cmds = [now - 34, now - 17, now].map(|t| {
        let mut c = input;
        c.server_time = t;
        if let Some(da) = da {
            for (a, d) in c.angles.iter_mut().zip(da) {
                *a = (*a - d) & 0xffff;
            }
        }
        c
    });
    cl.send_cmds(&cmds);
}

fn diff(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

#[test]
fn a_moving_world_copy_pushes_like_retail() {
    let Some(Ours { log, lifts, .. }) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(
        log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours"
    );
    // Retail's two decimals put a lift up to 0.005 out of range.
    for &(time, floor, lift) in lifts {
        if report() {
            println!("landing {time} floor {floor} lift {lift}");
        }
        assert!(
            (-0.01..=MAX_LIFT + 0.01).contains(&lift),
            "the pack landed at {time} at {floor}, {lift} under retail's rest"
        );
    }
    assert!(!lifts.is_empty(), "the pack never landed on ours");
    let retail = phases(std::fs::read_to_string(SERVER).unwrap().lines());
    let ours = phases(log.iter().map(String::as_str));
    let bad = compare_phases(&retail, &ours);
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Every phase's frames column by column, each to [`TOL`]; the rows that
/// differ.
fn compare_phases(retail: &Phases, ours: &Phases) -> Vec<String> {
    let mut bad = Vec::new();
    for (name, r) in retail {
        let Some(o) = ours.get(name) else {
            bad.push(format!("ours never ran {name}"));
            continue;
        };
        if r.len() != o.len() {
            bad.push(format!("{name}: {} samples, ours {}", r.len(), o.len()));
        }
        for (k, (rf, of)) in r.iter().zip(o).enumerate() {
            if report() {
                println!("{name} {k} retail {rf:?}\n{name} {k} ours   {of:?}");
            }
            for col in 0..rf.len().max(of.len()) {
                let (a, b) = (
                    rf.get(col).copied().flatten(),
                    of.get(col).copied().flatten(),
                );
                let row = match (a, b) {
                    (Some(a), Some(b)) if diff(a, b) > TOL => Some(format!("{a:?} ours {b:?}")),
                    (Some(_), Some(_)) | (None, None) => None,
                    (a, b) => Some(format!("{a:?} ours {b:?}")),
                };
                if let Some(row) = row {
                    bad.push(format!("{name} frame {k} column {col}: retail {row}"));
                }
            }
        }
    }
    bad
}

/// The `PROBE` lines other than the per-frame samples, with the `<time>`
/// field of the tags that carry one dropped, since ours' clock starts
/// elsewhere.
fn events<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim_end()))
        .filter(|l| !l.starts_with("f ") && !l.starts_with("done") && !l.starts_with("player"))
        .map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            match t[0] {
                "ptr" => [&t[..2], &t[3..]].concat().join(" "),
                "notify" => [&t[..3], &t[4..]].concat().join(" "),
                _ => t.join(" "),
            }
        })
        .collect()
}

#[test]
fn a_turned_world_copy_clips_and_pushes_like_retail() {
    let Some(Ours { log, lifts, .. }) = ours2() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(
        log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours"
    );
    for &(time, floor, lift) in lifts {
        if report() {
            println!("landing {time} floor {floor} lift {lift}");
        }
        assert!(
            (-0.01..=MAX_LIFT + 0.01).contains(&lift),
            "the pack landed at {time} at {floor}, {lift} under retail's rest"
        );
    }
    let text = std::fs::read_to_string(SERVER2).unwrap();
    let retail = phases(text.lines());
    let ours = phases(log.iter().map(String::as_str));
    let mut bad = compare_phases(&retail, &ours);
    let (r, o) = (events(text.lines()), events(log.iter().map(String::as_str)));
    for i in 0..r.len().max(o.len()) {
        let (a, b) = (r.get(i), o.get(i));
        if report() {
            println!("retail {a:?}\nours   {b:?}");
        }
        if a != b {
            bad.push(format!("event {i}: retail {a:?} ours {b:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// `probe_worldcopy3` to the ten-thousandth on the client's own cmds: the
/// first lift's fall slides down the copy's slope as far as retail's, the
/// turns carry the player with retail's rounding, the turned centre
/// (`CM_TransformedBoxTrace`) blocks the carried spot on the frame retail's
/// does and the jitter lands where retail's did (cod11-movers.md 21).
#[test]
fn a_pitched_copy_blocks_its_rider_like_retail() {
    pitched_matches(SERVER3, ours3(), 0.0..=MAX_LIFT);
}

/// The same on 1.5b and UO, whose `G_TryPushingEntity` stores each step of
/// the carry (cod11-movers.md 21, "The carry's rounding").
#[test]
fn a_pitched_copy_blocks_its_rider_like_retail_on_15() {
    pitched_matches(SERVER3_15, run3_on(fs_15, SERVER3_15), LIFT_15);
}

#[test]
fn a_pitched_copy_blocks_its_rider_like_retail_on_uo() {
    pitched_matches(
        SERVER3_UO,
        run3_on(vcod_common::testing::uo_fs, SERVER3_UO),
        LIFT_15,
    );
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so this runs there too).
fn fs_15() -> Option<Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

fn run3_on(fs: fn() -> Option<Pk3Fs>, fixture: &str) -> Option<Ours> {
    run_ours_on(fs, MAP, "probe_worldcopy3", PROBE3, fixture, true)
}

/// `probe_worldcopy3`'s rows, every frame to the ten-thousandth, with
/// every pack lift in `lifts`.
fn pitched_matches(
    fixture: &str,
    ours: Option<impl std::borrow::Borrow<Ours>>,
    lifts: std::ops::RangeInclusive<f32>,
) {
    let Some(ours) = ours else {
        eprintln!("no install of this version under COD_DIR: skipping");
        return;
    };
    let log = &ours.borrow().log;
    // Retail's two decimals put a lift up to 0.005 out of range.
    for &(time, floor, lift) in &ours.borrow().lifts {
        if report() {
            println!("landing {time} floor {floor} lift {lift}");
        }
        assert!(
            (lifts.start() - 0.01..=lifts.end() + 0.01).contains(&lift),
            "the pack landed at {time} at {floor}, {lift} under retail's rest"
        );
    }
    let text = std::fs::read_to_string(fixture).unwrap();
    let retail = phases(text.lines());
    let ours = phases(log.iter().map(String::as_str));
    let (rp, op) = (
        precise(text.lines()),
        precise(log.iter().map(String::as_str)),
    );
    if report() {
        for (phase, r) in &rp {
            for (k, (a, b)) in r.iter().zip(&op[phase]).enumerate() {
                println!("g {phase} {k} retail {a:?}\ng {phase} {k} ours   {b:?}");
            }
        }
    }
    let mut bad = compare_phases(&retail, &ours);
    // Every frame to the ten-thousandth: the lift's slide down the copy's
    // slope, the carry through the turns, the pitch's blocked frame and the
    // jitter's fall.
    for (phase, r) in &rp {
        if r.len() != op.get(phase).map_or(0, Vec::len) {
            bad.push(format!(
                "{phase} precise: {} rows, ours {:?}",
                r.len(),
                op.get(phase).map(Vec::len)
            ));
            continue;
        }
        for (k, (a, b)) in r.iter().zip(&op[phase]).enumerate() {
            if diff(*a, *b) > PRECISE_TOL {
                bad.push(format!("{phase} {k} precise: retail {a:?} ours {b:?}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// A turned submodel tests a body beside it with a box axial in the
/// model's frame (`CM_TransformedBoxTrace`, cod11-movers.md 22): lifted 2
/// with the player 18 to 25 from its 45-degree face, retail's slab moves
/// and the player is never pushed, where a box axial in the world's would
/// reach the face to 23.2.
#[test]
fn a_turned_submodel_tests_its_neighbour_like_retail() {
    let Some(Ours { log, .. }) = ours_slab() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(
        log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours: {log:#?}"
    );
    let text = std::fs::read_to_string(SLAB).unwrap();
    let retail = phases(text.lines());
    let ours = phases(log.iter().map(String::as_str));
    let mut bad = compare_phases(&retail, &ours);
    // Each landing's rest, to the `PROBE g` print's six digits.
    let g = |lines: &mut dyn Iterator<Item = &str>| -> Vec<Vec<f32>> {
        lines
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.find("PROBE g ").map(|i| &l[i + 8..]))
            .map(|l| {
                l.split_whitespace()
                    .skip(2)
                    .map(|v| v.parse().unwrap())
                    .collect()
            })
            .collect()
    };
    let (rg, og) = (g(&mut text.lines()), g(&mut log.iter().map(String::as_str)));
    assert_eq!(rg.len(), og.len(), "PROBE g rows");
    for (k, (a, b)) in rg.iter().zip(&og).enumerate() {
        if a.iter().zip(b).any(|(a, b)| (a - b).abs() > PRECISE_TOL) {
            bad.push(format!("g {k}: retail {a:?} ours {b:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// `probe_tiltslab`'s `PROBE g` rows: pose, time, the player off the
/// slab's centre to the ten-thousandth and whether it is on the ground.
fn tilt_rows<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<(String, [f32; 3], bool)> {
    lines
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let t: Vec<&str> = l[l.find("PROBE g ")? + 8..].split_whitespace().collect();
            let v: Vec<f32> = t[2..5].iter().map(|v| v.parse().unwrap()).collect();
            Some((t[0].to_string(), [v[0], v[1], v[2]], t[5] == "1"))
        })
        .collect()
}

/// Every `probe_tiltslab` row of `fixture` against ours' run of it, to
/// the print's ten-thousandth.
fn tilt_matches(fixture: &str, ours: Option<&'static Ours>) {
    let Some(Ours { log, .. }) = ours else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap();
    // The pose's last bits, where the capture prints them.
    let poses = |it: &mut dyn Iterator<Item = &str>| -> Vec<String> {
        it.filter_map(|l| l.find("PROBE r ").map(|i| l[i..].trim_end().to_string()))
            .collect()
    };
    let retail_poses = poses(&mut text.lines());
    if !retail_poses.is_empty() {
        assert_eq!(poses(&mut log.iter().map(String::as_str)), retail_poses);
    }
    let retail = tilt_rows(text.lines());
    let ours = tilt_rows(log.iter().map(String::as_str));
    if report() {
        for (r, o) in retail.iter().zip(&ours) {
            println!("retail {r:?}\nours   {o:?}");
        }
    }
    assert_eq!(ours.len(), retail.len(), "row count");
    let bad: Vec<String> = retail
        .iter()
        .zip(&ours)
        .filter(|(r, o)| {
            assert_eq!(r.0, o.0, "pose order");
            r.2 != o.2 || diff(r.1, o.1) > PRECISE_TOL
        })
        .map(|(r, o)| format!("retail {r:?}\nours   {o:?}"))
        .collect();
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// With retail's cmds replayed every fall lands on retail's frame and
/// place, and every rest on the tilted faces slides as far: the pose is the
/// quantized `r.currentAngles` `Reached_ScriptMover` leaves (movers doc 22).
#[test]
fn a_capsule_lands_on_a_pitched_and_a_rolled_submodel_like_retail() {
    tilt_matches(TILT, ours_tilt());
}

/// The same probe's second capture, whose `PROBE r` lines show the pose's
/// quantized angles.
#[test]
fn a_capsule_lands_on_a_tilted_submodel_like_retail_again() {
    tilt_matches(TILT_RIDE, ours_tilt_ride());
}

/// `probe_rollramp`'s capture: a player dropped onto a rolled submodel's
/// non-axial side, where the turned sphere offset's `-l.z` sign picks the
/// sphere (movers doc 22, "A rolled submodel's non-axial side").
#[test]
fn a_capsule_lands_on_a_rolled_submodels_non_axial_side_like_retail() {
    tilt_matches(ROLL, ours_roll());
}

/// The rolled ramp on retail 1.5b: its engine's `AngleVectors` rounds the
/// turn as 1.1d's does, and a trace the ramp cuts short ends where
/// `SV_Trace` puts it, its delta stored (movers doc 22, "On 1.5b and United
/// Offensive").
#[test]
fn a_capsule_lands_on_a_rolled_submodels_non_axial_side_like_retail_on_1_5() {
    tilt_matches(ROLL_15, ours_roll_15());
}

/// And on United Offensive 1.51b.
#[test]
fn a_capsule_lands_on_a_rolled_submodels_non_axial_side_like_retail_on_uo() {
    tilt_matches(ROLL_UO, ours_roll_uo());
}

/// The same capture on retail 1.5b: its `Reached_ScriptMover` truncates the
/// angles as 1.1d's does (movers doc 22).
#[test]
fn a_capsule_lands_on_a_tilted_submodel_like_retail_on_1_5() {
    tilt_matches(TILT_15, ours_tilt_15());
}

/// And on United Offensive 1.51b.
#[test]
fn a_capsule_lands_on_a_tilted_submodel_like_retail_on_uo() {
    tilt_matches(TILT_UO, ours_tilt_uo());
}

#[test]
fn a_sine_mover_crushes_what_it_cannot_push_like_retail() {
    let Some(Ours { log, killed, .. }) = ours_crush() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let lines = |it: &mut dyn Iterator<Item = &str>| -> Vec<String> {
        it.filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.find("PROBE").map(|i| l[i..].trim_end().to_string()))
            .collect()
    };
    let text = std::fs::read_to_string(CRUSH).unwrap();
    let retail: Vec<String> = lines(&mut text.lines())
        .into_iter()
        .filter(|l| !l.starts_with("PROBE_ERROR"))
        .collect();
    let ours = lines(&mut log.iter().map(String::as_str));
    if report() {
        for (a, b) in retail.iter().zip(&ours) {
            println!("retail {a}\nours   {b}");
        }
    }
    assert_eq!(ours.len(), retail.len(), "line count");
    // Replayed on retail's cmds, the corpse falls and rests where retail's
    // did (cod11-movers.md 21): every line is exact.
    for (r, o) in retail.iter().zip(&ours) {
        assert_eq!(o, r);
    }
    assert!(killed, "the bobbing drops the level one period in");
}

#[test]
fn a_spawned_brush_entity_goes_on_the_wire_like_retail() {
    let Some(Ours { log, new, .. }) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let text = std::fs::read_to_string(SERVER).unwrap();
    let retail_log: Vec<String> = text.lines().map(str::to_string).collect();
    let retail_new: BTreeMap<u32, Sent> = std::fs::read_to_string(WIRE)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("RIDE_NEW "))
        .map(parse_new)
        .collect();
    for tag in ["copy", "static"] {
        let r = tagged(&retail_log, tag).and_then(|n| retail_new.get(&n));
        let o = tagged(log, tag).and_then(|n| new.get(&n));
        if report() {
            println!("{tag} retail {r:?} ours {o:?}");
        }
        let (Some(r), Some(o)) = (r, o) else {
            panic!("{tag}: retail {r:?} ours {o:?}");
        };
        assert_eq!(
            (r.etype, r.solid, r.index, r.eflags),
            (o.etype, o.solid, o.index, o.eflags),
            "{tag}"
        );
        assert!(diff(r.origin, o.origin) < TOL, "{tag}: {r:?} ours {o:?}");
    }
}
