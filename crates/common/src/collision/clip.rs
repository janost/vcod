//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! The primitive clips, with no world state: the capsule, brushes, model triangles and patches.

use super::{ModelTri, Prim, SURFACE_CLIP_EPSILON, Trace, WorldPatch};
use crate::patch::{Arith, Sweep};
use crate::terrain::CapsuleSweep;
use glam::Vec3;

/// How far outside an edge plane the static-model clip still counts a
/// crossing as inside the triangle (`cod_lnxded` rodata 0x80db974 and
/// 0x80db978: -0.001 and 1.001).
const MODEL_BARY_EPS: f32 = 0.001;

/// The sweep shape a box becomes: retail's `ClientThink_real` hands pmove
/// `trap_TraceCapsule`, so a player is a capsule against brushes and
/// terrain alike, in Q3's `CM_TestBoundingBoxInCapsule` shape: the radius
/// is the smaller of the half width and the half height, and the two sphere
/// centres sit `offset` above and below the box centre. For the player that
/// is a radius of 15 with spheres 15 and 55 above the feet, which rests on a
/// grade at `h + 15 (1 / n.z - 1)` where a box rests at `h + 15 tan`, the
/// 1-unit lift on a 4-degree street the retail captures do not carry, and
/// which slides along a diagonal wall at 15 where a box's corner holds it
/// at 21 (docs/research/cod11-mantle.md, "The player is a capsule").
#[derive(Clone, Copy)]
pub(crate) struct Capsule {
    /// From the trace origin to the box centre.
    pub(crate) center: Vec3,
    pub(crate) radius: f32,
    pub(crate) offset: Vec3,
}

impl Capsule {
    pub(crate) fn of(mins: Vec3, maxs: Vec3) -> Capsule {
        let half = (maxs - mins) * 0.5;
        let radius = half.x.min(half.z);
        Capsule {
            center: (mins + maxs) * 0.5,
            radius,
            offset: Vec3::Z * (half.z - radius),
        }
    }

    /// The capsule UO's box trace hands terrain and patches: the half width
    /// for the radius whatever the height, so a box wider than it is tall
    /// sits on its lower sphere's foot at the box's floor. A Panzer's hull
    /// (half 35 wide, 30 high) lands where the retail capture parks it
    /// only so (`vehicle_drive_ab`).
    pub(crate) fn wide(mins: Vec3, maxs: Vec3) -> Capsule {
        let half = (maxs - mins) * 0.5;
        Capsule {
            center: (mins + maxs) * 0.5,
            radius: half.x,
            offset: Vec3::Z * (half.z - half.x),
        }
    }
}

/// The temp box model an entity links with (`CM_TempBoxModel`): the
/// absolute box `box_min..box_max` met by the box `mins..maxs` swept from
/// `start` to `end`. Its six sides are axial, which take a capsule as its
/// box, so the sweep is the box's. A contact it makes names `entity`
/// ([`CollisionWorld::entity_num`](super::CollisionWorld::entity_num)) and its `hit` is `Prim::Clip(contents)`.
#[expect(clippy::too_many_arguments)]
pub fn clip_entity_box(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    box_min: Vec3,
    box_max: Vec3,
    contents: u32,
    entity: u32,
) {
    let planes = [
        (Vec3::NEG_X, maxs.x - box_min.x),
        (Vec3::X, box_max.x - mins.x),
        (Vec3::NEG_Y, maxs.y - box_min.y),
        (Vec3::Y, box_max.y - mins.y),
        (Vec3::NEG_Z, maxs.z - box_min.z),
        (Vec3::Z, box_max.z - mins.z),
    ];
    let before = (trace.fraction, trace.startsolid, trace.hit);
    clip_segment(
        trace,
        start,
        end,
        &planes,
        &[],
        Vec3::ZERO,
        Prim::Clip(contents),
    );
    if (trace.fraction, trace.startsolid, trace.hit) != before {
        trace.copy = Some(entity);
    }
}

