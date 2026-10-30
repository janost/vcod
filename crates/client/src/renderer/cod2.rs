//! CoD2's world shading (docs/research/cod2-materials.md 9): the
//! lightmapped phong family (`cod2_world.wgsl`) and the cube-map sky
//! (`sky_cube.wgsl`). A CoD2 world batch whose material carries
//! [`PhongLit`] draws through [`Cod2World`] instead of its stage; every
//! other CoD2 material, and every CoD1 one, keeps the stage path, so no
//! CoD1 pipeline or key changes.

use std::collections::HashMap;
use vcod_common::asset_source::AssetSource;
use wgpu::util::DeviceExt;

use super::{
    BundleViews, DEPTH_FORMAT, SkyBox, StageVariant, depth_bias, msaa_samples, upload_bundle_view,
    upload_levels,
};
use crate::camera::{Z_FAR, Z_NEAR};
use crate::sky;
use crate::texture_settings;
use vcod_common::asset_source::Asset;
use vcod_common::assets::{Image, ImageData, ImageMissing};
use vcod_common::mapworld::{LightmapBasis, MapWorld, NO_LIGHTMAP};
use vcod_common::material::{
    AlphaFunc, Cod4Light, ImageRef, Material, PhongLit, PolygonOffset, TexSemantic,
};
use vcod_common::pk3::Pk3Fs;

/// cod2_world.wgsl with CoD4's flag waves and the image samplers.
pub(super) const COD2_WORLD_WGSL: &str = concat!(
    include_str!("../cod2_world.wgsl"),
    include_str!("../cod4_flag.wgsl"),
    include_str!("../cod4_sunshadow_recv.wgsl"),
    include_str!("../tex_samplers.wgsl")
);

/// sky_cube.wgsl with the image samplers.
pub(super) const SKY_CUBE_WGSL: &str = concat!(
    include_str!("../sky_cube.wgsl"),
    include_str!("../tex_samplers.wgsl")
);

/// `$lightmapweights`' side.
const WEIGHTS_SIZE: usize = 32;
/// `$specularity`: one column per exponent step, one row per `N.H` step.
const SPECULARITY_W: usize = 32;
const SPECULARITY_H: usize = 256;

/// `$lightmapweights` (`Image_LoadLightmapWeights` mac 0xfd152), 32 x 32
/// texels of four bytes in the A8R8G8B8 page's memory order. A texel at
/// normal-map sample (x, y) in [0, 1] holds the weights of the lightmap's
/// four basis lights for the normal (2x - 1, 2y - 1, z) with z filling it
/// to unit length: byte 0 the straight-up light, ramping from 1 at the
/// pole to 0 at 54.7 degrees (acos(1/sqrt 3)), bytes 1, 2 and 3 three
/// lights 120 degrees apart around the pole sharing the rest, split
/// linearly by the normal's azimuth. The four sum to 255 up to rounding.
/// A coefficient texel dotted with this texel in the same byte order is
/// the shader's `dot(coeffs, weights)`, whatever the channel names.
pub fn lightmap_weights() -> Vec<u8> {
    let n = WEIGHTS_SIZE as f32;
    let byte = |v: f32| (v * 255.0 + 0.5).floor() as u8;
    let mut out = Vec::with_capacity(WEIGHTS_SIZE * WEIGHTS_SIZE * 4);
    for row in 0..WEIGHTS_SIZE {
        let y = (row as f32 + 0.5) / n * 2.0 - 1.0;
        for col in 0..WEIGHTS_SIZE {
            let x = (col as f32 + 0.5) / n * 2.0 - 1.0;
            let z = (1.0 - (x * x + y * y)).max(0.0).sqrt();
            // azimuth in thirds of a turn, starting a quarter turn back
            let mut a = (f64::from(y).atan2(f64::from(x)) * 0.477_464_829_275_686 - 0.75) as f32;
            if a < 0.0 {
                a += 3.0;
            } else if a > 3.0 {
                a -= 3.0;
            }
            let (w1, w2, w3) = if a < 1.0 {
                (1.0 - a, a, 0.0)
            } else if a < 2.0 {
                (0.0, 2.0 - a, a - 1.0)
            } else {
                (a - 2.0, 0.0, 3.0 - a)
            };
            let t = 1.0 - z.acos() / 0.955_316_6;
            let (up, side) = if t < 0.0 {
                (0.0, 1.0)
            } else if t <= 1.0 {
                (t, 1.0 - t)
            } else {
                (1.0, 0.0)
            };
            out.extend([up, w1 * side, w2 * side, w3 * side].map(byte));
        }
    }
    out
}

/// `$specularity` (`Image_BuildSpecularityMap` mac 0x109f2c), 32 x 256 L8,
/// read at (specular map alpha, `N.H`): column `i` raises `N.H` to the
/// power whose lobe falls to 0.1 at `(180 - 175 * i / 30) / 4` degrees
/// (45 at column 0, 1.25 at 30); column 31 takes the power 30.
pub fn specularity_map() -> Vec<u8> {
    let mut out = vec![0u8; SPECULARITY_W * SPECULARITY_H];
    for col in 0..SPECULARITY_W {
        let power = if col == SPECULARITY_W - 1 {
            30.0
        } else {
            let x = f64::from(col as f32 / 30.0);
            let d = if x < 0.0 {
                0.0
            } else if x <= 1.0 {
                -175.0 * x
            } else {
                -175.0
            };
            let half = ((d + 180.0) * (std::f64::consts::PI / 720.0)).cos();
            f64::from(0.1f32).ln() / half.ln()
        };
        for row in 0..SPECULARITY_H {
            let v = f64::from(row as f32 * (1.0 / 255.0)).powf(power);
            out[row * SPECULARITY_W + col] = ((v * 255.0) as f32 + 0.5).floor() as u8;
        }
    }
    out
}

/// cod2_world.wgsl's `Cod2Material`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialUniform {
    pub detail_scale: [f32; 4],
    /// Switches, alpha test, clamp flags, the mipmapped maps (`MIP_*`).
    pub flags: [u32; 4],
    /// A CoD4 set's `envMapParms` (`PhongLit::cod4`).
    pub env: [f32; 4],
    /// A CoD4 water set's `waterColor`.
    pub water: [f32; 4],
}

const M_SPECULAR: u32 = 1;
const M_DETAIL: u32 = 2;
const M_ALPHA: u32 = 4;
/// The material's fogged technique leaves the fog off (`Material::nofog`).
const M_NOFOG: u32 = 8;
/// A CoD4 lit world set: CoD4's lightmap, normal and specular
/// (`PhongLit::cod4`).
const M_COD4: u32 = 16;
/// The CoD4 set reads its normal map.
const M_NORMAL: u32 = 32;
/// A CoD4 `mc_ambient_*` set: the volume's block centre, the sun without
/// `N.L` (`Cod4Light::GridAmbient`).
const M_AMBIENT: u32 = 64;
/// A CoD4 `mc_l_*_flag_*` set: the vertices wave, the sun takes `|N.L|`
/// (`Cod4Lit::flag`).
const M_FLAG: u32 = 128;
/// A CoD4 `sm` set: technique 9's sun shadow map (`Cod4Lit::sun_shadow`).
const M_SUN_SHADOW: u32 = 256;
/// `MaterialUniform::flags[3]`: the colour, normal, specular and detail
/// maps that carry mipmaps (cod2_world.wgsl's `MIP_*`).
const MIP_COLOR: u32 = 1;
const MIP_NORMAL: u32 = 2;
const MIP_SPEC: u32 = 4;
const MIP_DETAIL: u32 = 8;

impl MaterialUniform {
    /// The uniform for `lit` under the stage's alpha test, its colour
    /// map's clamp passed to `sample_image` for the maps that share its
    /// coordinates (the detail map wraps).
    pub fn new(lit: &PhongLit, nofog: bool, alpha: Option<AlphaFunc>, clamp: u32) -> Self {
        let bit = |on: bool, b: u32| if on { b } else { 0 };
        let switches = bit(lit.specular, M_SPECULAR)
            | bit(lit.detail, M_DETAIL)
            | bit(lit.alpha, M_ALPHA)
            | bit(nofog, M_NOFOG)
            | bit(lit.cod4.is_some(), M_COD4)
            | bit(lit.cod4.is_some_and(|c| c.normal_map), M_NORMAL)
            | bit(
                lit.cod4.is_some_and(|c| c.light == Cod4Light::GridAmbient),
                M_AMBIENT,
            )
            | bit(lit.cod4.is_some_and(|c| c.flag), M_FLAG)
            | bit(lit.cod4.is_some_and(|c| c.sun_shadow), M_SUN_SHADOW);
        let test = match alpha {
            None => 0,
            Some(AlphaFunc::Gt0) => 1,
            Some(AlphaFunc::Lt128) => 2,
            Some(AlphaFunc::Ge128) => 3,
        };
        MaterialUniform {
            detail_scale: [lit.detail_scale[0], lit.detail_scale[1], 0.0, 0.0],
            flags: [switches, test, clamp, 0],
            env: lit.cod4.map_or([0.0; 4], |c| c.env),
            water: lit.cod4.map_or([0.0; 4], |c| c.water_color),
        }
    }
}

/// cod2_world.wgsl's `Sun`, and a CoD4 static model's spot or omni
/// light in its place (`fs_cod4_model`, cod4-world.md 3.5).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SunUniform {
    pub dir: [f32; 4],
    pub color: [f32; 4],
    pub specular: [f32; 4],
    /// `lightPosition`: the origin, one over the radius.
    pub src_pos: [f32; 4],
    /// The colour, w 0 none (the sun lights), 1 an omni light, 2 a spot,
    /// -1 no primary light at all.
    pub src_col: [f32; 4],
    /// `lightSpotDir`, w the attenuation row's length.
    pub src_dir: [f32; 4],
    /// `lightSpotFactors`: scale, bias, exponent.
    pub src_factors: [f32; 4],
    /// The attenuation row, RGBA in 0..1.
    pub atten: [[f32; 4]; 32],
}

impl SunUniform {
    pub fn of(sun: &vcod_cod2::world::SunLight) -> Self {
        let [x, y, z] = sun.dir;
        let [r, g, b] = sun.colour;
        SunUniform {
            dir: [x, y, z, 0.0],
            color: [r, g, b, 1.0],
            // `r_specularColorScale` 1
            specular: [r, g, b, 1.0],
            ..bytemuck::Zeroable::zeroed()
        }
    }

    /// The uniform of a CoD4 surface with no primary light (its index 0):
    /// technique 7, the baked light alone (cod4-world.md 3.5).
    pub fn none(mut self) -> Self {
        self.src_col = [0.0, 0.0, 0.0, -1.0];
        self
    }

    /// The uniform with `l` lighting in the sun's place.
    pub fn with_source(mut self, l: &vcod_common::mapworld::ModelLightSource) -> Self {
        let row = &l.attenuation[..l.attenuation.len().min(32)];
        let [x, y, z] = l.origin;
        let [r, g, b] = l.colour;
        let [dx, dy, dz] = l.dir;
        let [s, bias, e] = l.spot_factors;
        self.src_pos = [x, y, z, l.inv_radius];
        self.src_col = [r, g, b, if l.spot { 2.0 } else { 1.0 }];
        self.src_dir = [dx, dy, dz, row.len() as f32];
        self.src_factors = [s, bias, e, 0.0];
        for (a, t) in self.atten.iter_mut().zip(row) {
            *a = t.map(|c| f32::from(c) / 255.0);
        }
        self
    }
}

/// The blend and depth write of the variants the lit family draws in:
/// the phong replace and blend sets' opaque and alpha-blended passes.
/// `None` for an additive variant, which keeps the stage path.
pub(super) fn variant_state(v: StageVariant) -> Option<(usize, Option<wgpu::BlendState>, bool)> {
    Some(match v {
        StageVariant::Opaque => (0, None, true),
        StageVariant::Blend => (1, Some(wgpu::BlendState::ALPHA_BLENDING), false),
        StageVariant::BlendDepthWrite => (2, Some(wgpu::BlendState::ALPHA_BLENDING), true),
        _ => return None,
    })
}

/// Whether a world batch of `sh` on lightmap page `lightmap` draws through
/// the lit pipeline: a `PhongLit` material, a page the map carries in its
/// basis form, and an unblended or alpha-blended stage.
pub fn draws_lit(sh: &Material, lightmap: u16, basis_pages: usize, v: StageVariant) -> bool {
    sh.phong.is_some()
        && !draws_cod4_model(sh, v)
        && !draws_cod4_water(sh, v)
        && lightmap != NO_LIGHTMAP
        && usize::from(lightmap) < basis_pages
        && variant_state(v).is_some()
}

/// Whether a world batch of `sh` draws through `fs_cod4_model`: a CoD4
/// model set lit off the model lighting volume (`Cod4Light::Grid`,
/// `GridAmbient`; cod4-world.md 3.2), unblended or alpha-blended. The
/// batch's vertices carry their block's centre as their lightmap coords.
pub fn draws_cod4_model(sh: &Material, v: StageVariant) -> bool {
    sh.phong
        .and_then(|p| p.cod4)
        .is_some_and(|c| matches!(c.light, Cod4Light::Grid | Cod4Light::GridAmbient))
        && variant_state(v).is_some()
}

/// Whether a world batch of `sh` draws through `fs_cod4_water`: a CoD4
/// water set (`Cod4Light::Water`, cod4-world.md 9.8), unblended or
/// alpha-blended.
pub fn draws_cod4_water(sh: &Material, v: StageVariant) -> bool {
    sh.phong
        .and_then(|p| p.cod4)
        .is_some_and(|c| c.light == Cod4Light::Water)
        && variant_state(v).is_some()
}

/// Whether a static prop batch of `sh` draws through the lit pipeline's
/// `fs_smodel`: a probe lit set with an `lprobe_smc*` technique and an
/// unblended or alpha-blended stage (cod2-materials.md 9.6).
pub fn draws_smodel(sh: &Material, v: StageVariant) -> bool {
    sh.model_light == vcod_common::material::ModelLight::Probe
        && sh.smodel.is_some()
        && variant_state(v).is_some()
}

/// A lit batch's bind groups in [`Cod2World`]: a world batch's material
/// and lightmap page, (`smodel`) a static prop's material and the static
/// model cache, or (`water`) a water material's two groups in
/// [`Cod2World::water`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct LitBind {
    pub material: u32,
    pub lightmap: u32,
    pub entry: LitEntry,
    pub water: bool,
}

/// The lit pipeline's fragment entry: a lightmapped world batch
/// (`fs_main`), a CoD2 static prop's (`fs_smodel`), a CoD4 model set's
/// off the model lighting volume (`fs_cod4_model`), a CoD4 water set's
/// (`fs_cod4_water`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LitEntry {
    Main,
    Smodel,
    Cod4Model,
    Cod4Water,
}

impl LitEntry {
    fn name(self) -> &'static str {
        match self {
            LitEntry::Main => "fs_main",
            LitEntry::Smodel => "fs_smodel",
            LitEntry::Cod4Model => "fs_cod4_model",
            LitEntry::Cod4Water => "fs_cod4_water",
        }
    }
}

/// [`Cod2Builder::light_key`]'s key of a group the sun lights: no primary
/// light index reaches it (`GFX_MAX_PRIMARY_LIGHTS` is 255).
const SUN_KEY: u8 = 255;

/// The path a split lit pipeline's fragment stage was built for
/// (cod2_world.wgsl's `LIT_*`, cod4-client-perf.md 29, cod2-client-perf.md
/// 8): the material's switches (`M_*`) and alpha test (`AT_*`), and its
/// group's light kind (`SunUniform::src_col`'s w: -1 none, 0 the sun, 1
/// omni, 2 spot). One per technique and material combination, as 1.7 and
/// CoD2 compile them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LitPath {
    pub flags: u32,
    pub test: u32,
    pub light: i8,
}

