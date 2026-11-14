//! 1.7's DObj skeleton for a model with no anim tree: the base locals
//! `DObjCalcAnim` writes (`cod4_lnxded` 0x81b4c8c), the locals a
//! controller sets (0x81b00de), and `DObjCalcSkel`'s composition of them
//! into model-space bones (0x81af4f8, 0x81ae164), each float rounded where
//! the binary stores it. docs/research/cod4-gsc.md 3.2, "The skeleton".
//! `iw3mp.exe`'s `DObjCalcSkel` runs the same sums in the same order with
//! every result a float ([`Cod4Arith::Client`], cod4-player-anims.md 8).

use crate::assets::model::XModel;
use std::ops::{Add, Mul, Neg, Sub};
use vcod_common::playerpose::{Cod4Arith, Controllers};
use vcod_common::skeleton::{PoseBuffer, Skeleton};

/// `DObjAnimMat` (0x20): an unnormalised quat, a translation and the
/// `2 / |quat|^2` its matrix is built with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimMat {
    pub quat: [f32; 4],
    pub trans: [f32; 3],
    pub trans_weight: f32,
}

impl AnimMat {
    /// A root's local with no anim: the identity quat, no shift, weight 0
    /// (0x81b4dc0..0x81b4df3).
    pub const IDENTITY: AnimMat = AnimMat {
        quat: [0.0, 0.0, 0.0, 1.0],
        trans: [0.0; 3],
        trans_weight: 0.0,
    };

    /// The bone's rotation rows as `G_DObjGetWorldTagMatrix` builds them
    /// (0x80cbef3..0x80cbf8a): each component times the weight kept on
    /// the stack, `xx` and `yy` stored, the diagonals' sums stored before
    /// the `1 -`.
    pub fn axis(&self) -> [[f32; 3]; 3] {
        let [x, y, z, w] = self.quat.map(d);
        let t = d(self.trans_weight);
        let (xt, yt, zt) = (x * t, y * t, z * t);
        let xx = f(x * xt);
        let yy = f(y * yt);
        let (xy, xz, xw) = (xt * y, xt * z, xt * w);
        let (yz, yw) = (yt * z, yt * w);
        let (zz, zw) = (z * zt, w * zt);
        [
            [f(1.0 - d(f(d(yy) + zz))), f(xy + zw), f(xz - yw)],
            [f(xy - zw), f(1.0 - (zz + d(xx))), f(yz + xw)],
            [f(xz + yw), f(yz - xw), f(1.0 - d(f(d(xx) + d(yy))))],
        ]
    }
}

impl AnimMat {
    /// The rows `DObjTraceline` builds for the struck bone's face normal
    /// (0x81ad4e6..0x81ad5aa): [`Self::axis`]'s, but `xy` is stored and
    /// the first row's `yy + zz` kept on the stack.
    pub fn trace_axis(&self) -> [[f32; 3]; 3] {
        let [x, y, z, w] = self.quat.map(d);
        let t = d(self.trans_weight);
        let (xt, yt, zt) = (x * t, y * t, z * t);
        let xx = f(x * xt);
        let xy = d(f(y * xt));
        let yy = f(y * yt);
        let (xz, xw) = (z * xt, w * xt);
        let (yz, yw) = (z * yt, w * yt);
        let (zz, zw) = (z * zt, w * zt);
        [
            [f(1.0 - (d(yy) + zz)), f(xy + zw), f(xz - yw)],
            [f(xy - zw), f(1.0 - (zz + d(xx))), f(yz + xw)],
            [f(xz + yw), f(yz - xw), f(1.0 - d(f(d(xx) + d(yy))))],
        ]
    }
}

/// A float on the x87 stack (an `f64` here).
fn d(x: f32) -> f64 {
    f64::from(x)
}

/// A store to a float.
fn f(x: f64) -> f32 {
    x as f32
}

/// A value on the x87 stack: `cod4_lnxded` keeps its 64-bit mantissa (an
/// `f64` here); `iw3mp.exe` runs the x87 at single precision, Direct3D 9's
/// default, so every result there is a float (`single`).
#[derive(Clone, Copy)]
struct X {
    v: f64,
    single: bool,
}

impl X {
    fn of(v: f32, a: Cod4Arith) -> X {
        X {
            v: d(v),
            single: a == Cod4Arith::Client,
        }
    }

