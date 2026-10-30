//! A walking player's bones against the retail capture.
//!
//! `movers/mp_carentan-dm-legs.txt` is the server's `games_mp.log` under
//! `client-probes/probe_legs.gsc`: from level time 25000 on, the player's
//! origin and four script_origins linked to its feet, right hand and head,
//! one `PROBE f`/`h` pair per server frame. `-legs-client.txt` is the
//! `--save-legs` client's half: every usercmd it sent and every snapshot's
//! `serverTime` and `commandTime`. Ours runs the same gametype with one
//! client that holds still until the placement, then sends retail's cmds
//! verbatim, each ahead of the frame whose snapshot first acknowledged it,
//! so both servers walk the same body on the same level times. The run,
//! stop, backpedal, strafes, diagonals and crouch walk exercise the legs
//! clip's rate and its first phase (`docs/research/cod11-movers.md` 15, "A
//! moving body's clip").
//!
//! `movers/mp_carentan-dm-torso.txt` and `-legs-client-torso.txt` are the
//! same placement under `client-probes/probe_torso.gsc` with fourteen tags
//! from the pelvis up through both arms to the weapon tag, while the client
//! fires, reloads and aims standing, running, strafing and crouched: the
//! torso clips over the legs (cod11-movers.md 15, "Torso over legs").
//!
//! `-legs-1.5.txt` / `-legs-client-1.5.txt` and `-legs-uo.txt` /
//! `-legs-client-uo.txt` are the walk on retail 1.5b and UO, which start
//! the run at their own spread phase (cod11-movers.md 15, "The phase on 1.5
//! and UO"); ours spawns where retail's probe did ([`SPAWN_15`]).
//! `-legs-stopgo-1.5.txt` and `-legs-stopgo-uo.txt` (client halves
//! `-legs-client-stopgo-*.txt`) stop and go inside the legs blends, so the
//! run comes back while it still has weight and keeps its time
//! (cod11-combat.md 3.1, "The setter on a weighted node").
//!
//! `-legs-uo-settle.txt` and `-legs-uo-spawn720.txt` (client halves
//! `-legs-client-uo-*.txt`) are the UO walk again with the join logged
//! too, the second on the spawn point the first UO walk drew: their spawn
//! idle is set on a snapshot the capture shows (cod11-movers.md 15, "The
//! spawn idle on 1.5 and UO").
//!
//! `LEGS_REPORT=1` prints every compared frame; `LEGS_IDLE_MS` sweeps the
//! 1.5 and UO join.
//!
//! Needs `COD_DIR`; without the paks the test returns early. The 1.1 walks
//! run at a 1.1 install, the 1.5 one at a 1.5 or UO install and the UO one at a
//! UO install.

use crate::common;

use common::{ADDR, ClientEnd, FRAME_MS, Join, Queues, holding, step_at};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, Protocol};
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-legs.txt";
const CLIENT: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client.txt";
const TORSO_SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-torso.txt";
const TORSO_CLIENT: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-torso.txt";
const SERVER_15: &str = "tests/fixtures/movers/mp_carentan-dm-legs-1.5.txt";
const CLIENT_15: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-1.5.txt";
const SERVER_UO: &str = "tests/fixtures/movers/mp_carentan-dm-legs-uo.txt";
const CLIENT_UO: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-uo.txt";
const SERVER_UO_SETTLE: &str = "tests/fixtures/movers/mp_carentan-dm-legs-uo-settle.txt";
const CLIENT_UO_SETTLE: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-uo-settle.txt";
const SERVER_UO_720: &str = "tests/fixtures/movers/mp_carentan-dm-legs-uo-spawn720.txt";
const CLIENT_UO_720: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-uo-spawn720.txt";
const PLACEBITS: &str = "tests/fixtures/spawnpoints/placebits-retail.txt";
const STOPGO_15: &str = "tests/fixtures/movers/mp_carentan-dm-legs-stopgo-1.5.txt";
const STOPGO_CLIENT_15: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-stopgo-1.5.txt";
const STOPGO_UO: &str = "tests/fixtures/movers/mp_carentan-dm-legs-stopgo-uo.txt";
const STOPGO_CLIENT_UO: &str = "tests/fixtures/movers/mp_carentan-dm-legs-client-stopgo-uo.txt";

/// The player's origin, to `logPrint`'s hundredth.
const ORIGIN_TOL: f32 = 0.02;

/// A bone's origin and its turn, in degrees, off retail's on a body whose
/// legs clip has played alone for a while: the run, backpedal, strafes,
/// diagonals, crouch walk and the spawn idle before them, rate and phase
/// both. Ours reads 0.03 and 0.13 at worst, with the keys rebuilt as the
/// engine does (xanim-v14-format.md, "Rotation keys as the engine rebuilds
/// them").
const STEADY_TOL: f32 = 0.05;
const STEADY_TURN: f32 = 0.2;

/// The same within [`BLEND_MS`] of a legs change, while the old clip blends
/// out: ours reads 0.03 and 0.21 at worst, the clips summed as the engine
/// sums them.
const BLEND_TOL: f32 = 0.05;
const BLEND_TURN: f32 = 0.3;

