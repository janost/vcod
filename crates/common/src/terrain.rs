//! Retail's terrain collision, transcribed from `cod_lnxded` 1.1d:
//! `CM_GenerateTerrainCollide` (0x8051b30) builds each lump-24 terrain
//! partition's triangle, edge and vertex records at load, and the capsule
//! clip (0x8052a58) sweeps one sphere of the capsule against them. Both run
//! on the x87, so every value the binary keeps on the stack is an `f64`
//! here and every value it stores is rounded to `f32` where it stores it
//! (docs/research/cod11-mantle.md, "Terrain is a swept sphere, a patch is a
//! facet", and "The terrain clip's arithmetic").

use crate::patch::Arith;

/// `CM_GenerateTerrainCollide`'s coplanarity factor (rodata 0x80cd2ec).
const COPLANAR_EPS: f32 = f32::from_bits(0x31db_e6ff);
/// A normal's z under this marks a downward face (0x80cd2f0), over its
/// negation an upward one (0x80cd2f4).
const FACING_EPS: f32 = f32::from_bits(0x3a83_126f);
/// The pad on the sphere's radius (0x80cd308).
const RADIUS_PAD: f32 = 0.125;
/// What every fraction loses, and the bound at or under which a fraction
/// is a `startsolid` (0x80cd30c).
pub(crate) const FRACTION_EPS: f32 = f32::from_bits(0x3727_c5ac);

const NONE: u32 = u32::MAX;

fn d(x: f32) -> f64 {
    f64::from(x)
}

fn r(x: f64) -> f32 {
    x as f32
}

/// `VectorNormalize` (0x8065a38): the length is stored as a float and each
/// component multiplied by its reciprocal. Returns the stored length.
fn normalize(v: &mut [f32; 3]) -> f32 {
    let len = r(((d(v[0]) * d(v[0]) + d(v[1]) * d(v[1])) + d(v[2]) * d(v[2])).sqrt());
    if len != 0.0 {
        let inv = 1.0 / d(len);
        for c in v.iter_mut() {
            *c = r(d(*c) * inv);
        }
    }
    len
}

/// `CrossProduct` (0x80659cc): one rounding per component.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        r(d(a[1]) * d(b[2]) - d(a[2]) * d(b[1])),
        r(d(a[2]) * d(b[0]) - d(a[0]) * d(b[2])),
        r(d(a[0]) * d(b[1]) - d(a[1]) * d(b[0])),
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f64 {
    let e = |i: usize| d(b[i]) - d(a[i]);
    (e(0) * e(0) + e(1) * e(1)) + e(2) * e(2)
}

/// `PerpendicularVector` (0x80661fc): the axis `dir` is shortest along,
/// projected onto the plane `dir` is the normal of (0x806717c), then
/// normalized.
fn perpendicular(dir: [f32; 3]) -> [f32; 3] {
    let mut k = 0;
    let mut min = 1.0f32;
    for (i, c) in dir.iter().enumerate() {
        if c.abs() < min {
            min = c.abs();
            k = i;
        }
    }
    let inv = 1.0 / ((d(dir[0]) * d(dir[0]) + d(dir[1]) * d(dir[1])) + d(dir[2]) * d(dir[2]));
    let dd = d(dir[k]) * inv;
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let p = if i == k { 1.0 } else { 0.0 };
        out[i] = r(p - dd * (d(dir[i]) * inv));
    }
    normalize(&mut out);
    out
}

/// `coduo_lnxded`'s `PerpendicularVector` (0x80676b4): the same axis pick,
/// projected by its `ProjectPointOnPlane` (0x806aa4e), which stores the
/// squared length, its reciprocal, the scaled dot and the scaled normal
/// before each component's subtraction, then normalized by
/// [`normalize_v22`].
fn perpendicular_v22(dir: [f32; 3]) -> [f32; 3] {
    let mut k = 0;
    let mut min = 1.0f32;
    for (i, c) in dir.iter().enumerate() {
        if c.abs() < min {
            min = c.abs();
            k = i;
        }
    }
    let inv = r(1.0 / d(r(dot3(dir, dir))));
    let dd = r(d(dir[k]) * d(inv));
    let mut out: [f32; 3] = std::array::from_fn(|i| {
        let p = if i == k { 1.0 } else { 0.0 };
        r(p - d(dd) * d(r(d(dir[i]) * d(inv))))
    });
    normalize_v22(&mut out);
    out
}

/// One triangle's record (0x48 bytes in the binary): its plane, and the
/// two barycentric planes `u` (along the first edge) and `v` (along the
/// second), each as a normal and a distance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainTri {
    normal: [f32; 3],
    dist: f32,
    u: [f32; 3],
    ud: f32,
    v: [f32; 3],
    vd: f32,
    /// Vertex records of corners 0, 1, 2 (`NONE`: no record).
    verts: [u32; 3],
    /// Edge records opposite corners 0, 1, 2 (`NONE`: no record).
    edges: [u32; 3],
    /// The plane did not normalize: the binary leaves the record half
    /// written, and no trace can hit it.
    degenerate: bool,
    /// The corners' bounds: not in the record, a reject vcod adds in front
    /// of the clip (a sphere that never comes within its radius of them
    /// can't touch the face, an edge or a corner).
    lo: [f32; 3],
    hi: [f32; 3],
    /// The same three planes as United Offensive's engine builds them
    /// ([`PointPlanes::v22`]), which its point clip reads.
    v22: PointPlanes,
}

/// A triangle's plane and its two barycentric planes, each a normal and a
/// distance: the half of the record a point trace reads.
#[derive(Clone, Copy, Debug, Default)]
struct PointPlanes {
    normal: [f32; 3],
    dist: f32,
    u: [f32; 3],
    ud: f32,
    v: [f32; 3],
    vd: f32,
}

/// `DotProduct` summed left to right on the stack.
fn dot3(a: [f32; 3], b: [f32; 3]) -> f64 {
    (d(a[0]) * d(b[0]) + d(a[1]) * d(b[1])) + d(a[2]) * d(b[2])
}

/// `coduo_lnxded`'s `VectorNormalize` (0x8066755): the squared length,
/// its root and the reciprocal each stored as a float, every component
/// rounded once. Returns the stored length.
fn normalize_v22(v: &mut [f32; 3]) -> f32 {
    let sq = r(dot3(*v, *v));
    let len = r(d(sq).sqrt());
    if len != 0.0 {
        let inv = r(1.0 / d(len));
        for c in v.iter_mut() {
            *c = r(d(*c) * d(inv));
        }
    }
    len
}

impl TerrainTri {
    /// The record's own planes, 1.1d's build.
    fn planes(&self) -> PointPlanes {
        PointPlanes {
            normal: self.normal,
            dist: self.dist,
            u: self.u,
            ud: self.ud,
            v: self.v,
            vd: self.vd,
        }
    }
}