impl LitPath {
    fn constants(self) -> [(&'static str, f64); 4] {
        [
            ("LIT_SPLIT", 1.0),
            ("LIT_FLAGS", f64::from(self.flags)),
            ("LIT_TEST", f64::from(self.test)),
            ("LIT_LIGHT", f64::from(self.light)),
        ]
    }

    /// The path `r_vc_normalMaps` 0 takes: no normal map.
    fn without_normal_map(self) -> Self {
        LitPath {
            flags: self.flags & !M_NORMAL,
            ..self
        }
    }
}

/// A lit fragment stage: the entry and, on a split CoD4 draw, its path
/// (`None` reads every switch off the uniforms).
pub type FsKey = (LitEntry, Option<LitPath>);

/// The pipeline key: variant slot, two-sided, polygon offset, fragment.
type Key = (usize, bool, bool, FsKey);

/// A depth prepass pipeline's key: two-sided, and the fragment of the
/// EQUAL colour pipeline (`None` the depth-only one).
type PrepassKey = (bool, Option<FsKey>);

/// The loaded map's lit draws.
pub(super) struct Cod2World {
    /// Per vertex of the world's vertex buffer, tangent and binormal.
    pub tangent_buf: wgpu::Buffer,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
    /// What each of `pipelines` was made from, for the variants
    /// [`Self::set_normal_maps`] makes.
    made: HashMap<Key, MadeKey>,
    pub material_bgs: Vec<wgpu::BindGroup>,
    pub lightmap_bgs: Vec<wgpu::BindGroup>,
    /// The map's water materials, when it draws any.
    pub water: Option<super::cod2_water::Waters>,
    /// The map's CoD4 water height maps, rebuilt each frame.
    pub cod4_waters: Vec<Cod4WaterSim>,
    /// `r_vc_depthprepass`' pipelines for the opaque keys
    /// ([`Self::takes_prepass`]).
    prepass: HashMap<PrepassKey, wgpu::RenderPipeline>,
    /// The prepass's `floatZ` writers by (two-sided, alpha tested)
    /// ([`Self::takes_floatz`]).
    floatz: HashMap<(bool, bool), wgpu::RenderPipeline>,
    /// By material group, its CoD4 material's `floatZ` draw
    /// (`Material::floatz`).
    floatz_draws: Vec<Option<bool>>,
    /// By material group, whether its stage alpha tests (`fs_*`'s discard).
    alpha_tested: Vec<bool>,
    /// By material group, whether `vs_main` waves it as a flag
    /// (`M_FLAG`): the one material switch its position reads.
    waves: Vec<bool>,
    /// By material group, a split CoD4 set's switches and alpha test
    /// ([`LitPath`]); `None` for one drawn unsplit.
    paths: Vec<Option<(u32, u32)>>,
    /// By lightmap group, its light kind ([`LitPath::light`]).
    light_kinds: Vec<i8>,
    /// The groups whose CoD4 set reads its normal map (`M_NORMAL`): the
    /// uniform buffer and what it holds, for `r_vc_normalMaps`.
    normal_maps: Vec<(wgpu::Buffer, MaterialUniform)>,
    /// Whether the uniforms hold their `M_NORMAL`s ([`Self::set_normal_maps`]).
    pub normal_maps_on: bool,
    /// The shader, layout and target format, for [`Self::set_normal_maps`].
    base: (
        wgpu::ShaderModule,
        wgpu::PipelineLayout,
        wgpu::TextureFormat,
    ),
}

impl Cod2World {
    /// `r_vc_normalMaps` (cod4-client-perf.md 25): off, every CoD4 set
    /// lights by its vertex normal and reads no normal map, as one
    /// without a map does. Its split pipelines' variants without the map
    /// are made on the first switch off.
    pub fn set_normal_maps(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, on: bool) {
        for (buf, u) in &self.normal_maps {
            let mut u = *u;
            if !on {
                u.flags[0] &= !M_NORMAL;
            }
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&u));
        }
        self.normal_maps_on = on;
        if on {
            return;
        }
        let (module, layout, format) = &self.base;
        let off = |(e, p): FsKey| (e, p.map(LitPath::without_normal_map));
        let todo: Vec<(Key, MadeKey)> = self
            .made
            .iter()
            .map(|(&(slot, ts, bias, fs), &made)| {
                (
                    (slot, ts, bias, off(fs)),
                    MadeKey {
                        fs: off(made.fs),
                        ..made
                    },
                )
            })
            .filter(|(k, _)| !self.pipelines.contains_key(k))
            .collect();
        for (key, made) in todo {
            let p = build_made(device, module, layout, *format, made);
            self.pipelines.insert(key, p);
            self.made.insert(key, made);
        }
        let todo: Vec<PrepassKey> = self
            .prepass
            .keys()
            .map(|&(ts, fs)| (ts, fs.map(off)))
            .filter(|k| !self.prepass.contains_key(k))
            .collect();
        for key in todo {
            let p = make_prepass_pipeline(device, module, layout, *format, key.0, key.1);
            self.prepass.insert(key, p);
        }
    }

    /// The fragment stage a lit draw of `lit` runs: its entry and, for a
    /// split CoD4 set, the path its material and group fix
    /// (`r_vc_normalMaps` 0 drops the normal map).
    pub fn fs_key(&self, lit: &LitBind) -> FsKey {
        if lit.water {
            return (lit.entry, None);
        }
        let path = self.paths[lit.material as usize].map(|(flags, test)| {
            let p = LitPath {
                flags,
                test,
                light: self.light_kinds[lit.lightmap as usize],
            };
            if self.normal_maps_on {
                p
            } else {
                p.without_normal_map()
            }
        });
        (lit.entry, path)
    }

    /// Whether a lit draw of `lit` takes the depth prepass: unblended,
    /// depth written, no polygon offset, no alpha test, not water. Every
    /// other draw keeps its pipeline and LessEqual.
    pub fn takes_prepass(
        &self,
        v: StageVariant,
        two_sided: bool,
        bias: bool,
        lit: &LitBind,
    ) -> bool {
        v == StageVariant::Opaque
            && !bias
            && !lit.water
            && lit.entry != LitEntry::Cod4Water
            && !self.alpha_tested[lit.material as usize]
            && self
                .prepass
                .contains_key(&(two_sided, Some(self.fs_key(lit))))
    }

    /// Whether a lit draw of `lit` writes `floatZ` in the depth prepass of
    /// a CoD4 frame a feather reads, and whether it alpha tests there:
    /// the draws whose material 1.7's prepass draws (`Material::floatz`:
    /// prepass key 0, 1 or 2 and a `build floatz` technique, whose state
    /// has no polygon offset; cod4-client-perf.md 35). Water stays out:
    /// its pipeline reads another vertex stream.
    pub fn takes_floatz(&self, v: StageVariant, lit: &LitBind) -> Option<bool> {
        if v != StageVariant::Opaque || lit.water || lit.entry == LitEntry::Cod4Water {
            return None;
        }
        self.floatz_draws[lit.material as usize]
    }

    /// The prepass's `floatZ` pipeline for a draw [`Self::takes_floatz`]
    /// accepted.
    pub fn floatz_pipeline(&self, two_sided: bool, atest: bool) -> &wgpu::RenderPipeline {
        &self.floatz[&(two_sided, atest)]
    }

    /// Whether material group `material`'s vertices wave as a flag.
    pub fn waves(&self, material: u32) -> bool {
        self.waves[material as usize]
    }

    /// The prepass's depth-only pipeline, or with `lit` its EQUAL colour
    /// one, for a draw [`Self::takes_prepass`] accepted.
    pub fn prepass_pipeline(
        &self,
        two_sided: bool,
        lit: Option<&LitBind>,
    ) -> &wgpu::RenderPipeline {
        &self.prepass[&(two_sided, lit.map(|l| self.fs_key(l)))]
    }

    /// The pipeline of a lit draw of `lit` in variant `v`.
    pub fn pipeline(
        &self,
        v: StageVariant,
        two_sided: bool,
        bias: bool,
        lit: &LitBind,
    ) -> &wgpu::RenderPipeline {
        let (slot, ..) = variant_state(v).expect("lit draws take a lit variant");
        &self.pipelines[&(slot, two_sided, bias, self.fs_key(lit))]
    }
}

/// Builds [`Cod2World`] during a world load: one bind group per material
/// and per lightmap page, pipelines as batches ask for them.
pub(super) struct Cod2Builder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    format: wgpu::TextureFormat,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    material_layout: wgpu::BindGroupLayout,
    lightmap_layout: wgpu::BindGroupLayout,
    sun_buf: wgpu::Buffer,
    /// The sun's uniform, for a group with a light in its place.
    sun: SunUniform,
    /// A CoD4 map's spot and omni lights, by primary light.
    model_lights: &'a [Option<vcod_common::mapworld::ModelLightSource>],
    weights: wgpu::TextureView,
    specularity: wgpu::TextureView,
    pages: &'a [LightmapBasis],
    camera_layout: &'a wgpu::BindGroupLayout,
    water: Option<super::cod2_water::WaterBuilder>,
    /// By page, reflection probe and primary light.
    page_bgs: HashMap<(u16, u8, u8), u32>,
    /// A page's four images, uploaded once whatever probes share it.
    page_views: HashMap<u16, [wgpu::TextureView; 4]>,
    /// The map's probes as cubes, uploaded as batches ask.
    probes: &'a [vcod_common::mapworld::ProbeCube],
    probe_views: HashMap<u8, wgpu::TextureView>,
    black_cube: wgpu::TextureView,
    black_volume: wgpu::TextureView,
    /// The static model cache's group, once a prop asked for it.
    smodel_bg: Option<u32>,
    /// A CoD4 map's model lighting volume and its group once a batch asked.
    model_volume: Option<&'a vcod_common::mapworld::ModelLightVolume>,
    /// The volume as uploaded, once.
    volume_view: Option<wgpu::TextureView>,
    /// By reflection probe and primary light.
    model_bgs: HashMap<(u8, u8), u32>,
    /// A water batch's group by probe: the sun and the cube.
    probe_bgs: HashMap<u8, u32>,
    /// The map's CoD4 water spectra, and each water material's height map
    /// as made.
    waters: &'a [(String, vcod_common::mapworld::WaterSpectrum)],
    cod4_waters: Vec<Cod4WaterSim>,
    /// The height map the next material group binds as its normal map.
    water_normal: Option<wgpu::TextureView>,
    smodel_cache: Option<&'a crate::cod2_light::SmodelCache>,
    /// By name and the entry that draws it.
    material_of: HashMap<(String, LitEntry), u32>,
    /// Normal and specular maps: data, so no gamma.
    raw_views: HashMap<String, wgpu::TextureView>,
    /// CoD4's `r_picmip_bump` and `r_picmip_spec` ([`Cod2Builder::set_picmip`]).
    picmip: [u32; 2],
    out: Vec<wgpu::BindGroup>,
    /// [`Cod2World::normal_maps`].
    normal_maps: Vec<(wgpu::Buffer, MaterialUniform)>,
    /// By `out` entry, whether its stage alpha tests.
    alpha_tested: Vec<bool>,
    /// By `out` entry, [`Cod2World::floatz_draws`].
    floatz_draws: Vec<Option<bool>>,
    /// By `out` entry, whether it waves as a flag.
    waves: Vec<bool>,
    lightmap_bgs: Vec<wgpu::BindGroup>,
    /// By `out` entry, [`Cod2World::paths`].
    paths: Vec<Option<(u32, u32)>>,
    /// By `lightmap_bgs` entry, [`Cod2World::light_kinds`].
    light_kinds: Vec<i8>,
    /// The pipelines the binds asked for, made at the finish.
    made: HashMap<Key, MadeKey>,
    /// Time spent making pipelines this load, for the load's log line.
    compile: std::time::Duration,
    /// Whether lit sets take split pipelines (`VCOD_LIT_SPLIT=0` keeps
    /// the one shader with its uniform branches, to measure against).
    split: bool,
    shared: LitShared,
}

/// What the lit draws make no differently from map to map, kept by the
/// renderer across loads: the shader, the layouts, the constant images
/// and every pipeline made, by all that went into it. A map's load still
/// keys its own pipelines as before ([`Key`], the first draw of a key
/// picking its blend and offset); it only skips compiling one it made
/// on an earlier map.
#[derive(Default)]
pub(super) struct LitShared {
    base: Option<LitBase>,
    pipelines: HashMap<MadeKey, wgpu::RenderPipeline>,
    prepass: HashMap<PrepassKey, wgpu::RenderPipeline>,
}

struct LitBase {
    format: wgpu::TextureFormat,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    material_layout: wgpu::BindGroupLayout,
    lightmap_layout: wgpu::BindGroupLayout,
    weights: wgpu::TextureView,
    specularity: wgpu::TextureView,
    /// The probe of a group that has none.
    black_cube: wgpu::TextureView,
    /// The model lighting volume of a group that reads none.
    black_volume: wgpu::TextureView,
}

/// Everything [`make_lit_pipeline`] is handed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct MadeKey {
    two_sided: bool,
    /// whether it takes a polygon offset, and which
    offset: Option<Option<PolygonOffset>>,
    fs: FsKey,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
}

impl LitBase {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod2_world.wgsl"),
            source: wgpu::ShaderSource::Wgsl(COD2_WORLD_WGSL.into()),
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 material layout"),
            entries: &[
                // the vertex stage reads a CoD4 flag's switch
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
            ],
        });
        let lightmap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 lightmap layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                texture_entry(6),
                // a CoD4 surface's reflection probe
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                // a CoD4 static model's lighting volume
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 world pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&material_layout),
                Some(&lightmap_layout),
            ],
            immediate_size: 0,
        });
        let side = WEIGHTS_SIZE as u32;
        let weights = upload_plain(
            device,
            queue,
            "$lightmapweights",
            (side, side),
            wgpu::TextureFormat::Rgba8Unorm,
            4,
            &lightmap_weights(),
        );
        let specularity = upload_plain(
            device,
            queue,
            "$specularity",
            (SPECULARITY_W as u32, SPECULARITY_H as u32),
            wgpu::TextureFormat::R8Unorm,
            1,
            &specularity_map(),
        );
        let black_cube = upload_probe(
            device,
            queue,
            "black probe",
            &vcod_common::mapworld::ProbeCube {
                size: 1,
                faces: vec![vec![vec![0; 4]]; 6],
            },
        );
        let black_volume = upload_volume(device, queue, "black volume", (1, 1), &[0; 16]);
        LitBase {
            format,
            module,
            layout,
            material_layout,
            lightmap_layout,
            weights,
            specularity,
            black_cube,
            black_volume,
        }
    }
}

pub(super) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn tex_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(super) fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A one-level texture of `format` from tightly packed rows.
/// A `width` x `height` x 4 RGBA volume, its slices one after another, as
/// 1.7's model lighting image (`R_InitModelLightingImage`, cod4-world.md
/// 3.2).
fn upload_volume(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    (width, height): (u32, u32),
    data: &[u8],
) -> wgpu::TextureView {
    debug_assert_eq!(data.len() as u32, width * height * 16);
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 4,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            data,
        )
        .create_view(&Default::default())
}

fn upload_plain(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    bytes_per_texel: u32,
    data: &[u8],
) -> wgpu::TextureView {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        data,
    );
    debug_assert_eq!(data.len() as u32, width * height * bytes_per_texel);
    texture.create_view(&Default::default())
}

/// The world vertex buffer's companion: tangent and binormal per vertex,
/// the map's own frame for its vertices and zero past them (props).
pub fn tangent_stream(bsp: &MapWorld, vertex_count: usize) -> Vec<[[f32; 3]; 2]> {
    let mut t = bsp.tangents.clone();
    t.resize(vertex_count, [[0.0; 3]; 2]);
    t
}

