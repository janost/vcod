//! A brush geom (class 12, a `physGeoms` brush) against the clip map, IW's
//! colliders in iw3mp.exe 1.7: the world collider's class 12 arm
//! (0x5a8513), a world brush (0x59b370) and a partition's triangles
//! (0x59c300, 0x59c050, 0x59bd00); cod4-gsc.md 3.6.4.

use std::ops::Range;

use glam::Vec3;

use super::ContactGeom;
use super::poly::clip;
use super::world::{Pieces, normalize};
use crate::assets::model::PhysBrush;
use crate::collision::{BrushFaces, Piece};
use crate::ode::math::{M3, V3, add, cross, dot, mul0_331, mul1_331, neg, sub};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

type Plane = ([f32; 3], f32);

/// `phys_narrowObjMaxLength`'s default (0xcc54428, registered at 0x5a4408).
pub const NARROW_MAX_LENGTH: f32 = 4.0;
/// `sqrt(3)` as a float widened (the double at 0x70ba90).
const SQRT3: f64 = 1.732_050_776_481_628_4;
/// A ref winding's edge, and its edge plane's normal, shorter than this
/// squared is skipped (the double at 0x70b040, 0.1f).
const MIN_EDGE: f64 = 0.100_000_001_490_116_12;
/// An incident face of the world must face the reference this much (the
/// double at 0x70b490, -0.1f).
const FACING: f64 = -0.100_000_001_490_116_12;
/// The most winding corners 0x59b370's buffers take (0x100) and 0x59bd00's
/// (0x400).
const ROOM_BRUSH: usize = 0x100;
const ROOM_TRIS: usize = 0x400;

/// A brush geom as the world collider's context holds it after 0x5a8513:
/// the model's origin, the body's position less its rotation times the
/// geom's centre (+0x50) (context +0x38); the body's rotation (its columns
/// the context's axes at +0x44); half the walk box's side (`sqrt 3` times
/// the brush's largest bound); narrow when
/// `phys_narrowObjMaxLength` is over the brush's top (context +0xa4); and
/// the brush's faces in the model's frame and turned into the world's.
#[derive(Clone, Debug)]
pub struct BrushGeom {
    pub pos: V3,
    pub r: M3,
    pub half: f32,
    pub narrow: bool,
    local: BrushFaces,
    placed: BrushFaces,
}

impl BrushGeom {
    /// 0x5a8513..0x5a86a8 for `brush` on a body at `pos` turned by `r`
    /// sitting at `offset` in the model's frame: the position less
    /// `R offset` (0x431440, each stored), the largest of the brush's six
    /// bounds' magnitudes (0x401000, a later one only when strictly
    /// larger) times `sqrt 3`; then 0x59b370's planes turned by the body
    /// (0x59b120 and its inlined copy).
    pub fn new(brush: &PhysBrush, offset: V3, pos: V3, r: &M3, narrow_max: f32) -> BrushGeom {
        let r = *r;
        let pos = sub(pos, mul0_331(&r, offset));
        let mut m = 0.0f32;
        for &x in brush.maxs.iter().chain(&brush.mins) {
            if m < x.abs() {
                m = x.abs();
            }
        }
        let local = model_faces(brush);
        let planes = local
            .planes
            .iter()
            .map(|&(n, d)| {
                let n = mul0_331(&r, n);
                (n, (w(dot(pos, n)) + w(d)) as f32)
            })
            .collect();
        let placed = BrushFaces {
            planes,
            ..local.clone()
        };
        BrushGeom {
            pos,
            r,
            half: (w(m) * SQRT3) as f32,
            narrow: narrow_max > brush.maxs[2],
            local,
            placed,
        }
    }

    /// 0x5a8370: the walk's box, the half side grown by 1 round `pos`.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let e = w(self.half) + 1.0;
        (
            Vec3::from(self.pos.map(|x| (w(x) - e) as f32)),
            Vec3::from(self.pos.map(|x| (e + w(x)) as f32)),
        )
    }

    /// 0x59c050: a world point in the model's frame, `R^T (v - pos)`.
    fn to_local(&self, v: V3) -> V3 {
        mul1_331(&self.r, sub(v, self.pos))
    }
}

