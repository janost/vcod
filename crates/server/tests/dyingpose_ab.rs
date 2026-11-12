//! A dying body's bones, its timers and its corpse's event ring, ours
//! against retail 1.1d's, 1.5b's and United Offensive 1.51b's.
//! `client-probes/probe_dyingpose` stands one client on mp_carentan's flat
//! prone spot facing 90, links a script_origin to each of eight bones, logs
//! them every frame for 0.5 s, then `suicide()`s him into the dead session
//! and logs them 2.5 s more. The killed callback clones the corpse and plays
//! an alias on it (docs/research/cod11-combat.md 5.2 and 16.4, "The dying
//! body"). `client-probes/probe_corpseblast` kills him with his own frag,
//! in the entity pass, for the corpse's 0x800 clock (5.2).
//!
//! `DP_REPORT=1` prints our log, `DP_TRACE=1` our snapshots, `DP_SEED=<n>`
//! runs another server seed (another death clip), `DP_RETAIL=1` prints the
//! retail capture's death frames. The 1.1 gates need `COD_DIR` at a 1.1
//! install, the 1.5 ones at a 1.5 or UO install (its `main/`), the UO ones
//! at a UO install.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::msg::{BUTTON_ATTACK, EntityState, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, Protocol};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_dyingpose.gsc");
const BLAST_SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_corpseblast.gsc");
const FIXTURE: &str = include_str!("fixtures/dyingpose/mp_carentan-dm.txt");
const FIXTURE_15: &str = include_str!("fixtures/dyingpose/mp_carentan-dm-1.5.txt");
const FIXTURE_UO: &str = include_str!("fixtures/dyingpose/mp_carentan-dm-uo.txt");

/// How far a bone's offset off the origin may sit from retail's: the bone
/// and the origin are each logged rounded to the hundredth, so their
/// difference carries up to 0.01, plus f32 noise at map coordinates.
const POSE_NEAR: f32 = 0.015;

/// The server seed whose death draw is retail's, `legsAnim` 20: the pick is
/// `rand()` on retail and ours draws its own sequence.
const SEED: u64 = 2;
/// 1.5b's and UO's, drawing their captures' death clips.
const SEED_15: u64 = 6;
const SEED_UO: u64 = 68;

/// One dead snapshot's `commandTime`, `legsTimer` and `torsoTimer`.
type Timers = (i32, i32, i32);

