//! Vector arithmetic rounded the way the 1.1 binaries' x87 code rounds it:
//! every intermediate kept on the 80-bit stack (an `f64` here) and one
//! rounding to `f32` where the binary stores. The game module's
//! `VectorNormalize` (game.mp 0x3d8d8) and `CrossProduct` (0x3d86c) and the
//! engine's copies (`cod_lnxded` 0x8065a38, 0x80659cc) round alike.

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

/// `VectorNormalize`: the length stored as a float, each component
/// multiplied by its unrounded reciprocal and rounded once. Zero stays zero.
pub fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt() as f32;
    if len == 0.0 {
        return Vec3::ZERO;
    }
    (v.as_dvec3() * (1.0 / f64::from(len))).as_vec3()
}

/// `VectorScale` by a factor left on the stack.
pub fn scale(v: Vec3, s: f64) -> Vec3 {
    (v.as_dvec3() * DVec3::splat(s)).as_vec3()
}
