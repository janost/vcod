//! The CoD2 antilag A/B: retail's rewound victims replayed on vcod. The
//! capture in `crates/cod2/tests/fixtures/antilag/` (recipe in its header)
//! is a shooter whose cmd clock runs 100 ms behind, holding a ppsh's
//! trigger while `probe_antilag.gsc` sweeps his view over a strafing
//! target, the frame archive on. Each `PROBE damage` line ends with the
//! target's `origin` as the damage callback reads it: where
//! `G_AntiLagRewindClientPos` put him for the shot. The gate puts the
//! target's playerstate where retail's snapshot of each frame has it,
//! archives the frame, and at each round asks vcod's rewind for the
//! target at the round's lag time; then it traces the round's line, from
//! the shooter's eye through the logged point, through the body standing
//! there. Compared: the rewound origin and the hit
//! location. Needs `COD2_DIR`. `ANTILAG_REPORT=1` prints every round.

use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::player::dobj::Mat;
use vcod_cod2::pmove::PlayerState;

use super::cod2_combat_ab::{parse, weapon_from_wire, word};
use crate::game::cod2::Cod2Level;

static FIXTURES: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../cod2/tests/fixtures/antilag");

/// How far the rewound origin may sit from retail's: `vtos`' six digits on
/// a coordinate near 2000.
const ORIGIN_TOLERANCE: f32 = 0.011;

/// The rounds whose hit location the replay still gets wrong, by frame
/// time, each with its section of cod2-combat.md.
const GAPS: &[(i32, &str)] = &[];

/// One round: (time, the target's live origin, retail's rewound origin,
/// ours, retail's hit location, ours, the body's bones the trace built, in
/// the body model's order).
struct Round {
    time: i32,
    live: Vec3,
    retail: Vec3,
    ours: Option<Vec3>,
    retail_loc: String,
    ours_loc: String,
    bones: Vec<Mat>,
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

/// The capture `name` (and `<name>-b`, the target's) replayed.
fn replay(fs: &Rc<vcod_common::pk3::Pk3Fs>, name: &str) -> Vec<Round> {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let shooter = parse(&read(name));
    let target = parse(&read(&format!("{name}-b")));
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
        let Some(snap) = target.side.ran_at(t) else {
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
        // The shooter's look-at, which the replay leaves out of the level,
        // builds the target once a frame (cod2-combat.md 9.5).
        if let Some(s) = shooter.side.ran_at(t) {
            level.look_trace_from(&PlayerState::from_wire(&s.ps));
        }
        archive(&mut level, t);
        for (_, retail, point, _, loc, lag) in rounds.iter().filter(|r| r.0 == t) {
            let ours = level.rewound_origin(0, *lag).and_then(|r| r.at());
            // The target's packets sent by the one carrying the shot's
            // cmd, whole, then the shooter's: each resets the skeleton
            // cache. The shooter's clock runs 100 ms behind the target's.
            let sent = shooter.side.sent_at(*lag) + 100;
            super::cod2_hitpose_ab::run_packets(&mut level, &target.side, &mut done, sent);
            level.packet();
            let origins: HashMap<usize, Vec3> = ours.map(|o| (0, o)).into_iter().collect();
            let (a, b) = round_line(shooter_eye(&shooter.side, t), *point);
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
                bones: level.built_body_mats(0).unwrap_or_default(),
            });
        }
    }
    out
}

/// A round's line, 64 units each way of its logged `point`, from the
/// shooter's `eye` through it. The log's direction is the view's forward
/// (`G_Damage`'s `dir`, cod2-combat.md 2.4), which the spread turns the
/// round off (2.3), so a line along it meets other boxes than the round's.
fn round_line(eye: Vec3, point: Vec3) -> (Vec3, Vec3) {
    let u = (point.as_dvec3() - eye.as_dvec3()).normalize().as_vec3();
    (point - u * 64.0, point + u * 64.0)
}

/// Where the shooter's rounds of the frame at `t` leave from: his eye on
/// the playerstate retail's snapshot of that frame has.
fn shooter_eye(side: &super::cod2_combat_ab::Side, t: i32) -> Vec3 {
    let s = side.ran_at(t).expect("the shooter's snapshot");
    Cod2Level::player_view_origin(&PlayerState::from_wire(&s.ps), t)
}

/// Every round of `rounds` off retail's, but those `gaps` names.
fn rounds_off(rounds: &[Round], gaps: &[(i32, &str)]) -> Vec<String> {
    let report = std::env::var_os("ANTILAG_REPORT").is_some();
    let mut wrong = Vec::new();
    for r in rounds {
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
        let gap = gaps.iter().any(|g| g.0 == r.time);
        if origin_off || (r.ours_loc != r.retail_loc) != gap {
            wrong.push(line);
        }
    }
    wrong
}

#[test]
fn retail_rewinds_match_vcods() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let rounds = replay(&fs, "antilag");
    let wrong = rounds_off(&rounds, GAPS);
    let moved = rounds
        .iter()
        .filter(|r| r.live.distance(r.retail) > 1.0)
        .count();
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

