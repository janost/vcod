//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE's quaternion helpers the stepper calls (cod4-player-anims.md 10.3).

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

/// The rotation matrix of a unit quaternion (`dQtoR`, 0x5b97e0).
pub fn q_to_r(q: Quat) -> M3 {
    let q = q.map(|x| x as f64);
    let qq1 = 2.0 * q[1] * q[1];
    let qq2 = 2.0 * q[2] * q[2];
    let qq3 = 2.0 * q[3] * q[3];
    [
        [
            (1.0 - qq2 - qq3) as f32,
            (2.0 * (q[1] * q[2] - q[0] * q[3])) as f32,
            (2.0 * (q[1] * q[3] + q[0] * q[2])) as f32,
        ],
        [
            (2.0 * (q[1] * q[2] + q[0] * q[3])) as f32,
            (1.0 - qq1 - qq3) as f32,
            (2.0 * (q[2] * q[3] - q[0] * q[1])) as f32,
        ],
        [
            (2.0 * (q[1] * q[3] - q[0] * q[2])) as f32,
            (2.0 * (q[2] * q[3] + q[0] * q[1])) as f32,
            (1.0 - qq1 - qq2) as f32,
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

/// `dNormalize4` (0x5b76d0): unit length, or the identity for a zero one.
pub fn normalize4(q: &mut Quat) {
    let w = q.map(|x| x as f64);
    let l = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2] + w[3] * w[3]) as f32;
    if l > 0.0 {
        let r = (1.0 / (l as f64).sqrt()) as f32;
        for x in q.iter_mut() {
            *x = (*x as f64 * r as f64) as f32;
        }
    } else {
        *q = [1.0, 0.0, 0.0, 0.0];
    }
}
