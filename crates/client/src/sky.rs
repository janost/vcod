//! Sky geometry: the farbox cube sides and the cloud-dome grid, transcribed
//! from RTCW-MP `src/renderer/tr_sky.c`. Pure math; the GPU side lives in
//! `renderer.rs` (`SkyRender`) and `sky.wgsl`.

use vcod_common::bsp::DrawVert;

pub const SKY_SUBDIVISIONS: usize = 8;
const HALF: i32 = (SKY_SUBDIVISIONS / 2) as i32;

/// `radiusWorld` from R_InitSkyTexCoords (tr_sky.c :751).
const RADIUS_WORLD: f32 = 4096.0;

/// MakeSkyVec's `st_to_vec` (tr_sky.c :302): maps (s, t, 1) onto cube axes
/// 0..6 = +x, -x, +y, -y, top, bottom.
const ST_TO_VEC: [[i8; 3]; 6] = [
    [3, -1, 2],
    [-3, 1, 2],
    [1, 3, 2],
    [-1, -3, 2],
    [-2, -1, 3], // 0 degrees yaw, look straight up
    [2, -1, -3], // look straight down
];

/// Cube axis -> env-image suffix. ParseSkyParms loads suffixes
/// {rt, bk, lf, ft, up, dn} into outerbox[0..5] and DrawSkyBox renders axis i
/// with outerbox[sky_texorder[i]] ({0,2,1,3,4,5}), so by axis:
/// +x rt, -x lf, +y bk, -y ft, +z up, -z dn.
pub const AXIS_SUFFIX: [&str; 6] = ["rt", "lf", "bk", "ft", "up", "dn"];

/// MakeSkyVec's box distance: zFar/1.75 ("div sqrt(3)"), never near-clipped.
pub fn box_size(z_far: f32, z_near: f32) -> f32 {
    (z_far / 1.75).max(z_near * 2.0)
}

/// Unit-size cube-side point for grid coordinate (s, t) in [-1, 1].
pub fn make_sky_vec(s: f32, t: f32, axis: usize) -> [f32; 3] {
    let b = [s, t, 1.0];
    let mut out = [0.0; 3];
    for (j, &k) in ST_TO_VEC[axis].iter().enumerate() {
        out[j] = if k < 0 {
            -b[-k as usize - 1]
        } else {
            b[k as usize - 1]
        };
    }
    out
}

/// Box-side uv per MakeSkyVec's outSt with sky_min 0 / sky_max 1 (DrawSkyBox).
fn box_uv(s: f32, t: f32) -> [f32; 2] {
    let s = ((s + 1.0) * 0.5).clamp(0.0, 1.0);
    let mut t = ((t + 1.0) * 0.5).clamp(0.0, 1.0);
    t = 1.0 - t;
    [s, t]
}

/// Cloud-layer uv for a ray direction, verbatim from R_InitSkyTexCoords
/// (tr_sky.c :749): intersect with the sphere of radius RADIUS_WORLD+height
/// centred at (0, 0, -RADIUS_WORLD), shift down to origin-centred, normalize,
/// then acos of x and y. The raw radians are the uv; stage tcMods scale them
/// onto the repeating cloud texture.
pub fn cloud_uv(dir: [f32; 3], height: f32) -> [f32; 2] {
    let sq = |a: f32| a * a;
    let [x, y, z] = dir;
    let h2 = sq(height);
    let r = sq(z) * sq(RADIUS_WORLD)
        + 2.0 * sq(x) * RADIUS_WORLD * height
        + sq(x) * h2
        + 2.0 * sq(y) * RADIUS_WORLD * height
        + sq(y) * h2
        + 2.0 * sq(z) * RADIUS_WORLD * height
        + sq(z) * h2;
    let p = (1.0 / (2.0 * (sq(x) + sq(y) + sq(z)))) * (-2.0 * z * RADIUS_WORLD + 2.0 * r.sqrt());
    // p*dir lands on the sphere around (0,0,-R); +R re-centres it.
    let vx = x * p;
    let vy = y * p;
    let vz = z * p + RADIUS_WORLD;
    let len = (sq(vx) + sq(vy) + sq(vz)).sqrt();
    [
        (vx / len).clamp(-1.0, 1.0).acos(),
        (vy / len).clamp(-1.0, 1.0).acos(),
    ]
}

/// One farbox side at `size`: full subdivided quad grid.
pub struct SideMesh {
    pub verts: Vec<SkyBoxVert>,
    pub indices: Vec<u32>,
}

