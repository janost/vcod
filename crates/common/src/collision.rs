//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! Brush clip planes, terrain triangles, patch facets and a BVH from the
//! BSP collision lumps, swept by a Q3-style `box_trace`. Layouts and why
//! triangles are required: docs/research/bsp-ibsp59-format.md, "Terrain has
//! no brushes".

use crate::leafwalk::{Item, LeafWalk, Listing};
use crate::mapworld::{MapWorld, Partition};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use crate::patch::{Arith, PatchCollide, Sweep};
use crate::terrain::{CapsuleSweep, Terrain};
use glam::Vec3;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

/// Shared with the xmodel collision surfaces, which use the same bits.
pub const CONTENTS_SOLID: u32 = 0x1;
/// Window panes (`glass@brokenwindow`, `dam_window`: 0x2090, 0x8000010,
/// 0x28000010 over the stock maps) and lamp glass on xmodels. In retail's
/// player mask, so a player stops at a window and a bullet does not
/// (docs/research/bsp-ibsp59-format.md, "Content flags").
pub const CONTENTS_GLASS: u32 = 0x10;
const CONTENTS_PLAYERCLIP: u32 = 0x10000;
/// Census-proven water bit: docs/research/bsp-ibsp59-format.md, "Content flags".
pub const CONTENTS_WATER: u32 = 0x20;
/// The mask `G_RunItem` (game.mp 0x4ec47) hands `trap_PointContents` before
/// it frees a landing item (docs/research/cod11-items.md, section 8). No
/// stock material carries the bit.
pub const CONTENTS_NODROP: u32 = 0x8000_0000;
/// Ladder-climb flag on brush materials; pmove grabs ladders from trace hits carrying it.
pub const SURF_LADDER: u32 = 0x8;
/// `surfaceparm slick` (CoDMP.exe surfaceparm table, 0x571a40); no stock map
/// carries it (docs/research/cod11-mantle.md, "Slick ground").
pub const SURF_SLICK: u32 = 0x2;
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
/// SOLID and GLASS: the `trap_Trace` mask of a missile's explode trace
/// down, its ground snap and `grenadeExplosionEffect` (combat doc 12.6,
/// 13.2). Not the move trace's: that is [`MASK_MISSILE_MOVE`].
pub const MASK_MISSILE: u32 = CONTENTS_SOLID | CONTENTS_GLASS;
/// The clipmask `fire_grenade` and `fire_rocket` store and `G_RunMissile`'s
/// move trace reads (1.1d 0x544a0, 0x546ef, read at 0x5409a; 1.5b 0x5539d,
/// 0x556bd; UO 0x5f7ad), the blast mask's value: it meets the 0x2080 kerb
/// brushes too (combat doc 11.1, 12.1).
pub const MASK_MISSILE_MOVE: u32 = MASK_BLAST;
/// The brush contents that enter the clip as brushes: every bit a trace
/// mask meets, the item masks' 0x80 and 0x400 included (cod11-items.md,
/// section 14.2), but water, which stays a volume. Glass carries no SOLID
/// bit (mp_depot's panes are 0x8000010), and the 0x2080 kerb and floor
/// brushes carry neither SOLID nor PLAYERCLIP, so a player walks through
/// what stops a bullet, a blast probe and a falling item.
/// United Offensive's LMG mount brushes (0x404000) enter too, for the
/// deploy's point probe (`pmove::CONTENTS_HMG`); no mask a trace runs with
/// meets them. So do its vehicle clip brushes (0x200), which only a
/// vehicle's clipmask 0x201 meets (docs/research/coduo-vehicles.md 12).
const CLIP_BRUSH: u32 =
    (MASK_PLAYERSOLID | MASK_SHOT | MASK_BLAST | 0x491 | 0x40_4000 | CONTENTS_VEHICLE_CLIP)
        & !CONTENTS_WATER;
/// United Offensive's vehicle clip brush contents.
pub const CONTENTS_VEHICLE_CLIP: u32 = 0x200;
const TRACE_MASK_MOVE: u32 = MASK_PLAYERSOLID;
const TRACE_MASK_SHOT: u32 = MASK_SHOT;

/// A brush as clip planes: point p is inside iff n·p <= d for every plane.
#[derive(Clone)]
pub struct BrushPlanes {
    pub planes: Vec<(Vec3, f32)>,
    /// Lump-0 surface flags of the brush's material (SURF_LADDER and friends).
    pub surface_flags: u32,
    /// Each side's own material's surface flags, parallel to `planes`: what a
    /// trace that stops on that side reports.
    pub side_surface_flags: Vec<u32>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Brush(u32),
    /// A lump-26 terrain triangle: what a trace reports it stopped on.
    Tri(u32),
    /// A lump-24 terrain partition, the unit the BVH holds and the clip
    /// walks, its triangles in lump order.
    Terrain(u32),
    /// A lump-24 patch, all its facets.
    Patch(u32),
    Model(u32),
    /// A player capsule hit, carrying its entity number (`movetrace.rs`).
    Body(u32),
    /// A contact a [`WorldClip`] reported, carrying its contents word.
    Clip(u32),
}

/// A game's own clip of the world (model 0), traced in place of the BVH and
/// leaf walk this module builds from a [`MapWorld`]: CoD2's
/// `vcod_cod2::collision::ClipMap`. The world's other models still clip
/// here.
pub trait WorldClip: Send + Sync {
    /// A sweep of the box `mins..maxs` from `start` to `end` under `mask`;
    /// `start == end` is a position test.
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> WorldHit;
    /// The contents of the world's brushes at `p`.
    fn point_contents(&self, p: Vec3) -> u32;
    /// The world's ladder brushes as axial boxes, for the bots' graph
    /// (`nav::ladders` reads [`CollisionWorld::brushes`] for the rest).
    fn ladder_boxes(&self) -> Vec<(Vec3, Vec3)> {
        Vec::new()
    }
}

/// What a [`WorldClip`] trace met.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldHit {
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub contents: u32,
    pub startsolid: bool,
    pub allsolid: bool,
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
    /// The hit's unclamped enter fraction. Only the posed brushes' clip
    /// ([`clip_segment`]) reads it: of two surfaces a box touches at fraction
    /// 0 it reports the one it sits closest to. The world's clips settle a
    /// tie by retail's walk order instead.
    enter: f32,
    /// The entity of the world copy the contact is on
    /// ([`CollisionWorld::set_world_copy`]); `hit` is then the copy's prim.
    copy: Option<u32>,
}

impl Trace {
    /// A trace that reached `end` untouched.
    pub fn clear(end: Vec3) -> Trace {
        Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
            copy: None,
        }
    }
}

pub const SURFACE_CLIP_EPSILON: f32 = 0.125;

/// What a mover's clip is, for the push's traces: a submodel's brushes, or
/// the world copy an entity with inline model 0 carries (movers doc 20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pusher {
    Model(usize),
    Copy(u32),
}

/// Where a world trace ends, as `CM_BoxTrace` (`cod_lnxded` 0x8056310)
/// rounds it: the box is centred on `(mins + maxs) / 2`, the start and the
/// end each shifted by it, the delta taken between the shifted points with
/// the start stored as a float (0x80563d1-0x8056420), and the end point
/// `start + fraction * delta` off the unshifted start, rounded once
/// (0x8056887-0x80568c4). On a clear trace that is not `end` in the last
/// bit (docs/research/cod11-player-clip.md 8.9). 1.5b's (0x805aba1) also
/// stores the shifted end (0x805acdd), under [`Arith::Stored`]; the Windows
/// clients keep it (CoDMP15.exe 0x4264d6, cod15-movement.md 8).
pub fn cm_endpos(
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    fraction: f32,
    arith: Arith,
) -> Vec3 {
    let off = (mins + maxs) * 0.5;
    let f = f64::from(fraction);
    let axis = |s: f32, e: f32, o: f32| {
        let shifted = s + o;
        let delta = (arith.gcc_var(f64::from(e) + f64::from(o)) - f64::from(shifted)) as f32;
        (f64::from(s) + f * f64::from(delta)) as f32
    };
    Vec3::new(
        axis(start.x, end.x, off.x),
        axis(start.y, end.y, off.y),
        axis(start.z, end.z, off.z),
    )
}

/// Where `SV_Trace` puts a trace an entity's clip cut short of the
/// world's, over the end point the entity's clip left: `start + fraction *
/// (end - start)` off the unshifted points, each component rounded once.
/// 1.1d keeps the delta on the x87 stack (`cod_lnxded` 0x8091a06..
/// 0x8091a51); 1.5b and UO store it as a float (1.5b 0x809a45f..0x809a4d4,
/// UO `coduo_lnxded` 0x809b457..0x809b4cc). Movers doc 22, "On 1.5b and
/// United Offensive".
fn sv_trace_endpos(start: Vec3, end: Vec3, fraction: f32, arith: Arith) -> Vec3 {
    let f = f64::from(fraction);
    Vec3::from(std::array::from_fn::<f32, 3, _>(|i| {
        let delta = arith.gcc_var(f64::from(end[i]) - f64::from(start[i]));
        (f64::from(start[i]) + delta * f) as f32
    }))
}

/// How far outside an edge plane the static-model clip still counts a
/// crossing as inside the triangle (`cod_lnxded` rodata 0x80db974 and
/// 0x80db978: -0.001 and 1.001).
const MODEL_BARY_EPS: f32 = 0.001;

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
/// ([`CollisionWorld::entity_num`]) and its `hit` is `Prim::Clip(contents)`.
#[allow(clippy::too_many_arguments)]
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

/// Q3 `cm_trace.c` `CM_TraceThroughBrush`, on planes already expanded by the
/// shape, which is retail's own brush arm (`cod_lnxded` 0x8054e90, the
/// `sphere.use` branch: `dist + radius` per side, the sphere nearest the
/// side).
///
/// `sphere` is the capsule's sphere offset: each plane is tested against the
/// sphere nearest it, with `start` and `end` already at the capsule's
/// centre.
fn clip_segment(
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
    static NEAR_TIE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How far apart two contacts' fractions may sit and still be ordered by
/// the walk: twice the terrain back-off, and a margin over a brush's float
/// steps.
const TIE_WINDOW: f64 = 2.0 * crate::terrain::FRACTION_EPS as f64 + 1e-6;

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
/// upright), the bounds in the binary's order (mins x, y, z, then maxs);
/// every other side tests
/// the sphere nearer it against its distance pushed out by the radius
/// (0x8055080-0x8055176). `sphere` is the sphere offset, `(0, 0, half
/// height - radius)` but in a turned model's frame, where
/// `CM_TransformedBoxTrace` turns it (movers doc 21). Distances and fractions stay on the x87 stack
/// but the enter fraction, which is stored and compared as a float, and
/// the leave fraction starts at the trace's. 1.5b's build
/// (0x80590d6) stores each side's distances, their difference and both
/// fractions, which `arith` says. A trace a brush clips always
/// lands closer than the trace's fraction, so the lead side replaces the
/// trace's contact outright.
#[allow(clippy::too_many_arguments)]
fn clip_brush_capsule(
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
    let var = |x: f64| arith.var(x);
    let r = sw.radius;
    // The capsule's box in the brush's frame: each axis the sphere offset's
    // reach plus the radius (0x8056310's `|offset| + radius`, read at
    // `+0x10c`), `(r, r, half height)` for an upright one.
    let ext = [0, 1, 2].map(|i| sphere[i].abs() + r);
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
            let d1 = (d(sw.start[axis]) - bound) * sign - d(ext[axis]);
            let d2 = (d(sw.end[axis]) - bound) * sign - d(ext[axis]);
            if !side(d1, d2, n, axis * 2 + k) {
                return;
            }
        }
    }
    for (k, &(n, dist)) in planes.iter().enumerate().skip(6) {
        let toward = if crate::x87::dot(n, sphere) > 0.0 {
            -sphere
        } else {
            sphere
        };
        // Windows keeps the pushed distance and the shifted start on the
        // stack and stores only the shifted end (CoDMP15.exe 0x4251c7,
        // 0x4251fb, 0x42521e).
        let msvc = arith == Arith::Msvc;
        let pushed = if msvc { d(dist) + d(r) } else { d(dist + r) };
        let d1 = if msvc {
            let (p, n) = (
                Vec3::from(sw.start).as_dvec3() + toward.as_dvec3(),
                n.as_dvec3(),
            );
            (n.x * p.x + n.y * p.y) + n.z * p.z - pushed
        } else {
            crate::x87::dot(n, Vec3::from(sw.start) + toward) - pushed
        };
        let d2 = crate::x87::dot(n, Vec3::from(sw.end) + toward) - pushed;
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
        endpos: end,
        normal: Vec3::ZERO,
        surface_flags: 0,
        startsolid: false,
        allsolid: false,
        hit: None,
        enter: -1.0,
        copy: None,
    };
    for (i, mt) in tris.iter().enumerate() {
        if mt.contents & mask != 0 {
            clip_segment_model(&mut trace, start, end, mt, Prim::Model(i as u32));
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

/// [`clip_model_tris`] a surface at a time, as the engine's posed xmodel
/// clip walks them (`coduo_lnxded` 0x80c4892): a surface whose contents
/// meet the mask is tried only when the segment, moved into its bone's
/// frame, enters its stored bounds before the best fraction so far
/// (0x805e387, no epsilon). A triangle a later surface carries can then
/// lose to one found first even where its backed-off fraction is smaller
/// (docs/research/coduo-vehicles.md 13, "A blast on a hull").
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
        endpos: end,
        normal: Vec3::ZERO,
        surface_flags: 0,
        startsolid: false,
        allsolid: false,
        hit: None,
        enter: -1.0,
        copy: None,
    };
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
            clip_segment_model(&mut trace, start, end, &tris[i], Prim::Model(i as u32));
        }
    }
    trace
        .hit
        .map(|_| (trace.fraction, trace.normal, trace.surface_flags))
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
enum Shape {
    /// The capsule and the box it was made of.
    Capsule(Capsule, Vec3, Vec3),
    Box(Vec3, Vec3),
}

/// A brush's planes pushed out by an axial box, Q3's `CM_TraceThroughBrush`
/// non-sphere arm: `dist - offset·normal` with each offset component the
/// box corner the normal points away from.
fn expand_brush_box(planes: &[(Vec3, f32)], mins: Vec3, maxs: Vec3, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    for &(n, d) in planes {
        let corner = Vec3::new(
            if n.x < 0.0 { maxs.x } else { mins.x },
            if n.y < 0.0 { maxs.y } else { mins.y },
            if n.z < 0.0 { maxs.z } else { mins.z },
        );
        out.push((n, d - corner.dot(n)));
    }
}

/// A brush's planes pushed out by the capsule's radius, Q3's
/// `dist = plane->dist + tw->sphere.radius`.
fn expand_brush(planes: &[(Vec3, f32)], radius: f32, out: &mut Vec<(Vec3, f32)>) {
    out.clear();
    for &(n, d) in planes {
        out.push((n, d + radius));
    }
}

