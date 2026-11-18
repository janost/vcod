//! A player a damage callback moves into another player, and the cmds that
//! run after it, ours against retail 1.1d's, 1.5b's and UO 1.51b's
//! (docs/research/cod11-player-clip.md 13.6). `client-probes/probe_dmgmove`
//! drops one `--probe-uo-weapon direct` client next to another; his
//! landing's `MOD_FALLING` callback, inside his own cmd, `setOrigin`s him
//! into the other player, or the other player into him, and the cmds behind
//! it in that frame start inside a body.
//!
//! Each capture carries both clients' `CMDS` lines and the server's receive
//! order (`RECV`, docs/dev/retail.md "Receive order"), so ours runs every
//! cmd on retail's side of each frame and in retail's order. Held exactly:
//! every line of a trial from its drop to the first frame after the landing,
//! times taken off the drop's. Later frames ride the end frame's
//! `StuckInClient` push, a `rand()` draw (player-clip doc 6.4). `DMGMOVE_REPORT=1`
//! prints both. Needs `COD_DIR` at the capture's install.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_dmgmove.gsc");
const RETAIL_11: &str = include_str!("fixtures/playerstate/mp_carentan-dm-dmgmove-1.1.txt");
const RETAIL_15: &str = include_str!("fixtures/playerstate/mp_carentan-dm-dmgmove-1.5.txt");
const RETAIL_UO: &str = include_str!("fixtures/playerstate/mp_carentan-dm-dmgmove-uo.txt");

/// How long before a client's first playing snapshot ours starts its join.
fn join_lead_ms(fs: &vcod_common::pk3::Pk3Fs) -> i64 {
    match vcod_server::server::install_protocol(fs).transport.version {
        1 => 800,
        6 => 1000,
        _ => 1150,
    }
}

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

/// A client's `SPAWNED` line: its first playing snapshot's time and origin.
fn spawned(fixture: &str, who: &str) -> (i64, [f32; 3]) {
    let l = fixture
        .lines()
        .filter_map(|l| l.strip_prefix(who))
        .find_map(|l| l.trim_start().strip_prefix("SPAWNED t="))
        .unwrap_or_else(|| panic!("{who}: a SPAWNED line"));
    let t = l.split_whitespace().next().unwrap().parse().unwrap();
    let o = l
        .split_whitespace()
        .find_map(|t| t.strip_prefix("origin="))
        .unwrap();
    let v: Vec<f32> = o.split(',').map(|x| x.parse().unwrap()).collect();
    (t, [v[0], v[1], v[2]])
}

