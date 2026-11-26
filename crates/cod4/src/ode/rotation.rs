//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE's quaternion helpers the stepper calls (cod4-player-anims.md 10.4).

use super::math::{M3, Quat, V3};

/// `qb * qc` (`dQMultiply0`, 0x5b95e0).
pub fn q_multiply0(qb: Quat, qc: Quat) -> Quat {
    let b = qb.map(|x| x as f64);
    let c = qc.map(|x| x as f64);
    [
        (b[0] * c[0] - b[1] * c[1] - b[2] * c[2] - b[3] * c[3]) as f32,
        (b[0] * c[1] + b[1] * c[0] + b[2] * c[3] - b[3] * c[2]) as f32,
        (b[0] * c[2] + b[2] * c[0] + b[3] * c[1] - b[1] * c[3]) as f32,
        (b[0] * c[3] + b[3] * c[0] + b[1] * c[2] - b[2] * c[1]) as f32,
    ]
}

/// The rotation matrix of a unit quaternion (`dQtoR`, 0x5b97e0): the
/// three `2 q q` squares stored first.
pub fn q_to_r(q: Quat) -> M3 {
    let q = q.map(|x| x as f64);
    let qq1 = (2.0 * q[1] * q[1]) as f32 as f64;
    let qq2 = (2.0 * q[2] * q[2]) as f32 as f64;
    let qq3 = (2.0 * q[3] * q[3]) as f32 as f64;
    [
        [
            ((1.0 - qq2) - qq3) as f32,
            ((q[1] * q[2] - q[0] * q[3]) * 2.0) as f32,
            ((q[1] * q[3] + q[0] * q[2]) * 2.0) as f32,
        ],
        [
            ((q[1] * q[2] + q[0] * q[3]) * 2.0) as f32,
            ((1.0 - qq1) - qq3) as f32,
            ((q[2] * q[3] - q[0] * q[1]) * 2.0) as f32,
        ],
        [
            ((q[1] * q[3] - q[0] * q[2]) * 2.0) as f32,
            ((q[2] * q[3] + q[0] * q[1]) * 2.0) as f32,
            ((1.0 - qq1) - qq2) as f32,
        ],
    ]
}

/// The quaternion's rate under angular velocity `w` (`dWtoDQ`, 0x5b9af0).
pub fn w_to_dq(w: V3, q: Quat) -> Quat {
    let w = w.map(|x| x as f64);
    let q = q.map(|x| x as f64);
    [
        (0.5 * (-w[0] * q[1] - w[1] * q[2] - w[2] * q[3])) as f32,
        (0.5 * (w[0] * q[0] + w[1] * q[3] - w[2] * q[2])) as f32,
        (0.5 * (-w[0] * q[3] + w[1] * q[0] + w[2] * q[1])) as f32,
        (0.5 * (w[0] * q[2] - w[1] * q[1] + w[2] * q[0])) as f32,
    ]
}

/// `dNormalize4` (0x5b76d0): unit length, the first three squares and
/// the root stored on the way; a zero (or NaN) one reads the identity.
pub fn normalize4(q: &mut Quat) {
    let w = q.map(|x| x as f64);
    let d = ((w[0] * w[0] + w[1] * w[1]) + w[2] * w[2]) as f32;
    let l = (d as f64 + w[3] * w[3]) as f32;
    if l > 0.0 {
        let s = (l as f64).sqrt() as f32;
        let r = (1.0 / s as f64) as f32;
        for x in q.iter_mut() {
            *x = (*x as f64 * r as f64) as f32;
        }
    } else {
        *q = [1.0, 0.0, 0.0, 0.0];
    }
}