impl PointPlanes {
    /// `coduo_lnxded`'s record build (0x805403e, from the
    /// `PlaneFromPoints` call at 0x80547b8): the code keeps nothing on the
    /// stack across a statement, so every assignment rounds to a float.
    fn v22(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> PointPlanes {
        // `PlaneFromPoints` (0x806a964): (p2 - p0) x (p1 - p0).
        let (e1, e2) = (sub(p1, p0), sub(p2, p0));
        let mut normal = cross(e2, e1);
        let mut dist = 0.0;
        if normalize_v22(&mut normal) != 0.0 {
            dist = r(dot3(p0, normal));
        }
        let (mut e1, mut e2) = (e1, e2);
        let len1 = normalize_v22(&mut e1);
        let len2 = normalize_v22(&mut e2);
        let dot = r(dot3(e1, e2));
        let k = r(1.0 / (1.0 - d(dot) * d(dot)));
        let e1 = e1.map(|c| r(d(c) * d(k)));
        let e2 = e2.map(|c| r(d(c) * d(k)));
        let side = |a: [f32; 3], b: [f32; 3], len: f32| {
            let inv = 1.0 / d(len);
            let n: [f32; 3] = std::array::from_fn(|i| r(inv * d(r(-d(dot) * d(b[i]) + d(a[i])))));
            (n, r(dot3(n, p0)))
        };
        let (u, ud) = side(e1, e2, len1);
        let (v, vd) = side(e2, e1, len2);
        PointPlanes {
            normal,
            dist,
            u,
            ud,
            v,
            vd,
        }
    }
}

/// The point clip's barycentric bounds (`cod_lnxded` 0x80cd2fc, 0x80cd300;
/// `coduo_lnxded` 0x80dc91c, 0x80dc920).
const BARY_LO: f32 = f32::from_bits(0xba83_126f);
const BARY_HI: f32 = f32::from_bits(0x3f80_20c5);
/// Its pad off the face (`cod_lnxded` 0x80cd2f8, `coduo_lnxded` 0x80dc918).
const POINT_CLIP_PAD: f32 = 0.125;

/// An edge's record (0x38 bytes): its first point, a frame whose `w` runs
/// along the edge, and its length.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainEdge {
    origin: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
    w: [f32; 3],
    len: f32,
}

/// A partition's triangles, in `tris` from `first` on, and its facing
/// flag: set when some face points down and none up, which sweeps the
/// capsule's upper sphere instead of its lower one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainPart {
    pub(crate) first: u32,
    pub(crate) count: u32,
    down: bool,
    /// The bounds of every point of the partition, unpadded
    /// (`ClearBounds` 0x8066ec0 and `AddPointToBounds` 0x8066eec in
    /// 0x8051b30), which the leaf walk tests a trace's bounds against.
    lo: [f32; 3],
    hi: [f32; 3],
}

#[derive(Clone, Default)]
pub(crate) struct Terrain {
    tris: Vec<TerrainTri>,
    verts: Vec<[f32; 3]>,
    edges: Vec<TerrainEdge>,
    /// The same edge records as United Offensive's engine builds them
    /// (0x805403e's edge loop: `VectorNormalize` 0x8066755,
    /// [`perpendicular_v22`]), which its capsule clip reads.
    edges_v22: Vec<TerrainEdge>,
    pub(crate) parts: Vec<TerrainPart>,
}

impl Terrain {
    /// Triangle `t`'s bounds.
    pub(crate) fn tri_bounds(&self, t: usize) -> ([f32; 3], [f32; 3]) {
        (self.tris[t].lo, self.tris[t].hi)
    }