/// Our run of the probe until its `done` line: the log, the corpse as
/// the client's first snapshot holding it carried it, and each dead
/// snapshot's timers.
fn ours(fs: vcod_common::pk3::Pk3Fs, seed: u64) -> (Vec<String>, Option<EntityState>, Vec<Timers>) {
    let map = "mp_carentan";
    let proto = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::with_seed(common::cfg(map, "probe_dyingpose"), now, seed);
    sv.set_cvar("sv_pure", "0");
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_dyingpose", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    // `--probe-yaw 90`: every cmd at world yaw 90.
    let mut hold = common::ProbeHold {
        base: Some([0.0, 90.0, 0.0]),
        ..Default::default()
    };
    let mut corpse = None;
    let mut timers = Vec::new();
    let mut last_snap = None;
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..1000 {
        now += frame;
        let cmds = hold.cmds(&cl, 0);
        if cmds.is_empty() {
            cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        } else {
            cl.send_cmds(&cmds);
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if std::env::var_os("DP_TRACE").is_some()
            && let Some(s) = cl.snapshots().newest()
        {
            let i = |n: &str| s.ps.field_i32(cl.protocol(), n);
            eprintln!(
                "SNAP t={} pm_type={} legs={} torso={} view={:?} eflags={:#x}",
                s.server_time,
                i("pm_type"),
                i("legsAnim"),
                i("torsoAnim"),
                s.ps.viewangles(cl.protocol()),
                i("eFlags"),
            );
        }
        if let Some(s) = cl.snapshots().newest()
            && last_snap != Some(s.server_time)
        {
            last_snap = Some(s.server_time);
            if let Some(t) = dead_timers(cl.protocol(), &s.ps) {
                timers.push(t);
            }
        }
        if corpse.is_none() {
            corpse = cl
                .snapshots()
                .newest()
                .and_then(|s| s.entities.get(&64).cloned());
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    (sv.script_log().to_vec(), corpse, timers)
}

/// A dead (`pm_type` 6) playerstate's timers.
fn dead_timers(p: &Protocol, ps: &vcod_common::net::msg::PlayerState) -> Option<Timers> {
    let i = |n: &str| ps.field_i32(p, n);
    (i("pm_type") == 6).then(|| (i("commandTime"), i("legsTimer"), i("torsoTimer")))
}

/// Retail's dead snapshots' timers out of a capture pair.
fn retail_timers(p: &Protocol, tag: &str) -> Vec<Timers> {
    vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    )
    .iter()
    .filter_map(|f| dead_timers(p, &f.snapshot.ps))
    .collect()
}

/// The death's `legsTimer` and `torsoTimer`: both the clip's duration + 50
/// on the first dead snapshot, counted from the last cmd's serverTime, then
/// run down by every cmd's msec, so each reaches 0 at one serverTime
/// (combat doc 16.4, "The dying body").
fn check_timers(fs: vcod_common::pk3::Pk3Fs, p: &Protocol, tag: &str, seed: u64) {
    let retail = retail_timers(p, tag);
    let (_, _, ours) = ours(fs, seed);
    let ends = |v: &[Timers]| -> Vec<(i32, i32)> {
        v.iter()
            .filter(|t| t.1 > 0)
            .map(|&(ct, l, t)| (ct + l, ct + t))
            .collect()
    };
    if std::env::var_os("DP_REPORT").is_some() {
        eprintln!("retail {retail:?}\nours {ours:?}");
    }
    let (r0, o0) = (retail[0], ours[0]);
    assert_eq!((o0.1, o0.2), (r0.1, r0.2), "the death snapshot's timers");
    let (re, oe) = (ends(&retail), ends(&ours));
    assert!(re.iter().all(|&e| e == re[0]), "retail: {retail:?}");
    assert!(oe.iter().all(|&e| e == oe[0]), "ours: {ours:?}");
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

/// Each frame's session and bones as (tag, offset off the origin the frame
/// before logged), in order: a link reads the body as the last frame left
/// it, its origin included, which the death's relink moves.
#[allow(clippy::type_complexity)]
fn pose(lines: &[String]) -> Vec<(String, Vec<(String, [f32; 3])>)> {
    let (mut origin, mut last) = ([0.0; 3], None);
    let mut frames: Vec<(String, Vec<(String, [f32; 3])>)> = Vec::new();
    for l in lines.iter().map(|l| l.trim()) {
        if let Some(rest) = l.strip_prefix("PROBE body ") {
            // `<time> <client> <origin> <angles> <session>`
            let (_, v) = rest.split_once(" (").unwrap();
            let (o, rest) = v.split_once(") (").unwrap();
            origin = last.unwrap_or(vec3(o));
            last = Some(vec3(o));
            let session = rest.rsplit(' ').next().unwrap().to_string();
            frames.push((session, Vec::new()));
        } else if let Some(rest) = l.strip_prefix("PROBE bone ") {
            // `<time> <client> <tag with spaces> <origin>`
            let (head, v) = rest.split_once(" (").unwrap();
            let tag = head.splitn(3, ' ').nth(2).unwrap().to_string();
            let b = vec3(v);
            frames
                .last_mut()
                .unwrap()
                .1
                .push((tag, [0, 1, 2].map(|i| b[i] - origin[i])));
        }
    }
    frames
}

fn retail_log(fixture: &str) -> Vec<String> {
    fixture
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect()
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so these run there too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

/// Every frame's bones within [`POSE_NEAR`] of retail's `fixture`.
fn check_pose(fs: vcod_common::pk3::Pk3Fs, fixture: &str, seed: u64) {
    let retail = retail_log(fixture);
    let seed = std::env::var("DP_SEED").map_or(seed, |v| v.parse().unwrap());
    let (ours, _, _) = ours(fs, seed);
    if std::env::var_os("DP_REPORT").is_some() {
        ours.iter().for_each(|l| eprintln!("ours {l}"));
    }
    let (rf, of) = (pose(&retail), pose(&ours));
    assert_eq!(of.len(), rf.len(), "{ours:?}");
    let mut misses = Vec::new();
    for (f, ((rs, r), (os, o))) in rf.iter().zip(&of).enumerate() {
        assert_eq!(rs, os, "frame {f}");
        for ((tag, a), (otag, b)) in r.iter().zip(o) {
            assert_eq!(tag, otag);
            if !(0..3).all(|i| (a[i] - b[i]).abs() <= POSE_NEAR) {
                misses.push(format!("frame {f} ({rs}) {tag}: retail {a:?}, ours {b:?}"));
            }
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}

/// Retail's corpse as the capture pair first sent entity 64.
fn retail_corpse(p: &Protocol, tag: &str) -> EntityState {
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    if std::env::var_os("DP_RETAIL").is_some() {
        let mut last = None;
        for f in &frames {
            let s = &f.snapshot;
            let i = |n: &str| s.ps.field_i32(p, n);
            let now = (
                i("pm_type"),
                i("legsAnim"),
                i("torsoAnim"),
                i("eFlags"),
                i("legsTimer"),
                i("torsoTimer"),
            );
            if last != Some(now) {
                eprintln!(
                    "RETAIL t={} pm_type={} legs={} torso={} eflags={:#x} timers={} {}",
                    s.server_time, now.0, now.1, now.2, now.3, now.4, now.5
                );
                last = Some(now);
            }
        }
    }
    frames
        .into_iter()
        .find_map(|f| f.snapshot.entities.get(&64).cloned())
        .expect("retail's corpse")
}

/// Our corpse's ring, flags and yaw against retail's first one.
fn check_corpse(fs: vcod_common::pk3::Pk3Fs, p: &Protocol, tag: &str, seed: u64) {
    let retail = retail_corpse(p, tag);
    let (log, corpse, _) = ours(fs, seed);
    assert!(
        log.iter()
            .any(|l| l.starts_with("PROBE killed") && l.ends_with(" body 64")),
        "the killed line names the corpse: {log:?}"
    );
    let corpse = corpse.expect("the corpse rode a snapshot");
    let mut fields = vec!["eventSequence", "eFlags", "apos.trBase[1]"];
    let ring: Vec<String> = (0..4)
        .flat_map(|i| [format!("events[{i}]"), format!("eventParms[{i}]")])
        .collect();
    fields.extend(ring.iter().map(String::as_str));
    for name in fields {
        if std::env::var_os("DP_REPORT").is_some() {
            eprintln!(
                "{name}: retail {} ours {}",
                retail.field_i32(p, name),
                corpse.field_i32(p, name)
            );
        }
        assert_eq!(
            corpse.field_i32(p, name),
            retail.field_i32(p, name),
            "{name}: ours {corpse:?}, retail {retail:?}"
        );
    }
}

/// The dead session's bones: retail's controller hook still runs on a body
/// with `EF_DEAD` and sets `tag_origin`'s yaw to the legs' world yaw
/// (combat doc 16.4, "The dying body").
#[test]
fn a_dying_body_poses_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_pose(fs, FIXTURE, SEED);
}

/// 1.5b's: the same hook and an ease with no dead test (16.4).
#[test]
fn a_dying_body_poses_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_pose(fs, FIXTURE_15, SEED_15);
}

/// UO's: the same, its ease (0x1f107) read for a dead test too (16.4).
#[test]
fn a_dying_body_poses_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_pose(fs, FIXTURE_UO, SEED_UO);
}

/// `cloneplayer` hands the corpse back as an entity, and an alias played on
/// it rides the corpse's own ring: retail's first snapshot of entity 64
/// (`net/snapshots-dyingpose.bin`) sent `eventSequence` 1, event 172
/// (`EV_SOUND_ALIAS`), parm 4, `eFlags` 0x818 and the body's yaw 90.
#[test]
fn a_sound_played_on_a_corpse_rides_its_ring() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_corpse(fs, &PROTOCOL_V1, "dyingpose", SEED);
    assert_eq!(
        retail_corpse(&PROTOCOL_V1, "dyingpose").field_i32(&PROTOCOL_V1, "events[0]"),
        172
    );
}

#[test]
fn a_sound_played_on_a_corpse_rides_its_ring_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_corpse(fs, &PROTOCOL_V6, "dyingpose-1.5", SEED_15);
}

#[test]
fn a_sound_played_on_a_corpse_rides_its_ring_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p = vcod_server::server::install_protocol(&fs);
    check_corpse(fs, p, "dyingpose-uo", SEED_UO);
}