/// Retail's bones at each struck trace of the target (entity 0) in a
/// `tools/re/cod2_trace_gdb.py` dump, by level time, the last one of a
/// frame: the round's, after the shooter's look-at.
fn dumped_bones(dump: &str) -> HashMap<i32, HashMap<usize, [f32; 8]>> {
    dumped_traces(dump)
        .into_iter()
        .filter_map(|(t, mut d)| Some((t, d.pop()?.mats)))
        .collect()
}

/// How many of `ours` a trace's dumped `mats` name, and each whose quat or
/// translation is off theirs by a word. The trace builds only the bones
/// with a box and their parents.
pub(super) fn bones_off(ours: &[Mat], mats: &HashMap<usize, [f32; 8]>) -> (usize, Vec<String>) {
    let (mut n, mut off) = (0, Vec::new());
    for (&i, m) in mats {
        let Some(&o) = ours.get(i).filter(|b| **b != Mat::ZERO) else {
            continue;
        };
        n += 1;
        let words = o.quat.iter().chain(&o.trans).map(|c| c.to_bits());
        if !words.eq(m[..7].iter().map(|c| c.to_bits())) {
            let d = (0..7)
                .map(|c| {
                    let x = if c < 4 { o.quat[c] } else { o.trans[c - 4] };
                    (x - m[c]).abs()
                })
                .fold(0.0, f32::max);
            off.push(format!("bone {i} off {d:e}"));
        }
    }
    (n, off)
}

/// A struck trace of the target in a dump: `SV_PointTraceToEntity`'s
/// world line, the fraction `DObjTraceline` returned along it and the
/// bones it read.
pub(super) struct DumpedTrace {
    pub start: Vec3,
    pub end: Vec3,
    pub fraction: f32,
    pub mats: HashMap<usize, [f32; 8]>,
}

impl DumpedTrace {
    /// How far its contact sits from `p`.
    pub fn near(&self, p: Vec3) -> f32 {
        (self.start + (self.end - self.start) * self.fraction).distance(p)
    }
}

/// The struck traces of the target (entity 0) at each level time of a
/// `tools/re/cod2_trace_gdb.py` dump, in order: the shooter's look-at
/// first, the rounds after.
pub(super) fn dumped_traces(dump: &str) -> HashMap<i32, Vec<DumpedTrace>> {
    let bits = |s: &str| f32::from_bits(u32::from_str_radix(s, 16).unwrap());
    let vec = |t: &[&str]| Vec3::new(bits(t[0]), bits(t[1]), bits(t[2]));
    let mut out = HashMap::new();
    let mut line = (Vec3::ZERO, Vec3::ZERO);
    let mut cur: Option<(i32, i32, HashMap<usize, [f32; 8]>)> = None;
    for l in dump.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t[0] {
            "point" => line = (vec(&t[5..8]), vec(&t[8..11])),
            "trace" => cur = Some((t[3].parse().unwrap(), t[7].parse().unwrap(), HashMap::new())),
            "mat" => {
                let m: [f32; 8] = std::array::from_fn(|i| bits(t[2 + i]));
                cur.as_mut().unwrap().2.insert(t[1].parse().unwrap(), m);
            }
            "result" => {
                let (time, ent, mats) = cur.take().unwrap();
                if ent == 0 && !t[7].ends_with("0000") {
                    let (start, end) = line;
                    let fraction = bits(t[2]);
                    let d = DumpedTrace {
                        start,
                        end,
                        fraction,
                        mats,
                    };
                    out.entry(time).or_insert_with(Vec::new).push(d);
                }
            }
            _ => {}
        }
    }
    out
}

/// The captures taken under gdb, each with its trace dump
/// (`crates/cod2/tests/fixtures/trace/`, the headers the recipes):
/// `antilag-gdb` under `tools/re/cod2_trace_gdb.py`, `antilag-slow` under
/// `tools/re/cod2_gdb_all.py`, whose breakpoints slow the server enough
/// that it runs frames back to back with no snapshot between
/// (cod2-player-anims.md 7).
const GDB_CAPTURES: [&str; 2] = ["antilag-gdb", "antilag-slow"];

/// Each capture taken under gdb: every round strikes retail's bone, and the
/// body each round's trace built is the one retail's `DObjTraceline` read,
/// bone for bone, word for word.
#[test]
fn retail_rewinds_and_bones_match_vcods_under_gdb() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    for name in GDB_CAPTURES {
        let rounds = replay(&fs, name);
        let mut wrong = rounds_off(&rounds, &[]);
        let dump = std::fs::read_to_string(format!(
            "{}/../cod2/tests/fixtures/trace/{name}.txt",
            vcod_common::manifest_dir!()
        ))
        .unwrap();
        let retail = dumped_bones(&dump);
        let mut bones = 0;
        for r in &rounds {
            let Some(mats) = retail.get(&r.time) else {
                wrong.push(format!("{}: no dumped trace", r.time));
                continue;
            };
            let (n, off) = bones_off(&r.bones, mats);
            bones += n;
            wrong.extend(off.into_iter().map(|o| format!("{}: {o}", r.time)));
        }
        eprintln!("{name}: {} rounds, {bones} bones", rounds.len());
        assert!(rounds.len() >= 25 && bones > 1000, "{name}");
        assert!(
            wrong.is_empty(),
            "{name} off retail's:\n{}",
            wrong.join("\n")
        );
    }
}
