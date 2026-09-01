//! The playerstate under each movement input, diffed against a retail
//! capture pose by pose. `playerstate_ab` pins a player standing still, which
//! leaves every field the client predicts at zero and so cannot catch a field
//! we never write; this one holds each input until the state settles and
//! diffs what we claim to model.
//!
//! The fixture is `--net-probe --save-motion` against `tools/run_server.sh`,
//! and each pose carries the usercmd that produced it on a `!input` line, so
//! the replay below runs the capture's own script rather than a copy of it.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{PlayerState, UserCmd, NULL_USERCMD};
use vcod_common::net::protocol::{ENTITYNUM_WORLD, PROTOCOL_V1};

/// What this gate claims the server reproduces. The animation indices are
/// compared separately below, because the toggle bit makes the raw word
/// incomparable. Everything else in the playerstate is another system's: the
/// weapon fields need a weapon simulation that does not exist yet.
const MODELLED: &[&str] = &[
    "eFlags",
    "pm_flags",
    "viewHeightCurrent",
    "viewHeightTarget",
    "viewHeightLerpTarget",
    "viewHeightLerpDown",
    "groundEntityNum",
    // Retail transmits the standing box in every stance; the collision box
    // the mover uses is derived from the stance, not from these.
    "mins[2]",
    "maxs[2]",
];

// `leanf` is transmitted but not diffed here, for two measured reasons.
// The lean is clamped by a trace against nearby geometry, and the spawn is
// weighted-random, so two captures on the same map settle at different
// fractions (-1.0 and -0.65 on 2026-09-01); `playerstate_ab` excludes
// `origin` from equality for the same reason. And the retail server's
// `leanf` does not return to centre when the lean bit clears, nor follow a
// lean the other way -- it ramps once and stays -- so every pose after the
// first lean carries a stale value a correct simulation cannot reproduce.
// The client predicts its own lean; the field is transmitted so that
// prediction re-bases on a real value instead of a zero. The wire mapping is
// unit-tested in `spectate.rs` and the ramp rate in `pmove.rs`.

/// The anim channels compared here, by index rather than by raw word: bit 512
/// is the restart toggle and a settled pose can carry it either way (retail's
/// `stand_up` reads 634 on mp_carentan and 122 on mp_pavlov, the same standing
/// idle), so only the low 9 bits are a fact about the pose. The toggle is
/// checked separately, frame by frame, where it does mean something.
///
/// `torsoAnim` is not here: retail leaves it 0 in every settled pose of both
/// captures, so comparing it would assert nothing about this stage and would
/// fail at `jump_takeoff`, the one pose where retail's value (index 208) comes
/// from no clause the selection reaches.
const ANIM_FIELDS: &[&str] = &["legsAnim"];

/// Poses whose anim nothing derives yet, with the reason. Everything else is
/// compared. Emptying this is a later stage's job, not a silent widening of
/// this one.
const ANIM_GAPS: &[(&str, &str)] = &[
    (
        "turn_left",
        "the turn movetypes need a body yaw that lags the view, which no code models",
    ),
    ("turn_right", "same body yaw"),
];

/// `eFlags` bit for prone, so a pose can be told to have taken.
const EF_PRONE: i32 = 0x40;

/// A prone one side took and the other refused. Both movers now run the fit
/// check -- the body needs 54 units of clear space behind the facing -- so the
/// outcome depends on the geometry around the player, and the capture's spawn
/// is not the replay's: `_spawnlogic` picks weighted-random from the top third
/// of its candidates, which is why `playerstate_ab` excludes `origin` from
/// equality too. A pose the two disagree about is therefore not comparable and
/// is skipped; the fit check itself is unit-tested against known geometry in
/// `pmove.rs` (docs/research/cod11-mantle.md, "Prone").
fn prone_disagrees(pose: &Pose, ours: &PlayerState) -> bool {
    let asked = pose.input.wbuttons & vcod_common::net::msg::WBUTTON_PRONE != 0;
    if !asked {
        return false;
    }
    let retail = pose.fields.get("eFlags").is_some_and(|f| f & EF_PRONE != 0);
    let mine = ours.field_i32(&PROTOCOL_V1, "eFlags") & EF_PRONE != 0;
    retail != mine
}

/// The horizontal speed a pose settled at, from the two wire words both sides
/// carry.
fn horizontal_speed(x: i32, y: i32) -> f32 {
    f32::from_bits(x as u32).hypot(f32::from_bits(y as u32))
}

/// A pose one side moved through and the other stood still in. The spawn is
/// weighted-random, so the two runs stand in different places and one can be
/// pressed into geometry where the other is not -- `prone_disagrees` skips
/// poses for the same reason. A blocked player is idle to the animscript, so
/// the two are then animating different things and the pose is not
/// comparable.
fn movement_disagrees(pose: &Pose, ours: &PlayerState) -> bool {
    let eps = vcod_server::spectate::ANIM_IDLE_SPEED;
    let field = |name: &str| {
        *pose
            .fields
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not in the capture"))
    };
    let retail = horizontal_speed(field("velocity[0]"), field("velocity[1]"));
    let mine = horizontal_speed(
        ours.field_i32(&PROTOCOL_V1, "velocity[0]"),
        ours.field_i32(&PROTOCOL_V1, "velocity[1]"),
    );
    (retail > eps) != (mine > eps)
}