/// Whether a sweep reaches `patch`'s clip at all: a position test always
/// does, a moving sweep when its bounds meet the patch's box.
fn patch_meets(start: Vec3, end: Vec3, capsule: Capsule, patch: &WorldPatch, arith: Arith) -> bool {
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
        crate::terrain::segment_bounds_meet(lo, hi, s, e)
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
fn clip_patch(
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

/// Per lump-24 terrain partition and patch of `bsp`, the model whose
/// lump-23 run lists it (lump 27 bytes 32..40); 0 where none does.
fn partition_owners(bsp: &MapWorld) -> (Vec<u32>, Vec<u32>) {
    let mut terrain = vec![0; bsp.terrain.len()];
    let mut patches = vec![0; bsp.patches.len()];
    for (mi, model) in bsp.models.iter().enumerate().skip(1) {
        let (first, count) = model.partitions;
        let Some(run) = bsp
            .leaf_partitions
            .get(first as usize..first as usize + count as usize)
        else {
            continue;
        };
        for part in run {
            match *part {
                Partition::Terrain(i) => terrain[i as usize] = mi as u32,
                Partition::Patch(i) => patches[i as usize] = mi as u32,
            }
        }
    }
    (terrain, patches)
}

/// The sweep `CM_BoxTrace` (`cod_lnxded` 0x8056310) hands the terrain
/// clip: start and end moved to the box centre, the delta between them, and
/// the capsule's radius (the smaller of the half width and the half height)
/// and half height (0x80563d1-0x805644b).
fn capsule_sweep(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> CapsuleSweep {
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

#[derive(Clone)]
struct BvhNode {
    lo: Vec3,
    hi: Vec3,
    /// leaf: absolute prim index; internal: left child
    first: u32,
    /// internal: right child; leaf: unused
    second: u32,
    /// 0 = internal, otherwise leaf covering `count` prims starting at `first`
    count: u32,
    /// Whether the subtree holds a prim outside model 0, all the walk after
    /// [`CollisionWorld::clip_leaf_walk`] clips.
    rest: bool,
    /// Whether the subtree holds a brush or a patch, which
    /// [`TraceInput::misses`] never culls.
    loose: bool,
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
    /// Lump 24's terrain partitions as retail's records, one triangle record
    /// per entry of `tris`.
    terrain: Terrain,
    /// Per terrain partition, the lump-27 model that owns it.
    terrain_model: Vec<u32>,
    /// Lump 24's patches as retail's facet grids.
    pub patches: Vec<WorldPatch>,
    /// Model 0's BSP tree and what its leaves list, which a trace that
    /// meets a patch walks in retail's order. `None` without a tree.
    leaf_walk: Option<LeafWalk>,
    /// Per terrain triangle, whether every leaf its box reaches lists its
    /// partition, so a trace that stops on it was met on retail's walk.
    tri_listed: Vec<bool>,
    nodes: Vec<BvhNode>,
    /// The subtree holding every [`Prim::Model`] and nothing else, which a
    /// trace without the static models skips whole; `u32::MAX` when the map
    /// has none or nothing else.
    models_root: u32,
    prims: Vec<(Prim, Vec3, Vec3)>,
    water: Vec<Volume>,
    /// Brushes whose material carries [`CONTENTS_NODROP`], for
    /// [`Self::point_contents`] only: nothing clips against them.
    nodrop: Vec<Volume>,
    /// Per lump-27 model, whether its brushes are in the clip. Retail holds
    /// a submodel's brushes only through the entity that links them, so a
    /// deleted `script_brushmodel` takes its brushes out; `set_model_linked`
    /// is that unlink.
    model_linked: Vec<AtomicBool>,
    /// Per lump-27 model, the entity number a trace that ends on its brushes
    /// reports: the world's for model 0 and for any submodel the server has
    /// not named an entity for.
    model_entity: Vec<AtomicU32>,
    /// Per lump-27 model, its brushes' run in `brushes`, the entity origin
    /// the stored planes are offset by, and the run's bounds about that
    /// origin: what a [`ModelPose`] moves.
    model_span: Vec<ModelSpan>,
    /// Per lump-27 model, whether a pose has taken its brushes off their
    /// spawn placement: the BVH, built at spawn, then skips them and the
    /// posed pass clips them where the pose puts them.
    model_posed: Vec<AtomicBool>,
    /// The posed models, few and short-lived: a script mover's, while its
    /// entity is anywhere but where the map placed it.
    poses: RwLock<Vec<(usize, ModelPose)>>,
    /// Whether `poses` holds any: a trace reads this and skips the lock,
    /// whose reader count every thread of a parallel nav build otherwise
    /// writes on every trace.
    any_posed: AtomicBool,
    /// Brush entities linked with inline model 0 (a script `spawn` of a
    /// `script_brushmodel` or sine mover): each a copy of the world's own
    /// clip where its entity stands (docs/research/cod11-movers.md 19).
    copies: RwLock<Vec<WorldCopy>>,
    /// Whether `copies` holds any, read like `any_posed`.
    any_copy: AtomicBool,
    /// Entities that each clip their own copy of a submodel where they
    /// stand: a UO vehicle and its collmap, one inline model serving every
    /// vehicle of a model (docs/research/coduo-vehicles.md 10.3).
    instances: RwLock<Vec<ModelInstance>>,
    /// Whether `instances` holds any.
    any_instance: AtomicBool,
    /// Terrain point traces and patches take 1.5b's and United
    /// Offensive's arithmetic ([`Self::set_arith`]).
    arith: AtomicU8,
    /// The world's own clip, when the game brings one
    /// ([`Self::set_world_clip`]).
    world_clip: Option<Arc<dyn WorldClip>>,
}

/// One entity's copy of a submodel: its pose and `r.contents`, which a
/// trace's mask must meet before the brushes are clipped.
#[derive(Clone, Copy, Debug)]
struct ModelInstance {
    entity: u32,
    model: usize,
    pose: ModelPose,
    contents: u32,
}

/// A submodel copy out of the traces for a while
/// ([`CollisionWorld::unlink_model_instance`]).
#[derive(Debug)]
pub struct UnlinkedInstance(ModelInstance);

/// One world copy: its entity, pose and `r.contents`, which a trace's mask
/// must meet before the copy is clipped at all (`SV_ClipMoveToEntities`
/// lists entities by contents).
#[derive(Clone, Copy, Debug)]
struct WorldCopy {
    entity: u32,
    origin: Vec3,
    /// `AnglesToAxis`, local to world; `None` unrotated.
    axis: Option<[Vec3; 3]>,
    contents: u32,
}

impl WorldCopy {
    /// [`ModelPose::to_local`]'s turn, with no spawn origin.
    fn to_local(self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        match self.axis {
            Some(axis) => Vec3::from(axis.map(|a| crate::x87::dot(a, d) as f32)),
            None => d,
        }
    }

    /// `CM_TransformedBoxTrace`'s sphere offset in this copy's frame
    /// (`cod_lnxded` 0x80568d8): x and z along the forward and up axes as a
    /// point turns, y along the right axis where a point takes the left.
    fn sphere(self, offset: Vec3) -> Vec3 {
        match self.axis {
            Some([f, l, u]) => Vec3::new(f.z, -l.z, u.z) * offset.z,
            None => offset,
        }
    }

    fn to_world_dir(self, n: Vec3) -> Vec3 {
        match self.axis {
            Some([f, l, u]) => f * n.x + l * n.y + u * n.z,
            None => n,
        }
    }
}

/// A snapshot: the links, poses and entity numbers as they stand now, which
/// the copy then keeps whatever the original does.
impl Clone for CollisionWorld {
    fn clone(&self) -> Self {
        let bools = |v: &[AtomicBool]| {
            v.iter()
                .map(|b| AtomicBool::new(b.load(Ordering::Relaxed)))
                .collect()
        };
        CollisionWorld {
            brushes: self.brushes.clone(),
            tris: self.tris.clone(),
            model_tris: self.model_tris.clone(),
            tris_surf: self.tris_surf.clone(),
            tris_contents: self.tris_contents.clone(),
            terrain: self.terrain.clone(),
            terrain_model: self.terrain_model.clone(),
            patches: self.patches.clone(),
            leaf_walk: self.leaf_walk.clone(),
            tri_listed: self.tri_listed.clone(),
            nodes: self.nodes.clone(),
            models_root: self.models_root,
            prims: self.prims.clone(),
            water: self.water.clone(),
            nodrop: self.nodrop.clone(),
            model_linked: bools(&self.model_linked),
            model_entity: self
                .model_entity
                .iter()
                .map(|e| AtomicU32::new(e.load(Ordering::Relaxed)))
                .collect(),
            model_span: self.model_span.clone(),
            model_posed: bools(&self.model_posed),
            poses: RwLock::new(self.poses.read().unwrap_or_else(|e| e.into_inner()).clone()),
            any_posed: AtomicBool::new(self.any_posed.load(Ordering::Acquire)),
            copies: RwLock::new(
                self.copies
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
            any_copy: AtomicBool::new(self.any_copy.load(Ordering::Acquire)),
            instances: RwLock::new(
                self.instances
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
            any_instance: AtomicBool::new(self.any_instance.load(Ordering::Acquire)),
            arith: AtomicU8::new(self.arith.load(Ordering::Acquire)),
            world_clip: self.world_clip.clone(),
        }
    }
}

/// A lump-24 patch with its material's words, which the mask test and a
/// hit report read.
#[derive(Clone, Debug)]
pub struct WorldPatch {
    pub collide: PatchCollide,
    /// The same patch in 1.5b's and United Offensive's arithmetic
    /// ([`Arith::Stored`]), where that builds it differently.
    pub stored: Option<PatchCollide>,
    /// The same patch as the Windows clients build it ([`Arith::Msvc`]),
    /// where that differs from `collide`.
    pub msvc: Option<PatchCollide>,
    pub content_flags: u32,
    pub surface_flags: u32,
    /// Its record's index among lump 24's patches, for `describe`.
    pub index: u32,
    /// The lump-27 model that owns it: 0 for the world's.
    pub model: u32,
}

impl WorldPatch {
    /// The build a trace under `arith` clips.
    pub fn clip(&self, arith: Arith) -> &PatchCollide {
        let other = match arith {
            Arith::Kept => None,
            Arith::Stored => self.stored.as_ref(),
            Arith::Msvc => self.msvc.as_ref(),
        };
        other.unwrap_or(&self.collide)
    }
}

/// One model's brushes as built: their run in `CollisionWorld::brushes`, the
/// spawn origin their planes carry, and their bounds relative to it.
#[derive(Clone, Copy, Debug, Default)]
struct ModelSpan {
    brushes: (u32, u32),
    origin: Vec3,
    lo: Vec3,
    hi: Vec3,
}

/// Where a submodel's brushes are now: its entity's origin and the rotation
/// its angles make, retail's `SV_ClipMoveToEntities` transform of an inline
/// model by `r.currentOrigin` and `r.currentAngles`, and the world bounds
/// that leaves them in.
#[derive(Clone, Copy, Debug)]
struct ModelPose {
    origin: Vec3,
    /// `AnglesToAxis`: forward, left, up, the columns of local to world.
    axis: [Vec3; 3],
    /// Any angle non-zero: `CM_TransformedBoxTrace` then turns the sweep
    /// into the model's frame instead of moving the brushes.
    turned: bool,
    lo: Vec3,
    hi: Vec3,
}

impl ModelPose {
    fn rotate(&self, v: Vec3) -> Vec3 {
        self.axis[0] * v.x + self.axis[1] * v.y + self.axis[2] * v.z
    }

    /// A world point in the frame the brushes were built in: turned back
    /// about the pose's origin and put at the spawn origin. Each dot stays
    /// on the x87 stack and is stored once (`cod_lnxded` 0x8056a7d..
    /// 0x8056bc6); at ten thousand units out a rounded product moves the
    /// point a few thousandths (movers doc 22).
    fn to_local(self, p: Vec3, spawn: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::from(self.axis.map(|a| crate::x87::dot(a, d) as f32)) + spawn
    }

    /// A plane of the brush as built, `n·p <= d` at the spawn `origin`, moved
    /// to this pose: `(R n)·q <= d - n·origin + (R n)·self.origin`.
    fn plane(&self, (n, d): (Vec3, f32), spawn: Vec3) -> (Vec3, f32) {
        let rn = self.rotate(n);
        (rn, d - n.dot(spawn) + rn.dot(self.origin))
    }
}

/// A brush as clip planes plus its axial bounds for the cheap reject: a
/// water volume, or a pane at build time.
#[derive(Clone)]
struct Volume {
    planes: Vec<(Vec3, f32)>,
    lo: Vec3,
    hi: Vec3,
    /// Per side, its material's surface flags: kept for water, which a
    /// missile's splash reads (`water_hit`), empty otherwise.
    side_flags: Vec<u32>,
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// A lump-4 brush's sides as clip planes, appended to `out` and offset to
/// `origin`, plus the brush's axial bounds. Sides 0..6 are the bit-cast axial
/// bounds and sides 6.. index `MapWorld::planes`
/// (docs/research/bsp-ibsp59-format.md, "Brushes"). Point p is inside the
/// brush iff n·p <= d for every plane returned.
pub fn brush_side_planes(
    bsp: &MapWorld,
    brush: &crate::mapworld::Brush,
    origin: Vec3,
    out: &mut Vec<(Vec3, f32)>,
) -> (Vec3, Vec3) {
    let sides = &bsp.brush_sides[brush.first_side as usize..][..brush.num_sides as usize];
    out.reserve(sides.len());
    let mut lo = Vec3::ZERO;
    let mut hi = Vec3::ZERO;
    for axis in 0..3 {
        let axis_lo = f32::from_bits(sides[axis * 2].plane_or_dist) + origin[axis];
        let axis_hi = f32::from_bits(sides[axis * 2 + 1].plane_or_dist) + origin[axis];
        // Retail zeroes the side's normal and stores -1 on the axis
        // (`cod_lnxded` 0x8054fd1), so the other components are +0.0.
        let mut down = Vec3::ZERO;
        down[axis] = -1.0;
        out.push((down, -axis_lo));
        out.push((AXES[axis], axis_hi));
        lo[axis] = axis_lo;
        hi[axis] = axis_hi;
    }
    for s in &sides[6..] {
        let p = &bsp.planes[s.plane_or_dist as usize];
        let n = Vec3::from_array(p.normal);
        out.push((n, p.dist + n.dot(origin)));
    }
    (lo, hi)
}

/// The triangle lists under construction in `build`.
struct Tris {
    tris: Vec<[Vec3; 3]>,
    surf: Vec<u32>,
    contents: Vec<u32>,
    prims: Vec<(Prim, Vec3, Vec3)>,
    terrain: Terrain,
}

impl Tris {
    /// One lump-24 terrain partition: its records for the clip, and each
    /// triangle in lump order wound with `cross(b - a, c - a)` on the
    /// outside, the side the sphere clip faces. The BVH holds the
    /// partition, padded by 0.25 so the query finds one the box merely
    /// touches.
    fn push_partition(
        &mut self,
        points: &[[f32; 3]],
        tris: &[[u16; 3]],
        surface_flags: u32,
        contents: u32,
    ) {
        if tris.is_empty() {
            return;
        }
        let part = self.terrain.add_partition(points, tris);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for tri in tris {
            let p = |i: usize| Vec3::from_array(points[tri[i] as usize]);
            let (a, b, c) = (p(0), p(2), p(1));
            lo = lo.min(a).min(b).min(c);
            hi = hi.max(a).max(b).max(c);
            self.tris.push([a, b, c]);
            self.surf.push(surface_flags);
            self.contents.push(contents);
        }
        self.prims.push((
            Prim::Terrain(part),
            lo - Vec3::splat(0.25),
            hi + Vec3::splat(0.25),
        ));
    }
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
    /// engine's own triangles, swept as a sphere, and its patches as
    /// `CM_GeneratePatchCollide`'s facets. No render soup collides: retail
    /// clips model 0 through its brushes and lump 24 alone.
    pub fn build(bsp: &MapWorld, model_tris: &[ModelTri]) -> Self {
        let mut brushes = Vec::new();
        let mut water = Vec::new();
        let mut nodrop = Vec::new();
        let mut t = Tris {
            tris: Vec::new(),
            surf: Vec::new(),
            contents: Vec::new(),
            prims: Vec::new(),
            terrain: Terrain::default(),
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

        let mut model_span = vec![ModelSpan::default(); bsp.models.len()];
        for (mi, model) in bsp.models.iter().enumerate() {
            let placement = &placements[mi];
            let span = &mut model_span[mi];
            span.origin = placement.origin;
            span.brushes = (brushes.len() as u32, brushes.len() as u32);
            let (mut span_lo, mut span_hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
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
                    || mat.content_flags & (CLIP_BRUSH | CONTENTS_WATER | CONTENTS_NODROP) == 0
                {
                    continue;
                }
                let mut planes = Vec::new();
                let (lo, hi) = brush_side_planes(bsp, b, placement.origin, &mut planes);
                if mi == 0 && mat.content_flags & CONTENTS_NODROP != 0 {
                    nodrop.push(Volume {
                        planes: planes.clone(),
                        lo,
                        hi,
                        side_flags: Vec::new(),
                    });
                }
                if mat.content_flags & CONTENTS_WATER != 0 {
                    let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                    water.push(Volume {
                        planes: planes.clone(),
                        lo,
                        hi,
                        side_flags: sides
                            .iter()
                            .map(|s| {
                                bsp.materials
                                    .get(s.material as usize)
                                    .map_or(mat.surface_flags, |m| m.surface_flags)
                            })
                            .collect(),
                    });
                }
                if mat.content_flags & CLIP_BRUSH != 0 {
                    let idx = brushes.len() as u32;
                    let sides = &bsp.brush_sides[b.first_side as usize..][..b.num_sides as usize];
                    brushes.push(BrushPlanes {
                        planes,
                        surface_flags: mat.surface_flags,
                        side_surface_flags: sides
                            .iter()
                            .map(|s| {
                                bsp.materials
                                    .get(s.material as usize)
                                    .map_or(mat.surface_flags, |m| m.surface_flags)
                            })
                            .collect(),
                        content_flags: mat.content_flags,
                        bsp_index: bi as u32,
                        material: mat.name.clone(),
                        model: mi as u32,
                    });
                    t.prims.push((Prim::Brush(idx), lo, hi));
                    span_lo = span_lo.min(lo);
                    span_hi = span_hi.max(hi);
                }
            }
            span.brushes.1 = brushes.len() as u32;
            if span.brushes.1 > span.brushes.0 {
                span.lo = span_lo - placement.origin;
                span.hi = span_hi - placement.origin;
            }
        }

        // The engine's terrain, wound the way `CM_GenerateTerrainCollide`
        // takes its plane (Q3's `PlaneFromPoints`: `cross(c - a, b - a)`).
        // A model-0 partition no leaf lists never clips
        // (`CM_TraceThroughTree`). A submodel's partitions are its entity's,
        // placed and linked with its brushes.
        let listed = LeafWalk::listed(bsp);
        let (terrain_owner, patch_owner) = partition_owners(bsp);
        let offset = |model: u32, points: &[[f32; 3]]| -> Vec<[f32; 3]> {
            let o = placements[model as usize].origin;
            points
                .iter()
                .map(|p| (Vec3::from_array(*p) + o).to_array())
                .collect()
        };
        let mut world_terrain = vec![None; bsp.terrain.len()];
        let mut terrain_model = Vec::new();
        for (i, part) in bsp.terrain.iter().enumerate() {
            let model = terrain_owner[i];
            if (model == 0 && listed.as_ref().is_some_and(|l| !l.terrain[i]))
                || placements[model as usize].trigger
            {
                continue;
            }
            let mat = &bsp.materials[part.material as usize];
            let Some(contents) = tri_contents(mat.content_flags) else {
                continue;
            };
            let idx =
                &bsp.collision_indices[part.first_index as usize..][..part.index_count as usize];
            let points = offset(
                model,
                &bsp.collision_verts[part.first_vert as usize..][..part.vert_count as usize],
            );
            let next = t.terrain.parts.len() as u32;
            t.push_partition(&points, idx.as_chunks::<3>().0, mat.surface_flags, contents);
            if t.terrain.parts.len() as u32 > next {
                terrain_model.push(model);
                if model == 0 {
                    world_terrain[i] = Some(next);
                }
            }
        }
        // `CM_LoadMap` builds every patch whatever its contents; the trace's
        // mask test skips the ones it does not meet.
        let mut patches = Vec::new();
        let mut world_patch = vec![None; bsp.patches.len()];
        for (index, part) in bsp.patches.iter().enumerate() {
            let model = patch_owner[index];
            if (model == 0 && listed.as_ref().is_some_and(|l| !l.patches[index]))
                || placements[model as usize].trigger
            {
                continue;
            }
            let mat = &bsp.materials[part.material as usize];
            let (w, h) = (part.width as usize, part.height as usize);
            let points = offset(
                model,
                &bsp.collision_verts[part.first_vert as usize..][..w * h],
            );
            let Some(collide) = PatchCollide::generate(w, h, part.tolerance, &points) else {
                log::warn!("lump 24 patch {index} ({w}x{h}) builds no collision");
                continue;
            };
            let stored = PatchCollide::generate_as(w, h, part.tolerance, &points, Arith::Stored)
                .filter(|s| !s.same_clip(&collide));
            let msvc = PatchCollide::generate_as(w, h, part.tolerance, &points, Arith::Msvc)
                .filter(|s| !s.same_clip(&collide));
            let (mut lo, mut hi) = (collide.mins, collide.maxs);
            for s in stored.iter().chain(&msvc) {
                for k in 0..3 {
                    lo[k] = lo[k].min(s.mins[k]);
                    hi[k] = hi[k].max(s.maxs[k]);
                }
            }
            t.prims.push((
                Prim::Patch(patches.len() as u32),
                Vec3::from_array(lo),
                Vec3::from_array(hi),
            ));
            if model == 0 {
                world_patch[index] = Some(patches.len() as u32);
            }
            patches.push(WorldPatch {
                collide,
                stored,
                msvc,
                content_flags: mat.content_flags,
                surface_flags: mat.surface_flags,
                index: index as u32,
                model,
            });
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

        // The static models sit in a subtree of their own under the root:
        // movement traces never clip them, and mixed in they widen every box
        // a player's sweep enters.
        let mut nodes = Vec::new();
        let split = t.prims.partition_point(|p| !matches!(p.0, Prim::Model(_)));
        let models_root = if split == 0 || split == t.prims.len() {
            if !t.prims.is_empty() {
                build_bvh(&mut t.prims, 0, &mut nodes);
            }
            u32::MAX
        } else {
            nodes.push(BvhNode {
                lo: Vec3::ZERO,
                hi: Vec3::ZERO,
                first: 0,
                second: 0,
                count: 0,
                rest: true,
                loose: true,
            });
            let (world_prims, model_prims) = t.prims.split_at_mut(split);
            let wi = build_bvh(world_prims, 0, &mut nodes);
            let mi = build_bvh(model_prims, split as u32, &mut nodes);
            let (w, m) = (&nodes[wi as usize], &nodes[mi as usize]);
            nodes[0] = BvhNode {
                lo: w.lo.min(m.lo),
                hi: w.hi.max(m.hi),
                first: wi,
                second: mi,
                count: 0,
                rest: true,
                loose: true,
            };
            mi
        };
        // Children sit after their parent, so one pass from the back sees
        // both before it.
        for i in (0..nodes.len()).rev() {
            let n = &nodes[i];
            nodes[i].rest = if n.count > 0 {
                t.prims[n.first as usize..][..n.count as usize]
                    .iter()
                    .any(|(prim, _, _)| match *prim {
                        Prim::Brush(b) => brushes[b as usize].model != 0,
                        Prim::Terrain(p) => terrain_model[p as usize] != 0,
                        Prim::Patch(p) => patches[p as usize].model != 0,
                        _ => true,
                    })
            } else {
                nodes[n.first as usize].rest || nodes[n.second as usize].rest
            };
            let n = &nodes[i];
            nodes[i].loose = if n.count > 0 {
                t.prims[n.first as usize..][..n.count as usize]
                    .iter()
                    .any(|(prim, _, _)| !TraceInput::tight(*prim))
            } else {
                nodes[n.first as usize].loose || nodes[n.second as usize].loose
            };
        }

        let mut world_brush = vec![None; bsp.brushes.len()];
        for (i, b) in brushes.iter().enumerate() {
            if b.model == 0 {
                world_brush[b.bsp_index as usize] = Some(i as u32);
            }
        }
        // Each listing carries its BVH prim's box.
        let (mut brush_box, mut terrain_box, mut patch_box) = (
            vec![Default::default(); brushes.len()],
            vec![Default::default(); terrain_model.len()],
            vec![Default::default(); patches.len()],
        );
        for &(prim, lo, hi) in &t.prims {
            let b = (lo.to_array(), hi.to_array());
            match prim {
                Prim::Brush(i) => brush_box[i as usize] = b,
                Prim::Terrain(i) => terrain_box[i as usize] = b,
                Prim::Patch(i) => patch_box[i as usize] = b,
                _ => {}
            }
        }
        let leaf_walk =
            LeafWalk::new(
                bsp,
                &world_brush,
                &world_terrain,
                &world_patch,
                |item| match item {
                    Item::Brush(i) => brush_box[i as usize],
                    Item::Terrain(i) => terrain_box[i as usize],
                    Item::Patch(i) => patch_box[i as usize],
                },
            );
        let tri_listed = (0..t.terrain.parts.len())
            .flat_map(|p| {
                let part = t.terrain.parts[p];
                let (terrain, leaf_walk) = (&t.terrain, &leaf_walk);
                let model = terrain_model[p];
                (part.first..part.first + part.count).map(move |i| {
                    let (lo, hi) = terrain.tri_bounds(i as usize);
                    let grow = |v: [f32; 3], by: f32| v.map(|x| x + by);
                    model == 0
                        && leaf_walk.as_ref().is_some_and(|w| {
                            w.box_lists(grow(lo, -1.0), grow(hi, 1.0), Item::Terrain(p as u32))
                        })
                })
            })
            .collect();
        CollisionWorld {
            brushes,
            tris: t.tris,
            model_tris: model_tris_out,
            tris_surf: t.surf,
            tris_contents: t.contents,
            terrain: t.terrain,
            terrain_model,
            patches,
            leaf_walk,
            tri_listed,
            nodes,
            models_root,
            prims: t.prims,
            water,
            nodrop,
            model_linked: bsp.models.iter().map(|_| AtomicBool::new(true)).collect(),
            model_entity: bsp
                .models
                .iter()
                .map(|_| AtomicU32::new(ENTITYNUM_WORLD))
                .collect(),
            model_posed: bsp.models.iter().map(|_| AtomicBool::new(false)).collect(),
            model_span,
            poses: RwLock::new(Vec::new()),
            any_posed: AtomicBool::new(false),
            copies: RwLock::new(Vec::new()),
            any_copy: AtomicBool::new(false),
            instances: RwLock::new(Vec::new()),
            any_instance: AtomicBool::new(false),
            arith: AtomicU8::new(Arith::Kept as u8),
            world_clip: None,
        }
    }

    /// Hands the world's clip to `clip`: every trace, position test and
    /// contents query meets it first. The [`MapWorld`] this was built from
    /// must leave model 0's brushes and partitions out.
    pub fn set_world_clip(&mut self, clip: Arc<dyn WorldClip>) {
        self.world_clip = Some(clip);
    }

    /// The world clip's ladder boxes ([`WorldClip::ladder_boxes`]); none
    /// without a world clip.
    pub fn world_clip_ladders(&self) -> Vec<(Vec3, Vec3)> {
        self.world_clip
            .as_ref()
            .map_or_else(Vec::new, |c| c.ladder_boxes())
    }

    /// The world clip's half of a trace, into a fresh `trace`.
    fn trace_world_clip(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        trace: &mut Trace,
    ) {
        let Some(clip) = &self.world_clip else {
            return;
        };
        let h = clip.trace(start, end, mins, maxs, mask);
        trace.fraction = h.fraction;
        trace.normal = h.normal;
        trace.surface_flags = h.surface_flags;
        trace.startsolid = h.startsolid;
        trace.allsolid = h.allsolid;
        if h.fraction < 1.0 || h.startsolid {
            trace.hit = Some(Prim::Clip(h.contents));
        }
    }

    /// Which engine build's arithmetic every trace follows: 1.1d's, 1.5b's
    /// and United Offensive's ([`Arith::Stored`]), whose unoptimized code
    /// stores every float variable, or the Windows clients'
    /// ([`Arith::Msvc`]). Under `Stored` a terrain point trace takes
    /// `Terrain::point_clip_stored`, and every trace clips each patch as
    /// its build makes it, which leaves cracks along other facet seams
    /// (docs/research/bsp-ibsp59-format.md, "Patch collision on 1.5 and
    /// UO").
    pub fn set_arith(&self, arith: Arith) {
        self.arith.store(arith as u8, Ordering::Release);
    }

    /// The build every trace runs as ([`Self::set_arith`]).
    pub fn arith(&self) -> Arith {
        match self.arith.load(Ordering::Acquire) {
            x if x == Arith::Stored as u8 => Arith::Stored,
            x if x == Arith::Msvc as u8 => Arith::Msvc,
            _ => Arith::Kept,
        }
    }

    /// Every placement a trace reads beyond the static lumps and the link
    /// bits, as bits in stored order: the arithmetic, the posed submodels,
    /// the world copies and the submodel instances (vehicles). Two worlds
    /// over the same lumps and links trace alike when these match.
    pub fn placements(&self) -> Vec<u32> {
        fn eat(out: &mut Vec<u32>, v: Vec3) {
            out.extend(v.to_array().map(f32::to_bits));
        }
        fn pose(out: &mut Vec<u32>, p: &ModelPose) {
            eat(out, p.origin);
            p.axis.iter().for_each(|&a| eat(out, a));
        }
        let mut out = vec![self.arith.load(Ordering::Acquire) as u32];
        for (m, p) in self.poses.read().unwrap_or_else(|e| e.into_inner()).iter() {
            out.push(*m as u32);
            pose(&mut out, p);
        }
        for c in self.copies.read().unwrap_or_else(|e| e.into_inner()).iter() {
            out.extend([u32::MAX, c.entity, c.contents]);
            eat(&mut out, c.origin);
            c.axis.iter().flatten().for_each(|&a| eat(&mut out, a));
        }
        for i in self
            .instances
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            out.extend([u32::MAX - 1, i.entity, i.model as u32, i.contents]);
            pose(&mut out, &i.pose);
        }
        out
    }

    /// Puts model `model`'s brushes where an entity at `origin` with
    /// `angles` holds them: a script mover's brush model follows its entity
    /// (docs/research/cod11-movers.md, section 11). The spawn placement, the
    /// entity lump's origin and no rotation, takes the pose off again. Model
    /// 0 and an index past the model count are ignored.
    pub fn set_model_pose(&self, model: usize, origin: Vec3, angles: Vec3) {
        let Some(span) = self.model_span.get(model).copied() else {
            return;
        };
        if model == 0 {
            return;
        }
        let mut poses = self.poses.write().unwrap_or_else(|e| e.into_inner());
        poses.retain(|(m, _)| *m != model);
        let spawn = origin == span.origin && angles == Vec3::ZERO;
        self.model_posed[model].store(!spawn, Ordering::Relaxed);
        if spawn {
            self.any_posed.store(!poses.is_empty(), Ordering::Release);
            return;
        }
        poses.push((model, Self::model_pose(span, origin, angles)));
        self.any_posed.store(true, Ordering::Release);
    }

    /// Model `span` at `origin` and `angles`, with the world box it then
    /// covers.
    fn model_pose(span: ModelSpan, origin: Vec3, angles: Vec3) -> ModelPose {
        let mut pose = ModelPose {
            origin,
            axis: crate::x87::rotation_matrix(angles),
            turned: angles != Vec3::ZERO,
            lo: Vec3::splat(f32::MAX),
            hi: Vec3::splat(f32::MIN),
        };
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { span.lo.x } else { span.hi.x },
                if i & 2 == 0 { span.lo.y } else { span.hi.y },
                if i & 4 == 0 { span.lo.z } else { span.hi.z },
            );
            let w = pose.rotate(corner) + origin;
            pose.lo = pose.lo.min(w);
            pose.hi = pose.hi.max(w);
        }
        pose
    }

    /// Puts entity `entity`'s own copy of submodel `model` at `origin` and
    /// `angles` with `r.contents` `contents`, or takes it out on `None`. A
    /// trace reports the entity on its brushes, and a `Pusher::Copy` of the
    /// entity passes them. Model 0 and an index past the model count are
    /// ignored.
    pub fn set_model_instance(&self, entity: u32, model: usize, pose: Option<(Vec3, Vec3, u32)>) {
        let Some(span) = self.model_span.get(model).copied() else {
            return;
        };
        if model == 0 {
            return;
        }
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        instances.retain(|i| i.entity != entity);
        if let Some((origin, angles, contents)) = pose {
            instances.push(ModelInstance {
                entity,
                model,
                pose: Self::model_pose(span, origin, angles),
                contents,
            });
        }
        self.any_instance
            .store(!instances.is_empty(), Ordering::Release);
    }

    /// Takes entity `entity`'s submodel copy out of the traces, as
    /// `trap_UnlinkEntity` would, for [`Self::relink_model_instance`] to
    /// put back. `None` when the entity has none.
    pub fn unlink_model_instance(&self, entity: u32) -> Option<UnlinkedInstance> {
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        let i = instances.iter().position(|i| i.entity == entity)?;
        let inst = instances.remove(i);
        self.any_instance
            .store(!instances.is_empty(), Ordering::Release);
        Some(UnlinkedInstance(inst))
    }

    /// Puts back a copy [`Self::unlink_model_instance`] took out, where it
    /// stood.
    pub fn relink_model_instance(&self, unlinked: UnlinkedInstance) {
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        instances.retain(|i| i.entity != unlinked.0.entity);
        instances.push(unlinked.0);
        self.any_instance.store(true, Ordering::Release);
    }

    /// Lump 27's model count, the world's model 0 included.
    pub fn model_count(&self) -> usize {
        self.model_span.len()
    }

    /// Back to the spawn placement, as a map load or restart has it.
    pub fn reset_model_pose(&self, model: usize) {
        if let Some(span) = self.model_span.get(model) {
            self.set_model_pose(model, span.origin, Vec3::ZERO);
        }
    }

    /// Puts entity `entity`'s copy of the world clip at `origin` and
    /// `angles` with `r.contents` `contents`, or takes it out on `None`: a
    /// brush entity whose inline model is 0 (movers doc 19). Retail clips
    /// it through `CM_TransformedBoxTrace` on the world tree; vcod turns
    /// the sweep into the copy's frame and keeps its box axial there.
    pub fn set_world_copy(&self, entity: u32, pose: Option<(Vec3, Vec3, i32)>) {
        let mut copies = self.copies.write().unwrap_or_else(|e| e.into_inner());
        copies.retain(|c| c.entity != entity);
        if let Some((origin, angles, contents)) = pose {
            let axis = (angles != Vec3::ZERO).then(|| crate::x87::rotation_matrix(angles));
            copies.push(WorldCopy {
                entity,
                origin,
                axis,
                contents: contents as u32,
            });
        }
        self.any_copy.store(!copies.is_empty(), Ordering::Release);
    }

    /// A `r.contents` write on entity `entity`'s world copy, which stays
    /// where it is (`solid` / `notSolid`).
    pub fn set_world_copy_contents(&self, entity: u32, contents: i32) {
        let mut copies = self.copies.write().unwrap_or_else(|e| e.into_inner());
        for c in copies.iter_mut().filter(|c| c.entity == entity) {
            c.contents = contents as u32;
        }
    }

    /// Takes every world copy out, as a map load or restart frees their
    /// entities.
    pub fn clear_world_copies(&self) {
        self.copies
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.any_copy.store(false, Ordering::Release);
        self.instances
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.any_instance.store(false, Ordering::Release);
    }

    /// A box sweep against model `model`'s brushes alone, wherever its pose
    /// has them, under `mask`: the pusher test of `G_MoverPush`, which asks
    /// with a plain `trap_Trace` box under the entity's own clipmask
    /// (game.mp 0x554ed) whether it is in the mover's way rather than in
    /// anything's.
    pub fn model_box_trace(
        &self,
        model: usize,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
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
            copy: None,
        };
        if model < self.model_span.len() && model != 0 && self.model_linked(model) {
            let pose = self
                .poses
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|(m, _)| *m == model)
                .map(|(_, p)| *p);
            let (mut a, mut b) = (Vec::new(), Vec::new());
            self.clip_model(
                model,
                pose.as_ref(),
                start,
                end,
                Shape::Box(mins, maxs),
                mask,
                &mut trace,
                &mut a,
                &mut b,
            );
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// [`Self::model_box_trace`] against either kind of pusher. A world copy
    /// is clipped through the world tree as `trap_Trace` clips it: its
    /// brushes take the box, its terrain and patches the capsule (movers doc
    /// 21: a capsule under a sloped ceiling met it two frames late).
    pub fn pusher_box_trace(
        &self,
        pusher: Pusher,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Trace {
        let entity = match pusher {
            Pusher::Model(m) => return self.model_box_trace(m, start, end, mins, maxs, mask),
            Pusher::Copy(e) => e,
        };
        let copy = self
            .copies
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.entity == entity)
            .copied();
        let mut trace = match copy.filter(|c| c.contents & mask != 0) {
            Some(c) => self.clip_copy(c, start, end, mins, maxs, mask, true),
            None => Trace {
                fraction: 1.0,
                endpos: end,
                normal: Vec3::ZERO,
                surface_flags: 0,
                startsolid: false,
                allsolid: false,
                hit: None,
                enter: -1.0,
                copy: None,
            },
        };
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Clips a sweep against one model's brushes, moved by `pose` when there
    /// is one and where the build left them otherwise. As the leaf trace a
    /// submodel's sweep goes to (1.1d 0x8055608, UO 0x8059875), the brushes
    /// clip in lump order and a brush that brings the fraction to 0 ends the
    /// model: a box it meets at 0 never reads a later brush all solid
    /// (bsp-ibsp59-format.md, "The leaf walk").
    #[allow(clippy::too_many_arguments)]
    fn clip_model(
        &self,
        model: usize,
        pose: Option<&ModelPose>,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        trace: &mut Trace,
        moved: &mut Vec<(Vec3, f32)>,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let span = self.model_span[model];
        if let Some(pose) = pose.filter(|p| p.turned) {
            self.clip_turned_model(model, pose, start, end, shape, mask, trace, scratch);
            return;
        }
        let moving = trace.fraction > 0.0;
        for b in span.brushes.0..span.brushes.1 {
            let brush = &self.brushes[b as usize];
            if brush.content_flags & mask == 0 {
                continue;
            }
            if moving && trace.fraction <= 0.0 {
                break;
            }
            let planes = match pose {
                Some(pose) => {
                    moved.clear();
                    moved.extend(brush.planes.iter().map(|&pl| pose.plane(pl, span.origin)));
                    &moved[..]
                }
                None => &brush.planes[..],
            };
            let (centre, sphere) = match shape {
                Shape::Capsule(c, _, _) => {
                    expand_brush(planes, c.radius, scratch);
                    (c.center, c.offset)
                }
                Shape::Box(mins, maxs) => {
                    expand_brush_box(planes, mins, maxs, scratch);
                    (Vec3::ZERO, Vec3::ZERO)
                }
            };
            clip_segment(
                trace,
                start + centre,
                end + centre,
                scratch,
                &brush.side_surface_flags,
                sphere,
                Prim::Brush(b),
            );
        }
    }

    /// [`Self::clip_model`] for a turned pose, as `CM_TransformedBoxTrace`
    /// serves an inline model with any angle set (`cod_lnxded` 0x80568d8,
    /// called from 0x8090f18 with `r.currentAngles`): the sweep's centre
    /// turned into the model's frame, the box axial there, the capsule's
    /// sphere offset turned, and the brushes clipped as built (movers doc
    /// 22). The normal comes back in the world's frame.
    #[allow(clippy::too_many_arguments)]
    fn clip_turned_model(
        &self,
        model: usize,
        pose: &ModelPose,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let span = self.model_span[model];
        let (mins, maxs) = match shape {
            Shape::Capsule(_, mins, maxs) | Shape::Box(mins, maxs) => (mins, maxs),
        };
        let mid = (mins + maxs) * 0.5;
        let (ls, le) = (
            pose.to_local(start + mid, span.origin) - mid,
            pose.to_local(end + mid, span.origin) - mid,
        );
        let moving = trace.fraction > 0.0;
        for b in span.brushes.0..span.brushes.1 {
            let brush = &self.brushes[b as usize];
            if brush.content_flags & mask == 0 {
                continue;
            }
            if moving && trace.fraction <= 0.0 {
                break;
            }
            match shape {
                Shape::Capsule(c, _, _) => {
                    let [f, l, u] = pose.axis;
                    let sphere = Vec3::new(f.z, -l.z, u.z) * c.offset.z;
                    let sweep = capsule_sweep(ls, le, mins, maxs);
                    clip_brush_capsule(
                        trace,
                        &sweep,
                        sphere,
                        &brush.planes,
                        &brush.side_surface_flags,
                        Prim::Brush(b),
                        self.arith(),
                        false,
                    );
                }
                Shape::Box(_, _) => {
                    expand_brush_box(&brush.planes, mins, maxs, scratch);
                    clip_segment(
                        trace,
                        ls,
                        le,
                        scratch,
                        &brush.side_surface_flags,
                        Vec3::ZERO,
                        Prim::Brush(b),
                    );
                }
            }
        }
        // One call per model and trace, so a hit on its brushes is this one's.
        if matches!(trace.hit, Some(Prim::Brush(b)) if (span.brushes.0..span.brushes.1).contains(&b))
        {
            trace.normal = pose.rotate(trace.normal);
        }
    }

    /// The posed models' half of a trace: the BVH skipped their brushes.
    #[allow(clippy::too_many_arguments)]
    fn trace_posed(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        capsule: Capsule,
        mask: u32,
        skip: Option<usize>,
        boxed: bool,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        if !self.any_posed.load(Ordering::Acquire) {
            return;
        }
        let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
        let mut moved = Vec::new();
        for (model, pose) in poses.iter() {
            let cur_end = start + (end - start) * trace.fraction;
            let lo = start.min(cur_end) + mins - Vec3::ONE;
            let hi = start.max(cur_end) + maxs + Vec3::ONE;
            if !(lo.cmple(pose.hi).all() && hi.cmpge(pose.lo).all())
                || !self.model_linked(*model)
                || skip == Some(*model)
            {
                continue;
            }
            self.clip_model(
                *model,
                Some(pose),
                start,
                end,
                if boxed {
                    Shape::Box(mins, maxs)
                } else {
                    Shape::Capsule(capsule, mins, maxs)
                },
                mask,
                trace,
                &mut moved,
                scratch,
            );
        }
    }

    /// The entity whose link puts model `model`'s brushes in the clip, the
    /// `script_brushmodel` or other `*N` entity the map load spawned.
    pub fn set_model_entity(&self, model: usize, entity: u32) {
        if let Some(m) = self.model_entity.get(model) {
            m.store(entity, Ordering::Relaxed);
        }
    }

    fn model_entity_num(&self, model: u32) -> u32 {
        self.model_entity
            .get(model as usize)
            .map_or(ENTITYNUM_WORLD, |m| m.load(Ordering::Relaxed))
    }

    /// Q3's `trace.entityNum`: the entity the reported contact belongs to,
    /// `ENTITYNUM_WORLD` for world geometry and `ENTITYNUM_NONE` for a trace
    /// that hit nothing.
    pub fn entity_num(&self, trace: &Trace) -> u32 {
        if let Some(e) = trace.copy {
            return e;
        }
        match trace.hit {
            Some(Prim::Brush(b)) => self
                .model_entity
                .get(self.brushes[b as usize].model as usize)
                .map_or(ENTITYNUM_WORLD, |m| m.load(Ordering::Relaxed)),
            Some(Prim::Body(e)) => e,
            Some(Prim::Patch(p)) => self.model_entity_num(self.patches[p as usize].model),
            Some(Prim::Terrain(p)) => self.model_entity_num(self.terrain_model[p as usize]),
            Some(Prim::Tri(t)) => {
                let parts = &self.terrain.parts;
                let part = parts.partition_point(|p| p.first <= t).saturating_sub(1);
                self.model_entity_num(self.terrain_model.get(part).copied().unwrap_or(0))
            }
            Some(_) => ENTITYNUM_WORLD,
            None if trace.fraction < 1.0 => ENTITYNUM_WORLD,
            None => ENTITYNUM_NONE,
        }
    }

    /// The contents word of what a trace stopped on, the locational trace's
    /// `+32` (docs/research/cod11-combat.md 3.2): a brush's material word, a
    /// prop surface's own, a triangle's kept bits. 0 for a miss and for a
    /// body, whose contents live on its entity.
    pub fn hit_contents(&self, trace: &Trace) -> u32 {
        match trace.hit {
            Some(Prim::Brush(b)) => self.brushes[b as usize].content_flags,
            Some(Prim::Tri(t)) => self.tris_contents[t as usize],
            Some(Prim::Patch(p)) => self.patches[p as usize].content_flags,
            Some(Prim::Model(t)) => self.model_tris[t as usize].contents,
            Some(Prim::Clip(contents)) => contents,
            Some(Prim::Terrain(_) | Prim::Body(_)) | None => 0,
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

    /// Whether model `model`'s brushes clip where the BSP put them: linked
    /// and not moved off its spawn pose. Model 0 always.
    pub fn model_in_place(&self, model: usize) -> bool {
        self.model_linked(model)
            && !self
                .model_posed
                .get(model)
                .is_some_and(|m| m.load(Ordering::Relaxed))
    }

    /// Unlinks the brush models the stock map-load scripts take out of the
    /// clip, for a world no script and no snapshot drives (the gates; the
    /// client takes them from the snapshot, `pmove::movers`):
    /// `_gameobjects::main` `delete()`s every entity whose
    /// `script_gameobjectname` the gametype does not list (`dm.gsc:78`,
    /// `tdm.gsc:78`: their own name; `sd.gsc:123`: `sd`, `bombzone`,
    /// `blocker`), and `_load.gsc` `notsolid()`s every exploder brush model.
    pub fn unlink_script_brushes(&self, entities: &str, gametype: &str) {
        let allowed: &[&str] = match gametype {
            "sd" => &["sd", "bombzone", "blocker"],
            "re" => &["re", "retrieval"],
            g => &[g][..],
        };
        for block in crate::bsp::entity_blocks(entities) {
            let deleted = block
                .get("script_gameobjectname")
                .is_some_and(|name| !allowed.contains(&name.as_str()));
            let exploder = block
                .get("classname")
                .is_some_and(|c| c == "script_brushmodel")
                && block.contains_key("script_exploder")
                && block
                    .get("targetname")
                    .is_some_and(|t| t == "exploder" || t == "exploderchunk");
            if !deleted && !exploder {
                continue;
            }
            if let Some(n) = block
                .get("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            {
                self.set_model_linked(n, false);
            }
        }
    }

    /// Whether a partition model `model` owns clips `sw`, as its brushes
    /// would: linked, where it spawned, and not the sweep's pass entity. A
    /// posed submodel's partitions are not clipped at all.
    fn model_clips(&self, model: u32, sw: &TraceInput) -> bool {
        model == 0
            || (!sw.world_only
                && sw.skip != Some(model as usize)
                && self.model_linked(model as usize)
                && !self.model_posed[model as usize].load(Ordering::Relaxed))
    }

    fn brush_linked(&self, brush: &BrushPlanes) -> bool {
        self.model_linked(brush.model as usize)
    }

    /// Whether the BVH's copy of this brush is where the brush is: false for
    /// a posed model's, which [`Self::trace_posed`] clips instead.
    fn brush_at_spawn(&self, brush: &BrushPlanes) -> bool {
        !self.model_posed[brush.model as usize].load(Ordering::Relaxed)
    }

    /// A point trace against the water brushes alone, the mask 0x20 trace
    /// UO's `G_ExplodeMissile` finds the surface with: the nearest water
    /// side the segment enters, as a fraction of it, or `None`.
    pub fn water_trace(&self, start: Vec3, end: Vec3) -> Option<f32> {
        self.water_hit(start, end).map(|t| t.fraction)
    }

    /// [`Self::water_trace`] with the side it entered: its normal and its
    /// material's surface flags, what `G_RunMissile`'s splash reads
    /// (docs/research/cod11-combat.md 12.1). A start inside water reads
    /// fraction 0.
    pub fn water_hit(&self, start: Vec3, end: Vec3) -> Option<Trace> {
        let mut trace = Trace::clear(end);
        for v in &self.water {
            if !(start.min(end).cmple(v.hi).all() && start.max(end).cmpge(v.lo).all()) {
                continue;
            }
            clip_segment(
                &mut trace,
                start,
                end,
                &v.planes,
                &v.side_flags,
                Vec3::ZERO,
                Prim::Brush(0),
            );
            if trace.allsolid {
                trace.endpos = start;
                return Some(trace);
            }
        }
        trace.endpos = start + (end - start) * trace.fraction;
        (trace.fraction < 1.0).then_some(trace)
    }

    /// Contents at a point: `CONTENTS_WATER` inside any water brush,
    /// [`CONTENTS_NODROP`] inside any world nodrop brush, plus the content
    /// flags of any solid brush containing it (Q3 `CM_PointContents` over the
    /// brushes that made it into the world).
    pub fn point_contents(&self, p: Vec3) -> u32 {
        let inside = |v: &Volume| {
            p.cmple(v.hi).all()
                && p.cmpge(v.lo).all()
                && v.planes.iter().all(|&(n, d)| n.dot(p) <= d)
        };
        let mut out = 0;
        if self.water.iter().any(inside) {
            out |= CONTENTS_WATER;
        }
        if self.nodrop.iter().any(inside) {
            out |= CONTENTS_NODROP;
        }
        if let Some(clip) = &self.world_clip {
            out |= clip.point_contents(p);
        }
        if self.nodes.is_empty() {
            return out;
        }
        // The traces' BVH: pmove samples this on every step.
        out |= self.node_contents(0, p, false);
        if self.any_copy.load(Ordering::Acquire) {
            let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
            for c in copies.iter().filter(|c| c.contents != 0) {
                out |= self.node_contents(0, c.to_local(p), true);
            }
        }
        if !self.any_posed.load(Ordering::Acquire) {
            return out;
        }
        let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
        for (model, pose) in poses.iter() {
            if !(p.cmple(pose.hi).all() && p.cmpge(pose.lo).all()) || !self.model_linked(*model) {
                continue;
            }
            let span = self.model_span[*model];
            for brush in &self.brushes[span.brushes.0 as usize..span.brushes.1 as usize] {
                if brush.planes.iter().all(|&pl| {
                    let (n, d) = pose.plane(pl, span.origin);
                    n.dot(p) <= d
                }) {
                    out |= brush.content_flags;
                }
            }
        }
        out
    }

    /// The contents of the spawn-placed brushes under node `i` that hold `p`,
    /// model 0's alone when `world_only`.
    fn node_contents(&self, i: u32, p: Vec3, world_only: bool) -> u32 {
        let node = &self.nodes[i as usize];
        if i == self.models_root || !(p.cmple(node.hi).all() && p.cmpge(node.lo).all()) {
            return 0;
        }
        if node.count == 0 {
            return self.node_contents(node.first, p, world_only)
                | self.node_contents(node.second, p, world_only);
        }
        let first = node.first as usize;
        let mut out = 0;
        for (prim, lo, hi) in &self.prims[first..first + node.count as usize] {
            if let Prim::Brush(b) = prim
                && p.cmple(*hi).all()
                && p.cmpge(*lo).all()
            {
                let brush = &self.brushes[*b as usize];
                if self.brush_linked(brush)
                    && self.brush_at_spawn(brush)
                    && (!world_only || brush.model == 0)
                    && brush.planes.iter().all(|&(n, d)| n.dot(p) <= d)
                {
                    out |= brush.content_flags;
                }
            }
        }
        out
    }

    /// [`Self::box_position_test`] against inline model 0 alone (brushes,
    /// terrain and patches, no submodel, world copy or static model):
    /// `SV_Trace`'s world clip, which ends the trace at fraction 0 before
    /// any entity is walked (`cod_lnxded` 0x80916f4).
    pub fn world_box_position_test(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> bool {
        self.position_walk(p, mins, maxs, mask, true).startsolid
    }

    fn position_walk(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32, world_only: bool) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            endpos: p,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
            copy: None,
        };
        let capsule = Capsule::of(mins, maxs);
        self.trace_world_clip(p, p, mins, maxs, mask, &mut trace);
        self.walk_tree(
            TraceInput {
                start: p,
                end: p,
                mins,
                maxs,
                capsule,
                mask,
                statics: false,
                skip: None,
                world_only,
                boxed: true,
                sphere: capsule.offset,
            },
            &mut trace,
        );
        trace
    }

    /// Whether a box at `p` is inside anything under `mask`, as `trap_Trace`
    /// with a box that does not move reads it: brushes (posed submodels and
    /// world copies included) against the box, terrain and patches by their
    /// capsule position tests. `G_TryPushingEntity`'s test of a spot
    /// (game.mp 0x54b3b, docs/research/cod11-movers.md 20).
    pub fn box_position_test(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> bool {
        let mut trace = self.position_walk(p, mins, maxs, mask, false);
        if trace.startsolid {
            return true;
        }
        if self.any_posed.load(Ordering::Acquire) {
            let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
            let (mut moved, mut scratch) = (Vec::new(), Vec::new());
            for (model, pose) in poses.iter() {
                if !self.model_linked(*model) {
                    continue;
                }
                self.clip_model(
                    *model,
                    Some(pose),
                    p,
                    p,
                    Shape::Box(mins, maxs),
                    mask,
                    &mut trace,
                    &mut moved,
                    &mut scratch,
                );
            }
            if trace.startsolid {
                return true;
            }
        }
        if self.any_copy.load(Ordering::Acquire) {
            let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
            if copies
                .iter()
                .filter(|c| c.contents & mask != 0)
                .any(|c| self.clip_copy(*c, p, p, mins, maxs, mask, true).startsolid)
            {
                return true;
            }
        }
        self.trace_instances(p, p, Shape::Box(mins, maxs), mask, None, &mut trace);
        trace.startsolid
    }

    /// Movement sweep (`mins == maxs == ZERO` is a ray), retail's
    /// `trap_Trace` / `trap_TraceCapsule`: brushes and terrain, never a
    /// static model (`cod_lnxded` 0x80916f4 traces them only on the flag the
    /// locational syscall passes).
    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, TRACE_MASK_MOVE, false, None)
    }

    /// [`Self::box_trace`] under pmove's `tracemask`.
    pub fn move_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, false, None)
    }

    /// A box sweep under `mask` that passes through `pusher`'s clip: a
    /// pushed body's sweep from where it stood to where the push puts it,
    /// which must not stop on the pusher it starts inside.
    pub fn box_trace_except(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        pusher: Pusher,
    ) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, false, Some(pusher))
    }

    /// Bullet segment, retail's `trap_LocationalTrace`: [`MASK_SHOT`] and the
    /// static models.
    pub fn shot_trace(&self, start: Vec3, end: Vec3) -> Trace {
        self.trace_with_mask(
            start,
            end,
            Vec3::ZERO,
            Vec3::ZERO,
            TRACE_MASK_SHOT,
            true,
            None,
        )
    }

    /// A point segment with any mask, with or without the static models:
    /// retail's `trap_Trace` on a zero box is one without them.
    pub fn point_trace(&self, start: Vec3, end: Vec3, mask: u32, statics: bool) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, statics, None)
    }

    /// [`Self::point_trace`] passing through `skip`'s clip: a trace whose
    /// pass entity is a brush model.
    pub fn point_trace_except(
        &self,
        start: Vec3,
        end: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, statics, skip)
    }

    /// A missile's segment, `G_RunMissile`'s `trap_LocationalTrace`:
    /// [`MASK_MISSILE_MOVE`] and the static models, which that syscall's
    /// `SV_Trace` walks (cod11-mantle.md, "Static models are clipped as a
    /// segment"; combat doc 12.3: a frag rests on a crate's mesh).
    pub fn missile_trace(&self, start: Vec3, end: Vec3) -> Trace {
        self.trace_with_mask(
            start,
            end,
            Vec3::ZERO,
            Vec3::ZERO,
            MASK_MISSILE_MOVE,
            true,
            None,
        )
    }

    /// An item's sweep, `G_RunItem`'s and `G_BounceItem`'s: the item's own
    /// box as a capsule (every item carries `eFlags` 0x10, which picks
    /// `trap_TraceCapsule`), the given mask, and the static models, as the
    /// missile's sweep has them (docs/research/cod11-items.md, section 8).
    pub fn item_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, true, None)
    }

    /// `trap_Trace`'s box sweep under `mask`, passing through `skip`'s
    /// clip: brushes take the box, terrain and patches the capsule (the
    /// engine fakes a box as a capsule only against terrain, 0x80536c8),
    /// [`Capsule::wide`]. What a UO vehicle's slide and probes sweep.
    pub fn box_sweep(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_shape(start, end, mins, maxs, mask, false, skip, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_with_mask(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_shape(start, end, mins, maxs, mask, statics, skip, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_shape(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
        boxed: bool,
    ) -> Trace {
        let (skip_model, skip_copy) = match skip {
            Some(Pusher::Model(m)) => (Some(m), None),
            Some(Pusher::Copy(e)) => (None, Some(e)),
            None => (None, None),
        };
        let mut trace = Trace {
            fraction: 1.0,
            endpos: end,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
            copy: None,
        };

        let mut scratch = Vec::new();
        let capsule = if boxed {
            Capsule::wide(mins, maxs)
        } else {
            Capsule::of(mins, maxs)
        };
        self.trace_world_clip(start, end, mins, maxs, mask, &mut trace);
        self.walk_tree(
            TraceInput {
                start,
                end,
                mins,
                maxs,
                capsule,
                mask,
                statics,
                skip: skip_model,
                world_only: false,
                boxed,
                sphere: capsule.offset,
            },
            &mut trace,
        );
        let world_fraction = trace.fraction;
        self.trace_posed(
            start,
            end,
            mins,
            maxs,
            capsule,
            mask,
            skip_model,
            boxed,
            &mut trace,
            &mut scratch,
        );
        self.trace_copies(start, end, mins, maxs, mask, skip_copy, boxed, &mut trace);
        self.trace_instances(
            start,
            end,
            if boxed {
                Shape::Box(mins, maxs)
            } else {
                Shape::Capsule(capsule, mins, maxs)
            },
            mask,
            skip_copy,
            &mut trace,
        );
        let arith = self.arith();
        trace.endpos = if arith != Arith::Msvc && trace.fraction < world_fraction {
            sv_trace_endpos(start, end, trace.fraction, arith)
        } else {
            cm_endpos(start, end, mins, maxs, trace.fraction, arith)
        };
        trace
    }

    /// The model instances' half of a trace, each clipped where its entity
    /// stands and merged as [`Self::trace_copies`] merges a copy's.
    fn trace_instances(
        &self,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        skip: Option<u32>,
        trace: &mut Trace,
    ) {
        if !self.any_instance.load(Ordering::Acquire) {
            return;
        }
        let (mins, maxs) = match shape {
            Shape::Box(lo, hi) | Shape::Capsule(_, lo, hi) => (lo, hi),
        };
        let instances = self.instances.read().unwrap_or_else(|e| e.into_inner());
        let (mut moved, mut scratch) = (Vec::new(), Vec::new());
        for inst in instances.iter() {
            if inst.contents & mask == 0 || trace.allsolid || skip == Some(inst.entity) {
                continue;
            }
            let lo = start.min(end) + mins - Vec3::ONE;
            let hi = start.max(end) + maxs + Vec3::ONE;
            if !(lo.cmple(inst.pose.hi).all() && hi.cmpge(inst.pose.lo).all()) {
                continue;
            }
            let mut t = Trace {
                fraction: 1.0,
                endpos: end,
                normal: Vec3::ZERO,
                surface_flags: 0,
                startsolid: false,
                allsolid: false,
                hit: None,
                enter: -1.0,
                copy: None,
            };
            self.clip_model(
                inst.model,
                Some(&inst.pose),
                start,
                end,
                shape,
                mask,
                &mut t,
                &mut moved,
                &mut scratch,
            );
            t.copy = (t.fraction < 1.0 || t.startsolid).then_some(inst.entity);
            if t.allsolid || t.fraction < trace.fraction {
                let startsolid = trace.startsolid || t.startsolid;
                *trace = Trace { startsolid, ..t };
            } else if t.startsolid {
                trace.startsolid = true;
            }
        }
    }

    /// The tree's half of a trace.
    fn walk_tree(&self, sweep: TraceInput, trace: &mut Trace) {
        if self.nodes.is_empty() {
            return;
        }
        // Brushes, then the partitions: retail's leaf walk clips a
        // leaf's brushes before its partitions (`cod_lnxded` 0x8055608),
        // and a contact at fraction 0 ends the walk, so a box resting on
        // a brush floor beside a patch reads the floor. The brush walk
        // records the leaves it enters; the partition pass visits a
        // subsequence of them in the same order (its fraction is never
        // larger), so it replays the list instead of walking again.
        let entry = *trace;
        NEAR_TIE.set(false);
        let mut walk = Walk {
            sweep,
            leaves: [0; LEAF_LOG],
            logged: 0,
            patch_met: std::cell::Cell::new(false),
            rest: false,
        };
        self.trace_node(0, &mut walk, true, trace);
        if walk.logged <= LEAF_LOG && !two_walks() {
            for &leaf in &walk.leaves[..walk.logged] {
                // A met patch throws this pass away.
                if walk.patch_met.get() {
                    break;
                }
                self.clip_leaf(leaf, &walk, false, trace);
            }
        } else {
            walk.logged = usize::MAX;
            self.trace_node(0, &mut walk, false, trace);
        }
        // A sweep that meets a patch starts over: model 0 in retail's leaf
        // order, then the submodels and the static models as before. So
        // does one whose contact nearly ties another, as a capsule standing
        // across two floor brushes, a brush and a terrain partition or two
        // partitions within the terrain's back-off do, and one stopped at
        // fraction 0 (a box always, a capsule all in solid or with a second
        // contact there), where the walk ends on whatever got there first:
        // retail's order names the contact. So does one stopped on terrain
        // no leaf on its walk lists, which retail never clips
        // (bsp-ibsp59-format.md, "The leaf walk").
        let tied = trace.fraction < 1.0
            && (NEAR_TIE.get()
                || trace.fraction <= 0.0
                    && (sweep.boxed || trace.allsolid || self.ties_at_zero(&sweep, trace))
                || self.terrain_off_the_walk(&sweep, trace)
                || self.brush_off_the_walk(&sweep, trace));
        if (walk.patch_met.get() || tied)
            && let Some(leaves) = &self.leaf_walk
        {
            *trace = entry;
            self.clip_leaf_walk(leaves, &sweep, trace);
            let mut rest = Walk {
                sweep,
                leaves: [0; LEAF_LOG],
                logged: usize::MAX,
                patch_met: std::cell::Cell::new(false),
                rest: true,
            };
            self.trace_node(0, &mut rest, true, trace);
            self.trace_node(0, &mut rest, false, trace);
        }
    }

    /// Whether the trace stopped on a model-0 terrain partition that no leaf
    /// the walk enters short of the contact lists. Retail clips a partition
    /// only from a leaf that lists it, so a sweep that reaches terrain from
    /// outside its leaves (out of the world, or under it) never meets it
    /// there, while the BVH does (bsp-ibsp59-format.md, "The leaf walk").
    fn terrain_off_the_walk(&self, sw: &TraceInput, trace: &Trace) -> bool {
        let (Some(Prim::Tri(t)), Some(leaves)) = (trace.hit, &self.leaf_walk) else {
            return false;
        };
        if self.tri_listed.get(t as usize).copied().unwrap_or(false) {
            return false;
        }
        let part = self.terrain.parts.partition_point(|p| p.first <= t) - 1;
        if self.terrain_model[part] != 0 {
            return false;
        }
        let item = Item::Terrain(part as u32);
        let lists = |leaf: u32| leaves.lists(leaf, item);
        // The leaf holding the contact lists the partition in the world, and
        // the walk enters it short of the contact; only a miss walks.
        let dir = sw.end - sw.start;
        let centre = sw.start + dir * trace.fraction + sw.capsule.center;
        let below = if trace.normal.z > 0.0 {
            -sw.capsule.offset
        } else {
            sw.capsule.offset
        };
        let contact = centre + below - trace.normal * sw.capsule.radius;
        if lists(leaves.leaf_at(contact.to_array())) {
            return false;
        }
        let mid = (sw.mins + sw.maxs) * 0.5;
        let mut listed = false;
        leaves.visit(
            (sw.start + mid).to_array(),
            (sw.end + mid).to_array(),
            (sw.maxs - mid).to_array(),
            sw.mins == Vec3::ZERO && sw.maxs == Vec3::ZERO,
            trace.fraction,
            |leaf, _| {
                listed |= lists(leaf);
                // A listing ends the walk.
                if listed { 0.0 } else { trace.fraction }
            },
        );
        !listed
    }

    /// Whether the trace stopped on a model-0 brush that no leaf the walk
    /// enters short of the contact lists. A leaf lists a brush only where
    /// the brush's volume reaches, and a sweep can meet its pushed-out
    /// planes past an edge with no bevel, outside every such leaf: retail
    /// never clips it there (bsp-ibsp59-format.md, "The leaf walk").
    fn brush_off_the_walk(&self, sw: &TraceInput, trace: &Trace) -> bool {
        let (Some(Prim::Brush(b)), Some(leaves)) = (trace.hit, &self.leaf_walk) else {
            return false;
        };
        if self.brushes[b as usize].model != 0 {
            return false;
        }
        let item = Item::Brush(b);
        let lists = |leaf: u32| leaves.lists(leaf, item);
        // A unit past the face the sweep stopped on is inside a brush it
        // really touched, in a leaf that lists it; only a miss walks.
        let dir = sw.end - sw.start;
        let centre = sw.start + dir * trace.fraction + sw.capsule.center;
        let below = if trace.normal.z > 0.0 {
            -sw.capsule.offset
        } else {
            sw.capsule.offset
        };
        let inside = centre + below - trace.normal * (sw.capsule.radius + 1.0);
        if lists(leaves.leaf_at(inside.to_array())) {
            return false;
        }
        let mid = (sw.mins + sw.maxs) * 0.5;
        let mut listed = false;
        leaves.visit(
            (sw.start + mid).to_array(),
            (sw.end + mid).to_array(),
            (sw.maxs - mid).to_array(),
            sw.mins == Vec3::ZERO && sw.maxs == Vec3::ZERO,
            trace.fraction,
            |leaf, _| {
                listed |= lists(leaf);
                if listed { 0.0 } else { trace.fraction }
            },
        );
        !listed
    }

    /// Whether a capsule the BVH stopped at fraction 0 has a second contact
    /// there that retail's walk may have met first. The walk ends at 0 on
    /// whatever got there first, so the other prim was never clipped and
    /// nothing noted the tie. Each prim at the start box is clipped on its
    /// own: one reaching the tie window that reads otherwise (normal,
    /// surface, solid flags, or a brush of another model), or a model-0
    /// patch, is the tie (bsp-ibsp59-format.md, "The leaf walk").
    fn ties_at_zero(&self, sw: &TraceInput, trace: &Trace) -> bool {
        let model = |hit: Option<Prim>| match hit {
            Some(Prim::Brush(b)) => Some(self.brushes[b as usize].model),
            _ => None,
        };
        let key = |t: &Trace| {
            (
                t.normal.to_array().map(f32::to_bits),
                t.surface_flags,
                t.startsolid,
                t.allsolid,
                model(t.hit),
            )
        };
        let (lo, hi) = sw.bounds(0.0);
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if !sw.statics && i == self.models_root
                || !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all())
            {
                continue;
            }
            if node.count == 0 {
                stack.extend([node.first, node.second]);
                continue;
            }
            let first = node.first as usize;
            for &(prim, plo, phi) in &self.prims[first..first + node.count as usize] {
                if !(lo.cmple(phi).all() && hi.cmpge(plo).all()) || Some(prim) == trace.hit {
                    continue;
                }
                if let Prim::Patch(p) = prim
                    && self.leaf_walk.is_some()
                    && self.patches[p as usize].model == 0
                {
                    let patch = &self.patches[p as usize];
                    if patch.content_flags & sw.mask != 0
                        && patch_meets(sw.start, sw.end, sw.capsule, patch, self.arith())
                    {
                        return true;
                    }
                    continue;
                }
                let mut alone = Trace::clear(sw.end);
                self.clip_prim(prim, sw, &mut alone);
                if (alone.startsolid || f64::from(alone.fraction) <= TIE_WINDOW)
                    && key(&alone) != key(trace)
                {
                    return true;
                }
            }
        }
        false
    }

    /// The world copies' half of a trace, each clipped on its own in its
    /// frame and merged as Q3's `SV_ClipMoveToEntities` merges an entity's
    /// trace: it wins when allsolid or strictly nearer, and a copy the sweep
    /// starts in marks the result startsolid either way.
    #[allow(clippy::too_many_arguments)]
    fn trace_copies(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        skip: Option<u32>,
        boxed: bool,
        trace: &mut Trace,
    ) {
        if !self.any_copy.load(Ordering::Acquire) {
            return;
        }
        let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
        for c in copies.iter() {
            if c.contents & mask == 0 || trace.allsolid || skip == Some(c.entity) {
                continue;
            }
            let t = self.clip_copy(*c, start, end, mins, maxs, mask, boxed);
            if t.allsolid || t.fraction < trace.fraction {
                let startsolid = trace.startsolid || t.startsolid;
                *trace = Trace { startsolid, ..t };
            } else if t.startsolid {
                trace.startsolid = true;
            }
        }
    }

    /// One world copy alone against a sweep, in its frame; the normal comes
    /// back in the world's, `endpos` is left to the caller.
    #[allow(clippy::too_many_arguments)]
    fn clip_copy(
        &self,
        c: WorldCopy,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        boxed: bool,
    ) -> Trace {
        // `CM_TransformedBoxTrace` turns the box's centre into the copy's
        // frame, not its origin (movers doc 21, "The turned centre").
        let mid = (mins + maxs) * 0.5;
        let (ls, le) = (c.to_local(start + mid) - mid, c.to_local(end + mid) - mid);
        let capsule = Capsule::of(mins, maxs);
        let mut t = Trace {
            fraction: 1.0,
            endpos: le,
            normal: Vec3::ZERO,
            surface_flags: 0,
            startsolid: false,
            allsolid: false,
            hit: None,
            enter: -1.0,
            copy: None,
        };
        self.walk_tree(
            TraceInput {
                start: ls,
                end: le,
                mins,
                maxs,
                capsule,
                mask,
                statics: false,
                skip: None,
                world_only: true,
                boxed,
                sphere: c.sphere(capsule.offset),
            },
            &mut t,
        );
        Trace {
            normal: c.to_world_dir(t.normal),
            copy: (t.fraction < 1.0 || t.startsolid).then_some(c.entity),
            ..t
        }
    }

    /// Nearest child first; the sweep box shrinks with `trace.fraction`, so
    /// prims past the current hit and its tie window are never clipped.
    /// Clips the brushes or everything else, and logs each leaf it enters
    /// while `walk.logged` is not `usize::MAX`.
    fn trace_node(&self, i: u32, walk: &mut Walk, brushes: bool, trace: &mut Trace) {
        let sw = &walk.sweep;
        // The brush walk enters the static models' subtree only to log it
        // for the partition pass of a trace that clips them.
        if !sw.statics && i == self.models_root {
            return;
        }
        let node = &self.nodes[i as usize];
        if walk.rest && !node.rest {
            return;
        }
        let (lo, hi) = sw.reach(trace.fraction);
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all())
            || (!node.loose && sw.misses(node.lo, node.hi))
        {
            return;
        }
        if node.count > 0 {
            if walk.logged != usize::MAX {
                if let Some(slot) = walk.leaves.get_mut(walk.logged) {
                    *slot = i;
                }
                walk.logged += 1;
            }
            self.clip_leaf(i, walk, brushes, trace);
            return;
        }
        let (start, dir) = (sw.start, sw.end - sw.start);
        let along = |n: u32| {
            let node = &self.nodes[n as usize];
            ((node.lo + node.hi) * 0.5 - start).dot(dir)
        };
        let (near, far) = if along(node.first) <= along(node.second) {
            (node.first, node.second)
        } else {
            (node.second, node.first)
        };
        for n in [near, far] {
            self.trace_node(n, walk, brushes, trace);
        }
    }

    /// Leaf `i`'s brushes, or its other prims, against `sw`; nothing when
    /// the leaf's box misses the sweep cut at the current fraction.
    fn clip_leaf(&self, i: u32, walk: &Walk, brushes: bool, trace: &mut Trace) {
        let sw = &walk.sweep;
        let node = &self.nodes[i as usize];
        let (lo, hi) = sw.reach(trace.fraction);
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
            return;
        }
        let first = node.first as usize;
        for (prim, plo, phi) in &self.prims[first..first + node.count as usize] {
            // Retail's leaf walk stops once the fraction reaches 0
            // (`cod_lnxded` 0x8055608, 0x8055fe0).
            if trace.fraction <= 0.0 {
                return;
            }
            // The node test once more per prim: a leaf's box is the union
            // of up to four, and setting up a brush's planes costs far
            // more than this.
            if !(lo.cmple(*phi).all() && hi.cmpge(*plo).all())
                || brushes != matches!(prim, Prim::Brush(_))
                || (TraceInput::tight(*prim) && sw.misses(*plo, *phi))
            {
                continue;
            }
            match *prim {
                // The leaf walk has clipped model 0.
                Prim::Brush(b) if walk.rest && self.brushes[b as usize].model == 0 => continue,
                Prim::Terrain(p) if walk.rest && self.terrain_model[p as usize] == 0 => continue,
                Prim::Patch(p) if walk.rest && self.patches[p as usize].model == 0 => continue,
                // Model 0's patches clip only from the leaves that list them,
                // and a point's patch clip compares its unpushed fraction, so
                // its answer rides the order too: the leaf walk clips them.
                Prim::Patch(p)
                    if !walk.rest
                        && self.leaf_walk.is_some()
                        && self.patches[p as usize].model == 0 =>
                {
                    let patch = &self.patches[p as usize];
                    if patch.content_flags & sw.mask != 0
                        && patch_meets(sw.start, sw.end, sw.capsule, patch, self.arith())
                    {
                        walk.patch_met.set(true);
                        return;
                    }
                }
                prim => self.clip_prim(prim, sw, trace),
            }
        }
    }

    /// One prim against `sw`, whatever its box: the mask and link tests,
    /// then its clip.
    fn clip_prim(&self, prim: Prim, sw: &TraceInput, trace: &mut Trace) {
        let TraceInput {
            start,
            end,
            mins,
            maxs,
            capsule,
            mask,
            statics,
            skip,
            world_only,
            boxed,
            sphere,
        } = *sw;
        match prim {
            Prim::Brush(b) => {
                let brush = &self.brushes[b as usize];
                if brush.content_flags & mask == 0
                    || !self.brush_linked(brush)
                    || !self.brush_at_spawn(brush)
                    || skip == Some(brush.model as usize)
                    || (world_only && brush.model != 0)
                {
                    return;
                }
                if boxed {
                    let mut planes = Vec::new();
                    expand_brush_box(&brush.planes, mins, maxs, &mut planes);
                    clip_segment(
                        trace,
                        start,
                        end,
                        &planes,
                        &brush.side_surface_flags,
                        Vec3::ZERO,
                        prim,
                    );
                } else {
                    let sweep = capsule_sweep(start, end, mins, maxs);
                    let same_model = matches!(trace.hit,
                        Some(Prim::Brush(o)) if self.brushes[o as usize].model == brush.model);
                    clip_brush_capsule(
                        trace,
                        &sweep,
                        sphere,
                        &brush.planes,
                        &brush.side_surface_flags,
                        prim,
                        self.arith(),
                        same_model,
                    );
                }
            }
            Prim::Terrain(p) => {
                if !self.model_clips(self.terrain_model[p as usize], sw) {
                    return;
                }
                self.clip_terrain(p, start, end, mins, maxs, capsule, mask, trace);
            }
            Prim::Tri(_) => unreachable!("the BVH holds partitions"),
            Prim::Patch(p) => {
                let patch = &self.patches[p as usize];
                if patch.content_flags & mask == 0 || !self.model_clips(patch.model, sw) {
                    return;
                }
                clip_patch(trace, start, end, capsule, patch, self.arith(), prim);
            }
            Prim::Model(t) => {
                let mt = &self.model_tris[t as usize];
                if !statics || mt.contents & mask == 0 {
                    return;
                }
                clip_segment_model(trace, start, end, mt, prim);
            }
            // Never stored in world.prims; only movetrace.rs and the world
            // clip construct them.
            Prim::Body(_) | Prim::Clip(_) => unreachable!(),
        }
    }

    /// Model 0 in retail's order, for a sweep that meets a patch: each leaf
    /// `CM_TraceThroughTree` enters short of the fraction clips the brushes
    /// it lists, then its partitions, each prim once (`cod_lnxded`
    /// 0x8055fe0, 0x8055608). A patch takes a facet only where its unpushed
    /// fraction is within the trace's, so a brush or patch met in an
    /// earlier leaf can keep it out (bsp-ibsp59-format.md, "The leaf walk").
    fn clip_leaf_walk(&self, leaves: &LeafWalk, sw: &TraceInput, trace: &mut Trace) {
        thread_local! {
            /// Retail's check counts: the walk's number per item it clipped.
            static SEEN: std::cell::RefCell<(u32, Vec<u32>)> =
                const { std::cell::RefCell::new((0, Vec::new())) };
        }
        let (nb, nt) = (self.brushes.len(), self.terrain.parts.len());
        let slots = nb + nt + self.patches.len();
        let mid = (sw.mins + sw.maxs) * 0.5;
        SEEN.with_borrow_mut(|(walk, seen)| {
            if seen.len() < slots || *walk == u32::MAX {
                *seen = vec![0; slots];
                *walk = 0;
            }
            *walk += 1;
            let walk = *walk;
            leaves.visit(
                (sw.start + mid).to_array(),
                (sw.end + mid).to_array(),
                (sw.maxs - mid).to_array(),
                sw.mins == Vec3::ZERO && sw.maxs == Vec3::ZERO,
                trace.fraction,
                |leaf, _| {
                    let (mut lo, mut hi) = sw.bounds(trace.fraction);
                    for &Listing {
                        item,
                        lo: plo,
                        hi: phi,
                    } in leaves.items(leaf)
                    {
                        // The BVH's box test: a prim clear of the sweep cut
                        // at the fraction can neither start it solid nor cut
                        // it shorter, here or in a later leaf.
                        if !(lo.cmple(Vec3::from_array(phi)).all()
                            && hi.cmpge(Vec3::from_array(plo)).all())
                        {
                            continue;
                        }
                        let (slot, prim) = match item {
                            Item::Brush(b) => (b as usize, Prim::Brush(b)),
                            Item::Terrain(p) => (nb + p as usize, Prim::Terrain(p)),
                            Item::Patch(p) => (nb + nt + p as usize, Prim::Patch(p)),
                        };
                        if seen[slot] == walk {
                            continue;
                        }
                        seen[slot] = walk;
                        self.clip_prim(prim, sw, trace);
                        if trace.fraction <= 0.0 {
                            break;
                        }
                        (lo, hi) = sw.bounds(trace.fraction);
                    }
                    trace.fraction
                },
            );
        });
    }

    /// Terrain partition `part` against a sweep (docs/research/cod11-mantle.md,
    /// "Terrain is a swept sphere, a patch is a facet"): retail's capsule clip
    /// (`terrain.rs`), or for a point trace its point arm (0x8052894) on each
    /// triangle. A trace already at fraction 0 has stopped, as retail's tree
    /// walk does.
    #[allow(clippy::too_many_arguments)]
    fn clip_terrain(
        &self,
        part: u32,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        capsule: Capsule,
        mask: u32,
        trace: &mut Trace,
    ) {
        let tp = self.terrain.parts[part as usize];
        let first = tp.first as usize;
        if self.tris_contents[first] & mask == 0 || trace.fraction <= 0.0 {
            return;
        }
        let surface_flags = self.tris_surf[first];
        if capsule.radius == 0.0 {
            let (s, e) = (start + capsule.center, end + capsule.center);
            if !tp.touches_segment(s.to_array(), e.to_array()) {
                return;
            }
            let (s, e) = (s.to_array(), e.to_array());
            let hit = if self.arith() == Arith::Stored {
                self.terrain.point_clip_stored(&tp, s, e, trace.fraction)
            } else {
                self.terrain.point_clip(&tp, s, e, trace.fraction)
            };
            if let Some((f, normal, t)) = hit {
                trace.fraction = f;
                trace.enter = f;
                trace.normal = Vec3::from_array(normal);
                trace.surface_flags = surface_flags;
                trace.hit = Some(Prim::Tri(t));
            }
            return;
        }
        let sweep = CapsuleSweep {
            radius: capsule.radius,
            ..capsule_sweep(start, end, mins, maxs)
        };
        if !tp.touches(&sweep) {
            return;
        }
        if start == end {
            if self.terrain.position_test(&tp, &sweep) {
                trace.startsolid = true;
                trace.allsolid = true;
                trace.fraction = 0.0;
                trace.normal = Vec3::ZERO;
                trace.surface_flags = surface_flags;
                trace.hit = Some(Prim::Terrain(part));
            }
            return;
        }
        if let Some(h) =
            self.terrain
                .clip_capsule(&tp, &sweep, trace.fraction, self.arith() == Arith::Stored)
        {
            trace.fraction = h.fraction;
            trace.enter = h.fraction + crate::terrain::FRACTION_EPS;
            trace.normal = Vec3::from_array(h.normal);
            trace.surface_flags = surface_flags;
            trace.hit = Some(Prim::Tri(h.tri));
            trace.startsolid |= h.startsolid;
        }
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
                "terrain tri {t} sf={:#x} cf={:#x}",
                self.tris_surf[t as usize], self.tris_contents[t as usize]
            ),
            Prim::Terrain(p) => format!("terrain partition {p}"),
            Prim::Patch(p) => {
                let patch = &self.patches[p as usize];
                format!(
                    "patch {} sf={:#x} cf={:#x}",
                    patch.index, patch.surface_flags, patch.content_flags
                )
            }
            Prim::Model(t) => {
                let mt = &self.model_tris[t as usize];
                format!(
                    "xmodel tri {t} cf={:#x} sf={:#x} at {:?}",
                    mt.contents, mt.surface_flags, mt.tri[0]
                )
            }
            Prim::Body(e) => format!("body {e}"),
            Prim::Clip(cf) => format!("world clip cf={cf:#x}"),
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

