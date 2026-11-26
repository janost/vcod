//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! A body's mass properties (`dMass`) and the two builders the ragdoll
//! uses (cod4-player-anims.md 10.3).

use super::math::{IDENTITY, M3, V3};

/// `dMass`: the mass, the centre of mass in the body frame and the inertia
/// about the body's point of reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mass {
    pub mass: f32,
    pub c: V3,
    pub i: M3,
}

impl Mass {
    /// `dBodyCreate`'s: 1, centred, unit inertia.
    pub fn unit() -> Mass {
        Mass {
            mass: 1.0,
            c: [0.0; 3],
            i: IDENTITY,
        }
    }

    /// `dMassSetCappedCylinder` (0x5b5cd0): a capsule of `density`, its
    /// axis along body axis `direction` (1..=3), `length` between the caps'
    /// centres. The constants are floats widened (pi at 0x70b080, 4 pi / 3
    /// at 0x70b258, 0.4 at 0x70b250, 1/12 at 0x70b240); the two masses are
    /// stored, the inertia's terms kept on the stack.
    pub fn capped_cylinder(density: f32, direction: usize, radius: f32, length: f32) -> Mass {
        assert!((1..=3).contains(&direction), "bad direction number");
        let (r, l, d) = (radius as f64, length as f64, density as f64);
        let pi = std::f32::consts::PI as f64;
        let k43 = (4.0f32 / 3.0 * std::f32::consts::PI) as f64;
        let (c04, c12) = (0.4f32 as f64, (1.0f32 / 12.0) as f64);
        let m1 = ((pi * r * r * l) * d) as f32 as f64;
        let m2 = ((r * k43 * r * r) * d) as f32 as f64;
        let c = (l * c12) * l + (r * 0.25) * r;
        let b = ((r * c04) * r + (r * 0.375) * l) + (l * 0.25) * l;
        let ia = (c * m1 + b * m2) as f32;
        let ib = (((c04 * m2) + (m1 * 0.5)) * r * r) as f32;
        let mut i = [[0.0; 3]; 3];
        for (k, row) in i.iter_mut().enumerate() {
            row[k] = ia;
        }
        i[direction - 1][direction - 1] = ib;
        Mass {
            mass: (m1 + m2) as f32,
            c: [0.0; 3],
            i,
        }
    }

    /// `dMassAdjust`: the same shape at `mass`.
    pub fn adjust(&mut self, mass: f32) {
        let scale = (mass as f64 / self.mass as f64) as f32;
        self.mass = mass;
        for row in self.i.iter_mut() {
            for x in row.iter_mut() {
                *x = (*x as f64 * scale as f64) as f32;
            }
        }
    }
}

/// `dInvertPDMatrix` for 3x3 (0x5b6840): Cholesky (0x5b6570) and a solve
/// per column (0x5b6680), every assignment stored. `None` when `a` is not
/// positive definite.
pub(crate) fn invert_pd3(a: &M3) -> Option<M3> {
    let w = |x: f32| x as f64;
    let mut l = *a;
    let mut recip = [0.0f32; 3];
    for i in 0..3 {
        for j in 0..i {
            let mut sum = l[i][j];
            for k in 0..j {
                sum = (w(sum) - w(l[i][k]) * w(l[j][k])) as f32;
            }
            l[i][j] = (w(recip[j]) * w(sum)) as f32;
        }
        let mut sum = l[i][i];
        for k in 0..i {
            sum = (w(sum) - w(l[i][k]) * w(l[i][k])) as f32;
        }
        if sum <= 0.0 {
            return None;
        }
        l[i][i] = w(sum).sqrt() as f32;
        recip[i] = (1.0 / w(l[i][i])) as f32;
    }
    let mut inv = [[0.0f32; 3]; 3];
    for col in 0..3 {
        let mut b = [0.0f32; 3];
        b[col] = 1.0;
        let mut y = [0.0f32; 3];
        for i in 0..3 {
            let mut sum = 0.0f32;
            for k in 0..i {
                sum = (w(y[k]) * w(l[i][k]) + w(sum)) as f32;
            }
            y[i] = ((w(b[i]) - w(sum)) / w(l[i][i])) as f32;
        }
        for i in (0..3).rev() {
            let mut sum = 0.0f32;
            for k in i + 1..3 {
                sum = (w(b[k]) * w(l[k][i]) + w(sum)) as f32;
            }
            b[i] = ((w(y[i]) - w(sum)) / w(l[i][i])) as f32;
        }
        for j in 0..3 {
            inv[j][col] = b[j];
        }
    }
    Some(inv)
}
