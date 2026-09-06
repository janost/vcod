//! A `dm` map end and the rotation that follows, against the retail capture
//! `tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`.
//!
//! The recipe is the fixture header's: `scr_dm_timelimit 1` and
//! `sv_mapRotation "gametype dm map mp_carentan map mp_brecourt"`, one client
//! joined allies, asking for the scoreboard every 2 s because retail pushes
//! no `b` unasked. The rotation's first token names the map already serving,
//! so the run crosses two level ends: a `map_restart` and then a real map
//! change (docs/research/cod11-map-cycle.md, sections 3 and 4).
//!
//! What is compared is an ordered subsequence of markers -- the intermission
//! frame, the restart's three reliable commands, the respawn, the second
//! intermission, the out-of-band notice, the gamestate and the respawn on the
//! second map -- and the interval between named pairs of them, plus the
//! presence of each command in [`BURST_CMDS`] and [`INTERMISSION_CMDS`] on
//! both sides. Not a command-for-command diff: the ordering of the burst's
//! commands inside one frame is not a wire fact either side pins. [`CMD_GAPS`]
//! suppresses the three ours does not send at all.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{NetchanEvent, NetchanKind};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;

const MAP: &str = "mp_carentan";
const NEXT: &str = "mp_brecourt";
const FIXTURE: &str = "tests/fixtures/netchan/mp_carentan-dm-mapchange.txt";

/// A server command or a snapshot a frame early or late is the tick phase,
/// not a divergence: a server frame is 50 ms and the probe's own pump adds
/// its own fraction of one on retail's side.
const ORDER_TOL_MS: i64 = 100;

/// The `wait 10` in `dm.gsc`'s `endMap`, which both sides count off the same
/// script clock. Five frames of slack for the frame each side happens to land
/// on plus the probe's pump: retail's two are 10133 and 10017 ms.
const WAIT_TOL_MS: i64 = 250;

/// A respawn waits on the client answering the weapon menu, which is a client
/// round trip and not a server property, so those two rows are bounded rather
/// than compared: ten frames, well inside which both sides land.
const MENU_TOL_MS: i64 = 500;

/// How often the probe asks for the scoreboard, and this replay with it.
const SCORE_PERIOD_MS: i64 = 2000;

/// Every reliable command retail's restart burst carries, as the first word or
/// words that name it. Each is asserted present within [`ORDER_TOL_MS`] of the
/// `n` on retail *and* here, unless [`CMD_GAPS`] suppresses it.
const BURST_CMDS: &[&str] = &[
    "d 13",
    "d 12",
    "d 3",
    "n",
    "d 1",
    "f",
    "v scr_showweapontab",
    "v g_scriptMainMenu",
    "t",
    "v cg_objectiveText",
];

/// The same for the intermission's own frame.
const INTERMISSION_CMDS: &[&str] = &["f", "u", "v g_scriptMainMenu", "v cg_objectiveText"];

/// Commands of the two tables above ours does not send, each with why. An
/// entry *suppresses* the presence comparison and asserts the divergence is
/// still there -- retail sends it in that window and we do not -- so an empty
/// list is full coverage and an entry that starts matching fails.
const CMD_GAPS: &[(&str, &str)] = &[
    (
        "d 13",
        "configstring 13 is `level.startTime` and vcod pins it to \"0\", so a \
         restart moves nothing to rebroadcast (map-cycle doc 4.5)",
    ),
    (
        "d 12",
        "configstring 3's `t` is pinned by `ambient_play`, and 12 rides the \
         same burst on retail because `sv.restarting` broadcasts every write \
         (map-cycle doc 4.5)",
    ),
];

/// A command names a pattern when it is that word or begins with it followed
/// by a space, so `d 1` does not swallow `d 12`.
fn names(cmd: &str, pat: &str) -> bool {
    cmd == pat || (cmd.starts_with(pat) && cmd.as_bytes().get(pat.len()) == Some(&b' '))
}

type Match = fn(&NetchanEvent) -> bool;

