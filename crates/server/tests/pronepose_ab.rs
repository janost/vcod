//! A prone body's bones, ours against retail's. `client-probes/probe_pronebones`
//! puts one prone client on a spot (`probe_spot`) twice, a second apart,
//! and once he has lain still logs a script_origin linked to each of eight
//! bones every frame for 2 s. The fixtures are retail 1.1d's, 1.5b's and
//! United Offensive 1.51b's: mp_carentan's ~19 degree mound, a flat spot on
//! mp_carentan and probe_scriptmodel's prone target on mp_brecourt's field.
//! Each bone is compared off the body's origin, which a fall onto the grade
//! leaves a fraction of a unit apart run to run
//! (docs/research/player-model-anim-system.md, "The prone body on a grade").
//!
//! On 1.5 and UO the body keeps the bend its last airborne cmd wrote
//! (cod15-movement.md 4, "The airborne prone body"), so ours' cmds run on
//! the probe client's 16, 16, 18 cadence and the playerstate's
//! `fTorsoHeight`, `fTorsoPitch` and `fWaistPitch` are held to the
//! fixture's last `FALL` line.
//!
//! With `probe_spawn_yaw` the first placement is a `spawn` at another yaw:
//! 1.5's and UO's spawn think lies the held prone down along it
//! (cod15-movement.md 4, "The spawn's lie-down").
//!
//! `PB_REPORT=1` prints both logs, `PB_TRACE=1` our snapshots,
//! `PB_FIXTURE=<path>` runs a `probe_rays` gate against another capture. The tests need `COD_DIR` at the install
//! each names and return early without.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, WBUTTON_PRONE};
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_pronebones.gsc");

/// How far a bone's offset off the origin may sit from retail's: the
/// logged origins are rounded to the hundredth.
const POSE_NEAR: f32 = 0.05;
/// How far the settled origin may sit from retail's: the drop onto the
/// spot lands within the ground trace's quarter unit, and a grade slides
/// it under a unit across, both by the cmd timing.
const ORIGIN_NEAR: f32 = 1.0;
/// How far the wire's prone body may sit from retail's: units, degrees.
const BODY_NEAR: f32 = 0.01;