/// The brush's faces in the model's frame: 0x596ae0's six axial planes
/// (`(-1, 0, 0)` the float at 0x70ad40), then its sides' planes; each
/// face's neighbours the brush's.
fn model_faces(b: &PhysBrush) -> BrushFaces {
    let mut planes = BrushFaces::axial_planes(b.mins, b.maxs).to_vec();
    planes.extend(b.side_planes.iter().map(|p| ([p[0], p[1], p[2]], p[3])));
    let mut adjacent = b.adjacent.clone();
    adjacent.resize(planes.len(), Vec::new());
    BrushFaces {
        surface: vec![0; planes.len()],
        planes,
        adjacent,
    }
}

/// 0x599cb0 and 0x599d30: every face's winding (0x599560 on the given
/// planes, 0x5998e0 on the brush's, the same arithmetic) into one buffer
/// of `room` corners, each face's run of it.
fn windings(f: &BrushFaces, room: usize) -> (Vec<V3>, Vec<Range<usize>>) {
    let mut pts = Vec::new();
    let mut runs = Vec::with_capacity(f.planes.len());
    for k in 0..f.planes.len() {
        let start = pts.len();
        pts.extend(f.winding(k, room - start));
        runs.push(start..pts.len());
    }
    (pts, runs)
}

/// 0x59acb0: the least of the points' heights over the plane, each
/// `n . p` and `- d` stored, a later one taken when it less the least so
/// far, stored, is under 0; `FLT_MAX` (0x70af58) with none.
fn least_height(pts: &[V3], (n, d): Plane) -> f32 {
    let mut least = f32::MAX;
    for &p in pts {
        let s = (w(dot(p, n)) - w(d)) as f32;
        if ((w(s) - w(least)) as f32) < 0.0 {
            least = s;
        }
    }
    least
}

/// 0x5974b0: 1 when the first edge crossed with the first later edge
/// longer than 0.1 squared (the float at 0x6df494 on the cross's square)
/// points against `n` (under the float 0 at 0x6ffa44), 0 when it does not,
/// 2 when no edge pair is that long.
fn orientation(p: &[V3], n: V3) -> u8 {
    if p.len() <= 2 {
        return 2;
    }
    let e0 = sub(p[1], p[0]);
    for k in 1..p.len() - 1 {
        let c = cross(e0, sub(p[k + 1], p[k]));
        if dot(c, c) > 0.01 {
            return u8::from(dot(c, n) < 0.0);
        }
    }
    2
}

/// 0x5975e0: the plane through `prev` holding the edge to `cur` and `n`,
/// unnormalized: `n x e` when the winding turns against `n`, else `e x n`;
/// none when the edge or the normal is under 0.1 squared.
fn edge_plane(cur: V3, prev: V3, n: V3, flip: bool) -> Option<Plane> {
    let e = sub(cur, prev);
    if w(dot(e, e)) < MIN_EDGE {
        return None;
    }
    let x = |a: f32, b: f32, c: f32, d: f32| (w(a) * w(b) - w(c) * w(d)) as f32;
    let o = if flip {
        [
            x(n[1], e[2], n[2], e[1]),
            x(n[2], e[0], n[0], e[2]),
            x(n[0], e[1], n[1], e[0]),
        ]
    } else {
        [
            x(n[2], e[1], n[1], e[2]),
            x(n[0], e[2], n[2], e[0]),
            x(e[0], n[1], e[1], n[0]),
        ]
    };
    if w(dot(o, o)) < MIN_EDGE {
        return None;
    }
    Some((o, dot(prev, o)))
}

/// 0x597e70: the incident polygon cut by each edge plane of the reference
/// winding (0x5975e0, turned by 0x5974b0's sense), from its last corner
/// round; none when the reference's sense cannot be read or a cut leaves
/// fewer than three.
fn clip_to(inc: &[V3], r: &[V3], n: V3) -> Vec<V3> {
    let mut out = inc.to_vec();
    let o = orientation(r, n);
    if o == 2 || r.is_empty() {
        return Vec::new();
    }
    let mut prev = r.len() - 1;
    for i in 0..r.len() {
        if let Some(ep) = edge_plane(r[i], r[prev], n, o == 1)
            && clip(&mut out, ep) < 3
        {
            return Vec::new();
        }
        prev = i;
    }
    out
}