/// The longest blend the setter gives (cod11-movers.md 15).
const BLEND_MS: i32 = 250;

/// The level time retail's spawn set the idle, which plays from there
/// untouched until the run: the bones fit it to 0.11 degrees, and the
/// client's netchan sequence (389 at 25000) puts its join a few frames
/// before (cod11-movers.md 15, "The spawn idle's phase").
const RETAIL_IDLE_MS: i32 = 5750;

/// The torso capture's [`RETAIL_IDLE_MS`], 37 snapshots (1850 ms) later
/// than the walk's as its placement snapshot is message 352 where the walk's
/// is 389. The settle reads 0.08 degrees off there, 11 at 7500 and 7700.
/// `TORSO_IDLE_MS` overrides it for a sweep.
const TORSO_IDLE_MS: i32 = 7600;

/// The netchan message whose snapshot first shows a 1.5b or UO walk's
/// spawn idle, one snapshot a frame since the gamestate: 5 in the
/// `-legs-uo-settle` and `-legs-uo-spawn720` captures, which log the join.
/// A capture that starts at the placement dates its idle from the
/// placement's message; the 1.5b walks and the UO walk fit only on 5, the
/// UO stop-and-go only on 6 ([`STOPGO_SPAWN_MSG_UO`]; cod11-movers.md 15,
/// "The spawn idle on 1.5 and UO"). `LEGS_IDLE_MS` overrides it for a
/// sweep.
const SPAWN_MSG: i32 = 5;
const STOPGO_SPAWN_MSG_UO: i32 = 6;

/// When retail's spawn set the idle on a 1.5b or UO walk: the logged join's
/// snapshot, else message `spawn_msg`'s.
fn retail_idle_ms(walk: &Walk, spawn_msg: i32) -> i32 {
    let placed = &walk.snaps[0];
    walk.spawned
        .unwrap_or(placed.t - (placed.msg - spawn_msg) * FRAME_MS as i32)
}

/// Where each walk's probe spawned (its log's `ps.origin` until the
/// placement): 1.5's and UO's controllers ease from the spawn's view.
const SPAWN_15: [f32; 2] = [-952.0, 2296.0];
const SPAWN_UO: [f32; 2] = [720.0, -32.0];

/// Where the settle walk's probe spawned.
const SPAWN_UO_SETTLE: [f32; 2] = [426.0, 2158.0];

/// The stop-and-go walks' [`SPAWN_15`].
const STOPGO_SPAWN_15: [f32; 2] = [144.0, 1808.0];
const STOPGO_SPAWN_UO: [f32; 2] = [776.0, 3488.0];

/// Ours' first snapshot showing the idle comes 800 ms after the client
/// starts connecting on a fresh server (900 on 1.5 and 1050 on UO, whose
/// gametypes open `serverinfo_<gametype>` before the team menu), so the
/// client starts this many frames in for an idle set at `idle_ms`.
fn join_frames(idle_ms: i32, proto: &Protocol) -> u32 {
    let join_ms = match proto.transport.version {
        v if v == PROTOCOL_V1.transport.version => 800,
        v if v == PROTOCOL_V6.transport.version => 900,
        _ => 1050,
    };
    (idle_ms - join_ms) as u32 / FRAME_MS as u32
}

fn report() -> bool {
    std::env::var("LEGS_REPORT").is_ok_and(|v| v == "1")
}

/// `(x, y, z)` as retail's `logPrint` renders a vector.
fn vectors(line: &str) -> Vec<[f32; 3]> {
    line.split('(')
        .skip(1)
        .filter_map(|s| {
            let inner = s.split(')').next()?;
            let v: Vec<f32> = inner
                .split(',')
                .filter_map(|t| t.trim().parse().ok())
                .collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .collect()
}

/// One frame of the bone log: the player's origin, then each bone's origin
/// and angles (l foot, r foot, hand, head).
#[derive(Clone, Debug, Default)]
struct Frame {
    origin: [f32; 3],
    bones: Vec<([f32; 3], [f32; 3])>,
}

fn frames<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<i32, Frame> {
    let mut out: BTreeMap<i32, Frame> = BTreeMap::new();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE ").map(|i| &line[i + 6..]) else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        let (Some(kind), Some(time)) = (t.first(), t.get(1).and_then(|s| s.parse().ok())) else {
            continue;
        };
        let v = vectors(rest);
        let f = out.entry(time).or_default();
        match *kind {
            "f" => {
                f.origin = v[0];
                f.bones.splice(0..0, [(v[1], v[2]), (v[3], v[4])]);
            }
            "h" => f.bones.extend([(v[0], v[1]), (v[2], v[3])]),
            _ => {}
        }
    }
    out
}

