//! Shots whose muzzle moves, ours against retail's: captures of
//! `--save-slope --probe-moveshot` beside `client-probes/probe_moveshot` on
//! mp_carentan (`mp_carentan-dm-slope-8ms-moveshot-<part>-<tag>.txt`).
//! `walk` fires a thompson strafing, running, crouched, sighted and
//! leaning, so 1.5's and UO's `CalcMuzzlePoint` adds the walk bob and the
//! lean to every round; `bipod` (UO) fires an mg30cal from the bipod short
//! of a wall, the view swung off it, so the muzzle trace stops on the wall,
//! then prone off the bipod (docs/research/cod11-combat.md 2.1). Each
//! server ran the weapon with `common::STEADY_WEAPON` applied: no spread and no gun kick,
//! so a round leaves along the view from the muzzle and its impact on the
//! wall 1090 units out reads the muzzle across the view.
//!
//! Ours runs the same probe gametype, replays the capture's cmds at their
//! own times (gunkick_ab.rs's replay) and compares every impact in order,
//! and every snapshot's `bobCycle`.
//! `MOVESHOT_REPORT=1` prints both sides. The 1.5 capture runs at a 1.5
//! install, the UO ones at a UO install.

use crate::common;

use glam::Vec3;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::event_ids::{EV_BULLET_HIT_LARGE, EV_BULLET_HIT_SMALL, EV_UO_BULLET_HIT};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const PROBE: &str = "probe_moveshot";
/// How far an impact of ours may sit from retail's: both are snapped to
/// whole units, so one the same muzzle leaves lands on the same unit or,
/// a hair over a unit edge, the next.
const TOL: f32 = 1.0;

/// Rounds of ours that may land off retail's by more than [`TOL`]: part,
/// tag, round and why. Empty since the wait before the script sends the
/// probe's 8 ms cmds: the street placement's fall then lands near enough
/// retail's on every cmd phase (cod11-combat.md 2.1, "The street
/// placement").
const GAPS: &[(&str, &str, usize, &str)] = &[];

/// One part of the probe: its `probe_spot`, the weapon and where the
/// gametype places the body.
#[derive(Clone, Copy)]
struct Part {
    name: &'static str,
    spot: &'static str,
    weapon: &'static str,
    at: [f32; 2],
}

const WALK: Part = Part {
    name: "walk",
    spot: "street",
    weapon: "thompson_mp",
    at: [900.0, 1930.0],
};
const BIPOD: Part = Part {
    name: "bipod",
    spot: "wall",
    weapon: "mg30cal_mp",
    at: [1971.0, 1930.0],
};

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn ints<const N: usize>(s: &str) -> [i32; N] {
    let v: Vec<i32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    std::array::from_fn(|i| v[i])
}

/// One `!cmd`: its serverTime, the view in absolute shorts (wire angles
/// plus the `delta_angles` of the snapshot before it) and the rest.
struct Cmd {
    st: i32,
    view: [i32; 3],
    cmd: UserCmd,
}

/// Impacts in the order they first showed, `(entity, at)` new against the
/// snapshot before.
#[derive(Default)]
struct Impacts {
    prev: Vec<(u32, [i32; 3])>,
    all: Vec<Vec3>,
}

impl Impacts {
    fn snapshot(&mut self, cur: Vec<(u32, [i32; 3])>) {
        for &(n, at) in &cur {
            if !self.prev.contains(&(n, at)) {
                self.all.push(Vec3::from(at.map(|c| c as f32)));
            }
        }
        self.prev = cur;
    }
}

/// What the capture holds: every cmd, each snapshot's `(serverTime,
/// commandTime)`, and the impacts.
struct Capture {
    cmds: Vec<Cmd>,
    snaps: Vec<(i32, i32)>,
    impacts: Vec<Vec3>,
    states: Vec<State>,
}

/// A snapshot's commandTime, origin, `bobCycle` and `leanf`: what the
/// muzzle reads.
type State = (i32, [f32; 3], i32, f32);