/// How long each pose is held, in server frames of 50 ms. The capture holds
/// every pose at least 1.5 s, so this settles the same lerps.
const HOLD_FRAMES: usize = 60;

/// The same server the standing gate configures, so both diff against
/// captures taken on the same settings.
fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

struct Pose {
    label: String,
    input: UserCmd,
    sample: Sample,
    fields: BTreeMap<String, i32>,
}

/// How the capture took a pose: after the input settled, at the first frame
/// off the ground, or at the first frame back on it. The replay has to sample
/// the same way -- the landing anim is over in 100 ms, so a settled sample of
/// `land` would read the idle that follows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sample {
    Settled,
    Airborne,
    Grounded,
}

fn parse_fixture(text: &str) -> Vec<Pose> {
    let mut poses: Vec<Pose> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(label) = line
            .strip_prefix("[pose ")
            .and_then(|l| l.strip_suffix(']'))
        {
            poses.push(Pose {
                label: label.to_string(),
                input: NULL_USERCMD,
                sample: Sample::Settled,
                fields: BTreeMap::new(),
            });
            continue;
        }
        let pose = poses.last_mut().expect("a field line before any [pose]");
        if let Some(rest) = line.strip_prefix("!input ") {
            for kv in rest.split_whitespace() {
                let (k, v) = kv.split_once('=').expect("!input takes key=value pairs");
                if k == "sample" {
                    pose.sample = match v {
                        "airborne" => Sample::Airborne,
                        "grounded" => Sample::Grounded,
                        _ => Sample::Settled,
                    };
                    continue;
                }
                let v: i32 = v.parse().expect("an integer in !input");
                match k {
                    "buttons" => pose.input.buttons = v as u8,
                    "wbuttons" => pose.input.wbuttons = v as u8,
                    "up" => pose.input.up = v as i8,
                    "forward" => pose.input.forward = v as i8,
                    "right" => pose.input.right = v as i8,
                    // ANGLE2SHORT units; the capture turns to pin the fields
                    // that only move when the view does.
                    "yaw" => pose.input.angles[1] = v,
                    other => panic!("unknown !input key {other}"),
                }
            }
            continue;
        }
        let (name, value) = line.rsplit_once(' ').expect("a `name value` field line");
        pose.fields
            .insert(name.to_string(), value.parse().expect("an i32 field value"));
    }
    poses
}

/// A replay of the capture's script against our server: one playerstate per
/// pose, sampled the way the probe sampled retail's, and every frame in
/// between. The frames are what a per-frame invariant is checked over; a pose
/// sample cannot see the transitions inside a pose, and there are several --
/// a run can cross a ledge and animate the fall.
#[derive(Default)]
struct Replay {
    poses: Vec<PlayerState>,
    frames: Vec<PlayerState>,
}