/// 0x5989f0: each point of the incident face clipped to the reference
/// (0x597e70) strictly behind the reference plane is a contact of that
/// depth (`d - n . p`), along `normal`, while `out` has room.
fn contacts(
    out: &mut Vec<ContactGeom>,
    max: usize,
    inc: &[V3],
    r: &[V3],
    normal: V3,
    (n, d): Plane,
    surface: u32,
) {
    for p in clip_to(inc, r, n) {
        let depth = (w(d) - w(dot(n, p))) as f32;
        if depth > 0.0 {
            if out.len() >= max {
                return;
            }
            out.push(ContactGeom {
                pos: p,
                normal,
                depth,
                surface,
            });
        }
    }
}

/// 0x5992a0: of the faces with a winding, the one whose normal is most
/// against `n` (from the float 2.0 at 0x700c0c, a later one when strictly
/// less).
fn most_against(n: V3, f: &BrushFaces, runs: &[Range<usize>]) -> Option<usize> {
    let (mut best, mut at) = (2.0f32, None);
    for (k, (p, run)) in f.planes.iter().zip(runs).enumerate() {
        let s = dot(n, p.0);
        if !run.is_empty() && best > s {
            (best, at) = (s, Some(k));
        }
    }
    at
}

/// 0x598d60: [`most_against`], only a face whose dot is under -0.1.
fn facing(n: V3, f: &BrushFaces, runs: &[Range<usize>]) -> Option<usize> {
    let (mut best, mut at) = (2.0f32, None);
    for (k, (p, run)) in f.planes.iter().zip(runs).enumerate() {
        let s = dot(n, p.0);
        if !run.is_empty() && s < best && w(s) < FACING {
            (best, at) = (s, Some(k));
        }
    }
    at
}

/// The windings of one side of a brush pair.
struct Wound<'a> {
    faces: &'a BrushFaces,
    pts: Vec<V3>,
    runs: Vec<Range<usize>>,
}

impl Wound<'_> {
    fn face(&self, k: usize) -> &[V3] {
        &self.pts[self.runs[k].clone()]
    }
}

/// 0x59b010 and the world side's loop: no contact once a face's least
/// height over the other's points reads 0 or more, else the least deep
/// face with edges (a later one only when strictly less deep) and its
/// depth.
fn reference(f: &BrushFaces, other: &[V3]) -> Option<(Option<usize>, f32)> {
    let (mut best, mut at) = (-f32::MAX, None);
    for (k, &p) in f.planes.iter().enumerate() {
        let d = least_height(other, p);
        if d >= 0.0 {
            return None;
        }
        if !f.adjacent[k].is_empty() && best < d {
            (best, at) = (d, Some(k));
        }
    }
    Some((at, best))
}

/// 0x59b1a0: world face `k` (plane `plane`) as the reference: the geom's
/// face most against it (0x5992a0) clipped to its winding, each contact
/// along its normal negated with its material's surface flags.
fn world_ref(
    out: &mut Vec<ContactGeom>,
    max: usize,
    wd: &Wound,
    gd: &Wound,
    k: usize,
    plane: Plane,
) {
    let n = neg(plane.0);
    let Some(i) = most_against(plane.0, gd.faces, &gd.runs) else {
        return;
    };
    contacts(
        out,
        max,
        gd.face(i),
        wd.face(k),
        n,
        plane,
        wd.faces.surface[k],
    );
}