impl<'a> Cod2Builder<'a> {
    #[cfg(test)]
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_layout: &'a wgpu::BindGroupLayout,
        bsp: &'a MapWorld,
    ) -> Self {
        Self::with_shared(
            device,
            queue,
            format,
            camera_layout,
            bsp,
            LitShared::default(),
        )
    }

    /// [`Self::new`] over what an earlier load kept ([`Self::finish_keeping`]).
    pub fn with_shared(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_layout: &'a wgpu::BindGroupLayout,
        bsp: &'a MapWorld,
        mut shared: LitShared,
    ) -> Self {
        if shared.base.as_ref().is_none_or(|b| b.format != format) {
            shared = LitShared {
                base: Some(LitBase::new(device, queue, format, camera_layout)),
                pipelines: HashMap::new(),
                prepass: HashMap::new(),
            };
        }
        let base = shared.base.as_ref().expect("made above");
        // a CoD4 map stores its sun; CoD2's is worldspawn's
        let sun = bsp.sun.map_or_else(
            || vcod_cod2::world::sun_light(&bsp.entities),
            |s| vcod_cod2::world::SunLight {
                dir: s.dir,
                colour: s.colour,
            },
        );
        log::info!("cod2 sun: dir {:?} colour {:?}", sun.dir, sun.colour);
        let sun = SunUniform::of(&sun);
        let sun_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cod2 sun"),
            contents: bytemuck::bytes_of(&sun),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let LitBase {
            module,
            layout,
            material_layout,
            lightmap_layout,
            weights,
            specularity,
            black_cube,
            black_volume,
            ..
        } = base;
        let (module, layout) = (module.clone(), layout.clone());
        let (material_layout, lightmap_layout) = (material_layout.clone(), lightmap_layout.clone());
        let (weights, specularity) = (weights.clone(), specularity.clone());
        Cod2Builder {
            device,
            queue,
            format,
            module,
            layout,
            material_layout,
            lightmap_layout,
            sun_buf,
            sun,
            model_lights: &bsp.model_lights,
            weights,
            specularity,
            pages: &bsp.lightmap_basis,
            camera_layout,
            water: None,
            page_bgs: HashMap::new(),
            page_views: HashMap::new(),
            probes: &bsp.reflection_probes,
            probe_views: HashMap::new(),
            black_cube: black_cube.clone(),
            black_volume: black_volume.clone(),
            smodel_bg: None,
            smodel_cache: None,
            // mp_creek's 22311 models stand 1396 texels, inside the
            // default 2048
            model_volume: bsp
                .model_lighting
                .as_ref()
                .filter(|v| v.height <= device.limits().max_texture_dimension_3d),
            volume_view: None,
            model_bgs: HashMap::new(),
            probe_bgs: HashMap::new(),
            waters: &bsp.waters,
            cod4_waters: Vec::new(),
            water_normal: None,
            material_of: HashMap::new(),
            raw_views: HashMap::new(),
            picmip: [0; 2],
            out: Vec::new(),
            normal_maps: Vec::new(),
            alpha_tested: Vec::new(),
            floatz_draws: Vec::new(),
            waves: Vec::new(),
            lightmap_bgs: Vec::new(),
            paths: Vec::new(),
            light_kinds: Vec::new(),
            made: HashMap::new(),
            compile: Default::default(),
            split: std::env::var_os("VCOD_LIT_SPLIT").is_none_or(|v| v != "0"),
            shared,
        }
    }

    /// The levels a CoD4 map's normal and specular maps drop
    /// (`r_picmip_bump`, `r_picmip_spec`; iw3mp.exe 0x616d40 picks them by
    /// the image's semantic). A CoD2 map keeps 0.
    pub fn set_picmip(&mut self, bump: u32, spec: u32) {
        self.picmip = [bump, spec];
    }

    /// The map's basis pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// [`Self::bind_in`] with probe 0, as every CoD2 batch's.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn bind(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        lightmap: u16,
        v: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> LitBind {
        self.bind_in(fs, bundle_views, sh, (lightmap, 0, 0), v, two_sided, bias)
    }

    /// Binds a lit batch of `sh` on page `lightmap` under reflection probe
    /// `probe` and primary light `light` (a CoD4 surface's; 0 elsewhere),
    /// which [`draws_lit`] accepted. A spot or omni light in `light` lights
    /// it in the sun's place (`lm_spot_*`, `lm_omni_*`).
    #[allow(clippy::too_many_arguments)]
    pub fn bind_in(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        (lightmap, probe, light): (u16, u8, u8),
        v: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> LitBind {
        let lit = sh.phong.expect("draws_lit checked");
        let material = self.prepare(fs, bundle_views, sh, lit, LitEntry::Main);
        // a set that reads no probe shares the page's group with probe 0
        let probe = if lit.specular { probe } else { 0 };
        let light = self.light_key(light);
        let lightmap = match self.page_bgs.get(&(lightmap, probe, light)) {
            Some(&i) => i,
            None => {
                let bg = self.page_group(lightmap, probe, light);
                let i = self.push_group(bg, light);
                self.page_bgs.insert((lightmap, probe, light), i);
                i
            }
        };
        self.finish_bind(sh, (v, two_sided, bias), material, lightmap, LitEntry::Main)
    }

    /// Binds a water batch of `sh`, which
    /// [`super::cod2_water::draws_water`] accepted.
    pub fn bind_water(
        &mut self,
        fs: &dyn AssetSource,
        sh: &Material,
        v: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> LitBind {
        let (device, camera_layout) = (self.device, self.camera_layout);
        let water = self
            .water
            .get_or_insert_with(|| super::cod2_water::WaterBuilder::new(device, camera_layout));
        let i = water.bind(
            self.device,
            self.queue,
            self.format,
            fs,
            &self.sun_buf,
            sh,
            (v, two_sided, bias),
        );
        LitBind {
            material: i,
            lightmap: i,
            entry: LitEntry::Main,
            water: true,
        }
    }

    /// The static model cache static props read ([`draws_smodel`]).
    pub fn set_smodel_cache(&mut self, cache: &'a crate::cod2_light::SmodelCache) {
        self.smodel_cache = Some(cache);
    }

    /// Binds a static prop batch of `sh`, which [`draws_smodel`] accepted,
    /// to its `lprobe_smc*` technique over the static model cache.
    pub fn bind_smodel(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        v: StageVariant,
        bias: bool,
    ) -> LitBind {
        let sm = sh.smodel.expect("draws_smodel checked");
        let lit = PhongLit {
            specular: sm.specular,
            detail: sm.detail,
            alpha: sm.alpha,
            detail_scale: sm.detail_scale,
            cod4: None,
        };
        // props keep the prop path's two-sidedness (xmodel winding)
        let material = self.prepare(fs, bundle_views, sh, lit, LitEntry::Smodel);
        let lightmap = match self.smodel_bg {
            Some(i) => i,
            None => {
                let bg = self.smodel_group();
                let i = self.push_group(bg, SUN_KEY);
                self.smodel_bg = Some(i);
                i
            }
        };
        self.finish_bind(sh, (v, true, bias), material, lightmap, LitEntry::Smodel)
    }

    /// Whether the map carries a model lighting volume for
    /// [`Self::bind_cod4_model`].
    pub fn has_model_volume(&self) -> bool {
        self.model_volume.is_some()
    }

    /// Binds a CoD4 model batch of `sh` under reflection probe `probe` and
    /// primary light `light` (the sun's technique unless it names a spot
    /// or omni light), which [`draws_cod4_model`] accepted, over the map's
    /// model lighting volume.
    #[allow(clippy::too_many_arguments)]
    pub fn bind_cod4_model(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        (probe, light): (u8, u8),
        v: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> LitBind {
        let lit = sh.phong.expect("draws_cod4_model checked");
        let entry = LitEntry::Cod4Model;
        let material = self.prepare(fs, bundle_views, sh, lit, entry);
        let probe = if lit.specular { probe } else { 0 };
        // the ambient set takes the sun alone (`lp_amb_sun_*`)
        let ambient = lit.cod4.is_some_and(|c| c.light == Cod4Light::GridAmbient);
        let light = match self.light_key(light) {
            k if ambient && k != 0 => SUN_KEY,
            k => k,
        };
        let lightmap = match self.model_bgs.get(&(probe, light)) {
            Some(&i) => i,
            None => {
                let bg = self.model_volume_group(probe, light);
                let i = self.push_group(bg, light);
                self.model_bgs.insert((probe, light), i);
                i
            }
        };
        self.finish_bind(sh, (v, two_sided, bias), material, lightmap, entry)
    }

    /// Binds a CoD4 water batch of `sh` under reflection probe `probe`,
    /// which [`draws_cod4_water`] accepted.
    #[allow(clippy::too_many_arguments)]
    pub fn bind_cod4_water(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        probe: u8,
        v: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> LitBind {
        let lit = sh.phong.expect("draws_cod4_water checked");
        let entry = LitEntry::Cod4Water;
        if !self.material_of.contains_key(&(sh.name.clone(), entry)) {
            self.water_normal = self.cod4_water_map(&sh.name);
        }
        let material = self.prepare(fs, bundle_views, sh, lit, entry);
        let lightmap = match self.probe_bgs.get(&probe) {
            Some(&i) => i,
            None => {
                let cube = self.probe_view(probe);
                let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("cod4 water probe"),
                    layout: &self.lightmap_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.sun_buf.as_entire_binding(),
                        },
                        tex_entry(1, &self.weights),
                        tex_entry(2, &self.weights),
                        tex_entry(3, &self.weights),
                        tex_entry(4, &self.weights),
                        tex_entry(5, &self.weights),
                        tex_entry(6, &self.specularity),
                        tex_entry(7, &cube),
                        tex_entry(8, &self.black_volume),
                    ],
                });
                let i = self.push_group(bg, SUN_KEY);
                self.probe_bgs.insert(probe, i);
                i
            }
        };
        self.finish_bind(sh, (v, two_sided, bias), material, lightmap, entry)
    }

    /// CoD4 water `name`'s height map, made and kept for the frame's
    /// update; `None` when the map stores no spectrum for it.
    fn cod4_water_map(&mut self, name: &str) -> Option<wgpu::TextureView> {
        let (_, w) = self.waters.iter().find(|(n, _)| n == name)?;
        let setup =
            vcod_cod2::water::WaterSetup::from_spectrum(w.width, w.h0.clone(), w.omega.clone());
        let side = w.width as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("watersetup {name}")),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: side.ilog2() + 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut sim = Cod4WaterSim {
            setup,
            texture,
            time: None,
        };
        sim.update(self.queue, 0.0);
        let view = sim.texture.create_view(&Default::default());
        self.cod4_waters.push(sim);
        Some(view)
    }

    /// Group 2 of a CoD4 model batch: the sun or its primary light, the
    /// model lighting volume (binding 8), the probe; the page slots unread.
    fn model_volume_group(&mut self, probe: u8, light: u8) -> wgpu::BindGroup {
        let cube = self.probe_view(probe);
        let v = self.model_volume.expect("has_model_volume checked");
        let (device, queue) = (self.device, self.queue);
        let volume = self
            .volume_view
            .get_or_insert_with(|| {
                upload_volume(
                    device,
                    queue,
                    "cod4 model lighting volume",
                    (v.width, v.height),
                    &v.rgba,
                )
            })
            .clone();
        let own = self.light_buffer(light);
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cod4 model lighting"),
            layout: &self.lightmap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: own.as_ref().unwrap_or(&self.sun_buf).as_entire_binding(),
                },
                tex_entry(1, &self.weights),
                tex_entry(2, &self.weights),
                tex_entry(3, &self.weights),
                tex_entry(4, &self.weights),
                tex_entry(5, &self.weights),
                tex_entry(6, &self.specularity),
                tex_entry(7, &cube),
                tex_entry(8, &volume),
            ],
        })
    }

    /// Probe `i`'s cube, uploaded on first use; black past the map's
    /// probes or for one that did not decode.
    fn probe_view(&mut self, i: u8) -> wgpu::TextureView {
        if let Some(v) = self.probe_views.get(&i) {
            return v.clone();
        }
        let v = match self.probes.get(usize::from(i)).filter(|p| p.size > 0) {
            Some(p) => upload_probe(self.device, self.queue, &format!("probe {i}"), p),
            None => self.black_cube.clone(),
        };
        self.probe_views.insert(i, v.clone());
        v
    }

    /// Group 2 entry `bg`, keyed [`Self::light_key`] `light`.
    fn push_group(&mut self, bg: wgpu::BindGroup, light: u8) -> u32 {
        let kind = match light {
            SUN_KEY => 0,
            0 => -1,
            k => match self.model_lights.get(usize::from(k)) {
                Some(Some(l)) if l.spot => 2,
                Some(Some(_)) => 1,
                // `light_buffer` falls back to the sun's
                _ => 0,
            },
        };
        self.lightmap_bgs.push(bg);
        self.light_kinds.push(kind);
        self.lightmap_bgs.len() as u32 - 1
    }

    /// The bind of a lit draw of `sh` on material and lightmap groups
    /// `material` and `lightmap`, its pipeline made on first use.
    fn finish_bind(
        &mut self,
        sh: &Material,
        (v, two_sided, bias): (StageVariant, bool, bool),
        material: u32,
        lightmap: u32,
        entry: LitEntry,
    ) -> LitBind {
        let bind = LitBind {
            material,
            lightmap,
            entry,
            water: false,
        };
        let (slot, blend, depth_write) = variant_state(v).expect("a lit variant");
        let fs = self.fs_key(&bind);
        // the first draw of a key picks its offset; [`Self::finish_keeping`]
        // compiles them all
        self.made
            .entry((slot, two_sided, bias, fs))
            .or_insert(MadeKey {
                two_sided,
                offset: bias.then_some(sh.depth_bias),
                fs,
                blend,
                depth_write,
            });
        bind
    }

    /// [`Cod2World::fs_key`] with every map's normal map on.
    fn fs_key(&self, lit: &LitBind) -> FsKey {
        let path = self.paths[lit.material as usize].map(|(flags, test)| LitPath {
            flags,
            test,
            light: self.light_kinds[lit.lightmap as usize],
        });
        (lit.entry, path)
    }

    /// `sh`'s material group under `lit` for fragment entry `entry`.
    fn prepare(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        lit: PhongLit,
        entry: LitEntry,
    ) -> u32 {
        let mkey = (sh.name.clone(), entry);
        match self.material_of.get(&mkey) {
            Some(&i) => i,
            None => {
                let waves = lit.cod4.is_some_and(|c| c.flag);
                let (bg, buf, uniform) = self.material_group(fs, bundle_views, sh, lit);
                self.out.push(bg);
                // a lit world or model set runs its own path, as retail
                // compiles one shader per technique (CoD2's `lmap_s_dtl` and
                // kin, cod2-client-perf.md 8); the water reads its switches
                let split = self.split
                    && matches!(
                        entry,
                        LitEntry::Main | LitEntry::Smodel | LitEntry::Cod4Model
                    );
                self.paths
                    .push(split.then_some((uniform.flags[0], uniform.flags[1])));
                if uniform.flags[0] & M_NORMAL != 0 {
                    self.normal_maps.push((buf, uniform));
                }
                self.alpha_tested
                    .push(sh.stages.first().is_some_and(|st| st.alpha_func.is_some()));
                self.floatz_draws.push(sh.floatz);
                self.waves.push(waves);
                let i = self.out.len() as u32 - 1;
                self.material_of.insert(mkey, i);
                i
            }
        }
    }

    /// Group 2 of a static prop: the sun, the cache's two layers in the
    /// slots of a page's red and green coefficients, the rest as a page's.
    fn smodel_group(&self) -> wgpu::BindGroup {
        let c = self.smodel_cache.expect("set_smodel_cache first");
        let layer = |k: usize| {
            upload_plain(
                self.device,
                self.queue,
                &format!("static model cache layer {k}"),
                (c.width, c.height),
                wgpu::TextureFormat::Rgba8Unorm,
                4,
                &c.layers[k],
            )
        };
        let (l0, l1) = (layer(0), layer(1));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cod2 static model cache"),
            layout: &self.lightmap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.sun_buf.as_entire_binding(),
                },
                tex_entry(1, &l0),
                tex_entry(2, &l1),
                tex_entry(3, &l0),
                tex_entry(4, &self.weights),
                tex_entry(5, &self.weights),
                tex_entry(6, &self.specularity),
                tex_entry(7, &self.black_cube),
                tex_entry(8, &self.black_volume),
            ],
        })
    }

    #[cfg(test)]
    pub fn finish(self, tangents: &[[[f32; 3]; 2]]) -> Cod2World {
        self.finish_keeping(tangents).0
    }

    /// [`Self::finish`], with what the next load reuses.
    pub fn finish_keeping(mut self, tangents: &[[[f32; 3]; 2]]) -> (Cod2World, LitShared) {
        let pipelines = self.lit_pipelines();
        let prepass = self.prepass_pipelines();
        let keys = [(false, false), (false, true), (true, false), (true, true)];
        let (device, module, layout) = (self.device, &self.module, &self.layout);
        let t = std::time::Instant::now();
        let floatz: HashMap<(bool, bool), wgpu::RenderPipeline> = keys
            .into_iter()
            .zip(par_compile(&keys, |&(two_sided, atest)| {
                make_floatz_pipeline(device, module, layout, two_sided, atest)
            }))
            .collect();
        self.compile += t.elapsed();
        log::info!(
            "cod2 lit: {} materials, {} lightmap pages, {} pipelines ({} split), {} prepass, {:.0} ms compiling",
            self.out.len(),
            self.lightmap_bgs.len(),
            pipelines.len(),
            pipelines.keys().filter(|k| k.3.1.is_some()).count(),
            prepass.len(),
            self.compile.as_secs_f64() * 1000.0
        );
        let world = Cod2World {
            tangent_buf: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("cod2 tangents"),
                    contents: bytemuck::cast_slice(tangents),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            pipelines,
            made: self.made,
            material_bgs: self.out,
            lightmap_bgs: self.lightmap_bgs,
            water: self.water.map(super::cod2_water::WaterBuilder::finish),
            cod4_waters: self.cod4_waters,
            prepass,
            floatz,
            alpha_tested: self.alpha_tested,
            floatz_draws: self.floatz_draws,
            waves: self.waves,
            paths: self.paths,
            light_kinds: self.light_kinds,
            normal_maps: self.normal_maps,
            normal_maps_on: true,
            base: (self.module.clone(), self.layout.clone(), self.format),
        };
        (world, self.shared)
    }

    /// The prepass pair of every opaque key the map's draws made (water's
    /// aside): a depth-only pipeline per sidedness, an EQUAL one per entry.
    fn prepass_pipelines(&mut self) -> HashMap<PrepassKey, wgpu::RenderPipeline> {
        let opaque = variant_state(StageVariant::Opaque).expect("lit").0;
        let mut keys: Vec<PrepassKey> = self
            .made
            .keys()
            .filter(|&&(slot, _, bias, (entry, _))| {
                slot == opaque && !bias && entry != LitEntry::Cod4Water
            })
            .flat_map(|&(_, two_sided, _, fs)| [(two_sided, None), (two_sided, Some(fs))])
            .collect();
        keys.sort_by_key(|&(two_sided, fs)| {
            let fs = fs.map(|(e, p)| (e.name(), p.map(|p| (p.flags, p.test, p.light))));
            (two_sided, fs)
        });
        keys.dedup();
        let fresh: Vec<PrepassKey> = keys
            .iter()
            .filter(|k| !self.shared.prepass.contains_key(k))
            .copied()
            .collect();
        let (device, module, layout, format) =
            (self.device, &self.module, &self.layout, self.format);
        let t = std::time::Instant::now();
        let built = par_compile(&fresh, |&(two_sided, fs)| {
            make_prepass_pipeline(device, module, layout, format, two_sided, fs)
        });
        self.compile += t.elapsed();
        self.shared.prepass.extend(fresh.into_iter().zip(built));
        keys.into_iter()
            .map(|key| (key, self.shared.prepass[&key].clone()))
            .collect()
    }

    /// Every lit pipeline the map's binds asked for, those an earlier load
    /// made taken from it.
    fn lit_pipelines(&mut self) -> HashMap<Key, wgpu::RenderPipeline> {
        // one key per variant slot, sidedness, offset and fragment: no
        // two make the same pipeline
        let fresh: Vec<MadeKey> = self
            .made
            .values()
            .filter(|m| !self.shared.pipelines.contains_key(m))
            .copied()
            .collect();
        let (device, module, layout, format) =
            (self.device, &self.module, &self.layout, self.format);
        let t = std::time::Instant::now();
        let built = par_compile(&fresh, |&m| build_made(device, module, layout, format, m));
        self.compile += t.elapsed();
        self.shared.pipelines.extend(fresh.into_iter().zip(built));
        self.made
            .iter()
            .map(|(k, m)| (*k, self.shared.pipelines[m].clone()))
            .collect()
    }

    fn material_group(
        &mut self,
        fs: &Pk3Fs,
        bundle_views: &mut BundleViews,
        sh: &Material,
        lit: PhongLit,
    ) -> (wgpu::BindGroup, wgpu::Buffer, MaterialUniform) {
        let stage = sh.stages.first();
        let clamp = stage.and_then(|st| st.bundles.first()).map_or(0, |b| {
            (u32::from(b.clamp.s) * super::STAGE_FLAG_CLAMP_S)
                | (u32::from(b.clamp.t) * super::STAGE_FLAG_CLAMP_T)
        });
        let mut uniform = MaterialUniform::new(
            &lit,
            sh.nofog,
            stage.and_then(|st| st.alpha_func.clone()),
            clamp,
        );
        let path = |sem| match sh.texture(sem) {
            Some(ImageRef::Path(p)) => Some(p.clone()),
            _ => None,
        };
        // colour and detail as the stage path loads its images (gamma,
        // picmip); white for a missing detail map, which the switch skips
        let color = match path(TexSemantic::Color) {
            Some(p) => upload_bundle_view(self.device, self.queue, fs, bundle_views, &p),
            None => self.raw_view(fs, None, "$white", None, 0),
        };
        // detail maps take no picmip (0x616d40)
        let detail = match path(TexSemantic::Detail) {
            Some(p) => {
                let picmip = std::mem::take(&mut bundle_views.picmip);
                let v = upload_bundle_view(self.device, self.queue, fs, bundle_views, &p);
                bundle_views.picmip = picmip;
                v
            }
            None => self.raw_view(fs, None, "$white", None, 0),
        };
        // a flat normal without a map: (0.5, 0.5) in alpha and green
        let pre = bundle_views.prefetched;
        let [bump, spec] = self.picmip;
        let normal = self.raw_view(fs, pre, "$flatnormal", path(TexSemantic::Normal), bump);
        // a CoD4 water's height map in place of its runtime `watersetup`
        let normal = self.water_normal.take().unwrap_or(normal);
        let spec = self.raw_view(fs, pre, "$black", path(TexSemantic::Specular), spec);
        // which maps carry mipmaps, so `sample_mipped` asks no
        // `textureNumLevels` (cod4-client-perf.md 26)
        let mipped = |v: &wgpu::TextureView, b: u32| {
            if v.texture().mip_level_count() > 1 {
                b
            } else {
                0
            }
        };
        uniform.flags[3] = mipped(&color, MIP_COLOR)
            | mipped(&normal, MIP_NORMAL)
            | mipped(&spec, MIP_SPEC)
            | mipped(&detail, MIP_DETAIL);
        let ubuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cod2 material"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("cod2 material {}", sh.name)),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                tex_entry(1, &color),
                tex_entry(2, &normal),
                tex_entry(3, &spec),
                tex_entry(4, &detail),
            ],
        });
        (bg, ubuf, uniform)
    }

    /// An image uploaded as stored: `path`'s file, else (or when it does
    /// not load) the constant named by `fallback`; a CoD4 `.iwi` drops
    /// `picmip` levels unless its header refuses
    /// ([`texture_settings::iwi_picmip`]).
    fn raw_view(
        &mut self,
        fs: &Pk3Fs,
        prefetched: Option<&vcod_common::assets::DecodedImages>,
        fallback: &str,
        path: Option<String>,
        picmip: u32,
    ) -> wgpu::TextureView {
        let key = path.clone().unwrap_or_else(|| fallback.to_string());
        if let Some(v) = self.raw_views.get(&key) {
            return v.clone();
        }
        // the prefetch decoded an `.iwi` as this does
        let ahead = path
            .as_deref()
            .filter(|p| p.to_ascii_lowercase().ends_with(".iwi"))
            .zip(prefetched)
            .and_then(|(p, pre)| pre.take(fs, p));
        let img = ahead
            .or_else(|| {
                // a material's maps are all `.iwi`, which the asset decodes
                // as `iwi::decode_or_placeholder`
                let p = path.as_deref()?;
                Image::find(fs, p)
                    .inspect_err(|e| {
                        if !e.is::<ImageMissing>() {
                            log::warn!("cod2: {p}: {e:#}")
                        }
                    })
                    .ok()
            })
            .unwrap_or_else(|| constant_image(fallback));
        let picmip = match path.as_deref() {
            Some(p) if picmip > 0 && p.to_ascii_lowercase().ends_with(".iwi") => {
                texture_settings::iwi_picmip(&fs.read_unflagged(p).unwrap_or_default(), picmip)
            }
            _ => 0,
        };
        let mut img = img;
        if picmip > 0 {
            // a block-compressed map keeps its own levels below the drop
            let chain = matches!(img.data, ImageData::Bc { .. });
            let mips = texture_settings::Mips {
                chain,
                custom: None,
            };
            texture_settings::prepare(&mut img, picmip, None, Default::default(), mips);
        }
        let v = upload_levels(self.device, self.queue, &key, &img, &[]);
        self.raw_views.insert(key, v.clone());
        v
    }

    /// How a CoD4 surface's or static model's `primaryLightIndex` lights
    /// it, as a group key: 0 no primary light (technique 7), a spot or omni
    /// light's own index (10, 12), else [`SUN_KEY`] (8, 9). Every CoD2
    /// batch takes the sun.
    fn light_key(&self, light: u8) -> u8 {
        match self.model_lights.get(usize::from(light)) {
            _ if self.model_lights.is_empty() => SUN_KEY,
            _ if light == 0 => 0,
            Some(Some(_)) => light,
            _ => SUN_KEY,
        }
    }

    /// The `Sun` uniform of a group keyed [`Self::light_key`]: `None` for
    /// the sun's own buffer.
    fn light_buffer(&self, key: u8) -> Option<wgpu::Buffer> {
        let u = match key {
            SUN_KEY => return None,
            0 => self.sun.none(),
            k => self
                .sun
                .with_source(self.model_lights[usize::from(k)].as_ref()?),
        };
        Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("cod4 primary light"),
                    contents: bytemuck::bytes_of(&u),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
        )
    }

    fn page_group(&mut self, page: u16, probe: u8, light: u8) -> wgpu::BindGroup {
        let cube = self.probe_view(probe);
        let own = self.light_buffer(light);
        let [r, g, b, sun] = self
            .page_views
            .entry(page)
            .or_insert_with(|| {
                let p = &self.pages[usize::from(page)];
                let lm = vcod_common::mapworld::LIGHTMAP_SIZE as u32;
                let coeff = |k: usize| {
                    upload_plain(
                        self.device,
                        self.queue,
                        &format!("lightmap {page} coefficients {k}"),
                        (lm, lm),
                        wgpu::TextureFormat::Rgba8Unorm,
                        4,
                        &p.coeffs[k],
                    )
                };
                let sun = upload_plain(
                    self.device,
                    self.queue,
                    &format!("lightmap {page} sun"),
                    (2 * lm, 2 * lm),
                    wgpu::TextureFormat::R8Unorm,
                    1,
                    &p.sun,
                );
                [coeff(0), coeff(1), coeff(2), sun]
            })
            .clone();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("cod2 lightmap {page}")),
            layout: &self.lightmap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: own.as_ref().unwrap_or(&self.sun_buf).as_entire_binding(),
                },
                tex_entry(1, &r),
                tex_entry(2, &g),
                tex_entry(3, &b),
                tex_entry(4, &sun),
                tex_entry(5, &self.weights),
                tex_entry(6, &self.specularity),
                tex_entry(7, &cube),
                tex_entry(8, &self.black_volume),
            ],
        })
    }
}

