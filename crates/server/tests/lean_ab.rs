//! The leaned muzzle against the retail capture `mp_carentan-tdm-lean.txt`:
//! scoped kar98k_sniper_mp shots at a wall about 400 units out, level,
//! leaning left, leaning right and crouched leaning left, replayed through
//! our server from the spot the capture's header records.
//!
//! The scoped sniper rifle has `adsSpread 0`, so a round leaves the muzzle
//! along the view plus the idle sway (combat doc 15). Taking the swayed ray
//! through the unleaned eye, what is left of the impact across that ray is
//! the muzzle's offset from the eye: the lean, through `G_AddLean` and the
//! truncation `FireWeapon` puts on it (combat doc 2.1). That offset, and
//! `leanf` at the shot, are compared shot for shot; the distance along the
//! ray is not, since it is the wall's and the sway's.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use common::{parse_fixture, replay};
use glam::Vec3;
use vcod_common::net::events::ET_EVENTS;
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::trajectory::Trajectory;

const EV_BULLET_HIT_SMALL: i32 = 173;
const EV_BULLET_HIT_LARGE: i32 = 174;
/// How far our muzzle offset may sit from retail's: none, measured
/// 2026-11-14. Each step opens on retail's level clock, so both sides
/// sway alike and every round strikes retail's whole-unit impact. Before
/// that ours ran about 13.7 s behind and the offsets, each taken out at its
/// side's own clock through this file's sway model, read 1.03 apart.
const OFFSET_TOL: f32 = 0.0;
/// How long after a tap step opens our replay's round leaves: the tap's
/// cmd runs in the step's first frame and its event rides that frame's
/// snapshot.
const SHOT_LAG_MS: i32 = common::FRAME_MS as i32;
const LEANF_TOL: f32 = 0.01;
/// `adsIdleAmount` of the scoped kar98k, and `idleCrouchFactor`.
const ADS_IDLE: f32 = 45.0;
const IDLE_CROUCH: f32 = 0.2;

/// One shot as its impact reads, on either side.
#[derive(Debug)]
struct Shot {
    server_time: i32,
    at: Vec3,
    origin: Vec3,
    view_height: f32,
    leanf: f32,
    view: [f32; 3],
}

impl Shot {
    /// The impact's offset across the swayed ray through the unleaned eye
    /// (origin and eye truncated as `FireWeapon` truncates them).
    fn muzzle_offset(&self) -> Vec3 {
        let o = self.origin.trunc();
        let eye = Vec3::new(o.x, o.y, (o.z + self.view_height).trunc());
        // A crouched shot has eased its sway to the crouch factor: every
        // crouched step holds 2.5 s before it taps.
        let amount = ADS_IDLE
            * if self.view_height < 50.0 {
                IDLE_CROUCH
            } else {
                1.0
            };
        let t = self.server_time as f64;
        let pitch = self.view[0] + 0.01 * amount * (t * 0.001).sin() as f32;
        let yaw = self.view[1] + 0.01 * amount * (t * 0.0007).sin() as f32;
        let (p, y) = (pitch.to_radians(), yaw.to_radians());
        let d = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
        let v = self.at - eye;
        v - d * v.dot(d)
    }
}

fn vec3(raw: &str) -> Vec3 {
    let v: Vec<f32> = raw.split(',').map(|c| c.parse().unwrap()).collect();
    Vec3::new(v[0], v[1], v[2])
}

/// Retail's shots: the first `!impact` line of each temp entity.
fn retail_shots(text: &str) -> Vec<Shot> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("!impact ") else {
            continue;
        };
        let kv: std::collections::BTreeMap<&str, &str> = rest
            .split_whitespace()
            .filter_map(|t| t.split_once('='))
            .collect();
        if !seen.insert((kv["ent"], kv["at"])) {
            continue;
        }
        let view = vec3(kv["viewangles"]);
        out.push(Shot {
            server_time: kv["serverTime"].parse().unwrap(),
            at: vec3(kv["at"]),
            origin: vec3(kv["ps_origin"]),
            view_height: kv["viewHeightCurrent"].parse().unwrap(),
            leanf: kv["leanf"].parse().unwrap(),
            view: view.to_array(),
        });
    }
    out
}

/// Ours: each bullet-impact temp entity the first snapshot it rides.
fn our_shots(steps: &[Vec<common::Sample>]) -> Vec<Shot> {
    let p = &PROTOCOL_V1;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for s in steps.iter().flatten() {
        for (&num, e) in &s.entities {
            let ev = e.field_i32(p, "eType") - ET_EVENTS;
            if ev != EV_BULLET_HIT_SMALL && ev != EV_BULLET_HIT_LARGE {
                continue;
            }
            let at = Trajectory::read(e, p, "pos").base;
            if !seen.insert((num, at.to_array().map(f32::to_bits))) {
                continue;
            }
            out.push(Shot {
                server_time: s.server_time,
                at,
                origin: Vec3::from(s.ps.origin(p)),
                view_height: s.ps.field_f32(p, "viewHeightCurrent"),
                leanf: s.ps.field_f32(p, "leanf"),
                view: s.ps.viewangles(p),
            });
        }
    }
    out
}

