//! `BG_CheckProneValid`, the prone entry, the prone view and pitch.

use super::angles::{
    angle_delta, angle_normalize180, angle_shorts, angle_subtract, delta_stored,
    normalize180_accurate, normalize360_accurate, short_normalize180, short_normalize360,
    vectopitch, view_deg, view_rad,
};
use super::stance::push_event;
use super::state::{MoveRules, PlayerState, PmEvent, PmInput, Stance};
use super::{DIVE_HEIGHT_LOW, DIVE_HEIGHT_STAND, HALF_WIDTH, HEIGHT_PRONE, MIN_WALK_NORMAL};
use crate::collision::MASK_PLAYERSOLID;
use crate::movetrace::{MASK_DEADSOLID, MoveWorld};
use crate::net::event_ids::EV_STANCE_FORCE_CROUCH;
use crate::x87;
use glam::Vec3;

/// Prone tunables, from the retail server: `bg_prone_yawcap` 85 and
/// `bg_prone_softyawedge` 1 are cvars, the 55 deg/s swing rate and the
/// 54-unit body clearance are rodata. The clearance is traced with a
/// +/-6 box straight behind the facing, which is the space a body needs to
/// lie down in (docs/research/cod11-mantle.md, "Prone").
pub const PRONE_YAWCAP: f32 = 85.0;
/// The body only starts turning once the view is this far off it.
pub const PRONE_SOFT_EDGE: f32 = PRONE_YAWCAP - PRONE_SOFT_MARGIN;
const PRONE_SOFT_MARGIN: f32 = 5.0;
/// United Offensive's `bg_lmg_yawcap` default, the cap round an LMG's
/// `proneDirection` (coduo-movement.md 7).
pub const LMG_YAWCAP: f32 = 55.0;

pub const PRONE_SWING_DEG_PER_SEC: f32 = 55.0;
/// How far the prone view may pitch off `proneTorsoPitch`, rodata 0x70c94
/// and 0x70c98 (`PM_UpdateViewAngles`).
pub const PRONE_PITCHCAP: f32 = 45.0;
/// The rate `proneDirectionPitch` and `proneTorsoPitch` ease toward the
/// ground's pitch, rodata 0x70ca4 (`PM_UpdatePronePitch`).
pub const PRONE_PITCH_DEG_PER_SEC: f32 = 70.0;
pub const PRONE_BODY_LENGTH: f32 = 54.0;
const PRONE_BODY_HALF_BOX: f32 = 6.0;

/// `fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`: the chest's height off the
/// origin and the pitches of the torso and the legs along the ground, in
/// degrees, wire convention.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProneBody {
    pub torso_height: f32,
    pub torso_pitch: f32,
    pub waist_pitch: f32,
}

/// What [`check_prone`] is asked: `entry` is `PM_CheckDuck`'s first-frame
/// call, which also needs room for the prone box where the player stands;
/// `ground` asks for the ground samples, which every caller passes as
/// "on the ground" but `PM_VerifyPronePosition`, which always does.
/// `length` is 1.5's body length argument, the sweep's reach plus the half
/// box: 60 everywhere but the prone view's swing (1.1's fixed sweep of
/// [`PRONE_BODY_LENGTH`] is the same 60).
#[derive(Clone, Copy)]
pub struct ProneQuery {
    pub origin: Vec3,
    pub yaw_deg: f32,
    pub entry: bool,
    pub ground: bool,
    pub length: f32,
    pub rules: MoveRules,
}

/// [`ProneQuery::length`] for every caller but the 1.5 swing.
pub const PRONE_CHECK_LENGTH: f32 = PRONE_BODY_LENGTH + PRONE_BODY_HALF_BOX;

