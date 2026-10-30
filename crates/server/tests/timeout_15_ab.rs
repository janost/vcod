//! 1.5's match timeout on a protocol 6 server, against the 1.5b capture in
//! `fixtures/timeout/mp_carentan-tdm-timeout-1.5.txt`: the same three client
//! commands, a second apart, must bring the same configstring and `e` lines
//! in the same order, and the snapshots must stand at one `serverTime` for
//! as many frames (docs/research/cod15-protocol-diff.md 12).
//!
//! Needs `COD_DIR` at a 1.5 install; on a 1.1 one (the suite's usual) or
//! without paks it returns early.

use crate::common;

use common::Queues;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::protocol::PROTOCOL_V6;

const FIXTURE: &str = "timeout/mp_carentan-tdm-timeout-1.5.txt";

/// Retail's frozen run is wall-timed: the probe's commands left on its own
/// clock and the recovery ends on `trap_Milliseconds`, so a frame either way.
const FROZEN_SLACK: i64 = 2;

struct Retail {
    says: Vec<(f32, String)>,
    cmds: Vec<String>,
    frozen: i64,
}

fn unescape(s: &str) -> String {
    s.replace("\\x14", "\u{14}").replace("\\x15", "\u{15}")
}

fn retail() -> Retail {
    let path = format!("{}/tests/fixtures/{FIXTURE}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut r = Retail {
        says: Vec::new(),
        cmds: Vec::new(),
        frozen: 0,
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!say ") {
            let (secs, cmd) = rest.split_once(' ').unwrap();
            r.says.push((secs.parse().unwrap(), cmd.to_string()));
        } else if let Some(cmd) = line.strip_prefix("!cmd ") {
            r.cmds.push(unescape(cmd));
        } else if let Some(rest) = line.strip_prefix("!frozen ") {
            r.frozen = rest.split(' ').nth(1).unwrap().parse().unwrap();
        }
    }
    r
}

/// The commands the fixture keeps: configstrings 1628, 1629, 227 and `e`.
fn kept(text: &str) -> bool {
    ["d 1628 ", "d 1629 ", "d 227 ", "e "]
        .iter()
        .any(|p| text.starts_with(p))
}

#[test]
fn a_match_timeout_runs_as_15b_ran_it() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let retail = retail();
    let bsp_path = fs
        .resolve_map("mp_carentan")
        .expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg("mp_carentan", "tdm"), now);
    sv.set_protocol(&PROTOCOL_V6);
    for (name, value) in [
        ("g_timeoutsAllowed", "2"),
        ("g_timeoutLength", "15000"),
        ("g_timeoutRecovery", "5000"),
        ("g_timeoutBank", "180000"),
    ] {
        sv.set_cvar(name, value);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (mut cl, join) = common::join(&mut sv, &q, &mut now, "axis", "kar98k_mp");
    assert!(join.findings().is_empty(), "{}", join.summary());

    // The commands keep retail's spacing, from the first.
    cl.take_server_commands();
    let first = retail.says[0].0;
    let start = now;
    let mut says = retail.says.iter().peekable();
    let mut cmds = Vec::new();
    // serverTime per snapshot message, each message once.
    let mut snaps: Vec<(u32, i32)> = Vec::new();
    for _ in 0..400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        while let Some((secs, cmd)) = says.peek()
            && now.duration_since(start).as_secs_f32() >= secs - first
        {
            cl.send_reliable(cmd);
            says.next();
        }
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        for e in common::step(&mut sv, &q, &mut cl, now) {
            assert!(!matches!(e, NetEvent::Dropped(_)), "{e:?}");
        }
        cmds.extend(cl.take_server_commands().into_iter().filter(|t| kept(t)));
        if let Some(s) = cl.snapshots().newest()
            && snaps.last().is_none_or(|(m, _)| *m != s.message_num)
        {
            snaps.push((s.message_num, s.server_time));
        }
        if cmds.len() >= retail.cmds.len() && cmds.last().is_some_and(|c| c == "d 227 1") {
            break;
        }
    }
    assert_eq!(cmds, retail.cmds, "the timeout's commands, in order");
    let mut runs: Vec<(i32, i64)> = Vec::new();
    for (_, t) in &snaps {
        match runs.last_mut() {
            Some((at, n)) if at == t => *n += 1,
            _ => runs.push((*t, 1)),
        }
    }
    let frozen = runs.iter().map(|(_, n)| *n).max().unwrap_or(0);
    assert!(
        (frozen - retail.frozen).abs() <= FROZEN_SLACK,
        "{frozen} snapshots stood at one serverTime, 1.5b's {}",
        retail.frozen
    );
}
