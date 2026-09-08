//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! Brush clip planes, render triangles and a BVH from the BSP collision
//! lumps, swept by a Q3-style AABB `box_trace`. Layouts and why triangles
//! are required: docs/research/bsp-ibsp59-format.md, "Terrain has no brushes".

use crate::bsp::Bsp;
use glam::Vec3;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared with the xmodel collision surfaces, which use the same bits.
pub const CONTENTS_SOLID: u32 = 0x1;
/// Window panes (`glass@brokenwindow`, `dam_window`: 0x2090, 0x8000010,
/// 0x28000010 over the stock maps) and lamp glass on xmodels. In retail's
/// player mask, so a player stops at a window and a bullet does not
/// (docs/research/bsp-ibsp59-format.md, "Content flags").
pub const CONTENTS_GLASS: u32 = 0x10;
const CONTENTS_PLAYERCLIP: u32 = 0x10000;
const CONTENTS_SKY: u32 = 0x800;
/// Census-proven water bit: docs/research/bsp-ibsp59-format.md, "Content flags".
pub const CONTENTS_WATER: u32 = 0x20;
/// Ladder-climb flag on brush materials; pmove grabs ladders from trace hits carrying it.
pub const SURF_LADDER: u32 = 0x8;
/// Brushless terrain is the one collidable material word without SOLID or
/// PLAYERCLIP (bsp-ibsp59-format.md, "Content flags").
const CONTENTS_TERRAIN: u32 = 0x4;

/// Retail's player tracemask, the `pm->tracemask` `ClientThink_real` stores
/// for a live player (docs/research/cod11-mantle.md, "The player is a
/// capsule"): SOLID, GLASS, PLAYERCLIP and two bits (0x800000, 0x2000000)
/// no stock material carries.
pub const MASK_PLAYERSOLID: u32 = 0x2810011;
/// The mask `Bullet_Fire_Extended` (game.mp 0x78890) hands
/// `trap_LocationalTrace`: SOLID, GLASS, WATER, 0x2000 (the kerb and floor
/// brushes' 0x2080 word) and the same two high bits. No PLAYERCLIP, so a
/// masked wire fence stops a player and not a bullet.
pub const MASK_SHOT: u32 = 0x2802031;
/// `CanDamage`'s (0x5a098) mask for a blast's five probes: the shot mask
/// with 0x80 for 0x20.
pub const MASK_BLAST: u32 = 0x2802091;
/// What a grenade flies against: `G_RunMissile` (game.mp 0x63fcc) traces
/// with the entity's `clipmask` and falls back to 0x11, SOLID and GLASS,
/// and no store into `fire_grenade`'s entity (0x643ac) was found.
pub const MASK_MISSILE: u32 = CONTENTS_SOLID | CONTENTS_GLASS;
const TRACE_MASK_MOVE: u32 = MASK_PLAYERSOLID;
const TRACE_MASK_SHOT: u32 = MASK_SHOT;

/// A brush as clip planes: point p is inside iff n·p <= d for every plane.
pub struct BrushPlanes {
    pub planes: Vec<(Vec3, f32)>,
    /// Lump-0 surface flags of the brush's material (SURF_LADDER and friends).
    pub surface_flags: u32,
    /// Lump-0 content flags, reported by `point_contents`.
    pub content_flags: u32,
    /// The brush's index in lump 4 and its material's name, for `describe`.
    pub bsp_index: u32,
    pub material: String,
    /// The lump-27 model the brush belongs to; 0 is the world.
    pub model: u32,
}

/// One collision triangle of a placed static xmodel. Retail's server clips
/// static models as the bare start-to-end segment of a trace, whatever box
/// or capsule the trace carries, one surface at a time against
/// `contents & mask` (docs/research/cod11-mantle.md, "Static models are
/// clipped as a segment"); the sweep shape never touches these.
#[derive(Clone, Copy, Debug)]
pub struct ModelTri {
    /// Wound so `cross(b - a, c - a)` is the surface's outward normal.
    pub tri: [Vec3; 3],
    /// The xmodel collision surface's `contents` word.
    pub contents: u32,
    /// The surface's `surf_flags`, the sound material in bits 20-24.
    pub surface_flags: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum Prim {
    Brush(u32),
    Tri(u32),
    Model(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub fraction: f32, // 1.0 = made it to end
    pub endpos: Vec3,
    pub normal: Vec3, // valid when fraction < 1.0
    pub surface_flags: u32,
    pub startsolid: bool,
    pub allsolid: bool,
    /// What the reported contact is against, for diagnostics (`describe`).
    pub hit: Option<Prim>,
    /// The hit's unclamped enter fraction. A box touching two surfaces
    /// clips both at fraction 0, and the one it sits closest to (the
    /// largest raw value) is the contact reported, so a 0.25-unit ground
    /// trace and a 9-unit snap agree on the normal at a mesh seam.
    enter: f32,
}

pub const SURFACE_CLIP_EPSILON: f32 = 0.125;
/// How far outside an edge plane the static-model clip still counts a
/// crossing as inside the triangle (`cod_lnxded` rodata 0x80db974 and
/// 0x80db978: -0.001 and 1.001).
const MODEL_BARY_EPS: f32 = 0.001;
/// What retail's terrain clip takes off every fraction it returns
/// (`cod_lnxded` rodata 0x80cd30c); a fraction at or under it is a
/// `startsolid` at 0. The radius pad is `SURFACE_CLIP_EPSILON` (0x80cd308).
const TERRAIN_FRACTION_EPS: f32 = 1e-5;
/// How far behind a facet's face a start still counts as resting on it.
/// A 30-unit shape straddling a convex seam is inside the uphill facet's
/// slab by half its width times the grade change, 8.7 units at 30 degrees.
const SEAM_DEPTH: f32 = 8.0;

/// The 5-bit sound-surface index every trace consumer reads
/// (`cod11-events-and-fx.md`, section 4). 0 means the material carries no
/// surfaceparm and retail emits no footstep for it.
pub fn sound_material(surface_flags: u32) -> i32 {
    ((surface_flags >> 20) & 0x1f) as i32
}

/// Build-time soup filter. Census over all 49 stock maps (`flag_census`
/// example): the walk-through cutouts (bushwalls, treelines, ground decals,
/// autosprite wire, sfx water) are TRANSLUCENT/WINDOW/DETAIL words with no
/// clip bit; everything collidable carries SOLID or PLAYERCLIP except bare
/// terrain 0x4. Kept tris store only their mask-relevant bits.
fn tri_contents(content_flags: u32) -> Option<u32> {
    if content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP) != 0 {
        Some(content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP))
    } else if content_flags == CONTENTS_TERRAIN {
        Some(CONTENTS_SOLID)
    } else {
        None
    }
}

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
struct Capsule {
    /// From the trace origin to the box centre.
    center: Vec3,
    radius: f32,
    offset: Vec3,
}

impl Capsule {
    fn of(mins: Vec3, maxs: Vec3) -> Capsule {
        let half = (maxs - mins) * 0.5;
        let radius = half.x.min(half.z);
        Capsule {
            center: (mins + maxs) * 0.5,
            radius,
            offset: Vec3::Z * (half.z - radius),
        }
    }
}

/// Q3 `cm_trace.c` `CM_TraceThroughBrush`, on planes already expanded by the
/// shape, which is retail's own brush arm (`cod_lnxded` 0x8054e90, the
/// `sphere.use` branch: `dist + radius` per side, the sphere nearest the
/// side) and, on a facet's planes, its patch arm (Q3's
/// `CM_TraceThroughPatchCollide`, every border and bevel `+= radius`).
///
/// `sphere` is the capsule's sphere offset: each plane is tested against the
/// sphere nearest it, with `start` and `end` already at the capsule's
/// centre. `hollow` is a facet's clip. A start inside the expanded slab of a
/// zero-thickness facet is never `startsolid`, the way Q3's patch facets
/// never are: a shape resting on one facet sits inside the neighbouring
/// facet's slab at every convex seam, and beside a kerb it is inside the
/// top face's slab, and reading either as solid made the ground trace fail
/// and trapped the walker (docs/research/cod11-mantle.md, "The ground
/// snap"). A start within `SEAM_DEPTH` behind a face is resting on that
/// face: a fraction-0 hit with the face's normal when the trace moves into
/// it, nothing when it moves out. Deeper than that the facet does not clip
/// the trace at all. Q3's facets trust their winding, which a soup does
/// not, so both faces of the slab clip an entry from outside.
#[allow(clippy::too_many_arguments)]
fn clip_segment(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    planes: &[(Vec3, f32)],
    surface_flags: u32,
    sphere: Vec3,
    hollow: bool,
    prim: Prim,
) {
    let mut enter = -1.0f32;
    let mut leave = 1.0f32;
    let mut clip_normal = Vec3::ZERO;
    let mut getout = false;
    let mut startout = false;
    // The face pair's distances, for the resting test below.
    let mut faces = [(0.0f32, 0.0f32); 2];

    for (i, &(n, d)) in planes.iter().enumerate() {
        // The sphere nearest the plane is the one the plane's normal points
        // away from.
        let shift = if n.dot(sphere) > 0.0 { -sphere } else { sphere };
        let d1 = n.dot(start + shift) - d;
        let d2 = n.dot(end + shift) - d;
        if i < 2 {
            faces[i] = (d1, d2);
        }
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
        if hollow {
            // `triangle_planes` pushes the face first and its reverse second;
            // the one the start is closer to is the surface it rests on.
            let i = usize::from(faces[1].0 > faces[0].0);
            let (d1, d2) = faces[i];
            if d1 > -SEAM_DEPTH && d1 > d2 {
                let enter = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
                if trace.fraction > 0.0 || enter > trace.enter {
                    trace.fraction = 0.0;
                    trace.enter = enter;
                    trace.normal = planes[i].0;
                    trace.surface_flags = surface_flags;
                    trace.hit = Some(prim);
                }
            }
            return;
        }
        trace.startsolid = true;
        if !getout {
            trace.allsolid = true;
            trace.fraction = 0.0;
            trace.surface_flags = 0;
            trace.hit = Some(prim);
        }
        return;
    }
    // `<=`, not Q3's `<`: a zero-thickness facet's paired face planes make
    // enter == leave for a grazing ray. Brushes have thickness, so unaffected.
    if enter <= leave && enter > -1.0 {
        let fraction = enter.max(0.0);
        if fraction < trace.fraction || (fraction == trace.fraction && enter > trace.enter) {
            trace.fraction = fraction;
            trace.enter = enter;
            trace.normal = clip_normal;
            trace.surface_flags = surface_flags;
            trace.hit = Some(prim);
        }
    }
}