    /// `CM_GenerateTerrainCollide` on one partition's points and
    /// triangles; the records go after every earlier partition's, so a
    /// triangle's index here is its index in the world's `tris`.
    pub(crate) fn add_partition(&mut self, points: &[[f32; 3]], tris: &[[u16; 3]]) -> u32 {
        // Every edge once, with the corner opposite it in the first
        // triangle that has it. A second triangle on an edge drops it when
        // the first's far corner is in front of its plane and its own
        // normal is under the coplanarity bound.
        let mut edge_list: Vec<([u16; 2], u16)> = Vec::new();
        for tri in tris {
            for k in 0..3 {
                let (a, b, c) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
                let p = |i: u16| points[i as usize];
                let found = edge_list
                    .iter()
                    .position(|&([x, y], _)| (x == b && y == a) || (x == a && y == b));
                let Some(j) = found else {
                    edge_list.push(([a, b], c));
                    continue;
                };
                let opp = edge_list[j].1;
                let d0 = sub(p(a), p(c));
                let d1 = sub(p(b), p(c));
                let d2 = sub(p(opp), p(c));
                let n = cross(d1, d0);
                let dot = (d(n[0]) * d(d2[0]) + d(n[1]) * d(d2[1])) + d(n[2]) * d(d2[2]);
                if 0.0 < dot {
                    let m1 = dist_sq(p(a), p(opp));
                    let m2 = dist_sq(p(b), p(opp));
                    let max = if d(r(m1)) > m2 { m1 } else { m2 };
                    let n_sq = (d(n[0]) * d(n[0]) + d(n[1]) * d(n[1])) + d(n[2]) * d(n[2]);
                    if max * d(COPLANAR_EPS) > n_sq {
                        edge_list.swap_remove(j);
                    }
                }
            }
        }

        // Vertex records for the corners the kept edges reach, in order.
        let mut vert_rec = vec![NONE; points.len()];
        for &([a, b], _) in &edge_list {
            for i in [a, b] {
                if vert_rec[i as usize] == NONE {
                    vert_rec[i as usize] = self.verts.len() as u32;
                    self.verts.push(points[i as usize]);
                }
            }
        }
        let edge_base = self.edges.len() as u32;
        let edge_of = |x: u16, y: u16| {
            edge_list
                .iter()
                .position(|&([a, b], _)| (a == x && b == y) || (a == y && b == x))
                .map_or(NONE, |j| edge_base + j as u32)
        };

        let first = self.tris.len() as u32;
        let (mut down, mut up) = (false, false);
        for tri in tris {
            let [i0, i1, i2] = *tri;
            let (p0, p1, p2) = (
                points[i0 as usize],
                points[i1 as usize],
                points[i2 as usize],
            );
            let mut rec = plane_record(p0, p1, p2);
            if rec.normal[2] < -FACING_EPS {
                down = true;
            } else if rec.normal[2] > FACING_EPS {
                up = true;
            }
            rec.verts = [
                vert_rec[i0 as usize],
                vert_rec[i1 as usize],
                vert_rec[i2 as usize],
            ];
            rec.edges = [edge_of(i2, i1), edge_of(i0, i2), edge_of(i1, i0)];
            rec.lo = std::array::from_fn(|i| p0[i].min(p1[i]).min(p2[i]));
            rec.hi = std::array::from_fn(|i| p0[i].max(p1[i]).max(p2[i]));
            self.tris.push(rec);
        }

        for &([a, b], _) in &edge_list {
            let origin = points[a as usize];
            let mut w = sub(points[b as usize], origin);
            let mut w22 = w;
            let len = normalize(&mut w);
            let u = perpendicular(w);
            let v = cross(w, u);
            self.edges.push(TerrainEdge {
                origin,
                u,
                v,
                w,
                len,
            });
            let len = normalize_v22(&mut w22);
            let u = perpendicular_v22(w22);
            self.edges_v22.push(TerrainEdge {
                origin,
                u,
                v: cross(w22, u),
                w: w22,
                len,
            });
        }

        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in points {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        self.parts.push(TerrainPart {
            first,
            count: tris.len() as u32,
            down: down && !up,
            lo,
            hi,
        });
        self.parts.len() as u32 - 1
    }
}

/// A triangle's plane (`PlaneFromPoints`, 0x8064fec: the normal is
/// `(p2 - p0) x (p1 - p0)`) and barycentric planes (0x80521a9-0x80523f4).
fn plane_record(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> TerrainTri {
    let e = |p: [f32; 3], i: usize| d(p[i]) - d(p0[i]);
    let (d1, d2) = (
        [e(p1, 0), e(p1, 1), e(p1, 2)],
        [e(p2, 0), e(p2, 1), e(p2, 2)],
    );
    let n = [
        d2[1] * d1[2] - d2[2] * d1[1],
        d2[2] * d1[0] - d2[0] * d1[2],
        d2[0] * d1[1] - d2[1] * d1[0],
    ];
    let len = r(((n[0] * n[0] + n[1] * n[1]) + n[2] * n[2]).sqrt());
    let mut normal = [r(n[0]), r(n[1]), r(n[2])];
    let mut dist = 0.0;
    let degenerate = len == 0.0;
    if !degenerate {
        let inv = 1.0 / d(len);
        normal = [r(n[0] * inv), r(n[1] * inv), r(n[2] * inv)];
        dist = r((d(p0[0]) * d(normal[0]) + d(p0[1]) * d(normal[1])) + d(p0[2]) * d(normal[2]));
    }

    let mut e1 = sub(p1, p0);
    let len1 = normalize(&mut e1);
    let mut e2 = sub(p2, p0);
    let len2 = normalize(&mut e2);
    let dot = (d(e1[0]) * d(e2[0]) + d(e1[1]) * d(e2[1])) + d(e1[2]) * d(e2[2]);
    let k = 1.0 / (1.0 - dot * dot);
    let nd = -dot;
    let e1k = [d(e1[0]) * k, d(e1[1]) * k, d(e1[2]) * k];
    let e2k = [d(e2[0]) * k, d(e2[1]) * k, d(e2[2]) * k];
    let (e1s, e2s) = (e1k.map(r), e2k.map(r));
    let mut u = [
        r(e1k[0] + e2k[0] * nd),
        r(d(e2s[1]) * nd + d(e1s[1])),
        r(d(e2s[2]) * nd + d(e1s[2])),
    ];
    let inv1 = 1.0 / d(len1);
    u[0] = r(d(u[0]) * inv1);
    u[1] = r(d(u[1]) * inv1);
    let uz = inv1 * d(u[2]);
    u[2] = r(uz);
    let ud = r((d(u[0]) * d(p0[0]) + d(u[1]) * d(p0[1])) + uz * d(p0[2]));
    let mut v = [
        r(d(e1s[0]) * nd + d(e2s[0])),
        r(d(e1s[1]) * nd + d(e2s[1])),
        r(nd * d(e1s[2]) + d(e2s[2])),
    ];
    let inv2 = 1.0 / d(len2);
    v[0] = r(d(v[0]) * inv2);
    v[1] = r(d(v[1]) * inv2);
    let vz = inv2 * d(v[2]);
    v[2] = r(vz);
    let vd = r((d(v[0]) * d(p0[0]) + d(v[1]) * d(p0[1])) + vz * d(p0[2]));
    let finite = u.iter().chain(&v).all(|c| c.is_finite()) && ud.is_finite() && vd.is_finite();
    TerrainTri {
        normal,
        dist,
        u,
        ud,
        v,
        vd,
        verts: [NONE; 3],
        edges: [NONE; 3],
        degenerate: degenerate || !finite,
        lo: [0.0; 3],
        hi: [0.0; 3],
        v22: PointPlanes::v22(p0, p1, p2),
    }
}

/// What a capsule trace carries into the clip, as `CM_BoxTrace`
/// (0x8056310) sets it up: the start and end shifted to the box centre and
/// stored as floats, the delta between them and its squared length, the
/// sphere radius and the half height.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CapsuleSweep {
    pub(crate) start: [f32; 3],
    pub(crate) end: [f32; 3],
    pub(crate) delta: [f32; 3],
    pub(crate) delta_sq: f32,
    pub(crate) radius: f32,
    pub(crate) half_height: f32,
}

/// What a partition clip leaves: a new fraction, the hit normal and the
/// triangle it came from, and whether it was a `startsolid` (which ends
/// the clip: the binary returns from the partition at once).
pub(crate) struct TerrainHit {
    pub(crate) fraction: f32,
    pub(crate) normal: [f32; 3],
    pub(crate) tri: u32,
    pub(crate) startsolid: bool,
}

impl TerrainPart {
    /// The leaf walk's bounds test ahead of every partition (0x80556e9
    /// calling 0x8054c48): the partition is skipped when its point bounds
    /// miss the trace's, strictly. A trace's bounds are `CM_BoxTrace`'s
    /// (0x80565c0-0x8056635): each axis from the nearer to the farther of
    /// the centre-shifted start and end, widened by the sphere offset's
    /// magnitude and the radius, without the clip's 0.125 pad. So a fall
    /// that ends above a flat partition's face is never clipped by it
    /// (docs/research/cod11-player-clip.md 8.12).
    pub(crate) fn touches(&self, sw: &CapsuleSweep) -> bool {
        let reach = r(d(sw.half_height) - d(sw.radius));
        capsule_bounds_meet(self.lo, self.hi, sw.start, sw.end, reach, sw.radius)
    }