/// `BG_CheckProneValid` (`game.mp.i386.so` 0x2d428, 1.5 0x1cf58;
/// docs/research/cod11-mantle.md, "The ground samples", and
/// cod15-movement.md 4): whether a body may lie at `origin` facing
/// `yaw_deg`, and how it bends over the ground there. `None` is a refusal.
/// Without `ground` a fit writes a flat body: 1.1 stops after the sweep, 1.5
/// runs the ground samples anyway and falls back to the flat body when one
/// misses. 1.5 also refuses a sample landing on a plane steeper than
/// [`MIN_WALK_NORMAL`], ground or not, and retries a blocked leg sweep 18
/// higher.
pub fn check_prone(world: &MoveWorld, q: ProneQuery) -> Option<ProneBody> {
    const MASK: u32 = MASK_DEADSOLID;
    let v15 = q.rules.v15();
    let o = q.origin;
    let hw = HALF_WIDTH;
    if q.entry {
        let t = world.box_trace(
            o,
            o + Vec3::Z * 10.0,
            Vec3::new(-hw, -hw, 0.0),
            Vec3::new(hw, hw, HEIGHT_PRONE),
            MASK,
        );
        if t.allsolid {
            return None;
        }
    }
    let half = Vec3::splat(PRONE_BODY_HALF_BOX);
    let trace = |a: Vec3, b: Vec3| world.box_trace(a, b, -half, half, MASK);
    // `AngleVectors` of (0, yaw - 180, 0) (0x2d5e0, 1.5 0x1d1d4, UO
    // 0x21955; the Windows cgames' own, cod11-movers.md 22).
    let back = Vec3::new(0.0, q.yaw_deg - 180.0, 0.0);
    let fwd = x87::angle_vectors_in(world.world.arith(), back)[0];
    let h = HEIGHT_PRONE - PRONE_BODY_HALF_BOX;
    let sweep = q.length - PRONE_BODY_HALF_BOX;
    // The sweep behind the facing: clear, or clear far enough (`reach`).
    let start = o + Vec3::Z * h;
    let mut end = start + fwd * sweep;
    let mut t = trace(start, end);
    let mut reach = None;
    if t.fraction < 1.0 {
        if !q.ground {
            return None;
        }
        let mut got = t.fraction * sweep + PRONE_BODY_HALF_BOX;
        if hw + 2.0 > got {
            return None;
        }
        let limit = h * 0.7 + 24.0;
        if got < limit {
            // Again 22 units higher: over a kerb or a sill.
            end.z += 22.0;
            let len = (end - start).length();
            t = trace(start, end);
            if t.fraction < 1.0 {
                got = t.fraction * len + PRONE_BODY_HALF_BOX;
                if got < limit {
                    return None;
                }
                reach = Some(got);
            }
        } else {
            reach = Some(got);
        }
    }
    let mut feet = t.endpos;
    if !q.ground && !v15 {
        return Some(ProneBody::default());
    }
    // A sample that misses refuses on the ground and lays the body flat off
    // it (1.5 0x2e655); a steep one refuses outright (0x1d842, 0x1dd86,
    // 0x1def4).
    let missed = || (!q.ground).then(ProneBody::default);
    let steep = |t: &crate::collision::Trace| v15 && t.normal.z < MIN_WALK_NORMAL;
    // The hips, 24 behind the origin, dropped onto the ground.
    let drop = h + hw * 2.5 - PRONE_BODY_HALF_BOX;
    let hips_from = o + fwd * 24.0 + Vec3::Z * h;
    let t = trace(hips_from, hips_from - Vec3::Z * drop);
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let hips = t.endpos;
    let hips_drop = drop * t.fraction + PRONE_BODY_HALF_BOX;
    if let Some(got) = reach {
        // A short body bends its legs up the obstacle; the sweep is again
        // from the hips, toward a point between the full length and that.
        if hips_drop * -0.75 > got - hips_drop {
            return missed();
        }
        let v = ((feet - hips) + fwd * PRONE_BODY_HALF_BOX + Vec3::Z * PRONE_BODY_HALF_BOX)
            .normalize_or_zero();
        let reach_to = hips_from + v * (sweep - 24.0);
        let far = o + fwd * sweep;
        let to = Vec3::new(
            0.5 * (far.x + reach_to.x),
            0.5 * (far.y + reach_to.y),
            reach_to.z,
        );
        let mut t = trace(hips_from, to);
        if t.fraction < 1.0 && v15 {
            // 1.5 tries once more from the hit, 18 up (rodata 0x74fb0).
            let up = Vec3::Z * 18.0;
            t = trace(t.endpos + up, to + up);
        }
        if t.fraction < 1.0 {
            return missed();
        }
        feet = t.endpos;
    }
    // The feet and the chest dropped onto the ground.
    let t = trace(feet, feet - Vec3::Z * ((feet.z - hips.z) * 2.0 + hw));
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let feet = t.endpos;
    let t = trace(o + Vec3::Z * h, Vec3::new(o.x, o.y, o.z - hw * 1.5));
    if t.fraction == 1.0 {
        return missed();
    }
    if steep(&t) {
        return None;
    }
    let chest = t.endpos;
    let bend = angle_subtract(vectopitch(feet - hips), vectopitch(hips - chest));
    // Legs bent up past the hips: 1.1 and 1.5 allow 50 degrees (rodata 0x701a8,
    // 1.5 0x74fb8), UO 45 (0x9a9e8; coduo-movement.md 17).
    let bend_up = if q.rules == MoveRules::Uo {
        -45.0
    } else {
        -50.0
    };
    if !(bend_up..=70.0).contains(&bend) {
        return missed();
    }
    // Nothing between the three samples, 5 units up.
    let up = Vec3::Z * 5.0;
    let line = |a: Vec3, b: Vec3| world.box_trace(a + up, b + up, Vec3::ZERO, Vec3::ZERO, MASK);
    if line(chest, hips).fraction < 1.0 || line(hips, feet).fraction < 1.0 {
        return missed();
    }
    // One x87 expression, stored once (1.1d 0x2e14c, 1.5b 0x1e589, UO
    // 0x22ed5): the difference is never rounded to a float on its own.
    let torso_height = (f64::from(chest.z) - f64::from(o.z)) - f64::from(PRONE_BODY_HALF_BOX);
    Some(ProneBody {
        torso_height: torso_height as f32,
        torso_pitch: angle_normalize180(vectopitch(chest - hips)),
        waist_pitch: angle_normalize180(vectopitch(hips - feet)),
    })
}