/// Retail's static-model clip (`cod_lnxded` 0x80c203c) on one triangle,
/// which is also its point-vs-terrain arm (0x8052894, the same epsilons):
/// the bare segment enters through the front face (`start` on or ahead of
/// the plane, `end` behind it), the fraction backs off
/// `SURFACE_CLIP_EPSILON` along the segment, and the crossing point has to
/// land inside the edge planes within `MODEL_BARY_EPS`. A closer hit
/// replaces the trace's; a tie does not. Retail leaves a fraction under
/// zero as it is, which pmove reads the same as zero (it moves the origin
/// only for a positive one); ours clamps.
fn clip_segment_model(trace: &mut Trace, start: Vec3, end: Vec3, mt: &ModelTri, prim: Prim) {
    let [a, b, c] = mt.tri;
    let n = (b - a).cross(c - a).normalize();
    let d1 = n.dot(start - a);
    let d2 = n.dot(end - a);
    if !(d2 < 0.0 && d1 >= 0.0) {
        return;
    }
    let f = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
    if f >= trace.fraction {
        return;
    }
    let p = start + (end - start) * (d1 / (d1 - d2));
    // Barycentrics of `p` in the triangle's plane: p = a + u (c - a) + v (b - a).
    let (e1, e2, w) = (c - a, b - a, p - a);
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let (dw1, dw2) = (w.dot(e1), w.dot(e2));
    let det = d11 * d22 - d12 * d12;
    if det.abs() < 1e-12 {
        return;
    }
    let u = (d22 * dw1 - d12 * dw2) / det;
    let v = (d11 * dw2 - d12 * dw1) / det;
    if u < -MODEL_BARY_EPS || v < -MODEL_BARY_EPS || u + v > 1.0 + MODEL_BARY_EPS {
        return;
    }
    trace.fraction = f.max(0.0);
    trace.enter = f;
    trace.normal = n;
    trace.surface_flags = mt.surface_flags;
    trace.hit = Some(prim);
}

/// A brush's planes pushed out by the capsule's radius, Q3's
/// `dist = plane->dist + tw->sphere.radius`.
fn expand_brush(planes: &[(Vec3, f32)], radius: f32, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    for &(n, d) in planes {
        out.push((n, d + radius));
    }
}

/// Planes for a facet swept by a capsule of `radius`: face, axis bevels,
/// edge x axis bevels, each pushed out by the radius the way Q3 expands a
/// patch facet for a sphere (`cm_patch.c`, `CM_AddFacetBevels` for the
/// planes, `plane[3] += tw->sphere.radius` in the trace). This is what a
/// patch's render soup gets, and what a brush face's soup gets, which the
/// brush behind it settles anyway (docs/research/cod11-mantle.md, "Terrain
/// is a swept sphere, a patch is a facet"). Edge hits report the bevel
/// normal, which lets pmove slide around edges.
fn triangle_planes(tri: &[Vec3; 3], radius: f32, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    let mut push = |n: Vec3| {
        let d_tri = n.dot(tri[0]).max(n.dot(tri[1])).max(n.dot(tri[2]));
        out.push((n, d_tri + radius));
    };
    let face = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();
    push(face);
    push(-face);
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        push(axis);
        push(-axis);
    }
    for k in 0..3 {
        let edge = tri[(k + 1) % 3] - tri[k];
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let c = edge.cross(axis);
            if c.length_squared() > 1e-8 {
                let c = c.normalize();
                push(c);
                push(-c);
            }
        }
    }
}

/// Retail's terrain clip (`cod_lnxded` 0x8052a58), one triangle at a time:
/// the capsule's sphere nearest the face is swept against the face, and
/// when its contact point projects outside the triangle, against the edges
/// as cylinders and the vertices as spheres. Nothing is bevelled, so a
/// sphere walking up a ramp into a flat is not lifted onto the flat's
/// radius-wide slab a facet would put beside it
/// (docs/research/cod11-mantle.md, "Terrain is a swept sphere, a patch is a
/// facet"). The face is one-sided: a start deeper than
/// the padded radius behind it is solid only where the capsule's axis
/// crosses the triangle, else the triangle is skipped. Every fraction loses
/// `TERRAIN_FRACTION_EPS`, and one at or under it is a `startsolid` at 0.
/// Retail picks the sphere per terrain partition off a stored facing flag;
/// ours takes the one nearest the plane, which is that flag for a floor and
/// for a ceiling. A point trace takes retail's point arm instead
/// (0x8052894): the front face alone, backed off `SURFACE_CLIP_EPSILON`
/// along the segment, the crossing inside the edges within
/// `MODEL_BARY_EPS`, no fraction epsilon and no `startsolid`.
///
/// `start` and `end` are the capsule's centre; `tri` is wound so
/// `cross(b - a, c - a)` faces out.
fn clip_sphere_triangle(
    trace: &mut Trace,
    start: Vec3,
    end: Vec3,
    tri: &[Vec3; 3],
    capsule: Capsule,
    surface_flags: u32,
    prim: Prim,
) {
    let [a, b, c] = *tri;
    let n = (b - a).cross(c - a).normalize();
    if capsule.radius == 0.0 {
        let mt = ModelTri {
            tri: *tri,
            contents: 0,
            surface_flags,
        };
        clip_segment_model(trace, start, end, &mt, prim);
        return;
    }
    let shift = if n.dot(capsule.offset) > 0.0 {
        -capsule.offset
    } else {
        capsule.offset
    };
    let (s, e) = (start + shift, end + shift);
    let r_eps = capsule.radius + SURFACE_CLIP_EPSILON;
    let d_e = n.dot(e - a);
    if d_e >= r_eps {
        return;
    }
    let d_s = n.dot(s - a);
    if d_s - d_e <= 0.0 {
        return;
    }
    // Barycentrics of `p` projected along `n`: p = a + u (c - a) + v (b - a),
    // and which edges it lies outside of, as retail's three bits.
    let (e1, e2) = (c - a, b - a);
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let det = d11 * d22 - d12 * d12;
    if det.abs() < 1e-12 {
        return;
    }
    let outside = |p: Vec3| -> u32 {
        let w = p - a;
        let (dw1, dw2) = (w.dot(e1), w.dot(e2));
        let u = (d22 * dw1 - d12 * dw2) / det;
        let v = (d11 * dw2 - d12 * dw1) / det;
        u32::from(u + v > 1.0) | (u32::from(u < 0.0) << 1) | (u32::from(v < 0.0) << 2)
    };
    let record = |trace: &mut Trace, raw: f32, normal: Vec3| {
        if raw <= TERRAIN_FRACTION_EPS {
            trace.fraction = 0.0;
            trace.startsolid = true;
        } else {
            trace.fraction = raw - TERRAIN_FRACTION_EPS;
        }
        trace.enter = raw;
        trace.normal = normal;
        trace.surface_flags = surface_flags;
        trace.hit = Some(prim);
    };
    if d_s <= -r_eps {
        // Deep behind the face: solid where the axis to the other sphere
        // crosses the triangle, at the near pad or at the far one.
        let axis = -2.0 * shift;
        let d_o = d_s + n.dot(axis);
        if d_o <= -r_eps {
            return;
        }
        let near = s + axis * ((-r_eps - d_s) / (d_o - d_s));
        let far = if d_o < r_eps {
            s + axis
        } else {
            s + axis * ((r_eps - d_s) / (d_o - d_s))
        };
        if outside(near) == 0 || outside(far) == 0 {
            record(trace, 0.0, n);
        }
        return;
    }
    let dir = e - s;
    let f = if d_s < r_eps {
        0.0
    } else {
        (d_s - r_eps) / (d_s - d_e)
    };
    if f > trace.fraction {
        return;
    }
    let p = s + dir * f;
    let bits = outside(p);
    if bits == 0 {
        if f < trace.fraction || f > trace.enter {
            record(trace, f, n);
        }
        return;
    }
    let dir_sq = dir.length_squared();
    let r = capsule.radius;
    // Vertex i is the one opposite edge i, in retail's bit order.
    let verts = [a, c, b];
    let edges = [(c, b), (a, b), (a, c)];
    for i in 0..3 {
        if bits & (1 << i) == 0 {
            let q = s - verts[i];
            let sep = q.length_squared() - r * r;
            if sep <= 0.0 {
                record(trace, 0.0, n);
                continue;
            }
            let bq = dir.dot(q);
            if bq >= 0.0 {
                continue;
            }
            let disc = bq * bq - dir_sq * sep;
            if disc < 0.0 {
                continue;
            }
            let t = (-disc.sqrt() - bq) / dir_sq;
            if t < trace.fraction {
                record(trace, t, (q + dir * t) / r);
            }
        } else {
            let (v0, v1) = edges[i];
            let along = v1 - v0;
            let len = along.length();
            if len < 1e-6 {
                continue;
            }
            let w = along / len;
            let u_axis = n;
            let v_axis = w.cross(u_axis);
            let q = s - v0;
            let (qu, qv, qw) = (q.dot(u_axis), q.dot(v_axis), q.dot(w));
            let sep = qu * qu + qv * qv - r * r;
            if sep <= 0.0 {
                if (0.0..=len).contains(&qw) {
                    record(trace, 0.0, n);
                }
                continue;
            }
            let (du, dv, dw) = (dir.dot(u_axis), dir.dot(v_axis), dir.dot(w));
            let bq = du * qu + dv * qv;
            if bq >= 0.0 {
                continue;
            }
            let aa = du * du + dv * dv;
            let disc = bq * bq - aa * sep;
            if disc <= 0.0 {
                continue;
            }
            let t = (-disc.sqrt() - bq) / aa;
            if t < trace.fraction && (0.0..=len).contains(&(qw + t * dw)) {
                let normal = (u_axis * (qu + t * du) + v_axis * (qv + t * dv)) / r;
                record(trace, t, normal);
            }
        }
    }
}