/// Runs the capture's script against our server and keeps the playerstate at
/// the end of each pose, the way the probe kept retail's.
fn ours(map: &str, poses: &[Pose], join: (&str, &str), fs: vcod_common::pk3::Pk3Fs) -> Replay {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(fs).expect("load the scripts");

    let q = Rc::new(RefCell::new(Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, join.0, join.1);

    let p = &PROTOCOL_V1;
    let mut out = Replay::default();
    for pose in poses {
        let mut sampled = None;
        for _ in 0..HOLD_FRAMES {
            now += Duration::from_millis(50);
            cl.send_frame(&pose.input);
            common::step(&mut sv, &q, &mut cl, now);
            let Some(ps) = cl.snapshots().newest().map(|s| s.ps.clone()) else {
                continue;
            };
            // An airborne pose is over at the first frame off the ground;
            // holding it out would sample the landing instead. A grounded one
            // ends at the first frame back on it, which is where the landing
            // anim is.
            let grounded = ps.field_i32(p, "groundEntityNum") == ENTITYNUM_WORLD as i32;
            let over = match pose.sample {
                Sample::Settled => false,
                Sample::Airborne => !grounded,
                Sample::Grounded => grounded,
            };
            out.frames.push(ps.clone());
            sampled = Some(ps);
            if over {
                break;
            }
        }
        out.poses.push(sampled.expect("no snapshot after a pose"));
    }
    out
}

fn check(map: &str, gametype: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-motion.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let poses = parse_fixture(&text);
    assert!(!poses.is_empty(), "{path} has no poses");

    // The capture's own header, so the replay asks the menus for the team and
    // weapon retail was holding, not a literal that can drift from it.
    let team = common::header_value(&text, "joined", &path).to_string();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let mine = ours(map, &poses, (&team, &weapon), fs).poses;
    let p = &PROTOCOL_V1;
    let mut diffs = Vec::new();
    let mut skipped = Vec::new();
    for (pose, ps) in poses.iter().zip(&mine) {
        if prone_disagrees(pose, ps) {
            skipped.push(pose.label.as_str());
            continue;
        }
        for name in MODELLED {
            let retail = *pose
                .fields
                .get(*name)
                .unwrap_or_else(|| panic!("{name} is not in the capture"));
            let ours = ps.field_i32(p, name);
            if retail != ours {
                diffs.push(format!(
                    "{}: {name} retail {retail} ({}), ours {ours} ({})",
                    pose.label,
                    f32::from_bits(retail as u32),
                    f32::from_bits(ours as u32),
                ));
            }
        }
    }
    // A capture where retail refused every prone it was asked for would pass
    // this gate while pinning nothing about prone at all.
    assert!(
        skipped.len() < poses.len(),
        "{path}: every pose was skipped; the capture pins nothing"
    );
    assert!(
        diffs.is_empty(),
        "{} of {} pose/field pairs differ from retail on {map}:\n  {}",
        diffs.len(),
        poses.len() * MODELLED.len(),
        diffs.join("\n  ")
    );
}

#[test]
fn the_moving_playerstate_matches_retail_on_mp_pavlov() {
    check("mp_pavlov", "dm");
}

#[test]
fn the_moving_playerstate_matches_retail_on_mp_carentan() {
    check("mp_carentan", "dm");
}

/// The animation index under each pose, against retail's. `check` diffs the
/// fields the mover owns; this diffs the one the animscript machine owns.
fn check_anims(map: &str, gametype: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-motion.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let poses = parse_fixture(&text);
    let team = common::header_value(&text, "joined", &path).to_string();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let mine = ours(map, &poses, (&team, &weapon), fs).poses;
    let p = &PROTOCOL_V1;
    let mut diffs = Vec::new();
    let mut skipped = Vec::new();
    for (pose, ps) in poses.iter().zip(&mine) {
        if prone_disagrees(pose, ps)
            || movement_disagrees(pose, ps)
            || ANIM_GAPS.iter().any(|(g, _)| *g == pose.label)
        {
            skipped.push(pose.label.as_str());
            continue;
        }
        for name in ANIM_FIELDS {
            let retail = *pose
                .fields
                .get(*name)
                .unwrap_or_else(|| panic!("{name} is not in the capture"));
            let ours = ps.field_i32(p, name);
            if retail & 511 != ours & 511 {
                diffs.push(format!(
                    "{}: {name} retail index {}, ours {}",
                    pose.label,
                    retail & 511,
                    ours & 511
                ));
            }
        }
    }
    // A capture the replay disagreed with everywhere would pass this gate
    // while pinning nothing about the animation at all.
    assert!(
        skipped.len() < poses.len(),
        "{path}: every pose was skipped; the capture pins nothing"
    );
    assert!(
        diffs.is_empty(),
        "{} anim indices differ from retail on {map}:\n  {}",
        diffs.len(),
        diffs.join("\n  ")
    );
}

/// The toggle bit is a restart signal, not a value: it must flip exactly when
/// the index changes and hold when it does not, or a client either restarts a
/// looping anim every frame or never restarts a repeated one. Frame by frame,
/// because a pose sample cannot see the anims a pose passes through.
fn check_anim_restarts(map: &str, gametype: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-motion.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let poses = parse_fixture(&text);
    let team = common::header_value(&text, "joined", &path).to_string();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let frames = ours(map, &poses, (&team, &weapon), fs).frames;
    let p = &PROTOCOL_V1;
    let mut bad = Vec::new();
    let mut changes = 0usize;
    for (i, w) in frames.windows(2).enumerate() {
        for name in ANIM_FIELDS {
            let a = w[0].field_i32(p, name);
            let b = w[1].field_i32(p, name);
            let index_changed = a & 511 != b & 511;
            changes += usize::from(index_changed);
            if index_changed != (a & 512 != b & 512) {
                bad.push(format!(
                    "frame {i}: {name} index {} -> {}, toggle {} -> {}",
                    a & 511,
                    b & 511,
                    a & 512 != 0,
                    b & 512 != 0
                ));
            }
        }
    }
    // A replay that never changed anim would pass this without testing it.
    assert!(changes > 4, "only {changes} anim changes in the replay");
    assert!(
        bad.is_empty(),
        "the restart toggle does not track the index change on {map}:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn the_anim_indices_match_retail_on_mp_carentan() {
    check_anims("mp_carentan", "dm");
}

#[test]
fn the_anim_indices_match_retail_on_mp_pavlov() {
    check_anims("mp_pavlov", "dm");
}

#[test]
fn the_restart_toggle_tracks_the_index_on_mp_carentan() {
    check_anim_restarts("mp_carentan", "dm");
}