/// The torso log's frames: the player's origin, then the fourteen tags in
/// `probe_torso.gsc`'s order.
fn torso_frames<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<i32, Frame> {
    let mut out: BTreeMap<i32, Frame> = BTreeMap::new();
    for line in lines.filter(|l| !l.starts_with('#')) {
        let Some(rest) = line.find("PROBE ").map(|i| &line[i + 6..]) else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        let (Some(kind), Some(time)) = (t.first(), t.get(1).and_then(|s| s.parse().ok())) else {
            continue;
        };
        let v = vectors(rest);
        let f = out.entry(time).or_default();
        match *kind {
            "o" => f.origin = v[0],
            "b" => f.bones.push((v[0], v[1])),
            _ => {}
        }
    }
    out
}

/// The torso log's tags, by index.
const TORSO_TAGS: [&str; 14] = [
    "pelvis",
    "spine",
    "spine2",
    "neck",
    "head",
    "l upperarm",
    "l forearm",
    "l hand",
    "r upperarm",
    "r forearm",
    "r hand",
    "tag_weapon_right",
    "l foot",
    "r foot",
];

/// One snapshot of the client half.
struct Snap {
    msg: i32,
    t: i32,
    ct: i32,
    step: String,
    legs: i32,
    dir: i32,
    torso: i32,
}

/// The client half: every cmd, and each snapshot.
struct Walk {
    cmds: Vec<(UserCmd, [i32; 3])>,
    snaps: Vec<Snap>,
    /// The level time of the first `!psnap` (a capture that logged the
    /// join) whose legs anim is set: the spawn's idle.
    spawned: Option<i32>,
}

fn walk(text: &str) -> Walk {
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut spawned = None;
    for line in text.lines() {
        let kv: BTreeMap<&str, &str> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|w| w.split_once('='))
            .collect();
        let n = |k: &str| kv[k].parse::<i32>().unwrap();
        let v3 = |k: &str| -> [i32; 3] {
            let v: Vec<i32> = kv[k].split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        };
        if line.starts_with("!psnap") && spawned.is_none() && n("legs") != 0 {
            spawned = Some(n("t"));
        } else if line.starts_with("!cmd") {
            let cmd = UserCmd {
                server_time: n("st"),
                buttons: n("buttons") as u8,
                wbuttons: n("wbuttons") as u8,
                weapon: n("weapon") as u8,
                up: n("up") as i8,
                forward: n("forward") as i8,
                right: n("right") as i8,
                ..NULL_USERCMD
            };
            cmds.push((cmd, v3("world")));
        } else if line.starts_with("!snap") {
            snaps.push(Snap {
                msg: n("msg"),
                t: n("t"),
                ct: n("ct"),
                step: kv["step"].to_string(),
                legs: n("legs"),
                dir: n("movementDir"),
                torso: n("torso"),
            });
        }
    }
    Walk {
        cmds,
        snaps,
        spawned,
    }
}

/// Ours' bone log over the same walk, and its `(legsAnim, movementDir,
/// torsoAnim)` by serverTime.
struct Ours {
    log: Vec<String>,
    legs: BTreeMap<i32, (i32, i32, i32)>,
}

fn ours() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        run_ours(
            vcod_common::testing::game_fs_11()?,
            "probe_legs",
            include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_legs.gsc"),
            CLIENT,
            RETAIL_IDLE_MS,
            None,
        )
    })
    .as_ref()
}

/// Ours over a 1.5 or UO walk, on `fs`, spawned on the spawn point at
/// `spawn` (x, y), where retail's probe sat until the placement, with its
/// idle set when retail's was ([`retail_idle_ms`] off `spawn_msg`).
fn ours_walk(
    fs: vcod_common::pk3::Pk3Fs,
    client: &str,
    spawn: [f32; 2],
    spawn_msg: i32,
) -> Option<Ours> {
    let idle_ms = std::env::var("LEGS_IDLE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            retail_idle_ms(&walk(&std::fs::read_to_string(client).unwrap()), spawn_msg)
        });
    let (origin, angles) = common::spawn_points(&fs, MAP)
        .into_iter()
        .find(|(o, _)| (o[0] - spawn[0]).abs() < 1.0 && (o[1] - spawn[1]).abs() < 1.0)
        .expect("retail's spawn point");
    // The point as the gametype's `placeSpawnpoint` dropped it, where
    // retail's spawn stood (cod11-gsc-object-model.md 16.2); from the
    // lump's origin ours' body fell onto it and set its idle late.
    let pin = (placed_spawn(&fs, spawn).unwrap_or(origin), angles);
    run_ours(
        fs,
        "probe_legs",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_legs.gsc"),
        client,
        idle_ms,
        Some(pin),
    )
}

/// Retail's `placeSpawnpoint` height for the mp_carentan deathmatch point
/// at `xy` on `fs`'s server, off `placebits-retail.txt`.
fn placed_spawn(fs: &vcod_common::pk3::Pk3Fs, xy: [f32; 2]) -> Option<[f32; 3]> {
    let server = match vcod_server::server::install_protocol(fs).transport.version {
        v if v == PROTOCOL_V6.transport.version => "1.5b",
        v if v == PROTOCOL_V1.transport.version => "1.1d",
        _ => "uo",
    };
    let text = std::fs::read_to_string(PLACEBITS).unwrap();
    let mut lines = text
        .lines()
        .skip_while(|l| *l != format!("## {server} {MAP}"))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .skip_while(|l| !l.starts_with("PROBE class mp_deathmatch_spawn "))
        .skip(1)
        .take_while(|l| !l.starts_with("PROBE class "));
    lines.find_map(|l| {
        let v: Vec<f32> = l
            .split_whitespace()
            .skip(3)
            .take(3)
            .map(|t| t.parse().unwrap())
            .collect();
        (v[0] == xy[0] && v[1] == xy[1]).then(|| [v[0], v[1], v[2]])
    })
}

