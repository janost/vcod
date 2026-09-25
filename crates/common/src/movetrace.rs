//! What pmove traces against: the map plus the other players' capsules,
//! retail's `SV_Trace` world-then-entities walk
//! (docs/research/cod11-player-clip.md).

use crate::collision::{Capsule, CollisionWorld, Prim, Trace};
use glam::Vec3;

pub const CONTENTS_BODY: u32 = 0x2000000;
pub const CONTENTS_CORPSE: u32 = 0x4000000;
/// `ClientThink_real`'s mask for `pm_type` > 5, and `BG_CheckProneValid`'s.
pub const MASK_DEADSOLID: u32 = 0x810011;
/// How far a body hit stops short of the bare Minkowski radius, measured
/// along the start point's normal (`cod_lnxded` 0x80cd424, 0x80cd428).
/// Head-on stop 30.125 in all three target stances: `mp_carentan-dm-bump-walker.txt`, phases `stand/headon`, `crouch/headon`, `prone/headon`.
pub const BODY_RADIUS_EPS: f32 = 0.125;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub entity: u32,
    /// Feet, as `ps.origin`.
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
}

impl Body {
    /// `SV_LinkEntity`'s packing (cod_lnxded 0x80908b0).
    pub fn pack_solid(mins: Vec3, maxs: Vec3) -> i32 {
        let x = maxs.x as i32;
        let zd = (-mins.z as i32).clamp(1, 255);
        let zu = (maxs.z as i32 + 32).clamp(0, 255);
        zu << 16 | zd << 8 | x
    }

    /// `CG_ClipMoveToEntities`' decode (cgame 0x30028df0).
    pub fn from_solid(entity: u32, origin: Vec3, solid: i32, contents: u32) -> Body {
        let x = (solid & 255) as f32;
        let zd = ((solid >> 8) & 255) as f32;
        let zu = ((solid >> 16) & 255) as f32 - 32.0;
        Body {
            entity,
            origin,
            mins: Vec3::new(-x, -x, -zd),
            maxs: Vec3::new(x, x, zu),
            contents,
        }
    }
}

#[derive(Clone, Copy)]
pub struct MoveWorld<'a> {
    pub world: &'a CollisionWorld,
    pub bodies: &'a [Body],
    /// The mover's own entity, never clipped.
    pub pass: u32,
}

impl<'a> MoveWorld<'a> {
    pub fn bare(world: &'a CollisionWorld) -> MoveWorld<'a> {
        MoveWorld {
            world,
            bodies: &[],
            pass: u32::MAX,
        }
    }

    pub fn new(world: &'a CollisionWorld, bodies: &'a [Body], pass: u32) -> MoveWorld<'a> {
        MoveWorld {
            world,
            bodies,
            pass,
        }
    }

    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        // world.box_trace always clips against TRACE_MASK_MOVE; mask here
        // only filters which bodies this trace clips against.
        let mut t = self.world.box_trace(start, end, mins, maxs);
        if t.fraction == 0.0 {
            return t;
        }
        let mover = Capsule::of(mins, maxs);
        for b in self.bodies {
            if b.entity == self.pass || b.contents & mask == 0 {
                continue;
            }
            clip_capsule(&mut t, start, end, mover, (maxs - mins).z * 0.5, b);
        }
        t.endpos = start + (end - start) * t.fraction;
        t
    }

    /// Point contents ignore entities for pmove.
    pub fn point_contents(&self, p: Vec3) -> u32 {
        self.world.point_contents(p)
    }

    pub fn entity_num(&self, t: &Trace) -> u32 {
        self.world.entity_num(t)
    }
}

