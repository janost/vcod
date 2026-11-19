//! The shellshock's playerstate fields and the shocked walk against the
//! retail capture.
//!
//! `shellshock/mp_carentan-dm-shock.txt` is the server's `games_mp.log`
//! under `client-probes/probe_shock.gsc`: the placement and each
//! `shellShock` / `stopShellShock` call with its level time.
//! `-shock-client.txt` is the `--save-shock` client's half: every usercmd
//! (forward and back a second each, `wbuttons` 0x4 while the shock runs)
//! and every snapshot's `shellshockIndex`, `shellshockTime`,
//! `shellshockDuration`, origin and velocity. Ours runs the same gametype
//! with one client that holds still until the placement, then sends
//! retail's cmds verbatim, each ahead of the frame whose snapshot first
//! acknowledged it, and every snapshot has to read what retail's did
//! (docs/research/cod11-shellshock.md, "Measured").
//!
//! `SHOCK_REPORT=1` prints every compared snapshot.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

use crate::common;

use common::{ADDR, ClientEnd, FRAME_MS, Join, Queues, holding, step_at};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const SERVER: &str = "tests/fixtures/shellshock/mp_carentan-dm-shock.txt";
const CLIENT: &str = "tests/fixtures/shellshock/mp_carentan-dm-shock-client.txt";

/// The client joins this many frames in, well before the gsc's 15000.
const JOIN_FRAMES: u32 = 100;

/// One snapshot's fields, as the fixture prints them.
#[derive(Clone, Debug, PartialEq)]
struct Snap {
    shock: [i32; 3],
    origin: [f32; 3],
    vel: [f32; 3],
}

fn report() -> bool {
    std::env::var("SHOCK_REPORT").is_ok_and(|v| v == "1")
}

fn kv(line: &str) -> BTreeMap<&str, &str> {
    line.split_whitespace()
        .skip(1)
        .filter_map(|w| w.split_once('='))
        .collect()
}

fn v3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    [v[0], v[1], v[2]]
}

/// The client half: every cmd, and each snapshot's `(serverTime,
/// commandTime, Snap)`.
fn client() -> (Vec<UserCmd>, Vec<(i32, i32, Snap)>) {
    let text = std::fs::read_to_string(CLIENT).unwrap();
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    for line in text.lines() {
        let kv = kv(line);
        let n = |k: &str| kv[k].parse::<i32>().unwrap();
        if line.starts_with("!cmd") {
            cmds.push(UserCmd {
                server_time: n("st"),
                buttons: n("buttons") as u8,
                wbuttons: n("wbuttons") as u8,
                weapon: n("weapon") as u8,
                up: n("up") as i8,
                forward: n("forward") as i8,
                right: n("right") as i8,
                ..NULL_USERCMD
            });
        } else if line.starts_with("!snap") {
            snaps.push((
                n("t"),
                n("ct"),
                Snap {
                    shock: [n("index"), n("time"), n("duration")],
                    origin: v3(kv["origin"]),
                    vel: v3(kv["vel"]),
                },
            ));
        }
    }
    (cmds, snaps)
}

/// The server half's `PROBE` calls, without the per-frame origins.
fn calls<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim().to_string()))
        .filter(|l| !l.starts_with("o "))
        .collect()
}

struct Ours {
    log: Vec<String>,
    snaps: BTreeMap<i32, Snap>,
}

fn run_ours() -> Option<Ours> {
    let fs = vcod_common::testing::game_fs_11()?;
    let (mut cmds, retail) = client();
    let (placed, placed_ct) = (retail[0].0, retail[0].1);
    // The probe sent a holding cmd at the placement's own time, before it
    // saw the placement, and captures before 2026-10-21 did not log it: the
    // fall from the placement runs from retail's commandTime through it.
    if cmds.first().is_some_and(|c| c.server_time > placed) {
        let weapon = cmds[0].weapon;
        cmds.insert(
            0,
            UserCmd {
                server_time: placed,
                weapon,
                ..NULL_USERCMD
            },
        );
    }
    let (mut sv, mut now) = common::Boot {
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_shock.gsc"
        )),
        ..common::Boot::new(MAP, "probe_shock")
    }
    .start(fs);
    for _ in 0..JOIN_FRAMES {
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick(now);
    }
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    let acked: BTreeMap<i32, i32> = retail.iter().map(|(t, ct, _)| (*t, *ct)).collect();
    let last = retail.last().unwrap().0;
    let mut snaps = BTreeMap::new();
    let mut next_cmd = 0;
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
            // acknowledged, so this frame runs the moves retail's did. The
            // world yaw is 0 throughout: the cmd carries the delta's undo.
            let ct = acked
                .range(..=t)
                .next_back()
                .map_or(i32::MIN, |(_, ct)| *ct);
            let delta = cl.snapshots().newest().map_or([0; 3], |s| {
                [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
            });
            let mut batch = Vec::new();
            while let Some(cmd) = cmds.get(next_cmd)
                && cmd.server_time <= ct
            {
                next_cmd += 1;
                let mut c = *cmd;
                c.angles = [0, 1, 2].map(|i| (-delta[i]) & 0xffff);
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
        if let Some(s) = cl.snapshots().newest()
            && s.server_time >= placed
        {
            let f = |n: &str| s.ps.field_i32(p, n);
            let g = |n: &str| s.ps.field_f32(p, n);
            snaps.insert(
                s.server_time,
                Snap {
                    shock: [
                        f("shellshockIndex"),
                        f("shellshockTime"),
                        f("shellshockDuration"),
                    ],
                    origin: s.ps.origin(p),
                    vel: [0, 1, 2].map(|i| g(&format!("velocity[{i}]"))),
                },
            );
        }
    }
    Some(Ours {
        log: sv.script_log().to_vec(),
        snaps,
    })
}

#[test]
fn shellshock_fields_and_the_shocked_walk_match_retail() {
    let Some(ours) = run_ours() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let retail_calls = calls(std::fs::read_to_string(SERVER).unwrap().lines());
    let our_calls = calls(ours.log.iter().map(String::as_str));
    assert_eq!(our_calls, retail_calls);

    let (_, retail) = client();
    let mut bad = Vec::new();
    let mut compared = 0;
    for (t, _, r) in &retail {
        let Some(o) = ours.snaps.get(t) else {
            bad.push(format!("{t}: no snapshot of ours"));
            continue;
        };
        compared += 1;
        if report() {
            println!("{t} retail {r:?}\n{t} ours   {o:?}");
        }
        if r.shock != o.shock {
            bad.push(format!("{t}: retail {:?} ours {:?}", r.shock, o.shock));
        }
        if r.origin != o.origin || r.vel != o.vel {
            bad.push(format!(
                "{t}: retail origin {:?} vel {:?}, ours origin {:?} vel {:?}",
                r.origin, r.vel, o.origin, o.vel
            ));
        }
    }
    assert!(compared > 200, "only {compared} snapshots compared");
    assert!(
        bad.is_empty(),
        "{} snapshots differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
