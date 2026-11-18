//! 1.7's anim sampler (cod4-player-anims.md 11): `DObjCalcAnim`
//! (`cod4_lnxded` 0x81b4c8c, `iw3mp.exe` 0x5875f0) walks the DObj's anim
//! tree (0x81b2578, 0x5872c0), each leaf adding its clip's keys at the node
//! time, times the weight it was handed, into the bones' `DObjAnimMat`s
//! without normalizing them, and each node of two or more live children
//! folding their sums. The locals it leaves are what `DObjCalcSkel`
//! composes ([`crate::skel`]). Both binaries run the same sums in the same
//! order; either binary's x87 stack is an `f64` here, one rounding to `f32`
//! where the binary stores, `iw3mp.exe` storing a few values `cod4_lnxded`
//! keeps ([`Cod4Arith`]).

use anyhow::{Result, ensure};

use crate::anim::{BodySource, Streams, script_string};
use crate::assets::Assets;
use crate::assets::model::XAnim;
use crate::skel::{AnimMat, X};
use vcod_common::playerpose::{Cod4Arith, Leaf};
use vcod_common::skeleton::Skeleton;

/// The float 1/32767 a quat short scales by (0x823ba10; `iw3mp.exe`'s
/// double at 0x70b2a0 holds the same value).
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
    /// +0x10.
    pub looped: bool,
    /// +0x28: frames a second.
    pub framerate: f32,
    /// +0x2c: node time a second.
    pub frequency: f32,
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
            looped: x.looped,
            framerate: x.framerate,
            frequency: x.frequency,
            bones,
            rot,
            trans,
        })
    }

    /// The leaf's sums (0x81b2629..0x81b2e4a, 0x81b1534 under 256 frames):
    /// the clip at node time `t`, times `w`, added into `mats` by `remap`
    /// (the clip's bone to the DObj's, `None` a bone the model lacks), in
    /// `a`'s arithmetic.
    pub fn accum(
        &self,
        remap: &[Option<usize>],
        t: f32,
        w: f32,
        mats: &mut [AnimMat],
        a: Cod4Arith,
    ) {
        let x = X::kept;
        let at = Frame::new(t, self.num_frames, a);
        let w = x(w);
        for (b, r) in self.rot.iter().enumerate() {
            let Some(m) = remap.get(b).copied().flatten() else {
                continue;
            };
            let m = &mut mats[m];
            let add4 = |m: &mut AnimMat, q0: [f32; 4], q1: Option<([f32; 4], X)>| match q1 {
                // `b = frac * w`, `a = w - b`, kept on the server, stored
                // on the client (0x587f07, 0x587f17); `(q0 * a + acc) + q1
                // * b`.
                Some((q1, frac)) => {
                    let bw = st(frac * w, a);
                    let aw = st(w - bw, a);
                    for c in 0..4 {
                        m.quat[c] = ((x(q0[c]) * aw + x(m.quat[c])) + x(q1[c]) * bw).f();
                    }
                }
                None => {
                    for (acc, q0) in m.quat.iter_mut().zip(q0) {
                        *acc = (x(q0) * w + x(*acc)).f();
                    }
                }
            };
            match r {
                Rot::None => m.quat[3] = (x(m.quat[3]) + w).f(),
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
                    let fv = |i: usize, c: usize| x(f32::from(v[i][c]));
                    let key = at.key(keys);
                    for c in 0..3 {
                        // `((v0 + (v1 - v0) * frac) * size + mins) * w + acc`,
                        // the client storing the lerp and the shift
                        // (0x58546b, 0x5854a2).
                        let p = match key {
                            Key::At(i, frac) => fv(i, c) + (fv(i + 1, c) - fv(i, c)) * frac,
                            Key::Last(i) => fv(i, c),
                        };
                        let o = st(st(p, a) * x(size[c]) + x(mins[c]), a);
                        m.trans[c] = (o * w + x(m.trans[c])).f();
                    }
                }
                Trans::Fixed(o) => {
                    for (acc, o) in m.trans.iter_mut().zip(o) {
                        *acc = (x(*o) * w + x(*acc)).f();
                    }
                }
                Trans::None => {}
            }
            m.trans_weight = (x(m.trans_weight) + w).f();
        }
    }
}