struct BvhNode {
    lo: Vec3,
    hi: Vec3,
    /// leaf: absolute prim index; internal: left child
    first: u32,
    /// internal: right child; leaf: unused
    second: u32,
    /// 0 = internal, otherwise leaf covering `count` prims starting at `first`
    count: u32,
}

pub struct CollisionWorld {
    pub brushes: Vec<BrushPlanes>,
    pub tris: Vec<[Vec3; 3]>,
    /// The placed props' collision triangles, clipped as a segment only.
    pub model_tris: Vec<ModelTri>,
    /// Per-triangle material `surface_flags`, parallel to `tris`. The sound
    /// surface rides bits 20-24 (`cod11-events-and-fx.md`, section 4).
    tris_surf: Vec<u32>,
    /// Per-triangle mask-relevant content flags, parallel to `tris`.
    tris_contents: Vec<u32>,
    /// Parallel to `tris`: a lump-26 terrain triangle, swept as a sphere,
    /// against a render soup's facet.
    tris_terrain: Vec<bool>,
    nodes: Vec<BvhNode>,
    prims: Vec<(Prim, Vec3, Vec3)>,
    water: Vec<WaterVolume>,
    /// Per lump-27 model, whether its brushes are in the clip. Retail holds
    /// a submodel's brushes only through the entity that links them, so a
    /// deleted `script_brushmodel` takes its brushes out; `set_model_linked`
    /// is that unlink.
    model_linked: Vec<AtomicBool>,
}

/// A water brush as clip planes plus its axial bounds for the cheap reject.
struct WaterVolume {
    planes: Vec<(Vec3, f32)>,
    lo: Vec3,
    hi: Vec3,
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// The triangle lists under construction in `build`.
struct Tris {
    tris: Vec<[Vec3; 3]>,
    surf: Vec<u32>,
    contents: Vec<u32>,
    terrain: Vec<bool>,
    prims: Vec<(Prim, Vec3, Vec3)>,
}

impl Tris {
    /// Drops slivers, pads the AABB by 0.25 so the BVH query finds a
    /// triangle the box merely touches. `tri` is wound with
    /// `cross(b - a, c - a)` on the outside, the side the sphere clip faces.
    fn push(&mut self, [a, b, c]: [Vec3; 3], surface_flags: u32, contents: u32, terrain: bool) {
        if (b - a).cross(c - a).length_squared() < 1e-6 {
            return;
        }
        let lo = a.min(b).min(c) - Vec3::splat(0.25);
        let hi = a.max(b).max(c) + Vec3::splat(0.25);
        self.prims.push((Prim::Tri(self.tris.len() as u32), lo, hi));
        self.tris.push([a, b, c]);
        self.surf.push(surface_flags);
        self.contents.push(contents);
        self.terrain.push(terrain);
    }
}

/// A vertex quantised to 1/8 unit, the key the terrain vertex table uses to
/// match a render soup's triangle to the lump-26 triangles it draws.
fn vkey(v: Vec3) -> [i32; 3] {
    [
        (v.x * 8.0).round() as i32,
        (v.y * 8.0).round() as i32,
        (v.z * 8.0).round() as i32,
    ]
}

impl CollisionWorld {
    /// `model_tris` are the placed props' collision triangles
    /// (`props::collision_tris`), clipped the way retail clips static
    /// models: by a point trace's segment, never by a movement trace.
    ///
    /// Every model's brushes enter (submodels translated by the entity origin
    /// from the entities lump), except those of `trigger*` entities: their
    /// brushes carry plain CONTENTS_SOLID in the lump, but retail leaves them
    /// hollow to movement. The terrain partitions of lump 24 enter as the
    /// engine's own triangles, swept as a sphere; the render soups of model
    /// 0 enter as facets, minus the ones that draw terrain (a soup whose
    /// centroid lies in a coplanar terrain triangle sharing a vertex with
    /// it; the render mesh triangulates the same grid the other way, so an
    /// edge match is not enough, and a flat patch abutting terrain shares
    /// an edge without drawing it). Submodel meshes are local-space and
    /// their brush hulls replace them.
    pub fn build(bsp: &Bsp, model_tris: &[ModelTri]) -> Self {
        let mut brushes = Vec::new();
        let mut water = Vec::new();
        let mut t = Tris {
            tris: Vec::new(),
            surf: Vec::new(),
            contents: Vec::new(),
            terrain: Vec::new(),
            prims: Vec::new(),
        };

        #[derive(Clone, Copy)]
        struct Placement {
            origin: Vec3,
            trigger: bool,
        }
        let mut placements = vec![
            Placement {
                origin: Vec3::ZERO,
                trigger: false,
            };
            bsp.models.len()
        ];
        for block in crate::bsp::entity_blocks(&bsp.entities) {
            let Some(idx) = block
                .get("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            if idx == 0 || idx >= placements.len() {
                continue;
            }
            let p = &mut placements[idx];
            if let Some([x, y, z]) = block.get("origin").and_then(|o| crate::bsp::parse_vec3(o)) {
                p.origin = Vec3::new(x, y, z);
            }
            if block
                .get("classname")
                .is_some_and(|c| c.starts_with("trigger"))
            {
                p.trigger = true;
            }
        }

        for (mi, model) in bsp.models.iter().enumerate() {
            let placement = &placements[mi];
            let brush_range =
                model.first_brush as usize..(model.first_brush + model.num_brushes) as usize;
            for (bi, b) in bsp
                .brushes
                .iter()
                .enumerate()
                .take(brush_range.end)
                .skip(brush_range.start)
            {
                let mat = &bsp.materials[b.material as usize];
                if placement.trigger
                    || mat.content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP | CONTENTS_WATER)
                        == 0
                {
                    continue;
                }
                let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                let mut planes = Vec::with_capacity(sides.len());
                let mut lo = Vec3::ZERO;
                let mut hi = Vec3::ZERO;
                for axis in 0..3 {
                    let axis_lo =
                        f32::from_bits(sides[axis * 2].plane_or_dist) + placement.origin[axis];
                    let axis_hi =
                        f32::from_bits(sides[axis * 2 + 1].plane_or_dist) + placement.origin[axis];
                    planes.push((-AXES[axis], -axis_lo));
                    planes.push((AXES[axis], axis_hi));
                    lo[axis] = axis_lo;
                    hi[axis] = axis_hi;
                }
                for s in &sides[6..] {
                    let p = &bsp.planes[s.plane_or_dist as usize];
                    let n = Vec3::from_array(p.normal);
                    planes.push((n, p.dist + n.dot(placement.origin)));
                }
                if mat.content_flags & CONTENTS_WATER != 0 {
                    water.push(WaterVolume {
                        planes: planes.clone(),
                        lo,
                        hi,
                    });
                }
                if mat.content_flags & (CONTENTS_SOLID | CONTENTS_PLAYERCLIP) != 0 {
                    let idx = brushes.len() as u32;
                    brushes.push(BrushPlanes {
                        planes,
                        surface_flags: mat.surface_flags,
                        content_flags: mat.content_flags,
                        bsp_index: bi as u32,
                        material: mat.name.clone(),
                        model: mi as u32,
                    });
                    t.prims.push((Prim::Brush(idx), lo, hi));
                }
            }
        }

