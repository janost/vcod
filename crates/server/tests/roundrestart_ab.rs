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
//! out, plus the vitals either side of the respawn. Not a command-for-command
//! diff -- the burst carries slots ours pins to constants
//! (`mapchange_ab::CMD_GAPS`) -- but the *set* of configstring slots each
//! half is sent an update for is compared whole: a restart keeps the
//! engine's table, so the incoming level's registrations land where the
//! outgoing level's did and nothing is re-broadcast (map-cycle doc, 4.6).
//! Before that was modelled, the kill after a restart re-allocated the
//! dropped weapon's model, the item registry and the elimination string at
//! fresh slots, and every restart re-sent the move-in aliases.
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

/// Configstring slots whose updates are left out of the slot-set comparison:
/// the restart burst's own (`mapchange_ab::CMD_GAPS` says why 12 and 13
/// are ours to miss; 1 and 3 are compared as the burst).
const CS_BURST_SLOTS: &[usize] = &[1, 3, 12, 13];

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

/// The death that emptied a team, to the restart it ends the round with.
/// `sd.gsc` gets there through a chain of script `wait`s, each of which lands
/// on the next server frame rather than on the millisecond it asked for, and
/// the death itself lands on whichever frame the `kill` was read on: a frame
/// per link is not enough and a second covers the chain. Retail's is 5177 ms.
const ELIMINATION_TOL_MS: i64 = ORDER_TOL_MS + 1000;

/// Fields of the respawn this server still gets wrong across a round restart,
/// each with why. An entry *suppresses* the comparison against retail below
/// and asserts the divergence is still there, so an empty list is full
/// coverage and an entry that starts matching fails. The two this list once
/// held are fixed and gone.
const RESTART_GAPS: &[(&str, &str)] = &[];

/// The fields compared either side of the respawn. `RESTART_GAPS` may only
/// name one of these, or it suppresses a comparison nothing makes.
const RESPAWN_FIELDS: &[&str] = &[
    "health before",
    "health after",
    "weapon before",
    "weapon after",
    "eFlags before",
    "eFlags after",
];

/// The watched playerstate fields of one trace.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Vitals {
    pm_type: i32,
    eflags: i32,
    health: i32,
    weapon: i32,
}

/// A live client's respawn: what it had before the burst and what it came
/// back with.
#[derive(Clone, Copy)]
struct Respawn {
    before: Vitals,
    after: Vitals,
}

/// One `map_restart` as a client saw it.
struct Restart {
    cs3: i64,
    n: i64,
    cs1: i64,
    server_id: i32,
    /// The last trace before the burst and the one in force after it.
    before: Vitals,
    after: Vitals,
    after_ms: i64,
}

fn vitals(k: &NetchanKind) -> Vitals {
    match *k {
        NetchanKind::Trace {
            pm_type,
            eflags,
            health,
            weapon,
        } => Vitals {
            pm_type,
            eflags,
            health,
            weapon,
        },
        _ => unreachable!("not a trace"),
    }
}

/// Compares one field of the respawn against retail, unless [`RESTART_GAPS`]
/// suppresses it -- in which case the divergence itself is what is asserted,
/// so a gap that starts matching fails and has to be deleted.
fn compare(half: &str, what: &str, retail: i32, ours: i32) {
    match RESTART_GAPS.iter().find(|(w, _)| *w == what) {
        Some((_, why)) => assert_ne!(
            retail, ours,
            "{half}: {what} now matches retail; drop it from RESTART_GAPS ({why})"
        ),
        None => assert_eq!(
            retail, ours,
            "{half}: {what} is {retail} on retail and {ours} here"
        ),
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
            before: vitals(&before.kind),
            after: vitals(&after.kind),
        });
    }
    assert!(!out.is_empty(), "{who}: no round restarted in the run");
    out
}

