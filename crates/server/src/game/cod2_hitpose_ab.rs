//! The CoD2 hit pose A/B: retail's rounds on a posed body, replayed on
//! vcod's. The capture in `crates/cod2/tests/fixtures/hitpose/` (recipe in
//! its header) is a shooter holding a ppsh's trigger while
//! `probe_hitpose.gsc` sweeps his view over a target that stands, crouches,
//! lies prone and strafes. Each `PROBE damage` line gives a round's point,
//! direction and hit location at a frame's time; the gate puts the target's
//! playerstate where retail's snapshot of that frame has it, runs the frame,
//! and traces the same line through vcod's posed body. Compared: the hit
//! location and the point along the line. Needs `COD2_DIR`.
//! `HITPOSE_REPORT=1` prints every round.

use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::pmove::PlayerState;

use super::cod2_combat_ab::{parse, weapon_from_wire, word};
use crate::game::cod2::Cod2Level;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod2/tests/fixtures/hitpose"
);

/// How far along its line a round's point may sit from retail's: the log's
/// six digits on a coordinate near 2000.
const POINT_TOLERANCE: f32 = 0.02;

/// The rounds the replay still places off retail's, by frame time, each
/// with its section of cod2-combat.md.
const GAPS: &[(i32, &str)] = &[(
    13400,
    "9.5 the hit pose: 0.0204 along a grazing face, inside the log's rounding",
)];

/// What one round found: (time, retail hit location, ours, how far along
/// the line ours sits past retail's, the target's `eFlags`).
struct Round {
    time: i32,
    retail: String,
    ours: String,
    along: Option<f32>,
    eflags: u32,
}

fn replay(fs: &Rc<vcod_common::pk3::Pk3Fs>, name: &str) -> Vec<Round> {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let shooter = parse(&read(name));
    let target = parse(&read(&format!("{name}-b")));
    let gsc = std::fs::read_to_string(format!("{FIXTURES}/probe_hitpose.gsc")).unwrap();
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    for (k, v) in &shooter.cvars {
        cvars.set(k, v);
    }
    let playing = |s: &super::cod2_combat_ab::Snap| word(&s.ps, "pm_type") == 0;
    let start = target
        .side
        .snaps
        .iter()
        .find(|s| playing(s))
        .map(|s| s.time)
        .expect("the target never plays");
    let mut level = Cod2Level::load_with_script(
        fs.clone(),
        &shooter.map,
        "probe_hitpose",
        (
            "maps/mp/gametypes/probe_hitpose".to_string(),
            gsc.to_string(),
        ),
        cvars,
        start - 2000,
    )
    .expect("load probe_hitpose");
    level.connect(0, "target");
    level.run_frame(start - 1950);
    level.begin(0);
    let mut t = start - 1950;
    while t < start - 50 {
        t += 50;
        level.run_frame(t);
    }
    // The rounds: (time, point, direction, hit location, the shooter's cmd
    // time: `level.time` less the logged time offset).
    let rounds: Vec<(i32, Vec3, Vec3, String, i32)> = shooter
        .probes
        .iter()
        .filter_map(|l| l.strip_prefix("damage "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let vecs: Vec<Vec3> = l
                .split('(')
                .skip(1)
                .map(|v| {
                    let n: Vec<f32> = v
                        .split(')')
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|x| x.trim().parse().unwrap())
                        .collect();
                    Vec3::new(n[0], n[1], n[2])
                })
                .collect();
            let time: i32 = w[1].parse().unwrap();
            let offset: i32 = l.rsplit(' ').next().unwrap().parse().unwrap();
            (time, vecs[0], vecs[1], w[6].to_string(), time - offset)
        })
        .collect();
    let mut out = Vec::new();
    let end = target.side.snaps.last().unwrap().time;
    let mut t = start - 50;
    while t < end {
        t += 50;
        let Some(snap) = target.side.at(t) else {
            continue;
        };
        let mut done = {
            let p = level.players[0].as_mut().expect("connected");
            p.ps = PlayerState::from_wire(&snap.ps);
            weapon_from_wire(p, &snap.ps);
            if let Some(c) = target
                .side
                .cmds
                .iter()
                .rev()
                .find(|c| c.server_time <= p.ps.command_time)
            {
                p.oldcmd = *c;
            }
            p.ps.command_time
        };
        level.run_frame(t);
        if std::env::var("HITPOSE_BONES").is_ok_and(|v| v == t.to_string()) {
            for (n, w, b) in level.body_bones(0) {
                eprintln!(
                    "  {n:<24} {:8.3} {:8.3} {:8.3}   bind {:8.3} {:8.3} {:8.3}",
                    w.x, w.y, w.z, b.x, b.y, b.z
                );
            }
        }
        for (_, point, dir, loc, shot) in rounds.iter().filter(|r| r.0 == t) {
            // The target's packets sent by the one carrying the shot's cmd,
            // whole, then the shooter's: each resets the skeleton cache.
            run_packets(
                &mut level,
                &target.side,
                &mut done,
                shooter.side.sent_at(*shot),
            );
            level.packet();
            let (a, b) = (*point - *dir * 64.0, *point + *dir * 64.0);
            let (f, client, hl) = level.round_trace(a, b, &Default::default());
            let ours = match client {
                Some(_) => vcod_cod2::xmodel::HIT_LOCATIONS[usize::from(hl)].to_string(),
                None => "miss".to_string(),
            };
            out.push(Round {
                time: t,
                retail: loc.clone(),
                ours,
                along: client.map(|_| (f - 0.5) * 128.0),
                eflags: word(&snap.ps, "eFlags"),
            });
        }
    }
    out
}

