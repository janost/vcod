//! Retail's terrain collision, transcribed from `cod_lnxded` 1.1d:
//! `CM_GenerateTerrainCollide` (0x8051b30) builds each lump-24 terrain
//! partition's triangle, edge and vertex records at load, and the capsule
//! clip (0x8052a58) sweeps one sphere of the capsule against them. Both run
//! on the x87, so every value the binary keeps on the stack is an `f64`
//! here and every value it stores is rounded to `f32` where it stores it
//! (docs/research/cod11-mantle.md, "Terrain is a swept sphere, a patch is a
//! facet", and "The terrain clip's arithmetic").

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
}

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
}

#[derive(Clone, Default)]
pub(crate) struct Terrain {
    tris: Vec<TerrainTri>,
    verts: Vec<[f32; 3]>,
    edges: Vec<TerrainEdge>,
    pub(crate) parts: Vec<TerrainPart>,
}

impl Terrain {
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
            self.tris.push(rec);
        }

        for &([a, b], _) in &edge_list {
            let origin = points[a as usize];
            let mut w = sub(points[b as usize], origin);
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
        }

        self.parts.push(TerrainPart {
            first,
            count: tris.len() as u32,
            down: down && !up,
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

impl Terrain {
    /// The capsule clip (0x8052a58) of one partition against a trace whose
    /// fraction is `fraction`. Returns the last hit it recorded, if any.
    pub(crate) fn clip_capsule(
        &self,
        part: &TerrainPart,
        sw: &CapsuleSweep,
        mut fraction: f32,
    ) -> Option<TerrainHit> {
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

        for ti in part.first..part.first + part.count {
            let t = &self.tris[ti as usize];
            if t.degenerate {
                continue;
            }
            let n = t.normal.map(d);
            let d_e = ((d(e[1]) * n[1] + d(e[0]) * n[0]) + d(e[2]) * n[2]) - d(t.dist);
            if d_e >= re {
                continue;
            }
            let d_s = ((d(s[1]) * n[1] + d(s[0]) * n[0]) + d(s[2]) * n[2]) - d(t.dist);
            let dd = d_s - d_e;
            if dd <= 0.0 {
                continue;
            }
            let bary = |p: [f64; 3]| {
                let u = ((p[0] * d(t.u[0]) + p[1] * d(t.u[1])) + p[2] * d(t.u[2])) - d(t.ud);
                let v = ((p[0] * d(t.v[0]) + p[1] * d(t.v[1])) + p[2] * d(t.v[2])) - d(t.vd);
                (u, v)
            };
            let outside = |u: f64, v: f64| u < 0.0 || v < 0.0 || u + v > 1.0;

            if d_s <= nre {
                // Deep behind the face: solid where the capsule's axis
                // crosses the triangle, at the near pad or the far one.
                let far_d = d(axis) * n[2] + d_s;
                if far_d <= nre {
                    continue;
                }
                let (u0, v0) = bary(s.map(d));
                let tn = (nre - d_s) / n[2];
                let (un, vn) = (u0 + tn * d(t.u[2]), v0 + tn * d(t.v[2]));
                if outside(un, vn) {
                    let tf = if d(r(far_d)) < re {
                        d(axis)
                    } else {
                        (re - d_s) / n[2]
                    };
                    let (uf, vf) = (u0 + tf * d(t.u[2]), v0 + tf * d(t.v[2]));
                    if outside(uf, vf) {
                        continue;
                    }
                }
                return Some(solid(ti, t.normal));
            }

            let (f, p) = if d_s < re {
                (0.0, s)
            } else {
                let f = (d_s - re) / dd;
                if f > d(fraction) {
                    continue;
                }
                (f, std::array::from_fn(|i| r(dl[i] * f + d(s[i]))))
            };
            let (u, v) = bary(p.map(d));
            let bits =
                u32::from(u + v > 1.0) | (u32::from(u < 0.0) << 1) | (u32::from(v < 0.0) << 2);
            if bits == 0 {
                if f <= d(FRACTION_EPS) {
                    return Some(solid(ti, t.normal));
                }
                fraction = r(f - d(FRACTION_EPS));
                hit = Some(TerrainHit {
                    fraction,
                    normal: t.normal,
                    tri: ti,
                    startsolid: false,
                });
                continue;
            }

            for i in 0..3 {
                let swept = if bits & (1 << i) == 0 {
                    self.sweep_vertex(t.verts[i], &s, &dl, sw.delta_sq, r_eps, fraction)
                } else {
                    self.sweep_edge(t.edges[i], &s, &dl, r_eps, fraction)
                };
                match swept {
                    Sweep::Miss => {}
                    Sweep::Inside => return Some(solid(ti, t.normal)),
                    Sweep::Hit(tt, normal) => {
                        // The binary tests the trace's fraction before this
                        // hit, not the hit's, against the epsilon.
                        if fraction <= FRACTION_EPS {
                            return Some(solid(ti, t.normal));
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

    /// A corner swept as a sphere of radius `r_eps` (0x8053121-0x8053235).
    fn sweep_vertex(
        &self,
        rec: u32,
        s: &[f32; 3],
        dl: &[f64; 3],
        delta_sq: f32,
        r_eps: f32,
        fraction: f32,
    ) -> Sweep {
        if rec == NONE {
            return Sweep::Miss;
        }
        let p = self.verts[rec as usize];
        let q: [f64; 3] = std::array::from_fn(|i| d(s[i]) - d(p[i]));
        let sep = ((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) - d(r_eps) * d(r_eps);
        if sep <= 0.0 {
            return Sweep::Inside;
        }
        let bq = (dl[0] * q[0] + dl[1] * q[1]) + dl[2] * q[2];
        if bq >= 0.0 {
            return Sweep::Miss;
        }
        let disc = bq * bq - d(delta_sq) * sep;
        if disc < 0.0 {
            return Sweep::Miss;
        }
        let t = r((-disc.sqrt() - bq) / d(delta_sq));
        if t >= fraction {
            return Sweep::Miss;
        }
        let inv = 1.0 / d(r_eps);
        let normal = std::array::from_fn(|i| r((dl[i] * d(t) + q[i]) * inv));
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

    pub(crate) fn degenerate(&self, tri: u32) -> bool {
        self.tris[tri as usize].degenerate
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
    fn past_the_edge_the_sphere_meets_the_edge_cylinder() {
        let t = flat_square();
        // Falling 10 units outside the x = 100 edge: the sphere grazes the
        // edge cylinder of radius 15.125, at a height of sqrt(15.125^2 - 10^2).
        let hit = t
            .clip_capsule(
                &t.parts[0],
                &sweep([110.0, 50.0, 85.0], [110.0, 50.0, 25.0]),
                1.0,
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
