//! CoD2's `Cloud` element: one particle that draws 1024 flakes out of one
//! shared vertex buffer, each a quad facing the view, through
//! `particle_cloud*.hlsl` (docs/research/cod2-efx.md 5.1). The sim keeps
//! the particle; this module turns it into the draw's constants
//! ([`FxCloud`]) and builds the buffer (`R_CreateParticleCloudBuffer`).
//! The renderer's `fx_cloud` pass draws it.

use glam::{Vec2, Vec3};

/// Flakes in the buffer: 8 x 8 x 16 cells of the unit cube.
pub const FLAKES: usize = 8 * 8 * 16;

/// One flake corner of the buffer: the flake's point in the cloud's
/// -1..1 cube, then the corner's texcoord.
pub type CloudVertex = [f32; 5];

/// A frame's clouds, far to near, and the view's right and up their
/// flakes face.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clouds {
    pub list: Vec<FxCloud>,
    pub right: Vec3,
    pub up: Vec3,
}

/// One cloud as `RB_TessParticleCloud` (Mac 0x000fdb0a) draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct FxCloud {
    /// The world matrix's rows (`RB_SetWorldMatrixForEntity`): the spin
    /// axes times the cloud's `size`.
    pub axis: [Vec3; 3],
    pub origin: Vec3,
    /// `particleCloudMatrix`: a corner's offset along the view's right and
    /// up is `(u - 0.5) * xy + (v - 0.5) * zw`.
    pub matrix: [f32; 4],
    /// `particleCloudColor`, 0..1.
    pub rgba: [f32; 4],
    pub shader: String,
    /// `Cloud::Cull`'s sphere radius about `origin`: the size plus the
    /// larger of the flake's width and length.
    pub cull_radius: f32,
}

/// `Cloud::Cloud` (Mac 0x000a98e6): up to four draws of a point in the
/// -1..1 cube until one lies between 0.1 and 1 of the centre, normalized;
/// (1, 0, 0) after four misses. `draw` is `flrand(-1, 1)`.
pub fn spin_axis(mut draw: impl FnMut() -> f32) -> Vec3 {
    for _ in 0..4 {
        let v = Vec3::new(draw(), draw(), draw());
        let d = v.length_squared();
        if (0.01..=1.0).contains(&d) {
            return v.normalize();
        }
    }
    Vec3::X
}

/// `AngleAxisToQuat` then `QuatToAxis` (Mac 0x0002dc44, 0x0002b5f0): the
/// axes turned `deg` about `axis`, as rows.
pub fn spin(deg: f32, axis: Vec3) -> [Vec3; 3] {
    let half = (f64::from(deg) * 0.008726646259971648) as f32;
    let (s, w) = half.sin_cos();
    let [x, y, z] = (axis * s).to_array();
    let k = 2.0 / (w * w + x * x + y * y + z * z);
    let (xx, yy, zz) = (x * x * k, y * y * k, z * z * k);
    let (xk, zy, yw, zw) = (x * k, z * y * k, y * k * w, z * k * w);
    [
        Vec3::new(1.0 - (yy + zz), y * xk + zw, z * xk - yw),
        Vec3::new(y * xk - zw, 1.0 - (zz + xx), xk * w + zy),
        Vec3::new(yw + z * xk, zy - xk * w, 1.0 - (xx + yy)),
    ]
}

/// `particleCloudMatrix` from the flake's `width` and `length` and the
/// cloud's motion over the frame in the view's right and up (`None` when
/// it held still). A square flake faces the view; a longer one stretches
/// along the motion, at least `width` long, only when the motion has a
/// right or up part of 0.001 or more (the test is signed, as retail's).
pub fn cloud_matrix(width: f32, length: f32, motion: Option<Vec2>) -> [f32; 4] {
    if width != length
        && let Some(v) = motion
        && (v.x >= 0.001 || v.y >= 0.001)
    {
        let len = (v.y * v.y + v.x * v.x).sqrt();
        let f = width / len;
        let zw = if len < width { v * f } else { v };
        return [f * v.y, f * -v.x, zw.x, zw.y];
    }
    [width, 0.0, 0.0, length]
}

impl FxCloud {
    /// A cloud particle at `pos` moving along `vel`, `t` through its life,
    /// turned `rot_deg` about `spin_axis`. `size` is the cube's half-extent;
    /// `width` is `size2` under `nonUniformScale`, else `size`; `length`
    /// is the `length` channel under `useLength`, else `width`
    /// (`Cloud::Update`, Mac 0x000a6638).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pos: Vec3,
        vel: Vec3,
        (size, width, length): (f32, f32, f32),
        rot_deg: f32,
        spin_axis: Vec3,
        rgba: [f32; 4],
        shader: &str,
        (right, up): (Vec3, Vec3),
    ) -> FxCloud {
        let motion = (width != length && vel.length_squared() > 1e-12).then(|| {
            let v = vel.normalize() * length;
            Vec2::new(v.dot(right), v.dot(up))
        });
        FxCloud {
            axis: spin(rot_deg, spin_axis).map(|a| a * size),
            origin: pos,
            matrix: cloud_matrix(width, length, motion),
            rgba,
            shader: shader.to_string(),
            cull_radius: size + width.max(length),
        }
    }

    #[cfg(test)]
    /// `vs_main`'s position for one buffer vertex, before the projection:
    /// the flake's world point plus its corner's offset along the view's
    /// right and up. The renderer's `fx_cloud.wgsl` does the same.
    pub fn corner(&self, v: &CloudVertex, right: Vec3, up: Vec3) -> Vec3 {
        let centre = self.flake(v);
        let (s, t) = (v[3] - 0.5, v[4] - 0.5);
        let [a, b, c, d] = self.matrix;
        let off = Vec2::new(s * a + t * c, s * b + t * d);
        centre + right * off.x + up * off.y
    }

    #[cfg(test)]
    /// A buffer vertex's flake in the world, what the outdoor lookup reads.
    pub fn flake(&self, v: &CloudVertex) -> Vec3 {
        self.origin + self.axis[0] * v[0] + self.axis[1] * v[1] + self.axis[2] * v[2]
    }
}

