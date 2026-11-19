//! `DObjCalcAnim`'s walk of a DObj's anim tree and its folds, as CoD2 1.3
//! (`cod2_lnxded` 0x80bfc04) and 1.7 (`cod4_lnxded` 0x81b2578, `iw3mp.exe`
//! 0x5872c0) run it: each leaf adds its clip's keys into the bones, and a
//! node of two or more live children folds their sums
//! (cod2-player-anims.md 11, cod4-player-anims.md 11). Each game brings its
//! arithmetic and its leaf sampler ([`Calc`]); the walk, the sum orders and
//! the store points of the folds are the same in all three binaries.

use std::ops::{Add, Div, Mul, Sub};

/// A `DObjAnimMat` as the walk reads and writes it.
pub trait Mat: Copy {
    const ZERO: Self;
    /// The unnormalized quat, the translation and the weight.
    fn parts(&mut self) -> (&mut [f32; 4], &mut [f32; 3], &mut f32);
}

/// A game's arithmetic and leaf sampler, the parts of the walk that are its
/// own.
pub trait Calc: Copy {
    /// A value on the x87 stack.
    type V: Copy
        + Add<Output = Self::V>
        + Sub<Output = Self::V>
        + Mul<Output = Self::V>
        + Div<Output = Self::V>;
    type Mat: Mat;
    /// A leaf's clip.
    type Clip;
    /// A float loaded onto the stack.
    fn ld(self, x: f32) -> Self::V;
    /// A double constant.
    fn ext(self, x: f64) -> Self::V;
    /// A store to a float.
    fn st(self, v: Self::V) -> f32;
    /// A factor the folds pass on: `cod2_lnxded` and `iw3mp.exe` store it
    /// and load it back, `cod4_lnxded` keeps it on the stack.
    fn pass(self, v: Self::V) -> Self::V;
    /// `v`, as [`Self::pass`] left it, is zero.
    fn is_zero(self, v: Self::V) -> bool;
    /// One leaf's sums: `clip` at node time `t`, times `w`, added into
    /// `mats` through `remap` (the clip's bone to the DObj's), the bones
    /// `need` holds.
    fn leaf(
        self,
        clip: &Self::Clip,
        remap: &[Option<usize>],
        t: f32,
        w: f32,
        mats: &mut [Self::Mat],
        need: &[bool],
    );
}

/// A node of the anim tree as the walk meets it: a leaf's clip at its node
/// time and weight, or a node's weight and children in index order.
pub enum Tree<'a, C> {
    Leaf {
        clip: &'a C,
        /// The clip's bone to the DObj's, `None` a bone the models lack.
        remap: &'a [Option<usize>],
        time: f32,
        weight: f32,
    },
    Node {
        weight: f32,
        children: Vec<Tree<'a, C>>,
    },
}

impl<C> Tree<'_, C> {
    fn weight(&self) -> f32 {
        match self {
            Tree::Leaf { weight, .. } | Tree::Node { weight, .. } => *weight,
        }
    }
}

/// The walk from the root (weight 1, the bones cleared, its children's sums
/// divided by their weight): the locals of `bones` bones and which a leaf
/// reached. Only the bones `need` holds are summed and folded; each bone's
/// words are its own.
pub fn calc<C: Calc>(
    c: C,
    root: &Tree<C::Clip>,
    bones: usize,
    need: &[bool],
) -> (Vec<C::Mat>, Vec<bool>) {
    let mut mats = vec![C::Mat::ZERO; bones];
    let mut reached = vec![false; bones];
    walk(c, root, 1.0, true, false, &mut mats, &mut reached, need);
    (mats, reached)
}

/// `node` at weight `w`; `clear` zeroes the bones first, `nested` marks a
/// child of a fold, whose own fold normalizes its quats rather than
/// dividing by its weight.
#[allow(clippy::too_many_arguments)]
fn walk<C: Calc>(
    c: C,
    node: &Tree<C::Clip>,
    w: f32,
    clear: bool,
    nested: bool,
    mats: &mut [C::Mat],
    reached: &mut [bool],
    need: &[bool],
) {
    match node {
        Tree::Leaf {
            clip, remap, time, ..
        } => {
            if clear {
                mats.fill(C::Mat::ZERO);
            }
            for &b in remap.iter().flatten() {
                reached[b] = true;
            }
            c.leaf(clip, remap, *time, w, mats, need);
        }
        Tree::Node { children, .. } => {
            let live: Vec<&Tree<C::Clip>> = children.iter().filter(|n| n.weight() != 0.0).collect();
            match live.len() {
                0 => {
                    if clear {
                        mats.fill(C::Mat::ZERO);
                    }
                }
                // One live child takes the node's weight and flags
                // (`cod4_lnxded` 0x81b46bc).
                1 => walk(c, live[0], w, clear, nested, mats, reached, need),
                // The children sum into the bones themselves when the node
                // clears, else into scratch the first one clears.
                _ if clear => {
                    for (i, n) in live.iter().enumerate() {
                        walk(c, n, n.weight(), i == 0, true, mats, reached, need);
                    }
                    match nested {
                        false => divide(c, mats, need),
                        true => normalize(c, mats, w, need),
                    }
                }
                _ => {
                    let mut scratch = vec![C::Mat::ZERO; mats.len()];
                    for (i, n) in live.iter().enumerate() {
                        walk(c, n, n.weight(), i == 0, true, &mut scratch, reached, need);
                    }
                    add_normalized(c, mats, &scratch, w, need);
                }
            }
        }
    }
}

