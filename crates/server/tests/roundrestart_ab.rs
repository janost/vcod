//! An S&D round restart on a pair of clients, against the two retail captures
//! `tests/fixtures/netchan/mp_carentan-sd-roundrestart-shooter.txt` and
//! `-target.txt`.
//!
//! The recipe is the fixture headers': `scr_sd_roundlength 1` and
//! `scr_friendlyfire 1`, one probe per team, the axis half killing itself 20 s
//! in so the first round ends by elimination and the second one runs the timer
//! out. A round end is a `map_restart` (docs/research/cod11-map-cycle.md,
//! section 4), so what goes on the wire is `d 3`, `n`, `d 1` and no gamestate
//! at all, with `sv_serverid`'s low nibble up one each time.
//!
//! Compared per half: every restart's three reliable commands and their order,
//! the serverId the `d 1` carries, the absence of a gamestate, the respawn on
//! the restart's own frame, and the length of the round that ran its timer
//! out. Not a command-for-command diff -- the burst carries slots ours pins to
//! constants (`mapchange_ab::CMD_GAPS`) -- and [`RESTART_GAPS`] lists what a
//! respawn here still gets wrong.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{NetchanEvent, NetchanKind};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;

const MAP: &str = "mp_carentan";
const SHOOTER: &str = "tests/fixtures/netchan/mp_carentan-sd-roundrestart-shooter.txt";
const TARGET: &str = "tests/fixtures/netchan/mp_carentan-sd-roundrestart-target.txt";

/// The probe's `ROUNDRESTART_KILL_AT`: the axis half kills itself 20 s in, so
/// the first round ends by elimination and the timer still has room to end the
/// second one inside the run.
const KILL_AT_MS: i64 = 20_000;

/// How often the target half asks for the scoreboard. The shooter half asks
/// for none, and its fixture carries no `b` line, which is the fixture
/// header's own point about retail never pushing one.
const SCORE_PERIOD_MS: i64 = 2000;

/// A reliable command or a snapshot a frame early or late is the tick phase:
/// a server frame is 50 ms and the probe's own pump adds part of one.
const ORDER_TOL_MS: i64 = 100;

/// The round timer, which both sides count off the same `scr_sd_roundlength`
/// minute of script clock. Five frames of slack: retail's timer round is
/// 64619 ms on one half and 64624 on the other.
const TIMER_TOL_MS: i64 = 250;

/// A round shorter than this ended by elimination, not by the clock. The
/// limit is one minute, so anything past it is the timer's round plus the
/// end-of-round wait.
const TIMER_ROUND_FLOOR_MS: i64 = 60_000;

/// What a live client's respawn still gets wrong across a round restart, each
/// with the retail line that says so. Empty is the goal, and the guard below
/// fails on any entry that starts matching, so this list cannot rot into a
/// lie.
const RESTART_GAPS: &[(&str, &str)] = &[
    (
        "weapon",
        "retail carries the same weapon index either side of the restart \
         (shooter 12, target 9); a client that was alive comes back holding \
         nothing here, while one that was dead comes back armed",
    ),
    (
        "eFlags",
        "retail flips the per-life teleport bit 0x8 on the respawn (16 -> 24), \
         which is what stops a client interpolating from the old position; \
         ours leaves it where it was",
    ),
];

/// One `map_restart` as a client saw it.
struct Restart {
    cs3: i64,
    n: i64,
    cs1: i64,
    server_id: i32,
    /// The last trace before the burst and the first playing one after it.
    before: NetchanKind,
    after: NetchanKind,
    after_ms: i64,
}

fn trace_fields(k: &NetchanKind) -> (i32, i32, i32, i32) {
    match *k {
        NetchanKind::Trace {
            pm_type,
            eflags,
            health,
            weapon,
        } => (pm_type, eflags, health, weapon),
        _ => unreachable!("not a trace"),
    }
}