/// Where the point segment `start..end` meets the brush `planes` as
/// retail's sight trace does (`trap_SightTraceToEntity`, cod11-combat.md
/// 14.8.1): with no `SURFACE_CLIP_EPSILON`, an end on or ahead of a face
/// misses. The enter fraction, 0 for a start inside; `None` for a miss.
pub fn sight_meets_brush(start: Vec3, end: Vec3, planes: &[(Vec3, f32)]) -> Option<f32> {
    let (mut enter, mut leave, mut startout) = (0.0f32, 1.0f32, false);
    for &(n, d) in planes {
        let (d1, d2) = (n.dot(start) - d, n.dot(end) - d);
        if d1 > 0.0 {
            if d2 >= 0.0 {
                return None;
            }
            startout = true;
            enter = enter.max(d1 / (d1 - d2));
        } else if d2 > 0.0 {
            leave = leave.min(d1 / (d1 - d2));
        }
    }
    (!planes.is_empty() && (!startout || enter < leave)).then_some(enter)
}

/// Q3 `cm_trace.c` `CM_TraceThroughBrush`, on planes already expanded by the
/// shape, which is retail's own brush arm (`cod_lnxded` 0x8054e90, the
/// `sphere.use` branch: `dist + radius` per side, the sphere nearest the
/// side).
///
/// `sphere` is the capsule's sphere offset: each plane is tested against the
/// sphere nearest it, with `start` and `end` already at the capsule's
/// centre.
pub(super) fn clip_segment(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    planes: &[(Vec3, f32)],
    side_flags: &[u32],
    sphere: Vec3,
    prim: Prim,
) {
    let mut enter = -1.0f32;
    let mut leave = 1.0f32;
    let mut clip_normal = Vec3::ZERO;
    let mut clip_side = 0;
    let mut getout = false;
    let mut startout = false;

    for (k, &(n, d)) in planes.iter().enumerate() {
        // The sphere nearest the plane is the one the plane's normal points
        // away from.
        let shift = if n.dot(sphere) > 0.0 { -sphere } else { sphere };
        let d1 = n.dot(start + shift) - d;
        let d2 = n.dot(end + shift) - d;
        if d2 > 0.0 {
            getout = true;
        }
        if d1 > 0.0 {
            startout = true;
        }
        // entirely in front of this face
        if d1 > 0.0 && (d2 >= SURFACE_CLIP_EPSILON || d2 >= d1) {
            return;
        }
        if d1 <= 0.0 && d2 <= 0.0 {
            continue;
        }
        if d1 > d2 {
            // entering
            let f = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
            if f > enter {
                enter = f;
                clip_normal = n;
                clip_side = k;
            }
        } else {
            // leaving
            let f = (d1 + SURFACE_CLIP_EPSILON) / (d1 - d2);
            if f < leave {
                leave = f;
            }
        }
    }

    if !startout {
        trace.startsolid = true;
        if !getout {
            trace.allsolid = true;
            trace.fraction = 0.0;
            trace.surface_flags = 0;
            trace.hit = Some(prim);
        }
        return;
    }
    // `<=` where Q3 has `<`; it only differs for a zero-thickness slab, which
    // no brush has.
    if enter <= leave && enter > -1.0 {
        let fraction = enter.max(0.0);
        if trace.hit.is_some() {
            note_contact(f64::from(fraction), f64::from(trace.fraction));
        }
        if fraction < trace.fraction || (fraction == trace.fraction && enter > trace.enter) {
            trace.fraction = fraction;
            trace.enter = enter;
            trace.normal = clip_normal;
            trace.surface_flags = side_flags.get(clip_side).copied().unwrap_or(0);
            trace.hit = Some(prim);
        }
    }
}