/// The markers, in the order both sides have to show them. Each is the first
/// event at or after the one before it that matches.
const MARKERS: &[(&str, Match)] = &[
    ("intermission", |e| e.pm_type() == Some(5)),
    ("restart d 3", |e| {
        e.cmd().is_some_and(|c| c.starts_with("d 3 "))
    }),
    ("restart n", |e| e.cmd() == Some("n")),
    ("restart d 1", |e| {
        e.cmd().is_some_and(|c| c.starts_with("d 1 "))
    }),
    ("restart spectator", |e| e.pm_type() == Some(4)),
    ("restart playing", |e| e.pm_type() == Some(0)),
    ("second intermission", |e| e.pm_type() == Some(5)),
    (
        "loadingnewmap",
        |e| matches!(&e.kind, NetchanKind::Oob(t) if t.starts_with("loadingnewmap")),
    ),
    ("gamestate", |e| {
        matches!(e.kind, NetchanKind::Gamestate { .. })
    }),
    ("map spectator", |e| e.pm_type() == Some(4)),
    ("map playing", |e| e.pm_type() == Some(0)),
];

/// One side's markers: the name, the `ms` it landed on and the event itself.
struct Marks<'a> {
    who: &'static str,
    found: Vec<(&'static str, i64, &'a NetchanEvent)>,
}

impl<'a> Marks<'a> {
    fn scan(who: &'static str, events: &'a [NetchanEvent]) -> Marks<'a> {
        let mut found = Vec::new();
        let mut at = 0usize;
        for (name, is) in MARKERS {
            match events[at..].iter().position(is) {
                Some(off) => {
                    at += off + 1;
                    found.push((*name, events[at - 1].ms, &events[at - 1]));
                }
                None => panic!(
                    "{who}: no {name:?} after {:?}; the markers found were {:?}",
                    found.last().map(|(n, ms, _)| (*n, *ms)),
                    found.iter().map(|(n, ms, _)| (*n, *ms)).collect::<Vec<_>>()
                ),
            }
        }
        Marks { who, found }
    }

    fn ms(&self, name: &str) -> i64 {
        self.found
            .iter()
            .find(|(n, _, _)| *n == name)
            .unwrap_or_else(|| panic!("{}: no marker {name:?}", self.who))
            .1
    }

    fn event(&self, name: &str) -> &NetchanEvent {
        self.found.iter().find(|(n, _, _)| *n == name).unwrap().2
    }

    fn gap(&self, from: &str, to: &str) -> i64 {
        self.ms(to) - self.ms(from)
    }
}

/// Every command within `window` ms either side of `at`, which is how a
/// marker's own frame is read: the fixture sorts commands ahead of the trace
/// that shares their `ms`, so a command raised by the same script frame as the
/// intermission sits *before* it in the stream.
fn cmds_near(events: &[NetchanEvent], at: i64, window: i64) -> Vec<&str> {
    events
        .iter()
        .filter(|e| (e.ms - at).abs() <= window)
        .filter_map(|e| e.cmd())
        .collect()
}