fn ours_torso() -> Option<&'static Ours> {
    static RUN: std::sync::OnceLock<Option<Ours>> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        run_ours(
            vcod_common::testing::game_fs_11()?,
            "probe_torso",
            include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_torso.gsc"),
            TORSO_CLIENT,
            std::env::var("TORSO_IDLE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(TORSO_IDLE_MS),
            None,
        )
    })
    .as_ref()
}

/// Ours under `gametype` replaying the client half at `client`.
fn run_ours(
    fs: vcod_common::pk3::Pk3Fs,
    gametype: &str,
    script: &str,
    client: &str,
    idle_ms: i32,
    spawn: Option<([f32; 3], [f32; 3])>,
) -> Option<Ours> {
    let proto = vcod_server::server::install_protocol(&fs);
    let mut walk = walk(&std::fs::read_to_string(client).unwrap());
    let first = walk.snaps.first().expect("a snapshot");
    let (placed, placed_ct) = (first.t, first.ct);
    // The probe sent a cmd at the placement's own time, before it saw the
    // placement, and captures before 2026-10-21 did not log it (its cmds
    // end each frame on the frame's time): the fall from the placement runs
    // from retail's commandTime through it (cod11-movers.md 15, "The cmd at
    // the placement").
    if let Some(&(c, world)) = walk.cmds.first()
        && c.server_time > placed
    {
        walk.cmds.insert(
            0,
            (
                UserCmd {
                    server_time: placed,
                    ..c
                },
                world,
            ),
        );
    }
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, gametype), now);
    sv.set_protocol(proto);
    sv.overlay_script(&format!("maps/mp/gametypes/{gametype}"), script);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    if spawn.is_some() {
        sv.pin_spawns(vec![vec![spawn]]);
    }
    for _ in 0..join_frames(idle_ms, proto) {
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick(now);
    }
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_protocol(ClientEnd(q.clone()), now, 0x2001, proto);
    let mut join = Join::new("allies", "m1carbine_mp");
    let p = proto;
    let acked: BTreeMap<i32, i32> = walk.snaps.iter().map(|s| (s.t, s.ct)).collect();
    let mut legs = BTreeMap::new();
    let last = walk.snaps.last().unwrap().t;
    let mut next_cmd = 0;
    let mut last_st = i32::MIN;
    loop {
        let newest = cl.snapshots().newest().map(|s| s.server_time);
        let t = newest.map_or(0, |t| t + FRAME_MS as i32);
        if t > last {
            break;
        }
        if t < placed {
            cl.send_frame(&holding(&cl));
        } else if t == placed {
            // The placement frame runs the cmds up to retail's commandTime.
            let mut c = holding(&cl);
            c.server_time = placed_ct;
            c.angles = cl.snapshots().newest().map_or([0; 3], |s| {
                [0, 1, 2].map(|i| -s.ps.field_i32(p, &format!("delta_angles[{i}]")) & 0xffff)
            });
            cl.send_cmds(&[c]);
        } else {
            // Retail's cmds up to what the snapshot of frame `t` had
            // acknowledged, so this frame runs the moves retail's did.
            let ct = acked
                .range(..=t)
                .next_back()
                .map_or(i32::MIN, |(_, ct)| *ct);
            let delta = cl.snapshots().newest().map_or([0; 3], |s| {
                [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
            });
            let mut batch = Vec::new();
            while let Some((cmd, world)) = walk.cmds.get(next_cmd)
                && cmd.server_time <= ct
            {
                next_cmd += 1;
                if cmd.server_time <= last_st {
                    continue;
                }
                last_st = cmd.server_time;
                let mut c = *cmd;
                c.angles = [0, 1, 2].map(|i| (world[i] - delta[i]) & 0xffff);
                batch.push(c);
            }
            cl.send_cmds(&batch);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        for e in step_at(&mut sv, ADDR, &q, &mut cl, now) {
            match e {
                NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, &mut cl, now),
                NetEvent::Dropped(r) => panic!("dropped: {r}"),
                _ => {}
            }
        }
        assert!(sv.script_aborts().is_empty(), "{:?}", sv.script_aborts());
        if let Some(s) = cl.snapshots().newest() {
            if legs.values().all(|l: &(i32, i32, i32)| l.0 == 0) {
                let l = s.ps.field_i32(p, "legsAnim");
                let retail = walk.spawned.unwrap_or(idle_ms);
                assert!(
                    l == 0 || s.server_time == retail,
                    "ours set the spawn idle at {}, not retail's {retail}",
                    s.server_time
                );
            }
            legs.insert(
                s.server_time,
                (
                    s.ps.field_i32(p, "legsAnim"),
                    s.ps.field_i32(p, "movementDir"),
                    s.ps.field_i32(p, "torsoAnim"),
                ),
            );
        }
    }
    Some(Ours {
        log: sv.script_log().to_vec(),
        legs,
    })
}