thread_local! {
    /// Set when a sweep met two contacts close enough that which one it
    /// reports rides the order it clipped them in: brushes entered at
    /// fractions a few float steps apart, or a terrain contact within its
    /// `FRACTION_EPS` back-off of another. Retail's leaf order settles those
    /// (`walk_tree`).
    pub(super) static NEAR_TIE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How far apart two contacts' fractions may sit and still be ordered by
/// the walk: twice the terrain back-off, and a margin over a brush's float
/// steps.
pub(super) const TIE_WINDOW: f64 = 2.0 * crate::terrain::FRACTION_EPS as f64 + 1e-6;

/// Notes a contact at `candidate` met while the trace already held one at
/// `fraction`, kept or not.
pub(crate) fn note_contact(candidate: f64, fraction: f64) {
    if (candidate - fraction).abs() <= TIE_WINDOW {
        NEAR_TIE.set(true);
    }
}

/// Retail's brush clip (`cod_lnxded` 0x8054e90, the capsule arm) on a
/// brush whose first six planes are the axial ones, `-x, +x, -y, +y, -z,
/// +z`, as `brush_side_planes` lays them out. The axial sides take the
/// capsule as its box, `|sphere| + r` a side (`(r, r, half height)`
/// upright); every other side tests the sphere nearer it against its
/// distance pushed out by the radius (0x8055080-0x8055176). `sphere` is the
/// sphere offset, `(0, 0, half height - radius)` but in a turned model's
/// frame, where `CM_TransformedBoxTrace` turns it (movers doc 21). The side
/// walk is [`BrushSides::clip`].
pub(super) fn clip_brush_capsule(
    trace: &mut Trace,
    sw: &CapsuleSweep,
    sphere: Vec3,
    planes: &[(Vec3, f32)],
    side_flags: &[u32],
    prim: Prim,
    arith: Arith,
    same_model: bool,
) {
    let d = f64::from;
    let r = sw.radius;
    // The capsule's box in the brush's frame: each axis the sphere offset's
    // reach plus the radius (0x8056310's `|offset| + radius`, read at
    // `+0x10c`), `(r, r, half height)` for an upright one.
    let ext = [0, 1, 2].map(|i| sphere[i].abs() + r);
    let (start, end) = (Vec3::from(sw.start), Vec3::from(sw.end));
    let msvc = arith == Arith::Msvc;
    let far = |n: Vec3, dist: f32| {
        let toward = if crate::x87::dot(n, sphere) > 0.0 {
            -sphere
        } else {
            sphere
        };
        // Windows keeps the pushed distance and the shifted start on the
        // stack and stores only the shifted end (CoDMP15.exe 0x4251c7,
        // 0x4251fb, 0x42521e).
        let pushed = if msvc { d(dist) + d(r) } else { d(dist + r) };
        let d1 = if msvc {
            let (p, n) = (start.as_dvec3() + toward.as_dvec3(), n.as_dvec3());
            (n.x * p.x + n.y * p.y) + n.z * p.z - pushed
        } else {
            crate::x87::dot(n, start + toward) - pushed
        };
        (d1, crate::x87::dot(n, end + toward) - pushed)
    };
    let sides = BrushSides {
        start: sw.start,
        end: sw.end,
        ext,
        planes,
        side_flags,
        prim,
        arith,
        same_model,
    };
    sides.clip(trace, far);
}

/// The box arm of the same clip (UO `coduo_lnxded` 0x8058f0a, the arm at
/// 0x80593dc), a plain `trap_Trace` box's: start and end moved to the box's
/// centre, the axial sides against its half size, every other side's
/// distance pulled in by the box corner its normal points away from (Q3's
/// `offsets[signbits]`) and stored. Its side walk is [`BrushSides::clip`],
/// the capsule arm's: a leave fraction takes no `SURFACE_CLIP_EPSILON`
/// (coduo-vehicles.md 17.5).
#[expect(clippy::too_many_arguments)]
pub(super) fn clip_brush_box(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    planes: &[(Vec3, f32)],
    side_flags: &[u32],
    prim: Prim,
    arith: Arith,
    same_model: bool,
) {
    let off = (mins + maxs) * 0.5;
    let (s, e) = (start + off, end + off);
    let (lo, hi) = (mins - off, maxs - off);
    let far = |n: Vec3, dist: f32| {
        let corner = Vec3::new(
            if n.x < 0.0 { hi.x } else { lo.x },
            if n.y < 0.0 { hi.y } else { lo.y },
            if n.z < 0.0 { hi.z } else { lo.z },
        );
        let pulled = arith.var(f64::from(dist) - crate::x87::dot(corner, n));
        (
            crate::x87::dot(n, s) - pulled,
            crate::x87::dot(n, e) - pulled,
        )
    };
    let sides = BrushSides {
        start: s.to_array(),
        end: e.to_array(),
        ext: hi.to_array(),
        planes,
        side_flags,
        prim,
        arith,
        same_model,
    };
    sides.clip(trace, far);
}

/// One brush as both arms of retail's brush clip walk it.
struct BrushSides<'a> {
    start: [f32; 3],
    end: [f32; 3],
    /// The shape's reach along each axis, which the axial sides add.
    ext: [f32; 3],
    planes: &'a [(Vec3, f32)],
    side_flags: &'a [u32],
    prim: Prim,
    arith: Arith,
    same_model: bool,
}