/// A probe cube with its mip chain, faces in D3D's order as the cube's
/// layers (the sky's convention, `sky_cube_reads_the_face_the_view_looks_at`).
fn upload_probe(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    p: &vcod_common::mapworld::ProbeCube,
) -> wgpu::TextureView {
    let levels = p.faces.first().map_or(1, Vec::len) as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: p.size,
            height: p.size,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (face, levels) in p.faces.iter().enumerate() {
        for (level, px) in levels.iter().enumerate() {
            let side = (p.size >> level).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: face as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * side),
                    rows_per_image: Some(side),
                },
                wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// A CoD4 water material's height map (cod4-world.md 9.8): its spectrum
/// run to the view's time as 1.7's `R_UploadWaterTexture` does, once per
/// distinct time.
pub(super) struct Cod4WaterSim {
    setup: vcod_cod2::water::WaterSetup,
    texture: wgpu::Texture,
    time: Option<f32>,
}

impl Cod4WaterSim {
    pub fn update(&mut self, queue: &wgpu::Queue, time: f32) {
        if self.time == Some(time) {
            return;
        }
        self.time = Some(time);
        let levels = self.setup.levels_cod4(time);
        super::cod2_water::write_levels(queue, &self.texture, &levels);
    }
}

/// A 1 x 1 stand-in for a map a material lacks.
fn constant_image(name: &str) -> Image {
    let px = match name {
        "$white" => [255, 255, 255, 255],
        "$flatnormal" => [128, 128, 128, 128],
        _ => [0, 0, 0, 0],
    };
    Image {
        width: 1,
        height: 1,
        data: ImageData::Rgba8(px.to_vec()),
    }
}

/// The lit pipeline: the world vertex in buffer 0, [`tangent_stream`] in
/// buffer 1, depth LessEqual, wound as the map's triangles.
#[allow(clippy::too_many_arguments)]
pub(super) fn make_lit_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    two_sided: bool,
    (fs_entry, path): (&str, Option<LitPath>),
    bias: wgpu::DepthBiasState,
) -> wgpu::RenderPipeline {
    lit_pipeline(
        device,
        module,
        layout,
        format,
        blend,
        (depth_write, wgpu::CompareFunction::LessEqual),
        two_sided,
        Some((fs_entry, path)),
        bias,
        &[],
    )
}

/// `make` over `jobs`, in order, on eight threads
/// ([`super::par_build_on`]): a driver that compiles on the calling
/// thread (lavapipe) takes 10 to 20 ms a lit pipeline.
fn par_compile<T: Sync>(
    jobs: &[T],
    make: impl Fn(&T) -> wgpu::RenderPipeline + Sync,
) -> Vec<wgpu::RenderPipeline> {
    super::par_build_on(8, jobs.len(), |i| make(&jobs[i]))
}

/// The lit pipeline `made` asks for.
fn build_made(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    made: MadeKey,
) -> wgpu::RenderPipeline {
    make_lit_pipeline(
        device,
        module,
        layout,
        format,
        made.blend,
        made.depth_write,
        made.two_sided,
        (made.fs.0.name(), made.fs.1),
        match made.offset {
            Some(offset) => depth_bias(offset.unwrap_or(PolygonOffset::Single)),
            None => Default::default(),
        },
    )
}

/// The depth prepass pair of an opaque lit key ([`Cod2World::prepass`]):
/// with `fs` `None` the depth-only pipeline (no fragment stage, depth
/// written), else the colour pipeline that tests EQUAL against it and
/// writes no depth. Both run `vs_main`, whose position is `@invariant`
/// and reads no pipeline constant, so their depths match the plain
/// pipeline's bit for bit.
fn make_prepass_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    two_sided: bool,
    fs: Option<FsKey>,
) -> wgpu::RenderPipeline {
    let fs_entry = fs.map(|(e, p)| (e.name(), p));
    let depth = match fs_entry {
        None => (true, wgpu::CompareFunction::LessEqual),
        Some(_) => (false, wgpu::CompareFunction::Equal),
    };
    lit_pipeline(
        device,
        module,
        layout,
        format,
        None,
        depth,
        two_sided,
        fs_entry,
        Default::default(),
        &[],
    )
}

/// The depth prepass's pipeline on a CoD4 frame a feather reads
/// ([`Cod2World::floatz_pipeline`]): the depth-only one's depth, and
/// `fs_floatz`'s view depth into the R32F `floatZ` target, under `atest`
/// the material's alpha test first.
fn make_floatz_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    two_sided: bool,
    atest: bool,
) -> wgpu::RenderPipeline {
    lit_pipeline(
        device,
        module,
        layout,
        crate::soft_particles::LINZ_FORMAT,
        None,
        (true, wgpu::CompareFunction::LessEqual),
        two_sided,
        Some(("fs_floatz", None)),
        Default::default(),
        &[
            ("FLOATZ_ATEST", f64::from(u8::from(atest))),
            ("FLOATZ_NEAR", f64::from(crate::camera::Z_NEAR)),
            ("FLOATZ_FAR", f64::from(crate::camera::Z_FAR)),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn lit_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    (depth_write, depth_compare): (bool, wgpu::CompareFunction),
    two_sided: bool,
    fs_entry: Option<(&str, Option<LitPath>)>,
    bias: wgpu::DepthBiasState,
    extra: &[(&str, f64)],
) -> wgpu::RenderPipeline {
    let mut constants: Vec<(&str, f64)> = fs_entry
        .and_then(|(_, p)| p)
        .map(LitPath::constants)
        .map_or_else(Vec::new, |c| c.to_vec());
    constants.extend_from_slice(extra);
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match fs_entry {
            Some(("fs_floatz", _)) => "cod2 lit floatZ prepass",
            Some(_) => "cod2 lit pipeline",
            None => "cod2 lit depth prepass",
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: 44,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // pos
                        1 => Float32x2, // uv
                        2 => Float32x2, // lm_uv
                        3 => Float32x3, // normal
                        4 => Unorm8x4,  // color
                    ],
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        5 => Float32x3, // tangent
                        6 => Float32x3, // binormal
                    ],
                }),
            ],
        },
        fragment: fs_entry.map(|(entry, _)| wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            targets: &targets,
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: if two_sided {
                None
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil: Default::default(),
            bias,
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples(),
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// Uploads a cube's six faces (`D3DCUBEMAP_FACES` order, which is wgpu's
/// layer order) as one cube texture: every level of a block-compressed
/// cube, the top level of a bitmap one.
pub(super) fn upload_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    faces: &[Image],
) -> Option<wgpu::TextureView> {
    let first = faces.first()?;
    let (w, h) = (first.width, first.height);
    let (format, levels) = match &first.data {
        ImageData::Rgba8(_) => (wgpu::TextureFormat::Rgba8Unorm, 1),
        ImageData::Bc { format, mips } => (super::wgpu_format(*format), mips.len() as u32),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, face) in faces.iter().enumerate() {
        let data: Vec<&[u8]> = match &face.data {
            ImageData::Rgba8(px) => vec![px],
            ImageData::Bc { mips, .. } => mips.iter().map(Vec::as_slice).collect(),
        };
        for (level, bytes) in data.into_iter().enumerate() {
            let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
            let (copy, row, rows) = match format.block_copy_size(None) {
                Some(b) if format.is_compressed() => {
                    let (bw, bh) = (lw.div_ceil(4), lh.div_ceil(4));
                    ((bw * 4, bh * 4), bw * b, bh)
                }
                _ => ((lw, lh), lw * 4, lh),
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: copy.0,
                    height: copy.1,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    Some(texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    }))
}

/// The six faces of the cube `.iwi` at `path`; `None` (warned) when it is
/// missing, not a cube, or not square.
pub fn load_cube_faces(fs: &dyn AssetSource, path: &str) -> Option<Vec<Image>> {
    let Some(bytes) = fs.read(path) else {
        log::warn!("cod2 sky {path}: not found");
        return None;
    };
    let iwi = vcod_common::iwi::Iwi::parse_any(&bytes)
        .inspect_err(|e| log::warn!("cod2 sky {path}: {e:#}"))
        .ok()?;
    if !iwi.header.is_cube() || iwi.header.width != iwi.header.height {
        log::warn!("cod2 sky {path}: not a square cube map");
        return None;
    }
    Some((0..6).map(|f| iwi.face_image(f)).collect())
}

/// The CoD2 sky as a [`SkyBox`]: the farbox geometry, every side bound to
/// the one cube texture, `sky_cube.wgsl` sampling it along the direction.
/// `None` when the cube does not load (the sky then draws nothing).
pub(super) fn build_sky_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    fs: &dyn AssetSource,
    camera_layout: &wgpu::BindGroupLayout,
    path: &str,
) -> Option<SkyBox> {
    let faces = load_cube_faces(fs, path)?;
    let view = upload_cube(device, queue, path, &faces)?;
    let size = sky::box_size(Z_FAR, Z_NEAR);
    let mut verts: Vec<sky::SkyBoxVert> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut sides = [(0u32, 0u32); 6];
    for (axis, side_slot) in sides.iter_mut().enumerate() {
        let side = sky::box_side(axis, size);
        let first = indices.len() as u32;
        indices.extend(side.indices.iter().map(|&i| i + verts.len() as u32));
        *side_slot = (first, indices.len() as u32);
        verts.extend(side.verts);
    }
    let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("cod2 sky vertices"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("cod2 sky indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cod2 sky cube layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::Cube,
                multisampled: false,
            },
            count: None,
        }],
    });
    let cube_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cod2 sky cube"),
        layout: &tex_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let eye_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cod2 sky eye"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cod2 sky eye layout"),
        entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
    });
    let eye_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cod2 sky eye"),
        layout: &eye_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: eye_buf.as_entire_binding(),
        }],
    });
    let pipeline = make_sky_pipeline(device, format, camera_layout, &tex_layout, &eye_layout);
    log::info!(
        "cod2 sky: {path} {}x{}, {} levels",
        faces[0].width,
        faces[0].height,
        view.texture().mip_level_count()
    );
    Some(SkyBox {
        pipeline,
        vertex_buf,
        index_buf,
        eye_buf,
        eye_bg,
        side_bgs: vec![cube_bg; 6],
        sides,
        cube: Some(view),
    })
}