/// Farbox vertex; stride 20, mirrored by sky.wgsl's VsIn.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkyBoxVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

/// Quad indices over an `(SKY_SUBDIVISIONS+1)^2` row-major (t, s) grid,
/// FillCloudySkySide's index pattern (tr_sky.c :597).
fn grid_indices(base: u32) -> Vec<u32> {
    let w = SKY_SUBDIVISIONS as u32 + 1;
    let mut idx = Vec::new();
    for t in 0..SKY_SUBDIVISIONS as u32 {
        for s in 0..SKY_SUBDIVISIONS as u32 {
            let v = base + s + t * w;
            let vd = base + s + (t + 1) * w;
            idx.extend_from_slice(&[v, vd, v + 1, vd, vd + 1, v + 1]);
        }
    }
    idx
}

fn grid_point(si: usize, ti: usize, axis: usize) -> ([f32; 3], [f32; 2]) {
    let s = (si as f32 - HALF as f32) / HALF as f32;
    let t = (ti as f32 - HALF as f32) / HALF as f32;
    (make_sky_vec(s, t, axis), box_uv(s, t))
}

/// Farbox side `axis` scaled to `size`.
pub fn box_side(axis: usize, size: f32) -> SideMesh {
    let n = SKY_SUBDIVISIONS;
    let mut verts = Vec::with_capacity((n + 1) * (n + 1));
    for ti in 0..=n {
        for si in 0..=n {
            let (p, uv) = grid_point(si, ti, axis);
            verts.push(SkyBoxVert {
                pos: [p[0] * size, p[1] * size, p[2] * size],
                uv,
            });
        }
    }
    SideMesh {
        verts,
        indices: grid_indices(0),
    }
}

/// The cloud dome: every cube face but the bottom (FillCloudBox skips axis 5),
/// positions on the box grid, uvs from [`cloud_uv`]. DrawVert-shaped so the
/// dome draws through the stage pipelines' vertex layout.
pub fn dome_mesh(size: f32, cloud_height: f32) -> (Vec<DrawVert>, Vec<u32>) {
    let n = SKY_SUBDIVISIONS;
    let mut verts: Vec<DrawVert> = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..6 {
        if axis == 5 {
            continue;
        }
        let base = verts.len() as u32;
        for ti in 0..=n {
            for si in 0..=n {
                let s = (si as f32 - HALF as f32) / HALF as f32;
                let t = (ti as f32 - HALF as f32) / HALF as f32;
                let dir = make_sky_vec(s, t, axis);
                let uv = cloud_uv(dir, cloud_height);
                verts.push(DrawVert {
                    pos: [dir[0] * size, dir[1] * size, dir[2] * size],
                    uv,
                    lm_uv: [0.0; 2],
                    normal: [0.0, 0.0, 1.0],
                    color: [255; 4],
                });
            }
        }
        indices.extend(grid_indices(base));
    }
    (verts, indices)
}