        // The engine's terrain, wound the way `CM_GenerateTerrainCollide`
        // takes its plane (Q3's `PlaneFromPoints`: `cross(c - a, b - a)`),
        // and its vertices keyed for the soup pass.
        let mut terrain_by_vertex: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
        for part in &bsp.terrain {
            let mat = &bsp.materials[part.material as usize];
            let Some(contents) = tri_contents(mat.content_flags) else {
                continue;
            };
            let idx =
                &bsp.collision_indices[part.first_index as usize..][..part.index_count as usize];
            for tri in idx.as_chunks::<3>().0 {
                let p = |i: usize| {
                    Vec3::from_array(
                        bsp.collision_verts[part.first_vert as usize + tri[i] as usize],
                    )
                };
                let tri = [p(0), p(2), p(1)];
                let before = t.tris.len();
                t.push(tri, mat.surface_flags, contents, true);
                if t.tris.len() == before {
                    continue;
                }
                for v in tri {
                    terrain_by_vertex.entry(vkey(v)).or_default().push(before);
                }
            }
        }
        // A soup triangle whose centroid lies in a coplanar terrain triangle
        // sharing one of its vertices draws that terrain; the rest are
        // facets. A soup winds clockwise seen from its normal's side
        // (bsp-ibsp59-format.md, lump 6), so it is stored reversed.
        let terrain_tris = t.tris.clone();
        let draws_terrain = |tri: &[Vec3; 3]| {
            let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();
            let c = (tri[0] + tri[1] + tri[2]) / 3.0;
            tri.iter().any(|v| {
                terrain_by_vertex.get(&vkey(*v)).is_some_and(|owners| {
                    owners.iter().any(|&i| {
                        let [a, b, cc] = terrain_tris[i];
                        let n2 = (b - a).cross(cc - a).normalize();
                        if n.dot(n2) < 0.995 || (n2.dot(c - a)).abs() > 0.5 {
                            return false;
                        }
                        // Barycentrics of the centroid in that triangle's plane.
                        let (e1, e2, w) = (cc - a, b - a, c - a);
                        let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
                        let (dw1, dw2) = (w.dot(e1), w.dot(e2));
                        let det = d11 * d22 - d12 * d12;
                        if det.abs() < 1e-12 {
                            return false;
                        }
                        let u = (d22 * dw1 - d12 * dw2) / det;
                        let v = (d11 * dw2 - d12 * dw1) / det;
                        u >= -0.01 && v >= -0.01 && u + v <= 1.01
                    })
                })
            })
        };
        let world_model = &bsp.models[0];
        let soup_range = world_model.first_soup as usize
            ..(world_model.first_soup + world_model.num_soups) as usize;
        for soup in &bsp.soups[soup_range] {
            let mat = &bsp.materials[soup.material as usize];
            if mat.content_flags & CONTENTS_SKY != 0 {
                continue;
            }
            let Some(contents) = tri_contents(mat.content_flags) else {
                continue;
            };
            let idx = &bsp.indices[soup.first_index as usize..][..soup.index_count as usize];
            for tri in idx.as_chunks::<3>().0 {
                let p = |i: usize| {
                    Vec3::from_array(bsp.verts[soup.first_vertex as usize + tri[i] as usize].pos)
                };
                let tri = [p(0), p(2), p(1)];
                if !terrain_by_vertex.is_empty() && draws_terrain(&tri) {
                    continue;
                }
                t.push(tri, mat.surface_flags, contents, false);
            }
        }
        let mut model_tris_out = Vec::with_capacity(model_tris.len());
        for mt in model_tris {
            let [a, b, c] = mt.tri;
            if (b - a).cross(c - a).length_squared() < 1e-6 {
                continue;
            }
            t.prims.push((
                Prim::Model(model_tris_out.len() as u32),
                a.min(b).min(c) - Vec3::splat(0.25),
                a.max(b).max(c) + Vec3::splat(0.25),
            ));
            model_tris_out.push(*mt);
        }

        let mut nodes = Vec::new();
        if !t.prims.is_empty() {
            build_bvh(&mut t.prims, 0, &mut nodes);
        }

        CollisionWorld {
            brushes,
            tris: t.tris,
            model_tris: model_tris_out,
            tris_surf: t.surf,
            tris_contents: t.contents,
            tris_terrain: t.terrain,
            nodes,
            prims: t.prims,
            water,
            model_linked: bsp.models.iter().map(|_| AtomicBool::new(true)).collect(),
        }
    }

    /// Whether model `model`'s brushes clip. The server clears it when the
    /// `script_brushmodel` that links them is deleted, the way retail's
    /// `G_FreeEntity` unlinks (docs/research/cod11-mantle.md, "A submodel's
    /// brushes are its entity's").
    pub fn set_model_linked(&self, model: usize, linked: bool) {
        if let Some(m) = self.model_linked.get(model) {
            m.store(linked, Ordering::Relaxed);
        }
    }

    /// Reads [`set_model_linked`](Self::set_model_linked) back. An index past
    /// the model count is `true`, matching the trace's own treatment of one.
    pub fn model_linked(&self, model: usize) -> bool {
        self.model_linked
            .get(model)
            .is_none_or(|m| m.load(Ordering::Relaxed))
    }

    fn brush_linked(&self, brush: &BrushPlanes) -> bool {
        self.model_linked(brush.model as usize)
    }

    /// Contents at a point: `CONTENTS_WATER` inside any water brush, plus the
    /// content flags of any solid brush containing it (Q3 `CM_PointContents`
    /// over the brushes that made it into the world).
    pub fn point_contents(&self, p: Vec3) -> u32 {
        let mut out = 0;
        for v in &self.water {
            if p.cmple(v.hi).all()
                && p.cmpge(v.lo).all()
                && v.planes.iter().all(|&(n, d)| n.dot(p) <= d)
            {
                out |= CONTENTS_WATER;
                break;
            }
        }
        for (prim, lo, hi) in &self.prims {
            if let Prim::Brush(b) = prim {
                if p.cmple(*hi).all() && p.cmpge(*lo).all() {
                    let brush = &self.brushes[*b as usize];
                    if self.brush_linked(brush) && brush.planes.iter().all(|&(n, d)| n.dot(p) <= d)
                    {
                        out |= brush.content_flags;
                    }
                }
            }
        }
        out
    }

    /// Movement sweep (`mins == maxs == ZERO` is a ray), retail's
    /// `trap_Trace` / `trap_TraceCapsule`: brushes and terrain, never a
    /// static model (`cod_lnxded` 0x80916f4 traces them only on the flag the
    /// locational syscall passes).
    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, TRACE_MASK_MOVE, false)
    }

    /// Bullet segment, retail's `trap_LocationalTrace`: [`MASK_SHOT`] and the
    /// static models.
    pub fn shot_trace(&self, start: Vec3, end: Vec3) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, TRACE_MASK_SHOT, true)
    }

    /// A point segment with any mask, with or without the static models:
    /// retail's `trap_Trace` on a zero box is one without them.
    pub fn point_trace(&self, start: Vec3, end: Vec3, mask: u32, statics: bool) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, statics)
    }

    /// A missile's segment: [`MASK_MISSILE`] and the static models. The
    /// syscall `G_RunMissile`'s `trap_Trace` reaches (`cod_lnxded` 0x8088333,
    /// case 0x22) passes `SV_Trace` no static-model flag, yet the retail
    /// capture rests a frag on a crate stack whose only geometry in that
    /// mask is the crates' own mesh (`crates/server/tests/missile_ab.rs`,
    /// z 179.3; the `clip_nosight` around them is 0x28031640, outside
    /// 0x11). The capture wins; how retail gets there is open
    /// (docs/research/cod11-combat.md, section 12).
    pub fn missile_trace(&self, start: Vec3, end: Vec3) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, MASK_MISSILE, true)
    }

    fn trace_with_mask(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        statics: bool,
    ) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
        };

        if !self.nodes.is_empty() {
            let mut scratch = Vec::new();
            let capsule = Capsule::of(mins, maxs);
            self.trace_node(
                0,
                start,
                end,
                mins,
                maxs,
                capsule,
                mask,
                statics,
                &mut trace,
                &mut scratch,
            );
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Nearest child first; the sweep box shrinks with `trace.fraction`, so
    /// prims past the current hit are never clipped.
    #[allow(clippy::too_many_arguments)]
    fn trace_node(
        &self,
        i: u32,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        capsule: Capsule,
        mask: u32,
        statics: bool,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let node = &self.nodes[i as usize];
        let cur_end = start + (end - start) * trace.fraction;
        let lo = start.min(cur_end) + mins - Vec3::ONE;
        let hi = start.max(cur_end) + maxs + Vec3::ONE;
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
            return;
        }
        if node.count > 0 {
            let first = node.first as usize;
            for (prim, _, _) in &self.prims[first..first + node.count as usize] {
                match *prim {
                    Prim::Brush(b) => {
                        let brush = &self.brushes[b as usize];
                        if brush.content_flags & mask == 0 || !self.brush_linked(brush) {
                            continue;
                        }
                        expand_brush(&brush.planes, capsule.radius, scratch);
                        clip_segment(
                            trace,
                            start + capsule.center,
                            end + capsule.center,
                            scratch,
                            brush.surface_flags,
                            capsule.offset,
                            false,
                            *prim,
                        );
                    }
                    Prim::Tri(t) => {
                        if self.tris_contents[t as usize] & mask == 0 {
                            continue;
                        }
                        if self.tris_terrain[t as usize] {
                            clip_sphere_triangle(
                                trace,
                                start + capsule.center,
                                end + capsule.center,
                                &self.tris[t as usize],
                                capsule,
                                self.tris_surf[t as usize],
                                *prim,
                            );
                        } else {
                            triangle_planes(&self.tris[t as usize], capsule.radius, scratch);
                            clip_segment(
                                trace,
                                start + capsule.center,
                                end + capsule.center,
                                scratch,
                                self.tris_surf[t as usize],
                                capsule.offset,
                                true,
                                *prim,
                            );
                        }
                    }
                    Prim::Model(t) => {
                        let mt = &self.model_tris[t as usize];
                        if !statics || mt.contents & mask == 0 {
                            continue;
                        }
                        clip_segment_model(trace, start, end, mt, *prim);
                    }
                }
            }
            return;
        }
        let dir = end - start;
        let along = |n: u32| {
            let node = &self.nodes[n as usize];
            ((node.lo + node.hi) * 0.5 - start).dot(dir)
        };
        let (near, far) = if along(node.first) <= along(node.second) {
            (node.first, node.second)
        } else {
            (node.second, node.first)
        };
        self.trace_node(
            near, start, end, mins, maxs, capsule, mask, statics, trace, scratch,
        );
        self.trace_node(
            far, start, end, mins, maxs, capsule, mask, statics, trace, scratch,
        );
    }

    /// A one-line name for what a trace hit, for reports.
    pub fn describe(&self, prim: Prim) -> String {
        match prim {
            Prim::Brush(b) => {
                let br = &self.brushes[b as usize];
                format!(
                    "brush {} {} cf={:#x}",
                    br.bsp_index, br.material, br.content_flags
                )
            }
            Prim::Tri(t) => format!(
                "{} tri {t} sf={:#x} cf={:#x}",
                if self.tris_terrain[t as usize] {
                    "terrain"
                } else {
                    "facet"
                },
                self.tris_surf[t as usize],
                self.tris_contents[t as usize]
            ),
            Prim::Model(t) => {
                let mt = &self.model_tris[t as usize];
                format!(
                    "xmodel tri {t} cf={:#x} sf={:#x} at {:?}",
                    mt.contents, mt.surface_flags, mt.tri[0]
                )
            }
        }
    }

    /// Only the tests call this now; `box_trace` walks the BVH itself via `trace_node`.
    #[cfg(test)]
    fn candidates(&self, lo: Vec3, hi: Vec3, out: &mut Vec<Prim>) {
        if self.nodes.is_empty() {
            return;
        }
        // the root is nodes[0]: build() makes one top-level build_bvh call
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
                continue;
            }
            if node.count > 0 {
                let start = node.first as usize;
                for (prim, _, _) in &self.prims[start..start + node.count as usize] {
                    out.push(*prim);
                }
            } else {
                stack.push(node.first);
                stack.push(node.second);
            }
        }
    }
}