fn retail(text: &str) -> Capture {
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut states = Vec::new();
    let mut impacts = Impacts::default();
    let mut da = [0; 3];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!snap ") {
            da = ints(field(rest, "da").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            snaps.push((num("t"), num("ct")));
            let o: Vec<f32> = field(rest, "origin")
                .unwrap()
                .split(',')
                .map(|c| c.parse().unwrap())
                .collect();
            let bob = field(rest, "wire")
                .and_then(|w| {
                    w.split(',')
                        .find_map(|kv| kv.strip_prefix("bobCycle:")?.parse().ok())
                })
                .unwrap_or(0);
            let lean = field(rest, "leanf").unwrap().parse().unwrap();
            states.push((num("ct"), [o[0], o[1], o[2]], bob, lean));
            let cur = field(rest, "imp")
                .map(|s| {
                    s.split(';')
                        .map(|e| {
                            let (n, at) = e.split_once(':').unwrap();
                            let at: Vec<f32> = at.split(',').map(|c| c.parse().unwrap()).collect();
                            (n.parse().unwrap(), std::array::from_fn(|i| at[i] as i32))
                        })
                        .collect()
                })
                .unwrap_or_default();
            impacts.snapshot(cur);
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let angles: [i32; 3] = ints(field(rest, "angles").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            cmds.push(Cmd {
                st: num("st"),
                view: std::array::from_fn(|i| (angles[i] + da[i]) & 0xffff),
                cmd: UserCmd {
                    buttons: num("buttons") as u8,
                    wbuttons: num("wbuttons") as u8,
                    forward: num("forward") as i8,
                    right: num("right") as i8,
                    up: num("up") as i8,
                    ..NULL_USERCMD
                },
            });
        }
    }
    Capture {
        cmds,
        snaps,
        impacts: impacts.all,
        states,
    }
}

/// The bullet impacts on one of our snapshots.
fn our_impacts(p: &Protocol, s: &vcod_common::net::snapshot::Snapshot) -> Vec<(u32, [i32; 3])> {
    s.entities
        .iter()
        .filter(|(_, e)| {
            p.event_of(e.field_i32(p, "eType")).is_some_and(|ev| {
                [EV_BULLET_HIT_SMALL, EV_BULLET_HIT_LARGE, EV_UO_BULLET_HIT].contains(&ev)
            })
        })
        .map(|(&n, e)| {
            let at = vcod_common::net::trajectory::Trajectory::read(e, p, "pos").base;
            (n, at.to_array().map(|c| c as i32))
        })
        .collect()
}