/// The frame a player goes prone, the tail of `PM_CheckDuck`'s prone arm
/// (0x31ca9-0x31f50): a press on a player moving forward or back throws it
/// into the air, the body takes the view's yaw, and both prone pitches start
/// from the ground under it (docs/research/cod11-mantle.md, "Prone").
pub(super) fn enter_prone(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld) {
    // Retail tests the ADS flag here too, but a move clears it first, so a
    // forward or back cmd always dives. 1.5 has no dive.
    if input.forward != 0.0 && ps.rules == MoveRules::V1_1 {
        ps.prone_dive = true;
        if ps.on_ground {
            let height = if ps.ducked {
                DIVE_HEIGHT_LOW
            } else {
                DIVE_HEIGHT_STAND
            };
            ps.velocity.z = (2.0 * height * ps.gravity).sqrt();
            ps.on_ground = false;
            ps.ground_plane = None;
        }
        ps.aim_spread_scale = 255.0;
    }
    if ps.rules == MoveRules::V1_1 {
        // 0x31d35-0x31df1: the box goes 10 up and back down, and the body
        // takes where it lands.
        let (mins, maxs) = (ps.mins(), ps.maxs());
        let up = world.box_trace(
            ps.origin,
            ps.origin + Vec3::Z * 10.0,
            mins,
            maxs,
            MASK_PLAYERSOLID,
        );
        ps.origin = world
            .box_trace(up.endpos, ps.origin, mins, maxs, MASK_PLAYERSOLID)
            .endpos;
    }
    // The body takes the view's yaw as it is (1.1 0x31dfc; 1.5 in 0x232c4).
    ps.prone_direction = view_deg(ps.yaw);
    let t = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * 0.25,
        ps.mins(),
        ps.maxs(),
        MASK_PLAYERSOLID,
    );
    ps.prone_direction_pitch = if t.fraction < 1.0 && !t.startsolid {
        pitch_for_yaw_on_normal(ps.prone_direction, t.normal)
    } else {
        0.0
    };
    // The torso starts on the ground's pitch, but no further than the cap
    // from the view, so lying down never yanks the view.
    let view_pitch = view_deg(-ps.pitch);
    let d = angle_delta(
        ps.prone_direction_pitch,
        view_pitch,
        delta_stored(ps.rules, world),
    );
    ps.prone_torso_pitch = if d < -PRONE_PITCHCAP {
        view_pitch - PRONE_PITCHCAP
    } else if d > PRONE_PITCHCAP {
        view_pitch + PRONE_PITCHCAP
    } else {
        ps.prone_direction_pitch
    };
}