/// `base` is the absolute index of prims[0] within CollisionWorld::prims.
fn build_bvh(prims: &mut [(Prim, Vec3, Vec3)], base: u32, nodes: &mut Vec<BvhNode>) -> u32 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (_, plo, phi) in prims.iter() {
        lo = lo.min(*plo);
        hi = hi.max(*phi);
    }
    let idx = nodes.len() as u32;
    nodes.push(BvhNode {
        lo,
        hi,
        first: base,
        second: 0,
        count: prims.len() as u32,
    });
    if prims.len() <= 4 {
        return idx;
    }
    let extent = hi - lo;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    let mid = prims.len() / 2;
    prims.select_nth_unstable_by(mid, |x, y| {
        let cx = (x.1[axis] + x.2[axis]) * 0.5;
        let cy = (y.1[axis] + y.2[axis]) * 0.5;
        cx.total_cmp(&cy)
    });
    let (l, r) = prims.split_at_mut(mid);
    let li = build_bvh(l, base, nodes);
    let ri = build_bvh(r, base + mid as u32, nodes);
    nodes[idx as usize].first = li;
    nodes[idx as usize].second = ri;
    nodes[idx as usize].count = 0;
    idx
}

/// Test helper: flat solid floor (-1024..1024, top at z=0) plus extra axial
/// solid brushes. Always compiled: the client's fx tests use it.
#[doc(hidden)]
pub fn test_world(extra: &[(Vec3, Vec3)]) -> CollisionWorld {
    let mut specs: Vec<(usize, [f32; 3], [f32; 3])> =
        vec![(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])];
    specs.extend(
        extra
            .iter()
            .map(|(lo, hi)| (0, lo.to_array(), hi.to_array())),
    );
    synthetic_world(&[("textures/test/solid", CONTENTS_SOLID, 0)], &specs)
}

/// Test helper: [`test_world`]'s floor with a ramp rising `deg` degrees along
/// +x from `x0` to `x1`, and level ground at the ramp's height beyond it. The
/// slope arrives as loose triangles, which is the only way this module takes
/// geometry that is not axis-aligned.
#[doc(hidden)]
pub fn ramp_test_world(deg: f32, x0: f32, x1: f32) -> CollisionWorld {
    let h = (x1 - x0) * deg.to_radians().tan();
    let (y0, y1) = (-512.0, 512.0);
    let quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| [[a, b, c], [a, c, d]];
    let mut tris = Vec::new();
    tris.extend(quad(
        Vec3::new(x0, y0, 0.0),
        Vec3::new(x1, y0, h),
        Vec3::new(x1, y1, h),
        Vec3::new(x0, y1, 0.0),
    ));
    tris.extend(quad(
        Vec3::new(x1, y0, h),
        Vec3::new(1024.0, y0, h),
        Vec3::new(1024.0, y1, h),
        Vec3::new(x1, y1, h),
    ));
    synthetic_world_tris(
        &[("textures/test/solid", CONTENTS_SOLID, 0)],
        &[(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])],
        &tris,
    )
}

/// Test helper: axial brushes with named materials `(name, contents, surface)`.
#[doc(hidden)]
pub fn synthetic_world(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
) -> CollisionWorld {
    synthetic_world_tris(materials, brushes, &[])
}

/// Test helper: [`test_world`]'s floor as model 0 plus one submodel per
/// `submodels` box, each holding a single brush in the model's own local
/// space. `entities` is the entity lump verbatim, since a submodel's
/// classname and `"model" "*N"` are what place its brushes and decide
/// whether they are solid at all. The only shape in which
/// [`CollisionWorld::set_model_linked`] is observable.
#[doc(hidden)]
pub fn submodel_test_world(entities: &str, submodels: &[([f32; 3], [f32; 3])]) -> CollisionWorld {
    let side = |v: f32| crate::bsp::BrushSide {
        plane_or_dist: v.to_bits(),
        material: 0,
    };
    let floor = ([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]);
    let mut brush_sides = Vec::new();
    let mut brushes = Vec::new();
    let mut models = Vec::new();
    for (i, (lo, hi)) in std::iter::once(&floor).chain(submodels).enumerate() {
        brushes.push(crate::bsp::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: 0,
        });
        for axis in 0..3 {
            brush_sides.push(side(lo[axis]));
            brush_sides.push(side(hi[axis]));
        }
        models.push(crate::bsp::Model {
            mins: *lo,
            maxs: *hi,
            first_soup: 0,
            num_soups: 0,
            first_brush: i as u32,
            num_brushes: 1,
        });
    }
    CollisionWorld::build(
        &crate::bsp::Bsp {
            materials: vec![crate::bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: CONTENTS_SOLID,
            }],
            lightmaps: vec![],
            soups: vec![],
            verts: vec![],
            indices: vec![],
            entities: entities.to_string(),
            planes: vec![],
            brush_sides,
            brushes,
            models,
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        },
        &[],
    )
}

