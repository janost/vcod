//! The 1.7 client's position test of a capsule in an indexed triangle
//! (`iw3mp.exe` 0x4f1930, cod4-collision.md 3.1): a sphere against the
//! triangle's nearest point, a capsule's axis against its face and edges.
//! The client runs at 53-bit precision, so a kept value is an `f64` here
//! and a stored one goes through `f32` (`st`).

// A test written `!(a < b)` keeps the branch a NaN takes.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::Vec3;

/// A value stored to a float.
fn st(x: f64) -> f32 {
    x as f32
}

/// `a - b` per component, each stored.
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        st(f64::from(a.x) - f64::from(b.x)),
        st(f64::from(a.y) - f64::from(b.y)),
        st(f64::from(a.z) - f64::from(b.z)),
    )
}

/// `(x + y) + z` of the products, stored.
fn dot(a: Vec3, b: Vec3) -> f32 {
    st(super::trace::dot(a, b))
}

/// Whether the capsule from `start`, its end spheres `sphere_z` above and
/// below, comes within `radius` of the triangle `tri` (0x4f1930). A
/// capsule with no length takes the sphere arm, any other its axis.
pub(super) fn capsule_in_tri(start: Vec3, radius: f32, sphere_z: f32, tri: [Vec3; 3]) -> bool {
    let r2 = st(f64::from(radius) * f64::from(radius));
    if sphere_z == 0.0 {
        return r2 > point_dist2(start, tri);
    }
    let top = Vec3::new(
        start.x,
        start.y,
        st(f64::from(sphere_z) + f64::from(start.z)),
    );
    let bottom = Vec3::new(
        start.x,
        start.y,
        st(f64::from(start.z) - f64::from(sphere_z)),
    );
    segment_meets_tri(top, bottom, r2, tri)
}

/// The squared distance from `p` to the point 0x4f0b70 returns (0x4f0f90),
/// each offset stored.
fn point_dist2(p: Vec3, [v0, v1, v2]: [Vec3; 3]) -> f32 {
    let d = sub(nearest(p, v0, sub(v1, v0), sub(v2, v0)), p);
    dot(d, d)
}

/// 0x4f0b70: the triangle `v0`, `v0 + e1`, `v0 + e2`'s point nearest
/// `2 v0 - p`, not `p`: it takes `p - v0` where the regions it then picks
/// need `v0 - p` (cod4-collision.md 3.1). Each product sum is stored; the
/// region tests' sums are kept.
fn nearest(p: Vec3, v0: Vec3, e1: Vec3, e2: Vec3) -> Vec3 {
    let (a, b, c) = (dot(e1, e1), dot(e1, e2), dot(e2, e2));
    let dp = sub(p, v0);
    let (d, e) = (dot(dp, e1), dot(dp, e2));
    let [a, b, c, d, e] = [a, b, c, d, e].map(f64::from);
    let det = f64::from(st(a * c - b * b));
    let s = f64::from(st(e * b - d * c));
    let t = f64::from(st(d * b - e * a));
    // `-x` stored, then against `limit`: the fraction it gives, or 1.
    let ratio = |x: f64, limit: f64| {
        let n = f64::from(st(-x));
        if !(n >= limit) { st(n / limit) } else { 1.0 }
    };
    // Along e1 with no e2 (0x4f0ddf), along e2 with no e1 (0x4f0cf6).
    let edge1 = || {
        if 0.0 <= d {
            (0.0, 0.0)
        } else {
            (ratio(d, a), 0.0)
        }
    };
    let edge2 = || {
        if 0.0 <= e {
            (0.0, 0.0)
        } else {
            (0.0, ratio(e, c))
        }
    };
    // Across the far edge (0x4f0e3e).
    let far = || {
        let num = f64::from(st((e + (c - b)) - d));
        if num < 0.0 {
            return (0.0, 1.0);
        }
        let den = f64::from(st(c + (a - (b + b))));
        if den <= num {
            return (1.0, 0.0);
        }
        let s = st(num / den);
        (s, st(1.0 - f64::from(s)))
    };
    let (s, t) = if det >= s + t {
        if s < 0.0 {
            if !(t < 0.0) {
                edge2()
            } else if 0.0 > d {
                edge1()
            } else if e < 0.0 {
                edge2()
            } else {
                (0.0, 0.0)
            }
        } else if t < 0.0 {
            edge1()
        } else {
            let inv = f64::from(st(1.0 / det));
            (st(s * inv), st(inv * t))
        }
    } else if s < 0.0 {
        let k0 = e + c;
        if 0.0 <= k0 {
            edge2()
        } else if d + b < k0 {
            far()
        } else {
            (0.0, 1.0)
        }
    } else if !(t < 0.0) {
        far()
    } else {
        let k0 = d + a;
        if 0.0 <= k0 {
            edge1()
        } else if e + b < k0 {
            far()
        } else {
            (1.0, 0.0)
        }
    };
    let (s, t) = (f64::from(s), f64::from(t));
    Vec3::from(std::array::from_fn::<f32, 3, _>(|k| {
        st((f64::from(e1[k]) * s + f64::from(v0[k])) + f64::from(e2[k]) * t)
    }))
}

