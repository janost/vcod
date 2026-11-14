//! The anim channels' `legsTimer` and `torsoTimer` against retail's,
//! snapshot by snapshot. The fixtures are `--save-slope` runs from
//! `client-probes/probe_prone`'s street placement on mp_carentan, whose
//! `wire=` column carries every non-zero netfield: the `--probe-moves`
//! script on 1.1d (carbine and springfield) and 1.5b (jumps, landings,
//! stance changes, shots, reloads), and UO's `--probe-sprint` run. Ours
//! runs the same gametype and replays the capture's cmds at their own
//! times (gunkick_ab.rs's replay), then compares both timers on every
//! snapshot whose `commandTime` both servers reached. Each event's timer
//! is its duration + 50 set inside the pmove of the cmd that raised it and
//! run down by every cmd after (player-model-anim-system.md, "The weapon
//! channel"), so a timer stamped on the wrong cmd reads off by the cmds
//! between.
//!
//! `TIMERS_REPORT=1` prints every compared snapshot. Needs `COD_DIR`: the
//! 1.1 captures run at a 1.1 install, the 1.5 one at a 1.5 or UO install,
//! the UO one at a UO install.

use crate::common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const PROBE: &str = "probe_prone";
/// `probe_prone`'s street spot.
const SPOT: [f32; 2] = [900.0, 1930.0];

/// Snapshots whose timers may differ from retail's: tag, `commandTime`,
/// field and why. Empty: every compared snapshot matches.
const GAPS: &[(&str, i32, &str, &str)] = &[];

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn ints<const N: usize>(s: &str) -> [i32; N] {
    let v: Vec<i32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    std::array::from_fn(|i| v[i])
}

struct Cmd {
    st: i32,
    /// Absolute view shorts: the wire angles plus the snapshot before's
    /// `delta_angles`.
    view: [i32; 3],
    cmd: UserCmd,
}

/// Retail's snapshot: `serverTime`, `commandTime` and the two timers.
struct Snap {
    t: i32,
    ct: i32,
    timers: [i32; 2],
    /// `legsAnim` and `torsoAnim`, printed beside a miss; compared, toggle
    /// aside, only where the test asks.
    anims: [i32; 2],
}

struct Capture {
    weapon: String,
    cmds: Vec<Cmd>,
    snaps: Vec<Snap>,
}

fn retail(text: &str) -> Capture {
    let weapon = text
        .lines()
        .find_map(|l| l.split(", weapon ").nth(1))
        .and_then(|w| w.split(',').next())
        .expect("a weapon in the header")
        .to_string();
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut da = [0; 3];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!snap ") {
            da = ints(field(rest, "da").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            let wire: BTreeMap<&str, i32> = field(rest, "wire")
                .unwrap_or("")
                .split(',')
                .filter_map(|kv| kv.rsplit_once(':'))
                .map(|(k, v)| (k, v.parse().unwrap()))
                .collect();
            let timer = |k| wire.get(k).copied().unwrap_or(0);
            snaps.push(Snap {
                t: num("t"),
                ct: num("ct"),
                timers: [timer("legsTimer"), timer("torsoTimer")],
                anims: [timer("legsAnim"), timer("torsoAnim")],
            });
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let angles: [i32; 3] = ints(field(rest, "angles").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            cmds.push(Cmd {
                st: num("st"),
                view: std::array::from_fn(|i| (angles[i] + da[i]) & 0xffff),
                cmd: UserCmd {
                    buttons: num("buttons") as u8,
                    wbuttons: num("wbuttons") as u8,
                    weapon: num("weapon") as u8,
                    forward: num("forward") as i8,
                    right: num("right") as i8,
                    up: num("up") as i8,
                    ..NULL_USERCMD
                },
            });
        }
    }
    Capture {
        weapon,
        cmds,
        snaps,
    }
}

