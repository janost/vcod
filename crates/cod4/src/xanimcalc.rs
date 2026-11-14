//! 1.7's anim sampler as `cod4_lnxded` runs it (cod4-player-anims.md 11):
//! `DObjCalcAnim` (0x81b4c8c) walks the DObj's anim tree (0x81b2578), each
//! leaf adding its clip's keys at the node time, times the weight it was
//! handed, into the bones' `DObjAnimMat`s without normalizing them, and each
//! node of two or more live children folding their sums. The locals it
//! leaves are what `DObjCalcSkel` composes ([`crate::skel`]). The x87 stack
//! is an `f64` here, one rounding to `f32` where the binary stores.

use anyhow::{Result, ensure};

use crate::anim::{Streams, script_string};
use crate::assets::Assets;
use crate::assets::model::XAnim;
use crate::skel::AnimMat;

/// The float 1/32767 a quat short scales by (0x823ba10).
const QUAT_SCALE: f32 = f32::from_bits(0x3800_0100);

fn d(x: f32) -> f64 {
    f64::from(x)
}

fn f(x: f64) -> f32 {
    x as f32
}

/// A short times [`QUAT_SCALE`], stored (`fild`, `fmul`, `fstp`).
fn q(s: i16) -> f32 {
    f(f64::from(s) * d(QUAT_SCALE))
}

/// One bone's rotation part: none (the identity), a half quat `(0, 0, z, w)`
/// or a full one, keyed (`keys`, one quat a key) or held.
#[derive(Clone, Debug)]
pub enum Rot {
    None,
    Half(Option<Vec<u16>>, Vec<[i16; 2]>),
    Full(Option<Vec<u16>>, Vec<[i16; 4]>),
}

/// One bone's translation part: keyed `mins + size * v` (bytes for a small
/// part, shorts else), held, or none.
#[derive(Clone, Debug)]
pub enum Trans {
    Keyed {
        keys: Vec<u16>,
        mins: [f32; 3],
        size: [f32; 3],
        v: Vec<[u16; 3]>,
    },
    Fixed([f32; 3]),
    None,
}

/// An `XAnimParts` as the sampler reads it: each bone's parts, the raw
/// shorts and bytes the streams hold.
#[derive(Clone, Debug)]
pub struct Parts {
    pub name: String,
    /// `numframes` (+0xe): the last key frame.
    pub num_frames: u16,
    pub bones: Vec<String>,
    /// By the clip's bone.
    pub rot: Vec<Rot>,
    /// By the clip's bone.
    pub trans: Vec<Trans>,
}

