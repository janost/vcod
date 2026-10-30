//! `cod4_shot_aim_ab`: the shot aim capture (`play-1.7-sway-target.txt`,
//! `-shooter.txt`, `-hits.txt`, docs/research/cod4-combat.md 3.2) on ours.
//! Retail ran the `shot_aim` probe gsc, whose damage callback logs each
//! round's point and direction and does no damage; its shooter fired an
//! AK-47 at an idle target sighted while strafing, leaning left and right
//! and swinging its view, from the hip while strafing, and crouched. Both
//! probes join [`Cod4Server`] in process on retail's clock with the same
//! gsc over the zones' scripts, each on its retail spawn spot and seed,
//! and from their spawns send their captures' own cmds. Every retail hit
//! is matched to ours by `getTime()`: the direction, and the point's
//! distance off ours across the round's line.
//!
//! The entity penetration capture (`play-1.7-pen-*.txt`, cod4-combat.md
//! 13.1) runs the same way under the `pen_ent` gsc on mp_backlot: a
//! sighted shooter fires six rounds at a target behind a destructible
//! sedan, and the car's damage notifies and the target's hits are
//! compared line for line. Needs `COD4_DIR`.
//!
//! `SHOT_AIM_REPORT=1` prints every row.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::wire_ab::{Probe, replay_schedule, retail, side};
use super::*;

/// One retail run of two `cod4_probe`s under a probe gsc.
struct Run {
    map: &'static str,
    /// The captures' prefix: `play-1.7-<name>-target.txt`, `-shooter.txt`.
    name: &'static str,
    /// The run's `srand` seed (`G_InitGame`), recovered off the two
    /// challenges (cod4-combat.md 10.9).
    seed: u32,
    /// The probe gsc's directory under `crates/cod4/tests/fixtures/gsc`.
    gsc: &'static str,
    /// Each probe's `--goto`, sent 3.1 s past its first snapshot.
    goto: [Option<&'static str>; 2],
    run_ms: i32,
    /// When the probes answered the class menu past their first
    /// snapshot, as the capture sent it.
    class_ms: i32,
}

/// The shot aim run: the shooter's last round at 67250; seed off the
/// challenges at draws 17 and 19 (`svs.time` 40500, 41000).
const SWAY: Run = Run {
    map: "mp_crash",
    name: "sway",
    seed: 2_054_641_269,
    gsc: "shot_aim",
    goto: [
        Some("setviewpos 207 2291 296"),
        Some("setviewpos 123 2110 300"),
    ],
    run_ms: 68_300,
    class_ms: 2550,
};

/// The damage kick run: the shot aim run under the `shot_kick` gsc, whose
/// shooter is hurt every 1.3 s from 10 s past its spawn; the last round
/// at 67500; seed off the challenges at draws 17 and 19 (`svs.time` 40650,
/// 41150).
const KICK: Run = Run {
    map: "mp_crash",
    name: "kick",
    seed: 2_112_315_757,
    gsc: "shot_kick",
    goto: SWAY.goto,
    run_ms: 68_300,
    class_ms: 2550,
};

/// The entity penetration run: the last round at 49250; seed off the
/// challenges at draws 12 and 14 (`svs.time` 40550, 41050).
const PEN: Run = Run {
    map: "mp_backlot",
    name: "pen",
    seed: 2_056_099_660,
    gsc: "pen_ent",
    goto: [None, None],
    run_ms: 50_300,
    class_ms: 2550,
};

/// The script model run: the entity penetration run's two probes under
/// the `model_shot` gsc, which stands a script model of the sedan's model
/// between them in five phases; the lines up to 69400, before the
/// target's capture ends (its last snapshot at 69600); seed off the
/// challenges at draws 12 and 14 (`svs.time` 41200, 41700).
const MODELS: Run = Run {
    map: "mp_backlot",
    name: "models",
    seed: 2_113_691_329,
    gsc: "model_shot",
    goto: [None, None],
    run_ms: 69_500,
    // 1550 ms past the team, a frame later than the others'.
    class_ms: 2600,
};

/// One `PROBE <ms> hit` line: the time, the point and the direction.
#[derive(Clone, Copy, Debug)]
struct HitLine {
    ms: i32,
    point: glam::DVec3,
    dir: glam::DVec3,
}

fn hit_lines<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<i32, HitLine> {
    lines
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let at = w.iter().position(|&x| x == "PROBE")?;
            let w = &w[at..];
            if w.get(2) != Some(&"hit") || w.len() < 13 {
                return None;
            }
            let n = |i: usize| w[i].parse::<f64>().unwrap();
            let ms = w[1].parse().unwrap();
            Some((
                ms,
                HitLine {
                    ms,
                    point: glam::DVec3::new(n(7), n(8), n(9)) / 1000.0,
                    dir: glam::DVec3::new(n(10), n(11), n(12)) / 1_000_000.0,
                },
            ))
        })
        .collect()
}