impl BrushSides<'_> {
    /// The side walk: the axial sides in the binary's order (mins x, y, z,
    /// then maxs), then the rest, each through `far`'s distances. Distances
    /// and fractions stay on the x87 stack but the enter fraction, which is
    /// stored and compared as a float, and the leave fraction starts at the
    /// trace's and takes no epsilon. 1.5b's build (0x80590d6) stores each
    /// side's distances, their difference and both fractions, which `arith`
    /// says. A trace a brush clips always lands closer than the trace's
    /// fraction, so the lead side replaces the trace's contact outright.
    fn clip(&self, trace: &mut Trace, mut far: impl FnMut(Vec3, f32) -> (f64, f64)) {
        let &BrushSides {
            start,
            end,
            ext,
            planes,
            side_flags,
            prim,
            arith,
            same_model,
        } = self;
        let d = f64::from;
        let var = |x: f64| arith.var(x);
        let mut enter = 0.0f32;
        let frac0 = d(trace.fraction);
        let mut leave = frac0;
        let mut allsolid = true;
        let mut lead: Option<(Vec3, usize)> = None;
        let contact = trace.hit.is_some();
        // A contact that repeats the trace's own (same fraction, side and
        // surface, a brush of the same model) reads alike in either order.
        let (frac_f, normal0, flags0) = (trace.fraction, trace.normal, trace.surface_flags);
        let repeats = |f: f32, normal: Vec3, k: usize| {
            same_model
                && f == frac_f
                && normal == normal0
                && side_flags.get(k).copied().unwrap_or(0) == flags0
        };
        // One side's distances; `false` when the side rules the brush out.
        // The Windows clients store `d1`, the enter path's gap and both
        // fractions only (CoDMP15.exe 0x424fc0, cod15-movement.md 8).
        let gcc = |x: f64| arith.gcc_var(x);
        let mut side = |d1: f64, d2: f64, normal: Vec3, k: usize| -> bool {
            let (d1, d2) = (var(d1), gcc(d2));
            if d1 <= 0.0 {
                let gap = gcc(d1 - d2);
                if d2 > 0.0 {
                    allsolid = false;
                    if leave * gap < d1 {
                        leave = var(d1 / gap);
                        if leave <= d(enter) {
                            return false;
                        }
                    }
                }
                return true;
            }
            let gap = var(d1 - d2);
            if d2 > 0.0 {
                if gap <= 0.0 || d(SURFACE_CLIP_EPSILON) <= d2 {
                    return false;
                }
                allsolid = false;
            }
            let back = gcc(d1 - d(SURFACE_CLIP_EPSILON));
            if d(enter) * gap < back {
                let e = back / gap;
                enter = e as f32;
                if leave <= gcc(e) {
                    // Entering at about the trace's fraction is a tie the walk
                    // order settles (`walk_tree`).
                    if contact && leave == frac0 && !repeats(e as f32, normal, k) {
                        note_contact(gcc(e), frac0);
                    }
                    return false;
                }
                lead = Some((normal, k));
            } else if lead.is_none() {
                lead = Some((normal, k));
            }
            true
        };
        for (k, sign) in [(0usize, -1.0f64), (1, 1.0)] {
            for axis in 0..3 {
                let (n, dist) = planes[axis * 2 + k];
                let bound = d(dist) * sign;
                let d1 = (d(start[axis]) - bound) * sign - d(ext[axis]);
                let d2 = (d(end[axis]) - bound) * sign - d(ext[axis]);
                if !side(d1, d2, n, axis * 2 + k) {
                    return;
                }
            }
        }
        for (k, &(n, dist)) in planes.iter().enumerate().skip(6) {
            let (d1, d2) = far(n, dist);
            if !side(d1, d2, n, k) {
                return;
            }
        }
        match lead {
            None => {
                trace.startsolid = true;
                if allsolid {
                    trace.allsolid = true;
                    trace.fraction = 0.0;
                    trace.surface_flags = 0;
                    trace.hit = Some(prim);
                }
            }
            Some((normal, k)) => {
                if contact && !repeats(enter, normal, k) {
                    note_contact(f64::from(enter), frac0);
                }
                trace.fraction = enter;
                trace.enter = enter;
                trace.normal = normal;
                trace.surface_flags = side_flags.get(k).copied().unwrap_or(0);
                trace.hit = Some(prim);
            }
        }
    }
}

