//! 1.7's anim sampler (cod4-player-anims.md 11): `DObjCalcAnim`
//! (`cod4_lnxded` 0x81b4c8c, `iw3mp.exe` 0x5875f0) walks the DObj's anim
//! tree (0x81b2578, 0x5872c0), each leaf adding its clip's keys at the node
//! time, times the weight it was handed, into the bones' `DObjAnimMat`s
//! without normalizing them, and each node of two or more live children
//! folding their sums. The locals it leaves are what `DObjCalcSkel`
//! composes ([`crate::skel`]). Both binaries run the same sums in the same
//! order; either binary's x87 stack is an `f64` here, one rounding to `f32`
//! where the binary stores, `iw3mp.exe` storing a few values `cod4_lnxded`
//! keeps ([`Cod4Arith`]). The walk and the folds are CoD2's too
//! ([`dobjanim`]); the leaf's sums ([`Parts::accum`]) are 1.7's own.

use anyhow::{Context, Result, ensure};

use crate::anim::{BodySource, Streams, script_string};
use crate::assets::Assets;
use crate::assets::model::XAnim;
use crate::skel::{AnimMat, QUAT_SCALE, X, d, f};
use vcod_common::dobjanim;
use vcod_common::playerpose::{Cod4Arith, Leaf};
use vcod_common::skeleton::Skeleton;

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
    /// (cod4-player-anims.md 1): every key list ascending from 0 to
    /// `numframes`, each bone translated once, each stream read to its end.
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
        let last = x.num_frames;
        let mut s = Streams::of(x);
        let mut rot = vec![Rot::None; n];
        let mut trans = vec![Trans::None; n];
        let mut decode = || -> Result<()> {
            let mut bone = bc[0];
            for (part, &count) in bc.iter().enumerate().take(5).skip(1) {
                for _ in 0..count {
                    let keys = match part <= 2 {
                        true => {
                            let k = s.keys()?;
                            check_keys(&k, last)
                                .with_context(|| format!("bone {}", bones[bone]))?;
                            Some(k)
                        }
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
                                .map(|_| {
                                    Ok([src(&mut s)?, src(&mut s)?, src(&mut s)?, src(&mut s)?])
                                })
                                .collect::<Result<_>>()?,
                        ),
                    };
                    bone += 1;
                }
            }
            let mut seen = vec![false; n];
            for (part, &count) in bc.iter().enumerate().take(9).skip(5) {
                for _ in 0..count {
                    let b = usize::from(s.byte.u8()?);
                    ensure!(b < n && !seen[b], "trans bone {b}");
                    seen[b] = true;
                    trans[b] = match part {
                        5 | 6 => {
                            let keys = s.keys()?;
                            check_keys(&keys, last)
                                .with_context(|| format!("bone {}", bones[b]))?;
                            let mins = s.int.vec3()?.to_array();
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
            Ok(())
        };
        decode().with_context(|| x.name.clone())?;
        s.finished().with_context(|| x.name.clone())?;
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

/// A key list checked: ascending, starting at 0 and ending at the clip's
/// last frame, as `XAnim_GetTimeIndex` reads it.
fn check_keys(keys: &[u16], last: u16) -> Result<()> {
    ensure!(
        keys.first() == Some(&0) && keys.last() == Some(&last),
        "keys run {:?}..{:?}, the clip 0..{last}",
        keys.first(),
        keys.last()
    );
    ensure!(keys.windows(2).all(|w| w[0] < w[1]), "keys not ascending");
    Ok(())
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

/// A node of the anim tree as the walk meets it, a leaf over its clip's
/// parts.
pub type Tree<'a> = dobjanim::Tree<'a, Parts>;

/// `a`'s arithmetic for the shared walk ([`dobjanim`]).
#[derive(Clone, Copy)]
struct Stack(Cod4Arith);

impl dobjanim::Mat for AnimMat {
    const ZERO: AnimMat = AnimMat {
        quat: [0.0; 4],
        trans: [0.0; 3],
        trans_weight: 0.0,
    };

    fn parts(&mut self) -> (&mut [f32; 4], &mut [f32; 3], &mut f32) {
        (&mut self.quat, &mut self.trans, &mut self.trans_weight)
    }
}

impl dobjanim::Calc for Stack {
    type V = X;
    type Mat = AnimMat;
    type Clip = Parts;

    fn ld(self, x: f32) -> X {
        X::kept(x)
    }

    fn ext(self, x: f64) -> X {
        X { v: x }
    }

    fn st(self, v: X) -> f32 {
        v.f()
    }

    fn pass(self, v: X) -> X {
        st(v, self.0)
    }

    fn is_zero(self, v: X) -> bool {
        v.v == 0.0
    }

    /// Every bone summed: 1.7's walk takes no `need`.
    fn leaf(
        self,
        clip: &Parts,
        remap: &[Option<usize>],
        t: f32,
        w: f32,
        mats: &mut [AnimMat],
        _need: &[bool],
    ) {
        clip.accum(remap, t, w, mats, self.0);
    }
}

/// `DObjCalcAnim`'s walk from the root ([`dobjanim::calc`]) in `a`'s
/// arithmetic: the locals of `bones` bones, and which bones a leaf reached
/// (the rest keep the model's own, 0x81b4d6c).
pub fn calc(root: &Tree, bones: usize, a: Cod4Arith) -> (Vec<AnimMat>, Vec<bool>) {
    dobjanim::calc(Stack(a), root, bones, &vec![true; bones])
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
                .map(|(l, (clip, remap))| Tree::Leaf {
                    clip,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two bones: a `noQuat` one and a keyed half quat, both `noTrans`,
    /// over 10 frames: the half quat's keys `keys`, the bytes `tail` after
    /// the trans bones.
    fn xanim(keys: [u8; 3], trans: [u8; 2], tail: &[u8]) -> XAnim {
        XAnim {
            name: "t".into(),
            num_frames: 10,
            looped: false,
            delta: false,
            bone_count: [1, 1, 0, 0, 0, 0, 0, 0, 2, 2],
            framerate: 30.0,
            frequency: 3.0,
            asset_type: 0,
            names: vec![0, 0],
            notify: Vec::new(),
            data_byte: [&keys[..], &trans, tail].concat(),
            data_short: 2u16.to_le_bytes().to_vec(),
            data_int: Vec::new(),
            random_data_short: [1i16, 2, 3, 4, 5, 6]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
            random_data_byte: Vec::new(),
            random_data_int: Vec::new(),
            indices: Vec::new(),
            root: None,
        }
    }

    /// The sampler divides by each key gap and takes the last key for
    /// `numframes`: a key list out of order or short of the clip, a bone
    /// translated twice and bytes left over are refused.
    #[test]
    fn parts_refuse_what_the_sampler_cannot_read() {
        let a = Assets::default();
        let p = Parts::of(&a, &xanim([0, 5, 10], [0, 1], &[])).unwrap();
        assert!(matches!(&p.rot[1], Rot::Half(Some(k), v) if *k == [0, 5, 10] && v.len() == 3));
        for (keys, trans, tail, why) in [
            ([0, 7, 5], [0, 1], &[][..], "descending keys"),
            ([0, 5, 9], [0, 1], &[], "keys short of numframes"),
            ([1, 5, 10], [0, 1], &[], "keys from 1"),
            ([0, 5, 10], [0, 0], &[], "bone 0 translated twice"),
            ([0, 5, 10], [0, 1], &[0], "a byte left over"),
        ] {
            assert!(Parts::of(&a, &xanim(keys, trans, tail)).is_err(), "{why}");
        }
    }
}