/// Every restart in one client's stream, in order. Panics naming `who` when a
/// `n` is not flanked by its `d 3` and `d 1` or is not followed by a respawn:
/// that is the ordered subsequence the gate is about.
fn restarts(who: &str, events: &[NetchanEvent]) -> Vec<Restart> {
    let mut out = Vec::new();
    for (i, e) in events.iter().enumerate() {
        if e.cmd() != Some("n") {
            continue;
        }
        let cs3 = events[..i]
            .iter()
            .rev()
            .find(|p| p.cmd().is_some_and(|c| c.starts_with("d 3 ")))
            .unwrap_or_else(|| panic!("{who}: the `n` at {} ms has no `d 3` before it", e.ms));
        let cs1 = events[i + 1..]
            .iter()
            .find(|p| p.cmd().is_some_and(|c| c.starts_with("d 1 ")))
            .unwrap_or_else(|| panic!("{who}: the `n` at {} ms has no `d 1` after it", e.ms));
        let before = events[..i]
            .iter()
            .rev()
            .find(|p| p.ms < cs3.ms && p.trace().is_some())
            .unwrap_or_else(|| panic!("{who}: no trace before the `n` at {} ms", e.ms))
            .clone();
        // The state in force once the burst is through, which is the last
        // trace on or before the restart's frame: a client whose watched
        // fields the restart did not move raises no new one.
        let after = events
            .iter()
            .rev()
            .find(|p| p.ms <= e.ms + ORDER_TOL_MS && p.trace().is_some())
            .expect("`before` is one such trace")
            .clone();
        assert_eq!(
            after.pm_type(),
            Some(0),
            "{who}: the client is not playing across the restart at {} ms",
            e.ms
        );
        out.push(Restart {
            cs3: cs3.ms,
            n: e.ms,
            cs1: cs1.ms,
            server_id: common::serverid_of(cs1.cmd().unwrap()).unwrap_or_else(|| {
                panic!("{who}: the `d 1` at {} ms carries no sv_serverid", cs1.ms)
            }),
            after_ms: after.ms,
            before: before.kind,
            after: after.kind,
        });
    }
    assert!(!out.is_empty(), "{who}: no round restarted in the run");
    out
}

/// The connect-time gamestate's serverId, and a check that nothing sent
/// another one from the first restart onwards: a restart pushes none
/// (map-cycle doc 3.1).
fn one_gamestate(who: &str, events: &[NetchanEvent], from_ms: i64) {
    let late = events
        .iter()
        .filter(|e| e.ms >= from_ms)
        .filter(|e| matches!(e.kind, NetchanKind::Gamestate { .. }))
        .count();
    assert_eq!(late, 0, "{who}: a round restart pushed a gamestate");
}

/// The first round that ran its timer out, as the gap between two restarts.
fn timer_round(who: &str, rs: &[Restart]) -> i64 {
    rs.windows(2)
        .map(|w| w[1].n - w[0].n)
        .find(|g| *g >= TIMER_ROUND_FLOOR_MS)
        .unwrap_or_else(|| {
            panic!(
                "{who}: no round ran the {TIMER_ROUND_FLOOR_MS} ms timer out; the gaps were {:?}",
                rs.windows(2).map(|w| w[1].n - w[0].n).collect::<Vec<_>>()
            )
        })
}