/// Where a leaf samples: its node time, `numframes` times it, and the
/// frame that truncates to.
struct Frame {
    /// `t == 1` or a clip of no frames: each part's last key.
    last: bool,
    /// The frame as the fraction reads it: under 256 frames kept on
    /// `cod4_lnxded`'s stack (0x81b1557), at or past it stored (0x81b2b1b);
    /// `iw3mp.exe` stores it either way (0x587cac).
    frame: X,
    /// The truncated frame the key search compares, off the stored float.
    whole: u32,
    wide: bool,
    arith: Cod4Arith,
}

#[derive(Clone, Copy)]
enum Key {
    /// Between key `i` and `i + 1`, `frac` past `i`.
    At(usize, X),
    Last(usize),
}

impl Frame {
    fn new(t: f32, num_frames: u16, a: Cod4Arith) -> Frame {
        let exact = X::kept(t) * X::kept(f32::from(num_frames));
        let stored = exact.f();
        let wide = num_frames > 0xff;
        Frame {
            last: t == 1.0 || num_frames == 0,
            frame: match wide || a == Cod4Arith::Client {
                true => exact.st(),
                false => exact,
            },
            whole: stored as u32,
            wide,
            arith: a,
        }
    }

    /// The key at or below the frame (`keys` ascending from 0 to
    /// `numframes`) and the fraction past it: on the server kept under 256
    /// frames, stored by 0x81b02b6 at or past it; the client's key search
    /// stores it (0x587b8a, 0x587c6a).
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
        let k = |k: u16| self.frame.with(f64::from(k));
        let frac = (self.frame - k(keys[i])) / self.frame.with(f64::from(keys[i + 1] - keys[i]));
        Key::At(
            i,
            match self.wide {
                true => frac.st(),
                false => st(frac, self.arith),
            },
        )
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
/// children's sums divided by their weight) in `a`'s arithmetic: the
/// locals of `bones` bones, and which bones a leaf reached (the rest keep
/// the model's own, 0x81b4d6c).
pub fn calc(root: &Tree, bones: usize, a: Cod4Arith) -> (Vec<AnimMat>, Vec<bool>) {
    let mut mats = vec![ZERO; bones];
    let mut reached = vec![false; bones];
    walk(root, 1.0, true, false, &mut mats, &mut reached, a);
    (mats, reached)
}

/// A player body's locals as 1.7's sampler leaves them, by `skel`'s bone:
/// `DObjCalcAnim` over `root` and `main` over the legs and torso nodes at
/// their weights, each over its clips at their node times and weights
/// (`channels`, `playerpose::player_leaves`; cod4-player-anims.md 11), a
/// bone no leaf reached its own local in the first of `models` that holds
/// it. `None` where
/// a clip is not in `src`. The server's hit poses and the client's drawn
/// bodies both take it.
pub fn player_locals(
    src: &BodySource,
    skel: &Skeleton,
    models: &[&str],
    channels: &[(f32, Vec<Leaf>)],
    a: Cod4Arith,
) -> Option<Vec<Option<AnimMat>>> {
    let mut held = Vec::new();
    for (_, leaves) in channels {
        for l in leaves {
            let parts = src.parts(&l.clip)?;
            let remap: Vec<Option<usize>> =
                parts.bones.iter().map(|n| skel.bone_index(n)).collect();
            held.push((parts, remap));
        }
    }
    let mut held = held.iter();
    let nodes = channels
        .iter()
        .map(|(weight, leaves)| Tree::Node {
            weight: *weight,
            children: leaves
                .iter()
                .zip(held.by_ref())
                .map(|(l, (parts, remap))| Tree::Leaf {
                    parts,
                    remap,
                    time: l.time,
                    weight: l.weight,
                })
                .collect(),
        })
        .collect();
    let root = Tree::Node {
        weight: 1.0,
        children: vec![Tree::Node {
            weight: 1.0,
            children: nodes,
        }],
    };
    let (mats, reached) = calc(&root, skel.bones().len(), a);
    // Each model's base locals looked up once, in `models`' order.
    let bases: Vec<_> = models.iter().filter_map(|m| src.base_locals(m)).collect();
    Some(
        mats.into_iter()
            .zip(reached)
            .zip(skel.bones())
            .map(|((m, r), bone)| match r {
                true => Some(m),
                false => bases
                    .iter()
                    .find_map(|b| b.iter().find(|(n, _)| *n == bone.name).map(|(_, m)| *m)),
            })
            .collect(),
    )
}

/// A value `iw3mp.exe` stores where `cod4_lnxded` keeps it on the stack:
/// both binaries' x87 hold more than a float between stores, the server's
/// a 64-bit mantissa, the client's a double (cod4-player-anims.md 11).
fn st(v: X, a: Cod4Arith) -> X {
    match a {
        Cod4Arith::Client => v.st(),
        Cod4Arith::Server => v,
    }
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
    a: Cod4Arith,
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
            parts.accum(remap, *time, w, mats, a);
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
                1 => walk(live[0], w, clear, nested, mats, reached, a),
                _ => {
                    // The children sum into the bones themselves when the
                    // node clears, else into scratch the first one clears.
                    if clear {
                        for (i, c) in live.iter().enumerate() {
                            walk(c, c.weight(), i == 0, true, mats, reached, a);
                        }
                        match nested {
                            false => divide(mats, a),
                            true => normalize(mats, w, a),
                        }
                    } else {
                        let mut scratch = vec![ZERO; mats.len()];
                        for (i, c) in live.iter().enumerate() {
                            walk(c, c.weight(), i == 0, true, &mut scratch, reached, a);
                        }
                        add_normalized(mats, &scratch, w, a);
                    }
                }
            }
        }
    }
}

