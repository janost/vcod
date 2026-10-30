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
/// The pick's colours carry identityLight 0.5; `identity_light` rescales
/// them to the current one, ahead of GL's clamp.
pub fn pack(l: &EntityLights, to_view: Option<Mat4>, identity_light: f32) -> GpuLightSet {
    let m = to_view.unwrap_or(Mat4::IDENTITY);
    let k = identity_light / 0.5;
    let amb = l.ambient * k;
    let mut set = GpuLightSet {
        ambient: [amb.x, amb.y, amb.z, 0.0],
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
        let d = light.diffuse * *w * k;
        let a = light.ambient * *w * k;
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

/// `LightSet::ambient.w` of a CoD4 entity's set (cod2_model.wgsl's
/// `cod4_model`); a CoD2 probe set's is -1.
pub const COD4_BLOCK: f32 = -2.0;

/// A CoD4 entity's lighting and the sun, packed for cod2_model.wgsl's
/// `cod4_model` (cod4-world.md 3.3): the ambient the sun's diffuse colour,
/// w [`COD4_BLOCK`]; light 0's `pos` the unit vector toward the sun, w
/// identityLight; light 0's `diffuse.y` the reflection probe's layer and
/// `.zw` the centre of entity volume block `block` ([`cod4_block_base`]),
/// which holds the lookup's texels ([`cod4_volume_rows`]). The sun's
/// specular is its diffuse (`r_specularColorScale` 1, cod4-world.md 9.6).
///
/// A lookup settled on a spot or omni light `source` (cod4-world.md 3.5)
/// takes it in the sun's place from float [`COD4_SOURCE`] on:
/// `lightPosition` (origin, one over the radius), the colour with w 1 for
/// an omni light and 2 for a spot, `lightSpotDir` with w the attenuation
/// row's length, `lightSpotFactors`, then up to [`COD4_ATTENUATION`]
/// texels of the row, each the integer `r + 256 g + 65536 b`.
pub fn pack_cod4(
    light: &vcod_common::mapworld::EntityLight,
    block: u32,
    sun_dir: Vec3,
    sun_colour: Vec3,
    source: Option<&vcod_common::mapworld::ModelLightSource>,
    identity_light: f32,
) -> GpuLightSet {
    let mut set = GpuLightSet {
        ambient: [sun_colour.x, sun_colour.y, sun_colour.z, COD4_BLOCK],
        ..GpuLightSet::default()
    };
    set.lights[0].pos = [sun_dir.x, sun_dir.y, sun_dir.z, identity_light];
    let [u, v] = cod4_block_base(block);
    set.lights[0].diffuse[1] = f32::from(light.probe);
    set.lights[0].diffuse[2] = u;
    set.lights[0].diffuse[3] = v;
    let floats: &mut [f32; 164] = bytemuck::cast_mut(&mut set);
    let rgb =
        |[r, g, b, _]: &[u8; 4]| (u32::from(*r) | u32::from(*g) << 8 | u32::from(*b) << 16) as f32;
    if let Some(l) = source {
        let row = &l.attenuation[..l.attenuation.len().min(COD4_ATTENUATION)];
        let kind = if l.spot { 2.0 } else { 1.0 };
        let at = COD4_SOURCE;
        floats[at..at + 16].copy_from_slice(&[
            l.origin[0],
            l.origin[1],
            l.origin[2],
            l.inv_radius,
            l.colour[0],
            l.colour[1],
            l.colour[2],
            kind,
            l.dir[0],
            l.dir[1],
            l.dir[2],
            row.len() as f32,
            l.spot_factors[0],
            l.spot_factors[1],
            l.spot_factors[2],
            0.0,
        ]);
        for (f, t) in floats[at + 16..].iter_mut().zip(row) {
            *f = rgb(t);
        }
    }
    set
}

/// Blocks along a row of the entity model lighting volume: its 256 texels
/// across, as 1.7's (cod4-world.md 3.2).
pub const COD4_VOLUME_ROW: u32 = 64;
/// Rows of blocks in the entity volume: 1024 blocks, 1.7's
/// `xmodelEntryLimit` for one client view (`maxClientViews << 10`).
/// cod2_model.wgsl's `COD4_LOOKUP_SCALE` reads its height.
pub const COD4_VOLUME_ROWS: u32 = 16;
/// The entity volume's blocks; the last is the view model's.
pub const COD4_VOLUME_BLOCKS: u32 = COD4_VOLUME_ROW * COD4_VOLUME_ROWS;

/// Block `i`'s centre in the entity volume's x and y, 1.7's
/// `R_SetModelLightingCoords` (`baseLightingCoords`, z 0.5).
pub fn cod4_block_base(i: u32) -> [f32; 2] {
    [
        (4 * (i % COD4_VOLUME_ROW) + 2) as f32 / (4 * COD4_VOLUME_ROW) as f32,
        (4 * (i / COD4_VOLUME_ROW) + 2) as f32 / (4 * COD4_VOLUME_ROWS) as f32,
    ]
}

/// Blocks `0..blocks.len()` laid out as the first `4 * rows` texel rows of
/// the entity volume, its four slices one after another, RGBA (texel
/// `x + 4 y + 16 z` of a block at `(4 (i % 64) + x, 4 (i / 64) + y, z)`);
/// the rows' count, at least one.
pub fn cod4_volume_rows(blocks: &[[[u8; 4]; 64]]) -> (u32, Vec<u8>) {
    let rows = (blocks.len() as u32).div_ceil(COD4_VOLUME_ROW).max(1);
    let (w, h) = ((4 * COD4_VOLUME_ROW) as usize, (4 * rows) as usize);
    let mut rgba = vec![0u8; w * h * 4 * 4];
    for (i, texels) in blocks.iter().enumerate() {
        let (bx, by) = (
            4 * (i % COD4_VOLUME_ROW as usize),
            4 * (i / COD4_VOLUME_ROW as usize),
        );
        for (t, px) in texels.iter().enumerate() {
            let (x, y, z) = (t & 3, (t >> 2) & 3, t >> 4);
            let at = 4 * ((z * h + by + y) * w + bx + x);
            rgba[at..at + 4].copy_from_slice(px);
        }
    }
    (rows, rgba)
}

/// Where [`pack_cod4`] puts a spot or omni light, in floats of the set.
pub const COD4_SOURCE: usize = 88;
/// The most attenuation texels [`pack_cod4`] keeps (every stock light
/// def's image is 32 wide).
pub const COD4_ATTENUATION: usize = 32;
const _: () = assert!(COD4_SOURCE + 16 + COD4_ATTENUATION <= 164);

/// The fx `Light` blocks as scene lights; the block's size is
/// `RE_AddLightToScene`'s intensity (the efx light draw, 0x490625).
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

    /// A block's texels land from light 1's first float on, R low.
    #[test]
    fn cod4_set_names_its_block_and_the_block_lands_in_the_volume() {
        let block: [[u8; 4]; 64] = std::array::from_fn(|t| [t as u8, 1, 2, 204]);
        let light = vcod_common::mapworld::EntityLight {
            texels: block,
            probe: 3,
            primary: 1,
        };
        let set = pack_cod4(&light, 65, Vec3::Z, Vec3::splat(0.5), None, 1.0);
        assert_eq!(set.ambient, [0.5, 0.5, 0.5, COD4_BLOCK]);
        assert_eq!(set.lights[0].pos, [0.0, 0.0, 1.0, 1.0]);
        // block 65: the second row's second block, its centre texel 6, 6
        assert_eq!(set.lights[0].diffuse[1..], [3.0, 6.0 / 256.0, 6.0 / 64.0]);
        let mut blocks = vec![[[0; 4]; 64]; 66];
        blocks[65] = block;
        let (rows, rgba) = cod4_volume_rows(&blocks);
        assert_eq!(rows, 2);
        // texel 27 (x 3, y 2, z 1) at (7, 6) of slice 1, 8 rows a slice
        let at = 4 * ((8 + 6) * 256 + 7);
        assert_eq!(rgba[at..at + 4], [27, 1, 2, 204]);
    }

    #[test]
    fn weight_folds_into_colours_and_view_moves_positions() {
        let l = Light::dynamic(Vec3::new(10.0, 0.0, 0.0), Vec3::ONE, 8.0);
        let lights = EntityLights {
            ambient: Vec3::splat(0.25),
            lights: vec![(l, 0.5)],
        };
        let to_view = Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0));
        let set = pack(&lights, Some(to_view), 0.5);
        assert_eq!(set.ambient, [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(set.lights[0].pos, [0.0, 0.0, 0.0, 1.0]);
        // 0.5 * 64 / 32 * 0.5
        assert_eq!(set.lights[0].diffuse[0], 0.5);
        assert_eq!(set.lights[0].ambient[3], -2.0);
        assert_eq!(set.lights[0].atten, [0.001, 0.0, 1.0, 0.0]);
        assert_eq!(set.lights[1], GpuLight::default());
        // windowed (identityLight 1) doubles every colour
        let w = pack(&lights, None, 1.0);
        assert_eq!((w.ambient[0], w.lights[0].diffuse[0]), (0.5, 1.0));
    }
}
