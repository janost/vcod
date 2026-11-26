//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! One island's step (`dxQuickStepper`, iw3mp.exe 0x5b86e0) and its
//! projected Gauss-Seidel solver (`SOR_LCP`, 0x5b8080, iterating in
//! 0x5b7ad0 or, for a one-body island, 0x5b7df0), with IW's changes:
//! no warm start, its row order and reorder, and the velocity clamp after
//! the solve (cod4-player-anims.md 10.3).

use super::body::{Body, DISABLED, NO_GRAVITY};
use super::joint::{Info2, Joint};
use super::math::{M3, V3, dot, mul0_331, mul0_333, mul2_333};
use super::world::{BodyId, JointId, Params};

/// ODE's `dInfinity` in this build: `FLT_MAX` (the floats at 0x70af58,
/// 0x70af5c that seed each row's `hi` and `lo`).
pub const INFINITY: f32 = f32::MAX;

/// The angular speed past which a body is slowed, squared (the double at
/// 0x70b928), and the speed it is slowed to (0x70b920).
const MAX_ANGULAR2: f64 = 1600.0;
const CLAMP_ANGULAR: f64 = 39.0;
/// The linear pair (0x70b918, `1e12` as a float; 0x70b910).
const MAX_LINEAR2: f64 = 999_999_995_904.0;
const CLAMP_LINEAR: f64 = 999_900.0;

/// One constraint row, IW's 0x90-byte record at world +0x750: the
/// Jacobian's four blocks, `inv(M) J'`'s, the bounds, the right-hand side
/// and `Ad`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Row {
    pub j1l: V3,
    pub j1a: V3,
    pub j2l: V3,
    pub j2a: V3,
    pub lambda: f32,
    pub b1: i32,
    pub b2: i32,
    pub imj1l: V3,
    pub imj1a: V3,
    pub imj2l: V3,
    pub imj2a: V3,
    pub lo: f32,
    pub hi: f32,
    pub rhs: f32,
    pub ad: f32,
}

/// The MSVC `rand` step IW's reorder draws with (0x5b7b1c).
#[inline]
pub fn lcg(seed: u32) -> u32 {
    seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3)
}

/// The next draw in `0..m` and the seed after it: `((s >> 17) * m) >> 15`
/// through `__allmul` and `__allshr` (0x5b7b2c..0x5b7b39).
#[inline]
pub fn draw(seed: u32, m: usize) -> (usize, u32) {
    let s = lcg(seed);
    ((((s >> 17) as i64 * m as i64) >> 15) as usize, s)
}

/// IW's reorder (0x5b7aec..0x5b7b7d): `m` swaps of two drawn rows.
pub fn reorder(order: &mut [usize], seed: &mut u32) {
    let m = order.len();
    for _ in 0..m {
        let (a, s) = draw(*seed, m);
        let (b, s) = draw(s, m);
        *seed = s;
        order.swap(a, b);
    }
}

/// The two Jacobian halves' dot with `v[b1]` and `v[b2]`, IW's sum: each
/// three-term dot stored, added to the running sum and stored again,
/// linear block before angular (`multiply_J`, 0x5b79f0; the diagonal at
/// 0x5b80db).
fn row_dot(a: [V3; 4], v: [[V3; 2]; 2], two: bool) -> f32 {
    let b1 = v[0];
    let mut s = (dot(a[0], b1[0]) as f64 + 0.0) as f32;
    s = (dot(a[1], b1[1]) as f64 + s as f64) as f32;
    if two {
        let b2 = v[1];
        s = (dot(a[2], b2[0]) as f64 + s as f64) as f32;
        s = (dot(a[3], b2[1]) as f64 + s as f64) as f32;
    }
    s
}

/// A body's world-frame inertia `R I R'` (0x5b6490 then 0x5aeec0).
fn world_inertia(i: &M3, r: &M3) -> M3 {
    mul0_333(r, &mul2_333(i, r))
}

