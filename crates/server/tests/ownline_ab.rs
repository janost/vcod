//! Whether a gunner's round meets his own body, against retail 1.1d
//! (`client-probes/probe_ownline.gsc` on mp_carentan, `tools/run_probe.sh`;
//! capture in `fixtures/ownline/`, recipe in the client probes' README):
//! one gunner sweeps mp_carentan's gun over both arcs with the trigger held
//! (cod11-turrets.md 6.3).
//!
//! Needs `COD_DIR` at a 1.1 install; otherwise it returns early.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

/// The `PROBE` lines of a log.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    lines.filter(|l| l.starts_with("PROBE")).collect()
}

/// The `buttons` of a `f "PROBE_IN <buttons> <wbuttons> <up>"` command.
fn direct_buttons(tokens: &[String]) -> Option<u8> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    t.next()?.parse().ok()
}

/// How long after the client connects ours places him, run alone for
/// `PLACED_MS` less than the capture's `PROBE player` time: both clocks
/// agree and his cmds go out on the times the capture's `CMDS` lines
/// logged.
const PLACED_MS: i64 = 800;

/// Runs the probe on ours with one allied `--probe-uo-weapon direct`
/// client, on the clock and cmd times of `capture`'s `gunner CMDS` lines,
/// until it logs `PROBE done`; its `PROBE` lines. The drop from his first
/// placement lands where his cmds split it (cod11-combat.md 3.6), and the
/// placement on the gun starts from there.
fn ours(fs: vcod_common::pk3::Pk3Fs, capture: &str, cvars: &[(&str, &str)]) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let (map, probe) = ("mp_carentan", "probe_ownline");
    let src = std::fs::read_to_string(format!(
        "../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc"
    ))
    .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        proto: Some(proto),
        cvars,
        script: Some(&src),
        ..common::Boot::new(map, probe)
    }
    .start(fs);
    let placed: i64 = capture
        .lines()
        .find_map(|l| l.strip_prefix("PROBE player "))
        .and_then(|r| r.split_whitespace().nth(1)?.parse().ok())
        .expect("the capture's PROBE player time");
    common::run_alone(&mut sv, &mut now, placed - PLACED_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        framed: true,
        times: common::cmds_times(capture, "gunner"),
        ran: common::cmds_ran(capture, "gunner"),
        ..Default::default()
    };
    let (mut buttons, mut selected) = (0u8, None::<u8>);
    // The velocity his placement kept: it shares his spawn's frame, and the
    // cmds run at the spawn point before it leave him moving
    // (coduo-vehicles.md 11.6, "The first drop").
    let mut spawn_vel: Option<[f32; 3]> = capture
        .lines()
        .find_map(|l| l.strip_prefix("gunner SPAWNED "))
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("vel=")))
        .map(|v| {
            let v: Vec<f32> = v
                .split(',')
                .map(|c| c.parse().expect("a velocity"))
                .collect();
            [v[0], v[1], v[2]]
        });
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..3000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, buttons);
            for c in &mut cmds {
                if let Some(w) = selected {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(common::ADDR, &p, now);
        }
        sv.tick(now);
        if spawn_vel.is_some()
            && sv
                .script_log()
                .iter()
                .any(|l| l.starts_with("PROBE player "))
        {
            sv.test_set_client_velocity(0, spawn_vel.take().unwrap());
        }
        for (_, p) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(p);
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                if let Some(b) = direct_buttons(&tokens) {
                    buttons = b;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
    }
    sv.script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect()
}

/// Both sides' lines past the mount: `PROBE` lines but the origin rows,
/// and the origin rows up to `PROBE done` (retail's server runs on past
/// it, ours stops there).
fn sides(retail: &str, cvars: &[(&str, &str)]) -> Option<(Vec<String>, Vec<String>)> {
    let fs = vcod_common::testing::game_fs_11()?;
    let ours = ours(fs, retail, cvars);
    if std::env::var_os("OWNLINE_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    let done: i64 = retail
        .lines()
        .find_map(|l| l.strip_prefix("PROBE done "))
        .and_then(|t| t.trim().parse().ok())
        .expect("retail's PROBE done");
    let r = probe_lines(retail.lines())
        .into_iter()
        .filter(|l| {
            l.strip_prefix("PROBE at ")
                .and_then(|t| t.split_whitespace().next()?.parse::<i64>().ok())
                .is_none_or(|t| t <= done)
        })
        .map(str::to_string)
        .collect();
    let o = probe_lines(ours.iter().map(String::as_str))
        .into_iter()
        .map(str::to_string)
        .collect();
    Some((r, o))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/ownline/{name}")).expect("the capture")
}

fn is_at(l: &str) -> bool {
    l.starts_with("PROBE at ")
}

/// Every line but the gunner's origin rows word for word: no round of a
/// whole sweep over both arcs met him, on retail as on ours, and the rows
/// started on the same frames.
#[test]
fn a_gunner_never_meets_his_own_round_on_retail() {
    let Some((r, o)) = sides(&read("mp_carentan-1.1.txt"), &[]) else {
        return;
    };
    let rest = |v: &[String]| -> Vec<String> { v.iter().filter(|l| !is_at(l)).cloned().collect() };
    assert!(
        !r.iter().any(|l| l.starts_with("PROBE hit ")),
        "retail logged a hit"
    );
    assert_eq!(rest(&r), rest(&o));
}

/// The gunner's origin rows of `name` (`cvars` on top of the probe's)
/// against ours, each within a float step at 2048 (gunduel's rows read
/// retail's to one, cod11-turrets.md 13.10).
fn placement(name: &str, cvars: &[(&str, &str)]) {
    let Some((r, o)) = sides(&read(name), cvars) else {
        return;
    };
    let fine = |c: &str| -> f64 {
        let (neg, c) = c.strip_prefix('-').map_or((false, c), |c| (true, c));
        let p: Vec<f64> = c.split(':').map(|x| x.parse().expect("a number")).collect();
        let v = p[0] + p[1] / 1e3 + p[2] / 1e9;
        if neg { -v } else { v }
    };
    let (at_r, at_o): (Vec<&String>, Vec<&String>) = (
        r.iter().filter(|l| is_at(l)).collect(),
        o.iter().filter(|l| is_at(l)).collect(),
    );
    assert_eq!(at_r.len(), at_o.len(), "origin row counts");
    let off: Vec<String> = at_r
        .iter()
        .zip(&at_o)
        .filter(|(a, b)| {
            let (wa, wb): (Vec<&str>, Vec<&str>) = (
                a.split_whitespace().collect(),
                b.split_whitespace().collect(),
            );
            wa[..4] != wb[..4]
                || wa[4]
                    .split(',')
                    .zip(wb[4].split(','))
                    .any(|(x, y)| (fine(x) - fine(y)).abs() > 2.5e-4)
        })
        .map(|(a, b)| format!("retail {a}\nours   {b}"))
        .collect();
    assert!(off.is_empty(), "{} rows off\n{}", off.len(), off.join("\n"));
}

/// The same sweep with the trigger up: every frame's placement is retail's.
#[test]
fn a_sweeping_gunner_is_placed_where_retail_places_him() {
    placement("mp_carentan-nofire-1.1.txt", &[("probe_nofire", "1")]);
}

/// The same with the trigger held: the fire group's `fire_30left` leaves
/// key a second root spot, and the placement reads each leaf's root at the
/// group's own time (cod11-turrets.md 7.2). Read at frame 0, 97 of 362
/// frames sat up to 0.57 off.
#[test]
fn a_firing_gunner_is_placed_where_retail_places_him() {
    placement("mp_carentan-1.1.txt", &[]);
}
