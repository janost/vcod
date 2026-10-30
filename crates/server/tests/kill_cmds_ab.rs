//! A `kill` death frame against retail's, by how many usercmds reach the
//! server behind the `kill` before the frame ends. The captures are the
//! target probe's `--probe-kill-cmds 1,3` on all three retail servers:
//! `fixtures/playerstate/mp_carentan-dm-hit-target-killcmds{,-1.5,-uo}.txt`.
//!
//! The first cmd behind the `kill` still runs at `pm_type` 0 and puts the
//! dropped carbine away to empty hands (`weaponstate` 1, `EV_RAISE_WEAPON`);
//! a second cmd in the same frame finishes that raise, weapon 0's
//! `raiseTime` being 0, and reads 0. `ClientEndFrame` then makes the player
//! dead and the dead arm of `PM_Weapon` keeps whatever state was left
//! (combat doc 9.2, "The death frame's `weaponstate`").
//!
//! The `killtimers` captures (`-killtimers{,-1.5,-uo}.txt`) also log the
//! anim timers: the death's hold, the clip + 50, is set on the last cmd
//! ahead of the `kill` and the cmds behind it run it down (combat doc 16.4,
//! "The dying body").
//!
//! Each gate returns early unless `COD_DIR` is the install its capture came
//! from.

use crate::common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::animtree::PlayerAnims;
use vcod_common::net::msg::{BUTTON_USE, NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;

/// One `!kill` with the death frame retail sent behind it.
#[derive(Debug, PartialEq)]
struct Death {
    cmds: usize,
    weaponstate: i32,
    event_sequence: i32,
    events: String,
    /// What the cmds behind the `kill` ran off the death's `legsTimer` and
    /// `torsoTimer`, each read against the clip's length + 50 that
    /// `player_die`'s `BG_AnimScriptEvent` set them to: the msec the
    /// frame's cmds ran past the death. Only in captures that log the
    /// timers (`killtimers`).
    timers_run: Option<(i32, i32)>,
}

/// The death clip's hold, its length + 50 (`BG_PlayAnim`), for a wire
/// `legsAnim`.
fn death_hold(anims: &PlayerAnims, legs_anim: i32) -> i32 {
    let name = anims.name(legs_anim & 511).expect("a death clip");
    anims.length_ms(name).expect("its length") as i32 + 50
}

fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("no {key}= in {line}"))
}

/// Every kill of the capture, each with the first dead `!trace` after it.
fn retail(path: &str, anims: &PlayerAnims) -> Vec<Death> {
    let full = format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}"));
    let traces: Vec<&str> = text.lines().filter(|l| l.starts_with("!trace ")).collect();
    text.lines()
        .filter(|l| l.starts_with("!kill "))
        .map(|kill| {
            let at: u64 = field(kill, "ms").parse().unwrap();
            let dead = traces
                .iter()
                .find(|t| {
                    field(t, "ms").parse::<u64>().unwrap() >= at && field(t, "pm_type") == "6"
                })
                .unwrap_or_else(|| panic!("{path}: no death after {kill}"));
            Death {
                cmds: field(kill, "cmds").parse().unwrap(),
                weaponstate: field(dead, "weaponstate").parse().unwrap(),
                event_sequence: field(dead, "eventSequence").parse().unwrap(),
                events: field(dead, "events").to_string(),
                timers_run: dead.contains(" legsTimer=").then(|| {
                    let hold = death_hold(anims, field(dead, "legsAnim").parse().unwrap());
                    let timer = |k| hold - field(dead, k).parse::<i32>().unwrap();
                    (timer("legsTimer"), timer("torsoTimer"))
                }),
            }
        })
        .collect()
}