    fn quat(q: [f32; 4], a: Cod4Arith) -> [X; 4] {
        q.map(|v| X::of(v, a))
    }

    fn with(self, v: f64) -> X {
        X {
            v: if self.single { d(f(v)) } else { v },
            single: self.single,
        }
    }

    /// The value stored to a float and loaded back.
    fn st(self) -> X {
        X {
            v: d(f(self.v)),
            single: self.single,
        }
    }

    fn f(self) -> f32 {
        f(self.v)
    }
}

impl Add for X {
    type Output = X;
    fn add(self, o: X) -> X {
        self.with(self.v + o.v)
    }
}

impl Sub for X {
    type Output = X;
    fn sub(self, o: X) -> X {
        self.with(self.v - o.v)
    }
}

impl Mul for X {
    type Output = X;
    fn mul(self, o: X) -> X {
        self.with(self.v * o.v)
    }
}

impl Neg for X {
    type Output = X;
    fn neg(self) -> X {
        self.with(-self.v)
    }
}

impl Sub<X> for f64 {
    type Output = X;
    fn sub(self, o: X) -> X {
        o.with(self - o.v)
    }
}

/// `x`'s `baseMat`, one per bone it keeps.
pub fn base_mats(x: &XModel) -> Vec<AnimMat> {
    x.base_mat
        .as_chunks::<32>()
        .0
        .iter()
        .map(|m| {
            let w =
                |k: usize| f32::from_le_bytes([m[4 * k], m[4 * k + 1], m[4 * k + 2], m[4 * k + 3]]);
            AnimMat {
                quat: [w(0), w(1), w(2), w(3)],
                trans: [w(4), w(5), w(6)],
                trans_weight: w(7),
            }
        })
        .collect()
}

/// `1 / 32767` as the float 1.7 scales an `XModelQuat`'s shorts by
/// (0x823ba10, 0x38000100).
const QUAT_SCALE: f32 = f32::from_bits(0x3800_0100);

/// The double `M_PI / 360` of `AnglesToQuat` (0x8238538).
const HALF_DEG2RAD: f64 = 0.008_726_646_259_971_648;

/// `iw3mp.exe`'s, the float `M_PI / 360` widened (0x70b298).
const HALF_DEG2RAD_CLIENT: f64 = 0.008_726_646_192_371_845;

/// `AnglesToQuat` (0x81b0008): each half angle stored as a float through
/// `sinf` and `cosf`; the roll's cosine kept on the stack. Pitch turns
/// about y, yaw about z, roll about x.
pub fn angles_to_quat(angles: [f32; 3]) -> [f32; 4] {
    angles_to_quat_in(angles, Cod4Arith::Server)
}

/// [`angles_to_quat`] in `a`'s arithmetic: `iw3mp.exe`'s (0x580a40) takes
/// its half angles off a float `M_PI / 360` through `fsincos` and stores
/// the four products before the sums.
pub fn angles_to_quat_in(angles: [f32; 3], a: Cod4Arith) -> [f32; 4] {
    let k = match a {
        Cod4Arith::Server => HALF_DEG2RAD,
        Cod4Arith::Client => HALF_DEG2RAD_CLIENT,
    };
    let sc = |deg: f32| {
        let r = f(k * d(deg));
        (X::of(r.sin(), a), X::of(r.cos(), a))
    };
    let (sy, cy) = sc(angles[1]);
    let (sp, cp) = sc(angles[0]);
    let (sr, cr) = sc(angles[2]);
    let nspsy = -sp * sy;
    let cysp = cy * sp;
    let sycp = sy * cp;
    let cycp = cy * cp;
    [
        (sr * cycp + cr * nspsy).f(),
        (cr * cysp + sr * sycp).f(),
        (sycp * cr - sr * cysp).f(),
        (cr * cycp - sr * nspsy).f(),
    ]
}

/// What `DObjCalcAnim` and `DObjCalcSkel` read of one xmodel.
#[derive(Clone, Debug)]
pub struct SkelModel {
    /// `numRootBones`: the first bones, which have no parent.
    pub roots: usize,
    /// Each non-root bone's parent, its own index less `parentList`'s
    /// offset.
    pub parents: Vec<usize>,
    /// Each non-root bone's `XModelQuat`.
    pub quats: Vec<[i16; 4]>,
    /// Each non-root bone's translation off its parent (`trans`, 12 bytes
    /// a bone in 1.7's reads at 0x81ae3de).
    pub trans: Vec<[f32; 3]>,
}

