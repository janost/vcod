//! Vector arithmetic rounded the way the 1.1 binaries' x87 code rounds it:
//! every intermediate kept on the 80-bit stack (an `f64` here, or an
//! [`F80`] where an `f64` would round twice) and one rounding to `f32`
//! where the binary stores. The game module's
//! `VectorNormalize` (game.mp 0x3d8d8) and `CrossProduct` (0x3d86c) and the
//! engine's copies (`cod_lnxded` 0x8065a38, 0x80659cc) round alike.

use crate::patch::Arith;
use glam::{DVec3, Vec3};

/// A dot product left on the stack.
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    let (a, b) = (a.as_dvec3(), b.as_dvec3());
    (a.x * b.x + a.y * b.y) + a.z * b.z
}

/// `CrossProduct`: each component rounded once.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    a.as_dvec3().cross(b.as_dvec3()).as_vec3()
}

/// `VectorNormalize2`: the length stored as a float, each component
/// multiplied by its unrounded reciprocal and rounded once. A zero vector
/// comes out `+0.0` in every component (game.mp 0x3da68).
pub fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt() as f32;
    if len == 0.0 {
        return Vec3::ZERO;
    }
    (v.as_dvec3() * (1.0 / f64::from(len))).as_vec3()
}

/// `VectorNormalize`, in place: as [`normalize`], but a zero vector is left
/// as it was, signed zeros and all (game.mp 0x3d8d8).
pub fn normalize_in_place(v: Vec3) -> Vec3 {
    if dot(v, v).sqrt() as f32 == 0.0 {
        return v;
    }
    normalize(v)
}

/// The two-axis dot left on the stack (CoD2's 0x80e9740 and
/// `PM_StepSlideMove`'s 0x80eae6e).
pub fn dot2(a: Vec3, b: Vec3) -> f64 {
    f64::from(a.x) * f64::from(b.x) + f64::from(a.y) * f64::from(b.y)
}

/// The CoD2 and CoD4 pmove's `VectorMA`: `a + s * b`, each component
/// stored (cod2-movement.md 2).
pub fn ma_stored(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    let s = f64::from(s);
    let m = |a: f32, b: f32| (f64::from(a) + s * f64::from(b)) as f32;
    Vec3::new(m(a.x, b.x), m(a.y, b.y), m(a.z, b.z))
}

/// The CoD2 and CoD4 pmove's `VectorScale`: `s * v`, each component
/// stored.
pub fn scale_stored(v: Vec3, s: f32) -> Vec3 {
    scale(v, f64::from(s))
}

/// The CoD2 and CoD4 pmove's `Vec3Lerp` (CoD2's 0x80e129e):
/// `a + (b - a) * f` per component, stored. What every pmove caller rebuilds a trace's
/// end point with: CoD2's `trace_t` has no `endpos`
/// (docs/research/cod2-movement.md 3).
pub fn lerp_stored(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    let f = f64::from(f);
    let l = |a: f32, b: f32| (f64::from(a) + (f64::from(b) - f64::from(a)) * f) as f32;
    Vec3::new(l(a.x, b.x), l(a.y, b.y), l(a.z, b.z))
}

/// `I_sqrt` (CoD2's 0x80e9590): libc `sqrt` of the float argument widened
/// to a double, stored back to a float.
pub fn sqrt_stored(x: f64) -> f32 {
    f64::from(x as f32).sqrt() as f32
}

/// `VectorLength` (CoD2's 0x80e99e8): the sum passed to [`sqrt_stored`]
/// as a float.
pub fn length_stored(v: Vec3) -> f32 {
    sqrt_stored(dot(v, v))
}

/// `Vec2Normalize` (CoD2's 0x80a44c2) on x and y: the length through
/// [`sqrt_stored`], each component times `1 / len` with the reciprocal
/// kept. A zero vector is left alone.
pub fn normalize2_kept(x: f32, y: f32) -> (f32, f32) {
    let (x64, y64) = (f64::from(x), f64::from(y));
    let len = sqrt_stored(x64 * x64 + y64 * y64);
    if len == 0.0 {
        return (x, y);
    }
    let inv = 1.0 / f64::from(len);
    ((x64 * inv) as f32, (y64 * inv) as f32)
}

// The CoD2 and CoD4 pmove's own forms: `_stored` rounds as CoD2 1.3 does,
// `_kept` as CoD4 1.7 (cod4-pmove.md 1); the angle folds differ in method.

/// CoD2 1.3's `Vec3Normalize` (`cod2_lnxded` 0x80a4424): the length
/// stored, its reciprocal stored, each component times it stored; a zero
/// vector is left alone. Returns the normalized vector and the length.
pub fn vec3_normalize_stored(v: Vec3) -> (Vec3, f32) {
    let len = length_stored(v);
    if len == 0.0 {
        return (v, 0.0);
    }
    let inv = (1.0 / f64::from(len)) as f32;
    (scale_stored(v, inv), len)
}

/// CoD2 1.3's `Vec3NormalizeTo` (0x80a45fc): [`vec3_normalize_stored`], a
/// zero vector zeroed.
pub fn vec3_normalize_to_stored(v: Vec3) -> (Vec3, f32) {
    let (n, len) = vec3_normalize_stored(v);
    if len == 0.0 {
        (Vec3::ZERO, 0.0)
    } else {
        (n, len)
    }
}

