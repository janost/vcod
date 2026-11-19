//! Whether a lean moves the eye a snapshot is culled from, ours against
//! retail 1.1d's. `client-probes/probe_leanpvs` holds an allies watcher and
//! an axis target where vcod's PVS changes along the watcher's lean, and
//! drives the watcher's lean through `PROBE_IN` lines (`--probe-uo-weapon
//! direct`); the fixture keeps each change in whether his snapshots carry
//! the target (docs/research/cod11-combat.md 3.6, "The leaning eye's
//! PVS").
//!
//! `LPVS_REPORT=1` prints ours' changes. Needs `COD_DIR` at a 1.1 install.

use crate::common;
use vcod_common::net::Msec;

use common::{Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_leanpvs.gsc");
const FIXTURE: &str = include_str!("fixtures/leanpvs/mp_carentan-leanpvs.txt");

/// Server time before ours' target connects, and how long after it the
/// watcher does: the recipe's first playing snapshots, 7700 and 9900.
const TARGET_JOIN_MS: i64 = 6900;
const WATCHER_LATE_MS: i64 = 2200;

/// The watcher's snapshot span the fixture covers.
const FROM_MS: i32 = 14000;
const TO_MS: i32 = 27000;

/// The buttons, `wbuttons` and `up` of a `f "PROBE_IN ..."` command.
fn direct_input(tokens: &[String]) -> Option<(u8, u8, i8)> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    Some((
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    ))
}

/// Ours' changes in whether the watcher's snapshots carry the target's
/// entity, as `(serverTime, seen)`.
fn ours() -> Vec<(i32, bool)> {
    let fs = vcod_common::testing::game_fs_11().expect("checked by the caller");
    let map = "mp_carentan";
    let (mut sv, mut now) = common::Boot {
        script: Some(SCRIPT),
        ..common::Boot::new(map, "probe_leanpvs")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, TARGET_JOIN_MS);
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut ca: Option<NetClient<common::ClientEnd>> = None;
    let mut cb = NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2002);
    let (mut ja, mut jb) = (
        Join::new("allies", "springfield_mp"),
        Join::new("axis", "kar98k_mp"),
    );
    let hold = || common::ProbeHold {
        aim: true,
        framed: true,
        ..Default::default()
    };
    let (mut ha, mut hb) = (hold(), hold());
    let mut ia = (0u8, 0u8, 0i8);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let live = |cl: &NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0)
    };
    let send = |cl: &mut NetClient<common::ClientEnd>,
                hold: &mut common::ProbeHold,
                input: (u8, u8, i8)| {
        if live(cl) {
            let mut cmds = hold.cmds(cl, input.0);
            for c in &mut cmds {
                c.wbuttons = input.1;
                c.up = input.2;
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
    };
    let mut seen: Vec<(i32, bool)> = Vec::new();
    let mut last_time = 0;
    for i in 0..1000i64 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        if ca.is_none() && i * common::FRAME_MS >= WATCHER_LATE_MS {
            ca = Some(NetClient::start_with_qport(
                common::ClientEnd(qa.clone()),
                now,
                0x2001,
            ));
        }
        if let Some(ca) = ca.as_mut() {
            send(ca, &mut ha, ia);
        }
        send(&mut cb, &mut hb, (0, 0, 0));
        for (addr, q) in [(common::ADDR, &qa), (common::ADDR_B, &qb)] {
            let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(addr, &p, now);
            }
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            let q = if to == common::ADDR { &qa } else { &qb };
            q.borrow_mut().to_client.push_back(p);
        }
        if let Some(ca) = ca.as_mut() {
            for e in ca.pump_at(now) {
                if let NetEvent::ServerCommand(tokens) = e {
                    if let Some(input) = direct_input(&tokens) {
                        ia = input;
                    }
                    ja.on_server_command(&tokens, ca, now);
                }
            }
            if let Some(s) = ca.snapshots().newest()
                && s.server_time != Msec::from_raw(last_time)
                && (FROM_MS..=TO_MS).contains(&s.server_time.raw())
            {
                last_time = s.server_time.raw();
                let carried = s.entities.contains_key(&0);
                if seen.last().is_none_or(|&(_, v)| v != carried) {
                    seen.push((s.server_time.raw(), carried));
                }
            }
        }
        for e in cb.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                jb.on_server_command(&tokens, &mut cb, now);
            }
        }
    }
    seen
}

#[test]
fn a_leaning_eye_culls_the_snapshot_as_on_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let retail: Vec<(i32, bool)> = FIXTURE
        .lines()
        .filter_map(|l| {
            let mut t = l.strip_prefix("SEEN ")?.split(' ');
            Some((t.next()?.parse().ok()?, t.next()? == "1"))
        })
        .collect();
    let ours = ours();
    if std::env::var_os("LPVS_REPORT").is_some() {
        eprintln!("retail {retail:?}\nours   {ours:?}");
    }
    assert_eq!(ours, retail, "the target in the watcher's snapshots");
}
