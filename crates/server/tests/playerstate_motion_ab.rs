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

/// What this gate claims the server reproduces. Everything else in the
/// playerstate is another system's: the animation indices need the animscript
/// state machine `playerstate_ab`'s GAPS entry describes, and the weapon
/// fields need a weapon simulation that does not exist yet.
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
    /// How the capture took this pose: after the input settled, or at the
    /// first frame off the ground. The replay has to sample the same way.
    airborne: bool,
    fields: BTreeMap<String, i32>,
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
                airborne: false,
                fields: BTreeMap::new(),
            });
            continue;
        }
        let pose = poses.last_mut().expect("a field line before any [pose]");
        if let Some(rest) = line.strip_prefix("!input ") {
            for kv in rest.split_whitespace() {
                let (k, v) = kv.split_once('=').expect("!input takes key=value pairs");
                if k == "sample" {
                    pose.airborne = v == "airborne";
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

/// Runs the capture's script against our server and keeps the playerstate at
/// the end of each pose, the way the probe kept retail's.
fn ours(
    map: &str,
    poses: &[Pose],
    join: (&str, &str),
    fs: vcod_common::pk3::Pk3Fs,
) -> Vec<PlayerState> {
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
    let mut out = Vec::new();
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
            // holding it out would sample the landing instead.
            if pose.airborne && ps.field_i32(p, "groundEntityNum") != ENTITYNUM_WORLD as i32 {
                sampled = Some(ps);
                break;
            }
            sampled = Some(ps);
        }
        out.push(sampled.expect("no snapshot after a pose"));
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
    let mine = ours(map, &poses, (&team, &weapon), fs);
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
