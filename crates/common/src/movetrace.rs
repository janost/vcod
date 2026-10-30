//! What pmove traces against: the map plus the other players' capsules,
//! retail's `SV_Trace` world-then-entities walk
//! (docs/research/cod11-player-clip.md).

use crate::collision::{Capsule, CollisionWorld, Prim, Trace};
use crate::net::protocol::ENTITYNUM_WORLD;
use crate::patch::Arith;
use glam::Vec3;
use std::cell::{Cell, RefCell};

pub const CONTENTS_BODY: u32 = 0x2000000;
pub const CONTENTS_CORPSE: u32 = 0x4000000;
/// `ClientThink_real`'s mask for `pm_type` > 5, and `BG_CheckProneValid`'s.
pub const MASK_DEADSOLID: u32 = 0x810011;
/// `SpectatorThink`'s `pm.tracemask` (0x3fbd8): no player clip, no bodies.
pub const MASK_SPECTATOR: u32 = 0x800011;
/// A body hit's backoff along the start normal (docs/research/cod11-player-clip.md 2.2, 9.1).
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

/// A linked entity's own box as `SV_PointContents` tests it: the server's
/// point contents meet entities, the client's do not. Only United
/// Offensive's `trigger_mount` is handed in (docs/research/coduo-movement.md 11).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentsBox {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
}

impl ContentsBox {
    /// The contents of every box in `boxes` holding `p`.
    pub fn contents_at(boxes: &[ContentsBox], p: Vec3) -> u32 {
        boxes
            .iter()
            .filter(|b| p.cmpge(b.mins).all() && p.cmple(b.maxs).all())
            .fold(0, |c, b| c | b.contents)
    }
}

/// What one `Pmove` keeps beside the playerstate across its steps:
/// `pm->tracemask` once [`MoveWorld::pm_trace`] dropped the bodies from it,
/// and `pm->touchents`, which `ClientImpacts` notifies after the move
/// (docs/research/cod11-player-clip.md 13). A caller makes one per cmd and
/// hands it to every step's world through [`MoveWorld::in_pmove`].
#[derive(Debug, Default)]
pub struct Pm {
    bodies_off: Cell<bool>,
    touched: RefCell<Vec<u32>>,
}

/// `MAXTOUCH`: `PM_AddTouchEnt` keeps no more (1.1d 0x3466c, 1.5b 0x1e857).
const MAX_TOUCH: usize = 32;

impl Pm {
    /// `PM_AddTouchEnt`: once per entity, never the world, at most 32.
    pub fn add_touch(&self, entity: u32) {
        let mut t = self.touched.borrow_mut();
        if entity != ENTITYNUM_WORLD && t.len() < MAX_TOUCH && !t.contains(&entity) {
            t.push(entity);
        }
    }

    /// The touch list in the order the move first met each entity.
    pub fn touched(&self) -> Vec<u32> {
        self.touched.borrow().clone()
    }
}

/// An entity linked as a temp box model that a move clips: its absolute
/// box and contents. CoD2's mantle block (cod2-movement.md 9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipBox {
    pub entity: u32,
    /// `r.ownerNum`: the mover it lets through (`SV_ClipMoveToEntities`).
    pub owner: u32,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: u32,
}

#[derive(Clone, Copy)]
pub struct MoveWorld<'a> {
    pub world: &'a CollisionWorld,
    pub bodies: &'a [Body],
    /// The mover's own entity, never clipped.
    pub pass: u32,
    /// Entity boxes the point contents add; empty on the client.
    pub boxes: &'a [ContentsBox],
    /// The `Pmove` this world serves, if any.
    pub pm: Option<&'a Pm>,
    /// Box entities the traces clip, after the bodies.
    pub clip_boxes: &'a [ClipBox],
}

impl<'a> MoveWorld<'a> {
    pub fn bare(world: &'a CollisionWorld) -> MoveWorld<'a> {
        MoveWorld {
            world,
            bodies: &[],
            pass: u32::MAX,
            boxes: &[],
            pm: None,
            clip_boxes: &[],
        }
    }

    pub fn new(world: &'a CollisionWorld, bodies: &'a [Body], pass: u32) -> MoveWorld<'a> {
        MoveWorld {
            world,
            bodies,
            pass,
            boxes: &[],
            pm: None,
            clip_boxes: &[],
        }
    }