/// `R_CreateParticleCloudBuffer` (Mac 0x000ca59a): flake `i * 128 + j * 16
/// + k` sits at a random point of cell (i, j, k) of the -1..1 cube, 8 x 8
/// x 16 cells, its four corners at texcoords (0,0), (0,1), (1,0), (1,1)
/// and two triangles 0 1 2, 2 1 3. `rand01` stands in for retail's
/// `rand() / 2^31`, whose sequence at renderer start vcod cannot know.
pub fn buffer(mut rand01: impl FnMut() -> f32) -> (Vec<CloudVertex>, Vec<u16>) {
    const CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]];
    const QUAD: [u16; 6] = [0, 1, 2, 2, 1, 3];
    let mut verts = Vec::with_capacity(FLAKES * 4);
    let mut indices = Vec::with_capacity(FLAKES * 6);
    for i in 0..8 {
        for j in 0..8 {
            for k in 0..16 {
                let x = (rand01() + i as f32) * 0.25 - 1.0;
                let y = (rand01() + j as f32) * 0.25 - 1.0;
                let z = (rand01() + k as f32) * 0.125 - 1.0;
                let base = verts.len() as u16;
                verts.extend(CORNERS.map(|[u, v]| [x, y, z, u, v]));
                indices.extend(QUAD.map(|q| base + q));
            }
        }
    }
    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_fills_the_cube_cell_by_cell() {
        let (v, i) = buffer(|| 0.5);
        assert_eq!((v.len(), i.len()), (4096, 6144));
        assert_eq!(v[0], [-0.875, -0.875, -0.9375, 0.0, 0.0]);
        // flake 1 is the next z cell, flake 16 the next y, 128 the next x
        assert_eq!(v[4][2], -0.8125);
        assert_eq!(v[16 * 4][1], -0.625);
        assert_eq!(v[128 * 4][0], -0.625);
        assert_eq!(&i[6..12], &[4, 5, 6, 6, 5, 7]);
        assert_eq!(*i.iter().max().unwrap(), 4095);
    }

    #[test]
    fn the_spin_axis_takes_the_first_point_inside_the_shell() {
        let mut d = [0.0, 0.0, 0.0, 0.6, 0.0, 0.8].into_iter();
        assert_eq!(spin_axis(|| d.next().unwrap()), Vec3::new(0.6, 0.0, 0.8));
        assert_eq!(spin_axis(|| 1.0), Vec3::X);
    }

    #[test]
    fn a_spin_turns_about_its_axis() {
        let a = spin(90.0, Vec3::Z);
        assert!(a[0].abs_diff_eq(Vec3::Y, 1e-6), "{:?}", a[0]);
        assert!(a[1].abs_diff_eq(-Vec3::X, 1e-6), "{:?}", a[1]);
        assert!(a[2].abs_diff_eq(Vec3::Z, 1e-6));
        let id = spin(0.0, Vec3::new(0.6, 0.0, 0.8));
        assert_eq!(id, [Vec3::X, Vec3::Y, Vec3::Z]);
    }

    #[test]
    fn a_long_flake_stretches_only_along_right_or_up_motion() {
        // square: faces the view whatever the motion
        let m = Some(Vec2::new(30.0, 40.0));
        assert_eq!(cloud_matrix(2.0, 2.0, m), [2.0, 0.0, 0.0, 2.0]);
        // longer than wide: across the motion `width`, along it the motion
        // (in retail's order: width / |v|, then times the parts)
        let f = 2.0f32 / 50.0;
        assert_eq!(
            cloud_matrix(2.0, 50.0, m),
            [f * 40.0, f * -30.0, 30.0, 40.0]
        );
        // motion shorter than the width is scaled up to it
        let short = cloud_matrix(10.0, 50.0, Some(Vec2::new(3.0, 4.0)));
        assert_eq!(short, [8.0, -6.0, 6.0, 8.0]);
        // falling down and to the left on screen: no stretch
        let down = Some(Vec2::new(-3.0, -40.0));
        assert_eq!(cloud_matrix(2.0, 50.0, down), [2.0, 0.0, 0.0, 50.0]);
        assert_eq!(cloud_matrix(2.0, 50.0, None), [2.0, 0.0, 0.0, 50.0]);
    }

    #[test]
    fn a_corner_offsets_its_flake_in_the_view_plane() {
        let c = FxCloud::new(
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::ZERO,
            (300.0, 1.5, 1.5),
            0.0,
            Vec3::Z,
            [1.0; 4],
            "cloud_snowflake01",
            (Vec3::NEG_Y, Vec3::Z),
        );
        assert_eq!(c.cull_radius, 301.5);
        let v = [0.5, -0.5, 1.0, 1.0, 0.0];
        assert_eq!(c.flake(&v), Vec3::new(250.0, -150.0, 300.0));
        // u 1: half a width right (-y); v 0: half a length down
        assert_eq!(
            c.corner(&v, Vec3::NEG_Y, Vec3::Z),
            Vec3::new(250.0, -150.75, 299.25)
        );
    }
}