/// The two probes of `run` against `Cod4Server`: ours' script log and
/// script errors.
fn ours(install: &vcod_cod4::install::Install, run: &Run) -> (Vec<String>, Vec<String>) {
    let (target, shooter) = (
        format!("{}-target", run.name),
        format!("{}-shooter", run.name),
    );
    let rt = side(&retail(&target));
    let rs = side(&retail(&shooter));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: run.map.into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    for (k, v) in vcod_common::net::connectionless::info_pairs(&cs0) {
        if !matches!(k, "protocol" | "shortversion" | "fs_game") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_cvar("sv_cheats", "1");
    sv.pin_game_seed(run.seed);
    sv.set_script_overrides(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cod4/tests/fixtures/gsc")
            .join(run.gsc),
    );
    sv.load(Some(install)).unwrap();
    // Each spawn on its twin's spot, and the spawn snapshot's time.
    let spawn = |s: &super::wire_ab::Side, slot: u32| {
        let f = s
            .snaps
            .iter()
            .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(slot))
            .expect("a spawn");
        let yaw = f.ps.get_f32("viewangles[1]").unwrap_or(0.0);
        let origin = ["origin[0]", "origin[1]", "origin[2]"].map(|n| f.ps.get_f32(n).unwrap());
        (f.server_time, Some((origin, [0.0, yaw, 0.0])))
    };
    let (target_spawn, target_pin) = spawn(&rt, 0);
    let (shooter_spawn, shooter_pin) = spawn(&rs, 1);
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(target_pin);
    pins[1].push_back(shooter_pin);
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    // The answers past each first snapshot, as the probes sent them.
    let answers = |team: &'static str, goto: Option<&'static str>| {
        let mut a = vec![(1050, team), (run.class_ms, "mr {sid} 15 assault_mp,0")];
        a.extend(goto.map(|g| (3100, g)));
        a
    };
    let mut probes = [
        Probe::new(
            "target",
            0,
            28961,
            rt.snaps[0].server_time,
            &answers("mr {sid} 8 allies", run.goto[0]),
        ),
        Probe::new(
            "shooter",
            1,
            28962,
            rs.snaps[0].server_time,
            &answers("mr {sid} 8 axis", run.goto[1]),
        ),
    ];
    probes[0].replay = replay_schedule(&retail(&target), target_spawn);
    probes[0].replay_from = target_spawn;
    probes[1].replay = replay_schedule(&retail(&shooter), shooter_spawn);
    probes[1].replay_from = shooter_spawn;
    let mut now = Instant::now();
    while sv.core.sv_time_ms < run.run_ms {
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
    let level = sv.game.level.as_ref().unwrap();
    (level.rt.host.script_log.clone(), level.rt.aborts())
}

/// The round's line off `h`: how far `other`'s point sits across it.
fn off_line(h: &HitLine, other: &HitLine) -> f64 {
    let d = other.point - h.point;
    (d - h.dir * d.dot(h.dir)).length()
}

/// Every retail hit has its twin on ours at the same time, leaving along
/// retail's direction (a thousandth of a degree, the log's millionths) through retail's line
/// (a hundredth of a unit): the eye's bob and lean, the view the last cmd
/// left, the sway, the movement tilt, the idle drift and the gun spring.
#[test]
fn rounds_leave_along_retails_line() {
    rounds_match(&SWAY);
}

/// [`rounds_leave_along_retails_line`] with the shooter hurt as it fires:
/// the damage kick of each hurt (from ahead, from the right, from no
/// direction) turns the next half second's rounds as retail's.
#[test]
fn hurt_rounds_leave_along_retails_damage_kick() {
    rounds_match(&KICK);
}

fn rounds_match(run: &Run) {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let path = format!(
        "{}/../cod4/tests/fixtures/net/play-1.7-{}-hits.txt",
        env!("CARGO_MANIFEST_DIR"),
        run.name
    );
    let text = std::fs::read_to_string(path).unwrap();
    let retail = hit_lines(text.lines());
    let (log, aborts) = ours(&install, run);
    assert!(aborts.is_empty(), "{aborts:?}");
    let ours = hit_lines(log.iter().map(String::as_str));
    let report = std::env::var_os("SHOT_AIM_REPORT").is_some();
    let (mut worst_deg, mut worst_off) = (0f64, 0f64);
    let mut missing = Vec::new();
    for (ms, r) in &retail {
        let Some(o) = ours.get(ms) else {
            missing.push(*ms);
            continue;
        };
        let deg = r.dir.angle_between(o.dir).to_degrees();
        let off = off_line(r, o);
        worst_deg = worst_deg.max(deg);
        worst_off = worst_off.max(off);
        if report {
            eprintln!(
                "{} deg {deg:.4} off {off:.3} retail {:.3?} {:.5?} ours {:.3?} {:.5?}",
                r.ms, r.point, r.dir, o.point, o.dir
            );
        }
    }
    let extra: Vec<_> = ours.keys().filter(|k| !retail.contains_key(k)).collect();
    if report {
        eprintln!(
            "{} retail hits, {} ours; missing {missing:?} extra {extra:?}; worst {worst_deg:.4} deg \
             {worst_off:.3} units",
            retail.len(),
            ours.len()
        );
    }
    assert!(missing.is_empty(), "retail hits ours lacks: {missing:?}");
    assert!(extra.is_empty(), "hits retail lacks: {extra:?}");
    assert!(worst_deg < 0.001, "worst direction {worst_deg} degrees");
    assert!(
        worst_off < 0.01,
        "worst point {worst_off} units off the line"
    );
}

/// The `PROBE` lines past the placements, each without its log prefix.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter_map(|l| l.find("PROBE").map(|i| l[i..].trim().to_string()))
        .filter(|l| {
            l.split_whitespace()
                .nth(2)
                .is_some_and(|w| matches!(w, "hit" | "car" | "ent"))
        })
        .collect()
}