/// CoD4 1.7's `Vec3Normalize` (`cod4_lnxded` 0x81915d8): the squares'
/// sum, the length and its reciprocal kept on the stack, each component
/// times it stored; a zero vector is left alone. Returns the normalized
/// vector and the length as a float.
pub fn vec3_normalize_kept(v: Vec3) -> (Vec3, f32) {
    let len = dot(v, v).sqrt();
    if len == 0.0 {
        return (v, 0.0);
    }
    (scale(v, 1.0 / len), len as f32)
}

/// CoD4's pmove `Vec3NormalizeTo`: [`vec3_normalize_kept`], a zero vector
/// zeroed. [`normalize_to`] leaves a zero vector as it went in; the two
/// differ only in a zero's sign.
pub fn vec3_normalize_to_kept(v: Vec3) -> (Vec3, f32) {
    let (n, len) = vec3_normalize_kept(v);
    if len == 0.0 {
        (Vec3::ZERO, 0.0)
    } else {
        (n, len)
    }
}

/// 65536 / 360 as a float: CoD2's rodata 0x8155124 and CoD1's
/// `ANGLE2SHORT`, the short grid's factor and every `delta_angles` push's.
pub const SHORT_PER_DEG: f32 = f32::from_bits(0x4336_0b61);

/// `AngleNormalize360` through the short grid on an argument the x87 keeps
/// (1.1d 0x3eb30, 1.5b 0x36a3c, UO 0x3dc8c): the product with
/// [`SHORT_PER_DEG`] stays on the stack into the truncating `fistp`, masked
/// to 16 bits, times 360 / 65536. An angle a hair under a 65536th truncates
/// down where a float product would round it up onto the unit (combat doc
/// 12.7).
pub fn angle_normalize360_kept(a: f64) -> f32 {
    let short = (f64::from(SHORT_PER_DEG) * a) as i32 & 0xffff;
    (f64::from(short) * (360.0 / 65536.0)) as f32
}

/// `AngleNormalize180` (1.1d 0x3eb70) on a kept argument:
/// [`angle_normalize360_kept`] stored, less 360 over 180.
pub fn angle_normalize180_kept(a: f64) -> f32 {
    let a = angle_normalize360_kept(a);
    if a > 180.0 { a - 360.0 } else { a }
}

/// 1.1d's `AngleDelta` (0x3ec9c): the difference left on the stack.
pub fn angle_delta_kept(a: f32, b: f32) -> f32 {
    angle_normalize180_kept(f64::from(a) - f64::from(b))
}

/// CoD2 1.3's `AngleNormalize360` (`cod2_lnxded` 0x80a8432) and CoD1's on
/// a float argument: [`angle_normalize360_kept`] of the stored float.
pub fn angle_normalize360_short(a: f32) -> f32 {
    angle_normalize360_kept(f64::from(a))
}

/// CoD2 1.3's `AngleNormalize180` (0x80a8476): [`angle_normalize360_short`]
/// stored, less 360 over 180.
pub fn angle_normalize180_short(a: f32) -> f32 {
    angle_normalize180_kept(f64::from(a))
}

/// CoD2 1.3's `AngleDelta` (0x80a85bc), 1.5b's (0x36c0a) and UO's
/// (0x3de5a): `AngleNormalize180(a - b)`, the difference stored as the
/// argument.
pub fn angle_delta_short(a: f32, b: f32) -> f32 {
    angle_normalize180_short(a - b)
}

/// One `delta_angles` push: `(int)(deg * 182.04445)` under a truncating
/// control word, masked to 16 bits (`cod2_lnxded` 0x80e6eb8, 1.5b 0x26368).
pub fn angle_shorts(deg: f32) -> i32 {
    (f64::from(SHORT_PER_DEG) * f64::from(deg)) as i32 & 0xffff
}

/// `AngleNormalize360Accurate` (`cod2_lnxded` 0x80a84b8, 1.5b 0x36ae0):
/// whole turns added or taken away until the angle is in 0..360, each sum
/// stored.
pub fn normalize360_accurate(a: f32) -> f32 {
    let mut a = crate::fguard::turns(a);
    while a < 0.0 {
        a += 360.0;
    }
    while a >= 360.0 {
        a -= 360.0;
    }
    a
}

/// `AngleNormalize180Accurate` (`cod2_lnxded` 0x80a8536, 1.5b 0x36b75):
/// turns added while at or under -180 and taken away while over 180, so an
/// angle already in range comes back as it went in.
pub fn normalize180_accurate(a: f32) -> f32 {
    let mut a = crate::fguard::turns(a);
    while a <= -180.0 {
        a += 360.0;
    }
    while a > 180.0 {
        a -= 360.0;
    }
    a
}

/// `AngleSubtract` (1.1 0x3e968): `a - b` stored, wrapped into -180..180
/// by whole turns.
pub fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = crate::fguard::turns(a - b);
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// 1 / 360 as CoD4 1.7's angle helpers multiply by: the float at
/// `cod4_lnxded` rodata 0x8209630 (0x3b360b61), widened.
pub const INV_360: f64 = 0.002777777845039964;

/// CoD4 1.7's `AngleNormalize360`: the turns' fraction off `floor`, 360
/// taken away once more when the product rounds to a whole turn.
pub fn angle_normalize360_floor(a: f32) -> f32 {
    let scaled = (f64::from(a) * INV_360) as f32;
    let fl = f64::from(scaled).floor() as f32;
    let r = ((f64::from(scaled) - f64::from(fl)) * 360.0) as f32;
    if ((f64::from(r) - 360.0) as f32) < 0.0 {
        r
    } else {
        (f64::from(r) - 360.0) as f32
    }
}