/// The largest component difference.
fn off(r: [f32; 3], o: [f32; 3]) -> f32 {
    (0..3).map(|i| (r[i] - o[i]).abs()).fold(0.0, f32::max)
}

/// The rotation between two `(pitch, yaw, roll)` triples, in degrees: a foot
/// sits near pitch 90, where yaw and roll trade off and compare apart.
fn turn(r: [f32; 3], o: [f32; 3]) -> f32 {
    let q = |a: [f32; 3]| {
        let [p, y, r] = a.map(f32::to_radians);
        glam::Quat::from_rotation_z(y)
            * glam::Quat::from_rotation_y(p)
            * glam::Quat::from_rotation_x(r)
    };
    q(r).angle_between(q(o)).to_degrees()
}

#[test]
fn a_walking_body_is_posed_like_retail() {
    let Some(run) = ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    check_walk(run, SERVER, CLIENT);
}

/// The 1.5b walk (`-legs-1.5.txt`): the run out of the settle idle starts
/// at 1.5's spread phase (cod11-movers.md 15, "The phase on 1.5 and UO").
#[test]
fn a_walking_body_is_posed_like_retail_on_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let run = ours_walk(fs, CLIENT_15, SPAWN_15, SPAWN_MSG).unwrap();
    check_walk(&run, SERVER_15, CLIENT_15);
}

/// The UO walk (`-legs-uo.txt`), as [`a_walking_body_is_posed_like_retail_on_15`].
#[test]
fn a_walking_body_is_posed_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let run = ours_walk(fs, CLIENT_UO, SPAWN_UO, SPAWN_MSG).unwrap();
    check_walk(&run, SERVER_UO, CLIENT_UO);
}

/// The UO walk whose client half logs its join (`-legs-uo-settle.txt`):
/// the spawn's idle is set on a snapshot the capture shows, so the settle
/// has no unknown phase.
#[test]
fn a_body_settling_from_a_logged_spawn_is_posed_like_retail_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    for (server, client, spawn) in [
        (SERVER_UO_SETTLE, CLIENT_UO_SETTLE, SPAWN_UO_SETTLE),
        (SERVER_UO_720, CLIENT_UO_720, SPAWN_UO),
    ] {
        assert!(
            walk(&std::fs::read_to_string(client).unwrap())
                .spawned
                .is_some(),
            "{client} logs the join"
        );
        let run = ours_walk(
            vcod_common::testing::uo_fs().unwrap(),
            client,
            spawn,
            SPAWN_MSG,
        );
        check_walk(&run.unwrap(), server, client);
    }
}

/// The stop-and-go walk on `fs` (1.5b or UO): retail's capture, exact
/// past the settle and the first run, and one the old setter misses.
fn check_stop_and_go(
    fs: impl Fn() -> vcod_common::pk3::Pk3Fs,
    server: &str,
    client: &str,
    spawn: [f32; 2],
    spawn_msg: i32,
) {
    let run = ours_walk(fs(), client, spawn, spawn_msg).unwrap();
    let (compared, bad) = walk_rows(&run, server, client);
    assert!(compared > 300, "only {compared} frames compared");
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    // The setter before cod11-combat.md 3.1's "The setter on a weighted
    // node" restarts the run each go takes back: those rows, and only
    // rows after the settle, must part from retail's.
    vcod_common::animclock::restart_weighted_nodes(true);
    let old = ours_walk(fs(), client, spawn, spawn_msg);
    vcod_common::animclock::restart_weighted_nodes(false);
    let (_, old_bad) = walk_rows(&old.unwrap(), server, client);
    let goes = old_bad
        .iter()
        .filter(|r| {
            STOPGO_TAKEN_BACK
                .iter()
                .any(|s| r.contains(&format!(" {s}")))
        })
        .count();
    if report() {
        println!(
            "old setter: {} rows differ\n{}",
            old_bad.len(),
            old_bad.join("\n")
        );
    }
    assert!(goes > 0, "the old setter matches retail's stop and go");
}

/// The stop-and-go steps whose legs take back a clip with weight left.
const STOPGO_TAKEN_BACK: &[&str] = &["back_go", "go2", "go3", "go4", "strafe_right2"];

/// The 1.5b stop-and-go walk: every go inside a blend keeps the run's time.
#[test]
fn a_body_stopping_and_going_is_posed_like_retail_on_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check_stop_and_go(
        || vcod_common::testing::game_fs().unwrap(),
        STOPGO_15,
        STOPGO_CLIENT_15,
        STOPGO_SPAWN_15,
        SPAWN_MSG,
    );
}