/// A trace's fixed inputs, as `trace_node` and `clip_leaf` read them.
#[derive(Clone, Copy)]
struct TraceInput {
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    capsule: Capsule,
    mask: u32,
    statics: bool,
    skip: Option<usize>,
    /// Model 0's brushes and the terrain alone: a world copy's clip.
    world_only: bool,
    /// Brushes take the box rather than the capsule: `trap_Trace`'s box, which
    /// retail fakes as a capsule only against terrain (0x80536c8).
    boxed: bool,
    /// The capsule's sphere offset against brushes: `capsule.offset`, turned
    /// into a turned world copy's frame. Terrain and patches keep the
    /// upright offset, as retail's do.
    sphere: Vec3,
}

impl TraceInput {
    /// A prim nothing meets off its box, so [`Self::misses`] may cull it:
    /// a model triangle or a terrain partition. A model-0 patch's leaf-walk
    /// clip met mp_carentan's patch 9 56 units outside its box, and a
    /// brush notes a near tie (`note_contact`) for a segment 160 units off
    /// its box, which sends the trace through retail's leaf order: culling
    /// either changed which surface a trace named.
    fn tight(prim: Prim) -> bool {
        matches!(prim, Prim::Model(_) | Prim::Terrain(_))
    }

    /// The box the sweep covers up to `fraction`, padded a unit.
    fn bounds(&self, fraction: f32) -> (Vec3, Vec3) {
        let cur_end = self.start + (self.end - self.start) * fraction;
        (
            self.start.min(cur_end) + self.mins - Vec3::ONE,
            self.start.max(cur_end) + self.maxs + Vec3::ONE,
        )
    }

