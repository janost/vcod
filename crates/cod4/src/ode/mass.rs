//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! A body's mass properties (`dMass`) and its builders: the two the
//! ragdoll uses (cod4-player-anims.md 10.4) and the ones worlds 0 and 1's
//! geoms take (cod4-gsc.md 3.6.3). IW's sphere, box and cylinder take the
//! total mass where ODE 0.5's take a density.

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

    /// `dMassSetZero`, as 0x5a4ab0 clears its block.
    pub fn zero() -> Mass {
        Mass {
            mass: 0.0,
            c: [0.0; 3],
            i: [[0.0; 3]; 3],
        }
    }

    /// The 17 words of `dMass` (mass, `c[4]`, `I[12]`), the pads 0.
    pub fn words(&self) -> [u32; 17] {
        let mut w = [0u32; 17];
        w[0] = self.mass.to_bits();
        for k in 0..3 {
            w[1 + k] = self.c[k].to_bits();
            for j in 0..3 {
                w[5 + 4 * k + j] = self.i[k][j].to_bits();
            }
        }
        w
    }

    /// 0x5b5c70: a sphere of `total` mass, `I = total 0.4 r r` (0.4 the
    /// float widened at 0x70b250), the product stored.
    pub fn sphere_total(total: f32, radius: f32) -> Mass {
        let c04 = 0.4f32 as f64;
        let ii = (((total as f64 * c04) * radius as f64) * radius as f64) as f32;
        Mass::diagonal(total, [ii; 3])
    }

    /// `dMassSetBox` as IW has it (0x5b5e50): `total` mass, the full side
    /// lengths; each moment `(a^2 + b^2) (total / 12)` on the stack (12 the
    /// double at 0x70b238), stored once.
    pub fn box_total(total: f32, lx: f32, ly: f32, lz: f32) -> Mass {
        let (x2, y2, z2) = (
            lx as f64 * lx as f64,
            ly as f64 * ly as f64,
            lz as f64 * lz as f64,
        );
        let k = total as f64 / 12.0;
        Mass::diagonal(
            total,
            [
                ((z2 + y2) * k) as f32,
                ((z2 + x2) * k) as f32,
                ((y2 + x2) * k) as f32,
            ],
        )
    }

    /// 0x5b5dc0: a cylinder of `total` mass along body axis `direction`
    /// (1..=3), `length` the ODE formula's: `r^2` stored, the other two
    /// moments `total (length^2 / 12 + r^2 / 4)` (1/12 the float widened at
    /// 0x70b240), the axis's `total 0.5 r^2`, each stored.
    pub fn cylinder_total(total: f32, direction: usize, radius: f32, length: f32) -> Mass {
        assert!((1..=3).contains(&direction), "bad direction number");
        let r2 = (radius as f64 * radius as f64) as f32 as f64;
        let (m, l) = (total as f64, length as f64);
        let c12 = (1.0f32 / 12.0) as f64;
        let ia = ((c12 * l * l + 0.25 * r2) * m) as f32;
        let ib = (m * 0.5 * r2) as f32;
        let mut d = [ia; 3];
        d[direction - 1] = ib;
        Mass::diagonal(total, d)
    }

    /// `PhysMass` (0x5a49d0): a model's moments and products per unit of
    /// mass times `total`, each stored; a moment not over 0 takes 100 (the
    /// float at 0x6f9e58). The products go in as they are, not negated;
    /// the centre stays 0.
    pub fn phys_mass(total: f32, moments: [f32; 3], products: [f32; 3]) -> Mass {
        let t = total as f64;
        let s = |x: f32| (x as f64 * t) as f32;
        let mo = moments.map(|x| if x > 0.0 { x } else { 100.0 });
        let p = products.map(s);
        Mass {
            mass: total,
            c: [0.0; 3],
            i: [
                [s(mo[0]), p[0], p[1]],
                [p[0], s(mo[1]), p[2]],
                [p[1], p[2], s(mo[2])],
            ],
        }
    }

    fn diagonal(total: f32, d: [f32; 3]) -> Mass {
        let mut i = [[0.0; 3]; 3];
        for k in 0..3 {
            i[k][k] = d[k];
        }
        Mass {
            mass: total,
            c: [0.0; 3],
            i,
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