/// 0x59b240: geom face `k` as the reference: the world's face facing it
/// most (0x598d60) clipped to its winding, each contact along the geom
/// face's normal with the world face's surface flags; that world face.
fn geom_ref(
    out: &mut Vec<ContactGeom>,
    max: usize,
    wd: &Wound,
    gd: &Wound,
    k: usize,
) -> Option<usize> {
    let plane = gd.faces.planes[k];
    let i = facing(plane.0, wd.faces, &wd.runs)?;
    contacts(
        out,
        max,
        wd.face(i),
        gd.face(k),
        plane.0,
        plane,
        wd.faces.surface[i],
    );
    Some(i)
}

/// 0x59b2f0: something of the winding is left inside every plane.
fn survives(pts: &[V3], planes: &[Plane]) -> bool {
    let mut p = pts.to_vec();
    planes.iter().all(|&pl| clip(&mut p, pl) != 0)
}

/// 0x59b370: the geom's faces in the world (0x59b120) wound (0x599cb0);
/// no contact once a world face's least height over the geom's corners
/// (0x59acb0) or a geom face's over the world brush's (0x59b010) reads 0
/// or more. Of the two least deep faces with windings, the world's is the
/// reference when the geom's is deeper (or has none), its face most
/// against it the incident one (0x59b1a0); else the geom's, the world face
/// facing it most incident (0x59b240). A narrow geom then takes as
/// references the world brush's other faces its position lies past (an
/// axial one by the bounds, a side by its plane) whose windings the geom's
/// planes leave something of (0x59b2f0), axial pairs skipping the other
/// once one is taken.
pub fn brush(out: &mut Vec<ContactGeom>, max: usize, g: &BrushGeom, wf: &BrushFaces) {
    let (gpts, gruns) = windings(&g.placed, ROOM_BRUSH);
    if gpts.is_empty() {
        return;
    }
    let Some((wbest_at, wbest)) = reference(wf, &gpts) else {
        return;
    };
    let (wpts, wruns) = windings(wf, ROOM_BRUSH);
    if wpts.is_empty() {
        return;
    }
    let Some((gbest_at, gbest)) = reference(&g.placed, &wpts) else {
        return;
    };
    let wd = Wound {
        faces: wf,
        pts: wpts,
        runs: wruns,
    };
    let gd = Wound {
        faces: &g.placed,
        pts: gpts,
        runs: gruns,
    };
    let wr = wbest_at.filter(|&k| !wd.runs[k].is_empty());
    let gr = gbest_at.filter(|&k| !gd.runs[k].is_empty());
    let mut taken = wbest_at;
    match (wr, gr) {
        (Some(k), gr) if gbest < wbest || gr.is_none() => {
            world_ref(out, max, &wd, &gd, k, wf.planes[k]);
        }
        (_, Some(k)) => taken = geom_ref(out, max, &wd, &gd, k),
        _ => return,
    }
    if !g.narrow {
        return;
    }
    // 0x59b9cb: mins and maxs off the axial planes (0x596ae0's negation)
    let clipped = |k: usize| survives(wd.face(k), &g.placed.planes);
    for axis in 0..3 {
        let (lo, hi) = (2 * axis, 2 * axis + 1);
        let p = g.pos[axis];
        if !wd.runs[lo].is_empty() && -wf.planes[lo].1 > p {
            if taken != Some(lo) && clipped(lo) {
                world_ref(out, max, &wd, &gd, lo, wf.planes[lo]);
            }
            continue;
        }
        if !wd.runs[hi].is_empty() && wf.planes[hi].1 < p && taken != Some(hi) && clipped(hi) {
            world_ref(out, max, &wd, &gd, hi, wf.planes[hi]);
        }
    }
    for k in 6..wf.planes.len() {
        let (n, d) = wf.planes[k];
        if taken != Some(k) && !wd.runs[k].is_empty() && d < dot(g.pos, n) && clipped(k) {
            world_ref(out, max, &wd, &gd, k, wf.planes[k]);
        }
    }
}

/// 0x596bc0: a triangle's plane, `(t0 - t1) x (t2 - t0)` normalized
/// (0x4076d0), its distance at `t0`.
fn tri_plane(t: &[V3; 3]) -> Plane {
    let n = normalize(cross(sub(t[0], t[1]), sub(t[2], t[0])));
    (n, dot(t[0], n))
}