/// The UO stop-and-go walk, as [`a_body_stopping_and_going_is_posed_like_retail_on_15`].
#[test]
fn a_body_stopping_and_going_is_posed_like_retail_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    check_stop_and_go(
        || vcod_common::testing::uo_fs().unwrap(),
        STOPGO_UO,
        STOPGO_CLIENT_UO,
        STOPGO_SPAWN_UO,
        STOPGO_SPAWN_MSG_UO,
    );
}

/// The anim selection and the four bones of `run` against the retail walk
/// at `server` and `client`.
fn check_walk(run: &Ours, server: &str, client: &str) {
    let (compared, bad) = walk_rows(run, server, client);
    assert!(compared > 400, "only {compared} frames compared");
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// [`check_walk`]'s compared frames and the rows that differ.
fn walk_rows(run: &Ours, server: &str, client: &str) -> (usize, Vec<String>) {
    let retail = frames(std::fs::read_to_string(server).unwrap().lines());
    let ours = frames(run.log.iter().map(String::as_str));
    let walk = walk(&std::fs::read_to_string(client).unwrap());
    let step: BTreeMap<i32, &str> = walk.snaps.iter().map(|s| (s.t, s.step.as_str())).collect();
    let mut bad = Vec::new();
    // The anim selection first: the same index and movementDir on the same
    // frame, so a bone row that differs is the pose's doing.
    for s in &walk.snaps {
        let (t, l, dir) = (s.t, s.legs, s.dir);
        let o = run.legs.get(&t).map(|o| (o.0, o.1));
        let differs = o.is_some_and(|o| o.0 & 511 != l & 511 || o.1 != dir);
        if differs || report() && o != Some((l, dir)) {
            println!("{t} legsAnim, movementDir: retail {l} {dir} ours {o:?}");
        }
        if differs {
            bad.push(format!(
                "{t} legsAnim, movementDir: retail {l} {dir} ours {o:?}"
            ));
        }
    }
    // The end frames that changed the legs anim, toggle included.
    let changes: std::collections::BTreeSet<i32> = walk
        .snaps
        .windows(2)
        .filter(|w| w[0].legs != w[1].legs)
        .map(|w| w[1].t)
        .collect();
    let mut worst: BTreeMap<String, (f32, f32)> = BTreeMap::new();
    let mut compared = 0;
    for (t, r) in &retail {
        // The placement frame poses nothing yet: the links are made after it.
        let Some(o) = ours.get(t).filter(|_| r.bones.len() == 4) else {
            continue;
        };
        compared += 1;
        let label = step
            .range(..=*t)
            .next_back()
            .map_or("?", |(_, s)| *s)
            .to_string();
        let d_origin = off(r.origin, o.origin);
        if d_origin > ORIGIN_TOL {
            bad.push(format!(
                "{t} {label} origin: retail {:?} ours {:?}",
                r.origin, o.origin
            ));
        }
        let (mut dp, mut da) = (0.0f32, 0.0f32);
        for (rb, ob) in r.bones.iter().zip(&o.bones) {
            dp = dp.max(off(rb.0, ob.0));
            da = da.max(turn(rb.1, ob.1));
        }
        let w = worst.entry(label.clone()).or_default();
        w.0 = w.0.max(dp);
        w.1 = w.1.max(da);
        // The row logs the pose the frame before it took (movers doc 15),
        // and a blend counts from the end frame that set the clip.
        let posed = t - FRAME_MS as i32;
        let blending = changes
            .range(..=posed)
            .next_back()
            .is_some_and(|c| posed - c < BLEND_MS);
        let (tol, turn_tol) = if blending {
            (BLEND_TOL, BLEND_TURN)
        } else {
            (STEADY_TOL, STEADY_TURN)
        };
        if dp > tol || da > turn_tol {
            bad.push(format!(
                "{t} {label}{}: bones off {dp:.2}, turned {da:.2}",
                if blending { " (blending)" } else { "" }
            ));
        }
        if report() {
            println!(
                "{t} {label} origin {d_origin:.2} bones {dp:.2} angles {da:.2}{}",
                if blending { " blending" } else { "" }
            );
            for (k, (rb, ob)) in r.bones.iter().zip(&o.bones).enumerate() {
                println!(
                    "   {k} retail {:?} {:?}\n   {k} ours   {:?} {:?}",
                    rb.0, rb.1, ob.0, ob.1
                );
            }
        }
    }
    if report() {
        for (label, (p, a)) in &worst {
            println!("{label}: worst bone origin {p:.2}, turn {a:.2}");
        }
    }
    (compared, bad)
}

/// The cgame's copy of the clock (`cgame_mp_x86.dll` 0x30003be0) on
/// retail's own snapshots of the walk: a body drawn every `draw_ms`, each
/// snapshot arriving [`LEAD_MS`] before the drawn clock reaches its
/// `serverTime`, starts its clips and blends on the drawn clock and measures
/// its rate whenever the newest snapshot moves. Returns how many drawn
/// frames were compared and how far, at worst, a foot sat off where the end
/// frame's clock puts it `LEAD_MS` later; `None` without the paks.
fn drawn_feet(draw_ms: i32) -> Option<(usize, f32)> {
    use vcod_common::animclock::{ChannelClock, Seen, Times};
    use vcod_common::playerpose::{PoseInputs, pose_player};
    let fs = vcod_common::testing::game_fs_11()?;
    let anims = vcod_common::animtree::PlayerAnims::load(&fs).unwrap();
    let body = vcod_common::model::load(&fs, "playerbody_american_airborne").unwrap();
    let skel = vcod_common::skeleton::Skeleton::build(&[&body]);
    let feet = ["bip01 l foot", "bip01 r foot"].map(|n| skel.bone_index(n).expect(n));
    let mut clips: BTreeMap<String, Option<Rc<vcod_common::xanim::XAnim>>> = BTreeMap::new();
    let mut feet_of = |legs: &ChannelClock, torso: &ChannelClock, now: i32| {
        let inputs = PoseInputs {
            anims: &anims,
            legs: 0,
            torso: 0,
            legs_layers: legs.layers(now),
            torso_layers: torso.layers(now),
            nodes: torso.node_weights(now),
            group_pitch: 0.0,
            turret_leaves: None,
            turret_left: None,
            seat_leaves: Vec::new(),
            controllers: None,
            leaf_times: Vec::new(),
            node_time: false,
            binds: None,
        };
        let pose = pose_player(&skel, &inputs, |n| {
            clips
                .entry(n.to_string())
                .or_insert_with(|| vcod_common::xanim::load(&fs, n).ok().map(Rc::new))
                .clone()
        });
        feet.map(|b| pose.bone_world(&skel, b).0)
    };
    // `(serverTime, trBase, legsAnim, torsoAnim)` per snapshot.
    let snaps: Vec<(i32, glam::Vec3, i32, i32)> = std::fs::read_to_string(CLIENT)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("!snap"))
        .map(|line| {
            let kv: BTreeMap<&str, &str> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|w| w.split_once('='))
                .collect();
            let o: Vec<f32> = kv["origin"]
                .split(',')
                .map(|x| x.parse().unwrap())
                .collect();
            let n = |k: &str| kv[k].parse::<i32>().unwrap();
            let tr_base = glam::Vec3::new(o[0], o[1], o[2]).trunc();
            (n("t"), tr_base, n("legs"), n("torso"))
        })
        .collect();
    let seen = |tr_base| Seen {
        client_num: 0,
        tr_base,
        eflags: 0,
    };
    // The end frame's clocks, kept at every snapshot.
    let mut end = (ChannelClock::default(), ChannelClock::default());
    let mut ends = BTreeMap::new();
    for &(t, tr, l, to) in &snaps {
        end.0
            .update(&anims, true, l, &seen(tr), Times::end_frame(t));
        end.1
            .update(&anims, false, to, &seen(tr), Times::end_frame(t));
        ends.insert(t, end);
    }
    let mut drawn = (ChannelClock::default(), ChannelClock::default());
    let (first, last) = (snaps[0].0, snaps.last().unwrap().0);
    let (mut worst, mut compared) = (0.0f32, 0);
    let mut r = first - LEAD_MS;
    while r + LEAD_MS <= last {
        let &(t, tr, l, to) = snaps.iter().rev().find(|s| s.0 - LEAD_MS <= r)?;
        let at = Times {
            clock_ms: r,
            snapshot_ms: t,
        };
        drawn.0.update(&anims, true, l, &seen(tr), at);
        drawn.1.update(&anims, false, to, &seen(tr), at);
        let later = r + LEAD_MS;
        // Between two end frames the end frame's clock runs on from the
        // last one.
        let (_, e) = ends.range(..=later).next_back().unwrap();
        {
            let a = feet_of(&drawn.0, &drawn.1, r);
            let b = feet_of(&e.0, &e.1, later);
            let d = a[0].distance(b[0]).max(a[1].distance(b[1]));
            if report() && d > 0.1 {
                println!("drawn {r}: feet {d:.2} off the end frame's at {later}");
            }
            worst = worst.max(d);
            compared += 1;
        }
        r += draw_ms;
    }
    println!("drawn every {draw_ms} ms: {compared} frames, feet at worst {worst:.2} off");
    Some((compared, worst))
}