/// Picks the active sky block: highest soup reference count wins, ties go to
/// the alphabetically first name, an empty list means no sky pass.
pub fn pick_sky(candidates: &[(String, u32)]) -> Option<String> {
    candidates
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, _)| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn make_sky_vec_axes_point_their_way() {
        let eps = 1e-6;
        let v = |s, t, a| make_sky_vec(s, t, a);
        // +x face: dominant +x, s runs toward -y, t toward +z
        assert!(v(0.0, 0.0, 0)[0] > 1.0 - eps && v(0.0, 0.0, 0)[1].abs() < eps);
        assert!(v(1.0, 0.0, 0)[1] < -1.0 + eps);
        assert!(v(0.0, 1.0, 0)[2] > 1.0 - eps);
        // -x face
        assert!(v(0.0, 0.0, 1)[0] < -1.0 + eps);
        // +y / -y faces
        assert!(v(0.0, 0.0, 2)[1] > 1.0 - eps);
        assert!(v(0.0, 0.0, 3)[1] < -1.0 + eps);
        // top / bottom
        assert!(v(0.0, 0.0, 4)[2] > 1.0 - eps);
        assert!(v(0.0, 0.0, 5)[2] < -1.0 + eps);
    }

    /// The zenith ray hits the cloud plane directly overhead and lands at
    /// (pi/2, pi/2); a horizontal ray yields an extreme (smaller) angle.
    #[test]
    fn cloud_uv_zenith_and_horizon() {
        let h = 512.0;
        let up = cloud_uv([0.0, 0.0, 1.0], h);
        assert!(close(up[0], std::f32::consts::FRAC_PI_2), "{up:?}");
        assert!(close(up[1], std::f32::consts::FRAC_PI_2), "{up:?}");
        // straight east: v.x pulls away from the vertical, so acos shrinks
        let east = cloud_uv([1.0, 0.0, 0.0], h);
        assert!(east[0] > 0.0 && east[0] < up[0], "{east:?}");
        assert!(close(east[1], std::f32::consts::FRAC_PI_2), "{east:?}");
        // scale-invariance: the construction only uses normalized directions
        let big = cloud_uv([100.0, 0.0, 0.0], h);
        assert!((east[0] - big[0]).abs() < 1e-4);
    }

    #[test]
    fn dome_mesh_omits_the_bottom_face_and_covers_five() {
        let (verts, idx) = dome_mesh(34_000.0, 512.0);
        // 5 faces x 9x9 vertices, 5 x 64 quads x 6 indices
        assert_eq!(verts.len(), 5 * 81);
        assert_eq!(idx.len(), 5 * 64 * 6);
        // every vertex sits on one of the four walls or the top: the bottom
        // face itself is gone even though wall edges reach down to -size
        let size = 34_000.0;
        let eq = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(verts.iter().all(|v| {
            let p = v.pos;
            eq(p[0].abs(), size) || eq(p[1].abs(), size) || eq(p[2], size)
        }));
        // max index stays inside the vertex set
        assert_eq!(idx.iter().copied().max(), Some(verts.len() as u32 - 1));
    }

    #[test]
    fn dome_top_center_vertex_is_straight_overhead() {
        let size = 34_000.0;
        let (verts, _) = dome_mesh(size, 512.0);
        // top face starts after 4*81 verts; its centre grid point is (4,4)
        let top_base = 4 * 81;
        let centre = &verts[top_base + 4 * 9 + 4];
        assert!(close(centre.pos[0], 0.0) && close(centre.pos[1], 0.0));
        assert!(close(centre.pos[2], size));
        let uv = centre.uv;
        assert!(close(uv[0], std::f32::consts::FRAC_PI_2), "{uv:?}");
    }

    #[test]
    fn box_side_geometry_spans_the_cube() {
        let size = 10.0;
        for axis in 0..6 {
            let side = box_side(axis, size);
            assert_eq!(side.verts.len(), 81);
            assert_eq!(side.indices.len(), 64 * 6);
            let dom = axis / 2; // axes come in +/- pairs: x,x,y,y,z,z
                                // even axes (+x,+y,top) face positive, odd ones negative
            let want = if axis % 2 == 0 { size } else { -size };
            for v in &side.verts {
                assert!(close(v.pos[dom], want), "axis {axis} vert {:?}", v.pos);
            }
        }
    }

    #[test]
    fn box_uvs_cover_the_full_image_with_t_flipped() {
        // (-1,-1) is image left, BOTTOM row after the flip; (+1,+1) right/top.
        assert_eq!(box_uv(-1.0, -1.0), [0.0, 1.0]);
        assert_eq!(box_uv(1.0, 1.0), [1.0, 0.0]);
        assert_eq!(box_uv(0.0, 0.0), [0.5, 0.5]);
    }

    #[test]
    fn box_size_divides_by_sqrt3_but_never_near_clips() {
        assert!(close(box_size(60_000.0, 4.0), 60_000.0 / 1.75));
        assert!(close(box_size(2.0, 4.0), 8.0), "clamp to 2*zNear");
        assert!(close(box_size(14.0, 4.0), 8.0), "14/1.75 = 8 exactly");
    }

    #[test]
    fn pick_sky_prefers_refs_then_alphabetical() {
        let cands = |pairs: &[(&str, u32)]| {
            pick_sky(
                &pairs
                    .iter()
                    .map(|(n, r)| ((*n).to_string(), *r))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(cands(&[]), None, "no candidates, no sky pass");
        assert_eq!(
            cands(&[("textures/sky/b", 3), ("textures/sky/a", 7)]).as_deref(),
            Some("textures/sky/a")
        );
        assert_eq!(
            cands(&[("textures/sky/b", 3), ("textures/sky/a", 3)]).as_deref(),
            Some("textures/sky/a"),
            "tie breaks alphabetically"
        );
        // zero-reference candidates stay eligible (presence ranks, not wins)
        assert_eq!(
            cands(&[("textures/sky/b", 0), ("textures/sky/a", 0)]).as_deref(),
            Some("textures/sky/a")
        );
    }
}