#[test]
fn a_dm_map_end_and_rotation_match_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let retail = common::collapse_traces(&common::parse_netchan_fixture(&text));

    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "dm"), now);
    // The fixture header's two cvars, verbatim.
    sv.set_cvar("scr_dm_timelimit", "1");
    sv.set_cvar(
        "sv_mapRotation",
        &format!("gametype dm map {MAP} map {NEXT}"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut cl, mut join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");
    let id_before = i32::from(sv.server_id());

    let start = now;
    let mut ours: Vec<NetchanEvent> = Vec::new();
    let mut next_score = 0i64;
    // Two rounds of a one-minute time limit, each with its ten-second
    // intermission, plus the join: 160 s of simulated time at 50 ms a frame.
    for _ in 0..3200 {
        now += Duration::from_millis(50);
        let ms = now.duration_since(start).as_millis() as i64;
        if ms >= next_score {
            cl.send_reliable("score");
            next_score += SCORE_PERIOD_MS;
        }
        let cmd = common::holding(&cl);
        cl.send_frame(&cmd);
        let events = common::step(&mut sv, &q, &mut cl, now);
        for e in &events {
            match e {
                // A new level reopens the stock menus under the indices the
                // last one used, on a gamestate and on a restart alike.
                NetEvent::GamestateReady => join.reset_menus(),
                NetEvent::ServerCommand(t) => {
                    if t.first().map(String::as_str) == Some("n") {
                        join.reset_menus();
                    }
                    join.on_server_command(t, &mut cl, now);
                }
                NetEvent::Dropped(r) => panic!("dropped at {ms} ms: {r}"),
                _ => {}
            }
        }
        common::record_netchan(&mut cl, &events, ms, &mut ours);
        // The last marker: the respawn on the second map.
        if ours
            .iter()
            .any(|e| matches!(e.kind, NetchanKind::Gamestate { .. }))
            && ours.last().and_then(NetchanEvent::pm_type) == Some(0)
        {
            break;
        }
    }
    let ours = common::collapse_traces(&ours);

    let r = Marks::scan("retail", &retail);
    let o = Marks::scan("ours", &ours);

    // --- the two script waits, the one interval both sides really count ---
    for (from, to) in [
        ("intermission", "restart d 3"),
        ("second intermission", "loadingnewmap"),
    ] {
        let (rg, og) = (r.gap(from, to), o.gap(from, to));
        assert!(
            (rg - og).abs() <= WAIT_TOL_MS,
            "{from} -> {to}: retail {rg} ms, ours {og} ms, over the {WAIT_TOL_MS} ms \
             `wait 10` tolerance"
        );
    }

    // --- the restart burst: `d 3`, `n`, `d 1` in one frame, on both sides ---
    for m in [&r, &o] {
        for (from, to) in [("restart d 3", "restart n"), ("restart n", "restart d 1")] {
            assert!(
                (0..=ORDER_TOL_MS).contains(&m.gap(from, to)),
                "{}: {from} -> {to} is {} ms, not the same frame",
                m.who,
                m.gap(from, to)
            );
        }
        assert!(
            (0..=ORDER_TOL_MS).contains(&m.gap("restart d 1", "restart spectator")),
            "{}: the restart's first frame is {} ms after `d 1`",
            m.who,
            m.gap("restart d 1", "restart spectator")
        );
        assert!(
            (0..=ORDER_TOL_MS).contains(&m.gap("gamestate", "map spectator")),
            "{}: the first frame of the second map is {} ms after the gamestate",
            m.who,
            m.gap("gamestate", "map spectator")
        );
        // Bounded rather than compared: the respawn waits on the client's own
        // answer to the weapon menu.
        for (from, to) in [
            ("restart spectator", "restart playing"),
            ("map spectator", "map playing"),
        ] {
            assert!(
                (0..=MENU_TOL_MS).contains(&m.gap(from, to)),
                "{}: {from} -> {to} is {} ms, past the {MENU_TOL_MS} ms menu round trip",
                m.who,
                m.gap(from, to)
            );
        }
    }

    // --- `EF_TELEPORT_BIT`, spawn for spawn ---
    // Every spawn XORs the bit and a level boundary clears it with the rest
    // of the playerstate, so the six traces above alternate 24, 16 in a
    // pattern retail's capture pins exactly (map-cycle doc, 8.2).
    for (name, _) in MARKERS.iter().filter(|(n, _)| {
        matches!(
            *n,
            "intermission"
                | "restart spectator"
                | "restart playing"
                | "second intermission"
                | "map spectator"
                | "map playing"
        )
    }) {
        let (re, oe) = (r.event(name).eflags(), o.event(name).eflags());
        assert_eq!(re, oe, "{name}: eFlags, retail {re:?} against ours {oe:?}");
    }

    // The one interval that is a recorded divergence rather than a match:
    // retail sleeps 250 ms in `SV_SpawnServer` and its client fetches the
    // gamestate with its next message (1493 ms here); ours neither sleeps nor
    // waits on more than the settle frames. Bounded so ours cannot silently
    // become the slower of the two.
    let (r_fetch, o_fetch) = (
        r.gap("loadingnewmap", "gamestate"),
        o.gap("loadingnewmap", "gamestate"),
    );
    assert!(
        (0..=ORDER_TOL_MS).contains(&o_fetch),
        "the gamestate is {o_fetch} ms after the notice here, past the one frame it \
         should take. Retail's is {r_fetch} ms because it sleeps 250 ms inside \
         `SV_SpawnServer` and then waits for the client's next message; ours does \
         neither, so the client's very next message brings it."
    );

    // --- the serverId nibbles, the same transition on both sides ---
    let sid = |m: &Marks| {
        common::serverid_of(m.event("restart d 1").cmd().unwrap())
            .expect("the restart's `d 1` carries sv_serverid")
    };
    let gs_id = |m: &Marks| match m.event("gamestate").kind {
        NetchanKind::Gamestate { server_id, .. } => server_id,
        _ => unreachable!("the marker matched a gamestate"),
    };
    let retail_start = match retail.iter().find_map(|e| match &e.kind {
        NetchanKind::Gamestate { server_id, .. } => Some(*server_id),
        _ => None,
    }) {
        Some(id) => id,
        None => panic!("the fixture has no connect-time gamestate"),
    };
    for (who, before, restart, map) in [
        ("retail", retail_start, sid(&r), gs_id(&r)),
        ("ours", id_before, sid(&o), gs_id(&o)),
    ] {
        assert_eq!(
            restart,
            i32::from(vcod_server::console::next_restart_id(before as u8)),
            "{who}: the restart's serverId is not the low nibble of {before} up one"
        );
        assert_eq!(
            map,
            i32::from(vcod_server::console::next_map_id(restart as u8)),
            "{who}: the map change's serverId is not the high nibble of {restart} up one"
        );
    }

    // --- the second gamestate really is the second map ---
    for (who, m) in [("retail", &r), ("ours", &o)] {
        let NetchanKind::Gamestate { mapname, .. } = &m.event("gamestate").kind else {
            unreachable!("the marker matched a gamestate")
        };
        assert_eq!(mapname, NEXT, "{who}: the rotation loaded {mapname}");
    }
    assert!(
        cl.configstring(0).contains(&format!("mapname\\{NEXT}")),
        "configstring 0 still names another map: {:?}",
        cl.configstring(0)
    );

    // --- the restart pushes no gamestate (map-cycle doc 3.1) ---
    for (who, m, events) in [("retail", &r, &retail), ("ours", &o, &ours)] {
        let (from, to) = (m.ms("intermission"), m.ms("loadingnewmap"));
        let between = events
            .iter()
            .filter(|e| e.ms > from && e.ms < to)
            .filter(|e| matches!(e.kind, NetchanKind::Gamestate { .. }))
            .count();
        assert_eq!(between, 0, "{who}: the restart pushed a gamestate");
    }

    // --- the scoreboard is an answer, so it arrives inside one `score` period ---
    for (who, m, events) in [("retail", &r, &retail), ("ours", &o, &ours)] {
        let at = m.ms("intermission");
        let window = SCORE_PERIOD_MS + 2 * ORDER_TOL_MS;
        let answered = events
            .iter()
            .filter(|e| e.ms >= at && e.ms <= at + window)
            .any(|e| e.cmd().is_some_and(|c| c.starts_with("b ")));
        assert!(
            answered,
            "{who}: no scoreboard within {window} ms of the intermission at {at} ms"
        );
    }

    // --- the two command tables, and the gaps that suppress an entry ---
    for (marker, table) in [
        ("restart n", BURST_CMDS),
        ("intermission", INTERMISSION_CMDS),
    ] {
        let (r_near, o_near) = (
            cmds_near(&retail, r.ms(marker), ORDER_TOL_MS),
            cmds_near(&ours, o.ms(marker), ORDER_TOL_MS),
        );
        for pat in table {
            let (in_retail, in_ours) = (
                r_near.iter().any(|c| names(c, pat)),
                o_near.iter().any(|c| names(c, pat)),
            );
            // The table is read off the fixture, so retail always has it; a
            // pattern retail stopped sending is a table entry to delete.
            assert!(
                in_retail,
                "retail sends no {pat:?} within {ORDER_TOL_MS} ms of {marker:?}; drop it \
                 from the table: {r_near:?}"
            );
            match CMD_GAPS.iter().find(|(g, _)| g == pat) {
                Some((_, why)) => assert!(
                    !in_ours,
                    "{pat:?} at {marker:?} now matches retail; drop it from CMD_GAPS ({why})"
                ),
                None => assert!(
                    in_ours,
                    "no {pat:?} within {ORDER_TOL_MS} ms of {marker:?} here: {o_near:?}"
                ),
            }
        }
    }
    // A gap naming a command neither table carries suppresses nothing.
    for (pat, why) in CMD_GAPS {
        assert!(
            BURST_CMDS.contains(pat) || INTERMISSION_CMDS.contains(pat),
            "CMD_GAPS names {pat:?} ({why}), which neither command table reads"
        );
    }
}