impl SkelModel {
    /// `x`'s arrays; `None` when the walk did not keep them whole.
    pub fn of(x: &XModel) -> Option<SkelModel> {
        let n = usize::from(x.num_bones);
        let roots = usize::from(x.num_root_bones);
        let k = n.checked_sub(roots)?;
        if x.parent_list.len() != k || x.quats.len() < 8 * k || x.trans.len() < 12 * k {
            return None;
        }
        let mut parents = Vec::with_capacity(k);
        for (j, &off) in x.parent_list.iter().enumerate() {
            let off = usize::from(off);
            let bone = roots + j;
            if off == 0 || off > bone {
                return None;
            }
            parents.push(bone - off);
        }
        let i16_at = |o: usize| i16::from_le_bytes([x.quats[o], x.quats[o + 1]]);
        let f32_at = |o: usize| {
            f32::from_le_bytes([x.trans[o], x.trans[o + 1], x.trans[o + 2], x.trans[o + 3]])
        };
        Some(SkelModel {
            roots,
            parents,
            quats: (0..k)
                .map(|j| [0, 1, 2, 3].map(|c| i16_at(8 * j + 2 * c)))
                .collect(),
            trans: (0..k)
                .map(|j| [0, 1, 2].map(|c| f32_at(12 * j + 4 * c)))
                .collect(),
        })
    }

    pub fn bones(&self) -> usize {
        self.roots + self.parents.len()
    }

    /// `DObjCalcAnim` with no tree (0x81b4d80..0x81b4ecc): a root the
    /// identity, a non-root its quat's shorts each times [`QUAT_SCALE`]
    /// stored, no shift (`DObjCalcSkel` adds the model's), weight 0.
    pub fn base_locals(&self) -> Vec<AnimMat> {
        let mut out = vec![AnimMat::IDENTITY; self.roots];
        out.extend(self.quats.iter().map(|q| AnimMat {
            quat: q.map(|s| f(d(f32::from(s)) * d(QUAT_SCALE))),
            trans: [0.0; 3],
            trans_weight: 0.0,
        }));
        out
    }

    /// `DObjCalcSkel` over every bone of `mats`, the locals in, the bones
    /// in model space out: a root keeps its local and takes its weight
    /// (0x81af7ab..0x81af7da), each other bone in order is its parent's
    /// world times its local ([`compose`]).
    pub fn calc_skel(&self, mats: &mut [AnimMat]) {
        for m in mats.iter_mut().take(self.roots) {
            m.trans_weight = root_weight(&mut m.quat);
        }
        for (j, &p) in self.parents.iter().enumerate() {
            let b = self.roots + j;
            if b >= mats.len() {
                break;
            }
            mats[b] = compose(&mats[p], &mats[b], self.trans[j]);
        }
    }

    /// The model's bones with no anim and no controller: the base locals
    /// composed.
    pub fn base_pose(&self) -> Vec<AnimMat> {
        let mut mats = self.base_locals();
        self.calc_skel(&mut mats);
        mats
    }
}

/// One bone's local as the sampler or a controller left it.
#[derive(Clone, Copy, Debug)]
pub struct PosedLocal {
    pub quat: [f32; 4],
    /// The local translation, the model's own (`bind`) included.
    pub trans: [f32; 3],
    /// Set by a control angle (0x81b01e8): `DObjCalcSkel` turns it in bone
    /// 0's frame ([`compose_controlled`]).
    pub control: bool,
}