impl Parts {
    /// `x`'s parts, read in `XAnimCalcParts`' stream order
    /// (cod4-player-anims.md 1).
    pub fn of(a: &Assets, x: &XAnim) -> Result<Parts> {
        let bones: Vec<String> = x
            .names
            .iter()
            .map(|&i| script_string(a, i).to_string())
            .collect();
        let bc = x.bone_count.map(usize::from);
        let n = bones.len();
        ensure!(
            n == bc[9] && bc[..5].iter().sum::<usize>() == n && bc[5..9].iter().sum::<usize>() == n,
            "{}: bone counts {:?} for {n} names",
            x.name,
            x.bone_count
        );
        let mut s = Streams::of(x);
        let mut rot = vec![Rot::None; n];
        let mut bone = bc[0];
        for (part, &count) in bc.iter().enumerate().take(5).skip(1) {
            for _ in 0..count {
                let keys = match part <= 2 {
                    true => Some(s.keys()?),
                    false => None,
                };
                let k = keys.as_ref().map_or(1, Vec::len);
                let src = |s: &mut Streams| match part <= 2 {
                    true => s.rshort.i16(),
                    false => s.short.i16(),
                };
                rot[bone] = match part % 2 == 1 {
                    true => Rot::Half(
                        keys,
                        (0..k)
                            .map(|_| Ok([src(&mut s)?, src(&mut s)?]))
                            .collect::<Result<_>>()?,
                    ),
                    false => Rot::Full(
                        keys,
                        (0..k)
                            .map(|_| Ok([src(&mut s)?, src(&mut s)?, src(&mut s)?, src(&mut s)?]))
                            .collect::<Result<_>>()?,
                    ),
                };
                bone += 1;
            }
        }
        let mut trans = vec![Trans::None; n];
        for (part, &count) in bc.iter().enumerate().take(9).skip(5) {
            for _ in 0..count {
                let b = usize::from(s.byte.u8()?);
                ensure!(b < n, "{}: trans bone {b}", x.name);
                trans[b] = match part {
                    5 | 6 => {
                        let keys = s.keys()?;
                        let v = s.int.vec3()?;
                        let mins = v.to_array();
                        let size = s.int.vec3()?.to_array();
                        let v = (0..keys.len())
                            .map(|_| match part {
                                5 => {
                                    let b = s.rbyte.take(3)?;
                                    Ok([b[0], b[1], b[2]].map(u16::from))
                                }
                                _ => Ok([s.rshort.u16()?, s.rshort.u16()?, s.rshort.u16()?]),
                            })
                            .collect::<Result<_>>()?;
                        Trans::Keyed {
                            keys,
                            mins,
                            size,
                            v,
                        }
                    }
                    7 => Trans::Fixed(s.int.vec3()?.to_array()),
                    _ => Trans::None,
                };
            }
        }
        Ok(Parts {
            name: x.name.clone(),
            num_frames: x.num_frames,
            bones,
            rot,
            trans,
        })
    }

    /// The leaf's sums (0x81b2629..0x81b2e4a, 0x81b1534 under 256 frames):
    /// the clip at node time `t`, times `w`, added into `mats` by `remap`
    /// (the clip's bone to the DObj's, `None` a bone the model lacks).
    pub fn accum(&self, remap: &[Option<usize>], t: f32, w: f32, mats: &mut [AnimMat]) {
        let at = Frame::new(t, self.num_frames);
        let w = d(w);
        for (b, r) in self.rot.iter().enumerate() {
            let Some(m) = remap.get(b).copied().flatten() else {
                continue;
            };
            let m = &mut mats[m];
            let add4 = |m: &mut AnimMat, q0: [f32; 4], q1: Option<([f32; 4], f64)>| match q1 {
                // `b = frac * w`, `a = w - b` kept; `(q0 * a + acc) + q1 * b`.
                Some((q1, frac)) => {
                    let bw = frac * w;
                    let aw = w - bw;
                    for c in 0..4 {
                        m.quat[c] = f((d(q0[c]) * aw + d(m.quat[c])) + d(q1[c]) * bw);
                    }
                }
                None => {
                    for (a, q0) in m.quat.iter_mut().zip(q0) {
                        *a = f(d(q0) * w + d(*a));
                    }
                }
            };
            match r {
                Rot::None => m.quat[3] = f(d(m.quat[3]) + w),
                Rot::Half(keys, v) => {
                    let h = |i: usize| [0.0, 0.0, q(v[i][0]), q(v[i][1])];
                    match keys.as_ref().map(|k| at.key(k)) {
                        Some(Key::At(i, frac)) => add4(m, h(i), Some((h(i + 1), frac))),
                        Some(Key::Last(i)) => add4(m, h(i), None),
                        None => add4(m, h(0), None),
                    }
                }
                Rot::Full(keys, v) => {
                    let h = |i: usize| v[i].map(q);
                    match keys.as_ref().map(|k| at.key(k)) {
                        Some(Key::At(i, frac)) => add4(m, h(i), Some((h(i + 1), frac))),
                        Some(Key::Last(i)) => add4(m, h(i), None),
                        None => add4(m, h(0), None),
                    }
                }
            }
        }
        for (b, tr) in self.trans.iter().enumerate() {
            let Some(m) = remap.get(b).copied().flatten() else {
                continue;
            };
            let m = &mut mats[m];
            match tr {
                Trans::Keyed {
                    keys,
                    mins,
                    size,
                    v,
                } => {
                    let fv = |i: usize, c: usize| f64::from(v[i][c]);
                    let key = at.key(keys);
                    for c in 0..3 {
                        // `((v0 + (v1 - v0) * frac) * size + mins) * w + acc`.
                        let x = match key {
                            Key::At(i, frac) => fv(i, c) + (fv(i + 1, c) - fv(i, c)) * frac,
                            Key::Last(i) => fv(i, c),
                        };
                        m.trans[c] = f((x * d(size[c]) + d(mins[c])) * w + d(m.trans[c]));
                    }
                }
                Trans::Fixed(o) => {
                    for (a, o) in m.trans.iter_mut().zip(o) {
                        *a = f(d(*o) * w + d(*a));
                    }
                }
                Trans::None => {}
            }
            m.trans_weight = f(d(m.trans_weight) + w);
        }
    }
}