/// Runs `side`'s (slot 0's) cmds past `done` in the packets sent by
/// `sent`, a `packet()` before each. Retail runs a packet's cmds together,
/// so a round falls between two of the target's packets, never inside one
/// (cod2-combat.md 9.5).
pub(super) fn run_packets(
    level: &mut Cod2Level,
    side: &super::cod2_combat_ab::Side,
    done: &mut i32,
    sent: i32,
) {
    let mut packet = None;
    for (s, c) in side.packets_by(*done, sent) {
        if packet != Some(s) {
            packet = Some(s);
            level.packet();
        }
        level.client_think(0, &c);
        *done = c.server_time;
    }
}

#[test]
fn retail_rounds_strike_vcods_posed_body() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let report = std::env::var_os("HITPOSE_REPORT").is_some();
    let rounds = replay(&fs, "poses");
    let mut wrong = Vec::new();
    for r in &rounds {
        let line = format!(
            "{} eFlags {:#x}: retail {} ours {} along {}",
            r.time,
            r.eflags,
            r.retail,
            r.ours,
            r.along.map_or("-".to_string(), |a| format!("{a:.3}"))
        );
        if report {
            eprintln!("{line}");
        }
        let off = r.ours != r.retail || r.along.is_none_or(|a| a.abs() > POINT_TOLERANCE);
        let gap = GAPS.iter().any(|g| g.0 == r.time);
        if off != gap {
            wrong.push(if gap {
                format!("{line} (right, GAPS names it)")
            } else {
                line
            });
        }
    }
    eprintln!("{} of {} rounds wrong", wrong.len(), rounds.len());
    assert!(!rounds.is_empty());
    assert!(
        wrong.is_empty(),
        "rounds off retail's:\n{}",
        wrong.join("\n")
    );
}

/// The bones `probe_combat.gsc`'s `tags()` links to, in its order.
const TAGS: [&str; 8] = [
    "j_wrist_le",
    "j_elbow_le",
    "j_shoulder_le",
    "j_spine4",
    "j_mainroot",
    "j_knee_le",
    "j_ankle_le",
    "j_wrist_ri",
];

/// Units a bone may sit off retail's. The log keeps 1e-4; what is left
/// reaches 0.011 (cod2-combat.md 9.3, "The knocked body").
const TAG_TOLERANCE: f32 = 0.015;