/// [`synthetic_world`] plus world-space triangles as soups of material 0,
/// the only way this module takes swept geometry that is not axis-aligned.
#[doc(hidden)]
pub fn synthetic_world_tris(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
    tris: &[[Vec3; 3]],
) -> CollisionWorld {
    let side = |m: u32, v: f32| crate::bsp::BrushSide {
        plane_or_dist: v.to_bits(),
        material: m,
    };
    let mut brush_sides = Vec::new();
    let mut bsp_brushes = Vec::new();
    for (i, (mat, lo, hi)) in brushes.iter().enumerate() {
        bsp_brushes.push(crate::bsp::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: *mat as u16,
        });
        for axis in 0..3 {
            brush_sides.push(side(*mat as u32, lo[axis]));
            brush_sides.push(side(*mat as u32, hi[axis]));
        }
    }
    let n = bsp_brushes.len() as u32;
    let (mut mins, mut maxs) = ([0.0f32; 3], [0.0f32; 3]);
    for (_, lo, hi) in brushes {
        for axis in 0..3 {
            mins[axis] = mins[axis].min(lo[axis]);
            maxs[axis] = maxs[axis].max(hi[axis]);
        }
    }
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let mut soups = Vec::new();
    for tri in tris {
        let first_vertex = verts.len() as u32;
        verts.extend(tri.iter().map(|v| crate::bsp::DrawVert {
            pos: v.to_array(),
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        }));
        let first_index = indices.len() as u32;
        indices.extend_from_slice(&[0, 1, 2]);
        soups.push(crate::bsp::TriangleSoup {
            material: 0,
            lightmap: crate::bsp::NO_LIGHTMAP,
            first_vertex,
            vertex_count: 3,
            index_count: 3,
            first_index,
        });
    }
    let num_soups = soups.len() as u32;
    CollisionWorld::build(
        &crate::bsp::Bsp {
            materials: materials
                .iter()
                .map(|(name, content, surface)| crate::bsp::Material {
                    name: name.to_string(),
                    surface_flags: *surface,
                    content_flags: *content,
                })
                .collect(),
            lightmaps: vec![],
            soups,
            verts,
            indices,
            entities: String::new(),
            planes: vec![],
            brush_sides,
            brushes: bsp_brushes,
            models: vec![crate::bsp::Model {
                mins,
                maxs,
                first_soup: 0,
                num_soups,
                first_brush: 0,
                num_brushes: n,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        },
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prop wall at x = 5000 facing -x, two triangles wound outward.
    fn prop_wall(contents: u32) -> [ModelTri; 2] {
        let (a, b, c, d) = (
            Vec3::new(5000.0, -50.0, 0.0),
            Vec3::new(5000.0, 50.0, 0.0),
            Vec3::new(5000.0, 50.0, 100.0),
            Vec3::new(5000.0, -50.0, 100.0),
        );
        let mt = |tri| ModelTri {
            tri,
            contents,
            surface_flags: 21 << 20,
        };
        [mt([a, c, b]), mt([a, d, c])]
    }

    /// A prop's triangles stop a shot through them, front face only,
    /// backing off the clip epsilon, and carry the surface's flags.
    #[test]
    fn model_triangles_clip_a_shot() {
        let (start, end) = (Vec3::new(4900.0, 0.0, 50.0), Vec3::new(5100.0, 0.0, 50.0));
        let bare = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(bare.shot_trace(start, end).fraction, 1.0);
        let world = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_SOLID));
        let t = world.shot_trace(start, end);
        assert!(
            t.fraction < 1.0 && (t.endpos.x - (5000.0 - SURFACE_CLIP_EPSILON)).abs() < 1e-3,
            "{t:?}"
        );
        assert!(t.normal.abs_diff_eq(-Vec3::X, 1e-4), "{t:?}");
        assert_eq!(sound_material(t.surface_flags), 21);
        assert!(matches!(t.hit, Some(Prim::Model(_))), "{t:?}");
        // From behind, the same wall is open.
        assert_eq!(world.shot_trace(end, start).fraction, 1.0);
        // A segment past the triangle's edge misses by more than the epsilon.
        let over = world.shot_trace(start + Vec3::Z * 50.2, end + Vec3::Z * 50.2);
        assert_eq!(over.fraction, 1.0, "{over:?}");
        // Glass stops a shot too; a canopy (contents 0) stops nothing.
        let glass = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_GLASS));
        assert!(glass.shot_trace(start, end).fraction < 1.0);
        let canopy = CollisionWorld::build(&tiny_world(), &prop_wall(0));
        assert_eq!(canopy.shot_trace(start, end).fraction, 1.0);
    }

    /// A movement trace never meets a prop, whatever its shape: retail's
    /// `trap_Trace` and `trap_TraceCapsule` leave the static models to the
    /// locational trace, so a player walks through a chair a clip brush
    /// does not wrap.
    #[test]
    fn model_triangles_never_meet_a_movement_trace() {
        let world = CollisionWorld::build(&tiny_world(), &prop_wall(CONTENTS_SOLID));
        let (start, end) = (Vec3::new(4900.0, 0.0, 10.0), Vec3::new(5100.0, 0.0, 10.0));
        assert_eq!(
            world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction,
            1.0
        );
        let mins = Vec3::new(-15.0, -15.0, 0.0);
        let maxs = Vec3::new(15.0, 15.0, 70.0);
        assert_eq!(world.box_trace(start, end, mins, maxs).fraction, 1.0);
        assert!(world.shot_trace(start, end).fraction < 1.0);
    }
    use crate::bsp::{self, Bsp};
    use glam::Vec3;

    /// One axial brush (-64,-64,-16)..(64,64,0), one triangle at z=10 over x,y in 100..200.
    pub(crate) fn tiny_world() -> Bsp {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| bsp::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups: vec![bsp::TriangleSoup {
                material: 0,
                lightmap: bsp::NO_LIGHTMAP,
                first_vertex: 0,
                vertex_count: 3,
                index_count: 3,
                first_index: 0,
            }],
            verts: vec![
                vert([100.0, 100.0, 10.0]),
                vert([200.0, 100.0, 10.0]),
                vert([100.0, 200.0, 10.0]),
            ],
            indices: vec![0, 1, 2],
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![
                side(-64.0),
                side(64.0), // xmin, xmax
                side(-64.0),
                side(64.0), // ymin, ymax
                side(-16.0),
                side(0.0), // zmin, zmax
            ],
            brushes: vec![bsp::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            }],
            models: vec![bsp::Model {
                mins: [-64.0, -64.0, -16.0],
                maxs: [64.0, 64.0, 0.0],
                first_soup: 0,
                num_soups: 1,
                first_brush: 0,
                num_brushes: 1,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        }
    }

    fn vert(pos: [f32; 3]) -> bsp::DrawVert {
        bsp::DrawVert {
            pos,
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        }
    }

    #[test]
    fn builds_brush_planes_from_axial_bounds() {
        let world = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(world.brushes.len(), 1);
        let planes = &world.brushes[0].planes;
        assert_eq!(planes.len(), 6);
        // zmax face: normal +Z, dist 0
        assert!(planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(Vec3::Z, 1e-6) && *d == 0.0));
        // xmin face: normal -X, dist 64 (inside test: -x <= 64  =>  x >= -64)
        assert!(planes
            .iter()
            .any(|(n, d)| n.abs_diff_eq(-Vec3::X, 1e-6) && *d == 64.0));
    }

    #[test]
    fn harvests_triangles_and_answers_aabb_queries() {
        let world = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(world.tris.len(), 1);
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(140.0, 140.0, 0.0),
            Vec3::new(150.0, 150.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(_))));
        out.clear();
        world.candidates(
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::new(1.0, 1.0, 8.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Brush(_))));
        out.clear();
        world.candidates(
            Vec3::new(9000.0, 9000.0, 0.0),
            Vec3::new(9001.0, 9001.0, 1.0),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn builds_mp_pavlov_world() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        // model 0's 7575 solid+playerclip brushes plus its two stray
        // non-trigger submodel clips; the 32 trigger brushes stay hollow
        assert_eq!(world.brushes.len(), 7577);
        assert!(world.tris.len() > 10_000);
        // a query around a known spawn; only holds if candidates() walks from the root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-9100.0, 8950.0, -50.0),
            Vec3::new(-8800.0, 9250.0, 150.0),
            &mut out,
        );
        assert!(!out.is_empty());
    }

    /// Six triangles 1000 units apart along X: the median split yields a
    /// 3-node BVH, so root (0) and last-pushed node (2) differ and a walk
    /// that starts anywhere but the root misses the low-X leaf.
    fn spread_tris_world() -> Bsp {
        const N: usize = 6;
        let mut verts = Vec::with_capacity(N * 3);
        let mut indices = Vec::with_capacity(N * 3);
        let mut soups = Vec::with_capacity(N);
        for i in 0..N {
            let ox = i as f32 * 1000.0;
            verts.push(vert([ox, 0.0, 10.0]));
            verts.push(vert([ox + 10.0, 0.0, 10.0]));
            verts.push(vert([ox, 10.0, 10.0]));
            indices.extend_from_slice(&[0, 1, 2]);
            soups.push(bsp::TriangleSoup {
                material: 0,
                lightmap: bsp::NO_LIGHTMAP,
                first_vertex: (i * 3) as u32,
                vertex_count: 3,
                index_count: 3,
                first_index: (i * 3) as u32,
            });
        }
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups,
            verts,
            indices,
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![],
            brushes: vec![],
            models: vec![bsp::Model {
                mins: [0.0, 0.0, 0.0],
                maxs: [0.0, 0.0, 0.0],
                first_soup: 0,
                num_soups: N as u32,
                first_brush: 0,
                num_brushes: 0,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        }
    }

    fn world() -> CollisionWorld {
        CollisionWorld::build(&tiny_world(), &[])
    }

    #[test]
    fn ray_down_hits_brush_top() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0 && !t.startsolid);
        assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-5));
        // stops SURFACE_CLIP_EPSILON-ish above the face at z=0
        assert!((t.endpos.z - 0.0).abs() < 0.2, "endpos {}", t.endpos);
    }

    #[test]
    fn box_down_rests_on_brush_by_its_mins() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::new(-15.0, -15.0, 0.0),
            Vec3::new(15.0, 15.0, 70.0),
        );
        // bbox mins.z = 0 => origin comes to rest at the face, z ~ 0
        assert!(t.fraction < 1.0 && (t.endpos.z - 0.0).abs() < 0.2);
    }

    #[test]
    fn miss_returns_full_fraction() {
        let t = world().box_trace(
            Vec3::new(500.0, 500.0, 100.0),
            Vec3::new(500.0, 500.0, 50.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(t.fraction, 1.0);
        assert_eq!(t.endpos, Vec3::new(500.0, 500.0, 50.0));
    }

    #[test]
    fn start_inside_brush_is_startsolid() {
        let t = world().box_trace(
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.startsolid && t.allsolid && t.fraction == 0.0);
    }

    #[test]
    fn ray_down_hits_triangle() {
        let t = world().box_trace(
            Vec3::new(120.0, 120.0, 100.0),
            Vec3::new(120.0, 120.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0);
        assert!(t.normal.abs_diff_eq(Vec3::Z, 1e-4));
        assert!((t.endpos.z - 10.0).abs() < 0.2);
    }

    #[test]
    fn box_sideways_into_triangle_edge_stops_outside() {
        let t = world().box_trace(
            Vec3::new(50.0, 120.0, 10.0),
            Vec3::new(150.0, 120.0, 10.0),
            Vec3::splat(-5.0),
            Vec3::splat(5.0),
        );
        assert!(t.fraction < 1.0);
        assert!(
            t.endpos.x <= 95.5,
            "box center should stop before the edge, got {}",
            t.endpos.x
        );
    }

    #[test]
    fn answers_aabb_queries_across_a_multi_node_bvh() {
        let world = CollisionWorld::build(&spread_tris_world(), &[]);
        assert_eq!(world.tris.len(), 6);

        // low-X triangle: only reachable from the true root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-5.0, -5.0, 0.0),
            Vec3::new(15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(0))));

        // high-X triangle (index 5)
        out.clear();
        let ox = 5.0 * 1000.0;
        world.candidates(
            Vec3::new(ox - 5.0, -5.0, 0.0),
            Vec3::new(ox + 15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Tri(5))));

        // nowhere near any triangle
        out.clear();
        world.candidates(
            Vec3::new(50_000.0, 50_000.0, 0.0),
            Vec3::new(50_001.0, 50_001.0, 1.0),
            &mut out,
        );
        assert!(out.is_empty());
    }

    /// Every prim clipped, no traversal: the oracle for the ordered walk.
    fn brute_trace(
        world: &CollisionWorld,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
    ) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
        };
        let mut scratch = Vec::new();
        let capsule = Capsule::of(mins, maxs);
        for (prim, _, _) in &world.prims {
            match *prim {
                Prim::Brush(i) => {
                    let brush = &world.brushes[i as usize];
                    expand_brush(&brush.planes, capsule.radius, &mut scratch);
                    clip_segment(
                        &mut trace,
                        start + capsule.center,
                        end + capsule.center,
                        &scratch,
                        brush.surface_flags,
                        capsule.offset,
                        false,
                        *prim,
                    );
                }
                Prim::Tri(i) if world.tris_terrain[i as usize] => {
                    clip_sphere_triangle(
                        &mut trace,
                        start + capsule.center,
                        end + capsule.center,
                        &world.tris[i as usize],
                        capsule,
                        0,
                        *prim,
                    );
                }
                Prim::Tri(i) => {
                    triangle_planes(&world.tris[i as usize], capsule.radius, &mut scratch);
                    clip_segment(
                        &mut trace,
                        start + capsule.center,
                        end + capsule.center,
                        &scratch,
                        0,
                        capsule.offset,
                        true,
                        *prim,
                    );
                }
                Prim::Model(i) => {
                    clip_segment_model(
                        &mut trace,
                        start,
                        end,
                        &world.model_tris[i as usize],
                        *prim,
                    );
                }
            }
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Deterministic xorshift, so a failure reproduces.
    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    fn check_against_brute(world: &CollisionWorld, lo: Vec3, hi: Vec3, sweeps: usize, seed: u64) {
        let mut s = seed;
        let mut hits = 0;
        for _ in 0..sweeps {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let start = lo + (hi - lo) * r(&mut s);
            let end = lo + (hi - lo) * r(&mut s);
            let (mins, maxs) = if rng(&mut s) < 0.5 {
                (Vec3::ZERO, Vec3::ZERO)
            } else {
                (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 60.0))
            };
            let a = world.box_trace(start, end, mins, maxs);
            let b = brute_trace(world, start, end, mins, maxs);
            assert!(
                (a.fraction - b.fraction).abs() <= 1e-5,
                "{start} -> {end}: {a:?} vs {b:?}"
            );
            assert_eq!(
                (a.startsolid, a.allsolid),
                (b.startsolid, b.allsolid),
                "{start} -> {end}"
            );
            if a.fraction < 1.0 {
                hits += 1;
                assert!(
                    a.normal.abs_diff_eq(b.normal, 1e-4),
                    "{start} -> {end}: {a:?} vs {b:?}"
                );
            }
        }
        assert!(hits > 20, "too few hits to trust the oracle: {hits}");
    }

    #[test]
    fn ordered_walk_matches_brute_force_on_synthetic_worlds() {
        let w = spread_tris_world();
        // bounds hug triangle 0 so 300 sweeps land plenty of hits
        check_against_brute(
            &CollisionWorld::build(&w, &[]),
            Vec3::new(-20.0, -20.0, -20.0),
            Vec3::new(30.0, 30.0, 30.0),
            300,
            0x9E3779B97F4A7C15,
        );
        let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
        check_against_brute(
            &w,
            Vec3::new(-300.0, -300.0, -50.0),
            Vec3::new(300.0, 300.0, 200.0),
            300,
            0xD1B54A32D192ED03,
        );
    }

    #[test]
    fn ordered_walk_matches_brute_force_on_mp_pavlov() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (lo, hi) = crate::mesh::map_bounds(&bsp);
        // short sweeps so most of them hit something
        let mut s = 0x2545F4914F6CDD1Du64;
        let mut hits = 0;
        for _ in 0..200 {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let start = Vec3::from(lo) + (Vec3::from(hi) - Vec3::from(lo)) * r(&mut s);
            let end = start + (r(&mut s) - Vec3::splat(0.5)) * 600.0;
            let a = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
            let b = brute_trace(&world, start, end, Vec3::ZERO, Vec3::ZERO);
            assert!(
                (a.fraction - b.fraction).abs() <= 1e-5,
                "{start} -> {end}: {a:?} vs {b:?}"
            );
            assert_eq!((a.startsolid, a.allsolid), (b.startsolid, b.allsolid));
            hits += (a.fraction < 1.0) as usize;
        }
        assert!(hits > 20, "{hits}");
    }

    /// Model 0: floor z -16..0 over +-1024. Model 1: a door brush local
    /// (-8..8)^2 x 0..64 plus a stray render triangle at local x -60..-20,
    /// placed by an entity at (200, 0, 0).
    fn two_model_world(classname: &str) -> Bsp {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| bsp::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        let mut brush_sides = Vec::new();
        let mut push_box = |lo: [f32; 3], hi: [f32; 3]| {
            for axis in 0..3 {
                brush_sides.push(side(lo[axis]));
                brush_sides.push(side(hi[axis]));
            }
        };
        push_box([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]);
        push_box([-8.0, -8.0, 0.0], [8.0, 8.0, 64.0]);
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            lightmaps: vec![],
            soups: vec![bsp::TriangleSoup {
                material: 0,
                lightmap: bsp::NO_LIGHTMAP,
                first_vertex: 0,
                vertex_count: 3,
                index_count: 3,
                first_index: 0,
            }],
            verts: vec![
                vert([-60.0, 0.0, 30.0]),
                vert([-20.0, 0.0, 30.0]),
                vert([-60.0, 40.0, 30.0]),
            ],
            indices: vec![0, 1, 2],
            entities: format!(
                "{{\n\"classname\" \"{classname}\"\n\"model\" \"*1\"\n\"origin\" \"200 0 0\"\n}}"
            ),
            planes: vec![],
            brush_sides,
            brushes: vec![
                bsp::Brush {
                    first_side: 0,
                    num_sides: 6,
                    material: 0,
                },
                bsp::Brush {
                    first_side: 6,
                    num_sides: 6,
                    material: 0,
                },
            ],
            models: vec![
                bsp::Model {
                    mins: [-1024.0; 3],
                    maxs: [1024.0; 3],
                    first_soup: 1,
                    num_soups: 0,
                    first_brush: 0,
                    num_brushes: 1,
                },
                bsp::Model {
                    mins: [-8.0, -8.0, 0.0],
                    maxs: [8.0, 8.0, 64.0],
                    first_soup: 0,
                    num_soups: 1,
                    first_brush: 1,
                    num_brushes: 1,
                },
            ],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        }
    }

    #[test]
    fn submodel_brush_collides_at_its_entity_origin() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        let t = world.box_trace(
            Vec3::new(150.0, 0.0, 32.0),
            Vec3::new(250.0, 0.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0, "ray should hit the placed door: {t:?}");
        assert!(
            (t.endpos.x - 192.0).abs() < 1.0,
            "should stop at the door face, got {t:?}"
        );
    }

    #[test]
    fn trigger_submodels_do_not_collide() {
        let world = CollisionWorld::build(&two_model_world("trigger_multiple"), &[]);
        let t = world.box_trace(
            Vec3::new(150.0, 0.0, 32.0),
            Vec3::new(250.0, 0.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(t.fraction, 1.0, "trigger brushes must stay hollow: {t:?}");
    }

    #[test]
    fn submodel_render_triangles_leave_collision() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        // through the triangle's untranslated local spot: only the floor may stop this
        let t = world.box_trace(
            Vec3::new(-40.0, 20.0, 30.0),
            Vec3::new(-40.0, 20.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(
            (t.endpos.z - 0.0).abs() < 0.5,
            "should land on the floor, got {t:?}"
        );
    }

    fn water_world() -> Bsp {
        let dist = |v: f32| v.to_bits();
        let side = |m: u32, v: f32| bsp::BrushSide {
            plane_or_dist: dist(v),
            material: m,
        };
        let mut brush_sides = Vec::new();
        let mut brushes = Vec::new();
        for (m, lo, hi) in [
            (0u32, [-256.0, -256.0, -16.0], [256.0, 256.0, 0.0]),
            (1u32, [100.0, -100.0, -16.0], [300.0, 100.0, 36.0]),
        ] {
            for axis in 0..3 {
                brush_sides.push(side(m, lo[axis]));
                brush_sides.push(side(m, hi[axis]));
            }
            brushes.push(bsp::Brush {
                first_side: (brushes.len() * 6) as u32,
                num_sides: 6,
                material: m as u16,
            });
        }
        Bsp {
            materials: vec![
                bsp::Material {
                    name: "textures/test/solid".into(),
                    surface_flags: 0,
                    content_flags: 0x1,
                },
                bsp::Material {
                    name: "textures/common/water".into(),
                    surface_flags: 0,
                    content_flags: 0x20,
                },
            ],
            lightmaps: vec![],
            soups: vec![],
            verts: vec![],
            indices: vec![],
            entities: String::new(),
            planes: vec![],
            brush_sides,
            brushes,
            models: vec![bsp::Model {
                mins: [-256.0; 3],
                maxs: [256.0; 3],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                num_brushes: 2,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        }
    }

    #[test]
    fn point_contents_reports_water() {
        let world = CollisionWorld::build(&water_world(), &[]);
        assert_ne!(
            world.point_contents(Vec3::new(200.0, 0.0, 10.0)) & CONTENTS_WATER,
            0,
            "inside the pool"
        );
        assert_eq!(
            world.point_contents(Vec3::new(200.0, 0.0, 60.0)) & CONTENTS_WATER,
            0,
            "above the surface"
        );
        assert_eq!(
            world.point_contents(Vec3::new(0.0, 0.0, -8.0)) & CONTENTS_WATER,
            0,
            "dry ground next to the pool"
        );
    }

    /// One flagged playerclip wall at x 50..54 spanning y -200..200, z 0..100.
    fn ladder_wall_world(surface_flags: u32) -> Bsp {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| bsp::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        Bsp {
            materials: vec![bsp::Material {
                name: "textures/common/ladder".into(),
                surface_flags,
                content_flags: 0x10000,
            }],
            lightmaps: vec![],
            soups: vec![],
            verts: vec![],
            indices: vec![],
            entities: String::new(),
            planes: vec![],
            brush_sides: vec![
                side(50.0),
                side(54.0),
                side(-200.0),
                side(200.0),
                side(0.0),
                side(100.0),
            ],
            brushes: vec![bsp::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            }],
            models: vec![bsp::Model {
                mins: [50.0, -200.0, 0.0],
                maxs: [54.0, 200.0, 100.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                num_brushes: 1,
            }],
            cull_groups: vec![],
            cull_indices: vec![],
            portal_verts: vec![],
            occluders: vec![],
            occluder_plane_indices: vec![],
            occluder_edges: vec![],
            occluder_indices: vec![],
            aabb_nodes: vec![],
            cells: vec![],
            portals: vec![],
            nodes: vec![],
            leafs: vec![],
            terrain: vec![],
            patches: vec![],
            collision_verts: vec![],
            collision_indices: vec![],
            pvs: None,
        }
    }

    #[test]
    fn trace_reports_ladder_surface_flag() {
        let world = CollisionWorld::build(&ladder_wall_world(0x8), &[]);
        let t = world.box_trace(
            Vec3::new(0.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, 50.0),
            Vec3::new(-15.0, -15.0, 0.0),
            Vec3::new(15.0, 15.0, 60.0),
        );
        assert!(t.fraction < 1.0 && t.surface_flags & 0x8 != 0, "{t:?}");
        let plain = CollisionWorld::build(&ladder_wall_world(0x0), &[]);
        let t = plain.box_trace(
            Vec3::new(0.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, 50.0),
            Vec3::new(-15.0, -15.0, 0.0),
            Vec3::new(15.0, 15.0, 60.0),
        );
        assert!(t.fraction < 1.0 && t.surface_flags == 0, "{t:?}");
    }

    /// The sound surface rides the soup material's `surface_flags` bits 20-24,
    /// which a trace must carry through triangle hits too.
    #[test]
    fn triangle_hits_carry_the_sound_material() {
        let mut bsp = tiny_world();
        bsp.materials[0].surface_flags = 6 << 20; // dirt
        let world = CollisionWorld::build(&bsp, &[]);
        // straight down through the z=10 triangle
        let t = world.box_trace(
            Vec3::new(150.0, 150.0, 30.0),
            Vec3::new(150.0, 150.0, -8.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(t.fraction < 1.0 && t.normal.z > 0.9, "{t:?}");
        assert_eq!(super::sound_material(t.surface_flags), 6);
    }

    /// mp_harbor carries both stock-MP water and ladders; both must survive
    /// into the built world. Derived from the BSP itself, no hardcoded spots.
    #[test]
    fn harbor_water_volumes_and_ladder_flags_survive_build() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/mp/mp_harbor.bsp") else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();

        let axial_bounds = |b: &crate::bsp::Brush| -> ([f32; 3], [f32; 3]) {
            let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
            let mut lo = [0.0f32; 3];
            let mut hi = [0.0f32; 3];
            for axis in 0..3 {
                lo[axis] = f32::from_bits(sides[axis * 2].plane_or_dist);
                hi[axis] = f32::from_bits(sides[axis * 2 + 1].plane_or_dist);
            }
            (lo, hi)
        };

        let water = bsp
            .brushes
            .iter()
            .find(|b| bsp.materials[b.material as usize].content_flags & 0x20 != 0);
        let ladders: Vec<&crate::bsp::Brush> = bsp
            .brushes
            .iter()
            .filter(|b| bsp.materials[b.material as usize].surface_flags & 0x8 != 0)
            .collect();
        let (Some(water), Some(_)) = (water, ladders.first()) else {
            panic!("census says harbor has water and ladders");
        };

        let world = CollisionWorld::build(&bsp, &[]);

        let (wlo, whi) = axial_bounds(water);
        let mid = Vec3::new(
            (wlo[0] + whi[0]) * 0.5,
            (wlo[1] + whi[1]) * 0.5,
            (wlo[2] + whi[2]) * 0.5,
        );
        assert_ne!(
            world.point_contents(mid) & CONTENTS_WATER,
            0,
            "water brush interior at {mid}"
        );
        assert_eq!(
            world.point_contents(mid + Vec3::Z * (whi[2] - wlo[2] + 8.0)) & CONTENTS_WATER,
            0,
            "above the water surface"
        );

        // Ladder volumes sit inside staircases and walls; probe each from a
        // few offsets along its thinnest axis until one reports the flag.
        let player_box = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 60.0));
        let mut flagged = false;
        for ladder in &ladders {
            let (llo, lhi) = axial_bounds(ladder);
            let ex = lhi[0] - llo[0];
            let ey = lhi[1] - llo[1];
            let center = Vec3::new(
                (llo[0] + lhi[0]) * 0.5,
                (llo[1] + lhi[1]) * 0.5,
                ((llo[2] + lhi[2]) * 0.5).min(llo[2] + 40.0),
            );
            let dir = if ex < ey { Vec3::X } else { Vec3::Y };
            let span = (if ex < ey { ex } else { ey }) * 0.5;
            for back in [24.0, 40.0, 64.0] {
                for sign in [-1.0, 1.0] {
                    let t = world.box_trace(
                        center - dir * sign * (span + back),
                        center,
                        player_box.0,
                        player_box.1,
                    );
                    if !t.startsolid && t.fraction < 1.0 && t.surface_flags & 0x8 != 0 {
                        flagged = true;
                    }
                }
            }
        }
        assert!(flagged, "no approach reported the ladder flag");
    }

    /// Axial-free test helper: one triangle per soup entry against named
    /// `(name, contents, surface)` materials. Model 0 only, no brushes.
    #[doc(hidden)]
    pub fn synthetic_soup_world(
        materials: &[(&str, u32, u32)],
        soups: &[(usize, [[f32; 3]; 3])],
    ) -> CollisionWorld {
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        let mut soup_lump = Vec::new();
        for (mat, tri) in soups {
            let first_vertex = verts.len() as u32;
            for pos in tri {
                verts.push(vert(*pos));
            }
            let first_index = indices.len() as u16;
            indices.extend_from_slice(&[first_index, first_index + 1, first_index + 2]);
            soup_lump.push(crate::bsp::TriangleSoup {
                material: *mat as u16,
                lightmap: crate::bsp::NO_LIGHTMAP,
                first_vertex,
                vertex_count: 3,
                index_count: 3,
                first_index: first_index as u32,
            });
        }
        let soup_count = soup_lump.len() as u32;
        CollisionWorld::build(
            &crate::bsp::Bsp {
                materials: materials
                    .iter()
                    .map(|(name, content, surface)| crate::bsp::Material {
                        name: name.to_string(),
                        surface_flags: *surface,
                        content_flags: *content,
                    })
                    .collect(),
                lightmaps: vec![],
                soups: soup_lump,
                verts,
                indices,
                entities: String::new(),
                planes: vec![],
                brush_sides: vec![],
                brushes: vec![],
                models: vec![crate::bsp::Model {
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    first_soup: 0,
                    num_soups: soup_count,
                    first_brush: 0,
                    num_brushes: 0,
                }],
                cull_groups: vec![],
                cull_indices: vec![],
                portal_verts: vec![],
                occluders: vec![],
                occluder_plane_indices: vec![],
                occluder_edges: vec![],
                occluder_indices: vec![],
                aabb_nodes: vec![],
                cells: vec![],
                portals: vec![],
                nodes: vec![],
                leafs: vec![],
                terrain: vec![],
                patches: vec![],
                collision_verts: vec![],
                collision_indices: vec![],
                pvs: None,
            },
            &[],
        )
    }

    /// Census words from bsp-ibsp59-format.md "Content flags": bushwalls are
    /// TRANSLUCENT|WINDOW, brushless terrain bare 0x4, masked fences carry
    /// TRANSLUCENT|PLAYERCLIP|MONSTERCLIP.
    const BUSH_WALL: (&str, u32, u32) = (
        "textures/global_use/foliage_masked@bushwall1",
        0x2000_0002,
        8_454_176,
    );
    const TERRAIN: (&str, u32, u32) = ("textures/normandy/ground/a_grass1a", 0x4, 10 << 20);
    const BARBED_FENCE: (&str, u32, u32) = (
        "textures/normandy/transparents/metal_masked@barbed_fence1",
        0x2003_0000,
        13_713_440,
    );

    fn wall_tri(x: f32) -> [[f32; 3]; 3] {
        [[x, -50.0, 0.0], [x, 50.0, 0.0], [x, 50.0, 100.0]]
    }

    #[test]
    fn cutout_foliage_soups_enter_no_collision() {
        let world = synthetic_soup_world(&[BUSH_WALL], &[(0, wall_tri(100.0))]);
        assert!(world.tris.is_empty(), "the bush soup must not be harvested");
        assert_eq!(
            world
                .box_trace(
                    Vec3::new(0.0, 0.0, 50.0),
                    Vec3::new(200.0, 0.0, 50.0),
                    Vec3::ZERO,
                    Vec3::ZERO
                )
                .fraction,
            1.0
        );
    }

    #[test]
    fn terrain_soups_stay_solid() {
        let floor = [
            [100.0, -50.0, 10.0],
            [100.0, 50.0, 10.0],
            [300.0, 0.0, 10.0],
        ];
        let world = synthetic_soup_world(&[TERRAIN], &[(0, floor)]);
        let down = world.box_trace(
            Vec3::new(150.0, 0.0, 100.0),
            Vec3::new(150.0, 0.0, -100.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(
            down.fraction < 1.0 && (down.endpos.z - 10.0).abs() < 0.2,
            "{down:?}"
        );
        assert!(
            world
                .shot_trace(Vec3::new(150.0, 0.0, 100.0), Vec3::new(150.0, 0.0, -100.0))
                .fraction
                < 1.0
        );
    }

    #[test]
    fn playerclip_fence_soups_stop_movement_but_not_shots() {
        let world = synthetic_soup_world(&[BARBED_FENCE], &[(0, wall_tri(100.0))]);
        assert_eq!(
            world.tris.len(),
            1,
            "a clipped fence soup stays in the world"
        );
        let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
        let box_mins = Vec3::new(-15.0, -15.0, 0.0);
        let box_maxs = Vec3::new(15.0, 15.0, 60.0);
        let movement = world.box_trace(start, end, box_mins, box_maxs);
        assert!(movement.fraction < 1.0, "players are stopped: {movement:?}");
        let shot = world.shot_trace(start, end);
        assert_eq!(shot.fraction, 1.0, "bullets pass playerclip-only geometry");
    }

    #[test]
    fn solid_soups_stop_movement_and_shots() {
        let world = synthetic_soup_world(
            &[("textures/test/solid", 0x1, 5 << 20)],
            &[(0, wall_tri(100.0))],
        );
        let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
        assert!(
            world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction < 1.0,
            "movement stops"
        );
        assert!(world.shot_trace(start, end).fraction < 1.0, "shots stop");
    }
}