/// Where a leaf samples: its node time, `numframes` times it, and the
/// frame that truncates to.
struct Frame {
    /// `t == 1` or a clip of no frames: each part's last key.
    last: bool,
    /// The frame as the fraction reads it: under 256 frames kept on the
    /// stack (0x81b1557), at or past it stored (0x81b2b1b).
    frame: f64,
    /// The truncated frame the key search compares, off the stored float.
    whole: u32,
    wide: bool,
}

#[derive(Clone, Copy)]
enum Key {
    /// Between key `i` and `i + 1`, `frac` past `i`.
    At(usize, f64),
    Last(usize),
}

impl Frame {
    fn new(t: f32, num_frames: u16) -> Frame {
        let exact = d(t) * f64::from(num_frames);
        let stored = f(exact);
        let wide = num_frames > 0xff;
        Frame {
            last: t == 1.0 || num_frames == 0,
            frame: if wide { d(stored) } else { exact },
            whole: stored as u32,
            wide,
        }
    }

    /// The key at or below the frame (`keys` ascending from 0 to
    /// `numframes`) and the fraction past it: kept under 256 frames, stored
    /// by 0x81b02b6 at or past it.
    fn key(&self, keys: &[u16]) -> Key {
        let n = keys.len() - 1;
        if self.last || n == 0 {
            return Key::Last(n);
        }
        let i = keys[1..]
            .iter()
            .position(|&k| u32::from(k) > self.whole)
            .unwrap_or(n - 1)
            .min(n - 1);
        let (k0, k1) = (f64::from(keys[i]), f64::from(keys[i + 1]));
        let frac = (self.frame - k0) / (k1 - k0);
        Key::At(i, if self.wide { d(f(frac)) } else { frac })
    }
}