/// Ours over the capture, with the gametype's `cvars` on top of the street
/// placement's: `commandTime` to the two timers, `legsAnim` and
/// `torsoAnim`, one entry per snapshot.
fn ours(
    p: &'static Protocol,
    fs: Pk3Fs,
    cap: &Capture,
    cvars: &[(&str, &str)],
) -> BTreeMap<i32, [i32; 4]> {
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{PROBE}"), &probe);
    sv.set_cvar("probe_teleport", "1");
    sv.set_cvar("probe_spot", "street");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("allies", &cap.weapon);
    let weapon = vcod_server::defs::Defs::of(p)
        .weapon_index(&cap.weapon)
        .unwrap() as u8;
    let frame = |sv: &mut vcod_server::Server,
                 cl: &mut vcod_common::net::NetClient<common::ClientEnd>,
                 join: &mut common::Join,
                 now: Instant| {
        for e in common::step(sv, &q, cl, now) {
            if let vcod_common::net::NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, cl, now);
            }
        }
    };
    let mut placed_at = None;
    for i in 0..400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&UserCmd {
            weapon,
            ..NULL_USERCMD
        });
        frame(&mut sv, &mut cl, &mut join, now);
        let on_spot = cl.snapshots().newest().is_some_and(|s| {
            let o = s.ps.origin(p);
            s.ps.field_i32(p, "pm_type") == 0
                && (o[0] - SPOT[0]).abs() < 1.0
                && (o[1] - SPOT[1]).abs() < 1.0
                && s.ps.field_i32(p, "weapon") == weapon as i32
        });
        if on_spot && placed_at.is_none() {
            placed_at = Some(i);
        }
        if placed_at.is_some_and(|at| i >= at + 20) {
            break;
        }
    }
    assert!(placed_at.is_some(), "the probe never placed the client");

    let delta = |cl: &vcod_common::net::NetClient<common::ClientEnd>| {
        cl.snapshots().newest().map_or([0; 3], |s| {
            [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
        })
    };
    let first = cap.snaps.first().expect("a snapshot").t;
    let last = cap.snaps.last().unwrap().t;
    let mut next_cmd = 0;
    let mut out = BTreeMap::new();
    loop {
        let t = cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.server_time + common::FRAME_MS as i32);
        if t > last {
            break;
        }
        if t < first {
            cl.send_frame(&UserCmd {
                weapon,
                ..NULL_USERCMD
            });
        } else {
            let ct = cap
                .snaps
                .iter()
                .take_while(|s| s.t <= t)
                .last()
                .map_or(i32::MIN, |s| s.ct);
            let da = delta(&cl);
            let mut batch = Vec::new();
            while let Some(c) = cap.cmds.get(next_cmd)
                && c.st <= ct
            {
                next_cmd += 1;
                batch.push(UserCmd {
                    server_time: c.st,
                    angles: std::array::from_fn(|i| (c.view[i] - da[i]) & 0xffff),
                    ..c.cmd
                });
            }
            cl.send_cmds(&batch);
        }
        now += Duration::from_millis(common::FRAME_MS as u64);
        frame(&mut sv, &mut cl, &mut join, now);
        if let Some(s) = cl.snapshots().newest() {
            let ct = s.ps.field_i32(p, "commandTime");
            out.insert(
                ct,
                [
                    s.ps.field_i32(p, "legsTimer"),
                    s.ps.field_i32(p, "torsoTimer"),
                    s.ps.field_i32(p, "legsAnim"),
                    s.ps.field_i32(p, "torsoAnim"),
                ],
            );
        }
    }
    out
}

/// Retail's timers against ours on every snapshot both reached, and with
/// `anims` the two channels' anim indices too; returns how many non-zero
/// retail timers were compared.
fn check(p: &'static Protocol, fs: Pk3Fs, tag: &str) -> usize {
    check_with(p, fs, tag, &[], false)
}

