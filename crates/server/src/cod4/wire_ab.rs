//! The join against retail: `play-1.7-walk.txt` and `play-1.7-spec.txt`
//! (`crates/cod4/tests/fixtures/net/`, cod4-netfields.md 8) are two of three
//! `cod4_probe` clients on one retail `cod4_lnxded` 1.7 `war` level on
//! mp_crash (`+set sv_pure 0`): `mate` (slot 0, `--team allies --class
//! assault_mp,0 --script idle`, not committed), `spec` (slot 1, no team,
//! which answers the team menu `spectator`) and `walker` (slot 2, allies
//! and `assault_mp,0` a second after `mate`). The same three join
//! [`Cod4Server`] in process on retail's level clock, each reading its first
//! snapshot when its retail twin did, answering its menus when its twin did
//! (the mate's times are its probe's rule: the team a second past its first
//! snapshot, the class 1.5 s after) and spawning on its twin's spot; the
//! walker says its line when its twin did. The gamestate's configstrings
//! and baselines, the server commands each client read up to [`UNTIL_MS`],
//! the chat lines each read, and the walker's spawn frame (playerstate and
//! entities) are diffed; what still differs is [`GAPS`], each with the
//! reason. The level's scripts run without an error. Every message of ours
//! decodes and re-encodes ([`super::join_tests::reencodes`]). Needs
//! `COD4_DIR`.
//!
//! [`the_walk_replays_on_ours_as_on_retail`] runs the same join on to the
//! capture's end with the walker sending the capture's own cmds from its
//! spawn, and compares every snapshot's pmove words, the teammate's origin and yaw,
//! the spectator's own playerstate and the walker's body as the spectator
//! is sent it (docs/research/cod4-pmove.md 15.1).

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use vcod_cod4::net::capture::Line;
use vcod_cod4::net::parse::Snapshot;

use super::test_harness::*;
use super::*;

/// The server time the command comparison stops at: the walker's spawn
/// (47900) and the next four seconds, before its script starts walking.
const UNTIL_MS: i32 = 52_000;

/// The run goes on past the walker's `say` (sent 22.6 s after its first
/// snapshot), whose chat lines are compared alone, to the capture's last
/// snapshot (87300).
const RUN_MS: i32 = 87_350;

/// The walk run's `srand` seed (`G_InitGame`, 0x80b56ee): the wall
/// clock's milliseconds as the server's log file was created, found by
/// `recover_game_seed` (cod4-combat.md 10.9).
pub(super) const WALK_SEED: u32 = 1_803_421_502;

/// Each tolerated difference as a pattern over the [`diffs`] line's words
/// (`*` any word, else a prefix of it), and why.
const GAPS: &[(&str, &str)] = &[];

/// Whether `diff` is the [`GAPS`] row `pattern`.
fn gap_matches(pattern: &str, diff: &str) -> bool {
    let mut words = diff.split_whitespace();
    pattern
        .split_whitespace()
        .all(|p| words.next().is_some_and(|w| p == "*" || w.starts_with(p)))
}