    /// [`Self::bounds`] past `fraction` by the tie window: a prim whose
    /// contact lands just past the hit cannot cut the trace, but clipped
    /// it notes the near tie that sends the sweep through retail's leaf
    /// order (`note_contact`), where it may be met first.
    fn reach(&self, fraction: f32) -> (Vec3, Vec3) {
        self.bounds((f64::from(fraction) + TIE_WINDOW).min(1.0) as f32)
    }

    /// Whether a long sweep's whole segment, `start` to `end`, passes
    /// clear of the box `lo..hi` grown by the swept shape and a margin, so
    /// nothing in it can touch the trace. The margin (a unit, a hundredth
    /// of the box's size for a model triangle's barycentric slack, the
    /// turned capsule's sphere offset) covers every clip's epsilon; the
    /// box test of [`Self::reach`] stays the one that cuts by fraction. A
    /// short sweep's box already hugs it and is never tested: this only
    /// spares a long ray (the 15000-unit look-at trace, a sight line) the
    /// prims its box takes in off its line. Only [`Self::tight`] prims
    /// are culled.
    fn misses(&self, lo: Vec3, hi: Vec3) -> bool {
        const LONG: f32 = 256.0;
        let d = self.end - self.start;
        if d.length_squared() < LONG * LONG {
            return false;
        }
        let pad = Vec3::splat(1.0 + 0.01 * (hi - lo).max_element() + self.sphere.length());
        let (lo, hi) = (lo - self.maxs - pad, hi - self.mins + pad);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for a in 0..3 {
            let s = self.start[a];
            if d[a] == 0.0 {
                if s < lo[a] || s > hi[a] {
                    return true;
                }
                continue;
            }
            let (ta, tb) = ((lo[a] - s) / d[a], (hi[a] - s) / d[a]);
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
            if t0 > t1 {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
thread_local! {
    /// Set by a test: the partition pass walks the tree again, as it did
    /// before the leaf log, to hold the replay to the same answer.
    static TWO_WALKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn two_walks() -> bool {
    #[cfg(test)]
    return TWO_WALKS.get();
    #[cfg(not(test))]
    false
}

/// The leaves a brush walk logs for its partition pass; a walk that enters
/// more walks the tree again instead.
const LEAF_LOG: usize = 64;

struct Walk {
    sweep: TraceInput,
    leaves: [u32; LEAF_LOG],
    /// Leaves entered so far (past `LEAF_LOG` when some went unlogged);
    /// `usize::MAX` stops the logging.
    logged: usize,
    /// Set when the sweep meets a patch of model 0, which `walk_tree`
    /// then clips in retail's leaf order instead.
    patch_met: std::cell::Cell<bool>,
    /// After that leaf walk: clips only what it does not, the submodels'
    /// brushes and the static models.
    rest: bool,
}

/// `base` is the absolute index of prims[0] within CollisionWorld::prims.
/// Splits by the surface area heuristic over binned centroids: a few large
/// brushes or terrain sheets otherwise widen a median split's boxes until a
/// small sweep enters most of the tree.
fn build_bvh(prims: &mut [(Prim, Vec3, Vec3)], base: u32, nodes: &mut Vec<BvhNode>) -> u32 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (_, plo, phi) in prims.iter() {
        lo = lo.min(*plo);
        hi = hi.max(*phi);
        let c = (*plo + *phi) * 0.5;
        clo = clo.min(c);
        chi = chi.max(c);
    }
    let idx = nodes.len() as u32;
    nodes.push(BvhNode {
        lo,
        hi,
        first: base,
        second: 0,
        count: prims.len() as u32,
        rest: true,
        loose: true,
    });
    if prims.len() <= 4 {
        return idx;
    }
    const BINS: usize = 16;
    let area = |lo: Vec3, hi: Vec3| {
        let e = (hi - lo).max(Vec3::ZERO);
        e.x * e.y + e.y * e.z + e.z * e.x
    };
    let span = chi - clo;
    let bin_of = |axis: usize, p: &(Prim, Vec3, Vec3)| {
        let c = (p.1[axis] + p.2[axis]) * 0.5;
        (((c - clo[axis]) / span[axis] * BINS as f32) as usize).min(BINS - 1)
    };
    // (cost, axis, first bin of the right half)
    let mut best: Option<(f32, usize, usize)> = None;
    for axis in 0..3 {
        if span[axis] <= 0.0 {
            continue;
        }
        let mut bins = [(0usize, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); BINS];
        for p in prims.iter() {
            let b = &mut bins[bin_of(axis, p)];
            b.0 += 1;
            b.1 = b.1.min(p.1);
            b.2 = b.2.max(p.2);
        }
        // Right-to-left sweep first, so the left one can price each split.
        let mut right = [0.0f32; BINS];
        let (mut n, mut rlo, mut rhi) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in (1..BINS).rev() {
            n += bins[i].0;
            rlo = rlo.min(bins[i].1);
            rhi = rhi.max(bins[i].2);
            right[i] = n as f32 * area(rlo, rhi);
        }
        let (mut n, mut llo, mut lhi) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in 1..BINS {
            n += bins[i - 1].0;
            llo = llo.min(bins[i - 1].1);
            lhi = lhi.max(bins[i - 1].2);
            if n == 0 || n == prims.len() {
                continue;
            }
            let cost = n as f32 * area(llo, lhi) + right[i];
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, axis, i));
            }
        }
    }
    let mid = match best {
        Some((_, axis, split)) => {
            // In-place partition: bins below `split` to the front.
            let mut mid = 0;
            for i in 0..prims.len() {
                if bin_of(axis, &prims[i]) < split {
                    prims.swap(i, mid);
                    mid += 1;
                }
            }
            mid
        }
        // Every centroid in one spot: any halving will do.
        None => prims.len() / 2,
    };
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
    let side = |v: f32| crate::mapworld::BrushSide {
        plane_or_dist: v.to_bits(),
        material: 0,
    };
    let floor = ([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]);
    let mut brush_sides = Vec::new();
    let mut brushes = Vec::new();
    let mut models = Vec::new();
    for (i, (lo, hi)) in std::iter::once(&floor).chain(submodels).enumerate() {
        brushes.push(crate::mapworld::Brush {
            first_side: (i * 6) as u32,
            num_sides: 6,
            material: 0,
        });
        for axis in 0..3 {
            brush_sides.push(side(lo[axis]));
            brush_sides.push(side(hi[axis]));
        }
        models.push(crate::mapworld::Model {
            mins: *lo,
            maxs: *hi,
            first_soup: 0,
            num_soups: 0,
            first_brush: i as u32,
            partitions: (0, 0),
            num_brushes: 1,
        });
    }
    CollisionWorld::build(
        &crate::mapworld::MapWorld {
            materials: vec![crate::mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: CONTENTS_SOLID,
            }],
            entities: entities.to_string(),
            brush_sides,
            brushes,
            models,
            ..Default::default()
        },
        &[],
    )
}

