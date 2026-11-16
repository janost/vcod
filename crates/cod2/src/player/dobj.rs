//! `cod2_lnxded` 1.3's DObj pose, the bones its traces and tags read:
//! `DObjCalcAnim` (0x80c1260) walking the anim tree (0x80bfc04) into each
//! bone's `DObjAnimMat`, and `DObjCalcSkel` (0x80b9dfa) composing them,
//! each float rounded where the binary stores it, its x87 stack at the
//! 64-bit precision the process runs at ([`F80`]; cod2-player-anims.md 11).

use vcod_common::skeleton::Skeleton;
use vcod_common::x87::F80;
use vcod_common::xanim::XAnim;

/// `DObjAnimMat` (0x20 bytes): an unnormalized quat, a translation and a
/// weight; after the build the weight is `2 / |quat|^2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat {
    pub quat: [f32; 4],
    pub trans: [f32; 3],
    pub weight: f32,
}

impl Mat {
    pub const ZERO: Mat = Mat {
        quat: [0.0; 4],
        trans: [0.0; 3],
        weight: 0.0,
    };

    /// A bone no leaf reached (0x80c1260's tail): a root the identity, any
    /// other bone its model's quat shorts times [`SCALE`], no translation.
    pub fn base(quat: Option<[f32; 4]>) -> Mat {
        Mat {
            quat: quat.unwrap_or([0.0, 0.0, 0.0, 1.0]),
            ..Mat::ZERO
        }
    }

    /// The rotation rows 0x80ab4a0 builds off a built bone: `quat.xyz *
    /// weight` stored (0x80ab190), each product with the quat stored, each
    /// row element rounded once.
    pub fn axis(&self) -> [[f32; 3]; 3] {
        let [x, y, z, w] = self.quat.map(d);
        let [ax, ay, az] = [x, y, z].map(|c| d(st(c * d(self.weight))));
        let p = |a: F80, b: F80| d(st(a * b));
        let (xx, xy, xz, xw) = (p(ax, x), p(ax, y), p(ax, z), p(ax, w));
        let (yy, yz, yw) = (p(ay, y), p(ay, z), p(ay, w));
        let (zz, zw) = (p(az, z), p(az, w));
        [
            [st(ext(1.0) - (yy + zz)), st(xy + zw), st(xz - yw)],
            [st(xy - zw), st(ext(1.0) - (xx + zz)), st(yz + xw)],
            [st(xz + yw), st(yz - xw), st(ext(1.0) - (xx + yy))],
        ]
    }

    /// `v` in the bone's frame moved out to the model's (0x80bc39e): each
    /// component `((v.x * r0 + v.y * r1) + v.z * r2) + trans`, stored.
    pub fn place(&self, v: [f32; 3]) -> [f32; 3] {
        let m = self.axis();
        let [vx, vy, vz] = v.map(d);
        std::array::from_fn(|c| {
            st(((vx * d(m[0][c]) + vy * d(m[1][c])) + vz * d(m[2][c])) + d(self.trans[c]))
        })
    }
}

/// The float 1/32767 a quat short scales by (0x8150420, 0x8150018).
pub const SCALE: f32 = f32::from_bits(0x3800_0100);

fn d(x: f32) -> F80 {
    F80::from_f32(x)
}

fn ext(x: f64) -> F80 {
    F80::from_f64(x)
}

fn st(x: F80) -> f32 {
    x.to_f32()
}

/// A model's quat shorts as `DObjCalcAnim` gives an unreached bone
/// (0x80c3b90): each short times [`SCALE`], stored.
pub fn base_quat(shorts: [i16; 4]) -> [f32; 4] {
    shorts.map(|s| st(ext(f64::from(s)) * d(SCALE)))
}

/// A node of the anim tree as the walk meets it: a leaf's clip at its
/// node time and weight, or a node's weight and children in index order.
pub enum Tree<'a> {
    Leaf {
        clip: &'a XAnim,
        /// The clip's track to the skeleton's bone, `None` a bone the
        /// models lack.
        remap: &'a [Option<usize>],
        time: f32,
        weight: f32,
    },
    Node {
        weight: f32,
        children: Vec<Tree<'a>>,
    },
}