/// One island's step. `bodies` are the island's in order, `joints` its
/// joints; each joint's ends already carry their island index (or -1).
pub(crate) fn step(
    params: &Params,
    seed: &mut u32,
    bodies: &mut [Option<Body>],
    island: &[BodyId],
    joints: &mut [Option<Joint>],
    jlist: &[JointId],
    h: f32,
) {
    let nb = island.len();
    let hw = h as f64;
    let s1 = (1.0 / hw) as f32;
    let s1w = s1 as f64;
    fn at<'a>(bodies: &'a mut [Option<Body>], island: &[BodyId], i: usize) -> &'a mut Body {
        bodies[island[i].0 as usize].as_mut().expect("island body")
    }

    // The world-frame inertias and the gyroscopic torque (0x5b8740..0x5b88c1).
    let mut inv_i = Vec::with_capacity(nb);
    for i in 0..nb {
        let b = at(bodies, island, i);
        let ig = world_inertia(&b.mass.i, &b.r);
        inv_i.push(world_inertia(&b.inv_i, &b.r));
        let t = mul0_331(&ig, b.avel);
        let a = b.avel;
        let w = |x: f32| x as f64;
        b.tacc[0] = (w(b.tacc[0]) - (w(a[1]) * w(t[2]) - w(a[2]) * w(t[1]))) as f32;
        b.tacc[1] = (w(b.tacc[1]) - (w(a[2]) * w(t[0]) - w(a[0]) * w(t[2]))) as f32;
        b.tacc[2] = (w(b.tacc[2]) - (w(a[0]) * w(t[1]) - w(a[1]) * w(t[0]))) as f32;
    }

    // Gravity (0x5b88cd..0x5b8a41).
    for i in 0..nb {
        let b = at(bodies, island, i);
        if b.flags & NO_GRAVITY == 0 {
            for j in 0..3 {
                b.facc[j] =
                    (b.mass.mass as f64 * params.gravity[j] as f64 + b.facc[j] as f64) as f32;
            }
        }
    }

    // Every joint's row count; IW keeps the zero-row ones in the list (0x5b8a70).
    let mut info = Vec::with_capacity(jlist.len());
    let mut ofs = Vec::with_capacity(jlist.len());
    let mut m = 0usize;
    for &jid in jlist {
        let joint = joints[jid.0 as usize].as_mut().expect("island joint");
        let i1 = joint.info1(bodies);
        ofs.push(m);
        m += i1.m;
        info.push(i1);
    }

    let mut cforce = vec![[[0.0f32; 3]; 2]; nb];
    if m > 0 {
        let mut rows = vec![
            Row {
                lo: -INFINITY,
                hi: INFINITY,
                ..Row::default()
            };
            m
        ];
        let mut c = vec![0.0f32; m];
        let mut cfm = vec![params.cfm; m];
        let mut findex = vec![-1i32; m];
        for (k, &jid) in jlist.iter().enumerate() {
            let n = info[k].m;
            if n == 0 {
                continue;
            }
            let o = ofs[k];
            let joint = joints[jid.0 as usize].as_mut().expect("island joint");
            let mut info = Info2 {
                fps: s1,
                erp: params.erp,
                rows: &mut rows[o..o + n],
                c: &mut c[o..o + n],
                cfm: &mut cfm[o..o + n],
                findex: &mut findex[o..o + n],
                torque: Vec::new(),
            };
            joint.info2(bodies, params, &mut info);
            for (end, t) in info.torque {
                let id = joint.body[end].expect("torqued end");
                bodies[id.0 as usize]
                    .as_mut()
                    .expect("live body")
                    .add_torque(t);
            }
            for f in &mut findex[o..o + n] {
                if *f >= 0 {
                    *f += o as i32;
                }
            }
        }
        for (k, &jid) in jlist.iter().enumerate() {
            let joint = joints[jid.0 as usize].as_ref().expect("island joint");
            for row in &mut rows[ofs[k]..ofs[k] + info[k].m] {
                row.b1 = joint.index[0];
                row.b2 = joint.index[1];
            }
        }

        // `v / h + inv(M) fe` per body (0x5b8c9c..0x5b8d9b).
        let mut tmp1 = Vec::with_capacity(nb);
        for i in 0..nb {
            let b = at(bodies, island, i);
            let im = b.inv_mass as f64;
            let l: V3 =
                std::array::from_fn(|j| (b.facc[j] as f64 * im + b.lvel[j] as f64 * s1w) as f32);
            let t = mul0_331(&inv_i[i], b.tacc);
            let a: V3 = std::array::from_fn(|j| (b.avel[j] as f64 * s1w + t[j] as f64) as f32);
            tmp1.push([l, a]);
        }
        for (i, row) in rows.iter_mut().enumerate() {
            let v = [
                tmp1[row.b1 as usize],
                if row.b2 >= 0 {
                    tmp1[row.b2 as usize]
                } else {
                    [[0.0; 3]; 2]
                },
            ];
            let rhs = row_dot([row.j1l, row.j1a, row.j2l, row.j2a], v, row.b2 >= 0);
            row.rhs = (c[i] as f64 * s1w - rhs as f64) as f32;
            cfm[i] = (cfm[i] as f64 * s1w) as f32;
        }

        let inv_mass: Vec<f32> = (0..nb).map(|i| at(bodies, island, i).inv_mass).collect();
        sor_lcp(
            &mut rows,
            &inv_mass,
            &inv_i,
            &cfm,
            &findex,
            params.qs_iterations,
            params.qs_w,
            seed,
            &mut cforce,
        );

        for (i, cf) in cforce.iter().enumerate() {
            let b = at(bodies, island, i);
            for j in 0..3 {
                b.lvel[j] = (cf[0][j] as f64 * hw + b.lvel[j] as f64) as f32;
                b.avel[j] = (cf[1][j] as f64 * hw + b.avel[j] as f64) as f32;
            }
        }
    }

    // The external forces (0x5b9158..0x5b9284).
    for (i, ii) in inv_i.iter().enumerate() {
        let b = at(bodies, island, i);
        let k = b.inv_mass as f64 * hw;
        for j in 0..3 {
            b.lvel[j] = (b.facc[j] as f64 * k + b.lvel[j] as f64) as f32;
        }
        for j in 0..3 {
            b.tacc[j] = (b.tacc[j] as f64 * hw) as f32;
        }
        for j in 0..3 {
            b.avel[j] = (dot(ii[j], b.tacc) as f64 + b.avel[j] as f64) as f32;
        }
    }

    // IW's clamp (0x5b9294..0x5b93ab).
    for i in 0..nb {
        let b = at(bodies, island, i);
        clamp(&mut b.avel, MAX_ANGULAR2, CLAMP_ANGULAR);
        clamp(&mut b.lvel, MAX_LINEAR2, CLAMP_LINEAR);
    }

    for i in 0..nb {
        let b = at(bodies, island, i);
        b.step(h);
        b.flags &= !DISABLED;
    }
    for &jid in jlist {
        joints[jid.0 as usize].as_mut().expect("island joint").tag = 1;
    }
}

