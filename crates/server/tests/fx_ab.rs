//! Map effects on the wire against the retail captures
//! `tests/fixtures/gametypes/mp_carentan-playfx-fx.txt`: what `playFx`,
//! `playFx` with a forward, `playLoopedFx` and `playFxOnTag` put in a
//! client's snapshots under `client-probes/probe_playfx.gsc`, and
//! `mp_carentan-fxontag-fx.txt`: `playFxOnTag` on a script model under
//! `client-probes/probe_fxontag.gsc`. Both are recorded by
//! `vcod_common::net::capture::ScriptedCapture`'s `!fx` lines on both
//! sides (docs/research/cod11-events-and-fx.md 2, "On the wire").
//!
//! Each `!fx` line is compared with its server time taken from the probe
//! step before it (`mp_carentan-playfx-log.txt` for retail, the script log
//! for ours), so the two servers' clocks need not agree, only the frame each
//! effect reaches the wire on. `FX_REPORT=1` prints both sides.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{ADDR, ClientEnd, Queues};
use vcod_common::net::capture::ScriptedCapture;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const FRAME_MS: u64 = 50;
/// Retail's server had been up about this long when the probe joined.
const JOIN_MS: u64 = 5000;
const RUN_MS: u64 = 20_000;

/// The `PROBE <step> <time>` lines' times.
fn step_times(log: &[String]) -> Vec<i64> {
    log.iter()
        .filter_map(|l| l.strip_prefix("PROBE ")?.split(' ').nth(1)?.parse().ok())
        .collect()
}

/// The time of the `PROBE done` line, past which a probe plays on for a
/// human to watch.
fn done_time(log: &[String]) -> Option<i64> {
    log.iter()
        .find_map(|l| l.strip_prefix("PROBE done ")?.parse().ok())
}

/// Each `!fx` line before `until` without its `ms`, its `serverTime`
/// replaced by the offset from the last step at or before it.
fn fx_view(lines: &[String], steps: &[i64], until: Option<i64>) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| {
            let rest = l.strip_prefix("!fx ms=")?;
            let (_, rest) = rest.split_once(" serverTime=")?;
            let (t, what) = rest.split_once(' ')?;
            let t: i64 = t.parse().ok()?;
            if until.is_some_and(|u| t >= u) {
                return None;
            }
            let step = steps.iter().rev().find(|&&s| s <= t)?;
            // A tag play rides the player's own ring: its origin is the
            // settled body's, which the two servers place a few thousandths
            // apart, not the effect's.
            let what = match what.split_once(" origin=") {
                Some((head, _)) if what.starts_with("tag ") => head,
                _ => what,
            };
            Some(format!("+{} {what}", t - step))
        })
        .collect()
}

/// `probe` on ours with one allied client recording as retail's did, and
/// the client's lines and the script log.
fn run_ours(probe: &str) -> Option<(Vec<String>, Vec<String>)> {
    let fs = vcod_common::testing::game_fs_11()?;
    let src = std::fs::read_to_string(format!(
        "../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc"
    ))
    .expect("the probe");
    let (mut sv, mut now) = common::Boot {
        script: Some(&src),
        ..common::Boot::new(MAP, probe)
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, JOIN_MS as i64);

    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(ClientEnd(q.clone()), now, 0x2001);
    let mut cap = ScriptedCapture::default();
    let (start, mut seen, mut answered) = (now, None, Vec::new());
    let mut main_menu = String::new();
    for _ in 0..RUN_MS / FRAME_MS {
        now += Duration::from_millis(FRAME_MS);
        let ms = now.duration_since(start).as_millis() as i64;
        let cmd = cap.cmd(ms, cl.snapshots().newest());
        cl.send_frame(&cmd);
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for pkt in pending {
            sv.handle_packet(ADDR, &pkt, now);
        }
        sv.tick(now);
        for (_, pkt) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(pkt);
        }
        for e in cl.pump_at(now) {
            match e {
                NetEvent::GamestateReady => answered.clear(),
                NetEvent::ServerCommand(t) => match t.first().map(String::as_str) {
                    Some("v") if t.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                        main_menu = t.get(2).cloned().unwrap_or_default();
                    }
                    Some("t") => {
                        let Some(idx) = t.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                            continue;
                        };
                        let reply = if main_menu.starts_with("team_") {
                            "allies"
                        } else {
                            "m1carbine_mp"
                        };
                        if !answered.contains(&idx) {
                            let id = cl.server_id();
                            cl.send_reliable(&format!("mr {id} {idx} {reply}"));
                            answered.push(idx);
                        }
                    }
                    _ => {}
                },
                NetEvent::Dropped(r) => panic!("dropped at {ms} ms: {r}"),
                _ => {}
            }
        }
        cl.take_server_commands();
        if let Some(s) = cl.snapshots().newest()
            && seen != Some(s.message_num)
        {
            seen = Some(s.message_num);
            cap.sample(ms, s, cl.configstrings());
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new(), "{probe} aborted");
    Some((cap.lines, sv.script_log().to_vec()))
}

/// `probe`'s `!fx` lines on ours against retail's fixture pair
/// `mp_carentan-<gametype>-fx.txt` and `-log.txt`, up to `PROBE done`;
/// `expect` is how many retail has there.
fn compare(probe: &str, expect: usize) {
    let Some((ours, our_log)) = run_ours(probe) else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let lines = |path: &str| -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("fixture")
            .lines()
            .map(str::to_string)
            .collect()
    };
    let gametype = probe.strip_prefix("probe_").unwrap_or(probe);
    let dir = "tests/fixtures/gametypes";
    let retail_log = lines(&format!("{dir}/{MAP}-{gametype}-log.txt"));
    let retail = fx_view(
        &lines(&format!("{dir}/{MAP}-{gametype}-fx.txt")),
        &step_times(&retail_log),
        done_time(&retail_log),
    );
    let ours = fx_view(&ours, &step_times(&our_log), done_time(&our_log));
    if std::env::var("FX_REPORT").is_ok_and(|v| v == "1") {
        eprintln!("retail:\n{}\nours:\n{}", retail.join("\n"), ours.join("\n"));
    }
    assert_eq!(retail.len(), expect, "{retail:#?}");
    assert_eq!(ours, retail);
}

#[test]
fn map_effects_reach_the_wire_as_retails_do() {
    compare("probe_playfx", 10);
}

/// `playFxOnTag` on a script model rides the model's own entity, and the
/// tag pair is found again whatever the tag's case (`strcasecmp`).
#[test]
fn an_effect_on_a_script_models_tag_reaches_the_wire_as_retails_does() {
    compare("probe_fxontag", 3);
}