/// The probe on ours: joined allies with the part's steadied weapon, placed by the
/// gametype, held until ours' clock reaches the capture's first snapshot,
/// then each frame the cmds retail's snapshot of that frame had
/// acknowledged, at their own times: the sight's idle sway runs on the
/// level time, so both servers aim on the same one.
fn ours(p: &'static Protocol, mut fs: Pk3Fs, part: Part, cap: &Capture) -> (Vec<Vec3>, Vec<State>) {
    common::steady_weapon(&mut fs, part.weapon);
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{PROBE}"), &probe);
    sv.set_cvar("probe_teleport", "1");
    sv.set_cvar("probe_spot", part.spot);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("allies", part.weapon);
    let weapon = vcod_server::defs::Defs::of(p)
        .weapon_index(part.weapon)
        .unwrap() as u8;
    let mut impacts = Impacts::default();
    let mut states = Vec::new();
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
    // The net probe's cmds while it waits, idle at world yaw 0: one every
    // `--probe-cmd-ms` (8 here) from its first snapshot on, so the fall
    // after the placement steps as retail's did.
    let idle = |cl: &mut vcod_common::net::NetClient<common::ClientEnd>| {
        let Some(s) = cl.snapshots().newest() else {
            cl.send_frame(&UserCmd {
                weapon,
                ..NULL_USERCMD
            });
            return;
        };
        let da = [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")));
        let frame_ms = common::FRAME_MS as i32;
        let batch: Vec<UserCmd> = (1..=6)
            .map(|k| UserCmd {
                server_time: s.server_time + (k * frame_ms + 5) / 6,
                angles: da.map(|d| (-d) & 0xffff),
                weapon,
                ..NULL_USERCMD
            })
            .collect();
        cl.send_cmds(&batch);
    };
    // Until the gametype has put the body on the spot, and a second more,
    // as retail's capture opens.
    let mut placed_at = None;
    for i in 0..400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        idle(&mut cl);
        frame(&mut sv, &mut cl, &mut join, now);
        let on_spot = cl.snapshots().newest().is_some_and(|s| {
            let o = s.ps.origin(p);
            s.ps.field_i32(p, "pm_type") == 0
                && (o[0] - part.at[0]).abs() < 1.0
                && (o[1] - part.at[1]).abs() < 1.0
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
    let first = cap.snaps.first().expect("a snapshot").0;
    let last = cap.snaps.last().unwrap().0;
    let mut next_cmd = 0;
    loop {
        let t = cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.server_time + common::FRAME_MS as i32);
        if t > last {
            break;
        }
        if t < first {
            idle(&mut cl);
        } else {
            let ct = cap
                .snaps
                .iter()
                .take_while(|(st, _)| *st <= t)
                .last()
                .map_or(i32::MIN, |s| s.1);
            let da = delta(&cl);
            let mut batch = Vec::new();
            while let Some(c) = cap.cmds.get(next_cmd)
                && c.st <= ct
            {
                next_cmd += 1;
                batch.push(UserCmd {
                    server_time: c.st,
                    angles: std::array::from_fn(|i| (c.view[i] - da[i]) & 0xffff),
                    weapon,
                    ..c.cmd
                });
            }
            cl.send_cmds(&batch);
        }
        now += Duration::from_millis(common::FRAME_MS as u64);
        frame(&mut sv, &mut cl, &mut join, now);
        if let Some(s) = cl.snapshots().newest() {
            impacts.snapshot(our_impacts(p, s));
            let ct = s.ps.field_i32(p, "commandTime");
            if states.last().is_none_or(|l: &State| l.0 != ct) {
                states.push((
                    ct,
                    s.ps.origin(p),
                    s.ps.field_i32(p, "bobCycle"),
                    s.ps.field_f32(p, "leanf"),
                ));
            }
        }
    }
    (impacts.all, states)
}

/// The capture's impacts against ours, one for one.
fn check(p: &'static Protocol, fs: Pk3Fs, part: Part, tag: &str) {
    let path = format!(
        "tests/fixtures/playerstate/{MAP}-dm-slope-8ms-moveshot-{}-{tag}.txt",
        part.name
    );
    let text = std::fs::read_to_string(&path).expect("read the fixture");
    let cap = retail(&text);
    let (ours, our_states) = ours(p, fs, part, &cap);
    let report = std::env::var_os("MOVESHOT_REPORT").is_some();
    if report {
        // Where the bodies part: the muzzle's inputs, snapshot by snapshot.
        for r in &cap.states {
            if let Some(o) = our_states.iter().find(|o| o.0 == r.0)
                && (o.1 != r.1 || o.2 != r.2 || o.3 != r.3)
            {
                println!(
                    "state ct {}: retail {:?} bob {} lean {} ours {:?} bob {} lean {}",
                    r.0, r.1, r.2, r.3, o.1, o.2, o.3
                );
            }
        }
    }
    let retail = cap.impacts;
    let mut bad = Vec::new();
    // The walk bob rides every moving round, so its phase matches retail's
    // on every snapshot both sides sent, a lean-strafe's start included
    // (coduo-movement.md 16).
    for r in &cap.states {
        if let Some(o) = our_states.iter().find(|o| o.0 == r.0)
            && o.2 != r.2
        {
            bad.push(format!("ct {}: retail bobCycle {} ours {}", r.0, r.2, o.2));
        }
    }
    for i in 0..retail.len().max(ours.len()) {
        let (r, o) = (retail.get(i), ours.get(i));
        let line = format!("{} {tag} round {i}: retail {r:?} ours {o:?}", part.name);
        if report {
            println!("{line}");
        }
        let gap = GAPS.iter().any(|g| (g.0, g.1, g.2) == (part.name, tag, i));
        if !gap && !matches!((r, o), (Some(r), Some(o)) if r.distance(*o) <= TOL) {
            bad.push(line);
        }
    }
    assert!(bad.is_empty(), "rounds off retail's:\n{}", bad.join("\n"));
}

/// 1.5b's rounds fired strafing, running, crouched, sighted and leaning.
#[test]
fn a_15_moving_and_leaning_muzzle_lands_as_on_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(&PROTOCOL_V6, fs, WALK, "1.5");
}

/// United Offensive's.
#[test]
fn a_uo_moving_and_leaning_muzzle_lands_as_on_retail() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(&PROTOCOL_V22, fs, WALK, "uo");
}

/// United Offensive's mg30cal from the bipod short of the wall: the muzzle
/// trace stops on it, so a round with the view 30 degrees off the bipod
/// leaves from 13 units further forward than the eye.
#[test]
fn a_uo_bipod_muzzle_stops_on_the_wall_as_on_retail() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(&PROTOCOL_V22, fs, BIPOD, "uo");
}