/// Retail's static-model clip (`cod_lnxded` 0x80c203c) on one triangle,
/// which is also its point-vs-terrain arm (0x8052894, the same epsilons):
/// the bare segment enters through the front face (`start` on or ahead of
/// the plane, `end` behind it), the fraction backs off
/// `SURFACE_CLIP_EPSILON` along the segment, and the crossing point has to
/// land inside the edge planes within `MODEL_BARY_EPS`. A closer hit
/// replaces the trace's; a tie does not, unless `takes_tie` (the trace
/// holds an entity's contact, which retail clips after the static models).
/// Retail leaves a fraction under zero as it is, which pmove reads the same
/// as zero (it moves the origin only for a positive one); ours clamps.
/// Whether the hit replaced the trace's.
pub(super) fn clip_segment_model(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    mt: &ModelTri,
    prim: Prim,
    takes_tie: bool,
) -> bool {
    let [a, b, c] = mt.tri;
    let n = (b - a).cross(c - a).normalize();
    let d1 = n.dot(start - a);
    let d2 = n.dot(end - a);
    if !(d2 < 0.0 && d1 >= 0.0) {
        return false;
    }
    let f = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
    if f > trace.fraction || f == trace.fraction && !takes_tie {
        return false;
    }
    let p = start + (end - start) * (d1 / (d1 - d2));
    // Barycentrics of `p` in the triangle's plane: p = a + u (c - a) + v (b - a).
    let (e1, e2, w) = (c - a, b - a, p - a);
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let (dw1, dw2) = (w.dot(e1), w.dot(e2));
    let det = d11 * d22 - d12 * d12;
    if det.abs() < 1e-12 {
        return false;
    }
    let u = (d22 * dw1 - d12 * dw2) / det;
    let v = (d11 * dw2 - d12 * dw1) / det;
    if u < -MODEL_BARY_EPS || v < -MODEL_BARY_EPS || u + v > 1.0 + MODEL_BARY_EPS {
        return false;
    }
    trace.fraction = f.max(0.0);
    trace.enter = f;
    trace.normal = n;
    trace.surface_flags = mt.surface_flags;
    trace.hit = Some(prim);
    true
}

/// The nearest hit closer than `fraction` of a point segment on one model's
/// collision triangles, in whatever frame the triangles and the segment
/// share: its fraction, normal and surface flags. Retail's entity pass runs
/// the same clip on a linked xmodel entity, with the segment moved into the
/// entity's frame (docs/research/cod11-combat.md 2.7).
pub fn clip_model_tris(
    start: Vec3,
    end: Vec3,
    tris: &[ModelTri],
    mask: u32,
    fraction: f32,
) -> Option<(f32, Vec3, u32)> {
    let mut trace = Trace {
        fraction,
        ..Trace::clear(end)
    };
    for (i, mt) in tris.iter().enumerate() {
        if mt.contents & mask != 0 {
            clip_segment_model(&mut trace, start, end, mt, Prim::Model(i as u32), false);
        }
    }
    trace
        .hit
        .map(|_| (trace.fraction, trace.normal, trace.surface_flags))
}