impl Tree<'_> {
    fn weight(&self) -> f32 {
        match self {
            Tree::Leaf { weight, .. } | Tree::Node { weight, .. } => *weight,
        }
    }
}

/// The frame a leaf samples (0x80bd0a6): `t * numframes` stored, and that
/// truncated.
struct Frame {
    last: bool,
    frame: f32,
    whole: i64,
}

impl Frame {
    fn new(t: f32, num_frames: u32) -> Frame {
        let frame = st(ext(f64::from(num_frames)) * d(t));
        Frame {
            last: t == 1.0 || num_frames == 0,
            frame,
            whole: frame as i64,
        }
    }

    /// The key at or below the frame and the stored fraction past it
    /// (0x80bd638, 0x80bd4c4: the key list's search off the truncated
    /// frame), or the last key alone at node time 1 (0x80bd20e).
    fn key(&self, frames: impl Fn(usize) -> u16, n: usize) -> (usize, Option<f32>) {
        if self.last || n == 1 {
            return (n - 1, None);
        }
        let mut i = 0;
        while i + 2 < n && i64::from(frames(i + 1)) <= self.whole {
            i += 1;
        }
        let (f0, f1) = (frames(i), frames(i + 1));
        let frac =
            st((d(self.frame) - ext(f64::from(f0)))
                / ext(f64::from(i32::from(f1) - i32::from(f0))));
        (i, Some(frac))
    }
}

/// A clip key's shorts with `w` rebuilt as CoD2's loader rebuilds it
/// ([`crate::xmodel::quat_w`]), on the sign the loader gave the key.
fn key([x, y, z, w]: [i16; 4]) -> [i16; 4] {
    let r = crate::xmodel::quat_w(x, y, z);
    [x, y, z, if w < 0 { -r } else { r }]
}

/// One leaf's sums into `mats` (0x80c2ffa under 256 frames, 0x80c336e at
/// or past it, 0x80bd20e at node time 1): per track its rotation, the
/// shorts lerped on the stack times `w * SCALE` stored and added; its
/// translation lerped times `w` and added; `w` added to the weight. Every
/// sum stored. A track with no rotation keys adds `w` to the quat's `w`.
fn leaf(clip: &XAnim, remap: &[Option<usize>], t: f32, w: f32, mats: &mut [Mat]) {
    let at = Frame::new(t, clip.frame_count.saturating_sub(1));
    let k = d(st(d(SCALE) * d(w)));
    let w = d(w);
    for (track, slot) in clip.tracks.iter().zip(remap) {
        let Some(m) = slot.and_then(|b| mats.get_mut(b)) else {
            continue;
        };
        match track.rot_raw.len() {
            0 => m.quat[3] = st(d(m.quat[3]) + w),
            n => {
                let (i, frac) = at.key(|i| track.rot_keys[i].0, n);
                let s0 = key(track.rot_raw[i]);
                for c in 0..4 {
                    let v = match frac {
                        Some(f) => {
                            let s1 = key(track.rot_raw[i + 1]);
                            ext(f64::from(s0[c]))
                                + ext(f64::from(i32::from(s1[c]) - i32::from(s0[c]))) * d(f)
                        }
                        None => ext(f64::from(s0[c])),
                    };
                    m.quat[c] = st(v * k + d(m.quat[c]));
                }
            }
        }
        if let n @ 1.. = track.trans_keys.len() {
            let (i, frac) = at.key(|i| track.trans_keys[i].0, n);
            let v0 = track.trans_keys[i].1;
            for c in 0..3 {
                let v = match frac {
                    Some(f) => {
                        let v1 = track.trans_keys[i + 1].1;
                        (d(v1[c]) - d(v0[c])) * d(f) + d(v0[c])
                    }
                    None => d(v0[c]),
                };
                m.trans[c] = st(v * w + d(m.trans[c]));
            }
        }
        m.weight = st(d(m.weight) + w);
    }
}