/// CoD4 1.7's `AngleNormalize180` as the inlined `(x - floor(x + 0.5)) *
/// 360` of the turns, each step stored.
pub fn angle_normalize180_floor(a: f32) -> f32 {
    let scaled = (f64::from(a) * INV_360) as f32;
    let offset = (f64::from(scaled) + 0.5) as f32;
    let fl = f64::from(offset).floor() as f32;
    ((f64::from(scaled) - f64::from(fl)) * 360.0) as f32
}

/// CoD4 1.7's `AngleDelta`: `AngleNormalize180(a - b)`, the difference
/// stored.
pub fn angle_delta_floor(a: f32, b: f32) -> f32 {
    angle_normalize180_floor((f64::from(a) - f64::from(b)) as f32)
}

/// CoD2 1.3's `vectopitch` (`cod2_lnxded` 0x80a4e5a): `-180 / pi *
/// atan2(z, |xy|)` in 0..360, the flat length through [`sqrt_stored`].
pub fn vectopitch_stored(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let (x, y) = (f64::from(v.x), f64::from(v.y));
    let flat = sqrt_stored(y * y + x * x);
    let p = (-180.0 * f64::from(v.z).atan2(f64::from(flat)) / std::f64::consts::PI) as f32;
    if p < 0.0 { p + 360.0 } else { p }
}

/// CoD2 1.3's `vectoyaw` (0x80a4d5a): `180 / pi * atan2(y, x)` in 0..360,
/// 360 added to a negative angle only.
pub fn vectoyaw_stored(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let y = (180.0 * f64::from(v.y).atan2(f64::from(v.x)) / std::f64::consts::PI) as f32;
    if y < 0.0 { y + 360.0 } else { y }
}

/// CoD4 1.7's `vectopitch` (`cod4_lnxded` 0x8192510): `-180 / pi *
/// atan2(z, |xy|)` in 0..360, the flat length kept and passed to `atan2`
/// as a double. The stored angle always takes an add, 360 or 0 (`cmovb`,
/// 0x8192571), so a level vector's -0 comes back +0.
pub fn vectopitch_kept(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return if v.z > 0.0 { 270.0 } else { 90.0 };
    }
    let (x, y) = (f64::from(v.x), f64::from(v.y));
    let flat = (x * x + y * y).sqrt();
    let p = (-180.0 * f64::from(v.z).atan2(flat) / std::f64::consts::PI) as f32;
    p + if p < 0.0 { 360.0 } else { 0.0 }
}

/// CoD4 1.7's `vectoyaw` (0x8191c22): `180 / pi * atan2(y, x)` in 0..360,
/// the add as [`vectopitch_kept`]'s.
pub fn vectoyaw_kept(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let y = (180.0 * f64::from(v.y).atan2(f64::from(v.x)) / std::f64::consts::PI) as f32;
    y + if y < 0.0 { 360.0 } else { 0.0 }
}

/// CoD4 1.7's `Vec3NormalizeTo` (`cod4_lnxded` 0x8192a64): the squared
/// length and its root left on the stack, each component times the
/// reciprocal and rounded once; a zero vector comes out as it went in.
pub fn normalize_to(v: Vec3) -> Vec3 {
    normalize_to_len(v).0
}

/// [`normalize_to`] with the length it returns in `st(0)`, unrounded.
pub fn normalize_to_len(v: Vec3) -> (Vec3, f64) {
    let len = dot(v, v).sqrt();
    let inv = if len > 0.0 { 1.0 / len } else { 1.0 };
    ((v.as_dvec3() * inv).as_vec3(), len)
}

/// `VectorScale` by a factor left on the stack.
pub fn scale(v: Vec3, s: f64) -> Vec3 {
    (v.as_dvec3() * DVec3::splat(s)).as_vec3()
}

/// 1.5b's `VectorNormalize` in place (game.mp 0x32f73), which United
/// Offensive shares (UO 0x3a1c3): the squared length, summed x, y, z, the
/// length and its reciprocal each stored as a float, every component
/// rounded once. A zero vector is left as it was.
pub fn normalize_stored(v: Vec3) -> Vec3 {
    normalize_stored_len(v).0
}

/// [`normalize_stored`] with the stored length it returns.
pub fn normalize_stored_len(v: Vec3) -> (Vec3, f32) {
    let len = f64::from(dot(v, v) as f32).sqrt() as f32;
    if len == 0.0 {
        return (v, len);
    }
    let inv = 1.0 / len;
    ((v.as_dvec3() * f64::from(inv)).as_vec3(), len)
}

/// 1.5b's and UO's `VectorNormalize2` (game.mp 0x33216, UO 0x3a466): the
/// rounding of [`normalize_stored`], and a zero vector comes out `+0.0` in
/// every component.
pub fn normalize2_stored(v: Vec3) -> Vec3 {
    if f64::from(dot(v, v) as f32).sqrt() as f32 == 0.0 {
        return Vec3::ZERO;
    }
    normalize_stored(v)
}

/// `AngleVectors` as all three Linux servers round it, engine and game
/// module alike (`cod_lnxded` 0x8063388, 1.5b 0x806759d, UO 0x80674ad;
/// game.mp 0x3b228, 1.5b 0x33e18, UO 0x3b068): forward, right, up. Each
/// angle times the double `M_PI * 2 / 360` is stored as a float, its
/// `fsincos` stored as floats, and each component's products and sum kept
/// on the stack at the 64-bit precision the process runs at ([`F80`])
/// until one store. Rounding every product to a float instead turns a
/// pitched, yawed and rolled model by a few ulps, and a slide down its
/// face ends thousandths short (movers doc 22).
pub fn angle_vectors(angles: Vec3) -> [Vec3; 3] {
    angle_vectors_of_radians(Vec3::new(
        radians(angles.x),
        radians(angles.y),
        radians(angles.z),
    ))
}

