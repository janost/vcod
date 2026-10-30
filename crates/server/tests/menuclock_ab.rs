//! The clock a menu answer's spawn runs on, ours against retail 1.1d's,
//! 1.5b's and UO 1.51b's runs of `client-probes/probe_menuclock` on
//! mp_pavlov (docs/research/cod11-gametypes-re-bel.md 8.7). One client
//! sits as a spectator until its `mr`, whose waiter spawns it; two `kill`s
//! respawn it from the killed callback half a second later. Each spawn
//! logs `PROBE spawned` and, a `wait 0.05` past its "spawned" notify,
//! `PROBE placed`.
//!
//! Per spawn the gate compares the pick's `getTime()` with the first
//! playing snapshot (`pm_type` 0) after a non-playing one, and the placing
//! with the pick. Retail's client side is the probe's `--save-scripted`
//! capture (`menuclock-first*.txt`), its log `menuclock-log*.txt`.
//! `MC_REPORT=1` prints both summaries. Each test runs at its own
//! install's `COD_DIR`: 1.1, 1.5 (a UO install's `main/` counts) or UO.

use crate::common;

use common::{Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_menuclock.gsc");

/// Each spawn's pick against the first playing snapshot of its life, and
/// its placing against the pick. `plays` is every snapshot's
/// `(serverTime, pm_type)` in order.
fn summary(log: &[String], plays: &[(i64, i32)]) -> Vec<String> {
    let probe = |l: &str, tag: &str| -> Option<i64> {
        let w: Vec<&str> = l.trim().strip_prefix(tag)?.split(' ').collect();
        w.get(2)?.parse().ok()
    };
    let mut lives = Vec::new();
    let mut playing = false;
    for &(t, pm_type) in plays {
        if pm_type == 0 && !playing {
            lives.push(t);
        }
        playing = pm_type == 0;
    }
    let mut lives = lives.into_iter();
    let mut out = Vec::new();
    for (i, l) in log.iter().enumerate() {
        let Some(t) = probe(l, "PROBE spawned ") else {
            continue;
        };
        out.push(match lives.next() {
            Some(p) => format!("pick {:+} of its first playing snapshot", t - p),
            None => "never playing".to_string(),
        });
        let placed = log[i + 1..].iter().find_map(|l| probe(l, "PROBE placed "));
        out.push(match placed {
            Some(p) => format!("placed {:+}", p - t),
            None => "unplaced".to_string(),
        });
    }
    out
}

/// `(serverTime, pm_type)` off a `--save-scripted` capture's `!trace`
/// lines.
fn retail_plays(capture: &str) -> Vec<(i64, i32)> {
    capture
        .lines()
        .filter_map(|l| {
            let l = l.strip_prefix("!trace ")?;
            let field = |k: &str| {
                l.split(' ')
                    .find_map(|w| w.strip_prefix(k)?.strip_prefix('='))
            };
            Some((
                field("serverTime")?.parse().ok()?,
                field("pm_type")?.parse().ok()?,
            ))
        })
        .collect()
}

/// Ours: the script log and every snapshot's `(serverTime, pm_type)`.
fn ours(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol) -> (Vec<String>, Vec<(i64, i32)>) {
    let map = "mp_pavlov";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_menuclock"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_menuclock", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut plays: Vec<(i64, i32)> = Vec::new();
    let mut begun: Option<usize> = None;
    for i in 0..1000 {
        now += frame;
        if begun.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE begin")) {
            begun = Some(i);
        }
        // The `mr` and the two `kill`s, 3 s apart as in retail's run.
        match begun.map(|b| i - b) {
            Some(120) => cl.send_reliable(&format!("mr {} 0 a", cl.server_id())),
            Some(180 | 240) => cl.send_reliable("kill"),
            Some(300..) => break,
            _ => {}
        }
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest() {
            let t = i64::from(s.server_time);
            if plays.last().is_none_or(|&(last, _)| last < t) {
                plays.push((t, s.ps.field_i32(cl.protocol(), "pm_type")));
            }
        }
    }
    (sv.script_log().to_vec(), plays)
}

fn check(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol, log: &str, capture: &str) {
    let log: Vec<String> = log.lines().map(str::to_string).collect();
    let retail = summary(&log, &retail_plays(capture));
    let (our_log, plays) = ours(fs, proto);
    let ours = summary(&our_log, &plays);
    if std::env::var_os("MC_REPORT").is_some() {
        eprintln!("retail: {retail:#?}\nours: {ours:#?}");
    }
    assert_eq!(retail.len(), 6, "retail's three spawns: {retail:#?}");
    assert_eq!(ours, retail, "our log:\n{}", our_log.join("\n"));
}

#[test]
fn a_menu_answers_spawn_runs_on_retails_clock() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V1,
        include_str!("fixtures/threadorder/menuclock-log.txt"),
        include_str!("fixtures/threadorder/menuclock-first.txt"),
    );
}

#[test]
fn a_menu_answers_spawn_runs_on_retails_clock_on_1_5() {
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
        include_str!("fixtures/threadorder/menuclock-log-1.5.txt"),
        include_str!("fixtures/threadorder/menuclock-first-1.5.txt"),
    );
}

#[test]
fn a_menu_answers_spawn_runs_on_retails_clock_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V22,
        include_str!("fixtures/threadorder/menuclock-log-uo.txt"),
        include_str!("fixtures/threadorder/menuclock-first-uo.txt"),
    );
}
