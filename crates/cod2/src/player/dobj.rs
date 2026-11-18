//! `cod2_lnxded` 1.3's DObj pose, the bones its traces and tags read:
//! `DObjCalcAnim` (0x80c1260) walking the anim tree (0x80bfc04) into each
//! bone's `DObjAnimMat`, and `DObjCalcSkel` (0x80b9dfa) composing them,
//! each float rounded where the binary stores it, its x87 stack at the
//! 64-bit precision the process runs at ([`F80`]; cod2-player-anims.md 11).

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
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
        exactly(|| self.axis_in::<Bounded>(), || self.axis_in::<F80>())
    }

    fn axis_in<T: Ar>(&self) -> [[f32; 3]; 3] {
        let (d, ext) = (d::<T>, ext::<T>);
        let [x, y, z, w] = self.quat.map(d);
        let [ax, ay, az] = [x, y, z].map(|c| d(st(c * d(self.weight))));
        let p = |a: T, b: T| d(st(a * b));
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
        exactly(|| self.place_in::<Bounded>(v), || self.place_in::<F80>(v))
    }

    fn place_in<T: Ar>(&self, v: [f32; 3]) -> [f32; 3] {
        self.place_by::<T>(&self.axis_in::<T>(), v)
    }

    /// [`Self::place`] through rows already built off this bone.
    fn place_by<T: Ar>(&self, m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        let d = d::<T>;
        let [vx, vy, vz] = v.map(d);
        std::array::from_fn(|c| {
            st(((vx * d(m[0][c]) + vy * d(m[1][c])) + vz * d(m[2][c])) + d(self.trans[c]))
        })
    }
}

/// The float 1/32767 a quat short scales by (0x8150420, 0x8150018).
pub const SCALE: f32 = f32::from_bits(0x3800_0100);

/// A value on the x87 stack: an [`F80`] at the 64-bit precision the
/// binary runs at, or the fast arm [`Bounded`].
trait Ar:
    Copy
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + std::ops::Mul<Output = Self>
    + std::ops::Div<Output = Self>
    + std::ops::Neg<Output = Self>
{
    fn of(v: f64) -> Self;
    /// A store to a float.
    fn st(self) -> f32;
    /// `fsqrt`.
    fn sqrt(self) -> Self;
    /// `self > o`, as `fucompp` tells it.
    fn gt(self, o: Self) -> bool;
}

impl Ar for F80 {
    fn of(v: f64) -> F80 {
        F80::from_f64(v)
    }

    fn st(self) -> f32 {
        self.to_f32()
    }

    fn sqrt(self) -> F80 {
        F80::sqrt(self)
    }

    fn gt(self, o: F80) -> bool {
        self > o
    }
}

/// The fast arm: an `f64` and a bound on how far both it and the 64-bit
/// stack's value can sit from the exact one (0 when it is exact). A store
/// whose bound leaves the float's rounding in doubt marks the pass, which
/// [`exactly`] then reruns on [`F80`]s, so every stored word is the one
/// the 64-bit stack gives.
#[derive(Clone, Copy)]
struct Bounded(f64, f64);

thread_local! {
    /// A store this pass could not prove.
    static DOUBT: Cell<bool> = const { Cell::new(false) };
}

static RERUNS: AtomicU64 = AtomicU64::new(0);

/// How many passes [`exactly`] reran on [`F80`]s.
pub fn reruns() -> u64 {
    RERUNS.load(Ordering::Relaxed)
}

/// `fast`, or `wide` when one of `fast`'s stores was in doubt.
fn exactly<R>(fast: impl Fn() -> R, wide: impl Fn() -> R) -> R {
    DOUBT.set(false);
    let r = fast();
    if !DOUBT.get() {
        return r;
    }
    RERUNS.fetch_add(1, Ordering::Relaxed);
    wide()
}

/// 2^-52: one rounding of either arm, relative, with room to spare.
const U: f64 = f64::EPSILON;

