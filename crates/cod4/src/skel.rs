//! 1.7's DObj skeleton for a model with no anim tree: the base locals
//! `DObjCalcAnim` writes (`cod4_lnxded` 0x81b4c8c), the locals a
//! controller sets (0x81b00de), and `DObjCalcSkel`'s composition of them
//! into model-space bones (0x81af4f8, 0x81ae164), each float rounded where
//! the binary stores it. docs/research/cod4-gsc.md 3.2, "The skeleton".

use crate::assets::model::XModel;

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

/// A float on the x87 stack (an `f64` here).
fn d(x: f32) -> f64 {
    f64::from(x)
}

/// A store to a float.
fn f(x: f64) -> f32 {
    x as f32
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

/// `AnglesToQuat` (0x81b0008): each half angle stored as a float through
/// `sinf` and `cosf`; the roll's cosine kept on the stack. Pitch turns
/// about y, yaw about z, roll about x.
pub fn angles_to_quat(angles: [f32; 3]) -> [f32; 4] {
    let sc = |deg: f32| {
        let r = f(HALF_DEG2RAD * d(deg));
        (d(r.sin()), d(r.cos()))
    };
    let (sy, cy) = sc(angles[1]);
    let (sp, cp) = sc(angles[0]);
    let (sr, cr) = sc(angles[2]);
    let nspsy = -sp * sy;
    let cysp = cy * sp;
    let sycp = sy * cp;
    let cycp = cy * cp;
    [
        f(sr * cycp + cr * nspsy),
        f(cr * cysp + sr * sycp),
        f(sycp * cr - sr * cysp),
        f(cr * cycp - sr * nspsy),
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

/// `DObjCalcSkel` over a pose another sampler left: each bone's parent
/// (-1 a root, a parent ahead of its child), its local quat and
/// translation, and the model's own translation off the parent (`bind`),
/// which the local's includes. A root keeps its local; each other bone
/// composes its local less `bind` with `bind` added back ([`compose`]), as
/// 1.7 adds the model's `trans` to the anim's.
pub fn calc_posed(
    parents: &[i32],
    locals: &[([f32; 4], [f32; 3])],
    binds: &[[f32; 3]],
) -> Vec<AnimMat> {
    let mut out: Vec<AnimMat> = Vec::with_capacity(locals.len());
    for (b, &(quat, trans)) in locals.iter().enumerate() {
        let parent = parents
            .get(b)
            .and_then(|&p| usize::try_from(p).ok())
            .filter(|&p| p < b);
        let m = match parent {
            None => {
                let mut m = AnimMat {
                    quat,
                    trans,
                    trans_weight: 0.0,
                };
                m.trans_weight = root_weight(&mut m.quat);
                m
            }
            Some(p) => {
                let bind = binds.get(b).copied().unwrap_or_default();
                let local = AnimMat {
                    quat,
                    trans: [0, 1, 2].map(|k| trans[k] - bind[k]),
                    trans_weight: 0.0,
                };
                compose(&out[p], &local, bind)
            }
        };
        out.push(m);
    }
    out
}

/// A root's weight (0x81af7ab..0x81af7da): `2 / |q|^2` summed `x, y, z,
/// w` on the stack; a zero quat becomes the identity's `w` and weight 2.
fn root_weight(q: &mut [f32; 4]) -> f32 {
    let [x, y, z, w] = q.map(d);
    let s = ((x * x + y * y) + z * z) + w * w;
    if s == 0.0 {
        q[3] = 1.0;
        2.0
    } else {
        f(2.0 / s)
    }
}

/// One non-root bone of `DObjCalcSkel` (0x81ae2de..0x81ae54d): the quat
/// `parent * local` kept on the stack for the weight and stored; the
/// translation the local's plus `trans` (x and y stored, z kept) turned by
/// the parent's weighted matrix and added to the parent's, each row's
/// partial sums stored as the binary stores them.
pub fn compose(parent: &AnimMat, local: &AnimMat, trans: [f32; 3]) -> AnimMat {
    let [px, py, pz, pw] = parent.quat.map(d);
    let [cx, cy, cz, cw] = local.quat.map(d);
    let w = ((pw * cw - cx * px) - py * cy) - pz * cz;
    let x = ((cx * pw + cw * px) + py * cz) - pz * cy;
    let y = ((pw * cy - px * cz) + cw * py) + cx * pz;
    let z = ((cz * pw + px * cy) - cx * py) + cw * pz;
    let s = ((x * x + y * y) + z * z) + w * w;
    let (quat, trans_weight) = if s == 0.0 {
        ([f(x), f(y), f(z), 1.0], 2.0)
    } else {
        ([f(x), f(y), f(z), f(w)], f(2.0 / s))
    };

    let lx = d(f(d(local.trans[0]) + d(trans[0])));
    let ly = d(f(d(local.trans[1]) + d(trans[1])));
    let lz = d(local.trans[2]) + d(trans[2]);
    let t = d(parent.trans_weight);
    // `t * px` kept on the stack for `xx`, stored for the rest.
    let a_kept = t * px;
    let a = d(f(a_kept));
    let twpy = t * py;
    let b = d(f(t * pz));
    let xx = d(f(a_kept * px));
    let xy = d(f(a * py));
    let xz = a * pz;
    let xw = d(f(a * pw));
    let yy = d(f(py * twpy));
    let yz = d(f(pz * twpy));
    let yw = d(f(twpy * pw));
    let zz = b * pz;
    let zw = d(f(pw * b));
    let [tx, ty, tz] = parent.trans.map(d);

    let x1 = d(f((1.0 - (yy + zz)) * lx));
    let x2 = d(f(x1 + (xy - zw) * ly));
    let nx = f((x2 + (yw + xz) * lz) + tx);

    let y1 = (xy + zw) * lx + (1.0 - (xx + d(f(zz)))) * ly;
    let ny = f((y1 + (yz - xw) * lz) + ty);

    let z1 = (xz - yw) * lx + (xw + yz) * ly;
    let nz = f((z1 + (1.0 - (xx + yy)) * lz) + tz);

    AnimMat {
        quat,
        trans: [nx, ny, nz],
        trans_weight,
    }
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