/// `DObjCalcSkel` over a pose another sampler left: each bone's parent
/// (-1 a root, a parent ahead of its child), its local, and the model's own
/// translation off the parent (`bind`), which the local's includes. A root
/// keeps its local; each other bone composes its local less `bind` with
/// `bind` added back ([`compose`], [`compose_controlled`] for a control
/// bone), as 1.7 adds the model's `trans` to the anim's; all in `a`'s
/// arithmetic (`iw3mp.exe`'s 0x57f550 for the roots, 0x57fbd0 for the rest).
pub fn calc_posed(
    parents: &[i32],
    locals: &[PosedLocal],
    binds: &[[f32; 3]],
    a: Cod4Arith,
) -> Vec<AnimMat> {
    let mut out: Vec<AnimMat> = Vec::with_capacity(locals.len());
    for (b, l) in locals.iter().enumerate() {
        let parent = parents
            .get(b)
            .and_then(|&p| usize::try_from(p).ok())
            .filter(|&p| p < b);
        let m = match parent {
            None => {
                let mut m = AnimMat {
                    quat: l.quat,
                    trans: l.trans,
                    trans_weight: 0.0,
                };
                m.trans_weight = root_weight_in(&mut m.quat, a);
                m
            }
            Some(p) => {
                let bind = binds.get(b).copied().unwrap_or_default();
                let local = AnimMat {
                    quat: l.quat,
                    trans: [0, 1, 2].map(|k| l.trans[k] - bind[k]),
                    trans_weight: 0.0,
                };
                match l.control {
                    true => compose_controlled_in(&out[0], &out[p], &local, bind, a),
                    false => compose_in(&out[p], &local, bind, a),
                }
            }
        };
        out.push(m);
    }
    out
}

/// A player body's `DObjAnimMat`s: the locals `pose` left, `controls`'
/// bones set as 1.7's controller sets them ([`set_controls`]), composed by
/// [`calc_posed`] in `a`'s arithmetic. The server's hit poses and the
/// client's drawn bodies both take it.
pub fn player_mats(
    skel: &Skeleton,
    pose: &PoseBuffer,
    controls: Option<&Controllers>,
    a: Cod4Arith,
) -> Vec<AnimMat> {
    let bones = skel.bones();
    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
    let mut locals: Vec<PosedLocal> = (0..bones.len())
        .map(|b| {
            let (p, q) = pose.local(b);
            PosedLocal {
                quat: q.to_array(),
                trans: p.to_array(),
                control: false,
            }
        })
        .collect();
    let binds: Vec<[f32; 3]> = bones.iter().map(|b| b.local_pos.to_array()).collect();
    if let Some(c) = controls {
        set_controls(skel, &mut locals, &binds, c, a);
    }
    calc_posed(&parents, &locals, &binds, a)
}

/// Each bone's place and turn off its `DObjAnimMat`.
pub fn worlds_of(mats: &[AnimMat]) -> Vec<(glam::Vec3, glam::Quat)> {
    mats.iter()
        .map(|m| {
            (
                glam::Vec3::from(m.trans),
                glam::Quat::from_array(m.quat).normalize(),
            )
        })
        .collect()
}

/// The tags `BG_Player_DoControllers` sets (`cod4_lnxded` 0x8056b48,
/// `iw3mp.exe` 0x446280; cod4-player-anims.md 8): the spine's and head's
/// six control bones take their angles' quat (`AnglesToQuat`) and no shift
/// off the model's, marked to turn in bone 0's frame, in place of the
/// anim's local; `tag_origin` takes its angles' quat and translation.
fn set_controls(
    skel: &Skeleton,
    locals: &mut [PosedLocal],
    binds: &[[f32; 3]],
    c: &Controllers,
    a: Cod4Arith,
) {
    for (name, angles) in [
        ("pelvis", c.pelvis),
        ("back_low", c.back_low),
        ("back_mid", c.back_mid),
        ("back_up", c.back_up),
        ("neck", c.neck),
        ("head", c.head),
    ] {
        if let Some(b) = skel.bone_index(name) {
            locals[b] = PosedLocal {
                quat: angles_to_quat_in(angles, a),
                trans: binds[b],
                control: true,
            };
        }
    }
    if let Some(b) = skel.bone_index("tag_origin") {
        locals[b] = PosedLocal {
            quat: angles_to_quat_in(c.origin, a),
            trans: c.origin_trans.to_array(),
            control: false,
        };
    }
}

/// A root's weight (0x81af7ab..0x81af7da): `2 / |q|^2` summed `x, y, z,
/// w` on the stack; a zero quat becomes the identity's `w` and weight 2.
fn root_weight(q: &mut [f32; 4]) -> f32 {
    root_weight_in(q, Cod4Arith::Server)
}

/// [`root_weight`] in `a`'s arithmetic (`iw3mp.exe` 0x57f5e0..0x57f62e,
/// the sum stored before its test).
fn root_weight_in(q: &mut [f32; 4], a: Cod4Arith) -> f32 {
    let [x, y, z, w] = X::quat(*q, a);
    let s = ((x * x + y * y) + z * z) + w * w;
    if s.v == 0.0 {
        q[3] = 1.0;
        2.0
    } else {
        f(2.0 / s.v)
    }
}