/// The connect-time gamestate's serverId, and a check that nothing sent
/// another one from the first restart onwards: a restart pushes none
/// (map-cycle doc 3.1).
/// Every slot a half was sent a `d <slot>` for over the run, as a sorted set
/// minus [`CS_BURST_SLOTS`]. A set rather than a sequence: what it proves is
/// which slots moved at all, and retail's run has one more elimination round
/// (a second `d 6`) than the 120 s replayed here.
fn updated_slots(events: &[NetchanEvent]) -> Vec<usize> {
    let mut slots: Vec<usize> = events
        .iter()
        .filter_map(|e| e.cmd())
        .filter_map(|c| c.strip_prefix("d "))
        .filter_map(|rest| rest.split(' ').next()?.parse().ok())
        .filter(|s| !CS_BURST_SLOTS.contains(s))
        .collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

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

/// From the death that emptied a team to the `n` of the restart it ends the
/// round with. The death is the first trace reading `pm_type` 6 with no health
/// left; retail's target dies at 23430 ms and the round restarts at 28607.
fn elimination_wait(who: &str, events: &[NetchanEvent]) -> i64 {
    let died = events
        .iter()
        .find(|e| {
            e.trace()
                .map(vitals)
                .is_some_and(|v| v.pm_type == 6 && v.health == 0)
        })
        .unwrap_or_else(|| panic!("{who}: this half never died, so no round ended on it"));
    let restart = events
        .iter()
        .find(|e| e.ms > died.ms && e.cmd() == Some("n"))
        .unwrap_or_else(|| panic!("{who}: no restart after the death at {} ms", died.ms));
    restart.ms - died.ms
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
    // Retail ran a while before either probe joined (the target's gamestate
    // reads serverTime 24950), so the first round's own `sayMoveIn` had
    // played to nobody before the match-start restart; a join on the load
    // frame would put that thread's alias allocation on the restart's own
    // frame instead, where the slot-set comparison below cannot see it.
    for _ in 0..200 {
        now += Duration::from_millis(50);
        sv.tick(now);
    }

    let qa = Rc::new(RefCell::new(common::Queues::default()));
    let qb = Rc::new(RefCell::new(common::Queues::default()));
    let (mut ca, mut cb, mut ja, mut jb) = common::join_pair_logged(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
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
        // `kill` ahead of `score` in the frame both fall due, as the probe
        // orders them: a bare `score` opens the flood window
        // (`tests/flood_protect.rs`), and a `kill` behind it in the same
        // message is dropped.
        if !killed && ms >= KILL_AT_MS {
            cb.send_reliable("kill");
            killed = true;
        }
        if ms >= next_score {
            cb.send_reliable("score");
            next_score += SCORE_PERIOD_MS;
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

        // --- the slots updated over the run, as a set ---
        assert_eq!(
            updated_slots(ours),
            updated_slots(retail),
            "{half}: the configstring slots updated over the run differ from retail's"
        );

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
        let live = |who: &str, rs: &[Restart]| -> Respawn {
            let x = rs
                .iter()
                .find(|x| x.before.pm_type == 0)
                .unwrap_or_else(|| panic!("{who}: no restart caught this client alive"));
            Respawn {
                before: x.before,
                after: x.after,
            }
        };
        let (rl, ol) = (live(&r_who, &r), live(&o_who, &o));
        for (what, retail_v, ours_v) in [
            ("health before", rl.before.health, ol.before.health),
            ("health after", rl.after.health, ol.after.health),
            ("weapon before", rl.before.weapon, ol.before.weapon),
            ("weapon after", rl.after.weapon, ol.after.weapon),
            ("eFlags before", rl.before.eflags, ol.before.eflags),
            ("eFlags after", rl.after.eflags, ol.after.eflags),
        ] {
            assert!(
                RESPAWN_FIELDS.contains(&what),
                "{what:?} is compared but not named in RESPAWN_FIELDS"
            );
            compare(half, what, retail_v, ours_v);
        }

        // --- the round a death ended, from that death to the restart ---
        if half == "target" {
            let (rw, ow) = (
                elimination_wait(&r_who, retail),
                elimination_wait(&o_who, ours),
            );
            assert!(
                (rw - ow).abs() <= ELIMINATION_TOL_MS,
                "{half}: the death that ended the round is {rw} ms before the restart on \
                 retail and {ow} ms here, over the {ELIMINATION_TOL_MS} ms tolerance"
            );
        }
    }

    // A gap naming a field nothing compares suppresses nothing.
    for (what, why) in RESTART_GAPS {
        assert!(
            RESPAWN_FIELDS.contains(what),
            "RESTART_GAPS names {what:?} ({why}), which no comparison reads"
        );
    }
}
