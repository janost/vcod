//! A spawn inside another player, ours against retail 1.1d's, 1.5b's and
//! UO 1.51b's (docs/research/cod11-player-clip.md 13).
//! `client-probes/probe_spawnbody` drives two `--probe-uo-weapon direct`
//! clients through `PROBE_IN` lines: it `spawn`s one into or beside the
//! other, holding forward or not, and logs the origin the spawn's think
//! left, then walks him into the other on his own cmds. On 1.5 and UO the
//! think runs his held keys, and `PM_trace` lets a think that starts in the
//! body walk out of it, where one that starts beside it is stopped; 1.1's
//! null cmd moves nobody. Every `ClientImpacts` `"touch"` is logged.
//!
//! Ours runs the same gsc with two in-process clients. Held exactly: every
//! `spawned` line (the think's origin to the float) and its frame's
//! `touch` lines, and each walk's stop origin to a thousandth with
//! `touch` on both players while he pushes. `SPAWNBODY_REPORT=1` prints
//! both logs. Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_spawnbody.gsc");
const RETAIL_11: &str = include_str!("fixtures/playerstate/mp_carentan-dm-spawnbody-1.1.txt");
const RETAIL_15: &str = include_str!("fixtures/playerstate/mp_carentan-dm-spawnbody-1.5.txt");
const RETAIL_UO: &str = include_str!("fixtures/playerstate/mp_carentan-dm-spawnbody-uo.txt");

/// The spawner joined this long after the host.
const LATE_MS: i64 = 4000;

/// `(buttons, wbuttons, up, forward, right)` of a `f "PROBE_IN ..."`
/// command (`vehicle_probe::direct_input`).
fn direct_input(tokens: &[String]) -> Option<[i32; 5]> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let t: Vec<i32> = tokens
        .get(1)?
        .strip_prefix("PROBE_IN ")?
        .split_whitespace()
        .map(|t| t.parse().ok())
        .collect::<Option<_>>()?;
    let mut d = [0; 5];
    d[..t.len().min(5)].copy_from_slice(&t[..t.len().min(5)]);
    Some(d)
}

/// The script's log under ours, `PROBE` lines only.
fn ours(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_spawnbody"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_spawnbody", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let qs = [
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    ];
    let addrs = [common::ADDR, common::ADDR_B];
    let mut cls: [Option<NetClient<common::ClientEnd>>; 2] = [
        Some(NetClient::start_with_qport(
            common::ClientEnd(qs[0].clone()),
            now,
            0x2001,
        )),
        None,
    ];
    let mut joins = [
        Join::new("allies", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
    ];
    let mut holds = [(); 2].map(|_| common::ProbeHold {
        aim: true,
        sixty: true,
        ..Default::default()
    });
    let mut inputs = [[0i32; 5]; 2];
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for i in 0..3000i64 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        if cls[1].is_none() && i * common::FRAME_MS >= LATE_MS {
            cls[1] = Some(NetClient::start_with_qport(
                common::ClientEnd(qs[1].clone()),
                now,
                0x2002,
            ));
        }
        for (k, cl) in cls.iter_mut().enumerate() {
            let Some(cl) = cl.as_mut() else { continue };
            let live = cl
                .snapshots()
                .newest()
                .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
            if live {
                let d = inputs[k];
                let mut cmds = holds[k].cmds(cl, d[0] as u8);
                for c in &mut cmds {
                    c.buttons = d[0] as u8;
                    c.wbuttons = d[1] as u8;
                    c.up = d[2] as i8;
                    c.forward = d[3] as i8;
                    c.right = d[4] as i8;
                }
                cl.send_cmds(&cmds);
            } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
                holds[k].last = c.server_time;
            }
        }
        for (addr, q) in addrs.iter().zip(&qs) {
            let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(*addr, &p, now);
            }
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            let k = usize::from(to != common::ADDR);
            qs[k].borrow_mut().to_client.push_back(p);
        }
        for (k, cl) in cls.iter_mut().enumerate() {
            let Some(cl) = cl.as_mut() else { continue };
            for e in cl.pump_at(now) {
                if let NetEvent::ServerCommand(tokens) = e {
                    if let Some(d) = direct_input(&tokens) {
                        inputs[k] = d;
                    }
                    joins[k].on_server_command(&tokens, cl, now);
                }
            }
        }
    }
    sv.script_log()
        .iter()
        .filter_map(|l| l.find("PROBE ").map(|i| l[i..].trim_end().to_string()))
        .collect()
}