/// The root's fold (0x81b4608..0x81b4678, `iw3mp.exe` 0x586c10): each
/// bone of non-zero weight times `1 / weight` kept, each word stored; the
/// weight stays.
fn divide(mats: &mut [AnimMat], a: Cod4Arith) {
    for m in mats {
        if m.trans_weight == 0.0 {
            continue;
        }
        let r = st(X::kept(1.0) / X::kept(m.trans_weight), a);
        for c in 0..4 {
            m.quat[c] = (X::kept(m.quat[c]) * r).f();
        }
        for c in 0..3 {
            m.trans[c] = (X::kept(m.trans[c]) * r).f();
        }
    }
}

/// `|q|^2` summed `((x^2 + y^2) + z^2) + w^2` on the stack, and the
/// `0x5f3759df` step off its stored bits with one Newton step (0.5 and 1.5
/// at 0x8209628, 0x820962c), the sum kept on the server; the client stores
/// the sum, its half and the step (0x586f07, 0x586f2f, 0x586f51): `None`
/// for a zero quat.
fn rsqrt(q: [f32; 4], a: Cod4Arith) -> Option<X> {
    let [x, y, z, w] = q.map(X::kept);
    let s = st(((x * x + y * y) + z * z) + w * w, a);
    if s.v == 0.0 {
        return None;
    }
    let y0 = X::kept(f32::from_bits(0x5f37_59df - (s.f().to_bits() >> 1)));
    let h = st(s * X::kept(0.5), a);
    Some(st((1.5 - (h * y0) * y0) * y0, a))
}

/// A clearing child's fold in place (0x81b4a32..0x81b4abe, `iw3mp.exe`
/// 0x586ea0): the quat normalized times `w`, the shift times
/// `w / weight`, the weight `w`.
fn normalize(mats: &mut [AnimMat], w: f32, a: Cod4Arith) {
    let w = X::kept(w);
    for m in mats {
        if let Some(r) = rsqrt(m.quat, a) {
            let s = st(w * r, a);
            for c in 0..4 {
                m.quat[c] = (X::kept(m.quat[c]) * s).f();
            }
        }
        if m.trans_weight != 0.0 {
            let g = st(w / X::kept(m.trans_weight), a);
            for c in 0..3 {
                m.trans[c] = (X::kept(m.trans[c]) * g).f();
            }
            m.trans_weight = w.f();
        }
    }
}

/// A later child's fold out of scratch (0x81b4b78..0x81b4c0d, `iw3mp.exe`
/// 0x586fe0): its normalized quat times `w` and its shift times
/// `w / weight` added, `w` to the weight.
fn add_normalized(mats: &mut [AnimMat], src: &[AnimMat], w: f32, a: Cod4Arith) {
    let w = X::kept(w);
    let x = X::kept;
    for (m, s) in mats.iter_mut().zip(src) {
        if let Some(r) = rsqrt(s.quat, a) {
            let k = st(w * r, a);
            for c in 0..4 {
                m.quat[c] = (x(s.quat[c]) * k + x(m.quat[c])).f();
            }
        }
        if s.trans_weight != 0.0 {
            let g = st(w / x(s.trans_weight), a);
            for c in 0..3 {
                m.trans[c] = (x(s.trans[c]) * g + x(m.trans[c])).f();
            }
            m.trans_weight = (w + x(m.trans_weight)).f();
        }
    }
}