/// Q3 `CM_TraceCapsuleThroughCapsule`: the Minkowski sum of two upright
/// capsules is an upright capsule of the summed radius and summed segment
/// half-lengths, so the sweep is the mover's centre ray against that
/// capsule (two spheres and, when there is a vertical span left over, a
/// cylinder). The two primitives are retail's own, not Q3's (below).
fn clip_capsule(t: &mut Trace, start: Vec3, end: Vec3, mover: Capsule, hh_m: f32, b: &Body) {
    let c = start + mover.center;
    let c_end = end + mover.center;
    let r_m = mover.radius;

    let bc = Capsule::of(b.mins, b.maxs);
    let o = b.origin + bc.center;
    let top = o + bc.offset;
    let bottom = o - bc.offset;
    let hh_b = (b.maxs - b.mins).z * 0.5;

    let r = bc.radius + r_m;
    let h = hh_b + hh_m - r;
    if h > 0.0 {
        trace_cylinder(t, c, c_end, o, r, h, b.entity);
    }
    // The mover's own two sphere centres trace against the body's opposite
    // sphere: the mover's bottom sphere sweeps past the body's top, and the
    // mover's top sweeps past the body's bottom.
    trace_sphere(t, c - mover.offset, c_end - mover.offset, top, r, b.entity);
    trace_sphere(
        t,
        c + mover.offset,
        c_end + mover.offset,
        bottom,
        r,
        b.entity,
    );
}

/// Retail's cylinder trace (`cod_lnxded` 0x8055980): an infinite-height
/// circle of radius `r` swept against a segment, clamped to the half height
/// `h` about `o`. A start inside `r` is startsolid, and allsolid when the end
/// is inside the z-span, whatever its xy; a move that is not closing on the
/// axis, or whose line misses `r`, is not clipped at all; a hit must lie in
/// the z-span at the raw root and is backed off `BODY_RADIUS_EPS` along the
/// start point's normal, which is also the normal it reports
/// (docs/research/cod11-player-clip.md).
fn trace_cylinder(t: &mut Trace, start: Vec3, end: Vec3, o: Vec3, r: f32, h: f32, entity: u32) {
    let rel = (start - o).with_z(0.0);
    let c = rel.length_squared() - r * r;
    if (start.z - o.z).abs() <= h && c <= 0.0 {
        let end_inside = (end.z - o.z).abs() <= h;
        set_startsolid(t, entity, rel, end_inside);
        return;
    }
    let delta = (end - start).with_z(0.0);
    let Some((f0, f)) = backed_off_root(rel, delta, c) else {
        return;
    };
    if f >= t.fraction || (start.lerp(end, f0).z - o.z).abs() > h {
        return;
    }
    t.fraction = f.max(0.0);
    t.normal = rel.normalize_or_zero();
    t.surface_flags = 0;
    t.hit = Some(Prim::Body(entity));
}

/// Retail's sphere trace (`cod_lnxded` 0x8055794): the cylinder's rules in
/// three dimensions, a point swept against a sphere of radius `r` about `o`.
fn trace_sphere(t: &mut Trace, start: Vec3, end: Vec3, o: Vec3, r: f32, entity: u32) {
    let rel = start - o;
    let c = rel.length_squared() - r * r;
    if c <= 0.0 {
        let end_inside = (end - o).length_squared() <= r * r;
        set_startsolid(t, entity, rel, end_inside);
        return;
    }
    let Some((_, f)) = backed_off_root(rel, end - start, c) else {
        return;
    };
    if f >= t.fraction {
        return;
    }
    t.fraction = f.max(0.0);
    t.normal = rel.normalize_or_zero();
    t.surface_flags = 0;
    t.hit = Some(Prim::Body(entity));
}

/// The entry root of `|rel + delta f| = r`, raw and less the backoff, or
/// `None` when the move is not closing (`rel . delta >= 0`, which a zero
/// `delta` is) or its line misses the radius. `c` is `|rel|^2 - r^2`; the
/// backed-off root may be negative, which the callers clamp to 0 after the
/// fraction test.
fn backed_off_root(rel: Vec3, delta: Vec3, c: f32) -> Option<(f32, f32)> {
    let b = rel.dot(delta);
    if b >= 0.0 {
        return None;
    }
    let a = delta.length_squared();
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let f0 = (-b - disc.sqrt()) / a;
    Some((f0, f0 + BODY_RADIUS_EPS * rel.length() / b))
}