/// The triangle's set-up 0x4f1400 passes 0x4f0ec0: the edges from `v0`,
/// their dots, and `e22 e11 - e12^2`, kept.
struct Face {
    v0: Vec3,
    e1: Vec3,
    e2: Vec3,
    e11: f64,
    e12: f64,
    e22: f64,
    det: f64,
}

impl Face {
    /// 0x4f0ec0: whether `q`'s foot on the face lies inside the triangle,
    /// edges included.
    fn holds(&self, q: Vec3) -> bool {
        let dq = sub(self.v0, q);
        let (d1, d2) = (f64::from(dot(self.e1, dq)), f64::from(dot(self.e2, dq)));
        let s = d2 * self.e12 - d1 * self.e22;
        if s < 0.0 {
            return false;
        }
        let t = self.e12 * d1 - d2 * self.e11;
        0.0 <= t && s + t <= self.det
    }
}

/// 0x4f1400: whether the segment `top`..`bottom` comes within
/// `sqrt(r2)` of the triangle: through the face, at an end near the face
/// over it, or at an edge.
fn segment_meets_tri(top: Vec3, bottom: Vec3, r2: f32, [v0, v1, v2]: [Vec3; 3]) -> bool {
    let (e1, e2) = (sub(v1, v0), sub(v2, v0));
    let [ex, ey, ez] = e1.to_array().map(f64::from);
    let [fx, fy, fz] = e2.to_array().map(f64::from);
    let n = Vec3::new(
        st(fz * ey - fy * ez),
        st(fx * ez - fz * ex),
        st(fy * ex - fx * ey),
    );
    let plane = f64::from(dot(v0, n));
    let d_top = f64::from(dot(top, n)) - plane;
    let d_bottom = f64::from(dot(bottom, n)) - plane;
    let (e11, e12, e22) = (dot(e1, e1), dot(e1, e2), dot(e2, e2));
    let [e11, e12, e22] = [e11, e12, e22].map(f64::from);
    let face = Face {
        v0,
        e1,
        e2,
        e11,
        e12,
        e22,
        det: e22 * e11 - e12 * e12,
    };
    let crosses = d_bottom * d_top < 0.0;
    if crosses {
        let f = f64::from(st(d_top / (d_top - d_bottom)));
        let q = Vec3::from(std::array::from_fn::<f32, 3, _>(|k| {
            let (a, b) = (f64::from(top[k]), f64::from(bottom[k]));
            st((b - a) * f + a)
        }));
        if face.holds(q) {
            return true;
        }
    }
    let reach = f64::from(dot(n, n)) * f64::from(r2);
    let top_near = d_top * d_top < reach;
    let bottom_near = d_bottom * d_bottom < reach;
    if !crosses && !top_near && !bottom_near {
        return false;
    }
    if (top_near && face.holds(top)) || (bottom_near && face.holds(bottom)) {
        return true;
    }
    let axis = sub(bottom, top);
    [(v0, e1), (v0, e2), (v1, sub(v2, v1))]
        .into_iter()
        .any(|(from, along)| r2 > segment_dist2(top, axis, from, along))
}

/// 0x4f10e0: the squared distance between the segments `b0 + s d0` and
/// `b1 + t d1`, `s` and `t` in 0..1, as the region it picks evaluates it.
/// Its dots are stored; the determinant, the numerators and the sums are
/// kept.
fn segment_dist2(b0: Vec3, d0: Vec3, b1: Vec3, d1: Vec3) -> f32 {
    let diff = sub(b0, b1);
    let [a, b, c] = [dot(d0, d0), dot(d0, d1), dot(d1, d1)].map(f64::from);
    let [d, e, f] = [dot(d0, diff), dot(d1, diff), dot(diff, diff)].map(f64::from);
    let det = c * a - b * b;
    let s = e * b - d * c;
    // t = 0, s free (0x4f12aa).
    let along0 = || {
        if 0.0 <= d {
            f
        } else if !(-d >= a) {
            let s = -(d / a);
            ((a * s) + (d + d)) * s + f
        } else {
            (a + (d + d)) + f
        }
    };
    // t = 1, s free (0x4f1377).
    let along1 = || {
        if b <= d {
            f + (c - (e + e))
        } else if !(b - d >= a) {
            let s = (b - d) / a;
            f + ((c + s * ((a * s + (d - b)) + (d - b))) - (e + e))
        } else {
            (((a + c) + ((d - e) - b)) + ((d - e) - b)) + f
        }
    };
    let dist = if 0.0 >= s {
        // s = 0, t free.
        if 0.0 >= e {
            along0()
        } else if e >= c {
            along1()
        } else {
            let t = e / c;
            f + (c * t - (e + e)) * t
        }
    } else if s >= det {
        // s = 1, t free.
        let k = e + b;
        if 0.0 >= k {
            along0()
        } else if k >= c {
            along1()
        } else {
            let t = k / c;
            ((a + (c * t - (k + k)) * t) + (d + d)) + f
        }
    } else {
        let t = e * a - d * b;
        if 0.0 >= t {
            along0()
        } else if t >= det {
            along1()
        } else {
            let inv = 1.0 / det;
            let (s, t) = (inv * s, inv * t);
            let m = d - b * t;
            f + (s * ((a * s) + (m + m)) + t * ((c * t) - (e + e)))
        }
    };
    st(dist)
}

#[cfg(test)]
mod tests;