/// The pitch, wire convention (positive down), of the yaw's direction laid
/// on a plane: `PitchForYawOnNormal` (0x3d274), in 0..360.
pub fn pitch_for_yaw_on_normal(yaw_deg: f32, normal: Vec3) -> f32 {
    let (sin, cos) = yaw_deg.to_radians().sin_cos();
    let fwd = Vec3::new(cos, sin, 0.0);
    let inv = 1.0 / normal.length_squared();
    let p = fwd - normal * (fwd.dot(normal) * inv * inv);
    if p.x == 0.0 && p.y == 0.0 {
        return if p.z > 0.0 { 270.0 } else { 90.0 };
    }
    let pitch = -p.z.atan2(p.truncate().length()).to_degrees();
    if pitch < 0.0 { pitch + 360.0 } else { pitch }
}

/// The prone half of `PM_UpdateViewAngles` (0x32d7c): the body swinging to
/// follow the view, the view capped to the yaw cone around the body and
/// pitched no further than the cap off `proneTorsoPitch`. Retail enforces
/// both caps by pushing `delta_angles`, so the corrections are reported here
/// and the caller applies them to whatever owns the view
/// (docs/research/cod11-mantle.md, "Prone").
pub(super) fn update_prone_view(ps: &mut PlayerState, input: &PmInput, world: &MoveWorld, dt: f32) {
    let prone = ps.stance == Stance::Prone;
    if !prone && !ps.deployed() {
        return;
    }
    if ps.rules.v15() {
        // UO caps any LMG at `bg_lmg_yawcap`, swings only an undeployed
        // body and pulls the view to none holding an LMG (0x2b15c).
        let cap = if ps.lmg && ps.rules == MoveRules::Uo {
            ps.bg.lmg_yawcap as f32
        } else {
            PRONE_YAWCAP
        };
        let lmg = ps.lmg && ps.rules == MoveRules::Uo;
        update_prone_view_v15(
            ps,
            input,
            world,
            dt,
            cap,
            prone && !ps.deployed(),
            prone && !lmg,
        );
        cap_prone_pitch(ps, world);
        return;
    }
    let view = view_deg(ps.yaw);
    // `AngleNormalize180(AngleDelta(..))` (0x3306d, 0x3307b): folded through the
    // shorts twice, so the cap's push below truncates what retail's does.
    let delta = angle_normalize180(angle_delta(
        ps.prone_direction,
        view,
        delta_stored(ps.rules, world),
    ));
    // The body turns past the soft edge, and inside it whenever the player
    // moves (0x330bd), and only into a direction it still fits in.
    let moving = input.forward != 0.0 || input.right != 0.0;
    if delta.abs() > PRONE_SOFT_EDGE || (moving && delta != 0.0) {
        let step = PRONE_SWING_DEG_PER_SEC * dt;
        let candidate = if step > delta.abs() {
            view
        } else if delta > 0.0 {
            ps.prone_direction - step
        } else {
            ps.prone_direction + step
        };
        let q = ProneQuery {
            origin: ps.origin,
            yaw_deg: candidate,
            entry: false,
            ground: ps.on_ground,
            length: PRONE_CHECK_LENGTH,
            rules: ps.rules,
        };
        if let Some(body) = check_prone(world, q) {
            ps.prone_direction = candidate;
            ps.prone_body = body;
        } else if delta.abs() > PRONE_YAWCAP + 0.1 {
            // 0x3319d-0x331c4: a refused swing announces itself only once
            // the view is past the cap (rodata 0x70c90).
            ps.prone_blocked = true;
        }
    }
    // The cap measures the excess before the swing and places the view
    // after it (0x331c8-0x33235).
    // The view lands on the cap through `AngleNormalize360` (0x33266): on
    // the shorts grid, in 0..360.
    if delta.abs() > PRONE_YAWCAP {
        ps.view_yaw_correction = delta - PRONE_YAWCAP.copysign(delta);
        let yaw = short_normalize360(f64::from(ps.prone_direction - PRONE_YAWCAP.copysign(delta)));
        ps.view_capped[1] = Some(yaw);
        ps.yaw = view_rad(yaw);
    }
    cap_prone_pitch(ps, world);
}