/// The start is inside the bare radius: fraction 0 with the start point's
/// normal, and allsolid when the primitive's end test says so. pmove
/// (`slide_move`/step-up) treats allsolid alone as fully stuck; a bare
/// startsolid still clips at fraction 0 and lets the slide bump try a way out.
fn set_startsolid(t: &mut Trace, entity: u32, rel: Vec3, end_inside: bool) {
    t.startsolid = true;
    t.fraction = 0.0;
    t.normal = rel.normalize_or_zero();
    t.hit = Some(Prim::Body(entity));
    if end_inside {
        t.allsolid = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_world;
    use crate::collision::MASK_PLAYERSOLID as LIVE;

    const STAND: (Vec3, Vec3) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));

    fn body_at(x: f32, maxz: f32) -> Body {
        Body {
            entity: 1,
            origin: Vec3::new(x, 0.0, 0.0),
            mins: STAND.0,
            maxs: Vec3::new(15.0, 15.0, maxz),
            contents: CONTENTS_BODY,
        }
    }

    #[test]
    fn standing_solid_packs_to_the_retail_constant() {
        assert_eq!(Body::pack_solid(STAND.0, STAND.1), 6684943);
    }

    #[test]
    fn solid_round_trips_with_the_cgame_one_unit_foot() {
        let b = Body::from_solid(3, Vec3::ZERO, 6684943, CONTENTS_BODY);
        assert_eq!(
            (b.mins, b.maxs),
            (Vec3::new(-15.0, -15.0, -1.0), Vec3::new(15.0, 15.0, 70.0))
        );
    }

    #[test]
    fn a_walk_into_a_standing_body_stops_one_diameter_short() {
        let w = test_world(&[]);
        let bodies = [body_at(100.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let t = mw.box_trace(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(200.0, 0.0, 1.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(t.fraction < 1.0);
        let gap = 100.0 - t.endpos.x;
        assert!((gap - (30.0 + BODY_RADIUS_EPS)).abs() < 0.01, "gap {gap}");
        assert_eq!(mw.entity_num(&t), 1);
        assert!(t.normal.x < -0.99);
    }

    #[test]
    fn a_pass_whose_line_misses_the_bare_radius_is_not_clipped() {
        // Retail backs a hit off the bare radius; it pads nothing, so a pass
        // 30.05 off the axis goes by untouched, inside the 0.125.
        let w = test_world(&[]);
        let bodies = [body_at(100.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let (s, e) = (Vec3::new(0.0, 30.05, 1.0), Vec3::new(200.0, 30.05, 1.0));
        assert_eq!(mw.box_trace(s, e, STAND.0, STAND.1, LIVE).fraction, 1.0);
        let (s, e) = (Vec3::new(0.0, 29.95, 1.0), Vec3::new(200.0, 29.95, 1.0));
        assert!(mw.box_trace(s, e, STAND.0, STAND.1, LIVE).fraction < 1.0);
    }

    #[test]
    fn the_pass_entity_and_a_masked_out_body_do_not_clip() {
        let w = test_world(&[]);
        let bodies = [body_at(100.0, 70.0)];
        let (s, e) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(200.0, 0.0, 1.0));
        assert_eq!(
            MoveWorld::new(&w, &bodies, 1)
                .box_trace(s, e, STAND.0, STAND.1, LIVE)
                .fraction,
            1.0
        );
        assert_eq!(
            MoveWorld::new(&w, &bodies, 0)
                .box_trace(s, e, STAND.0, STAND.1, MASK_DEADSOLID)
                .fraction,
            1.0
        );
    }

    #[test]
    fn a_fall_onto_a_body_lands_on_its_top_sphere() {
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let t = mw.box_trace(
            Vec3::new(0.0, 0.0, 200.0),
            Vec3::new(0.0, 0.0, 0.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(t.normal.z > 0.99, "{:?}", t.normal);
        assert!(
            (t.endpos.z - (70.0 + BODY_RADIUS_EPS)).abs() < 0.01,
            "{}",
            t.endpos.z
        );
    }

    #[test]
    fn a_start_inside_a_body_is_startsolid() {
        let w = test_world(&[]);
        let bodies = [body_at(10.0, 70.0)];
        let t = MoveWorld::new(&w, &bodies, 0).box_trace(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-50.0, 0.0, 1.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(t.startsolid && t.fraction == 0.0);
    }

    #[test]
    fn walking_in_at_any_angle_then_pulling_away_clears_at_fraction_one() {
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        for i in 0..64 {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            let dir = Vec3::new(a.cos(), a.sin(), 0.0);
            let up = Vec3::new(0.0, 0.0, 1.0);
            let t = mw.box_trace(-dir * 200.0 + up, dir * 200.0 + up, STAND.0, STAND.1, LIVE);
            assert!(t.fraction < 1.0, "angle {a} missed the body");
            // Pull straight back out along the approach direction.
            let away = mw.box_trace(t.endpos, t.endpos - dir * 50.0, STAND.0, STAND.1, LIVE);
            assert!(
                away.fraction == 1.0 && !away.startsolid,
                "angle {a} stuck moving away: {away:?}"
            );
            // Slide along the tangent instead of pulling back.
            let tangent = Vec3::new(-dir.y, dir.x, 0.0);
            let side = mw.box_trace(t.endpos, t.endpos + tangent * 50.0, STAND.0, STAND.1, LIVE);
            assert!(
                side.fraction == 1.0 && !side.startsolid,
                "angle {a} stuck on tangent: {side:?}"
            );
        }
    }

    #[test]
    fn tracing_further_into_a_resting_contact_does_not_pass_through() {
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        for i in 0..64 {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            let dir = Vec3::new(a.cos(), a.sin(), 0.0);
            let up = Vec3::new(0.0, 0.0, 1.0);
            let t = mw.box_trace(-dir * 200.0 + up, dir * 200.0 + up, STAND.0, STAND.1, LIVE);
            assert!(t.fraction < 1.0, "angle {a} missed the body");
            let further = mw.box_trace(t.endpos, t.endpos + dir * 50.0, STAND.0, STAND.1, LIVE);
            assert!(
                further.fraction < 1e-4 && further.endpos.distance(t.endpos) < 0.01,
                "angle {a} passed through: {further:?}"
            );
        }
    }

    #[test]
    fn starting_inside_is_allsolid_until_the_end_leaves_the_z_span() {
        // Retail's cylinder reads the end's z alone: a walk out sideways
        // stays allsolid, a move up past the body does not.
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let start = Vec3::new(0.0, 0.0, 1.0);
        let side = mw.box_trace(start, start + Vec3::X * 200.0, STAND.0, STAND.1, LIVE);
        assert!(side.startsolid && side.allsolid, "{side:?}");
        let up = mw.box_trace(start, start + Vec3::Z * 200.0, STAND.0, STAND.1, LIVE);
        assert!(up.startsolid && !up.allsolid, "{up:?}");
    }

    #[test]
    fn resting_on_the_top_sphere_a_ground_check_and_a_side_step_do_not_stick() {
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let fall = mw.box_trace(
            Vec3::new(0.0, 0.0, 200.0),
            Vec3::new(0.0, 0.0, 0.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(fall.fraction < 1.0);
        let rest = fall.endpos;
        // pmove's 0.25-unit down probe for on_ground: still resting on the
        // sphere, so it should stop short with the sphere's normal, not
        // fall through or register stuck.
        let ground = mw.box_trace(rest, rest - Vec3::Z * 0.25, STAND.0, STAND.1, LIVE);
        assert!(
            ground.fraction < 1.0 && ground.normal.z > 0.7 && !ground.startsolid,
            "{ground:?}"
        );
        let step = mw.box_trace(
            rest,
            rest + Vec3::new(2.0, 0.0, 0.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(step.fraction == 1.0 && !step.startsolid, "{step:?}");
    }

    #[test]
    fn a_small_step_directly_away_from_a_resting_contact_clears_at_fraction_one() {
        // A move that is not closing on the body is never clipped, however
        // close it starts.
        let w = test_world(&[]);
        let bodies = [body_at(100.0, 70.0)];
        let mw = MoveWorld::new(&w, &bodies, 0);
        let t = mw.box_trace(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(200.0, 0.0, 1.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(t.fraction < 1.0);
        let away = mw.box_trace(
            t.endpos,
            t.endpos - Vec3::new(0.05, 0.0, 0.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert!(away.fraction == 1.0 && !away.startsolid, "{away:?}");
    }

    #[test]
    fn the_world_wins_at_fraction_zero() {
        // SV_Trace skips the entity pass when the world trace is already 0.
        let w = test_world(&[]);
        let bodies = [body_at(0.0, 70.0)];
        let t = MoveWorld::new(&w, &bodies, 0).box_trace(
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, -20.0),
            STAND.0,
            STAND.1,
            LIVE,
        );
        assert_eq!(t.fraction, 0.0);
        assert_eq!(
            MoveWorld::new(&w, &bodies, 0).entity_num(&t),
            crate::net::protocol::ENTITYNUM_WORLD
        );
    }
}