/// The knocked body's bones, frame by frame: `combat/hit-tags` is
/// `hit-smg`'s burst with `probe_tags 1` (recipe in its header), whose
/// `PROBE tags` lines log eight of the target's bones each frame through
/// linked script_origins. A line at a frame's time holds the bones the
/// frame before posed in its entity pass, after that frame's cmds and
/// before its end frame (cod2-combat.md 9.3, "The knocked body"). The
/// gate puts the target's playerstate where retail's snapshot of each
/// frame has it, so its body reads retail's anims and `pos.trBase`, poses
/// it as that entity pass would and runs the frame. Compared: each bone
/// less the body's origin. `HITPOSE_REPORT=1` prints every frame off.
#[test]
fn retail_bones_follow_a_knocked_body() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let report = std::env::var_os("HITPOSE_REPORT").is_some();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod2/tests/fixtures/combat");
    let read = |n: &str| std::fs::read_to_string(format!("{dir}/{n}.txt")).unwrap();
    let shooter = parse(&read("hit-tags"));
    let target = parse(&read("hit-tags-b"));
    let gsc = std::fs::read_to_string(format!("{dir}/probe_combat.gsc")).unwrap();
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    // vcod's `linkTo` reaches no CoD2 player bone: the gate poses them.
    for (k, v) in shooter.cvars.iter().filter(|c| c.0 != "probe_tags") {
        cvars.set(k, v);
    }
    let cvar = |n: &str| {
        shooter
            .cvars
            .iter()
            .find(|c| c.0 == n)
            .map_or(0.0, |c| c.1.parse::<f32>().unwrap())
    };
    let base = Vec3::new(cvar("probe_vx"), cvar("probe_vy"), cvar("probe_vz"));
    let tags: std::collections::BTreeMap<i32, Vec<Vec3>> = shooter
        .probes
        .iter()
        .filter_map(|l| l.strip_prefix("tags "))
        .map(|l| {
            let time = l.split_whitespace().next().unwrap().parse().unwrap();
            let vecs = l
                .split('(')
                .skip(1)
                .map(|v| {
                    let n: Vec<f32> = v
                        .split(')')
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|x| x.trim().parse().unwrap())
                        .collect();
                    Vec3::new(n[0], n[1], n[2]) + base
                })
                .collect();
            (time, vecs)
        })
        .collect();
    let playing = |s: &super::cod2_combat_ab::Snap| word(&s.ps, "pm_type") == 0;
    let start = target
        .side
        .snaps
        .iter()
        .find(|s| playing(s))
        .map(|s| s.time)
        .expect("the target never plays");
    let mut level = Cod2Level::load_with_script(
        fs.clone(),
        &shooter.map,
        "probe_combat",
        (
            "maps/mp/gametypes/probe_combat".to_string(),
            gsc.to_string(),
        ),
        cvars,
        start - 2000,
    )
    .expect("load probe_combat");
    level.connect(0, "target");
    level.run_frame(start - 1950);
    level.begin(0);
    let mut t = start - 1950;
    while t < start - 50 {
        t += 50;
        level.run_frame(t);
    }
    let end = *tags.keys().last().expect("no tags lines");
    let (mut frames, mut wrong) = (0, Vec::new());
    let mut t = start - 50;
    while t < end {
        t += 50;
        let Some(snap) = target.side.at(t) else {
            continue;
        };
        let origin = {
            let p = level.players[0].as_mut().expect("connected");
            p.ps = PlayerState::from_wire(&snap.ps);
            weapon_from_wire(p, &snap.ps);
            p.ps.origin
        };
        if let Some(retail) = tags.get(&(t + 50)) {
            frames += 1;
            let ours = level.tag_positions(0, t, &TAGS);
            let off: Vec<(&str, Vec3)> = TAGS
                .iter()
                .zip(ours.iter().zip(retail))
                .map(|(n, (a, r))| (*n, *a - *r))
                .filter(|(_, d)| d.abs().max_element() > TAG_TOLERANCE)
                .collect();
            if report && !off.is_empty() {
                for ((n, a), r) in TAGS.iter().zip(&ours).zip(retail) {
                    eprintln!(
                        "{} {n:14} ours {:.3} retail {:.3}",
                        t + 50,
                        *a - origin,
                        *r - origin
                    );
                }
            }
            if let Some((n, d)) = off.first() {
                wrong.push(format!(
                    "{}: {} bones off, first {n} by {d}",
                    t + 50,
                    off.len()
                ));
            }
        }
        level.run_frame(t);
    }
    eprintln!("{} of {frames} frames off", wrong.len());
    assert!(frames > 20);
    assert!(
        wrong.is_empty(),
        "bones off retail's:\n{}",
        wrong.join("\n")
    );
}