/// One xmodel collision surface of a posed model: its run in the model's
/// triangle list, the map from the model's frame into its bone's posed
/// frame (`bone = rot * p + pos`) and its stored bounds there.
#[derive(Clone, Debug)]
pub struct ModelSurf {
    pub tris: std::ops::Range<usize>,
    pub rot: glam::Quat,
    pub pos: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// [`clip_model_tris`] a surface at a time, as the engine's xmodel clip
/// walks them (1.1d `cod_lnxded` 0x80c203c, 1.5b 0x80cf4fe, `coduo_lnxded`
/// 0x80c4892; docs/research/cod11-combat.md 2.7.1): a surface whose contents
/// meet the mask is tried only when the segment, moved into its bone's
/// frame, enters its stored bounds before the best fraction so far
/// (0x805e387, no epsilon). A triangle a later surface carries can then
/// lose to one found first even where its backed-off fraction is smaller
/// (docs/research/coduo-vehicles.md 13.3, "A blast on a hull").
pub fn clip_model_surfs(
    start: Vec3,
    end: Vec3,
    tris: &[ModelTri],
    surfs: &[ModelSurf],
    mask: u32,
    fraction: f32,
) -> Option<(f32, Vec3, u32)> {
    let mut trace = Trace {
        fraction,
        ..Trace::clear(end)
    };
    clip_surfs(&mut trace, start, end, tris, surfs, mask, false);
    trace
        .hit
        .map(|_| (trace.fraction, trace.normal, trace.surface_flags))
}

/// The walk of [`clip_model_surfs`] into `trace`, `surfs`' runs indexing
/// `tris` and each hit naming its triangle as [`Prim::Model`]. With
/// `takes_tie` the first hit may tie the trace's fraction, the rest must
/// beat it. Whether any hit replaced the trace's.
pub(super) fn clip_surfs(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    tris: &[ModelTri],
    surfs: &[ModelSurf],
    mask: u32,
    mut takes_tie: bool,
) -> bool {
    let mut hit = false;
    for s in surfs {
        let Some(first) = tris.get(s.tris.start) else {
            continue;
        };
        if first.contents & mask == 0 {
            continue;
        }
        let (bs, be) = (s.rot * start + s.pos, s.rot * end + s.pos);
        if segment_misses_box(bs, be, s.mins, s.maxs, trace.fraction) {
            continue;
        }
        for i in s.tris.clone() {
            if clip_segment_model(
                trace,
                start,
                end,
                &tris[i],
                Prim::Model(i as u32),
                takes_tie,
            ) {
                hit = true;
                takes_tie = false;
            }
        }
    }
    hit
}

/// `coduo_lnxded` 0x805e387 (1.1d `cod_lnxded` 0x805a788, 1.5b 0x805e553):
/// whether the segment's part up to `max` misses the box, slab by slab, the
/// minimum faces first.
pub fn segment_misses_box(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, max: f32) -> bool {
    let (mut lo, mut hi) = (0.0f32, max);
    for (sign, b) in [(-1.0f32, mins), (1.0, maxs)] {
        for i in 0..3 {
            let f1 = (start[i] - b[i]) * sign;
            let f2 = (end[i] - b[i]) * sign;
            if 0.0 < f1 {
                if 0.0 < f2 {
                    return true;
                }
                if lo * (f1 - f2) < f1 {
                    lo = f1 / (f1 - f2);
                    if hi <= lo {
                        return true;
                    }
                }
            } else if 0.0 < f2 && hi * (f1 - f2) < f1 {
                hi = f1 / (f1 - f2);
                if hi <= lo {
                    return true;
                }
            }
        }
    }
    false
}

/// What a brush clip sweeps: the capsule every movement trace is, or the
/// plain box of retail's `trap_Trace`, which `G_MoverPush` tests bodies with.
#[derive(Clone, Copy)]
pub(super) enum Shape {
    /// The capsule and the box it was made of.
    Capsule(Capsule, Vec3, Vec3),
    Box(Vec3, Vec3),
}

/// A brush's planes pushed out by the capsule's radius, Q3's
/// `dist = plane->dist + tw->sphere.radius`.
pub(super) fn expand_brush(planes: &[(Vec3, f32)], radius: f32, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    for &(n, d) in planes {
        out.push((n, d + radius));
    }
}

/// Whether a sweep reaches `patch`'s clip at all: a position test always
/// does, a moving sweep when its bounds meet the patch's box.
pub(super) fn patch_meets(
    start: Vec3,
    end: Vec3,
    capsule: Capsule,
    patch: &WorldPatch,
    arith: Arith,
) -> bool {
    if start == end {
        return true;
    }
    let collide = patch.clip(arith);
    let (lo, hi) = (collide.mins, collide.maxs);
    let (s, e) = (
        (start + capsule.center).to_array(),
        (end + capsule.center).to_array(),
    );
    if capsule.radius == 0.0 {
        crate::terrain::segment_bounds_meet(lo, hi, s, e, arith)
    } else {
        crate::terrain::capsule_bounds_meet(lo, hi, s, e, capsule.offset.z, capsule.radius)
    }
}

/// One patch against a trace, retail's `CM_TraceThroughPatch` arm of the
/// leaf walk (`cod_lnxded` 0x8055608): a point takes the patch's point
/// clip, anything else its capsule; a trace that does not move is a
/// position test (`CM_PositionTestInPatchCollide`), the only way a patch
/// reads `startsolid`. A moving trace reaches the clip only when its bounds
/// meet the patch's box, the grid's grown by a unit (0x8054c48 at
/// 0x80556e9). A hit replaces the trace's only when it is strictly closer,
/// retail's `enterFrac < tw->trace.fraction`. `arith` picks the build of
/// the patch and of its trace.
pub(super) fn clip_patch(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    capsule: Capsule,
    patch: &WorldPatch,
    arith: Arith,
    prim: Prim,
) {
    let collide = patch.clip(arith);
    let sweep = if capsule.radius == 0.0 {
        Sweep::Point
    } else {
        Sweep::Capsule {
            radius: capsule.radius,
            offset: capsule.offset.z,
        }
    };
    let (s, e) = (start + capsule.center, end + capsule.center);
    if start == end {
        if collide.position_test(s.to_array(), sweep) {
            trace.startsolid = true;
            trace.allsolid = true;
            trace.fraction = 0.0;
            trace.hit = Some(prim);
        }
        return;
    }
    if !patch_meets(start, end, capsule, patch, arith) {
        return;
    }
    let (s, e) = (s.to_array(), e.to_array());
    let Some(hit) = collide.trace(s, e, sweep, trace.fraction, arith) else {
        return;
    };
    trace.fraction = hit.fraction;
    trace.enter = hit.raw;
    trace.normal = Vec3::from_array(hit.normal);
    trace.surface_flags = patch.surface_flags;
    trace.hit = Some(prim);
}

/// The sweep `CM_BoxTrace` (`cod_lnxded` 0x8056310) hands the terrain
/// clip: start and end moved to the box centre, the delta between them, and
/// the capsule's radius (the smaller of the half width and the half height)
/// and half height (0x80563d1-0x805644b).
pub(super) fn capsule_sweep(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> CapsuleSweep {
    let off = (mins + maxs) * 0.5;
    let (s, e) = (start + off, end + off);
    let delta: [f32; 3] =
        std::array::from_fn(|i| (f64::from(end[i]) + f64::from(off[i]) - f64::from(s[i])) as f32);
    let sq = |i: usize| f64::from(delta[i]) * f64::from(delta[i]);
    let size = maxs - off;
    CapsuleSweep {
        start: s.to_array(),
        end: e.to_array(),
        delta,
        delta_sq: ((sq(0) + sq(1)) + sq(2)) as f32,
        radius: if size.z < size.x { size.z } else { size.x },
        half_height: size.z,
    }
}