    /// The same test for a point trace, whose bounds are its segment's
    /// (0x8056640-0x80566ee), and its point arm's three cross-axis tests.
    pub(crate) fn touches_segment(&self, start: [f32; 3], end: [f32; 3], arith: Arith) -> bool {
        segment_bounds_meet(self.lo, self.hi, start, end, arith)
    }
}

/// 0x8054c48 for a capsule: whether the box `lo..hi` meets the trace's
/// bounds, `CM_BoxTrace`'s centre-shifted `start` and `end` widened by
/// `reach`, the sphere offset, on z and by `radius` on every axis.
pub(crate) fn capsule_bounds_meet(
    lo: [f32; 3],
    hi: [f32; 3],
    start: [f32; 3],
    end: [f32; 3],
    reach: f32,
    radius: f32,
) -> bool {
    (0..3).all(|i| {
        let (a, b) = if end[i] > start[i] {
            (start[i], end[i])
        } else {
            (end[i], start[i])
        };
        let reach = if i == 2 { d(reach).abs() } else { 0.0 };
        let l = r(d(a) - reach - d(radius));
        let h = r(d(b) + reach + d(radius));
        hi[i] >= l && lo[i] <= h
    })
}

/// 0x8054c48 for a point trace (1.5b 0x8058d51, UO 0x8058b85): the box
/// against the segment's own, then the point arm (0x8054d0f-0x8054e66), the
/// separating-axis test of the segment against the box on the three axes
/// `delta x e_i`, all in doubled units: the centre offset is
/// `(end + start) - (hi + lo)`, the extents `hi - lo` and the segment box's
/// spans `|delta|`. An axis separates when its cross term squared is over
/// its extent sum squared, strictly. 1.1d keeps both sums on the stack (the
/// cross term's store at 0x8054dde is not read back); 1.5b and UO store
/// them (docs/research/cod11-player-clip.md 8.12).
pub(crate) fn segment_bounds_meet(
    lo: [f32; 3],
    hi: [f32; 3],
    start: [f32; 3],
    end: [f32; 3],
    arith: Arith,
) -> bool {
    if !(0..3).all(|i| hi[i] >= start[i].min(end[i]) && lo[i] <= start[i].max(end[i])) {
        return false;
    }
    let sum = |a: f32, b: f32| r(d(a) + d(b));
    let dif = |a: f32, b: f32| r(d(a) - d(b));
    let c: [f32; 3] = std::array::from_fn(|i| dif(sum(end[i], start[i]), sum(lo[i], hi[i])));
    let h: [f32; 3] = std::array::from_fn(|i| dif(hi[i], lo[i]));
    let (b0, b1) = (
        std::array::from_fn::<f32, 3, _>(|i| start[i].min(end[i])),
        std::array::from_fn::<f32, 3, _>(|i| start[i].max(end[i])),
    );
    let span: [f32; 3] = std::array::from_fn(|i| dif(b1[i], b0[i]));
    let delta: [f32; 3] = std::array::from_fn(|i| dif(end[i], start[i]));
    let stored = arith == Arith::Stored;
    let var = |x: f64| if stored { d(r(x)) } else { x };
    // (i, j, k): the axis `delta x e_i` takes components j and k.
    [(1, 2), (2, 0), (0, 1)].into_iter().all(|(j, k)| {
        let cross = var(d(delta[j]) * d(c[k]) - d(delta[k]) * d(c[j]));
        let ext = var(d(h[k]) * d(span[j]) + d(h[j]) * d(span[k]));
        cross * cross <= ext * ext
    })
}

impl Terrain {
    /// The capsule clip (0x8052a58) of one partition against a trace whose
    /// fraction is `fraction`. Returns the last hit it recorded, if any.
    /// `stored` is 1.5b's and UO's build (`cod_lnxded` 1.5b 0x8055aab), the
    /// same steps with every variable stored as a float
    /// (docs/research/cod15-movement.md 7).
    pub(crate) fn clip_capsule(
        &self,
        part: &TerrainPart,
        sw: &CapsuleSweep,
        fraction: f32,
        stored: bool,
    ) -> Option<TerrainHit> {
        self.clip_capsule_as(part, sw, fraction, true, stored)
    }

    /// [`Self::clip_capsule`], with or without the bounds reject in front
    /// of each triangle; the tests hold the two to the same answer.
    fn clip_capsule_as(
        &self,
        part: &TerrainPart,
        sw: &CapsuleSweep,
        mut fraction: f32,
        reject: bool,
        stored: bool,
    ) -> Option<TerrainHit> {
        // A variable 1.5b stores; 1.1d keeps it.
        let st = |x: f64| if stored { d(r(x)) } else { x };
        // The sphere nearer the partition's faces: the lower one, or the
        // upper for a partition facing down only. `axis` reaches the other.
        let hr = d(sw.half_height) - d(sw.radius);
        let (mut s, mut e) = (sw.start, sw.end);
        let axis = if part.down {
            s[2] = r(d(s[2]) + hr);
            e[2] = r(d(e[2]) + hr);
            r(hr * -2.0)
        } else {
            s[2] = r(d(s[2]) - hr);
            e[2] = r(d(e[2]) - hr);
            r(hr + hr)
        };
        let r_eps = r(d(sw.radius) + d(RADIUS_PAD));
        let (re, nre) = (d(r_eps), -d(r_eps));
        let dl = sw.delta.map(d);
        let mut hit: Option<TerrainHit> = None;
        let solid = |tri: u32, normal: [f32; 3]| TerrainHit {
            fraction: 0.0,
            normal,
            tri,
            startsolid: true,
        };

        // Every contact puts a sphere centre on `s..e` or on the axis up
        // from `s` within `r_eps` of the triangle; a unit more of pad
        // covers the rounding.
        let pad = r_eps + 1.0;
        let reach_lo: [f32; 3] = std::array::from_fn(|i| {
            s[i].min(e[i]).min(if i == 2 { s[2] + axis } else { s[i] }) - pad
        });
        let reach_hi: [f32; 3] = std::array::from_fn(|i| {
            s[i].max(e[i]).max(if i == 2 { s[2] + axis } else { s[i] }) + pad
        });
        for ti in part.first..part.first + part.count {
            let t = &self.tris[ti as usize];
            if t.degenerate
                || reject && (0..3).any(|i| t.hi[i] < reach_lo[i] || t.lo[i] > reach_hi[i])
            {
                continue;
            }
            // 1.5b and UO read the planes their own build made.
            let pl = if stored { t.v22 } else { t.planes() };
            let n = pl.normal.map(d);
            let d_e = st(((d(e[1]) * n[1] + d(e[0]) * n[0]) + d(e[2]) * n[2]) - d(pl.dist));
            if d_e >= re {
                continue;
            }
            let d_s = st(((d(s[1]) * n[1] + d(s[0]) * n[0]) + d(s[2]) * n[2]) - d(pl.dist));
            let dd = st(d_s - d_e);
            if dd <= 0.0 {
                continue;
            }
            let bary = |p: [f64; 3]| {
                let u = ((p[0] * d(pl.u[0]) + p[1] * d(pl.u[1])) + p[2] * d(pl.u[2])) - d(pl.ud);
                let v = ((p[0] * d(pl.v[0]) + p[1] * d(pl.v[1])) + p[2] * d(pl.v[2])) - d(pl.vd);
                (st(u), st(v))
            };
            let outside = |u: f64, v: f64| u < 0.0 || v < 0.0 || u + v > 1.0;

            if d_s <= nre {
                // Deep behind the face: solid where the capsule's axis
                // crosses the triangle, at the near pad or the far one.
                let far_d = st(d(axis) * n[2] + d_s);
                if far_d <= nre {
                    continue;
                }
                let tn = st((nre - d_s) / n[2]);
                let (u0, v0) = bary(s.map(d));
                let (un, vn) = (st(u0 + tn * d(pl.u[2])), st(v0 + tn * d(pl.v[2])));
                if outside(un, vn) {
                    let tf = if d(r(far_d)) < re {
                        d(axis)
                    } else {
                        st((re - d_s) / n[2])
                    };
                    let (uf, vf) = (st(u0 + tf * d(pl.u[2])), st(v0 + tf * d(pl.v[2])));
                    if outside(uf, vf) {
                        continue;
                    }
                }
                return Some(solid(ti, pl.normal));
            }

            let (f, p) = if d_s < re {
                (0.0, s)
            } else {
                let f = st((d_s - re) / dd);
                if f > d(fraction) {
                    if fraction < 1.0 {
                        crate::collision::note_contact(f, d(fraction));
                    }
                    continue;
                }
                (f, std::array::from_fn(|i| r(dl[i] * f + d(s[i]))))
            };
            let (u, v) = bary(p.map(d));
            let bits =
                u32::from(u + v > 1.0) | (u32::from(u < 0.0) << 1) | (u32::from(v < 0.0) << 2);
            if bits == 0 {
                if f <= d(FRACTION_EPS) {
                    return Some(solid(ti, pl.normal));
                }
                fraction = r(f - d(FRACTION_EPS));
                hit = Some(TerrainHit {
                    fraction,
                    normal: pl.normal,
                    tri: ti,
                    startsolid: false,
                });
                continue;
            }

            for i in 0..3 {
                let swept = if bits & (1 << i) == 0 {
                    self.sweep_vertex(t.verts[i], &s, &dl, sw.delta_sq, r_eps, fraction, stored)
                } else if stored {
                    self.sweep_edge_stored(t.edges[i], &s, &dl, r_eps, fraction)
                } else {
                    self.sweep_edge(t.edges[i], &s, &dl, r_eps, fraction)
                };
                match swept {
                    Sweep::Miss => {}
                    Sweep::Inside => return Some(solid(ti, pl.normal)),
                    Sweep::Hit(tt, normal) => {
                        // The binary tests the trace's fraction before this
                        // hit, not the hit's, against the epsilon.
                        if fraction <= FRACTION_EPS {
                            return Some(solid(ti, pl.normal));
                        }
                        fraction = r(d(tt) - d(FRACTION_EPS));
                        hit = Some(TerrainHit {
                            fraction,
                            normal,
                            tri: ti,
                            startsolid: false,
                        });
                    }
                }
            }
        }
        hit
    }