    /// The same world clipping `clip_boxes` too.
    pub fn with_clip_boxes(self, clip_boxes: &'a [ClipBox]) -> MoveWorld<'a> {
        MoveWorld { clip_boxes, ..self }
    }

    /// The same world with entity boxes in its point contents.
    pub fn with_boxes(self, boxes: &'a [ContentsBox]) -> MoveWorld<'a> {
        MoveWorld { boxes, ..self }
    }

    /// The same world serving `pm`'s `Pmove`.
    pub fn in_pmove(self, pm: &'a Pm) -> MoveWorld<'a> {
        MoveWorld {
            pm: Some(pm),
            ..self
        }
    }

    /// `pm->tracemask` as `mask` once this `Pmove`'s [`Self::pm_trace`]
    /// dropped the bodies from it: what `PM_GroundTraceMissed` traces under.
    pub fn pm_mask(&self, mask: u32) -> u32 {
        if self.pm.is_some_and(|pm| pm.bodies_off.get()) {
            mask & !CONTENTS_BODY
        } else {
            mask
        }
    }

    /// 1.5's and UO's `PM_trace` (1.5b 0x1e738, UO 0x23174) under
    /// `pm->tracemask` `mask`: a trace that starts inside a body adds it
    /// to the touch list, drops bodies from the mask for the rest of the
    /// `Pmove` and is traced again without them
    /// (docs/research/cod11-player-clip.md 13).
    pub fn pm_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        let mask = self.pm_mask(mask);
        let t = self.box_trace(start, end, mins, maxs, mask);
        let body = match t.hit {
            Some(Prim::Body(e)) if t.startsolid => e,
            _ => return t,
        };
        if !self
            .bodies
            .iter()
            .any(|b| b.entity == body && b.contents & CONTENTS_BODY != 0)
        {
            return t;
        }
        if let Some(pm) = self.pm {
            pm.add_touch(body);
            pm.bodies_off.set(true);
        }
        self.box_trace(start, end, mins, maxs, mask & !CONTENTS_BODY)
    }

    /// `PM_AddTouchEnt` of what `t` stopped on, when a `Pmove` listens.
    pub fn touch(&self, t: &Trace) {
        if let Some(pm) = self.pm {
            pm.add_touch(self.entity_num(t));
        }
    }

    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        let mut t = self.world.move_trace(start, end, mins, maxs, mask);
        if t.fraction == 0.0 {
            return t;
        }
        let world_fraction = t.fraction;
        let mover = Capsule::of(mins, maxs);
        // Broad phase: a body whose padded box misses the padded swept AABB
        // can't be hit, since the backoff only ever shortens a real hit and
        // startsolid needs overlap.
        const PAD: Vec3 = Vec3::splat(1.0);
        let swept_min = start.min(end) + mins - PAD;
        let swept_max = start.max(end) + maxs + PAD;
        for b in self.bodies {
            if b.entity == self.pass || b.contents & mask == 0 {
                continue;
            }
            let b_min = b.origin + b.mins - PAD;
            let b_max = b.origin + b.maxs + PAD;
            if b_max.cmplt(swept_min).any() || b_min.cmpgt(swept_max).any() {
                continue;
            }
            if self.world.arith() == Arith::Kept {
                kept::clip_capsule(&mut t, start, end, mins, maxs, b);
            } else {
                clip_capsule(&mut t, start, end, mover, (maxs - mins).z * 0.5, b);
            }
        }
        for b in self.clip_boxes {
            if b.entity == self.pass || b.owner == self.pass || b.contents & mask == 0 {
                continue;
            }
            if (b.maxs + PAD).cmplt(swept_min).any() || (b.mins - PAD).cmpgt(swept_max).any() {
                continue;
            }
            crate::collision::clip_entity_box(
                &mut t, start, end, mins, maxs, b.mins, b.maxs, b.contents, b.entity,
            );
        }
        // `SV_Trace` recomputes the end point only when an entity cut the
        // world's fraction, from the unshifted points in one rounding
        // (`cod_lnxded` 0x8091a06-0x8091a51).
        if t.fraction < world_fraction {
            let f = f64::from(t.fraction);
            let lerp = |s: f32, e: f32| (f64::from(s) + (f64::from(e) - f64::from(s)) * f) as f32;
            t.endpos = Vec3::new(
                lerp(start.x, end.x),
                lerp(start.y, end.y),
                lerp(start.z, end.z),
            );
        }
        t
    }

    /// The world's contents at `p` and those of every [`ContentsBox`]
    /// holding it.
    pub fn point_contents(&self, p: Vec3) -> u32 {
        self.world.point_contents(p) | ContentsBox::contents_at(self.boxes, p)
    }

    pub fn entity_num(&self, t: &Trace) -> u32 {
        self.world.entity_num(t)
    }
}

/// A capsule sweep against `body`'s capsule alone, the transformed trace
/// against a temp capsule model that the UO cgame's flame chunks run on
/// the local player (docs/research/coduo-diff.md 19, "Chunks against bodies").
pub fn body_trace(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, body: &Body) -> Trace {
    let mut t = Trace::clear(end);
    clip_capsule(
        &mut t,
        start,
        end,
        Capsule::of(mins, maxs),
        (maxs - mins).z * 0.5,
        body,
    );
    t.endpos = start.lerp(end, t.fraction);
    t
}

/// The mover's centre swept through the Minkowski sum of the two capsules (docs/research/cod11-player-clip.md 2.2).
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
    // Each mover sphere meets the body's opposite one: bottom on top, top on bottom.
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

/// Retail's cylinder, `cod_lnxded` 0x8055980, half height `h` about `o` (docs/research/cod11-player-clip.md 2.2).
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

/// Retail's sphere, `cod_lnxded` 0x8055794 (docs/research/cod11-player-clip.md 2.2).
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

/// `(raw, backed off)` entry roots, `None` unless closing and hitting; the second may be negative.
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

/// A start inside the bare radius (docs/research/cod11-player-clip.md 2.2).
fn set_startsolid(t: &mut Trace, entity: u32, rel: Vec3, end_inside: bool) {
    t.startsolid = true;
    t.fraction = 0.0;
    t.normal = rel.normalize_or_zero();
    t.hit = Some(Prim::Body(entity));
    if end_inside {
        t.allsolid = true;
    }
}

/// 1.1d's capsule through capsule, in the body's own frame and rounded as
/// its x87 code rounds: `CM_TransformedBoxTrace` (`cod_lnxded` 0x80568d8),
/// `CM_Trace` (0x8056310) and the sphere and cylinder primitives
/// (0x8055794, 0x8055980) (docs/research/cod11-player-clip.md 2.4).
mod kept {
    use super::{BODY_RADIUS_EPS, Body};
    use crate::collision::{Prim, Trace};
    use crate::x87;
    use glam::{DVec3, Vec3};