/// A test map's lump 24: each triangle `[a, b, c]` as a flat 3x3 patch of
/// `material` whose last row collapses onto `c`, its front the side
/// `cross(b - a, c - a)` points to.
#[doc(hidden)]
pub fn push_tri_patches(bsp: &mut crate::mapworld::MapWorld, tris: &[[Vec3; 3]], material: u16) {
    for &[a, b, c] in tris {
        let mid = |p: Vec3, q: Vec3| (p + q) * 0.5;
        let ab = mid(a, b);
        let rows = [[a, ab, b], [mid(a, c), mid(ab, c), mid(b, c)], [c, c, c]];
        let first_vert = bsp.collision_verts.len() as u32;
        bsp.collision_verts
            .extend(rows.iter().flatten().map(|v| v.to_array()));
        bsp.patches.push(crate::mapworld::PatchPart {
            material,
            width: 3,
            height: 3,
            tolerance: 4,
            first_vert,
        });
    }
}

/// A test map's lump 24: `tris` as one terrain partition of `material`,
/// each front the side `cross(b - a, c - a)` points to.
#[doc(hidden)]
pub fn push_tri_terrain(bsp: &mut crate::mapworld::MapWorld, tris: &[[Vec3; 3]], material: u16) {
    let first_vert = bsp.collision_verts.len() as u32;
    let first_index = bsp.collision_indices.len() as u32;
    for (i, tri) in tris.iter().enumerate() {
        bsp.collision_verts.extend(tri.iter().map(|v| v.to_array()));
        let i = (i * 3) as u16;
        // The build reads a lump-26 triangle `a, c, b`.
        bsp.collision_indices.extend([i, i + 2, i + 1]);
    }
    bsp.terrain.push(crate::mapworld::TerrainPart {
        material,
        first_vert,
        vert_count: (tris.len() * 3) as u16,
        first_index,
        index_count: (tris.len() * 3) as u16,
    });
}