    /// The position test (0x8053280, reached through 0x80538d0 for a trace
    /// that does not move and is not a point): whether the capsule resting
    /// at `sw.start` is inside one of the partition's triangles. The sweep
    /// above reads no contact off a zero-length trace, so this is the only
    /// way terrain reads `startsolid`. Unlike the sweep, the radius is
    /// unpadded. 1.1d keeps every intermediate on the stack but the far
    /// end's distance and the axis; `stored` is 1.5b's and UO's build
    /// (1.5b 0x8056941, UO 0x8056773), which stores every variable and
    /// reads the planes and edge frames that build made (cod11-mantle.md,
    /// "A trace that does not move").
    pub(crate) fn position_test(
        &self,
        part: &TerrainPart,
        sw: &CapsuleSweep,
        stored: bool,
    ) -> bool {
        let st = |x: f64| if stored { d(r(x)) } else { x };
        let rad = d(sw.radius);
        let hr = st(d(sw.half_height) - rad);
        let mut s = sw.start;
        let axis = if part.down {
            s[2] = r(d(s[2]) + hr);
            r(hr * -2.0)
        } else {
            s[2] = r(d(s[2]) - hr);
            r(hr + hr)
        };
        let rad_sq = rad * rad;
        let dot = |a: [f64; 3], b: [f32; 3]| (a[0] * d(b[0]) + a[1] * d(b[1])) + a[2] * d(b[2]);
        let sd = s.map(d);
        let edges = if stored { &self.edges_v22 } else { &self.edges };
        for ti in part.first..part.first + part.count {
            let t = &self.tris[ti as usize];
            if t.degenerate {
                continue;
            }
            let pl = if stored { t.v22 } else { t.planes() };
            let n2 = d(pl.normal[2]);
            let ds = st(dot(sd, pl.normal) - d(pl.dist));
            if ds >= rad {
                continue;
            }
            if ds <= -rad {
                // Deep behind the face: inside where the axis crosses the
                // triangle within the radius of its plane. 1.1d tests the
                // far end unrounded against `-rad` and its stored float
                // against `rad`.
                let far = d(axis) * n2 + ds;
                if st(far) <= -rad {
                    continue;
                }
                let tn = st((-rad - ds) / n2);
                let u0 = st(dot(sd, pl.u) - d(pl.ud));
                let v0 = st(dot(sd, pl.v) - d(pl.vd));
                let (un, vn) = (st(tn * d(pl.u[2]) + u0), st(tn * d(pl.v[2]) + v0));
                if un >= 0.0 && vn >= 0.0 && un + vn <= 1.0 {
                    return true;
                }
                let tf = if d(r(far)) < rad {
                    d(axis)
                } else {
                    st((rad - ds) / n2)
                };
                let (uf, vf) = (st(tf * d(pl.u[2]) + u0), st(tf * d(pl.v[2]) + v0));
                // An unordered compare skips nothing here, as both builds
                // branch.
                if !(0.0 > uf || 0.0 > vf || uf + vf > 1.0) {
                    return true;
                }
                continue;
            }
            // Within the radius of the plane: the centre's foot on it.
            let p: [f64; 3] = std::array::from_fn(|i| st(-ds * d(pl.normal[i]) + sd[i]));
            let (u, v) = (st(dot(p, pl.u) - d(pl.ud)), st(dot(p, pl.v) - d(pl.vd)));
            let bits =
                u32::from(u + v > 1.0) | (u32::from(0.0 > u) << 1) | (u32::from(0.0 > v) << 2);
            if bits == 0 {
                return true;
            }
            for i in 0..3 {
                if bits & (1 << i) == 0 {
                    let Some(&q) = self.verts.get(t.verts[i] as usize) else {
                        continue;
                    };
                    let e: [f64; 3] = std::array::from_fn(|k| st(sd[k] - d(q[k])));
                    if (e[0] * e[0] + e[1] * e[1]) + e[2] * e[2] < rad_sq {
                        return true;
                    }
                } else {
                    let Some(ed) = edges.get(t.edges[i] as usize) else {
                        continue;
                    };
                    let e: [f64; 3] = std::array::from_fn(|k| st(sd[k] - d(ed.origin[k])));
                    let along = st(dot(e, ed.w));
                    if !(along >= 0.0 && along <= d(ed.len)) {
                        continue;
                    }
                    let q: [f64; 3] = std::array::from_fn(|k| st(-along * d(ed.w[k]) + e[k]));
                    if (q[0] * q[0] + q[1] * q[1]) + q[2] * q[2] < rad_sq {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// A corner swept as a sphere of radius `r_eps` (0x8053121-0x8053235).
    fn sweep_vertex(
        &self,
        rec: u32,
        s: &[f32; 3],
        dl: &[f64; 3],
        delta_sq: f32,
        r_eps: f32,
        fraction: f32,
        stored: bool,
    ) -> Sweep {
        if rec == NONE {
            return Sweep::Miss;
        }
        // 1.5b (0x8056695-0x8056920) stores every variable.
        let st = |x: f64| if stored { d(r(x)) } else { x };
        let p = self.verts[rec as usize];
        let q: [f64; 3] = std::array::from_fn(|i| st(d(s[i]) - d(p[i])));
        let sep = st(((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) - d(r_eps) * d(r_eps));
        if sep <= 0.0 {
            return Sweep::Inside;
        }
        let bq = st((dl[0] * q[0] + dl[1] * q[1]) + dl[2] * q[2]);
        if bq >= 0.0 {
            return Sweep::Miss;
        }
        let disc = st(bq * bq - d(delta_sq) * sep);
        if disc < 0.0 {
            return Sweep::Miss;
        }
        let t = r((-disc.sqrt() - bq) / d(delta_sq));
        if t >= fraction {
            if fraction < 1.0 {
                crate::collision::note_contact(d(t), d(fraction));
            }
            return Sweep::Miss;
        }
        let inv = 1.0 / d(r_eps);
        let normal = std::array::from_fn(|i| r(st(dl[i] * d(t) + q[i]) * inv));
        Sweep::Hit(t, normal)
    }

    /// An edge swept as a cylinder of radius `r_eps` (0x8052e7d-0x80530f9).
    fn sweep_edge(
        &self,
        rec: u32,
        s: &[f32; 3],
        dl: &[f64; 3],
        r_eps: f32,
        fraction: f32,
    ) -> Sweep {
        if rec == NONE {
            return Sweep::Miss;
        }
        let ed = &self.edges[rec as usize];
        let q: [f64; 3] = std::array::from_fn(|i| d(s[i]) - d(ed.origin[i]));
        let a = r(d(r(d(r(q[0] * d(ed.u[0]))) + q[1] * d(ed.u[1]))) + q[2] * d(ed.u[2]));
        let b = d(r(d(r(q[0] * d(ed.v[0]))) + q[1] * d(ed.v[1]))) + q[2] * d(ed.v[2]);
        let c = (q[0] * d(ed.w[0]) + q[1] * d(ed.w[1])) + q[2] * d(ed.w[2]);
        let sep = (d(a) * d(a) + b * b) - d(r_eps) * d(r_eps);
        if sep <= 0.0 {
            return if c >= 0.0 && c <= d(ed.len) {
                Sweep::Inside
            } else {
                Sweep::Miss
            };
        }
        let (bf, cf) = (r(b), r(c));
        let du = r(d(r(d(r(dl[0] * d(ed.u[0]))) + dl[1] * d(ed.u[1]))) + dl[2] * d(ed.u[2]));
        let dv = (dl[0] * d(ed.v[0]) + dl[1] * d(ed.v[1])) + dl[2] * d(ed.v[2]);
        let bq = dv * d(bf) + d(du) * d(a);
        if bq >= 0.0 {
            return Sweep::Miss;
        }
        let aa = dv * dv + d(du) * d(du);
        let disc = bq * bq - aa * sep;
        if disc <= 0.0 {
            return Sweep::Miss;
        }
        let t = r((-disc.sqrt() - bq) / aa);
        if t >= fraction {
            if fraction < 1.0 {
                crate::collision::note_contact(d(t), d(fraction));
            }
            return Sweep::Miss;
        }
        let dw = (dl[0] * d(ed.w[0]) + dl[1] * d(ed.w[1])) + dl[2] * d(ed.w[2]);
        let cw = d(cf) + d(t) * dw;
        if cw < 0.0 || cw > d(ed.len) {
            return Sweep::Miss;
        }
        let k1 = (d(t) * d(du) + d(a)) / d(r_eps);
        let k2 = (d(bf) + d(t) * dv) / d(r_eps);
        let normal = std::array::from_fn(|i| r(d(ed.u[i]) * k1 + d(ed.v[i]) * k2));
        Sweep::Hit(t, normal)
    }

    /// [`Self::sweep_edge`] as 1.5b builds it (0x80562c5-0x8056690): each
    /// dot product summed x first on the stack and every variable stored,
    /// the normal stored after its `u` term and again after its `v` term,
    /// on the edge frames that build made (`edges_v22`).
    fn sweep_edge_stored(
        &self,
        rec: u32,
        s: &[f32; 3],
        dl: &[f64; 3],
        r_eps: f32,
        fraction: f32,
    ) -> Sweep {
        if rec == NONE {
            return Sweep::Miss;
        }
        let st = |x: f64| d(r(x));
        let dot = |a: &[f64; 3], b: [f32; 3]| (a[0] * d(b[0]) + a[1] * d(b[1])) + a[2] * d(b[2]);
        let ed = &self.edges_v22[rec as usize];
        let q: [f64; 3] = std::array::from_fn(|i| st(d(s[i]) - d(ed.origin[i])));
        let (a, b, c) = (st(dot(&q, ed.u)), st(dot(&q, ed.v)), st(dot(&q, ed.w)));
        let sep = st((a * a + b * b) - d(r_eps) * d(r_eps));
        if sep <= 0.0 {
            return if c >= 0.0 && c <= d(ed.len) {
                Sweep::Inside
            } else {
                Sweep::Miss
            };
        }
        let (du, dv) = (st(dot(dl, ed.u)), st(dot(dl, ed.v)));
        let bq = st(du * a + dv * b);
        if bq >= 0.0 {
            return Sweep::Miss;
        }
        let aa = st(du * du + dv * dv);
        let disc = st(bq * bq - aa * sep);
        if disc <= 0.0 {
            return Sweep::Miss;
        }
        let t = r((-disc.sqrt() - bq) / aa);
        if t >= fraction {
            if fraction < 1.0 {
                crate::collision::note_contact(d(t), d(fraction));
            }
            return Sweep::Miss;
        }
        let cw = st(d(t) * st(dot(dl, ed.w)) + c);
        if cw < 0.0 || cw > d(ed.len) {
            return Sweep::Miss;
        }
        let k1 = st((d(t) * du + a) / d(r_eps));
        let k2 = st((d(t) * dv + b) / d(r_eps));
        let normal = std::array::from_fn(|i| r(st(d(ed.u[i]) * k1) + d(ed.v[i]) * k2));
        Sweep::Hit(t, normal)
    }

    /// 1.1's point trace against one partition's faces (`cod_lnxded`
    /// 0x8052894), below `fraction`: the nearest face the segment enters
    /// from the front, as fraction, normal and triangle. It reads the
    /// triangle's record and keeps every intermediate on the x87 stack,
    /// rounding only the fraction it stores.
    pub(crate) fn point_clip(
        &self,
        part: &TerrainPart,
        start: [f32; 3],
        end: [f32; 3],
        mut fraction: f32,
    ) -> Option<(f32, [f32; 3], u32)> {
        // `CM_BoxTrace` stores the delta as a float.
        let delta: [f32; 3] = std::array::from_fn(|i| end[i] - start[i]);
        let mut best = None;
        for t in part.first..part.first + part.count {
            let tri = &self.tris[t as usize];
            if tri.degenerate {
                continue;
            }
            let d2 = dot3(end, tri.normal) - d(tri.dist);
            if d2 >= 0.0 {
                continue;
            }
            let d1 = dot3(start, tri.normal) - d(tri.dist);
            if d1 <= 0.0 {
                continue;
            }
            let f = (d1 - d(POINT_CLIP_PAD)) / (d1 - d2);
            if f >= d(fraction) {
                continue;
            }
            let k = d1 / (d1 - d2);
            let p: [f64; 3] = std::array::from_fn(|i| d(start[i]) + d(delta[i]) * k);
            let bary = |n: [f32; 3], dist: f32| {
                (p[0] * d(n[0]) + p[1] * d(n[1])) + p[2] * d(n[2]) - d(dist)
            };
            let a = bary(tri.u, tri.ud);
            if a < d(BARY_LO) || a > d(BARY_HI) {
                continue;
            }
            let b = bary(tri.v, tri.vd);
            if b < d(BARY_LO) || a + b > d(BARY_HI) {
                continue;
            }
            fraction = r(f);
            best = Some((fraction, tri.normal, t));
        }
        best
    }

    /// United Offensive's point trace against one partition's faces
    /// (`coduo_lnxded` 0x8055595, and 1.5b's `cod_lnxded` 0x8055763, the
    /// same code), below `fraction`: the nearest face the segment enters
    /// from the front, as fraction, normal and triangle. Every distance is
    /// stored as a float before it is compared or divided, which is what
    /// makes a wheel's trace end move by a last bit with its start
    /// (docs/research/coduo-vehicles.md 12, the second shove).
    pub(crate) fn point_clip_stored(
        &self,
        part: &TerrainPart,
        start: [f32; 3],
        end: [f32; 3],
        mut fraction: f32,
    ) -> Option<(f32, [f32; 3], u32)> {
        let delta: [f32; 3] = std::array::from_fn(|i| end[i] - start[i]);
        let mut best = None;
        for t in part.first..part.first + part.count {
            let tri = &self.tris[t as usize];
            if tri.degenerate {
                continue;
            }
            let pl = &tri.v22;
            let d2 = r(dot3(end, pl.normal) - d(pl.dist));
            if d2 >= 0.0 {
                continue;
            }
            let d1 = r(dot3(start, pl.normal) - d(pl.dist));
            if d1 <= 0.0 {
                continue;
            }
            let f = r((d(d1) - d(POINT_CLIP_PAD)) / (d(d1) - d(d2)));
            if f >= fraction {
                continue;
            }
            let k = r(d(d1) / (d(d1) - d(d2)));
            let p: [f32; 3] = std::array::from_fn(|i| r(d(delta[i]) * d(k) + d(start[i])));
            let a = r(dot3(p, pl.u) - d(pl.ud));
            if a < BARY_LO || a > BARY_HI {
                continue;
            }
            let b = r(dot3(p, pl.v) - d(pl.vd));
            if b < BARY_LO || d(a) + d(b) > d(BARY_HI) {
                continue;
            }
            fraction = f;
            best = Some((f, pl.normal, t));
        }
        best
    }
}

enum Sweep {
    Miss,
    /// The sphere starts inside the corner or the edge's cylinder.
    Inside,
    Hit(f32, [f32; 3]),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every point-arm call gdb logged on 1.1d and 1.5b reads the same
    /// return here, and the trace's bounds and delta are the segment's
    /// (docs/research/cod11-player-clip.md 8.12).
    #[test]
    fn the_point_arm_returns_what_retail_returned() {
        for (file, arith) in [
            ("pointarm-1.1.txt", Arith::Kept),
            ("pointarm-1.5.txt", Arith::Stored),
        ] {
            let path = format!("{}/tests/fixtures/patch/{file}", crate::manifest_dir!());
            let text = std::fs::read_to_string(&path).unwrap();
            let (mut pass, mut fail) = (0, 0);
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                let w: Vec<&str> = line.split_whitespace().collect();
                let v: Vec<f32> = w[..21]
                    .iter()
                    .map(|h| f32::from_bits(u32::from_str_radix(h, 16).unwrap()))
                    .collect();
                let at = |i: usize| [v[i], v[i + 1], v[i + 2]];
                let (s, e, delta) = (at(0), at(3), at(6));
                for i in 0..3 {
                    assert_eq!(delta[i], e[i] - s[i], "{file}: {line}");
                    assert_eq!(v[9 + i], s[i].min(e[i]), "{file}: {line}");
                    assert_eq!(v[12 + i], s[i].max(e[i]), "{file}: {line}");
                }
                let retail = w[21] == "1";
                assert_eq!(
                    segment_bounds_meet(at(15), at(18), s, e, arith),
                    retail,
                    "{file}: {line}"
                );
                if retail { pass += 1 } else { fail += 1 }
            }
            assert!(
                pass > 100 && fail > 100,
                "{file}: {pass} passed, {fail} failed"
            );
        }
    }

    fn flat_square() -> Terrain {
        // Two triangles of a 100-unit square at z 0, facing up
        // (`(p2 - p0) x (p1 - p0)` is +z).
        let points = [
            [0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0],
            [100.0, 100.0, 0.0],
            [100.0, 0.0, 0.0],
        ];
        let mut t = Terrain::default();
        t.add_partition(&points, &[[0, 1, 2], [0, 2, 3]]);
        t
    }

    fn sweep(start: [f32; 3], end: [f32; 3]) -> CapsuleSweep {
        CapsuleSweep {
            start,
            end,
            delta: sub(end, start),
            delta_sq: (0..3).map(|i| (end[i] - start[i]).powi(2)).sum(),
            radius: 15.0,
            half_height: 35.0,
        }
    }

    #[test]
    fn the_bounds_reject_never_changes_a_clip() {
        // A bumpy 8x8 sheet, one partition, and capsules swept through and
        // over it from random spots; the down-facing copy checks the upper
        // sphere's case.
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut points = Vec::new();
        for y in 0..9 {
            for x in 0..9 {
                points.push([x as f32 * 64.0, y as f32 * 64.0, rnd() * 48.0]);
            }
        }
        let mut tris = Vec::new();
        for y in 0..8u16 {
            for x in 0..8u16 {
                let i = y * 9 + x;
                tris.extend([[i, i + 9, i + 10], [i, i + 10, i + 1]]);
            }
        }
        let mut t = Terrain::default();
        t.add_partition(&points, &tris);
        let flipped: Vec<[u16; 3]> = tris.iter().map(|&[a, b, c]| [a, c, b]).collect();
        t.add_partition(&points, &flipped);
        assert!(t.parts[1].down);
        let mut hits = 0;
        for _ in 0..4000 {
            let mut p = || {
                [
                    rnd() * 600.0 - 40.0,
                    rnd() * 600.0 - 40.0,
                    rnd() * 160.0 - 60.0,
                ]
            };
            let (a, b) = (p(), p());
            let short = rnd() < 0.5;
            let b = if short {
                std::array::from_fn(|i| a[i] + (b[i] - a[i]) * 0.05)
            } else {
                b
            };
            let sw = sweep(a, b);
            let f = if rnd() < 0.3 { rnd() } else { 1.0 };
            for part in &t.parts {
                let stored = rnd() < 0.5;
                let x = t.clip_capsule_as(part, &sw, f, true, stored);
                let y = t.clip_capsule_as(part, &sw, f, false, stored);
                let key = |h: &Option<TerrainHit>| {
                    h.as_ref().map(|h| {
                        (
                            h.fraction.to_bits(),
                            h.normal.map(f32::to_bits),
                            h.tri,
                            h.startsolid,
                        )
                    })
                };
                assert_eq!(key(&x), key(&y), "{a:?} -> {b:?} at {f}");
                hits += usize::from(x.is_some());
            }
        }
        assert!(hits > 400, "too few hits to trust the check: {hits}");
    }

    /// 1.1's point clip off a sloped face's record: the distances off the
    /// stored float `dist` and the fraction kept on the x87 stack until the
    /// store. The pinned bits are an exact 64-bit-mantissa emulation of
    /// 0x8052894's arithmetic on this record; rounding every step to a float
    /// gives 0x3ec60d75 instead.
    #[test]
    fn the_point_clip_rounds_only_the_fraction_it_stores() {
        let points = [
            [1600.0, 1000.0, -97.25],
            [1600.0, 1064.0, -99.5],
            [1664.0, 1064.0, -101.75],
            [1664.0, 1000.0, -99.0],
        ];
        let mut t = Terrain::default();
        t.add_partition(&points, &[[0, 1, 2], [0, 2, 3]]);
        let s = [1659.2268f32, 1006.322, -97.61637];
        let e = [1659.2722f32, 1039.1312, -102.645584];
        let (f, normal, tri) = t.point_clip(&t.parts[0], s, e, 1.0).unwrap();
        assert_eq!(f.to_bits(), 0x3ec6_0d85);
        assert_eq!(normal, t.tris[tri as usize].normal);
        // A trace already stopped short of the face keeps its fraction.
        assert!(t.point_clip(&t.parts[0], s, e, 0.3).is_none());
    }

    #[test]
    fn a_flat_square_keeps_its_outer_edges_and_faces_up() {
        let t = flat_square();
        assert_eq!(t.tris[0].normal, [0.0, 0.0, 1.0]);
        assert!(!t.parts[0].down);
        // The diagonal is shared by two coplanar triangles and stays: the
        // drop needs the far corner in front of the second plane.
        assert_eq!(t.edges.len(), 5);
        assert_eq!(t.verts.len(), 4);
    }

    #[test]
    fn the_lower_sphere_lands_on_the_face_short_of_the_pad() {
        let t = flat_square();
        // The box centre 35 over the feet; the lower sphere's centre at 15.
        let hit = t
            .clip_capsule(
                &t.parts[0],
                &sweep([50.0, 50.0, 85.0], [50.0, 50.0, 25.0]),
                1.0,
                false,
            )
            .expect("a hit");
        assert!(!hit.startsolid);
        assert_eq!(hit.normal, [0.0, 0.0, 1.0]);
        // The sphere falls from 65 to 5 and touches at 15.125.
        let want = 49.875f64 / 60.0 - f64::from(FRACTION_EPS);
        assert!(
            (f64::from(hit.fraction) - want).abs() < 1e-6,
            "{}",
            hit.fraction
        );
    }

    #[test]
    fn a_fall_ending_inside_the_pad_but_above_the_face_misses_the_bounds() {
        let p = flat_square().parts[0];
        // Feet from 0.138 to 0.036 over the face: inside the pad, not
        // through the face, so the partition is never clipped.
        assert!(!p.touches(&sweep([50.0, 50.0, 35.138], [50.0, 50.0, 35.036])));
        assert!(p.touches(&sweep([50.0, 50.0, 35.138], [50.0, 50.0, 34.9])));
    }

    /// The position test: a capsule resting on the face is clear, one
    /// sunk into it, through it or past its edge by less than the radius is
    /// inside, and one wholly under it is not (terrain is one-sided).
    #[test]
    fn a_capsule_sunk_into_the_face_is_inside() {
        let t = flat_square();
        let at = |feet: [f32; 3]| {
            let c = [feet[0], feet[1], feet[2] + 35.0];
            [false, true].map(|stored| t.position_test(&t.parts[0], &sweep(c, c), stored))
        };
        assert_eq!(at([50.0, 50.0, 0.125]), [false; 2]);
        assert_eq!(at([50.0, 50.0, -0.8]), [true; 2]);
        assert_eq!(
            at([40.0, 50.0, -40.0]),
            [true; 2],
            "the axis through the face"
        );
        // On the diagonal 1.5b's planes put the axis a hair outside both
        // triangles (u -1.4e-7), and the deep arm tests no edge.
        assert_eq!(at([50.0, 50.0, -40.0]), [true, false]);
        assert_eq!(at([50.0, 50.0, -80.0]), [false; 2], "wholly under it");
        assert_eq!(at([110.0, 50.0, -6.0]), [true; 2], "the edge cylinder");
        assert_eq!(at([120.0, 50.0, -6.0]), [false; 2]);
    }

    /// 1.5b's and UO's position test stores every step and reads their own
    /// planes and edge frames, 1.1d's keeps the stack: on a slanted sheet a
    /// capsule standing at the rim reads inside under one build and clear
    /// under the other, either way round.
    #[test]
    fn the_two_builds_part_at_a_slanted_rim() {
        let points = [
            [3.7, 1.3, 2.9],
            [1.1, 97.3, 13.7],
            [103.9, 99.1, 31.3],
            [98.3, 2.9, 7.1],
        ];
        let mut t = Terrain::default();
        t.add_partition(&points, &[[0, 1, 2], [0, 2, 3]]);
        let at = |c: [u32; 3]| {
            let c = c.map(f32::from_bits);
            [false, true].map(|stored| t.position_test(&t.parts[0], &sweep(c, c), stored))
        };
        // (90.96331, 100.55016, 30.1036) and (105.532265, 99.987595, 29.958782).
        assert_eq!(at([1119218999, 1120475567, 1106301996]), [false, true]);
        assert_eq!(at([1121128581, 1120401830, 1106226070]), [true, false]);
    }

    #[test]
    fn past_the_edge_the_sphere_meets_the_edge_cylinder() {
        let t = flat_square();
        // Falling 10 units outside the x = 100 edge: the sphere grazes the
        // edge cylinder of radius 15.125, at a height of sqrt(15.125^2 - 10^2).
        let hit = t
            .clip_capsule(
                &t.parts[0],
                &sweep([110.0, 50.0, 85.0], [110.0, 50.0, 25.0]),
                1.0,
                false,
            )
            .expect("an edge hit");
        let h = (15.125f64 * 15.125 - 100.0).sqrt();
        let want = (85.0 - 20.0 - h) / 60.0 - f64::from(FRACTION_EPS);
        assert!(
            (f64::from(hit.fraction) - want).abs() < 1e-5,
            "{}",
            hit.fraction
        );
        assert!(hit.normal[0] > 0.5 && hit.normal[2] > 0.5);
    }
}