/// The three clients against `Cod4Server` until [`UNTIL_MS`].
pub(super) fn ours(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
) -> (Vec<Probe>, Vec<String>) {
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    // Retail's command line as its serverinfo reflects it.
    for (k, v) in vcod_common::net::connectionless::info_pairs(retail_cs0) {
        if !matches!(k, "protocol" | "shortversion") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_cvar("sv_pure", "0");
    sv.pin_game_seed(WALK_SEED);
    // `the_challenges_come_off_the_levels_rand` reads the draws.
    sv.core.challenges.unmasked();
    sv.load(Some(install)).unwrap();
    // Each spawn on its twin's spot: the script's pick is `randomInt` off
    // a clock-seeded stream (`Server::pin_spawns`' rule).
    let pin = |o: [f32; 3], yaw: f32| Some((o, [0.0, yaw, 0.0]));
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(pin([207.0, 2291.0, 236.125], -135.0));
    pins[2].push_back(pin([-22.0, 1789.0, 224.25427], 45.0));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    // Team menu 8 (`team_marinesopfor`), class menu 15 (`changeclass`).
    let mut probes = vec![
        Probe::new(
            "mate",
            0,
            28961,
            44150,
            &[
                (1150, "mr {sid} 8 allies"),
                (2600, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new("spec", 1, 28962, 44200, &[(1150, "mr {sid} 8 spectator")]),
        Probe::new(
            "walker",
            2,
            28963,
            45200,
            &[
                (1150, "mr {sid} 8 allies"),
                (2650, "mr {sid} 15 assault_mp,0"),
                (22600, "say \"vcod probe here\""),
            ],
        ),
    ];
    probes[2].replay = replay_schedule(&retail("walk"), WALKER_SPAWN_MS);
    // Before its spawn the walker sends its twin's cmd angles: 1.7 keeps a
    // follower's cmds (`ClientThink`), and the spawn's view goes off the
    // last.
    probes[2].pre_angles = cmd_angles(&retail("walk"));
    let mut now = Instant::now();
    while sv.core.sv_time_ms < RUN_MS {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        for p in probes.iter_mut().filter(|p| p.joined) {
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (probes, aborts)
}

/// The first snapshot `slot` reads as its own playing self.
fn spawn_frame(s: &Side, slot: u32) -> Option<&Snapshot> {
    s.snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(slot))
}

/// Every difference of `pairs` (tag, retail's side, ours), the walker last.
fn diffs(pairs: &[(&str, &Side, &Side)]) -> Vec<String> {
    let mut out = Vec::new();
    let (_, r, o) = pairs[pairs.len() - 1];
    // The gamestate, slot by slot past configstring 1, the walker's.
    let keys: std::collections::BTreeSet<u16> =
        r.gs.configstrings
            .keys()
            .chain(o.gs.configstrings.keys())
            .copied()
            .collect();
    for i in keys.into_iter().filter(|&i| i >= 2) {
        if r.gs.configstring(i) != o.gs.configstring(i) {
            out.push(format!(
                "cs {i}: ours {:?} retail {:?}",
                String::from_utf8_lossy(o.gs.configstring(i)),
                String::from_utf8_lossy(r.gs.configstring(i))
            ));
        }
    }
    // The baselines, number by number and field by field.
    let numbers: std::collections::BTreeSet<u32> =
        r.gs.baselines
            .keys()
            .chain(o.gs.baselines.keys())
            .copied()
            .collect();
    for n in numbers {
        match (r.gs.baselines.get(&n), o.gs.baselines.get(&n)) {
            (Some(a), Some(b)) => {
                for f in vcod_cod4::net::delta::EntityState::table(a.etype()) {
                    if a.0[f.word()] != b.0[f.word()] {
                        out.push(format!(
                            "baseline {n} {}: ours {:#x} retail {:#x}",
                            f.name,
                            b.0[f.word()],
                            a.0[f.word()]
                        ));
                    }
                }
            }
            (a, _) => out.push(format!(
                "baseline {n}: {}",
                if a.is_some() {
                    "retail only"
                } else {
                    "ours only"
                }
            )),
        }
    }
    // The server commands each client read, in order, up to the cut.
    for &(tag, r, o) in pairs {
        let cut = |c: &[(i32, String)]| -> Vec<String> {
            c.iter()
                .filter(|(t, _)| *t < UNTIL_MS)
                .map(|(_, s)| s.clone())
                .collect()
        };
        let (rc, oc) = (cut(&r.commands), cut(&o.commands));
        if std::env::var_os("COD4_WIRE_CMDS").is_some() {
            for i in 0..rc.len().max(oc.len()) {
                eprintln!(
                    "{tag} {i:3} {:60} | {:?}",
                    format!("{:?}", rc.get(i)),
                    oc.get(i)
                );
            }
        }
        for i in 0..rc.len().max(oc.len()) {
            if rc.get(i) != oc.get(i) {
                out.push(format!(
                    "{tag} cmd {i}: ours {:?} retail {:?}",
                    oc.get(i),
                    rc.get(i)
                ));
                break;
            }
        }
    }
    // The chat lines, the whole run.
    for &(tag, r, o) in pairs {
        let chat = |c: &[(i32, String)]| -> Vec<String> {
            c.iter()
                .filter(|(_, s)| s.starts_with("h ") || s.starts_with("i "))
                .map(|(_, s)| s.clone())
                .collect()
        };
        if chat(&r.commands) != chat(&o.commands) {
            out.push(format!(
                "{tag} chat: ours {:?} retail {:?}",
                chat(&o.commands),
                chat(&r.commands)
            ));
        }
    }
    for &(tag, r, o) in pairs {
        hud_diffs(tag, r, o, &mut out);
    }
    // The walker's spawn frame.
    match (spawn_frame(r, 2), spawn_frame(o, 2)) {
        (Some(a), Some(b)) => {
            for f in vcod_cod4::net::fields::PLAYER_STATE {
                if a.ps.words[f.word()] != b.ps.words[f.word()] {
                    out.push(format!(
                        "walker ps {}: ours {:#x} retail {:#x}",
                        f.name,
                        b.ps.words[f.word()],
                        a.ps.words[f.word()]
                    ));
                }
            }
            for (name, x, y) in [
                ("stats", &a.ps.stats[..], &b.ps.stats[..]),
                ("ammo", &a.ps.ammo[..], &b.ps.ammo[..]),
                ("ammoclip", &a.ps.ammoclip[..], &b.ps.ammoclip[..]),
            ] {
                if x != y {
                    out.push(format!("walker ps {name}: ours {y:?} retail {x:?}"));
                }
            }
            // The entities it carries: the teammate's body and the script
            // models, field by field (no PVS cull on ours: the extra
            // entities are one gap).
            for e in &a.entities {
                let Some(f) = b.entities.iter().find(|x| x.number() == e.number()) else {
                    out.push(format!("walker entity {}: retail only", e.number()));
                    continue;
                };
                for t in vcod_cod4::net::delta::EntityState::table(e.etype()) {
                    if e.0[t.word()] != f.0[t.word()] {
                        out.push(format!(
                            "walker entity {} {}: ours {:#x} retail {:#x}",
                            e.number(),
                            t.name,
                            f.0[t.word()],
                            e.0[t.word()]
                        ));
                    }
                }
            }
            let extra = b
                .entities
                .iter()
                .filter(|x| !a.entities.iter().any(|e| e.number() == x.number()))
                .count();
            if extra > 0 {
                out.push(format!("walker entities: {extra} ours only"));
            }
            if a.server_time != b.server_time {
                out.push(format!(
                    "walker spawn time: ours {} retail {}",
                    b.server_time, a.server_time
                ));
            }
        }
        (a, b) => out.push(format!(
            "walker spawn frame: ours {} retail {}",
            b.is_some(),
            a.is_some()
        )),
    }
    out
}

#[test]
fn a_join_on_war_matches_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let (r_spec, r_walk) = (side(&retail("spec")), side(&retail("walk")));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    for p in &probes {
        super::join_tests::reencodes(&p.lines);
    }
    let (o_spec, o_walk) = (side(&probes[1].lines), side(&probes[2].lines));
    let mut all = diffs(&[("spec", &r_spec, &o_spec), ("walker", &r_walk, &o_walk)]);
    if std::env::var_os("COD4_WIRE_REPORT").is_some() {
        for (tag, r, o) in [("spec", &r_spec, &o_spec), ("walker", &r_walk, &o_walk)] {
            let first = |s: &Side| s.snaps.first().map(|f| f.server_time);
            eprintln!(
                "{tag} first snapshot: ours {:?} retail {:?}",
                first(o),
                first(r)
            );
        }
        for d in &all {
            eprintln!("{d}");
        }
    }
    // A row no difference matches is a gap closed: it goes.
    let unused: Vec<&str> = GAPS
        .iter()
        .filter(|(g, _)| !all.iter().any(|d| gap_matches(g, d)))
        .map(|(g, _)| *g)
        .collect();
    assert!(unused.is_empty(), "GAPS rows matching nothing: {unused:?}");
    let gap = |d: &str| GAPS.iter().any(|(g, _)| gap_matches(g, d));
    all.retain(|d| !gap(d));
    assert!(
        all.is_empty(),
        "{} differences:\n{}",
        all.len(),
        all.join("\n")
    );
}

/// The walker's playerstate words pmove writes (`cod4_moves_ab`'s list).
const WALK_FIELDS: &[&str] = &[
    "commandTime",
    "pm_type",
    "pm_flags",
    "pm_time",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "groundEntityNum",
    "jumpTime",
    "jumpOriginZ",
    "movementDir",
    "leanf",
    "viewHeightTarget",
    "viewHeightCurrent",
    "viewHeightLerpTime",
    "viewHeightLerpTarget",
    "viewHeightLerpDown",
    "bobCycle",
    "proneDirection",
    "holdBreathScale",
    "sprintState.lastSprintStart",
    "sprintState.lastSprintEnd",
    "sprintState.sprintStartMaxLength",
    "sprintState.sprintButtonUpRequired",
    "sprintState.sprintDelay",
];

/// `eFlags` bits pmove owns: crouch, prone, talk.
const EFLAGS_PMOVE: u32 = 0x20_000c;

/// What the walk replay still gets wrong, per compared word: (word, rows,
/// why). The rows are exact, so a fix that rights one fails the gate until
/// the entry shrinks.
const WALK_GAPS: &[(&str, usize, &str)] = &[];

/// What the walker's body, as the spectator is sent it, still gets wrong:
/// (word, rows, why).
const BODY_GAPS: &[(&str, usize, &str)] = &[];

/// How far the walker's origin may stray from retail's: not at all
/// (cod4-pmove.md 15.1).
const WALK_MAX_ERROR: f32 = 0.0;

/// One walk replay row's differences: the walker's own words, and its
/// teammate's body (entity 0) where both snapshots carry it.
fn walk_row(r: &Snapshot, o: &Snapshot) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    for &n in WALK_FIELDS.iter().chain(&["eFlags"]) {
        let (mut a, mut b) = (o.ps.get(n).unwrap_or(0), r.ps.get(n).unwrap_or(0));
        if n == "eFlags" {
            (a, b) = (a & EFLAGS_PMOVE, b & EFLAGS_PMOVE);
        }
        if a != b {
            out.push((n.to_string(), a, b));
        }
    }
    let mate = |s: &Snapshot| s.entities.iter().find(|e| e.number() == 0).copied();
    if let (Some(a), Some(b)) = (mate(o), mate(r)) {
        for n in [
            "lerp.pos.trBase[0]",
            "lerp.pos.trBase[1]",
            "lerp.pos.trBase[2]",
            "lerp.apos.trBase[1]",
        ] {
            let n = n.to_string();
            let w = vcod_cod4::net::delta::EntityState::table(1)
                .iter()
                .find(|f| f.name == n)
                .unwrap()
                .word();
            if a.0[w] != b.0[w] {
                out.push((format!("mate {n}"), a.0[w], b.0[w]));
            }
        }
    }
    out
}

/// The walk past the spawn: the walker replays the capture's own cmds on
/// ours, and every retail snapshot from the spawn on is compared with ours
/// of the same server time, word for word: the playerstate pmove writes
/// and the teammate's origin and yaw. Ours never resyncs to retail, so a word off
/// once may stay off; [`WALK_GAPS`] names each word's rows.
/// `COD4_WALK_REPORT=1` prints every wrong row.
#[test]
fn the_walk_replays_on_ours_as_on_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = side(&retail("walk"));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let o_walk = side(&probes[2].lines);
    let report = std::env::var_os("COD4_WALK_REPORT").is_some();
    let mut rows = 0;
    let mut exact = 0;
    let mut close = 0;
    let mut missing = Vec::new();
    let mut fields: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut max_err = (0f32, 0);
    for r in r_walk.snaps.iter().filter(|s| {
        s.server_time >= Msec::from_raw(WALKER_SPAWN_MS) && s.ps.get("clientNum") == Some(2)
    }) {
        let Some(o) = o_walk.snaps.iter().find(|s| s.server_time == r.server_time) else {
            missing.push(r.server_time);
            continue;
        };
        rows += 1;
        let d = walk_row(r, o);
        let origin = |s: &Snapshot| {
            glam::Vec3::from_array(
                [0, 1, 2].map(|i| s.ps.get_f32(&format!("origin[{i}]")).unwrap_or(0.0)),
            )
        };
        let err = (origin(o) - origin(r)).length();
        if err > max_err.0 {
            max_err = (err, r.server_time.raw());
        }
        if d.is_empty() {
            exact += 1;
            continue;
        }
        let float_only = d.iter().all(|(n, a, b)| {
            n.starts_with("origin[") || (n == "velocity[2]" && (a | b) & 0x7fff_ffff == 0)
        });
        if float_only && err <= WALK_MAX_ERROR {
            close += 1;
        }
        if report {
            let words: Vec<String> = d
                .iter()
                .map(|(n, a, b)| format!("{n} {a:#x} vs {b:#x}"))
                .collect();
            eprintln!("  {}: {}", r.server_time, words.join(", "));
        }
        for (n, a, b) in d {
            let e = fields.entry(n).or_insert_with(|| {
                (
                    0,
                    format!("first at {}: ours {a:#x} retail {b:#x}", r.server_time),
                )
            });
            e.0 += 1;
        }
    }
    eprintln!(
        "walk: {exact} of {rows} rows exact, {close} more off only in the float model, \
         largest origin error {:.4} at {}",
        max_err.0, max_err.1
    );
    let mut unexplained = Vec::new();
    for (n, (count, first)) in &fields {
        let gap = WALK_GAPS.iter().find(|g| g.0 == n);
        eprintln!(
            "  {n}: {count} rows ({}), {first}",
            gap.map_or("not a gap", |g| g.2)
        );
        if gap.map(|g| g.1) != Some(*count) {
            unexplained.push(format!("{n}: {count} rows"));
        }
    }
    for g in WALK_GAPS {
        if !fields.contains_key(g.0) {
            unexplained.push(format!("{}: gap listed, 0 rows", g.0));
        }
    }
    // The walker's body as the spectator is sent it.
    let (r_spec, o_spec) = (side(&retail("spec")), side(&probes[1].lines));
    let body = |s: &Snapshot| s.entities.iter().find(|e| e.number() == 2).copied();
    let (mut bodies, mut body_exact) = (0, 0);
    let mut body_words: BTreeMap<String, usize> = BTreeMap::new();
    for r in r_spec
        .snaps
        .iter()
        .filter(|s| (WALKER_SPAWN_MS..RUN_MS).contains(&s.server_time.raw()))
    {
        let Some(o) = o_spec.snaps.iter().find(|s| s.server_time == r.server_time) else {
            missing.push(r.server_time);
            continue;
        };
        let (Some(a), Some(b)) = (body(o), body(r)) else {
            continue;
        };
        bodies += 1;
        let mut same = true;
        for f in vcod_cod4::net::delta::EntityState::table(1) {
            if a.0[f.word()] != b.0[f.word()] {
                same = false;
                *body_words.entry(f.name.to_string()).or_default() += 1;
                if report && f.name == "lerp.apos.trBase[1]" {
                    let yaw = |e: &EntityState| f32::from_bits(e.0[f.word()]);
                    eprintln!(
                        "  body yaw {}: ours {} retail {}",
                        r.server_time,
                        yaw(&a),
                        yaw(&b)
                    );
                }
            }
        }
        body_exact += usize::from(same);
    }
    // The spectator's own playerstate while it floats (stock
    // `scr_game_spectatetype` 1: `ps.speed` 0, VERIFIED live). Its cmds are
    // the in-process probe's, not the capture's, so `commandTime` is not
    // compared.
    let (mut floats, mut float_exact) = (0, 0);
    for r in r_spec.snaps.iter().filter(|s| {
        (WALKER_SPAWN_MS..RUN_MS).contains(&s.server_time.raw()) && s.ps.get("clientNum") == Some(1)
    }) {
        let Some(o) = o_spec.snaps.iter().find(|s| s.server_time == r.server_time) else {
            continue;
        };
        floats += 1;
        let d: Vec<_> = walk_row(r, o)
            .into_iter()
            .filter(|(n, _, _)| !n.starts_with("mate") && n != "commandTime")
            .collect();
        if d.is_empty() {
            float_exact += 1;
        } else if report {
            eprintln!("  spec {}: {d:x?}", r.server_time);
        }
    }
    eprintln!("spectator: {float_exact} of {floats} rows exact");
    if float_exact != floats {
        unexplained.push(format!("spectator: {} rows wrong", floats - float_exact));
    }
    eprintln!("walker's body to the spectator: {body_exact} of {bodies} exact");
    for (n, c) in &body_words {
        let gap = BODY_GAPS.iter().find(|g| g.0 == n);
        eprintln!(
            "  body {n}: {c} rows ({})",
            gap.map_or("not a gap", |g| g.2)
        );
        if gap.map(|g| g.1) != Some(*c) {
            unexplained.push(format!("body {n}: {c} rows"));
        }
    }
    for g in BODY_GAPS {
        if !body_words.contains_key(g.0) {
            unexplained.push(format!("body {}: gap listed, 0 rows", g.0));
        }
    }
    assert!(missing.is_empty(), "no snapshot of ours at {missing:?}");
    assert!(
        max_err.0 <= WALK_MAX_ERROR,
        "origin {} off at {}",
        max_err.0,
        max_err.1
    );
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// The words of an entity's table, by name, where `a` and `b` differ.
fn entity_diff(a: &EntityState, b: &EntityState) -> Vec<(&'static str, u32, u32)> {
    EntityState::table(b.etype())
        .iter()
        .filter(|f| a.0[f.word()] != b.0[f.word()])
        .map(|f| (f.name, a.0[f.word()], b.0[f.word()]))
        .collect()
}

/// The walker's frag (thrown at 66050, cod4-netfields.md 8) as the walker is
/// sent it: the grenade (`ET_MISSILE`) every snapshot it rides, the burst's
/// event entity (`ET_GENERAL` with `EV_GRENADE_EXPLODE`) and the script's
/// earthquake (17 + 61), each by type, word for word but the entity number
/// (cod4-combat.md 10); what differs is [`FRAG_GAPS`].
/// `COD4_FRAG_REPORT=1` prints every row.
#[test]
fn the_frag_flies_on_ours_as_on_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = side(&retail("walk"));
    let cs0 = String::from_utf8_lossy(r_walk.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let o_walk = side(&probes[2].lines);
    let report = std::env::var_os("COD4_FRAG_REPORT").is_some();
    type Is = fn(&EntityState) -> bool;
    let kinds: [(&str, Is); 3] = [
        ("missile", |e| e.etype() == 4),
        ("burst", |e| {
            e.etype() == 0
                && e.0[EntityState::table(0)
                    .iter()
                    .find(|f| f.name == "events[0]")
                    .unwrap()
                    .word()]
                    == 45
        }),
        ("quake", |e| e.etype() == 17 + 61),
    ];
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    let mut rows: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in r_walk
        .snaps
        .iter()
        .filter(|s| (65_000..71_000).contains(&s.server_time.raw()))
    {
        let o = o_walk.snaps.iter().find(|s| s.server_time == r.server_time);
        for (kind, is) in kinds {
            let a = o.and_then(|o| o.entities.iter().find(|e| is(e)).copied());
            let b = r.entities.iter().find(|e| is(e)).copied();
            let e = rows.entry(kind).or_default();
            match (a, b) {
                (None, None) => continue,
                (Some(a), Some(b)) => {
                    e.0 += 1;
                    let d: Vec<_> = entity_diff(&a, &b)
                        .into_iter()
                        .filter(|(n, _, _)| *n != "number")
                        .collect();
                    if d.is_empty() {
                        e.1 += 1;
                    }
                    if report && !d.is_empty() {
                        let f: Vec<String> = d
                            .iter()
                            .map(|(n, x, y)| format!("{n} {x:#x} vs {y:#x}"))
                            .collect();
                        eprintln!("  {kind} {}: {}", r.server_time, f.join(", "));
                    }
                    for (n, _, _) in d {
                        *words.entry(format!("{kind} {n}")).or_default() += 1;
                    }
                }
                (a, b) => {
                    e.0 += 1;
                    if report {
                        eprintln!(
                            "  {kind} {}: ours {} retail {}",
                            r.server_time,
                            a.is_some(),
                            b.is_some()
                        );
                    }
                    *words.entry(format!("{kind} present")).or_default() += 1;
                }
            }
        }
    }
    for (k, (n, exact)) in &rows {
        eprintln!("{k}: {exact} of {n} rows exact");
    }
    let mut unexplained = Vec::new();
    for (n, c) in &words {
        let gap = FRAG_GAPS.iter().find(|g| g.0 == n);
        eprintln!("  {n}: {c} rows ({})", gap.map_or("not a gap", |g| g.2));
        if gap.map(|g| g.1) != Some(*c) {
            unexplained.push(format!("{n}: {c} rows"));
        }
    }
    for g in FRAG_GAPS {
        if !words.contains_key(g.0) {
            unexplained.push(format!("{}: gap listed, 0 rows", g.0));
        }
    }
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// What the frag replay still gets wrong: (kind and word, rows, why).
const FRAG_GAPS: &[(&str, usize, &str)] = &[];

/// A capture's `challengeResponse`.
fn challenge_of(lines: &[Line]) -> Option<i32> {
    lines.iter().filter(|l| l.dir == "s2c").find_map(|l| {
        let text = String::from_utf8_lossy(l.data.get(4..)?);
        text.strip_prefix("challengeResponse ")?.trim().parse().ok()
    })
}

/// The challenges come off the level's `rand()` (cod4-combat.md 10.9): on
/// [`WALK_SEED`], after the map's pickups drew 17, the spectator's and the
/// walker's draws are retail's. A challenge is `(rand() << 16) ^ rand() ^
/// svs.time`, and the in-process probes ask theirs at another `svs.time`
/// (under 2^17 on both), so the bits above it are compared. vcod masks the
/// challenge it sends (`engine::challenge`); [`ours`] turns the mask off.
#[test]
fn the_challenges_come_off_the_levels_rand() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r_walk = retail("walk");
    let cs0 = String::from_utf8_lossy(side(&r_walk).gs.configstring(0)).into_owned();
    let (probes, _) = ours(&install, &cs0);
    for (probe, name) in [(&probes[1], "spec"), (&probes[2], "walk")] {
        let (o, r) = (challenge_of(&probe.lines), challenge_of(&retail(name)));
        assert_eq!(
            o.map(|c| c >> 17),
            r.map(|c| c >> 17),
            "{name}: {o:?} {r:?}"
        );
    }
}

/// Recovers a run's `srand` seed: retail seeds `G_InitGame`'s with the
/// wall clock's milliseconds (`Sys_MillisecondsRaw`, 0x81d67ac), so the
/// search walks `COD4_SEED_FROM..COD4_SEED_TO` (Unix ms, around the run's
/// `games_mp.log`, which `G_InitGame` opens) for a stream whose first 64
/// draws give every capture in `COD4_SEED_CAPTURES` (default `walk,spec`)
/// its challenge with a `svs.time` under 2^18. Run by hand
/// (`cargo test -p vcod-server --lib -- --ignored recover_game_seed --nocapture`); it finds
/// [`WALK_SEED`] alone, and with `kill-target,kill-shooter` `combat_ab`'s
/// `KILL_SEED`.
#[test]
#[ignore = "seed search by hand: COD4_SEED_FROM and COD4_SEED_TO"]
fn recover_game_seed() {
    let ms = |k: &str| -> u64 { std::env::var(k).expect(k).parse().unwrap() };
    let (from, to) = (ms("COD4_SEED_FROM"), ms("COD4_SEED_TO"));
    let names = std::env::var("COD4_SEED_CAPTURES").unwrap_or("walk,spec".into());
    let challenges: Vec<u32> = names
        .split(',')
        .map(|n| challenge_of(&retail(n)).expect("a challenge") as u32)
        .collect();
    for t in from..to {
        let mut g = vcod_common::rng::GlibcRand::new(t as u32);
        let d: Vec<u32> = (0..65).map(|_| g.next_u31()).collect();
        let found: Vec<(usize, u32)> = challenges
            .iter()
            .filter_map(|&c| {
                (0..64)
                    .map(|k| (k, (d[k] << 16) ^ d[k + 1] ^ c))
                    .find(|&(_, svs)| svs < 1 << 18)
            })
            .collect();
        if found.len() == challenges.len() {
            println!("ms {t} seed {} (draw, svs.time) {found:?}", t as u32);
        }
    }
}