/// `DObjCalcAnim`'s walk from the root (weight 1, the bones cleared, its
/// children's sums divided by their weight): the locals of `bones` bones
/// and which a leaf reached.
pub fn calc(root: &Tree, bones: usize) -> (Vec<Mat>, Vec<bool>) {
    let mut mats = vec![Mat::ZERO; bones];
    let mut reached = vec![false; bones];
    walk(root, 1.0, true, false, &mut mats, &mut reached);
    (mats, reached)
}

/// 0x80bfc04: `node` at weight `w`; `clear` zeroes the bones first,
/// `nested` marks a child of a fold, whose own fold normalizes its quats
/// rather than dividing by its weight.
fn walk(node: &Tree, w: f32, clear: bool, nested: bool, mats: &mut [Mat], reached: &mut [bool]) {
    match node {
        Tree::Leaf {
            clip, remap, time, ..
        } => {
            if clear {
                mats.fill(Mat::ZERO);
            }
            for &b in remap.iter().flatten() {
                reached[b] = true;
            }
            leaf(clip, remap, *time, w, mats);
        }
        Tree::Node { children, .. } => {
            let live: Vec<&Tree> = children.iter().filter(|c| c.weight() != 0.0).collect();
            match live.len() {
                0 => {
                    if clear {
                        mats.fill(Mat::ZERO);
                    }
                }
                1 => walk(live[0], w, clear, nested, mats, reached),
                _ if clear => {
                    for (i, c) in live.iter().enumerate() {
                        walk(c, c.weight(), i == 0, true, mats, reached);
                    }
                    match nested {
                        false => divide(mats),
                        true => normalize(mats, w),
                    }
                }
                _ => {
                    let mut scratch = vec![Mat::ZERO; mats.len()];
                    for (i, c) in live.iter().enumerate() {
                        walk(c, c.weight(), i == 0, true, &mut scratch, reached);
                    }
                    add_normalized(mats, &scratch, w);
                }
            }
        }
    }
}

/// The root's fold: each bone of non-zero weight times `1 / weight`, the
/// factor passed as a float, each word stored; the weight stays.
fn divide(mats: &mut [Mat]) {
    for m in mats.iter_mut().filter(|m| m.weight != 0.0) {
        let r = d(st(ext(1.0) / d(m.weight)));
        m.quat = m.quat.map(|c| st(d(c) * r));
        m.trans = m.trans.map(|c| st(d(c) * r));
    }
}

/// `|q|^2` (0x80c38e6, `((x^2 + y^2) + z^2) + w^2` on the stack) stored,
/// and 0x80c34fe's `0x5f3759df` step off its bits with one Newton step
/// (0.5 at 0x815042c, 1.5 at 0x8150428), its half and result stored;
/// `None` for a zero quat.
fn rsqrt(q: [f32; 4]) -> Option<F80> {
    let [x, y, z, w] = q.map(d);
    let s = st(((x * x + y * y) + z * z) + w * w);
    if s == 0.0 {
        return None;
    }
    let h = d(st(d(s) * ext(0.5)));
    let y0 = d(f32::from_bits(0x5f37_59df - (s.to_bits() >> 1)));
    Some(d(st((ext(1.5) - (h * y0) * y0) * y0)))
}

/// A clearing child's fold in place: the quat times `rsqrt * w`, the
/// shift times `w / weight`, both factors passed as floats, the weight
/// then `w`.
fn normalize(mats: &mut [Mat], w: f32) {
    let w = d(w);
    for m in mats {
        if let Some(r) = rsqrt(m.quat) {
            let s = d(st(r * w));
            m.quat = m.quat.map(|c| st(d(c) * s));
        }
        if m.weight != 0.0 {
            let g = d(st(w / d(m.weight)));
            m.trans = m.trans.map(|c| st(d(c) * g));
            m.weight = st(w);
        }
    }
}

