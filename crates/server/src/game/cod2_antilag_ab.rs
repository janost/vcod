//! The CoD2 antilag A/B: retail's rewound victims replayed on vcod. The
//! capture in `crates/cod2/tests/fixtures/antilag/` (recipe in its header)
//! is a shooter whose cmd clock runs 100 ms behind, holding a ppsh's
//! trigger while `probe_antilag.gsc` sweeps his view over a strafing
//! target, the frame archive on. Each `PROBE damage` line ends with the
//! target's `origin` as the damage callback reads it: where
//! `G_AntiLagRewindClientPos` put him for the shot. The gate puts the
//! target's playerstate where retail's snapshot of each frame has it,
//! archives the frame, and at each round asks vcod's rewind for the
//! target at the round's lag time; then it traces the round's line through
//! the body standing there. Compared: the rewound origin and the hit
//! location. Needs `COD2_DIR`. `ANTILAG_REPORT=1` prints every round.

use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::pmove::PlayerState;

use super::cod2_combat_ab::{parse, weapon_from_wire, word};
use crate::game::cod2::Cod2Level;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod2/tests/fixtures/antilag"
);

/// How far the rewound origin may sit from retail's: `vtos`' six digits on
/// a coordinate near 2000.
const ORIGIN_TOLERANCE: f32 = 0.011;

/// The rounds whose hit location the replay still gets wrong, by frame
/// time, each with its section of cod2-combat.md.
const GAPS: &[(i32, &str)] = &[(
    16950,
    "9.5 the hit pose: still, a 0.4-unit corner of the right thumb's box for the torso",
)];

/// One round: (time, the target's live origin, retail's rewound origin,
/// ours, retail's hit location, ours).
struct Round {
    time: i32,
    live: Vec3,
    retail: Vec3,
    ours: Option<Vec3>,
    retail_loc: String,
    ours_loc: String,
}

/// The vectors in a log line, in order.
fn vectors(line: &str) -> Vec<Vec3> {
    line.split('(')
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
        .collect()
}

fn replay(fs: &Rc<vcod_common::pk3::Pk3Fs>) -> Vec<Round> {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let shooter = parse(&read("antilag"));
    let target = parse(&read("antilag-b"));
    let gsc = std::fs::read_to_string(format!("{FIXTURES}/probe_antilag.gsc")).unwrap();
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
        "probe_antilag",
        (
            "maps/mp/gametypes/probe_antilag".to_string(),
            gsc.to_string(),
        ),
        cvars,
        start - 2000,
    )
    .expect("load probe_antilag");
    assert!(level.rt.archive_on(), "the probe turns the archive on");
    level.connect(0, "target");
    level.run_frame(start - 1950);
    level.begin(0);
    let archive = |level: &mut Cod2Level, t: i32| {
        let world = level.world_entities(t);
        let roster = vec![level.client_state(0, "target")];
        level.archive_frame(&world, &roster);
    };
    let mut t = start - 1950;
    while t < start - 50 {
        t += 50;
        level.run_frame(t);
        archive(&mut level, t);
    }
    // The rounds: (time, the callback's origin, point, direction, hit
    // location, the lag time: `level.time` less the logged offset).
    let rounds: Vec<(i32, Vec3, Vec3, Vec3, String, i32)> = shooter
        .probes
        .iter()
        .filter_map(|l| l.strip_prefix("damage "))
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let v = vectors(l);
            let time: i32 = w[1].parse().unwrap();
            let offset: i32 = l
                .split(')')
                .nth(2)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            (time, v[2], v[0], v[1], w[6].to_string(), time - offset)
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
        archive(&mut level, t);
        for (_, retail, point, dir, loc, lag) in rounds.iter().filter(|r| r.0 == t) {
            let ours = level.rewound_origin(0, *lag).and_then(|r| r.at());
            // The target's packets sent by the one carrying the shot's
            // cmd, whole, then the shooter's: each resets the skeleton
            // cache. The shooter's clock runs 100 ms behind the target's.
            let sent = shooter.side.sent_at(*lag) + 100;
            super::cod2_hitpose_ab::run_packets(&mut level, &target.side, &mut done, sent);
            level.packet();
            let origins: HashMap<usize, Vec3> = ours.map(|o| (0, o)).into_iter().collect();
            let (a, b) = (*point - *dir * 64.0, *point + *dir * 64.0);
            let (_, client, hl) = level.round_trace(a, b, &origins);
            let ours_loc = match client {
                Some(_) => vcod_cod2::xmodel::HIT_LOCATIONS[usize::from(hl)].to_string(),
                None => "miss".to_string(),
            };
            out.push(Round {
                time: t,
                live: PlayerState::from_wire(&snap.ps).origin,
                retail: *retail,
                ours,
                retail_loc: loc.clone(),
                ours_loc,
            });
        }
    }
    out
}

#[test]
fn retail_rewinds_match_vcods() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let report = std::env::var_os("ANTILAG_REPORT").is_some();
    let rounds = replay(&fs);
    let mut wrong = Vec::new();
    let mut moved = 0;
    for r in &rounds {
        let line = format!(
            "{}: live {} retail {} {} ours {} {}",
            r.time,
            r.live,
            r.retail,
            r.retail_loc,
            r.ours.map_or("-".to_string(), |o| o.to_string()),
            r.ours_loc
        );
        if report {
            eprintln!("{line}");
        }
        let origin_off = r
            .ours
            .is_none_or(|o| (o - r.retail).abs().max_element() > ORIGIN_TOLERANCE);
        let gap = GAPS.iter().any(|g| g.0 == r.time);
        if origin_off || (r.ours_loc != r.retail_loc) != gap {
            wrong.push(line);
        }
        moved += usize::from(r.live.distance(r.retail) > 1.0);
    }
    eprintln!(
        "{} of {} rounds wrong, {moved} rewound off the live origin",
        wrong.len(),
        rounds.len()
    );
    assert!(moved > 10, "the capture rewinds {moved} rounds");
    assert!(
        wrong.is_empty(),
        "rounds off retail's:\n{}",
        wrong.join("\n")
    );
}