/// Scales `v` to `to` when its squared length passes `max2`.
fn clamp(v: &mut V3, max2: f64, to: f64) {
    let l2 = dot(*v, *v);
    if (l2 as f64) > max2 {
        let len = (l2 as f64).sqrt() as f32;
        let k = (to / len as f64) as f32;
        for x in v.iter_mut() {
            *x = (*x as f64 * k as f64) as f32;
        }
    }
}

/// `SOR_LCP` (0x5b8080): solves the rows for `lambda` and returns
/// `inv(M) J' lambda` per body in `fc`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sor_lcp(
    rows: &mut [Row],
    inv_mass: &[f32],
    inv_i: &[M3],
    cfm: &[f32],
    findex: &[i32],
    iterations: u32,
    w: f32,
    seed: &mut u32,
    fc: &mut [[V3; 2]],
) {
    let m = rows.len();
    // `inv(M) J'` (0x5b78c0).
    for r in rows.iter_mut() {
        let k = inv_mass[r.b1 as usize] as f64;
        r.imj1l = r.j1l.map(|x| (k * x as f64) as f32);
        r.imj1a = mul0_331(&inv_i[r.b1 as usize], r.j1a);
        if r.b2 >= 0 {
            let k = inv_mass[r.b2 as usize] as f64;
            r.imj2l = r.j2l.map(|x| (k * x as f64) as f32);
            r.imj2a = mul0_331(&inv_i[r.b2 as usize], r.j2a);
        }
    }
    for f in fc.iter_mut() {
        *f = [[0.0; 3]; 2];
    }

    // `Ad = w / (J inv(M) J' + cfm)` (0x5b80db..0x5b818f).
    for (i, r) in rows.iter_mut().enumerate() {
        let mut s = (dot(r.j1l, r.imj1l) as f64 + 0.0) as f32;
        s = (dot(r.j1a, r.imj1a) as f64 + s as f64) as f32;
        if r.b2 >= 0 {
            s = (dot(r.j2l, r.imj2l) as f64 + s as f64) as f32;
            s = (dot(r.j2a, r.imj2a) as f64 + s as f64) as f32;
        }
        r.ad = (w as f64 / (cfm[i] as f64 + s as f64)) as f32;
    }
    // J and b scaled by Ad, then Ad by CFM (0x5b81b0..0x5b8291).
    for (i, r) in rows.iter_mut().enumerate() {
        let ad = r.ad as f64;
        for v in [&mut r.j1l, &mut r.j1a, &mut r.j2l, &mut r.j2a] {
            *v = v.map(|x| (x as f64 * ad) as f32);
        }
        r.rhs = (r.rhs as f64 * ad) as f32;
        r.ad = (cfm[i] as f64 * ad) as f32;
    }

    // Rows without a friction index first, in order; the rest from the
    // back, so in reverse (0x5b8297..0x5b82cb).
    let mut order = vec![0usize; m];
    let (mut front, mut back) = (0, m);
    for (i, &f) in findex.iter().enumerate() {
        if f < 0 {
            order[front] = i;
            front += 1;
        } else {
            back -= 1;
            order[back] = i;
        }
    }

    let one_body = fc.len() == 1;
    for iteration in 0..iterations {
        if iteration & 7 == 7 {
            reorder(&mut order, seed);
        }
        for &index in &order {
            let (lo, hi) = match findex[index] {
                f if f >= 0 => {
                    let hi =
                        ((rows[f as usize].lambda as f64 * rows[index].hi as f64) as f32).abs();
                    (-hi, hi)
                }
                _ => (rows[index].lo, rows[index].hi),
            };
            let r = &mut rows[index];
            // 0x5b7be6..0x5b7cad: each partial dot stored, angular block
            // first, then folded into delta and stored.
            let mut delta = (r.rhs as f64 - r.ad as f64 * r.lambda as f64) as f32;
            let f1 = fc[r.b1 as usize];
            let ang = dot(f1[1], r.j1a);
            let lin = dot(f1[0], r.j1l);
            delta = (delta as f64 - (ang as f64 + lin as f64)) as f32;
            let two = r.b2 >= 0 && !one_body;
            if two {
                let f2 = fc[r.b2 as usize];
                let ang = dot(f2[1], r.j2a);
                let lin = dot(f2[0], r.j2l);
                delta = (delta as f64 - (ang as f64 + lin as f64)) as f32;
            }
            let new = (r.lambda as f64 + delta as f64) as f32;
            // 0x5b7cad..0x5b7d12: `new - hi` and `lo - new` stored and
            // compared with 0; `lo` wins when both hold.
            let mut v = if (new as f64 - hi as f64) as f32 >= 0.0 {
                hi
            } else {
                new
            };
            if (lo as f64 - new as f64) as f32 >= 0.0 {
                v = lo;
            }
            let delta = (v as f64 - r.lambda as f64) as f32;
            r.lambda = v;
            let d = delta as f64;
            let add = |f: &mut V3, imj: V3| {
                for j in 0..3 {
                    f[j] = (imj[j] as f64 * d + f[j] as f64) as f32;
                }
            };
            let b1 = r.b1 as usize;
            add(&mut fc[b1][0], r.imj1l);
            add(&mut fc[b1][1], r.imj1a);
            if two {
                let b2 = r.b2 as usize;
                add(&mut fc[b2][0], r.imj2l);
                add(&mut fc[b2][1], r.imj2a);
            }
        }
    }
}