    pub(super) fn clip_capsule(
        t: &mut Trace,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        b: &Body,
    ) {
        // The mover's box made symmetric about its centre, its points
        // shifted there and then into the body's frame, each step stored.
        let off = ((mins.as_dvec3() + maxs.as_dvec3()) * 0.5).as_vec3();
        let half = maxs - off;
        let s = (start + off) - b.origin;
        let e = (end + off) - b.origin;
        let r_m = half.x.min(half.z);
        let hh_m = half.z;
        let off_m = hh_m - r_m;
        // `CM_Trace`: the delta stored, its squared length stored.
        let delta = e - s;
        let a = x87::dot(delta, delta) as f32;
        // The body's temp capsule model, from its own bounds.
        let c = ((b.mins.as_dvec3() + b.maxs.as_dvec3()) * 0.5).as_vec3();
        let half_b = b.maxs - c;
        let r_b = half_b.x.min(half_b.z);
        let h_b = half_b.z - r_b;
        let top = Vec3::new(c.x, c.y, c.z + h_b);
        let bottom = Vec3::new(c.x, c.y, c.z - h_b);
        let lo = |p: Vec3| Vec3::new(p.x, p.y, p.z - off_m);
        let hi = |p: Vec3| Vec3::new(p.x, p.y, p.z + off_m);

        let mut tw = Work {
            fraction: 1.0,
            hit: None,
            delta,
            a,
            r_m,
            hh_m,
        };
        cylinder(&mut tw, s, e, c, h_b, r_b);
        // Each mover sphere meets the body's opposite one.
        sphere(&mut tw, lo(s), lo(e), top, r_b);
        sphere(&mut tw, hi(s), hi(e), bottom, r_b);

        let Some(hit) = tw.hit else {
            return;
        };
        // `SV_ClipMoveToEntities`: the solid flags carry over whatever the
        // fraction, the trace itself only when it is shorter.
        if hit.allsolid {
            t.allsolid = true;
        } else if hit.startsolid {
            t.startsolid = true;
        }
        if tw.fraction < t.fraction {
            t.fraction = tw.fraction;
            t.normal = hit.normal;
            t.surface_flags = 0;
            t.hit = Some(Prim::Body(b.entity));
            t.startsolid |= hit.startsolid;
        }
    }