/// Our run of the probe on `map` with one client holding prone at world
/// `yaw`, until the script's `done` line: the log, and the last snapshot's
/// prone body and `proneDirection`. `spawn_yaw` is `probe_spawn_yaw`.
/// `rays` sets `probe_rays` and replays that fixture's cmds (its `CMDS`
/// lines, `ProbeHold::times`) on its clock: the client joins so the first
/// placement lands on the fixture's.
fn ours(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    spot: &str,
    yaw: f32,
    spawn_yaw: Option<f32>,
    rays: Option<&str>,
) -> (Vec<String>, [f32; 3], f32) {
    let proto = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_pronebones"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_pronebones", SCRIPT);
    sv.set_cvar("probe_spot", spot);
    if let Some(y) = spawn_yaw {
        sv.set_cvar("probe_spawn_yaw", &y.to_string());
    }
    if rays.is_some() {
        sv.set_cvar("probe_rays", "1");
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let join_ms = rays.map_or(common::PROBE_JOIN_MS, |f| {
        first_place(f)
            - match proto.transport.version == PROTOCOL_V6.transport.version {
                true => PLACE_AFTER_JOIN_MS_15,
                false => PLACE_AFTER_JOIN_MS,
            }
    });
    common::run_alone(&mut sv, &mut now, join_ms);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let mut hold = common::ProbeHold {
        base: Some([0.0, yaw, 0.0]),
        framed: true,
        ..Default::default()
    };
    if let Some(f) = rays {
        hold.times = common::cmds_times(f, "");
        hold.ran = common::cmds_ran(f, "");
    }
    let mut body = [0.0; 3];
    let mut dir = 0.0;
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let live = |cl: &NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0)
    };
    for _ in 0..1000 {
        now += frame;
        if live(&cl) {
            let mut cmds = hold.cmds(&cl, 0);
            for c in &mut cmds {
                c.wbuttons |= WBUTTON_PRONE;
                c.up = -127;
            }
            cl.send_cmds(&cmds);
        } else {
            // Held from the join on, as the probe client holds it: 1.5's
            // spawn think runs this cmd.
            let held = UserCmd {
                wbuttons: WBUTTON_PRONE,
                up: -127,
                angles: [0, (yaw * 65536.0 / 360.0) as i32 & 0xffff, 0],
                ..Default::default()
            };
            if let Some(c) = cl.send_frame(&held) {
                hold.last = c.server_time;
            }
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            body = ["fTorsoHeight", "fTorsoPitch", "fWaistPitch"]
                .map(|n| f32::from_bits(s.ps.field_i32(cl.protocol(), n) as u32));
            dir = f32::from_bits(s.ps.field_i32(cl.protocol(), "proneDirection") as u32);
            if std::env::var_os("PB_TRACE").is_some() {
                let i = |n: &str| s.ps.field_i32(cl.protocol(), n);
                eprintln!(
                    "SNAP t={} ct={} pm_time={} lerp={} target={} pm_flags={:#x} view={} dir={dir} body={body:?} z={} legs={} torso={}",
                    s.server_time,
                    i("commandTime"),
                    i("pm_time"),
                    i("viewHeightLerpTime"),
                    i("viewHeightTarget"),
                    i("pm_flags"),
                    s.ps.viewangles(cl.protocol())[1],
                    f32::from_bits(i("origin[2]") as u32),
                    i("legsAnim"),
                    i("torsoAnim"),
                );
            }
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    (sv.script_log().to_vec(), body, dir)
}

fn vec3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .trim()
        .trim_matches(|c| c == '(' || c == ')')
        .split(", ")
        .map(|x| x.parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

/// The settled origin, and each frame's bones as (tag, offset off that
/// frame's origin), in order.
#[allow(clippy::type_complexity)]
fn pose(lines: &[String]) -> ([f32; 3], Vec<Vec<(String, [f32; 3])>>) {
    let mut origin = None;
    let mut frames: Vec<Vec<(String, [f32; 3])>> = Vec::new();
    for l in lines.iter().map(|l| l.trim()) {
        if let Some(rest) = l.strip_prefix("PROBE body ") {
            let (_, v) = rest.split_once(" (").unwrap();
            origin = Some(vec3(v));
            frames.push(Vec::new());
        } else if let Some(rest) = l.strip_prefix("PROBE bone ") {
            // `<time> <client> <tag with spaces> <origin>`
            let (head, v) = rest.split_once(" (").unwrap();
            let tag = head.splitn(3, ' ').nth(2).unwrap().to_string();
            let (o, b) = (origin.unwrap(), vec3(v));
            frames
                .last_mut()
                .unwrap()
                .push((tag, [0, 1, 2].map(|i| b[i] - o[i])));
        }
    }
    (origin.expect("no body line"), frames)
}

/// The prone body of the fixture's last `FALL` line, flat without one.
fn retail_body(fixture: &str) -> [f32; 3] {
    fixture
        .lines()
        .rev()
        .filter(|l| l.starts_with("FALL "))
        .find_map(|l| l.split_once(" prone=")?.1.split(' ').next())
        .map_or([0.0; 3], |v| {
            let v: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
}

/// The `proneDirection` of the fixture's last `FALL` line that carries one.
fn retail_dir(fixture: &str) -> Option<f32> {
    fixture
        .lines()
        .rev()
        .filter(|l| l.starts_with("FALL "))
        .find_map(|l| l.split_once(" dir=")?.1.split(' ').next()?.parse().ok())
}

fn check(fs: vcod_common::pk3::Pk3Fs, map: &str, spot: &str, yaw: f32, fixture: &str) {
    check_spawn(fs, map, spot, yaw, None, fixture, true);
}

/// `bones` false leaves the bones out, for a run whose clip timing ours
/// still misses (named at the caller).
fn check_spawn(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    spot: &str,
    yaw: f32,
    spawn_yaw: Option<f32>,
    fixture: &str,
    bones: bool,
) {
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect();
    let (ours, body, dir) = ours(fs, map, spot, yaw, spawn_yaw, None);
    if let Some(rd) = retail_dir(fixture) {
        assert!(
            ((rd - dir + 180.0).rem_euclid(360.0) - 180.0).abs() <= BODY_NEAR,
            "{spot}: retail's proneDirection {rd}, ours {dir}"
        );
    }
    if std::env::var_os("PB_REPORT").is_some() {
        ours.iter().for_each(|l| eprintln!("ours {l}"));
        retail.iter().for_each(|l| eprintln!("retail {l}"));
    }
    let rb = retail_body(fixture);
    assert!(
        (0..3).all(|i| (rb[i] - body[i]).abs() <= BODY_NEAR),
        "{spot}: retail's prone body {rb:?}, ours {body:?}"
    );
    let ((ro, rf), (oo, of)) = (pose(&retail), pose(&ours));
    assert_eq!(of.len(), rf.len(), "{ours:?}");
    assert!(
        (0..3).all(|i| (ro[i] - oo[i]).abs() <= ORIGIN_NEAR),
        "{spot}: retail settled at {ro:?}, ours at {oo:?}"
    );
    for (f, (r, o)) in rf.iter().zip(&of).enumerate().filter(|_| bones) {
        assert_eq!(r.len(), o.len());
        for ((tag, a), (otag, b)) in r.iter().zip(o) {
            assert_eq!(tag, otag);
            assert!(
                (0..3).all(|i| (a[i] - b[i]).abs() <= POSE_NEAR),
                "{spot} frame {f} {tag}: retail {a:?} off the origin, ours {b:?}"
            );
        }
    }
}

/// mp_carentan's ~19 degree mound, facing downhill: the slope terms bend
/// the body over the grade.
#[test]
fn a_prone_body_on_the_mp_carentan_mound_poses_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "mound",
        270.0,
        include_str!("fixtures/pronebones/mp_carentan-mound.txt"),
    );
}

#[test]
fn a_prone_body_on_flat_ground_poses_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "flat",
        225.0,
        include_str!("fixtures/pronebones/mp_carentan-flat.txt"),
    );
}