/// The same kills on ours: alive two seconds, then `kill` and its cmds in
/// one frame, one message each as the probe sent them, and a frame with no
/// cmd at all; then the use press until the respawn.
fn ours(proto: &'static Protocol, fs: Pk3Fs, kills: &[usize], timers: bool) -> Vec<Death> {
    let anims = PlayerAnims::load(&fs).expect("the player anims");
    let bsp_path = fs.resolve_map("mp_carentan").expect("map");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "dm"), now);
    sv.set_protocol(proto);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (mut cl, join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");
    assert!(join.findings().is_empty(), "{}", join.summary());
    let p = proto;
    let newest = |cl: &vcod_common::net::NetClient<common::ClientEnd>| {
        cl.snapshots().newest().unwrap().ps.clone()
    };
    let mut out = Vec::new();
    for &n in kills {
        let mut alive = 0;
        let mut last = None;
        for frame in 0..400 {
            now += Duration::from_millis(common::FRAME_MS as u64);
            let ps = newest(&cl);
            let dead = ps.field_i32(p, "pm_type") != 0;
            alive = if dead { 0 } else { alive + 1 };
            let cmd = UserCmd {
                weapon: ps.field_i32(p, "weapon") as u8,
                buttons: if dead && frame % 20 == 19 {
                    BUTTON_USE
                } else {
                    0
                },
                ..NULL_USERCMD
            };
            last = cl.send_frame(&cmd).or(last);
            common::step(&mut sv, &q, &mut cl, now);
            if alive >= 40 {
                break;
            }
        }
        assert!(alive >= 40, "never alive for long before the kill");
        let last = last.expect("a cmd went out");
        cl.send_reliable("kill");
        for k in 1..=n as i32 {
            cl.send_cmds(&[UserCmd {
                server_time: last.server_time + k,
                ..last
            }]);
        }
        now += Duration::from_millis(common::FRAME_MS as u64);
        common::step(&mut sv, &q, &mut cl, now);
        let ps = newest(&cl);
        assert_eq!(ps.field_i32(p, "pm_type"), 6, "the kill's own frame");
        let seq = ps.field_i32(p, "eventSequence");
        // In 1.1's numbering, as the probe prints them.
        let events: Vec<String> = (0..4)
            .map(|i| (p.event_to_v1)(ps.field_i32(p, &format!("events[{i}]"))).to_string())
            .collect();
        out.push(Death {
            cmds: n,
            weaponstate: ps.field_i32(p, "weaponstate"),
            event_sequence: seq,
            events: events.join(","),
            timers_run: timers.then(|| {
                let hold = death_hold(&anims, ps.field_i32(p, "legsAnim"));
                (
                    hold - ps.field_i32(p, "legsTimer"),
                    hold - ps.field_i32(p, "torsoTimer"),
                )
            }),
        });
        // The quiet frame: nothing behind the burst, as the probe sent.
        now += Duration::from_millis(common::FRAME_MS as u64);
        common::step(&mut sv, &q, &mut cl, now);
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    out
}

fn check(proto: &'static Protocol, fs: Pk3Fs, fixture: &str) {
    let anims = PlayerAnims::load(&fs).expect("the player anims");
    let retail = retail(fixture, &anims);
    assert!(
        retail.iter().any(|d| d.cmds == 1) && retail.iter().any(|d| d.cmds > 1),
        "{fixture}: the capture needs both a lone cmd and a burst"
    );
    let kills: Vec<usize> = retail.iter().map(|d| d.cmds).collect();
    let timers = retail.iter().all(|d| d.timers_run.is_some());
    let ours = ours(proto, fs, &kills, timers);
    assert_eq!(ours, retail, "{fixture}");
}

#[test]
fn a_kills_death_frame_matches_11d_by_its_cmds() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        &PROTOCOL_V1,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killcmds.txt",
    );
}

#[test]
fn a_kills_death_frame_matches_15b_by_its_cmds() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    // UO's `main/` is 1.5's, so this one runs at a UO install too.
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(
        &PROTOCOL_V6,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killcmds-1.5.txt",
    );
}

#[test]
fn a_kills_death_frame_matches_uo_by_its_cmds() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        &PROTOCOL_V22,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killcmds-uo.txt",
    );
}

/// The `killtimers` captures log the timers too: the death's hold is set at
/// the `kill`, on the last cmd ahead of it, and each cmd behind runs it
/// down (cod11-combat.md 16.4, "The dying body").
#[test]
fn a_kills_death_timers_match_11d_by_its_cmds() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        &PROTOCOL_V1,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killtimers.txt",
    );
}

#[test]
fn a_kills_death_timers_match_15b_by_its_cmds() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(
        &PROTOCOL_V6,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killtimers-1.5.txt",
    );
}

#[test]
fn a_kills_death_timers_match_uo_by_its_cmds() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        &PROTOCOL_V22,
        fs,
        "playerstate/mp_carentan-dm-hit-target-killtimers-uo.txt",
    );
}
