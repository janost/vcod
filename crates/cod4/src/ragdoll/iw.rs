//! The ragdoll's own float helpers in iw3mp.exe 1.7, each rounding where
//! the binary stores a DWORD (cod4-player-anims.md 10.7). IW's quaternions
//! are `x y z w`; ODE's (`crate::ode`) are `w x y z`.

use crate::ode::math::{M3, V3};

/// An IW quaternion, `w` last.
pub type IwQuat = [f32; 4];

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// 0x5bb840 called with `a` and `b`: `b (x) a`, each part stored.
pub fn qmul(a: IwQuat, b: IwQuat) -> IwQuat {
    let [ax, ay, az, aw] = a.map(w);
    let [bx, by, bz, bw] = b.map(w);
    [
        (((bx * aw + ax * bw) + by * az) - bz * ay) as f32,
        (((ay * bw + by * aw) + bz * ax) - bx * az) as f32,
        (((bz * aw + az * bw) + ay * bx) - by * ax) as f32,
        (((aw * bw - bx * ax) - ay * by) - bz * az) as f32,
    ]
}

/// `q (x) (0, 0, 1, 0)` (the constant at 0x71e648): a mirrored bone's turn
/// half round its local z. Every product but one is by 0 or 1, so exact.
pub fn flip(q: IwQuat) -> IwQuat {
    [q[1], -q[0], q[3], -q[2]]
}

pub fn conj(q: IwQuat) -> IwQuat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// 0x5636e0: `q` over its length, the squares' sum, the root and its
/// inverse stored; a zero one is left as it is.
pub fn normalize(q: IwQuat) -> IwQuat {
    let d =
        (((w(q[0]) * w(q[0]) + w(q[1]) * w(q[1])) + w(q[2]) * w(q[2])) + w(q[3]) * w(q[3])) as f32;
    let len = w(d).sqrt() as f32;
    if len == 0.0 {
        return q;
    }
    let k = (1.0 / w(len)) as f32;
    q.map(|x| (w(x) * w(k)) as f32)
}

/// 0x560fb0: the rotation's three axes as rows (row 0 the turned x), off
/// `2 / |q|^2`, each term stored.
pub fn axes(q: IwQuat) -> M3 {
    let f = |x: f64| x as f32;
    let [x, y, z, qw] = q;
    let (xx, yy, zz, ww) = (
        f(w(x) * w(x)),
        f(w(y) * w(y)),
        f(w(z) * w(z)),
        f(w(qw) * w(qw)),
    );
    let n = f(((w(yy) + w(xx)) + w(zz)) + w(ww));
    let s = f(2.0 / w(n));
    let (xx, yy, zz) = (f(w(xx) * w(s)), f(w(yy) * w(s)), f(w(zz) * w(s)));
    let xs = f(w(x) * w(s));
    let (xy, xz, xw) = (f(w(y) * w(xs)), f(w(z) * w(xs)), f(w(xs) * w(qw)));
    let ys = f(w(y) * w(s));
    let (yz, yw) = (f(w(z) * w(ys)), f(w(ys) * w(qw)));
    let zw = f((w(s) * w(z)) * w(qw));
    [
        [f(1.0 - (w(yy) + w(zz))), f(w(xy) + w(zw)), f(w(xz) - w(yw))],
        [f(w(xy) - w(zw)), f(1.0 - (w(xx) + w(zz))), f(w(yz) + w(xw))],
        [f(w(xz) + w(yw)), f(w(yz) - w(xw)), f(1.0 - (w(xx) + w(yy)))],
    ]
}

/// 0x596b80: axes as rows to ODE's `R`, whose columns they are.
pub fn axes_to_r(a: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[j][i]))
}

/// 0x5a4740: `p + axes . o` (`(a1 o1 + a0 o0) + a2 o2` per part), the turn
/// and the sum stored; `axes` by rows, as a phys object keeps them.
pub fn offset(p: V3, axes: &M3, o: V3) -> V3 {
    std::array::from_fn(|i| {
        let t =
            ((w(axes[1][i]) * w(o[1]) + w(axes[0][i]) * w(o[0])) + w(axes[2][i]) * w(o[2])) as f32;
        (w(p[i]) + w(t)) as f32
    })
}

/// 0x5bbb00: a unit quaternion's rotation vector, `xyz * 2a / sin a` for
/// `a = acos w`, clamped to pi; 0 when `|sin a|` is 1e-6 or less (the
/// double at 0x70b508) or NaN.
// A NaN sine reads as 0, as the compare's flags do.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn rotation_vector(q: IwQuat) -> V3 {
    let pi = std::f32::consts::PI as f64;
    let mut a = w(q[3]).acos() as f32;
    if w(a) > pi || w(a) < -pi {
        a = std::f32::consts::PI;
    }
    let s = w(a).sin() as f32;
    if !(w(s).abs() > 1e-6) {
        return [0.0; 3];
    }
    let k = ((w(a) + w(a)) / w(s)) as f32;
    [q[0], q[1], q[2]].map(|x| (w(x) * w(k)) as f32)
}
