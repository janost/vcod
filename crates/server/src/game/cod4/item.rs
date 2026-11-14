//! A dropped weapon's fall and rest on 1.7: `G_RunItem` (`cod4_lnxded`
//! 0x80908bc) and the math it calls (cod4-combat.md 6.1).

use glam::{DVec3, Vec3};
use vcod_cod4::pmove::PmWorld;

use super::combat::Item4;
use super::missile::{TR_GRAVITY, TR_STATIONARY, Traj};

/// `TR_LINEAR_STOP`: a hit's 50 ms move.
pub const TR_LINEAR_STOP: u32 = 3;
/// An item's clip mask when `clipmask` is 0, as `LaunchItem` leaves it.
pub const MASK_ITEM: u32 = 0x491;
/// `CONTENTS_NODROP`: an item coming to rest in it is freed.
const CONTENTS_NODROP: u32 = 0x8000_0000;
/// `ENTITYNUM_NONE`.
const NONE: u32 = 1023;
/// The item's box, ±1 on every axis (`LaunchItem`, 0x809141b..0x8091453).
pub const ITEM_MINS: Vec3 = Vec3::splat(-1.0);
pub const ITEM_MAXS: Vec3 = Vec3::splat(1.0);

/// What one `G_RunItem` did to the item.
pub enum Step {
    /// Moved, fell, or only thought.
    Kept,
    /// Came to rest upside down or in `CONTENTS_NODROP`: `G_FreeEntity`.
    Freed,
}

/// `BG_EvaluateTrajectory` (0x80536ba) for an item's `pos`: `TR_LINEAR_STOP`
/// (0x80536f5) clamps the time to `trTime + trDuration` and at 0, the
/// `0.001f` times the ms kept on the stack, each axis `delta t + base` one
/// store; `TR_GRAVITY` as a missile's.
pub fn evaluate(it: &Item4, at_ms: i32) -> Vec3 {
    match it.tr_type {
        TR_LINEAR_STOP => {
            let t = at_ms.min(it.start_ms + it.duration_ms) - it.start_ms;
            let dt = (f64::from(0.001f32) * f64::from(t)).max(0.0);
            (it.delta.as_dvec3() * dt + it.base.as_dvec3()).as_vec3()
        }
        TR_GRAVITY => Traj {
            tr_type: TR_GRAVITY,
            time: it.start_ms,
            base: it.base,
            delta: it.delta,
        }
        .at(at_ms),
        _ => it.base,
    }
}

/// One `G_RunItem` at level time `now` against `world` (the clip map and
/// the brush entities). `moves` says whether an entity's `pos.trType` is
/// set (a client's always is); `contents` is `SV_PointContents` at a
/// point.
pub fn run(
    it: &mut Item4,
    now: i32,
    world: &dyn PmWorld,
    moves: impl Fn(u32) -> bool,
    contents: impl Fn(Vec3) -> u32,
) -> Step {
    // With no ground, or one that moves, an item not already falling falls
    // from where it is (0x80908cb..0x809096b). An item in flight has
    // `groundEntityNum` 0, so one met mid-air falls on from rest.
    if (it.ground == NONE || moves(it.ground)) && it.tr_type != TR_GRAVITY && it.tr_type != 10 {
        it.tr_type = TR_GRAVITY;
        it.start_ms = now;
        it.base = it.origin;
        it.delta = Vec3::ZERO;
    }
    if it.tr_type == TR_STATIONARY {
        return Step::Kept;
    }
    let mut goal = evaluate(it, now + 50);
    // Under 0.1 squared off, it aims a unit lower (0x80909a1..0x80909fa).
    let off = (goal.as_dvec3() - it.origin.as_dvec3()).length_squared();
    if f64::from(0.1f32) > off {
        goal.z = (f64::from(goal.z) - 1.0) as f32;
    }
    let cur = it.origin;
    let mut tr = world.trace(cur, goal, ITEM_MINS, ITEM_MAXS, NONE, MASK_ITEM);
    if tr.fraction >= 1.0 {
        it.origin = goal;
        return Step::Kept;
    }
    let lerp = |a: Vec3, b: Vec3, f: f32| {
        let (a, b) = (a.as_dvec3(), b.as_dvec3());
        a + (b - a) * f64::from(f)
    };
    let mut end = lerp(cur, goal, tr.fraction);
    // A start barely off a wall slides once along it (0x8090c4d..0x8090d20).
    if !tr.startsolid && 0.01f32 > tr.fraction && 0.5 > tr.normal.z {
        let n = tr.normal.as_dvec3();
        let dot = (goal.as_dvec3() - cur.as_dvec3()).dot(n);
        let k = 1.0 - dot;
        goal = (goal.as_dvec3() + n * k).as_vec3();
        let from = end.as_vec3();
        tr = world.trace(from, goal, ITEM_MINS, ITEM_MAXS, NONE, MASK_ITEM);
        end = lerp(from, goal, tr.fraction);
    }
    // `TR_LINEAR_STOP` for 50 ms from the origin along the move times 20
    // (0x8090d5d..0x8090dd1): x off the stack, y and z off their stores.
    let c = cur.as_dvec3();
    it.tr_type = TR_LINEAR_STOP;
    it.start_ms = now;
    it.duration_ms = 50;
    it.base = cur;
    it.delta = Vec3::new(
        ((end.x - c.x) * 20.0) as f32,
        (f64::from((end.y - c.y) as f32) * 20.0) as f32,
        (f64::from((end.z - c.z) as f32) * 20.0) as f32,
    );
    it.origin = end.as_vec3();
    if 0.01f32 <= tr.fraction {
        return Step::Kept;
    }
    // At rest (0x8090ab0..0x8090bed).
    if 0.0 >= tr.normal.z || contents(it.origin) & CONTENTS_NODROP != 0 {
        return Step::Freed;
    }
    let mut angles = rest_angles(it.angles, tr.normal);
    if it.weapon != 0 {
        angles.z = (f64::from(angles.z) + 90.0) as f32;
    }
    it.angles = angles;
    it.tr_type = TR_STATIONARY;
    it.start_ms = 0;
    it.duration_ms = 0;
    it.base = it.origin;
    it.delta = Vec3::ZERO;
    // `Trace_GetEntityHitId` (0x811a870).
    it.ground = tr.entity_num;
    Step::Kept
}