/// A round into the sedan's near window stops there (the glass breaks
/// after three); once the window is gone the next rounds strike the far
/// door from inside and go on to the target at the damage left, flagged
/// `iDFLAGS_PENETRATION`. Every car notify and hit of retail's, line for
/// line: the time, the damage, the part's effect and the point to the
/// thousandth.
#[test]
fn rounds_go_through_a_sedan_as_on_retail() {
    lines_match(&PEN);
}

/// Rounds stop on a script model whether or not it is open to damage,
/// raised or not (no line while one stands); on one opened to damage
/// they strike its attached model as well, ignoreCollision or not, and
/// its notify names the attachment's model and tag and the struck bone.
/// Every line of retail's, as [`rounds_go_through_a_sedan_as_on_retail`].
#[test]
fn rounds_stop_on_script_models_as_on_retail() {
    lines_match(&MODELS);
}

fn lines_match(run: &Run) {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let path = format!(
        "{}/../cod4/tests/fixtures/net/play-1.7-{}-hits.txt",
        env!("CARGO_MANIFEST_DIR"),
        run.name
    );
    let text = std::fs::read_to_string(path).unwrap();
    let before = |l: &String| {
        l.split_whitespace()
            .nth(1)
            .and_then(|t| t.parse::<i32>().ok())
            .is_some_and(|t| t < run.run_ms)
    };
    let retail: Vec<String> = probe_lines(text.lines())
        .into_iter()
        .filter(before)
        .collect();
    let (log, aborts) = ours(&install, run);
    assert!(aborts.is_empty(), "{aborts:?}");
    let ours = probe_lines(log.iter().map(String::as_str));
    if std::env::var_os("SHOT_AIM_REPORT").is_some() {
        for l in log.iter().filter(|l| l.contains("PROBE")) {
            eprintln!("ours   {}", l.trim());
        }
        for l in &retail {
            eprintln!("retail {l}");
        }
    }
    assert_eq!(
        ours.len(),
        retail.len(),
        "ours {ours:#?} retail {retail:#?}"
    );
    for (o, r) in ours.iter().zip(&retail) {
        let (ow, rw): (Vec<&str>, Vec<&str>) = (
            o.split_whitespace().collect(),
            r.split_whitespace().collect(),
        );
        assert_eq!(ow.len(), rw.len(), "ours {o} retail {r}");
        // The point (thousandths) and the direction (millionths) within
        // 0.05 units and 1e-4: the gsc's placement off `anglesToRight`
        // differs from retail's below the log's thousandth.
        let coords = if rw[2] == "hit" { 7..13 } else { 5..8 };
        for (k, (a, b)) in ow.iter().zip(&rw).enumerate() {
            if coords.contains(&k) {
                let d = (a.parse::<i64>().unwrap() - b.parse::<i64>().unwrap()).abs();
                assert!(
                    d <= if k < coords.start + 3 { 50 } else { 100 },
                    "ours {o} retail {r}"
                );
            } else {
                assert_eq!(a, b, "ours {o} retail {r}");
            }
        }
    }
}