#[test]
fn an_sd_round_restart_matches_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let read = |path: &str| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        common::collapse_traces(&common::parse_netchan_fixture(&text))
    };
    let retail = [read(SHOOTER), read(TARGET)];

    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "sd"), now);
    // The fixture headers' two cvars, verbatim.
    sv.set_cvar("scr_sd_roundlength", "1");
    sv.set_cvar("scr_friendlyfire", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let qa = Rc::new(RefCell::new(common::Queues::default()));
    let qb = Rc::new(RefCell::new(common::Queues::default()));
    // One frame of delay on every menu answer: `sd`'s `Callback_PlayerConnect`
    // reaches its `menuresponse` loop only after `spawnSpectator` ->
    // `updateTeamStatus`, whose first statement is `wait 0`, and this harness
    // has no network delay of its own to cover that frame with.
    let (mut ca, mut cb, mut ja, mut jb) = common::join_pair_delayed(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
        1,
    );
    let id_before = sv.server_id();

    let start = now;
    let mut ours: [Vec<NetchanEvent>; 2] = Default::default();
    let mut killed = false;
    let mut next_score = 0i64;
    // The round start, the elimination round and one full timer round: 120 s
    // of simulated time at 50 ms a frame.
    for _ in 0..2400 {
        now += Duration::from_millis(50);
        let ms = now.duration_since(start).as_millis() as i64;
        if ms >= next_score {
            cb.send_reliable("score");
            next_score += SCORE_PERIOD_MS;
        }
        if !killed && ms >= KILL_AT_MS {
            cb.send_reliable("kill");
            killed = true;
        }
        let (cmd_a, cmd_b) = (common::holding(&ca), common::holding(&cb));
        ca.send_frame(&cmd_a);
        cb.send_frame(&cmd_b);
        let (ea, eb) = common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        for (i, (events, join, cl)) in [(ea, &mut ja, &mut ca), (eb, &mut jb, &mut cb)]
            .into_iter()
            .enumerate()
        {
            for e in &events {
                match e {
                    NetEvent::GamestateReady => join.reset_menus(),
                    NetEvent::ServerCommand(t) => {
                        // A restart reopens whatever menus the level's own
                        // connect callback opens, under the indices the last
                        // one used.
                        if t.first().map(String::as_str) == Some("n") {
                            join.reset_menus();
                        }
                        join.on_server_command(t, cl, now);
                    }
                    NetEvent::Dropped(r) => panic!("client {i} dropped at {ms} ms: {r}"),
                    _ => {}
                }
            }
            common::record_netchan(cl, &events, ms, &mut ours[i]);
            join.tick_answers(cl, now);
        }
    }
    let ours = [
        common::collapse_traces(&ours[0]),
        common::collapse_traces(&ours[1]),
    ];

    for (half, retail, ours) in [
        ("shooter", &retail[0], &ours[0]),
        ("target", &retail[1], &ours[1]),
    ] {
        let (r_who, o_who) = (format!("retail {half}"), format!("ours {half}"));
        let r = restarts(&r_who, retail);
        let o = restarts(&o_who, ours);

        // --- the burst: `d 3`, `n`, `d 1`, in that order, in one frame ---
        for (who, rs) in [(&r_who, &r), (&o_who, &o)] {
            for x in rs.iter() {
                assert!(
                    (0..=ORDER_TOL_MS).contains(&(x.n - x.cs3)),
                    "{who}: `d 3` is {} ms before the `n` at {} ms",
                    x.n - x.cs3,
                    x.n
                );
                assert!(
                    (0..=ORDER_TOL_MS).contains(&(x.cs1 - x.n)),
                    "{who}: `d 1` is {} ms after the `n` at {} ms",
                    x.cs1 - x.n,
                    x.n
                );
                // A respawn the restart's fields did move lands on the
                // restart's own frame; one it did not raises no trace and
                // there is nothing to time.
                if x.after_ms >= x.cs3 {
                    assert!(
                        (0..=ORDER_TOL_MS).contains(&(x.after_ms - x.cs1)),
                        "{who}: the respawn is {} ms after the `d 1` at {} ms",
                        x.after_ms - x.cs1,
                        x.cs1
                    );
                }
            }
        }

        // --- the serverId's low nibble, one per restart, from the connect id ---
        for (who, rs, first) in [
            (
                &r_who,
                &r,
                retail
                    .iter()
                    .find_map(|e| match &e.kind {
                        NetchanKind::Gamestate { server_id, .. } => Some(*server_id as u8),
                        _ => None,
                    })
                    .expect("the fixture has a connect-time gamestate"),
            ),
            (&o_who, &o, id_before),
        ] {
            let mut id = first;
            for x in rs.iter() {
                id = vcod_server::console::next_restart_id(id);
                assert_eq!(
                    x.server_id,
                    i32::from(id),
                    "{who}: the restart at {} ms carries sv_serverid {}, not {id}",
                    x.n,
                    x.server_id
                );
            }
        }

        // --- no gamestate from the first restart onwards (doc 3.1) ---
        one_gamestate(&r_who, retail, r[0].cs3);
        one_gamestate(&o_who, ours, o[0].cs3);

        // --- the round that ran its timer out, the one interval both count ---
        let (rg, og) = (timer_round(&r_who, &r), timer_round(&o_who, &o));
        assert!(
            (rg - og).abs() <= TIMER_TOL_MS,
            "{half}: the timer round is {rg} ms on retail and {og} ms here, over the \
             {TIMER_TOL_MS} ms tolerance"
        );

        // --- the respawn itself, on the restart of a client that was alive ---
        let live = |who: &str, rs: &[Restart]| -> (i32, i32, i32, i32, i32, i32, i32, i32) {
            let x = rs
                .iter()
                .find(|x| trace_fields(&x.before).0 == 0)
                .unwrap_or_else(|| panic!("{who}: no restart caught this client alive"));
            let (b, a) = (trace_fields(&x.before), trace_fields(&x.after));
            (b.0, b.1, b.2, b.3, a.0, a.1, a.2, a.3)
        };
        let rl = live(&r_who, &r);
        let ol = live(&o_who, &o);
        assert_eq!(
            (rl.2, rl.6),
            (ol.2, ol.6),
            "{half}: health either side of the restart is {:?} on retail and {:?} here",
            (rl.2, rl.6),
            (ol.2, ol.6)
        );
        assert_eq!(
            rl.3, ol.3,
            "{half}: the weapon held before the restart is {} on retail and {} here",
            rl.3, ol.3
        );

        // --- the known gaps, and the guard that deletes them ---
        for (what, why) in RESTART_GAPS {
            match *what {
                "weapon" => {
                    assert_eq!(
                        rl.7, rl.3,
                        "RESTART_GAPS: retail dropped the weapon too ({why})"
                    );
                    assert_eq!(
                        ol.7, 0,
                        "{half}: the weapon now survives the restart; drop it from \
                         RESTART_GAPS ({why})"
                    );
                }
                "eFlags" => {
                    assert_ne!(
                        rl.5, rl.1,
                        "RESTART_GAPS: retail left the teleport bit alone too ({why})"
                    );
                    assert_eq!(
                        ol.5, ol.1,
                        "{half}: the teleport bit now flips on a restart; drop it from \
                         RESTART_GAPS ({why})"
                    );
                }
                other => panic!("RESTART_GAPS has no guard for {other:?}"),
            }
        }
    }
}
