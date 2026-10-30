//! A prone press from a crouch and a body let go of a `setOrigin` hold,
//! ours against retail 1.1d's, 1.5b's and UO 1.51b's
//! (docs/research/cod11-mantle.md, "Prone under a setOrigin hold").
//! `client-probes/probe_pronecrouch` drives one `--probe-uo-weapon direct`
//! client through `PROBE_IN` lines on mp_carentan: crouch then prone on a
//! flat floor, the same keys while a thread holds him a unit over the
//! ground with `setOrigin` every frame, and a body let go alive and dead.
//! The fixtures are the client's `--save-slope` capture at the net probe's
//! 60 Hz cmds, cut to the fields read here, and the server's `PROBE` lines.
//!
//! Ours replays each captured cmd on the side of a frame retail ran it on
//! (`ProbeHold::ran`, the snapshots' `ct=`), with the captured keys. Each
//! phase is one row, held exactly: every change of `pm_flags & 3` (prone,
//! ducked) and of the origin's height, each at its snapshot's time past the
//! phase's start, and the `PROBE pos` rows likewise.
//!
//! `PRONECROUCH_REPORT=1` prints both sides' rows. Needs `COD_DIR`; without
//! the paks it returns early.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::event_ids;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::protocol::PROTOCOL_V22;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_pronecrouch.gsc");

/// About how long our client takes from its start to the script's first
/// phase (the join, the spawn and the script's 4 s wait); the replay runs
/// again on the measured lead when it is off.
const JOIN_TO_PHASE_MS: i32 = 5150;

/// One snapshot as both sides are read: time, `pm_flags & 3`, origin z,
/// and the event ids it brought in 1.1's numbering.
type Snap = (i32, i32, f32, Vec<i32>);

fn kv(line: &str) -> BTreeMap<&str, &str> {
    line.split_whitespace()
        .filter_map(|t| t.split_once('='))
        .collect()
}

fn retail_snaps(fixture: &str) -> Vec<Snap> {
    fixture
        .lines()
        .filter(|l| l.starts_with("!snap "))
        .map(|l| {
            let m = kv(l);
            let z = m["origin"].split(',').nth(2).unwrap().parse().unwrap();
            let events = m["ev"]
                .split(',')
                .filter_map(|e| e.split_once(':'))
                .map(|e| e.0.parse().unwrap())
                .collect();
            (
                m["t"].parse().unwrap(),
                m["pm_flags"].parse::<i32>().unwrap() & 3,
                z,
                events,
            )
        })
        .collect()
}

/// The cmd times, the key changes and the snapshots' `(time, commandTime)`.
type Cmds = (Vec<i32>, Vec<(i32, common::CmdInput)>, Vec<(i32, i32)>);

/// The capture's cmds the server ran (`serverTime` rising) and each one's
/// keys, and each snapshot's time and `commandTime`.
fn retail_cmds(fixture: &str) -> Cmds {
    let (mut times, mut pressed) = (Vec::new(), Vec::new());
    let mut last = None;
    for l in fixture.lines().filter(|l| l.starts_with("!cmd ")) {
        let m = kv(l);
        let t: i32 = m["st"].parse().unwrap();
        if times.last().is_some_and(|&n| t <= n) {
            continue;
        }
        times.push(t);
        let input = (
            m["buttons"].parse().unwrap(),
            m["wbuttons"].parse().unwrap(),
            m["up"].parse().unwrap(),
        );
        if last != Some(input) {
            pressed.push((t, input));
            last = Some(input);
        }
    }
    let ran = fixture
        .lines()
        .filter(|l| l.starts_with("!snap "))
        .map(|l| {
            let m = kv(l);
            (m["t"].parse().unwrap(), m["ct"].parse().unwrap())
        })
        .collect();
    (times, pressed, ran)
}

/// `PROBE phase <time> <name>` and `PROBE done <time>`, in order.
fn phases(lines: &[String]) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.as_slice() {
            ["PROBE", "phase", time, name] => out.push((time.parse().unwrap(), name.to_string())),
            ["PROBE", "done", time] => out.push((time.parse().unwrap(), "done".into())),
            _ => {}
        }
    }
    out
}

/// The capture's first phase's time.
fn first_phase(fixture: &str) -> i32 {
    let lines: Vec<String> = fixture.lines().map(str::to_string).collect();
    phases(&lines).first().expect("a phase").0
}

/// Each phase's changes of stance bits and height, the events its
/// snapshots brought, and its `PROBE pos` rows, at their time past the
/// phase's start.
fn rows(snaps: &[Snap], lines: &[String]) -> BTreeMap<String, Vec<String>> {
    let ph = phases(lines);
    let mut out = BTreeMap::new();
    for w in ph.windows(2) {
        let ((start, name), (end, _)) = (&w[0], &w[1]);
        let mut row = Vec::new();
        let mut last = None;
        for (t, bits, z, events) in snaps.iter().filter(|s| s.0 >= *start && s.0 < *end) {
            if last != Some((*bits, *z)) {
                row.push(format!("{} {bits} {z}", t - start));
                last = Some((*bits, *z));
            }
            if !events.is_empty() {
                row.push(format!("ev {} {events:?}", t - start));
            }
        }
        for l in lines {
            let t: Vec<&str> = l.split_whitespace().collect();
            if let ["PROBE", "pos", time, x, y, z] = t.as_slice() {
                let time: i32 = time.parse().unwrap();
                if time > *start && time <= *end {
                    row.push(format!("pos {} {x} {y} {z}", time - start));
                }
            }
        }
        out.insert(name.clone(), row);
    }
    out
}

