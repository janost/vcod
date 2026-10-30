//! The order a frame runs the threads the touch pass woke, ours against
//! retail 1.1d's, 1.5b's and UO 1.51b's runs of
//! `client-probes/probe_touchorder` on mp_pavlov
//! (docs/research/cod11-combat.md 14.7, "The touch pass";
//! docs/research/cod11-gsc-language.md, "Engine notifies and the trigger
//! queue"). One client stands where minefields 101 and 102 overlap beside
//! three spawned items for a fifth of a second per phase; the script logs
//! every `"trigger"` (`f`), item `"touch"` (`t`), mine `"touch"` (`mt`) and
//! player `"touch"` (`pt`).
//!
//! Retail logs, each frame of a phase: one `f` pair, then the touch lines,
//! then one more `f` pair for every later cmd of the frame. A `"touch"` is
//! notified inside each cmd and only the first cmd's reaches a waiter (it
//! has not run again by the next), where `Touch_Multi` queues its
//! `"trigger"` and `G_RunFrame` hands the queue out one trigger per entity
//! a round, running the threads after each.
//!
//! The probe client's cmds are not on the frame grid, so how many land in
//! one frame differs run to run: the gate compares each frame's first round
//! and its touch lines exactly, and checks that every later line is an `f`
//! pair. `TO_REPORT=1` prints both logs.
//!
//! The three items fall onto the floor before the first phase, and each
//! rests at its sweep's end plus `G_BounceItem`'s random lift, a `rand()`
//! no seed replays. The gate hands each landing retail's lift (retail's
//! rest over ours' floor), holds it to the patch's range, (0, 0.5] on
//! 1.1d and [0.5, 1) on 1.5b and UO (cod11-items.md 14.2), and compares
//! every item line exactly.
//!
//! The `probe_trigqueue` tests fill the queue (see `check_trigqueue`).
//! Each test runs at its own install's `COD_DIR`: 1.1, 1.5 (a UO install's
//! `main/` counts) or UO.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_touchorder.gsc");

/// The phase lines and, under each, every frame's lines (times stripped)
/// in log order.
fn frames(lines: &[String]) -> Vec<(String, Vec<Vec<String>>)> {
    let mut out: Vec<(String, Vec<Vec<String>>)> = Vec::new();
    let mut last_time = None;
    for l in lines.iter().map(|l| l.trim()) {
        let Some(rest) = l.strip_prefix("PROBE ") else {
            continue;
        };
        if let Some(tag) = rest.strip_prefix("phase ") {
            out.push((tag.to_string(), Vec::new()));
            last_time = None;
            continue;
        }
        let mut w = rest.split(' ');
        let kind = w.next().unwrap_or_default();
        if !matches!(kind, "f" | "t" | "mt" | "pt") {
            continue;
        }
        let (Some(who), Some(time)) = (w.next(), w.next()) else {
            continue;
        };
        let Some((_, phase)) = out.last_mut() else {
            continue;
        };
        if last_time != Some(time.to_string()) {
            phase.push(Vec::new());
            last_time = Some(time.to_string());
        }
        phase.last_mut().unwrap().push(format!("{kind} {who}"));
    }
    out
}

/// A frame's lines up to the end of its touch lines; the rest have to be
/// `f` pairs, one per later cmd.
fn first_round(frame: &[String]) -> Vec<String> {
    let end = frame
        .iter()
        .rposition(|l| !l.starts_with("f "))
        .map_or(2.min(frame.len()), |i| i + 1);
    let tail = &frame[end..];
    assert!(
        tail.len().is_multiple_of(2) && tail.chunks(2).all(|c| c == ["f 101", "f 102"]),
        "a frame's later lines are not f pairs: {frame:?}"
    );
    frame[..end].to_vec()
}

/// Each item's spawned rest in a retail log, `(x, y, z)` per item line.
fn rests(lines: &[String]) -> Vec<[f32; 3]> {
    lines
        .iter()
        .filter(|l| l.contains(" spawned item "))
        .map(|l| {
            let v = &l[l.find('(').unwrap() + 1..l.find(')').unwrap()];
            let n: Vec<f32> = v.split(", ").map(|c| c.parse().unwrap()).collect();
            [n[0], n[1], n[2]]
        })
        .collect()
}