/// 0x59ae80: a brush face's least height over the triangle (0x59acb0), or
/// `-FLT_MAX` (0x70af5c) when the triangle is sound (both edges off `t0`
/// and their cross over 0.1 squared) and its normal `e2 x e1` leans with
/// the face's (over the float 0 at 0x6ffa44).
fn tri_face_depth(p: Plane, t: &[V3; 3]) -> f32 {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let c = cross(e2, e1);
    if dot(e1, e1) > 0.01 && dot(e2, e2) > 0.01 && dot(c, c) > 0.01 && dot(c, p.0) > 0.0 {
        return -f32::MAX;
    }
    least_height(t, p)
}

/// 0x59bd00, in the model's frame: no contact once a brush face's depth
/// against the triangle (0x59ae80) or the triangle's plane's least height
/// over the brush's corners (0x59acb0, its windings 0x599d30 in 0x400)
/// reads 0 or more. The least deep brush face with edges is the reference
/// when it is strictly less deep than the triangle and has a winding, the
/// triangle incident, each contact along the face's normal; else the
/// triangle is, the brush face facing it most incident (0x598d60), each
/// contact along the triangle's normal negated.
fn local_tri(out: &mut Vec<ContactGeom>, max: usize, f: &BrushFaces, t: &[V3; 3], surface: u32) {
    let (mut best, mut at) = (-f32::MAX, None);
    for (k, &p) in f.planes.iter().enumerate() {
        let d = tri_face_depth(p, t);
        if d >= 0.0 {
            return;
        }
        if !f.adjacent[k].is_empty() && best < d {
            (best, at) = (d, Some(k));
        }
    }
    let (pts, runs) = windings(f, ROOM_TRIS);
    if pts.is_empty() {
        return;
    }
    let tp = tri_plane(t);
    let dt = least_height(&pts, tp);
    if dt >= 0.0 {
        return;
    }
    if best > dt
        && let Some(k) = at
        && !runs[k].is_empty()
    {
        let plane = f.planes[k];
        contacts(out, max, t, &pts[runs[k].clone()], plane.0, plane, surface);
        return;
    }
    let Some(k) = facing(tp.0, f, &runs) else {
        return;
    };
    contacts(out, max, &pts[runs[k].clone()], t, neg(tp.0), tp, surface);
}

/// 0x59c300: each triangle of a partition while `out` has room, taken
/// into the model's frame (0x59c050), collided there (0x59bd00) and its
/// contacts' normals and points turned back. 0x596c70's cull of the
/// triangle against a sphere round the geom is not ported.
pub fn tris(
    out: &mut Vec<ContactGeom>,
    max: usize,
    g: &BrushGeom,
    tris: &[[Vec3; 3]],
    surface: u32,
) {
    for t in tris {
        if out.len() >= max {
            return;
        }
        let local = t.map(|v| g.to_local(v.to_array()));
        let start = out.len();
        local_tri(out, max, &g.local, &local, surface);
        for c in &mut out[start..] {
            c.normal = mul0_331(&g.r, c.normal);
            c.pos = add(g.pos, mul0_331(&g.r, c.pos));
        }
    }
}

/// `dCollide(o1 = the brush geom, o2 = the world)` (0x5a95a0) with room
/// for `max`: 0x5a8110 walks the world pieces round [`BrushGeom::bounds`]
/// under `mask`, a brush only while there is room (0x5a79b0), and every
/// normal is turned round (0x5a9651..), so each points out of the world.
pub fn collide_world(world: &dyn Pieces, g: &BrushGeom, mask: u32, max: usize) -> Vec<ContactGeom> {
    let (lo, hi) = g.bounds();
    let mut out = Vec::new();
    for piece in world.pieces(Vec3::from(g.pos), lo, hi, mask) {
        match piece {
            Piece::Brush(b) => {
                if out.len() < max {
                    brush(&mut out, max, g, &world.brush(b));
                }
            }
            Piece::Tris { tris: t, surface } => tris(&mut out, max, g, &t, surface),
        }
    }
    for c in &mut out {
        c.normal = neg(c.normal);
    }
    out
}