/// The prone pitch cap (1.1 0x33295.., 1.5 0x26ba0..): the view pitch no
/// further than [`PRONE_PITCHCAP`] off `proneTorsoPitch`, measured through
/// `AngleDelta` on both. 1.1 places the view through the short-truncating
/// `AngleNormalize180` (0x33346), 1.5 and UO through
/// `AngleNormalize180Accurate` (cod11-mantle.md, "AngleDelta").
pub(super) fn cap_prone_pitch(ps: &mut PlayerState, world: &MoveWorld) {
    let view_pitch = view_deg(-ps.pitch);
    let d = angle_delta(
        ps.prone_torso_pitch,
        view_pitch,
        delta_stored(ps.rules, world),
    );
    if d.abs() > PRONE_PITCHCAP {
        ps.view_pitch_correction = d - PRONE_PITCHCAP.copysign(d);
        let at = ps.prone_torso_pitch - PRONE_PITCHCAP.copysign(d);
        let pitch = if ps.rules.v15() {
            normalize180_accurate(at)
        } else {
            short_normalize180(f64::from(at))
        };
        ps.view_capped[0] = Some(pitch);
        ps.pitch = -view_rad(pitch);
    }
}

/// The prone arm of `SetClientViewAngle` (1.1 0x41e30, 1.5 0x3bf08, UO
/// 0x44301): a prone player not on a turret or in a seat has the angles
/// he is given held 45 degrees either side of the body's yaw and from 45
/// down to 15 up off `proneTorsoPitch`, each measured through
/// `AngleDelta` and placed through the short-truncating
/// `AngleNormalize360` / `AngleNormalize180` on all three. Wire degrees,
/// pitch positive down (cod11-mantle.md, "AngleDelta").
pub fn prone_view_angle(ps: &PlayerState, angles: [f32; 3]) -> [f32; 3] {
    ProneCone::of(ps).map_or(angles, |cone| cone.hold(angles))
}

/// What [`prone_view_angle`] reads off a playerstate: the body's yaw and
/// torso pitch of a prone player not on a turret or in a seat. The server
/// mirrors it to the script host, whose `setPlayerAngles` writes the held
/// angles to the entity's `angles` (1.1 0x4209f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProneCone {
    pub direction: f32,
    pub torso_pitch: f32,
    pub rules: MoveRules,
}

impl ProneCone {
    pub fn of(ps: &PlayerState) -> Option<Self> {
        (ps.stance == Stance::Prone && ps.mounted.is_none() && ps.seat.is_none()).then_some(Self {
            direction: ps.prone_direction,
            torso_pitch: ps.prone_torso_pitch,
            rules: ps.rules,
        })
    }

    /// `angles` held inside the cone, wire degrees.
    pub fn hold(self, mut angles: [f32; 3]) -> [f32; 3] {
        let d = angle_normalize180(angle_delta(self.direction, angles[1], self.rules.v15()));
        if d > 45.0 {
            angles[1] = short_normalize360(f64::from(self.direction - 45.0));
        } else if d < -45.0 {
            angles[1] = short_normalize360(f64::from(self.direction + 45.0));
        }
        let d = angle_normalize180(angle_delta(self.torso_pitch, angles[0], self.rules.v15()));
        if d > 45.0 {
            angles[0] = short_normalize180(f64::from(self.torso_pitch - 45.0));
        } else if d < -15.0 {
            angles[0] = short_normalize180(f64::from(self.torso_pitch + 15.0));
        }
        angles
    }
}