/// [`synthetic_world`] plus world-space triangles as terrain of material 0
/// ([`push_tri_terrain`]), the only way this module takes swept geometry
/// that is not axis-aligned.
#[doc(hidden)]
pub fn synthetic_world_tris(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
    tris: &[[Vec3; 3]],
) -> CollisionWorld {
    let mut bsp = synthetic_bsp(materials, brushes);
    push_tri_terrain(&mut bsp, tris, 0);
    CollisionWorld::build(&bsp, &[])
}

/// [`synthetic_world`] plus world-space triangles as patches of material 0
/// ([`push_tri_patches`]).
#[doc(hidden)]
pub fn synthetic_world_patches(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
    tris: &[[Vec3; 3]],
) -> CollisionWorld {
    let mut bsp = synthetic_bsp(materials, brushes);
    push_tri_patches(&mut bsp, tris, 0);
    CollisionWorld::build(&bsp, &[])
}

/// A model-0 map of named materials and axial brushes.
fn synthetic_bsp(
    materials: &[(&str, u32, u32)],
    brushes: &[(usize, [f32; 3], [f32; 3])],
) -> crate::mapworld::MapWorld {
    let side = |m: u32, v: f32| crate::mapworld::BrushSide {
        plane_or_dist: v.to_bits(),
        material: m,
    };
    let mut brush_sides = Vec::new();
    let mut bsp_brushes = Vec::new();
    for (i, (mat, lo, hi)) in brushes.iter().enumerate() {
        bsp_brushes.push(crate::mapworld::Brush {
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
    crate::mapworld::MapWorld {
        materials: materials
            .iter()
            .map(|(name, content, surface)| crate::mapworld::Material {
                name: name.to_string(),
                surface_flags: *surface,
                content_flags: *content,
            })
            .collect(),
        brush_sides,
        brushes: bsp_brushes,
        models: vec![crate::mapworld::Model {
            mins,
            maxs,
            first_soup: 0,
            num_soups: 0,
            first_brush: 0,
            partitions: (0, 0),
            num_brushes: n,
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mp_depot's `*1` is an exploder and a `bombzone`, so `sd` keeps the
    /// gameobject and `_load.gsc` still takes it out; `*2` carries
    /// `script_exploder` with no targetname and stays solid.
    #[test]
    fn mp_depots_exploder_leaves_the_clip_under_sd() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_depot.bsp").unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        world.unlink_script_brushes(&bsp.entities, "sd");
        assert!(!world.model_linked(1));
        assert!(world.model_linked(2));
    }

    /// A world copy clips the world's own brushes where its entity stands,
    /// names that entity, and leaves the submodels out: the retail probe
    /// `probe_spawnclip` (movers doc 19) put a ground trace 64 units up
    /// under a script_brushmodel spawned at (0, 0, 64).
    #[test]
    fn a_world_copy_clips_where_its_entity_stands() {
        let world = submodel_test_world("", &[([400.0, -8.0, 0.0], [416.0, 8.0, 32.0])]);
        let down = |x: f32| {
            let t = world.shot_trace(Vec3::new(x, 0.0, 200.0), Vec3::new(x, 0.0, -100.0));
            (t.endpos.z.round(), world.entity_num(&t))
        };
        assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
        world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, 64.0), Vec3::ZERO, 1)));
        assert_eq!(down(0.0), (64.0, 299));
        assert_eq!(down(408.0), (64.0, 299), "the floor copy, not the submodel");
        assert_ne!(world.point_contents(Vec3::new(0.0, 0.0, 60.0)), 0);
        // `notSolid`: contents 0 meets no mask.
        world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, 64.0), Vec3::ZERO, 0)));
        assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
        world.set_world_copy(299, Some((Vec3::new(0.0, 0.0, -8.0), Vec3::ZERO, 1)));
        assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD), "a copy below loses");
        world.set_world_copy(299, None);
        assert_eq!(down(0.0), (0.0, ENTITYNUM_WORLD));
    }

    /// `CM_TransformedBoxTrace` turns the capsule's sphere offset into a
    /// turned copy's frame, so the capsule stays upright in the world.
    #[test]
    fn a_turned_copy_keeps_its_capsule_upright() {
        let copy = |angles: [f32; 3]| WorldCopy {
            entity: 299,
            origin: Vec3::ZERO,
            axis: Some(crate::pmove::aim::angles_to_axis(angles).map(Vec3::from)),
            contents: 1,
        };
        let up = Vec3::new(0.0, 0.0, 20.0);
        for angles in [[30.0, 0.0, 0.0], [5.0, 45.0, 0.0], [-60.0, 120.0, 0.0]] {
            let c = copy(angles);
            let world = c.to_world_dir(c.sphere(up));
            assert!((world - up).length() < 1e-4, "{angles:?}: {world:?}");
        }
        assert_eq!(copy([0.0, 90.0, 0.0]).sphere(up), up);
    }

    /// A capsule meets a brush's axial sides as its own box in the brush's
    /// frame, `|sphere| + r` a side: a player falling onto a slab pitched 90
    /// degrees rests on its top face, as retail's did (`probe_tiltslab`,
    /// movers doc 22), not 20 units into it.
    #[test]
    fn a_turned_capsule_meets_axial_sides_with_its_turned_box() {
        // The slab in its own frame, 4 thick along x; pitched 90 its +x face
        // is the world's top and the world's down is its +x.
        let planes = [
            (Vec3::NEG_X, 2.0),
            (Vec3::X, 2.0),
            (Vec3::NEG_Y, 28.0),
            (Vec3::Y, 28.0),
            (Vec3::NEG_Z, 48.0),
            (Vec3::Z, 48.0),
        ];
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, -35.0), Vec3::new(15.0, 15.0, 35.0));
        let (start, end) = (Vec3::new(100.0, 0.0, 0.0), Vec3::ZERO);
        let stop = |sphere: Vec3| {
            let mut trace = Trace {
                fraction: 1.0,
                endpos: end,
                normal: Vec3::ZERO,
                surface_flags: 0,
                startsolid: false,
                allsolid: false,
                hit: None,
                enter: -1.0,
                copy: None,
            };
            let sweep = capsule_sweep(start, end, mins, maxs);
            clip_brush_capsule(
                &mut trace,
                &sweep,
                sphere,
                &planes,
                &[],
                Prim::Brush(0),
                Arith::Kept,
                false,
            );
            start.x + (end.x - start.x) * trace.fraction
        };
        // Upright: the radius off the face.
        assert!((stop(Vec3::new(0.0, 0.0, 20.0)) - 17.125).abs() < 1e-3);
        // Pitched 90: the half height off it.
        assert!((stop(Vec3::new(-20.0, 0.0, 0.0)) - 37.125).abs() < 1e-3);
    }

    /// The push's two questions about a world copy: whether a box is in
    /// the copy alone, and whether a spot is clear of everything with the
    /// copy in it; the push sweep passes through the copy it skips.
    #[test]
    fn a_pusher_copy_is_tested_on_its_own() {
        let world = submodel_test_world("", &[]);
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let at = Vec3::new(0.0, 0.0, 0.125);
        world.set_world_copy(171, Some((Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, 1)));
        let copy = Pusher::Copy(171);
        assert!(
            world
                .pusher_box_trace(copy, at, at, mins, maxs, MASK_PLAYERSOLID)
                .startsolid
        );
        assert!(
            !world
                .pusher_box_trace(Pusher::Copy(5), at, at, mins, maxs, MASK_PLAYERSOLID)
                .startsolid
        );
        assert!(world.box_position_test(at, mins, maxs, MASK_PLAYERSOLID));
        let up = at + Vec3::new(0.0, 0.0, 8.0);
        assert!(!world.box_position_test(up, mins, maxs, MASK_PLAYERSOLID));
        let t = world.box_trace_except(at, up, mins, maxs, MASK_PLAYERSOLID, copy);
        assert!(!t.startsolid && t.fraction == 1.0);
        world.set_world_copy(171, Some((Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, 0)));
        assert!(
            !world.box_position_test(at, mins, maxs, MASK_PLAYERSOLID),
            "notSolid"
        );
    }

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

    /// A window pane (contents 0x8000010, no SOLID bit) is a brush every
    /// mask meets, and the trace names its contents; a shot that starts
    /// inside it, where a glass leg's nudge puts the next one, leaves it,
    /// through a SOLID soup drawn on its far face.
    #[test]
    fn a_glass_brush_clips_and_reports_its_contents() {
        let glass = 0x8000010;
        let face = |x: f32| {
            let c = |y: f32, z: f32| Vec3::new(x, y, z);
            [
                [c(-64.0, 0.0), c(64.0, 0.0), c(64.0, 128.0)],
                [c(-64.0, 0.0), c(64.0, 128.0), c(-64.0, 128.0)],
            ]
        };
        let world = synthetic_world_tris(
            &[
                ("textures/test/solid", CONTENTS_SOLID, 0),
                ("textures/test/glass", glass, 0x900000),
            ],
            &[(1, [40.0, -64.0, 0.0], [42.0, 64.0, 128.0])],
            &face(42.0),
        );
        let (start, end) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(100.0, 0.0, 60.0));
        let t = world.shot_trace(start, end);
        assert!(
            (t.endpos.x - (40.0 - SURFACE_CLIP_EPSILON)).abs() < 1e-3,
            "{t:?}"
        );
        assert_eq!(world.hit_contents(&t), glass);
        assert_eq!(sound_material(t.surface_flags), 9);
        let mins = Vec3::new(-15.0, -15.0, 0.0);
        let maxs = Vec3::new(15.0, 15.0, 70.0);
        let moved = world.box_trace(start - Vec3::Z * 60.0, end - Vec3::Z * 60.0, mins, maxs);
        assert!(moved.fraction < 1.0, "a player stops at the pane");
        assert!(world.missile_trace(start, end).fraction < 1.0);

        let inside = world.shot_trace(Vec3::new(40.125, 0.0, 60.0), end);
        assert_eq!(inside.fraction, 1.0, "{inside:?}");
        assert_eq!(world.hit_contents(&inside), 0);
    }

    /// A 0x2080 kerb brush stops a shot, a blast probe, a missile and a
    /// falling item, and a player walks through it.
    #[test]
    fn a_kerb_brush_clips_every_mask_but_the_players() {
        let kerb = 0x2080;
        let world = synthetic_world(
            &[("textures/test/kerb", kerb, 0)],
            &[(0, [-64.0, -64.0, -16.0], [64.0, 64.0, 0.0])],
        );
        let (above, below) = (Vec3::new(0.0, 0.0, 32.0), Vec3::new(0.0, 0.0, -32.0));
        let shot = world.shot_trace(above, below);
        assert!(shot.fraction < 1.0, "{shot:?}");
        assert_eq!(world.hit_contents(&shot), kerb);
        assert!(world.point_trace(above, below, MASK_BLAST, true).fraction < 1.0);
        let item = world.item_trace(above, below, -Vec3::ONE, Vec3::ONE, 0x81);
        assert!(item.fraction < 1.0, "{item:?}");
        assert_eq!(world.point_contents(Vec3::new(0.0, 0.0, -8.0)), kerb);

        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        assert!(world.missile_trace(above, below).fraction < 1.0);
        assert_eq!(world.box_trace(above, below, mins, maxs).fraction, 1.0);
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
    use crate::mapworld::{self, MapWorld};
    use glam::Vec3;

    /// One axial brush (-64,-64,-16)..(64,64,0), one upward triangle patch
    /// at z=10 over x,y in 100..200.
    pub(crate) fn tiny_world() -> MapWorld {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| mapworld::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        let mut bsp = MapWorld {
            materials: vec![mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            brush_sides: vec![
                side(-64.0),
                side(64.0), // xmin, xmax
                side(-64.0),
                side(64.0), // ymin, ymax
                side(-16.0),
                side(0.0), // zmin, zmax
            ],
            brushes: vec![mapworld::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            }],
            models: vec![mapworld::Model {
                mins: [-64.0, -64.0, -16.0],
                maxs: [64.0, 64.0, 0.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 1,
            }],
            ..Default::default()
        };
        push_tri_patches(
            &mut bsp,
            &[[
                Vec3::new(100.0, 100.0, 10.0),
                Vec3::new(200.0, 100.0, 10.0),
                Vec3::new(100.0, 200.0, 10.0),
            ]],
            0,
        );
        bsp
    }

    fn vert(pos: [f32; 3]) -> mapworld::DrawVert {
        mapworld::DrawVert {
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
        assert!(
            planes
                .iter()
                .any(|(n, d)| n.abs_diff_eq(Vec3::Z, 1e-6) && *d == 0.0)
        );
        // xmin face: normal -X, dist 64 (inside test: -x <= 64  =>  x >= -64)
        assert!(
            planes
                .iter()
                .any(|(n, d)| n.abs_diff_eq(-Vec3::X, 1e-6) && *d == 64.0)
        );
    }

    #[test]
    fn builds_patches_and_answers_aabb_queries() {
        let world = CollisionWorld::build(&tiny_world(), &[]);
        assert_eq!(world.patches.len(), 1);
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(140.0, 140.0, 0.0),
            Vec3::new(150.0, 150.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Patch(_))));
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
        // terrain triangles and lump 24's patches
        assert!(world.tris.len() > 4_000, "{}", world.tris.len());
        assert_eq!(world.patches.len(), bsp.patches.len());
        // a query around a known spawn; only holds if candidates() walks from the root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-9100.0, 8950.0, -50.0),
            Vec3::new(-8800.0, 9250.0, 150.0),
            &mut out,
        );
        assert!(!out.is_empty());
    }

    /// Six triangle patches 1000 units apart along X: the median split
    /// yields a 3-node BVH, so root (0) and last-pushed node (2) differ and
    /// a walk that starts anywhere but the root misses the low-X leaf.
    fn spread_tris_world() -> MapWorld {
        const N: usize = 6;
        let tris: Vec<[Vec3; 3]> = (0..N)
            .map(|i| {
                let ox = i as f32 * 1000.0;
                [
                    Vec3::new(ox, 0.0, 10.0),
                    Vec3::new(ox + 10.0, 0.0, 10.0),
                    Vec3::new(ox, 10.0, 10.0),
                ]
            })
            .collect();
        let mut bsp = MapWorld {
            materials: vec![mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            models: vec![mapworld::Model {
                mins: [0.0, 0.0, 0.0],
                maxs: [0.0, 0.0, 0.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 0,
            }],
            ..Default::default()
        };
        push_tri_patches(&mut bsp, &tris, 0);
        bsp
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
        assert_eq!(world.patches.len(), 6);

        // low-X triangle: only reachable from the true root
        let mut out = Vec::new();
        world.candidates(
            Vec3::new(-5.0, -5.0, 0.0),
            Vec3::new(15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Patch(0))));

        // high-X triangle (index 5)
        out.clear();
        let ox = 5.0 * 1000.0;
        world.candidates(
            Vec3::new(ox - 5.0, -5.0, 0.0),
            Vec3::new(ox + 15.0, 15.0, 20.0),
            &mut out,
        );
        assert!(out.iter().any(|p| matches!(p, Prim::Patch(5))));

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
            copy: None,
        };
        let capsule = Capsule::of(mins, maxs);
        let sweep = capsule_sweep(start, end, mins, maxs);
        for brushes in [true, false] {
            for (prim, _, _) in &world.prims {
                if trace.fraction <= 0.0 || brushes != matches!(prim, Prim::Brush(_)) {
                    continue;
                }
                match *prim {
                    Prim::Brush(i) => {
                        let brush = &world.brushes[i as usize];
                        clip_brush_capsule(
                            &mut trace,
                            &sweep,
                            capsule.offset,
                            &brush.planes,
                            &brush.side_surface_flags,
                            *prim,
                            Arith::Kept,
                            false,
                        );
                    }
                    Prim::Terrain(i) => {
                        world.clip_terrain(
                            i,
                            start,
                            end,
                            mins,
                            maxs,
                            capsule,
                            u32::MAX,
                            &mut trace,
                        );
                    }
                    Prim::Tri(_) => unreachable!(),
                    Prim::Patch(i) => {
                        clip_patch(
                            &mut trace,
                            start,
                            end,
                            capsule,
                            &world.patches[i as usize],
                            Arith::Kept,
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
                    // Never stored in world.prims.
                    Prim::Body(_) | Prim::Clip(_) => unreachable!(),
                }
            }
        }
        trace.endpos = cm_endpos(start, end, mins, maxs, trace.fraction, Arith::Kept);
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
    fn the_leaf_replay_matches_a_second_walk_on_mp_pavlov() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_pavlov.bsp").unwrap()).unwrap();
        // With the props, so a shot walks the static models' subtree.
        let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
        assert_ne!(world.models_root, u32::MAX);
        let (lo, hi) = crate::mesh::map_bounds(&bsp);
        let mut s = 0x9E3779B97F4A7C15u64;
        let mut hits = 0;
        for i in 0..3000 {
            let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
            let start = Vec3::from(lo) + (Vec3::from(hi) - Vec3::from(lo)) * r(&mut s);
            let end =
                start + (r(&mut s) - Vec3::splat(0.5)) * if i % 2 == 0 { 100.0 } else { 2000.0 };
            let (mins, maxs) = if i % 3 == 0 {
                (Vec3::ZERO, Vec3::ZERO)
            } else {
                (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0))
            };
            let both = |f: &dyn Fn() -> Trace| {
                TWO_WALKS.set(false);
                let a = f();
                TWO_WALKS.set(true);
                let b = f();
                TWO_WALKS.set(false);
                (a, b)
            };
            for (a, b) in [
                both(&|| world.box_trace(start, end, mins, maxs)),
                both(&|| world.shot_trace(start, end)),
            ] {
                assert_eq!(
                    a.fraction.to_bits(),
                    b.fraction.to_bits(),
                    "{start} -> {end}"
                );
                assert_eq!(a.endpos, b.endpos);
                assert_eq!(a.normal, b.normal);
                assert_eq!(
                    (a.startsolid, a.allsolid, format!("{:?}", a.hit)),
                    (b.startsolid, b.allsolid, format!("{:?}", b.hit))
                );
                assert_eq!(a.surface_flags, b.surface_flags);
                hits += usize::from(a.fraction < 1.0);
            }
        }
        assert!(hits > 1000, "{hits}");
    }

    /// A trace reports the side it stopped on, not the brush's material:
    /// the floor brush at x 232..552, y 1096..1408 on mp_carentan is
    /// `rock@cobbl_patrn1a` (17) with a wood top side (21), and retail
    /// 1.1d's footsteps across it are `EV_FOOTSTEP_RUN_WOOD` (the
    /// probe_surfside capture, docs/research/bsp-ibsp59-format.md).
    #[test]
    fn a_trace_reports_the_side_it_stops_on() {
        let Some(data) =
            crate::testing::game_fs().and_then(|fs| fs.read("maps/mp/mp_carentan.bsp"))
        else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let t = world.point_trace(
            Vec3::new(450.0, 1252.0, 40.0),
            Vec3::new(450.0, 1252.0, 0.0),
            MASK_PLAYERSOLID,
            false,
        );
        let Some(Prim::Brush(b)) = t.hit else {
            panic!("{:?}", t.hit);
        };
        assert_eq!(
            world.brushes[b as usize].material,
            "textures/normandy/floors/rock@cobbl_patrn1a"
        );
        assert_eq!(sound_material(t.surface_flags), 21);
    }

    /// A capsule standing across two floor brushes that meet at one
    /// height enters both at the same fraction; retail reports the one its
    /// leaf walk reaches first. On mp_carentan at (817.4, 694.6) that is
    /// brush 3882's material 4, where the BVH met brush 3880's 21 first:
    /// retail 1.1d, 1.5b and UO all step there with `EV_FOOTSTEP_RUN` 4
    /// (the patchwalk pillar captures, bsp-ibsp59-format.md "The leaf
    /// walk").
    #[test]
    fn a_capsule_on_two_floor_brushes_reads_the_first_in_leaf_order() {
        let Some(data) =
            crate::testing::game_fs().and_then(|fs| fs.read("maps/mp/mp_carentan.bsp"))
        else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let at = Vec3::new(817.44934, 694.5726, 8.125);
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        for arith in [Arith::Kept, Arith::Stored] {
            world.set_arith(arith);
            let t = world.move_trace(
                at + Vec3::Z * 0.25,
                at - Vec3::Z * 0.25,
                mins,
                maxs,
                MASK_PLAYERSOLID,
            );
            assert_eq!(t.fraction, 0.5, "{arith:?}");
            assert_eq!(sound_material(t.surface_flags), 4, "{arith:?} {:?}", t.hit);
        }
        world.set_arith(Arith::Kept);
    }

    /// Model 0 in retail's leaf order alone, then the rest as `walk_tree`
    /// clips it after its leaf walk: retail's answer to a trace.
    fn leaf_order_trace(
        world: &CollisionWorld,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        boxed: bool,
    ) -> Trace {
        let capsule = if boxed {
            Capsule::wide(mins, maxs)
        } else {
            Capsule::of(mins, maxs)
        };
        let sweep = TraceInput {
            start,
            end,
            mins,
            maxs,
            capsule,
            mask: MASK_PLAYERSOLID,
            statics: false,
            skip: None,
            world_only: false,
            boxed,
            sphere: capsule.offset,
        };
        let mut trace = Trace::clear(end);
        world.clip_leaf_walk(world.leaf_walk.as_ref().unwrap(), &sweep, &mut trace);
        let mut rest = Walk {
            sweep,
            leaves: [0; LEAF_LOG],
            logged: usize::MAX,
            patch_met: std::cell::Cell::new(false),
            rest: true,
        };
        world.trace_node(0, &mut rest, true, &mut trace);
        world.trace_node(0, &mut rest, false, &mut trace);
        trace
    }

    /// Ground traces, short runs and step-downs of a standing player, as a
    /// capsule and as `trap_Trace`'s box, from random spots on the floors of
    /// two stock maps: the tree walk reports what retail's leaf order does
    /// wherever two contacts nearly tie (two brushes a float step apart, a
    /// brush or a partition within the terrain's back-off of another) and,
    /// wherever it stops at fraction 0 or starts all in solid, and wherever
    /// it reaches terrain no leaf on its walk lists. Before the near-tie
    /// replay mp_depot alone gave about one answer in eighty from the
    /// BVH's order (bsp-ibsp59-format.md, "The leaf walk").
    #[test]
    fn near_ties_read_as_retails_leaf_order() {
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        for map in ["mp_depot", "mp_brecourt"] {
            let Some(data) =
                crate::testing::game_fs_11().and_then(|fs| fs.read(&format!("maps/mp/{map}.bsp")))
            else {
                return;
            };
            let bsp = crate::bsp::parse(&data).unwrap();
            let world = CollisionWorld::build(&bsp, &[]);
            let (lo, hi) = crate::mesh::map_bounds(&bsp);
            let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
            let mut s = 0x2545F4914F6CDD1Du64;
            let (mut traces, mut wrong) = (0, Vec::new());
            for k in 0..6000 {
                let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
                let p = lo + (hi - lo) * r(&mut s);
                let down = world.move_trace(p, p - Vec3::Z * 2000.0, mins, maxs, MASK_PLAYERSOLID);
                if down.fraction >= 1.0 || down.startsolid {
                    continue;
                }
                let o = down.endpos;
                let (a, b) = match k % 3 {
                    0 => (o + Vec3::Z * 0.25, o - Vec3::Z * 0.25),
                    1 => {
                        let run = (r(&mut s) - Vec3::splat(0.5)).with_z(0.0) * 40.0;
                        (o + Vec3::Z * 0.125, o + run + Vec3::Z * 0.125)
                    }
                    _ => (o + Vec3::Z * 18.0, o - Vec3::Z * 9.0),
                };
                let boxed = k % 2 == 1;
                let ours =
                    world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, boxed);
                let retail = leaf_order_trace(&world, a, b, mins, maxs, boxed);
                traces += 1;
                let key = |t: &Trace| {
                    (
                        t.fraction.to_bits(),
                        t.normal.to_array().map(f32::to_bits),
                        t.surface_flags,
                        t.startsolid,
                        t.allsolid,
                    )
                };
                if key(&ours) != key(&retail) {
                    wrong.push(format!(
                        "{map} {a} -> {b} boxed {boxed}: {ours:?} vs {retail:?}"
                    ));
                }
            }
            assert!(traces > 2000, "{map}: {traces}");
            assert!(
                wrong.is_empty(),
                "{} of {traces}:\n{}",
                wrong.len(),
                wrong.join("\n")
            );
        }
    }

    /// `near_ties_read_as_retails_leaf_order`'s sampler over every stock
    /// 1.1 map (`LEAF_MAPS` narrows it, `LEAF_SAMPLES` spots a map): each
    /// difference from the leaf walk, as `in` (the walk enters an open
    /// leaf) or `out` (only solid ones). Run in release.
    /// A capsule the BVH stops at fraction 0 or starts all in solid ends
    /// on whatever it clipped first, so it reads the contact retail's leaf
    /// order meets first: on mp_depot a run into the wall where brushes 182
    /// and 193 both stop it at 0 (leaf order: 193) and a step-down that
    /// starts in brush 5285 after the floor's contact, whose normal it keeps;
    /// on mp_railyard a run into two terrain triangles at 0. The
    /// `leaf_order_census` sampler found them (bsp-ibsp59-format.md, "The
    /// leaf walk").
    #[test]
    fn a_capsule_stopped_at_0_reads_the_first_in_leaf_order() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        for (map, a, b) in [
            (
                "mp_depot",
                [-2682.3232, -1800.0657, -31.750011],
                [-2674.6855, -1785.2223, -31.750011],
            ),
            (
                "mp_depot",
                [978.2998, 1853.0566, 42.067444],
                [978.2998, 1853.0566, 15.067444],
            ),
            (
                "mp_railyard",
                [2404.5742, 4883.2344, -95.75002],
                [2411.0657, 4897.0728, -95.75002],
            ),
        ] {
            let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
            let world = CollisionWorld::build(&bsp, &[]);
            let (a, b) = (Vec3::from(a), Vec3::from(b));
            let ours = world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, false);
            let retail = leaf_order_trace(&world, a, b, mins, maxs, false);
            assert_eq!(ours.fraction, 0.0, "{map} {a}");
            assert_eq!(
                (ours.hit, ours.normal, ours.surface_flags, ours.allsolid),
                (
                    retail.hit,
                    retail.normal,
                    retail.surface_flags,
                    retail.allsolid
                ),
                "{map} {a}"
            );
        }
    }

    #[test]
    #[ignore]
    fn leaf_order_census() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let maps = std::env::var("LEAF_MAPS").unwrap_or_else(|_| {
            "mp_brecourt mp_carentan mp_chateau mp_dawnville mp_depot mp_harbor mp_hurtgen \
             mp_pavlov mp_powcamp mp_railyard mp_rocket mp_ship"
                .into()
        });
        let samples: usize = std::env::var("LEAF_SAMPLES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30000);
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let (mut total, mut zeros, mut wrong) = (0usize, 0usize, Vec::new());
        for map in maps.split_whitespace() {
            let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
            let world = CollisionWorld::build(&bsp, &[]);
            let walk = world.leaf_walk.as_ref().unwrap();
            let (lo, hi) = crate::mesh::map_bounds(&bsp);
            let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
            let mut s = 0x2545F4914F6CDD1Du64;
            for k in 0..samples {
                let r = |s: &mut u64| Vec3::new(rng(s), rng(s), rng(s));
                let p = lo + (hi - lo) * r(&mut s);
                let down = world.move_trace(p, p - Vec3::Z * 2000.0, mins, maxs, MASK_PLAYERSOLID);
                if down.fraction >= 1.0 || down.startsolid {
                    continue;
                }
                let o = down.endpos;
                let (a, b) = match k % 3 {
                    0 => (o + Vec3::Z * 0.25, o - Vec3::Z * 0.25),
                    1 => {
                        let run = (r(&mut s) - Vec3::splat(0.5)).with_z(0.0) * 40.0;
                        (o + Vec3::Z * 0.125, o + run + Vec3::Z * 0.125)
                    }
                    _ => (o + Vec3::Z * 18.0, o - Vec3::Z * 9.0),
                };
                let mid = (mins + maxs) * 0.5;
                let mut open = false;
                walk.visit(
                    (a + mid).to_array(),
                    (b + mid).to_array(),
                    (maxs - mid).to_array(),
                    false,
                    1.0,
                    |leaf, _| {
                        open |= bsp.leafs[leaf as usize].cluster >= 0;
                        1.0
                    },
                );
                for boxed in [false, true] {
                    let ours =
                        world.trace_shape(a, b, mins, maxs, MASK_PLAYERSOLID, false, None, boxed);
                    let retail = leaf_order_trace(&world, a, b, mins, maxs, boxed);
                    total += 1;
                    zeros += usize::from(!boxed && ours.fraction <= 0.0);
                    let key = |t: &Trace| {
                        (
                            t.fraction.to_bits(),
                            t.normal.to_array().map(f32::to_bits),
                            t.surface_flags,
                            t.startsolid,
                            t.allsolid,
                        )
                    };
                    if key(&ours) != key(&retail) {
                        wrong.push(format!(
                            "{} {map} {a} -> {b} boxed {boxed}: {:?} {:?} {} vs {:?} {:?} {}",
                            if open { "in" } else { "out" },
                            ours.hit,
                            ours.normal,
                            ours.fraction,
                            retail.hit,
                            retail.normal,
                            retail.fraction
                        ));
                    }
                }
            }
        }
        println!(
            "{total} traces, {zeros} capsules stopped at 0, {} differ",
            wrong.len()
        );
        for w in &wrong {
            println!("  {w}");
        }
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
    fn two_model_world(classname: &str) -> MapWorld {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| mapworld::BrushSide {
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
        MapWorld {
            materials: vec![mapworld::Material {
                name: "textures/test/solid".into(),
                surface_flags: 0,
                content_flags: 0x1,
            }],
            soups: vec![mapworld::TriangleSoup {
                material: 0,
                lightmap: mapworld::NO_LIGHTMAP,
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
            brush_sides,
            brushes: vec![
                mapworld::Brush {
                    first_side: 0,
                    num_sides: 6,
                    material: 0,
                },
                mapworld::Brush {
                    first_side: 6,
                    num_sides: 6,
                    material: 0,
                },
            ],
            models: vec![
                mapworld::Model {
                    mins: [-1024.0; 3],
                    maxs: [1024.0; 3],
                    first_soup: 1,
                    num_soups: 0,
                    first_brush: 0,
                    partitions: (0, 0),
                    num_brushes: 1,
                },
                mapworld::Model {
                    mins: [-8.0, -8.0, 0.0],
                    maxs: [8.0, 8.0, 64.0],
                    first_soup: 0,
                    num_soups: 1,
                    first_brush: 1,
                    partitions: (0, 0),
                    num_brushes: 1,
                },
            ],
            ..Default::default()
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

    /// A pose moves a submodel's brushes with its entity: the door that
    /// stood at x 192 stops the ray at 292 once its entity sits at 300, a
    /// 45-degree yaw stands it on a corner, and the spawn pose puts it back.
    #[test]
    fn a_posed_submodel_clips_where_its_entity_is() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        let ray = |w: &CollisionWorld| {
            w.box_trace(
                Vec3::new(150.0, 0.0, 32.0),
                Vec3::new(350.0, 0.0, 32.0),
                Vec3::ZERO,
                Vec3::ZERO,
            )
            .endpos
            .x
        };
        world.set_model_pose(1, Vec3::new(300.0, 0.0, 0.0), Vec3::ZERO);
        assert!((ray(&world) - 292.0).abs() < 0.5, "{}", ray(&world));
        assert_ne!(world.point_contents(Vec3::new(300.0, 0.0, 32.0)), 0);
        assert_eq!(world.point_contents(Vec3::new(200.0, 0.0, 32.0)), 0);

        world.set_model_pose(1, Vec3::new(200.0, 0.0, 0.0), Vec3::new(0.0, 45.0, 0.0));
        let corner = 200.0 - 8.0 * std::f32::consts::SQRT_2;
        assert!((ray(&world) - corner).abs() < 0.5, "{}", ray(&world));

        world.set_model_pose(1, Vec3::new(200.0, 0.0, 0.0), Vec3::ZERO);
        assert!((ray(&world) - 192.0).abs() < 0.5, "{}", ray(&world));
    }

    /// Two entities' instances of one submodel each clip where they stand
    /// and name their own entity; a trace passing one still meets the
    /// other, and an instance whose contents miss the mask is not clipped.
    #[test]
    fn model_instances_clip_where_their_entities_stand() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        world.set_model_linked(1, false);
        let ray = |w: &CollisionWorld, skip: Option<Pusher>| {
            let t = w.trace_with_mask(
                Vec3::new(150.0, 0.0, 32.0),
                Vec3::new(450.0, 0.0, 32.0),
                Vec3::ZERO,
                Vec3::ZERO,
                TRACE_MASK_MOVE,
                false,
                skip,
            );
            (t.endpos.x, w.entity_num(&t))
        };
        assert_eq!(ray(&world, None).1, ENTITYNUM_NONE);
        world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 1)));
        world.set_model_instance(301, 1, Some((Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO, 1)));
        let (x, e) = ray(&world, None);
        assert!((x - 242.0).abs() < 0.5 && e == 300, "{x} {e}");
        let (x, e) = ray(&world, Some(Pusher::Copy(300)));
        assert!((x - 392.0).abs() < 0.5 && e == 301, "{x} {e}");
        world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 0x4)));
        assert_eq!(ray(&world, None).1, 301, "contents off the mask");
        world.set_model_instance(301, 1, None);
        world.set_model_instance(300, 1, None);
        assert_eq!(ray(&world, None).1, ENTITYNUM_NONE);
    }

    /// A turned instance's brushes clip in lump order and a contact at
    /// fraction 0 ends the model, as the leaf trace's loop does (UO
    /// 0x8059875): a later brush holding the whole sweep never reads it all
    /// solid. The jeep's collmap under a corpse its hull drove into
    /// (coduo-vehicles.md 11.7).
    #[test]
    fn a_contact_at_zero_ends_a_turned_instance_s_brushes() {
        let mut bsp = two_model_world("script_brushmodel");
        for (lo, hi) in [(-6.0f32, 6.0), (-6.0, 6.0), (60.0, 90.0)] {
            for v in [lo, hi] {
                bsp.brush_sides.push(mapworld::BrushSide {
                    plane_or_dist: v.to_bits(),
                    material: 0,
                });
            }
        }
        bsp.brushes.push(mapworld::Brush {
            first_side: 12,
            num_sides: 6,
            material: 0,
        });
        bsp.models[1].num_brushes = 2;
        let world = CollisionWorld::build(&bsp, &[]);
        world.set_model_linked(1, false);
        let at = Vec3::new(500.0, 0.0, 0.0);
        world.set_model_instance(300, 1, Some((at, Vec3::new(0.0, 90.0, 0.0), 1)));
        // A capsule resting a twentieth over the first brush's top, inside
        // the second from start to end.
        let (mins, maxs) = (Vec3::new(-4.0, -4.0, 0.0), Vec3::new(4.0, 4.0, 16.0));
        let start = at + Vec3::new(0.0, 0.0, 64.05);
        let t = world.trace_with_mask(
            start,
            start - Vec3::Z * 0.5,
            mins,
            maxs,
            TRACE_MASK_MOVE,
            false,
            None,
        );
        assert!(!t.allsolid && !t.startsolid, "{t:?}");
        assert_eq!(t.fraction, 0.0);
        assert!((t.normal - Vec3::Z).length() < 1e-5, "{:?}", t.normal);
        assert_eq!(world.entity_num(&t), 300);
    }

    /// An unlinked instance clips nothing until it is put back where it
    /// stood.
    #[test]
    fn an_unlinked_model_instance_clips_again_once_relinked() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        world.set_model_linked(1, false);
        let ray = |w: &CollisionWorld| {
            let t = w.trace_with_mask(
                Vec3::new(150.0, 0.0, 32.0),
                Vec3::new(450.0, 0.0, 32.0),
                Vec3::ZERO,
                Vec3::ZERO,
                TRACE_MASK_MOVE,
                false,
                None,
            );
            w.entity_num(&t)
        };
        world.set_model_instance(300, 1, Some((Vec3::new(250.0, 0.0, 0.0), Vec3::ZERO, 1)));
        assert!(world.unlink_model_instance(301).is_none());
        let unlinked = world.unlink_model_instance(300).unwrap();
        assert_eq!(ray(&world), ENTITYNUM_NONE);
        world.relink_model_instance(unlinked);
        assert_eq!(ray(&world), 300);
    }

    /// `model_box_trace` sees one model: the floor under the door is not in
    /// it, the door is.
    #[test]
    fn a_model_trace_sees_only_that_model() {
        let world = CollisionWorld::build(&two_model_world("script_brushmodel"), &[]);
        let floor = Vec3::new(0.0, 0.0, -8.0);
        assert!(
            !world
                .model_box_trace(1, floor, floor, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID)
                .startsolid
        );
        let door = Vec3::new(200.0, 0.0, 32.0);
        assert!(
            world
                .model_box_trace(1, door, door, Vec3::ZERO, Vec3::ZERO, MASK_PLAYERSOLID)
                .startsolid
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

    fn water_world() -> MapWorld {
        let dist = |v: f32| v.to_bits();
        let side = |m: u32, v: f32| mapworld::BrushSide {
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
            brushes.push(mapworld::Brush {
                first_side: (brushes.len() * 6) as u32,
                num_sides: 6,
                material: m as u16,
            });
        }
        MapWorld {
            materials: vec![
                mapworld::Material {
                    name: "textures/test/solid".into(),
                    surface_flags: 0,
                    content_flags: 0x1,
                },
                mapworld::Material {
                    name: "textures/common/water".into(),
                    surface_flags: 0,
                    content_flags: 0x20,
                },
            ],
            brush_sides,
            brushes,
            models: vec![mapworld::Model {
                mins: [-256.0; 3],
                maxs: [256.0; 3],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 2,
            }],
            ..Default::default()
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
    fn ladder_wall_world(surface_flags: u32) -> MapWorld {
        let dist = |v: f32| v.to_bits();
        let side = |v: f32| mapworld::BrushSide {
            plane_or_dist: dist(v),
            material: 0,
        };
        MapWorld {
            materials: vec![mapworld::Material {
                name: "textures/common/ladder".into(),
                surface_flags,
                content_flags: 0x10000,
            }],
            brush_sides: vec![
                side(50.0),
                side(54.0),
                side(-200.0),
                side(200.0),
                side(0.0),
                side(100.0),
            ],
            brushes: vec![mapworld::Brush {
                first_side: 0,
                num_sides: 6,
                material: 0,
            }],
            models: vec![mapworld::Model {
                mins: [50.0, -200.0, 0.0],
                maxs: [54.0, 200.0, 100.0],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 1,
            }],
            ..Default::default()
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

    /// A brush trace reports the struck side's own material, not the
    /// brush's (`cod_lnxded` 0x8055565, bsp-ibsp59-format.md lump 3): a
    /// caulk brush with a plaster +y face reads plaster off that face and
    /// nothing off its caulk top, for a point and a capsule.
    #[test]
    fn a_brush_hit_reports_the_struck_sides_material() {
        let mut bsp = synthetic_bsp(
            &[
                ("textures/common/caulk", CONTENTS_SOLID, 0x400a0),
                ("textures/test/plaster", CONTENTS_SOLID, 16 << 20),
            ],
            &[(0, [0.0, 0.0, 0.0], [64.0, 64.0, 64.0])],
        );
        bsp.brush_sides[3].material = 1;
        let world = CollisionWorld::build(&bsp, &[]);
        let face = |t: Trace| (t.fraction < 1.0).then(|| super::sound_material(t.surface_flags));
        let side = |mins: Vec3, maxs: Vec3| {
            world.box_trace(
                Vec3::new(32.0, 120.0, 32.0),
                Vec3::new(32.0, 32.0, 32.0),
                mins,
                maxs,
            )
        };
        assert_eq!(face(side(Vec3::ZERO, Vec3::ZERO)), Some(16));
        assert_eq!(face(side(Vec3::splat(-4.0), Vec3::splat(4.0))), Some(16));
        let capsule = world.box_trace(
            Vec3::new(32.0, 140.0, 0.0),
            Vec3::new(32.0, 32.0, 0.0),
            Vec3::new(-15.0, -15.0, 0.0),
            Vec3::new(15.0, 15.0, 70.0),
        );
        assert_eq!(face(capsule), Some(16));
        let top = world.box_trace(
            Vec3::new(32.0, 32.0, 120.0),
            Vec3::new(32.0, 32.0, 32.0),
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(face(top), Some(0));
    }

    /// The BVH walk reports what a scan of every brush does, on and off
    /// brushes: each prim's centre and corners, where the walk's node bounds
    /// are tightest.
    #[test]
    fn point_contents_matches_a_scan_of_every_brush() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_carentan.bsp").unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let scan = |p: Vec3| {
            let water = world.water.iter().any(|v| {
                p.cmple(v.hi).all()
                    && p.cmpge(v.lo).all()
                    && v.planes.iter().all(|&(n, d)| n.dot(p) <= d)
            });
            world
                .prims
                .iter()
                .filter_map(|(prim, _, _)| match prim {
                    Prim::Brush(b) => Some(&world.brushes[*b as usize]),
                    _ => None,
                })
                .filter(|b| world.brush_linked(b) && b.planes.iter().all(|&(n, d)| n.dot(p) <= d))
                .fold(if water { CONTENTS_WATER } else { 0 }, |c, b| {
                    c | b.content_flags
                })
        };
        let mut hits = 0;
        for &(_, lo, hi) in world.prims.iter().step_by(7) {
            for p in [(lo + hi) * 0.5, lo, hi, hi + Vec3::ONE] {
                let want = scan(p);
                hits += usize::from(want != 0);
                assert_eq!(world.point_contents(p), want, "at {p}");
            }
        }
        assert!(hits > 100, "only {hits} samples inside a brush");
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

        let axial_bounds = |b: &crate::mapworld::Brush| -> ([f32; 3], [f32; 3]) {
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
        let ladders: Vec<&crate::mapworld::Brush> = bsp
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

    /// Axial-free test helper: one triangle patch per entry against named
    /// `(name, contents, surface)` materials. Model 0 only, no brushes.
    fn synthetic_patch_world(
        materials: &[(&str, u32, u32)],
        tris: &[(usize, [[f32; 3]; 3])],
    ) -> CollisionWorld {
        let mut bsp = MapWorld {
            materials: materials
                .iter()
                .map(|(name, content, surface)| crate::mapworld::Material {
                    name: name.to_string(),
                    surface_flags: *surface,
                    content_flags: *content,
                })
                .collect(),
            models: vec![crate::mapworld::Model {
                mins: [0.0; 3],
                maxs: [0.0; 3],
                first_soup: 0,
                num_soups: 0,
                first_brush: 0,
                partitions: (0, 0),
                num_brushes: 0,
            }],
            ..Default::default()
        };
        for (mat, tri) in tris {
            push_tri_patches(&mut bsp, &[tri.map(Vec3::from_array)], *mat as u16);
        }
        CollisionWorld::build(&bsp, &[])
    }

    /// Patch content words from the stock maps' lump 24: foliage-like
    /// TRANSLUCENT|WINDOW (a terrain word; no patch carries it, so it stands
    /// for any word outside every mask) and a masked fence's
    /// TRANSLUCENT|PLAYERCLIP|MONSTERCLIP.
    const BUSH_WALL: (&str, u32, u32) = (
        "textures/global_use/foliage_masked@bushwall1",
        0x2000_0002,
        8_454_176,
    );
    const IRON_FENCE: (&str, u32, u32) = (
        "textures/austria/transparents/metal_masked@ironfence1a",
        0x2003_0000,
        13_713_440,
    );

    /// A wall facing -x, toward a trace from the origin.
    fn wall_tri(x: f32) -> [[f32; 3]; 3] {
        [[x, -50.0, 0.0], [x, -50.0, 100.0], [x, 50.0, 0.0]]
    }

    #[test]
    fn a_patch_outside_every_mask_clips_nothing() {
        let world = synthetic_patch_world(&[BUSH_WALL], &[(0, wall_tri(100.0))]);
        assert_eq!(world.patches.len(), 1);
        let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
        assert_eq!(
            world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO).fraction,
            1.0
        );
        assert_eq!(world.shot_trace(start, end).fraction, 1.0);
    }

    #[test]
    fn a_playerclip_fence_patch_stops_movement_but_not_shots() {
        let world = synthetic_patch_world(&[IRON_FENCE], &[(0, wall_tri(100.0))]);
        let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
        let box_mins = Vec3::new(-15.0, -15.0, 0.0);
        let box_maxs = Vec3::new(15.0, 15.0, 60.0);
        let movement = world.box_trace(start, end, box_mins, box_maxs);
        assert!(movement.fraction < 1.0, "players are stopped: {movement:?}");
        let shot = world.shot_trace(start, end);
        assert_eq!(shot.fraction, 1.0, "bullets pass playerclip-only geometry");
    }

    /// A patch collides by its curve, not its control points: a 64 x 64
    /// arch whose middle control row stands 64 up peaks at 32, and its box
    /// (what the leaf walk culls by) is the subdivided grid's grown by a
    /// unit, not the control points' (`cod_lnxded` 0x804e26d-0x804e314).
    #[test]
    fn a_curved_patch_collides_and_is_culled_by_its_curve() {
        let mut bsp = synthetic_bsp(&[("textures/test/solid", CONTENTS_SOLID, 0)], &[]);
        bsp.collision_verts = (0..3)
            .flat_map(|j| {
                (0..3).map(move |i| [i as f32 * 32.0, j as f32 * 32.0, (i % 2) as f32 * 64.0])
            })
            .collect();
        bsp.patches.push(crate::mapworld::PatchPart {
            material: 0,
            width: 3,
            height: 3,
            tolerance: 4,
            first_vert: 0,
        });
        let world = CollisionWorld::build(&bsp, &[]);
        let box_ = &world.patches[0].collide;
        assert_eq!((box_.mins, box_.maxs), ([-1.0; 3], [65.0, 65.0, 33.0]));

        // A shot down the crown stops on it, one that ends 1.5 over it never
        // reaches the clip, and a player walks over it with his feet above.
        let shot = world.shot_trace(Vec3::new(32.0, 32.0, 60.0), Vec3::new(32.0, 32.0, 20.0));
        assert!((shot.endpos.z - 32.125).abs() < 0.01, "{shot:?}");
        let short = world.shot_trace(Vec3::new(32.0, 32.0, 60.0), Vec3::new(32.0, 32.0, 33.5));
        assert_eq!(short.fraction, 1.0);
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let over = world.box_trace(
            Vec3::new(-40.0, 32.0, 34.0),
            Vec3::new(104.0, 32.0, 34.0),
            mins,
            maxs,
        );
        assert_eq!(over.fraction, 1.0, "{over:?}");
        let into = world.box_trace(
            Vec3::new(-40.0, 32.0, 20.0),
            Vec3::new(104.0, 32.0, 20.0),
            mins,
            maxs,
        );
        assert!(into.fraction < 1.0, "{into:?}");
    }

    #[test]
    fn a_solid_patch_stops_movement_and_shots_from_its_front() {
        let world = synthetic_patch_world(
            &[("textures/test/solid", 0x1, 5 << 20)],
            &[(0, wall_tri(100.0))],
        );
        let (start, end) = (Vec3::new(0.0, 0.0, 50.0), Vec3::new(200.0, 0.0, 50.0));
        let t = world.box_trace(start, end, Vec3::ZERO, Vec3::ZERO);
        assert!(t.fraction < 1.0, "movement stops");
        assert_eq!(world.hit_contents(&t), 0x1);
        assert_eq!(sound_material(t.surface_flags), 5);
        assert!(world.shot_trace(start, end).fraction < 1.0, "shots stop");
        assert_eq!(
            world.shot_trace(end, start).fraction,
            1.0,
            "the back is open"
        );
    }

    /// The 1.5b turret round `turret_ab` replays (`mp_carentan-dm-turret-15.txt`
    /// at 30000) and a ray beside it, under retail's own `bulletTrace`
    /// (`client-probes/probe_patchshot`, 2026-10-16): 1.1d stops the round
    /// on patch 136, where it meets patch 133, and lets the other through;
    /// 1.5b and UO 1.51b let the round through a crack along that facet
    /// seam to the wall behind and stop the other.
    #[test]
    fn the_15_turret_round_slips_a_seam_1_1_closes() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/mp/mp_carentan.bsp") else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let muzzle = Vec3::new(1706.721, 1822.8395, 28.679152);
        let end = Vec3::new(-3318.5461, -4644.5234, -140.95471);
        let up = Vec3::new(0.0, 0.0, 0.1);
        let shot = |s, e| world.shot_trace(s, e).fraction;
        assert!((shot(muzzle, end) - 0.0633128).abs() < 1e-6);
        assert!((shot(muzzle + up, end + up) - 0.111953).abs() < 1e-6);
        world.set_arith(Arith::Stored);
        assert!((shot(muzzle, end) - 0.111953).abs() < 1e-6);
        assert!((shot(muzzle + up, end + up) - 0.0633132).abs() < 1e-6);
    }

    /// Retail's own `bulletTrace` along 790 segments through mp_carentan's
    /// patches (`tests/fixtures/patch/mp_carentan-seams.txt`): 1.1d's
    /// answers off the 1.1 build, 1.5b's off the stored one. The fraction
    /// and its distance from the line's base, as the gsc's floats work them
    /// out and print them to six digits, tell two fractions a bit apart.
    #[test]
    fn patch_seams_clip_as_each_retail_build() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/mp/mp_carentan.bsp") else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        // The probe ran as a dm gametype, and a round meets the props too.
        let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
        world.unlink_script_brushes(&bsp.entities, "dm");
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/patch/mp_carentan-seams.txt"
        ))
        .unwrap();
        // Six significant digits, as `%g` prints them.
        let close = |ours: f32, printed: f32| {
            (ours - printed).abs() <= 6e-6 * printed.abs().max(ours.abs()) + 1e-6
        };
        let mut bad = Vec::new();
        let mut lines = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let v: Vec<f32> = line.split(' ').map(|w| w.parse().unwrap()).collect();
            let (start, end) = (Vec3::new(v[1], v[2], v[3]), Vec3::new(v[4], v[5], v[6]));
            for (arith, f, d) in [(Arith::Kept, v[8], v[9]), (Arith::Stored, v[10], v[11])] {
                world.set_arith(arith);
                let ours = world.shot_trace(start, end).fraction;
                let delta = (ours - v[7]) * 1_000_000.0;
                if !close(ours, f) || !close(delta, d) {
                    bad.push(format!(
                        "{} {arith:?}: ours {ours} ({delta}), retail {f} ({d})",
                        v[0]
                    ));
                }
            }
            lines += 1;
        }
        assert_eq!(lines, 790);
        assert!(
            bad.is_empty(),
            "{} of 1580 off retail:\n{}",
            bad.len(),
            bad.join("\n")
        );
    }

    /// Retail's own `bulletTrace` and a falling weapon item straight down
    /// over patches the leaves under them do not list, and over patches a
    /// submodel owns (`tests/fixtures/patch/leafgate.txt`): a capsule meets
    /// model 0's patches only from the leaves that list them, as a point
    /// does, and a submodel's only while its entity is linked.
    #[test]
    fn lump_24_clips_from_its_leaves_or_as_its_submodel() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        assert_eq!(check_leaf_fixture(&fs, "leafgate.txt"), 23);
    }

    /// Retail UO's `bulletTrace` and a falling weapon item straight down
    /// over terrain partitions no leaf lists
    /// (`tests/fixtures/patch/terrainleaf-uo.txt`): neither clips them, as
    /// neither clips an unlisted patch. No stock 1.1 map has such a
    /// partition with a triangle (`unlisted_lump_24_on_stock_1_1_maps`).
    #[test]
    fn unlisted_terrain_never_clips() {
        let Some(fs) = crate::testing::uo_fs() else {
            return;
        };
        assert_eq!(check_leaf_fixture(&fs, "terrainleaf-uo.txt"), 12);
    }

    /// A terrain partition the leaves list clips only a trace whose walk
    /// enters one of them: retail 1.1d's bulletTrace and falling item from
    /// over mp_powcamp's terrain past its playable walls, where the walk
    /// enters only solid leaves, pass it, where the BVH stopped both on it
    /// (`tests/fixtures/patch/outterrain.txt`, bsp-ibsp59-format.md "The
    /// leaf walk").
    #[test]
    fn terrain_out_of_the_world_never_clips() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        assert_eq!(check_leaf_fixture(&fs, "outterrain.txt"), 12);
    }

    /// A patch facet stops a trace only from in front of its surface:
    /// retail UO's bulletTrace through mp_streets' arch corner 391 (x 292,
    /// facing +x) from behind passes it and stops on its mirror 393 (x 284,
    /// facing -x), as ours does (`tests/fixtures/patch/patchside-uo.txt`,
    /// bsp-ibsp59-format.md "The leaf walk").
    #[test]
    fn a_patch_stops_traces_from_its_front_only() {
        let Some(fs) = crate::testing::uo_fs() else {
            return;
        };
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/patch/patchside-uo.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let bsp = crate::bsp::parse(&fs.read("maps/mp/mp_streets.bsp").unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &crate::props::collision_tris(&fs, &bsp.entities));
        world.set_arith(Arith::Stored);
        let mut checked = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let w: Vec<&str> = line.split(' ').collect();
            let v = |i: usize| w[i].parse::<f32>().unwrap();
            let start = Vec3::new(v(1), v(2), v(3));
            let end = start + Vec3::X * if w[4] == "front" { -40.0 } else { 40.0 };
            let t = world.shot_trace(start, end);
            assert!(
                t.fraction == v(5) && (t.endpos.x - v(6)).abs() <= 0.01 && t.normal.x == v(7),
                "{line}: ours {} at {} normal {}",
                t.fraction,
                t.endpos.x,
                t.normal.x
            );
            checked += 1;
        }
        assert_eq!(checked, 6);
    }

    /// 1.5's mp_neuville: its patches 68 and 69, which no leaf lists, stand
    /// in open air, and retail 1.5b's bulletTrace passes them
    /// (`tests/fixtures/patch/neuvilleleaf-15.txt`).
    #[test]
    fn neuvilles_unlisted_patches_never_clip() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        if fs.read("maps/mp/mp_neuville.bsp").is_none() {
            return;
        }
        assert_eq!(check_leaf_fixture(&fs, "neuvilleleaf-15.txt"), 6);
    }

    /// 1.5's mp_neuville: `placeSpawnpoint`'s ceiling check over entity 90
    /// meets caulk brush 2647's pushed-out underside past its roof slope's
    /// unbevelled edge, 40 units off the brush, where no leaf lists it.
    /// Retail 1.5b's drop starts the full 128 up
    /// (`tests/fixtures/spawnpoints/placebits-retail.txt`).
    #[test]
    fn a_brush_no_leaf_on_the_walk_lists_never_clips() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(data) = fs.read("maps/mp/mp_neuville.bsp") else {
            return;
        };
        let world = CollisionWorld::build(&crate::bsp::parse(&data).unwrap(), &[]);
        let start = Vec3::new(-14688.0, 5072.0, 8.0);
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let up = world.box_trace(start, start + Vec3::Z * 128.0, mins, maxs);
        assert_eq!((up.fraction, up.hit), (1.0, None));
    }

    /// Every model-0 lump-24 partition no leaf lists on the stock 1.1
    /// maps: mp_brecourt's terrain record 385 (five points, no triangle)
    /// and mp_carentan's four patches.
    #[test]
    fn unlisted_lump_24_on_stock_1_1_maps() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let mut found = Vec::new();
        for map in [
            "mp_brecourt",
            "mp_carentan",
            "mp_chateau",
            "mp_dawnville",
            "mp_depot",
            "mp_harbor",
            "mp_hurtgen",
            "mp_pavlov",
            "mp_powcamp",
            "mp_railyard",
            "mp_rocket",
            "mp_ship",
        ] {
            let bsp = crate::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
            let listed = LeafWalk::listed(&bsp).unwrap();
            let (terrain, patches) = partition_owners(&bsp);
            for (i, t) in bsp.terrain.iter().enumerate() {
                if terrain[i] == 0 && !listed.terrain[i] {
                    found.push(format!("{map} terrain {i} indices {}", t.index_count));
                }
            }
            for (i, (&owner, &on)) in patches.iter().zip(&listed.patches).enumerate() {
                if owner == 0 && !on {
                    found.push(format!("{map} patch {i}"));
                }
            }
        }
        assert_eq!(
            found,
            [
                "mp_brecourt terrain 385 indices 0",
                "mp_carentan patch 69",
                "mp_carentan patch 109",
                "mp_carentan patch 283",
                "mp_carentan patch 284",
            ]
        );
    }

    /// Replays a `tests/fixtures/patch/` file of retail point and item
    /// traces straight down on ours; the number of lines checked.
    fn check_leaf_fixture(fs: &crate::pk3::Pk3Fs, file: &str) -> usize {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/patch/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut worlds = std::collections::HashMap::new();
        let mut checked = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let w: Vec<&str> = line.split(' ').collect();
            let data = fs.read(&format!("maps/mp/{}.bsp", w[0])).unwrap();
            let world = worlds.entry(w[0]).or_insert_with(|| {
                let bsp = crate::bsp::parse(&data).unwrap();
                let world =
                    CollisionWorld::build(&bsp, &crate::props::collision_tris(fs, &bsp.entities));
                world.unlink_script_brushes(&bsp.entities, "dm");
                // mp_ship.gsc deletes the docks outside `re`.
                for block in crate::bsp::entity_blocks(&bsp.entities) {
                    if block.get("targetname").is_some_and(|t| t == "dock")
                        && let Some(n) = block
                            .get("model")
                            .and_then(|m| m.strip_prefix('*'))
                            .and_then(|n| n.parse().ok())
                    {
                        world.set_model_linked(n, false);
                    }
                }
                world
            });
            let v = |i: usize| w[i].parse::<f32>().unwrap();
            let start = Vec3::new(v(1), v(2), v(3));
            let end = start - Vec3::Z * 400.0;
            if w[4] == "west" {
                let t = world.shot_trace(start, start - Vec3::X * 80.0);
                assert!(
                    (t.fraction - v(5)).abs() <= 1e-6 && (t.endpos.x - v(6)).abs() <= 0.01,
                    "{line}: ours {} at {}",
                    t.fraction,
                    t.endpos.x
                );
            } else if w[4] == "point" {
                let t = world.shot_trace(start, end);
                assert!(
                    (t.fraction - v(5)).abs() <= 1e-6 && (t.endpos.z - v(6)).abs() <= 0.01,
                    "{line}: ours {} at {}",
                    t.fraction,
                    t.endpos.z
                );
            } else {
                let t = world.item_trace(start, end, Vec3::splat(-1.0), Vec3::splat(1.0), 0x491);
                // A landing lifts the item by up to a unit; one that falls
                // through keeps falling past the sweep's end.
                let z = v(5);
                let ok = if z < end.z {
                    t.fraction == 1.0
                } else {
                    (t.endpos.z..=t.endpos.z + 1.0).contains(&z)
                };
                assert!(ok, "{line}: ours {} at {}", t.fraction, t.endpos.z);
            }
            checked += 1;
        }
        checked
    }

    /// A point sliding down a hull's side onto its deck: the side's
    /// backed-off fraction is under the deck's, but the deck's surface comes
    /// first and the side's bounds start past it, so the posed clip stops on
    /// the deck (coduo-vehicles.md 13, "A blast on a hull").
    #[test]
    fn a_later_surface_past_the_best_fraction_is_not_clipped() {
        let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| ModelTri {
            tri: [Vec3::from(a), Vec3::from(b), Vec3::from(c)],
            contents: 1,
            surface_flags: 0,
        };
        // The deck faces +z at z 0, the side +y at y 0 below it; each
        // surface's bounds a unit deep behind its face.
        let tris = [
            tri([-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]),
            tri([-10.0, 0.0, -10.0], [0.0, 0.0, 10.0], [10.0, 0.0, -10.0]),
        ];
        let surf = |i: usize, mins: [f32; 3], maxs: [f32; 3]| ModelSurf {
            tris: i..i + 1,
            rot: glam::Quat::IDENTITY,
            pos: Vec3::ZERO,
            mins: Vec3::from(mins),
            maxs: Vec3::from(maxs),
        };
        let surfs = [
            surf(0, [-10.0, -10.0, -1.0], [10.0, 10.0, 0.0]),
            surf(1, [-10.0, -1.0, -10.0], [10.0, 0.0, 10.0]),
        ];
        let (start, end) = (Vec3::new(0.0, 0.1, 1.0), Vec3::new(0.0, -0.4, -5.0));
        let whole = clip_model_tris(start, end, &tris, 1, 1.0).unwrap();
        assert_eq!(whole.1.y, 1.0, "every triangle at once: the side");
        let posed = clip_model_surfs(start, end, &tris, &surfs, 1, 1.0).unwrap();
        assert_eq!(posed.1.z, 1.0, "a surface at a time: the deck");
        assert!((posed.0 - (1.0 - 0.125) / 6.0).abs() < 1e-6, "{}", posed.0);
    }
}