/// How long before the drawn clock reaches a snapshot's `serverTime` the
/// snapshot arrives: one snapshot interval, as `cg.nextSnap` runs.
const LEAD_MS: i32 = 50;

/// Drawn on a cadence that meets every arrival, the cgame's clock is the end
/// frame's run a snapshot early: the setter, the rate and the blend all land
/// on the same clip times (cod11-movers.md 15, "The drawn body's clip").
#[test]
fn a_drawn_body_runs_the_end_frame_clips_a_snapshot_early() {
    let Some((compared, worst)) = drawn_feet(10) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(compared > 1500, "only {compared} drawn frames compared");
    assert!(worst < 0.01, "feet {worst:.2} off the end frame's");
}

/// On a cadence that misses the arrivals each change lands on the first
/// drawn frame past it, up to 7 ms late. A steady clip then reads under 1.9
/// units off, a strafe-to-strafe blend 2.9 at worst while it runs, and
/// nothing piles up over the walk.
#[test]
fn a_drawn_body_off_the_arrivals_stays_within_a_frame_of_clip() {
    let Some((compared, worst)) = drawn_feet(7) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    assert!(compared > 2000, "only {compared} drawn frames compared");
    assert!(worst < 3.0, "feet {worst:.2} off the end frame's");
}

/// A bone's origin and turn off retail's, blends and the torso clips'
/// blend-outs included: ours reads 0.04 and 0.13 at worst.
const TORSO_TOL: f32 = 0.05;
const TORSO_TURN: f32 = 0.2;

