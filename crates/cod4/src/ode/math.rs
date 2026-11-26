//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! ODE's vector, matrix and quaternion helpers as iw3mp.exe compiled them:
//! `f32` storage, the x87 at 53 bits between the binary's stores, so each
//! function here computes in `f64` and rounds where the binary writes a
//! DWORD (cod4-player-anims.md 10.3).

/// A vector (ODE's `dVector3` without its pad).
pub type V3 = [f32; 3];
/// A 3x3 matrix by rows (ODE's `dMatrix3` without its pad column).
pub type M3 = [[f32; 3]; 3];
/// A quaternion, `w` first (ODE's `dQuaternion`).
pub type Quat = [f32; 4];

pub const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `(a0 b0 + a1 b1) + a2 b2` on the x87 stack.
#[inline]
pub fn dot_wide(a: V3, b: V3) -> f64 {
    (a[0] as f64 * b[0] as f64 + a[1] as f64 * b[1] as f64) + a[2] as f64 * b[2] as f64
}

/// [`dot_wide`] stored.
#[inline]
pub fn dot(a: V3, b: V3) -> f32 {
    dot_wide(a, b) as f32
}

/// `m * v`, each row's dot stored (ODE's `dMULTIPLY0_331`).
pub fn mul0_331(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// `m^T * v`, each column's dot stored (ODE's `dMULTIPLY1_331`).
pub fn mul1_331(m: &M3, v: V3) -> V3 {
    let col = |j: usize| [m[0][j], m[1][j], m[2][j]];
    [dot(col(0), v), dot(col(1), v), dot(col(2), v)]
}

/// `a * b`, each element stored (ODE's `dMULTIPLY0_333`).
pub fn mul0_333(a: &M3, b: &M3) -> M3 {
    let col = |j: usize| [b[0][j], b[1][j], b[2][j]];
    std::array::from_fn(|i| std::array::from_fn(|j| dot(a[i], col(j))))
}

/// `a * b^T`, each element stored (ODE's `dMULTIPLY2_333`).
pub fn mul2_333(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| dot(a[i], b[j])))
}

/// `a x b`, each component `a1 b2 - a2 b1` stored.
pub fn cross(a: V3, b: V3) -> V3 {
    let c = |i: usize, j: usize| (a[i] as f64 * b[j] as f64 - a[j] as f64 * b[i] as f64) as f32;
    [c(1, 2), c(2, 0), c(0, 1)]
}

/// `a + b * s`, each component stored.
pub fn add_scaled(a: V3, b: V3, s: f32) -> V3 {
    std::array::from_fn(|i| (a[i] as f64 + b[i] as f64 * s as f64) as f32)
}

pub fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| (a[i] as f64 - b[i] as f64) as f32)
}

pub fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| (a[i] as f64 + b[i] as f64) as f32)
}

pub fn scale(a: V3, s: f32) -> V3 {
    std::array::from_fn(|i| (a[i] as f64 * s as f64) as f32)
}

/// The rows of `m` (its transpose).
pub fn transpose(m: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}