fn check_with(
    p: &'static Protocol,
    fs: Pk3Fs,
    tag: &str,
    cvars: &[(&str, &str)],
    anims: bool,
) -> usize {
    let path = format!("tests/fixtures/playerstate/{MAP}-dm-slope-8ms-{tag}.txt");
    let text = std::fs::read_to_string(&path).expect("read the fixture");
    let cap = retail(&text);
    let ours = ours(p, fs, &cap, cvars);
    let report = std::env::var_os("TIMERS_REPORT").is_some();
    let mut bad = Vec::new();
    let mut live = 0;
    let mut compared = 0;
    for s in &cap.snaps {
        let Some(ours_ct) = ours.get(&s.ct) else {
            continue;
        };
        compared += 1;
        for (i, name) in ["legsTimer", "torsoTimer"].into_iter().enumerate() {
            let (r, o) = (s.timers[i], ours_ct[i]);
            live += usize::from(r != 0);
            let line = format!(
                "{tag} ct {}: {name} retail {r} ours {o} (legsAnim retail {} ours {}, torsoAnim retail {} ours {})",
                s.ct,
                s.anims[0] & 511,
                ours_ct[2] & 511,
                s.anims[1] & 511,
                ours_ct[3] & 511
            );
            if report && (r != 0 || o != 0) {
                println!("{line}");
            }
            if r != o && !GAPS.iter().any(|g| (g.0, g.1, g.2) == (tag, s.ct, name)) {
                bad.push(line);
            }
        }
        for (i, name) in ["legsAnim", "torsoAnim"].into_iter().enumerate() {
            let (r, o) = (s.anims[i] & 511, ours_ct[2 + i] & 511);
            if anims && r != o && !GAPS.iter().any(|g| (g.0, g.1, g.2) == (tag, s.ct, name)) {
                bad.push(format!("{tag} ct {}: {name} retail {r} ours {o}", s.ct));
            }
        }
    }
    assert!(
        compared * 10 >= cap.snaps.len() * 9,
        "{tag}: only {compared} of {} snapshots compared",
        cap.snaps.len()
    );
    assert!(
        bad.is_empty(),
        "{} timers off retail's:\n{}",
        bad.len(),
        bad.join("\n")
    );
    live
}

#[test]
fn the_anim_timers_run_like_retail_on_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let live = check(&PROTOCOL_V1, fs, "moves-1.1");
    assert!(live > 100, "{live}");
}

#[test]
fn the_springfield_timers_run_like_retail_on_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let live = check(&PROTOCOL_V1, fs, "springfield-1.1");
    assert!(live > 100, "{live}");
}

#[test]
fn the_anim_timers_run_like_retail_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let live = check(&PROTOCOL_V6, fs, "moves-1.5");
    assert!(live > 50, "{live}");
}

#[test]
fn the_anim_timers_run_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let live = check(&PROTOCOL_V22, fs, "sprint-uo");
    assert!(live > 100, "{live}");
}

/// The prone move key (`--probe-pronemove rifle`, springfield_mp): the
/// sighted, rechambering and swinging presses, the dive from a crouched
/// walk back and its `jumpbk` (`both pb_chicken_dance_crouch`, the torso
/// on 0), with both channels' anims compared.
#[test]
fn the_prone_move_anims_run_like_retail_on_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let live = check_with(&PROTOCOL_V1, fs, "pronemove-rifle-1.1", &[], true);
    assert!(live > 20, "{live}");
}

#[test]
fn the_prone_move_anims_run_like_retail_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let live = check_with(&PROTOCOL_V6, fs, "pronemove-rifle-1.5", &[], true);
    assert!(live > 20, "{live}");
}

/// UO's rifle run, then the bipod and the binoculars (`--probe-pronemove
/// lmg`, mg30cal_mp, `probe_binoculars 1`): the breakdown continue on each
/// prone flip and the flips' torso clears.
#[test]
fn the_prone_move_anims_run_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let live = check_with(&PROTOCOL_V22, fs, "pronemove-rifle-uo", &[], true);
    assert!(live > 20, "{live}");
    let live = check_with(
        &PROTOCOL_V22,
        vcod_common::testing::uo_fs().unwrap(),
        "pronemove-lmg-uo",
        &[("probe_binoculars", "1")],
        true,
    );
    assert!(live > 50, "{live}");
}
