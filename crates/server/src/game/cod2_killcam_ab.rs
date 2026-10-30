//! The CoD2 killcam A/B: retail's captures in
//! `crates/cod2/tests/fixtures/killcam/`, each a pair (the shooter's
//! `<name>.txt`, the target's `<name>-b.txt`; the recipe in each header)
//! replayed on `Cod2Level` under the same `probe_killcam.gsc`, the replay
//! as `cod2_combat_ab`'s. `killcam` is the plain kill; in `killerdies`
//! the killer drops his helmet, suicides inside the replayed span and
//! respawns. Each frame archives after the snapshot, as `SV_Frame` does.
//!
//! Compared: both clients' `eFlags` at the script's first spawn; per
//! snapshot of the target its own words while alive or dead, inside the
//! killcam the copy's (`pm_type`, `pm_flags`, `deltaTime`,
//! `commandTime`, view, origin, `eFlags`, the event ring), on the frame
//! it ends and the seven after it the dead spawn's, which clients and
//! bodies (numbers below 72) the killcam's snapshots carry, and every
//! snapshot's roster where the capture logged one; per snapshot of the
//! shooter its own words (its death and respawn); and the gametype's
//! PROBE lines. A difference must be named in [`GAPS`] with its section
//! of docs/research/cod2-killcam.md. Needs `COD2_DIR`; `KILLCAM_REPORT=1`
//! prints every mismatching row.

use std::rc::Rc;

use vcod_cod2::net::delta::ClientState2;
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;
use vcod_cod2::net::{PlayerState2, UserCmd2};
use vcod_cod2::pmove::PlayerState;

use super::cod2::Cod2Level;
use super::cod2_combat_ab::{Roster, Snap, parse, weapon_from_wire, word};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod2/tests/fixtures/killcam"
);

/// What the replay still gets wrong: (check, count, why).
const GAPS: &[(&str, usize, &str)] = &[];

/// The copy's words compared exactly inside the killcam.
const COPY_WORDS: &[&str] = &[
    "clientNum",
    "pm_type",
    "pm_flags",
    "deltaTime",
    "commandTime",
    "weapon",
    "viewangles[0]",
    "viewangles[1]",
    "eFlags",
    "eventSequence",
];

/// A client's own words outside the killcam: alive, dead (`pm_flags`
/// 0x800000 kept) and respawned (the event ring cleared).
const OWN_WORDS: &[&str] = &[
    "clientNum",
    "pm_type",
    "pm_flags",
    "eFlags",
    "eventSequence",
];

/// The dead spawn's words on the frame the killcam ends and after.
const END_WORDS: &[&str] = &[
    "clientNum",
    "pm_type",
    "pm_flags",
    "eFlags",
    "deltaTime",
    "commandTime",
    "weapon",
    "viewHeightCurrent",
    "viewHeightTarget",
    "viewangles[0]",
    "viewangles[1]",
    "eventSequence",
];

/// The clients' names, as move_probe joins.
const NAMES: [&str; 2] = ["target", "vcodmoves"];

fn compare(
    note: &mut impl FnMut(&str, String),
    check: &str,
    row: &str,
    words: &[&str],
    ours: &PlayerState2,
    retail: &PlayerState2,
) {
    for &n in words {
        let (a, r) = (word(ours, n), word(retail, n));
        if a != r {
            note(
                &format!("{check} {n}"),
                format!("{row}: ours {a:x} retail {r:x}"),
            );
        }
    }
}

fn roster_of(clients: &[ClientState2]) -> Roster {
    clients.iter().map(|c| (c.number, c.f)).collect()
}