/// The rest's angles: the axis rebuilt on the ground's `normal` from the
/// item's old forward (`left = n x f` kept on the stack, `forward = left x
/// n` stored), then `AxisToAngles` (0x81951fe).
pub fn rest_angles(angles: Vec3, normal: Vec3) -> Vec3 {
    let f = vcod_common::x87::angle_vectors(angles)[0].as_dvec3();
    let n = normal.as_dvec3();
    let left = DVec3::new(
        n.y * f.z - n.z * f.y,
        n.z * f.x - n.x * f.z,
        n.x * f.y - n.y * f.x,
    );
    let forward = DVec3::new(
        left.y * n.z - left.z * n.y,
        left.z * n.x - left.x * n.z,
        left.x * n.y - left.y * n.x,
    );
    axis_to_angles(forward.as_vec3(), left.as_vec3())
}

/// `vectoangles` (0x8191a72): yaw then pitch off `atan2` in double, each
/// stored as a float and then taking its add, 360 or 0, before the store;
/// the flat length goes to `atan2` unrounded.
pub fn vectoangles(v: Vec3) -> Vec3 {
    let d = v.as_dvec3();
    if v.x == 0.0 && v.y == 0.0 {
        let pitch = if -v.z < 0.0 { 270.0 } else { 90.0 };
        return Vec3::new(pitch, 0.0, 0.0);
    }
    let wrap = |a: f32| (f64::from(a) + if a < 0.0 { 360.0 } else { 0.0 }) as f32;
    let yaw = (d.y.atan2(d.x) * 180.0 / std::f64::consts::PI) as f32;
    let flat = (d.x * d.x + d.y * d.y).sqrt();
    let pitch = (d.z.atan2(flat) * -180.0 / std::f64::consts::PI) as f32;
    Vec3::new(wrap(pitch), wrap(yaw), 0.0)
}

/// `AxisToAngles` (0x81951fe): pitch and yaw are the forward's
/// [`vectoangles`]; the roll turns the left back by the yaw and pitch
/// (`sinf`, `cosf` of the negated angles in radians, stored floats) and
/// takes `-180 / pi * atan2` of what is left of its z over its x and y.
pub fn axis_to_angles(forward: Vec3, left: Vec3) -> Vec3 {
    let mut out = vectoangles(forward);
    let rad = |deg: f32| (-f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32;
    let (sy, cy) = (rad(out.y).sin(), rad(out.y).cos());
    let a = (f64::from((f64::from(left.x) * f64::from(cy)) as f32)
        - f64::from(left.y) * f64::from(sy)) as f32;
    let b = (f64::from(left.x) * f64::from(sy) + f64::from(left.y) * f64::from(cy)) as f32;
    let (sp, cp) = (rad(out.x).sin(), rad(out.x).cos());
    let (a, lz) = (f64::from(a), f64::from(left.z));
    let c1 = a * f64::from(cp) + lz * f64::from(sp);
    let d = lz * f64::from(cp) - a * f64::from(sp);
    let r = if b == 0.0 && c1 == 0.0 {
        if -d >= 0.0 { 90.0f32 } else { -90.0 }
    } else {
        let h = (f64::from(b) * f64::from(b) + c1 * c1).sqrt();
        (d.atan2(h) * -180.0 / std::f64::consts::PI) as f32
    };
    out.z = if 0.0 > b {
        (f64::from(r) + if r < 0.0 { 180.0 } else { -180.0 }) as f32
    } else {
        -r
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_to_angles_inverts_angle_vectors() {
        for a in [
            Vec3::new(10.0, 30.0, 20.0),
            Vec3::new(-25.0, 200.0, -70.0),
            Vec3::new(0.0, -135.0, 90.0),
        ] {
            let [f, r, _] = vcod_common::x87::angle_vectors(a);
            let b = axis_to_angles(f, -r);
            let want = Vec3::new(
                a.x.rem_euclid(360.0),
                a.y.rem_euclid(360.0),
                if a.z > 180.0 { a.z - 360.0 } else { a.z },
            );
            assert!((b - want).abs().max_element() < 1e-3, "{a} -> {b}");
        }
    }

    #[test]
    fn a_level_rest_rolls_the_item_on_its_side() {
        let a = rest_angles(Vec3::new(0.0, -134.989, 0.0), Vec3::Z);
        assert!((a.y - 225.011).abs() < 1e-3, "{a}");
        assert_eq!(a.x, 0.0);
        assert!(a.z.abs() < 1e-4, "{a}");
    }
}