/// A node of the anim tree as the walk meets it: a leaf's clip, its node
/// time and weight, or a node's weight and children.
pub enum Tree<'a> {
    Leaf {
        parts: &'a Parts,
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

/// `DObjCalcAnim`'s walk from the root (weight 1, the bones cleared, its
/// children's sums divided by their weight): the locals of `bones` bones,
/// and which bones a leaf reached (the rest keep the model's own,
/// 0x81b4d6c).
pub fn calc(root: &Tree, bones: usize) -> (Vec<AnimMat>, Vec<bool>) {
    let mut mats = vec![ZERO; bones];
    let mut reached = vec![false; bones];
    walk(root, 1.0, true, false, &mut mats, &mut reached);
    (mats, reached)
}

const ZERO: AnimMat = AnimMat {
    quat: [0.0; 4],
    trans: [0.0; 3],
    trans_weight: 0.0,
};

/// 0x81b2578: `node` at weight `w`; `clear` zeroes the bones first, `nested`
/// (`p5`) marks a child of a fold, which folds its own children by
/// normalizing their quats rather than dividing by their weight.
fn walk(
    node: &Tree,
    w: f32,
    clear: bool,
    nested: bool,
    mats: &mut [AnimMat],
    reached: &mut [bool],
) {
    match node {
        Tree::Leaf {
            parts, remap, time, ..
        } => {
            if clear {
                mats.fill(ZERO);
            }
            for &m in remap.iter().flatten() {
                reached[m] = true;
            }
            parts.accum(remap, *time, w, mats);
        }
        Tree::Node { children, .. } => {
            let live: Vec<&Tree> = children.iter().filter(|c| c.weight() != 0.0).collect();
            match live.len() {
                0 => {
                    if clear {
                        mats.fill(ZERO);
                    }
                }
                // One live child takes the node's weight and flags (0x81b46bc).
                1 => walk(live[0], w, clear, nested, mats, reached),
                _ => {
                    // The children sum into the bones themselves when the
                    // node clears, else into scratch the first one clears.
                    if clear {
                        for (i, c) in live.iter().enumerate() {
                            walk(c, c.weight(), i == 0, true, mats, reached);
                        }
                        match nested {
                            false => divide(mats),
                            true => normalize(mats, w),
                        }
                    } else {
                        let mut scratch = vec![ZERO; mats.len()];
                        for (i, c) in live.iter().enumerate() {
                            walk(c, c.weight(), i == 0, true, &mut scratch, reached);
                        }
                        add_normalized(mats, &scratch, w);
                    }
                }
            }
        }
    }
}

/// The root's fold (0x81b4608..0x81b4678): each bone of non-zero weight
/// times `1 / weight` kept, each word stored; the weight stays.
fn divide(mats: &mut [AnimMat]) {
    for m in mats {
        if m.trans_weight == 0.0 {
            continue;
        }
        let r = 1.0 / d(m.trans_weight);
        for c in 0..4 {
            m.quat[c] = f(d(m.quat[c]) * r);
        }
        for c in 0..3 {
            m.trans[c] = f(d(m.trans[c]) * r);
        }
    }
}

/// `|q|^2` summed `((x^2 + y^2) + z^2) + w^2` on the stack, and the
/// `0x5f3759df` step off its stored bits with one Newton step (0.5 and 1.5
/// at 0x8209628, 0x820962c), the sum kept: `None` for a zero quat.
fn rsqrt(q: [f32; 4]) -> Option<f64> {
    let [x, y, z, w] = q.map(d);
    let s = ((x * x + y * y) + z * z) + w * w;
    if s == 0.0 {
        return None;
    }
    let y0 = d(f32::from_bits(0x5f37_59df - (f(s).to_bits() >> 1)));
    Some((1.5 - ((s * 0.5) * y0) * y0) * y0)
}

/// A clearing child's fold in place (0x81b4a32..0x81b4abe): the quat
/// normalized times `w`, the shift times `w / weight`, the weight `w`.
fn normalize(mats: &mut [AnimMat], w: f32) {
    for m in mats {
        if let Some(r) = rsqrt(m.quat) {
            let s = d(w) * r;
            for c in 0..4 {
                m.quat[c] = f(d(m.quat[c]) * s);
            }
        }
        if m.trans_weight != 0.0 {
            let g = d(w) / d(m.trans_weight);
            for c in 0..3 {
                m.trans[c] = f(d(m.trans[c]) * g);
            }
            m.trans_weight = w;
        }
    }
}

/// A later child's fold out of scratch (0x81b4b78..0x81b4c0d): its
/// normalized quat times `w` and its shift times `w / weight` added, `w`
/// to the weight.
fn add_normalized(mats: &mut [AnimMat], src: &[AnimMat], w: f32) {
    for (m, s) in mats.iter_mut().zip(src) {
        if let Some(r) = rsqrt(s.quat) {
            let k = d(w) * r;
            for c in 0..4 {
                m.quat[c] = f(d(s.quat[c]) * k + d(m.quat[c]));
            }
        }
        if s.trans_weight != 0.0 {
            let g = d(w) / d(s.trans_weight);
            for c in 0..3 {
                m.trans[c] = f(d(s.trans[c]) * g + d(m.trans[c]));
            }
            m.trans_weight = f(d(w) + d(m.trans_weight));
        }
    }
}