/// The sky pipeline: drawn first, depth Always without a write, uncull'd,
/// as the CoD1 farbox draws (`build_sky_box`).
fn make_sky_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    tex_layout: &wgpu::BindGroupLayout,
    eye_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky_cube.wgsl"),
        source: wgpu::ShaderSource::Wgsl(SKY_CUBE_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cod2 sky pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(tex_layout), Some(eye_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cod2 sky pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<sky::SkyBoxVert>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples(),
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one_and_favour_up_at_the_centre() {
        let w = lightmap_weights();
        assert_eq!(w.len(), 32 * 32 * 4);
        for t in w.as_chunks::<4>().0 {
            let sum: u32 = t.iter().map(|&b| u32::from(b)).sum();
            assert!((253..=257).contains(&sum), "{t:?}");
        }
        // texel (16, 16) sits a half texel off the pole: mostly up
        let c = &w[(16 * 32 + 16) * 4..][..4];
        assert!(c[0] > 240, "{c:?}");
        // the rim leans on the side lights only
        let rim = &w[(16 * 32) * 4..][..4];
        assert_eq!(rim[0], 0, "{rim:?}");
    }

    #[test]
    fn weights_turn_through_the_three_side_lights() {
        let w = lightmap_weights();
        let at = |row: usize, col: usize| -> [u8; 4] {
            w[(row * 32 + col) * 4..][..4].try_into().unwrap()
        };
        // azimuth 90 degrees (+y) is all byte 1's light, 210 byte 2's and
        // 330 byte 3's
        let texel = |deg: f32| {
            let (y, x) = deg.to_radians().sin_cos();
            let cell = |v: f32| (((v * 0.95 + 1.0) / 2.0) * 32.0) as usize;
            at(cell(y), cell(x))
        };
        for (deg, byte) in [(90.0, 1), (210.0, 2), (330.0, 3)] {
            let t = texel(deg);
            assert!(t[0] == 0 && t[byte] > 220, "{deg}: {t:?}");
        }
        // halfway between two lights they share
        let t = texel(150.0);
        assert!(t[1].abs_diff(t[2]) < 30 && t[3] == 0, "{t:?}");
    }

    #[test]
    fn specularity_columns_sharpen() {
        let s = specularity_map();
        assert_eq!(s.len(), 32 * 256);
        let at = |row: usize, col: usize| s[row * 32 + col];
        for col in 0..32 {
            assert_eq!(at(255, col), 255);
            assert_eq!(at(0, col), 0);
        }
        // column 0 falls to 0.1 at cos 45 degrees; column 30 is far sharper
        let r = (45f64.to_radians().cos() * 255.0).round() as usize;
        assert!((24..=28).contains(&at(r, 0)), "{}", at(r, 0));
        assert!(at(250, 30) < at(250, 0));
        // column 31's power 30: 0.9^30
        assert_eq!(
            at(230, 31),
            ((230f64 / 255.0).powf(30.0) * 255.0 + 0.5) as u8
        );
    }

    #[test]
    fn uniforms_pack_as_the_wgsl_structs() {
        assert_eq!(std::mem::size_of::<MaterialUniform>(), 64);
        // the sun's three vectors, then a CoD4 model light's four and its
        // 32-texel row
        assert_eq!(std::mem::size_of::<SunUniform>(), 48 + 64 + 512);
        let lit = PhongLit {
            specular: true,
            detail: false,
            alpha: true,
            detail_scale: [8.0, 4.0],
            cod4: None,
        };
        let u = MaterialUniform::new(&lit, false, Some(AlphaFunc::Ge128), 7);
        assert_eq!(u.flags, [M_SPECULAR | M_ALPHA, 3, 7, 0]);
        assert_eq!(u.detail_scale, [8.0, 4.0, 0.0, 0.0]);
    }

    #[test]
    fn tangents_pad_past_the_map() {
        let bsp = MapWorld {
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 2],
            ..Default::default()
        };
        let t = tangent_stream(&bsp, 5);
        assert_eq!(t.len(), 5);
        assert_eq!(t[1][1], [0.0, 1.0, 0.0]);
        assert_eq!(t[4], [[0.0; 3]; 2]);
    }

    /// A device off a headless adapter, `None` where the machine has none.
    pub(in crate::renderer) fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let info = adapter.get_info();
        eprintln!("adapter: {} {:?}", info.name, info.backend);
        // GL binds a texture to one sampler, which sample_image's
        // per-mode samplers break, here as in every stage pipeline
        if info.backend == wgpu::Backend::Gl {
            return None;
        }
        let bc = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cod2 test device"),
            required_features: bc,
            required_limits: super::super::device_limits(&adapter),
            ..Default::default()
        }))
        .ok()
    }

    /// The camera group the shaders read: the uniform and the image
    /// samplers (bindings 9..16), as `Renderer::new` lays it out.
    pub(in crate::renderer) fn camera_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        // the renderer's camera layout: the Camera block, the fx lights,
        // vc_shadows' receiver (the stage path reads them) and the samplers
        let mut entries = vec![
            uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
        ];
        entries.extend(crate::shadows::receive_layout_entries());
        for binding in 9..=16 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        // the dynamic shadow (renderer::cod2_cookie)
        entries.push(texture_entry(super::super::DYNAMIC_SHADOW_BINDING));
        // CoD4's sun shadow map (cod4_sunshadow)
        entries.extend(crate::cod4_sunshadow::layout_entries());
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("test camera layout"),
            entries: &entries,
        })
    }

    /// Builds the unsplit fragment stages and then each of shader_ops'
    /// paths split, one at a time behind a `LIT_REGS` line naming it, for
    /// a driver to report what each compiles to (cod4-client-perf.md 29):
    /// RADV with no card, `VK_DRIVER_FILES=<radeon_icd.json>
    /// RADV_FORCE_FAMILY=navi21 RADV_DEBUG=shaderstats`, prints each
    /// fragment shader's registers and waves after its line.
    #[test]
    #[ignore = "a driver's shader statistics; run by hand"]
    fn lit_pipeline_register_census() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let base = LitBase::new(&device, &queue, super::super::SCENE_FORMAT, &camera);
        let build = |label: &str, entry: LitEntry, path: Option<LitPath>| {
            eprintln!("LIT_REGS {label}");
            let made = MadeKey {
                two_sided: false,
                offset: None,
                fs: (entry, path),
                blend: None,
                depth_write: true,
            };
            build_made(
                &device,
                &base.module,
                &base.layout,
                super::super::SCENE_FORMAT,
                made,
            );
        };
        build("fs_main unsplit", LitEntry::Main, None);
        build("fs_cod4_model unsplit", LitEntry::Cod4Model, None);
        for &(label, entry, flags, light) in super::super::shader_ops::WORLD_PATHS {
            let entry = match entry {
                "fs_main" => LitEntry::Main,
                _ => LitEntry::Cod4Model,
            };
            let light = light.parse::<f32>().expect("a light kind") as i8;
            let path = LitPath {
                flags,
                test: 0,
                light,
            };
            build(&format!("{label} split"), entry, Some(path));
        }
    }

    /// Every lit pipeline, a material and a page bind group, and the sky
    /// pipeline over a cube build without a validation error. Passes
    /// vacuously on a machine with no adapter.
    #[test]
    fn lit_and_sky_pipelines_build_on_a_device() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera = camera_layout(&device);
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: std::array::from_fn(|_| vec![128; 512 * 512 * 4]),
                sun: vec![255; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            ..Default::default()
        };
        let fs = Pk3Fs::empty();
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let sh = Material {
            phong: Some(PhongLit {
                specular: true,
                detail: true,
                alpha: false,
                detail_scale: [8.0, 8.0],
                cod4: None,
            }),
            depth_bias: Some(PolygonOffset::Single),
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        for v in [
            StageVariant::Opaque,
            StageVariant::Blend,
            StageVariant::BlendDepthWrite,
        ] {
            for two_sided in [false, true] {
                for bias in [false, true] {
                    assert!(draws_lit(&sh, 0, b.page_count(), v));
                    let bind = b.bind(&fs, &mut views, &sh, 0, v, two_sided, bias);
                    assert_eq!(
                        bind,
                        LitBind {
                            material: 0,
                            lightmap: 0,
                            entry: LitEntry::Main,
                            water: false,
                        }
                    );
                }
            }
        }
        assert!(!draws_lit(&sh, 1, b.page_count(), StageVariant::Opaque));
        assert!(!draws_lit(
            &sh,
            0,
            b.page_count(),
            StageVariant::AdditiveFixed
        ));
        let world = b.finish(&tangent_stream(&bsp, 3));
        assert_eq!(world.pipelines.len(), 12);

        let faces: Vec<Image> = (0..6)
            .map(|_| Image {
                width: 4,
                height: 4,
                data: ImageData::Rgba8(vec![200; 64]),
            })
            .collect();
        let view = upload_cube(&device, &queue, "test cube", &faces).unwrap();
        assert_eq!(view.texture().depth_or_array_layers(), 6);
        let tex = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let eye = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        make_sky_pipeline(&device, super::super::SCENE_FORMAT, &camera, &tex, &eye);
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
        let err = pollster::block_on(internal.pop());
        assert!(err.is_none(), "{err:?}");
    }

    /// The camera uniform's bytes (shader.wgsl's `Camera`): `view_proj`,
    /// identityLight 1, the eye, `exp_fog` (colour, density, radial as
    /// CoD2's) or none, an identity model.
    pub(in crate::renderer) fn camera_bytes(
        view_proj: glam::Mat4,
        eye: glam::Vec3,
        exp_fog: Option<([f32; 3], f32)>,
    ) -> Vec<u8> {
        let mut f: Vec<f32> = view_proj.to_cols_array().to_vec();
        f.extend([0.0, 1.0, 0.0, 0.0]);
        let ([r, g, b], density) = exp_fog.unwrap_or_default();
        f.extend([eye.x, eye.y, eye.z, f32::from(u8::from(exp_fog.is_some()))]);
        f.extend([r, g, b, density]);
        // fog_range.z: CoD2's radial fog distance
        f.extend([0.0, 0.0, f32::from(u8::from(exp_fog.is_some())), 0.0]);
        f.extend([0.0; 4]);
        f.extend(glam::Mat4::IDENTITY.to_cols_array());
        bytemuck::cast_slice(&f).to_vec()
    }

    pub(in crate::renderer) fn camera_group(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        bytes: &[u8],
    ) -> wgpu::BindGroup {
        camera_group_sun(device, queue, layout, bytes, None)
    }

    /// [`camera_group`] reading `sun`'s sun shadow map (none drawn without).
    pub(in crate::renderer) fn camera_group_sun(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        bytes: &[u8],
        sun: Option<&crate::cod4_sunshadow::SunShadowPass>,
    ) -> wgpu::BindGroup {
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let lights = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let off = crate::shadows::ReceiveOff::new(device);
        let recv = off.receive();
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights.as_entire_binding(),
            },
        ];
        entries.extend(recv.entries());
        for binding in 9..=16 {
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        // no shadow cookie: a fresh dynamic shadow takes nothing
        let unshadowed = super::super::cod2_cookie::DynamicShadows::new(device, queue, 1, 1);
        entries.push(tex_entry(
            super::super::DYNAMIC_SHADOW_BINDING,
            unshadowed.view(),
        ));
        // no sun shadow map drawn, or `sun`'s
        let (sun_view, sun_buf) =
            crate::cod4_sunshadow::off_resources(device, bytemuck::Zeroable::zeroed());
        let sun_sampler = crate::cod4_sunshadow::receive_sampler(device);
        match sun {
            Some(sun) => entries.extend(sun.entries()),
            None => entries.extend(crate::cod4_sunshadow::entries(
                &sun_view,
                &sun_buf,
                &sun_sampler,
            )),
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        })
    }

    /// Runs `draw` into an 8 x 8 multisampled scene-format target and
    /// returns its resolved centre texel.
    fn centre_texel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) -> [u8; 4] {
        let target = |samples, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let att = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa = target(msaa_samples(), super::super::SCENE_FORMAT, att);
        let depth = target(msaa_samples(), DEPTH_FORMAT, att);
        let resolved = target(
            1,
            super::super::SCENE_FORMAT,
            att | wgpu::TextureUsages::COPY_SRC,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (mv, dv, rv) = (
            msaa.create_view(&Default::default()),
            depth.create_view(&Default::default()),
            resolved.create_view(&Default::default()),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &mv,
                    depth_slice: None,
                    resolve_target: Some(&rv),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            draw(&mut pass);
        }
        enc.copy_texture_to_buffer(
            resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(8),
                },
            },
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        let at = 4 * 256 + 4 * 4;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }

    /// A screen-filling lit triangle on a flat normal map: the lightmap's
    /// coefficients through the basis weights (which sum to one) plus the
    /// sun straight overhead at full visibility. Vulkan or Metal only.
    #[test]
    fn lit_surface_shades_lightmap_plus_sun() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let coeff = |v: u8| vec![v; 512 * 512 * 4];
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: [coeff(200), coeff(100), coeff(0)],
                sun: vec![255; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            entities: "{\n\"classname\" \"worldspawn\"\n\"sundirection\" \"-90 0 0\"\n\"sunlight\" \"0.4\"\n\"suncolor\" \"1 1 1\"\n}\n".into(),
            ..Default::default()
        };
        let fs = Pk3Fs::empty();
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let sh = Material {
            phong: Some(PhongLit {
                specular: false,
                detail: false,
                alpha: false,
                detail_scale: [1.0, 1.0],
                cod4: None,
            }),
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let bind = b.bind(&fs, &mut views, &sh, 0, StageVariant::Opaque, true, false);
        let tangents = tangent_stream(&bsp, 3);
        let world = b.finish(&tangents);
        // clip-space positions under an identity view_proj
        let verts: Vec<vcod_common::mapworld::DrawVert> = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]]
            .map(|[x, y]| vcod_common::mapworld::DrawVert {
                pos: [x, y, 0.5],
                uv: [0.5, 0.5],
                lm_uv: [0.5, 0.5],
                normal: [0.0, 0.0, 1.0],
                color: [255; 4],
            })
            .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let eye = glam::Vec3::new(0.0, 0.0, 10.0);
        let shade = |fog| {
            let cam = camera_group(
                &device,
                &queue,
                &camera,
                &camera_bytes(glam::Mat4::IDENTITY, eye, fog),
            );
            centre_texel(&device, &queue, |pass| {
                pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
                pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
                pass.draw(0..3, 0..1);
            })
        };
        let close = |px: [u8; 4], want: [f32; 3]| {
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 3.0,
                    "{px:?} vs {want:?}"
                );
            }
        };
        // lightmap (200, 100, 0) plus the sun's 0.2 * 255 = 51 on each
        let lit = [251.0, 151.0, 51.0];
        close(shade(None), lit);
        // exp fog per vertex (cod2-client-perf.md 9): the centre texel,
        // (0, 0) at barycentrics (1/2, 1/4, 1/4), keeps the vertices'
        // keeps so weighted and blends the rest toward blue
        let k = |x: f32, y: f32| (-0.1 * (glam::Vec3::new(x, y, 0.5) - eye).length()).exp();
        let keep = 0.5 * k(-1.0, -1.0) + 0.25 * k(3.0, -1.0) + 0.25 * k(-1.0, 3.0);
        let fogged = [0.0, 0.0, 255.0].map(|f: f32| f * (1.0 - keep));
        close(
            shade(Some(([0.0, 0.0, 1.0], 0.1))),
            [0, 1, 2].map(|k| lit[k] * keep + fogged[k]),
        );
    }

    /// A 4 x 4 `.iwi` of one A8R8G8B8 texel `rgba`, unmipped.
    fn flat_iwi(rgba: [u8; 4]) -> Vec<u8> {
        let mut f = b"IWi".to_vec();
        f.extend([5, 1, 0x3]);
        f.extend([4u16, 4, 1].iter().flat_map(|v| v.to_le_bytes()));
        f.extend([0u8; 16]);
        for _ in 0..16 {
            f.extend([rgba[2], rgba[1], rgba[0], rgba[3]]);
        }
        f
    }

    /// `tex` (`w` x `h`, `stride` bytes a texel) filtered linearly with
    /// clamped edges at (`u`, `v`), byte `comp` of each texel, in [0, 1].
    fn bilinear(
        tex: &[u8],
        (w, h): (usize, usize),
        stride: usize,
        comp: usize,
        u: f32,
        v: f32,
    ) -> f32 {
        let at = |x: f32, y: f32| {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            f32::from(tex[(y as usize * w + x as usize) * stride + comp]) / 255.0
        };
        let (x, y) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
        let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// A screen-filling `lmap_s_dtl` triangle against `lib/lightmap.hlsl`,
    /// `lib/normalmap.hlsl`, `lib/shade.hlsl` and `lib/phong.hlsl` worked by
    /// hand: the detail added as `detail - 0.5` before the vertex colour
    /// multiplies, the normal map's alpha along the tangent and green along
    /// the binormal, the four basis coefficients weighted at the raw
    /// normal sample, and the sun's diffuse and specular off the tilted
    /// normal (cod2-materials.md 9, 9.1). Vulkan or Metal only.
    #[test]
    fn detail_and_normal_mapped_surface_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        // four different coefficients per channel, so the weights matter
        let basis = |c: [u8; 4]| c.repeat(512 * 512);
        let coeffs = [[90, 30, 50, 70], [70, 20, 40, 60], [40, 10, 30, 50]];
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: coeffs.map(basis),
                sun: vec![255; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            // the sun 45 degrees up along +x, colour (0.8 - 0) * 0.5
            entities: "{\n\"classname\" \"worldspawn\"\n\"sundirection\" \"-45 0 0\"\n\"sunlight\" \"0.8\"\n\"suncolor\" \"1 1 1\"\n}\n".into(),
            ..Default::default()
        };
        let (color, detail, normal, spec) = (
            [153u8, 102, 51, 255],
            [191u8, 191, 191, 255],
            // x 0.75 in alpha, y 0.5 in green
            [128u8, 128, 128, 191],
            [128u8, 128, 128, 128],
        );
        let mut fs = Pk3Fs::empty();
        for (name, rgba) in [("c", color), ("d", detail), ("n", normal), ("s", spec)] {
            fs.overlay(&format!("images/{name}.iwi"), flat_iwi(rgba));
        }
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let bind_tex = |semantic, name: &str| vcod_common::material::TextureBinding {
            semantic,
            image: ImageRef::Path(format!("images/{name}.iwi")),
        };
        let sh = Material {
            phong: Some(PhongLit {
                specular: true,
                detail: true,
                alpha: false,
                detail_scale: [8.0, 8.0],
                cod4: None,
            }),
            textures: vec![
                bind_tex(TexSemantic::Color, "c"),
                bind_tex(TexSemantic::Detail, "d"),
                bind_tex(TexSemantic::Normal, "n"),
                bind_tex(TexSemantic::Specular, "s"),
            ],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let bind = b.bind(&fs, &mut views, &sh, 0, StageVariant::Opaque, true, false);
        let world = b.finish(&tangent_stream(&bsp, 3));
        let vcolor = [255u8, 204, 255, 255];
        let corners = [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]];
        let verts: Vec<vcod_common::mapworld::DrawVert> = corners
            .map(|[x, y]| vcod_common::mapworld::DrawVert {
                pos: [x, y, 0.5],
                uv: [0.5, 0.5],
                lm_uv: [0.5, 0.5],
                normal: [0.0, 0.0, 1.0],
                color: vcolor,
            })
            .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let eye = glam::Vec3::new(0.0, 0.0, 10.0);
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, eye, None),
        );
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });

        // the HLSL by hand
        let unit = |b: u8| f32::from(b) / 255.0;
        let diffuse: [f32; 3] =
            std::array::from_fn(|k| (unit(color[k]) + unit(detail[k]) - 0.5) * unit(vcolor[k]));
        let (nx, ny) = (unit(normal[3]), unit(normal[1]));
        let weights = lightmap_weights();
        let w: [f32; 4] = std::array::from_fn(|c| bilinear(&weights, (32, 32), 4, c, nx, ny));
        let lm: [f32; 3] =
            std::array::from_fn(|k| (0..4).map(|c| unit(coeffs[k][c]) * w[c]).sum::<f32>());
        let n = glam::Vec3::new(2.0 * nx - 1.0, 2.0 * ny - 1.0, 1.0).normalize();
        let sun = glam::Vec3::new(1.0, 0.0, 1.0).normalize();
        let sun_colour = 0.4;
        let n_dot_l = n.dot(sun).max(0.0);
        // the half angle per vertex, interpolated to the centre texel at
        // clip (0.125, -0.125) and renormalized
        let half = |p: glam::Vec3| ((eye - p).normalize() + sun).normalize();
        let [h0, h1, h2] = corners.map(|[x, y]| half(glam::Vec3::new(x, y, 0.5)));
        let (cx, cy) = (0.125f32, -0.125f32);
        let (b1, b2) = ((cx + 1.0) / 4.0, (cy + 1.0) / 4.0);
        let h = (h0 * (1.0 - b1 - b2) + h1 * b1 + h2 * b2).normalize();
        let strength = bilinear(&specularity_map(), (32, 256), 1, 0, unit(spec[3]), h.dot(n));
        let want: [f32; 3] = std::array::from_fn(|k| {
            let rgb =
                diffuse[k] * (lm[k] + sun_colour * n_dot_l) + sun_colour * unit(spec[k]) * strength;
            rgb * 255.0
        });
        eprintln!("got {px:?}, want {want:?} (weights {w:?}, N.L {n_dot_l}, strength {strength})");
        // the tilt is real: a flat normal would read byte 0's coefficient
        assert!(w[0] < 0.6 && w[1] + w[2] + w[3] > 0.4, "{w:?}");
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 3.0,
                "{px:?} vs {want:?}"
            );
        }
    }

    /// A screen-filling CoD4 `wc_l_sm_r0c0n0s0` triangle against
    /// `lm_sun_r0c0n0s0_sm3.hlsl` worked by hand (cod4-world.md 9.6): the
    /// secondary's halves A and B with their direction alphas, the normal
    /// map decoded as the direction is, A by the normal's z, B by the
    /// cosine between them, the sun off the tilted normal, and the sun's
    /// reflection by the specular map's alpha and `envMapParms`' fresnel.
    /// No reflection probe. Vulkan or Metal only.
    #[test]
    fn cod4_normal_and_specular_mapped_surface_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let (a, b) = ([90u8, 60, 40, 150], [120u8, 100, 70, 110]);
        let sun_dir = glam::Vec3::new(-0.65, 0.65, 0.33).normalize();
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: [
                    a.repeat(512 * 512),
                    b.repeat(512 * 512),
                    vec![0; 4 * 512 * 512],
                ],
                sun: vec![204; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            sun: Some(vcod_common::mapworld::MapSun {
                dir: sun_dir.to_array(),
                colour: [0.6, 0.5, 0.4],
            }),
            ..Default::default()
        };
        let (color, normal, spec) = (
            [153u8, 102, 51, 255],
            // x in alpha, y in green
            [128u8, 160, 128, 100],
            [200u8, 180, 160, 170],
        );
        let mut fs = Pk3Fs::empty();
        for (name, rgba) in [("c", color), ("n", normal), ("s", spec)] {
            fs.overlay(&format!("images/{name}.iwi"), flat_iwi(rgba));
        }
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut builder =
            Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let bind_tex = |semantic, name: &str| vcod_common::material::TextureBinding {
            semantic,
            image: ImageRef::Path(format!("images/{name}.iwi")),
        };
        let env = [0.25, 0.9, 3.0, 1.5];
        let sh = Material {
            phong: Some(PhongLit {
                specular: true,
                detail: false,
                alpha: false,
                detail_scale: [1.0, 1.0],
                cod4: Some(vcod_common::material::Cod4Lit {
                    normal_map: true,
                    env,
                    light: vcod_common::material::Cod4Light::Lightmap,
                    water_color: [0.0; 4],
                    flag: false,
                    sun_shadow: false,
                }),
            }),
            textures: vec![
                bind_tex(TexSemantic::Color, "c"),
                bind_tex(TexSemantic::Normal, "n"),
                bind_tex(TexSemantic::Specular, "s"),
            ],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let bind = builder.bind(&fs, &mut views, &sh, 0, StageVariant::Opaque, true, false);
        let world = builder.finish(&tangent_stream(&bsp, 3));
        let vcolor = [255u8, 204, 255, 255];
        let corners = [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]];
        let verts: Vec<vcod_common::mapworld::DrawVert> = corners
            .map(|[x, y]| vcod_common::mapworld::DrawVert {
                pos: [x, y, 0.5],
                uv: [0.5, 0.5],
                lm_uv: [0.5, 0.5],
                normal: [0.0, 0.0, 1.0],
                color: vcolor,
            })
            .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let eye = glam::Vec3::new(0.4, -0.2, 3.0);
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, eye, None),
        );
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });

        // the HLSL by hand
        let unit = |b: u8| f32::from(b) / 255.0;
        let dir = glam::Vec2::new(unit(a[3]) * 4.08 - 2.08, unit(b[3]) * 4.064_516 - 2.064_516);
        let nt = glam::Vec2::new(
            unit(normal[3]) * 4.08 - 2.08,
            unit(normal[1]) * 4.064_516 - 2.064_516,
        );
        let (rl, rn) = (
            1.0 / (dir.length_squared() + 1.0).sqrt(),
            1.0 / (nt.length_squared() + 1.0).sqrt(),
        );
        let cosine = ((dir.dot(nt) + 1.0) * rl * rn).clamp(0.0, 1.0);
        let n = glam::Vec3::new(nt.x, nt.y, 1.0).normalize();
        let vis = unit(204);
        let sun_colour = [0.6f32, 0.5, 0.4];
        let n_dot_l = n.dot(sun_dir).clamp(0.0, 1.0);
        // the centre texel at clip (0.125, -0.125), z 0.5
        let v = (glam::Vec3::new(0.125, -0.125, 0.5) - eye).normalize();
        let vn = v.dot(n);
        let r = v - 2.0 * vn * n;
        let power = (unit(spec[3]) * 9.377_518).exp2() + 7.0;
        let glint = ((r.dot(sun_dir) - 0.99925) * power).exp().clamp(0.0, 1.0);
        let fresnel = env[0] + (env[1] - env[0]) * (1.0 - vn.abs()).powf(env[2]);
        let want: [f32; 3] = std::array::from_fn(|k| {
            let diffuse = unit(color[k]) * unit(vcolor[k]);
            let light = unit(a[k]) * rn + unit(b[k]) * cosine + vis * sun_colour[k] * n_dot_l;
            let specular = vis * glint * env[3] * sun_colour[k] * unit(spec[k]) * fresnel;
            (diffuse * light + specular) * 255.0
        });
        eprintln!("got {px:?}, want {want:?} (cos {cosine}, glint {glint}, fresnel {fresnel})");
        assert!(glint > 0.05, "{glint}");
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 3.0,
                "{px:?} vs {want:?}"
            );
        }
    }

    /// CoD4's sun shadow map end to end (cod4-world.md 3.6): a floor and,
    /// 32 units over its west half, a slab cast into the two partitions
    /// fitted around a view, and the floor drawn through technique 9
    /// (`wc_l_sm_*`, black secondary, the primary 255) against
    /// `lm_sm_sun_r0c0_sm3.hlsl` worked by hand: the sun's `N.L` diffuse
    /// times the visibility the bytecode's 4-tap test reads off the atlas
    /// the casters wrote, read back. Three views look at the slab's
    /// shadow, the open floor and the shadow's edge, where half the taps
    /// fall in the shadow. Vulkan or Metal only.
    #[test]
    fn cod4_floor_takes_the_sun_shadow_map_as_the_hlsl_does() {
        use crate::cod4_sunshadow::SunShadowPass;
        use vcod_cod4::sunshadow::{self, PARTITION, View};
        use vcod_common::mapworld::{MapSun, Model, SunCaster, TriangleSoup};
        use vcod_common::material::{Cod4Light, Cod4Lit};
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let to_sun = glam::Vec3::new(0.3, 0.1, 1.0).normalize();
        let vert = |x: f32, y: f32, z: f32| vcod_common::mapworld::DrawVert {
            pos: [x, y, z],
            uv: [0.5, 0.5],
            lm_uv: [0.5, 0.5],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        };
        let quad = |x0: f32, x1: f32, z: f32| {
            [
                vert(x0, -512.0, z),
                vert(x1, -512.0, z),
                vert(x1, 512.0, z),
                vert(x0, 512.0, z),
            ]
        };
        let soup = |material: u16, first_vertex: u32| TriangleSoup {
            material,
            lightmap: 0,
            first_vertex,
            vertex_count: 4,
            index_count: 6,
            first_index: 0,
        };
        let caster = Some(SunCaster {
            alpha_test: false,
            image: String::new(),
        });
        let mut verts = quad(-512.0, 512.0, 0.0).to_vec();
        verts.extend(quad(-512.0, 0.0, 32.0));
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: [
                    [0, 0, 0, 130].repeat(512 * 512),
                    [0, 0, 0, 130].repeat(512 * 512),
                    vec![0; 4 * 512 * 512],
                ],
                sun: vec![255; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 8],
            sun: Some(MapSun {
                dir: to_sun.to_array(),
                colour: [0.5, 0.5, 0.5],
            }),
            soups: vec![soup(0, 0), soup(1, 4)],
            indices: vec![0, 1, 2, 0, 2, 3],
            verts: verts.clone(),
            sun_casters: vec![caster.clone(), caster],
            models: vec![Model {
                mins: [-512.0, -512.0, 0.0],
                maxs: [512.0, 512.0, 32.0],
                first_soup: 0,
                num_soups: 2,
                first_brush: 0,
                num_brushes: 0,
                partitions: (0, 0),
            }],
            ..Default::default()
        };
        let mut fs = Pk3Fs::empty();
        fs.overlay("images/c.iwi", flat_iwi([255; 4]));
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut builder =
            Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let sh = Material {
            phong: Some(PhongLit {
                specular: false,
                detail: false,
                alpha: false,
                detail_scale: [1.0, 1.0],
                cod4: Some(Cod4Lit {
                    normal_map: false,
                    env: [0.0; 4],
                    light: Cod4Light::Lightmap,
                    water_color: [0.0; 4],
                    flag: false,
                    sun_shadow: true,
                }),
            }),
            textures: vec![vcod_common::material::TextureBinding {
                semantic: TexSemantic::Color,
                image: ImageRef::Path("images/c.iwi".into()),
            }],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let bind = builder.bind(&fs, &mut views, &sh, 0, StageVariant::Opaque, true, false);
        let world = builder.finish(&tangent_stream(&bsp, verts.len()));
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mut pass = SunShadowPass::new(&device, &queue);
        let casters = pass
            .casters(&device, &bsp, &[0, 0], |_| unreachable!("no cut"))
            .expect("both soups cast");

        // eye 48 up, looking down 50 degrees toward `at` on the floor
        let proj = glam::camera::rh::proj::directx::perspective(
            80f32.to_radians(),
            1.0,
            crate::camera::Z_NEAR,
            60000.0,
        );
        let mut seen = Vec::new();
        for at_x in [-60.0f32, 40.0, -16.55] {
            let at = glam::Vec3::new(at_x, 0.0, 0.0);
            let eye = at + glam::Vec3::new(40.0, 6.0, 48.0);
            let fwd = (at - eye).normalize();
            let view_proj = proj * glam::camera::rh::view::look_to_mat4(eye, fwd, glam::Vec3::Z);
            let mut enc = device.create_command_encoder(&Default::default());
            pass.encode(
                &device,
                &queue,
                &mut enc,
                Some((&casters, &vbuf)),
                None,
                (view_proj, eye, fwd, crate::camera::Z_NEAR),
                to_sun,
                &Default::default(),
            );
            // the atlas as written, whole (a depth copy takes no part): the
            // partitions, then the spot's tile
            let width = PARTITION + crate::cod4_spotshadow::TILE;
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(width * PARTITION * 2 * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: pass.atlas(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(2 * PARTITION),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: 2 * PARTITION,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let atlas: Vec<f32> =
                bytemuck::cast_slice(&readback.slice(..).get_mapped_range().unwrap()).to_vec();
            // the same view again: the partitions' static layers come out
            // of their caches, the atlas bit for bit the same
            let redraws = pass.sun_redraws();
            let mut enc = device.create_command_encoder(&Default::default());
            pass.encode(
                &device,
                &queue,
                &mut enc,
                Some((&casters, &vbuf)),
                None,
                (view_proj, eye, fwd, crate::camera::Z_NEAR),
                to_sun,
                &Default::default(),
            );
            assert_eq!(pass.sun_redraws(), redraws, "a still view redrew");
            let first = atlas;
            let width = PARTITION + crate::cod4_spotshadow::TILE;
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(width * PARTITION * 2 * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: pass.atlas(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(2 * PARTITION),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: 2 * PARTITION,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let atlas: Vec<f32> =
                bytemuck::cast_slice(&readback.slice(..).get_mapped_range().unwrap()).to_vec();
            assert!(first == atlas, "the cached atlas differs");

            let cam = camera_group_sun(
                &device,
                &queue,
                &camera,
                &camera_bytes(view_proj, eye, None),
                Some(&pass),
            );
            let px = centre_texel(&device, &queue, |p| {
                p.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
                p.set_bind_group(0, &cam, &[]);
                p.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
                p.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
                p.set_vertex_buffer(0, vbuf.slice(..));
                p.set_vertex_buffer(1, world.tangent_buf.slice(..));
                p.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                p.draw_indexed(0..6, 0, 0..1);
            });

            // the centre texel's floor point, clip (0.125, -0.125)
            let inv = view_proj.inverse();
            let (a, b) = (
                inv.project_point3(glam::Vec3::new(0.125, -0.125, 0.0)),
                inv.project_point3(glam::Vec3::new(0.125, -0.125, 1.0)),
            );
            let p = a + (b - a) * (a.z / (a.z - b.z));
            let sh = sunshadow::setup(
                &View::of(view_proj, eye, fwd, crate::camera::Z_NEAR),
                to_sun,
                (
                    glam::Vec3::new(-512.0, -512.0, 0.0),
                    glam::Vec3::new(512.0, 512.0, 32.0),
                ),
                &Default::default(),
            );
            let s = sh.lookup * p.extend(1.0);
            let vis = sunshadow::visibility(&sh, s, 1.0, |x, y| atlas[(y * width + x) as usize]);
            let want = vis * 0.5 * to_sun.z * 255.0;
            eprintln!("floor at {p}: lookup {s}, visibility {vis}, got {px:?}, want {want}");
            assert!(s.w < 0.75, "the near partition alone: {s}");
            for k in 0..3 {
                assert!((f32::from(px[k]) - want).abs() <= 2.0, "{px:?} vs {want}");
            }
            seen.push(vis);
        }
        // the slab shades its side of the floor and leaves the other lit,
        // and the taps part on its edge
        assert_eq!(&seen[..2], &[0.0, 1.0]);
        assert!(seen[2] > 0.0 && seen[2] < 1.0, "{seen:?}");
    }

    /// A CoD4 world surface by its `primaryLightIndex` against the
    /// `lm_*_r0c0_sm3.hlsl` techniques worked by hand (cod4-world.md 3.5):
    /// an omni light (`lm_omni`, 12) and a spot (`lm_spot`, 10) add their
    /// colour by the attenuation row at the distance over the radius, the
    /// spot's by its cone, times the primary and `N.L` to the secondary's
    /// light, with no sun; the sun's index adds the sun (`lm_sun`, 8);
    /// index 0 (`lm`, 7) the secondary alone. Vulkan or Metal only.
    #[test]
    fn cod4_world_surface_takes_its_primary_light_as_the_hlsl_does() {
        use vcod_common::mapworld::{MapSun, ModelLightSource};
        use vcod_common::material::{Cod4Light, Cod4Lit};
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let (a, b) = ([90u8, 60, 40, 150], [120u8, 100, 70, 110]);
        let sun_dir = glam::Vec3::new(-0.65, 0.65, 0.33).normalize();
        let sun_colour = [0.6f32, 0.5, 0.4];
        // the centre texel at clip (0.125, -0.125), z 0.5
        let p = glam::Vec3::new(0.125, -0.125, 0.5);
        let row: Vec<[u8; 4]> = (0..32u8)
            .map(|i| [250 - 6 * i, 240 - 4 * i, 230 - 7 * i, 255])
            .collect();
        let origin = p + glam::Vec3::new(20.0, -30.0, 25.0);
        let (outer, inner) = (0.9f32, 0.99f32);
        let scale = 1.0 / (inner - outer);
        let omni = ModelLightSource {
            spot: false,
            can_shadow: true,
            origin: origin.to_array(),
            inv_radius: 1.0 / 100.0,
            colour: [0.8, 0.6, 0.4],
            dir: (origin - p - glam::Vec3::new(15.0, 0.0, 0.0))
                .normalize()
                .to_array(),
            spot_factors: [scale, -scale * outer, 2.0],
            attenuation: row.clone(),
        };
        let spot = ModelLightSource {
            spot: true,
            can_shadow: true,
            ..omni.clone()
        };
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: [
                    a.repeat(512 * 512),
                    b.repeat(512 * 512),
                    vec![0; 4 * 512 * 512],
                ],
                sun: vec![204; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            model_lights: vec![None, None, Some(omni.clone()), Some(spot.clone())],
            sun: Some(MapSun {
                dir: sun_dir.to_array(),
                colour: sun_colour,
            }),
            ..Default::default()
        };
        let color = [153u8, 102, 51, 255];
        let mut fs = Pk3Fs::empty();
        fs.overlay("images/c.iwi", flat_iwi(color));
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut builder =
            Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let sh = Material {
            phong: Some(PhongLit {
                specular: false,
                detail: false,
                alpha: false,
                detail_scale: [1.0, 1.0],
                cod4: Some(Cod4Lit {
                    normal_map: false,
                    env: [0.0; 4],
                    light: Cod4Light::Lightmap,
                    water_color: [0.0; 4],
                    flag: false,
                    sun_shadow: false,
                }),
            }),
            textures: vec![vcod_common::material::TextureBinding {
                semantic: TexSemantic::Color,
                image: ImageRef::Path("images/c.iwi".into()),
            }],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let binds = [0u8, 1, 2, 3].map(|light| {
            builder.bind_in(
                &fs,
                &mut views,
                &sh,
                (0, 0, light),
                StageVariant::Opaque,
                true,
                false,
            )
        });
        let world = builder.finish(&tangent_stream(&bsp, 3));
        let vcolor = [255u8, 204, 255, 255];
        let verts: Vec<vcod_common::mapworld::DrawVert> =
            [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]]
                .map(|[x, y]| vcod_common::mapworld::DrawVert {
                    pos: [x, y, 0.5],
                    uv: [0.5, 0.5],
                    lm_uv: [0.5, 0.5],
                    normal: [0.0, 0.0, 1.0],
                    color: vcolor,
                })
                .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, glam::Vec3::new(0.4, -0.2, 3.0), None),
        );

        // the HLSL by hand
        let unit = |v: u8| f32::from(v) / 255.0;
        let dir = glam::Vec2::new(unit(a[3]) * 4.08 - 2.08, unit(b[3]) * 4.064_516 - 2.064_516);
        let cosine = (1.0 / (dir.length_squared() + 1.0).sqrt()).clamp(0.0, 1.0);
        let vis = unit(204);
        let n = glam::Vec3::Z;
        let lit = |k: usize, primary: f32| {
            let light = unit(a[k]) + unit(b[k]) * cosine + primary;
            unit(color[k]) * unit(vcolor[k]) * light * 255.0
        };
        let source = |l: &ModelLightSource, k: usize| {
            let lv = glam::Vec3::from(l.origin) - p;
            let ld = lv.normalize();
            let x = ((lv.length() * l.inv_radius).clamp(0.0, 1.0) * 32.0 - 0.5).clamp(0.0, 31.0);
            let (i0, f) = (x.floor() as usize, x.fract());
            let i1 = (i0 + 1).min(31);
            let att = unit(row[i0][k]) * (1.0 - f) + unit(row[i1][k]) * f;
            let mut cone = 1.0;
            if l.spot {
                let s = (ld.dot(glam::Vec3::from(l.dir)) * l.spot_factors[0] + l.spot_factors[1])
                    .clamp(0.0, 1.0);
                cone = if s <= 0.0 {
                    0.0
                } else {
                    s.powf(l.spot_factors[2])
                };
                assert!(cone > 0.05 && cone < 0.95, "{cone}");
            }
            vis * att * cone * l.colour[k] * n.dot(ld).clamp(0.0, 1.0)
        };
        let wants: [[f32; 3]; 4] = [
            std::array::from_fn(|k| lit(k, 0.0)),
            std::array::from_fn(|k| lit(k, vis * sun_colour[k] * n.dot(sun_dir).clamp(0.0, 1.0))),
            std::array::from_fn(|k| lit(k, source(&omni, k))),
            std::array::from_fn(|k| lit(k, source(&spot, k))),
        ];
        for (bind, want) in binds.iter().zip(wants) {
            let px = centre_texel(&device, &queue, |pass| {
                pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, bind));
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
                pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
                pass.draw(0..3, 0..1);
            });
            eprintln!("got {px:?}, want {want:?}");
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 3.0,
                    "{px:?} vs {want:?}"
                );
            }
        }
        // four groups: no light, the sun, the omni and the spot
        let mut groups: Vec<u32> = binds.iter().map(|b| b.lightmap).collect();
        groups.dedup();
        assert_eq!(groups.len(), 4);
    }

    /// A CoD4 static model's `mc_l_sm_r0c0n0s0` and `mc_ambient_t0c0`
    /// draws through `fs_cod4_model` against `lp_sun_r0c0n0s0_sm3.hlsl` and
    /// `lp_amb_sun_t0c0_sm3.hlsl` worked by hand (cod4-world.md 3.2): the
    /// model lighting volume read trilinearly at the block centre plus
    /// 1.5 texels times the normal mapped normal over its longest axis,
    /// twice its colour and its alpha on the sun's `N.L` and specular, the
    /// reflection probe's face along the reflected view times its alpha
    /// added to the specular; the ambient set at the centre, the sun
    /// without `N.L`; batches on an omni and a spot light (3.5) take the
    /// light toward its origin, its attenuation row and the spot's cone in
    /// the sun's place, and the ambient set keeps the sun. The block is
    /// model 1's, its neighbours white, so a lookup that bleeds shows.
    /// Vulkan or Metal only.
    #[test]
    fn cod4_model_shades_its_volume_block_as_the_hlsl_does() {
        use vcod_common::mapworld::{MapSun, ModelLightVolume};
        use vcod_common::material::{Cod4Light, Cod4Lit};
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        // a 256 x 4 slice, blocks 0 and 2 white, block 1 every texel apart
        let (w, h) = (256usize, 4usize);
        let mut rgba = vec![255u8; w * h * 4 * 4];
        let texel = |x: usize, y: usize, z: usize| -> [u8; 4] {
            [
                (20 + 30 * x + 9 * y + 3 * z) as u8,
                (200 - 25 * y - 7 * z - 2 * x) as u8,
                (40 + 35 * z + 5 * x + 11 * y) as u8,
                (60 + 40 * x + 10 * z) as u8,
            ]
        };
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    let at = 4 * ((z * h + y) * w + 4 + x);
                    rgba[at..at + 4].copy_from_slice(&texel(x, y, z));
                }
            }
        }
        let sun_dir = glam::Vec3::new(0.3, -0.5, 0.8).normalize();
        let sun_colour = [0.7f32, 0.6, 0.5];
        // the probe: each face one colour on every level
        let face_colour = |f: usize| [30 + 8 * f as u8, 60 - 5 * f as u8, 20 + 6 * f as u8, 128];
        let probe = vcod_common::mapworld::ProbeCube {
            size: 4,
            faces: (0..6)
                .map(|f| {
                    (0..3)
                        .map(|l| face_colour(f).repeat(16 >> (2 * l)))
                        .collect()
                })
                .collect(),
        };
        // an omni and a spot light near the centre texel (cod4-world.md 3.5)
        let p = glam::Vec3::new(0.125, -0.125, 0.5);
        let row: Vec<[u8; 4]> = (0..32u8)
            .map(|i| [250 - 6 * i, 240 - 4 * i, 230 - 7 * i, 255])
            .collect();
        let origin = p + glam::Vec3::new(20.0, -30.0, 25.0);
        let (outer, inner) = (0.9f32, 0.99f32);
        let scale = 1.0 / (inner - outer);
        let omni = vcod_common::mapworld::ModelLightSource {
            spot: false,
            can_shadow: true,
            origin: origin.to_array(),
            inv_radius: 1.0 / 100.0,
            colour: [0.8, 0.6, 0.4],
            dir: (origin - p - glam::Vec3::new(15.0, 0.0, 0.0))
                .normalize()
                .to_array(),
            spot_factors: [scale, -scale * outer, 2.0],
            attenuation: row.clone(),
        };
        let spot = vcod_common::mapworld::ModelLightSource {
            spot: true,
            can_shadow: true,
            ..omni.clone()
        };
        let bsp = MapWorld {
            model_lights: vec![None, None, Some(omni.clone()), Some(spot.clone())],
            reflection_probes: vec![probe],
            sun: Some(MapSun {
                dir: sun_dir.to_array(),
                colour: sun_colour,
            }),
            model_lighting: Some(ModelLightVolume {
                width: w as u32,
                height: h as u32,
                rgba,
            }),
            ..Default::default()
        };
        let (color, normal, spec) = (
            [153u8, 102, 51, 255],
            // x in alpha, y in green
            [128u8, 112, 128, 150],
            [200u8, 180, 160, 170],
        );
        let mut fs = Pk3Fs::empty();
        for (name, rgba) in [("c", color), ("n", normal), ("s", spec)] {
            fs.overlay(&format!("images/{name}.iwi"), flat_iwi(rgba));
        }
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        assert!(b.has_model_volume());
        let bind_tex = |semantic, name: &str| vcod_common::material::TextureBinding {
            semantic,
            image: ImageRef::Path(format!("images/{name}.iwi")),
        };
        let env = [0.25, 0.9, 3.0, 1.5];
        let material = |name: &str, light, normal_map, specular| Material {
            name: name.into(),
            phong: Some(PhongLit {
                specular,
                detail: false,
                alpha: false,
                detail_scale: [1.0, 1.0],
                cod4: Some(Cod4Lit {
                    normal_map,
                    env,
                    light,
                    water_color: [0.0; 4],
                    flag: false,
                    sun_shadow: false,
                }),
            }),
            textures: vec![
                bind_tex(TexSemantic::Color, "c"),
                bind_tex(TexSemantic::Normal, "n"),
                bind_tex(TexSemantic::Specular, "s"),
            ],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let lit = material("lit", Cod4Light::Grid, true, true);
        let amb = material("amb", Cod4Light::GridAmbient, false, false);
        assert!(draws_cod4_model(&lit, StageVariant::Opaque));
        assert!(!draws_lit(&lit, 0, 1, StageVariant::Opaque));
        let bl = b.bind_cod4_model(
            &fs,
            &mut views,
            &lit,
            (0, 1),
            StageVariant::Opaque,
            true,
            false,
        );
        let ba = b.bind_cod4_model(
            &fs,
            &mut views,
            &amb,
            (0, 1),
            StageVariant::Opaque,
            true,
            false,
        );
        let bo = b.bind_cod4_model(
            &fs,
            &mut views,
            &lit,
            (0, 2),
            StageVariant::Opaque,
            true,
            false,
        );
        let bs = b.bind_cod4_model(
            &fs,
            &mut views,
            &lit,
            (0, 3),
            StageVariant::Opaque,
            true,
            false,
        );
        // the ambient set keeps the sun whatever its batch's light
        let bao = b.bind_cod4_model(
            &fs,
            &mut views,
            &amb,
            (0, 2),
            StageVariant::Opaque,
            true,
            false,
        );
        assert_eq!(bao.lightmap, ba.lightmap);
        assert_ne!(bo.lightmap, bl.lightmap);
        // index 0 is no primary light at all (technique 7), the sun's own
        // group not
        let bn0 = b.bind_cod4_model(
            &fs,
            &mut views,
            &lit,
            (0, 0),
            StageVariant::Opaque,
            true,
            false,
        );
        assert_ne!(bn0.lightmap, bl.lightmap);
        assert_eq!((bl.entry, bl.lightmap), (LitEntry::Cod4Model, ba.lightmap));
        let (t, bn) = (
            glam::Vec3::new(0.8660254, 0.5, 0.0),
            glam::Vec3::new(-0.5, 0.8660254, 0.0),
        );
        let world = b.finish(&[[t.to_array(), bn.to_array()]; 3]);
        let vcolor = [255u8, 204, 255, 255];
        let base = [6.0 / 256.0, 2.0 / 4.0];
        let verts: Vec<vcod_common::mapworld::DrawVert> =
            [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]]
                .map(|[x, y]| vcod_common::mapworld::DrawVert {
                    pos: [x, y, 0.5],
                    uv: [0.5, 0.5],
                    lm_uv: base,
                    normal: [0.0, 0.0, 1.0],
                    color: vcolor,
                })
                .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let unit = |b: u8| f32::from(b) / 255.0;
        let nt = glam::Vec2::new(
            unit(normal[3]) * 4.08 - 2.08,
            unit(normal[1]) * 4.064_516 - 2.064_516,
        );
        let n = (glam::Vec3::Z + nt.x * t + nt.y * bn).normalize();
        // the eye where the sun's reflection off the centre texel meets it
        let eye = p - 3.0 * (sun_dir - 2.0 * sun_dir.dot(n) * n).normalize();
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, eye, None),
        );
        let shade = |bind: LitBind| {
            centre_texel(&device, &queue, |pass| {
                pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
                pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
                pass.draw(0..3, 0..1);
            })
        };

        // the HLSL by hand: the volume's linear filter in texels, centres
        // at i + 0.5, the block from x 4
        let volume = |p: glam::Vec3| -> [f32; 4] {
            let at = |x: i32, y: i32, z: i32| texel((x - 4) as usize, y as usize, z as usize);
            let q = p - 0.5;
            let (x0, y0, z0) = (q.x.floor(), q.y.floor(), q.z.floor());
            let f = q - glam::Vec3::new(x0, y0, z0);
            std::array::from_fn(|c| {
                let mut sum = 0.0;
                for (dx, dy, dz) in (0..8).map(|i| (i & 1, (i >> 1) & 1, i >> 2)) {
                    let wt = [f.x, f.y, f.z]
                        .iter()
                        .zip([dx, dy, dz])
                        .map(|(&f, d)| if d == 1 { f } else { 1.0 - f })
                        .product::<f32>();
                    let v = at(x0 as i32 + dx, y0 as i32 + dy, z0 as i32 + dz)[c];
                    sum += wt * unit(v);
                }
                sum
            })
        };
        let centre = glam::Vec3::new(6.0, 2.0, 2.0);
        let diffuse: [f32; 3] = std::array::from_fn(|k| unit(color[k]) * unit(vcolor[k]));
        let ml = volume(centre + 1.5 * n / n.abs().max_element());
        let n_dot_l = n.dot(sun_dir).clamp(0.0, 1.0);
        let v = (p - eye).normalize();
        let vn = v.dot(n);
        let r = v - 2.0 * vn * n;
        let power = (unit(spec[3]) * 9.377_518).exp2() + 7.0;
        let glint = ((r.dot(sun_dir) - 0.99925) * power).exp().clamp(0.0, 1.0);
        let fresnel = env[0] + (env[1] - env[0]) * (1.0 - vn.abs()).powf(env[2]);
        // the cube's face along the reflection's longest axis, D3D's order
        let ar = r.abs();
        let face = if ar.x >= ar.y && ar.x >= ar.z {
            usize::from(r.x < 0.0)
        } else if ar.y >= ar.z {
            2 + usize::from(r.y < 0.0)
        } else {
            4 + usize::from(r.z < 0.0)
        };
        let pc = face_colour(face);
        let want: [f32; 3] = std::array::from_fn(|k| {
            let light = 2.0 * ml[k] + ml[3] * sun_colour[k] * n_dot_l;
            let reflected = ml[3] * glint * env[3] * sun_colour[k] + unit(pc[k]) * unit(pc[3]);
            let specular = reflected * unit(spec[k]) * fresnel;
            (diffuse[k] * light + specular) * 255.0
        });
        assert!(glint > 0.05, "{glint}");
        let px = shade(bl);
        eprintln!(
            "lit: got {px:?}, want {want:?} (volume {ml:?}, N.L {n_dot_l}, glint {glint}, face {face})"
        );
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 3.0,
                "{px:?} vs {want:?}"
            );
        }
        // the ambient set: the centre's eight inner texels, the sun whole
        let mc = volume(centre);
        let want: [f32; 3] = std::array::from_fn(|k| {
            (diffuse[k] * (2.0 * mc[k] + mc[3] * sun_colour[k]) * 255.0).min(255.0)
        });
        let px = shade(ba);
        eprintln!("ambient: got {px:?}, want {want:?} (volume {mc:?})");
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 3.0,
                "{px:?} vs {want:?}"
            );
        }
        // `lp_omni_*` and `lp_spot_*`: the light toward its origin, its row
        // at the distance over the radius, the spot's cone, in the sun's
        // place
        for (bind, l) in [(bo, &omni), (bs, &spot)] {
            let lv = glam::Vec3::from(l.origin) - p;
            let ld = lv.normalize();
            let x = ((lv.length() * l.inv_radius).clamp(0.0, 1.0) * 32.0 - 0.5).clamp(0.0, 31.0);
            let (i0, f) = (x.floor() as usize, x.fract());
            let i1 = (i0 + 1).min(31);
            let mut cone = 1.0;
            if l.spot {
                let s = (ld.dot(glam::Vec3::from(l.dir)) * l.spot_factors[0] + l.spot_factors[1])
                    .clamp(0.0, 1.0);
                cone = if s <= 0.0 {
                    0.0
                } else {
                    s.powf(l.spot_factors[2])
                };
                assert!(cone > 0.05 && cone < 0.95, "{cone}");
            }
            let n_dot_l = n.dot(ld).clamp(0.0, 1.0);
            let glint = ((r.dot(ld) - 0.99925) * power).exp().clamp(0.0, 1.0);
            let want: [f32; 3] = std::array::from_fn(|k| {
                let att = unit(row[i0][k]) * (1.0 - f) + unit(row[i1][k]) * f;
                let col = att * cone * l.colour[k];
                let light = 2.0 * ml[k] + ml[3] * col * n_dot_l;
                let reflected = ml[3] * glint * env[3] * col + unit(pc[k]) * unit(pc[3]);
                ((diffuse[k] * light + reflected * unit(spec[k]) * fresnel) * 255.0).min(255.0)
            });
            let px = shade(bind);
            eprintln!("spot {}: got {px:?}, want {want:?}", l.spot);
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 3.0,
                    "{px:?} vs {want:?}"
                );
            }
        }
    }

    /// A CoD4 `wc_water` draw through `fs_cod4_water` against
    /// `water_l_sun.hlsl` worked by hand (cod4-world.md 9.8): the normal
    /// map's red, a 4 x 4 of different heights, moves the coords along the
    /// view and gives three octaves of height whose differences 1/256 apart
    /// tilt the normal; the probe's face along the reflection (its z made
    /// positive) lerps from `waterColor` by the fresnel, the glint adds the
    /// sun (next to nothing: the fresnel to the 64th); the vertex alpha.
    /// Vulkan or Metal only.
    #[test]
    fn cod4_water_shades_as_water_l_sun() {
        use vcod_common::mapworld::{MapSun, ProbeCube};
        use vcod_common::material::{Cod4Light, Cod4Lit};
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let face_colour = |f: usize| [40 + 20 * f as u8, 90 - 9 * f as u8, 30 + 15 * f as u8, 150];
        let sun_dir = glam::Vec3::new(0.2, 0.3, 0.9).normalize();
        let sun_colour = [0.8f32, 0.7, 0.6];
        let bsp = MapWorld {
            reflection_probes: vec![ProbeCube {
                size: 2,
                faces: (0..6)
                    .map(|f| {
                        (0..2)
                            .map(|l| face_colour(f).repeat(4 >> (2 * l)))
                            .collect()
                    })
                    .collect(),
            }],
            sun: Some(MapSun {
                dir: sun_dir.to_array(),
                colour: sun_colour,
            }),
            // a second material's stored spectrum: one wave on a 4 x 4
            waters: vec![(
                "sim".into(),
                vcod_common::mapworld::WaterSpectrum {
                    width: 4,
                    h0: (0..16)
                        .map(|i| if i == 5 { [9.0, 5.0] } else { [0.0; 2] })
                        .collect(),
                    omega: (0..16).map(|i| if i == 5 { 3.3 } else { 0.0 }).collect(),
                },
            )],
            ..Default::default()
        };
        // red heights over a 4 x 4, B G R A as the iwi stores them
        let red = |x: usize, y: usize| (30 + 50 * x + 13 * y + 7 * (x * y)) as u8;
        let mut iwi = b"IWi".to_vec();
        iwi.extend([5, 1, 0x3]);
        iwi.extend([4u16, 4, 1].iter().flat_map(|v| v.to_le_bytes()));
        iwi.extend([0u8; 16]);
        for y in 0..4 {
            for x in 0..4 {
                iwi.extend([128, 128, red(x, y), 255]);
            }
        }
        let mut fs = Pk3Fs::empty();
        fs.overlay("images/w.iwi", iwi);
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let env = [0.2, 0.8, 2.0, 2.5];
        let water_color = [0.4, 0.35, 0.3, 1.0];
        let sh = Material {
            phong: Some(PhongLit {
                specular: true,
                detail: false,
                alpha: true,
                detail_scale: [1.0, 1.0],
                cod4: Some(Cod4Lit {
                    normal_map: true,
                    env,
                    light: Cod4Light::Water,
                    water_color,
                    flag: false,
                    sun_shadow: false,
                }),
            }),
            textures: vec![vcod_common::material::TextureBinding {
                semantic: TexSemantic::Normal,
                image: ImageRef::Path("images/w.iwi".into()),
            }],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        assert!(draws_cod4_water(&sh, StageVariant::Opaque));
        assert!(!draws_cod4_model(&sh, StageVariant::Opaque));
        assert!(!draws_lit(&sh, 0, 1, StageVariant::Opaque));
        let bind = b.bind_cod4_water(&fs, &mut views, &sh, 0, StageVariant::Opaque, true, false);
        let sim = Material {
            name: "sim".into(),
            ..sh.clone()
        };
        let sim_bind =
            b.bind_cod4_water(&fs, &mut views, &sim, 0, StageVariant::Opaque, true, false);
        assert_ne!(sim_bind.material, bind.material);
        let mut world = b.finish(&tangent_stream(&bsp, 3));
        // the spectrum's height map is made and runs on the view's time
        assert_eq!(world.cod4_waters.len(), 1);
        world.cod4_waters[0].update(&queue, 2.5);
        let (uv, alpha) = ([0.3f32, 0.55], 200u8);
        let verts: Vec<vcod_common::mapworld::DrawVert> =
            [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]]
                .map(|[x, y]| vcod_common::mapworld::DrawVert {
                    pos: [x, y, 0.5],
                    uv,
                    lm_uv: [0.0; 2],
                    normal: [0.0, 0.0, 1.0],
                    color: [255, 255, 255, alpha],
                })
                .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let eye = glam::Vec3::new(-0.6, 0.9, 2.5);
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, eye, None),
        );
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });

        // the HLSL by hand: the test sampler clamps to the edge
        let tex: Vec<u8> = (0..16).flat_map(|i| [red(i % 4, i / 4), 0, 0, 0]).collect();
        let height = |u: f32, v: f32| bilinear(&tex, (4, 4), 4, 0, u, v);
        let octaves = |u: f32, v: f32| {
            (0..3)
                .map(|i| height(u * 3.7f32.powi(i), v * 3.7f32.powi(i)) * 0.6f32.powi(i))
                .sum::<f32>()
        };
        let unit = |b: u8| f32::from(b) / 255.0;
        let v = (glam::Vec3::new(0.125, -0.125, 0.5) - eye).normalize();
        let lift = 0.5 - height(0.5 * uv[0], 0.5 * uv[1]);
        let (u0, v0) = (
            uv[0] + lift * v.x * 0.0234375,
            uv[1] + lift * v.y * 0.0234375,
        );
        let h0 = octaves(u0, v0);
        let n = glam::Vec3::new(
            octaves(u0 + 1.0 / 256.0, v0) - h0,
            octaves(u0, v0 + 1.0 / 256.0) - h0,
            1.0,
        )
        .normalize();
        let vn = v.dot(n);
        let r = v - 2.0 * vn * n;
        let q = glam::Vec3::new(r.x, r.y, r.z.abs());
        let aq = q.abs();
        let face = if aq.x >= aq.y && aq.x >= aq.z {
            usize::from(q.x < 0.0)
        } else if aq.y >= aq.z {
            2 + usize::from(q.y < 0.0)
        } else {
            4
        };
        let pc = face_colour(face);
        let fresnel = (env[0] + (env[1] - env[0]) * (1.0 - vn.abs()).powf(env[2])).clamp(0.0, 1.0);
        let glint = (fresnel * (r.dot(sun_dir) + 0.00075).max(0.0)).powf(64.0);
        let want: [f32; 3] = std::array::from_fn(|k| {
            let reflected = (unit(pc[k]) * unit(pc[3]) * 4.0).min(1.0);
            let base = water_color[k] * n.z;
            let rgb = base + (reflected - base) * fresnel + glint * sun_colour[k] * env[3];
            rgb * 255.0
        });
        eprintln!(
            "got {px:?}, want {want:?} (normal {n}, face {face}, fresnel {fresnel}, glint {glint})"
        );
        assert!(n.x > 1e-3, "the heights tilt the normal: {n}");
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 3.0,
                "{px:?} vs {want:?}"
            );
        }
        assert_eq!(px[3], alpha);
    }

    /// A static prop's `lprobe_smc_s` draw through `fs_smodel` against
    /// `lib/lightprobe_sm.hlsl` worked by hand: the normal mapped normal
    /// turned into the probe basis and pushed onto its block's face weights
    /// the eight cached corners linearly, the texel's alpha is the sun's
    /// visibility on the diffuse and (through `lerp(0.3, 1, vis)`) the
    /// specular. The block is model 1's, its neighbours bright, so a lookup
    /// that bleeds out of it shows (cod2-materials.md 9.6). Vulkan or Metal
    /// only.
    #[test]
    fn static_model_shades_its_cached_block_as_the_hlsl_does() {
        use crate::cod2_light::SmodelCache;
        use vcod_cod2::lightgrid::{self, GridSample};
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let camera = camera_layout(&device);
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: [[0u8; 4]; 3].map(|c| c.repeat(512 * 512)),
                sun: vec![0; 1024 * 1024],
            }],
            entities: "{\n\"classname\" \"worldspawn\"\n\"sundirection\" \"-45 0 0\"\n\"sunlight\" \"0.8\"\n\"suncolor\" \"1 1 1\"\n}\n".into(),
            ..Default::default()
        };
        // eight different corners per channel, the sun 60 percent visible
        let lit = GridSample {
            rgb: [
                [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
                [0.8, 0.1, 0.6, 0.2, 0.4, 0.3, 0.5, 0.7],
                [0.3, 0.3, 0.9, 0.1, 0.2, 0.6, 0.4, 0.5],
            ],
            sun: 0.6,
        };
        let bright = GridSample {
            rgb: [[1.0; 8]; 3],
            sun: 1.0,
        };
        let cache = SmodelCache::of(&[Some(bright), Some(lit), Some(bright)]);
        let (color, normal, spec) = (
            [153u8, 102, 51, 255],
            // x 0.75 in alpha, y 0.375 in green
            [128u8, 96, 128, 191],
            [128u8, 160, 96, 128],
        );
        let mut fs = Pk3Fs::empty();
        for (name, rgba) in [("c", color), ("n", normal), ("s", spec)] {
            fs.overlay(&format!("images/{name}.iwi"), flat_iwi(rgba));
        }
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        b.set_smodel_cache(&cache);
        let bind_tex = |semantic, name: &str| vcod_common::material::TextureBinding {
            semantic,
            image: ImageRef::Path(format!("images/{name}.iwi")),
        };
        let sm = vcod_common::material::ProbeLit {
            specular: true,
            alpha: false,
            detail: false,
            scroll: false,
            flag: false,
            detail_scale: [1.0, 1.0],
        };
        let sh = Material {
            model_light: vcod_common::material::ModelLight::Probe,
            smodel: Some(sm),
            textures: vec![
                bind_tex(TexSemantic::Color, "c"),
                bind_tex(TexSemantic::Normal, "n"),
                bind_tex(TexSemantic::Specular, "s"),
            ],
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        assert!(draws_smodel(&sh, StageVariant::Opaque));
        assert!(!draws_smodel(&sh, StageVariant::AdditiveFixed));
        let bind = b.bind_smodel(&fs, &mut views, &sh, StageVariant::Opaque, false);
        assert_eq!(bind.entry, LitEntry::Smodel);
        // a prop's frame: normal up, tangent along +x turned 30 degrees
        // about z, binormal across it
        let (t, bn) = (
            glam::Vec3::new(0.8660254, 0.5, 0.0),
            glam::Vec3::new(-0.5, 0.8660254, 0.0),
        );
        let world = b.finish(&[[t.to_array(), bn.to_array()]; 3]);
        let vcolor = [255u8, 204, 255, 255];
        let corners = [[-1.0f32, -1.0], [3.0, -1.0], [-1.0, 3.0]];
        let base = cache.centre(1);
        let verts: Vec<vcod_common::mapworld::DrawVert> = corners
            .map(|[x, y]| vcod_common::mapworld::DrawVert {
                pos: [x, y, 0.5],
                uv: [0.5, 0.5],
                lm_uv: base,
                normal: [0.0, 0.0, 1.0],
                color: vcolor,
            })
            .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let eye = glam::Vec3::new(0.0, 0.0, 10.0);
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, eye, None),
        );
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(StageVariant::Opaque, true, false, &bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });

        // the HLSL by hand: the cache holds bytes, the lookup weights its
        // corners linearly
        let unit = |b: u8| f32::from(b) / 255.0;
        let diffuse: [f32; 3] = std::array::from_fn(|k| unit(color[k]) * unit(vcolor[k]));
        let (nx, ny) = (unit(normal[3]), unit(normal[1]));
        let up = glam::Vec3::Z;
        let n = (up + (2.0 * nx - 1.0) * t + (2.0 * ny - 1.0) * bn).normalize();
        let w = lightgrid::static_weights(n);
        let texel = |j: usize| crate::cod2_light::smodel_texel(&lit, j);
        let probe: [f32; 3] =
            std::array::from_fn(|k| (0..8).map(|j| unit(texel(j)[k]) * w[j]).sum::<f32>());
        let vis = unit(texel(0)[3]);
        let sun = glam::Vec3::new(1.0, 0.0, 1.0).normalize();
        let sun_colour = 0.4;
        let n_dot_l = n.dot(sun).max(0.0);
        let half = |p: glam::Vec3| ((eye - p).normalize() + sun).normalize();
        let [h0, h1, h2] = corners.map(|[x, y]| half(glam::Vec3::new(x, y, 0.5)));
        let (cx, cy) = (0.125f32, -0.125f32);
        let (b1, b2) = ((cx + 1.0) / 4.0, (cy + 1.0) / 4.0);
        let h = (h0 * (1.0 - b1 - b2) + h1 * b1 + h2 * b2).normalize();
        let strength = bilinear(&specularity_map(), (32, 256), 1, 0, unit(spec[3]), h.dot(n));
        let spec_vis = 0.3 + 0.7 * vis;
        let want: [f32; 3] = std::array::from_fn(|k| {
            let rgb = diffuse[k] * (probe[k] + vis * sun_colour * n_dot_l)
                + spec_vis * sun_colour * unit(spec[k]) * strength;
            rgb * 255.0
        });
        eprintln!("got {px:?}, want {want:?} (weights {w:?}, N.L {n_dot_l}, strength {strength})");
        // the tilt spreads the weight over several corners
        assert!(w.iter().filter(|&&x| x > 0.05).count() >= 3, "{w:?}");
        for k in 0..3 {
            assert!(
                (f32::from(px[k]) - want[k]).abs() <= 2.0,
                "{px:?} vs {want:?}"
            );
        }
    }

    /// The cube's layers are D3D's faces and the sky samples them along
    /// the world direction: looking down +x reads face 0, +y face 2, -z
    /// face 5. Vulkan or Metal only.
    #[test]
    fn sky_cube_reads_the_face_the_view_looks_at() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let faces: Vec<Image> = (0..6u8)
            .map(|f| Image {
                width: 4,
                height: 4,
                data: ImageData::Rgba8([f * 40, 0, 0, 255].repeat(16)),
            })
            .collect();
        let view = upload_cube(&device, &queue, "test cube", &faces).unwrap();
        let camera = camera_layout(&device);
        let tex = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let eye = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let pipeline = make_sky_pipeline(&device, super::super::SCENE_FORMAT, &camera, &tex, &eye);
        let cube_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &tex,
            entries: &[tex_entry(0, &view)],
        });
        let eye_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0.0f32; 4]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let eye_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &eye,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: eye_buf.as_entire_binding(),
            }],
        });
        let size = sky::box_size(Z_FAR, Z_NEAR);
        let mut verts = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for axis in 0..6 {
            let side = sky::box_side(axis, size);
            indices.extend(side.indices.iter().map(|&i| i + verts.len() as u32));
            verts.extend(side.verts);
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // yaw and pitch in degrees, `camera::basis`'s (a positive pitch
        // looks up)
        for (yaw, pitch, face) in [
            (0.0, 0.0, 0u8),
            (180.0, 0.0, 1),
            (90.0, 0.0, 2),
            (-90.0, 0.0, 3),
            (0.0, 89.0, 4),
            (0.0, -89.0, 5),
        ] {
            let (y, p) = (f32::to_radians(yaw), f32::to_radians(pitch));
            let vp = crate::camera::view_proj_from(glam::Vec3::ZERO, y, p, 0.0, 90.0, 1.0);
            let cam = camera_group(
                &device,
                &queue,
                &camera,
                &camera_bytes(vp, glam::Vec3::ZERO, None),
            );
            let px = centre_texel(&device, &queue, |pass| {
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, &cube_bg, &[]);
                pass.set_bind_group(2, &eye_bg, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
            });
            assert_eq!(px[0], face * 40, "yaw {yaw} pitch {pitch}: {px:?}");
        }
    }

    /// On mp_toujane most lightmapped world triangles take the lit path,
    /// and the sky material names a cube that loads. Needs `COD2_DIR`.
    #[test]
    fn toujane_draws_lit_and_has_a_cube_sky() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        assert_eq!(bsp.lightmap_basis.len(), bsp.lightmaps.len());
        let (mut lit, mut lightmapped) = (0usize, 0usize);
        let mut sky = None;
        for s in &bsp.soups {
            let name = &bsp.materials[s.material as usize].name;
            let Some(sh) = lib.get(name) else { continue };
            if sh.sky.is_some() {
                sky = sh.sky.clone();
            }
            if s.lightmap == NO_LIGHTMAP || sh.stages.is_empty() {
                continue;
            }
            lightmapped += usize::from(s.index_count);
            let v = super::super::stage_draw_facts(sh, 0).unwrap().0;
            if draws_lit(sh, s.lightmap, bsp.lightmap_basis.len(), v) {
                lit += usize::from(s.index_count);
            }
        }
        assert!(lit * 10 > lightmapped * 7, "{lit} of {lightmapped}");
        let sky = sky.expect("toujane has a sky");
        let faces = load_cube_faces(&fs, &sky.env).expect("the cube loads");
        assert_eq!(faces.len(), 6);
        assert!(faces.iter().all(|f| f.width == faces[0].width));
    }

    /// The stage pipeline table is indexed by discriminant.
    #[test]
    fn stage_variant_order_is_the_discriminant_order() {
        for (i, v) in super::super::STAGE_VARIANT_ORDER.iter().enumerate() {
            assert_eq!(*v as usize, i, "{v:?}");
        }
    }

    /// A world material as `vcod_cod2::materials::compile` builds it for
    /// `technique` under the pass bits `bits` and `sort_key`, colour map
    /// white (no textures).
    fn compiled(technique: &str, bits: [u32; 2], sort_key: u8) -> Material {
        use vcod_cod2::material::{MaterialFile, TechniqueType};
        use vcod_cod2::materials::{LitPass, compile};
        let file = MaterialFile {
            name: technique.into(),
            ref_image_name: String::new(),
            hash_index: 0,
            sorted_index: 0,
            game_flags: 3,
            sort_key,
            atlas_rows: 1,
            atlas_columns: 1,
            max_deform_move: 0.0,
            deform_flags: 0,
            usage: 0,
            tool_flags: 0,
            locale: 0,
            auto_tex_scale: [0; 2],
            tess_size: 0.0,
            surface_flags: 0,
            contents: 1,
            state_bits: bits,
            technique_set: String::new(),
            textures: Vec::new(),
            constants: Vec::new(),
        };
        let ty = if technique.starts_with("lmap") {
            TechniqueType::LightmapNoFog
        } else {
            TechniqueType::EmissiveNoFog
        };
        let pass = LitPass {
            ty,
            technique: technique.into(),
            bits,
            fogs: bits[0] & vcod_cod2::materials::FOG_BIT != 0,
        };
        compile(&file, Some(&pass), &mut Default::default()).0
    }

    /// The lit and stage draws of CoD2's layered world: a `phong_blend`
    /// layer (`lmap_a`, SrcAlpha/InvSrcAlpha after the `nofog` statemap)
    /// over a lit base weighs the two by the vertex alpha, and an
    /// `effect_multiply` decal (`vertcol_mul`, Zero/SrcColor) multiplies a
    /// grey wall by its colour faded to white by the vertex alpha
    /// (mul.hlsl). Vulkan or Metal only; under lavapipe set
    /// `GALLIVM_PERF=nopt`, or llvmpipe spends minutes compiling shader.wgsl.
    #[test]
    fn blend_layers_weigh_by_vertex_alpha_and_decals_multiply() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera = camera_layout(&device);
        // every coefficient 200 and no sun: a white surface lights to 200
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: std::array::from_fn(|_| vec![200; 512 * 512 * 4]),
                sun: vec![0; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            ..Default::default()
        };
        let fs = Pk3Fs::empty();
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        // stock words after `nofog` (cod2-materials.md 3, 6)
        let base = compiled("lmap", [0x0812_8812, 0x0000_0005], 3);
        let layer = compiled("lmap_a", [0x0812_9965, 0x0000_0014], 8);
        let decal = compiled("vertcol_mul", [0x0812_8931, 0x0000_0014], 11);
        let facts = |sh: &Material| super::super::stage_draw_facts(sh, 0).unwrap();
        assert_eq!(facts(&base).0, StageVariant::Opaque);
        assert_eq!(facts(&layer).0, StageVariant::Blend);
        assert!(layer.phong.is_some_and(|p| p.alpha));
        assert_eq!(facts(&decal).0, StageVariant::Multiply);
        assert!(base.sort < layer.sort && layer.sort < decal.sort);

        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let base_bind = b.bind(
            &fs,
            &mut views,
            &base,
            0,
            StageVariant::Opaque,
            false,
            false,
        );
        let (v, two, bias) = facts(&layer);
        assert!(draws_lit(&layer, 0, b.page_count(), v));
        let layer_bind = b.bind(&fs, &mut views, &layer, 0, v, two, bias);
        let lit_layer_key = (v, two, bias);
        let world = b.finish(&tangent_stream(&bsp, 3));

        // the decal through the renderer's own stage pipeline and groups
        let material_layout = super::super::material_layout(&device);
        let stage_layout = super::super::stage_layout(&device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(super::super::WORLD_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera), Some(&material_layout), Some(&stage_layout)],
            immediate_size: 0,
        });
        let (dv, dtwo, dbias) = facts(&decal);
        let (fs_entry, blend, depth_write) = super::super::stage_variant_state(dv);
        let decal_pipeline = super::super::scene_pipeline(
            &device,
            &shader,
            super::super::SCENE_FORMAT,
            "test multiply",
            &layout,
            ("vs_stage", fs_entry),
            if dbias {
                depth_bias(decal.depth_bias.unwrap())
            } else {
                Default::default()
            },
            false,
            blend,
            depth_write,
            (!dtwo).then_some(wgpu::Face::Back),
        );
        let params = super::super::stage_params(&decal, 0, 0.0).unwrap();
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let white = super::super::upload_rgba(&device, &queue, "white", 1, 1, &[255; 4]);
        let page = crate::lightmap_blocks::LightmapPage {
            view: white.clone(),
            edges: crate::lightmap_blocks::upload_edges(&device, &queue, "edges", 1, &[[0; 4]]),
        };
        let lm_sampler = device.create_sampler(&Default::default());
        let (mat_bg, stage_bg) = super::super::stage_bind_groups(
            &device,
            &queue,
            &fs,
            &material_layout,
            &stage_layout,
            &params_buf,
            super::super::STAGE_PARAMS_SIZE,
            &white,
            &page,
            &lm_sampler,
            &mut views,
            &decal.stages[0],
        );

        let tri = |rgba: [u8; 4]| {
            let verts: Vec<vcod_common::mapworld::DrawVert> =
                // clockwise on screen: the map's front face
                [[-1.0, -1.0], [-1.0, 3.0], [3.0, -1.0]]
                    .map(|[x, y]| vcod_common::mapworld::DrawVert {
                        pos: [x, y, 0.5],
                        uv: [0.5, 0.5],
                        lm_uv: [0.5, 0.5],
                        normal: [0.0, 0.0, 1.0],
                        color: rgba,
                    })
                    .to_vec();
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, glam::Vec3::new(0.0, 0.0, 10.0), None),
        );
        let lit_draw = |pass: &mut wgpu::RenderPass<'_>,
                        key: (StageVariant, bool, bool),
                        bind: LitBind,
                        vbuf: &wgpu::Buffer| {
            pass.set_pipeline(world.pipeline(key.0, key.1, key.2, &bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        };
        let close = |px: [u8; 4], want: [f32; 3]| {
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 3.0,
                    "{px:?} vs {want:?}"
                );
            }
        };
        let red = tri([255, 0, 0, 255]);
        let base_key = (StageVariant::Opaque, false, false);
        // the layer's own colour is green; its vertex alpha is its weight
        for (a, want) in [
            (0u8, [200.0, 0.0, 0.0]),
            (255, [0.0, 200.0, 0.0]),
            (102, [120.0, 80.0, 0.0]),
        ] {
            let green = tri([0, 255, 0, a]);
            let px = centre_texel(&device, &queue, |pass| {
                lit_draw(pass, base_key, base_bind, &red);
                lit_draw(pass, lit_layer_key, layer_bind, &green);
            });
            close(px, want);
        }
        // a grey 200 wall times the decal's (0.5, 0.25, 1) faded by its alpha
        let grey = tri([255; 4]);
        for (a, want) in [
            (0u8, [200.0, 200.0, 200.0]),
            (255, [100.4, 50.2, 200.0]),
            (128, [150.0, 125.1, 200.0]),
        ] {
            let stain = tri([128, 64, 255, a]);
            let px = centre_texel(&device, &queue, |pass| {
                lit_draw(pass, base_key, base_bind, &grey);
                pass.set_pipeline(&decal_pipeline);
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, mat_bg.bind_group(0.0), &[]);
                pass.set_bind_group(2, stage_bg.bind_group(0.0), &[0]);
                pass.set_vertex_buffer(0, stain.slice(..));
                pass.draw(0..3, 0..1);
            });
            close(px, want);
        }
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
    }

    /// mp_carentan's `setExpFog(0.0001, 0.55, 0.6, 0.55)` on a surface 2000
    /// units out: exp(-0.2) = 82% of its colour stays and the rest is the
    /// fog colour, through the lit pipeline and the stage path alike; a
    /// material whose fog technique clears the fog bit stays clear. The
    /// view forward is zero, so only the radial distance fogs. Vulkan or
    /// Metal only; under lavapipe set `GALLIVM_PERF=nopt`.
    #[test]
    fn carentan_fog_keeps_82_percent_at_2000_units() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera = camera_layout(&device);
        let bsp = MapWorld {
            lightmap_basis: vec![LightmapBasis {
                coeffs: std::array::from_fn(|_| vec![200; 512 * 512 * 4]),
                sun: vec![0; 1024 * 1024],
            }],
            tangents: vec![[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]; 3],
            ..Default::default()
        };
        let fs = Pk3Fs::empty();
        let mut views = BundleViews {
            views: HashMap::new(),
            table: None,
            picmip: 0,
            mipmap: true,
            store: Default::default(),
            prefetched: None,
        };
        // stock words after `fogged` with the material's fog bit, and
        // after `nofog` (cod2-materials.md 3, 6)
        let lit = compiled("lmap", [0x2812_8812, 5], 3);
        let stage = compiled("vertcol_simple", [0x2812_8812, 5], 3);
        let clear = compiled("vertcol_simple", [0x0812_8812, 5], 3);
        assert!(!lit.nofog && !stage.nofog && clear.nofog);

        let mut b = Cod2Builder::new(&device, &queue, super::super::SCENE_FORMAT, &camera, &bsp);
        let key = (StageVariant::Opaque, false, false);
        let lit_bind = b.bind(&fs, &mut views, &lit, 0, key.0, key.1, key.2);
        let world = b.finish(&tangent_stream(&bsp, 3));

        let material_layout = super::super::material_layout(&device);
        let stage_layout = super::super::stage_layout(&device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(super::super::WORLD_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera), Some(&material_layout), Some(&stage_layout)],
            immediate_size: 0,
        });
        let (fs_entry, blend, depth_write) =
            super::super::stage_variant_state(super::super::stage_draw_facts(&stage, 0).unwrap().0);
        let pipeline = super::super::scene_pipeline(
            &device,
            &shader,
            super::super::SCENE_FORMAT,
            "test fog",
            &layout,
            ("vs_stage", fs_entry),
            Default::default(),
            false,
            blend,
            depth_write,
            None,
        );
        let white = super::super::upload_rgba(&device, &queue, "white", 1, 1, &[255; 4]);
        let page = crate::lightmap_blocks::LightmapPage {
            view: white.clone(),
            edges: crate::lightmap_blocks::upload_edges(&device, &queue, "edges", 1, &[[0; 4]]),
        };
        let lm_sampler = device.create_sampler(&Default::default());
        let groups = |sh: &Material, views: &mut BundleViews| {
            let params = super::super::stage_params(sh, 0, 0.0).unwrap();
            // the binding spans a whole slot
            let mut contents = bytemuck::bytes_of(&params).to_vec();
            contents.resize(super::super::STAGE_PARAMS_SIZE as usize, 0);
            let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &contents,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            super::super::stage_bind_groups(
                &device,
                &queue,
                &fs,
                &material_layout,
                &stage_layout,
                &params_buf,
                super::super::STAGE_PARAMS_SIZE,
                &white,
                &page,
                &lm_sampler,
                views,
                &sh.stages[0],
            )
        };
        let fogged_groups = groups(&stage, &mut views);
        let clear_groups = groups(&clear, &mut views);

        let tri = |rgba: [u8; 4]| {
            // clockwise on screen: the map's front face
            let verts: Vec<vcod_common::mapworld::DrawVert> =
                [[-1.0, -1.0], [-1.0, 3.0], [3.0, -1.0]]
                    .map(|[x, y]| vcod_common::mapworld::DrawVert {
                        pos: [x, y, 0.5],
                        uv: [0.5, 0.5],
                        lm_uv: [0.5, 0.5],
                        normal: [0.0, 0.0, 1.0],
                        color: rgba,
                    })
                    .to_vec();
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        // the centre texel sits at (0, 0, 0.5), 2000 from the eye
        let fog = Some(([0.55, 0.6, 0.55], 0.0001));
        let cam = camera_group(
            &device,
            &queue,
            &camera,
            &camera_bytes(glam::Mat4::IDENTITY, glam::Vec3::new(0.0, 0.0, 2000.5), fog),
        );
        let keep = (-0.2f32).exp();
        assert!((keep - 0.8187).abs() < 1e-4);
        let fogged = |c: [f32; 3]| {
            [0, 1, 2].map(|k| c[k] * keep + [0.55, 0.6, 0.55][k] * 255.0 * (1.0 - keep))
        };
        let close = |px: [u8; 4], want: [f32; 3]| {
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 3.0,
                    "{px:?} vs {want:?}"
                );
            }
        };
        // lit: white under every coefficient 200
        let white_tri = tri([255; 4]);
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(key.0, key.1, key.2, &lit_bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[lit_bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[lit_bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, white_tri.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });
        close(px, fogged([200.0; 3]));
        // stage: the vertex colour, exactVertex
        let col = tri([200, 100, 50, 255]);

        for (g, want) in [
            (&fogged_groups, fogged([200.0, 100.0, 50.0])),
            (&clear_groups, [200.0, 100.0, 50.0]),
        ] {
            let px = centre_texel(&device, &queue, |pass| {
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, g.0.bind_group(0.0), &[]);
                pass.set_bind_group(2, g.1.bind_group(0.0), &[0]);
                pass.set_vertex_buffer(0, col.slice(..));
                pass.draw(0..3, 0..1);
            });
            close(px, want);
        }
        // CoD4's mp_crash, `setExpFog(500, 3500, ...)`: the fog starts 500
        // out (fog_range.w), so at 2000 it keeps exp(-1500 ln 2 / 3500)
        // (cod4-world.md 11.1)
        let density = 0.000_198_042_f32;
        let mut crash = camera_bytes(
            glam::Mat4::IDENTITY,
            glam::Vec3::new(0.0, 0.0, 2000.5),
            Some(([0.5, 0.5, 0.45], density)),
        );
        crash[31 * 4..32 * 4].copy_from_slice(&500.0f32.to_le_bytes());
        let cam = camera_group(&device, &queue, &camera, &crash);
        let keep = (-density * 1500.0).exp();
        assert!((keep - 0.743).abs() < 1e-3);
        let want = [200.0, 100.0, 50.0].map(|c| c * keep);
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, fogged_groups.0.bind_group(0.0), &[]);
            pass.set_bind_group(2, fogged_groups.1.bind_group(0.0), &[0]);
            pass.set_vertex_buffer(0, col.slice(..));
            pass.draw(0..3, 0..1);
        });
        close(
            px,
            [0, 1, 2].map(|k| want[k] + [0.5, 0.5, 0.45][k] * 255.0 * (1.0 - keep)),
        );
        let px = centre_texel(&device, &queue, |pass| {
            pass.set_pipeline(world.pipeline(key.0, key.1, key.2, &lit_bind));
            pass.set_bind_group(0, &cam, &[]);
            pass.set_bind_group(1, &world.material_bgs[lit_bind.material as usize], &[]);
            pass.set_bind_group(2, &world.lightmap_bgs[lit_bind.lightmap as usize], &[]);
            pass.set_vertex_buffer(0, white_tri.slice(..));
            pass.set_vertex_buffer(1, world.tangent_buf.slice(..));
            pass.draw(0..3, 0..1);
        });
        close(
            px,
            [0.5, 0.5, 0.45].map(|f| 200.0 * keep + f * 255.0 * (1.0 - keep)),
        );
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
    }
}
