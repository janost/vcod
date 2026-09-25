//! What pmove traces against: the map plus the other players' capsules,
//! retail's `SV_Trace` world-then-entities walk
//! (docs/research/cod11-player-clip.md).

use crate::collision::{Capsule, CollisionWorld, Prim, Trace};
use glam::Vec3;

pub const CONTENTS_BODY: u32 = 0x2000000;
pub const CONTENTS_CORPSE: u32 = 0x4000000;
/// `ClientThink_real`'s mask for `pm_type` > 5, and `BG_CheckProneValid`'s.
pub const MASK_DEADSOLID: u32 = 0x810011;
/// Step 1's read of `cod_lnxded`: the sphere/cylinder trace adds `tw+0xf8`
/// to the radius (0x80557c6), but that field traces back through
/// `CM_TraceCapsuleThroughCapsule`'s tw pointer to a value either memcpy'd
/// from the mover's own capsule descriptor or computed as a per-axis extent
/// in the box-mover branch, not a rodata immediate reachable in the ~20
/// minute budget. Falls back to `SURFACE_CLIP_EPSILON`: the retail melee
/// fixture (`mp_carentan-tdm-melee-shooter.txt` lines 58-60) stops two
/// standing players 30.1 apart, which fits 0.125 better than Q3's 1.0.
/// Task 8's capture is the arbiter.
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
/// cylinder). `CM_TraceThroughVerticalCylinder` and `CM_TraceThroughSphere`
/// are folded in here, ported private to this module.
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
    if h > 0.0 && (c_end - c).with_z(0.0) != Vec3::ZERO {
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

/// Q3 `CM_TraceThroughVerticalCylinder`: an infinite-height circle swept
/// against a segment, clamped to the cylinder's half height `h` about `o`.
fn trace_cylinder(t: &mut Trace, start: Vec3, end: Vec3, o: Vec3, r: f32, h: f32, entity: u32) {
    let ray = (end - start).with_z(0.0);
    let rel = (start - o).with_z(0.0);
    let a = ray.length_squared();
    if a < 1e-8 {
        return;
    }
    let b = 2.0 * rel.dot(ray);
    let c = rel.length_squared() - (r + BODY_RADIUS_EPS).powi(2);
    if c < 0.0 {
        // Already inside the cylinder's disc; the z-span test below decides
        // startsolid.
        if (start.z - o.z).abs() < h {
            set_startsolid(t, entity);
        }
        return;
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return;
    }
    let f = (-b - disc.sqrt()) / (2.0 * a);
    if !(0.0..=1.0).contains(&f) {
        return;
    }
    let hit = start.lerp(end, f);
    if (hit.z - o.z).abs() > h {
        return;
    }
    if f < t.fraction {
        t.fraction = f;
        t.normal = (rel + ray * f).with_z(0.0).normalize_or_zero();
        t.surface_flags = 0;
        t.hit = Some(Prim::Body(entity));
    }
}

/// Q3 `CM_TraceThroughSphere`: a point swept against a sphere of radius
/// `r + BODY_RADIUS_EPS`; `startsolid`/`allsolid` test the bare `r`.
fn trace_sphere(t: &mut Trace, start: Vec3, end: Vec3, o: Vec3, r: f32, entity: u32) {
    let ray = end - start;
    let rel = start - o;
    let a = ray.length_squared();
    if a < 1e-8 {
        if rel.length_squared() < r * r {
            set_startsolid(t, entity);
        }
        return;
    }
    let b = 2.0 * rel.dot(ray);
    if rel.length_squared() < r * r {
        set_startsolid(t, entity);
    }
    let c = rel.length_squared() - (r + BODY_RADIUS_EPS).powi(2);
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return;
    }
    let f = (-b - disc.sqrt()) / (2.0 * a);
    if !(0.0..=1.0).contains(&f) {
        return;
    }
    if f < t.fraction {
        t.fraction = f;
        t.normal = (rel + ray * f).normalize_or_zero();
        t.surface_flags = 0;
        t.hit = Some(Prim::Body(entity));
    }
}

/// Unlike the world's brush clip, a sphere or cylinder primitive never
/// computes a partial exit fraction, so a start inside one is stuck outright.
fn set_startsolid(t: &mut Trace, entity: u32) {
    t.startsolid = true;
    t.allsolid = true;
    t.fraction = 0.0;
    t.hit = Some(Prim::Body(entity));
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