/// The radians the Linux builds' `AngleVectors` turn a degree angle into:
/// times the double `M_PI * 2 / 360` on the stack, stored as a float.
/// pmove's `ps.yaw` and `-ps.pitch` hold exactly these (`cmd::apply_view`).
pub fn radians(deg: f32) -> f32 {
    const K: f64 = std::f64::consts::PI * 2.0 / 360.0;
    let x = f64::from(deg) * K;
    stored(x, x.abs(), || {
        (F80::from_f32(deg) * F80::from_f64(K)).to_f32()
    })
}

/// [`angle_vectors`] of angles already turned to radians by [`radians`].
/// `right` and `up`'s first two components are a product of three floats
/// plus one of two: the triple product takes 72 bits, so an `f64` sum
/// rounds twice where the x87's rounds once, and lands a float ulp off
/// about once in 10^8, more often where the two terms nearly cancel
/// (`the_kept_sums_round_at_64_bits`).
pub fn angle_vectors_of_radians(rad: Vec3) -> [Vec3; 3] {
    angle_vectors_by(rad, |r| {
        let r = f64::from(r);
        (r.sin() as f32, r.cos() as f32)
    })
}

/// CoD4 1.7's `AngleVectors` (`cod4_lnxded` 0x818ea3a): [`angle_vectors`]'
/// radians and stack, each sine and cosine glibc's `sinf` and `cosf`
/// ([`crate::libm`]) where the 1.x servers' `fsincos` rounds correctly
/// (cod4-gsc.md 3.2, "Missiles under 1.7").
pub fn angle_vectors_libm(angles: Vec3) -> [Vec3; 3] {
    let rad = Vec3::new(radians(angles.x), radians(angles.y), radians(angles.z));
    angle_vectors_by(rad, |r| (crate::libm::sinf(r), crate::libm::cosf(r)))
}

fn angle_vectors_by(rad: Vec3, trig: impl Fn(f32) -> (f32, f32)) -> [Vec3; 3] {
    let sin_cos = |r: f32| {
        let (s, c) = trig(r);
        (f64::from(s), f64::from(c))
    };
    let (sp, cp) = sin_cos(rad.x);
    let (sy, cy) = sin_cos(rad.y);
    let (sr, cr) = sin_cos(rad.z);
    // `p * c + d * e`, `p` the exact product of two floats.
    let sum = |p: f64, c: f64, d: f64, e: f64| {
        let pc = p * c;
        let s = pc + d * e;
        stored(s, pc.abs() + s.abs(), || {
            let f = F80::from_f64;
            (f(p) * f(c) + f(d) * f(e)).to_f32()
        })
    };
    let forward = Vec3::new((cp * cy) as f32, (cp * sy) as f32, -sp as f32);
    // game.mp 0x3b2b8..0x3b2f5: `-sr * sp` once, times `cy` and `sy`.
    let srsp = -sr * sp;
    let right = Vec3::new(
        sum(srsp, cy, cr, sy),
        sum(srsp, sy, -cr, cy),
        (-sr * cp) as f32,
    );
    // 0x3b2fe..0x3b33e: `cr * sp` once, times `cy` and `sy`.
    let crsp = cr * sp;
    let up = Vec3::new(
        sum(crsp, cy, sr, sy),
        sum(crsp, sy, -sr, cy),
        (cr * cp) as f32,
    );
    [forward, right, up]
}

/// The float the x87 stores for a value an `f64` computed as `x`, within
/// `2^-52 * scale` of the exact one: `x`'s own rounding when no float
/// midpoint lies that close, else `exact`, the [`F80`] steps.
fn stored(x: f64, scale: f64, exact: impl FnOnce() -> f32) -> f32 {
    let f = x as f32;
    if f.is_normal() && scale <= 2.0 * x.abs() {
        // The slack is then under 8 of `x`'s ulps, and a float midpoint in
        // `x`'s binade is an `f64` whose low 29 mantissa bits are 2^28.
        let low = (x.to_bits() & ((1 << 29) - 1)) as i64;
        if (low - (1 << 28)).abs() > 16 {
            return f;
        }
    } else if f.is_normal() {
        let mid = |g: f32| (f64::from(f) + f64::from(g)) * 0.5;
        let slack = scale * f64::EPSILON * 2.0;
        if (x - mid(f.next_up())).abs() > slack && (x - mid(f.next_down())).abs() > slack {
            return f;
        }
    }
    exact()
}

/// A value on the x87 stack of the Linux servers, whose control word keeps
/// the default 64-bit precision: `m * 2^e`, `m` 0 or with its top bit set.
/// The four operations and the root, each rounded to nearest even as the
/// FPU rounds; no infinities or NaNs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct F80 {
    neg: bool,
    m: u64,
    e: i32,
}