/// The root's fold (`cod4_lnxded` 0x81b4608..0x81b4678, `iw3mp.exe`
/// 0x586c10): each bone of non-zero weight times `1 / weight` passed, each
/// word stored; the weight stays.
pub fn divide<C: Calc>(c: C, mats: &mut [C::Mat], need: &[bool]) {
    for (m, _) in mats.iter_mut().zip(need).filter(|(_, n)| **n) {
        let (q, t, w) = m.parts();
        if *w == 0.0 {
            continue;
        }
        let r = c.pass(c.ext(1.0) / c.ld(*w));
        *q = q.map(|v| c.st(c.ld(v) * r));
        *t = t.map(|v| c.st(c.ld(v) * r));
    }
}

/// `|q|^2` summed `((x^2 + y^2) + z^2) + w^2` on the stack and passed, and
/// the `0x5f3759df` step off its stored bits with one Newton step (0.5 and
/// 1.5 at `cod2_lnxded` 0x815042c, 0x8150428; `cod4_lnxded` 0x8209628,
/// 0x820962c), its half and its result passed (`cod2_lnxded` 0x80c38e6,
/// 0x80c34fe; `iw3mp.exe` 0x586f07, 0x586f2f, 0x586f51): `None` for a zero
/// quat.
pub fn rsqrt<C: Calc>(c: C, q: [f32; 4]) -> Option<C::V> {
    let [x, y, z, w] = q.map(|v| c.ld(v));
    let s = c.pass(((x * x + y * y) + z * z) + w * w);
    if c.is_zero(s) {
        return None;
    }
    let h = c.pass(s * c.ext(0.5));
    let y0 = c.ld(f32::from_bits(0x5f37_59df - (c.st(s).to_bits() >> 1)));
    Some(c.pass((c.ext(1.5) - (h * y0) * y0) * y0))
}

/// A clearing child's fold in place (`cod4_lnxded` 0x81b4a32..0x81b4abe,
/// `iw3mp.exe` 0x586ea0): the quat times `rsqrt * w`, the shift times
/// `w / weight`, both factors passed, the weight then `w`.
pub fn normalize<C: Calc>(c: C, mats: &mut [C::Mat], w: f32, need: &[bool]) {
    let w = c.ld(w);
    for (m, _) in mats.iter_mut().zip(need).filter(|(_, n)| **n) {
        let (q, t, mw) = m.parts();
        if let Some(r) = rsqrt(c, *q) {
            let s = c.pass(r * w);
            *q = q.map(|v| c.st(c.ld(v) * s));
        }
        if *mw != 0.0 {
            let g = c.pass(w / c.ld(*mw));
            *t = t.map(|v| c.st(c.ld(v) * g));
            *mw = c.st(w);
        }
    }
}

/// A later child's fold out of scratch (`cod4_lnxded` 0x81b4b78..0x81b4c0d,
/// `iw3mp.exe` 0x586fe0): its quat times `rsqrt * w` and its shift times
/// `w / weight` added, `w` added to the weight.
pub fn add_normalized<C: Calc>(c: C, mats: &mut [C::Mat], src: &[C::Mat], w: f32, need: &[bool]) {
    let w = c.ld(w);
    for ((m, s), _) in mats.iter_mut().zip(src).zip(need).filter(|(_, n)| **n) {
        let mut s = *s;
        let (sq, st, sw) = s.parts();
        let (q, t, mw) = m.parts();
        if let Some(r) = rsqrt(c, *sq) {
            let k = c.pass(r * w);
            for i in 0..4 {
                q[i] = c.st(k * c.ld(sq[i]) + c.ld(q[i]));
            }
        }
        if *sw != 0.0 {
            let g = c.pass(w / c.ld(*sw));
            for i in 0..3 {
                t[i] = c.st(g * c.ld(st[i]) + c.ld(t[i]));
            }
            *mw = c.st(w + c.ld(*mw));
        }
    }
}
