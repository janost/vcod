//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! A lump-24 bezier patch as retail collides it: `CM_GeneratePatchCollide`'s
//! facet grid and `CM_TraceThroughPatchCollide`'s clip, Q3's `cm_patch.c`
//! with CoD 1.1's changes (docs/research/bsp-ibsp59-format.md, "Patch
//! collision"). Arithmetic follows the x87 code: every value the binary
//! stores to a float is rounded to `f32` here, everything it keeps in a
//! register is `f64`. Which values the binary stores is [`Arith`]'s.

type V3 = [f32; 3];

const MAX_GRID: usize = 129;
const MAX_PATCH_PLANES: usize = 0x1000;
const MAX_FACETS: usize = 0x400;
const NORMAL_EPSILON: f64 = 0.0001;
const DIST_EPSILON: f64 = 0.02;
const PLANE_TRI_EPSILON: f64 = 0.1;
const POINT_EPSILON: f64 = 0.1;
/// The base winding's half size and the facet sanity bound (rodata
/// 0x80ccf68, 0x80cccc4); Q3 has 65535.
const MAP_BOUNDS: f32 = 131072.0;
const CHOP_EPSILON: f32 = 0.1;
const SURFACE_CLIP_EPSILON: f64 = 0.125;

/// Where the engine's x87 code keeps a float variable. 1.1d's `cod_lnxded`
/// (gcc 2.95, optimized) holds many in registers; 1.5b's and United
/// Offensive's (gcc 3.2.3, unoptimized) store every assignment to a float
/// in memory, rounding it, while each expression still runs at the
/// stack's precision (docs/research/bsp-ibsp59-format.md, "Patch collision
/// on 1.5 and UO"). The Windows clients (MSVC, every patch's `CoDMP.exe`,
/// `CoDUOMP.exe` and cgame) store some of those and keep the rest, at the
/// CRT's 53-bit precision (cod15-movement.md 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Kept,
    Stored,
    Msvc,
}

impl Arith {
    /// The build a protocol's Linux server runs: 1.1d's keeps its floats,
    /// 1.5b's and United Offensive's store them.
    pub fn for_protocol(p: &crate::net::protocol::Protocol) -> Self {
        if p.is_v1() {
            Arith::Kept
        } else {
            Arith::Stored
        }
    }

    /// The build a retail client predicts with: the Windows one, whose
    /// collision and pmove code is one MSVC build's on every patch
    /// (cod15-movement.md 8).
    pub fn for_client() -> Self {
        Arith::Msvc
    }

    /// A float variable both stores keep: rounded under gcc 3.2.3 and MSVC.
    pub(crate) fn var(self, x: f64) -> f64 {
        match self {
            Arith::Kept => x,
            Arith::Stored | Arith::Msvc => f64::from(x as f32),
        }
    }