impl std::ops::Add for Bounded {
    type Output = Bounded;
    #[inline(always)]
    fn add(self, o: Bounded) -> Bounded {
        let (Bounded(a, ea), Bounded(b, eb)) = (self, o);
        let v = a + b;
        if ea == 0.0 && eb == 0.0 {
            // Knuth's two-sum: the add's own error, exactly.
            let bb = v - a;
            let err = (a - (v - bb)) + (b - bb);
            return Bounded(v, if err == 0.0 { 0.0 } else { v.abs() * U });
        }
        Bounded(v, (ea + eb) * (1.0 + U) + v.abs() * U)
    }
}

impl std::ops::Neg for Bounded {
    type Output = Bounded;
    #[inline(always)]
    fn neg(self) -> Bounded {
        Bounded(-self.0, self.1)
    }
}

impl std::ops::Sub for Bounded {
    type Output = Bounded;
    #[inline(always)]
    fn sub(self, o: Bounded) -> Bounded {
        self + -o
    }
}

impl std::ops::Mul for Bounded {
    type Output = Bounded;
    #[inline(always)]
    fn mul(self, o: Bounded) -> Bounded {
        let (Bounded(a, ea), Bounded(b, eb)) = (self, o);
        let v = a * b;
        if ea == 0.0 && eb == 0.0 {
            // Two floats' product fits a double's 53 bits; anything wider
            // takes its exact error off a fused multiply-add.
            let narrow = |x: f64| f64::from(x as f32) == x;
            if narrow(a) && narrow(b) {
                return Bounded(v, 0.0);
            }
            let err = a.mul_add(b, -v);
            return Bounded(v, if err == 0.0 { 0.0 } else { v.abs() * U });
        }
        Bounded(
            v,
            (a.abs() * eb + b.abs() * ea + ea * eb) * (1.0 + 4.0 * U) + v.abs() * U,
        )
    }
}

impl std::ops::Div for Bounded {
    type Output = Bounded;
    #[inline(always)]
    fn div(self, o: Bounded) -> Bounded {
        let (Bounded(a, ea), Bounded(b, eb)) = (self, o);
        let v = a / b;
        if ea == 0.0 && eb == 0.0 {
            let err = (-v).mul_add(b, a);
            return Bounded(v, if err == 0.0 { 0.0 } else { v.abs() * U });
        }
        if b.abs() <= 2.0 * eb {
            DOUBT.set(true);
            return Bounded(v, 0.0);
        }
        Bounded(
            v,
            (ea + v.abs() * eb) / (b.abs() - eb) * (1.0 + 4.0 * U) + v.abs() * U,
        )
    }
}

impl Ar for Bounded {
    #[inline(always)]
    fn of(v: f64) -> Bounded {
        Bounded(v, 0.0)
    }

    /// The rounding of every value both arms could hold, or a doubt. An
    /// exact value rounds alike on both.
    #[inline(always)]
    fn st(self) -> f32 {
        let Bounded(v, e) = self;
        let mid = v as f32;
        if e != 0.0 {
            let w = 2.0 * e + v.abs() * U;
            let (lo, hi) = ((v - w) as f32, (v + w) as f32);
            if lo.to_bits() != mid.to_bits() || hi.to_bits() != mid.to_bits() {
                DOUBT.set(true);
            }
        }
        mid
    }

    #[inline(always)]
    fn sqrt(self) -> Bounded {
        let Bounded(a, ea) = self;
        let v = a.sqrt();
        if ea == 0.0 {
            let err = (-v).mul_add(v, a);
            return Bounded(v, if err == 0.0 { 0.0 } else { v * U });
        }
        if ea * 4.0 >= a {
            DOUBT.set(true);
            return Bounded(v, 0.0);
        }
        // sqrt(a + e) - sqrt(a) is under e / sqrt(a) for e under a / 4.
        Bounded(v, ea / v * (1.0 + 4.0 * U) + v * U)
    }

    /// A difference inside both bounds leaves the order in doubt.
    #[inline(always)]
    fn gt(self, o: Bounded) -> bool {
        let (Bounded(a, ea), Bounded(b, eb)) = (self, o);
        if (ea != 0.0 || eb != 0.0) && (a - b).abs() <= 2.0 * (ea + eb) + (a.abs() + b.abs()) * U {
            DOUBT.set(true);
        }
        a > b
    }
}