/// The map's spawn point nearest `at`, which the capture's client stood on.
fn nearest_spawn(fs: &vcod_common::pk3::Pk3Fs, at: [f32; 3]) -> common::SpawnAt {
    let d = |p: &[f32; 3]| (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f32>();
    common::spawn_points(fs, "mp_carentan")
        .into_iter()
        .min_by(|a, b| d(&a.0).total_cmp(&d(&b.0)))
        .expect("a spawn point")
}

/// The script's log under ours, both clients replaying the capture's cmds.
fn ours(fs: vcod_common::pk3::Pk3Fs, fixture: &str) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let lead = join_lead_ms(&fs);
    let (host_t, host_at) = spawned(fixture, "host");
    let (faller_t, faller_at) = spawned(fixture, "faller");
    let pins = vec![
        vec![Some(nearest_spawn(&fs, host_at))],
        vec![Some(nearest_spawn(&fs, faller_at))],
    ];
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_dmgmove"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_dmgmove", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    sv.pin_spawns(pins);
    common::run_alone(&mut sv, &mut now, host_t - lead);
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
    let who = ["host", "faller"];
    let mut holds = who.map(|w| common::ProbeHold {
        aim: true,
        framed: true,
        times: common::cmds_times(fixture, w),
        ran: common::cmds_ran(fixture, w),
        ..Default::default()
    });
    let pressed = who.map(|w| common::cmds_inputs(fixture, w));
    // Each cmd's place in retail's read order, keyed (slot, serverTime).
    let read: std::collections::HashMap<(usize, i32), usize> = common::cmds_read(fixture, &who)
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    let mut inputs = [[0i32; 5]; 2];
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let report = std::env::var_os("DMGMOVE_REPORT").is_some();
    let mut live_at = [None; 2];
    for i in 0..4000i64 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        if cls[1].is_none() && i * common::FRAME_MS >= faller_t - host_t {
            cls[1] = Some(NetClient::start_with_qport(
                common::ClientEnd(qs[1].clone()),
                now,
                0x2002,
            ));
        }
        let mut batch: Vec<(usize, UserCmd)> = Vec::new();
        for (k, cl) in cls.iter_mut().enumerate() {
            let Some(cl) = cl.as_mut() else { continue };
            let newest = cl.snapshots().newest();
            let live = newest.is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
            if live && live_at[k].is_none() {
                live_at[k] = newest.map(|s| s.server_time);
                if report {
                    eprintln!("{} live at {:?}", who[k], live_at[k]);
                }
            }
            if live {
                let d = inputs[k];
                for mut c in holds[k].cmds(cl, d[0] as u8) {
                    // The captured buttons of the cmd; the keys are the
                    // script's `PROBE_IN`, which the capture does not log.
                    let p = &pressed[k];
                    let (b, wb, up) = match p.partition_point(|e| e.0 <= c.server_time) {
                        0 if p.is_empty() => (d[0] as u8, d[1] as u8, d[2] as i8),
                        0 => (0, 0, 0),
                        i => p[i - 1].1,
                    };
                    c.buttons = b;
                    c.wbuttons = wb;
                    c.up = up;
                    c.forward = d[3] as i8;
                    c.right = d[4] as i8;
                    batch.push((k, c));
                }
            } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
                holds[k].last = c.server_time;
            }
        }
        if !batch.is_empty()
            && batch
                .iter()
                .all(|(k, c)| read.contains_key(&(*k, c.server_time)))
        {
            // One cmd a packet, in the order retail read them.
            batch.sort_by_key(|(k, c)| read[&(*k, c.server_time)]);
        }
        for (k, cmd) in batch {
            cls[k].as_mut().unwrap().send_cmds(&[cmd]);
            let pending: Vec<Vec<u8>> = qs[k].borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(addrs[k], &p, now);
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

/// Each trial's lines from its `drop` to the first frame after its `hurt`,
/// each time made the drop's offset and each frame's lines sorted.
fn trials(log: &[String]) -> Vec<Vec<String>> {
    let time = |l: &str| l.split(' ').nth(2).and_then(|t| t.parse::<i32>().ok());
    let mut out = Vec::new();
    for (i, l) in log.iter().enumerate() {
        if !l.starts_with("PROBE drop ") {
            continue;
        }
        let t0 = time(l).unwrap();
        let hurt = log[i..]
            .iter()
            .find(|l| l.starts_with("PROBE hurt "))
            .and_then(|l| time(l))
            .expect("a landing");
        let end = hurt + common::FRAME_MS as i32;
        let mut rows: Vec<(i32, String)> = log[i..]
            .iter()
            .filter(|l| time(l).is_some_and(|t| t <= end))
            .take_while(|l| !l.starts_with("PROBE done"))
            .map(|l| {
                let t = time(l).unwrap();
                let mut f: Vec<&str> = l.split(' ').collect();
                let rel = (t - t0).to_string();
                f[2] = &rel;
                (t, f.join(" "))
            })
            .collect();
        rows.sort();
        out.push(rows.into_iter().map(|r| r.1).collect());
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
    let log = ours(fs, fixture);
    if std::env::var_os("DMGMOVE_REPORT").is_some() {
        println!("retail:\n{}\nours:\n{}", retail.join("\n"), log.join("\n"));
    }
    let (theirs, mine) = (trials(&retail), trials(&log));
    assert_eq!(theirs.len(), 8, "the capture's eight trials");
    assert_eq!(mine.len(), theirs.len(), "ours ran every trial");
    let mut bad = Vec::new();
    for (k, (want, got)) in theirs.iter().zip(&mine).enumerate() {
        if want != got {
            bad.push(format!(
                "trial {k}:\n  retail {}\n  ours   {}",
                want.join("\n         "),
                got.join("\n         ")
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn a_cmd_behind_a_callbacks_move_starts_where_it_put_him_on_11() {
    check(vcod_common::testing::game_fs_11(), RETAIL_11);
}

#[test]
fn a_cmd_behind_a_callbacks_move_starts_where_it_put_him_on_15() {
    let fs = vcod_common::testing::game_fs_15();
    check(fs, RETAIL_15);
}

#[test]
fn a_cmd_behind_a_callbacks_move_starts_where_it_put_him_on_uo() {
    check(vcod_common::testing::uo_fs(), RETAIL_UO);
}