    struct Hit {
        normal: Vec3,
        startsolid: bool,
        allsolid: bool,
    }

    /// The trace state the primitives read and write.
    struct Work {
        fraction: f32,
        hit: Option<Hit>,
        delta: Vec3,
        /// `deltaLenSq`, stored.
        a: f32,
        r_m: f32,
        hh_m: f32,
    }

    fn start_solid(tw: &mut Work, rel: Vec3, allsolid: bool) {
        let prev = tw.hit.take();
        tw.fraction = 0.0;
        tw.hit = Some(Hit {
            normal: x87::normalize(rel),
            startsolid: true,
            allsolid: allsolid || prev.is_some_and(|h| h.allsolid),
        });
    }

    /// The backed-off entry root, compared and stored (0x80558d5-0x8055942,
    /// 0x8055b0d-0x8055bbb): the raw root stored as a float, the 0.125
    /// backoff along the start normal added on the stack. `None` when it is
    /// no nearer than the fraction so far.
    fn root(tw: &Work, rel: Vec3, b: f64, disc: f64, a: f64) -> Option<(f64, f64)> {
        let len = f64::from(x87::dot(rel, rel).sqrt() as f32);
        let back = len * f64::from(BODY_RADIUS_EPS) / b;
        let f0 = f64::from(((-b - disc.sqrt()) / a) as f32);
        let f = f0 + back;
        (f < f64::from(tw.fraction)).then_some((f, back))
    }

    fn store(tw: &mut Work, f: f64, rel: Vec3) {
        tw.fraction = if f > 0.0 { f as f32 } else { 0.0 };
        tw.hit = Some(Hit {
            normal: x87::normalize(rel),
            startsolid: false,
            allsolid: false,
        });
    }

    /// 0x8055794: the sweep of the mover's sphere from `s` against the
    /// body's sphere at `o`, the summed radius squared and stored.
    fn sphere(tw: &mut Work, s: Vec3, e: Vec3, o: Vec3, r: f32) {
        let rel = s.as_dvec3() - o.as_dvec3();
        let rel_f = rel.as_vec3();
        let rr = ((f64::from(r) + f64::from(tw.r_m)).powi(2)) as f32;
        let c = (rel.x * rel.x + rel.y * rel.y) + rel.z * rel.z - f64::from(rr);
        if c <= 0.0 {
            let d = e.as_dvec3() - o.as_dvec3();
            let inside = (d.x * d.x + d.y * d.y) + d.z * d.z <= f64::from(rr);
            start_solid(tw, rel_f, inside);
            return;
        }
        let dl = tw.delta.as_dvec3();
        let b = rel.z * dl.z + (rel.x * dl.x + rel.y * dl.y);
        if b >= 0.0 {
            return;
        }
        let a = f64::from(tw.a);
        let disc = b * b - a * c;
        if disc < 0.0 {
            return;
        }
        if let Some((f, _)) = root(tw, rel_f, b, disc, a) {
            store(tw, f, rel_f);
        }
    }

