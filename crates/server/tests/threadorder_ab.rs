//! The order a frame runs the threads one client packet woke, ours against
//! retail 1.1d's run of `client-probes/probe_threadorder` on mp_pavlov
//! (docs/research/cod11-gsc-language.md, "The packet pass"). One client
//! sends its commands in pairs, both of a pair in one packet: `mr` alone,
//! `mr` then `kill`, `kill` then `mr`, two `mr`s. The script logs every
//! `menuresponse` waiter, the level waiter `Callback_PlayerKilled`'s notify
//! wakes, the thread it starts and its own `wait 0`, each with `getTime()`
//! and the last time a ticker ran.
//!
//! Each pair's lines are compared with their times taken relative to the
//! first line's ticker reading, since the commands land on different
//! frames run to run.
//!
//! The second gate is two clients each sending `kill` (`-pair.txt`): on the
//! rounds where both packets landed in one frame, client 1's first, retail
//! ran client 0's threads first. `TO_REPORT=1` prints both logs.
//!
//! Retail 1.5b and UO 1.51b ran both on 2026-10-20 (`*-1.5.txt`,
//! `*-uo.txt`) and logged 1.1d's order. Each test runs at its own
//! install's `COD_DIR`: 1.1, 1.5 (a UO install's `main/` counts) or UO.

use crate::common;

use common::{Join, Queues, step, step_pair_b_first};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_threadorder.gsc");

/// The commands, both of a pair in one packet, three seconds apart as in
/// retail's run (`--probe-say` at 6, 9, 12 and 15 s).
const PAIRS: &[&[&str]] = &[
    &["mr {sid} 0 a"],
    &["mr {sid} 0 b", "kill"],
    &["kill", "mr {sid} 0 c"],
    &["mr {sid} 0 d", "mr {sid} 0 e"],
];

/// The log cut into one group per pair (a line more than a second past the
/// last starts the next), each line's `t` and `tick` taken from the group's
/// first `tick`.
fn groups(lines: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut last_t: Option<i64> = None;
    let mut base = 0;
    for l in lines.iter().map(|l| l.trim()) {
        let Some(rest) = l.strip_prefix("PROBE ") else {
            continue;
        };
        let w: Vec<&str> = rest.split(' ').collect();
        let (Some(ti), Some(ki)) = (
            w.iter().position(|&x| x == "t"),
            w.iter().position(|&x| x == "tick"),
        ) else {
            continue;
        };
        let t: i64 = w[ti + 1].parse().expect("t is a number");
        let tick: i64 = w[ki + 1].parse().expect("tick is a number");
        if last_t.is_none_or(|lt| t - lt > 1000) {
            out.push(Vec::new());
            base = tick;
        }
        last_t = Some(t);
        out.last_mut().unwrap().push(format!(
            "{} t +{} tick +{}",
            w[..ti].join(" "),
            t - base,
            tick - base
        ));
    }
    out
}

fn server(
    fs: vcod_common::pk3::Pk3Fs,
    proto: &'static Protocol,
    now: &mut Instant,
) -> vcod_server::Server {
    let map = "mp_pavlov";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_threadorder"), *now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_threadorder", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, now, common::PROBE_JOIN_MS);
    sv
}

fn ours(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol) -> Vec<String> {
    let mut now = Instant::now();
    let mut sv = server(fs, proto, &mut now);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "mosin_nagant_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut begun: Option<usize> = None;
    let mut sent = 0;
    for i in 0..1000 {
        now += frame;
        if begun.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE begin")) {
            begun = Some(i);
        }
        if let Some(b) = begun
            && sent < PAIRS.len()
            && i >= b + 20 + 60 * sent
        {
            for cmd in PAIRS[sent] {
                cl.send_reliable(&cmd.replace("{sid}", &cl.server_id().to_string()));
            }
            sent += 1;
        }
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if begun.is_some_and(|b| i >= b + 20 + 60 * PAIRS.len()) {
            break;
        }
    }
    sv.script_log().to_vec()
}

/// Ours against one retail log of the one-client run.
fn check(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol, retail: &str) {
    let retail: Vec<String> = retail.lines().map(str::to_string).collect();
    let ours = ours(fs, proto);
    if std::env::var_os("TO_REPORT").is_some() {
        eprintln!("retail:\n{}\nours:\n{}", retail.join("\n"), ours.join("\n"));
    }
    let r = groups(&retail);
    assert_eq!(r.len(), PAIRS.len(), "retail's groups: {r:#?}");
    assert_eq!(groups(&ours), r, "ours:\n{}", ours.join("\n"));
}

#[test]
fn one_packets_threads_run_in_retails_order() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V1,
        include_str!("fixtures/threadorder/mp_pavlov.txt"),
    );
}

