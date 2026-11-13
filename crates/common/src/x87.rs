//! Vector arithmetic rounded the way the 1.1 binaries' x87 code rounds it:
//! every intermediate kept on the 80-bit stack (an `f64` here) and one
//! rounding to `f32` where the binary stores. The game module's
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
/// Offensive shares: the squared length, the length and its reciprocal each
/// stored as a float, every component rounded once. A zero vector is left
/// as it was.
pub fn normalize_stored(v: Vec3) -> Vec3 {
    let len = f64::from(dot(v, v) as f32).sqrt() as f32;
    if len == 0.0 {
        return v;
    }
    let inv = 1.0 / len;
    (v.as_dvec3() * f64::from(inv)).as_vec3()
}

/// `AngleVectors` as all three Linux servers round it, engine and game
/// module alike (`cod_lnxded` 0x8063388, 1.5b 0x806759d, UO 0x80674ad;
/// game.mp 0x3b228, 1.5b 0x33e18, UO 0x3b068): forward, right, up. Each
/// angle times the double `M_PI * 2 / 360` is stored as a float, its
/// `fsincos` stored as floats, and each component's sum of products kept on
/// the stack until one store. Rounding every product to a float instead
/// turns a pitched, yawed and rolled model by a few ulps, and a slide down
/// its face ends thousandths short (movers doc 22).
pub fn angle_vectors(angles: Vec3) -> [Vec3; 3] {
    angle_vectors_of_radians(Vec3::new(
        radians(angles.x),
        radians(angles.y),
        radians(angles.z),
    ))
}

/// The radians the Linux builds' `AngleVectors` turn a degree angle into:
/// times the double `M_PI * 2 / 360`, stored as a float. pmove's
/// `ps.yaw` and `-ps.pitch` hold exactly these (`cmd::apply_view`).
pub fn radians(deg: f32) -> f32 {
    (f64::from(deg) * (std::f64::consts::PI * 2.0 / 360.0)) as f32
}

/// [`angle_vectors`] of angles already turned to radians by [`radians`].
pub fn angle_vectors_of_radians(rad: Vec3) -> [Vec3; 3] {
    let sin_cos = |r: f32| {
        let r = f64::from(r);
        (f64::from(r.sin() as f32), f64::from(r.cos() as f32))
    };
    let (sp, cp) = sin_cos(rad.x);
    let (sy, cy) = sin_cos(rad.y);
    let (sr, cr) = sin_cos(rad.z);
    let forward = DVec3::new(cp * cy, cp * sy, -sp);
    let right = DVec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = DVec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    [forward.as_vec3(), right.as_vec3(), up.as_vec3()]
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

#[cfg(test)]
mod tests {
    use super::*;

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