fn replay(fs: &Rc<vcod_common::pk3::Pk3Fs>, name: &str, report: bool) -> Vec<(String, String)> {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}")).unwrap();
    let shooter = parse(&read(&format!("{name}.txt")));
    let target = parse(&read(&format!("{name}-b.txt")));
    let gsc = read("probe_killcam.gsc");
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    for (k, v) in &shooter.cvars {
        cvars.set(k, v);
    }
    let playing = |s: &Snap| word(&s.ps, "pm_type") == 0 && word(&s.ps, "weapon") != 0;
    let start = shooter
        .side
        .snaps
        .iter()
        .filter(|s| playing(s))
        .find(|s| target.side.at(s.time).is_some_and(playing))
        .map(|s| s.time)
        .expect("no snapshot time both share");
    let mut level = Cod2Level::load_with_script(
        fs.clone(),
        &shooter.map,
        "probe_killcam",
        ("maps/mp/gametypes/probe_killcam".to_string(), gsc),
        cvars,
        start - 2000,
    )
    .expect("load probe_killcam");
    level.connect(0, NAMES[0]);
    level.connect(1, NAMES[1]);
    level.run_frame(start - 1950);
    level.begin(0);
    level.begin(1);
    let roster = |level: &mut Cod2Level| -> Vec<ClientState2> {
        (0..2).map(|s| level.client_state(s, NAMES[s])).collect()
    };
    let mut t = start - 1950;
    while t < start {
        t += 50;
        level.run_frame(t);
        let world = level.world_entities(t);
        let r = roster(&mut level);
        level.archive_frame(&world, &r);
    }
    let sides = [&target.side, &shooter.side];
    let mut done = [0i32; 2];
    let mut found: Vec<(String, String)> = Vec::new();
    let mut note = |check: &str, what: String| {
        if report {
            eprintln!("  {check}: {what}");
        }
        found.push((check.to_string(), what));
    };
    for (slot, side) in sides.iter().enumerate() {
        // The script's first spawn, out of `ClientConnect`'s zeroed
        // `eFlags`: the teleport bit flips on (cod2-killcam.md 5).
        let team = level.rt.client_team(slot);
        let ours = level.wire_playerstate(slot, team).unwrap();
        let (a, r) = (word(&ours, "eFlags"), word(&side.snaps[0].ps, "eFlags"));
        if a != r {
            note(
                "spawn eFlags",
                format!("slot {slot}: ours {a:x} retail {r:x}"),
            );
        }
        let snap = side.at(start).unwrap();
        let p = level.players[slot].as_mut().expect("both connected");
        p.ps = PlayerState::from_wire(&snap.ps);
        weapon_from_wire(p, &snap.ps);
        done[slot] = p.ps.command_time;
        if let Some(c) = side.cmds.iter().rev().find(|c| c.server_time <= done[slot]) {
            p.oldcmd = *c;
        }
    }
    let end = shooter
        .side
        .snaps
        .last()
        .unwrap()
        .time
        .min(target.side.snaps.last().unwrap().time);
    let mut last_retail: Option<u32> = None;
    let mut killcam_frames = 0;
    let mut after = 0usize;
    while t < end {
        t += 50;
        let (Some(rs), Some(rt)) = (shooter.side.at(t), target.side.at(t)) else {
            continue;
        };
        let mut due: Vec<(i32, usize, UserCmd2)> = Vec::new();
        for (slot, snap) in [(0, rt), (1, rs)] {
            let ct = word(&snap.ps, "commandTime") as i32;
            let side = sides[slot];
            for (k, c) in side.cmds.iter().enumerate() {
                if c.server_time > done[slot] && c.server_time <= ct {
                    let sent = side.sent.get(k).copied().unwrap_or(c.server_time);
                    due.push((sent, slot, *c));
                }
            }
            done[slot] = done[slot].max(ct);
        }
        due.sort_by_key(|&(sent, slot, c)| (sent, slot, c.server_time));
        for (_, slot, c) in &due {
            level.client_think(*slot, c);
        }
        level.run_frame(t);
        let world = level.world_entities(t);
        let live = roster(&mut level);
        let team = level.rt.client_team(0);
        let ours: PlayerState2 = level.wire_playerstate(0, team).unwrap();
        let (ents, archived) = level.snapshot_entities(&world, 0, &ours);
        let team = level.rt.client_team(1);
        let shooter_ps = level.wire_playerstate(1, team).unwrap();
        level.archive_frame(&world, &live);
        let row = format!("serverTime {t}");
        let retail_cn = word(&rt.ps, "clientNum");
        let in_killcam = retail_cn == 1;
        let ended = last_retail == Some(1) && retail_cn == 0;
        if ended {
            after = 8;
        }
        let after_end = after > 0;
        after = after.saturating_sub(1);
        last_retail = Some(retail_cn);
        killcam_frames += usize::from(in_killcam);
        let (check, words) = if after_end {
            (if ended { "end" } else { "after" }, END_WORDS)
        } else if in_killcam {
            ("copy", COPY_WORDS)
        } else {
            ("own", OWN_WORDS)
        };
        compare(&mut note, check, &row, words, &ours, &rt.ps);
        compare(&mut note, "shooter", &row, OWN_WORDS, &shooter_ps, &rs.ps);
        // The roster: the archived frame's inside the killcam
        // (cod2-killcam.md 4), the live one outside it.
        if !rt.clients.is_empty() {
            let a = roster_of(archived.as_deref().unwrap_or(&live));
            for (n, r) in &rt.clients {
                let Some(c) = a.get(n) else {
                    note("roster", format!("{row}: client {n} missing"));
                    continue;
                };
                for (i, f) in CLIENT_STATE_FIELDS.iter().enumerate() {
                    if c[i] != r[i] {
                        note(
                            &format!("roster {}", f.name),
                            format!("{row}: client {n} ours {:x} retail {:x}", c[i], r[i]),
                        );
                    }
                }
            }
            if a.len() != rt.clients.len() {
                note("roster", format!("{row}: ours {} clients", a.len()));
            }
        }
        if in_killcam || ended {
            for k in 0..3 {
                let n = format!("origin[{k}]");
                let (a, r) = (word(&ours, &n), word(&rt.ps, &n));
                if (f32::from_bits(a) - f32::from_bits(r)).abs() > 0.01 {
                    note(
                        &format!("{check} origin"),
                        format!(
                            "{row}: {n} ours {} retail {}",
                            f32::from_bits(a),
                            f32::from_bits(r)
                        ),
                    );
                }
            }
            let bodies = |it: &mut dyn Iterator<Item = u32>| -> Vec<u32> {
                it.filter(|&n| n < 72).collect()
            };
            let (a, r) = (
                bodies(&mut ents.iter().map(|e| e.number)),
                bodies(&mut rt.ents.keys().copied()),
            );
            if a != r {
                note("bodies", format!("{row}: ours {a:?} retail {r:?}"));
            }
        }
    }
    if report {
        eprintln!("  {killcam_frames} killcam frames, replay {start}..{end}");
    }
    if killcam_frames == 0 {
        note("killcam", "retail's capture holds no killcam".into());
    }
    // The gametype's lines past the spawns: the kills, the killcam's
    // start, the trimmed `archivetime`, the end. Times compare exactly.
    let ours: Vec<String> = level
        .rt
        .host
        .script_log
        .iter()
        .filter_map(|l| l.split_once("PROBE ").map(|p| p.1.trim().to_string()))
        .filter(|l| !l.starts_with("spawned") && !l.starts_with("damage"))
        .map(|l| l.split(" kar98k").next().unwrap().to_string())
        .collect();
    let retail: Vec<String> = shooter
        .probes
        .iter()
        .filter(|l| !l.starts_with("spawned") && !l.starts_with("damage"))
        .map(|l| l.trim().split(" kar98k").next().unwrap().to_string())
        .collect();
    if ours != retail {
        note("probe lines", format!("ours {ours:?} retail {retail:?}"));
    }
    let aborts = level.rt.aborts();
    if !aborts.is_empty() {
        note("script errors", aborts.join("; "));
    }
    found
}

fn gate(name: &str) {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let report = std::env::var_os("KILLCAM_REPORT").is_some();
    let found = replay(&Rc::new(fs), name, report);
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for (check, _) in &found {
        *counts.entry(check.as_str()).or_default() += 1;
    }
    let mut unexplained = Vec::new();
    for (check, n) in &counts {
        let allowed = GAPS.iter().find(|g| g.0 == *check).map_or(0, |g| g.1);
        if *n != allowed {
            let first = found.iter().find(|f| f.0 == *check).unwrap();
            unexplained.push(format!("{check}: {n} (allowed {allowed}); {}", first.1));
        }
    }
    for (check, n, why) in GAPS {
        if !counts.contains_key(check) && *n > 0 {
            unexplained.push(format!("{check}: gap no longer seen ({why})"));
        }
    }
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

#[test]
fn retail_killcam_replays_on_cod2_level() {
    gate("killcam");
}

/// The killer drops his helmet, dies and respawns while the target's
/// killcam replays him: the copy follows the dead killer, the snapshots
/// carry the archived roster, the respawn clears his event ring.
#[test]
fn retail_killcam_of_a_dead_killer_replays_on_cod2_level() {
    gate("killerdies");
}