/// One non-root bone of `DObjCalcSkel` (0x81ae2de..0x81ae54d): the quat
/// `parent * local` kept on the stack for the weight and stored; the
/// translation [`place`]s.
pub fn compose(parent: &AnimMat, local: &AnimMat, trans: [f32; 3]) -> AnimMat {
    compose_in(parent, local, trans, Cod4Arith::Server)
}

/// [`compose`] in `a`'s arithmetic (`iw3mp.exe` 0x57fbd0's first arm, the
/// same sums in the same order).
fn compose_in(parent: &AnimMat, local: &AnimMat, trans: [f32; 3], a: Cod4Arith) -> AnimMat {
    let [px, py, pz, pw] = X::quat(parent.quat, a);
    let [cx, cy, cz, cw] = X::quat(local.quat, a);
    let w = ((pw * cw - cx * px) - py * cy) - pz * cz;
    let x = ((cx * pw + cw * px) + py * cz) - pz * cy;
    let y = ((pw * cy - px * cz) + cw * py) + cx * pz;
    let z = ((cz * pw + px * cy) - cx * py) + cw * pz;
    let (quat, trans_weight) = weighted([x, y, z, w]);
    AnimMat {
        quat,
        trans: place(parent, local, trans, a),
        trans_weight,
    }
}

/// A control bone of `DObjCalcSkel` (its bit at skeleton +0x24, set by
/// 0x81b01e8; the arm at 0x81ae5da..0x81ae819): the control's quat `l`
/// turns the parent `p` in bone 0's frame `r`, `r * (l * (r^-1 * p))`,
/// each step's components stored as the binary stores them, the bone's own
/// anim dropped; the weight and translation as [`compose`]'s.
pub fn compose_controlled(
    root: &AnimMat,
    parent: &AnimMat,
    local: &AnimMat,
    trans: [f32; 3],
) -> AnimMat {
    compose_controlled_in(root, parent, local, trans, Cod4Arith::Server)
}

/// [`compose_controlled`] in `a`'s arithmetic. `iw3mp.exe`'s arm
/// (0x57fd4e..0x57ff15, the bit tested at 0x57fcb0) sums in the server's
/// order and stores each of the three quats whole.
pub fn compose_controlled_in(
    root: &AnimMat,
    parent: &AnimMat,
    local: &AnimMat,
    trans: [f32; 3],
    ar: Cod4Arith,
) -> AnimMat {
    let [px, py, pz, pw] = X::quat(parent.quat, ar);
    let [rx, ry, rz, rw] = X::quat(root.quat, ar);
    // `r^-1 * p`, each partial sum stored (0x81ae5da..0x81ae67f).
    let a = (((px * rw - pw * rx).st() - pz * ry).st() + py * rz).st();
    let b = (((((rw * py).st() + rx * pz).st()) - pw * ry).st() - px * rz).st();
    let c = (((pz * rw - rx * py).st() + px * ry).st() - pw * rz).st();
    let dw = (((rw * pw + px * rx) + ry * py) + pz * rz).st();
    // `l * m` (0x81ae682..0x81ae763): w and y stored, x and z kept.
    let [lx, ly, lz, lw] = X::quat(local.quat, ar);
    let e = ((((c * lw).st() + b * lx).st()) - a * ly).st();
    let tw = (dw * lw).st();
    let tw = (tw - a * lx).st();
    let tw = (tw - b * ly).st();
    let tw = (tw - c * lz).st();
    let tx = ((a * lw + dw * lx) + c * ly) - b * lz;
    let ty = (((b * lw - c * lx) + ly * dw) + lz * a).st();
    let tz = e + (dw * lz).st();
    // `r * t` (0x81ae766..0x81ae819), kept for the weight.
    let w = ((rw * tw - rx * tx) - ry * ty) - rz * tz;
    let x = ((rw * tx + tw * rx) + ry * tz) - ty * rz;
    let y = ((rw * ty - rx * tz) + tw * ry) + rz * tx;
    let z = ((rw * tz + rx * ty) - tx * ry) + tw * rz;
    let (quat, trans_weight) = weighted([x, y, z, w]);
    AnimMat {
        quat,
        trans: place(parent, local, trans, ar),
        trans_weight,
    }
}