/// probe_scriptmodel's prone target on mp_brecourt's field.
#[test]
fn a_prone_body_on_the_mp_brecourt_field_poses_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        include_str!("fixtures/pronebones/mp_brecourt-brecourt.txt"),
    );
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so these run there too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

#[test]
fn a_prone_body_on_the_mp_carentan_mound_poses_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "mound",
        270.0,
        include_str!("fixtures/pronebones/mp_carentan-mound-1.5.txt"),
    );
}

#[test]
fn a_prone_body_on_flat_ground_poses_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "flat",
        225.0,
        include_str!("fixtures/pronebones/mp_carentan-flat-1.5.txt"),
    );
}

#[test]
fn a_prone_body_on_the_mp_brecourt_field_poses_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        include_str!("fixtures/pronebones/mp_brecourt-brecourt-1.5.txt"),
    );
}

#[test]
fn a_prone_body_on_the_mp_carentan_mound_poses_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "mound",
        270.0,
        include_str!("fixtures/pronebones/mp_carentan-mound-uo.txt"),
    );
}

#[test]
fn a_prone_body_on_flat_ground_poses_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        "mp_carentan",
        "flat",
        225.0,
        include_str!("fixtures/pronebones/mp_carentan-flat-uo.txt"),
    );
}

#[test]
fn a_prone_body_on_the_mp_brecourt_field_poses_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        include_str!("fixtures/pronebones/mp_brecourt-brecourt-uo.txt"),
    );
}

/// The spawn's own think lies a client holding prone down along the spawn's
/// yaw on 1.5 and UO, which run his last cmd there; 1.1's null cmd leaves
/// him standing until his next cmd, by when the view is the spot's
/// (cod15-movement.md 4, "The spawn's lie-down").
#[test]
fn a_spawn_lies_down_on_its_own_yaw_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_spawn(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        Some(315.0),
        include_str!("fixtures/pronebones/mp_brecourt-spawn315.txt"),
        true,
    );
}

#[test]
fn a_spawn_lies_down_on_its_own_yaw_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_spawn(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        Some(315.0),
        include_str!("fixtures/pronebones/mp_brecourt-spawn315-1.5.txt"),
        true,
    );
}

#[test]
fn a_spawn_lies_down_on_its_own_yaw_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_spawn(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        Some(315.0),
        include_str!("fixtures/pronebones/mp_brecourt-spawn315-uo.txt"),
        true,
    );
}

/// How long after the client's join ours' first placement lands: 1.5's
/// join answers its `serverinfo_<gametype>` menu first.
const PLACE_AFTER_JOIN_MS: i64 = 1100;
const PLACE_AFTER_JOIN_MS_15: i64 = 950;

/// The time of `fixture`'s first `PROBE place` line.
fn first_place(fixture: &str) -> i64 {
    fixture
        .lines()
        .find_map(|l| l.strip_prefix("PROBE place "))
        .and_then(|l| l.split(' ').next()?.parse().ok())
        .expect("a PROBE place line")
}

/// A float `probe_rays` printed as its integer part and remainder.
fn exact(int: &str, frac: &str) -> f32 {
    (int.parse::<f64>().unwrap() + frac.parse::<f64>().unwrap()) as f32
}

/// One `probe_rays` frame: the origin, each bone mark and each ray that
/// stopped on the body, keyed by its offset off the spot.
#[derive(Debug, Default)]
struct RayFrame {
    origin: [f32; 3],
    bones: Vec<(String, [f32; 3])>,
    rays: Vec<((i32, i32), f32)>,
}