/// The unit quaternion of a rotation matrix (`dQfromR`, 0x5b98b0): the
/// trace's branch, else the largest diagonal's.
pub fn r_to_q(r: &M3) -> Quat {
    let w = |x: f32| x as f64;
    let (r0, r5, r10) = (w(r[0][0]), w(r[1][1]), w(r[2][2]));
    // `s = sqrt(x + 1)` and `0.5 / s`, each stored.
    let root = |x: f64| -> (f32, f64) {
        let s = ((x + 1.0) as f32 as f64).sqrt() as f32;
        (
            (0.5 * s as f64) as f32,
            (0.5 * (1.0 / s as f64) as f32 as f64) as f32 as f64,
        )
    };
    let sum = |a: f32, b: f32, s2: f64| ((w(a) + w(b)) * s2) as f32;
    let diff = |a: f32, b: f32, s2: f64| ((w(a) - w(b)) * s2) as f32;
    let tr = (r10 + (r0 + r5)) as f32;
    if tr >= 0.0 {
        let (h, s2) = root(tr as f64);
        return [
            h,
            diff(r[2][1], r[1][2], s2),
            diff(r[0][2], r[2][0], s2),
            diff(r[1][0], r[0][1], s2),
        ];
    }
    let case = if r5 > r0 {
        if r10 > r5 { 2 } else { 1 }
    } else if r10 > r0 {
        2
    } else {
        0
    };
    match case {
        0 => {
            let (h, s2) = root(r0 - (r10 + r5));
            [
                diff(r[2][1], r[1][2], s2),
                h,
                sum(r[1][0], r[0][1], s2),
                sum(r[2][0], r[0][2], s2),
            ]
        }
        1 => {
            let (h, s2) = root(r5 - (r0 + r10));
            [
                diff(r[0][2], r[2][0], s2),
                sum(r[1][0], r[0][1], s2),
                h,
                sum(r[2][1], r[1][2], s2),
            ]
        }
        _ => {
            let (h, s2) = root(r10 - (r0 + r5));
            [
                diff(r[1][0], r[0][1], s2),
                sum(r[2][0], r[0][2], s2),
                sum(r[2][1], r[1][2], s2),
                h,
            ]
        }
    }
}

/// `qb' * qc` (`dQMultiply1`, 0x5b9660).
pub fn q_multiply1(qb: Quat, qc: Quat) -> Quat {
    let (b, c) = (qb.map(|x| x as f64), qc.map(|x| x as f64));
    [
        (((b[0] * c[0] + b[1] * c[1]) + b[2] * c[2]) + b[3] * c[3]) as f32,
        (((b[0] * c[1] - b[1] * c[0]) - b[2] * c[3]) + b[3] * c[2]) as f32,
        (((b[0] * c[2] - b[2] * c[0]) - b[3] * c[1]) + b[1] * c[3]) as f32,
        (((b[0] * c[3] - b[3] * c[0]) - b[1] * c[2]) + b[2] * c[1]) as f32,
    ]
}

/// `qb * qc'` (`dQMultiply2`, 0x5b96e0).
pub fn q_multiply2(qb: Quat, qc: Quat) -> Quat {
    let (b, c) = (qb.map(|x| x as f64), qc.map(|x| x as f64));
    [
        (((b[0] * c[0] + b[1] * c[1]) + b[2] * c[2]) + b[3] * c[3]) as f32,
        (((b[1] * c[0] - b[0] * c[1]) - b[2] * c[3]) + b[3] * c[2]) as f32,
        (((b[2] * c[0] - b[0] * c[2]) - b[3] * c[1]) + b[1] * c[3]) as f32,
        (((b[3] * c[0] - b[0] * c[3]) - b[1] * c[2]) + b[2] * c[1]) as f32,
    ]
}

/// `qb' * qc'` (`dQMultiply3`, 0x5b9760).
pub fn q_multiply3(qb: Quat, qc: Quat) -> Quat {
    let (b, c) = (qb.map(|x| x as f64), qc.map(|x| x as f64));
    [
        (((b[0] * c[0] - b[1] * c[1]) - b[2] * c[2]) - b[3] * c[3]) as f32,
        ((b[2] * c[3] - (b[0] * c[1] + b[1] * c[0])) - b[3] * c[2]) as f32,
        ((b[3] * c[1] - (b[0] * c[2] + b[2] * c[0])) - b[1] * c[3]) as f32,
        ((b[1] * c[2] - (b[0] * c[3] + b[3] * c[0])) - b[2] * c[1]) as f32,
    ]
}