    /// 0x8055980: the sweep of the mover's centre against the body's
    /// upright cylinder about `o`, half height `h` plus the mover's half
    /// height less its radius, in the plane; the squared summed radius and
    /// the squared delta stay on the stack.
    fn cylinder(tw: &mut Work, s: Vec3, e: Vec3, o: Vec3, h: f32, r: f32) {
        let rel = s.as_dvec3() - o.as_dvec3();
        let rel_f = rel.as_vec3();
        let big_r = f64::from(r) + f64::from(tw.r_m);
        let c = (rel.y * rel.y + rel.x * rel.x) - big_r * big_r;
        let span = f64::from(h) + (f64::from(tw.hh_m) - f64::from(tw.r_m));
        let flat = Vec3::new(rel_f.x, rel_f.y, 0.0);
        if c <= 0.0 {
            if span < rel.z || -span > rel.z {
                return;
            }
            let ez = f64::from(e.z) - f64::from(o.z);
            let inside = ez <= span && ez >= f64::from(-span as f32);
            start_solid(tw, flat, inside);
            return;
        }
        let dl = DVec3::new(f64::from(tw.delta.x), f64::from(tw.delta.y), 0.0);
        let b = rel.y * dl.y + rel.x * dl.x;
        if b >= 0.0 {
            return;
        }
        let a = dl.y * dl.y + dl.x * dl.x;
        let disc = b * b - a * c;
        if disc < 0.0 {
            return;
        }
        let Some((f, back)) = root(tw, flat, b, disc, a) else {
            return;
        };
        let z = f64::from(s.z) + (f - back) * f64::from(tw.delta.z) - f64::from(o.z);
        if z > span || -span > z {
            return;
        }
        store(tw, f, flat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::MASK_PLAYERSOLID as LIVE;
    use crate::collision::test_world;

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
    fn a_clip_box_stops_a_walk_and_lets_its_owner_through() {
        let w = test_world(&[]);
        let blocks = [ClipBox {
            entity: 70,
            owner: 2,
            mins: Vec3::new(85.0, -15.0, 0.0),
            maxs: Vec3::new(115.0, 15.0, 70.0),
            contents: 0x10000,
        }];
        let (s, e) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(200.0, 0.0, 1.0));
        let walk = |pass| {
            let mw = MoveWorld {
                pass,
                ..MoveWorld::bare(&w)
            }
            .with_clip_boxes(&blocks);
            (mw.box_trace(s, e, STAND.0, STAND.1, LIVE), mw)
        };
        let (t, mw) = walk(0);
        assert!((t.endpos.x - 70.0).abs() < 0.2, "{:?}", t.endpos);
        assert_eq!(mw.entity_num(&t), 70);
        assert!(t.normal.x < -0.99);
        assert_eq!(walk(2).0.fraction, 1.0);
        // A mask without 0x10000 (the spectator's) passes it.
        let mw = MoveWorld::bare(&w).with_clip_boxes(&blocks);
        assert_eq!(
            mw.box_trace(s, e, STAND.0, STAND.1, MASK_SPECTATOR)
                .fraction,
            1.0
        );
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
    fn a_body_just_outside_the_swept_box_is_skipped() {
        // The trace runs 0..200 on x with a 15-unit mover radius and a
        // 1-unit pad on each side, so the swept box reaches x=216. A body
        // whose own padded box starts past that (origin 233: 233-15-1=217)
        // never reaches clip_capsule.
        let w = test_world(&[]);
        let (s, e) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(200.0, 0.0, 1.0));
        let far = [body_at(233.0, 70.0)];
        assert_eq!(
            MoveWorld::new(&w, &far, 0)
                .box_trace(s, e, STAND.0, STAND.1, LIVE)
                .fraction,
            1.0
        );
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

    /// `PM_trace`: a start inside a body retraces without bodies, lists the
    /// body as touched and keeps bodies out of the rest of the `Pmove`; a
    /// later sweep into another body passes it.
    #[test]
    fn a_trace_starting_in_a_body_drops_bodies_for_the_rest_of_the_move() {
        let w = test_world(&[]);
        let bodies = [
            body_at(8.0, 70.0),
            Body {
                entity: 2,
                ..body_at(100.0, 70.0)
            },
        ];
        let pm = Pm::default();
        let mw = MoveWorld::new(&w, &bodies, 0).in_pmove(&pm);
        let start = Vec3::new(0.0, 0.0, 1.0);
        let end = Vec3::new(200.0, 0.0, 1.0);
        assert!(mw.box_trace(start, end, STAND.0, STAND.1, LIVE).startsolid);
        assert_eq!(mw.pm_mask(LIVE), LIVE);
        let t = mw.pm_trace(start, end, STAND.0, STAND.1, LIVE);
        assert!(!t.startsolid && t.fraction == 1.0, "{t:?}");
        assert_eq!(pm.touched(), vec![1]);
        assert_eq!(mw.pm_mask(LIVE), LIVE & !CONTENTS_BODY);
        let t = mw.pm_trace(Vec3::new(50.0, 0.0, 1.0), end, STAND.0, STAND.1, LIVE);
        assert_eq!(t.fraction, 1.0);
        // A fresh move clips both again.
        let pm = Pm::default();
        let mw = MoveWorld::new(&w, &bodies, 0).in_pmove(&pm);
        let t = mw.pm_trace(Vec3::new(50.0, 0.0, 1.0), end, STAND.0, STAND.1, LIVE);
        assert!(t.fraction < 1.0);
        assert!(pm.touched().is_empty());
    }

    #[test]
    fn the_touch_list_skips_the_world_and_repeats_and_holds_32() {
        let pm = Pm::default();
        pm.add_touch(crate::net::protocol::ENTITYNUM_WORLD);
        for e in (0..40).chain(0..5) {
            pm.add_touch(e);
        }
        assert_eq!(pm.touched(), (0..32).collect::<Vec<_>>());
    }
}