/// One trial or walk as both sides are held to it.
#[derive(Debug, PartialEq)]
enum Row {
    /// A `spawned` line less its time, and the `touch` lines of its frame,
    /// each as `(notified, other)`.
    Spawn(String, Vec<(String, String)>),
    /// A walk's stop origin, and whether each player was touched by the
    /// other while he pushed.
    Walk([f64; 3], bool, bool),
}

fn exact3(t: &[&str]) -> [f64; 3] {
    let f = |i: usize| t[2 * i].parse::<f64>().unwrap() + t[2 * i + 1].parse::<f64>().unwrap();
    [f(0), f(1), f(2)]
}

fn rows(log: &[String]) -> Vec<Row> {
    let lines: Vec<Vec<&str>> = log.iter().map(|l| l.split(' ').collect()).collect();
    let touches = |from: &str, to: Option<&str>| -> Vec<(String, String)> {
        lines
            .iter()
            .filter(|t| t[1] == "touch")
            .filter(|t| {
                let at: i32 = t[2].parse().unwrap();
                at >= from.parse::<i32>().unwrap() && to.is_none_or(|to| at <= to.parse().unwrap())
            })
            .map(|t| (t[3].to_string(), t[4].to_string()))
            .collect()
    };
    let mut out = Vec::new();
    for (i, t) in lines.iter().enumerate() {
        match t[1] {
            "spawned" => {
                let mut on: Vec<_> = touches(t[2], Some(t[2]));
                on.sort();
                out.push(Row::Spawn(t[3..].join(" "), on));
            }
            "walk" if t[4] == "stop" => {
                let start = lines[..i]
                    .iter()
                    .rev()
                    .find(|s| s[1] == "walk" && s[4] == "start")
                    .expect("a walk's start");
                let on = touches(start[2], Some(t[2]));
                let mover = t[3];
                let to_mover = on.iter().any(|(n, _)| n == mover);
                let to_other = on.iter().any(|(_, o)| o == mover);
                out.push(Row::Walk(exact3(&t[5..]), to_mover, to_other));
            }
            _ => {}
        }
    }
    out
}

fn check(fs: Option<vcod_common::pk3::Pk3Fs>, fixture: &str) {
    let Some(fs) = fs else {
        return;
    };
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect();
    let log = ours(fs);
    if std::env::var_os("SPAWNBODY_REPORT").is_some() {
        println!("retail:\n{}\nours:\n{}", retail.join("\n"), log.join("\n"));
    }
    let (theirs, mine) = (rows(&retail), rows(&log));
    assert_eq!(theirs.len(), 10, "the capture's eight spawns and two walks");
    assert_eq!(mine.len(), theirs.len(), "ours ran every trial: {mine:#?}");
    let mut bad = Vec::new();
    for (k, (want, got)) in theirs.iter().zip(&mine).enumerate() {
        let same = match (want, got) {
            (Row::Walk(a, ta, oa), Row::Walk(b, tb, ob)) => {
                (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3) && ta == tb && oa == ob
            }
            _ => want == got,
        };
        if !same {
            bad.push(format!("row {k}: retail {want:?}\n        ours   {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_spawn_into_a_player_runs_null_on_retail() {
    check(vcod_common::testing::game_fs_11(), RETAIL_11);
}

#[test]
fn a_spawn_into_a_player_walks_out_of_him_as_on_15() {
    let fs = vcod_common::testing::game_fs().filter(|fs| {
        vcod_server::server::install_protocol(fs).transport.version
            == vcod_common::net::protocol::PROTOCOL_V6.transport.version
    });
    check(fs, RETAIL_15);
}

#[test]
fn a_spawn_into_a_player_walks_out_of_him_as_on_uo() {
    check(vcod_common::testing::uo_fs(), RETAIL_UO);
}