/// Fire, reload and aim on the move: the anim selection on every snapshot
/// and fourteen bones from the pelvis to the weapon tag on every frame
/// (cod11-movers.md 15, "Torso over legs").
#[test]
fn a_body_firing_and_reloading_on_the_move_is_posed_like_retail() {
    let Some(run) = ours_torso() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let retail = torso_frames(std::fs::read_to_string(TORSO_SERVER).unwrap().lines());
    let ours = torso_frames(run.log.iter().map(String::as_str));
    let walk = walk(&std::fs::read_to_string(TORSO_CLIENT).unwrap());
    let step: BTreeMap<i32, &str> = walk.snaps.iter().map(|s| (s.t, s.step.as_str())).collect();
    let mut bad = Vec::new();
    for s in &walk.snaps {
        let r = (s.legs, s.dir, s.torso);
        let o = run.legs.get(&s.t);
        let differs =
            o.is_some_and(|o| o.0 & 511 != r.0 & 511 || o.1 != r.1 || o.2 & 511 != r.2 & 511);
        if differs || report() && o != Some(&r) {
            println!(
                "{} {} legs, dir, torso: retail {r:?} ours {o:?}",
                s.t, s.step
            );
        }
        if differs {
            bad.push(format!(
                "{} {} legs, dir, torso: retail {r:?} ours {o:?}",
                s.t, s.step
            ));
        }
    }
    let changes: std::collections::BTreeSet<i32> = walk
        .snaps
        .windows(2)
        .filter(|w| w[0].legs != w[1].legs || w[0].torso != w[1].torso)
        .map(|w| w[1].t)
        .collect();
    let mut worst: BTreeMap<String, (f32, f32)> = BTreeMap::new();
    let mut worst_bone = [(0.0f32, 0.0f32); TORSO_TAGS.len()];
    let mut compared = 0;
    for (t, r) in &retail {
        let Some(o) = ours.get(t).filter(|_| r.bones.len() == TORSO_TAGS.len()) else {
            continue;
        };
        // The placement frame poses nothing yet.
        if *t == walk.snaps[0].t {
            continue;
        }
        compared += 1;
        let label = step
            .range(..=*t)
            .next_back()
            .map_or("?", |(_, s)| *s)
            .to_string();
        if off(r.origin, o.origin) > ORIGIN_TOL {
            bad.push(format!(
                "{t} {label} origin: retail {:?} ours {:?}",
                r.origin, o.origin
            ));
        }
        let (mut dp, mut da) = (0.0f32, 0.0f32);
        for (k, (rb, ob)) in r.bones.iter().zip(&o.bones).enumerate() {
            let (p, a) = (off(rb.0, ob.0), turn(rb.1, ob.1));
            worst_bone[k].0 = worst_bone[k].0.max(p);
            worst_bone[k].1 = worst_bone[k].1.max(a);
            dp = dp.max(p);
            da = da.max(a);
        }
        let w = worst.entry(label.clone()).or_default();
        w.0 = w.0.max(dp);
        w.1 = w.1.max(da);
        let posed = t - FRAME_MS as i32;
        let blending = changes
            .range(..=posed)
            .next_back()
            .is_some_and(|c| posed - c < BLEND_MS);
        if dp > TORSO_TOL || da > TORSO_TURN {
            bad.push(format!(
                "{t} {label}{}: bones off {dp:.2}, turned {da:.2}",
                if blending { " (blending)" } else { "" }
            ));
        }
        if report() {
            println!(
                "{t} {label} bones {dp:.2} angles {da:.2}{}",
                if blending { " blending" } else { "" }
            );
            for (k, (rb, ob)) in r.bones.iter().zip(&o.bones).enumerate() {
                let (p, a) = (off(rb.0, ob.0), turn(rb.1, ob.1));
                if p > 0.05 || a > 0.3 {
                    println!(
                        "   {} retail {:?} {:?}\n   {} ours   {:?} {:?}",
                        TORSO_TAGS[k], rb.0, rb.1, TORSO_TAGS[k], ob.0, ob.1
                    );
                }
            }
        }
    }
    if report() {
        for (label, (p, a)) in &worst {
            println!("{label}: worst bone origin {p:.2}, turn {a:.2}");
        }
        for (k, (p, a)) in worst_bone.iter().enumerate() {
            println!("{}: worst origin {p:.2}, turn {a:.2}", TORSO_TAGS[k]);
        }
    }
    assert!(compared > 500, "only {compared} frames compared");
    assert!(
        bad.is_empty(),
        "{} rows differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