impl F80 {
    /// A double, exactly.
    pub fn from_f64(x: f64) -> F80 {
        let bits = x.to_bits();
        let neg = bits >> 63 != 0;
        let exp = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1 << 52) - 1);
        let (m, e) = if exp == 0 {
            (frac, -1074)
        } else {
            (frac | 1 << 52, exp - 1075)
        };
        F80::round(neg, u128::from(m), e)
    }

    /// A float, exactly.
    pub fn from_f32(x: f32) -> F80 {
        F80::from_f64(f64::from(x))
    }

    /// `m * 2^e` rounded to a 64-bit mantissa.
    fn round(neg: bool, m: u128, e: i32) -> F80 {
        if m == 0 {
            return F80 { neg, m: 0, e: 0 };
        }
        let bits = 128 - m.leading_zeros() as i32;
        if bits <= 64 {
            let sh = 64 - bits;
            return F80 {
                neg,
                m: (m << sh) as u64,
                e: e - sh,
            };
        }
        let sh = bits - 64;
        let (mut q, rem, half) = (m >> sh, m & ((1 << sh) - 1), 1u128 << (sh - 1));
        if rem > half || rem == half && q & 1 == 1 {
            q += 1;
        }
        let (q, e) = if q >> 64 != 0 {
            (q >> 1, e + sh + 1)
        } else {
            (q, e + sh)
        };
        F80 {
            neg,
            m: q as u64,
            e,
        }
    }

    /// `fadd`; an exact zero is `+0` unless both terms are `-0`.
    fn sum(self, o: F80) -> F80 {
        if o.m == 0 {
            return if self.m == 0 {
                F80 {
                    neg: self.neg && o.neg,
                    ..self
                }
            } else {
                self
            };
        }
        if self.m == 0 {
            return o;
        }
        let (big, small) = if (self.e, self.m) >= (o.e, o.m) {
            (self, o)
        } else {
            (o, self)
        };
        // Two bits of headroom for the carry; what the alignment shifts out
        // of the smaller term survives as a sticky bit.
        let a = u128::from(big.m) << 62;
        let sh = (big.e - small.e) as u32;
        let b = u128::from(small.m) << 62;
        let b = if sh >= 127 {
            1
        } else {
            (b >> sh) | u128::from(b & ((1u128 << sh) - 1) != 0)
        };
        let (neg, m) = if big.neg == small.neg {
            (big.neg, a + b)
        } else if a == b {
            (false, 0)
        } else {
            (big.neg, a - b)
        };
        F80::round(neg, m, big.e - 62)
    }

    /// `frndint` under round-down: the greatest integer not above.
    pub fn floor(self) -> F80 {
        if self.m == 0 || self.e >= 0 {
            return self;
        }
        let sh = -self.e;
        let (int, frac) = if sh >= 64 {
            (0, self.m)
        } else {
            (self.m >> sh, self.m & ((1u64 << sh) - 1))
        };
        let int = if self.neg && frac != 0 { int + 1 } else { int };
        if int == 0 {
            return F80 {
                neg: false,
                m: 0,
                e: 0,
            };
        }
        F80::round(self.neg, u128::from(int), 0)
    }

    /// `fsqrt`; a negative's root is that of its magnitude.
    pub fn sqrt(self) -> F80 {
        if self.m == 0 {
            return self;
        }
        // An even exponent, the radicand 127 or 128 bits wide: the root
        // has 64.
        let (r, e) = if self.e & 1 == 0 {
            (u128::from(self.m) << 64, self.e - 64)
        } else {
            (u128::from(self.m) << 63, self.e - 63)
        };
        let mut x = (r as f64).sqrt() as u128;
        for _ in 0..3 {
            x = (x + r / x) / 2;
        }
        while x.checked_mul(x).is_none_or(|s| s > r) {
            x -= 1;
        }
        while (x + 1).checked_mul(x + 1).is_some_and(|s| s <= r) {
            x += 1;
        }
        // The root lies past `x + 1/2` exactly when the remainder passes
        // `x`; it is never on the half.
        let up = r - x * x > x;
        F80::round(
            false,
            (x << 1 | u128::from(up)) << 1 | u128::from(up),
            e / 2 - 2,
        )
    }

    /// Rounded to a double, nearest even (a `fstpl`).
    pub fn to_f64(self) -> f64 {
        if self.m == 0 {
            return if self.neg { -0.0 } else { 0.0 };
        }
        let sh = 11u32;
        let m = u128::from(self.m);
        let (mut q, rem, half) = (m >> sh, m & ((1u128 << sh) - 1), 1u128 << (sh - 1));
        if rem > half || rem == half && q & 1 == 1 {
            q += 1;
        }
        let v = q as f64 * 2f64.powi(self.e + sh as i32);
        if self.neg { -v } else { v }
    }

    /// `fstps`: rounded to a float, nearest even.
    pub fn to_f32(self) -> f32 {
        if self.m == 0 {
            return if self.neg { -0.0 } else { 0.0 };
        }
        // The leading bit's exponent; under -126 the float is subnormal and
        // keeps fewer than 24 bits.
        let lead = self.e + 63;
        let keep = if lead < -126 { 24 - (-126 - lead) } else { 24 };
        let sh = (64 - keep).min(127) as u32;
        let m = u128::from(self.m);
        let (mut q, rem, half) = (m >> sh, m & ((1u128 << sh) - 1), 1u128 << (sh - 1));
        if rem > half || rem == half && q & 1 == 1 {
            q += 1;
        }
        let v = (q as f64 * 2f64.powi(self.e + sh as i32)) as f32;
        if self.neg { -v } else { v }
    }

    /// `fsin`. The FPU reduces by its own 66-bit pi, [`FPU_PI`], so a
    /// multiple of the double pi lands off its zero by the gap (`sin(M_PI)`
    /// is 1.2246063e-16, not 1.2246469e-16), and an operand of 2^63 or more
    /// comes back unchanged (C2 set). Measured by `probe_math` on 1.1d
    /// (cod11-gsc-object-model.md 9.3).
    pub fn fsin(self) -> F80 {
        self.fpu_sin(false)
    }

    /// `fcos`: [`F80::fsin`] a quarter turn of [`FPU_PI`] on.
    pub fn fcos(self) -> F80 {
        self.fpu_sin(true)
    }

    fn fpu_sin(self, cos: bool) -> F80 {
        if self.m == 0 {
            return if cos { F80::from_f64(1.0) } else { self };
        }
        if self.e >= 0 {
            return self;
        }
        // Under 2^-2 no reduction happens; from 2^62 the shift below would
        // overflow, so that band takes the double's (unmeasured).
        if self.e < -65 || self.e == -1 {
            let x = self.to_f64();
            return F80::from_f64(if cos { x.cos() } else { x.sin() });
        }
        // In 2^-65 units: |x|, a half turn 2 * FPU_PI, a quarter turn FPU_PI.
        let mut units = u128::from(self.m) << (self.e + 65);
        if cos {
            units += FPU_PI;
        }
        let k = (units + FPU_PI) / (2 * FPU_PI);
        let rest = units.wrapping_sub(k * 2 * FPU_PI) as i128;
        let s = (rest as f64 * 2f64.powi(-65)).sin();
        let odd = k & 1 == 1;
        let flip = odd != (self.neg && !cos);
        F80::from_f64(if flip { -s } else { s })
    }
}