/// Ours under the probe: its snapshots and its script log.
fn ours(fs: vcod_common::pk3::Pk3Fs, fixture: &str, lead: i32) -> (Vec<Snap>, Vec<String>) {
    let p = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_pronecrouch"), now);
    sv.set_protocol(p);
    sv.overlay_script("maps/mp/gametypes/probe_pronecrouch", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (times, pressed, ran) = retail_cmds(fixture);
    // Join so that our first phase starts on retail's frame: the cmds run
    // on retail's clock.
    common::run_alone(&mut sv, &mut now, i64::from(first_phase(fixture) - lead));
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "m1carbine_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        framed: true,
        times,
        ran,
        ..Default::default()
    };
    let mut snaps: Vec<Snap> = Vec::new();
    let mut spawned = false;
    let mut last_seq: Option<i32> = None;
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..4000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        // Once spawned the captured cmds go out dead or alive, as the net
        // probe's did.
        spawned |= cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(p, "pm_type") == 0);
        if spawned {
            let mut cmds = hold.cmds(&cl, 0);
            for c in &mut cmds {
                let input = match pressed.partition_point(|e| e.0 <= c.server_time) {
                    0 => (0, 0, 0),
                    i => pressed[i - 1].1,
                };
                (c.buttons, c.wbuttons, c.up) = input;
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for pk in pending {
            sv.handle_packet(common::ADDR, &pk, now);
        }
        sv.tick(now);
        for (_, pk) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(pk);
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                // The keys come off the capture's cmds, not `PROBE_IN`.
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest()
            && snaps.last().is_none_or(|l| l.0 < s.server_time)
        {
            let z = s.ps.origin(p)[2];
            let bits = s.ps.field_i32(p, "pm_flags") & 3;
            let seq = s.ps.field_i32(p, "eventSequence");
            let gained = last_seq.map_or(0, |prev| ((seq - prev) & 0xff).min(4));
            let events = ((seq - gained)..seq)
                .map(|k| s.ps.field_i32(p, &format!("events[{}]", k & 3)))
                .map(
                    |id| match p.transport.version == PROTOCOL_V22.transport.version {
                        true => event_ids::event_v22_to_v1(id),
                        false => id,
                    },
                )
                .collect();
            last_seq = Some(seq);
            snaps.push((s.server_time, bits, z, events));
        }
    }
    let log = sv
        .script_log()
        .iter()
        .filter_map(|l| l.find("PROBE ").map(|i| l[i..].trim_end().to_string()))
        .collect();
    (snaps, log)
}

/// `fs` mounts the install's paks, `None` where the case does not run.
fn check(fs: fn() -> Option<vcod_common::pk3::Pk3Fs>, fixture: &str) {
    let Some(paks) = fs() else {
        return;
    };
    let retail_log: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect();
    let theirs = rows(&retail_snaps(fixture), &retail_log);
    let (mut snaps, mut log) = ours(paks, fixture, JOIN_TO_PHASE_MS);
    let off = phases(&log).first().expect("our first phase").0 - first_phase(fixture);
    if off != 0 {
        (snaps, log) = ours(fs().unwrap(), fixture, JOIN_TO_PHASE_MS + off);
    }
    assert_eq!(
        phases(&log).first().map(|p| p.0),
        Some(first_phase(fixture)),
        "our first phase on retail's frame"
    );
    let mine = rows(&snaps, &log);
    let report = std::env::var_os("PRONECROUCH_REPORT").is_some();
    let mut bad = Vec::new();
    for (name, want) in &theirs {
        let got = mine.get(name);
        if report {
            println!("{name}\n  retail {want:?}\n  ours   {got:?}");
        }
        if got != Some(want) {
            bad.push(name.as_str());
        }
    }
    assert!(theirs.len() >= 10, "the capture holds its phases");
    assert!(
        bad.is_empty(),
        "phases off retail: {bad:?} (PRONECROUCH_REPORT=1)"
    );
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = vcod_common::net::protocol::PROTOCOL_V6.transport.version;
    (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs)
}

#[test]
fn prone_from_a_crouch_and_a_held_body_s_landing_match_retail() {
    check(
        vcod_common::testing::game_fs_11,
        include_str!("fixtures/pronecrouch/mp_carentan-1.1.txt"),
    );
}

#[test]
fn prone_from_a_crouch_and_a_held_body_s_landing_match_15() {
    check(
        fs_15,
        include_str!("fixtures/pronecrouch/mp_carentan-1.5.txt"),
    );
}

#[test]
fn prone_from_a_crouch_and_a_held_body_s_landing_match_uo() {
    check(
        vcod_common::testing::uo_fs,
        include_str!("fixtures/pronecrouch/mp_carentan-uo.txt"),
    );
}