fn ray_frames(lines: &[String]) -> Vec<RayFrame> {
    let mut out: Vec<RayFrame> = Vec::new();
    let v3 = |t: &[&str]| [0, 1, 2].map(|i| exact(t[2 * i], t[2 * i + 1]));
    for l in lines.iter().map(|l| l.trim()) {
        if let Some(rest) = l.strip_prefix("PROBE xbody ") {
            let t: Vec<&str> = rest.split(' ').collect();
            out.push(RayFrame {
                origin: v3(&t[2..]),
                ..Default::default()
            });
        } else if let Some(rest) = l.strip_prefix("PROBE xbone ") {
            // `<time> <client> <tag with spaces> <x> <xf> <y> <yf> <z> <zf>`
            let t: Vec<&str> = rest.split(' ').collect();
            let n = t.len();
            let tag = t[2..n - 6].join(" ");
            out.last_mut().unwrap().bones.push((tag, v3(&t[n - 6..])));
        } else if let Some(rest) = l.strip_prefix("PROBE ray ") {
            let t: Vec<&str> = rest.split(' ').collect();
            let key = (t[1].parse().unwrap(), t[2].parse().unwrap());
            out.last_mut().unwrap().rays.push((key, exact(t[3], t[4])));
        }
    }
    out
}

/// Float steps between two floats of one sign.
fn steps(a: f32, b: f32) -> i64 {
    (a.to_bits() as i64 - b.to_bits() as i64).abs()
}

/// `probe_rays` against `fixture`: the settled origin, every bone mark and
/// every ray's stop on the body to retail's float.
fn check_rays(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    spot: &str,
    yaw: f32,
    spawn_yaw: Option<f32>,
    fixture: &str,
) {
    let read = std::env::var("PB_FIXTURE")
        .ok()
        .map(|p| std::fs::read_to_string(p).unwrap());
    let fixture = read.as_deref().unwrap_or(fixture);
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect();
    let (ours, _, _) = ours(fs, map, spot, yaw, spawn_yaw, Some(fixture));
    let (rf, of) = (ray_frames(&retail), ray_frames(&ours));
    assert_eq!(rf.len(), 2, "{retail:?}");
    assert_eq!(of.len(), rf.len(), "{ours:?}");
    let report = std::env::var_os("PB_REPORT").is_some();
    let mut off = Vec::new();
    for (f, (r, o)) in rf.iter().zip(&of).enumerate() {
        if r.origin != o.origin {
            off.push(format!(
                "frame {f} origin: retail {:?}, ours {:?}",
                r.origin, o.origin
            ));
        }
        for ((tag, a), (_, b)) in r.bones.iter().zip(&o.bones) {
            if a != b {
                off.push(format!(
                    "frame {f} {tag}: retail {a:?}, ours {b:?}, steps {:?}",
                    [0, 1, 2].map(|i| steps(a[i], b[i]))
                ));
            }
        }
        let (rk, ok): (Vec<_>, Vec<_>) = (
            r.rays.iter().map(|x| x.0).collect(),
            o.rays.iter().map(|x| x.0).collect(),
        );
        for k in rk.iter().filter(|k| !ok.contains(k)) {
            off.push(format!("frame {f} ray {k:?}: retail only"));
        }
        for k in ok.iter().filter(|k| !rk.contains(k)) {
            off.push(format!("frame {f} ray {k:?}: ours only"));
        }
        for (k, a) in &r.rays {
            if let Some((_, b)) = o.rays.iter().find(|x| x.0 == *k)
                && a != b
            {
                off.push(format!(
                    "frame {f} ray {k:?}: retail {a}, ours {b}, {} steps",
                    steps(*a, *b)
                ));
            }
        }
    }
    if report {
        ours.iter().for_each(|l| eprintln!("ours {l}"));
        off.iter().for_each(|l| eprintln!("{l}"));
    }
    assert!(
        off.is_empty(),
        "{} rows off retail's:\n{}",
        off.len(),
        off.join("\n")
    );
}

/// probe_scriptmodel's prone target on mp_brecourt's grade, on 1.5b,
/// spawned on the spot: his origin, the eight bone marks and every
/// downward `bulletTrace` that stops on him to retail's float. 1.5b's
/// prone idle moves, so its keys lerp at the engine's float node time
/// (cod11-combat.md 3.1, "The clip's float time").
#[test]
fn a_prone_body_on_the_mp_brecourt_field_traces_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_rays(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        Some(270.0),
        include_str!("fixtures/pronebones/mp_brecourt-brecourt-rays-1.5.txt"),
    );
}

/// [`a_prone_body_on_the_mp_brecourt_field_traces_like_15`] on United
/// Offensive 1.51b, whose prone idle stands on its keys.
#[test]
fn a_prone_body_on_the_mp_brecourt_field_traces_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_rays(
        fs,
        "mp_brecourt",
        "brecourt",
        270.0,
        None,
        include_str!("fixtures/pronebones/mp_brecourt-brecourt-rays-uo.txt"),
    );
}