#[inline(always)]
fn d<T: Ar>(x: f32) -> T {
    T::of(f64::from(x))
}

#[inline(always)]
fn ext<T: Ar>(x: f64) -> T {
    T::of(x)
}

#[inline(always)]
fn st<T: Ar>(x: T) -> f32 {
    x.st()
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
    fn new<T: Ar>(t: f32, num_frames: u32) -> Frame {
        let (d, ext) = (d::<T>, ext::<T>);
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
    fn key<T: Ar>(&self, frames: impl Fn(usize) -> u16, n: usize) -> (usize, Option<f32>) {
        let (d, ext) = (d::<T>, ext::<T>);
        if self.last || n == 1 {
            return (n - 1, None);
        }
        // The last key at or below the frame, short of the last one; the
        // keys ascend.
        let (mut i, mut hi) = (0, n - 2);
        while i < hi {
            let mid = (i + hi).div_ceil(2);
            match i64::from(frames(mid)) <= self.whole {
                true => i = mid,
                false => hi = mid - 1,
            }
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
fn leaf<T: Ar>(
    clip: &XAnim,
    remap: &[Option<usize>],
    t: f32,
    w: f32,
    mats: &mut [Mat],
    need: &[bool],
) {
    let (d, ext) = (d::<T>, ext::<T>);
    let at = Frame::new::<T>(t, clip.frame_count.saturating_sub(1));
    let k = d(st(d(SCALE) * d(w)));
    let w = d(w);
    for (track, slot) in clip.tracks.iter().zip(remap) {
        let Some(m) = slot.filter(|&b| need[b]).and_then(|b| mats.get_mut(b)) else {
            continue;
        };
        match track.rot_raw.len() {
            0 => m.quat[3] = st(d(m.quat[3]) + w),
            n => {
                let (i, frac) = at.key::<T>(|i| track.rot_keys[i].0, n);
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
            let (i, frac) = at.key::<T>(|i| track.trans_keys[i].0, n);
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
///
/// Only the bones `need` holds are summed and folded (all without it):
/// each bone's words are its own, so the rest read as retail's whatever is
/// skipped.
pub fn calc(root: &Tree, bones: usize, need: Option<&[bool]>) -> (Vec<Mat>, Vec<bool>) {
    let all = vec![true; bones];
    let need = need.unwrap_or(&all);
    exactly(
        || calc_in::<Bounded>(root, bones, need),
        || calc_in::<F80>(root, bones, need),
    )
}

fn calc_in<T: Ar>(root: &Tree, bones: usize, need: &[bool]) -> (Vec<Mat>, Vec<bool>) {
    let mut mats = vec![Mat::ZERO; bones];
    let mut reached = vec![false; bones];
    walk::<T>(root, 1.0, true, false, &mut mats, &mut reached, need);
    (mats, reached)
}

/// 0x80bfc04: `node` at weight `w`; `clear` zeroes the bones first,
/// `nested` marks a child of a fold, whose own fold normalizes its quats
/// rather than dividing by its weight.
fn walk<T: Ar>(
    node: &Tree,
    w: f32,
    clear: bool,
    nested: bool,
    mats: &mut [Mat],
    reached: &mut [bool],
    need: &[bool],
) {
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
            leaf::<T>(clip, remap, *time, w, mats, need);
        }
        Tree::Node { children, .. } => {
            let live: Vec<&Tree> = children.iter().filter(|c| c.weight() != 0.0).collect();
            match live.len() {
                0 => {
                    if clear {
                        mats.fill(Mat::ZERO);
                    }
                }
                1 => walk::<T>(live[0], w, clear, nested, mats, reached, need),
                _ if clear => {
                    for (i, c) in live.iter().enumerate() {
                        walk::<T>(c, c.weight(), i == 0, true, mats, reached, need);
                    }
                    match nested {
                        false => divide::<T>(mats, need),
                        true => normalize::<T>(mats, w, need),
                    }
                }
                _ => {
                    let mut scratch = vec![Mat::ZERO; mats.len()];
                    for (i, c) in live.iter().enumerate() {
                        walk::<T>(c, c.weight(), i == 0, true, &mut scratch, reached, need);
                    }
                    add_normalized::<T>(mats, &scratch, w, need);
                }
            }
        }
    }
}

/// The root's fold: each bone of non-zero weight times `1 / weight`, the
/// factor passed as a float, each word stored; the weight stays.
fn divide<T: Ar>(mats: &mut [Mat], need: &[bool]) {
    let (d, ext) = (d::<T>, ext::<T>);
    for m in mats
        .iter_mut()
        .zip(need)
        .filter_map(|(m, &n)| n.then_some(m))
        .filter(|m| m.weight != 0.0)
    {
        let r = d(st(ext(1.0) / d(m.weight)));
        m.quat = m.quat.map(|c| st(d(c) * r));
        m.trans = m.trans.map(|c| st(d(c) * r));
    }
}

/// `|q|^2` (0x80c38e6, `((x^2 + y^2) + z^2) + w^2` on the stack) stored,
/// and 0x80c34fe's `0x5f3759df` step off its bits with one Newton step
/// (0.5 at 0x815042c, 1.5 at 0x8150428), its half and result stored;
/// `None` for a zero quat.
fn rsqrt<T: Ar>(q: [f32; 4]) -> Option<T> {
    let (d, ext) = (d::<T>, ext::<T>);
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
fn normalize<T: Ar>(mats: &mut [Mat], w: f32, need: &[bool]) {
    let d = d::<T>;
    let w = d(w);
    for m in mats
        .iter_mut()
        .zip(need)
        .filter_map(|(m, &n)| n.then_some(m))
    {
        if let Some(r) = rsqrt::<T>(m.quat) {
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
fn add_normalized<T: Ar>(mats: &mut [Mat], src: &[Mat], w: f32, need: &[bool]) {
    let d = d::<T>;
    let w = d(w);
    for ((m, s), _) in mats.iter_mut().zip(src).zip(need).filter(|(_, n)| **n) {
        if let Some(r) = rsqrt::<T>(s.quat) {
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
fn mul<T: Ar>(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let d = d::<T>;
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
fn weigh<T: Ar>(m: &mut Mat) {
    let (d, ext) = (d::<T>, ext::<T>);
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
///
/// Only the bones `need` holds are built (all without it), each needed
/// bone's parent needed too; the rest stay zero.
pub fn build(skel: &Skeleton, locals: &[Mat], control: &[bool], need: Option<&[bool]>) -> Vec<Mat> {
    let all = vec![true; locals.len()];
    let need = need.unwrap_or(&all);
    exactly(
        || build_in::<Bounded>(skel, locals, control, need),
        || build_in::<F80>(skel, locals, control, need),
    )
}

fn build_in<T: Ar>(skel: &Skeleton, locals: &[Mat], control: &[bool], need: &[bool]) -> Vec<Mat> {
    let d = d::<T>;
    let mut out: Vec<Mat> = Vec::with_capacity(locals.len());
    // Each parent's rows, built once: the same words for every child.
    let mut rows: Vec<Option<[[f32; 3]; 3]>> = vec![None; locals.len()];
    for (i, (b, l)) in skel.bones().iter().zip(locals).enumerate() {
        if !need[i] {
            out.push(Mat::ZERO);
            continue;
        }
        let mut m = *l;
        match usize::try_from(b.parent) {
            Err(_) => weigh::<T>(&mut m),
            Ok(p) => {
                let pm = out[p];
                m.quat = match control.get(i).copied().unwrap_or(false) {
                    true => mul::<T>(pm.quat, l.quat),
                    false => mul::<T>(l.quat, pm.quat),
                };
                weigh::<T>(&mut m);
                let bind = b.local_pos.to_array();
                let t: [f32; 3] = std::array::from_fn(|c| st(d(l.trans[c]) + d(bind[c])));
                let r = *rows[p].get_or_insert_with(|| pm.axis_in::<T>());
                m.trans = pm.place_by::<T>(&r, t);
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
pub fn angles_quat(angles: [f32; 3]) -> [f32; 4] {
    exactly(
        || angles_quat_in::<Bounded>(angles),
        || angles_quat_in::<F80>(angles),
    )
}

fn angles_quat_in<T: Ar>([pitch, yaw, roll]: [f32; 3]) -> [f32; 4] {
    let (d, ext) = (d::<T>, ext::<T>);
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

/// What [`traceline`] struck.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineHit {
    /// Along `start..end`; 0 on a start-solid.
    pub fraction: f32,
    pub bone: usize,
    pub hit_location: u8,
    /// The segment lies inside the bone's box end to end.
    pub start_solid: bool,
    /// In the body's frame, as `DObjTraceline` leaves it (see [`traceline`]).
    pub normal: [f32; 3],
}

/// `DObjTraceline` (0x80bb27e): the segment `start..end`, in the body's
/// frame, against the hit boxes of the bones `mats` posed, under `priority`
/// (one byte per hit location), clipped to `limit`. Each bone's box sphere
/// (centre `(mins + maxs) * 0.5`, radius squared `|maxs - centre|^2`, both
/// stored at load, 0x80c7322) culls first, then the slab test in the
/// bone's frame. A higher priority takes the hit at any fraction up to
/// `limit`; an equal one only nearer. cod2-player-anims.md 12.
pub fn traceline(
    skel: &Skeleton,
    mats: &[Mat],
    start: [f32; 3],
    end: [f32; 3],
    priority: &[u8; 19],
    limit: f32,
) -> Option<LineHit> {
    exactly(
        || traceline_in::<Bounded>(skel, mats, start, end, priority, limit),
        || traceline_in::<F80>(skel, mats, start, end, priority, limit),
    )
}

/// `a > b` on the stack; a pass that cannot tell is in doubt.
fn gt<T: Ar>(a: T, b: T) -> bool {
    a.gt(b)
}

/// A 3-vector's components each `a - b` stored (0x80bc178).
fn sub3<T: Ar>(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| st(d::<T>(a[i]) - d::<T>(b[i])))
}

/// `((a.x b.x + a.y b.y) + a.z b.z)` on the stack (0x80bc248, 0x80bc27c).
fn dot3<T: Ar>(a: [f32; 3], b: [f32; 3]) -> T {
    let d = d::<T>;
    (d(a[0]) * d(b[0]) + d(a[1]) * d(b[1])) + d(a[2]) * d(b[2])
}

fn traceline_in<T: Ar>(
    skel: &Skeleton,
    mats: &[Mat],
    start: [f32; 3],
    end: [f32; 3],
    priority: &[u8; 19],
    limit: f32,
) -> Option<LineHit> {
    let d = d::<T>;
    let dir = sub3::<T>(end, start);
    let len2 = st(dot3::<T>(dir, dir));
    if len2 == 0.0 {
        return None;
    }
    let inv = st(ext::<T>(1.0) / d(len2));
    let mut best_priority = 2u8;
    let mut fraction = limit;
    let mut hit: Option<(usize, u8, usize)> = None;
    // The last face any bone's entry took, axis and sign: the sign is read
    // at the end whichever bone set it last (0x80bb89d, 0x80bbb02).
    let mut face_axis = 0usize;
    let mut face_sign = 0.0f32;
    for (bi, b) in skel.bones().iter().enumerate() {
        let (lo, hi) = (b.hit_mins.to_array(), b.hit_maxs.to_array());
        let centre: [f32; 3] = std::array::from_fn(|i| st((d(lo[i]) + d(hi[i])) * ext::<T>(0.5)));
        let r2 = st(dot3::<T>(sub3::<T>(hi, centre), sub3::<T>(hi, centre)));
        if r2 == 0.0 {
            continue;
        }
        let prio = priority.get(b.hit_location as usize).copied().unwrap_or(0);
        if prio < best_priority {
            continue;
        }
        let m = mats[bi];
        let rows = m.axis_in::<T>();
        // The sphere: the nearest point of the line to its centre.
        let c = m.place_by::<T>(&rows, centre);
        let v = sub3::<T>(start, c);
        let t = st(-dot3::<T>(v, dir) * d(inv));
        let dist2 = if gt(ext::<T>(1.0), d(t)) {
            if gt(d(t), ext::<T>(0.0)) {
                let p: [f32; 3] = std::array::from_fn(|i| st(d(t) * d(dir[i]) + d(v[i])));
                st(dot3::<T>(p, p))
            } else {
                st(dot3::<T>(v, v))
            }
        } else {
            let w = sub3::<T>(end, c);
            st(dot3::<T>(w, w))
        };
        let gap = st(d(r2) - d(dist2));
        if !gt(d(gap), ext::<T>(0.0)) {
            continue;
        }
        if prio == best_priority {
            let s = st(d(st(d(gap) * d(inv))).sqrt());
            if !gt(d(fraction), d(t) - d(s)) {
                continue;
            }
        }
        // Into the bone's frame (0x80bb1f0).
        let into = |p: [f32; 3]| -> [f32; 3] {
            let q = sub3::<T>(p, m.trans);
            std::array::from_fn(|i| st(dot3::<T>(q, rows[i])))
        };
        let (ls, le) = (into(start), into(end));
        let mut enter = 0.0f32;
        let mut exit = if prio == best_priority {
            fraction
        } else {
            limit
        };
        let (mut starts_in, mut ends_in) = (true, true);
        let mut axis_here = None;
        let mut missed = false;
        'planes: for (sign, planes) in [(-1.0f32, lo), (1.0, hi)] {
            for a in 0..3 {
                let d1 = st((d(ls[a]) - d(planes[a])) * d(sign));
                let d2 = st((d(le[a]) - d(planes[a])) * d(sign));
                if d1 > 0.0 {
                    if d2 > 0.0 {
                        missed = true;
                        break 'planes;
                    }
                    starts_in = false;
                    let den = st(d(d1) - d(d2));
                    if gt(d(d1), d(enter) * d(den)) {
                        enter = st(d(d1) / d(den));
                        if enter >= exit {
                            missed = true;
                            break 'planes;
                        }
                        face_sign = sign;
                        axis_here = Some(a);
                    }
                } else if d2 > 0.0 {
                    ends_in = false;
                    let den = st(d(d1) - d(d2));
                    if gt(d(d1), d(exit) * d(den)) {
                        exit = st(d(d1) / d(den));
                        if enter >= exit {
                            missed = true;
                            break 'planes;
                        }
                    }
                }
            }
        }
        if let Some(a) = axis_here {
            face_axis = a;
        }
        if missed {
            continue;
        }
        if starts_in {
            if !ends_in
                || gt(
                    d(dir[0]) * d(start[0]) + d(dir[1]) * d(start[1]),
                    ext::<T>(0.0),
                )
            {
                continue;
            }
            // Out from the body's axis toward the start, or along the
            // line's z (0x80bb9d0).
            let normal = if dir[0] != 0.0 || dir[1] != 0.0 {
                let len = st(d(st(d(start[0]) * d(start[0]) + d(start[1]) * d(start[1]))).sqrt());
                if len == 0.0 {
                    [start[0], start[1], 0.0]
                } else {
                    let r = st(ext::<T>(1.0) / d(len));
                    [st(d(start[0]) * d(r)), st(d(start[1]) * d(r)), 0.0]
                }
            } else {
                [0.0, 0.0, if dir[2] >= 0.0 { -1.0 } else { 1.0 }]
            };
            return Some(LineHit {
                fraction: 0.0,
                bone: bi,
                hit_location: b.hit_location,
                start_solid: true,
                normal,
            });
        }
        if prio == best_priority && enter >= fraction {
            continue;
        }
        best_priority = prio;
        fraction = enter;
        hit = Some((bi, b.hit_location, face_axis));
    }
    let (bone, hit_location, axis) = hit?;
    let rows = mats[bone].axis_in::<T>();
    Some(LineHit {
        fraction,
        bone,
        hit_location,
        start_solid: false,
        normal: rows[axis].map(|r| st(d(r) * d(face_sign))),
    })
}