/// A later child's fold out of scratch: its quat times `rsqrt * w` and its
/// shift times `w / weight` added, `w` added to the weight.
fn add_normalized(mats: &mut [Mat], src: &[Mat], w: f32) {
    let w = d(w);
    for (m, s) in mats.iter_mut().zip(src) {
        if let Some(r) = rsqrt(s.quat) {
            let k = d(st(r * w));
            for c in 0..4 {
                m.quat[c] = st(k * d(s.quat[c]) + d(m.quat[c]));
            }
        }
        if s.weight != 0.0 {
            let g = d(st(w / d(s.weight)));
            for c in 0..3 {
                m.trans[c] = st(g * d(s.trans[c]) + d(m.trans[c]));
            }
            m.weight = st(w + d(m.weight));
        }
    }
}

/// `a * b` as 0x80bc452 composes a bone's quat onto its parent's (`a` the
/// bone's, `b` the parent's) and 0x80bc58a a control bone's (`a` the
/// parent's): each component's products and sums on the stack, stored.
fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a.map(d), b.map(d));
    [
        st(((ax * bw + aw * bx) + az * by) - ay * bz),
        st(((ay * bw - az * bx) + aw * by) + ax * bz),
        st(((az * bw + ay * bx) - ax * by) + aw * bz),
        st(((aw * bw - ax * bx) - ay * by) - az * bz),
    ]
}

/// 0x80bc34e: the weight `2 / |q|^2`, the square sum stored; a zero quat
/// becomes `w` 1 with weight 2.
fn weigh(m: &mut Mat) {
    let [x, y, z, w] = m.quat.map(d);
    let s = st(((x * x + y * y) + z * z) + w * w);
    if s == 0.0 {
        m.quat[3] = 1.0;
        m.weight = 2.0;
    } else {
        m.weight = st(ext(2.0) / d(s));
    }
}

/// `DObjCalcSkel` over `locals`, by `skel`'s bone (a later model's bone
/// that shares a name is the first's, the copy 0x80b9dfa makes): a root
/// keeps its local and takes its weight; any other bone's quat is its
/// local's times its parent's (`control`: the parent's times its own), its
/// translation the local's plus the model's bind offset, stored, moved
/// out through its parent ([`Mat::place`]).
pub fn build(skel: &Skeleton, locals: &[Mat], control: &[bool]) -> Vec<Mat> {
    let mut out: Vec<Mat> = Vec::with_capacity(locals.len());
    for (i, (b, l)) in skel.bones().iter().zip(locals).enumerate() {
        let mut m = *l;
        match usize::try_from(b.parent) {
            Err(_) => weigh(&mut m),
            Ok(p) => {
                let pm = out[p];
                m.quat = match control.get(i).copied().unwrap_or(false) {
                    true => mul(pm.quat, l.quat),
                    false => mul(l.quat, pm.quat),
                };
                weigh(&mut m);
                let bind = b.local_pos.to_array();
                let t: [f32; 3] = std::array::from_fn(|c| st(d(l.trans[c]) + d(bind[c])));
                m.trans = pm.place(t);
            }
        }
        out.push(m);
    }
    out
}

/// `angles` as a quat the way 0x80bbd20 sets a local tag: each angle times
/// the double pi / 360 (0x814ff70) stored, its libm sine and cosine stored
/// (0x80bc046), the pitch and yaw products stored, then each component
/// summed off the roll's, stored.
pub fn angles_quat([pitch, yaw, roll]: [f32; 3]) -> [f32; 4] {
    let half = |a: f32| {
        let r = f64::from(st(d(a) * ext(std::f64::consts::PI / 360.0)));
        (d(r.sin() as f32), d(r.cos() as f32))
    };
    let (ys, yc) = half(yaw);
    let (ps, pc) = half(pitch);
    let (rs, rc) = half(roll);
    let t0 = d(st(-ps * ys));
    let t1 = d(st(ps * yc));
    let t2 = d(st(pc * ys));
    let t3 = d(st(pc * yc));
    [
        st(rs * t3 + rc * t0),
        st(rc * t1 + rs * t2),
        st(-rs * t1 + rc * t2),
        st(rc * t3 - rs * t0),
    ]
}

/// A local tag's `DObjAnimMat` (0x80bbd20): its angles' quat, the
/// translation given, weight 0.
pub fn tag(angles: [f32; 3], trans: [f32; 3]) -> Mat {
    Mat {
        quat: angles_quat(angles),
        trans,
        weight: 0.0,
    }
}
