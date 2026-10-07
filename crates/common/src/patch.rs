//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! A lump-24 bezier patch as retail collides it: `CM_GeneratePatchCollide`'s
//! facet grid and `CM_TraceThroughPatchCollide`'s clip, Q3's `cm_patch.c`
//! with CoD 1.1's changes (docs/research/bsp-ibsp59-format.md, "Patch
//! collision"). Arithmetic follows the x87 code: every value the binary
//! stores to a float is rounded to `f32` here, everything it keeps in a
//! register is `f64`.

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

#[derive(Clone, Copy, Debug)]
struct Plane {
    n: V3,
    d: f32,
}

#[derive(Clone, Copy, Debug)]
struct Border {
    plane: i32,
    inward: bool,
}

#[derive(Clone, Debug)]
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
/// reciprocal scales each component; a zero vector is left alone.
fn normalize(v: &mut V3) -> f32 {
    let len = dot(*v, *v).sqrt() as f32;
    if len != 0.0 {
        let inv = 1.0 / f64::from(len);
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
    fn subdivide_columns(&mut self, tolerance: i32) {
        let tol = f64::from(tolerance);
        let mut i = 0;
        while i + 2 < self.width {
            let needs = (0..self.height).any(|j| {
                let (a, b, c) = (self.at(i, j), self.at(i + 1, j), self.at(i + 2, j));
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
                    let v = (f64::from(mid[c]) + f64::from(next[c])) * 0.5;
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
fn base_winding(n: V3, dist: f32) -> Winding {
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
    let v = -dot(vup, n);
    for k in 0..3 {
        vup[k] = (f64::from(vup[k]) + f64::from(n[k]) * v) as f32;
    }
    let len = dot(vup, vup).sqrt() as f32;
    if len != 0.0 {
        let inv = 1.0 / f64::from(len);
        for c in vup.iter_mut() {
            *c = (f64::from(*c) * inv) as f32;
        }
    } else {
        vup = [0.0; 3];
    }
    let org: V3 = std::array::from_fn(|k| dist * n[k]);
    let vright = cross(vup, n);
    let vup: V3 = std::array::from_fn(|k| vup[k] * MAP_BOUNDS);
    let vright: V3 = std::array::from_fn(|k| vright[k] * MAP_BOUNDS);
    let corner = |sr: f64, su: f64| -> V3 {
        std::array::from_fn(|k| {
            (f64::from(org[k]) + sr * f64::from(vright[k]) + su * f64::from(vup[k])) as f32
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
/// split reads the stored ones.
fn chop_winding(w: Winding, n: V3, dist: f32, epsilon: f32) -> Option<Winding> {
    const FRONT: u8 = 0;
    const BACK: u8 = 1;
    const ON: u8 = 2;
    let eps = f64::from(epsilon);
    let mut dists = Vec::with_capacity(w.len() + 1);
    let mut sides = Vec::with_capacity(w.len() + 1);
    let mut counts = [0usize; 3];
    for p in &w {
        let d = dot(*p, n) - f64::from(dist);
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
        let t = f64::from(dists[i]) / (f64::from(dists[i]) - f64::from(dists[i + 1]));
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
        if normalize(&mut n) == 0.0 {
            return -1;
        }
        let d = dot(p1, n) as f32;
        for (i, pl) in self.planes.iter().enumerate() {
            if dot(n, pl.n) < 0.0 {
                continue;
            }
            let on = |p: V3| {
                let dd = dot(p, pl.n) - f64::from(pl.d);
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
                    let d = dot(*p, pl.n) - f64::from(pl.d);
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
        let mut w = base_winding(s.n, s.d);
        for b in &facet.borders {
            if b.plane == -1 {
                return false;
            }
            let (n, d) = self.border_plane(*b);
            match chop_winding(w, n, d, CHOP_EPSILON) {
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
        let mut w = Some(base_winding(s.n, s.d));
        for b in facet.borders.clone() {
            let Some(cur) = w else { break };
            if b.plane == facet.surface {
                w = Some(cur);
                continue;
            }
            let (n, d) = self.border_plane(b);
            w = chop_winding(cur, n, d, CHOP_EPSILON);
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
            if f64::from(normalize(&mut vec)) < 0.5 {
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
                    if f64::from(normalize(&mut n)) < 0.5 {
                        continue;
                    }
                    let d_ext = dot(w[j], n);
                    let d = d_ext as f32;
                    let mut behind = false;
                    let mut front = false;
                    for p in &w {
                        let dd = dot(*p, n) - d_ext;
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
                    if chop_winding(w.clone(), cn, cd, CHOP_EPSILON).is_some() {
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

impl PatchCollide {
    /// `CM_GeneratePatchCollide` (0x804dfb4): `points` are the record's
    /// `width x height` control points, rows of `width`; `tolerance` is the
    /// record's subdivision distance. `None` where retail would stop the map
    /// load (a bad size, a pool overflow).
    pub fn generate(width: usize, height: usize, tolerance: i32, points: &[V3]) -> Option<Self> {
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
        g.subdivide_columns(tolerance);
        g.remove_degenerate_columns();
        g.transpose();
        g.set_wrap_width();
        g.subdivide_columns(tolerance);
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
            planes: Vec::new(),
            facets: Vec::new(),
            overflow: false,
        };
        b.build_grid(&g);
        if b.overflow {
            return None;
        }
        Some(PatchCollide {
            planes: b.planes,
            facets: b.facets,
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

    /// `CM_TraceThroughPatchCollide` (0x804e944) for `start` to `end`, the
    /// shape's centre, against a trace already at `fraction`. A point takes
    /// `CM_TracePointThroughPatchCollide` (0x804e334).
    pub fn trace(&self, start: V3, end: V3, sweep: Sweep, fraction: f32) -> Option<PatchHit> {
        let (radius, offset) = match sweep {
            Sweep::Point => return self.trace_point(start, end, fraction),
            Sweep::Capsule { radius, offset } => (radius, offset),
        };
        let mut fraction = fraction;
        let mut best: Option<PatchHit> = None;
        'facets: for facet in &self.facets {
            let mut enter = -1.0f32;
            let mut leave = fraction;
            let mut enter_raw = -1.0f32;
            let mut hitnum: i64 = -1;
            let mut normal = [0.0f32; 3];
            let mut check = |n: V3, d: f32, hit_index: i64| -> bool {
                let (d, s, e) = Self::capsule_plane(n, d, radius, offset, start, end);
                let d1 = dot(s, n) - f64::from(d);
                let d2 = dot(e, n) - f64::from(d);
                if d1 > 0.0 && (d2 >= SURFACE_CLIP_EPSILON || d2 >= d1) {
                    return false;
                }
                if d1 <= 0.0 && d2 <= 0.0 {
                    return true;
                }
                if d1 > d2 {
                    let raw = (d1 - SURFACE_CLIP_EPSILON) / (d1 - d2);
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
                    let f = ((d1 + SURFACE_CLIP_EPSILON) / (d1 - d2)).min(1.0);
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

    fn trace_point(&self, start: V3, end: V3, mut fraction: f32) -> Option<PatchHit> {
        let mut front = Vec::with_capacity(self.planes.len());
        let mut inter = Vec::with_capacity(self.planes.len());
        for p in &self.planes {
            let d1 = dot(start, p.n) - f64::from(p.d);
            let d2 = dot(end, p.n) - f64::from(p.d);
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
            let d1 = dot(start, p.n) - f64::from(p.d);
            let d2 = dot(end, p.n) - f64::from(p.d);
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
            .trace([20.0, 20.0, 60.0], [20.0, 20.0, 20.0], sweep, 1.0)
            .expect("a hit from above");
        assert!(
            (down.fraction - (60.0 - 35.125) / 40.0).abs() < 1e-5,
            "{down:?}"
        );
        assert_eq!(down.normal, [0.0, 0.0, 1.0]);
        let up = p.trace([20.0, 20.0, -80.0], [20.0, 20.0, -20.0], sweep, 1.0);
        assert!(up.is_none(), "the back face clipped: {up:?}");
    }

    #[test]
    fn a_point_crosses_the_surface_where_its_borders_allow() {
        let p = flat();
        let hit = p
            .trace([10.0, 10.0, 8.0], [10.0, 10.0, -8.0], Sweep::Point, 1.0)
            .expect("a hit");
        assert!((hit.fraction - (8.0 - 0.125) / 16.0).abs() < 1e-6);
        let miss = p.trace([80.0, 10.0, 8.0], [80.0, 10.0, -8.0], Sweep::Point, 1.0);
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
}