/// 1.5's prone yaw (`PM_UpdateViewAngles` 0x26118, cod15-movement.md 4).
/// The body swings toward the view as in 1.1, but a swing that does not fit
/// walks back toward the old heading a degree at a time until one does, and
/// each try is the fit at a direction and length between the body's and the
/// view's (`swing_fits`, 0x25ff5) plus a 45-unit fit of both the view and
/// the new heading. Then a body still off the view must fit a 45-unit body
/// along its own heading; while it does not, the view turns a degree at a
/// time toward the body. The cap is measured after that and places the view
/// off the new heading. Any refusal sets 0x8000, and a refused turn of at
/// most a degree eases the view 98% of the way back to where it was.
fn update_prone_view_v15(
    ps: &mut PlayerState,
    input: &PmInput,
    world: &MoveWorld,
    dt: f32,
    cap: f32,
    swing: bool,
    pull: bool,
) {
    // `AngleDelta` folds through the shorts (`AngleNormalize180` 0x36a8f),
    // the float difference times the float 182.04445 left unrounded on the
    // x87 stack before the truncation (`AngleNormalize360` 0x36a4e-0x36a6b).
    let delta = |a: f32, b: f32| {
        let short = (f64::from(a - b) * f64::from(65536.0f32 / 360.0)) as i32 & 0xffff;
        let d = short as f32 * (360.0 / 65536.0);
        if d > 180.0 { d - 360.0 } else { d }
    };
    let origin = ps.origin;
    let query = |yaw_deg: f32, length: f32, ground: bool| ProneQuery {
        origin,
        yaw_deg,
        entry: false,
        ground,
        length,
        rules: MoveRules::V1_5,
    };
    let ground = ps.on_ground;
    let fits45 = |yaw: f32| check_prone(world, query(yaw, 45.0, ground)).is_some();
    // 0x25ff5: off the view by `d`, the body is tried a third of the way
    // from the view per 80 degrees, and shorter, down to 45 at 240.
    let swing_fits = |body: &mut ProneBody, yaw: f32, view: f32| {
        let d = delta(yaw, view);
        let f = d.abs() / 240.0;
        let q = query(
            normalize360_accurate(yaw - (1.0 - f) * d),
            60.0 * (1.0 - f) + 45.0 * f,
            ground,
        );
        let fit = check_prone(world, q);
        if let Some(b) = fit {
            *body = b;
        }
        fit.is_some()
    };
    let old_view = ps.prev_yaw.to_degrees();
    // The view starts on the shorts grid (`SHORT2ANGLE` of the cmd plus
    // `delta_angles`); taking it back off the radians would miss a fold by
    // an ulp.
    let new_view = {
        let step = 360.0 / 65536.0;
        f32::from((ps.yaw.to_degrees() / step).round() as i32 as i16) * step
    };
    let mut view = new_view;
    let mut shorts = 0i32;
    let mut pushed = false;
    let mut blocked = false;
    let mut body = ps.prone_body;

    let mut d = delta(ps.prone_direction, view);
    let moving = input.forward != 0.0 || input.right != 0.0;
    'swing: {
        if !swing || d.abs() <= cap - PRONE_SOFT_MARGIN && !(moving && d != 0.0) {
            break 'swing;
        }
        let step = PRONE_SWING_DEG_PER_SEC * dt;
        let mut cand = if d.abs() < step {
            view
        } else if d > 0.0 {
            ps.prone_direction - step
        } else {
            ps.prone_direction + step
        };
        let mut searching = true;
        while !swing_fits(&mut body, cand, view) {
            if !searching {
                break 'swing;
            }
            let mut back = delta(ps.prone_direction, cand);
            searching = back.abs() > 1.0;
            if searching {
                back = if back > 0.0 { 1.0 } else { -1.0 };
            } else {
                blocked = true;
            }
            cand = normalize360_accurate(cand + back);
        }
        if fits45(view) && fits45(cand) {
            ps.prone_direction = cand;
        } else {
            blocked = true;
        }
    }
    d = delta(ps.prone_direction, view);
    'pull: {
        if !pull || d == 0.0 {
            break 'pull;
        }
        let mut cand = ps.prone_direction;
        let mut searching = true;
        loop {
            let fit = fits45(cand);
            if fit && swing_fits(&mut body, cand, view) {
                break;
            }
            if !searching {
                break 'pull;
            }
            searching = d.abs() > 1.0;
            if searching {
                d = if d > 0.0 { 1.0 } else { -1.0 };
            }
            blocked = true;
            shorts += angle_shorts(d);
            pushed = true;
            view = normalize360_accurate(view + d);
            d = delta(ps.prone_direction, view);
            if !fit {
                cand = normalize360_accurate(cand + d);
            }
        }
        ps.prone_direction = cand;
    }
    if d.abs() > cap {
        let excess = d - cap.copysign(d);
        shorts += angle_shorts(excess);
        pushed = true;
        view = normalize360_accurate(ps.prone_direction - cap.copysign(excess));
    }
    if blocked {
        ps.prone_blocked = true;
        let mut back = delta(old_view, view);
        if back.abs() <= 1.0 && back * delta(new_view, view) > 0.0 {
            back *= 0.98;
            view = normalize360_accurate(view + back);
            shorts += angle_shorts(back);
            pushed = true;
        }
    }
    ps.prone_body = body;
    if pushed {
        ps.view_yaw_shorts = Some(shorts & 0xffff);
        ps.view_capped[1] = Some(view);
        ps.yaw = view.to_radians();
    }
}

