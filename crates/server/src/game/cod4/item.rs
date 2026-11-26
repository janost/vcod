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

/// Whether a player at `origin` whose box is `height` tall touches `it` at
/// `level.time` `now` in `G_TouchTriggers` (`cod4_lnxded` 0x80a5352,
/// cod4-combat.md 6.2.1): the item's link box meets the player's grown by
/// 20 (0x8209654), both boxes grown 1 as [`crate::area::Link::boxed`]
/// does, and `BG_PlayerTouchesItem` (0x8053910) finds `pos` within 36
/// across and from 18 below to 88 above the origin. Falling or at rest.
pub fn player_touches(it: &Item4, origin: Vec3, height: f32, now: i32) -> bool {
    const REACH: f32 = 20.0;
    let lo = origin + vcod_cod4::pmove::PLAYER_MINS - (1.0 + REACH);
    let hi = origin + vcod_cod4::pmove::PLAYER_MAXS.with_z(height) + (1.0 + REACH);
    let (alo, ahi) = (it.origin + ITEM_MINS - 1.0, it.origin + ITEM_MAXS + 1.0);
    if (alo.cmpgt(hi) | ahi.cmplt(lo)).any() {
        return false;
    }
    // The difference on the x87 stack, exact.
    let d = origin.as_dvec3() - evaluate(it, now).as_dvec3();
    (-36.0..=36.0).contains(&d.x) && (-36.0..=36.0).contains(&d.y) && (-88.0..=18.0).contains(&d.z)
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
    // `groundEntityNum` 0, so one met mid-air falls on from rest. `trType`
    // 5 or 10 is already falling (cod4-combat.md 6.1).
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
    let f = vcod_common::x87::angle_vectors_libm(angles)[0].as_dvec3();
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

/// `vectoangles` (0x8191a72): [`crate::game::builtins::math::vectoangles_1_7`].
pub fn vectoangles(v: Vec3) -> Vec3 {
    Vec3::from(crate::game::builtins::math::vectoangles_1_7(v.to_array()))
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

    /// A floor at z 0 or a ceiling at z 100 under the item's ±1 box,
    /// struck by a straight sweep: the stub `G_RunItem` traces against.
    /// A start against the ceiling is solid, so no slide runs off it.
    struct Slab {
        ceiling: bool,
    }

    impl PmWorld for Slab {
        fn trace(
            &self,
            start: Vec3,
            end: Vec3,
            mins: Vec3,
            maxs: Vec3,
            _: u32,
            _: u32,
        ) -> vcod_cod4::pmove::Tr {
            let (from, to, wall, normal) = match self.ceiling {
                false => (start.z + mins.z, end.z + mins.z, 0.0, Vec3::Z),
                true => (start.z + maxs.z, end.z + maxs.z, 100.0, Vec3::NEG_Z),
            };
            let crosses = if self.ceiling { to > wall } else { to < wall };
            if !crosses {
                return vcod_cod4::pmove::Tr::default();
            }
            vcod_cod4::pmove::Tr {
                fraction: (from - wall) / (from - to),
                normal,
                entity_num: 1022,
                // A box already against the ceiling starts in it.
                startsolid: self.ceiling && from >= wall,
                ..vcod_cod4::pmove::Tr::default()
            }
        }
    }

    fn dropped(origin: Vec3) -> Item4 {
        Item4 {
            id: vcod_gsc::EntId(0, 0),
            weapon: 0,
            model: 0,
            owner: 0,
            clear_owner_at: 0,
            tr_type: TR_STATIONARY,
            start_ms: 0,
            duration_ms: 0,
            base: origin,
            delta: Vec3::ZERO,
            origin,
            angles: Vec3::ZERO,
            ground: NONE,
            clip: 0,
            stock: 0,
        }
    }

    /// 1.7's reach: 36 across, 18 below and 88 above a standing player's
    /// origin; a prone one's box, 30 tall, cuts the top at 53; a falling
    /// item is met where its arc puts it at the frame's time.
    #[test]
    fn a_player_touches_an_item_within_1_7_s_reach() {
        let o = Vec3::new(100.0, 200.0, 50.0);
        let at = |d: Vec3| player_touches(&dropped(o + d), o, 70.0, 0);
        assert!(at(Vec3::new(36.0, -36.0, 88.0)) && at(Vec3::new(-36.0, 36.0, -18.0)));
        assert!(!at(Vec3::new(36.5, 0.0, 0.0)) && !at(Vec3::new(0.0, -36.5, 0.0)));
        assert!(!at(Vec3::new(0.0, 0.0, 88.5)) && !at(Vec3::new(0.0, 0.0, -18.5)));
        let prone = |dz: f32| player_touches(&dropped(o + Vec3::Z * dz), o, 30.0, 0);
        assert!(prone(53.0) && !prone(53.5));
        // Linked at 80 above, falling from 100: out of reach at its start,
        // 84 above 200 ms on.
        let mut falling = dropped(o + Vec3::Z * 80.0);
        falling.tr_type = TR_GRAVITY;
        falling.base = o + Vec3::Z * 100.0;
        assert!(!player_touches(&falling, o, 70.0, 0));
        assert!(player_touches(&falling, o, 70.0, 200));
    }

    /// With no ground it falls from where it is; free of the floor it goes
    /// to the arc's point 50 ms on; a hit makes the 50 ms stop along the
    /// move; the contact under 0.01 of the move rests it on the floor, its
    /// ground the struck entity (cod4-combat.md 6.1).
    #[test]
    fn a_dropped_item_falls_stops_and_rests() {
        let floor = Slab { ceiling: false };
        let mut it = dropped(Vec3::new(0.0, 0.0, 40.0));
        assert!(matches!(
            run(&mut it, 1000, &floor, |_| false, |_| 0),
            Step::Kept
        ));
        assert_eq!((it.tr_type, it.start_ms), (TR_GRAVITY, 1000));
        assert!(it.origin.z < 40.0 && it.origin.z > 38.0, "{}", it.origin);
        let mut now = 1000;
        while it.tr_type == TR_GRAVITY && now < 5000 {
            now += 50;
            run(&mut it, now, &floor, |_| false, |_| 0);
        }
        assert_eq!(
            (it.tr_type, it.start_ms, it.duration_ms),
            (TR_LINEAR_STOP, now, 50)
        );
        assert!(
            (it.origin.z - 1.0).abs() < 1e-3,
            "on the floor: {}",
            it.origin
        );
        // The stop's 50 ms then the fall's next step meets the floor at once.
        while !it.resting() && now < 6000 {
            now += 50;
            assert!(matches!(
                run(&mut it, now, &floor, |_| false, |_| 0),
                Step::Kept
            ));
        }
        assert!(it.resting());
        assert_eq!((it.ground, it.delta), (1022, Vec3::ZERO));
    }

    /// A rest against a surface facing down (a start against it, so the
    /// one slide does not run), or in `CONTENTS_NODROP`, frees the item.
    #[test]
    fn an_item_resting_upside_down_or_in_nodrop_is_freed() {
        let mut it = dropped(Vec3::new(0.0, 0.0, 99.0));
        it.tr_type = TR_GRAVITY;
        it.delta = Vec3::new(0.0, 0.0, 200.0);
        it.start_ms = 1000;
        let up = Slab { ceiling: true };
        assert!(matches!(
            run(&mut it, 1000, &up, |_| false, |_| 0),
            Step::Freed
        ));
        let floor = Slab { ceiling: false };
        let mut it = dropped(Vec3::new(0.0, 0.0, 1.0));
        let nodrop = |_: Vec3| CONTENTS_NODROP;
        assert!(matches!(
            run(&mut it, 1000, &floor, |_| false, nodrop),
            Step::Freed
        ));
    }

    #[test]
    fn a_level_rest_rolls_the_item_on_its_side() {
        let a = rest_angles(Vec3::new(0.0, -134.989, 0.0), Vec3::Z);
        assert!((a.y - 225.011).abs() < 1e-3, "{a}");
        assert_eq!(a.x, 0.0);
        assert!(a.z.abs() < 1e-4, "{a}");
    }
}