/// The x87's internal pi, `0x3.243f6a8885a308d3`, in 2^-64 units: 66 bits,
/// 4.0e-21 under pi.
pub const FPU_PI: u128 = 0x3_243f_6a88_85a3_08d3;

/// `fchs`.
impl std::ops::Neg for F80 {
    type Output = F80;
    fn neg(self) -> F80 {
        F80 {
            neg: !self.neg,
            ..self
        }
    }
}

/// `fmul`.
impl std::ops::Mul for F80 {
    type Output = F80;
    fn mul(self, o: F80) -> F80 {
        F80::round(
            self.neg != o.neg,
            u128::from(self.m) * u128::from(o.m),
            self.e + o.e,
        )
    }
}

impl std::ops::Add for F80 {
    type Output = F80;
    fn add(self, o: F80) -> F80 {
        self.sum(o)
    }
}

/// `fsub`.
impl std::ops::Sub for F80 {
    type Output = F80;
    fn sub(self, o: F80) -> F80 {
        self.sum(-o)
    }
}

/// `fdiv`; the divisor is never zero.
impl std::ops::Div for F80 {
    type Output = F80;
    fn div(self, o: F80) -> F80 {
        if self.m == 0 {
            return F80 {
                neg: self.neg != o.neg,
                ..self
            };
        }
        // 64 or 65 quotient bits, then a guard bit and the sticky rest.
        let (n, d) = (u128::from(self.m) << 64, u128::from(o.m));
        let (q, r) = (n / d, n % d);
        let guard = u128::from(r << 1 >= d);
        let sticky = u128::from(r << 1 != d * guard);
        F80::round(
            self.neg != o.neg,
            (q << 2) | guard << 1 | sticky,
            self.e - o.e - 64 - 2,
        )
    }
}

/// The x87's compare (`fucomi`).
impl PartialOrd for F80 {
    fn partial_cmp(&self, o: &F80) -> Option<std::cmp::Ordering> {
        let d = *self - *o;
        Some(if d.m == 0 {
            std::cmp::Ordering::Equal
        } else if d.neg {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        })
    }
}

/// The float `pi / 180` the Windows cgames' `AngleVectors` multiply by (1.1
/// `cgame_mp_x86.dll` 0x30069390, 1.5 0x3006b608, UO 0x3007bd70: bytes
/// `35 fa 8e 3c`), `f32::to_radians`' factor.
const DEG2RAD_MSVC: f32 = f32::from_bits(0x3c8e_fa35);

/// The radians the Windows cgames' `AngleVectors` turn a degree angle into:
/// times [`DEG2RAD_MSVC`], stored as a float.
pub fn radians_msvc(deg: f32) -> f32 {
    deg * DEG2RAD_MSVC
}

/// `AngleVectors` as the Windows cgames round it (1.1 `cgame_mp_x86.dll`
/// 0x3003a790, 1.5 `cgame_mp15.dll` 0x3003bea0, UO `uo_cgame_mp_x86.dll`
/// 0x3004a200, one MSVC build at the CRT's 53-bit precision): each angle
/// by [`radians_msvc`], its `fsincos` stored as floats, `sr * sp` and
/// `cr * sp` stored as floats, and each component's remaining products and
/// sum kept until one store.
pub fn angle_vectors_msvc(angles: Vec3) -> [Vec3; 3] {
    angle_vectors_msvc_of_radians(Vec3::new(
        radians_msvc(angles.x),
        radians_msvc(angles.y),
        radians_msvc(angles.z),
    ))
}

/// [`angle_vectors_msvc`] of angles already turned by [`radians_msvc`].
pub fn angle_vectors_msvc_of_radians(rad: Vec3) -> [Vec3; 3] {
    let sin_cos = |r: f32| {
        let r = f64::from(r);
        (f64::from(r.sin() as f32), f64::from(r.cos() as f32))
    };
    let (sp, cp) = sin_cos(rad.x);
    let (sy, cy) = sin_cos(rad.y);
    let (sr, cr) = sin_cos(rad.z);
    let forward = DVec3::new(cp * cy, cp * sy, -sp);
    let t = f64::from((sr * sp) as f32);
    let right = DVec3::new(cr * sy - t * cy, -(cr * cy) - t * sy, -(sr * cp));
    let t = f64::from((cr * sp) as f32);
    let up = DVec3::new(t * cy + sr * sy, t * sy - sr * cy, cr * cp);
    [forward.as_vec3(), right.as_vec3(), up.as_vec3()]
}