#[test]
fn a_death_runs_its_timers_down_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_timers(fs, &PROTOCOL_V1, "dyingpose", SEED);
}

#[test]
fn a_death_runs_its_timers_down_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_timers(fs, &PROTOCOL_V6, "dyingpose-1.5", SEED_15);
}

#[test]
fn a_death_runs_its_timers_down_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p = vcod_server::server::install_protocol(&fs);
    check_timers(fs, p, "dyingpose-uo", SEED_UO);
}

/// The serverTime the corpse at 64 first rode a snapshot and the first one
/// that carried it without `eFlags` 0x800.
type CorpseClock = (Option<i32>, Option<i32>);

fn corpse_clock<'a>(
    p: &Protocol,
    snaps: impl Iterator<Item = &'a vcod_common::net::snapshot::Snapshot>,
) -> CorpseClock {
    let (mut seen, mut cleared) = (None, None);
    for s in snaps {
        let Some(e) = s.entities.get(&64) else {
            continue;
        };
        seen.get_or_insert(s.server_time);
        if cleared.is_none() && e.field_i32(p, "eFlags") & 0x800 == 0 {
            cleared = Some(s.server_time);
        }
    }
    (seen, cleared)
}

/// Our run of `client-probes/probe_corpseblast` with a
/// `--probe-uo-weapon throw` client: the log and the corpse's clock.
fn ours_blast(fs: vcod_common::pk3::Pk3Fs) -> (Vec<String>, CorpseClock) {
    let map = "mp_carentan";
    let proto = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_corpseblast"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_corpseblast", BLAST_SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let (mut selected, mut spawned): (Option<u8>, Option<i64>) = (None, None);
    let mut snaps = Vec::new();
    for i in 0..1000i64 {
        now += frame;
        let live = cl.state() == vcod_common::net::NetState::Active
            && cl
                .snapshots()
                .newest()
                .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live || spawned.is_some() {
            // `--probe-uo-weapon throw`: settle 3 s, pull back for 1 s.
            let at = *spawned.get_or_insert(i);
            let ms = (i - at) * common::FRAME_MS;
            let buttons = if (3000..4000).contains(&ms) {
                BUTTON_ATTACK
            } else {
                0
            };
            let mut cmds = hold.cmds(&cl, buttons);
            if let Some(w) = selected {
                cmds.iter_mut().for_each(|c| c.weapon = w);
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest()
            && snaps
                .last()
                .is_none_or(|l: &vcod_common::net::snapshot::Snapshot| {
                    l.server_time != s.server_time
                })
        {
            snaps.push(s.clone());
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    let clock = corpse_clock(cl.protocol(), snaps.iter());
    (sv.script_log().to_vec(), clock)
}

/// A death in the entity pass: the grenade's blast runs the killed
/// callback on this frame's `level.time`, not the last frame's a cmd runs
/// on, so the clone's 250 ms think clears 0x800 250 ms after the corpse
/// first rides a snapshot where a cmd-path death's clears it after 200
/// (combat doc 5.2).
fn check_blast(fs: vcod_common::pk3::Pk3Fs, p: &Protocol, tag: &str, fixture: &str) {
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    let retail = corpse_clock(p, frames.iter().map(|f| &f.snapshot));
    let (log, ours) = ours_blast(fs);
    if std::env::var_os("DP_REPORT").is_some() {
        log.iter().for_each(|l| eprintln!("ours {l}"));
        eprintln!("retail {retail:?} ours {ours:?}");
    }
    // `PROBE killed <time> <n> <mod>`: the callback's clock is the frame
    // the corpse first rides.
    let killed = |log: &[String]| -> i32 {
        let l = log
            .iter()
            .find(|l| l.starts_with("PROBE killed"))
            .unwrap_or_else(|| panic!("no death: {log:?}"));
        assert!(l.ends_with("MOD_GRENADE_SPLASH"), "{l}");
        l.split(' ').nth(2).unwrap().parse().unwrap()
    };
    assert_eq!(Some(killed(&retail_log(fixture))), retail.0, "retail");
    assert_eq!(Some(killed(&log)), ours.0, "ours");
    let span = |(seen, cleared): CorpseClock| Some(cleared? - seen?);
    assert_eq!(span(retail), Some(250), "retail {retail:?}");
    assert_eq!(span(ours), span(retail), "ours {ours:?}, retail {retail:?}");
}

#[test]
fn a_blast_corpse_keeps_its_fresh_flag_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_blast(
        fs,
        &PROTOCOL_V1,
        "corpseblast",
        include_str!("fixtures/dyingpose/mp_carentan-dm-corpseblast.txt"),
    );
}

#[test]
fn a_blast_corpse_keeps_its_fresh_flag_like_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    check_blast(
        fs,
        &PROTOCOL_V6,
        "corpseblast-1.5",
        include_str!("fixtures/dyingpose/mp_carentan-dm-corpseblast-1.5.txt"),
    );
}

#[test]
fn a_blast_corpse_keeps_its_fresh_flag_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p = vcod_server::server::install_protocol(&fs);
    check_blast(
        fs,
        p,
        "corpseblast-uo",
        include_str!("fixtures/dyingpose/mp_carentan-dm-corpseblast-uo.txt"),
    );
}
