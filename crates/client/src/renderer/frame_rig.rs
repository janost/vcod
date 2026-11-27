//! What the headless render checks and the CoD2 profiles share: a headless
//! `Renderer` at the checks' size and the frames drawn through it.

use vcod_common::asset_source::AssetSource;

use glam::Vec3;

use super::headless::Errors;
use super::*;
use crate::fx::sim::FxQuad;

pub(super) const SIZE: (u32, u32) = (320, 240);

impl Renderer {
    /// [`Renderer::new`] without a window: the same device features, limits
    /// and build, drawing into an offscreen target of the swapchain's
    /// format. `None` without an adapter that has BC textures.
    pub(super) fn headless(
        fs: &dyn AssetSource,
        hdr: bool,
        samples: u32,
    ) -> Option<(Renderer, Errors)> {
        Self::headless_sized(fs, hdr, samples, SIZE)
    }
}

/// A camera quad `ahead` units down the view from `eye`, with `shader`.
pub(super) fn fx_quad(eye: Vec3, yaw: f32, ahead: f32, shader: &str) -> FxQuad {
    let (f, r, u) = crate::camera::basis(yaw.to_radians(), 0.0);
    let c = eye + f * ahead;
    let s = 16.0;
    FxQuad {
        verts: [
            c - r * s + u * s,
            c + r * s + u * s,
            c + r * s - u * s,
            c - r * s - u * s,
        ]
        .map(|v| v.to_array()),
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba: [1.0; 4],
        shader: shader.into(),
    }
}

/// Effects, a mark, HUD quads and text lines in front of `eye`, then
/// two frames (the shadow layer's first redraw and a steady one), the
/// view weapon `vm` over them.
pub(super) fn draw_frames(
    r: &mut Renderer,
    fs: &Pk3Fs,
    (eye, yaw): (Vec3, f32),
    fx: &[&str],
    vm: impl Fn() -> Option<VmDraw>,
) {
    let quads = fx
        .iter()
        .enumerate()
        .map(|(i, s)| fx_quad(eye, yaw, 64.0 + 32.0 * i as f32, s))
        .collect();
    r.set_fx_quads(fs, quads, 1.0);
    // a CoD2 snow cloud through the cloud pass (no map: drawn untested)
    let (f, right, up) = crate::camera::basis(yaw.to_radians(), 0.0);
    let cloud = crate::fx::cod2_cloud::FxCloud::new(
        eye + f * 200.0,
        Vec3::new(0.0, 0.0, -50.0),
        (100.0, 1.5, 1.5),
        30.0,
        Vec3::Z,
        [1.0; 4],
        "cloud_snowflake01",
        (right, up),
    );
    let clouds = crate::fx::cod2_cloud::Clouds {
        list: vec![cloud],
        right,
        up,
    };
    r.set_fx_clouds(fs, None, &clouds, 1.0);
    r.set_fx_lights(&[crate::fx::sim::FxLight {
        pos: eye.to_array(),
        radius: 200.0,
        rgb: [1.0, 0.5, 0.2],
    }]);
    let mark = crate::marks::MarkRequest {
        shader: "gfx/impact/bullethole1".into(),
        origin: eye - Vec3::Z * 60.0,
        dir: Vec3::Z,
        orientation: 0.0,
        rgba: [1.0; 4],
        alpha_fade: true,
        radius: 8.0,
        life_ms: 8000,
    };
    r.set_marks(fs, &[mark], &[], 1.0, true);
    r.set_hud_quads(
        fs,
        // A plain quad, then one through the additive pipelines (the font
        // glow's).
        ["white".to_string(), format!("{}white", hud::ADDITIVE)]
            .into_iter()
            .map(|texture| HudQuad {
                verts: [[10.0, 10.0], [60.0, 10.0], [60.0, 60.0], [10.0, 60.0]],
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0; 4],
                texture,
            })
            .collect(),
    );
    let fov = 80.0;
    for time in [1.0, 1.05] {
        let (fwd, _, up) = crate::camera::basis(yaw.to_radians(), 0.0);
        let frame = Frame {
            view_proj: crate::camera::view_proj_from(
                eye,
                yaw.to_radians(),
                0.0,
                0.0,
                fov,
                r.aspect(),
            ),
            eye,
            fwd,
            up,
            time,
            fov_x: fov,
            cull: CullMode::On,
            hud_lines: vec!["pipeline check".into()],
        };
        r.render(frame, vm().map(|v| VmDraw { fov_x: fov, ..v }));
    }
    r.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the frames finish");
}

pub(super) fn assert_clean(errors: &Errors, what: &str) {
    let errors = errors.lock().unwrap();
    assert!(
        errors.is_empty(),
        "{what}: {} wgpu validation error(s):\n{}",
        errors.len(),
        errors.join("\n---\n")
    );
}