    /// A float variable only gcc 3.2.3 stores; MSVC keeps it on the stack.
    pub(crate) fn gcc_var(self, x: f64) -> f64 {
        match self {
            Arith::Stored => f64::from(x as f32),
            Arith::Kept | Arith::Msvc => x,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Plane {
    n: V3,
    d: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Border {
    plane: i32,
    inward: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Facet {
    surface: i32,
    borders: Vec<Border>,
}

/// One patch's collision: its planes, its facets and the grid's bounds
/// grown by a unit, which is the box retail culls the patch by.
#[derive(Clone, Debug)]
pub struct PatchCollide {
    planes: Vec<Plane>,
    facets: Vec<Facet>,
    /// Per facet, the box its axial planes (the bevels, or the surface)
    /// close, as the trace reads them; `None` where one of the six is
    /// missing. A capsule sweep that stays past one of them, by its radius
    /// and more, fails that plane's check, so [`Self::trace`] skips the
    /// facet without walking its planes.
    facet_boxes: Vec<Option<(V3, V3)>>,
    pub mins: V3,
    pub maxs: V3,
}

/// What sweeps a patch: a point (`tw->isPoint`, the `cm_playerCurveClip`
/// arm) or retail's capsule, its sphere `radius` and the centre-to-sphere
/// offset `(0, 0, offset)`.
#[derive(Clone, Copy, Debug)]
pub enum Sweep {
    Point,
    Capsule { radius: f32, offset: f32 },
}

/// A hit closer than the fraction handed in: retail's fraction, the plane
/// normal it reports, and the unclamped enter fraction of that plane.
#[derive(Clone, Copy, Debug)]
pub struct PatchHit {
    pub fraction: f32,
    pub normal: V3,
    pub raw: f32,
}

fn dot(a: V3, b: V3) -> f64 {
    f64::from(a[0]) * f64::from(b[0])
        + f64::from(a[1]) * f64::from(b[1])
        + f64::from(a[2]) * f64::from(b[2])
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `CrossProduct` (0x80659cc): each component one rounding.
fn cross(a: V3, b: V3) -> V3 {
    let m = |x: f32, y: f32| f64::from(x) * f64::from(y);
    [
        (m(a[1], b[2]) - m(a[2], b[1])) as f32,
        (m(a[2], b[0]) - m(a[0], b[2])) as f32,
        (m(a[0], b[1]) - m(a[1], b[0])) as f32,
    ]
}

/// `VectorNormalize` (0x8065a38): the length is stored as a float and its
/// reciprocal scales each component; a zero vector is left alone. 1.5b's
/// (0x8066845) and its `VectorNormalize2` (0x8066a90) also store the
/// squared length and the reciprocal.
fn normalize(v: &mut V3, arith: Arith) -> f32 {
    let len = arith.var(dot(*v, *v)).sqrt() as f32;
    if len != 0.0 {
        let inv = arith.var(1.0 / f64::from(len));
        for c in v.iter_mut() {
            *c = (f64::from(*c) * inv) as f32;
        }
    }
    len
}

/// `cGrid_t`: `p[i * MAX_GRID + j]` is column `i`, row `j`.
struct Grid {
    width: usize,
    height: usize,
    wrap_width: bool,
    wrap_height: bool,
    p: Vec<V3>,
}

impl Grid {
    fn at(&self, i: usize, j: usize) -> V3 {
        self.p[i * MAX_GRID + j]
    }

    fn set(&mut self, i: usize, j: usize, v: V3) {
        self.p[i * MAX_GRID + j] = v;
    }

    /// Two points within `POINT_EPSILON` on every axis
    /// (`CM_ComparePoints`), as the wrap and degenerate-column tests read it.
    fn same(a: V3, b: V3) -> bool {
        (0..3).all(|k| {
            let d = f64::from(a[k]) - f64::from(b[k]);
            !(d < -POINT_EPSILON || d > POINT_EPSILON)
        })
    }

    fn set_wrap_width(&mut self) {
        self.wrap_width =
            (0..self.height).all(|j| Self::same(self.at(0, j), self.at(self.width - 1, j)));
    }

    /// `CM_SubdivideGridColumns` (0x804c040). CoD's differs from Q3's in two
    /// ways: the tolerance is the record's own (an integer), and a column
    /// that needs no subdivision keeps its approximating point and the walk
    /// steps over both, where Q3 removes it.
    fn subdivide_columns(&mut self, tolerance: i32, arith: Arith) {
        let tol = f64::from(tolerance);
        let mut i = 0;
        while i + 2 < self.width {
            let needs = (0..self.height).any(|j| {
                let (a, b, c) = (self.at(i, j), self.at(i + 1, j), self.at(i + 2, j));
                if arith == Arith::Msvc {
                    // The Windows build keeps all of it (CoDMP15.exe
                    // 0x41c500, CoDUOMP.exe 0x41db40).
                    let dev = [0, 1, 2]
                        .map(|k| (f64::from(a[k]) + f64::from(c[k])) - 2.0 * f64::from(b[k]));
                    let len = (dev[0] * dev[0] + dev[1] * dev[1] + dev[2] * dev[2]).sqrt();
                    return tol < len * 0.25;
                }
                let dev: V3 = std::array::from_fn(|k| {
                    ((f64::from(a[k]) + f64::from(c[k])) - 2.0 * f64::from(b[k])) as f32
                });
                let len = dot(dev, dev).sqrt() as f32;
                tol < f64::from(len) * 0.25
            });
            if !needs {
                i += 2;
                continue;
            }
            for j in 0..self.height {
                let (prev, mid, next) = (self.at(i, j), self.at(i + 1, j), self.at(i + 2, j));
                let mut k = self.width - 1;
                while k > i + 1 {
                    let v = self.at(k, j);
                    self.set(k + 2, j, v);
                    k -= 1;
                }
                let mut p1 = [0.0f32; 3];
                let mut p2 = [0.0f32; 3];
                let mut p3 = [0.0f32; 3];
                for c in 0..3 {
                    p1[c] = ((f64::from(prev[c]) + f64::from(mid[c])) * 0.5) as f32;
                    let v = arith.gcc_var((f64::from(mid[c]) + f64::from(next[c])) * 0.5);
                    p3[c] = v as f32;
                    p2[c] = ((v + f64::from(p1[c])) * 0.5) as f32;
                }
                self.set(i + 1, j, p1);
                self.set(i + 2, j, p2);
                self.set(i + 3, j, p3);
            }
            self.width += 2;
        }
    }

    /// `CM_RemoveDegenerateColumns` (0x804c338), Q3's.
    fn remove_degenerate_columns(&mut self) {
        let mut i = 0;
        while i + 1 < self.width {
            if (0..self.height).all(|j| Self::same(self.at(i, j), self.at(i + 1, j))) {
                for j in 0..self.height {
                    for k in i + 2..self.width {
                        let v = self.at(k, j);
                        self.set(k - 1, j, v);
                    }
                }
                self.width -= 1;
            } else {
                i += 1;
            }
        }
    }

    /// `CM_TransposeGrid` (0x804bdf0).
    fn transpose(&mut self) {
        let mut t = vec![[0.0f32; 3]; MAX_GRID * MAX_GRID];
        for i in 0..self.width {
            for j in 0..self.height {
                t[j * MAX_GRID + i] = self.at(i, j);
            }
        }
        self.p = t;
        std::mem::swap(&mut self.width, &mut self.height);
        std::mem::swap(&mut self.wrap_width, &mut self.wrap_height);
    }
}

/// A winding as `cm_polylib.c` keeps it.
type Winding = Vec<V3>;

/// `BaseWindingForPlane` (0x804fb7c).
/// 1.5b's (0x8051cdf) stores the dot product and builds each corner in
/// two stored steps, `org -/+ vright`, then `-/+ vup`.
fn base_winding(n: V3, dist: f32, arith: Arith) -> Winding {
    let mut max = -MAP_BOUNDS;
    let mut x = usize::MAX;
    for (i, c) in n.iter().enumerate() {
        if c.abs() > max {
            max = c.abs();
            x = i;
        }
    }
    let mut vup = [0.0f32; 3];
    match x {
        0 | 1 => vup[2] = 1.0,
        2 => vup[0] = 1.0,
        _ => {}
    }
    let v = -arith.var(dot(vup, n));
    for k in 0..3 {
        vup[k] = (f64::from(vup[k]) + f64::from(n[k]) * v) as f32;
    }
    if normalize(&mut vup, arith) == 0.0 {
        vup = [0.0; 3];
    }
    let org: V3 = std::array::from_fn(|k| dist * n[k]);
    let vright = cross(vup, n);
    let vup: V3 = std::array::from_fn(|k| vup[k] * MAP_BOUNDS);
    let vright: V3 = std::array::from_fn(|k| vright[k] * MAP_BOUNDS);
    let corner = |sr: f64, su: f64| -> V3 {
        std::array::from_fn(|k| {
            let side = arith.var(f64::from(org[k]) + sr * f64::from(vright[k]));
            (side + su * f64::from(vup[k])) as f32
        })
    };
    vec![
        corner(-1.0, 1.0),
        corner(1.0, 1.0),
        corner(1.0, -1.0),
        corner(-1.0, -1.0),
    ]
}

/// `ChopWindingInPlace` (0x805046c): `None` when nothing is left in front.
/// The side tests read the distance before it is stored as a float; the
/// split reads the stored ones. 1.5b's (0x80528b5) stores the dot product,
/// the distance and the split fraction.
fn chop_winding(w: Winding, n: V3, dist: f32, epsilon: f32, arith: Arith) -> Option<Winding> {
    const FRONT: u8 = 0;
    const BACK: u8 = 1;
    const ON: u8 = 2;
    let eps = f64::from(epsilon);
    let mut dists = Vec::with_capacity(w.len() + 1);
    let mut sides = Vec::with_capacity(w.len() + 1);
    let mut counts = [0usize; 3];
    for p in &w {
        let d = arith.var(arith.gcc_var(dot(*p, n)) - f64::from(dist));
        dists.push(d as f32);
        let side = if d > eps {
            FRONT
        } else if d < -eps {
            BACK
        } else {
            ON
        };
        sides.push(side);
        counts[side as usize] += 1;
    }
    sides.push(sides[0]);
    dists.push(dists[0]);
    if counts[FRONT as usize] == 0 {
        return None;
    }
    if counts[BACK as usize] == 0 {
        return Some(w);
    }
    let mut f = Vec::with_capacity(w.len() + 4);
    for i in 0..w.len() {
        let p1 = w[i];
        if sides[i] == ON {
            f.push(p1);
            continue;
        }
        if sides[i] == FRONT {
            f.push(p1);
        }
        if sides[i + 1] == ON || sides[i + 1] == sides[i] {
            continue;
        }
        let p2 = w[(i + 1) % w.len()];
        let t = arith.var(f64::from(dists[i]) / (f64::from(dists[i]) - f64::from(dists[i + 1])));
        let mid: V3 = std::array::from_fn(|j| {
            if n[j] == 1.0 {
                dist
            } else if n[j] == -1.0 {
                -dist
            } else {
                (f64::from(p1[j]) + t * (f64::from(p2[j]) - f64::from(p1[j]))) as f32
            }
        });
        f.push(mid);
    }
    Some(f)
}

fn winding_bounds(w: &Winding) -> (V3, V3) {
    let mut lo = [MAP_BOUNDS; 3];
    let mut hi = [-MAP_BOUNDS; 3];
    for p in w {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// The per-patch plane pool and facet list `CM_PatchCollideFromGrid` fills.
struct Builder {
    arith: Arith,
    planes: Vec<Plane>,
    facets: Vec<Facet>,
    overflow: bool,
}

impl Builder {
    fn push_plane(&mut self, n: V3, d: f32) -> i32 {
        if self.planes.len() == MAX_PATCH_PLANES {
            self.overflow = true;
            return -1;
        }
        self.planes.push(Plane { n, d });
        self.planes.len() as i32 - 1
    }

    fn plane(&self, i: i32) -> Plane {
        self.planes[i as usize]
    }

    /// `CM_PlaneEqual` (0x804c454): `Some(flipped)` on a match.
    fn plane_equal(p: Plane, n: V3, d: f32) -> Option<bool> {
        let close = |s: f64| {
            (0..3).all(|k| (f64::from(p.n[k]) - s * f64::from(n[k])).abs() < NORMAL_EPSILON)
                && (f64::from(p.d) - s * f64::from(d)).abs() < DIST_EPSILON
        };
        if close(1.0) {
            Some(false)
        } else if close(-1.0) {
            Some(true)
        } else {
            None
        }
    }

    /// `CM_FindPlane2` (0x804c568).
    fn find_plane2(&mut self, n: V3, d: f32) -> (i32, bool) {
        for (i, p) in self.planes.iter().enumerate() {
            if let Some(flipped) = Self::plane_equal(*p, n, d) {
                return (i as i32, flipped);
            }
        }
        (self.push_plane(n, d), false)
    }

    /// `CM_FindPlane` (0x804c648): the plane through three points, or one
    /// already in the pool that faces the same way and holds all three
    /// within `PLANE_TRI_EPSILON`.
    fn find_plane(&mut self, p1: V3, p2: V3, p3: V3) -> i32 {
        let d1 = sub(p2, p1);
        let d2 = sub(p3, p1);
        let mut n = cross(d2, d1);
        if normalize(&mut n, self.arith) == 0.0 {
            return -1;
        }
        let d = dot(p1, n) as f32;
        for (i, pl) in self.planes.iter().enumerate() {
            if dot(n, pl.n) < 0.0 {
                continue;
            }
            let on = |p: V3| {
                let dd = self.arith.gcc_var(dot(p, pl.n) - f64::from(pl.d));
                (-PLANE_TRI_EPSILON..=PLANE_TRI_EPSILON).contains(&dd)
            };
            if on(p1) && on(p2) && on(p3) {
                return i as i32;
            }
        }
        self.push_plane(n, d)
    }

    /// `CM_EdgePlaneNum` (0x804c8b0): the plane through one edge of a grid
    /// square and a point 4 units off the triangle's plane.
    fn edge_plane(&mut self, g: &Grid, gp: &[[i32; 2]], i: usize, j: usize, k: u8) -> i32 {
        let tri_plane = |tri: usize| {
            let e = gp[i * MAX_GRID + j];
            if e[tri] != -1 { e[tri] } else { e[1 - tri] }
        };
        let (p1, p2, tri, swap) = match k {
            0 => (g.at(i, j), g.at(i + 1, j), 0, false),
            1 => (g.at(i + 1, j), g.at(i + 1, j + 1), 0, false),
            2 => (g.at(i, j + 1), g.at(i + 1, j + 1), 1, true),
            3 => (g.at(i, j), g.at(i, j + 1), 1, true),
            4 => (g.at(i + 1, j + 1), g.at(i, j), 0, false),
            _ => (g.at(i, j), g.at(i + 1, j + 1), 1, false),
        };
        let p = tri_plane(tri);
        if p < 0 {
            return -1;
        }
        let n = self.plane(p).n;
        let up: V3 = std::array::from_fn(|c| n[c] * 4.0 + p1[c]);
        if swap {
            self.find_plane(p2, p1, up)
        } else {
            self.find_plane(p1, p2, up)
        }
    }

    /// `CM_SetBorderInward` (0x804caf4).
    fn set_border_inward(&self, facet: &mut Facet, g: &Grid, i: usize, j: usize, which: i32) {
        let points: Vec<V3> = match which {
            -1 => vec![
                g.at(i, j),
                g.at(i + 1, j),
                g.at(i + 1, j + 1),
                g.at(i, j + 1),
            ],
            0 => vec![g.at(i, j), g.at(i + 1, j), g.at(i + 1, j + 1)],
            _ => vec![g.at(i + 1, j + 1), g.at(i, j + 1), g.at(i, j)],
        };
        for b in facet.borders.iter_mut() {
            let (mut front, mut back) = (0, 0);
            if b.plane != -1 {
                let pl = self.plane(b.plane);
                for p in &points {
                    let d = self.arith.gcc_var(dot(*p, pl.n) - f64::from(pl.d));
                    if d > PLANE_TRI_EPSILON {
                        front += 1;
                    } else if d < -PLANE_TRI_EPSILON {
                        back += 1;
                    }
                }
            }
            if front > 0 && back == 0 {
                b.inward = true;
            } else if front == 0 && back == 0 {
                b.plane = -1;
            } else {
                b.inward = false;
            }
        }
    }

    /// A border as the half-space its facet lies behind.
    fn border_plane(&self, b: Border) -> (V3, f32) {
        let p = self.plane(b.plane);
        if b.inward {
            (p.n, p.d)
        } else {
            ([-p.n[0], -p.n[1], -p.n[2]], -p.d)
        }
    }

    /// `CM_ValidateFacet` (0x804cd28).
    fn validate_facet(&self, facet: &Facet) -> bool {
        if facet.surface == -1 {
            return false;
        }
        let s = self.plane(facet.surface);
        let mut w = base_winding(s.n, s.d, self.arith);
        for b in &facet.borders {
            if b.plane == -1 {
                return false;
            }
            let (n, d) = self.border_plane(*b);
            match chop_winding(w, n, d, CHOP_EPSILON, self.arith) {
                Some(next) => w = next,
                None => return false,
            }
        }
        let (lo, hi) = winding_bounds(&w);
        (0..3).all(|k| hi[k] - lo[k] <= MAP_BOUNDS && lo[k] < MAP_BOUNDS && hi[k] > -MAP_BOUNDS)
    }

    /// `CM_AddFacetBevels` (0x804ceec). Unlike Q3's, an edge bevel is kept
    /// only when some winding point lies more than 0.1 behind it, and the
    /// surface plane itself is not checked for.
    fn add_facet_bevels(&mut self, facet: &mut Facet) {
        let s = self.plane(facet.surface);
        let mut w = Some(base_winding(s.n, s.d, self.arith));
        for b in facet.borders.clone() {
            let Some(cur) = w else { break };
            if b.plane == facet.surface {
                w = Some(cur);
                continue;
            }
            let (n, d) = self.border_plane(b);
            w = chop_winding(cur, n, d, CHOP_EPSILON, self.arith);
        }
        let Some(w) = w else { return };
        let (lo, hi) = winding_bounds(&w);

        for axis in 0..3 {
            for dir in [-1.0f32, 1.0] {
                let mut n = [0.0f32; 3];
                n[axis] = dir;
                let d = if dir == 1.0 { hi[axis] } else { -lo[axis] };
                if Self::plane_equal(s, n, d).is_some() {
                    continue;
                }
                if facet
                    .borders
                    .iter()
                    .any(|b| Self::plane_equal(self.plane(b.plane), n, d).is_some())
                {
                    continue;
                }
                let (plane, flipped) = self.find_plane2(n, d);
                facet.borders.push(Border {
                    plane,
                    inward: flipped,
                });
            }
        }

        for j in 0..w.len() {
            let k = (j + 1) % w.len();
            let mut vec = sub(w[j], w[k]);
            if f64::from(normalize(&mut vec, self.arith)) < 0.5 {
                continue;
            }
            snap_vector(&mut vec);
            if vec.iter().any(|&c| c == 1.0 || c == -1.0) {
                continue;
            }
            for axis in 0..3 {
                for dir in [-1.0f32, 1.0] {
                    let mut vec2 = [0.0f32; 3];
                    vec2[axis] = dir;
                    let mut n = cross(vec, vec2);
                    if f64::from(normalize(&mut n, self.arith)) < 0.5 {
                        continue;
                    }
                    // 1.5b's bevel test (0x804e8ab) reads the stored
                    // distance back; 1.1d's keeps it on the stack.
                    let d_ext = self.arith.var(dot(w[j], n));
                    let d = d_ext as f32;
                    let mut behind = false;
                    let mut front = false;
                    for p in &w {
                        let dd = self.arith.var(dot(*p, n) - d_ext);
                        if dd > 0.1 {
                            front = true;
                            break;
                        }
                        if dd < -0.1 {
                            behind = true;
                        }
                    }
                    if front || !behind {
                        continue;
                    }
                    if facet
                        .borders
                        .iter()
                        .any(|b| Self::plane_equal(self.plane(b.plane), n, d).is_some())
                    {
                        continue;
                    }
                    let (plane, flipped) = self.find_plane2(n, d);
                    if plane < 0 {
                        continue;
                    }
                    let border = Border {
                        plane,
                        inward: flipped,
                    };
                    let (cn, cd) = self.border_plane(border);
                    if chop_winding(w.clone(), cn, cd, CHOP_EPSILON, self.arith).is_some() {
                        facet.borders.push(border);
                    }
                }
            }
        }

        // The opposite plane, which the trace never reports a hit on.
        facet.borders.push(Border {
            plane: facet.surface,
            inward: true,
        });
    }

    fn add_facet(&mut self, mut facet: Facet, g: &Grid, i: usize, j: usize, which: i32) {
        if self.facets.len() == MAX_FACETS {
            self.overflow = true;
            return;
        }
        self.set_border_inward(&mut facet, g, i, j, which);
        if self.validate_facet(&facet) {
            self.add_facet_bevels(&mut facet);
            self.facets.push(facet);
        }
    }

    /// `CM_PatchCollideFromGrid` (0x804d754), Q3's.
    fn build_grid(&mut self, g: &Grid) {
        let (w, h) = (g.width, g.height);
        let mut gp = vec![[-1i32; 2]; MAX_GRID * MAX_GRID];
        for i in 0..w - 1 {
            for j in 0..h - 1 {
                let a = self.find_plane(g.at(i, j), g.at(i + 1, j), g.at(i + 1, j + 1));
                let b = self.find_plane(g.at(i + 1, j + 1), g.at(i, j + 1), g.at(i, j));
                gp[i * MAX_GRID + j] = [a, b];
            }
        }
        let at = |i: usize, j: usize| gp[i * MAX_GRID + j];
        for i in 0..w - 1 {
            for j in 0..h - 1 {
                let own = at(i, j);
                // top, right, bottom, left
                let mut border = [-1i32; 4];
                let mut no_adjust = [false; 4];
                let neighbours = [
                    (
                        if j > 0 {
                            at(i, j - 1)[1]
                        } else if g.wrap_height {
                            at(i, h - 2)[1]
                        } else {
                            -1
                        },
                        own[0],
                        0u8,
                    ),
                    (
                        if i + 2 < w {
                            at(i + 1, j)[1]
                        } else if g.wrap_width {
                            at(0, j)[1]
                        } else {
                            -1
                        },
                        own[0],
                        1,
                    ),
                    (
                        if j + 2 < h {
                            at(i, j + 1)[0]
                        } else if g.wrap_height {
                            at(i, 0)[0]
                        } else {
                            -1
                        },
                        own[1],
                        2,
                    ),
                    (
                        if i > 0 {
                            at(i - 1, j)[0]
                        } else if g.wrap_width {
                            at(w - 2, j)[0]
                        } else {
                            -1
                        },
                        own[1],
                        3,
                    ),
                ];
                for (e, &(n, own_tri, k)) in neighbours.iter().enumerate() {
                    border[e] = n;
                    no_adjust[e] = n == own_tri;
                    if n == -1 || no_adjust[e] {
                        border[e] = self.edge_plane(g, &gp, i, j, k);
                    }
                }
                let [top, right, bottom, left] = border;
                let b = |plane: i32| Border {
                    plane,
                    inward: false,
                };
                if own[0] == own[1] {
                    if own[0] == -1 {
                        continue;
                    }
                    let facet = Facet {
                        surface: own[0],
                        borders: vec![b(top), b(right), b(bottom), b(left)],
                    };
                    self.add_facet(facet, g, i, j, -1);
                } else {
                    let mut third = own[1];
                    if third == -1 {
                        third = bottom;
                        if third == -1 {
                            third = self.edge_plane(g, &gp, i, j, 4);
                        }
                    }
                    let facet = Facet {
                        surface: own[0],
                        borders: vec![b(top), b(right), b(third)],
                    };
                    self.add_facet(facet, g, i, j, 0);

                    let mut third = own[0];
                    if third == -1 {
                        third = top;
                        if third == -1 {
                            third = self.edge_plane(g, &gp, i, j, 5);
                        }
                    }
                    let facet = Facet {
                        surface: own[1],
                        borders: vec![b(bottom), b(left), b(third)],
                    };
                    self.add_facet(facet, g, i, j, 1);
                }
            }
        }
    }
}

/// `CM_SnapVector`, Q3's.
fn snap_vector(v: &mut V3) {
    for i in 0..3 {
        for s in [1.0f32, -1.0] {
            if (f64::from(v[i]) - f64::from(s)).abs() < NORMAL_EPSILON {
                *v = [0.0; 3];
                v[i] = s;
                return;
            }
        }
    }
}

/// The box `facet`'s axial planes close, in the trace's sense of each
/// border (`inward` flips the plane), when all six are there.
fn facet_box(planes: &[Plane], facet: &Facet) -> Option<(V3, V3)> {
    let (mut lo, mut hi) = ([None; 3], [None; 3]);
    let s = planes[facet.surface as usize];
    let borders = facet.borders.iter().map(|b| {
        let p = planes[b.plane as usize];
        if b.inward {
            ([-p.n[0], -p.n[1], -p.n[2]], -p.d)
        } else {
            (p.n, p.d)
        }
    });
    for (n, d) in std::iter::once((s.n, s.d)).chain(borders) {
        for i in 0..3 {
            let axial = (0..3).all(|j| j == i || n[j] == 0.0);
            if axial && n[i] == 1.0 {
                hi[i] = Some(hi[i].map_or(d, |h: f32| h.min(d)));
            } else if axial && n[i] == -1.0 {
                lo[i] = Some(lo[i].map_or(-d, |l: f32| l.max(-d)));
            }
        }
    }
    let lo = [lo[0]?, lo[1]?, lo[2]?];
    let hi = [hi[0]?, hi[1]?, hi[2]?];
    Some((lo, hi))
}

impl PatchCollide {
    /// `CM_GeneratePatchCollide` (0x804dfb4): `points` are the record's
    /// `width x height` control points, rows of `width`; `tolerance` is the
    /// record's subdivision distance. `None` where retail would stop the map
    /// load (a bad size, a pool overflow).
    pub fn generate(width: usize, height: usize, tolerance: i32, points: &[V3]) -> Option<Self> {
        Self::generate_as(width, height, tolerance, points, Arith::Kept)
    }

    /// [`Self::generate`] in `arith`'s arithmetic: 1.5b's own build of
    /// the same functions (0x804bf99..0x804f9b3) rounds where 1.1d's does
    /// not.
    pub fn generate_as(
        width: usize,
        height: usize,
        tolerance: i32,
        points: &[V3],
        arith: Arith,
    ) -> Option<Self> {
        if width < 3
            || height < 3
            || width.is_multiple_of(2)
            || height.is_multiple_of(2)
            || width > MAX_GRID
            || height > MAX_GRID
            || points.len() < width * height
        {
            return None;
        }
        let mut g = Grid {
            width,
            height,
            wrap_width: false,
            wrap_height: false,
            p: vec![[0.0; 3]; MAX_GRID * MAX_GRID],
        };
        for i in 0..width {
            for j in 0..height {
                g.set(i, j, points[j * width + i]);
            }
        }
        g.set_wrap_width();
        g.subdivide_columns(tolerance, arith);
        g.remove_degenerate_columns();
        g.transpose();
        g.set_wrap_width();
        g.subdivide_columns(tolerance, arith);
        g.remove_degenerate_columns();

        let mut mins = [f32::MAX; 3];
        let mut maxs = [f32::MIN; 3];
        for i in 0..g.width {
            for j in 0..g.height {
                let p = g.at(i, j);
                for k in 0..3 {
                    mins[k] = mins[k].min(p[k]);
                    maxs[k] = maxs[k].max(p[k]);
                }
            }
        }
        let mut b = Builder {
            arith,
            planes: Vec::new(),
            facets: Vec::new(),
            overflow: false,
        };
        b.build_grid(&g);
        if b.overflow {
            return None;
        }
        let facet_boxes = b.facets.iter().map(|f| facet_box(&b.planes, f)).collect();
        Some(PatchCollide {
            planes: b.planes,
            facets: b.facets,
            facet_boxes,
            mins: mins.map(|v| v - 1.0),
            maxs: maxs.map(|v| v + 1.0),
        })
    }

    pub fn facet_count(&self) -> usize {
        self.facets.len()
    }

    /// One facet plane as the capsule meets it: the plane pushed out by the
    /// radius and the trace moved to the sphere nearest it.
    fn capsule_plane(n: V3, d: f32, radius: f32, offset: f32, start: V3, end: V3) -> (f32, V3, V3) {
        let d = d + radius;
        let t = f64::from(n[2]) * f64::from(offset);
        let shift = if t > 0.0 { -offset } else { offset };
        let s = [start[0], start[1], start[2] + shift];
        let e = [end[0], end[1], end[2] + shift];
        (d, s, e)
    }

    /// `CM_TraceThroughPatchCollide` (0x804e944, 1.5b 0x805050a) for
    /// `start` to `end`, the shape's centre, against a trace already at
    /// `fraction`. A point takes `CM_TracePointThroughPatchCollide`
    /// (0x804e334), whose 1.5b build (0x804f9b3) stores both distances when
    /// `arith` says so; so does 1.5b's capsule arm.
    pub fn trace(
        &self,
        start: V3,
        end: V3,
        sweep: Sweep,
        fraction: f32,
        arith: Arith,
    ) -> Option<PatchHit> {
        self.trace_as(start, end, sweep, fraction, true, arith)
    }

    /// Whether `other` holds the same planes and facets.
    pub fn same_clip(&self, other: &PatchCollide) -> bool {
        self.planes == other.planes && self.facets == other.facets
    }

    /// [`Self::trace`], with or without the facet box reject; the tests
    /// hold the two to the same answer.
    fn trace_as(
        &self,
        start: V3,
        end: V3,
        sweep: Sweep,
        fraction: f32,
        reject: bool,
        arith: Arith,
    ) -> Option<PatchHit> {
        let (radius, offset) = match sweep {
            Sweep::Point => return self.trace_point(start, end, fraction, arith),
            Sweep::Capsule { radius, offset } => (radius, offset),
        };
        let mut fraction = fraction;
        let mut best: Option<PatchHit> = None;
        // The sweep's box: an axial plane's check moves the trace to the
        // sphere nearest it, `offset` along z, and pushes the plane out by
        // the radius. A unit more covers the epsilon and the rounding.
        let off = offset.abs();
        let pad = [radius + 1.0, radius + 1.0, radius + off + 1.0];
        let lo: V3 = std::array::from_fn(|i| start[i].min(end[i]) - pad[i]);
        let hi: V3 = std::array::from_fn(|i| start[i].max(end[i]) + pad[i]);
        'facets: for (facet, bounds) in self.facets.iter().zip(&self.facet_boxes) {
            if reject
                && let Some((flo, fhi)) = bounds
                && (0..3).any(|i| hi[i] < flo[i] || lo[i] > fhi[i])
            {
                continue;
            }
            let mut enter = -1.0f32;
            let mut leave = fraction;
            let mut enter_raw = -1.0f32;
            let mut hitnum: i64 = -1;
            let mut normal = [0.0f32; 3];
            // 1.5b's `CM_CheckFacetPlane` (0x805036e) stores `d1` and `d2`
            // (0x80503b3, 0x80503ec) and each fraction before it compares
            // one (0x805047a, 0x80504cb); 1.1d's inlined copy keeps them.
            let var = |x| arith.var(x);
            let mut check = |n: V3, d: f32, hit_index: i64| -> bool {
                let (d, s, e) = Self::capsule_plane(n, d, radius, offset, start, end);
                let d1 = var(dot(s, n) - f64::from(d));
                let d2 = var(dot(e, n) - f64::from(d));
                if d1 > 0.0 && (d2 >= SURFACE_CLIP_EPSILON || d2 >= d1) {
                    return false;
                }
                if d1 <= 0.0 && d2 <= 0.0 {
                    return true;
                }
                if d1 > d2 {
                    let raw = arith.gcc_var((d1 - SURFACE_CLIP_EPSILON) / (d1 - d2));
                    let f = raw.max(0.0);
                    if f > f64::from(enter) {
                        enter = f as f32;
                        enter_raw = raw as f32;
                        normal = n;
                        if hit_index >= 0 {
                            hitnum = hit_index;
                        }
                    }
                } else {
                    let f = arith
                        .gcc_var((d1 + SURFACE_CLIP_EPSILON) / (d1 - d2))
                        .min(1.0);
                    if f < f64::from(leave) {
                        leave = f as f32;
                    }
                }
                true
            };
            let s = self.planes[facet.surface as usize];
            if !check(s.n, s.d, -1) {
                continue;
            }
            for (j, b) in facet.borders.iter().enumerate() {
                let p = self.planes[b.plane as usize];
                let (n, d) = if b.inward {
                    ([-p.n[0], -p.n[1], -p.n[2]], -p.d)
                } else {
                    (p.n, p.d)
                };
                if !check(n, d, j as i64) {
                    continue 'facets;
                }
            }
            if hitnum == facet.borders.len() as i64 - 1 {
                continue;
            }
            if enter < leave && enter >= 0.0 && enter < fraction {
                fraction = enter;
                best = Some(PatchHit {
                    fraction: enter,
                    normal,
                    raw: enter_raw,
                });
            }
        }
        best
    }

    fn trace_point(&self, start: V3, end: V3, mut fraction: f32, arith: Arith) -> Option<PatchHit> {
        let mut front = Vec::with_capacity(self.planes.len());
        let mut inter = Vec::with_capacity(self.planes.len());
        let var = |x| arith.var(x);
        for p in &self.planes {
            let d1 = var(dot(start, p.n) - f64::from(p.d));
            let d2 = arith.gcc_var(dot(end, p.n) - f64::from(p.d));
            front.push(d1 > 0.0);
            let mut t = 99999.0f32;
            if d1 != d2 {
                let f = (d1 / (d1 - d2)) as f32;
                if f > 0.0 {
                    t = f;
                }
            }
            inter.push(t);
        }
        let mut best = None;
        for facet in &self.facets {
            let s = facet.surface as usize;
            if !front[s] {
                continue;
            }
            let hit = inter[s];
            if hit < 0.0 || hit > fraction {
                continue;
            }
            let blocked = facet.borders.iter().any(|b| {
                let k = b.plane as usize;
                if front[k] == b.inward {
                    inter[k] < hit
                } else {
                    hit < inter[k]
                }
            });
            if blocked {
                continue;
            }
            let p = self.planes[s];
            let d1 = arith.gcc_var(dot(start, p.n) - f64::from(p.d));
            let d2 = arith.gcc_var(dot(end, p.n) - f64::from(p.d));
            let raw = ((d1 - SURFACE_CLIP_EPSILON) / (d1 - d2)) as f32;
            fraction = raw.max(0.0);
            best = Some(PatchHit {
                fraction,
                normal: p.n,
                raw,
            });
        }
        best
    }

    /// `CM_PositionTestInPatchCollide` (0x804f49c): whether a shape resting
    /// at `start` is behind some facet's surface and every one of its
    /// borders. A point never is.
    pub fn position_test(&self, start: V3, sweep: Sweep) -> bool {
        let (radius, offset) = match sweep {
            Sweep::Point => return false,
            Sweep::Capsule { radius, offset } => (radius, offset),
        };
        let behind = |n: V3, d: f32| {
            let (d, s, _) = Self::capsule_plane(n, d, radius, offset, start, start);
            dot(s, n) - f64::from(d) <= 0.0
        };
        self.facets.iter().any(|facet| {
            let s = self.planes[facet.surface as usize];
            behind(s.n, s.d)
                && facet.borders.iter().all(|b| {
                    let p = self.planes[b.plane as usize];
                    if b.inward {
                        behind([-p.n[0], -p.n[1], -p.n[2]], -p.d)
                    } else {
                        behind(p.n, p.d)
                    }
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat 3x3 patch at z 0 over x, y in 0..64: CoD keeps the
    /// approximating points of a flat patch, so it is four facets where
    /// Q3's would be one.
    fn flat() -> PatchCollide {
        let pts: Vec<V3> = (0..3)
            .flat_map(|j| (0..3).map(move |i| [i as f32 * 32.0, j as f32 * 32.0, 0.0]))
            .collect();
        PatchCollide::generate(3, 3, 4, &pts).expect("a valid patch")
    }

    #[test]
    fn a_flat_patch_keeps_its_middle_points() {
        assert_eq!(flat().facet_count(), 4);
    }

    #[test]
    fn a_capsule_lands_on_the_patch_from_its_front_only() {
        let p = flat();
        let sweep = Sweep::Capsule {
            radius: 15.0,
            offset: 20.0,
        };
        // Centre 35 up is the lower sphere's centre 15 up: radius plus the
        // clip epsilon above the surface stops it.
        let down = p
            .trace(
                [20.0, 20.0, 60.0],
                [20.0, 20.0, 20.0],
                sweep,
                1.0,
                Arith::Kept,
            )
            .expect("a hit from above");
        assert!(
            (down.fraction - (60.0 - 35.125) / 40.0).abs() < 1e-5,
            "{down:?}"
        );
        assert_eq!(down.normal, [0.0, 0.0, 1.0]);
        let up = p.trace(
            [20.0, 20.0, -80.0],
            [20.0, 20.0, -20.0],
            sweep,
            1.0,
            Arith::Kept,
        );
        assert!(up.is_none(), "the back face clipped: {up:?}");
    }

    #[test]
    fn a_point_crosses_the_surface_where_its_borders_allow() {
        let p = flat();
        let hit = p
            .trace(
                [10.0, 10.0, 8.0],
                [10.0, 10.0, -8.0],
                Sweep::Point,
                1.0,
                Arith::Kept,
            )
            .expect("a hit");
        assert!((hit.fraction - (8.0 - 0.125) / 16.0).abs() < 1e-6);
        let miss = p.trace(
            [80.0, 10.0, 8.0],
            [80.0, 10.0, -8.0],
            Sweep::Point,
            1.0,
            Arith::Kept,
        );
        assert!(miss.is_none());
    }

    #[test]
    fn a_capsule_sunk_into_the_surface_tests_solid() {
        let p = flat();
        let sweep = Sweep::Capsule {
            radius: 15.0,
            offset: 20.0,
        };
        assert!(p.position_test([32.0, 32.0, 30.0], sweep));
        assert!(!p.position_test([32.0, 32.0, 40.0], sweep));
        assert!(!p.position_test([32.0, 32.0, 40.0], Sweep::Point));
    }

    #[test]
    fn a_curve_past_the_tolerance_subdivides() {
        // A 3x3 bulge 16 up in the middle row along x.
        let pts: Vec<V3> = (0..3)
            .flat_map(|j| {
                (0..3).map(move |i| {
                    [
                        i as f32 * 32.0,
                        j as f32 * 32.0,
                        if i == 1 { 16.0 } else { 0.0 },
                    ]
                })
            })
            .collect();
        let coarse = PatchCollide::generate(3, 3, 16, &pts).unwrap();
        let fine = PatchCollide::generate(3, 3, 2, &pts).unwrap();
        assert!(fine.facet_count() > coarse.facet_count());
    }

    #[test]
    fn the_facet_box_reject_never_changes_a_trace() {
        // An arch 5 control points wide, curved in x and z, finely cut,
        // and capsules swept through and round it from random spots.
        let pts: Vec<V3> = (0..5)
            .flat_map(|j| {
                (0..5).map(move |i| {
                    let a = i as f32 * std::f32::consts::FRAC_PI_4;
                    [
                        64.0 * a.cos(),
                        j as f32 * 48.0 + i as f32 * 6.0,
                        64.0 * a.sin(),
                    ]
                })
            })
            .collect();
        let patch = PatchCollide::generate(5, 5, 2, &pts).unwrap();
        assert!(patch.facet_boxes.iter().filter(|b| b.is_some()).count() > 8);
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut hits = 0;
        for _ in 0..4000 {
            let mut p = || {
                [
                    rnd() * 240.0 - 120.0,
                    rnd() * 260.0 - 40.0,
                    rnd() * 160.0 - 60.0,
                ]
            };
            let (a, b) = (p(), p());
            let b = if rnd() < 0.5 {
                std::array::from_fn(|i| a[i] + (b[i] - a[i]) * 0.1)
            } else {
                b
            };
            let sweep = Sweep::Capsule {
                radius: 15.0,
                offset: if rnd() < 0.5 { 20.0 } else { -20.0 },
            };
            let f = if rnd() < 0.3 { rnd() } else { 1.0 };
            let x = patch.trace_as(a, b, sweep, f, true, Arith::Kept);
            let y = patch.trace_as(a, b, sweep, f, false, Arith::Kept);
            let key = |h: &Option<PatchHit>| {
                h.as_ref().map(|h| {
                    (
                        h.fraction.to_bits(),
                        h.normal.map(f32::to_bits),
                        h.raw.to_bits(),
                    )
                })
            };
            assert_eq!(key(&x), key(&y), "{a:?} -> {b:?} at {f}");
            hits += usize::from(x.is_some());
        }
        assert!(hits > 300, "too few hits to trust the check: {hits}");
    }

    /// 1.5b's capsule arm stores `d1`, `d2` and each fraction
    /// (`CM_CheckFacetPlane`, 0x805036e); on a slanted facet that moves the
    /// answer a bit off 1.1d's.
    #[test]
    fn the_stored_capsule_arm_rounds_where_1_5b_stores() {
        let pts: Vec<V3> = (0..3)
            .flat_map(|j| {
                (0..3).map(move |i| {
                    let bulge = if i == 1 { 13.0 } else { 0.0 };
                    [i as f32 * 32.0, j as f32 * 32.0, bulge + j as f32 * 3.3]
                })
            })
            .collect();
        let p = PatchCollide::generate(3, 3, 4, &pts).expect("a valid patch");
        let sweep = Sweep::Capsule {
            radius: 15.0,
            offset: 20.0,
        };
        let (s, e) = (
            [3.530079, 3.2592847, 74.457214],
            [27.258076, 13.62681, 10.73115],
        );
        let bits = |arith| {
            p.trace(s, e, sweep, 1.0, arith)
                .map(|h| h.fraction.to_bits())
        };
        assert_eq!(bits(Arith::Kept), Some(1056969342));
        assert_eq!(bits(Arith::Stored), Some(1056969341));
        // Windows stores `d1` and `d2` as well (CoDMP15.exe 0x41f002,
        // 0x41f01d), and they are what moves this one.
        assert_eq!(bits(Arith::Msvc), Some(1056969341));
    }
}
