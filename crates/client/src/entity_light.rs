//! Entity and viewmodel lighting: the light set `StaticLighting::entity_lights`
//! picks, packed for `dynamic_model.wgsl` and `viewmodel.wgsl`, which apply
//! it as GL's fixed-function lighting did (docs/research/
//! cod11-light-grid-and-leaf-lights.md, section 13).

use crate::fx::sim::FxLight;
use glam::{Mat4, Vec3};
use vcod_common::static_light::{EntityLights, SceneLight};

/// GL's light count, which `r_maxEntLights` cannot pass.
pub const MAX_ENT_LIGHTS: usize = 8;

/// One GL light. Matches `EntLight` in the two WGSL modules.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    /// xyz position, or for w 0 the unit vector towards the light.
    pub pos: [f32; 4],
    /// rgb diffuse times the weight; w the spot exponent.
    pub diffuse: [f32; 4],
    /// rgb ambient times the weight; w the cosine of the spot cutoff, below
    /// -1 for no cone.
    pub ambient: [f32; 4],
    /// Constant, linear, quadratic falloff; w unused.
    pub atten: [f32; 4],
    /// xyz the spot direction (`GL_SPOT_DIRECTION`).
    pub spot_dir: [f32; 4],
}

/// One model's lights. Matches `LightSet` in the two WGSL modules.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLightSet {
    /// rgb the light model ambient; w the light count.
    pub ambient: [f32; 4],
    pub lights: [GpuLight; MAX_ENT_LIGHTS],
}

const _: () = assert!(std::mem::size_of::<GpuLightSet>() == 656);

/// Packs a pick, folding each weight into the light's colours as 0x4d63a0
/// does. `to_view` moves positions and directions into the space the
/// shader lights in (the viewmodel's view space); `None` keeps world space.
pub fn pack(l: &EntityLights, to_view: Option<Mat4>) -> GpuLightSet {
    let m = to_view.unwrap_or(Mat4::IDENTITY);
    let mut set = GpuLightSet {
        ambient: [l.ambient.x, l.ambient.y, l.ambient.z, 0.0],
        ..GpuLightSet::default()
    };
    let n = l.lights.len().min(MAX_ENT_LIGHTS);
    set.ambient[3] = n as f32;
    for (g, (light, w)) in set.lights.iter_mut().zip(&l.lights) {
        let pos = if light.directional {
            m.transform_vector3(light.origin).extend(0.0)
        } else {
            m.transform_point3(light.origin).extend(1.0)
        };
        let cone = if light.spot_cutoff >= 180.0 {
            -2.0
        } else {
            light.spot_cutoff.to_radians().cos()
        };
        let d = light.diffuse * *w;
        let a = light.ambient * *w;
        *g = GpuLight {
            pos: pos.to_array(),
            diffuse: [d.x, d.y, d.z, light.spot_exponent],
            ambient: [a.x, a.y, a.z, cone],
            atten: [light.atten[0], light.atten[1], light.atten[2], 0.0],
            spot_dir: m.transform_vector3(light.spot_dir).extend(0.0).to_array(),
        };
    }
    set
}

/// The fx `Light` blocks as scene lights; the block's size stands in for
/// `RE_AddLightToScene`'s intensity (not traced).
pub fn scene_lights(lights: &[FxLight]) -> Vec<SceneLight> {
    lights
        .iter()
        .map(|l| SceneLight {
            origin: Vec3::from_array(l.pos),
            color: Vec3::from_array(l.rgb),
            intensity: l.radius,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::static_light::Light;

    #[test]
    fn weight_folds_into_colours_and_view_moves_positions() {
        let l = Light::dynamic(Vec3::new(10.0, 0.0, 0.0), Vec3::ONE, 8.0);
        let lights = EntityLights {
            ambient: Vec3::splat(0.25),
            lights: vec![(l, 0.5)],
        };
        let to_view = Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0));
        let set = pack(&lights, Some(to_view));
        assert_eq!(set.ambient, [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(set.lights[0].pos, [0.0, 0.0, 0.0, 1.0]);
        // 0.5 * 64 / 32 * 0.5
        assert_eq!(set.lights[0].diffuse[0], 0.5);
        assert_eq!(set.lights[0].ambient[3], -2.0);
        assert_eq!(set.lights[0].atten, [0.001, 0.0, 1.0, 0.0]);
        assert_eq!(set.lights[1], GpuLight::default());
    }
}