/// `AngleVectors` as the build `arith` names rounds it: the Linux servers'
/// [`angle_vectors`] (1.1d, 1.5b and UO alike) or the Windows cgames'
/// [`angle_vectors_msvc`].
pub fn angle_vectors_in(arith: Arith, angles: Vec3) -> [Vec3; 3] {
    match arith {
        Arith::Kept | Arith::Stored => angle_vectors(angles),
        Arith::Msvc => angle_vectors_msvc(angles),
    }
}

/// `AnglesToAxis` as the Windows cgames build it: [`angle_vectors_msvc`]
/// with the right negated (1.1 cgame 0x3003a790's callers).
pub fn angles_to_axis_msvc(angles: Vec3) -> [Vec3; 3] {
    let [f, r, u] = angle_vectors_msvc(angles);
    [f, Vec3::ZERO - r, u]
}

/// `AnglesToAxis` (engine UO 0x806a46e, game.mp 0x3ef3c, 1.5b 0x36ffa, UO
/// 0x3e24a): forward, left, up, the left `vec3_origin - right`, so a zero
/// in it is always `+0.0`.
pub fn angles_to_axis(angles: Vec3) -> [Vec3; 3] {
    let [f, r, u] = angle_vectors(angles);
    [f, Vec3::ZERO - r, u]
}

/// `CreateRotationMatrix` (engine 1.5b 0x8057df6, UO 0x8057c2a, under
/// `CM_TransformedBoxTrace`; game.mp `G_CreateRotationMatrix` 0x579fc, 1.5b
/// 0x55943, UO 0x60487): [`angle_vectors`] with the right negated in place
/// (`VectorInverse`, `cod_lnxded` 0x8065c38). It differs from
/// [`angles_to_axis`] only in a zero's sign.
pub fn rotation_matrix(angles: Vec3) -> [Vec3; 3] {
    let [f, r, u] = angle_vectors(angles);
    [f, -r, u]
}

/// `p` into the frame whose axes are `rows`, at `origin`, as CoD2's
/// `MatrixTransformVector43`'s inverse (lnxded 0x80a74f8) and the bone
/// frame's (0x80bb1f0) run it: each `p - origin` stored, each dot product
/// on the stack ([`F80`]), stored.
pub fn into_frame_x87(p: Vec3, rows: &[Vec3; 3], origin: Vec3) -> Vec3 {
    let f = F80::from_f32;
    let d: [F80; 3] = std::array::from_fn(|i| f((f(p[i]) - f(origin[i])).to_f32()));
    let dot = |r: Vec3| ((d[0] * f(r.x) + d[1] * f(r.y)) + d[2] * f(r.z)).to_f32();
    Vec3::new(dot(rows[0]), dot(rows[1]), dot(rows[2]))
}