/// Retail 1.5b's run: 1.1d's lines.
#[test]
fn one_packets_threads_run_in_retails_order_on_1_5() {
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
        include_str!("fixtures/threadorder/mp_pavlov-1.5.txt"),
    );
}

/// Retail UO 1.51b's run: 1.1d's lines.
#[test]
fn one_packets_threads_run_in_retails_order_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        &PROTOCOL_V22,
        include_str!("fixtures/threadorder/mp_pavlov-uo.txt"),
    );
}

/// The log with each client's tags (`killed.1`, `w3.0z`) renumbered in
/// the order the clients began: on UO's run the second probe to connect
/// began first, and the level waiters run in start order.
fn by_begin_order(lines: &[String]) -> Vec<String> {
    let begun: Vec<String> = lines
        .iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE begin ").map(str::to_string))
        .collect();
    lines
        .iter()
        .map(|l| {
            let mut w: Vec<String> = l.trim().split(' ').map(str::to_string).collect();
            if let Some(tag) = w.get_mut(1)
                && let Some((name, rest)) = tag.rsplit_once('.')
            {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if let Some(i) = begun.iter().position(|n| *n == digits) {
                    *tag = format!("{name}.{i}{}", &rest[digits.len()..]);
                }
            }
            w.join(" ")
        })
        .collect()
}

/// The first frame both clients' `kill` callbacks ran in, client 1's
/// first, through the frame after it: the threads they started and woke.
fn same_frame_kills(lines: &[String]) -> Vec<String> {
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).collect();
    let t_of = |l: &str| -> i64 {
        let w: Vec<&str> = l.split(' ').collect();
        let i = w.iter().position(|&x| x == "t").expect("a t field");
        w[i + 1].parse().expect("t is a number")
    };
    let start = lines
        .windows(4)
        .position(|w| {
            w[0].starts_with("PROBE killed.1 ")
                && w[3].starts_with("PROBE killed.0 ")
                && t_of(w[0]) == t_of(w[3])
        })
        .expect("both kills in one frame, client 1's first");
    let t = t_of(lines[start]);
    groups(
        &lines[start..]
            .iter()
            .take_while(|l| t_of(l) <= t + 50)
            .map(|l| l.to_string())
            .collect::<Vec<_>>(),
    )
    .remove(0)
}

fn ours_pair(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol) -> Vec<String> {
    let mut now = Instant::now();
    let mut sv = server(fs, proto, &mut now);
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut ca = NetClient::start_with_qport(common::ClientEnd(qa.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let begins = |sv: &vcod_server::Server| {
        sv.script_log()
            .iter()
            .filter(|l| l.contains("PROBE begin"))
            .count()
    };
    // Client 1 connects once client 0 is in, as retail's second probe did.
    for _ in 0..400 {
        now += frame;
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        step(&mut sv, &qa, &mut ca, now);
        if begins(&sv) == 1 {
            break;
        }
    }
    let mut cb = NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2002);
    let mut begun: Option<usize> = None;
    for i in 0..1000 {
        now += frame;
        if begun.is_none() && begins(&sv) == 2 {
            begun = Some(i);
        }
        if begun.is_some_and(|b| i == b + 20) {
            ca.send_reliable("kill");
            cb.send_reliable("kill");
        }
        ca.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        cb.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        // Client 1's packet lands first, as on retail's rounds.
        step_pair_b_first(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        if begun.is_some_and(|b| i >= b + 40) {
            break;
        }
    }
    sv.script_log().to_vec()
}

/// Ours against one retail log of the two-client run.
fn check_pair(fs: vcod_common::pk3::Pk3Fs, proto: &'static Protocol, retail: &str) {
    let retail: Vec<String> = retail.lines().map(str::to_string).collect();
    let ours = ours_pair(fs, proto);
    if std::env::var_os("TO_REPORT").is_some() {
        eprintln!("ours:\n{}", ours.join("\n"));
    }
    assert_eq!(
        same_frame_kills(&by_begin_order(&ours)),
        same_frame_kills(&by_begin_order(&retail)),
        "ours:\n{}",
        ours.join("\n")
    );
}

#[test]
fn two_packets_in_one_frame_run_the_later_ones_threads_first() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_pair(
        fs,
        &PROTOCOL_V1,
        include_str!("fixtures/threadorder/mp_pavlov-pair.txt"),
    );
}

#[test]
fn two_packets_in_one_frame_run_the_later_ones_threads_first_on_1_5() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check_pair(
        fs,
        &PROTOCOL_V6,
        include_str!("fixtures/threadorder/mp_pavlov-pair-1.5.txt"),
    );
}

#[test]
fn two_packets_in_one_frame_run_the_later_ones_threads_first_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_pair(
        fs,
        &PROTOCOL_V22,
        include_str!("fixtures/threadorder/mp_pavlov-pair-uo.txt"),
    );
}