/// A bone's quat stored and its weight `2 / |q|^2` off the kept sums
/// (0x81ae3a7..0x81ae3d3); a zero quat becomes `w` 1, weight 2 (0x81ae5bb).
fn weighted([x, y, z, w]: [X; 4]) -> ([f32; 4], f32) {
    let s = ((x * x + y * y) + z * z) + w * w;
    if s.v == 0.0 {
        ([x.f(), y.f(), z.f(), 1.0], 2.0)
    } else {
        ([x.f(), y.f(), z.f(), w.f()], f(2.0 / s.v))
    }
}

/// A bone's translation (0x81ae3de..0x81ae54d): the local's plus `trans`
/// (x and y stored, z kept) turned by the parent's weighted matrix and
/// added to the parent's, each row's partial sums stored as the binary
/// stores them.
fn place(parent: &AnimMat, local: &AnimMat, trans: [f32; 3], ar: Cod4Arith) -> [f32; 3] {
    let [px, py, pz, pw] = X::quat(parent.quat, ar);
    let [ltx, lty, ltz] = local.trans.map(|v| X::of(v, ar));
    let [mtx, mty, mtz] = trans.map(|v| X::of(v, ar));
    let lx = (ltx + mtx).st();
    let ly = (lty + mty).st();
    let lz = ltz + mtz;
    let t = X::of(parent.trans_weight, ar);
    // `t * px` kept on the stack for `xx`, stored for the rest.
    let a_kept = t * px;
    let a = a_kept.st();
    let twpy = t * py;
    let b = (t * pz).st();
    let xx = (a_kept * px).st();
    let xy = (a * py).st();
    let xz = a * pz;
    let xw = (a * pw).st();
    let yy = (py * twpy).st();
    let yz = (pz * twpy).st();
    let yw = (twpy * pw).st();
    let zz = b * pz;
    let zw = (pw * b).st();
    let [tx, ty, tz] = parent.trans.map(|v| X::of(v, ar));

    let x1 = ((1.0 - (yy + zz)) * lx).st();
    let x2 = (x1 + (xy - zw) * ly).st();
    let nx = ((x2 + (yw + xz) * lz) + tx).f();

    let y1 = (xy + zw) * lx + (1.0 - (xx + zz.st())) * ly;
    let ny = ((y1 + (yz - xw) * lz) + ty).f();

    let z1 = (xz - yw) * lx + (xw + yz) * ly;
    let nz = ((z1 + (1.0 - (xx + yy)) * lz) + tz).f();

    [nx, ny, nz]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quat from `angles_to_quat` turns as the axis 1.7 builds off it:
    /// yaw 90 takes x to y, pitch 90 takes x to -z.
    #[test]
    fn angles_turn_as_their_axis() {
        let m = |q: [f32; 4]| {
            let mut a = AnimMat {
                quat: q,
                ..AnimMat::IDENTITY
            };
            a.trans_weight = root_weight(&mut a.quat);
            a.axis()
        };
        let yaw = m(angles_to_quat([0.0, 90.0, 0.0]));
        assert!((yaw[0][1] - 1.0).abs() < 1e-6, "{yaw:?}");
        let pitch = m(angles_to_quat([90.0, 0.0, 0.0]));
        assert!((pitch[0][2] + 1.0).abs() < 1e-6, "{pitch:?}");
    }

    /// A child 10 along its parent's x, the parent yawed 90 and 5 up:
    /// the child stands at (0, 10, 5), turned with it.
    #[test]
    fn a_child_rides_its_parents_turn() {
        let mut parent = AnimMat {
            quat: angles_to_quat([0.0, 90.0, 0.0]),
            trans: [0.0, 0.0, 5.0],
            trans_weight: 0.0,
        };
        parent.trans_weight = root_weight(&mut parent.quat);
        let child = compose(&parent, &AnimMat::IDENTITY, [10.0, 0.0, 0.0]);
        let [x, y, z] = child.trans;
        assert!(x.abs() < 1e-5 && (y - 10.0).abs() < 1e-5 && (z - 5.0).abs() < 1e-6);
        assert!((child.axis()[0][1] - 1.0).abs() < 1e-6);
    }
}
