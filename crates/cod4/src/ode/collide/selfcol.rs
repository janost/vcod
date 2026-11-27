//! A ragdoll's bones against each other (iw3mp.exe 1.7's self collision
//! hook 0x5bd9c0): per `ragdoll_selfpair`, the nearest points of the two
//! bones' segments (0x5bd4e0) closer than their radii times
//! `ragdoll_self_collision_scale` make one contact (0x5bd890, 0x5a2b50);
//! cod4-player-anims.md 10.6.

use super::{BOUNCE_VEL, ContactGeom, Dvars, MODE, Obj, contact_pass};
use crate::ode::Body;
use crate::ode::joint::contact::Surface;
use crate::ode::math::{V3, add, dot, mul0_331, sub};
use crate::ode::world::{BodyId, World};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// One bone of a self pair: its body, segment, `ragdoll_bone` radius
/// (def bone +0x30) and friction (+0x40).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bone {
    pub body: BodyId,
    pub seg: [V3; 2],
    pub radius: f32,
    pub friction: f32,
}

/// 0x5bd800: a bone's segment, from the body's position moved by
/// `offset` in the body's frame (0x5a4740) along the body's x axis for
/// `length`.
pub fn bone_segment(b: &Body, offset: V3, length: f32) -> [V3; 2] {
    let p0 = add(b.pos, mul0_331(&b.r, offset));
    let col = [b.r[0][0], b.r[1][0], b.r[2][0]];
    [
        p0,
        std::array::from_fn(|k| (w(col[k]) * w(length) + w(p0[k])) as f32),
    ]
}

/// 0x5bd4e0: the parameters `(s, t)` of the nearest points of segments
/// `a` and `b`, Dan Sunday's clamped solve: parallel under a determinant
/// of 1e-6 (the double at 0x70b508), and a numerator under 1e-4 in size
/// (0x70b000) reads 0.
pub fn closest(a: [V3; 2], b: [V3; 2]) -> (f32, f32) {
    let u = sub(a[1], a[0]);
    let v = sub(b[1], b[0]);
    let wv = sub(a[0], b[0]);
    let (aa, bb, cc) = (dot(u, u), dot(v, u), dot(v, v));
    let (dd, ee) = (dot(u, wv), dot(v, wv));
    let det = (w(cc) * w(aa) - w(bb) * w(bb)) as f32;
    let (mut sn, mut sd, mut tn, mut td);
    if w(det) < 1e-6 {
        (sn, sd, tn, td) = (0.0, 1.0, ee, cc);
    } else {
        (sd, td) = (det, det);
        sn = (w(ee) * w(bb) - w(dd) * w(cc)) as f32;
        tn = (w(ee) * w(aa) - w(dd) * w(bb)) as f32;
        if sn < 0.0 {
            sn = 0.0;
            tn = ee;
            td = cc;
        } else if sn > det {
            sn = det;
            tn = (w(ee) + w(bb)) as f32;
            td = cc;
        }
    }
    if tn < 0.0 {
        tn = 0.0;
        let md = -dd;
        if md < 0.0 {
            sn = 0.0;
        } else if md > aa {
            sn = sd;
        } else {
            sn = md;
            sd = aa;
        }
    } else if td < tn {
        tn = td;
        let bd = w(bb) - w(dd);
        if bd < 0.0 {
            sn = 0.0;
        } else if w(aa) < bd {
            sn = sd;
        } else {
            sn = bd as f32;
            sd = aa;
        }
    }
    let div = |n: f32, d: f32| {
        if w(n.abs()) >= 9.999_999_747_378_752e-5 {
            (w(n) / w(d)) as f32
        } else {
            0.0
        }
    };
    (div(sn, sd), div(tn, td))
}

/// One self pair as 0x5bd9c0 runs it: the nearest points `pa`, `pb`
/// (0x5bdab1), each radius times `ragdoll_self_collision_scale`; with the
/// points no farther apart than the two radii (0x5bdb99) and not within
/// `sqrt(0.002)` (0x70b070), one contact at the middle of the overlap
/// along `pb - pa`, half the overlap deep, `mu` bone `a`'s friction,
/// bounce 0, `phys_contact_erp` and `_cfm`, `b1` bone `b`'s body and `b2`
/// bone `a`'s, through the contact pass (0x5a2b50 into 0x5a2760).
// A NaN distance makes no contact, as the compare's flags do.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn self_pair(world: &mut World, a: &Bone, oa: &mut Obj, b: &Bone, ob: &mut Obj, dv: &Dvars) {
    let (s, t) = closest(a.seg, b.seg);
    let at = |g: [V3; 2], s: f32| -> V3 {
        std::array::from_fn(|k| ((w(g[1][k]) - w(g[0][k])) * w(s) + w(g[0][k])) as f32)
    };
    let (pa, pb) = (at(a.seg, s), at(b.seg, t));
    let d = sub(pb, pa);
    let ra = (w(a.radius) * w(dv.self_collision_scale)) as f32;
    let rb = (w(b.radius) * w(dv.self_collision_scale)) as f32;
    let len = w(dot(d, d)).sqrt() as f32;
    if !(w(len) <= w(ra) + w(rb)) {
        return;
    }
    if dot(d, d) < 0.002 {
        return;
    }
    let (n, len) = normalize(d);
    let depth = (((w(ra) + w(rb)) - w(len)) * 0.5) as f32;
    let back = (w(ra) - w(depth)) as f32;
    let pos = std::array::from_fn(|k| (w(n[k]) * w(back) + w(pa[k])) as f32);
    let surface = Surface {
        mode: MODE,
        mu: a.friction,
        mu2: 0.0,
        bounce: 0.0,
        bounce_vel: BOUNCE_VEL,
        soft_erp: dv.contact_erp,
        soft_cfm: dv.contact_cfm,
        ..Surface::default()
    };
    let c = ContactGeom {
        pos,
        normal: n,
        depth,
        surface: 0,
    };
    contact_pass(
        world,
        vec![c],
        [Some((b.body, ob)), Some((a.body, oa))],
        surface,
        None,
    );
}

/// 0x55f6e0: `v` over its stored length (1 when that is not over 0) and
/// the length.
fn normalize(v: V3) -> (V3, f32) {
    let l = w(dot(v, v)).sqrt() as f32;
    let k = if -l < 0.0 { l } else { 1.0 };
    let inv = (1.0 / w(k)) as f32;
    (v.map(|x| (w(x) * w(inv)) as f32), l)
}