/// `v` turned out of the frame whose axes are `rows` (CoD2 lnxded
/// 0x80a71b6): each component `((v.x r0 + v.y r1) + v.z r2)` on the stack,
/// stored.
pub fn out_of_frame_x87(v: Vec3, rows: &[Vec3; 3]) -> Vec3 {
    let f = F80::from_f32;
    let c = |i: usize| {
        ((f(v.x) * f(rows[0][i]) + f(v.y) * f(rows[1][i])) + f(v.z) * f(rows[2][i])).to_f32()
    };
    Vec3::new(c(0), c(1), c(2))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_stored_lerp_rounds_once_per_component() {
        let a = Vec3::new(1.0, 0.1, -3.0);
        let b = Vec3::new(2.0, 0.3, 5.0);
        let f = 0.3f32;
        let p = lerp_stored(a, b, f);
        let x = (1.0f64 + (2.0f64 - 1.0) * f64::from(f)) as f32;
        assert_eq!(p.x.to_bits(), x.to_bits());
        assert_eq!(lerp_stored(a, b, 1.0), b);
        assert_eq!(lerp_stored(a, b, 0.0), a);
    }

    use super::*;

    /// The model `angle_vectors` replaced: every stack value an `f64`.
    fn angle_vectors_f64(angles: Vec3) -> [Vec3; 3] {
        let rad = |deg: f32| (f64::from(deg) * (std::f64::consts::PI * 2.0 / 360.0)) as f32;
        let sin_cos = |deg: f32| {
            let r = f64::from(rad(deg));
            (f64::from(r.sin() as f32), f64::from(r.cos() as f32))
        };
        let (sp, cp) = sin_cos(angles.x);
        let (sy, cy) = sin_cos(angles.y);
        let (sr, cr) = sin_cos(angles.z);
        let forward = DVec3::new(cp * cy, cp * sy, -sp);
        let right = DVec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
        let up = DVec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
        [forward.as_vec3(), right.as_vec3(), up.as_vec3()]
    }

    /// [`angle_vectors`] with every step through [`F80`], no `f64` path.
    fn angle_vectors_f80(angles: Vec3) -> [Vec3; 3] {
        let k = F80::from_f64(std::f64::consts::PI * 2.0 / 360.0);
        let sin_cos = |deg: f32| {
            let r = f64::from((F80::from_f32(deg) * k).to_f32());
            (F80::from_f32(r.sin() as f32), F80::from_f32(r.cos() as f32))
        };
        let (sp, cp) = sin_cos(angles.x);
        let (sy, cy) = sin_cos(angles.y);
        let (sr, cr) = sin_cos(angles.z);
        let v = |x: F80, y: F80, z: F80| Vec3::new(x.to_f32(), y.to_f32(), z.to_f32());
        let (srsp, crsp) = (-sr * sp, cr * sp);
        [
            v(cp * cy, cp * sy, -sp),
            v(srsp * cy + cr * sy, srsp * sy + -cr * cy, -sr * cp),
            v(crsp * cy + sr * sy, crsp * sy + -sr * cy, cr * cp),
        ]
    }

    /// Angles (as bits) and the forward, right and up retail's 1.1d
    /// `AngleVectors` (game.mp 0x3b228) wrote for them, read with gdb on
    /// the live server, whose x87 control word is 0x137f (64-bit
    /// precision). An `f64` sum lands each a float ulp off in one
    /// component (cod11-movers.md 22).
    const RETAIL_1_1D: [([u32; 3], [u32; 9]); 3] = [
        (
            [1099807040, 1102888640, 1025343488],
            [
                1063221737, 1052980346, 3197881319, 1053610912, 3211435836, 3123156517, 1049553211,
                1039673846, 1064557627,
            ],
        ),
        (
            [1079015936, 1128529664, 1117623216],
            [
                3212161202, 3196902753, 3177758401, 835481921, 1045372465, 3212490097, 3197097589,
                1064344752, 1044825817,
            ],
        ),
        (
            [1093149312, 1109722048, 1126746584],
            [
                1060977527, 1059453250, 3191521751, 3207358586, 1060556072, 3192111702, 3150844831,
                3196534067, 3212240672,
            ],
        ),
    ];

    fn bits(axes: [Vec3; 3]) -> [u32; 9] {
        let a = axes.map(|v| v.to_array().map(f32::to_bits));
        std::array::from_fn(|i| a[i / 3][i % 3])
    }

    #[test]
    fn the_kept_sums_round_at_64_bits() {
        for (angles, want) in RETAIL_1_1D {
            let a = Vec3::from_array(angles.map(f32::from_bits));
            assert_eq!(bits(angle_vectors(a)), want, "{a}");
            assert_ne!(bits(angle_vectors_f64(a)), want, "{a}");
        }
    }

    /// The `f64` path with its fallback gives what every step at 64 bits
    /// gives, near-cancelling sums included.
    #[test]
    fn the_fast_path_agrees_with_the_f80_steps() {
        let grid = |s: u32| s as f32 * (360.0 / 65536.0);
        for i in 0..200_000u32 {
            let a = Vec3::new(
                grid(i.wrapping_mul(7919) % 65536) - 90.0,
                grid(i.wrapping_mul(104_729) % 65536),
                grid(i.wrapping_mul(31) % 65536),
            );
            assert_eq!(bits(angle_vectors(a)), bits(angle_vectors_f80(a)), "{a}");
        }
        for (angles, _) in RETAIL_1_1D {
            let a = Vec3::from_array(angles.map(f32::from_bits));
            assert_eq!(bits(angle_vectors(a)), bits(angle_vectors_f80(a)));
        }
    }

    #[test]
    fn f80_divides_and_roots_to_nearest_even() {
        let f = F80::from_f64;
        // 1/3 at 64 bits: 0xaaaa..aaab * 2^-65.
        let third = f(1.0) / f(3.0);
        assert_eq!((third.m, third.e), (0xaaaa_aaaa_aaaa_aaab, -65));
        assert_eq!((f(1.0) / f(3.0)).to_f64(), 1.0 / 3.0);
        assert_eq!((f(-6.0) / f(4.0)).to_f64(), -1.5);
        // sqrt 2 at 64 bits: 0xb504f333f9de6484 (its next bit 0).
        let r2 = f(2.0).sqrt();
        assert_eq!((r2.m, r2.e), (0xb504_f333_f9de_6484, -63));
        assert_eq!(f(9.0).sqrt().to_f64(), 3.0);
        assert_eq!(f(0.25).sqrt().to_f64(), 0.5);
        assert!(f(1.0) - f(2f64.powi(-60)) < f(1.0));
        assert!(f(-1.0) < f(0.0) && f(2.0) > f(1.5));
    }

    #[test]
    fn f80_rounds_to_nearest_even() {
        let one = F80::from_f32(1.0);
        // 1 + 2^-64 is a tie at 64 bits and rounds to 1; 1 + 3 * 2^-65 up.
        let tie = F80::from_f64(2f64.powi(-64));
        assert_eq!(one + tie, one);
        assert_eq!(one + tie + tie, one);
        let up = F80::from_f64(3.0 * 2f64.powi(-65));
        assert_eq!((one + up).m, (1u64 << 63) + 1);
        assert_eq!((one + -one).to_f32().to_bits(), 0);
        assert_eq!(
            (F80::from_f32(-0.0) + F80::from_f32(-0.0))
                .to_f32()
                .to_bits(),
            0x8000_0000
        );
        assert_eq!(
            (F80::from_f32(0.1) * F80::from_f32(3.0)).to_f32(),
            0.1f32 * 3.0
        );
        let tiny = F80::from_f32(f32::from_bits(1));
        assert_eq!(tiny.to_f32().to_bits(), 1);
    }

    /// The cgames' float is `pi / 180` rounded once, Rust's `to_radians`
    /// factor; the Linux servers' double puts some shorts-grid angles a
    /// radian ulp off the cgames' (6052 of the 65536).
    #[test]
    fn the_windows_radians_are_not_the_servers() {
        assert_eq!(DEG2RAD_MSVC, (std::f64::consts::PI / 180.0) as f32);
        assert_eq!(1f32.to_radians(), DEG2RAD_MSVC);
        let differ = (0..65536)
            .map(|s| s as f32 * (360.0 / 65536.0))
            .filter(|&d| radians(d) != radians_msvc(d))
            .count();
        assert!(differ > 0);
    }
}