/// `PM_UpdatePronePitch` (0x3338c): both prone pitches ease toward the
/// ground's pitch under the body and under the view, or toward level with no
/// ground plane under the player. Airborne it checks the fit and flattens the
/// body, or refuses it (0x33434-0x33461): `pm_flags` 0x8000 and a forced
/// crouch for the client, while the body stays prone.
pub(super) fn update_prone_pitch(
    ps: &mut PlayerState,
    world: &MoveWorld,
    dt: f32,
    events: &mut Vec<PmEvent>,
) {
    if ps.stance != Stance::Prone {
        return;
    }
    if !ps.on_ground {
        let q = ProneQuery {
            origin: ps.origin,
            yaw_deg: ps.prone_direction,
            entry: false,
            ground: false,
            length: PRONE_CHECK_LENGTH,
            rules: ps.rules,
        };
        match check_prone(world, q) {
            Some(body) => ps.prone_body = body,
            None => {
                push_event(events, EV_STANCE_FORCE_CROUCH);
                ps.prone_blocked = true;
            }
        }
    }
    let rules = ps.rules;
    // Both eases measure through the short-truncating `AngleDelta` (1.1
    // 0x334e3, 1.5 0x26fc8), so a pitch stops up to a short short of its
    // target. 1.1 stores through `AngleNormalize180` (0x33577), whole
    // shorts; 1.5 and UO through `AngleNormalize180Accurate` (0x2709a)
    // (cod11-mantle.md, "The prone pitches").
    let ease = |cur: f32, target: f32| {
        let d = angle_delta(target, cur, delta_stored(rules, world));
        if d == 0.0 {
            return cur;
        }
        let rate = f64::from(dt) * f64::from(PRONE_PITCH_DEG_PER_SEC);
        let next = if f64::from(d.abs()) > rate {
            (f64::from(cur) + rate.copysign(f64::from(d))) as f32
        } else {
            cur + d
        };
        if rules.v15() {
            normalize180_accurate(next)
        } else {
            short_normalize180(f64::from(next))
        }
    };
    let (body, view) = match ps.ground_plane {
        Some(n) => (
            pitch_for_yaw_on_normal(ps.prone_direction, n),
            pitch_for_yaw_on_normal(ps.yaw.to_degrees(), n),
        ),
        None => (0.0, 0.0),
    };
    ps.prone_direction_pitch = ease(ps.prone_direction_pitch, body);
    ps.prone_torso_pitch = ease(ps.prone_torso_pitch, view);
}