#[test]
fn a_leaning_scoped_shot_leaves_from_retails_muzzle() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let (map, gametype) = ("mp_carentan", "tdm");
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-lean.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let team = common::header_value(&text, "joined", &path).to_string();
    let held =
        vcod_server::configstrings::weapon_index(&weapon).expect("the joined weapon in CS 7");
    let mut steps = parse_fixture(&text, held as u8);
    let retail = retail_shots(&text);
    assert_eq!(retail.len(), 7, "the capture's seven shots");
    // Each fire step opens where its round leaves on retail's level clock.
    // Each step opens on retail's level clock, so the sway matches: where
    // retail's first trace did, and a tap step a frame before its round
    // left, since retail's tap missed the step's first frame once (the
    // probe's first snapshot came 16 ms into `scope_right_1`).
    let mut shots = retail.iter();
    for step in &mut steps {
        step.open_at = if step.pulses > 0 {
            shots.next().map(|s| s.server_time - SHOT_LAG_MS)
        } else {
            step.trace.first().and_then(|t| t.server_time)
        };
    }

    // The probe aimed every cmd at one absolute yaw, so the view every shot
    // reads is where the replay faces; the spawn's own facing is not.
    let origin = retail[0].origin.to_array();
    let mine = replay(
        map,
        gametype,
        &steps,
        (&team, &weapon),
        fs,
        Some((origin, retail[0].view[1])),
    );
    let ours = our_shots(&mine);
    assert_eq!(ours.len(), retail.len(), "one impact per tap: {ours:#?}");

    let mut bad = Vec::new();
    for (i, (r, o)) in retail.iter().zip(&ours).enumerate() {
        let (rm, om) = (r.muzzle_offset(), o.muzzle_offset());
        let line = format!(
            "shot {i} at {} / {}: leanf retail {} ours {}, offset retail {rm:.2} ours {om:.2}",
            r.server_time, o.server_time, r.leanf, o.leanf
        );
        eprintln!("{line}");
        if r.server_time != o.server_time
            || r.at != o.at
            || (r.leanf - o.leanf).abs() > LEANF_TOL
            || (rm - om).length() > OFFSET_TOL
        {
            bad.push(line);
        }
    }
    assert!(
        bad.is_empty(),
        "shots off retail's muzzle:\n{}",
        bad.join("\n")
    );
}

/// When a step's `leanf` first reads the value it ends on, ms into the step,
/// and that value.
fn settle(samples: impl Iterator<Item = (i64, f32)>) -> Option<(i64, f32)> {
    let all: Vec<(i64, f32)> = samples.collect();
    let last = all.last()?.1;
    let first = all
        .iter()
        .position(|&(_, v)| (v - last).abs() < LEANF_TOL)?;
    // A settled step reads its final value from the start.
    Some((if first == 0 { 0 } else { all[first].0 }, last))
}

/// The lean ramp: `PM_UpdateLean` moves `leanf` toward the held side about
/// 0.14 a frame and back to the centre about 0.18, and a crouched lean stops
/// at half. Each ramp step of the capture has to settle on retail's value
/// within two frames of retail's time.
#[test]
fn the_lean_ramps_and_settles_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let (map, gametype) = ("mp_carentan", "tdm");
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-lean.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let team = common::header_value(&text, "joined", &path).to_string();
    let held =
        vcod_server::configstrings::weapon_index(&weapon).expect("the joined weapon in CS 7");
    let steps = parse_fixture(&text, held as u8);
    let retail = retail_shots(&text);
    let mine = replay(
        map,
        gametype,
        &steps,
        (&team, &weapon),
        fs,
        Some((retail[0].origin.to_array(), retail[0].view[1])),
    );
    // The `!trace` lines carry leanf; `parse_fixture` does not keep it.
    let mut retail_lean: std::collections::BTreeMap<String, Vec<(i64, f32)>> = Default::default();
    let mut label = String::new();
    for line in text.lines() {
        if let Some(l) = line
            .strip_prefix("[step ")
            .and_then(|l| l.strip_suffix(']'))
        {
            label = l.to_string();
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let kv: std::collections::BTreeMap<&str, &str> = rest
                .split_whitespace()
                .filter_map(|t| t.split_once('='))
                .collect();
            retail_lean
                .entry(label.clone())
                .or_default()
                .push((kv["ms"].parse().unwrap(), kv["leanf"].parse().unwrap()));
        }
    }
    let p = &PROTOCOL_V1;
    let mut bad = Vec::new();
    for (step, samples) in steps.iter().zip(&mine) {
        if !["lean_left", "lean_right", "lean_off", "crouch_lean_left"]
            .contains(&step.label.as_str())
        {
            continue;
        }
        let r = settle(retail_lean[&step.label].iter().copied()).unwrap();
        let o = settle(samples.iter().map(|s| (s.ms, s.ps.field_f32(p, "leanf")))).unwrap();
        let line = format!(
            "{}: retail settles on {} at {} ms, ours on {} at {} ms",
            step.label, r.1, r.0, o.1, o.0
        );
        eprintln!("{line}");
        if (r.1 - o.1).abs() > LEANF_TOL || (r.0 - o.0).abs() > 2 * common::FRAME_MS {
            bad.push(line);
        }
    }
    assert!(
        bad.is_empty(),
        "lean ramps off retail's:\n{}",
        bad.join("\n")
    );
}
