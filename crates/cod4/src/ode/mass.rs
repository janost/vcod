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

    /// `dMassSetCappedCylinder`: a capsule of `density`, its axis along body
    /// axis `direction` (1..=3), `length` between the caps' centres.
    pub fn capped_cylinder(density: f32, direction: usize, radius: f32, length: f32) -> Mass {
        assert!((1..=3).contains(&direction), "bad direction number");
        let (r, l, d) = (radius as f64, length as f64, density as f64);
        let pi = std::f64::consts::PI;
        let m1 = pi * r * r * l * d;
        let m2 = (4.0 / 3.0) * pi * r * r * r * d;
        let ia = m1 * (0.25 * r * r + (1.0 / 12.0) * l * l)
            + m2 * (0.4 * r * r + 0.375 * r * l + 0.25 * l * l);
        let ib = (m1 * 0.5 + m2 * 0.4) * r * r;
        let mut i = [[0.0; 3]; 3];
        for (k, row) in i.iter_mut().enumerate() {
            row[k] = ia as f32;
        }
        i[direction - 1][direction - 1] = ib as f32;
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