/// Ours' run with every item landing handed retail's lift over ours' floor;
/// the lifts come back beside the log.
fn ours(
    fs: vcod_common::pk3::Pk3Fs,
    proto: &'static Protocol,
    retail: &[[f32; 3]],
) -> (Vec<String>, Vec<f32>) {
    let map = "mp_pavlov";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_touchorder"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_touchorder", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let lifts = Rc::new(RefCell::new(Vec::new()));
    let (seen, retail) = (lifts.clone(), retail.to_vec());
    sv.test_item_lifts(Box::new(move |_, floor| {
        let r = retail
            .iter()
            .find(|r| r[0] == floor[0] && r[1] == floor[1])
            .unwrap_or_else(|| panic!("no retail item lands at {floor:?}"));
        seen.borrow_mut().push(r[2] - floor[2]);
        r[2] - floor[2]
    }));
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "mosin_nagant_mp");
    let mut hold = common::ProbeHold::default();
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..1000 {
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let cmds = hold.cmds(&cl, 0);
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&vcod_common::net::msg::NULL_USERCMD) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    let lifts = lifts.borrow().clone();
    (sv.script_log().to_vec(), lifts)
}

/// Ours against one retail log of the probe.
fn check(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol, retail: &str) {
    let retail: Vec<String> = retail.lines().map(str::to_string).collect();
    let (ours, lifts) = ours(fs, proto, &rests(&retail));
    if std::env::var_os("TO_REPORT").is_some() {
        eprintln!("retail:\n{}\nours:\n{}", retail.join("\n"), ours.join("\n"));
        eprintln!("lifts {lifts:?}");
    }
    // Every item lands once, inside its patch's lift range; the log prints
    // to the hundredth, so a lift may read up to 0.005 past either end.
    let (lo, hi) = if proto.transport.version == PROTOCOL_V1.transport.version {
        (0.0, 0.5)
    } else {
        (0.5, 1.0)
    };
    assert_eq!(lifts.len(), 3, "landings {lifts:?}");
    for l in &lifts {
        assert!(
            *l > lo - 0.005 && *l < hi + 0.005,
            "lift {l} outside ({lo}, {hi}]"
        );
    }
    let items = |log: &[String]| -> Vec<String> {
        log.iter()
            .filter(|l| l.contains(" item "))
            .map(|l| l.trim().to_string())
            .collect()
    };
    assert_eq!(items(&ours), items(&retail), "item rests");
    let (r, o) = (frames(&retail), frames(&ours));
    let shape = |p: &[(String, Vec<Vec<String>>)]| -> Vec<(String, usize, Vec<String>)> {
        p.iter()
            .map(|(tag, fr)| (tag.clone(), fr.len(), first_round(&fr[0])))
            .collect()
    };
    assert_eq!(shape(&o), shape(&r), "ours:\n{}", ours.join("\n"));
    // Every later frame of a phase repeats the first's touch lines.
    for (tag, fr) in r.iter().chain(&o) {
        for f in &fr[1..] {
            assert_eq!(first_round(f), first_round(&fr[0]), "phase {tag}");
        }
    }
}

#[test]
fn the_touch_pass_wakes_its_threads_in_retails_order() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V1,
        include_str!("fixtures/touchorder/mp_pavlov.txt"),
    );
}

/// Retail 1.5b's run: the same lines as 1.1d's.
#[test]
fn the_touch_pass_wakes_its_threads_in_retails_order_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(
        fs,
        &PROTOCOL_V6,
        include_str!("fixtures/touchorder/mp_pavlov-1.5.txt"),
    );
}

/// Retail UO 1.51b's run: the same lines as 1.1d's.
#[test]
fn the_touch_pass_wakes_its_threads_in_retails_order_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V22,
        include_str!("fixtures/touchorder/mp_pavlov-uo.txt"),
    );
}

/// `client-probes/probe_trigqueue` on ours, no client, on mp_carentan.
fn trigqueue(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol) -> Vec<String> {
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_trigqueue"), now);
    sv.set_protocol(proto);
    sv.overlay_script(
        "maps/mp/gametypes/probe_trigqueue",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_trigqueue.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, 8000);
    sv.script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect()
}

/// The full queue: retail 1.1d, 1.5b and UO 1.51b logged the same lines
/// (`fixtures/touchorder/trigqueue.txt`). 256 activations in one step
/// queue and are dropped with the round's queue; the 257th notifies on the
/// spot, its waiter running after the step; a thread off the frame grid
/// queues after the rounds, so its entries are handed out next frame, 256
/// rounds of one.
fn check_trigqueue(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol) {
    let retail: Vec<&str> = include_str!("fixtures/touchorder/trigqueue.txt")
        .lines()
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    let ours = trigqueue(fs, proto);
    if std::env::var_os("TO_REPORT").is_some() {
        eprintln!("ours:\n{}", ours.join("\n"));
    }
    assert_eq!(ours, retail);
}

#[test]
fn a_full_trigger_queue_notifies_on_the_spot_on_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_trigqueue(fs, &PROTOCOL_V1);
}

#[test]
fn a_full_trigger_queue_notifies_on_the_spot_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check_trigqueue(fs, &PROTOCOL_V6);
}

#[test]
fn a_full_trigger_queue_notifies_on_the_spot_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_trigqueue(fs, &PROTOCOL_V22);
}
