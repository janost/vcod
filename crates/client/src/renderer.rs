use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use vcod_common::asset_source::{AssetSource, RawFile};
use wgpu::util::DeviceExt;

use crate::camera::{Z_FAR, Z_NEAR};
use crate::dlight::{self, DlightBlend};
use crate::entity_light::{self, GpuLightSet};
use crate::entity_material::{
    ENT_STAGE_STRIDE, EntPass, EntPipelineKey, EntSkin, EntityMaterials, PassLayouts, StageImage,
};
use crate::fx::sim::{FxLight, FxQuad, MAX_LIGHTS};
use crate::gamma::GammaPass;
use crate::hud::{self, HudQuad};
use crate::hud_text::{self, HudVert};
use crate::lightmap_blocks::{self, LightmapPage};
use crate::shadows;
use crate::sky;
use crate::sun_flare::{FlareParams, SunFlareGpu};
use crate::texture_settings::{self, TexSamplers, TextureSettings};
use vcod_common::assets::{self, Image, ImageData};
use vcod_common::bsp::{self, DrawVert};
use vcod_common::deform;
use vcod_common::mapworld::MapWorld;
use vcod_common::material::{
    AlphaFunc, AlphaGen, BlendFactor, Bundle, Clamp, DLIGHT_IMAGE, Deform, DepthFunc, DrawClass,
    FalloffKind, ImageRef, Material, MaterialLib, ModelLight, PolygonOffset, RgbGen, SORT_BLEND,
    SORT_DECAL, SORT_OPAQUE, Sprite, Stage, TcGen, TcMod, TexEnv, TexSemantic, Wave, WaveForm,
    bundle_affine, bundle_turb, has_animated_tcmods, rgb_wave_value, wave_value,
};
use vcod_common::mesh::{self, Batch, IndexRange, MaterialKind};
use vcod_common::model::{self, Vertex as VmVert};
use vcod_common::net::FogParams;
use vcod_common::pk3::Pk3Fs;
use vcod_common::props;
use vcod_common::shader::{Shader, ShaderLib};
use vcod_common::static_light::{SceneLight, StaticLighting};
use vcod_common::vis::{Frustum, Visible, WorldVis};

mod capture;
mod cod2;
mod cod2_cookie;
mod cod2_water;
#[cfg(test)]
mod cod4_census;
#[cfg(test)]
mod cod4_distort_live;
#[cfg(test)]
mod cod4_dof_live;
#[cfg(test)]
mod cod4_draws_live;
#[cfg(test)]
mod cod4_live;
#[cfg(test)]
mod cod4_passes_live;
#[cfg(test)]
mod cod4_scope_live;
#[cfg(test)]
mod cod4_spot2_live;
#[cfg(test)]
mod cod4_spot_live;
mod cod4_worldfx;
#[cfg(test)]
mod cod4_worldfx_live;
pub use capture::Capture;
pub(crate) use cod2::specularity_map;
pub use cod2_cookie::{CookieEntity, CookieSettings};
mod fx_cloud;
#[cfg(test)]
mod shader_ops;
pub use fx_cloud::OutdoorSource;
#[cfg(test)]
mod fx_feather;
#[cfg(test)]
mod hud_shot;
mod mark_pass;
#[cfg(test)]
mod material_keys;
#[cfg(test)]
pub(crate) mod pipeline_check;
#[cfg(test)]
mod prop_blends;
#[cfg(test)]
mod screenshot_live;

// The vertex layout below hardcodes the BSP drawvert stride.
const _: () = assert!(std::mem::size_of::<DrawVert>() == 44);
// Ditto the viewmodel vertex stride.
const _: () = assert!(std::mem::size_of::<VmVert>() == 80);

pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
thread_local! {
    static MSAA: std::cell::Cell<u32> = const { std::cell::Cell::new(4) };
}

/// The scene's sample count, `r_aaSamples` as [`Renderer::new`] took it
/// (1 or 4) on this thread, the render thread, and on the build's worker
/// threads ([`par_build`]); every scene target and
/// pipeline reads it. 4 until a renderer sets it.
pub(crate) fn msaa_samples() -> u32 {
    MSAA.with(|m| m.get())
}

/// Sets this thread's [`msaa_samples`]: the renderer's thread, and each
/// worker it builds pipelines on.
fn set_msaa_samples(samples: u32) {
    MSAA.with(|m| m.set(samples));
}

/// `r_aaSamples` to a count wgpu takes on every adapter: 1 stays 1, any
/// other value is 4.
pub fn aa_samples(cvar: f32) -> u32 {
    if cvar <= 1.0 { 1 } else { 4 }
}

/// CoD4's Options switches the renderer reads (cod4-front-end.md 9):
/// `r_glow_allowed` (the level's glow), `r_specular` (the lit sets'
/// specular), `r_drawWater` (the water's animation), `r_zFeather` (the
/// effects' depth feather), `r_dof_enable` (the depth of field) and
/// `r_distortion` (the distortion effects' bend of the scene copy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cod4Options {
    pub glow: bool,
    pub specular: bool,
    pub water: bool,
    pub zfeather: bool,
    pub dof: bool,
    pub distortion: bool,
    /// `r_fog`; off, no draw fogs (the camera's fog mode 0).
    pub fog: bool,
    /// `r_vc_normalMaps` (vcod's): off, no CoD4 set reads its normal map
    /// ([`cod2::Cod2World::set_normal_maps`]).
    pub normal_maps: bool,
    /// `r_vc_film` (vcod's): off, the level's film grade is not drawn.
    pub film: bool,
}

impl Default for Cod4Options {
    fn default() -> Self {
        Cod4Options {
            glow: true,
            specular: true,
            water: true,
            zfeather: true,
            dof: true,
            distortion: true,
            fog: true,
            normal_maps: true,
            film: true,
        }
    }
}

/// A shader's multisampled depth binding, single-sampled when the scene is:
/// `textureLoad(t, p, 0)` reads sample 0 or mip 0 alike.
pub(crate) fn depth_wgsl(src: &str) -> std::borrow::Cow<'_, str> {
    if msaa_samples() > 1 {
        src.into()
    } else {
        src.replace("texture_depth_multisampled_2d", "texture_depth_2d")
            .into()
    }
}
/// The camera group's binding of CoD2's dynamic shadow (`t_dyn_shadow`).
pub(crate) const DYNAMIC_SHADOW_BINDING: u32 = 17;
/// Every pass draws retail's framebuffer bytes into this, blending and
/// clamping in the same byte space; the gamma pass maps it onto the
/// swapchain (docs/research/cod11-gamma.md, section 4). The HUD always
/// draws here; the world does too unless `vc_hdr` is on.
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The world's target with `vc_hdr 1`: framebuffer units that pass 1.0,
/// tonemapped back into `SCENE_FORMAT` before the HUD (crate::post).
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// `Camera` in shader.wgsl: proj, time, eye/fog tail, view forward, model.
const CAMERA_FLOATS: usize = 52;
const CAMERA_BYTES: u64 = (CAMERA_FLOATS * 4) as u64;

/// The viewmodel shares the world's fov but has its own near plane, retail's
/// `r_znear_depthhack` default (docs/research/xmodel-v14-format.md, "The
/// view fov").
pub const VM_NEAR: f32 = 0.1;
pub(crate) const VM_FAR: f32 = 500.0;
/// The depth range's front slice the viewmodel is squeezed into, retail's
/// 0..0.2 for a depth-hack entity (CoDMP.exe 0x4d6a7e).
pub(crate) const VM_DEPTH_RANGE: f32 = 0.2;
/// `proj` (64) + `model` (64) + the light set (656) + `misc` (16) +
/// `view_to_world` (64), std140-compatible as is.
const VM_UNIFORM_SIZE: u64 = VM_VIEW_TO_WORLD_OFFSET + 64;
/// The light set sits after the projection and motion transform.
const VM_LIGHTS_OFFSET: u64 = 128;
/// `misc` (x identityLight, y 1 on CoD2, z the clock) follows the light set.
const VM_MISC_OFFSET: u64 = VM_LIGHTS_OFFSET + std::mem::size_of::<GpuLightSet>() as u64;
/// The view's rotation back to the world, which a CoD2 probe set reads.
const VM_VIEW_TO_WORLD_OFFSET: u64 = VM_MISC_OFFSET + 16;
/// Matches `array<mat4x4<f32>, 128>` in viewmodel.wgsl.
pub(crate) const VM_BONE_COUNT: usize = 128;
const VM_BONE_BUF_SIZE: u64 = (VM_BONE_COUNT * 64) as u64;

/// Debug-HUD glyph budget per frame; lines past it are cut.
const MAX_HUD_GLYPHS: usize = 1024;

/// `fx::sim::MAX_PARTICLES` and room for the head icons.
const MAX_FX_QUADS: usize = 2048 + 256;
/// Quads the fx buffers hold once each is drawn once per stage: the stock
/// effect materials have at most two base stages.
const MAX_FX_DRAWN: usize = MAX_FX_QUADS * 2;

const MAX_HUD_QUADS: usize = 16384;

/// Per-vertex rgba lets particles, decals and tracers share one pipeline.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FxVert {
    pos: [f32; 3],
    uv: [f32; 2],
    rgba: [f32; 4],
    /// The quad's smaller half-extent; scales the `vc_softParticles` fade.
    size: f32,
    /// `FX_VERT_*` bits, the stage's (`fx_stage_draw`).
    flags: u32,
    /// A `zfeather*` material's `featherParms.x` on a stage whose blend
    /// reads source alpha; 0 draws no feather (`fx_feather`).
    feather: f32,
}

const _: () = assert!(std::mem::size_of::<FxVert>() == 48);

/// `fx.wgsl`: the material is `nofog`.
const FX_VERT_NOFOG: u32 = 1;
/// `fx.wgsl`: an additive draw scales by the texture's alpha
/// (`GL_SRC_ALPHA GL_ONE`).
const FX_VERT_TEX_ALPHA: u32 = 2;
/// `fx.wgsl`: the stage's `alphaFunc`, tested on texture alpha times the
/// gen's: `GT0`, `LT128`, `GE128`.
const FX_VERT_ALPHA_GT0: u32 = 4;
const FX_VERT_ALPHA_LT128: u32 = 8;
const FX_VERT_ALPHA_GE128: u32 = 16;
/// `fx.wgsl`: the feather reads the view depth itself, CoD4's
/// `zfeather.hlsl`, not CoD2's clip z (`Material::feather_view_depth`).
const FX_VERT_FEATHER_W: u32 = 32;
/// `fx.wgsl`: a CoD4 distortion quad, whose `feather` and `size` carry
/// the material's `distortionScale` (`fs_distort`).
const FX_VERT_DISTORT: u32 = 64;
/// `tex_samplers.wgsl`'s `F_CLAMP_S`, `F_CLAMP_T`: bundle 0 clamps that axis.
const FX_VERT_CLAMP_S: u32 = STAGE_FLAG_CLAMP_S;
const FX_VERT_CLAMP_T: u32 = STAGE_FLAG_CLAMP_T;

/// The fx pass's blend states, retail's `glBlendFunc` for each stage
/// (GL_State 0x4d57b0 maps the script's pair straight through;
/// cod11-shader-scripts.md section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FxBlend {
    /// `GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA`, and any pair not below.
    Alpha,
    /// Onto `GL_ONE`, drawn premultiplied as `GL_ONE GL_ONE`.
    Add,
    /// `GL_ONE GL_ONE_MINUS_SRC_COLOR` (UO's `flamethrowerFire*`).
    Screen,
    /// No blend (or `GL_ONE GL_ZERO`): writes depth, as the gib and debris
    /// materials' `alphaFunc GE128` stages do.
    Opaque,
}

/// One draw of an fx material: a stage's image (the anim frame at the
/// frame's shader time), blend, `FX_VERT_*` bits and texture matrix. The
/// fx pass draws a run's quads once per stage, stage by stage, as retail's
/// stage loop does (0x500b30).
struct FxStageDraw<'a> {
    image: &'a str,
    blend: FxBlend,
    flags: u32,
    /// bundle 0's tcMods folded (`bundle_affine`); identity without a script.
    uv: [f32; 6],
    /// `None` for a material without a script (`rgbGen vertex`).
    stage: Option<&'a Stage>,
}

/// The stages the fx pass draws for a material, in order. Retail's base loop
/// (0x500b30) draws every stage not flagged `perLight` (vcod parks those in
/// `Material::dlight`), each through 0x5008a0 with its own colours
/// (0x4ffa60), texture coordinates (0x500400), image (0x4fe0e0) and state
/// (0x4d57b0). Bundle 1 is not drawn: a `$lightmap` there is the white
/// image under a registered material's lightmap index -4 (0x4f3699). A stage
/// whose bundle 0 is not an image file draws nothing here. A stage wraps
/// its image unless the bundle says `clamp` (image flags 0x10, 0x20;
/// 0x4ec6e9). A material without a script draws its name as the image,
/// straight alpha and clamped, as the pass always drew it.
fn fx_stage_draws<'a>(shader: Option<&'a Material>, name: &'a str, t: f32) -> Vec<FxStageDraw<'a>> {
    let Some(sh) = shader else {
        return vec![FxStageDraw {
            image: name,
            blend: FxBlend::Alpha,
            flags: FX_VERT_CLAMP_S | FX_VERT_CLAMP_T,
            uv: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stage: None,
        }];
    };
    sh.stages
        .iter()
        .filter_map(|st| {
            let b0 = st.bundles.first()?;
            let image = match (&b0.anim, &b0.image) {
                (Some(anim), _) if !anim.paths.is_empty() => {
                    anim.paths[anim_frame_index(anim.fps, anim.paths.len(), t)].as_str()
                }
                (_, ImageRef::Path(p)) => p.as_str(),
                _ => return None,
            };
            let (blend, mut flags) = fx_stage_draw(sh, st);
            if b0.clamp.s {
                flags |= FX_VERT_CLAMP_S;
            }
            if b0.clamp.t {
                flags |= FX_VERT_CLAMP_T;
            }
            Some(FxStageDraw {
                image,
                blend,
                flags,
                uv: bundle_affine(&b0.tcmods, t),
                stage: Some(st),
            })
        })
        .collect()
}

/// Every image [`fx_stage_draws`] can name for a material at any time
/// (each anim frame), and whether a script stage names it.
fn fx_stage_images<'a>(shader: Option<&'a Material>, name: &'a str) -> Vec<(&'a str, bool)> {
    let Some(sh) = shader else {
        return vec![(name, false)];
    };
    let mut out = Vec::new();
    for st in &sh.stages {
        let Some(b0) = st.bundles.first() else {
            continue;
        };
        match (&b0.anim, &b0.image) {
            (Some(anim), _) if !anim.paths.is_empty() => {
                out.extend(anim.paths.iter().map(|p| (p.as_str(), true)));
            }
            (_, ImageRef::Path(p)) => out.push((p.as_str(), true)),
            _ => {}
        }
    }
    out
}

/// The feather scale a stage's quads carry: the material's
/// `featherParms.x` when the stage's blend reads the source alpha the
/// feather fades (z_feather.hlsl scales only `color.a`;
/// cod2-materials.md 9.5), else 0. `r_zFeather` 0 (`zfeather` false)
/// draws a CoD4 set as its plain remap, with no feather at all
/// (cod4-effects.md 14).
fn fx_feather(sh: Option<&Material>, draw: &FxStageDraw, zfeather: bool) -> f32 {
    let Some(fp) = sh
        .filter(|sh| zfeather || !sh.feather_view_depth)
        .and_then(|sh| sh.feather)
    else {
        return 0.0;
    };
    let reads_alpha = match draw.blend {
        FxBlend::Alpha => true,
        FxBlend::Add => draw.flags & FX_VERT_TEX_ALPHA != 0,
        FxBlend::Screen | FxBlend::Opaque => false,
    };
    if reads_alpha { fp.max(0.0) } else { 0.0 }
}

/// How the fx pass draws one stage: its blend and its `FX_VERT_*` bits.
/// Retail fogs a stage unless the shader is `nofog` (0x4fbc71), towards
/// black when it blends onto `GL_ONE` (0x4d2ad0); the colour that reaches
/// the blend is texture times the gen's colour under `GL_MODULATE`, so a
/// `GL_SRC_ALPHA` source reads the texture's alpha.
fn fx_stage_draw(sh: &Material, st: &Stage) -> (FxBlend, u32) {
    use BlendFactor as F;
    let mut flags = if sh.nofog { FX_VERT_NOFOG } else { 0 };
    flags |= match st.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => FX_VERT_ALPHA_GT0,
        Some(AlphaFunc::Lt128) => FX_VERT_ALPHA_LT128,
        Some(AlphaFunc::Ge128) => FX_VERT_ALPHA_GE128,
    };
    let blend = match st.blend.as_ref() {
        None | Some((F::One, F::Zero)) => FxBlend::Opaque,
        Some((F::SrcAlpha, F::One)) => {
            flags |= FX_VERT_TEX_ALPHA;
            FxBlend::Add
        }
        Some((_, F::One)) => FxBlend::Add,
        Some((F::One, F::OneMinusSrcColor)) => FxBlend::Screen,
        _ => FxBlend::Alpha,
    };
    (blend, flags)
}

/// A stage's vertex colour for a sprite whose own colour is `v` (the efx
/// `rgb` and `alpha` curves, CoDMP.exe 0x48e640, 0x48e7d0), as 0x4ffa60
/// computes it: `identity` is white and `identityLighting` identityLight
/// flat, both opaque, whatever the sprite's colour; `vertex` takes the
/// sprite's colour times identityLight, `exactVertex` as it is; `const` and
/// `wave` ignore it. The parser gives `rgbGen vertex` and `exactVertex`
/// `alphaGen vertex` (0x4f8d4f), so their stages fade with the sprite, and
/// alphaGen identity is opaque elsewhere (0x4ffa60's alpha case 0, which
/// leaves `rgbGen vertex`'s copied alpha at identityLight 1). A
/// stage blending `GL_ONE GL_ONE` reads no source alpha; `fs_additive`
/// scales by it, so it gets 1.
fn fx_stage_rgba(stage: Option<&Stage>, v: [f32; 4], identity_light: f32, t: f32) -> [f32; 4] {
    let [r, g, b, a] = v;
    let il = identity_light;
    let Some(st) = stage else {
        return [r * il, g * il, b * il, a];
    };
    let rgb = match &st.rgb_gen {
        RgbGen::Identity => [1.0; 3],
        RgbGen::IdentityLighting => [il; 3],
        RgbGen::Vertex | RgbGen::LightingDiffuse => [r * il, g * il, b * il],
        RgbGen::ExactVertex | RgbGen::LightingPrecalc => [r, g, b],
        RgbGen::Const(c) => *c,
        RgbGen::ConstLighting(c) => c.map(|x| x * il),
        // a table wave clamps after its identityLight scale; the noise
        // glow skips the scale (0x514c00)
        RgbGen::Wave(w) if w.form == WaveForm::Noise => [rgb_wave_value(w, t).clamp(0.0, 1.0); 3],
        RgbGen::Wave(w) => [(rgb_wave_value(w, t) * il).clamp(0.0, 1.0); 3],
    };
    let alpha = match &st.alpha_gen {
        AlphaGen::Vertex | AlphaGen::Entity => a,
        // case 0 keeps what `rgbGen vertex` copied only at identityLight 1
        AlphaGen::Identity if st.rgb_gen == RgbGen::Vertex && il == 1.0 => a,
        AlphaGen::Identity => 1.0,
        AlphaGen::Const(c) => c.clamp(0.0, 1.0),
        AlphaGen::Wave(w) => wave_value(w, t).clamp(0.0, 1.0),
    };
    let alpha = if st.blend == Some((BlendFactor::One, BlendFactor::One)) {
        1.0
    } else {
        alpha
    };
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// The wgpu blend of each [`FxBlend`] pipeline. `fs_additive` and
/// `fs_screen` write alpha 0, so their alpha component leaves the target's
/// alpha alone.
fn fx_blend_state(blend: FxBlend) -> Option<wgpu::BlendState> {
    let component = |src_factor, dst_factor| wgpu::BlendComponent {
        src_factor,
        dst_factor,
        operation: wgpu::BlendOperation::Add,
    };
    let add = component(wgpu::BlendFactor::One, wgpu::BlendFactor::One);
    match blend {
        FxBlend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
        FxBlend::Add => Some(wgpu::BlendState {
            color: add,
            alpha: add,
        }),
        FxBlend::Screen => Some(wgpu::BlendState {
            color: component(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrc),
            alpha: add,
        }),
        FxBlend::Opaque => None,
    }
}

/// The world batches' pipelines. Layers (polygonOffset terrain blends) and
/// overlays (decals, masked cutouts) are coplanar with the world and draw
/// depth-biased on top. `Stage` marks batches expanded into per-stage draws
/// instead. Placement comes from the batch's sort key (`order_draws`).
#[derive(Copy, Clone, PartialEq)]
enum Pass {
    /// `fs_main`: the image's alpha cut at 0.5.
    Opaque,
    /// A CoD1 `Opaque` batch whose image has no texel the cut drops, drawn
    /// without the discard (`fs_main_solid`) so early depth writes hold.
    Solid,
    /// Static models (`fs_prop`): `culled` as retail culls a one-sided
    /// skin, `solid` as [`Pass::Solid`] (cod11-client-perf.md 5).
    Prop {
        culled: bool,
        solid: bool,
    },
    Layer,
    Overlay,
    Stage,
}

struct DrawCall {
    bind_group: usize,
    pass: Pass,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum CullMode {
    On,
    Locked,
    Off,
}

impl CullMode {
    pub fn next(self) -> Self {
        match self {
            CullMode::On => CullMode::Locked,
            CullMode::Locked => CullMode::Off,
            CullMode::Off => CullMode::On,
        }
    }
}

impl std::fmt::Display for CullMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CullMode::On => "on",
            CullMode::Locked => "locked",
            CullMode::Off => "off",
        })
    }
}

/// A per-frame draw: `batch` picks the bind group and pass, the range is in
/// the gathered index buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DrawRange {
    pub batch: u32,
    pub first: u32,
    pub count: u32,
}

/// One entry of a frame's emission-ordered list.
#[derive(Copy, Clone)]
enum DrawRef {
    /// A legacy batch on its fixed pipeline.
    Legacy(Pass),
    /// An authored stage draw; index into `WorldGpu::stage_batches`.
    Stage(u32),
}

struct FrameDraw {
    kind: DrawRef,
    /// The batch's material sort and its place in the sorted shader list
    /// (`WorldGpu::batch_keys`).
    sort: f32,
    rank: u32,
    /// Index into `WorldGpu::draws`, the gathered visible range.
    src: u32,
}

/// The pipeline a run of world draws has bound.
#[derive(Copy, Clone, PartialEq)]
enum Bound {
    Legacy(Pass),
    Stage(StageVariant, bool, bool),
    /// CoD2's lit pipeline (`cod2::Cod2World`), then a static prop's and
    /// a water's, and which of the depth prepass's pipelines.
    Lit(StageVariant, bool, bool, cod2::FsKey, bool, Depth),
    /// A CoD4 feathered or distortion stage (`cod4_worldfx`).
    Fx(cod4_worldfx::WorldFx, StageVariant, bool, bool),
}

/// What [`Renderer::encode_cookie_receivers`] reads and writes.
struct CookieReceivers<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    world: Option<&'a WorldGpu>,
    cookies: &'a mut cod2_cookie::Cookies,
    dyn_shadows: &'a mut cod2_cookie::DynamicShadows,
    depth_view: &'a wgpu::TextureView,
    /// A CoD2 level's lighting is up.
    cod2: bool,
}

/// A receiving soup's range of the gathered indices and its box (`None`:
/// every cookie), `WorldGpu::cookie_runs`.
type CookieRun = (std::ops::Range<u32>, Option<(glam::Vec3, glam::Vec3)>);

/// Which draws one depth prepass pass takes ([`Renderer::encode_depth_prepass`]):
/// the world's, the entities', `floatZ` too, and whether it keeps the
/// depth an earlier part left.
#[derive(Clone, Copy)]
struct PrepassParts {
    world: bool,
    ents: bool,
    floatz: bool,
    load: bool,
}

/// Whether world draw `kind` takes the depth prepass
/// ([`cod2::Cod2World::takes_prepass`]): only the lit family's opaque ones.
fn takes_prepass(world: &WorldGpu, kind: DrawRef) -> bool {
    let DrawRef::Stage(i) = kind else {
        return false;
    };
    let sb = &world.stage_batches[i as usize];
    match (sb.cod2, &world.cod2) {
        (Some(lit), Some(c2)) => c2.takes_prepass(sb.variant, sb.two_sided, sb.bias, &lit),
        _ => false,
    }
}

/// Whether world draw `kind` writes `floatZ` in the depth prepass of a
/// CoD4 frame a feather reads, and whether it alpha tests there
/// ([`cod2::Cod2World::takes_floatz`]).
fn takes_floatz(world: &WorldGpu, kind: DrawRef) -> Option<bool> {
    let DrawRef::Stage(i) = kind else {
        return None;
    };
    let sb = &world.stage_batches[i as usize];
    match (sb.cod2, &world.cod2) {
        (Some(lit), Some(c2)) => c2.takes_floatz(sb.variant, &lit),
        _ => None,
    }
}

/// Whether an entity surface on `skin` takes the depth prepass: its
/// first stage is opaque, depth written and not alpha tested.
fn ent_takes_prepass(skin: &EntSkin) -> bool {
    skin.stages.first().is_some_and(|st| st.prepass)
}

/// Test only: one entity draw of [`Renderer::ent_census`].
#[cfg(test)]
pub(crate) type EntCensus = (u32, u32, u32, usize, bool, bool);

/// A frame's draw calls by kind and its pipeline and bind group sets, in
/// the depth prepass and the main scene pass (cod4-client-perf.md 21).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct PassCensus {
    pub sky: u32,
    pub world: u32,
    pub ents: u32,
    pub gun: u32,
    pub marks: u32,
    pub fx: u32,
    pub dlights: u32,
    pub prepass_world: u32,
    pub prepass_ents: u32,
    pub pipelines: u32,
    pub bind_groups: u32,
}

impl PassCensus {
    pub fn draws(&self) -> u32 {
        self.sky
            + self.world
            + self.ents
            + self.gun
            + self.marks
            + self.fx
            + self.dlights
            + self.prepass_world
            + self.prepass_ents
    }
}

/// One GPU write of a frame (cod4-client-perf.md 24): a pass with the
/// bytes its attachments store (each attachment once, whatever the
/// overdraw), a copy or a buffer upload.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct FrameWrite {
    pub what: String,
    pub bytes: u64,
}

/// Logs a frame write on `$r` in tests; nothing in the shipped client.
macro_rules! frame_write {
    ($r:expr, $what:expr, $bytes:expr) => {
        #[cfg(test)]
        $r.frame_writes.borrow_mut().push(FrameWrite {
            what: ($what).to_string(),
            bytes: ($bytes) as u64,
        });
    };
}
/// What `Renderer::cull` reads off the frame, bit for bit.
type CullKey = ([u32; 3], [u32; 16], [u32; 2]);

fn cull_key(eye: glam::Vec3, view_proj: &glam::Mat4, lod: (f32, f32)) -> CullKey {
    (
        eye.to_array().map(f32::to_bits),
        view_proj.to_cols_array().map(f32::to_bits),
        [lod.0.to_bits(), lod.1.to_bits()],
    )
}

/// Where `emit_world_draw` sends its commands: the scene pass itself, or
/// a [`Recording`] that a render bundle replays (cod4-client-perf.md 31).
trait DrawSink {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline);
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>);
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>);
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat);
    fn draw(&mut self, indices: std::ops::Range<u32>);
    /// An entity surface's draw: its vertices from `base`, one instance.
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32);
}

impl DrawSink for wgpu::RenderPass<'_> {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline) {
        self.set_pipeline(p);
    }
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>) {
        self.set_bind_group(slot, bg, offset.as_slice());
    }
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>) {
        self.set_vertex_buffer(slot, s);
    }
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat) {
        self.set_index_buffer(s, f);
    }
    fn draw(&mut self, indices: std::ops::Range<u32>) {
        self.draw_indexed(indices, 0, 0..1);
    }
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32) {
        self.draw_indexed(indices, base, instance..instance + 1);
    }
}

/// One command of a [`Recording`].
#[derive(PartialEq)]
enum Cmd {
    Pipeline(wgpu::RenderPipeline),
    Group(u32, wgpu::BindGroup, Option<u32>),
    Vertices(u32, wgpu::Buffer, u64, u64),
    Indices(wgpu::Buffer, wgpu::IndexFormat, u64, u64),
    Draw(std::ops::Range<u32>),
    DrawInst(std::ops::Range<u32>, i32, u32),
}

/// A run of world draws' commands, recorded every frame (an `Arc` clone
/// a handle, no validation) and compared with the last frame's: the
/// same commands replay last frame's bundle.
#[derive(Default, PartialEq)]
struct Recording(Vec<Cmd>);

impl DrawSink for Recording {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline) {
        self.0.push(Cmd::Pipeline(p.clone()));
    }
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>) {
        self.0.push(Cmd::Group(slot, bg.clone(), offset));
    }
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>) {
        self.0.push(Cmd::Vertices(
            slot,
            s.buffer().clone(),
            s.offset(),
            s.size(),
        ));
    }
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat) {
        self.0
            .push(Cmd::Indices(s.buffer().clone(), f, s.offset(), s.size()));
    }
    fn draw(&mut self, indices: std::ops::Range<u32>) {
        self.0.push(Cmd::Draw(indices));
    }
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32) {
        self.0.push(Cmd::DrawInst(indices, base, instance));
    }
}

impl Recording {
    fn draws(&self) -> usize {
        self.0
            .iter()
            .filter(|c| matches!(c, Cmd::Draw(_) | Cmd::DrawInst(..)))
            .count()
    }

    /// The commands into `pass`, one by one.
    fn replay(&self, pass: &mut impl DrawSink) {
        for c in &self.0 {
            match c {
                Cmd::Pipeline(p) => pass.pipeline(p),
                Cmd::Group(slot, bg, o) => pass.group(*slot, bg, *o),
                Cmd::Vertices(slot, b, o, n) => pass.vertices(*slot, b.slice(*o..*o + *n)),
                Cmd::Indices(b, f, o, n) => pass.indices(b.slice(*o..*o + *n), *f),
                Cmd::Draw(r) => pass.draw(r.clone()),
                Cmd::DrawInst(r, b, i) => pass.draw_inst(r.clone(), *b, *i),
            }
        }
    }

    /// The commands in a bundle for a pass with colour target `format`
    /// (none: the depth prepass) over the scene's depth.
    fn bundle(
        &self,
        device: &wgpu::Device,
        format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderBundle {
        let colour = [format];
        let mut enc = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
            label: Some("world run"),
            color_formats: match format {
                Some(_) => &colour,
                None => &[],
            },
            depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                format: DEPTH_FORMAT,
                depth_read_only: false,
                stencil_read_only: false,
            }),
            sample_count: msaa_samples(),
            multiview: None,
        });
        for c in &self.0 {
            match c {
                Cmd::Pipeline(p) => enc.set_pipeline(p),
                Cmd::Group(slot, bg, o) => enc.set_bind_group(*slot, bg, o.as_slice()),
                Cmd::Vertices(slot, b, o, n) => enc.set_vertex_buffer(*slot, b.slice(*o..*o + *n)),
                Cmd::Indices(b, f, o, n) => enc.set_index_buffer(b.slice(*o..*o + *n), *f),
                Cmd::Draw(r) => enc.draw_indexed(r.clone(), 0, 0..1),
                Cmd::DrawInst(r, b, i) => enc.draw_indexed(r.clone(), *b, *i..*i + 1),
            }
        }
        enc.finish(&wgpu::RenderBundleDescriptor {
            label: Some("world run"),
        })
    }
}

/// A pass's runs of draws, in frame order, each with its bundle once its
/// commands held from one frame to the next (cod4-client-perf.md 31, 40):
/// the scene pass's world runs and entity runs, the depth prepass's world
/// and entities. A run whose commands change every frame (a moving view,
/// a body entering or leaving it) replays into the pass: a bundle made
/// for one frame costs more than it saves.
#[derive(Default)]
struct WorldBundles {
    runs: Vec<(Recording, Option<wgpu::RenderBundle>)>,
    /// The targets the bundles were made for: colour format (none in a
    /// depth-only prepass), samples.
    targets: Option<(Option<wgpu::TextureFormat>, u32)>,
    /// This frame's runs so far.
    at: usize,
}

impl WorldBundles {
    fn begin(&mut self, format: Option<wgpu::TextureFormat>) {
        let targets = Some((format, msaa_samples()));
        if self.targets != targets {
            self.runs.clear();
            self.targets = targets;
        }
        self.at = 0;
    }

    /// This frame's next run, recorded in `rec`, into `pass`: last frame's
    /// run at the same place held the same commands, its bundle (made now
    /// if it was new then), else the commands one by one. A bundled run's
    /// draws count under `kind`.
    fn draw(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        device: &wgpu::Device,
        rec: Recording,
        kind: crate::frame_prof::Count,
    ) {
        use crate::frame_prof::{Count, count};
        let format = self.targets.and_then(|t| t.0);
        let at = self.at;
        self.at += 1;
        match self.runs.get_mut(at) {
            Some((old, bundle)) if *old == rec => {
                if bundle.is_none() {
                    count(Count::Rebundled, 1);
                }
                count(kind, rec.draws());
                pass.execute_bundles([&*bundle.get_or_insert_with(|| rec.bundle(device, format))]);
            }
            run => {
                rec.replay(pass);
                match run {
                    Some(run) => *run = (rec, None),
                    None => self.runs.push((rec, None)),
                }
            }
        }
    }

    /// Drops the runs past this frame's last.
    fn end(&mut self) {
        self.runs.truncate(self.at);
    }
}

/// The bind groups a run of world draws left at groups 1 and 2: each
/// one's address and dynamic offset. A pipeline set clears it, so a
/// draw rebinds only a group the last draw under its pipeline did not
/// leave (cod4-client-perf.md 21).
#[derive(Clone, Copy, Default, PartialEq)]
struct WorldGroups([Option<(usize, u32)>; 2]);

impl WorldGroups {
    /// Binds `bg` at group `slot` (1 or 2) unless it is there already.
    fn bind(
        &mut self,
        pass: &mut impl DrawSink,
        slot: u32,
        bg: &wgpu::BindGroup,
        offset: Option<u32>,
        census: &std::cell::Cell<PassCensus>,
    ) {
        let key = Some((std::ptr::from_ref(bg) as usize, offset.unwrap_or(u32::MAX)));
        let at = &mut self.0[slot as usize - 1];
        if *at == key {
            return;
        }
        *at = key;
        pass.group(slot, bg, offset);
        tally(census, |c| c.bind_groups += 1);
    }
}

/// Adds to a frame's [`PassCensus`].
fn tally(c: &std::cell::Cell<PassCensus>, f: impl FnOnce(&mut PassCensus)) {
    let mut v = c.get();
    f(&mut v);
    c.set(v);
}

/// Where a draw stands to `r_vc_depthprepass`: its own pipeline, the
/// prepass's depth-only draw, or its colour draw under EQUAL after it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Depth {
    Plain,
    Prepass,
    /// The prepass writing `floatZ` too (cod4-client-perf.md 35).
    FloatZ,
    Equal,
}

/// One entity or viewmodel surface in draw order.
struct EntDraw {
    /// Instance (or viewmodel part) and its surface.
    inst: u32,
    surf: u32,
    sort: f32,
    order: u32,
    /// A viewmodel surface: depth-hack range and the view camera.
    gun: bool,
}

/// The live entities' and the viewmodel's `(instance, surface, material)`
/// in instance order, in one list sorted as retail's key does: the
/// material's sort, its load order (the sorted index), then the entity
/// number. The gun is added after the packet entities, so on one material
/// it draws after them (docs/research/cod11-shader-scripts.md, §5).
fn sorted_ent_draws<'a>(
    ents: impl Iterator<Item = (u32, u32, &'a EntSkin)>,
    gun: impl Iterator<Item = (u32, u32, &'a EntSkin)>,
) -> Vec<EntDraw> {
    let draw = |is_gun| {
        move |(inst, surf, sk): (u32, u32, &EntSkin)| EntDraw {
            inst,
            surf,
            sort: sk.sort,
            order: sk.order,
            gun: is_gun,
        }
    };
    let mut out: Vec<EntDraw> = ents.map(draw(false)).chain(gun.map(draw(true))).collect();
    // stable: equal keys keep instance then surface order
    out.sort_by(|a, b| {
        a.sort
            .total_cmp(&b.sort)
            .then(a.order.cmp(&b.order))
            .then(a.gun.cmp(&b.gun))
    });
    out
}

/// Where the dlight pass goes in the merged list: after the last
/// world draw that sorts up to opaque, as retail lights a batch right
/// after its base stages (0x500d3a) and only those sorts take it.
fn dlight_slot(items: &[SortedItem], world_sorts: &[f32]) -> usize {
    items
        .iter()
        .rposition(|it| matches!(it, SortedItem::World(wi) if world_sorts[*wi] <= SORT_OPAQUE))
        .map_or(0, |at| at + 1)
}

/// The frame's MSAA colour and depth. With SSAO the frame is two passes
/// split after the last opaque or decal draw (`ssao_split`): the first
/// keeps its MSAA colour for the SSAO multiply, the second loads it and
/// resolves.
struct SceneTargets<'a> {
    msaa: &'a wgpu::TextureView,
    depth: &'a wgpu::TextureView,
    resolve: &'a wgpu::TextureView,
    timer: Option<&'a crate::ssao::GpuTimer>,
    clear: wgpu::Color,
    /// The sun flare's probe query, which runs in the last pass.
    sun_queries: Option<&'a wgpu::QuerySet>,
    /// The depth prepass laid the depth down: the first pass loads it.
    depth_cleared: bool,
}

impl SceneTargets<'_> {
    fn begin<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        first: bool,
        last: bool,
    ) -> wgpu::RenderPass<'e> {
        // single-sampled, the scene draws straight into its resolve target
        let resolving = msaa_samples() > 1;
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: if resolving { self.msaa } else { self.resolve },
                depth_slice: None,
                resolve_target: (last && resolving).then_some(self.resolve),
                ops: wgpu::Operations {
                    load: if first {
                        wgpu::LoadOp::Clear(self.clear)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    // only the resolved single-sample image is needed
                    store: if last && resolving {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: if first && !self.depth_cleared {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            // a middle pass (the feather's depth copy splits one off) times nothing
            timestamp_writes: self.timer.filter(|_| first || last).map(|t| {
                t.writes(
                    first.then_some(crate::ssao::GpuTimer::SCENE_BEGIN),
                    last.then_some(crate::ssao::GpuTimer::SCENE_END),
                )
            }),
            occlusion_query_set: if last { self.sun_queries } else { None },
            multiview_mask: None,
        })
    }
}

/// Where the SSAO multiply goes in the merged list: after the last draw
/// that sorts up to decal, so it darkens the opaque surfaces and the
/// polygonOffset layers and impact marks blended onto them and none of the
/// blends after.
/// Where the depth copy a CoD2 feather reads goes: before the first
/// entity or view weapon surface, so the copy holds the world (props
/// included) and not the models, as retail's `floatZ` under
/// `r_depthPrepassModels 0` does (cod2-materials.md 9.5); never past the
/// frame's `split`.
fn feather_copy_at(items: &[SortedItem], split: usize) -> usize {
    items
        .iter()
        .position(|it| matches!(it, SortedItem::Model(_)))
        .map_or(split, |m| m.min(split))
}

fn ssao_split(
    items: &[SortedItem],
    world: &[f32],
    marks: &[f32],
    models: &[f32],
    fx: &[f32],
) -> usize {
    items
        .iter()
        .rposition(|it| {
            let sort = match *it {
                SortedItem::World(i) => world[i],
                SortedItem::Mark(i) => marks[i],
                SortedItem::Model(i) => models[i],
                SortedItem::Fx(i) => fx[i],
            };
            sort <= SORT_DECAL
        })
        .map_or(0, |at| at + 1)
}

/// What a run of entity draws left bound: the pipeline (a stage's index,
/// or a prepass one's `usize::MAX - n`), and since that pipeline the image
/// group's address and the stage parameters' group and offset, so a draw
/// rebinds only what changed (cod4-client-perf.md 38).
#[derive(Clone, Copy, Default, PartialEq)]
struct EntBound {
    pipeline: Option<usize>,
    image: Option<usize>,
    params: Option<(usize, u32)>,
}

/// Draws one entity surface's stages back to back, as retail's stage
/// iterator does, over `indices` (the surface's own, or a run of its
/// neighbours' from [`ent_run`]). `bound` tracks the bound pipeline across
/// calls.
#[allow(clippy::too_many_arguments)]
fn draw_ent_stages(
    pass: &mut impl DrawSink,
    mats: &EntityMaterials,
    which: EntPass,
    skin_bg: &wgpu::BindGroup,
    skin: &EntSkin,
    surf: &VmSurface,
    (indices, instance): (std::ops::Range<u32>, u32),
    bound: &mut EntBound,
    depth: Depth,
    census: &std::cell::Cell<PassCensus>,
) {
    for (si, st) in skin.stages.iter().enumerate() {
        // the prepass draws the first stage alone
        if matches!(depth, Depth::Prepass | Depth::FloatZ) && si > 0 {
            break;
        }
        // the prepass's own pipelines bind as `usize::MAX - n`, culled as
        // the stage's own pipeline is
        let cull = mats.pipelines[st.pipeline].0.3;
        let c = usize::from(cull);
        let (key, pipeline) = match depth {
            Depth::Prepass => (usize::MAX - 4 * c, &mats.prepass[c][0]),
            Depth::FloatZ => (usize::MAX - 2 - 4 * c, &mats.floatz[c]),
            Depth::Equal if si == 0 => (usize::MAX - 1 - 4 * c, &mats.prepass[c][1]),
            _ => (st.pipeline, &mats.pipelines[st.pipeline].1[which as usize]),
        };
        if bound.pipeline != Some(key) {
            pass.pipeline(pipeline);
            *bound = EntBound {
                pipeline: Some(key),
                ..EntBound::default()
            };
            tally(census, |c| c.pipelines += 1);
        }
        let image = match st.image {
            StageImage::Skin => skin_bg,
            StageImage::Own(i) => &mats.images[i],
        };
        let image_key = Some(std::ptr::from_ref(image) as usize);
        if bound.image != image_key {
            pass.group(1, image, None);
            bound.image = image_key;
            tally(census, |c| c.bind_groups += 1);
        }
        let params = (st.params, (u64::from(st.slot) * ENT_STAGE_STRIDE) as u32);
        if bound.params != Some(params) {
            pass.group(3, &mats.param_groups[params.0], Some(params.1));
            bound.params = Some(params);
            tally(census, |c| c.bind_groups += 1);
        }
        pass.draw_inst(indices.clone(), surf.base_vertex, instance);
        tally(census, |c| match (depth, which) {
            (Depth::Prepass | Depth::FloatZ, _) => c.prepass_ents += 1,
            (_, EntPass::View) => c.gun += 1,
            _ => c.ents += 1,
        });
    }
}

/// How many of `items` from `k` on draw as one: the entity surface at `k`
/// and each next item that is the same instance's next surface on the
/// same one-stage skin and bind group, its indices following on. One draw
/// over their joined range rasterizes the same triangles in the same
/// order. `stops` are the item indices something else happens before
/// (a split, the dlights), which a run never crosses.
fn ent_run(
    items: &[SortedItem],
    k: usize,
    draws: &[EntDraw],
    model: &DynModel,
    stops: &[Option<usize>],
) -> usize {
    let SortedItem::Model(first) = items[k] else {
        return 1;
    };
    let d = &draws[first];
    let skin = &model.skins[d.surf as usize];
    if skin.stages.len() != 1 {
        return 1;
    }
    let mut end = &model.vm.surfaces[d.surf as usize];
    let mut n = 1;
    while let Some(&SortedItem::Model(next)) = items.get(k + n) {
        let e = &draws[next];
        let Some(surf) = model.vm.surfaces.get(e.surf as usize) else {
            break;
        };
        if e.gun
            || e.inst != d.inst
            || e.surf != d.surf + n as u32
            || stops.contains(&Some(k + n))
            || !Rc::ptr_eq(&model.skins[e.surf as usize], skin)
            || surf.bind_group != end.bind_group
            || surf.base_vertex != end.base_vertex
            || surf.first_index != end.first_index + end.index_count
        {
            break;
        }
        end = surf;
        n += 1;
    }
    n
}

/// The emission order of `draws`, into `out`: by the batch's place in the
/// sorted shader list (`keys`, material sort first), each batch's stages
/// back to back in stage order, as retail's draw loop iterates a shader's
/// stages over its surfaces (shader doc 5). Sky soups are never in `draws`.
fn order_draws(
    keys: &[(f32, u32)],
    calls: &[DrawCall],
    stages_of_batch: &[Vec<u32>],
    draws: &[DrawRange],
    out: &mut Vec<FrameDraw>,
) {
    out.clear();
    for (di, d) in draws.iter().enumerate() {
        let b = d.batch as usize;
        let (sort, rank) = keys[b];
        let at = |kind| FrameDraw {
            kind,
            sort,
            rank,
            src: di as u32,
        };
        match calls[b].pass {
            Pass::Stage => out.extend(stages_of_batch[b].iter().map(|&sb| at(DrawRef::Stage(sb)))),
            p => out.push(at(DrawRef::Legacy(p))),
        }
    }
    // stable: a batch keeps its stage order
    out.sort_by_key(|fd| fd.rank);
}

/// A shader as R_SortNewShader (CoDMP.exe 0x4fb790) reads it: its sort and
/// its first stage's bundle 0 and 1 image numbers (0 for none), `None` for
/// a shader without stages.
#[derive(Copy, Clone, Debug)]
struct SortEntry {
    sort: f32,
    images: Option<[u32; 2]>,
}

/// A scripted shader's entry: its stages' images numbered in stage order,
/// then its sort and its sorting stage's two bundle images.
fn staged_sort_entry(sh: &Material, image: &mut dyn FnMut(&ImageRef) -> u32) -> SortEntry {
    for st in &sh.stages {
        for bu in &st.bundles {
            image(&bu.image);
            for p in bu.anim.iter().flat_map(|a| &a.paths) {
                image(&ImageRef::Path(p.clone()));
            }
        }
    }
    SortEntry {
        sort: sh.sort,
        images: sh.sort_images.as_ref().map(|imgs| {
            let mut at = |i: usize| imgs.get(i).map_or(0, &mut *image);
            [at(0), at(1)]
        }),
    }
}

/// Each entry's place in retail's sorted shader list when registered in
/// this order. R_SortNewShader walks back from the end and puts the new
/// shader after the first one that sorts lower, or sorts the same with a
/// lower bundle 0 image, or the same bundle 0 image and a bundle 1 image
/// no higher; it passes everything else, a stageless shader and a missing
/// bundle 1 included.
fn sorted_places(entries: &[SortEntry]) -> Vec<u32> {
    let mut list: Vec<usize> = Vec::with_capacity(entries.len());
    for (n, new) in entries.iter().enumerate() {
        let [b0, b1] = new.images.unwrap_or([0, 0]);
        let mut at = list.len();
        while at > 0 {
            let e = &entries[list[at - 1]];
            if e.sort < new.sort {
                break;
            }
            if e.sort == new.sort
                && let Some([e0, e1]) = e.images
            {
                let b0_holds = e0 == 0 || e0 == b0;
                if e0 != 0 && e0 < b0 {
                    break;
                }
                if b0_holds && e1 != 0 && e1 <= b1 {
                    break;
                }
            }
            at -= 1;
        }
        list.insert(at, n);
    }
    let mut place = vec![0; entries.len()];
    for (p, &e) in list.iter().enumerate() {
        place[e] = p as u32;
    }
    place
}

/// Each entry's place in a CoD4 frame's order: by sort, then the first
/// place in 1.7's lit draw order (`MapWorld::soup_order`) any of the
/// batch's soups holds, so a batch draws where retail's list reaches its
/// first surface and the primary light groups the materials as 1.7's key
/// does (cod4-world.md 12). Entries past the world's batches keep their
/// registration order after them.
fn cod4_places(entries: &[SortEntry], ranges: &[Option<IndexRange>], order: &[u64]) -> Vec<u32> {
    let mut first = vec![u64::MAX; entries.len()];
    for (r, &o) in ranges.iter().zip(order) {
        if let Some(f) = r.and_then(|r| first.get_mut(r.batch as usize)) {
            *f = (*f).min(o);
        }
    }
    let mut by: Vec<usize> = (0..entries.len()).collect();
    by.sort_by(|&a, &b| {
        let (ea, eb) = (&entries[a], &entries[b]);
        ea.sort
            .total_cmp(&eb.sort)
            .then(first[a].cmp(&first[b]))
            .then(a.cmp(&b))
    });
    let mut place = vec![0; entries.len()];
    for (p, &e) in by.iter().enumerate() {
        place[e] = p as u32;
    }
    place
}

/// Retail's image numbers in load order (`image+0x58`, CoDMP.exe
/// 0x4ec310: the creation count plus 0x400); 0 stays "no image".
#[derive(Default)]
struct ImageNumbers(HashMap<String, u32>);

impl ImageNumbers {
    fn number(&mut self, name: &str) -> u32 {
        let mut key = name.to_ascii_lowercase();
        for ext in [".tga", ".jpg", ".dds"] {
            if let Some(stem) = key.strip_suffix(ext) {
                key.truncate(stem.len());
                break;
            }
        }
        let next = self.0.len() as u32 + 1;
        *self.0.entry(key).or_insert(next)
    }
}

/// One entry of the frame's merged draw list.
#[derive(Copy, Clone, Debug, PartialEq)]
enum SortedItem {
    World(usize),
    /// A run of impact marks (`mark_pass::MarkPass::runs`).
    Mark(usize),
    /// An entity or viewmodel surface.
    Model(usize),
    Fx(usize),
}

/// Merges the world's draws, the mark runs, the entity and viewmodel
/// surfaces and the fx runs, each list already in sort order, by material
/// sort. On a tie the world goes first, then the marks, the models and the
/// fx: the world's materials and images register at map load, before the
/// cgame's (R_SortNewShader breaks ties on image numbers, 0x4fb790). The
/// marks' place among the cgame's lists is vcod's guess.
fn merge_by_sort(world: &[f32], marks: &[f32], models: &[f32], fx: &[f32]) -> Vec<SortedItem> {
    let lists = [world, marks, models, fx];
    let mut at = [0usize; 4];
    let mut out = Vec::with_capacity(lists.iter().map(|l| l.len()).sum());
    loop {
        let mut best: Option<usize> = None;
        for l in 0..4 {
            if at[l] < lists[l].len() && best.is_none_or(|b| lists[l][at[l]] < lists[b][at[b]]) {
                best = Some(l);
            }
        }
        let Some(l) = best else {
            break;
        };
        out.push(match l {
            0 => SortedItem::World(at[l]),
            1 => SortedItem::Mark(at[l]),
            2 => SortedItem::Model(at[l]),
            _ => SortedItem::Fx(at[l]),
        });
        at[l] += 1;
    }
    out
}

/// What a merged list's last draw left bound.
enum Emit {
    None,
    World {
        set: usize,
        bound: Option<Bound>,
        groups: WorldGroups,
    },
    Ent {
        /// The `DynamicPass::models` index whose buffers are bound.
        model: usize,
        bound: EntBound,
    },
    Gun {
        part: u32,
        bound: EntBound,
    },
    Fx {
        blend: FxBlend,
        soft: bool,
    },
}

/// Copies the visible `ranges` out of `src` batch by batch into `out`, one
/// `DrawRange` per batch that got anything. `per_batch` is scratch, kept
/// between frames.
pub(crate) fn gather(
    src: &[u32],
    ranges: impl IntoIterator<Item = IndexRange>,
    batch_count: usize,
    out: &mut Vec<u32>,
    per_batch: &mut Vec<Vec<IndexRange>>,
) -> Vec<DrawRange> {
    per_batch.resize_with(batch_count, Vec::new);
    for b in per_batch.iter_mut() {
        b.clear();
    }
    for r in ranges {
        per_batch[r.batch as usize].push(r);
    }
    out.clear();
    let mut draws = Vec::new();
    for (batch, rs) in per_batch.iter().enumerate() {
        if rs.is_empty() {
            continue;
        }
        let first = out.len() as u32;
        for r in rs {
            out.extend_from_slice(&src[r.first as usize..(r.first + r.count) as usize]);
        }
        draws.push(DrawRange {
            batch: batch as u32,
            first,
            count: out.len() as u32 - first,
        });
    }
    draws
}

#[derive(Clone)]
struct VmSurface {
    first_index: u32,
    index_count: u32,
    /// Surfaces share one vertex buffer. A model whose vertices all fit
    /// u16 indices has its indices rebased and every `base_vertex` 0, so
    /// neighbouring surfaces on one skin draw as one range
    /// ([`ent_run`]); a bigger one keeps each surface's indices unrebased.
    base_vertex: i32,
    /// One bind group per distinct skin of the model.
    bind_group: usize,
    /// Its index in the `model::Surface`s it was uploaded from (an empty
    /// one uploads nothing).
    src: u32,
}

#[derive(Clone)]
struct VmModel {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    bind_groups: Vec<wgpu::BindGroup>,
    surfaces: Vec<VmSurface>,
    /// Per surface, its skin's name: the key of its material.
    skins: Vec<String>,
    bone_buf: wgpu::Buffer,
    bone_bg: wgpu::BindGroup,
}

/// Built up front so the shader is validated at startup even with no viewmodel set.
struct VmPass {
    uniform_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    skin_layout: wgpu::BindGroupLayout,
    bone_layout: wgpu::BindGroupLayout,
    models: Vec<Rc<VmModel>>,
    /// Per model, per surface, its material; empty until resolved against
    /// this map's entity materials.
    skins: Vec<Vec<Rc<EntSkin>>>,
    /// Every viewmodel part uploaded so far, keyed by [`vm_cache_key`], so a
    /// weapon switch back to a gun (and every gun's shared hands) skips the
    /// upload. Only one viewmodel draws at a time, so parts can share their
    /// bone buffer.
    cache: HashMap<String, Rc<VmModel>>,
    /// Parts uploaded inside `set_viewmodel`, the draw's own first uses:
    /// none once a level's view weapons are preloaded (cod2-client-perf.md,
    /// "The view weapon's first draw").
    first_uses: usize,
}

/// LOD name plus skins: the hands files share one mesh and differ only in
/// their sleeve skins.
fn vm_cache_key(m: &model::Model) -> String {
    format!("{}|{}", m.name, m.materials().join("|"))
}

/// Each player is 4-7 part instances, so 32 players plus corpses and dropped
/// weapons land at 200-280.
const MAX_DYNAMIC_INSTANCES: usize = 512;
pub(crate) const DYNAMIC_INSTANCE_STRIDE: u64 = 80;
/// Unskinned instances point at a shared identity block of this many matrices.
pub const MAX_INSTANCE_BONES: usize = 128;

/// Index into the renderer's dynamic model table. `pub(crate)` so other
/// modules' tests can build a dummy handle without a GPU.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelHandle(pub(crate) usize);

/// `bones: None` draws the baked bind pose; `Some` is truncated at [`MAX_INSTANCE_BONES`].
pub struct DynamicModelInstance {
    pub model: ModelHandle,
    pub transform: glam::Mat4,
    pub bones: Option<Vec<glam::Mat4>>,
    /// Where the light grid is sampled: the refEntity's lighting origin.
    pub light_origin: glam::Vec3,
    /// The material clock in seconds when the refEntity carries its own
    /// `shaderTime` (a UO vehicle's treads and tyres); `None` runs the
    /// frame's.
    pub shader_time: Option<f32>,
}

/// Per-instance vertex data. `bone_base` 0 is the shared identity block.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub transform: [f32; 16],
    pub bone_base: u32,
    /// Index into the frame's light sets.
    pub light_set: u32,
    /// Seconds this instance's material clock runs ahead of the frame's.
    pub time_offset: f32,
    pub _pad: u32,
}

// The instance vertex layout below hardcodes this stride.
const _: () = assert!(std::mem::size_of::<InstanceRaw>() == 80);

/// Bone buffer slot 0 is a [`MAX_INSTANCE_BONES`]-matrix identity block and
/// `None` bone sets point there. The `usize` per tuple is the model index, unused here.
#[allow(clippy::type_complexity)]
pub fn pack_instances(
    instances: &[(usize, [f32; 16], Option<&[glam::Mat4]>)],
) -> (Vec<InstanceRaw>, Vec<[f32; 16]>) {
    static BONE_CAP_WARNED: std::sync::Once = std::sync::Once::new();

    let mut mats: Vec<[f32; 16]> = vec![glam::Mat4::IDENTITY.to_cols_array(); MAX_INSTANCE_BONES];
    let mut raw = Vec::with_capacity(instances.len());
    for &(_, transform, bones) in instances {
        let bone_base = match bones {
            None => 0u32,
            Some(bones) => {
                let base = mats.len() as u32;
                let bones = if bones.len() > MAX_INSTANCE_BONES {
                    BONE_CAP_WARNED.call_once(|| {
                        log::warn!(
                            "dynamic instance bone set exceeds {MAX_INSTANCE_BONES} bones; truncating"
                        );
                    });
                    &bones[..MAX_INSTANCE_BONES]
                } else {
                    bones
                };
                mats.extend(bones.iter().map(glam::Mat4::to_cols_array));
                base
            }
        };
        raw.push(InstanceRaw {
            transform,
            bone_base,
            light_set: 0,
            time_offset: 0.0,
            _pad: 0,
        });
    }
    (raw, mats)
}

/// `bone_sets[i]` belongs to `set_viewmodel` model i. Absent or empty leaves
/// that model's GPU buffer as it was.
pub struct VmDraw {
    pub transform: glam::Mat4,
    /// World-space lighting origin: the eye, `ps.origin` plus
    /// `viewHeightCurrent` (cgame 0x30036cf0).
    pub light_origin: glam::Vec3,
    /// Horizontal degrees, the world view's own.
    pub fov_x: f32,
    pub bone_sets: Vec<Vec<glam::Mat4>>,
}

pub struct Frame {
    pub view_proj: glam::Mat4,
    /// Camera position, the cell the visibility walk starts from.
    pub eye: glam::Vec3,
    /// Unit view forward. Fog depth is measured along it (eye-space Z),
    /// matching retail without GL_NV_fog_distance.
    pub fwd: glam::Vec3,
    /// The view matrix's up (`camera::up_hint`); with `fwd` it takes world
    /// lights into the viewmodel's view space.
    pub up: glam::Vec3,
    /// Seconds since start; drives tcMod and wave animation in the stage shaders.
    pub time: f32,
    /// The refdef's horizontal fov in degrees; the LOD pick reads it.
    pub fov_x: f32,
    pub cull: CullMode,
    /// Debug-HUD lines, top line first; empty when the overlay is off.
    pub hud_lines: Vec<String>,
}

/// Drawn/total per frame for the F3 `vis` line.
#[derive(Clone, Debug, Default)]
pub struct VisCounts {
    pub mode: Option<CullMode>,
    pub cells: (usize, usize),
    pub soups: (usize, usize),
    pub tris: (usize, usize),
    pub props: (usize, usize),
    /// Occluder volumes built and portals hidden by them this frame (On mode).
    pub occluders: usize,
    pub portals_occluded: usize,
    pub gather_ms: f32,
}

/// Matches `FxLights` in shader.wgsl (std140, vec4 aligned); `vs_prop` reads it.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FxLightsUniform {
    /// xyz position, w radius; zero radius means unused.
    pos_radius: [[f32; 4]; MAX_LIGHTS],
    /// rgb, w unused.
    color: [[f32; 4]; MAX_LIGHTS],
}

impl FxLightsUniform {
    fn from_lights(lights: &[FxLight]) -> Self {
        let mut pos_radius = [[0.0f32; 4]; MAX_LIGHTS];
        let mut color = [[0.0f32; 4]; MAX_LIGHTS];
        for (i, l) in lights.iter().take(MAX_LIGHTS).enumerate() {
            pos_radius[i] = [l.pos[0], l.pos[1], l.pos[2], l.radius];
            color[i] = [l.rgb[0], l.rgb[1], l.rgb[2], 0.0];
        }
        Self { pos_radius, color }
    }
}

/// What a CoD4 vision set draws over the level (cod4-world.md 11).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelLook {
    pub film: Option<crate::post::Film>,
    pub glow: Option<crate::cod2_glow::Cod4Glow>,
}

/// The fog values as currently displayed.
#[derive(Copy, Clone, Default, PartialEq)]
struct FogLive {
    linear: bool,
    color: [f32; 3],
    density: f32,
    near: f32,
    far: f32,
    start: f32,
}

impl FogLive {
    fn of(p: &FogParams) -> Self {
        FogLive {
            linear: p.is_linear(),
            color: p.color,
            density: p.density,
            near: p.near,
            far: p.far,
            start: p.start,
        }
    }

    /// 1 = GL_EXP, 2 = GL_LINEAR — the WGSL Camera mode encoding; the off
    /// state is carried by [`FogState::set`], not here.
    fn mode(&self) -> f32 {
        if self.linear { 2.0 } else { 1.0 }
    }
}

/// Global fog from configstring 12, faded toward each new target. Same-mode
/// transitions lerp over the script's fade time; cross-mode changes snap and
/// linear fog clears to the fog colour — RTCW-MP tr_main.c R_SetFog /
/// R_AddFogInterpolation.
#[derive(Default)]
struct FogState {
    set: bool,
    /// CoD2's world: fog runs on the radial eye distance (fogcalc.hlsl's
    /// `useRadialFog`), not CoD1's eye-space depth.
    radial: bool,
    /// The last frame's time, where [`Renderer::set_fog_now`] starts a fade.
    clock: f32,
    live: FogLive,
    from: FogLive,
    /// target params, transition start (frame seconds), duration (s)
    target: Option<(FogParams, f32, f32)>,
}

impl FogState {
    fn set_target(&mut self, p: FogParams, now: f32) {
        // No fade, first fog, or a mode change snaps like retail.
        let snap = !self.set || p.fade_ms == 0 || self.live.linear != p.is_linear();
        self.from = self.live;
        self.target = Some((p, now, p.fade_ms as f32 / 1000.0));
        self.set = true;
        if snap {
            self.finish();
        }
    }

    fn clear(&mut self) {
        self.set = false;
        self.target = None;
    }

    fn finish(&mut self) {
        if let Some((p, _, _)) = self.target.take() {
            self.live = FogLive::of(&p);
        }
    }

    fn advance(&mut self, now: f32) {
        self.clock = now;
        if self.target.is_none() {
            return;
        }
        let (_, t0, dur) = self.target.unwrap();
        let k = ((now - t0) / dur.max(f32::EPSILON)).clamp(0.0, 1.0);
        let to = match self.target {
            Some((p, _, _)) => FogLive::of(&p),
            None => return,
        };
        let f = &self.from;
        self.live = FogLive {
            linear: to.linear,
            color: [
                f.color[0] + (to.color[0] - f.color[0]) * k,
                f.color[1] + (to.color[1] - f.color[1]) * k,
                f.color[2] + (to.color[2] - f.color[2]) * k,
            ],
            density: f.density + (to.density - f.density) * k,
            near: f.near + (to.near - f.near) * k,
            far: f.far + (to.far - f.far) * k,
            start: f.start + (to.start - f.start) * k,
        };
        if k >= 1.0 {
            self.target = None;
        }
    }
}

#[cfg(test)]
mod fx_lights_uniform_tests {
    use super::*;

    #[test]
    fn is_256_bytes_std140() {
        assert_eq!(std::mem::size_of::<FxLightsUniform>(), 256);
    }

    #[test]
    fn empty_list_zeroes_every_slot() {
        let u = FxLightsUniform::from_lights(&[]);
        assert_eq!(u.pos_radius, [[0.0; 4]; MAX_LIGHTS]);
        assert_eq!(u.color, [[0.0; 4]; MAX_LIGHTS]);
    }

    #[test]
    fn packs_position_radius_and_color_into_matching_slots() {
        let lights = vec![
            FxLight {
                pos: [1.0, 2.0, 3.0],
                radius: 4.0,
                rgb: [0.5, 0.6, 0.7],
            },
            FxLight {
                pos: [10.0, 20.0, 30.0],
                radius: 40.0,
                rgb: [0.1, 0.2, 0.3],
            },
        ];
        let u = FxLightsUniform::from_lights(&lights);
        assert_eq!(u.pos_radius[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(u.color[0], [0.5, 0.6, 0.7, 0.0]);
        assert_eq!(u.pos_radius[1], [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(u.color[1], [0.1, 0.2, 0.3, 0.0]);
        for i in 2..MAX_LIGHTS {
            assert_eq!(u.pos_radius[i], [0.0; 4]);
        }
    }
}

// ---- shader-script stage machinery (consumed by Tasks 7-8) ----

pub const STAGE_FLAG_VERTEX_RGB: u32 = 1;
pub const STAGE_FLAG_VERTEX_ALPHA: u32 = 2;
pub const STAGE_FLAG_BUNDLE1_LIGHTMAP: u32 = 4;
pub const STAGE_FLAG_BUNDLE0_LIGHTMAP: u32 = 8;
// alphaFunc encoding in bits 16..32: 0 = none, GT0 = 16, LT128 = 32, GE128 = both
pub const STAGE_FLAG_ALPHAFUNC_GT0: u32 = 16;
pub const STAGE_FLAG_ALPHAFUNC_LT128: u32 = 32;
pub const STAGE_FLAG_ALPHAFUNC_GE128: u32 = 48;
pub const STAGE_FLAG_BUNDLE0_VECTOR: u32 = 64;
pub const STAGE_FLAG_BUNDLE1_VECTOR: u32 = 128;
/// Sky dome draws only: add the view origin (StageParams.eye_off) to vertices.
pub const STAGE_FLAG_EYE_OFFSET: u32 = 256;
/// Sky dome draws only: fog applies in exp mode, never in linear mode
/// (RTCW-MP tr_main.c R_SetFog sets drawsky per mode).
pub const STAGE_FLAG_SKY: u32 = 512;
/// `rgbGen vertex` scales the vertex rgb by identityLight; `exactVertex`
/// stays raw (RTCW-MP tr_shade.c CGEN_VERTEX vs CGEN_EXACT_VERTEX).
pub const STAGE_FLAG_VERTEX_RGB_HALF: u32 = 1024;
/// The stage blends onto `GL_ONE`: GL_State loads a black fog colour for
/// it (CoDMP.exe 0x4d2ad0), so fog fades it instead of greying it.
pub const STAGE_FLAG_FOG_BLACK: u32 = 2048;
/// The tint rgb scales by identityLight, which the camera uniform carries:
/// `identityLighting`, `constLighting` and `wave` (0x4ffa60).
pub const STAGE_FLAG_TINT_LIGHT: u32 = 4096;
/// `lightingDiffuse`: the entity pass lights the stage from its light set
/// (`dynamic_model.wgsl`); the world draws it as `identityLighting`.
pub const STAGE_FLAG_LIT: u32 = 8192;
/// `tcGen environment` on bundle 0 / 1: UVs from the view vector reflected
/// about the normal in the model's frame (RB_CalcEnvironmentTexCoords,
/// CoDMP.exe 0x514d40), before the bundle's tcMods.
pub const STAGE_FLAG_BUNDLE0_ENV: u32 = 16384;
pub const STAGE_FLAG_BUNDLE1_ENV: u32 = 32768;
/// Bundle 1's texture environment, a `nextbundle`'s `blendFunc`: `add`
/// adds it, `blend` decals it; neither multiplies (shader doc 3).
pub const STAGE_FLAG_ENV1_ADD: u32 = 65536;
pub const STAGE_FLAG_ENV1_DECAL: u32 = 131072;
/// The sprite deforms over vertices `vcod_common::deform` encoded at load.
pub const STAGE_FLAG_AUTOSPRITE: u32 = 1 << 20;
pub const STAGE_FLAG_AUTOSPRITE2: u32 = 1 << 21;
/// `nofog`: the stage draws unfogged.
pub const STAGE_FLAG_NOFOG: u32 = 1 << 22;

/// Sorted past `vc_shadows`' mask, the stage lays its own colour over what
/// is behind it and darkens that colour itself (`shadow_receive.wgsl`).
pub const STAGE_FLAG_SUN_SHADOW: u32 = 1 << 23;
/// Bundle 0's image clamps on `s` / `t` (`tex_samplers.wgsl`); bundle 1's
/// two bits sit two higher (`STAGE_FLAG_CLAMP1_S`, `_T`).
pub const STAGE_FLAG_CLAMP_S: u32 = 1 << 24;
pub const STAGE_FLAG_CLAMP_T: u32 = 1 << 25;
pub const STAGE_FLAG_CLAMP1_S: u32 = 1 << 26;
pub const STAGE_FLAG_CLAMP1_T: u32 = 1 << 27;
/// A CoD2 model's `lprobe*` stage (`Material::probe`), lit per pixel by
/// cod2_model.wgsl, and its `_a`, `_s`, `_dtl` and `_scroll` switches.
pub const STAGE_FLAG_COD2_PROBE: u32 = 1 << 28;
pub const STAGE_FLAG_COD2_ALPHA: u32 = 1 << 18;
pub const STAGE_FLAG_COD2_SPEC: u32 = 1 << 29;
pub const STAGE_FLAG_COD2_DETAIL: u32 = 1 << 30;
pub const STAGE_FLAG_COD2_SCROLL: u32 = 1 << 19;
/// A CoD2 `objective` set's stage (`ModelLight::Objective`).
pub const STAGE_FLAG_OBJECTIVE: u32 = 1 << 31;

/// `StageParams::deform_kinds` codes: retail's deform numbers
/// (CoDMP.exe 0x5149b0's switch), 0 for an empty slot.
pub const DEFORM_WAVE: u32 = 1;
pub const DEFORM_FLAP_S: u32 = 2;
pub const DEFORM_FLAP_T: u32 = 3;
pub const DEFORM_NORMAL: u32 = 4;
pub const DEFORM_BULGE: u32 = 6;
pub const DEFORM_MOVE: u32 = 7;
/// `StageParams::deform_kinds[3]`, past the three deform slots: a CoD2
/// `lprobe_flag*` stage, whose vertices `dynamic_model.wgsl` waves
/// (cod2-materials.md 9.7).
pub const DEFORM_COD2_FLAG: u32 = 1;
/// `StageParams::deform_kinds[3]`: a CoD4 reflex sight's dot
/// (`Material::reflex`), which `viewmodel.wgsl` projects off the view's
/// angle to the lens, its `detailScale` in `eye_off` (cod4-client-view.md
/// 4.5).
pub const DEFORM_COD4_REFLEX: u32 = 2;
/// `StageParams::deform_kinds[3]`: a CoD4 `lp_flag_*` stage, whose
/// vertices `dynamic_model.wgsl` waves (`cod4_flag.wgsl`) and whose sun
/// takes `|N.L|` (cod4-world.md 3.4).
pub const DEFORM_COD4_FLAG: u32 = 3;

/// Whether a stage lays its own colour over the frame: no blend, an alpha
/// or a premultiplied blend. An additive stage adds light and a filter
/// (source from the frame's colour, or zero) tints what is behind, which the
/// shadow mask has already darkened.
fn lays_own_colour(blend: &Option<(BlendFactor, BlendFactor)>) -> bool {
    match blend {
        None => true,
        Some((src, dst)) => {
            matches!(src, BlendFactor::One | BlendFactor::SrcAlpha) && *dst != BlendFactor::One
        }
    }
}

/// shader.wgsl with `vc_shadows`' receiver tail and the image samplers.
pub(crate) const WORLD_WGSL: &str = concat!(
    include_str!("shader.wgsl"),
    include_str!("noise.wgsl"),
    include_str!("deform.wgsl"),
    include_str!("shadow_receive.wgsl"),
    include_str!("tex_samplers.wgsl")
);

/// fx.wgsl with the image samplers.
pub(crate) const FX_WGSL: &str =
    concat!(include_str!("fx.wgsl"), include_str!("tex_samplers.wgsl"));

/// sky.wgsl (farbox and sun sprite) with the image samplers.
const SKY_WGSL: &str = concat!(include_str!("sky.wgsl"), include_str!("tex_samplers.wgsl"));

/// Per-stage draw parameters, one dynamic-offset slot per stage batch. WGSL
/// mirror is `StageParams` in shader.wgsl; byte offsets:
/// uv0 @0 and uv1 @24 as mat3x2 (column-major, matching [`bundle_affine`]),
/// turb01 @48, tint @64, flags @80, pad @84..96, then per bundle the two
/// tcGen vector bases, or `tcGen environment`'s s and t axes, padded to
/// vec4: vec0_s @96, vec0_t @112, vec1_s @128, vec1_t @144,
/// the sky dome's view-origin offset @160, then the deforms: wave @176,
/// bulge @192, move @208, normal @224, then the falloff and the CoD4
/// world fx (`fx`) @272. The entity, viewmodel and shadow
/// shaders declare only the prefix they read.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StageParams {
    pub(crate) uv0: [f32; 6],
    pub(crate) uv1: [f32; 6],
    pub(crate) turb01: [f32; 4],
    pub(crate) tint: [f32; 4],
    pub(crate) flags: u32,
    /// Bundle 0's lone `tcMod scroll` (s and t per second) or `rotate`
    /// (degrees per second), which an instance with its own material clock
    /// (`InstanceRaw::time_offset`) runs further; zero otherwise.
    pub(crate) uv0_rate: [f32; 3],
    pub(crate) vec0_s: [f32; 4],
    pub(crate) vec0_t: [f32; 4],
    pub(crate) vec1_s: [f32; 4],
    pub(crate) vec1_t: [f32; 4],
    pub(crate) eye_off: [f32; 4],
    /// The shader's deform records in script order, which `deform.wgsl`
    /// runs one after another: wave and flap `[base, amp, phase + t *
    /// rate, spread]`, bulge `[width, height, t * speed, 0]`, move this
    /// frame's offset, normal `[amp, t * freq, 0, 0]`.
    pub(crate) deform: [[f32; 4]; 3],
    /// Each record's `DEFORM_*` code; the fourth is padding.
    pub(crate) deform_kinds: [u32; 4],
    /// A `Material::falloff` stage's falloff: `[begin rgb, scale]`,
    /// `[end rgb, bias]`; the vertex shader applies it while the scale is
    /// nonzero (no flag bit is left), its kind in `eye_off.w`.
    pub(crate) falloff: [[f32; 4]; 2],
    /// A CoD4 world or static model surface's `cod4_worldfx` draw: a
    /// `zfeather` set's `featherParms.x`, then a distortion set's
    /// `distortionScale`; zero otherwise.
    pub(crate) fx: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<StageParams>() == 288);
pub(crate) const STAGE_PARAMS_SIZE: u64 = std::mem::size_of::<StageParams>() as u64;

/// `tcGen environment`'s s and t axes, which the reflection is dotted
/// with: the drawing frame's y and z, or a static prop's model axes.
fn env_axes(shader: &Material) -> [[f32; 4]; 2] {
    let [s, t] = shader
        .model_axes
        .unwrap_or([[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    [[s[0], s[1], s[2], 0.0], [t[0], t[1], t[2], 0.0]]
}

const UV_AFFINE_IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Evaluates one stage's draw parameters at time `t`: bundle affines/turb,
/// tint from rgbGen/alphaGen, and the flag bits the WGSL reads. `None` when
/// `idx` is out of range or the stage has no bundles.
pub(crate) fn stage_params(shader: &Material, idx: usize, t: f32) -> Option<StageParams> {
    let st = shader.stages.get(idx)?;
    let b0 = st.bundles.first()?;
    let b1 = st.bundles.get(1);

    let mut flags = match &st.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => STAGE_FLAG_ALPHAFUNC_GT0,
        Some(AlphaFunc::Lt128) => STAGE_FLAG_ALPHAFUNC_LT128,
        Some(AlphaFunc::Ge128) => STAGE_FLAG_ALPHAFUNC_GE128,
    };
    // the lightmap and the dlight image share the lightmap's coordinates
    if b0.takes_lightmap_coords() {
        flags |= STAGE_FLAG_BUNDLE0_LIGHTMAP;
    }
    if b1.is_some_and(Bundle::takes_lightmap_coords) {
        flags |= STAGE_FLAG_BUNDLE1_LIGHTMAP;
    }
    // each bundle wraps its own axes (image flags 0x10, 0x20; 0x4ec6e9)
    let clamp_bits = |c: Clamp, s, t| (if c.s { s } else { 0 }) | (if c.t { t } else { 0 });
    flags |= clamp_bits(b0.clamp, STAGE_FLAG_CLAMP_S, STAGE_FLAG_CLAMP_T);
    if let Some(b1) = b1 {
        flags |= clamp_bits(b1.clamp, STAGE_FLAG_CLAMP1_S, STAGE_FLAG_CLAMP1_T);
    }
    match b1.map(|b| b.env) {
        Some(TexEnv::Add) => flags |= STAGE_FLAG_ENV1_ADD,
        Some(TexEnv::Decal) => flags |= STAGE_FLAG_ENV1_DECAL,
        _ => {}
    }

    let mut rgb = [1.0f32; 3];
    // a table wave clamps after its identityLight scale (0x514c00), which
    // the shader applies, so it reaches the shader unclamped above
    let mut rgb_max = 1.0f32;
    // identityLight (1 / (1 << overbrightBits)) scales the lit gens in the
    // shader; identity, const and exactVertex pass through unscaled
    // (docs/research/cod11-gamma.md, section 5).
    match &st.rgb_gen {
        RgbGen::Vertex => {
            flags |= STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_RGB_HALF;
        }
        RgbGen::ExactVertex => flags |= STAGE_FLAG_VERTEX_RGB,
        RgbGen::Identity => {}
        RgbGen::IdentityLighting => flags |= STAGE_FLAG_TINT_LIGHT,
        RgbGen::LightingDiffuse => flags |= STAGE_FLAG_TINT_LIGHT | STAGE_FLAG_LIT,
        // the entity's shaderRGBA, which no stock entity material reads
        RgbGen::LightingPrecalc => {}
        RgbGen::Const(c) => rgb = *c,
        RgbGen::ConstLighting(c) => {
            flags |= STAGE_FLAG_TINT_LIGHT;
            rgb = *c;
        }
        RgbGen::Wave(w) => {
            // the noise glow skips identityLight (0x514c00)
            if w.form != WaveForm::Noise {
                flags |= STAGE_FLAG_TINT_LIGHT;
                rgb_max = f32::MAX;
            }
            rgb = [rgb_wave_value(w, t); 3];
        }
    }
    let alpha = match &st.alpha_gen {
        // `rgbGen vertex` and `exactVertex` imply it unless an alphaGen was
        // set first (the parser, shader doc 3)
        AlphaGen::Vertex => {
            flags |= STAGE_FLAG_VERTEX_ALPHA;
            1.0
        }
        AlphaGen::Const(v) => *v,
        AlphaGen::Wave(w) => wave_value(w, t),
        // the world entity's shaderRGBA is opaque white; no stock entity
        // material reads an entity alpha
        AlphaGen::Identity | AlphaGen::Entity => 1.0,
    };

    let mut p = StageParams {
        uv0: bundle_affine(&b0.tcmods, t),
        uv1: b1.map_or(UV_AFFINE_IDENTITY, |b| bundle_affine(&b.tcmods, t)),
        // [amp0, now0, amp1, now1]; the VS adds amp*sin(pos_axis/1024 + now),
        // s on x+z, t on y (RB_CalcTurbulentTexCoords)
        turb01: {
            let tb0 = bundle_turb(&b0.tcmods, t);
            let tb1 = b1.map_or([0.0; 4], |b| bundle_turb(&b.tcmods, t));
            [tb0[0], tb0[1], tb1[0], tb1[1]]
        },
        // Retail writes these as colour bytes (Q3's EvalWaveFormClamped).
        tint: [
            rgb[0].clamp(0.0, rgb_max),
            rgb[1].clamp(0.0, rgb_max),
            rgb[2].clamp(0.0, rgb_max),
            alpha.clamp(0.0, 1.0),
        ],
        flags,
        uv0_rate: match b0.tcmods.as_slice() {
            [TcMod::Scroll(s, t)] => [*s, *t, 0.0],
            [TcMod::Rotate(dps)] => [0.0, 0.0, *dps],
            _ => [0.0; 3],
        },
        vec0_s: [0.0; 4],
        vec0_t: [0.0; 4],
        vec1_s: [0.0; 4],
        vec1_t: [0.0; 4],
        eye_off: [0.0; 4],
        deform: [[0.0; 4]; 3],
        deform_kinds: [0; 4],
        falloff: shader.falloff.map_or([[0.0; 4]; 2], |f| {
            let [b, e] = [f.begin, f.end];
            [[b[0], b[1], b[2], f.scale], [e[0], e[1], e[2], f.bias]]
        }),
        fx: {
            let feather = shader.feather.filter(|_| shader.feather_view_depth);
            let d = shader.distortion.unwrap_or([0.0; 2]);
            [feather.unwrap_or(0.0), d[0], d[1], 0.0]
        },
    };
    match &b0.tc_gen {
        TcGen::Base => {}
        TcGen::Vector(v) => {
            flags |= STAGE_FLAG_BUNDLE0_VECTOR;
            p.vec0_s = [v[0], v[1], v[2], 0.0];
            p.vec0_t = [v[3], v[4], v[5], 0.0];
        }
        TcGen::Environment => {
            flags |= STAGE_FLAG_BUNDLE0_ENV;
            [p.vec0_s, p.vec0_t] = env_axes(shader);
        }
    }
    match b1.map(|b| &b.tc_gen) {
        None | Some(TcGen::Base) => {}
        Some(TcGen::Vector(v)) => {
            flags |= STAGE_FLAG_BUNDLE1_VECTOR;
            p.vec1_s = [v[0], v[1], v[2], 0.0];
            p.vec1_t = [v[3], v[4], v[5], 0.0];
        }
        Some(TcGen::Environment) => {
            flags |= STAGE_FLAG_BUNDLE1_ENV;
            [p.vec1_s, p.vec1_t] = env_axes(shader);
        }
    }
    // a wave of rate 0 is one phase for every vertex (0x513570, 0x5136e0)
    let wave = |w: &Wave, spread: f32| {
        let spread = if w.freq == 0.0 { 0.0 } else { spread };
        [w.base, w.amp, w.phase + t * w.freq, spread]
    };
    let records = shader.deforms.iter().filter_map(|d| match d {
        Deform::Wave { wave: w, spread } => Some((DEFORM_WAVE, wave(w, *spread))),
        Deform::Flap(f) => Some((
            if f.along_t {
                DEFORM_FLAP_T
            } else {
                DEFORM_FLAP_S
            },
            wave(&f.wave, f.spread),
        )),
        Deform::Bulge(b) => Some((DEFORM_BULGE, [b.width, b.height, t * b.speed, 0.0])),
        Deform::Move { by, wave: w } => {
            let k = wave_value(w, t);
            Some((DEFORM_MOVE, [by[0] * k, by[1] * k, by[2] * k, 0.0]))
        }
        Deform::Normal(n) => Some((DEFORM_NORMAL, [n.amp, t * n.freq, 0.0, 0.0])),
        Deform::Other => None,
    });
    for (i, (kind, rec)) in records.take(3).enumerate() {
        p.deform_kinds[i] = kind;
        p.deform[i] = rec;
    }
    match shader.sprite {
        Some(Sprite::Auto) => flags |= STAGE_FLAG_AUTOSPRITE,
        Some(Sprite::Auto2) => flags |= STAGE_FLAG_AUTOSPRITE2,
        None => {}
    }
    if shader.nofog {
        flags |= STAGE_FLAG_NOFOG;
    }
    if matches!(st.blend, Some((_, BlendFactor::One))) {
        flags |= STAGE_FLAG_FOG_BLACK;
    }
    if shader.sort > SORT_DECAL && lays_own_colour(&st.blend) {
        flags |= STAGE_FLAG_SUN_SHADOW;
    }
    // an entity's CoD2 probe stage; the world and props draw it as before
    if let Some(pr) = shader
        .probe
        .filter(|_| st.rgb_gen == RgbGen::LightingDiffuse)
    {
        flags |= STAGE_FLAG_COD2_PROBE;
        let bit = |on: bool, f: u32| if on { f } else { 0 };
        flags |= bit(pr.alpha, STAGE_FLAG_COD2_ALPHA)
            | bit(pr.specular, STAGE_FLAG_COD2_SPEC)
            | bit(pr.detail, STAGE_FLAG_COD2_DETAIL)
            | bit(pr.scroll, STAGE_FLAG_COD2_SCROLL);
        if pr.flag {
            p.deform_kinds[3] = DEFORM_COD2_FLAG;
        }
        if shader.phong.and_then(|p| p.cod4).is_some_and(|c| c.flag) {
            p.deform_kinds[3] = DEFORM_COD4_FLAG;
        }
        // the sky's eye offset is free on a model stage
        p.eye_off = [pr.detail_scale[0], pr.detail_scale[1], 0.0, 0.0];
        // a CoD4 lit model set's envMapParms, normal map, ambient and sun
        // shadow map switches (cod2_model.wgsl's `cod4_model`) in the tcGen vector
        // slots, which a model stage never reads
        if let Some(c) = shader.phong.and_then(|p| p.cod4) {
            p.vec0_s = c.env;
            let ambient = c.light == vcod_common::material::Cod4Light::GridAmbient;
            p.vec0_t = [
                f32::from(u8::from(c.normal_map)),
                f32::from(u8::from(ambient)),
                f32::from(u8::from(c.sun_shadow)),
                0.0,
            ];
        }
    }
    if shader.model_light == ModelLight::Objective {
        flags |= STAGE_FLAG_OBJECTIVE;
    }
    // CoD4's falloff kind and eye offset (cod4-world.md 9.5): a CoD4
    // stage is neither the sky nor a CoD2 probe, the two other eye_off users
    if let Some(f) = shader.falloff {
        p.eye_off[3] = match f.kind {
            FalloffKind::Distance => 0.0,
            FalloffKind::Angle => 1.0,
        };
    }
    if let Some(e) = shader.eye_offset {
        p.eye_off[2] = e;
    }
    // a reflex sight's detailScale; it carries no falloff or eye offset
    if let Some([sx, sy]) = shader.reflex {
        p.deform_kinds[3] = DEFORM_COD4_REFLEX;
        p.eye_off[0] = sx;
        p.eye_off[1] = sy;
    }
    p.flags = flags;
    Some(p)
}

/// A CoD4 map's entity lighting: the grid lookup, the sun and the spot
/// and omni lights.
struct Cod4EntityLight {
    lighter: vcod_common::mapworld::EntityLighter,
    sun_dir: glam::Vec3,
    sun_colour: glam::Vec3,
    lights: Vec<Option<vcod_common::mapworld::ModelLightSource>>,
}

impl Cod4EntityLight {
    /// The light set of a model lit from `origin` whose world box is
    /// `bounds` (mins, maxs), naming entity volume block `block`, and the
    /// texels that block takes.
    fn set(
        &self,
        origin: glam::Vec3,
        bounds: [glam::Vec3; 2],
        block: u32,
        identity_light: f32,
    ) -> (GpuLightSet, [[u8; 4]; 64]) {
        let light = (self.lighter.0)(origin.to_array(), bounds.map(|v| v.to_array()));
        let source = self
            .lights
            .get(usize::from(light.primary))
            .and_then(Option::as_ref);
        let set = entity_light::pack_cod4(
            &light,
            block,
            self.sun_dir,
            self.sun_colour,
            source,
            identity_light,
        );
        (set, light.texels)
    }
}

/// A frame-indexed set of bind groups: one per animMap frame, or a single
/// static entry (fps 0 sticks at index 0).
struct AnimFrames {
    fps: f32,
    groups: Vec<wgpu::BindGroup>,
}

impl AnimFrames {
    /// A single, non-animating entry (fps 0 sticks at index 0).
    fn static_(group: wgpu::BindGroup) -> Self {
        Self {
            fps: 0.0,
            groups: vec![group],
        }
    }

    fn bind_group(&self, t: f32) -> &wgpu::BindGroup {
        &self.groups[anim_frame_index(self.fps, self.groups.len(), t)]
    }
}

/// One authored stage expanded over its parent batch's index range. Batches
/// of one material share the params slot per stage.
struct StageBatch {
    /// Dynamic-offset slot of this (material, stage)'s params.
    slot: u32,
    variant: StageVariant,
    two_sided: bool,
    bias: bool,
    /// group(1): bundle-0 image + lightmap page + lightmap sampler.
    mat: AnimFrames,
    /// group(2): params UBO slot + bundle-1 image.
    stage: AnimFrames,
    /// A CoD2 lit draw: the main pass draws it through `WorldGpu::cod2`
    /// in place of the stage; every other pass keeps the stage.
    cod2: Option<cod2::LitBind>,
    /// A CoD4 feathered or distortion stage: the main pass draws it
    /// through `Renderer::world_fx` past the frame's split.
    fx: cod4_worldfx::WorldFx,
}

/// One dlight-pass stage over a parent batch's range, drawn once per light
/// whose gathered range holds that batch.
struct DlightBatch {
    blend: DlightBlend,
    two_sided: bool,
    bias: bool,
    slot: u32,
    mat: AnimFrames,
    stage: AnimFrames,
}

/// One light's draws this frame: its slot in the light uniform, the moved
/// brush model whose camera it draws under (`None` for the world), and the
/// gathered range in `WorldGpu::dlight_index_buf`.
struct DlightRun {
    light: u32,
    moved: Option<usize>,
    draw: DrawRange,
}

/// floor(t * fps) % len, clamped to a valid frame; static or empty anims stick at 0.
fn anim_frame_index(fps: f32, len: usize, t: f32) -> usize {
    if fps <= 0.0 || len == 0 {
        return 0;
    }
    (t.max(0.0) * fps) as usize % len
}

/// Which prebuilt stage pipeline a StageBatch draws with; selected from
/// `ClassedStage` plus the stage's blend pair.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum StageVariant {
    /// Unblended: depth write on, alphafunc discard in fs_stage.
    Opaque,
    /// SrcAlpha/OneMinusSrcAlpha, no depth write.
    Blend,
    /// Blend that kept depth write (explicit depthWrite keyword).
    BlendDepthWrite,
    /// dst factor exactly GL_ONE with src GL_ONE, drawn One/One.
    AdditiveFixed,
    /// Any other additive pair, drawn SrcAlpha/One so source alpha carries.
    AdditiveFaded,
    /// Additive stages that kept depth write (explicit depthWrite keyword).
    AdditiveFixedDepthWrite,
    AdditiveFadedDepthWrite,
    /// A CoD2 multiply decal (`Material::vertex_multiply`): fs_multiply's
    /// factor times the framebuffer, no depth write.
    Multiply,
    /// A CoD4 `distfalloff` stage (`Material::falloff`): (DstColor,
    /// SrcColor), twice the colour times the framebuffer, no depth write.
    Multiply2x,
    /// A CoD4 premultiplied (OneMinusDstColor, One) stage
    /// (`Material::premultiplied`): the colour times its alpha, times the
    /// framebuffer's complement, onto the framebuffer; no depth write.
    AdditiveInvDst,
}
const STAGE_VARIANTS: usize = 10;
/// `StageVariant`s in discriminant order, the stage pipeline table's index.
const STAGE_VARIANT_ORDER: [StageVariant; STAGE_VARIANTS] = [
    StageVariant::Opaque,
    StageVariant::Blend,
    StageVariant::BlendDepthWrite,
    StageVariant::AdditiveFixed,
    StageVariant::AdditiveFaded,
    StageVariant::AdditiveFixedDepthWrite,
    StageVariant::AdditiveFadedDepthWrite,
    StageVariant::Multiply,
    StageVariant::Multiply2x,
    StageVariant::AdditiveInvDst,
];

/// A stage variant's fragment entry in shader.wgsl, blend and depth write.
fn stage_variant_state(v: StageVariant) -> (&'static str, Option<wgpu::BlendState>, bool) {
    let both = |src_factor, dst_factor| wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        },
    };
    use wgpu::BlendFactor as F;
    let one_one = both(F::One, F::One);
    let srcalpha_one = both(F::SrcAlpha, F::One);
    match v {
        StageVariant::Opaque => ("fs_stage", None, true),
        StageVariant::Blend => ("fs_stage", Some(wgpu::BlendState::ALPHA_BLENDING), false),
        StageVariant::BlendDepthWrite => ("fs_stage", Some(wgpu::BlendState::ALPHA_BLENDING), true),
        StageVariant::AdditiveFixed => ("fs_stage", Some(one_one), false),
        StageVariant::AdditiveFaded => ("fs_stage", Some(srcalpha_one), false),
        // explicit depthWrite on additive stages
        StageVariant::AdditiveFixedDepthWrite => ("fs_stage", Some(one_one), true),
        StageVariant::AdditiveFadedDepthWrite => ("fs_stage", Some(srcalpha_one), true),
        // dst * src colour; the destination alpha stays
        StageVariant::Multiply => (
            "fs_multiply",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::Src,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
        // src * a * (1 - dst) + dst; the destination alpha stays
        StageVariant::AdditiveInvDst => (
            "fs_stage_premul",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::OneMinusDst,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
        // src * dst + dst * src; the destination alpha stays
        StageVariant::Multiply2x => (
            "fs_stage",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: F::Dst,
                    dst_factor: F::Src,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: F::Zero,
                    dst_factor: F::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            false,
        ),
    }
}

/// The engine's polygon offsets, `glPolygonOffset(r_offsetfactor,
/// r_offsetunits)` at their registered -1 and -2, doubled for
/// `polygonOffset2` and with no slope for `polygonOffsetConst` (CoDMP.exe
/// 0x4d5991..0x4d5a05, 0x4b3f0a; events doc 9.5).
pub(crate) fn depth_bias(form: PolygonOffset) -> wgpu::DepthBiasState {
    let (slope_scale, constant) = match form {
        PolygonOffset::Single => (-1.0, -2),
        PolygonOffset::Double => (-2.0, -4),
        PolygonOffset::Const => (0.0, -2),
    };
    wgpu::DepthBiasState {
        constant,
        slope_scale,
        clamp: 0.0,
    }
}

fn stage_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

/// Group 1 of the world and stage pipelines: the colour image, the
/// lightmap page, a sampler and the page's block runs.
fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("material layout"),
        entries: &[
            stage_texture_entry(0),
            stage_texture_entry(1),
            // images sample through the camera group's samplers
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // the lightmap page's block runs (lightmap_blocks), read by
            // vc_bicubic's taps
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

fn stage_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    // Group 2 of the stage pipelines: the per-stage params slot plus
    // bundle 1's image (white is bound when bundle 1 has none).
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("stage params layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                },
                count: None,
            },
            stage_texture_entry(1),
        ],
    })
}

/// A pipeline over shader.wgsl's world vertex layout into the scene target:
/// the world, layer, overlay and stage pipelines, and the marks'.
/// `f(0..n)` on scoped threads, in index order. Pipeline creation is
/// thread safe, and a driver that compiles on the calling thread (lavapipe)
/// takes seconds a pipeline for `shader.wgsl`'s stage entries.
pub(crate) fn par_build<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    par_build_on(n, n, f)
}

/// [`par_build`] on at most `workers` threads, each taking the next index
/// as it finishes one: no thread waits on a slow neighbour's batch.
pub(crate) fn par_build_on<T: Send>(
    workers: usize,
    n: usize,
    f: impl Fn(usize) -> T + Sync,
) -> Vec<T> {
    let samples = msaa_samples();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<T>> = (0..n).map(|_| None).collect();
    std::thread::scope(|s| {
        let (f, next) = (&f, &next);
        let handles: Vec<_> = (0..workers.min(n))
            .map(|_| {
                s.spawn(move || {
                    set_msaa_samples(samples);
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            return done;
                        }
                        done.push((i, f(i)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, t) in h.join().unwrap() {
                out[i] = Some(t);
            }
        }
    });
    out.into_iter()
        .map(|t| t.expect("every index built"))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn scene_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
    layout: &wgpu::PipelineLayout,
    (vs_entry, fs_entry): (&str, &str),
    bias: wgpu::DepthBiasState,
    alpha_to_coverage: bool,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 44,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, // pos
                    1 => Float32x2, // uv
                    2 => Float32x2, // lm_uv
                    3 => Float32x3, // normal
                    4 => Unorm8x4,  // color
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Map triangles are wound clockwise seen from the front.
            // Cull back faces like the engine; coplanar interior and
            // exterior faces z-fight otherwise.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias,
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples(),
            mask: !0,
            alpha_to_coverage_enabled: alpha_to_coverage,
        },
        multiview_mask: None,
        cache: None,
    })
}

/// Pipeline variant plus the draw facts a StageBatch needs. `None` when
/// `idx` is out of range.
fn stage_draw_facts(sh: &Material, idx: usize) -> Option<(StageVariant, bool, bool)> {
    let cs = sh.classify_stage(idx)?;
    let st = &sh.stages[idx];
    let variant = match cs.class {
        DrawClass::Opaque => StageVariant::Opaque,
        _ if sh.vertex_multiply => StageVariant::Multiply,
        _ if sh.falloff.is_some_and(|f| f.kind == FalloffKind::Distance) => {
            StageVariant::Multiply2x
        }
        _ if sh.premultiplied
            && st.blend == Some((BlendFactor::OneMinusDstColor, BlendFactor::One)) =>
        {
            StageVariant::AdditiveInvDst
        }
        DrawClass::Blend => {
            if cs.depth_write {
                StageVariant::BlendDepthWrite
            } else {
                StageVariant::Blend
            }
        }
        DrawClass::Additive => {
            let fixed = st.blend == Some((BlendFactor::One, BlendFactor::One));
            match (fixed, cs.depth_write) {
                (true, false) => StageVariant::AdditiveFixed,
                (true, true) => StageVariant::AdditiveFixedDepthWrite,
                (false, true) => StageVariant::AdditiveFadedDepthWrite,
                (false, false) => StageVariant::AdditiveFaded,
            }
        }
    };
    Some((variant, cs.two_sided, cs.bias))
}

/// Whether the stage's params change over time and need per-frame rewrites.
pub(crate) fn stage_animated(sh: &Material, idx: usize) -> bool {
    let Some(st) = sh.stages.get(idx) else {
        return false;
    };
    sh.deforms.iter().any(|d| *d != Deform::Other)
        || st.bundles.iter().any(|b| has_animated_tcmods(&b.tcmods))
        || matches!(st.rgb_gen, RgbGen::Wave(_))
        || matches!(st.alpha_gen, AlphaGen::Wave(_))
}

/// Group(1) and group(2) bind groups for one authored stage, shared by the
/// world stage-batch loop and the cloud dome. `lm` is the batch's
/// lightmap page (bundle-0 `$lightmap` samples it through the flag).
#[allow(clippy::too_many_arguments)]
fn stage_bind_groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &Pk3Fs,
    material_layout: &wgpu::BindGroupLayout,
    stage_layout: &wgpu::BindGroupLayout,
    params_buf: &wgpu::Buffer,
    params_size: u64,
    white_view: &wgpu::TextureView,
    lm: &LightmapPage,
    lightmap_sampler: &wgpu::Sampler,
    bundle_views: &mut BundleViews,
    st: &Stage,
) -> (AnimFrames, AnimFrames) {
    let b0 = &st.bundles[0];
    let b1 = st.bundles.get(1);
    let mat_group = |b0_view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage material bind group"),
            layout: material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(b0_view),
                },
                // bundle-0 lightmaps sample here via the flag
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lm.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&lm.edges),
                },
            ],
        })
    };
    let stage_group = |b1_view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage draw bind group"),
            layout: stage_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: params_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(params_size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b1_view),
                },
            ],
        })
    };
    let mut resolve = |p: &str| upload_bundle_view(device, queue, fs, bundle_views, p);
    let mat = match (&b0.anim, &b0.image) {
        (Some(anim), _) => AnimFrames {
            fps: anim.fps,
            groups: anim.paths.iter().map(|p| mat_group(&resolve(p))).collect(),
        },
        (None, ImageRef::Path(p)) => AnimFrames::static_(mat_group(&resolve(p))),
        // $whiteimage draws white; $lightmap samples t_lightmap
        // through the flag, so bundle 0's own slot stays neutral
        (None, _) => AnimFrames::static_(mat_group(white_view)),
    };
    let stage = match b1.map(|b| (&b.anim, &b.image)) {
        Some((Some(anim), _)) => AnimFrames {
            fps: anim.fps,
            groups: anim
                .paths
                .iter()
                .map(|p| stage_group(&resolve(p)))
                .collect(),
        },
        Some((None, ImageRef::Path(p))) => AnimFrames::static_(stage_group(&resolve(p))),
        _ => AnimFrames::static_(stage_group(white_view)),
    };
    (mat, stage)
}

/// World-space fx quads. One pipeline per [`FxBlend`], and the same four
/// with the `vc_softParticles` fade. One draw call per stage of each
/// contiguous same-shader run.
struct FxPass {
    /// Indexed by `FxBlend as usize`.
    pipelines: [wgpu::RenderPipeline; 4],
    /// The `vc_softParticles` four; group 2 is
    /// `SoftParticles::fx_bind_group`.
    soft_pipelines: [wgpu::RenderPipeline; 4],
    /// `fs_distort`, straight alpha; group 2 is `SceneCopy::bind_group`.
    distort_pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer, // static quad pattern, MAX_FX_DRAWN * 6, u32
    /// image name -> bind group; None = failed to resolve, warned already.
    textures: HashMap<String, Option<wgpu::BindGroup>>,
    /// Draws in order, by sort first.
    runs: Vec<FxRun>,
    /// CoD2's particle clouds, whose runs ride `runs`.
    clouds: fx_cloud::CloudPass,
}

/// One stage of a same-shader run of fx quads.
struct FxRun {
    /// Key into `FxPass::textures`.
    image: String,
    blend: FxBlend,
    quads: usize,
    /// The material's sort.
    sort: f32,
    /// Its quads carry a feather, so it reads the scene depth copy.
    feather: bool,
    /// CoD4's feather (`feather_view_depth`): 1.7's `floatZ` holds the
    /// models and the view weapon, so its copy waits for the split
    /// (cod4-client-perf.md 20); CoD2's goes before the first model.
    view_depth: bool,
    /// A CoD4 distortion run (`FX_VERT_DISTORT`): drawn through
    /// `FxPass::distort_pipeline` over the scene copy, after the split.
    distort: bool,
    /// A run of CoD2 clouds (`fx_cloud`): its instances, and no quads.
    cloud: Option<std::ops::Range<u32>>,
}

/// Debug-HUD text over an embedded font atlas; drawn last, ignoring depth.
struct HudTextPass {
    pipeline: wgpu::RenderPipeline,
    pipeline_1x: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// On-screen HUD (chat, killfeed, scoreboard, status). Same lazy texture
/// cache and run pattern as [`FxPass`], straight alpha only, no depth test.
struct HudPass {
    pipeline: wgpu::RenderPipeline,
    pipeline_1x: wgpu::RenderPipeline,
    /// The same blending `SrcAlpha One`, for [`hud::ADDITIVE`] quads.
    additive: wgpu::RenderPipeline,
    additive_1x: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    texture_layout: wgpu::BindGroupLayout,
    /// image name -> texture; `None` = failed to resolve, warned already.
    textures: HashMap<String, Option<HudTexture>>,
    /// Implicit images the cgame registers mipmapped ([`hud_mip_names`]);
    /// empty without a gamestate, where no cgame runs.
    mip_names: HashSet<String>,
    /// (texture, quad count, additive) runs in draw order.
    runs: Vec<(String, usize, bool)>,
    /// For the F3 overlay.
    quad_count: usize,
}

impl HudPass {
    /// The runs into `pass`, the single-sample pipelines when `single`.
    /// Returns the draws.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, single: bool) -> usize {
        let mut draws = 0;
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint16);
        let mut first = 0usize;
        let mut bound = None;
        for (texture, count, add) in &self.runs {
            if let Some(Some(tex)) = self.textures.get(texture) {
                if bound != Some(*add) {
                    pass.set_pipeline(match (*add, single) {
                        (false, false) => &self.pipeline,
                        (false, true) => &self.pipeline_1x,
                        (true, false) => &self.additive,
                        (true, true) => &self.additive_1x,
                    });
                    bound = Some(*add);
                }
                pass.set_bind_group(0, &tex.bg, &[]);
                pass.draw_indexed((first * 6) as u32..((first + count) * 6) as u32, 0, 0..1);
                draws += 1;
            }
            first += count;
        }
        draws
    }
}

/// A HUD image and its bind group, which holds `r_textureMode`'s sampler
/// for the image's address mode and is rebuilt when the mode changes.
struct HudTexture {
    view: wgpu::TextureView,
    clamp: bool,
    bg: wgpu::BindGroup,
    /// Retail's default shader, which draws whatever the quad's colour.
    default: bool,
}

/// Live snapshot entities: xmodels uploaded once with bind pose baked in,
/// drawn with per-instance transforms streamed through one instance-step
/// vertex buffer.
struct DynamicPass {
    instance_buf: wgpu::Buffer,
    bone_layout: wgpu::BindGroupLayout,
    /// Identity block first, then each skinned instance's bone set. Sized
    /// for every instance at the cap.
    bone_buf: wgpu::Buffer,
    bone_bg: wgpu::BindGroup,
    /// Indexed by [`ModelHandle`]: each model's LOD table; never shrinks.
    handles: Vec<DynHandle>,
    /// One per uploaded LOD and per hidden-part view of one; never shrinks.
    models: Vec<DynModel>,
    /// LODs whose buffers and skins went to the GPU.
    uploads: usize,
    /// This frame's instances before the LOD pick, from `set_dynamic_models`.
    placed: Vec<Placed>,
    /// `(models index, packed instance)` after the LOD pick; position here
    /// is the slot in `instance_buf`.
    instances: Vec<(usize, InstanceRaw)>,
    /// Per entry of `instances`, its `Placed::src`.
    instance_src: Vec<usize>,
    /// Last pick: instances per LOD, instances past their last LOD, and the
    /// triangles drawn.
    lod_counts: LodCounts,
    /// Written to `bone_buf` in [`Renderer::render`].
    bone_mats: Vec<[f32; 16]>,
    /// Per instance, its index in `placed`.
    instance_placed: Vec<usize>,
    /// Per instance, where its lights are picked.
    light_origins: Vec<glam::Vec3>,
    /// CoD4: each instance's posed world box (mins, maxs), as
    /// `light_origins`.
    light_boxes: Vec<(glam::Vec3, glam::Vec3)>,
    /// One `GpuLightSet` per distinct lighting origin this frame.
    light_buf: wgpu::Buffer,
    /// The map's light grid; `None` with no map.
    lighting: Option<StaticLighting>,
    /// A CoD2 map's light grid, which lights its models in place of
    /// `lighting`.
    cod2_light: Option<crate::cod2_light::Cod2Lighting>,
    /// A CoD4 map's light grid lookup and sun, which light its entities
    /// and view model through their own model lighting blocks
    /// (cod4-world.md 3.3).
    cod4_light: Option<Cod4EntityLight>,
    /// This frame's fx lights, for the entity pick.
    scene_lights: Vec<SceneLight>,
    /// The cgame's own scene lights this frame (`constantLight`,
    /// `projectileDLight`), queued after the fx lights.
    entity_scene_lights: Vec<SceneLight>,
    materials: EntityMaterials,
}

/// A dynamic model and, per drawn surface, its material.
struct DynModel {
    vm: VmModel,
    skins: Vec<Rc<EntSkin>>,
    /// Some surface takes the light grid; an unlit model skips the pick.
    lit: bool,
    tris: usize,
}

/// An uploaded xmodel: `Model::lod_dists`, and per LOD its `DynModel`
/// (`None` when that LOD has nothing to draw).
struct DynHandle {
    dists: Vec<f32>,
    lods: Vec<Option<usize>>,
    /// Per bone LOD 0 weights any vertex to, the box of those vertices in
    /// model space: posed by the bone's skin matrix, their union bounds
    /// the posed mesh (`cod2_cookie::Candidate`).
    bone_boxes: Vec<(u8, glam::Vec3, glam::Vec3)>,
    /// The same over every LOD's vertices: what the view cull poses
    /// ([`instance_seen`]), whichever LOD draws.
    view_boxes: Vec<(u8, glam::Vec3, glam::Vec3)>,
}

/// One `DynamicModelInstance`, packed, before its LOD is picked.
struct Placed {
    handle: usize,
    /// Its index in the slice `set_dynamic_models` was handed.
    src: usize,
    raw: InstanceRaw,
    lod_origin: glam::Vec3,
    light_origin: glam::Vec3,
    shader_time: Option<f32>,
}

/// F3's `lod` counts for the dynamic models.
#[derive(Clone, Copy, Default)]
pub struct LodCounts {
    pub per_lod: [usize; 3],
    pub gone: usize,
    pub tris: usize,
}

/// Everything built from one map: buffers, material bind groups, batches,
/// visibility. Dropped whole on a map change.
struct WorldGpu {
    vertex_buf: wgpu::Buffer,
    /// CoD2's lit draws; `None` on a CoD1 map.
    cod2: Option<cod2::Cod2World>,
    index_buf: wgpu::Buffer,
    bind_groups: Vec<wgpu::BindGroup>,
    /// Merged CPU indices (world then props); `index_buf` holds the gathered visible subset.
    cpu_indices: Vec<u32>,
    soup_ranges: Vec<Option<IndexRange>>,
    /// (placement, LOD, range).
    prop_ranges: Vec<(u32, u8, IndexRange)>,
    /// LOD 0's box per placement.
    prop_bounds: Vec<(glam::Vec3, glam::Vec3)>,
    prop_lods: Vec<props::PropLod>,
    /// Each placement's LOD at the last gather, `None` past its last.
    prop_lod: Vec<Option<u8>>,
    /// Static per-batch draw descriptors; `draws` is rebuilt each frame from them.
    batch_draws: Vec<DrawCall>,
    /// Per batch, its CoD4 primary light (`mesh::Batch::light`; 0 for a
    /// prop): what the frame's `draws` mark used for the spot shadows.
    batch_lights: Vec<u8>,
    draws: Vec<DrawRange>,
    /// Authored stages expanded from their parent batches.
    stage_batches: Vec<StageBatch>,
    /// Parent batch id -> indices into `stage_batches`.
    stages_of_batch: Vec<Vec<u32>>,
    /// Animated stages: (shader clone, stage idx, params slot); rewritten each frame.
    animated: Vec<(Material, usize, u32)>,
    /// Cloud-dome stages: like `animated` but every entry also gets the view
    /// origin stamped into its params each frame.
    sky_stages: Vec<(Material, usize, u32)>,
    /// Per batch, its material sort and its place in the sorted shader
    /// list (`sorted_places`), the draw list's key.
    batch_keys: Vec<(f32, u32)>,
    /// This frame's emission-ordered draw list.
    frame_draws: Vec<FrameDraw>,
    /// Stage draws emitted last frame, for the F3 overlay.
    stage_draws_last: usize,
    /// Per-stage params UBO (one dynamic-offset slot per (material, stage)).
    /// Group(2) bind groups are per stage batch: bundle 1's image varies.
    stage_params_buf: wgpu::Buffer,
    /// StageParams size rounded up to `min_uniform_buffer_offset_alignment`.
    stage_params_stride: u64,
    vis: WorldVis,
    gathered: Vec<u32>,
    gather_scratch: Vec<Vec<IndexRange>>,
    /// The ranges the world's part of `gathered` (its first
    /// `world_gathered` indices) was gathered from, so a frame whose
    /// visible set holds still neither gathers, orders nor uploads it again.
    gathered_from: Vec<IndexRange>,
    world_gathered: usize,
    /// The frozen set and the prop flags it was computed with, so `Locked`
    /// holds props still too.
    locked: Option<(Visible, Vec<bool>)>,
    /// Last frame's cull and its eye, view and LOD ramp's bits: a frame
    /// that holds all three takes it again (cod4-client-perf.md 31).
    culled: Option<(CullKey, (Visible, Vec<bool>))>,
    /// The lump-27 model each soup belongs to, 0 for the world's own.
    soup_model: Vec<u32>,
    /// CoD4's static model placements, each drawn at one LOD a frame
    /// ([`vcod_common::mapworld::StaticLod`]).
    static_lods: Vec<vcod_common::mapworld::StaticLod>,
    /// Per soup, whether it is a static model's LOD past 0: the cached
    /// casters and the unculled view draw LOD 0 alone.
    coarse_lod: Vec<bool>,
    /// The world's sky soups and their boxes (`sun::sky_drawn`).
    sky_soups: Vec<(usize, (glam::Vec3, glam::Vec3))>,
    /// Per model, whether its soups draw with the world this frame. A
    /// submodel draws only through the entity that carries it
    /// ([`Renderer::set_submodels`]); model 0 always draws.
    static_models: Vec<bool>,
    /// Per model, its soups, `bsp.models[m]`'s `first_soup..+num_soups`.
    model_soups: Vec<std::ops::Range<usize>>,
    /// The brush models drawn off their spawn pose this frame, with that
    /// pose: their soups gathered after the world's, under their own camera.
    moved: Vec<(usize, glam::Mat4)>,
    /// Per entry of `moved`, its gathered draws and their emission order.
    moved_draws: Vec<(Vec<DrawRange>, Vec<FrameDraw>)>,
    /// Per vertex, the tangent s axis the dlight projection reads
    /// (`dlight::soup_s_axes`; zero for props).
    s_axis_buf: wgpu::Buffer,
    /// Per soup, its spawn-pose bounds, which a light must reach.
    soup_bounds: Vec<(glam::Vec3, glam::Vec3)>,
    /// The dlight pass's stages and, per parent batch, its entries there.
    dlight_batches: Vec<DlightBatch>,
    dlights_of_batch: Vec<Vec<u32>>,
    /// This frame's per-light ranges, indices into `dlight_index_buf`.
    dlight_runs: Vec<DlightRun>,
    dlight_indices: Vec<u32>,
    dlight_index_buf: wgpu::Buffer,
    /// `vc_shadows`' static casters, built the first frame it is on.
    shadow_casters: Option<shadows::Casters>,
    /// Per batch, how it casts in `vc_shadows`.
    shadow_cast: Vec<ShadowCast>,
    /// Per batch, a CoD2 BSP surface whose material takes the shadow
    /// cookies (`cod2_cookie`; static models never do).
    cookie_receive: Vec<bool>,
    /// By a receiving soup's range (its first index), the soup's bounds:
    /// a receiver draw's box for its cookie mask.
    cookie_bounds: HashMap<u32, (glam::Vec3, glam::Vec3)>,
    /// Every receiving soup of the last gather (`gathered_from`), in the
    /// gathered order: its range of `gathered` and its box (`None`: every
    /// cookie), what the receivers draw and mask soup by soup.
    cookie_runs: Vec<CookieRun>,
    /// The lightmaps' measured shadow darkness for `vc_shadows`.
    sun_keep: SunKeepState,
    /// CoD4's sun shadow map casters; `None` elsewhere.
    sun_casters: Option<crate::cod4_sunshadow::Casters>,
}

/// `static_light::sun_keep_regions` for the loaded map: its samples until
/// `vc_shadows` first wants it, then the job, then the keep.
enum SunKeepState {
    Samples(Vec<(glam::Vec3, f32)>, f32),
    Running(std::thread::JoinHandle<Option<vcod_common::static_light::RegionKeep>>),
    Done(Option<Arc<vcod_common::static_light::RegionKeep>>),
}

/// How a world batch casts in `vc_shadows`' layers.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ShadowCast {
    /// Not at all: layers, decals, past opaque, or no depth-writing stage.
    None,
    Solid,
    /// Alpha-tested at half through the batch's own bind group, as
    /// `fs_main`, `fs_prop` and the overlays' coverage cut it.
    Cut,
    /// Alpha-tested by its first depth-writing stage, this stage batch.
    Stage(u32),
    /// Like `Stage`, but its shader deforms the vertices over time
    /// (`deformVertexes wave` / `flap`), so it casts in the dynamic layer
    /// each frame, deformed, and not in the static one.
    Deformed(u32),
}

/// Records stage batch `sb` (stage `si` of `sh`) as the batch's first
/// depth-writing stage if it is one and none came before. It cuts when it
/// alpha-tests a texture.
fn note_first_depth(slot: &mut Option<(u32, bool)>, sh: &Material, si: usize, sb: usize) {
    if slot.is_some() || !sh.classify_stage(si).is_some_and(|c| c.depth_write) {
        return;
    }
    let st = &sh.stages[si];
    let cuts = st.alpha_func.is_some()
        && st
            .bundles
            .first()
            .is_some_and(|b| !b.takes_lightmap_coords());
    *slot = Some((sb as u32, cuts));
}

/// One cloud-dome draw: a stage of the active sky block over the whole dome
/// mesh, through the shared stage pipelines and params slots.
struct DomeStage {
    variant: StageVariant,
    two_sided: bool,
    slot: u32,
    mat: AnimFrames,
    stage: AnimFrames,
}

struct DomeLayer {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    stages: Vec<DomeStage>,
}

/// The farbox cube. Absent from [`SkyRender`] when the env art is missing:
/// retail gates the whole box on `outerbox[0]` having loaded
/// (RB_StageIteratorSky tr_sky.c :994) and draws nothing otherwise.
struct SkyBox {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    /// View origin per frame; the cube never rotates (sky.wgsl).
    eye_buf: wgpu::Buffer,
    eye_bg: wgpu::BindGroup,
    /// Per-axis bind groups in [`sky::AXIS_SUFFIX`] order.
    side_bgs: Vec<wgpu::BindGroup>,
    /// Index ranges per axis into `index_buf`.
    sides: [(u32, u32); 6],
    /// A CoD2 sky's cube, which the glow's sky bleed samples too.
    cube: Option<wgpu::TextureView>,
}

/// The map's active sky: the farbox cube plus optional cloud dome. Built at
/// load, drawn FIRST every frame with depth-always / write-off / cull-off so
/// the world overdraws it everywhere geometry exists (retail draws at depth
/// range 1.0 before everything; same net effect).
struct SkyRender {
    /// Chosen block name, for the F3 overlay and the load log.
    name: String,
    farbox: Option<SkyBox>,
    dome: Option<DomeLayer>,
    /// `sunfile` glow sprite; needs a resolvable `.sun` sprite image and a
    /// lump-30 sun light.
    sun: Option<SunSprite>,
    /// The `.sun` file's flare and screen washes (`sun_flare.rs`).
    flare: Option<SunFlareGpu>,
}

/// The `sunfile` disc: an additive billboard along `bsp::sun_direction`
/// at farbox distance, rebuilt per frame around the view origin.
struct SunSprite {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    dir: glam::Vec3,
    half_extent: f32,
}

/// Encodes the soups of every sprite-deformed map material for `vs_stage`
/// (`vcod_common::deform`), their vertices in `vertices` and their index
/// runs in `indices` in place. Autosprite rewrites a soup's indices as
/// stamps, so a soup whose index count is not six per four vertices keeps
/// its geometry and is warned.
fn encode_world_sprites(
    bsp: &MapWorld,
    lib: &MaterialLib,
    soup_ranges: &[Option<IndexRange>],
    vertices: &mut [DrawVert],
    indices: &mut [u32],
) {
    for (soup, range) in bsp.soups.iter().zip(soup_ranges) {
        let Some(r) = range else { continue };
        let name = &bsp.materials[soup.material as usize].name;
        let Some(sprite) = lib.get(name).and_then(|sh| sh.sprite) else {
            continue;
        };
        let first = soup.first_vertex as usize;
        let verts = &mut vertices[first..first + soup.vertex_count as usize];
        let idx = &mut indices[r.first as usize..(r.first + r.count) as usize];
        match sprite {
            Sprite::Auto if verts.len().is_multiple_of(4) && idx.len() == verts.len() / 4 * 6 => {
                idx.copy_from_slice(&deform::encode_autosprite(verts, soup.first_vertex));
            }
            Sprite::Auto => log::warn!("Autosprite shader {name} had odd vertex or index count"),
            Sprite::Auto2 => deform::encode_autosprite2(verts, idx, soup.first_vertex),
        }
    }
}

/// Builds the farbox for the block's env name. Missing non-rt sides draw
/// tr.defaultImage flat white (tr_shader.c:1170), warned per load.
#[allow(clippy::too_many_arguments)]
fn build_sky_box(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    fs: &Pk3Fs,
    camera_layout: &wgpu::BindGroupLayout,
    bundle_views: &mut BundleViews,
    white_view: &wgpu::TextureView,
    env: &str,
    sh_name: &str,
) -> SkyBox {
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
        label: Some("sky box vertices"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sky box indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky side layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let mut side_bgs = Vec::with_capacity(6);
    for suffix in sky::AXIS_SUFFIX {
        let path = format!("{env}_{suffix}");
        let found = assets::resolve_bundle_image(fs, &path).is_some();
        let view = if found {
            upload_bundle_view(device, queue, fs, bundle_views, &path)
        } else {
            // retail binds tr.defaultImage for a side the paks lack
            log::warn!("sky {sh_name}: env image {path} not found, drawing white");
            white_view.clone()
        };
        let label = format!("sky {sh_name}_{suffix}");
        side_bgs.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&label),
            layout: &tex_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        }));
    }

    let eye_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sky eye"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky eye layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let eye_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky eye bind group"),
        layout: &eye_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: eye_buf.as_entire_binding(),
        }],
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky.wgsl"),
        source: wgpu::ShaderSource::Wgsl(SKY_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(&tex_layout), Some(&eye_layout)],
        immediate_size: 0,
    });
    let box_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky box pipeline"),
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
            // interior faces of a closed box: no culling to second-guess
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // drawn first; the world must overwrite it without a fight
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
    });

    SkyBox {
        pipeline: box_pipeline,
        vertex_buf,
        index_buf,
        eye_buf,
        eye_bg,
        side_bgs,
        sides,
        cube: None,
    }
}

/// The sun disc's half-width per unit of `r_sunsprite_size` at unit
/// distance (CoDMP.exe 0x569024, read by 0x4e7980).
const SUN_SPRITE_SCALE: f32 = 0.001_311_092_9;

/// Builds the sunfile sprite for a parsed `.sun` file along the sun
/// direction. `None` when the sprite image does not resolve.
#[allow(clippy::too_many_arguments)]
fn build_sun_sprite(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    fs: &Pk3Fs,
    camera_layout: &wgpu::BindGroupLayout,
    bundle_views: &mut BundleViews,
    sprite: &str,
    dir: glam::Vec3,
    size: f32,
) -> Option<SunSprite> {
    if assets::resolve_bundle_image(fs, sprite).is_none() {
        log::warn!("sunfile: sprite image {sprite} not found, drawing no sun");
        return None;
    }
    let view = upload_bundle_view(device, queue, fs, bundle_views, sprite);
    let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sun sprite layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sun sprite bind group"),
        layout: &tex_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky.wgsl"),
        source: wgpu::ShaderSource::Wgsl(SKY_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sun sprite pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(&tex_layout)],
        immediate_size: 0,
    });
    // Additive like the corpus glow blocks (`blendFunc GL_ONE GL_ONE` with
    // the texture's alpha folded into rgb by fs_sun); depth-always so the
    // disc draws behind everything.
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sun sprite pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_sun"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<sky::SkyBoxVert>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_sun"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
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
    });

    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sun sprite vertices"),
        size: 6 * std::mem::size_of::<sky::SkyBoxVert>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let dir = dir.normalize_or_zero();
    // 0x4e7980 builds the quad at unit distance with a half-width of
    // `r_sunsprite_size * 0.0013110929`; scaled out to the farbox
    let half_extent = size * SUN_SPRITE_SCALE * sky::box_size(Z_FAR, Z_NEAR);
    Some(SunSprite {
        pipeline,
        vertex_buf,
        bind_group,
        dir,
        half_extent,
    })
}

/// Where a frame lands: the window's swapchain, or a texture for a
/// headless renderer (tests).
enum Target {
    Surface(wgpu::Surface<'static>),
    #[cfg_attr(not(test), allow(dead_code))]
    Offscreen(wgpu::Texture),
}

pub struct Renderer {
    target: Target,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    /// `Pass::Solid`'s.
    solid_pipeline: wgpu::RenderPipeline,
    /// `[culled][solid]`, `Pass::Prop`'s.
    prop_pipelines: [[wgpu::RenderPipeline; 2]; 2],
    layer_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    /// `[variant][two_sided][bias]`; built at startup so shader.wgsl validates
    /// even mapless.
    stage_pipelines: [[[wgpu::RenderPipeline; 2]; 2]; STAGE_VARIANTS],
    dlight: dlight::DlightPass,
    camera_layout: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    camera_bg: wgpu::BindGroup,
    /// One camera per moved brush model drawn this frame, the shared camera
    /// with that model's pose in `model`; grown on demand, never shrunk.
    moved_cams: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    fx_lights_buf: wgpu::Buffer,
    /// The camera group's `vc_shadows` receiver inputs while it is off.
    shadow_off: shadows::ReceiveOff,
    /// Device-stage state `load_world` builds map bind groups from.
    material_layout: wgpu::BindGroupLayout,
    stage_layout: wgpu::BindGroupLayout,
    /// `r_textureMode`'s samplers, bound in the camera and viewmodel
    /// groups; `set_texture_filter` swaps them.
    tex_samplers: TexSamplers,
    lightmap_sampler: wgpu::Sampler,
    white_view: wgpu::TextureView,
    /// Bound for a vertex-lit surface: `white_view` in one block.
    white_lightmap: LightmapPage,
    /// `vc_bicubic`: lightmaps filter B-spline bicubic; rides `time_pad.z`.
    bicubic: bool,
    world: Option<WorldGpu>,
    vis_counts: VisCounts,
    /// Stage draws emitted last frame; stages dropped at the current map's
    /// load; unique MaterialLib warnings. The F3 `shader:` line.
    shader_dropped: usize,
    lib_warn_count: usize,
    vm_pass: VmPass,
    dynamic: DynamicPass,
    fx: FxPass,
    /// Impact marks (`crate::marks`), drawn through the stage pipelines.
    marks: mark_pass::MarkPass,
    /// The map's `corona` entities, drawn as sprites through `marks`.
    coronas: Vec<bsp::Corona>,
    /// `r_flares` is non-zero.
    flares: bool,
    /// `r_drawSun` is non-zero.
    draw_sun: bool,
    /// The sky drew this frame (`sun::sky_drawn`), so the sun did too.
    sky_drawn: bool,
    hud: HudTextPass,
    hud_pass: HudPass,
    gamma: GammaPass,
    /// The last frame drew its post pass, or its scene, and 2D into the
    /// swapchain and ran no gamma pass (`GammaPass::identity`).
    gamma_skipped: bool,
    /// [`Renderer::request_capture`]: the next drawn frame is read back,
    /// before the gamma ramp when the flag is set.
    capture_wanted: Option<bool>,
    /// The frame read back (`None` inside when it could not be), until
    /// [`Renderer::take_capture`].
    captured: Option<Option<Capture>>,
    /// Off keeps the gamma pass on an identity ramp, for an A/B.
    #[cfg(test)]
    swap_direct: bool,
    /// `SCENE_FORMAT`, or `HDR_FORMAT` with `vc_hdr`: what every world,
    /// entity, fx and sky pipeline draws into. Fixed for the renderer's life.
    world_format: wgpu::TextureFormat,
    /// `vc_hdr`'s tonemap and `vc_grade`/`vc_vignette`/`vc_grain`.
    post: crate::post::PostPass,
    /// CoD4's depth of field over `post`'s world target
    /// (`crate::cod4_dof`), and the frame's, set every frame it draws.
    dof: crate::cod4_dof::DofPass,
    cod4_dof: Option<crate::cod4_dof::Dof>,
    bloom: crate::bloom::Bloom,
    /// CoD2's `r_glow`, on every CoD2 map (`crate::cod2_glow`).
    cod2_glow: crate::cod2_glow::Cod2Glow,
    /// Whether the bloom chain and the sun shafts read `post`'s world target
    /// (else the gamma pass's scene).
    effects_read_post: bool,
    /// `vc_grade`, `vc_vignette`, `vc_grain`, read every frame.
    post_settings: crate::post::PostSettings,
    /// A CoD4 level's look (`set_level_look`); `None` on CoD1 and CoD2.
    level_look: Option<LevelLook>,
    /// `vc_bloom` and `vc_bloomThreshold`; no bloom passes while the
    /// intensity is 0.
    bloom_intensity: f32,
    bloom_threshold: f32,
    ssao: crate::ssao::SsaoPass,
    /// `vc_sunShafts`, added into the resolved scene before bloom.
    shafts: crate::sun_shafts::SunShafts,
    /// The shellshock's screen blend, and this frame's decay for it
    /// (`set_screen_blend`).
    screen_blend: crate::screen_blend::ScreenBlend,
    shock_blend: Option<i32>,
    /// A CoD4 shellshock's screen for the next frame, over
    /// [`Self::shock_blend`] (`set_cod4_shock`).
    cod4_shock: Option<crate::play::cod4_shock::Screen>,
    /// The CoD2 view's blur radius (`set_world_blur`).
    world_blur: f32,
    /// When the blend last captured, for render command 10's wall clock.
    shock_captured_at: Option<std::time::Instant>,
    /// CoD2 model skins and their stage images decoded, kept across maps
    /// (players and weapons come back on the next one) until the search
    /// path reopens.
    model_images: assets::DecodedImages,
    /// A batch's model skins decoded and mipped ahead on every core
    /// ([`Self::upload_dynamic_models`]), emptied when the batch ends.
    prepared_skins: HashMap<String, PreparedImage>,
    /// The CoD2 lit draws' shader, layouts and pipelines, kept across maps.
    cod2_lit: cod2::LitShared,
    soft: crate::soft_particles::SoftParticles,
    /// The colour copy the distortion effects bend (`resolvedPostSun`).
    scene_copy: crate::scene_copy::SceneCopy,
    /// The loaded world's feathered and distortion stage pipelines
    /// (`cod4_worldfx`); `None` when it has none.
    world_fx: Option<cod4_worldfx::WorldFxPass>,
    /// Retail's `identityLight` for the current overbright bits
    /// (`gamma::identity_light`): what lit colours are scaled by.
    identity_light: f32,
    /// The texture gamma table images load through (`gamma::bake_image`),
    /// set before a world load while `r_ignorehwgamma` is 1.
    image_gamma: Option<[u8; 256]>,
    /// `r_picmip`, `r_picmip2`, `r_texturebits` and `r_textureMode` as of
    /// this renderer's build (`crate::texture_settings`).
    textures: TextureSettings,
    /// CoD4's `vidConfig.aspectRatioWindow` ([`Renderer::set_view_aspect`]).
    view_aspect: Option<f32>,
    /// `r_dynamiclight`, read every frame: 0 none, 1 world and models,
    /// anything else models only (0x4e9cb0).
    dynamic_light: i32,
    /// `r_lodscale`, `r_lodbias` (Model Detail).
    lod_cvars: (f32, f32),
    /// CoD4's Options switches, read every frame.
    cod4_options: Cod4Options,
    /// `r_vc_depthprepass`: the opaque lit world and entity surfaces lay
    /// their depth down first and draw their colour under EQUAL.
    depth_prepass: bool,
    /// CoD2's cookie receivers draw over that prepass's world depth when
    /// it runs ([`Self::cookie_receivers_on_prepass`]); off, on their own
    /// depth, for an A/B.
    cookies_on_prepass: bool,
    /// The last frame's prepass draws.
    #[cfg(test)]
    prepass_drawn: std::cell::Cell<usize>,
    /// Whether the last frame's prepass wrote `floatZ`.
    #[cfg(test)]
    pub(crate) floatz_drawn: std::cell::Cell<bool>,
    /// The depth prepass joins its draws ([`Self::encode_prepass_world_joined`]),
    /// the world draws skip a bind group already bound ([`WorldGroups`])
    /// and the view skips an entity out of its frustum ([`instance_seen`]);
    /// off draws and binds one by one and every entity, for an A/B.
    draw_joins: bool,
    /// A CoD4 frame's runs of world draws replay from render bundles kept
    /// while their commands hold (cod4-client-perf.md 31);
    /// `VCOD_WORLD_BUNDLES=0` encodes them draw by draw, for an A/B.
    world_bundles_on: bool,
    /// Its entity runs and its depth prepass do too (cod4-client-perf.md
    /// 40); `VCOD_ENT_BUNDLES=0` encodes them draw by draw, for an A/B.
    ent_bundles_on: bool,
    /// A CoD4 frame a feather reads writes `floatZ` in the depth prepass
    /// instead of splitting the scene pass for a depth copy
    /// (cod4-client-perf.md 35); `VCOD_C4_FLOATZ=0` keeps the copy, for an
    /// A/B.
    floatz_prepass: bool,
    world_bundles: WorldBundles,
    /// The depth prepass's runs, kept as `world_bundles` are.
    prepass_bundles: WorldBundles,
    /// The last frame's draws and state sets ([`PassCensus`]).
    census: std::cell::Cell<PassCensus>,
    /// The last frame's GPU writes in order (`frame_write!`).
    #[cfg(test)]
    frame_writes: std::cell::RefCell<Vec<FrameWrite>>,
    /// The time the CoD4 waters' height maps were last built at; it holds
    /// while `r_drawWater` is 0.
    water_time: f32,
    /// `r_dlightQuality`, read every frame: the dlight projection on each
    /// vertex's tangent frame, else on the model's axes (0x4ff690).
    dlight_quality: bool,
    /// Kept past map load so later inline submodels resolve `textures/...`
    /// names the same way the world did.
    shaders: assets::Shaders,
    /// Parsed shader scripts; Task 7 classifies map materials against it.
    shader_lib: MaterialLib,
    /// The map's active sky (farbox + optional cloud dome); `None` without a
    /// matching sky block.
    sky: Option<SkyRender>,
    /// Global fog from configstring 12; the F3 `fog:` line.
    fog: FogState,
    hud_quad_cap_warned: bool,
    /// `vc_shadows`; built when it turns on, dropped when it turns off.
    shadows: Option<shadows::ShadowPass>,
    /// CoD2's shadow cookies, drawn on a CoD2 map while `sc_enable` is set.
    cookies: cod2_cookie::Cookies,
    /// The screen's dynamic shadow they darken, bound in the camera group.
    dyn_shadows: cod2_cookie::DynamicShadows,
    /// CoD4's sun shadow map, its atlas in the camera group.
    sun_shadow: crate::cod4_sunshadow::SunShadowPass,
    /// `sm_enable` and `sm_sunSampleSizeNear`.
    sun_shadow_set: (bool, vcod_cod4::sunshadow::Settings),
    /// The spot light shadow dvars (`crate::cod4_spotshadow`).
    spot_shadow_set: crate::cod4_spotshadow::Settings,
    /// This frame's CoD4 rigid LOD ramp `(scale, bias)`, which the shadow
    /// casters' static models pick by too.
    cod4_lod: (f32, f32),
    /// `VCOD_FRAME_PROFILE`'s GPU stamps (`frame_prof`).
    prof_gpu: Option<crate::frame_prof::GpuStamps>,
    /// The main pass's fragment counts by draw kind (`VCOD_FRAG_STATS`).
    pub(crate) frag_stats: Option<crate::frame_prof::FragStats>,
    /// Test only: the main pass draws this instance's entity surfaces
    /// (or its one surface) alone, the prepass keeps them all, for their
    /// fragment count.
    #[cfg(test)]
    pub(crate) ent_only: std::cell::Cell<Option<(u32, Option<u32>)>>,
    /// Test only: the last frame's entity draws as (instance, surface,
    /// triangles, stages, under EQUAL, its first stage one the prepass
    /// takes).
    #[cfg(test)]
    pub(crate) ent_census: std::cell::RefCell<Vec<EntCensus>>,
    /// The profile's GPU groups inside and between the passes.
    gpu_groups: Option<crate::frame_prof::GpuGroups>,
}

/// The limits every renderer device asks for, the window's and the
/// headless one's alike: wgpu's defaults with the adapter's own where a
/// map outgrows them.
pub(crate) fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let a = adapter.limits();
    wgpu::Limits {
        // One stage-params UBO must stay reachable through dynamic offsets,
        // past the default 64 KiB uniform binding window.
        max_uniform_buffer_binding_size: a.max_uniform_buffer_binding_size,
        // A CoD4 map's reflection probes, six layers each.
        max_texture_array_layers: a.max_texture_array_layers,
        // A CoD4 map's static models baked at every LOD: mp_creek's
        // vertices are 393 MB, past the default 256 MB.
        max_buffer_size: a.max_buffer_size,
        // The CoD4 lit world pass's 17 images with its static models'
        // lighting volume, past WebGPU's 16 (every desktop driver allows
        // far more).
        max_sampled_textures_per_shader_stage: a.max_sampled_textures_per_shader_stage,
        ..Default::default()
    }
}

impl Renderer {
    /// `hdr` is `vc_hdr`: the world draws into a float target.
    pub fn new(
        window: Arc<winit::window::Window>,
        fs: &dyn AssetSource,
        hdr: bool,
        textures: TextureSettings,
        samples: u32,
    ) -> Result<Renderer> {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        // `WGPU_BACKEND=vulkan|gl|dx12|metal` narrows the backend choice.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window)
            .context("failed to create a surface for the window")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter found; is a Vulkan (or DX12/Metal) driver installed?")?;
        // Named in the log for bug reports.
        let info = adapter.get_info();
        log::info!(
            "GPU: {} ({:?}, {} {})",
            info.name,
            info.backend,
            info.driver,
            info.driver_info
        );
        let _ = crate::BACKEND.set(format!("{:?}", info.backend).to_lowercase());

        if !adapter
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            let info = adapter.get_info();
            bail!(
                "GPU '{}' ({:?}) does not support BC texture compression, \
                 which is required to load CoD's DDS textures",
                info.name,
                info.backend
            );
        }

        // Timestamps feed the F3 `ssao` line's GPU times where the adapter has them.
        let timestamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vcod device"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC
                | (adapter.features() & wgpu::Features::TIMESTAMP_QUERY)
                | (adapter.features() & crate::frame_prof::extra_features())
                | (adapter.features() & crate::frame_prof::frag_features())
                | (adapter.features() & crate::frame_prof::group_features()),
            required_limits: device_limits(&adapter),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("failed to create the wgpu device")?;

        let caps = surface.get_capabilities(&adapter);
        // the scene's own format where the surface takes it, so a frame
        // with an identity gamma ramp draws its last pass straight into the
        // swapchain (`Renderer::gamma_direct`)
        let format = if caps.formats.contains(&SCENE_FORMAT) {
            SCENE_FORMAT
        } else {
            caps.formats
                .iter()
                .copied()
                .find(|f| f.is_srgb())
                .unwrap_or(caps.formats[0])
        };
        let config = wgpu::SurfaceConfiguration {
            // read back for `screenshot` where the surface allows it
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | (caps.usages & wgpu::TextureUsages::COPY_SRC),
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        // whether an identity ramp can skip the gamma pass rides on these two
        log::info!(
            "swapchain {format:?}, alpha {:?} (surface lists {:?})",
            config.alpha_mode,
            caps.formats
        );
        surface.configure(&device, &config);
        Ok(Self::build(
            device,
            queue,
            Target::Surface(surface),
            config,
            timestamps,
            fs,
            hdr,
            textures,
            samples,
        ))
    }

    /// Everything past the device: the passes, pipelines and layouts every
    /// frame uses, drawing into `target` sized by `config`.
    #[allow(clippy::too_many_arguments)]
    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target: Target,
        config: wgpu::SurfaceConfiguration,
        timestamps: bool,
        fs: &dyn AssetSource,
        hdr: bool,
        textures: TextureSettings,
        samples: u32,
    ) -> Renderer {
        set_msaa_samples(samples);
        let world_format = if hdr { HDR_FORMAT } else { SCENE_FORMAT };
        let (width, height, format) = (config.width, config.height, config.format);
        let msaa_view = create_msaa_view(&device, world_format, width, height);
        let depth_view = create_depth_view(&device, width, height);
        let ssao = crate::ssao::SsaoPass::new(
            &device,
            &queue,
            &depth_view,
            world_format,
            width,
            height,
            timestamps,
        );

        let [r0, r1, r2, r3, r4, r5, r6] = shadows::receive_layout_entries();
        let [s0, s1, s2, s3, s4, s5, s6, s7] = TexSamplers::layout_entries();
        let [m0, m1, m2] = crate::cod4_sunshadow::layout_entries();
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Fx lights. The fx pass shares this layout without declaring
                // binding 1 in fx.wgsl, which wgpu allows. Props light
                // per vertex from them.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // vc_shadows on what draws after the mask (2..=8)
                r0,
                r1,
                r2,
                r3,
                r4,
                r5,
                r6,
                // r_textureMode's samplers (9, 10)
                s0,
                s1,
                s2,
                s3,
                s4,
                s5,
                s6,
                s7,
                // CoD2's dynamic shadow the lit world and models read
                // (cod2_cookie::DynamicShadows)
                wgpu::BindGroupLayoutEntry {
                    binding: DYNAMIC_SHADOW_BINDING,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // CoD4's sun shadow map (cod4_sunshadow, 18 and 19)
                m0,
                m1,
                m2,
            ],
        });
        let material_layout = material_layout(&device);
        let stage_layout = stage_layout(&device);

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            // proj + time + eye/fog tail + view forward + model, matching
            // Camera in shader.wgsl; the other modules read a prefix
            size: CAMERA_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fx_lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx lights uniform"),
            size: std::mem::size_of::<FxLightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_off = shadows::ReceiveOff::new(&device);
        let tex_samplers = TexSamplers::new(&device, textures.filter);
        let dyn_shadows = cod2_cookie::DynamicShadows::new(&device, &queue, width, height);
        let sun_shadow = crate::cod4_sunshadow::SunShadowPass::new(&device, &queue);
        let camera_bg = camera_bind_group(
            &device,
            &camera_layout,
            &camera_buf,
            &fx_lights_buf,
            &shadow_off.receive(),
            &tex_samplers,
            dyn_shadows.view(),
            &sun_shadow,
        );

        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let white_view = upload_rgba(
            &device,
            &queue,
            "white lightmap",
            1,
            1,
            &[255, 255, 255, 255],
        );

        let white_lightmap = LightmapPage {
            view: white_view.clone(),
            edges: lightmap_blocks::upload_edges(
                &device,
                &queue,
                "white lightmap blocks",
                1,
                &[[0; 4]],
            ),
        };

        let shaders = assets::load_shaders(fs);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader.wgsl"),
            source: wgpu::ShaderSource::Wgsl(WORLD_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let stage_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("stage pipeline layout"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&material_layout),
                    Some(&stage_layout),
                ],
                immediate_size: 0,
            });
        let make_pipeline = |label: &str,
                             layout: &wgpu::PipelineLayout,
                             vs_entry: &str,
                             bias: wgpu::DepthBiasState,
                             fs_entry: &str,
                             alpha_to_coverage: bool,
                             blend: Option<wgpu::BlendState>,
                             depth_write: bool,
                             cull: Option<wgpu::Face>| {
            scene_pipeline(
                &device,
                &shader,
                world_format,
                label,
                layout,
                (vs_entry, fs_entry),
                bias,
                alpha_to_coverage,
                blend,
                depth_write,
                cull,
            )
        };
        let bias = depth_bias(PolygonOffset::Single);
        let cull_back = Some(wgpu::Face::Back);
        let pipeline = make_pipeline(
            "map pipeline",
            &pipeline_layout,
            "vs_main",
            Default::default(),
            "fs_main",
            false,
            None,
            true,
            cull_back,
        );
        let solid_pipeline = make_pipeline(
            "map pipeline, no cut",
            &pipeline_layout,
            "vs_main",
            Default::default(),
            "fs_main_solid",
            false,
            None,
            true,
            cull_back,
        );
        // Props take the opaque alpha-test path: `*_masked` skins are cutouts
        // on real geometry, not coplanar decals, so no overlay depth bias.
        // An xmodel's triangles wind opposite to the map's, so a culled
        // prop drops its front-wound faces (cod11-client-perf.md 5).
        let prop_pipelines = [false, true].map(|culled| {
            [false, true].map(|solid| {
                make_pipeline(
                    &format!("prop pipeline culled={culled} solid={solid}"),
                    &pipeline_layout,
                    "vs_prop",
                    Default::default(),
                    if solid { "fs_prop_solid" } else { "fs_prop" },
                    false,
                    None,
                    true,
                    culled.then_some(wgpu::Face::Front),
                )
            })
        });
        // Blend layers alpha-blend over the base ground by vertex alpha. No
        // depth write: the base wrote depth, and stacked layers must all draw.
        let layer_pipeline = make_pipeline(
            "layer pipeline",
            &pipeline_layout,
            "vs_main",
            bias,
            "fs_layer",
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            cull_back,
        );
        // Overlays are pulled toward the viewer like the engine's polygon
        // offset (Q3 defaults -1/-2); their alpha drives MSAA coverage.
        let overlay_pipeline = make_pipeline(
            "overlay pipeline",
            &pipeline_layout,
            "vs_main",
            bias,
            "fs_overlay",
            true,
            None,
            true,
            cull_back,
        );
        // One twin pair per variant, [back-culled, two-sided], picked per
        // StageBatch by ClassedStage in Task 7.
        // The stage table and the dlight pass compile on their own threads
        // while this one builds the entity pass (`par_build`).
        let (stage_pipelines, dlight, vm_pass, dynamic) = std::thread::scope(|s| {
            let stages = s.spawn(|| {
                set_msaa_samples(samples);
                par_build(STAGE_VARIANTS * 4, |i| {
                    let variant = STAGE_VARIANT_ORDER[i / 4];
                    let (two_sided, want_bias) = (i & 2 != 0, i & 1 != 0);
                    let (fs_entry, blend, depth_write) = stage_variant_state(variant);
                    make_pipeline(
                        &format!("stage {variant:?} twosided={two_sided} bias={want_bias}"),
                        &stage_pipeline_layout,
                        "vs_stage",
                        if want_bias { bias } else { Default::default() },
                        fs_entry,
                        false,
                        blend,
                        depth_write,
                        if two_sided { None } else { cull_back },
                    )
                })
            });
            let dlight = s.spawn(|| {
                set_msaa_samples(samples);
                dlight::DlightPass::new(
                    &device,
                    &queue,
                    &shader,
                    [&camera_layout, &material_layout, &stage_layout],
                    &lightmap_sampler,
                    bias,
                    world_format,
                )
            });
            let vm_pass = create_vm_pass(&device, &tex_samplers);
            let dynamic = create_dynamic_pass(
                &device,
                &queue,
                world_format,
                &camera_layout,
                &vm_pass,
                &white_view,
                textures,
            );
            (
                stages.join().unwrap(),
                dlight.join().unwrap(),
                vm_pass,
                dynamic,
            )
        });
        let mut stage_pipelines = stage_pipelines.into_iter();
        let stage_pipelines: [[[wgpu::RenderPipeline; 2]; 2]; STAGE_VARIANTS] =
            std::array::from_fn(|_| {
                std::array::from_fn(|_| std::array::from_fn(|_| stage_pipelines.next().unwrap()))
            });
        let soft = crate::soft_particles::SoftParticles::new(&device, &depth_view, width, height);
        let scene_copy =
            crate::scene_copy::SceneCopy::new(&device, world_format, soft.linz(), width, height);
        let fx = create_fx_pass(
            &device,
            world_format,
            &camera_layout,
            &vm_pass.skin_layout,
            &soft.fx_layout,
            &scene_copy.layout,
        );
        let cookies = cod2_cookie::Cookies::new(&device, &dynamic.bone_layout);
        let hud = create_hud_text_pass(&device, &queue, SCENE_FORMAT);
        let hud_pass = create_hud_pass(&device, SCENE_FORMAT);
        let gamma = GammaPass::new(&device, format, width, height);
        let post = crate::post::PostPass::new(&device, world_format, width, height);
        let dof = crate::cod4_dof::DofPass::new(&device, world_format, width, height);
        let bloom =
            crate::bloom::Bloom::new(&device, world_format, gamma.scene_view(), width, height);
        let cod2_glow = crate::cod2_glow::Cod2Glow::new(&device, world_format, width, height);
        let shafts = crate::sun_shafts::SunShafts::new(
            &device,
            world_format,
            gamma.scene_view(),
            &depth_view,
            width,
            height,
        );
        let screen_blend =
            crate::screen_blend::ScreenBlend::new(&device, gamma.scene_view(), width, height);

        let lib = material_lib(fs);
        let marks = mark_pass::MarkPass::new(
            &device,
            shader.clone(),
            stage_pipeline_layout.clone(),
            world_format,
        );
        let prof_gpu = crate::frame_prof::GpuStamps::new(&device, &queue);
        let frag_stats = crate::frame_prof::FragStats::new(&device);
        let gpu_groups = crate::frame_prof::GpuGroups::new(&device, &queue);
        Renderer {
            target,
            device,
            queue,
            config,
            msaa_view,
            depth_view,
            pipeline,
            solid_pipeline,
            prop_pipelines,
            layer_pipeline,
            overlay_pipeline,
            stage_pipelines,
            dlight,
            camera_layout,
            camera_buf,
            camera_bg,
            moved_cams: Vec::new(),
            fx_lights_buf,
            shadow_off,
            material_layout,
            stage_layout,
            tex_samplers,
            lightmap_sampler,
            white_view,
            white_lightmap,
            bicubic: false,
            world: None,
            vis_counts: VisCounts::default(),
            shader_dropped: 0,
            lib_warn_count: lib.warn_count(),
            vm_pass,
            dynamic,
            fx,
            marks,
            coronas: Vec::new(),
            flares: true,
            draw_sun: true,
            sky_drawn: false,
            hud,
            hud_pass,
            gamma,
            gamma_skipped: false,
            capture_wanted: None,
            captured: None,
            #[cfg(test)]
            swap_direct: true,
            world_format,
            post,
            dof,
            cod4_dof: None,
            bloom,
            cod2_glow,
            effects_read_post: false,
            post_settings: Default::default(),
            level_look: None,
            bloom_intensity: 0.0,
            bloom_threshold: 0.0,
            ssao,
            shafts,
            screen_blend,
            shock_blend: None,
            cod4_shock: None,
            world_blur: 0.0,
            shock_captured_at: None,
            model_images: Default::default(),
            prepared_skins: HashMap::new(),
            cod2_lit: Default::default(),
            soft,
            scene_copy,
            world_fx: None,
            identity_light: 0.5,
            image_gamma: None,
            textures,
            view_aspect: None,
            dynamic_light: 1,
            lod_cvars: (1.0, 0.0),
            cod4_options: Cod4Options::default(),
            depth_prepass: false,
            cookies_on_prepass: true,
            #[cfg(test)]
            prepass_drawn: Default::default(),
            #[cfg(test)]
            floatz_drawn: Default::default(),
            draw_joins: true,
            world_bundles_on: std::env::var_os("VCOD_WORLD_BUNDLES").is_none_or(|v| v != "0"),
            floatz_prepass: std::env::var_os("VCOD_C4_FLOATZ").is_none_or(|v| v != "0"),
            world_bundles: WorldBundles::default(),
            ent_bundles_on: std::env::var_os("VCOD_ENT_BUNDLES").is_none_or(|v| v != "0"),
            prepass_bundles: WorldBundles::default(),
            census: Default::default(),
            #[cfg(test)]
            frame_writes: Default::default(),
            water_time: 0.0,
            dlight_quality: true,
            hud_quad_cap_warned: false,
            shadows: None,
            cookies,
            dyn_shadows,
            sun_shadow,
            sun_shadow_set: (true, Default::default()),
            spot_shadow_set: Default::default(),
            cod4_lod: (1.0, 0.0),
            prof_gpu,
            frag_stats,
            #[cfg(test)]
            ent_only: Default::default(),
            #[cfg(test)]
            ent_census: Default::default(),
            gpu_groups,
            shaders,
            shader_lib: lib,
            sky: None,
            fog: FogState::default(),
        }
    }

    /// Draws the next map with `lib` in place of the game's materials (a
    /// CoD4 map carries its own).
    pub fn set_material_lib(&mut self, lib: MaterialLib) {
        self.shader_lib = lib;
        self.lib_warn_count = self.shader_lib.warn_count();
    }

    /// Builds the map's GPU state; a previous map is dropped first.
    pub fn load_world(&mut self, bsp: &MapWorld, fs: &Pk3Fs) -> Result<()> {
        let mut laps = vcod_common::laps::Laps::new("world GPU state");
        self.unload_world();
        let device = &self.device;
        let queue = &self.queue;
        let material_layout = &self.material_layout;
        let lightmap_sampler = &self.lightmap_sampler;
        let white_view = &self.white_view;
        let white_lightmap = &self.white_lightmap;

        // Per-material draw decision: authored blocks classify from their
        // parsed surface bits, implicit names from the gameplay prefixes.
        let kinds = vcod_common::sun::material_kinds(&self.shader_lib, bsp);
        let active_sky = vcod_common::sun::active_sky(&self.shader_lib, bsp, &kinds);

        // Props (misc_model entities) are baked to world space on the CPU in
        // the same vertex format and extend the map's buffers.
        let (mut indices, batches, soup_ranges) = mesh::build_batches(bsp, &kinds);
        let mut soup_model = vec![0u32; bsp.soups.len()];
        for (mi, m) in bsp.models.iter().enumerate().skip(1) {
            let first = m.first_soup as usize;
            for s in soup_model.iter_mut().skip(first).take(m.num_soups as usize) {
                *s = mi as u32;
            }
        }
        if batches.is_empty() {
            bail!("map has no drawable surfaces");
        }
        laps.lap("batches");
        let mut lighting = StaticLighting::new(bsp);
        let lib = &self.shader_lib;
        let cod2 = fs.game() == vcod_common::game::Game::Cod2;
        self.fog.radial = cod2;
        // The light grid's sight traces' world builds beside the props.
        let (mut props, light_clip) = std::thread::scope(|sc| {
            let clip = bsp
                .light_grid
                .is_some()
                .then(|| sc.spawn(|| vcod_common::collision::CollisionWorld::build(bsp, &[])));
            let props = props::build(fs, bsp, &mut lighting, &|skin| {
                let script = prop_material(lib, cod2, skin);
                props::SkinDraw {
                    sprite: script.and_then(|sh| sh.sprite),
                    reflects: match script.filter(|sh| !sh.stages.is_empty()) {
                        Some(sh) => sh.reflects(),
                        None => {
                            matches!(prop_skin_draw(fs, skin), PropSkinDraw::Stages(sh) if sh.reflects())
                        }
                    },
                    model_colour: cod2 && script.is_some_and(cod2_model_colour),
                    ambient: script.is_some_and(|m| m.model_light == ModelLight::Ambient),
                    smodel: cod2
                        && !bsp.lightmap_basis.is_empty()
                        && script.is_some_and(cod2_smodel),
                    skip: cod2 && script.is_some_and(|m| m.model_light == ModelLight::Objective),
                }
            });
            (props, clip.map(vcod_common::laps::joined))
        });
        laps.lap("props");
        self.dynamic.lighting = Some(lighting);
        self.dynamic.cod2_light =
            light_clip.and_then(|clip| crate::cod2_light::Cod2Lighting::with_clip(bsp, clip));
        self.dynamic.cod4_light = bsp
            .entity_lighting
            .clone()
            .zip(bsp.sun)
            .map(|(lighter, sun)| Cod4EntityLight {
                lighter,
                sun_dir: glam::Vec3::from(sun.dir),
                sun_colour: glam::Vec3::from(sun.colour),
                lights: bsp.model_lights.clone(),
            });
        // CoD4: the probes an entity's specular picks from (empty elsewhere)
        self.dynamic
            .materials
            .set_probes(&self.device, &self.queue, &bsp.reflection_probes);
        // `R_ResetShadowCookies` (Mac 0x000f99fc) at the map's load
        self.cookies.clock = Default::default();
        self.cookies.entities.clear();
        let smodel_cache = self
            .dynamic
            .cod2_light
            .as_ref()
            .and_then(|c2| c2.light_props(fs, &bsp.entities, &mut props));
        laps.lap("prop light");
        let prop_first_index = indices.len() as u32;
        let prop_first_vertex = bsp.verts.len() as u32;
        indices.extend(props.indices.iter().map(|i| i + prop_first_vertex));
        // prop batches follow the world's in `batch_draws`, prop indices follow
        // the world's in the merged buffer
        let prop_ranges: Vec<(u32, u8, IndexRange)> = props
            .ranges
            .iter()
            .map(|&(p, l, r)| {
                (
                    p,
                    l,
                    IndexRange {
                        batch: r.batch + batches.len() as u32,
                        first: r.first + prop_first_index,
                        count: r.count,
                    },
                )
            })
            .collect();
        let mut vertices = bsp.verts.clone();
        encode_world_sprites(
            bsp,
            &self.shader_lib,
            &soup_ranges,
            &mut vertices,
            &mut indices,
        );
        vertices.extend_from_slice(&props.verts);

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut s_axes = dlight::soup_s_axes(&bsp.soups, &bsp.verts, &bsp.indices);
        s_axes.resize(vertices.len(), [0.0; 3]);
        let cod2_tangents = cod2.then(|| {
            let mut t = cod2::tangent_stream(bsp, vertices.len() - props.verts.len());
            t.extend_from_slice(&props.vert_tangent);
            t
        });
        let s_axis_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map tangent s axes"),
            contents: bytemuck::cast_slice(&s_axes),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });

        laps.lap("buffers");
        // CoD2's images, decoded on every core before the loops below take
        // them one by one in their own order.
        let prefetched = assets::DecodedImages::default();
        if cod2 {
            let mats = bsp
                .materials
                .iter()
                .filter_map(|m| self.shader_lib.get(&m.name))
                .chain(
                    props
                        .batches
                        .iter()
                        .filter_map(|b| prop_material(&self.shader_lib, cod2, &b.skin)),
                );
            prefetched.prefetch(fs, prefetch_paths(fs, mats));
        }
        laps.lap("prefetch");
        let fallback_px = fallback_pixels();
        let shaders = assets::load_shaders(fs);
        let mut material_views: HashMap<u16, wgpu::TextureView> = HashMap::new();
        // materials whose image has texels the 0.5 alpha test cuts
        let mut cut_materials: HashSet<u16> = HashSet::new();
        // CoD1 materials no level of whose image the cut drops a texel of
        let cod1 = fs.game() == vcod_common::game::Game::Cod1;
        let mut solid_materials: HashSet<u16> = HashSet::new();
        let (mut loaded, mut fallbacks) = (0usize, 0usize);
        let load_level = texture_settings::path_levels(fs);
        for batch in &batches {
            if material_views.contains_key(&batch.material) {
                continue;
            }
            let name = &bsp.materials[batch.material as usize].name;
            let sh = self.shader_lib.get(name);
            let mut img = if cod2 {
                colour_map_image(fs, sh, &prefetched)
            } else {
                assets::load_material_image(fs, &shaders, name)
            };
            let picmip = self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
            let mips = if sh.is_some_and(|sh| sh.nomipmaps) {
                texture_settings::Mips::single(name, &load_level)
            } else {
                texture_settings::Mips::chain(name, &load_level)
            };
            if is_fallback(&img, &fallback_px) {
                fallbacks += 1;
            } else {
                loaded += 1;
            }
            let mat = &bsp.materials[batch.material as usize];
            if sh.is_none() && !mesh::is_overlay(mat) && world_type_draws_opaque(fs, name) {
                assets::force_opaque(&mut img);
            }
            if assets::has_cutout_alpha(&img) {
                cut_materials.insert(batch.material);
            }
            let levels = texture_settings::prepare(
                &mut img,
                picmip,
                self.image_gamma.as_ref(),
                self.textures.store(),
                mips,
            );
            if cod1 && !uploads_cutout(&img, &levels) {
                solid_materials.insert(batch.material);
            }
            material_views.insert(
                batch.material,
                upload_levels(device, queue, name, &img, &levels),
            );
        }

        laps.lap("material images");
        // One RGBA lightmap page per BSP page, plus a white 1x1 for unlit soups.
        let block_edges = lightmap_blocks::edges(bsp);
        let pages_rgba = vcod_common::laps::par_map(&bsp.lightmaps, |page| {
            let mut rgba = Vec::with_capacity(page.len() / 3 * 4);
            for px in page.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            rgba
        });
        let lightmap_views: Vec<LightmapPage> = pages_rgba
            .iter()
            .zip(&block_edges)
            .enumerate()
            .map(|(i, (rgba, edges))| {
                let size = bsp::LIGHTMAP_SIZE as u32;
                LightmapPage {
                    view: upload_rgba(device, queue, &format!("lightmap {i}"), size, size, rgba),
                    edges: lightmap_blocks::upload_edges(
                        device,
                        queue,
                        &format!("lightmap {i} blocks"),
                        size,
                        edges,
                    ),
                }
            })
            .collect();

        laps.lap("lightmaps");
        self.marks.load(
            crate::marks::MarkWorld::build(bsp, |m| kinds[m as usize] == MaterialKind::Draw),
            lightmap_views.clone(),
        );
        self.coronas = bsp::coronas(&bsp.entities);
        if !self.coronas.is_empty() {
            log::info!("coronas: {}", self.coronas.len());
        }

        laps.lap("marks");
        let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
        let mut cache: HashMap<(u16, u16), usize> = HashMap::new();
        // each batch's CoD4 primary light, the props' none, in
        // `batch_draws`' order
        let batch_lights: Vec<u8> = batches
            .iter()
            .map(|b| b.light)
            .chain(props.batches.iter().map(|_| 0))
            .collect();
        let mut batch_draws = Vec::with_capacity(batches.len());
        for Batch {
            material, lightmap, ..
        } in &batches
        {
            let idx = *cache.entry((*material, *lightmap)).or_insert_with(|| {
                let lm = match *lightmap {
                    bsp::NO_LIGHTMAP => white_lightmap,
                    i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
                };
                bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("material bind group"),
                    layout: material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&material_views[material]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&lm.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&lm.edges),
                        },
                    ],
                }));
                bind_groups.len() - 1
            });
            let mat = &bsp.materials[*material as usize];
            let pass = if mesh::is_overlay(mat) {
                Pass::Overlay
            } else if shaders.uses_polygon_offset(&mat.name) {
                Pass::Layer
            } else if solid_materials.contains(material) {
                Pass::Solid
            } else {
                Pass::Opaque
            };
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass,
            });
        }

        laps.lap("bind groups");
        // Prop materials are skin filenames, not `textures/...` names, so
        // `load_skin_image` resolves them.
        let mut skin_cache: HashMap<&str, (usize, bool, Option<usize>, Pass)> = HashMap::new();
        let skin_level = texture_settings::skin_levels(fs);
        let mut prop_cut: Vec<bool> = Vec::with_capacity(props.batches.len());
        // `type@` templates past one unblended stage (metal_env, the
        // blends), bound to their skin; they draw through the stage path
        let mut prop_templates: Vec<Material> = Vec::new();
        let mut prop_template_of: Vec<Option<usize>> = Vec::with_capacity(props.batches.len());
        for batch in &props.batches {
            let (idx, cut, template, pass) =
                *skin_cache.entry(batch.skin.as_str()).or_insert_with(|| {
                    let script = prop_material(&self.shader_lib, cod2, &batch.skin);
                    let scripted = script.is_some_and(|sh| !sh.stages.is_empty());
                    let mut img = if cod2 {
                        colour_map_image(fs, script, &prefetched)
                    } else {
                        assets::load_skin_image(fs, &batch.skin)
                    };
                    let mut template = None;
                    if !scripted {
                        match prop_skin_draw(fs, &batch.skin) {
                            PropSkinDraw::Stages(sh) => {
                                prop_templates.push(*sh);
                                template = Some(prop_templates.len() - 1);
                            }
                            PropSkinDraw::Opaque => assets::force_opaque(&mut img),
                            PropSkinDraw::Cut => {}
                        }
                    }
                    let cut = assets::has_cutout_alpha(&img);
                    let levels = texture_settings::prepare(
                        &mut img,
                        self.textures.picmip2,
                        self.image_gamma.as_ref(),
                        self.textures.store(),
                        texture_settings::Mips::chain(&batch.skin, &skin_level),
                    );
                    let view = upload_levels(device, queue, &batch.skin, &img, &levels);
                    let pass = Pass::Prop {
                        culled: cod1 && !scripted && !prop_skin_two_sided(fs, &batch.skin),
                        solid: cod1 && !uploads_cutout(&img, &levels),
                    };
                    bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("prop skin bind group"),
                        layout: material_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(white_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(&white_lightmap.edges),
                            },
                        ],
                    }));
                    (bind_groups.len() - 1, cut, template, pass)
                });
            prop_cut.push(cut);
            prop_template_of.push(template);
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass,
            });
        }

        laps.lap("prop skins");
        let count = |p: fn(Pass) -> bool| batch_draws.iter().filter(|d| p(d.pass)).count();
        println!(
            "{} batches ({} opaque, {} prop, {} layer, {} overlay), {} draw indices, {} vertices",
            batch_draws.len(),
            count(|p| matches!(p, Pass::Opaque | Pass::Solid)),
            count(|p| matches!(p, Pass::Prop { .. })),
            count(|p| p == Pass::Layer),
            count(|p| p == Pass::Overlay),
            indices.len(),
            vertices.len()
        );
        println!(
            "{} textures loaded, {} missing, {} lightmap pages",
            loaded,
            fallbacks,
            lightmap_views.len()
        );

        // One dynamic-offset slot per authored stage of every map material
        // that resolves in the shader lib; batches of one material share the
        // slot per stage. Implicit materials use no slots.
        let mut stage_slots: Vec<(&Material, usize)> = Vec::new();
        let mut slot_of: HashMap<(u16, usize), u32> = HashMap::new();
        for (mi, mat) in bsp.materials.iter().enumerate() {
            if let Some(sh) = self.shader_lib.get(&mat.name) {
                for idx in 0..sh.stages.len() {
                    slot_of.insert((mi as u16, idx), stage_slots.len() as u32);
                    stage_slots.push((sh, idx));
                }
            }
        }
        // A prop skin with a `skins/<name>` block, or a template the prop
        // path cannot draw, goes through its stages like a map material,
        // slotted after them (shader doc 8).
        // A lit CoD2 prop's stages read its baked light as `lightingDiffuse`,
        // rescaled from identityLight 0.5 as `vs_prop` does, never raw.
        let cod2_lit: HashMap<&str, Material> = props
            .batches
            .iter()
            .filter(|_| cod2)
            .filter_map(|b| {
                let m = self.shader_lib.get(&b.skin)?;
                let lit = matches!(m.model_light, ModelLight::Probe | ModelLight::Ambient);
                (lit && !m.stages.is_empty())
                    .then(|| (b.skin.as_str(), crate::entity_material::lit_stages(m)))
            })
            .collect();
        let prop_script = |b: &props::Batch, template: Option<usize>| {
            cod2_lit.get(b.skin.as_str()).or_else(|| {
                prop_material(&self.shader_lib, cod2, &b.skin)
                    .filter(|sh| !sh.stages.is_empty())
                    .or_else(|| template.map(|i| &prop_templates[i]))
            })
        };
        // a reflecting placement's own copy, in its model's frame
        let prop_framed: Vec<Option<Material>> = props
            .batches
            .iter()
            .zip(&prop_template_of)
            .map(|(b, &template)| {
                let axes = b.model_axes?;
                let mut sh = prop_script(b, template)?.clone();
                sh.model_axes = Some(axes);
                Some(sh)
            })
            .collect();
        let prop_scripts: Vec<Option<(&Material, u32)>> = props
            .batches
            .iter()
            .zip(&prop_template_of)
            .zip(&prop_framed)
            .map(|((b, &template), framed)| {
                let sh = framed.as_ref().or_else(|| prop_script(b, template))?;
                let first = stage_slots.len() as u32;
                stage_slots.extend((0..sh.stages.len()).map(|idx| (sh, idx)));
                (!sh.stages.is_empty()).then_some((sh, first))
            })
            .collect();
        // The dlight pass's stages (shader doc 3), slotted after the base
        // stages: per scripted material its own as single-stage blocks that
        // keep the material's deform and culling, then the implicit
        // surfaces' copy (texture times the dlight image, rgbGen identity),
        // the same alpha-tested for masked overlays, and `dlightshader`.
        let blob = self.shader_lib.get("dlightshader");
        let mut dl_shaders: Vec<Material> = Vec::new();
        let mut dl_of_material: HashMap<u16, Vec<u32>> = HashMap::new();
        for (mi, mat) in bsp.materials.iter().enumerate() {
            let Some(sh) = self.shader_lib.get(&mat.name) else {
                continue;
            };
            if sh.stages.is_empty() {
                continue;
            }
            for syn in dlight::stage_shaders(sh, blob) {
                dl_shaders.push(syn);
                let i = dl_shaders.len() as u32 - 1;
                dl_of_material.entry(mi as u16).or_default().push(i);
            }
        }
        let implicit_copy = |alpha_func| Stage {
            bundles: [ImageRef::White, ImageRef::Path(DLIGHT_IMAGE.into())]
                .map(|image| Bundle {
                    image,
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: Vec::new(),
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                })
                .into(),
            blend: Some((BlendFactor::One, BlendFactor::One)),
            depth_write: None,
            alpha_func,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        };
        // the copies fog as the implicit surface does; the blob as
        // `dlightshader` (`nofog`)
        let implicit = |st: Stage, nofog: bool| {
            Material::from_shader(&Shader {
                stages: vec![st],
                nofog,
                ..Shader::default()
            })
        };
        let dl_implicit = [
            Some(implicit(implicit_copy(None), false)),
            Some(implicit(implicit_copy(Some(AlphaFunc::Ge128)), false)),
            blob.and_then(|b| Some(implicit(b.stages.first()?.clone(), b.nofog))),
        ]
        .map(|syn| {
            syn.map(|syn| {
                dl_shaders.push(syn);
                dl_shaders.len() as u32 - 1
            })
        });
        let dl_first_slot = stage_slots.len() as u32;

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stage_params_stride = STAGE_PARAMS_SIZE.div_ceil(align) * align;
        let stage_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage params"),
            size: ((stage_slots.len() + dl_shaders.len()) as u64 * stage_params_stride)
                .max(stage_params_stride),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Provisional fill at t = 0 so every slot starts valid (static stages
        // keep these values; animated ones are rewritten each frame).
        let all_slots = stage_slots
            .iter()
            .copied()
            .chain(dl_shaders.iter().map(|sh| (sh, 0)));
        for (i, (sh, idx)) in all_slots.enumerate() {
            if let Some(p) = stage_params(sh, idx, 0.0) {
                queue.write_buffer(
                    &stage_params_buf,
                    i as u64 * stage_params_stride,
                    bytemuck::bytes_of(&p),
                );
            }
        }
        println!(
            "stage params: {} slots x {STAGE_PARAMS_SIZE} B (stride {stage_params_stride})",
            stage_slots.len()
        );

        laps.lap("stage slots");
        // Expand authored materials: one StageBatch per parsed stage sharing
        // the parent batch's index range. Implicit materials and blocks with
        // no effective stages keep the legacy fast path.
        let mut stage_batches: Vec<StageBatch> = Vec::new();
        let mut stages_of_batch: Vec<Vec<u32>> = vec![Vec::new(); batch_draws.len()];
        // per batch, its first depth-writing stage batch and whether it cuts
        let mut first_depth: Vec<Option<(u32, bool)>> = vec![None; batch_draws.len()];
        let mut animated: Vec<(Material, usize, u32)> = Vec::new();
        let mut bundle_views = BundleViews {
            views: HashMap::new(),
            table: self.image_gamma.as_ref(),
            picmip: 0,
            mipmap: true,
            store: self.textures.store(),
            prefetched: Some(&prefetched),
        };
        // once per material, not per batch: a material can hold several batches
        let dropped_stages: usize = bsp
            .materials
            .iter()
            .zip(&kinds)
            .filter(|(_, k)| **k == MaterialKind::Draw)
            .filter_map(|(m, _)| self.shader_lib.get(&m.name))
            .map(|sh| sh.dropped_stages)
            .sum();
        // CoD2's lit draws, over the map's basis pages
        let mut lit = (cod2 && !bsp.lightmap_basis.is_empty()).then(|| {
            let mut b = cod2::Cod2Builder::with_shared(
                device,
                queue,
                self.world_format,
                &self.camera_layout,
                bsp,
                std::mem::take(&mut self.cod2_lit),
            );
            if let Some(c) = &smodel_cache {
                b.set_smodel_cache(c);
            }
            b.set_picmip(self.textures.picmip_bump, self.textures.picmip_spec);
            b
        });
        for (bi, batch) in batches.iter().enumerate() {
            if kinds[batch.material as usize] != MaterialKind::Draw {
                continue;
            }
            let name = &bsp.materials[batch.material as usize].name;
            let Some(sh) = self.shader_lib.get(name) else {
                continue;
            };
            if sh.stages.is_empty() {
                continue;
            }
            bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            let lm_view = match batch.lightmap {
                bsp::NO_LIGHTMAP => white_lightmap,
                i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
            };
            for si in 0..sh.stages.len() {
                let Some((variant, two_sided, bias)) = stage_draw_facts(sh, si) else {
                    continue;
                };
                let slot = slot_of[&(batch.material, si)];
                let (mat, stage) = stage_bind_groups(
                    device,
                    queue,
                    fs,
                    material_layout,
                    &self.stage_layout,
                    &stage_params_buf,
                    STAGE_PARAMS_SIZE,
                    white_view,
                    lm_view,
                    lightmap_sampler,
                    &mut bundle_views,
                    &sh.stages[si],
                );
                if stage_animated(sh, si) {
                    animated.push((sh.clone(), si, slot));
                }
                stages_of_batch[bi].push(stage_batches.len() as u32);
                note_first_depth(&mut first_depth[bi], sh, si, stage_batches.len());
                let lit_bind = lit.as_mut().and_then(|b| {
                    if cod2_water::draws_water(sh, variant) {
                        return Some(b.bind_water(fs, sh, variant, two_sided, bias));
                    }
                    if cod2::draws_cod4_water(sh, variant) {
                        let probe = batch.probe;
                        return Some(b.bind_cod4_water(
                            fs,
                            &mut bundle_views,
                            sh,
                            probe,
                            variant,
                            two_sided,
                            bias,
                        ));
                    }
                    // a CoD4 model set off the model lighting volume
                    if cod2::draws_cod4_model(sh, variant) {
                        return b.has_model_volume().then(|| {
                            b.bind_cod4_model(
                                fs,
                                &mut bundle_views,
                                sh,
                                (batch.probe, batch.light),
                                variant,
                                two_sided,
                                bias,
                            )
                        });
                    }
                    cod2::draws_lit(sh, batch.lightmap, b.page_count(), variant).then(|| {
                        b.bind_in(
                            fs,
                            &mut bundle_views,
                            sh,
                            (batch.lightmap, batch.probe, batch.light),
                            variant,
                            two_sided,
                            bias,
                        )
                    })
                });
                stage_batches.push(StageBatch {
                    slot,
                    variant,
                    two_sided,
                    bias,
                    mat,
                    stage,
                    cod2: lit_bind,
                    fx: cod4_worldfx::world_fx(sh, variant),
                });
            }
        }
        // Scripted prop skins keep the prop path's two-sidedness (xmodel
        // winding) and take no lightmap.
        for (pi, script) in prop_scripts.iter().enumerate() {
            let Some((sh, first)) = *script else { continue };
            bundle_views.picmip = self.textures.model_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            for si in 0..sh.stages.len() {
                let Some((variant, _, bias)) = stage_draw_facts(sh, si) else {
                    continue;
                };
                let slot = first + si as u32;
                let (mat, stage) = stage_bind_groups(
                    device,
                    queue,
                    fs,
                    material_layout,
                    &self.stage_layout,
                    &stage_params_buf,
                    STAGE_PARAMS_SIZE,
                    white_view,
                    white_lightmap,
                    lightmap_sampler,
                    &mut bundle_views,
                    &sh.stages[si],
                );
                if stage_animated(sh, si) {
                    animated.push((sh.clone(), si, slot));
                }
                stages_of_batch[batches.len() + pi].push(stage_batches.len() as u32);
                note_first_depth(
                    &mut first_depth[batches.len() + pi],
                    sh,
                    si,
                    stage_batches.len(),
                );
                // a per pixel static model (`SkinDraw::smodel`) through its
                // `lprobe_smc*` technique
                let smodel = smodel_cache.as_ref().and_then(|_| {
                    let m = self.shader_lib.get(&props.batches[pi].skin)?;
                    let b = lit.as_mut()?;
                    cod2_smodel(m).then(|| b.bind_smodel(fs, &mut bundle_views, m, variant, bias))
                });
                stage_batches.push(StageBatch {
                    slot,
                    variant,
                    two_sided: true,
                    bias,
                    mat,
                    stage,
                    cod2: smodel,
                    fx: cod4_worldfx::world_fx(sh, variant),
                });
            }
        }
        for (bi, sbs) in stages_of_batch.iter().enumerate() {
            if !sbs.is_empty() {
                batch_draws[bi].pass = Pass::Stage;
            }
        }

        // The dlight pass per world batch: a scripted material's stages
        // through their own bind groups, an implicit one through its base
        // bind group and a neutral bundle 1.
        laps.lap("stage images");
        let implicit_stage_bg =
            AnimFrames::static_(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("implicit dlight stage bind group"),
                layout: &self.stage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &stage_params_buf,
                            offset: 0,
                            size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(white_view),
                    },
                ],
            }));
        let mut dlight_batches: Vec<DlightBatch> = Vec::new();
        let mut dlights_of_batch: Vec<Vec<u32>> = vec![Vec::new(); batch_draws.len()];
        for (bi, batch) in batches.iter().enumerate() {
            if kinds[batch.material as usize] != MaterialKind::Draw {
                continue;
            }
            let sh = self
                .shader_lib
                .get(&bsp.materials[batch.material as usize].name);
            if batch_draws[bi].pass == Pass::Stage {
                let Some(list) = dl_of_material.get(&batch.material) else {
                    continue;
                };
                let lm_view = match batch.lightmap {
                    bsp::NO_LIGHTMAP => white_lightmap,
                    i => lightmap_views.get(i as usize).unwrap_or(white_lightmap),
                };
                for &di in list {
                    let syn = &dl_shaders[di as usize];
                    bundle_views.picmip = self.textures.world_picmip(syn.nopicmip);
                    bundle_views.mipmap = !syn.nomipmaps;
                    let st = &syn.stages[0];
                    let (Some(blend), Some((_, two_sided, bias))) =
                        (DlightBlend::of(st), stage_draw_facts(syn, 0))
                    else {
                        log::warn!("{}: dlight stage blend {:?} not drawn", syn.name, st.blend);
                        continue;
                    };
                    let slot = dl_first_slot + di;
                    let (mat, stage) = stage_bind_groups(
                        device,
                        queue,
                        fs,
                        material_layout,
                        &self.stage_layout,
                        &stage_params_buf,
                        STAGE_PARAMS_SIZE,
                        white_view,
                        lm_view,
                        lightmap_sampler,
                        &mut bundle_views,
                        st,
                    );
                    if stage_animated(syn, 0) {
                        animated.push((syn.clone(), 0, slot));
                    }
                    dlights_of_batch[bi].push(dlight_batches.len() as u32);
                    dlight_batches.push(DlightBatch {
                        blend,
                        two_sided,
                        bias,
                        slot,
                        mat,
                        stage,
                    });
                }
                continue;
            }
            // retail's implicit shader multiplies the texture by the
            // lightmap (0x4fc440), which the copy takes; a vertex-lit one
            // has no lightmap bundle and gets `dlightshader`
            if sh.is_some_and(|sh| sh.dlight.is_none()) {
                continue;
            }
            let call = &batch_draws[bi];
            let which = match (call.pass, batch.lightmap) {
                (Pass::Opaque | Pass::Solid | Pass::Overlay, bsp::NO_LIGHTMAP) => 2,
                (Pass::Opaque | Pass::Solid, _) => 0,
                (Pass::Overlay, _) => 1,
                _ => continue,
            };
            let Some(di) = dl_implicit[which] else {
                continue;
            };
            let blend = DlightBlend::of(&dl_shaders[di as usize].stages[0])
                .expect("the implicit dlight stages blend onto GL_ONE");
            dlights_of_batch[bi].push(dlight_batches.len() as u32);
            dlight_batches.push(DlightBatch {
                blend,
                two_sided: false,
                bias: call.pass == Pass::Overlay,
                slot: dl_first_slot + di,
                mat: AnimFrames::static_(bind_groups[call.bind_group].clone()),
                stage: AnimFrames::static_(implicit_stage_bg.groups[0].clone()),
            });
        }
        println!(
            "dlight pass: {} stage draws over {} batches",
            dlight_batches.len(),
            dlights_of_batch.iter().filter(|d| !d.is_empty()).count()
        );
        self.shader_dropped = dropped_stages;
        println!(
            "shader scripts: {} stage batches, {} dropped stages, {} lib warnings",
            stage_batches.len(),
            dropped_stages,
            self.shader_lib.warn_count()
        );

        // ---- sky pass: farbox now, cloud dome through the stage path ----
        self.sky = None;
        laps.lap("dlight stages");
        let mut sky_stages: Vec<(Material, usize, u32)> = Vec::new();
        if let Some((name, mat_idx)) = active_sky {
            let sh = self
                .shader_lib
                .get(&name)
                .expect("sky candidate comes from the lib");
            let parms = sh.sky.as_ref().expect("sky_blocks only yields skyparms");
            bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
            bundle_views.mipmap = !sh.nomipmaps;
            log::info!(
                "sky: {name}  env {}  cloud height {}  stages {}",
                parms.env,
                parms.cloud_height,
                sh.stages.len()
            );
            // retail skips the whole box when outerbox[0] fails to load
            let mut farbox = None;
            if cod2 {
                farbox = cod2::build_sky_cube(
                    device,
                    queue,
                    self.world_format,
                    fs,
                    &self.camera_layout,
                    &parms.env,
                );
            } else if parms.env.is_empty() {
                log::info!("sky {name}: skyParms '-' outerbox, drawing no farbox");
            } else {
                let rt = format!("{}_rt", parms.env);
                if assets::resolve_bundle_image(fs, &rt).is_some() {
                    // skyParms loads its sides with flags 0x33 whatever the
                    // block says: mipmapped, `r_picmip`, clamped (0x4fa3cd)
                    bundle_views.picmip = self.textures.world_picmip(false);
                    bundle_views.mipmap = true;
                    farbox = Some(build_sky_box(
                        device,
                        queue,
                        self.world_format,
                        fs,
                        &self.camera_layout,
                        &mut bundle_views,
                        white_view,
                        &parms.env,
                        &name,
                    ));
                } else {
                    log::warn!("sky {name}: env image {rt} not found, drawing no farbox");
                }
                bundle_views.picmip = self.textures.world_picmip(sh.nopicmip);
                bundle_views.mipmap = !sh.nomipmaps;
            }
            let mut dome = None;
            if !sh.stages.is_empty() {
                let size = sky::box_size(Z_FAR, Z_NEAR);
                let (verts, idx) = sky::dome_mesh(size, parms.cloud_height);
                let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sky dome vertices"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sky dome indices"),
                    contents: bytemuck::cast_slice(&idx),
                    usage: wgpu::BufferUsages::INDEX,
                });
                let mut dome_stages = Vec::new();
                for si in 0..sh.stages.len() {
                    if sh.stages[si].bundles.is_empty() {
                        continue;
                    }
                    let Some((variant, two_sided, _)) = stage_draw_facts(sh, si) else {
                        continue;
                    };
                    // a sky stage may carry explicit depthWrite; the dome must
                    // still never write depth, it draws first at far depth
                    let variant = match variant {
                        StageVariant::BlendDepthWrite => StageVariant::Blend,
                        StageVariant::AdditiveFixedDepthWrite => StageVariant::AdditiveFixed,
                        StageVariant::AdditiveFadedDepthWrite => StageVariant::AdditiveFaded,
                        other => other,
                    };
                    // the block's slots were allocated with every other material's
                    let slot = slot_of[&(mat_idx, si)];
                    let (mat, stage) = stage_bind_groups(
                        device,
                        queue,
                        fs,
                        material_layout,
                        &self.stage_layout,
                        &stage_params_buf,
                        STAGE_PARAMS_SIZE,
                        white_view,
                        white_lightmap,
                        lightmap_sampler,
                        &mut bundle_views,
                        &sh.stages[si],
                    );
                    // dome params ride the per-frame rewrite with the eye offset
                    sky_stages.push((sh.clone(), si, slot));
                    dome_stages.push(DomeStage {
                        variant,
                        two_sided,
                        slot,
                        mat,
                        stage,
                    });
                }
                if !dome_stages.is_empty() {
                    log::info!(
                        "cloud dome: {} stage(s), {} verts",
                        dome_stages.len(),
                        verts.len()
                    );
                    dome = Some(DomeLayer {
                        vertex_buf,
                        index_buf,
                        index_count: idx.len() as u32,
                        stages: dome_stages,
                    });
                }
            }
            // The disc and the flare's angle test follow `sundirection`, the
            // flare sits on the lump-30 light's own direction (0x4d9250).
            let mut sun = None;
            let mut flare = None;
            if let Some(vcod_common::sun::MapSun {
                file: sf,
                light_dir,
                dir,
            }) = &vcod_common::sun::load(fs, sh, bsp)
            {
                let (light_dir, dir) = (*light_dir, *dir);
                if let Some(sprite) = &sf.sprite {
                    log::info!("sky {name}: sun sprite {sprite} size {}", sf.sprite_size);
                    // a shader of its own (0x4e7930); stock ones say
                    // `nomipmaps` and `nopicmip`
                    let sh = self.shader_lib.get(sprite);
                    bundle_views.picmip =
                        self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
                    bundle_views.mipmap = !sh.is_some_and(|sh| sh.nomipmaps);
                    sun = build_sun_sprite(
                        device,
                        queue,
                        self.world_format,
                        fs,
                        &self.camera_layout,
                        &mut bundle_views,
                        sprite,
                        dir,
                        sf.sprite_size,
                    );
                }
                if let Some(f) = &sf.flare {
                    let sh = self.shader_lib.get(f);
                    bundle_views.picmip =
                        self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip));
                    bundle_views.mipmap = !sh.is_some_and(|sh| sh.nomipmaps);
                }
                // the flare's image: its block's first stage, or the name
                let image = sf.flare.as_ref().and_then(|f| {
                    let path = self.shader_lib.image(f).unwrap_or(f).to_string();
                    assets::resolve_bundle_image(fs, &path).map(|_| path)
                });
                match image {
                    Some(path) => {
                        log::info!("sky {name}: sun flare {path}");
                        let view = upload_bundle_view(device, queue, fs, &mut bundle_views, &path);
                        flare = Some(SunFlareGpu::new(
                            device,
                            self.world_format,
                            FlareParams::of(sf),
                            light_dir,
                            dir,
                            &view,
                            &self.tex_samplers,
                        ));
                    }
                    None => log::info!("sky {name}: no sun flare image, drawing no flare"),
                }
            }
            self.sky = Some(SkyRender {
                name,
                farbox,
                dome,
                sun,
                flare,
            });
        }

        // Retail's sorted shader list over the map's shaders (shader doc 5):
        // one shader per (material, lightmap), registered in batch order,
        // which follows the shader lump's first-use order; images number
        // as they first load, white and the lightmap pages before the
        // textures. vcod's own: props and the implicit overlays keep their
        // slot after every other opaque.
        laps.lap("sky and sun");
        let batch_keys: Vec<(f32, u32)> = {
            let mut images = ImageNumbers::default();
            images.number("$white");
            let lightmap = |i: u16| format!("$lightmap{i}");
            for i in 0..lightmap_views.len() {
                images.number(&lightmap(i as u16));
            }
            let mut entries: Vec<SortEntry> = Vec::with_capacity(batch_draws.len());
            for (bi, batch) in batches.iter().enumerate() {
                let mat = &bsp.materials[batch.material as usize];
                let sh = self.shader_lib.get(&mat.name);
                let mut image = |r: &ImageRef| match r {
                    ImageRef::Path(p) => images.number(p),
                    ImageRef::Lightmap if batch.lightmap != bsp::NO_LIGHTMAP => {
                        images.number(&lightmap(batch.lightmap))
                    }
                    ImageRef::Lightmap | ImageRef::White => images.number("$white"),
                };
                entries.push(match (batch_draws[bi].pass, sh) {
                    (Pass::Overlay, _) => SortEntry {
                        sort: SORT_OPAQUE,
                        images: Some([u32::MAX, 0]),
                    },
                    (Pass::Stage, Some(sh)) => staged_sort_entry(sh, &mut image),
                    // implicit: the texture, then the lightmap in bundle 1
                    (_, sh) => {
                        let tex = images.number(&mat.name);
                        let lm = match batch.lightmap {
                            bsp::NO_LIGHTMAP => 0,
                            i => images.number(&lightmap(i)),
                        };
                        SortEntry {
                            sort: sh.map_or(SORT_OPAQUE, |sh| sh.sort),
                            images: Some([tex, lm]),
                        }
                    }
                });
            }
            for script in &prop_scripts {
                entries.push(match script {
                    Some((sh, _)) => staged_sort_entry(sh, &mut |r: &ImageRef| match r {
                        ImageRef::Path(p) => images.number(p),
                        ImageRef::Lightmap | ImageRef::White => images.number("$white"),
                    }),
                    None => SortEntry {
                        sort: SORT_OPAQUE,
                        images: Some([u32::MAX - 1, 0]),
                    },
                });
            }
            // `VCOD_C4_DRAW_ORDER=0` keeps the CoD1 sorted shader list's
            // order, to measure against
            let old_order = std::env::var_os("VCOD_C4_DRAW_ORDER").is_some_and(|v| v == "0");
            let places = match bsp.soup_order.is_empty() || old_order {
                true => sorted_places(&entries),
                false => cod4_places(&entries, &soup_ranges, &bsp.soup_order),
            };
            entries
                .iter()
                .zip(places)
                .map(|(e, p)| (e.sort, p))
                .collect()
        };

        let shadow_cast: Vec<ShadowCast> = batch_draws
            .iter()
            .enumerate()
            .map(|(bi, call)| {
                if batch_keys.get(bi).is_some_and(|k| k.0 > SORT_OPAQUE) {
                    return ShadowCast::None;
                }
                let world = batches.get(bi).map(|b| b.material);
                // water lets the sun through; mp_ship's ocean also waves
                // away from the pose the static layer would hold
                if world.is_some_and(|m| {
                    self.shader_lib
                        .get(&bsp.materials[m as usize].name)
                        .is_some_and(|sh| sh.surface.water)
                }) {
                    return ShadowCast::None;
                }
                let image_cut = match world {
                    Some(m) => cut_materials.contains(&m),
                    None => prop_cut[bi - batches.len()],
                };
                let script = match world {
                    Some(m) => self.shader_lib.get(&bsp.materials[m as usize].name),
                    None => prop_scripts[bi - batches.len()].map(|(sh, _)| sh),
                };
                let deformed = script.is_some_and(|sh| sh.deforms.iter().any(Deform::moves));
                let solid_or_cut = |cut| {
                    if cut {
                        ShadowCast::Cut
                    } else {
                        ShadowCast::Solid
                    }
                };
                match call.pass {
                    Pass::Layer => ShadowCast::None,
                    // decals lie on other surfaces; masked overlays are cutouts
                    Pass::Overlay
                        if world.is_some_and(|m| !mesh::is_masked(&bsp.materials[m as usize])) =>
                    {
                        ShadowCast::None
                    }
                    Pass::Overlay | Pass::Opaque | Pass::Solid | Pass::Prop { .. } => {
                        solid_or_cut(image_cut)
                    }
                    Pass::Stage => match first_depth[bi] {
                        None => ShadowCast::None,
                        Some((sb, _)) if deformed => ShadowCast::Deformed(sb),
                        Some((sb, true)) => ShadowCast::Stage(sb),
                        Some((_, false)) => ShadowCast::Solid,
                    },
                }
            })
            .collect();
        let cookie_receive: Vec<bool> = (0..batch_draws.len())
            .map(|bi| {
                batches.get(bi).is_some_and(|b| {
                    kinds[b.material as usize] == MaterialKind::Draw
                        && self
                            .shader_lib
                            .get(&bsp.materials[b.material as usize].name)
                            .is_some_and(|sh| sh.cookie.receiver)
                })
            })
            .collect();
        laps.lap("sort and shadows");
        let cod2_world = lit.zip(cod2_tangents.as_deref()).map(|(b, t)| {
            let (world, shared) = b.finish_keeping(t);
            self.cod2_lit = shared;
            world
        });
        // the feathered and distortion stages' pipelines, when the map has any
        let fx_keys: Vec<_> = stage_batches
            .iter()
            .filter(|sb| sb.fx != cod4_worldfx::WorldFx::None)
            .map(|sb| (sb.fx, sb.variant, sb.two_sided, sb.bias))
            .collect();
        self.world_fx = (!fx_keys.is_empty()).then(|| {
            cod4_worldfx::WorldFxPass::new(
                device,
                self.world_format,
                [
                    &self.camera_layout,
                    material_layout,
                    &self.stage_layout,
                    &self.scene_copy.world_layout,
                ],
                depth_bias(PolygonOffset::Single),
                fx_keys,
            )
        });
        let sun_casters = self.sun_shadow.casters(device, bsp, &soup_model, |p| {
            upload_bundle_view(device, queue, fs, &mut bundle_views, p)
        });
        let cookie_bounds = soup_ranges
            .iter()
            .enumerate()
            .filter_map(|(si, r)| {
                let r = (*r)?;
                cookie_receive
                    .get(r.batch as usize)
                    .is_some_and(|&on| on)
                    .then(|| (r.first, vcod_common::vis::soup_bounds(bsp, si)))
            })
            .collect();
        self.world = Some(WorldGpu {
            sun_casters,
            vertex_buf,
            cod2: cod2_world,
            index_buf,
            bind_groups,
            cpu_indices: indices,
            soup_ranges,
            prop_ranges,
            prop_bounds: props.bounds,
            prop_lods: props.lods,
            prop_lod: Vec::new(),
            batch_draws,
            batch_lights,
            draws: Vec::new(),
            stage_batches,
            stages_of_batch,
            animated,
            sky_stages,
            batch_keys,
            shadow_cast,
            cookie_bounds,
            cookie_runs: Vec::new(),
            cookie_receive,
            sun_keep: SunKeepState::Samples(
                vcod_common::static_light::sun_keep_samples(bsp, 16_384),
                vcod_common::static_light::world_reach(bsp),
            ),
            frame_draws: Vec::new(),
            stage_draws_last: 0,
            stage_params_buf,
            stage_params_stride,
            vis: WorldVis::build(bsp),
            gathered: Vec::new(),
            gathered_from: Vec::new(),
            world_gathered: 0,
            gather_scratch: Vec::new(),
            locked: None,
            culled: None,
            soup_model,
            static_lods: bsp.static_lods.clone(),
            coarse_lod: {
                let mut c = vec![false; bsp.soups.len()];
                for &(_, first, n) in bsp.static_lods.iter().flat_map(|l| l.lods.iter().skip(1)) {
                    c[first as usize..(first + n) as usize].fill(true);
                }
                c
            },
            sky_soups: bsp
                .models
                .first()
                .map_or(0..0, |m| {
                    m.first_soup as usize..(m.first_soup + m.num_soups) as usize
                })
                .filter(|&si| {
                    bsp.soups
                        .get(si)
                        .is_some_and(|s| kinds[s.material as usize] == MaterialKind::Sky)
                })
                .map(|si| (si, vcod_common::vis::soup_bounds(bsp, si)))
                .collect(),
            static_models: (0..bsp.models.len()).map(|m| m == 0).collect(),
            model_soups: bsp
                .models
                .iter()
                .map(|m| m.first_soup as usize..(m.first_soup + m.num_soups) as usize)
                .collect(),
            moved: Vec::new(),
            moved_draws: Vec::new(),
            s_axis_buf,
            soup_bounds: dlight::soup_bounds(bsp),
            dlight_batches,
            dlights_of_batch,
            dlight_runs: Vec::new(),
            dlight_indices: Vec::new(),
            dlight_index_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dlight indices"),
                size: 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            shadow_casters: None,
        });
        // the materials were dropped with the old map
        self.resolve_viewmodel_skins(fs);
        laps.lap("finish");
        laps.finish();
        Ok(())
    }

    /// The submodels drawn this frame, each with its entity's pose. One at
    /// the identity stands where the map put it and draws with the world;
    /// any other draws its own soups through a camera carrying its pose, so
    /// both keep the baked lightmap, as retail's brush model ref entity
    /// does. A submodel not listed, one whose entity is not in the snapshot,
    /// draws nowhere: retail's world draw holds model 0's surfaces only.
    pub fn set_submodels(&mut self, models: &[(usize, glam::Mat4)]) {
        let Some(world) = &mut self.world else {
            return;
        };
        let mut want: Vec<bool> = (0..world.static_models.len()).map(|m| m == 0).collect();
        world.moved.clear();
        for &(m, pose) in models {
            if m == 0 || m >= want.len() {
                continue;
            }
            if pose == glam::Mat4::IDENTITY {
                want[m] = true;
            } else {
                world.moved.push((m, pose));
            }
        }
        if want != world.static_models {
            world.static_models = want;
        }
    }

    /// A download reopened the search path: reparse the shader scripts the
    /// next `load_world` classifies against, and drop the fx and HUD textures
    /// resolved off the old paks, misses included.
    pub fn reopen(&mut self, fs: &dyn AssetSource) {
        self.shaders = assets::load_shaders(fs);
        self.shader_lib = material_lib(fs);
        self.lib_warn_count = self.shader_lib.warn_count();
        self.fx.textures.clear();
        self.marks.reopen();
        self.hud_pass.textures.clear();
        self.vm_pass.cache.clear();
        self.model_images.clear();
    }

    /// Drops the map and every model uploaded for it, so no `ModelHandle`
    /// from the old map can index a new upload.
    pub fn unload_world(&mut self) {
        self.world = None;
        self.marks.unload();
        self.coronas.clear();
        self.sky = None;
        self.dynamic.handles.clear();
        self.dynamic.models.clear();
        self.dynamic.placed.clear();
        self.dynamic.instances.clear();
        self.dynamic.bone_mats.clear();
        self.dynamic.light_origins.clear();
        self.dynamic.light_boxes.clear();
        self.dynamic.lighting = None;
        self.dynamic.cod2_light = None;
        self.dynamic.cod4_light = None;
        self.dynamic.materials.clear();
        self.vm_pass.skins.clear();
        self.dynamic.entity_scene_lights.clear();
        self.vis_counts = VisCounts::default();
    }

    /// Quads in draw order (already back-to-front); `time` is the shader
    /// clock in seconds (tcMod, animMap, wave gens). Unseen images resolve
    /// lazily; an unresolvable one warns once and is cached as `None`. Each
    /// run draws once per stage (`fx_stage_draws`).
    /// An fx material's image as the fx pass binds it: a scripted stage
    /// names its image file, a material without a script resolves through
    /// the image map.
    fn fx_texture(
        &self,
        fs: &dyn AssetSource,
        sh: Option<&Material>,
        image: &str,
        scripted: bool,
    ) -> Option<wgpu::BindGroup> {
        let no_map = HashMap::new();
        let images = if scripted {
            &no_map
        } else {
            self.shaders.image_map()
        };
        resolve_fx_texture(
            &self.device,
            &self.queue,
            &self.vm_pass.skin_layout,
            fs,
            images,
            image,
            FxImage {
                gamma: self.image_gamma.as_ref(),
                picmip: self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip)),
                store: self.textures.store(),
                mipmap: !sh.is_some_and(|sh| sh.nomipmaps),
            },
        )
    }

    /// Loads every image the fx materials in `shaders` draw now, so an
    /// effect's first play binds them without reading and uploading them
    /// in its frame (cod4-map-sweep.md, "The soak").
    pub fn prewarm_fx<'a>(
        &mut self,
        fs: &dyn AssetSource,
        shaders: impl IntoIterator<Item = &'a str>,
    ) {
        for name in shaders {
            let sh = self.shader_lib.get(name);
            for (image, scripted) in fx_stage_images(sh, name) {
                if !self.fx.textures.contains_key(image) {
                    let bg = self.fx_texture(fs, sh, image, scripted);
                    self.fx.textures.insert(image.to_string(), bg);
                }
            }
        }
    }

    /// Images the fx pass holds, loaded or not found.
    #[cfg(test)]
    pub(crate) fn fx_images(&self) -> usize {
        self.fx.textures.len()
    }

    pub fn set_fx_quads(&mut self, fs: &dyn AssetSource, quads: Vec<FxQuad>, time: f32) {
        // build_quads is capped by construction; the vertex buffer is sized
        // exactly, so truncate anyway.
        let quads = if quads.len() > MAX_FX_QUADS {
            log::warn!(
                "fx: {} quads exceeds the {MAX_FX_QUADS} cap, truncating",
                quads.len()
            );
            &quads[..MAX_FX_QUADS]
        } else {
            &quads[..]
        };

        // Retail's draw list keys an effect's polygons by material sort
        // first (shader doc 5); within a sort they keep the sim's
        // back-to-front order. An fx material without a script is
        // implicit, straight alpha, and sorts as a blend.
        // One sort lookup per run of a shader, keyed before the (stable)
        // sort, not two hash lookups per comparison.
        let mut sorts: HashMap<&str, f32> = HashMap::new();
        let mut last: Option<(&str, f32)> = None;
        let mut order: Vec<(f32, &FxQuad)> = Vec::with_capacity(quads.len());
        for quad in quads {
            let sort = match last {
                Some((name, sort)) if name == &*quad.shader => sort,
                _ => {
                    let sort = *sorts.entry(&*quad.shader).or_insert_with(|| {
                        self.shader_lib
                            .get(&quad.shader)
                            .map_or(SORT_BLEND, |sh| sh.sort)
                    });
                    last = Some((&*quad.shader, sort));
                    sort
                }
            };
            order.push((sort, quad));
        }
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut verts: Vec<FxVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<FxRun> = Vec::new();
        let mut rest = &order[..];
        'runs: while let Some(&(sort, first)) = rest.first() {
            let name = &*first.shader;
            let len = rest.iter().take_while(|(_, q)| &*q.shader == name).count();
            let (group, tail) = rest.split_at(len);
            rest = tail;
            let sh = self.shader_lib.get(name);
            for draw in fx_stage_draws(sh, name, time) {
                if verts.len() / 4 + group.len() > MAX_FX_DRAWN {
                    log::warn!("fx: stage draws exceed the {MAX_FX_DRAWN} quad cap, truncating");
                    break 'runs;
                }
                if !self.fx.textures.contains_key(draw.image) {
                    let bg = self.fx_texture(fs, sh, draw.image, draw.stage.is_some());
                    self.fx.textures.insert(draw.image.to_string(), bg);
                }
                let [a, b, c, d, tx, ty] = draw.uv;
                let distortion = sh.and_then(|sh| sh.distortion);
                let feather = match distortion {
                    Some(_) => 0.0,
                    None => fx_feather(sh, &draw, self.cod4_options.zfeather),
                };
                let mut flags = if sh.is_some_and(|sh| sh.feather_view_depth) {
                    draw.flags | FX_VERT_FEATHER_W
                } else {
                    draw.flags
                };
                if distortion.is_some() {
                    flags |= FX_VERT_DISTORT;
                }
                for (_, quad) in group {
                    let rgba = fx_stage_rgba(draw.stage, quad.rgba, self.identity_light, time);
                    let size = quad_half_extent(&quad.verts);
                    // a distortion quad's two scales ride feather and size
                    let (size, feather) = distortion.map_or((size, feather), |d| (d[1], d[0]));
                    for i in 0..4 {
                        let [s, t] = quad.uvs[i];
                        verts.push(FxVert {
                            pos: quad.verts[i],
                            uv: [a * s + c * t + tx, b * s + d * t + ty],
                            rgba,
                            size,
                            flags,
                            feather,
                        });
                    }
                }
                runs.push(FxRun {
                    image: draw.image.to_string(),
                    blend: draw.blend,
                    quads: group.len(),
                    sort,
                    feather: feather > 0.0,
                    view_depth: sh.is_some_and(|sh| sh.feather_view_depth),
                    distort: distortion.is_some(),
                    cloud: None,
                });
            }
        }

        self.queue
            .write_buffer(&self.fx.vertex_buf, 0, bytemuck::cast_slice(&verts));
        self.fx.runs = runs;
    }

    /// One frame of impact marks: `requests` placed at `now_ms` (the
    /// cgame's clock), the pool expired, faded and uploaded after this
    /// frame's `temporary` marks (`crate::blob_shadow`). `enabled` is
    /// `cg_marks`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_marks(
        &mut self,
        fs: &Pk3Fs,
        requests: &[crate::marks::MarkRequest],
        temporary: &[crate::marks::MarkRequest],
        now_ms: i32,
        time: f32,
        enabled: bool,
    ) {
        let ctx = mark_pass::MarkCtx {
            device: &self.device,
            queue: &self.queue,
            fs,
            shader_lib: &self.shader_lib,
            material_layout: &self.material_layout,
            stage_layout: &self.stage_layout,
            white_view: &self.white_view,
            white_lightmap: &self.white_lightmap,
            lightmap_sampler: &self.lightmap_sampler,
            image_gamma: self.image_gamma.as_ref(),
            textures: self.textures,
        };
        if !self.coronas.is_empty() {
            self.marks.prepare_sprites(&ctx, crate::corona::SHADER);
        }
        self.marks
            .update(&ctx, requests, temporary, now_ms, time, enabled);
    }

    /// Builds every mark shader in `shaders` now (its images, stages and
    /// pipelines): a surface's first impact mark then binds only its
    /// lightmap (cod4-map-sweep.md, "The soak").
    pub fn prewarm_marks<'a>(&mut self, fs: &Pk3Fs, shaders: impl IntoIterator<Item = &'a str>) {
        let ctx = mark_pass::MarkCtx {
            device: &self.device,
            queue: &self.queue,
            fs,
            shader_lib: &self.shader_lib,
            material_layout: &self.material_layout,
            stage_layout: &self.stage_layout,
            white_view: &self.white_view,
            white_lightmap: &self.white_lightmap,
            lightmap_sampler: &self.lightmap_sampler,
            image_gamma: self.image_gamma.as_ref(),
            textures: self.textures,
        };
        self.marks.prewarm(&ctx, shaders);
    }

    /// `map_restart` frees every mark (cgame `CG_MapRestart` 0x3002ca60).
    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    /// Dynamic model LODs uploaded to the GPU so far.
    #[cfg(test)]
    pub(crate) fn dynamic_uploads(&self) -> usize {
        self.dynamic.uploads
    }

    /// Mark shaders registered (their images and stages built) so far.
    #[cfg(test)]
    pub(crate) fn mark_shaders(&self) -> usize {
        self.marks.registered()
    }

    /// Live marks in the pool, for the F3 `fx` line.
    pub fn mark_count(&self) -> usize {
        self.marks.live()
    }

    /// The cgame's scene lights for this frame (`entities::BuiltScene`);
    /// entity picks see them after the fx lights.
    pub fn set_entity_lights(&mut self, lights: Vec<SceneLight>) {
        self.dynamic.entity_scene_lights = lights;
    }

    /// The frame's scene lights (at most `MAX_SCENE_LIGHTS`), nearest the
    /// camera first: entity picks and the dlight pass see all of them, the
    /// prop vertex shader the first `MAX_LIGHTS`. An empty slice zeroes
    /// every slot.
    pub fn set_fx_lights(&mut self, lights: &[FxLight]) {
        self.dynamic.scene_lights = entity_light::scene_lights(lights);
        let uniform = FxLightsUniform::from_lights(lights);
        self.queue
            .write_buffer(&self.fx_lights_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Quads in draw order, pixel coords with origin top-left. Textures
    /// resolve lazily by name; an unresolvable one warns once and is cached
    /// as `None`.
    pub fn set_hud_quads(&mut self, fs: &dyn AssetSource, quads: Vec<HudQuad>) {
        let up = HudUpload {
            device: &self.device,
            queue: &self.queue,
            samplers: &self.tex_samplers,
            image_map: self.shaders.image_map(),
            lib: &self.shader_lib,
            textures: &self.textures,
            image_gamma: self.image_gamma.as_ref(),
            identity_light: self.identity_light,
            size: (self.config.width as f32, self.config.height as f32),
        };
        up.upload(
            &mut self.hud_pass,
            fs,
            &quads,
            &mut self.hud_quad_cap_warned,
        );
    }

    /// Quads drawn last frame, for the F3 overlay.
    pub fn hud_quad_count(&self) -> usize {
        self.hud_pass.quad_count
    }

    /// Slice order is draw order (hands, then the gun). Replaces anything set before.
    pub fn set_viewmodel(&mut self, fs: &Pk3Fs, models: &[model::Model]) {
        let before = self.vm_pass.cache.len();
        self.prefetch_skins(fs, models.iter().flat_map(|m| m.materials()));
        let uploaded: Vec<_> = models
            .iter()
            .filter_map(|m| self.vm_upload(fs, m))
            .collect();

        let surfaces: usize = uploaded.iter().map(|m| m.surfaces.len()).sum();
        log::debug!(
            "viewmodel: {} models, {surfaces} drawn surfaces",
            uploaded.len()
        );
        self.vm_pass.models = uploaded;
        self.resolve_viewmodel_skins(fs);
        let after = self.vm_pass.cache.len();
        self.vm_pass.first_uses += after - before;
    }

    /// Parts uploaded by `set_viewmodel` itself so far.
    #[cfg(test)]
    pub(crate) fn viewmodel_first_uses(&self) -> usize {
        self.vm_pass.first_uses
    }

    /// The drawn viewmodel's materials, from this map's entity materials.
    fn resolve_viewmodel_skins(&mut self, fs: &Pk3Fs) {
        let gamma = self.image_gamma;
        self.vm_pass.skins = self
            .vm_pass
            .models
            .iter()
            .map(|m| {
                m.skins
                    .iter()
                    .map(|skin| {
                        self.dynamic.materials.skin(
                            &self.device,
                            &self.queue,
                            (fs, &self.model_images),
                            &self.shader_lib,
                            gamma.as_ref(),
                            skin,
                        )
                    })
                    .collect()
            })
            .collect();
    }

    /// CoD2 model `skins`' images and their stage images decoded on every
    /// core into [`Self::model_images`], ahead of the uploads that take them.
    fn prefetch_skins<'s>(&self, fs: &Pk3Fs, skins: impl Iterator<Item = &'s String>) {
        if fs.game() != vcod_common::game::Game::Cod2 {
            return;
        }
        let lib = &self.shader_lib;
        let mats = skins.filter_map(|skin| lib.get(skin));
        self.model_images.prefetch(fs, prefetch_paths(fs, mats));
    }

    /// Builds the entity and view model pipelines of `keys` now, so no
    /// model's first upload builds one (cod2-client-perf.md, "The sd
    /// level's first frame").
    pub(crate) fn prewarm_entity_pipelines(&mut self, keys: &[EntPipelineKey]) {
        self.dynamic.materials.prewarm(&self.device, keys);
    }

    /// Entity and view model pipeline pairs built so far.
    #[cfg(test)]
    pub(crate) fn entity_pipelines(&self) -> usize {
        self.dynamic.materials.pipeline_count()
    }

    /// Uploads viewmodel parts into the cache without drawing them, so a
    /// later `set_viewmodel` of the same parts uploads nothing.
    pub fn preload_viewmodel(&mut self, fs: &Pk3Fs, models: &[model::Model]) {
        let fresh: Vec<&model::Model> = models
            .iter()
            .filter(|m| !self.vm_pass.cache.contains_key(&vm_cache_key(m)))
            .collect();
        self.prepare_skins(
            fs,
            &drawn_skins(fresh.iter().map(|m| (m.surfaces(), m.materials()))),
        );
        for m in models {
            self.vm_upload(fs, m);
        }
        self.prepared_skins.clear();
    }

    /// `models` uploaded as [`Self::upload_dynamic_model`] uploads each,
    /// in order, with their skins' entity pipelines built and their images
    /// decoded (and on CoD1 mipped) on every core first: a level's
    /// prewarm (cod4-client-perf.md 32).
    pub fn upload_dynamic_models(
        &mut self,
        fs: &Pk3Fs,
        models: &[&model::Model],
    ) -> Vec<Option<ModelHandle>> {
        // the skins the uploads will read: every LOD of a model whose LOD 0
        // draws, as `upload_dynamic_model` stops at an empty LOD 0
        let meshes = models
            .iter()
            .filter(|m| {
                m.lod_mesh(0).is_some_and(|(s, _)| {
                    s.iter()
                        .any(|s| !s.verts.is_empty() && !s.indices.is_empty())
                })
            })
            .flat_map(|m| (0..m.lods.len()).filter_map(|i| m.lod_mesh(i)));
        let skins = drawn_skins(meshes);
        let keys = self
            .dynamic
            .materials
            .skin_pipeline_keys(fs, &self.shader_lib, &skins);
        self.dynamic.materials.prewarm(&self.device, &keys);
        self.prepare_skins(fs, &skins);
        let out = models
            .iter()
            .map(|m| self.upload_dynamic_model(fs, m))
            .collect();
        self.prepared_skins.clear();
        out
    }

    /// `skins` decoded on every core ahead of a batch of uploads: CoD2's
    /// and CoD4's colour, normal and specular maps into
    /// [`Self::model_images`], CoD1's skin images decoded and mipped into
    /// [`Self::prepared_skins`].
    fn prepare_skins(&mut self, fs: &Pk3Fs, skins: &[String]) {
        if fs.game() == vcod_common::game::Game::Cod2 {
            self.prefetch_skins(fs, skins.iter());
            return;
        }
        let (lib, images) = (&self.shader_lib, &self.model_images);
        let (textures, gamma) = (self.textures, self.image_gamma);
        let fresh: Vec<&String> = skins
            .iter()
            .filter(|s| !self.prepared_skins.contains_key(s.as_str()))
            .collect();
        let prepared = vcod_common::laps::par_map(&fresh, |skin| {
            prepare_skin(fs, lib, images, &textures, gamma.as_ref(), skin)
        });
        for (skin, p) in fresh.into_iter().zip(prepared) {
            self.prepared_skins.insert(skin.clone(), p);
        }
    }

    /// A model skin's image as an upload binds it: the batch's when
    /// [`Self::prepare_skins`] made it, else decoded and mipped now.
    fn skin_image(&self, fs: &Pk3Fs, skin: &str) -> PreparedImage {
        match self.prepared_skins.get(skin) {
            Some(p) => p.clone(),
            None => prepare_skin(
                fs,
                &self.shader_lib,
                &self.model_images,
                &self.textures,
                self.image_gamma.as_ref(),
                skin,
            ),
        }
    }

    /// A viewmodel part's upload, out of the cache after the first time.
    fn vm_upload(&mut self, fs: &Pk3Fs, m: &model::Model) -> Option<Rc<VmModel>> {
        let key = vm_cache_key(m);
        if let Some(hit) = self.vm_pass.cache.get(&key) {
            return Some(hit.clone());
        }
        let Some(model) = upload_vm_model(
            &self.device,
            &self.queue,
            &self.vm_pass,
            m.surfaces(),
            m.materials(),
            &|skin| self.skin_image(fs, skin),
        ) else {
            log::warn!("viewmodel {}: no drawable surfaces, skipping it", m.name);
            return None;
        };
        let model = Rc::new(model);
        self.vm_pass.cache.insert(key, model.clone());
        Some(model)
    }

    /// Same upload as the viewmodel (bind pose baked into the mesh), every
    /// LOD; a model whose LOD 0 is empty returns `None`.
    pub fn upload_dynamic_model(
        &mut self,
        fs: &Pk3Fs,
        model: &model::Model,
    ) -> Option<ModelHandle> {
        // every LOD's skins and their stage images, decoded on every core
        self.prefetch_skins(
            fs,
            (0..model.lods.len())
                .filter_map(|i| model.lod_mesh(i))
                .flat_map(|(_, materials)| materials.iter()),
        );
        let mut lods = Vec::with_capacity(model.lods.len());
        for i in 0..model.lods.len() {
            let uploaded = model
                .lod_mesh(i)
                .and_then(|(surfaces, materials)| self.upload_dyn_lod(fs, surfaces, materials));
            if i == 0 && uploaded.is_none() {
                return None;
            }
            lods.push(uploaded);
        }
        self.dynamic.handles.push(DynHandle {
            dists: model.lod_dists(),
            lods,
            bone_boxes: bone_boxes(model),
            view_boxes: bone_boxes_of(model.lods.iter().flat_map(|l| &l.surfaces)),
        });
        Some(ModelHandle(self.dynamic.handles.len() - 1))
    }

    /// `h`, uploaded from `model`, drawn without the LOD 0 surfaces whose
    /// `keep` entry (by index into `model`'s LOD 0 surfaces) is false: a
    /// new handle onto the same buffers, skins and materials, so nothing
    /// is uploaded (cod4-client-perf.md 18). Its other LODs are `h`'s.
    /// `None` when `h` is stale or nothing is kept.
    pub fn dynamic_model_hiding(
        &mut self,
        h: ModelHandle,
        model: &model::Model,
        keep: &[bool],
    ) -> Option<ModelHandle> {
        let d = &mut self.dynamic;
        let full = d.handles.get(h.0)?;
        let lod0 = &d.models[(*full.lods.first()?)?];
        let kept = |src: u32| keep.get(src as usize).copied().unwrap_or(true);
        let (surfaces, skins): (Vec<_>, Vec<_>) = lod0
            .vm
            .surfaces
            .iter()
            .zip(&lod0.skins)
            .filter(|(s, _)| kept(s.src))
            .map(|(s, k)| (s.clone(), k.clone()))
            .unzip();
        if surfaces.is_empty() {
            return None;
        }
        let vm = VmModel {
            skins: lod0
                .vm
                .surfaces
                .iter()
                .zip(&lod0.vm.skins)
                .filter(|(s, _)| kept(s.src))
                .map(|(_, k)| k.clone())
                .collect(),
            surfaces,
            ..lod0.vm.clone()
        };
        let lit = skins.iter().any(|s| s.lit);
        let tris = vm.surfaces.iter().map(|s| s.index_count as usize / 3).sum();
        let mut lods = full.lods.clone();
        let dists = full.dists.clone();
        let lod0_surfaces = model.lods.first().map_or(&[][..], |l| &l.surfaces[..]);
        let bone_boxes = bone_boxes_of(
            lod0_surfaces
                .iter()
                .enumerate()
                .filter(|(i, _)| kept(*i as u32))
                .map(|(_, s)| s),
        );
        let view_boxes = full.view_boxes.clone();
        d.models.push(DynModel {
            vm,
            skins,
            lit,
            tris,
        });
        lods[0] = Some(d.models.len() - 1);
        d.handles.push(DynHandle {
            dists,
            lods,
            bone_boxes,
            view_boxes,
        });
        Some(ModelHandle(d.handles.len() - 1))
    }

    fn upload_dyn_lod(
        &mut self,
        fs: &Pk3Fs,
        surfaces: &[model::Surface],
        materials: &[String],
    ) -> Option<usize> {
        let vm = &self.vm_pass;
        let uploaded = upload_vm_model(
            &self.device,
            &self.queue,
            vm,
            surfaces,
            materials,
            &|skin| self.skin_image(fs, skin),
        )?;
        let gamma = self.image_gamma;
        let skins: Vec<_> = uploaded
            .skins
            .iter()
            .map(|skin| {
                self.dynamic.materials.skin(
                    &self.device,
                    &self.queue,
                    (fs, &self.model_images),
                    &self.shader_lib,
                    gamma.as_ref(),
                    skin,
                )
            })
            .collect();
        let lit = skins.iter().any(|s| s.lit);
        let tris = uploaded
            .surfaces
            .iter()
            .map(|s| s.index_count as usize / 3)
            .sum();
        self.dynamic.uploads += 1;
        self.dynamic.models.push(DynModel {
            vm: uploaded,
            skins,
            lit,
            tris,
        });
        Some(self.dynamic.models.len() - 1)
    }

    /// Instances past `MAX_DYNAMIC_INSTANCES` are dropped (warned once); a
    /// stale handle is skipped. Written to the GPU in [`Self::render`].
    pub fn set_dynamic_models(&mut self, instances: &[DynamicModelInstance]) {
        static OVERFLOW_WARNED: std::sync::Once = std::sync::Once::new();

        let mut handles = Vec::with_capacity(instances.len());
        let mut items = Vec::with_capacity(instances.len());
        for (src, inst) in instances.iter().enumerate() {
            if handles.len() == MAX_DYNAMIC_INSTANCES {
                OVERFLOW_WARNED.call_once(|| {
                    log::warn!(
                        "more than {MAX_DYNAMIC_INSTANCES} live entities this frame; dropping the rest"
                    );
                });
                break;
            }
            let ModelHandle(idx) = inst.model;
            if idx >= self.dynamic.handles.len() {
                continue;
            }
            handles.push((src, inst));
            items.push((idx, inst.transform.to_cols_array(), inst.bones.as_deref()));
        }
        let (raw, bone_mats) = pack_instances(&items);
        self.dynamic.placed = handles
            .into_iter()
            .zip(raw)
            .map(|((src, inst), raw)| Placed {
                handle: inst.model.0,
                src,
                raw,
                lod_origin: inst.transform.w_axis.truncate(),
                light_origin: inst.light_origin,
                shader_time: inst.shader_time,
            })
            .collect();
        self.dynamic.bone_mats = bone_mats;
    }

    /// Picks each placed instance's LOD from the view; one past its last LOD
    /// is not drawn (docs/research/xmodel-v14-format.md, "LOD selection").
    /// Entities carry no refEntity scale. `time` is the frame's material
    /// clock, which an instance's own `shader_time` is taken against.
    fn pick_dynamic_lods(&mut self, eye: glam::Vec3, view: &model::LodView, time: f32) {
        let d = &mut self.dynamic;
        d.instances.clear();
        d.instance_src.clear();
        d.instance_placed.clear();
        d.light_origins.clear();
        d.light_boxes.clear();
        // CoD4: each instance's posed world box picks its spot or omni light
        let boxes: Vec<_> = match d.cod4_light {
            Some(_) => d.placed.iter().map(|p| posed_box(d, p)).collect(),
            None => Vec::new(),
        };
        let mut counts = LodCounts::default();
        for (i, p) in d.placed.iter().enumerate() {
            let h = &d.handles[p.handle];
            let picked = model::select_lod(&h.dists, view.distance(eye, p.lod_origin, 0.0));
            let Some(lod) = picked else {
                counts.gone += 1;
                continue;
            };
            if let Some(n) = counts.per_lod.get_mut(lod) {
                *n += 1;
            }
            let Some(m) = h.lods[lod] else {
                continue;
            };
            counts.tris += d.models[m].tris;
            let mut raw = p.raw;
            raw.time_offset = p.shader_time.map_or(0.0, |t| t - time);
            d.instances.push((m, raw));
            d.instance_src.push(p.src);
            d.instance_placed.push(i);
            d.light_origins.push(p.light_origin);
            if let Some(&b) = boxes.get(i) {
                d.light_boxes
                    .push(b.unwrap_or((p.light_origin, p.light_origin)));
            }
        }
        d.lod_counts = counts;
    }

    /// Picks each instance's lights (one pick per distinct origin, so a
    /// player's parts share one) and points its `light_set` at them.
    fn pick_entity_lights(&mut self) -> Vec<GpuLightSet> {
        let d = &mut self.dynamic;
        if let Some(c4) = &d.cod4_light {
            // CoD4: a lit model's set is its own block, one per distinct
            // lighting origin as `R_AllocModelLighting`'s cache keeps, over
            // the box its instances span (a scene entity's)
            let mut origins: Vec<(glam::Vec3, [glam::Vec3; 2])> = Vec::new();
            let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
            let ents = d
                .instances
                .iter_mut()
                .zip(&d.light_origins)
                .zip(&d.light_boxes);
            for (((model, raw), origin), &(lo, hi)) in ents {
                if !d.models[*model].lit {
                    raw.light_set = 0;
                    continue;
                }
                let key = origin.to_array().map(f32::to_bits);
                raw.light_set = *seen.entry(key).or_insert_with(|| {
                    origins.push((*origin, [lo, hi]));
                    origins.len() as u32 - 1
                });
                let b = &mut origins[raw.light_set as usize].1;
                *b = [b[0].min(lo), b[1].max(hi)];
            }
            // block i for set i; past the volume's blocks (the last is the
            // view model's) the last entity block is shared
            let last = entity_light::COD4_VOLUME_BLOCKS - 2;
            let (mut sets, blocks): (Vec<GpuLightSet>, Vec<_>) = origins
                .iter()
                .enumerate()
                .map(|(i, (o, b))| c4.set(*o, *b, (i as u32).min(last), self.identity_light))
                .unzip();
            d.materials.cod4_volume.write_blocks(&self.queue, &blocks);
            if sets.is_empty() {
                sets.push(GpuLightSet::default());
            }
            return sets;
        }
        if let Some(c2) = &d.cod2_light {
            // CoD2: a lit model's light set is its grid sample
            let mut sets = Vec::new();
            let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
            for ((model, raw), origin) in d.instances.iter_mut().zip(&d.light_origins) {
                if !d.models[*model].lit {
                    raw.light_set = 0;
                    continue;
                }
                let key = origin.to_array().map(f32::to_bits);
                raw.light_set = *seen.entry(key).or_insert_with(|| {
                    sets.push(c2.entity_set(*origin, self.identity_light));
                    sets.len() as u32 - 1
                });
            }
            if sets.is_empty() {
                sets.push(GpuLightSet::default());
            }
            return sets;
        }
        let Some(lighting) = d.lighting.as_mut() else {
            return vec![GpuLightSet::default()];
        };
        let mut scene = d.scene_lights.clone();
        scene.extend_from_slice(&d.entity_scene_lights);
        scene.truncate(crate::fx::sim::MAX_SCENE_LIGHTS);
        if !lights_models(self.dynamic_light) {
            scene.clear();
        }
        let mut sets = Vec::new();
        let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
        for ((model, raw), origin) in d.instances.iter_mut().zip(&d.light_origins) {
            // retail picks only for a model with a lit surface (0x50e3e0)
            if !d.models[*model].lit {
                raw.light_set = 0;
                continue;
            }
            let key = origin.to_array().map(f32::to_bits);
            raw.light_set = *seen.entry(key).or_insert_with(|| {
                let l = lighting.entity_lights(*origin, &scene);
                sets.push(entity_light::pack(&l, None, self.identity_light));
                sets.len() as u32 - 1
            });
        }
        if sets.is_empty() {
            sets.push(GpuLightSet::default());
        }
        sets
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        if let Target::Surface(s) = &self.target {
            s.configure(&self.device, &self.config);
        }
        self.msaa_view = create_msaa_view(&self.device, self.world_format, w, h);
        self.depth_view = create_depth_view(&self.device, w, h);
        self.gamma.resize(&self.device, w, h);
        self.ssao.resize(&self.device, &self.depth_view, w, h);
        self.soft.resize(&self.device, &self.depth_view, w, h);
        self.scene_copy.resize(&self.device, self.soft.linz(), w, h);
        self.post.resize(&self.device, w, h);
        self.dof.resize(&self.device, w, h);
        let src = if self.effects_read_post {
            self.post.world_view()
        } else {
            self.gamma.scene_view()
        };
        self.bloom.resize(&self.device, src, w, h);
        self.shafts
            .resize(&self.device, src, &self.depth_view, w, h);
        self.screen_blend
            .resize(&self.device, self.gamma.scene_view(), w, h);
        self.cod2_glow.resize(&self.device, w, h);
        self.dyn_shadows = cod2_cookie::DynamicShadows::new(&self.device, &self.queue, w, h);
        self.camera_bg = camera_bind_group(
            &self.device,
            &self.camera_layout,
            &self.camera_buf,
            &self.fx_lights_buf,
            &self.shadow_receive(),
            &self.tex_samplers,
            self.dyn_shadows.view(),
            &self.sun_shadow,
        );
        self.moved_cams.clear();
    }

    /// The shellshock's screen blend for the next frame
    /// (`play::shock::ShockFrame::blend_ms`): `Some(decay)` blends the
    /// last capture over the 3D view and captures again, `None` drops it.
    pub fn set_screen_blend(&mut self, decay_ms: Option<i32>) {
        self.shock_blend = decay_ms;
    }

    /// This frame's CoD4 depth of field (`cod4_client`); drawn under
    /// `r_dof_enable` when [`crate::cod4_dof::Dof::used`].
    pub fn set_cod4_dof(&mut self, dof: crate::cod4_dof::Dof) {
        self.cod4_dof = Some(dof);
    }

    /// A CoD4 session's shellshock screen for the next frame only
    /// (`play::cod4_shock::Screen`); without one the frame takes
    /// [`Self::set_screen_blend`]'s.
    pub fn set_cod4_shock(&mut self, screen: Option<crate::play::cod4_shock::Screen>) {
        self.cod4_shock = screen;
    }

    /// CoD2's view blur for the next frame, 480-high virtual units (a
    /// `blurWorld` menu's, `frontend::Ui::blur_world`); 0 draws it sharp.
    pub fn set_world_blur(&mut self, radius: f32) {
        self.world_blur = radius;
    }

    /// `vc_grade`, `vc_vignette` and `vc_grain`, applied from the next frame.
    pub fn set_post(&mut self, settings: crate::post::PostSettings) {
        self.post_settings = settings;
    }

    /// A CoD4 level's vision set for the next frame: the film tweak runs
    /// in the post pass and the glow replaces CoD2's default one (`None`
    /// draws none). `None` leaves CoD1's and CoD2's own.
    pub fn set_level_look(&mut self, look: Option<LevelLook>) {
        self.level_look = look;
    }

    /// Whether the world draws into the float target (`vc_hdr` when the
    /// renderer was built).
    pub fn hdr(&self) -> bool {
        self.world_format == HDR_FORMAT
    }

    /// `vc_bloom` (0 off) and `vc_bloomThreshold`, applied from the next frame.
    pub fn set_bloom(&mut self, intensity: f32, threshold: f32) {
        self.bloom_intensity = intensity.max(0.0);
        self.bloom_threshold = threshold;
    }

    /// `r_textureMode`, applied from the next frame: new samplers and the
    /// group-0 bind groups that hold them, as `GL_TextureMode` (0x4ea530)
    /// resets every mipmapped image's filter in place.
    pub fn set_texture_filter(&mut self, filter: texture_settings::TextureFilter) {
        if filter == self.textures.filter {
            return;
        }
        self.textures.filter = filter;
        self.tex_samplers = TexSamplers::new(&self.device, filter);
        self.camera_bg = camera_bind_group(
            &self.device,
            &self.camera_layout,
            &self.camera_buf,
            &self.fx_lights_buf,
            &self.shadow_receive(),
            &self.tex_samplers,
            self.dyn_shadows.view(),
            &self.sun_shadow,
        );
        self.moved_cams.clear();
        self.vm_pass.uniform_bg = vm_uniform_group(
            &self.device,
            &self.vm_pass.uniform_layout,
            &self.vm_pass.uniform_buf,
            &self.tex_samplers,
        );
        if let Some(flare) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            flare.set_samplers(&self.device, &self.tex_samplers);
        }
        for tex in self.hud_pass.textures.values_mut().flatten() {
            tex.bg = hud_bind_group(
                &self.device,
                &self.hud_pass.texture_layout,
                &self.tex_samplers,
                &tex.view,
                tex.clamp,
            );
        }
    }

    /// The `sc_*` dvars CoD2's shadow cookies read.
    pub fn set_cookie_settings(&mut self, s: CookieSettings) {
        self.cookies.settings = s;
    }

    /// The scene entities that may cast a shadow cookie this frame, each a
    /// range of the instances last handed to [`Self::set_dynamic_models`].
    pub fn set_cookie_entities(&mut self, entities: Vec<CookieEntity>) {
        self.cookies.entities = entities;
    }

    /// `vc_shadows`: builds the shadow pass on the first frame it is on,
    /// drops it when it goes off.
    pub fn set_shadows(&mut self, on: bool) {
        if on == self.shadows.is_some() {
            return;
        }
        self.shadows = on.then(|| {
            let [vertex, instance] = crate::entity_material::vertex_layouts();
            let mats = &self.dynamic.materials;
            shadows::ShadowPass::new(
                &self.device,
                &self.queue,
                self.world_format,
                msaa_samples(),
                shadows::EntLayouts {
                    skin: &mats.skin_layout,
                    bones: &self.dynamic.bone_layout,
                    params: &mats.params_layout,
                    vertex,
                    instance,
                },
                shadows::WorldLayouts {
                    material: &self.material_layout,
                    stage: &self.stage_layout,
                },
            )
        });
        // the camera groups bind the receiver inputs of the new state
        self.camera_bg = camera_bind_group(
            &self.device,
            &self.camera_layout,
            &self.camera_buf,
            &self.fx_lights_buf,
            &self.shadow_receive(),
            &self.tex_samplers,
            self.dyn_shadows.view(),
            &self.sun_shadow,
        );
        self.moved_cams.clear();
    }

    fn shadow_receive(&self) -> shadows::Receive<'_> {
        self.shadows
            .as_ref()
            .map_or_else(|| self.shadow_off.receive(), shadows::ShadowPass::receive)
    }

    /// The F3 `shadow:` line while `vc_shadows` is on.
    pub fn shadow_debug(&self) -> Option<String> {
        self.shadows.as_ref().map(shadows::ShadowPass::debug_line)
    }

    /// `r_gamma`, already clamped to retail's range, and retail's overbright
    /// bits (`gamma::overbright_bits`); applied from the next frame.
    pub fn set_gamma(&mut self, gamma: f32, overbright: u32) {
        self.gamma.set_gamma(&self.queue, gamma, overbright);
        self.identity_light = crate::gamma::identity_light(overbright);
    }

    /// `vc_bicubic`, applied from the next frame.
    pub fn set_bicubic(&mut self, on: bool) {
        self.bicubic = on;
    }

    /// `vc_sunShafts` (0 off), applied from the next frame.
    pub fn set_sun_shafts(&mut self, strength: f32) {
        self.shafts.set_strength(strength);
    }

    /// `r_flares`: off, the sun's flare and washes neither step nor draw.
    pub fn set_flares(&mut self, on: bool) {
        self.flares = on;
    }

    /// `r_drawSun`: off, the sky stage neither draws the disc nor adds the
    /// sun's flare (0x4e84d0); the washes still fade.
    pub fn set_draw_sun(&mut self, on: bool) {
        self.draw_sun = on;
    }

    /// `vc_softParticles`, applied from the next frame; 0 is off.
    pub fn set_soft_particles(&mut self, scale: f32) {
        self.soft.set_scale(&self.queue, scale);
    }

    /// `vc_ssao`, applied from the next frame; 0 is off.
    pub fn set_ssao(&mut self, strength: f32) {
        self.ssao.set_strength(strength);
    }

    /// The F3 `ssao` line: strength and smoothed GPU ms for the scene
    /// passes and the SSAO passes alone.
    pub fn ssao_line(&self) -> String {
        let s = self.ssao.strength();
        let head = if s > 0.0 {
            format!("ssao {s}")
        } else {
            "ssao off".to_string()
        };
        match self.ssao.timer().map(|t| t.ms()) {
            Some((scene, Some(ao))) => format!("{head}  gpu scene {scene:.2}ms ao {ao:.2}ms"),
            Some((scene, None)) => format!("{head}  gpu scene {scene:.2}ms"),
            None => format!("{head}  (no gpu timestamps)"),
        }
    }

    /// The `r_gamma` textures bake in from the next `load_world` (retail
    /// reloads every image per map), or `None` with device gamma.
    /// `r_swapInterval`, read every frame (`GLimp_EndFrame` 0x50a610 calls
    /// `wglSwapIntervalEXT` when it changes): 0 presents without waiting
    /// for the vertical blank.
    pub fn set_swap_interval(&mut self, on: bool) {
        let mode = if on {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            if let Target::Surface(s) = &self.target {
                s.configure(&self.device, &self.config);
            }
        }
    }

    /// CoD4's Options switches (cod4-front-end.md 9).
    pub fn set_cod4_options(&mut self, options: Cod4Options) {
        self.cod4_options = options;
    }

    /// The last frame's draws and state sets.
    #[cfg(test)]
    pub(crate) fn census(&self) -> PassCensus {
        self.census.get()
    }

    /// The last frame's GPU writes in order ([`FrameWrite`]).
    #[cfg(test)]
    pub(crate) fn frame_writes(&self) -> Vec<FrameWrite> {
        self.frame_writes.borrow().clone()
    }

    /// `r_vc_depthprepass` (cod4-client-perf.md 12).
    /// Whether a CoD4 feather's `floatZ` comes from the depth prepass
    /// ([`Self::floatz_prepass`]) or the copy at the split.
    #[cfg(test)]
    pub(crate) fn set_floatz_prepass(&mut self, on: bool) {
        self.floatz_prepass = on;
    }

    /// Off: the cookie receivers keep their own depth (the A/B in
    /// `pipeline_check`).
    #[cfg(test)]
    pub fn set_cookies_on_prepass(&mut self, on: bool) {
        self.cookies_on_prepass = on;
    }

    pub fn set_depth_prepass(&mut self, on: bool) {
        self.depth_prepass = on;
    }

    /// `r_lodscale` and `r_lodbias`, read every frame like retail's
    /// `RE_RenderScene`.
    pub fn set_lod(&mut self, scale: f32, bias: f32) {
        self.lod_cvars = (scale, bias);
    }

    /// `r_dynamiclight` and `r_dlightQuality`, both read every frame.
    pub fn set_dynamic_lights(&mut self, mode: i32, quality: bool) {
        self.dynamic_light = mode;
        self.dlight_quality = quality;
    }

    /// The texture gamma of the next world load. HUD images reload under
    /// a new one, as retail reloads every image at a map load.
    pub fn set_image_gamma(&mut self, gamma: Option<f32>) {
        let table = gamma.map(|g| crate::gamma::ramp(g, 0));
        if table != self.image_gamma {
            self.hud_pass.textures.clear();
        }
        self.image_gamma = table;
    }

    /// The weapons a gamestate's cgame registers, configstring 7's table
    /// (`None` without a gamestate): their icons go up mipmapped. A changed
    /// set reloads the HUD images.
    pub fn set_hud_weapons(&mut self, weapons: Option<&[Option<vcod_common::weapon::WeaponDef>]>) {
        let names = weapons.map(hud_mip_names).unwrap_or_default();
        if names != self.hud_pass.mip_names {
            self.hud_pass.textures.clear();
            self.hud_pass.mip_names = names;
        }
    }

    /// Reads the next frame `render` draws back for [`Self::take_capture`]
    /// (`screenshot`, `cl_avidemo`): as the screen shows it, or with
    /// `pre_gamma` the framebuffer's bytes before the gamma ramp.
    pub fn request_capture(&mut self, pre_gamma: bool) {
        self.capture_wanted = Some(pre_gamma);
    }

    /// The frame [`Self::request_capture`] asked for, once drawn: `None`
    /// before that, `Some(None)` when it could not be read back.
    pub fn take_capture(&mut self) -> Option<Option<Capture>> {
        self.captured.take()
    }

    /// For a lost or outdated swapchain.
    pub fn reconfigure(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        self.resize(w, h);
    }

    /// The dynamic models' LOD pick, for the debug overlay.
    pub fn lod_counts(&self) -> LodCounts {
        self.dynamic.lod_counts
    }

    /// (world draw calls, dynamic instances, bone matrices) for the debug overlay.
    pub fn debug_counts(&self) -> (usize, usize, usize) {
        (
            self.world.as_ref().map_or(0, |w| w.draws.len()),
            self.dynamic.instances.len(),
            self.dynamic.bone_mats.len(),
        )
    }

    pub fn vis_counts(&self) -> &VisCounts {
        &self.vis_counts
    }

    /// (stage draws emitted last frame, stages dropped at load, unique
    /// shader-lib warnings) for the F3 `shader:` line.
    pub fn shader_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.world.as_ref().map_or(0, |w| w.stage_draws_last),
            self.world.as_ref().map_or(0, |w| {
                w.dlight_runs
                    .iter()
                    .map(|r| w.dlights_of_batch[r.draw.batch as usize].len())
                    .sum()
            }),
            self.shader_dropped,
            self.lib_warn_count,
        )
    }

    /// Name of the map's active sky block, for the F3 `sky` field.
    pub fn sky_name(&self) -> Option<&str> {
        self.sky.as_ref().map(|s| s.name.as_str())
    }

    /// New global fog target from configstring 12; `None` clears it.
    pub fn set_fog(&mut self, fog: Option<FogParams>, now: f32) {
        match fog {
            Some(p) => self.fog.set_target(p, now),
            None => self.fog.clear(),
        }
    }

    /// [`Self::set_fog`] with a fade starting at the last frame's time,
    /// for a caller that does not hold the frame clock (CoD2's session).
    pub fn set_fog_now(&mut self, fog: Option<FogParams>) {
        let now = self.fog.clock;
        self.set_fog(fog, now);
    }

    /// The displayed fog, for the F3 `fog:` line.
    pub fn fog_debug(&self) -> String {
        if !self.fog.set {
            return "off".to_string();
        }
        let f = &self.fog.live;
        let tail = if self.fog.target.is_some() { " ->" } else { "" };
        if f.linear {
            format!("linear {}..{} {:?}{tail}", f.near, f.far, f.color)
        } else {
            format!("exp den {:.6} {:?}{tail}", f.density, f.color)
        }
    }

    /// The profile's GPU group of the main pass's `item` (`frame_prof::Group`):
    /// a world draw on a static model's lit set is `Smodel`; `soft` says
    /// whether an fx run reads the depth copy.
    fn draw_group(
        &self,
        item: SortedItem,
        world_list: &[(f32, u32, usize, u32)],
        ent_draws: &[EntDraw],
        soft: impl Fn(usize) -> bool,
    ) -> crate::frame_prof::Group {
        use crate::frame_prof::Group;
        match item {
            SortedItem::World(wi) => {
                let (_, _, set, i) = world_list[wi];
                let smodel = self.world.as_ref().is_some_and(|world| {
                    self.world_sets(world).nth(set).is_some_and(|(_, fds, _)| {
                        match fds[i as usize].kind {
                            DrawRef::Stage(si) => {
                                world.stage_batches[si as usize].cod2.is_some_and(|b| {
                                    matches!(
                                        b.entry,
                                        cod2::LitEntry::Cod4Model | cod2::LitEntry::Smodel
                                    )
                                })
                            }
                            DrawRef::Legacy(_) => false,
                        }
                    })
                });
                if smodel { Group::Smodel } else { Group::World }
            }
            SortedItem::Model(mi) if ent_draws[mi].gun => Group::Gun,
            SortedItem::Model(_) => Group::Ents,
            SortedItem::Mark(_) => Group::Marks,
            SortedItem::Fx(fi) if soft(fi) => Group::FxSoft,
            SortedItem::Fx(_) => Group::Fx,
        }
    }

    /// Whether `item` is a CoD1 static model draw (`Pass::Prop`), which the
    /// fragment counts keep apart.
    fn is_prop_draw(&self, item: SortedItem, world_list: &[(f32, u32, usize, u32)]) -> bool {
        let SortedItem::World(wi) = item else {
            return false;
        };
        let (_, _, set, i) = world_list[wi];
        self.world.as_ref().is_some_and(|world| {
            self.world_sets(world).nth(set).is_some_and(|(_, fds, _)| {
                matches!(fds[i as usize].kind, DrawRef::Legacy(Pass::Prop { .. }))
            })
        })
    }

    /// `[variant][two_sided][bias]`.
    /// The world's draw sets: the world under the shared camera, then each
    /// moved brush model under its own.
    fn world_sets<'a>(
        &'a self,
        world: &'a WorldGpu,
    ) -> impl Iterator<Item = (&'a Vec<DrawRange>, &'a Vec<FrameDraw>, &'a wgpu::BindGroup)> {
        std::iter::once((&world.draws, &world.frame_draws, &self.camera_bg)).chain(
            world
                .moved_draws
                .iter()
                .zip(&self.moved_cams)
                .map(|((d, o), (_, bg))| (d, o, bg)),
        )
    }

    /// Which `cod4_worldfx` stages the frame's gathered world draws hold:
    /// (feathered, distortion).
    fn world_fx_drawn(&self, world: &WorldGpu) -> (bool, bool) {
        use cod4_worldfx::WorldFx;
        let mut out = (false, false);
        if self.world_fx.is_none() {
            return out;
        }
        for (_, frame_draws, _) in self.world_sets(world) {
            for fd in frame_draws {
                if let DrawRef::Stage(sb) = fd.kind {
                    match world.stage_batches[sb as usize].fx {
                        WorldFx::Feather => out.0 = true,
                        WorldFx::Distort => out.1 = true,
                        WorldFx::None => {}
                    }
                }
            }
        }
        out
    }

    /// One world draw on its pipeline; `bound` tracks the bound pipeline.
    /// `fx_ready` says whether this frame's depth and colour copies are
    /// taken for a `cod4_worldfx` stage: without its copy a feathered one
    /// draws as its plain stage, a distortion one not at all.
    #[allow(clippy::too_many_arguments)]
    fn emit_world_draw(
        &self,
        pass: &mut impl DrawSink,
        world: &WorldGpu,
        draw: &DrawRange,
        kind: DrawRef,
        bound: &mut Option<Bound>,
        groups: &mut WorldGroups,
        time: f32,
        fx_ready: (bool, bool),
        depth: Depth,
    ) {
        if !self.draw_joins {
            *groups = WorldGroups::default();
        }
        if let DrawRef::Stage(sb_idx) = kind
            && world.stage_batches[sb_idx as usize].fx != cod4_worldfx::WorldFx::None
        {
            use cod4_worldfx::WorldFx;
            let sb = &world.stage_batches[sb_idx as usize];
            let ready = match sb.fx {
                WorldFx::Feather => fx_ready.0,
                _ => fx_ready.1,
            };
            let tangents = world.cod2.as_ref().map(|c| &c.tangent_buf);
            let pipe = self
                .world_fx
                .as_ref()
                .filter(|_| ready && (sb.fx == WorldFx::Feather || tangents.is_some()))
                .and_then(|p| p.pipeline(sb.fx, sb.variant, sb.two_sided, sb.bias));
            match pipe {
                Some(p) => {
                    let key = Bound::Fx(sb.fx, sb.variant, sb.two_sided, sb.bias);
                    if *bound != Some(key) {
                        pass.pipeline(p);
                        pass.group(3, self.scene_copy.world_bind_group(), None);
                        if let (WorldFx::Distort, Some(t)) = (sb.fx, tangents) {
                            pass.vertices(1, t.slice(..));
                        }
                        *bound = Some(key);
                        *groups = WorldGroups::default();
                        tally(&self.census, |c| {
                            c.pipelines += 1;
                            c.bind_groups += 1;
                        });
                    }
                    groups.bind(pass, 1, sb.mat.bind_group(time), None, &self.census);
                    groups.bind(
                        pass,
                        2,
                        sb.stage.bind_group(time),
                        Some(
                            u32::try_from(u64::from(sb.slot) * world.stage_params_stride)
                                .expect("stage params slot offset fits a dynamic offset"),
                        ),
                        &self.census,
                    );
                    pass.draw(draw.first..draw.first + draw.count);
                    tally(&self.census, |c| c.world += 1);
                    return;
                }
                None if sb.fx == WorldFx::Distort => return,
                None => {}
            }
        }
        match kind {
            DrawRef::Legacy(want) => {
                if *bound != Some(Bound::Legacy(want)) {
                    pass.pipeline(match want {
                        Pass::Opaque => &self.pipeline,
                        Pass::Solid => &self.solid_pipeline,
                        Pass::Prop { culled, solid } => {
                            &self.prop_pipelines[usize::from(culled)][usize::from(solid)]
                        }
                        Pass::Layer => &self.layer_pipeline,
                        Pass::Overlay => &self.overlay_pipeline,
                        Pass::Stage => return,
                    });
                    *bound = Some(Bound::Legacy(want));
                    *groups = WorldGroups::default();
                    tally(&self.census, |c| c.pipelines += 1);
                }
                let call = &world.batch_draws[draw.batch as usize];
                groups.bind(
                    pass,
                    1,
                    &world.bind_groups[call.bind_group],
                    None,
                    &self.census,
                );
            }
            DrawRef::Stage(sb_idx)
                if let (Some(lit), Some(c2)) =
                    (world.stage_batches[sb_idx as usize].cod2, &world.cod2) =>
            {
                let sb = &world.stage_batches[sb_idx as usize];
                let key = Bound::Lit(
                    sb.variant,
                    sb.two_sided,
                    sb.bias,
                    c2.fs_key(&lit),
                    lit.water,
                    depth,
                );
                let water = c2.water.as_ref().filter(|_| lit.water);
                if *bound != Some(key) {
                    // one lit pipeline after another (a split CoD4 path's,
                    // cod4-client-perf.md 29) shares its layout and buffers:
                    // the groups and tangents bound stay
                    let kept = !lit.water && matches!(*bound, Some(Bound::Lit(.., false, _)));
                    pass.pipeline(match (water, depth) {
                        (Some(w), _) => w.pipeline(sb.variant, sb.two_sided, sb.bias),
                        (None, Depth::Plain) => {
                            c2.pipeline(sb.variant, sb.two_sided, sb.bias, &lit)
                        }
                        (None, Depth::Prepass) => c2.prepass_pipeline(sb.two_sided, None),
                        (None, Depth::FloatZ) => c2.floatz_pipeline(
                            sb.two_sided,
                            c2.takes_floatz(sb.variant, &lit) == Some(true),
                        ),
                        (None, Depth::Equal) => c2.prepass_pipeline(sb.two_sided, Some(&lit)),
                    });
                    if !kept {
                        pass.vertices(1, c2.tangent_buf.slice(..));
                        *groups = WorldGroups::default();
                    }
                    *bound = Some(key);
                    tally(&self.census, |c| c.pipelines += 1);
                }
                let (mat, second) = match water {
                    Some(w) => (&w.material_bgs, &w.sims[lit.lightmap as usize].bg),
                    None => (&c2.material_bgs, &c2.lightmap_bgs[lit.lightmap as usize]),
                };
                groups.bind(pass, 1, &mat[lit.material as usize], None, &self.census);
                groups.bind(pass, 2, second, None, &self.census);
            }
            DrawRef::Stage(sb_idx) => {
                let sb = &world.stage_batches[sb_idx as usize];
                let key = Bound::Stage(sb.variant, sb.two_sided, sb.bias);
                if *bound != Some(key) {
                    pass.pipeline(self.stage_pipeline(sb.variant, sb.two_sided, sb.bias));
                    *bound = Some(key);
                    *groups = WorldGroups::default();
                    tally(&self.census, |c| c.pipelines += 1);
                }
                // animMap stages pick their frame per draw
                groups.bind(pass, 1, sb.mat.bind_group(time), None, &self.census);
                groups.bind(
                    pass,
                    2,
                    sb.stage.bind_group(time),
                    Some(
                        u32::try_from(u64::from(sb.slot) * world.stage_params_stride)
                            .expect("stage params slot offset fits a dynamic offset"),
                    ),
                    &self.census,
                );
            }
        }
        pass.draw(draw.first..draw.first + draw.count);
        tally(&self.census, |c| match depth {
            Depth::Prepass | Depth::FloatZ => c.prepass_world += 1,
            _ => c.world += 1,
        });
    }

    /// The depth prepass's world draws joined. A pass that only writes the
    /// nearest depth (LessEqual, no colour) leaves every sample at its
    /// nearest fragment whatever the draw order, so per set, sidedness and
    /// flag wave (the one material switch `vs_main`'s position reads) the
    /// gathered ranges go in index order and ranges that touch draw as
    /// one; batches of one material sit side by side in the gathered
    /// buffer. With `floatz` the draws [`takes_floatz`] accepts write
    /// `floatZ` too; an alpha-tested one reads its material's colour map,
    /// so its ranges join only within the material. Returns the draws.
    fn encode_prepass_world_joined(
        &self,
        pass: &mut impl DrawSink,
        world: &WorldGpu,
        world_list: &[(f32, u32, usize, u32)],
        floatz: bool,
    ) -> usize {
        let Some(c2) = &world.cod2 else {
            return 0;
        };
        // (set, two-sided, waves, the material when alpha tested), first,
        // count, material, lightmap
        type Range = ((usize, bool, bool, Option<u32>), u32, u32, u32, u32);
        let mut ranges: Vec<Range> = Vec::new();
        for &(_, _, set, i) in world_list {
            let (draws, frame_draws, _) = self
                .world_sets(world)
                .nth(set)
                .expect("collected from these sets");
            let fd = &frame_draws[i as usize];
            let atest = match floatz {
                true => match takes_floatz(world, fd.kind) {
                    Some(atest) => atest,
                    None => continue,
                },
                false if takes_prepass(world, fd.kind) => false,
                false => continue,
            };
            let DrawRef::Stage(sb) = fd.kind else {
                continue;
            };
            let sb = &world.stage_batches[sb as usize];
            let Some(lit) = sb.cod2 else {
                continue;
            };
            let d = &draws[fd.src as usize];
            ranges.push((
                (
                    set,
                    sb.two_sided,
                    c2.waves(lit.material),
                    atest.then_some(lit.material),
                ),
                d.first,
                d.count,
                lit.material,
                lit.lightmap,
            ));
        }
        ranges.sort_unstable_by_key(|r| (r.0, r.1));
        // a batch's stages share its range
        ranges.dedup_by_key(|r| (r.0, r.1));
        let mut n = 0;
        let mut cam_set = None;
        let mut key = None;
        let mut k = 0;
        while k < ranges.len() {
            let (group, first, mut count, material, lightmap) = ranges[k];
            let (set, two_sided, waves, atest) = group;
            k += 1;
            while let Some(&(g2, f2, c, ..)) = ranges.get(k)
                && g2 == group
                && f2 == first + count
            {
                count += c;
                k += 1;
            }
            if cam_set != Some(set) {
                let (_, _, cam) = self
                    .world_sets(world)
                    .nth(set)
                    .expect("collected from these sets");
                pass.group(0, cam, None);
                cam_set = Some(set);
                tally(&self.census, |c| c.bind_groups += 1);
            }
            if key != Some((two_sided, waves, atest)) {
                pass.pipeline(match floatz {
                    true => c2.floatz_pipeline(two_sided, atest.is_some()),
                    false => c2.prepass_pipeline(two_sided, None),
                });
                pass.vertices(1, c2.tangent_buf.slice(..));
                pass.group(1, &c2.material_bgs[material as usize], None);
                pass.group(2, &c2.lightmap_bgs[lightmap as usize], None);
                key = Some((two_sided, waves, atest));
                tally(&self.census, |c| {
                    c.pipelines += 1;
                    c.bind_groups += 2;
                });
            }
            pass.draw(first..first + count);
            tally(&self.census, |c| c.prepass_world += 1);
            n += 1;
        }
        n
    }

    /// `r_vc_depthprepass`' depth-only draws: every world draw
    /// [`takes_prepass`] accepts across the world's sets and, with `ents`,
    /// every non-gun entity surface whose skin [`ent_takes_prepass`].
    /// Its own pass, with no colour target; the scene's first pass loads
    /// the depth it leaves. With `floatz` (a CoD4 frame a feather reads,
    /// cod4-client-perf.md 35) it takes 1.7's set instead, the draws whose
    /// material has a `build floatz` technique and prepass key 0 to 2
    /// ([`takes_floatz`], alpha-tested ones among them), and every draw
    /// writes the view depth into the feather's R32F target, as 1.7's
    /// prepass writes `floatZ` (`R_DepthPrepassCallback`, iw3mp.exe
    /// 0x658f40).
    fn encode_depth_prepass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        world_list: &[(f32, u32, usize, u32)],
        ent_draws: &[EntDraw],
        PrepassParts {
            world: world_draws,
            ents,
            floatz,
            load,
        }: PrepassParts,
        time: f32,
        bundles: Option<&mut WorldBundles>,
    ) -> usize {
        let Some(world) = &self.world else {
            return 0;
        };
        // the cleared far plane reads as the copy's `far`
        let floatz_target = floatz.then(|| wgpu::RenderPassColorAttachment {
            view: self.soft.linz(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match load {
                    true => wgpu::LoadOp::Load,
                    false => wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(crate::camera::Z_FAR),
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                },
                store: wgpu::StoreOp::Store,
            },
        });
        let depth = match floatz {
            true => Depth::FloatZ,
            false => Depth::Prepass,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth prepass"),
            color_attachments: match &floatz_target {
                Some(_) => std::slice::from_ref(&floatz_target),
                None => &[],
            },
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: match load {
                        true => wgpu::LoadOp::Load,
                        false => wgpu::LoadOp::Clear(1.0),
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let pass = &mut pass;
        let mut n = 0;
        let mut bundles = bundles;
        pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
        pass.set_index_buffer(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound = None;
        let mut cam_set = None;
        if !world_draws {
            // the entities' part alone
        } else if let Some(b) = bundles.as_deref_mut()
            && self.draw_joins
        {
            let mut rec = Recording::default();
            rec.vertices(0, world.vertex_buf.slice(..));
            rec.indices(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            n += self.encode_prepass_world_joined(&mut rec, world, world_list, floatz);
            b.draw(
                pass,
                &self.device,
                rec,
                crate::frame_prof::Count::PreBundled,
            );
        } else if self.draw_joins {
            n += self.encode_prepass_world_joined(pass, world, world_list, floatz);
        } else {
            for &(_, _, set, i) in world_list {
                let (draws, frame_draws, cam) = self
                    .world_sets(world)
                    .nth(set)
                    .expect("collected from these sets");
                let fd = &frame_draws[i as usize];
                let takes = match floatz {
                    true => takes_floatz(world, fd.kind).is_some(),
                    false => takes_prepass(world, fd.kind),
                };
                if !takes {
                    continue;
                }
                if cam_set != Some(set) {
                    pass.set_bind_group(0, cam, &[]);
                    cam_set = Some(set);
                    tally(&self.census, |c| c.bind_groups += 1);
                }
                let draw = &draws[fd.src as usize];
                self.emit_world_draw(
                    pass,
                    world,
                    draw,
                    fd.kind,
                    &mut bound,
                    &mut WorldGroups::default(),
                    time,
                    (false, false),
                    depth,
                );
                n += 1;
            }
        }
        if !ents {
            return n;
        }
        match bundles {
            Some(b) => {
                let mut rec = Recording::default();
                n += self.encode_prepass_ents(&mut rec, ent_draws, floatz, depth);
                b.draw(
                    pass,
                    &self.device,
                    rec,
                    crate::frame_prof::Count::EntBundled,
                );
            }
            None => n += self.encode_prepass_ents(pass, ent_draws, floatz, depth),
        }
        n
    }

    /// The depth prepass's entity surfaces ([`Renderer::encode_depth_prepass`])
    /// into `sink`, binding all they read. Returns the draws.
    fn encode_prepass_ents(
        &self,
        sink: &mut impl DrawSink,
        ent_draws: &[EntDraw],
        floatz: bool,
        depth: Depth,
    ) -> usize {
        let mut n = 0;
        let dynamic = &self.dynamic;
        sink.group(0, &self.camera_bg, None);
        sink.group(2, &dynamic.bone_bg, None);
        tally(&self.census, |c| c.bind_groups += 2);
        // (instance, surface) of every entity surface the prepass takes
        let mut surfs: Vec<(u32, u32)> = ent_draws
            .iter()
            .filter(|d| !d.gun)
            .filter(|d| {
                let model = &dynamic.models[dynamic.instances[d.inst as usize].0];
                let skin = &model.skins[d.surf as usize];
                match floatz {
                    true => skin.stages.first().is_some_and(|st| st.floatz),
                    false => ent_takes_prepass(skin),
                }
            })
            .map(|d| (d.inst, d.surf))
            .collect();
        // the nearest depth wins in any order, so an instance's rigid
        // surfaces go in surface order and the touching ones join
        if self.draw_joins {
            surfs.sort_unstable();
        }
        let rigid = |skin: &EntSkin| skin.stages.first().is_some_and(|st| st.rigid);
        let mut bound = EntBound::default();
        let mut bound_model = None;
        sink.vertices(1, dynamic.instance_buf.slice(..));
        let mut k = 0;
        while k < surfs.len() {
            let (i, s) = surfs[k];
            k += 1;
            let model_at = dynamic.instances[i as usize].0;
            let model = &dynamic.models[model_at];
            let skin = &model.skins[s as usize];
            let vm = &model.vm;
            if bound_model != Some(model_at) {
                sink.vertices(0, vm.vertex_buf.slice(..));
                sink.indices(vm.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                bound_model = Some(model_at);
            }
            let surf = &vm.surfaces[s as usize];
            let mut end = surf.first_index + surf.index_count;
            if self.draw_joins && rigid(skin) {
                while let Some(&(i2, s2)) = surfs.get(k)
                    && i2 == i
                    && let Some(next) = vm.surfaces.get(s2 as usize)
                    && rigid(&model.skins[s2 as usize])
                    && next.base_vertex == surf.base_vertex
                    && next.first_index == end
                {
                    end += next.index_count;
                    k += 1;
                }
            }
            draw_ent_stages(
                sink,
                &dynamic.materials,
                EntPass::World,
                &vm.bind_groups[surf.bind_group],
                skin,
                surf,
                (surf.first_index..end, i),
                &mut bound,
                depth,
                &self.census,
            );
            n += 1;
        }
        n
    }

    /// Each light over the opaque surfaces it reaches (`dlight.rs`).
    fn emit_dlights(&self, pass: &mut wgpu::RenderPass<'_>, world: &WorldGpu, time: f32) {
        pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
        pass.set_vertex_buffer(1, world.s_axis_buf.slice(..));
        pass.set_index_buffer(world.dlight_index_buf.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound = None;
        for run in &world.dlight_runs {
            let cam = run
                .moved
                .map_or(&self.camera_bg, |mi| &self.moved_cams[mi].1);
            pass.set_bind_group(0, cam, &[]);
            pass.set_bind_group(
                3,
                &self.dlight.bg,
                &[(u64::from(run.light) * self.dlight.stride) as u32],
            );
            tally(&self.census, |c| c.bind_groups += 2);
            for &db in &world.dlights_of_batch[run.draw.batch as usize] {
                let d = &world.dlight_batches[db as usize];
                let key = (d.blend, d.two_sided, d.bias);
                if bound != Some(key) {
                    pass.set_pipeline(self.dlight.pipeline(d.blend, d.two_sided, d.bias));
                    bound = Some(key);
                    tally(&self.census, |c| c.pipelines += 1);
                }
                tally(&self.census, |c| {
                    c.bind_groups += 2;
                    c.dlights += 1;
                });
                pass.set_bind_group(1, d.mat.bind_group(time), &[]);
                pass.set_bind_group(
                    2,
                    d.stage.bind_group(time),
                    &[u32::try_from(u64::from(d.slot) * world.stage_params_stride)
                        .expect("stage params slot offset fits a dynamic offset")],
                );
                pass.draw_indexed(run.draw.first..run.draw.first + run.draw.count, 0, 0..1);
            }
        }
        pass.set_bind_group(0, &self.camera_bg, &[]);
    }

    /// `vc_shadows`' uniforms and caster passes for this frame; false
    /// when it is off or the map has no world or no sun.
    fn encode_shadow_maps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        camera: &[f32],
        moved: &[glam::Mat4],
    ) -> bool {
        let (Some(sp), Some(world), Some(sun)) = (
            self.shadows.as_mut(),
            self.world.as_mut(),
            self.dynamic.lighting.as_ref().and_then(StaticLighting::sun),
        ) else {
            return false;
        };
        if world.shadow_casters.is_none() {
            world.shadow_casters = Some(shadow_casters(&self.device, world));
        }
        let keep = match std::mem::replace(&mut world.sun_keep, SunKeepState::Done(None)) {
            SunKeepState::Samples(samples, reach) => {
                world.sun_keep = self
                    .dynamic
                    .lighting
                    .as_ref()
                    .and_then(|l| l.sun_keep_job(samples, reach))
                    .map_or(SunKeepState::Done(None), SunKeepState::Running);
                None
            }
            SunKeepState::Running(job) if job.is_finished() => {
                let k = job.join().ok().flatten().map(Arc::new);
                if let Some(k) = &k {
                    log::info!(
                        "vc_shadows: lightmaps measure {:?}, {} cells of their own",
                        k.whole,
                        k.keeps.iter().flatten().count()
                    );
                } else {
                    log::info!("vc_shadows: lightmaps measure nothing");
                }
                world.sun_keep = SunKeepState::Done(k.clone());
                k
            }
            running @ SunKeepState::Running(_) => {
                world.sun_keep = running;
                None
            }
            SunKeepState::Done(keep) => {
                world.sun_keep = SunKeepState::Done(keep.clone());
                keep
            }
        };
        let world = &*world;
        let Some(casters) = &world.shadow_casters else {
            return false;
        };
        sp.prepare(
            &self.queue,
            &sun,
            casters,
            frame.view_proj,
            frame.eye,
            frame.fwd,
            camera,
            (self.config.width as f32, self.config.height as f32),
            moved,
            keep.as_ref(),
        );
        // each entity surface casts through its first depth-writing stage
        let dynamic = &self.dynamic;
        let mats = &dynamic.materials;
        let mut ents = Vec::new();
        for (i, (m, _)) in dynamic.instances.iter().enumerate() {
            let model = &dynamic.models[*m];
            for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                let Some(st) = skin
                    .stages
                    .iter()
                    .find(|st| mats.pipelines[st.pipeline].0.1)
                else {
                    continue;
                };
                ents.push(shadows::EntCaster {
                    vertex_buf: &model.vm.vertex_buf,
                    index_buf: &model.vm.index_buf,
                    instance: i as u32,
                    first_index: surf.first_index,
                    index_count: surf.index_count,
                    base_vertex: surf.base_vertex,
                    image: match st.image {
                        StageImage::Skin => &model.vm.bind_groups[surf.bind_group],
                        StageImage::Own(k) => &mats.images[k],
                    },
                    params_offset: (u64::from(st.slot) * ENT_STAGE_STRIDE) as u32,
                });
            }
        }
        let moved_models: Vec<usize> = world.moved.iter().map(|&(m, _)| m).collect();
        let time = frame.time;
        let cut = |b: u32| match *world.shadow_cast.get(b as usize)? {
            ShadowCast::Cut => Some(shadows::CutBinding {
                mat: &world.bind_groups[world.batch_draws[b as usize].bind_group],
                stage: None,
            }),
            ShadowCast::Stage(sb) | ShadowCast::Deformed(sb) => {
                let sb = &world.stage_batches[sb as usize];
                Some(shadows::CutBinding {
                    mat: sb.mat.bind_group(time),
                    stage: Some((
                        sb.stage.bind_group(time),
                        u32::try_from(u64::from(sb.slot) * world.stage_params_stride)
                            .expect("stage params slot offset fits a dynamic offset"),
                    )),
                })
            }
            ShadowCast::None | ShadowCast::Solid => None,
        };
        sp.encode_maps(
            encoder,
            &world.vertex_buf,
            casters,
            &moved_models,
            &ents,
            &shadows::EntBindings {
                instance_buf: &dynamic.instance_buf,
                instance_stride: DYNAMIC_INSTANCE_STRIDE,
                bones: &dynamic.bone_bg,
                params: &mats.params_bg,
            },
            &cut,
        );
        true
    }

    /// CoD2's shadow cookies for this frame (`cod2_cookie`): picks them
    /// off the scene's entities, draws their casters and blurs them; the
    /// receivers follow ([`Self::encode_cookie_receivers`]).
    fn encode_cookies(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        let d = &self.dynamic;
        let Some(c2) = d.cod2_light.as_ref().filter(|_| self.world.is_some()) else {
            return;
        };
        // each entity's posed box over its placed parts
        let entities = if self.cookies.settings.enable {
            self.cookies.entities.len()
        } else {
            0
        };
        let mut boxes: Vec<Option<(glam::Vec3, glam::Vec3)>> = vec![None; entities];
        let mut owner = HashMap::new();
        for (e, ent) in self.cookies.entities[..entities].iter().enumerate() {
            for part in ent.parts.clone() {
                owner.insert(part, e);
            }
        }
        for p in &d.placed {
            let (Some(&e), Some((lo, hi))) = (owner.get(&p.src), posed_box(d, p)) else {
                continue;
            };
            let b = &mut boxes[e];
            *b = Some(b.map_or((lo, hi), |(a, z)| (a.min(lo), z.max(hi))));
        }
        let cands: Vec<cod2_cookie::Candidate> = self.cookies.entities[..entities]
            .iter()
            .zip(&boxes)
            .map(|(ent, b)| {
                let (mins, maxs) = b.unwrap_or((ent.origin, ent.origin));
                cod2_cookie::Candidate {
                    origin: ent.origin,
                    mins,
                    maxs,
                }
            })
            .collect();
        let sun = glam::Vec3::from(c2.sun().dir);
        let now_ms = (frame.time * 1000.0) as i32;
        let cookies = self
            .cookies
            .prepare(&self.queue, sun, frame.eye, &cands, now_ms);
        let mut draws = Vec::new();
        for (ci, c) in cookies.iter().enumerate() {
            let parts = &self.cookies.entities[c.candidate].parts;
            for (slot, ((m, _), src)) in d.instances.iter().zip(&d.instance_src).enumerate() {
                if !parts.contains(src) {
                    continue;
                }
                let model = &d.models[*m];
                for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                    if !skin.cookie_caster {
                        continue;
                    }
                    let indices = surf.first_index..surf.first_index + surf.index_count;
                    // the instance's last caster draw takes a surface whose
                    // indices follow on: the same triangles, in order
                    if let Some(last) =
                        draws.last_mut().filter(|l: &&mut cod2_cookie::CasterDraw| {
                            l.cookie == ci
                                && l.instance == slot as u32
                                && l.base_vertex == surf.base_vertex
                                && l.indices.end == indices.start
                        })
                    {
                        last.indices.end = indices.end;
                        continue;
                    }
                    draws.push(cod2_cookie::CasterDraw {
                        cookie: ci,
                        vertex_buf: &model.vm.vertex_buf,
                        index_buf: &model.vm.index_buf,
                        indices,
                        base_vertex: surf.base_vertex,
                        instance: slot as u32,
                    });
                }
            }
        }
        crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, draws.len());
        self.cookies.encode(
            encoder,
            &draws,
            &d.instance_buf,
            DYNAMIC_INSTANCE_STRIDE,
            &d.bone_bg,
        );
    }

    fn cookie_receivers(&mut self) -> CookieReceivers<'_> {
        CookieReceivers {
            device: &self.device,
            queue: &self.queue,
            world: self.world.as_ref(),
            cookies: &mut self.cookies,
            dyn_shadows: &mut self.dyn_shadows,
            depth_view: &self.depth_view,
            cod2: self.dynamic.cod2_light.is_some(),
        }
    }

    /// Whether this frame's cookie receivers draw over the world's depth
    /// prepass, between it and the entities' (retail's order, and each
    /// pixel shades once): with the prepass on, one sample.
    fn cookie_receivers_on_prepass(&self) -> bool {
        self.depth_prepass
            && self.world.is_some()
            && self.dynamic.cod2_light.is_some()
            && msaa_samples() == 1
            && self.cookies_on_prepass
    }

    /// The cookies' receivers into the dynamic shadow the main pass's lit
    /// world and models read; with no cookie it takes no sun. With
    /// `prepass` they test against the scene depth the world's prepass
    /// left (`cod2-client-perf.md` 11).
    /// An associated function over the fields it reads, so a caller
    /// holding the scene's targets can run it.
    fn encode_cookie_receivers(
        r: CookieReceivers<'_>,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
        prepass: bool,
    ) {
        let CookieReceivers {
            device,
            queue,
            world,
            cookies,
            dyn_shadows,
            depth_view,
            cod2,
        } = r;
        if !cod2 {
            return;
        }
        let Some(world) = world else {
            return;
        };
        let views: Vec<(glam::Mat4, glam::Mat4)> = std::iter::once(glam::Mat4::IDENTITY)
            .chain(world.moved.iter().map(|&(_, pose)| pose))
            .map(|pose| (frame.view_proj, pose))
            .collect();
        // each receiving soup's cookie mask, off its box
        let mask_of = cookies.receiver_masks();
        let every = mask_of(None);
        let depth = prepass.then_some(depth_view);
        cookies.encode_receivers(
            device,
            queue,
            encoder,
            dyn_shadows,
            depth,
            &views,
            |pass, set| {
                pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
                pass.set_index_buffer(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                // abutting draws of one mask go as one
                let mut run: Option<(std::ops::Range<u32>, u32)> = None;
                let mut add = |pass: &mut wgpu::RenderPass<'_>,
                               next: std::ops::Range<u32>,
                               mask| {
                    match &mut run {
                        Some((at, m)) if at.end == next.start && *m == mask => at.end = next.end,
                        _ => {
                            if let Some((at, m)) = run.replace((next, mask)) {
                                pass.draw_indexed(at, 0, m..m + 1);
                                crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, 1);
                            }
                        }
                    }
                };
                if set == 0 {
                    // retail's per-cookie receiver lists keep a surface whose
                    // box a cookie's volume reaches (0x10027980): each soup
                    // tests the cookies its own box reaches, and over the
                    // prepass, which a receiver writes no depth into, one
                    // that reaches none is left out
                    for (at, b) in &world.cookie_runs {
                        let mask = match b {
                            Some(b) => mask_of(Some(*b)),
                            None => every,
                        };
                        if mask == 0 && prepass {
                            continue;
                        }
                        add(pass, at.clone(), mask);
                    }
                } else {
                    // a moved brush model's draws test every cookie
                    for r in &world.moved_draws[set - 1].0 {
                        if world.cookie_receive.get(r.batch as usize) == Some(&true) {
                            add(pass, r.first..r.first + r.count, every);
                        }
                    }
                }
                if let Some((at, m)) = run {
                    pass.draw_indexed(at, 0, m..m + 1);
                    crate::frame_prof::count(crate::frame_prof::Count::CookieDraws, 1);
                }
            },
        );
    }

    /// CoD4's sun shadow map for this frame's view (cod4_sunshadow), or
    /// its receivers turned off when `sm_enable` is 0 or the map casts none.
    fn encode_sun_shadow(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        #[cfg(test)]
        let before = self.sun_shadow.redraws();
        self.encode_sun_shadow_maps(encoder, frame);
        #[cfg(test)]
        {
            // a depth texel is 4 bytes: the partitions are 1024 square,
            // the spot tiles 512
            let (drawn, sun, spot) = self.sun_shadow.redraws();
            let part: u64 = 1024 * 1024 * 4;
            if sun > before.1 {
                frame_write!(
                    self,
                    "sun shadow static redraw",
                    u64::from(sun - before.1) * part
                );
            }
            if drawn {
                frame_write!(self, "sun shadow atlas (static blit + entities)", 2 * part);
            }
            if spot > before.2 {
                frame_write!(
                    self,
                    "spot shadow static redraw",
                    u64::from(spot - before.2) * part / 4
                );
            }
            let tiles = self.sun_shadow.spots().len();
            if tiles > 0 {
                frame_write!(
                    self,
                    "spot shadow tiles (static blit + entities)",
                    tiles as u64 * part / 4
                );
            }
        }
    }

    fn encode_sun_shadow_maps(&mut self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        let (on, settings) = self.sun_shadow_set;
        let world = self.world.as_ref();
        let sun = self.dynamic.cod4_light.as_ref().map(|l| l.sun_dir);
        let casters = world
            .and_then(|w| w.sun_casters.as_ref())
            .filter(|_| on && sun.is_some());
        let casters = casters.zip(world.map(|w| &w.vertex_buf));
        // every live entity's casting surfaces at its picked LOD
        let d = &self.dynamic;
        let mats = &d.materials;
        let mut draws = Vec::new();
        if casters.is_some() {
            for (slot, (m, _)) in d.instances.iter().enumerate() {
                let model = &d.models[*m];
                for (surf, skin) in model.vm.surfaces.iter().zip(&model.skins) {
                    if skin.sun_caster {
                        draws.push(crate::cod4_sunshadow::EntityCaster {
                            cut: skin.sun_cut.map(|image| match image {
                                StageImage::Skin => &model.vm.bind_groups[surf.bind_group],
                                StageImage::Own(k) => &mats.images[k],
                            }),
                            vertex_buf: &model.vm.vertex_buf,
                            index_buf: &model.vm.index_buf,
                            indices: surf.first_index..surf.first_index + surf.index_count,
                            base_vertex: surf.base_vertex,
                            instance: slot as u32,
                            bounds: d.light_boxes.get(slot).copied(),
                        });
                    }
                }
            }
        }
        let entities = crate::cod4_sunshadow::Entities {
            draws,
            instance_buf: &d.instance_buf,
            bones: &d.bone_bg,
            bone_layout: &d.bone_layout,
            image_layout: &mats.skin_layout,
        };
        let lod = self.cod4_lod;
        self.sun_shadow
            .pick_smodel_lods(casters.map(|c| c.0), frame.eye, lod);
        self.sun_shadow.encode(
            &self.device,
            &self.queue,
            encoder,
            casters,
            Some(&entities),
            (frame.view_proj, frame.eye, frame.fwd, crate::camera::Z_NEAR),
            sun.unwrap_or(glam::Vec3::Z),
            &settings,
        );
        // the spot light's map under `sm_enable` too (`R_GetAllowShadowMaps`),
        // over the same casters
        let lights = d.cod4_light.as_ref().map_or(&[][..], |l| &l.lights[..]);
        // the lights a visible surface stores (`scene.shadowableLightIsUsed`):
        // this frame's world and static model draws
        let mut used = vec![false; lights.len()];
        if let Some(w) = world {
            for r in &w.draws {
                if let Some(u) = w
                    .batch_lights
                    .get(r.batch as usize)
                    .and_then(|&l| used.get_mut(usize::from(l)))
                {
                    *u = true;
                }
            }
        }
        self.sun_shadow.encode_spot(
            &self.device,
            &self.queue,
            encoder,
            casters,
            Some(&entities),
            (lights, &used),
            (frame.eye, frame.fwd, frame.time),
            &self.spot_shadow_set,
        );
    }

    /// The spot light shadow dvars (`r_spotLightShadows`, `sm_spotEnable`
    /// and kin, crate::cod4_spotshadow).
    pub fn set_spot_shadow(&mut self, settings: crate::cod4_spotshadow::Settings) {
        self.spot_shadow_set = settings;
    }

    /// The F3 `spotshadow` line.
    pub fn spot_shadow_line(&self) -> String {
        self.sun_shadow.spot_line()
    }

    /// The shadowed spot lights and their fades, a tile each.
    #[cfg(test)]
    pub fn shadowed_spots(&self) -> Vec<(usize, f32)> {
        self.sun_shadow.spots()
    }

    /// `sm_enable` and the dvars the partitions read (CoD4's sun shadow
    /// map), clamped to their registered ranges.
    pub fn set_sun_shadow(&mut self, on: bool, mut settings: vcod_cod4::sunshadow::Settings) {
        settings.sample_size_near = settings.sample_size_near.clamp(0.0625, 32.0);
        settings.scale = settings
            .scale
            .clamp(vcod_cod4::sunshadow::SHADOW_SCALE_MIN, 1.0);
        self.sun_shadow_set = (on, settings);
    }

    /// The F3 `sunshadow` line.
    pub fn sun_shadow_line(&self) -> String {
        self.sun_shadow.debug_line()
    }

    fn stage_pipeline(
        &self,
        variant: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> &wgpu::RenderPipeline {
        &self.stage_pipelines[variant as usize][usize::from(two_sided)][usize::from(bias)]
    }

    /// The aspect every 3D view projects at: the window's, or a CoD4
    /// session's [`Renderer::set_view_aspect`].
    pub fn aspect(&self) -> f32 {
        self.view_aspect
            .unwrap_or(self.config.width as f32 / self.config.height.max(1) as f32)
    }

    /// CoD4's `r_aspectRatio` (`R_StoreWindowSettings`, iw3mp.exe
    /// 0x5f3500, `camera::cod4_aspect`): the view projects at that aspect
    /// across the whole window, stretched where the window's own differs.
    /// `None` is the window's.
    pub fn set_view_aspect(&mut self, aspect: Option<f32>) {
        self.view_aspect = aspect;
    }

    /// Pixels.
    pub fn screen_size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    /// The visible set and the prop flags that go with it. A placement whose
    /// model failed to load has zero bounds and is never drawn or counted.
    /// `lod` is CoD4's rigid LOD ramp, `(scale, bias)`
    /// ([`vcod_common::mapworld::cod4_lod_ramp`]): each static model keeps
    /// the soups of the LOD it picks, none past its `cullDist`.
    fn cull(
        world: &WorldGpu,
        eye: glam::Vec3,
        view_proj: &glam::Mat4,
        frustum: &Frustum,
        lod: (f32, f32),
    ) -> (Visible, Vec<bool>) {
        let mut v = world.vis.visible(eye, frustum, Some(*view_proj));
        for sl in &world.static_lods {
            let (Some(&(_, first, _)), Some(&(_, last, n))) = (sl.lods.first(), sl.lods.last())
            else {
                continue;
            };
            let all = first as usize..(last + n) as usize;
            if !v.soups[all.clone()].iter().any(|&s| s) {
                continue;
            }
            let keep = sl.pick(eye.into(), lod.0, lod.1).map(|l| {
                let (_, f, n) = sl.lods[l];
                f as usize..(f + n) as usize
            });
            for si in all {
                v.soups[si] = keep.as_ref().is_some_and(|k| k.contains(&si));
            }
        }
        let prop_ok = world
            .prop_bounds
            .iter()
            .map(|&(lo, hi)| {
                (lo, hi) != (glam::Vec3::ZERO, glam::Vec3::ZERO)
                    && world.vis.prop_visible(&v, frustum, lo, hi)
            })
            .collect();
        (v, prop_ok)
    }

    /// `vm` is `None` to draw the world only. Skips the frame if no
    /// swapchain texture can be acquired.
    pub fn render(&mut self, frame: Frame, vm: Option<VmDraw>) {
        #[cfg(test)]
        self.frame_writes.borrow_mut().clear();
        let lod_view = model::LodView::new(
            frame.fov_x,
            crate::camera::fov_y(frame.fov_x, self.aspect()),
            self.lod_cvars.0,
            self.lod_cvars.1,
        );
        self.pick_dynamic_lods(frame.eye, &lod_view, frame.time);
        // `proj` (64) + `time_pad` (16) + eye/fog tail (48) + view forward
        // (16), matching Camera in the WGSL modules.
        self.fog.advance(frame.time);
        let mut camera = [0.0f32; CAMERA_FLOATS];
        camera[..16].copy_from_slice(&frame.view_proj.to_cols_array());
        camera[16] = frame.time;
        camera[17] = self.identity_light;
        camera[18] = if self.bicubic { 1.0 } else { 0.0 };
        // `r_specular` 0 drops a CoD4 set's specular (cod2_world.wgsl).
        camera[19] = if self.cod4_options.specular { 0.0 } else { 1.0 };
        if let Some(c2) = self.world.as_mut().and_then(|w| w.cod2.as_mut())
            && c2.normal_maps_on != self.cod4_options.normal_maps
        {
            c2.set_normal_maps(&self.device, &self.queue, self.cod4_options.normal_maps);
        }
        if self.fog.set && self.cod4_options.fog {
            let f = &self.fog.live;
            camera[20..24].copy_from_slice(&[frame.eye.x, frame.eye.y, frame.eye.z, f.mode()]);
            // GL_FOG_COLOR carries identityLight (0x4d2ad0); the clear doesn't.
            let il = self.identity_light;
            camera[24..28].copy_from_slice(&[
                f.color[0] * il,
                f.color[1] * il,
                f.color[2] * il,
                f.density,
            ]);
            let radial = if self.fog.radial { 1.0 } else { 0.0 };
            camera[28..32].copy_from_slice(&[f.near, f.far, radial, f.start]);
        } else {
            camera[20..24].copy_from_slice(&[frame.eye.x, frame.eye.y, frame.eye.z, 0.0]);
        }
        camera[32..36].copy_from_slice(&[frame.fwd.x, frame.fwd.y, frame.fwd.z, 0.0]);
        camera[36..52].copy_from_slice(&glam::Mat4::IDENTITY.to_cols_array());
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::cast_slice(&camera));
        frame_write!(self, "upload camera", CAMERA_BYTES);
        // CoD4's water height maps at the view's time (cod4-world.md 9.8),
        // held under `r_drawWater` 0 (cod4-front-end.md 9)
        if self.cod4_options.water {
            self.water_time = frame.time;
        }
        if let Some(c2) = self.world.as_mut().and_then(|w| w.cod2.as_mut()) {
            for w in &mut c2.cod4_waters {
                w.update(&self.queue, self.water_time);
            }
        }
        // CoD2's water height maps at the view's time (cod2-materials.md 9.12)
        if let Some(w) = self
            .world
            .as_mut()
            .and_then(|w| w.cod2.as_mut()?.water.as_mut())
        {
            w.update(&self.queue, frame.time);
        }
        let ssao_on = self.ssao.strength() > 0.0 && self.world.is_some();
        if ssao_on {
            self.ssao.prepare(
                &self.queue,
                &crate::ssao::FrameParams {
                    view_proj: frame.view_proj,
                    near: Z_NEAR,
                    far: Z_FAR,
                    fog_mode: camera[23],
                    fog_density: camera[27],
                    fog_near: camera[28],
                    fog_far: camera[29],
                },
            );
        }
        let moved: Vec<glam::Mat4> = self.world.as_ref().map_or_else(Vec::new, |w| {
            w.moved.iter().map(|&(_, pose)| pose).collect()
        });
        while self.moved_cams.len() < moved.len() {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("moved brush model camera"),
                size: CAMERA_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = camera_bind_group(
                &self.device,
                &self.camera_layout,
                &buf,
                &self.fx_lights_buf,
                &self.shadow_receive(),
                &self.tex_samplers,
                self.dyn_shadows.view(),
                &self.sun_shadow,
            );
            self.moved_cams.push((buf, bg));
        }
        for (pose, (buf, _)) in moved.iter().zip(&self.moved_cams) {
            camera[36..52].copy_from_slice(&pose.to_cols_array());
            self.queue
                .write_buffer(buf, 0, bytemuck::cast_slice(&camera));
            frame_write!(self, "upload moved model camera", CAMERA_BYTES);
        }
        if let Some(sky) = &self.sky
            && let Some(farbox) = &sky.farbox
        {
            self.queue.write_buffer(
                &farbox.eye_buf,
                0,
                bytemuck::bytes_of(&[frame.eye.x, frame.eye.y, frame.eye.z, 0.0f32]),
            );
        }
        let t0 = std::time::Instant::now();
        let mut prof = crate::frame_prof::span(crate::frame_prof::Stage::Cull);
        crate::frame_prof::size(self.config.width, self.config.height);
        let frustum = Frustum::from_view_proj(frame.view_proj);
        let cod4_lod = vcod_common::mapworld::cod4_lod_ramp(
            crate::camera::fov_y(frame.fov_x, self.aspect()),
            self.lod_cvars.0,
            self.lod_cvars.1,
        );
        self.cod4_lod = cod4_lod;
        self.sky_drawn = false;
        if let Some(world) = &mut self.world {
            let visible = match frame.cull {
                CullMode::Off => None,
                CullMode::Locked => Some(match world.locked.take() {
                    Some(v) => v,
                    None => Self::cull(world, frame.eye, &frame.view_proj, &frustum, cod4_lod),
                }),
                CullMode::On => {
                    let key = cull_key(frame.eye, &frame.view_proj, cod4_lod);
                    Some(match world.culled.take() {
                        Some((k, v)) if k == key => v,
                        _ => Self::cull(world, frame.eye, &frame.view_proj, &frustum, cod4_lod),
                    })
                }
            };
            // A corona's box goes through the frustum and the visited cells,
            // as a static model's does (0x4e48b8..0x4e4a41).
            let sprites: Vec<crate::corona::Sprite> = self
                .coronas
                .iter()
                .filter(|c| {
                    let (lo, hi) = crate::corona::cull_box(c);
                    match &visible {
                        Some((v, _)) => world.vis.prop_visible(v, &frustum, lo, hi),
                        None => frustum.intersects_aabb(lo, hi),
                    }
                })
                .filter_map(|c| crate::corona::sprite(c, frame.eye, frame.fwd, frame.up))
                .collect();
            self.marks
                .set_sprites(&self.queue, crate::corona::SHADER, &sprites);
            // A static model reads the LOD table like an entity, at its origin
            // and scale (docs/research/xmodel-v14-format.md, "LOD selection").
            let prop_lod: Vec<Option<u8>> = world
                .prop_lods
                .iter()
                .map(|l| {
                    let d = lod_view.distance(frame.eye, l.origin, l.scale);
                    model::select_lod(&l.dists, d).map(|i| i as u8)
                })
                .collect();
            let lod_shown = |p: u32, l: u8| prop_lod[p as usize] == Some(l);
            let mut props_drawn = 0usize;
            prof.next(crate::frame_prof::Stage::Gather);
            // unculled, a static model draws its LOD 0; culled, the pick
            // in `cull` left one LOD's soups
            let culled = visible.is_some();
            let shown = |si: usize| {
                world.static_models[world.soup_model[si] as usize]
                    && (culled || !world.coarse_lod[si])
            };
            let ranges: Vec<IndexRange> = match &visible {
                None => world
                    .soup_ranges
                    .iter()
                    .enumerate()
                    .filter_map(|(si, r)| if shown(si) { *r } else { None })
                    .chain(
                        world
                            .prop_ranges
                            .iter()
                            .filter(|&&(p, l, _)| lod_shown(p, l))
                            .map(|&(_, _, r)| r),
                    )
                    .collect(),
                Some((v, prop_ok)) => {
                    props_drawn = prop_ok
                        .iter()
                        .zip(&prop_lod)
                        .filter(|&(&ok, l)| ok && l.is_some())
                        .count();
                    world
                        .soup_ranges
                        .iter()
                        .zip(&v.soups)
                        .enumerate()
                        .filter_map(|(si, (r, &vis))| if vis && shown(si) { *r } else { None })
                        .chain(
                            world
                                .prop_ranges
                                .iter()
                                .filter(|&&(p, l, _)| prop_ok[p as usize] && lod_shown(p, l))
                                .map(|&(_, _, r)| r),
                        )
                        .collect()
                }
            };
            let same_world = ranges == world.gathered_from;
            if same_world {
                // last frame's moved brush models follow the world's part
                world.gathered.truncate(world.world_gathered);
            } else {
                world.draws = gather(
                    &world.cpu_indices,
                    ranges.iter().copied(),
                    world.batch_draws.len(),
                    &mut world.gathered,
                    &mut world.gather_scratch,
                );
                // the receiving soups where the gather put them, for the
                // cookie masks: a batch's soups follow each other in its draw
                world.cookie_runs.clear();
                for d in &world.draws {
                    if world.cookie_receive.get(d.batch as usize) != Some(&true) {
                        continue;
                    }
                    let mut at = d.first;
                    for r in &world.gather_scratch[d.batch as usize] {
                        let b = world.cookie_bounds.get(&r.first).copied();
                        world.cookie_runs.push((at..at + r.count, b));
                        at += r.count;
                    }
                }
                world.gathered_from = ranges;
                world.world_gathered = world.gathered.len();
            }
            // Animated stages get fresh params; static slots keep the load fill.
            for (sh, si, slot) in &world.animated {
                if let Some(p) = stage_params(sh, *si, frame.time) {
                    self.queue.write_buffer(
                        &world.stage_params_buf,
                        u64::from(*slot) * world.stage_params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            // Dome stages always carry the view origin, animated or not, and
            // the sky flag so linear fog skips them.
            for (sh, si, slot) in &world.sky_stages {
                if let Some(mut p) = stage_params(sh, *si, frame.time) {
                    p.flags |= STAGE_FLAG_EYE_OFFSET | STAGE_FLAG_SKY;
                    p.eye_off = [frame.eye.x, frame.eye.y, frame.eye.z, 0.0];
                    self.queue.write_buffer(
                        &world.stage_params_buf,
                        u64::from(*slot) * world.stage_params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            if !same_world {
                let mut frame_draws = std::mem::take(&mut world.frame_draws);
                order_draws(
                    &world.batch_keys,
                    &world.batch_draws,
                    &world.stages_of_batch,
                    &world.draws,
                    &mut frame_draws,
                );
                world.frame_draws = frame_draws;
            }
            // A moved brush model's soups follow the world's in the index
            // buffer, each set ordered on its own and drawn under its own
            // camera.
            let mut moved_draws = std::mem::take(&mut world.moved_draws);
            moved_draws.clear();
            for &(m, _) in &world.moved {
                let ranges: Vec<IndexRange> = world.model_soups[m]
                    .clone()
                    .filter_map(|si| world.soup_ranges.get(si).copied().flatten())
                    .collect();
                let mut out = Vec::new();
                let mut draws = gather(
                    &world.cpu_indices,
                    ranges,
                    world.batch_draws.len(),
                    &mut out,
                    &mut world.gather_scratch,
                );
                let base = world.gathered.len() as u32;
                for d in &mut draws {
                    d.first += base;
                }
                world.gathered.extend(out);
                let mut order = Vec::new();
                order_draws(
                    &world.batch_keys,
                    &world.batch_draws,
                    &world.stages_of_batch,
                    &draws,
                    &mut order,
                );
                moved_draws.push((draws, order));
            }
            world.moved_draws = moved_draws;
            // The dlight pass: per light, the visible soups it reaches
            // whose batch takes dlights, then each moved brush model's
            // with the light carried into the model's frame (0x4b59f0).
            world.dlight_runs.clear();
            world.dlight_indices.clear();
            let mut scene = self.dynamic.scene_lights.clone();
            scene.extend_from_slice(&self.dynamic.entity_scene_lights);
            let lights = if lights_world(self.dynamic_light) {
                dlight::from_scene(&scene)
            } else {
                Vec::new()
            };
            if !lights.is_empty() {
                for (i, l) in lights.iter().enumerate() {
                    self.queue.write_buffer(
                        &self.dlight.buf,
                        i as u64 * self.dlight.stride,
                        bytemuck::bytes_of(&dlight::DlightUniform::of(l, self.dlight_quality)),
                    );
                }
                let takes = |si: usize| {
                    world.soup_ranges[si]
                        .is_some_and(|r| !world.dlights_of_batch[r.batch as usize].is_empty())
                };
                let lit: Vec<usize> = (0..world.soup_ranges.len())
                    .filter(|&si| {
                        takes(si) && shown(si) && visible.as_ref().is_none_or(|(v, _)| v.soups[si])
                    })
                    .collect();
                let mut sets: Vec<(Option<usize>, glam::Mat4, Vec<usize>)> =
                    vec![(None, glam::Mat4::IDENTITY, lit)];
                for (mi, &(m, pose)) in world.moved.iter().enumerate() {
                    let soups = world.model_soups[m]
                        .clone()
                        .filter(|&si| takes(si))
                        .collect();
                    sets.push((Some(mi), pose.inverse(), soups));
                }
                let mut out = Vec::new();
                for (moved, to_model, soups) in &sets {
                    for (li, l) in lights.iter().enumerate() {
                        let origin = to_model.transform_point3(l.origin);
                        let ranges = soups.iter().filter_map(|&si| {
                            let (lo, hi) = world.soup_bounds[si];
                            dlight::reaches(l, origin, lo, hi)
                                .then_some(world.soup_ranges[si])
                                .flatten()
                        });
                        let draws = gather(
                            &world.cpu_indices,
                            ranges,
                            world.batch_draws.len(),
                            &mut out,
                            &mut world.gather_scratch,
                        );
                        let base = world.dlight_indices.len() as u32;
                        world.dlight_indices.extend_from_slice(&out);
                        world
                            .dlight_runs
                            .extend(draws.into_iter().map(|d| DlightRun {
                                light: li as u32,
                                moved: *moved,
                                draw: DrawRange {
                                    first: d.first + base,
                                    ..d
                                },
                            }));
                    }
                }
                let bytes = (world.dlight_indices.len() * 4) as u64;
                if bytes > world.dlight_index_buf.size() {
                    world.dlight_index_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("dlight indices"),
                        size: bytes.next_power_of_two(),
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                }
                if bytes > 0 {
                    self.queue.write_buffer(
                        &world.dlight_index_buf,
                        0,
                        bytemuck::cast_slice(&world.dlight_indices),
                    );
                    frame_write!(self, "upload dlight indices", bytes);
                }
            }
            world.stage_draws_last = world
                .frame_draws
                .iter()
                .filter(|fd| matches!(fd.kind, DrawRef::Stage(_)))
                .count();
            // a world part gathered from last frame's ranges is in the
            // buffer already; the moved models' part after it is not
            let from = if same_world { world.world_gathered } else { 0 };
            if from < world.gathered.len() {
                self.queue.write_buffer(
                    &world.index_buf,
                    from as u64 * 4,
                    bytemuck::cast_slice(&world.gathered[from..]),
                );
                frame_write!(
                    self,
                    "upload gathered world indices",
                    (world.gathered.len() - from) * 4
                );
            }
            world.prop_lod = prop_lod;
            let cell_total = world.vis.cell_count();
            self.vis_counts = VisCounts {
                mode: Some(frame.cull),
                cells: visible.as_ref().map_or((cell_total, cell_total), |(v, _)| {
                    (v.cells.iter().filter(|&&c| c).count(), cell_total)
                }),
                soups: visible.as_ref().map_or(
                    (world.soup_ranges.len(), world.soup_ranges.len()),
                    |(v, _)| (v.stats.soups, world.soup_ranges.len()),
                ),
                tris: (world.gathered.len() / 3, world.cpu_indices.len() / 3),
                props: (
                    if visible.is_some() {
                        props_drawn
                    } else {
                        world.prop_bounds.len()
                    },
                    world.prop_bounds.len(),
                ),
                occluders: visible.as_ref().map_or(0, |(v, _)| v.stats.occluders),
                portals_occluded: visible
                    .as_ref()
                    .map_or(0, |(v, _)| v.stats.portals_occluded),
                gather_ms: t0.elapsed().as_secs_f32() * 1000.0,
            };
            prof.next(crate::frame_prof::Stage::Build);
            crate::frame_prof::count(crate::frame_prof::Count::Tris, self.vis_counts.tris.0);
            crate::frame_prof::count(crate::frame_prof::Count::Soups, self.vis_counts.soups.0);
            // linear fog is the far plane retail's 0x4e1670 reads (no `r_zfar`)
            let far = (self.fog.set && self.fog.live.linear).then_some(self.fog.live.far);
            let walked = world
                .sky_soups
                .iter()
                .any(|&(si, _)| visible.as_ref().is_none_or(|(v, _)| v.soups[si]) && shown(si));
            let boxes: Vec<_> = world.sky_soups.iter().map(|&(_, b)| b).collect();
            self.sky_drawn = vcod_common::sun::sky_drawn(walked, far, frame.eye, frame.fwd, &boxes);
            world.locked = None;
            world.culled = None;
            match frame.cull {
                CullMode::Locked => world.locked = visible,
                CullMode::On => {
                    let key = cull_key(frame.eye, &frame.view_proj, cod4_lod);
                    world.culled = visible.map(|v| (key, v));
                }
                CullMode::Off => {}
            }
        }
        // The glyph cap truncates whole quads, so a cut readout stays legible.
        let hud_quads = if frame.hud_lines.is_empty() {
            0
        } else {
            let mut verts = hud_text::layout_lines(
                &frame.hud_lines,
                self.config.width as f32,
                self.config.height as f32,
                2.0,
            );
            verts.truncate(MAX_HUD_GLYPHS * 2 * 4);
            self.queue
                .write_buffer(&self.hud.vertex_buf, 0, bytemuck::cast_slice(&verts));
            verts.len() / 4
        };
        // Instance i draws from slot i of instance_buf.
        if !self.dynamic.instances.is_empty() {
            let m = &self.dynamic.materials;
            for (sh, si, slot) in &m.animated {
                if let Some(p) = stage_params(sh, *si, frame.time) {
                    self.queue.write_buffer(
                        &m.params_buf,
                        u64::from(*slot) * ENT_STAGE_STRIDE,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            let sets = self.pick_entity_lights();
            self.queue
                .write_buffer(&self.dynamic.light_buf, 0, bytemuck::cast_slice(&sets));
            let raw: Vec<InstanceRaw> = self.dynamic.instances.iter().map(|(_, r)| *r).collect();
            self.queue
                .write_buffer(&self.dynamic.instance_buf, 0, bytemuck::cast_slice(&raw));
            self.queue.write_buffer(
                &self.dynamic.bone_buf,
                0,
                bytemuck::cast_slice(&self.dynamic.bone_mats),
            );
            frame_write!(
                self,
                "upload entity lights, instances and bones",
                std::mem::size_of_val(sets.as_slice())
                    + std::mem::size_of_val(raw.as_slice())
                    + std::mem::size_of_val(self.dynamic.bone_mats.as_slice())
            );
        }
        let draw_vm = match &vm {
            Some(draw) if !self.vm_pass.models.is_empty() => {
                let to_view = glam::camera::rh::view::look_to_mat4(frame.eye, frame.fwd, frame.up);
                // CoD2: the grid at the eye, world space (cod2_model.wgsl);
                // CoD4: the view model's own block, world space too
                let cod2 = self.dynamic.cod2_light.is_some() || self.dynamic.cod4_light.is_some();
                let lights = match (&self.dynamic.cod2_light, self.dynamic.lighting.as_mut()) {
                    _ if self.dynamic.cod4_light.is_some() => {
                        let c4 = self.dynamic.cod4_light.as_ref().expect("checked");
                        // the gun's box: its lighting origin alone; its
                        // block the volume's last
                        let o = draw.light_origin;
                        let block = entity_light::COD4_VOLUME_BLOCKS - 1;
                        let (set, texels) = c4.set(o, [o, o], block, self.identity_light);
                        self.dynamic
                            .materials
                            .cod4_volume
                            .write_view_block(&self.queue, &texels);
                        set
                    }
                    (Some(c2), _) => c2.entity_set(draw.light_origin, self.identity_light),
                    (None, Some(l)) => {
                        let picked = l.entity_lights(draw.light_origin, &self.dynamic.scene_lights);
                        entity_light::pack(&picked, Some(to_view), self.identity_light)
                    }
                    (None, None) => GpuLightSet::default(),
                };
                let uniform = vm_uniform(draw.transform, draw.fov_x, self.aspect());
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    0,
                    bytemuck::cast_slice(&uniform),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_LIGHTS_OFFSET,
                    bytemuck::bytes_of(&lights),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_MISC_OFFSET,
                    bytemuck::cast_slice(&[
                        self.identity_light,
                        f32::from(u8::from(cod2)),
                        frame.time,
                        0.0,
                    ]),
                );
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    VM_VIEW_TO_WORLD_OFFSET,
                    bytemuck::cast_slice(&to_view.inverse().to_cols_array()),
                );
                for (i, model) in self.vm_pass.models.iter().enumerate() {
                    let Some(bones) = draw.bone_sets.get(i) else {
                        continue;
                    };
                    if bones.is_empty() {
                        continue;
                    }
                    if bones.len() > VM_BONE_COUNT {
                        static VM_BONE_CAP_WARNED: std::sync::Once = std::sync::Once::new();
                        VM_BONE_CAP_WARNED.call_once(|| {
                            log::warn!(
                                "viewmodel bone set of {} exceeds the shader's {VM_BONE_COUNT}; truncating",
                                bones.len()
                            );
                        });
                    }
                    let cols: Vec<[f32; 16]> = bones
                        .iter()
                        .take(VM_BONE_COUNT)
                        .map(glam::Mat4::to_cols_array)
                        .collect();
                    self.queue
                        .write_buffer(&model.bone_buf, 0, bytemuck::cast_slice(&cols));
                    frame_write!(self, "upload view model bones", cols.len() * 64);
                }
                true
            }
            _ => false,
        };

        let draw_vm = draw_vm && self.vm_pass.skins.len() == self.vm_pass.models.len();
        let vm_skins: &[Vec<Rc<EntSkin>>] = if draw_vm { &self.vm_pass.skins } else { &[] };
        // the instances the view can see; the shadow casters keep them all
        let frustum = vcod_common::vis::Frustum::from_view_proj(frame.view_proj);
        let seen: Vec<bool> = (0..self.dynamic.instances.len())
            .map(|k| !self.draw_joins || instance_seen(&self.dynamic, k, &frustum))
            .collect();
        let ent_draws = sorted_ent_draws(
            self.dynamic
                .instances
                .iter()
                .enumerate()
                .filter(|&(i, _)| seen[i])
                .flat_map(|(i, (m, _))| {
                    self.dynamic.models[*m]
                        .skins
                        .iter()
                        .enumerate()
                        .map(move |(j, sk)| (i as u32, j as u32, &**sk))
                }),
            vm_skins.iter().enumerate().flat_map(|(i, skins)| {
                skins
                    .iter()
                    .enumerate()
                    .map(move |(j, sk)| (i as u32, j as u32, &**sk))
            }),
        );
        prof.next(crate::frame_prof::Stage::Acquire);
        let surface_tex = match &self.target {
            Target::Offscreen(_) => None,
            Target::Surface(s) => Some(s.get_current_texture()),
        };
        let surface_tex = match surface_tex {
            None => None,
            Some(t) => Some(match t {
                wgpu::CurrentSurfaceTexture::Success(tex)
                | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    return;
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.reconfigure();
                    return;
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    log::warn!("dropped frame: surface validation error");
                    return;
                }
            }),
        };
        let view = match (&surface_tex, &self.target) {
            (Some(t), _) => t.texture.create_view(&Default::default()),
            (None, Target::Offscreen(t)) => t.create_view(&Default::default()),
            (None, Target::Surface(_)) => unreachable!("a surface frame holds its texture"),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        prof.next(crate::frame_prof::Stage::Encode);
        let prof_slot = self.prof_gpu.as_mut().and_then(|g| g.begin());
        if let Some(f) = &mut self.frag_stats {
            f.begin_frame(crate::frame_prof::frame());
        }
        // out of `self` for the frame, so a stamp never holds `self`
        let mut gpu_groups = self.gpu_groups.take();
        if let Some(g) = &mut gpu_groups {
            g.begin_frame();
        }
        // The profile's opaque and split stamps go at the split, the blend's
        // at the post pass; a frame without one writes them later, together.
        let (mut split_stamped, mut post_stamped) = (false, false);
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 0);
        }
        let shadow_on = self.encode_shadow_maps(&mut encoder, &frame, &camera, &moved);
        if shadow_on {
            frame_write!(self, "vc_shadows maps (not sized)", 0);
        }
        // a full-screen target's bytes at `bpp` a texel, for the write log
        #[cfg(test)]
        let full = {
            let px = u64::from(self.config.width) * u64::from(self.config.height);
            move |bpp: u32| px * u64::from(bpp)
        };
        #[cfg(test)]
        let world_bpp = self.world_format.block_copy_size(None).unwrap_or(4);
        // a scene pass's colour and depth (each sample), and its resolve
        #[cfg(test)]
        let scene_pass = |last: bool| {
            let s = msaa_samples();
            full((world_bpp + 4) * s) + if last && s > 1 { full(world_bpp) } else { 0 }
        };
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 1);
        }
        self.encode_cookies(&mut encoder, &frame);
        let receivers_on_prepass = self.cookie_receivers_on_prepass();
        if !receivers_on_prepass {
            Self::encode_cookie_receivers(self.cookie_receivers(), &mut encoder, &frame, false);
        }
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 2);
        }
        self.encode_sun_shadow(&mut encoder, &frame);
        if let Some(g) = &self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 3);
        }
        if !shadow_on && let Some(sp) = &self.shadows {
            sp.idle(&self.queue);
        }
        {
            // Linear fog clears to the fog colour (RTCW clearscreen=true);
            // otherwise the fixed steel blue stands in until the sky draws.
            let clear = if self.fog.set && self.fog.live.linear {
                wgpu::Color {
                    r: self.fog.live.color[0] as f64,
                    g: self.fog.live.color[1] as f64,
                    b: self.fog.live.color[2] as f64,
                    a: 1.0,
                }
            } else {
                wgpu::Color {
                    r: 0.35,
                    g: 0.42,
                    b: 0.50,
                    a: 1.0,
                }
            };
            let bloom = self.bloom_intensity > 0.0;
            let hdr = self.hdr();
            // With vc_hdr or a grade the world resolves into the post
            // pass's target; otherwise straight into the gamma scene.
            let film = self
                .level_look
                .and_then(|l| l.film)
                .filter(|_| self.cod4_options.film);
            // a CoD4 glow: the setup reads the film off or on, and the post
            // pass adds the result after the film
            let glow4 = self
                .level_look
                .and_then(|l| l.glow)
                .filter(|_| self.cod4_options.glow)
                .and_then(|g| {
                    let consts = film.unwrap_or(crate::post::Film::OFF);
                    crate::cod2_glow::plan_cod4(
                        &g,
                        consts.setup(self.identity_light),
                        self.config.width,
                        self.config.height,
                    )
                    .map(|p| (p, g.intensity))
                });
            // CoD4's depth of field blurs the resolved world before the
            // film and the glow (`RB_ProcessPostEffects`)
            let dof4 = self
                .cod4_dof
                .take()
                .filter(|d| self.cod4_options.dof && self.world.is_some() && d.used());
            let post = self.post_settings.active(hdr)
                || film.is_some()
                || glow4.is_some()
                || dof4.is_some();
            if post != self.effects_read_post {
                self.effects_read_post = post;
                let src = if post {
                    self.post.world_view()
                } else {
                    self.gamma.scene_view()
                };
                let (w, h) = (self.config.width, self.config.height);
                self.bloom.resize(&self.device, src, w, h);
                self.shafts
                    .resize(&self.device, src, &self.depth_view, w, h);
            }
            // An identity ramp leaves the gamma pass a copy, which 1.7 never
            // makes (its ramp is the device's): the post pass and the 2D then
            // draw straight into the swapchain (cod4-client-perf.md 30). It
            // needs the swapchain in the scene's format and opaque (the 2D
            // blends write alpha the gamma pass sets to 1), and nothing that
            // samples or copies the gamma scene: without the post pass the
            // glow, blur, bloom and shafts work in it (`scene_direct` below),
            // and a shellshock captures it.
            #[cfg(not(test))]
            let swap_on = true;
            #[cfg(test)]
            let swap_on = self.swap_direct;
            let swap_direct = swap_on
                && self.gamma.identity()
                && self.config.format == SCENE_FORMAT
                && (matches!(self.target, Target::Offscreen(_))
                    || self.config.alpha_mode == wgpu::CompositeAlphaMode::Opaque)
                && self.shock_blend.is_none()
                && !matches!(
                    self.cod4_shock,
                    Some(
                        crate::play::cod4_shock::Screen::Save
                            | crate::play::cod4_shock::Screen::Blur { .. }
                    )
                );
            let gamma_direct = post && swap_direct;
            // Shafts need a sun flare's direction (shader doc 8).
            let shafts = self
                .sky
                .as_ref()
                .and_then(|s| s.flare.as_ref())
                .is_some_and(|f| {
                    self.shafts.prepare(
                        &self.queue,
                        frame.view_proj,
                        f.dir(),
                        (self.config.width as f32, self.config.height as f32),
                        self.identity_light,
                    )
                });
            if let Some(flare) = self
                .sky
                .as_mut()
                .and_then(|s| s.flare.as_mut())
                .filter(|_| self.flares)
            {
                flare.prepare(
                    &self.device,
                    &self.queue,
                    &frame,
                    (self.config.width as f32, self.config.height as f32),
                    self.identity_light,
                    self.sky_drawn && self.draw_sun,
                );
            }
            // A CoD1 or UO frame at an identity ramp (windowed, `r_gamma` 1)
            // with no post pass resolves the scene and draws the 2D straight
            // into the swapchain, as retail's ramp is the device's
            // (cod11-client-perf.md 4): only while nothing reads the gamma
            // scene (bloom, shafts, CoD2's glow and blur) or draws into the
            // split's scene (vc_ssao, vc_shadows).
            let scene_direct = swap_direct
                && !post
                && !bloom
                && !shafts
                && !ssao_on
                && !shadow_on
                && self.world_fx.is_none()
                && self.world.as_ref().is_none_or(|w| w.cod2.is_none());
            self.gamma_skipped = gamma_direct || scene_direct;
            // With SSAO or vc_shadows the frame is two passes split after
            // the last opaque or decal draw (`ssao_split`): the first keeps
            // its MSAA colour for the shadow mask and the SSAO multiply, the
            // second loads it and resolves.
            let targets = SceneTargets {
                msaa: &self.msaa_view,
                depth: &self.depth_view,
                resolve: if post {
                    self.post.world_view()
                } else if scene_direct {
                    &view
                } else {
                    self.gamma.scene_view()
                },
                timer: self.ssao.timer(),
                clear,
                sun_queries: self
                    .sky
                    .as_ref()
                    .and_then(|s| s.flare.as_ref())
                    .and_then(SunFlareGpu::query_set),
                depth_cleared: false,
            };
            // only worth the split when an fx run that reads the depth
            // copy draws after it: every run under vc_softParticles, a
            // feathered one always, a CoD4 distortion one under
            // r_distortion, which reads the colour copy too
            // (`resolvedPostSun`)
            let soft_all = self.soft.on();
            // and a world feathered or distortion stage (`cod4_worldfx`)
            let (world_feather, world_distort) = self
                .world
                .as_ref()
                .map_or((false, false), |w| self.world_fx_drawn(w));
            let distort_on = self.world.is_some()
                && self.cod4_options.distortion
                && (world_distort
                    || self
                        .fx
                        .runs
                        .iter()
                        .any(|r| r.sort > SORT_DECAL && r.distort));
            let soft_on = distort_on
                || world_feather && self.cod4_options.zfeather
                || self.world.is_some()
                    && self
                        .fx
                        .runs
                        .iter()
                        .any(|r| r.sort > SORT_DECAL && (soft_all || r.feather));
            // The world's draws across the world and every moved brush
            // model, by their place in the sorted shader list; on one
            // shader a moved model, an entity in retail's key, draws before
            // the world (entity 0x3fe, shader doc 5).
            let mut world_list: Vec<(f32, u32, usize, u32)> = Vec::new();
            if let Some(world) = &self.world {
                for (set, (_, frame_draws, _)) in self.world_sets(world).enumerate() {
                    for (i, fd) in frame_draws.iter().enumerate() {
                        world_list.push((fd.sort, fd.rank, set, i as u32));
                    }
                }
            }
            // stable: a batch keeps its stage order
            world_list.sort_by_key(|&(_, rank, set, _)| (rank, set.wrapping_sub(1)));
            // CoD2's feather reads a depth without the entities (floatZ,
            // cod2-materials.md 9.5): its copy goes before the first one.
            // CoD4's `floatZ` holds them (cod4-client-perf.md 20), so a
            // CoD4 feather copies at the split and keeps them in the
            // prepass.
            let feathered = self.fx.runs.iter().any(|r| r.feather && !r.view_depth);
            // `r_vc_depthprepass` (cod4-client-perf.md 12): the opaque lit
            // world draws and the entities' opaque first stages lay their
            // depth down in a pass of their own, then draw their colour
            // under EQUAL in place. Every other draw keeps its state, so
            // the frame comes out the same. The entities stay out while a
            // feather's depth copy must hold the world alone.
            let prepass_on = self.depth_prepass && self.world.is_some();
            let ent_prepass = prepass_on && !feathered;
            // A CoD4 frame a feather reads writes `floatZ` in the prepass,
            // as 1.7's does (cod4-client-perf.md 35), and keeps no split or
            // copy for it. One sample only: the copy reads sample 0 of an
            // MSAA depth, which a fragment's write does not give. With
            // `r_zFeather` 0 and the distortion alone the frame is as
            // before.
            let feather_on = self.cod4_options.zfeather
                && (world_feather
                    || self
                        .fx
                        .runs
                        .iter()
                        .any(|r| r.sort > SORT_DECAL && r.feather));
            let floatz = self.floatz_prepass
                && ent_prepass
                && feather_on
                && !soft_all
                && msaa_samples() == 1
                && self.dynamic.cod4_light.is_some();
            // the draws the prepass laid down untested, which draw their
            // colour under EQUAL: a `floatZ` prepass takes only 1.7's set
            let in_prepass = |world: &WorldGpu, kind: DrawRef| {
                takes_prepass(world, kind) && (!floatz || takes_floatz(world, kind) == Some(false))
            };
            self.census.set(PassCensus::default());
            // CoD4's entity runs and depth prepass replay from bundles as its
            // world runs do (cod4-client-perf.md 40)
            let ent_bundling = self.ent_bundles_on && self.dynamic.cod4_light.is_some();
            let mut pre_bundles = std::mem::take(&mut self.prepass_bundles);
            pre_bundles.begin(floatz.then_some(crate::soft_particles::LINZ_FORMAT));
            if prepass_on {
                if let Some(g) = &mut gpu_groups {
                    g.enc(&mut encoder, Some(crate::frame_prof::Group::Prepass));
                }
                // the cookie receivers test the world's depth alone, so the
                // entities' prepass follows them
                let split = receivers_on_prepass && !floatz;
                let mut n = self.encode_depth_prepass(
                    &mut encoder,
                    &world_list,
                    &ent_draws,
                    PrepassParts {
                        world: true,
                        ents: ent_prepass && !split,
                        floatz,
                        load: false,
                    },
                    frame.time,
                    ent_bundling.then_some(&mut pre_bundles),
                );
                if split {
                    if let Some(g) = &mut gpu_groups {
                        g.enc(&mut encoder, Some(crate::frame_prof::Group::Receivers));
                    }
                    Self::encode_cookie_receivers(
                        CookieReceivers {
                            device: &self.device,
                            queue: &self.queue,
                            world: self.world.as_ref(),
                            cookies: &mut self.cookies,
                            dyn_shadows: &mut self.dyn_shadows,
                            depth_view: &self.depth_view,
                            cod2: self.dynamic.cod2_light.is_some(),
                        },
                        &mut encoder,
                        &frame,
                        true,
                    );
                    if ent_prepass {
                        if let Some(g) = &mut gpu_groups {
                            g.enc(&mut encoder, Some(crate::frame_prof::Group::Prepass));
                        }
                        n += self.encode_depth_prepass(
                            &mut encoder,
                            &world_list,
                            &ent_draws,
                            PrepassParts {
                                world: false,
                                ents: true,
                                floatz,
                                load: true,
                            },
                            frame.time,
                            ent_bundling.then_some(&mut pre_bundles),
                        );
                    }
                }
                crate::frame_prof::count(crate::frame_prof::Count::Prepass, n);
                frame_write!(self, "depth prepass (depth)", full(4 * msaa_samples()));
                if floatz {
                    frame_write!(self, "depth prepass floatZ (R32F)", full(4));
                }
                crate::frame_prof::count(
                    crate::frame_prof::Count::Feathered,
                    usize::from(!ent_prepass),
                );
                #[cfg(test)]
                self.prepass_drawn.set(n);
                #[cfg(test)]
                self.floatz_drawn.set(floatz);
            }
            pre_bundles.end();
            self.prepass_bundles = pre_bundles;
            let targets = SceneTargets {
                depth_cleared: prepass_on,
                ..targets
            };
            let split = ssao_on || shadow_on || distort_on || soft_on && !floatz;
            if let Some(g) = &mut gpu_groups {
                g.enc(&mut encoder, Some(crate::frame_prof::Group::Sky));
            }
            let mut pass = targets.begin(&mut encoder, true, !split);
            frame_write!(self, "scene pass (colour + depth)", scene_pass(!split));
            pass.set_bind_group(0, &self.camera_bg, &[]);
            // The sky draws first so every world draw overwrites it; the box
            // and dome both ignore depth (cleared 1.0) and write none. CoD
            // keeps the sky visible under linear fog (VERIFIED live on
            // mp_ship, unlike RTCW's drawsky=false).
            if let (Some(world), Some(sky)) = (&self.world, &self.sky) {
                if let Some(farbox) = &sky.farbox {
                    pass.set_pipeline(&farbox.pipeline);
                    pass.set_bind_group(2, &farbox.eye_bg, &[]);
                    pass.set_vertex_buffer(0, farbox.vertex_buf.slice(..));
                    pass.set_index_buffer(farbox.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    for (axis, &(first, end)) in farbox.sides.iter().enumerate() {
                        pass.set_bind_group(1, &farbox.side_bgs[axis], &[]);
                        pass.draw_indexed(first..end, 0, 0..1);
                    }
                    tally(&self.census, |c| {
                        let n = farbox.sides.len() as u32;
                        c.sky += n;
                        c.pipelines += 1;
                        c.bind_groups += 1 + n;
                    });
                }
                // The sunfile disc: additive billboard along the sun
                // direction at farbox distance, camera-facing via world axes.
                if let Some(sun) = sky.sun.as_ref().filter(|_| self.sky_drawn && self.draw_sun) {
                    let center = frame.eye + sun.dir * sky::box_size(Z_FAR, Z_NEAR);
                    let up = glam::Vec3::Z;
                    let right = up.cross(sun.dir).normalize_or_zero();
                    let quad_up = sun.dir.cross(right);
                    let r = right * sun.half_extent;
                    let u = quad_up * sun.half_extent;
                    let corner = |dr: f32, du: f32| sky::SkyBoxVert {
                        pos: (center + r * dr + u * du).to_array(),
                        uv: [(dr + 1.0) * 0.5, 1.0 - (du + 1.0) * 0.5],
                    };
                    let verts = [
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ];
                    self.queue
                        .write_buffer(&sun.vertex_buf, 0, bytemuck::cast_slice(&verts));
                    pass.set_pipeline(&sun.pipeline);
                    pass.set_bind_group(1, &sun.bind_group, &[]);
                    pass.set_vertex_buffer(0, sun.vertex_buf.slice(..));
                    pass.draw(0..6, 0..1);
                    tally(&self.census, |c| {
                        c.sky += 1;
                        c.pipelines += 1;
                        c.bind_groups += 1;
                    });
                }
                if let Some(dome) = &sky.dome {
                    pass.set_vertex_buffer(0, dome.vertex_buf.slice(..));
                    pass.set_index_buffer(dome.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    let mut bound: Option<(StageVariant, bool)> = None;
                    for ds in &dome.stages {
                        let key = (ds.variant, ds.two_sided);
                        if bound != Some(key) {
                            pass.set_pipeline(self.stage_pipeline(ds.variant, ds.two_sided, false));
                            bound = Some(key);
                            tally(&self.census, |c| c.pipelines += 1);
                        }
                        pass.set_bind_group(1, ds.mat.bind_group(frame.time), &[]);
                        pass.set_bind_group(
                            2,
                            ds.stage.bind_group(frame.time),
                            &[
                                u32::try_from(u64::from(ds.slot) * world.stage_params_stride)
                                    .expect("stage params slot offset fits a dynamic offset"),
                            ],
                        );
                        pass.draw_indexed(0..dome.index_count, 0, 0..1);
                        tally(&self.census, |c| {
                            c.sky += 1;
                            c.bind_groups += 2;
                        });
                    }
                }
            }
            // One list, as retail's draw list keys every surface (shader doc
            // 5): the world's draws, the entity and viewmodel surfaces and
            // the fx runs merged by material sort, the world first on a
            // tie, then the models, then the fx. The gun keeps its
            // depth-hack range (renderfx 0x8c: the front fifth), so what
            // draws after it behind it fails the depth test. The flat list
            // mixes pipeline layouts; rebind only on change. An fx run
            // whose texture failed to resolve is skipped.
            let (vw, vh) = (self.config.width as f32, self.config.height as f32);
            #[cfg(test)]
            let ent_draws = {
                let mut d = ent_draws;
                if let Some((i, s)) = self.ent_only.get() {
                    d.retain(|e| e.gun || e.inst == i && s.is_none_or(|s| e.surf == s));
                }
                *self.ent_census.borrow_mut() = d
                    .iter()
                    .filter(|e| !e.gun)
                    .map(|e| {
                        let model = &self.dynamic.models[self.dynamic.instances[e.inst as usize].0];
                        let skin = &model.skins[e.surf as usize];
                        let surf = &model.vm.surfaces[e.surf as usize];
                        let equal = ent_prepass
                            && ent_takes_prepass(skin)
                            && (!floatz || skin.stages.first().is_some_and(|st| st.floatz));
                        let takes = ent_takes_prepass(skin);
                        (
                            e.inst,
                            e.surf,
                            surf.index_count / 3,
                            skin.stages.len(),
                            equal,
                            takes,
                        )
                    })
                    .collect();
                d
            };
            let world_sorts: Vec<f32> = world_list.iter().map(|w| w.0).collect();
            let model_sorts: Vec<f32> = ent_draws.iter().map(|d| d.sort).collect();
            let fx_sorts: Vec<f32> = self.fx.runs.iter().map(|r| r.sort).collect();
            let fx_firsts: Vec<usize> = self
                .fx
                .runs
                .iter()
                .scan(0, |first, run| {
                    let at = *first;
                    *first += run.quads;
                    Some(at)
                })
                .collect();
            let mark_sorts: Vec<f32> = self.marks.runs.iter().map(|r| r.sort).collect();
            let items = merge_by_sort(&world_sorts, &mark_sorts, &model_sorts, &fx_sorts);
            crate::frame_prof::count(crate::frame_prof::Count::Draws, items.len());
            let mut dlight_at = dlight_slot(&items, &world_sorts);
            let split_at = split
                .then(|| ssao_split(&items, &world_sorts, &mark_sorts, &model_sorts, &fx_sorts));
            let copy_at = split_at.filter(|_| !floatz).map(|at| match feathered {
                true => feather_copy_at(&items, at),
                false => at,
            });
            // where the effects may read the feather's depth from
            let soft_from = if floatz { Some(0) } else { copy_at };
            // the dlights add light, so a shadow must not darken them
            if shadow_on && let Some(at) = split_at {
                dlight_at = dlight_at.max(at);
            }
            let mut emit = Emit::None;
            // Past the items an entity surface run or a world bundle drew.
            let mut skip_to = 0;
            // (a run is one `Frag::World` kind for the fragment counts, and
            // one GPU group, its first item's, for the profile's stamps)
            let bundling = self.world_bundles_on && self.dynamic.cod4_light.is_some();
            let mut bundles = std::mem::take(&mut self.world_bundles);
            bundles.begin(Some(self.world_format));
            // The items where the loop below ends a pass or draws the
            // dlights: a world run stops short of them.
            let dlights_drawn = self
                .world
                .as_ref()
                .is_some_and(|w| !w.dlight_runs.is_empty());
            let stops = [
                copy_at.filter(|_| soft_on && copy_at != split_at),
                split_at,
                dlights_drawn.then_some(dlight_at),
            ];
            for k in 0..=items.len() {
                if k < skip_to {
                    continue;
                }
                if soft_on && copy_at == Some(k) && copy_at != split_at {
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, None);
                    }
                    drop(pass);
                    if let Some(g) = &mut gpu_groups {
                        g.enc(&mut encoder, Some(crate::frame_prof::Group::SoftCopy));
                    }
                    self.soft.encode(&mut encoder);
                    frame_write!(self, "feather depth copy (R32F)", full(4));
                    pass = targets.begin(&mut encoder, false, false);
                    frame_write!(self, "scene pass after the depth copy", scene_pass(false));
                    pass.set_bind_group(0, &self.camera_bg, &[]);
                    emit = Emit::None;
                }
                // Before the dlights, so their light is not occluded; the
                // shadow mask before the SSAO multiply.
                if split_at == Some(k) {
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, None);
                    }
                    drop(pass);
                    if let Some(g) = &self.prof_gpu {
                        g.stamp(prof_slot, &mut encoder, 4);
                    }
                    let scene = if msaa_samples() > 1 {
                        &self.msaa_view
                    } else {
                        targets.resolve
                    };
                    use crate::frame_prof::Group;
                    let mut group = |encoder: &mut wgpu::CommandEncoder, g| {
                        if let Some(gg) = &mut gpu_groups {
                            gg.enc(encoder, Some(g));
                        }
                    };
                    if shadow_on && let Some(sp) = &self.shadows {
                        group(&mut encoder, Group::ShadowMask);
                        sp.encode_mask(&self.device, &mut encoder, scene, &self.depth_view);
                        frame_write!(self, "vc_shadows mask (not sized)", 0);
                    }
                    if ssao_on {
                        group(&mut encoder, Group::Ssao);
                        self.ssao.encode(&mut encoder, scene, &self.depth_view);
                        frame_write!(self, "vc_ssao (not sized)", 0);
                    }
                    if soft_on && copy_at == split_at {
                        group(&mut encoder, Group::SoftCopy);
                        self.soft.encode(&mut encoder);
                        frame_write!(self, "feather depth copy (R32F)", full(4));
                    }
                    if distort_on {
                        group(&mut encoder, Group::SceneCopy);
                        self.scene_copy.encode(&self.device, &mut encoder, scene);
                        frame_write!(self, "distortion colour copy", full(world_bpp));
                    }
                    if let Some(g) = &self.prof_gpu {
                        g.stamp(prof_slot, &mut encoder, 5);
                    }
                    split_stamped = true;
                    pass = targets.begin(&mut encoder, false, true);
                    frame_write!(self, "scene pass after the split", scene_pass(true));
                    pass.set_bind_group(0, &self.camera_bg, &[]);
                    emit = Emit::None;
                }
                if let Some(world) = &self.world
                    && dlight_at == k
                    && !world.dlight_runs.is_empty()
                {
                    if matches!(emit, Emit::Gun { .. }) {
                        pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                    }
                    if let Some(f) = &mut self.frag_stats {
                        f.switch(&mut pass, Some(crate::frame_prof::Frag::Dlight));
                    }
                    if let Some(g) = &mut gpu_groups {
                        g.pass(&mut pass, Some(crate::frame_prof::Group::Dlight));
                    }
                    self.emit_dlights(&mut pass, world, frame.time);
                    emit = Emit::None;
                }
                let Some(&item) = items.get(k) else {
                    break;
                };
                let prop = self.frag_stats.is_some() && self.is_prop_draw(item, &world_list);
                if let Some(f) = &mut self.frag_stats {
                    use crate::frame_prof::Frag;
                    let kind = match item {
                        SortedItem::World(_) if prop => Frag::Props,
                        SortedItem::World(_) => Frag::World,
                        SortedItem::Model(mi) if ent_draws[mi].gun => Frag::Gun,
                        SortedItem::Model(_) => Frag::Ents,
                        SortedItem::Mark(_) => Frag::Marks,
                        SortedItem::Fx(fi) => {
                            let r = &self.fx.runs[fi];
                            let soft = soft_on
                                && (soft_all || r.feather)
                                && soft_from.is_some_and(|at| k >= at);
                            if soft && !r.distort && r.cloud.is_none() {
                                Frag::FxSoft
                            } else {
                                Frag::Fx
                            }
                        }
                    };
                    f.switch(&mut pass, Some(kind));
                }
                if gpu_groups.is_some() {
                    let g = self.draw_group(item, &world_list, &ent_draws, |fi| {
                        let r = &self.fx.runs[fi];
                        soft_on
                            && (soft_all || r.feather)
                            && soft_from.is_some_and(|at| k >= at)
                            && !r.distort
                            && r.cloud.is_none()
                    });
                    if let Some(gg) = &mut gpu_groups {
                        gg.pass(&mut pass, Some(g));
                    }
                }
                match item {
                    SortedItem::World(_) if bundling => {
                        // the run: the world items up to the next stop or
                        // other item, recorded as `SortedItem::World`
                        // below would encode them from `Emit::None`
                        let world = self.world.as_ref().expect("world draws come from a world");
                        let mut end = k + 1;
                        while matches!(items.get(end), Some(SortedItem::World(_)))
                            && !stops.contains(&Some(end))
                        {
                            end += 1;
                        }
                        if matches!(emit, Emit::Gun { .. }) {
                            pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                        }
                        let fx_ready = (
                            soft_on
                                && self.cod4_options.zfeather
                                && soft_from.is_some_and(|at| k >= at),
                            distort_on && split_at.is_some_and(|at| k >= at),
                        );
                        let mut rec = Recording::default();
                        rec.vertices(0, world.vertex_buf.slice(..));
                        rec.indices(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                        let (mut cam_set, mut bound, mut groups) =
                            (None, None, WorldGroups::default());
                        for item in &items[k..end] {
                            let SortedItem::World(wi) = *item else {
                                unreachable!("a world run holds world items")
                            };
                            let (_, _, set, i) = world_list[wi];
                            let (draws, frame_draws, cam) = self
                                .world_sets(world)
                                .nth(set)
                                .expect("collected from these sets");
                            if cam_set != Some(set) {
                                rec.group(0, cam, None);
                                tally(&self.census, |c| c.bind_groups += 1);
                                cam_set = Some(set);
                            }
                            let fd = &frame_draws[i as usize];
                            let depth = if prepass_on && in_prepass(world, fd.kind) {
                                Depth::Equal
                            } else {
                                Depth::Plain
                            };
                            self.emit_world_draw(
                                &mut rec,
                                world,
                                &draws[fd.src as usize],
                                fd.kind,
                                &mut bound,
                                &mut groups,
                                frame.time,
                                fx_ready,
                                depth,
                            );
                        }
                        bundles.draw(
                            &mut pass,
                            &self.device,
                            rec,
                            crate::frame_prof::Count::Bundled,
                        );
                        // a bundle leaves the pass's pipeline, groups and
                        // buffers unset
                        skip_to = end;
                        emit = Emit::None;
                    }
                    SortedItem::World(wi) => {
                        let world = self.world.as_ref().expect("world draws come from a world");
                        let (_, _, set, i) = world_list[wi];
                        let (draws, frame_draws, cam) = self
                            .world_sets(world)
                            .nth(set)
                            .expect("collected from these sets");
                        let (mut bound, mut groups) = match emit {
                            Emit::World {
                                set: s,
                                bound,
                                groups,
                            } if s == set => (bound, groups),
                            Emit::World { bound, groups, .. } => {
                                pass.set_bind_group(0, cam, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                (bound, groups)
                            }
                            other => {
                                if matches!(other, Emit::Gun { .. }) {
                                    pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                }
                                pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
                                pass.set_index_buffer(
                                    world.index_buf.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                pass.set_bind_group(0, cam, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                (None, WorldGroups::default())
                            }
                        };
                        let fd = &frame_draws[i as usize];
                        // the copies are this frame's past the split only
                        let fx_ready = (
                            soft_on
                                && self.cod4_options.zfeather
                                && soft_from.is_some_and(|at| k >= at),
                            distort_on && split_at.is_some_and(|at| k >= at),
                        );
                        let depth = if prepass_on && in_prepass(world, fd.kind) {
                            Depth::Equal
                        } else {
                            Depth::Plain
                        };
                        self.emit_world_draw(
                            &mut pass,
                            world,
                            &draws[fd.src as usize],
                            fd.kind,
                            &mut bound,
                            &mut groups,
                            frame.time,
                            fx_ready,
                            depth,
                        );
                        emit = Emit::World { set, bound, groups };
                    }
                    SortedItem::Model(mi) if ent_bundling && !ent_draws[mi].gun => {
                        // the entity runs up to the next stop or other item,
                        // recorded as the arm below would encode them from
                        // `Emit::None`
                        let mut end = k + 1;
                        while let Some(&SortedItem::Model(m)) = items.get(end)
                            && !ent_draws[m].gun
                            && !stops.contains(&Some(end))
                        {
                            end += 1;
                        }
                        if matches!(emit, Emit::Gun { .. }) {
                            pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                        }
                        let dynamic = &self.dynamic;
                        let mut rec = Recording::default();
                        rec.group(0, &self.camera_bg, None);
                        rec.group(2, &dynamic.bone_bg, None);
                        rec.vertices(1, dynamic.instance_buf.slice(..));
                        tally(&self.census, |c| c.bind_groups += 2);
                        let (mut bound_model, mut bound) = (None, EntBound::default());
                        // a superset of `stops`, so no run crosses `end`
                        let run_stops = [copy_at, split_at, Some(dlight_at)];
                        let mut j = k;
                        while j < end {
                            let SortedItem::Model(mi) = items[j] else {
                                unreachable!("an entity run holds entity items")
                            };
                            let d = &ent_draws[mi];
                            let model_at = dynamic.instances[d.inst as usize].0;
                            let model = &dynamic.models[model_at];
                            let vm = &model.vm;
                            if bound_model != Some(model_at) {
                                rec.vertices(0, vm.vertex_buf.slice(..));
                                rec.indices(vm.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                                bound_model = Some(model_at);
                            }
                            let surf = &vm.surfaces[d.surf as usize];
                            let n = ent_run(&items, j, &ent_draws, model, &run_stops);
                            let last = &vm.surfaces[d.surf as usize + n - 1];
                            let skin = &model.skins[d.surf as usize];
                            let depth = if ent_prepass
                                && ent_takes_prepass(skin)
                                && (!floatz || skin.stages.first().is_some_and(|st| st.floatz))
                            {
                                Depth::Equal
                            } else {
                                Depth::Plain
                            };
                            draw_ent_stages(
                                &mut rec,
                                &dynamic.materials,
                                EntPass::World,
                                &vm.bind_groups[surf.bind_group],
                                skin,
                                surf,
                                (
                                    surf.first_index..last.first_index + last.index_count,
                                    d.inst,
                                ),
                                &mut bound,
                                depth,
                                &self.census,
                            );
                            crate::frame_prof::count(crate::frame_prof::Count::EntDraws, 1);
                            j += n;
                        }
                        bundles.draw(
                            &mut pass,
                            &self.device,
                            rec,
                            crate::frame_prof::Count::EntBundled,
                        );
                        skip_to = end;
                        emit = Emit::None;
                    }
                    SortedItem::Model(mi) if !ent_draws[mi].gun => {
                        let d = &ent_draws[mi];
                        let dynamic = &self.dynamic;
                        let model_at = dynamic.instances[d.inst as usize].0;
                        let model = &dynamic.models[model_at];
                        let vm = &model.vm;
                        // the instance comes in as `first_instance` over the
                        // whole instance buffer, so instances of one model
                        // keep its buffers bound
                        let (same_model, mut bound) = match emit {
                            Emit::Ent { model, bound } => (model == model_at, bound),
                            other => {
                                if matches!(other, Emit::Gun { .. }) {
                                    pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                }
                                pass.set_bind_group(0, &self.camera_bg, &[]);
                                pass.set_bind_group(2, &dynamic.bone_bg, &[]);
                                pass.set_vertex_buffer(1, dynamic.instance_buf.slice(..));
                                tally(&self.census, |c| c.bind_groups += 2);
                                (false, EntBound::default())
                            }
                        };
                        if !same_model {
                            pass.set_vertex_buffer(0, vm.vertex_buf.slice(..));
                            pass.set_index_buffer(
                                vm.index_buf.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                        }
                        let surf = &vm.surfaces[d.surf as usize];
                        let stops = [copy_at, split_at, Some(dlight_at)];
                        let n = ent_run(&items, k, &ent_draws, model, &stops);
                        let last = &vm.surfaces[ent_draws[mi].surf as usize + n - 1];
                        let skin = &model.skins[d.surf as usize];
                        // a run is one skin, so one answer for all of it
                        let depth = if ent_prepass
                            && ent_takes_prepass(skin)
                            && (!floatz || skin.stages.first().is_some_and(|st| st.floatz))
                        {
                            Depth::Equal
                        } else {
                            Depth::Plain
                        };
                        draw_ent_stages(
                            &mut pass,
                            &dynamic.materials,
                            EntPass::World,
                            &vm.bind_groups[surf.bind_group],
                            skin,
                            surf,
                            (
                                surf.first_index..last.first_index + last.index_count,
                                d.inst,
                            ),
                            &mut bound,
                            depth,
                            &self.census,
                        );
                        skip_to = k + n;
                        crate::frame_prof::count(crate::frame_prof::Count::EntDraws, 1);
                        emit = Emit::Ent {
                            model: model_at,
                            bound,
                        };
                    }
                    SortedItem::Model(mi) => {
                        let d = &ent_draws[mi];
                        let vmp = &self.vm_pass;
                        let model = &vmp.models[d.inst as usize];
                        let (same_part, mut bound) = match emit {
                            Emit::Gun { part, bound } => (part == d.inst, bound),
                            _ => {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, VM_DEPTH_RANGE);
                                pass.set_bind_group(0, &vmp.uniform_bg, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                (false, EntBound::default())
                            }
                        };
                        if !same_part {
                            pass.set_vertex_buffer(0, model.vertex_buf.slice(..));
                            pass.set_index_buffer(
                                model.index_buf.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.set_bind_group(2, &model.bone_bg, &[]);
                            tally(&self.census, |c| c.bind_groups += 1);
                        }
                        let surf = &model.surfaces[d.surf as usize];
                        draw_ent_stages(
                            &mut pass,
                            &self.dynamic.materials,
                            EntPass::View,
                            &model.bind_groups[surf.bind_group],
                            &vmp.skins[d.inst as usize][d.surf as usize],
                            surf,
                            (surf.first_index..surf.first_index + surf.index_count, 0),
                            &mut bound,
                            Depth::Plain,
                            &self.census,
                        );
                        emit = Emit::Gun {
                            part: d.inst,
                            bound,
                        };
                    }
                    SortedItem::Mark(ri) => {
                        if matches!(emit, Emit::Gun { .. }) {
                            pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                        }
                        pass.set_bind_group(0, &self.camera_bg, &[]);
                        let n = self.marks.draw(&mut pass, ri, frame.time);
                        tally(&self.census, |c| {
                            c.marks += n;
                            c.pipelines += n;
                            c.bind_groups += 1 + 2 * n;
                        });
                        emit = Emit::None;
                    }
                    SortedItem::Fx(fi) => {
                        let fx = &self.fx;
                        let FxRun {
                            image,
                            blend,
                            quads: count,
                            feather,
                            ..
                        } = &fx.runs[fi];
                        let Some(Some(bg)) = fx.textures.get(image) else {
                            continue;
                        };
                        if fx.runs[fi].distort {
                            // after the split only, where the copies are
                            // this frame's (retail skips the draw without
                            // r_distortion: `R_SetupMaterial`)
                            if !distort_on || split_at.is_none_or(|at| k < at) {
                                continue;
                            }
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            pass.set_bind_group(0, &self.camera_bg, &[]);
                            pass.set_bind_group(1, bg, &[]);
                            pass.set_bind_group(2, self.scene_copy.bind_group(), &[]);
                            pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
                            pass.set_index_buffer(
                                fx.index_buf.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_pipeline(&fx.distort_pipeline);
                            let start = (fx_firsts[fi] * 6) as u32;
                            let end = ((fx_firsts[fi] + count) * 6) as u32;
                            pass.draw_indexed(start..end, 0, 0..1);
                            tally(&self.census, |c| {
                                c.fx += 1;
                                c.pipelines += 1;
                                c.bind_groups += 3;
                            });
                            emit = Emit::None;
                            continue;
                        }
                        if let Some(instances) = fx.runs[fi].cloud.clone() {
                            if matches!(emit, Emit::Gun { .. }) {
                                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                            }
                            let vp = frame.view_proj;
                            fx.clouds
                                .draw(&mut pass, &self.camera_bg, bg, *blend, instances, vp);
                            tally(&self.census, |c| {
                                c.fx += 1;
                                c.pipelines += 1;
                                c.bind_groups += 3;
                            });
                            emit = Emit::None;
                            continue;
                        }
                        // only after the split is the depth copy this frame's
                        let soft = soft_on
                            && (soft_all || *feather)
                            && soft_from.is_some_and(|at| k >= at);
                        let bound = match emit {
                            Emit::Fx { blend, soft: s } => {
                                if soft && !s {
                                    pass.set_bind_group(2, self.soft.fx_bind_group(), &[]);
                                    tally(&self.census, |c| c.bind_groups += 1);
                                }
                                Some((blend, s))
                            }
                            other => {
                                if matches!(other, Emit::Gun { .. }) {
                                    pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
                                }
                                pass.set_bind_group(0, &self.camera_bg, &[]);
                                tally(&self.census, |c| c.bind_groups += 1);
                                if soft {
                                    pass.set_bind_group(2, self.soft.fx_bind_group(), &[]);
                                    tally(&self.census, |c| c.bind_groups += 1);
                                }
                                pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
                                pass.set_index_buffer(
                                    fx.index_buf.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                None
                            }
                        };
                        if bound != Some((*blend, soft)) {
                            let set = if soft {
                                &fx.soft_pipelines
                            } else {
                                &fx.pipelines
                            };
                            pass.set_pipeline(&set[*blend as usize]);
                            tally(&self.census, |c| c.pipelines += 1);
                        }
                        pass.set_bind_group(1, bg, &[]);
                        let start = (fx_firsts[fi] * 6) as u32;
                        let end = ((fx_firsts[fi] + count) * 6) as u32;
                        pass.draw_indexed(start..end, 0, 0..1);
                        tally(&self.census, |c| {
                            c.fx += 1;
                            c.bind_groups += 1;
                        });
                        emit = Emit::Fx {
                            blend: *blend,
                            soft,
                        };
                    }
                }
            }
            bundles.end();
            self.world_bundles = bundles;
            if let Some(f) = &mut self.frag_stats {
                f.switch(&mut pass, None);
            }
            let census = self.census.get();
            crate::frame_prof::count(
                crate::frame_prof::Count::Pipelines,
                census.pipelines as usize,
            );
            crate::frame_prof::count(
                crate::frame_prof::Count::BindGroups,
                census.bind_groups as usize,
            );
            crate::frame_prof::count(crate::frame_prof::Count::Calls, census.draws() as usize);
            crate::frame_prof::count(
                crate::frame_prof::Count::EntCalls,
                (census.ents + census.prepass_ents) as usize,
            );
            if matches!(emit, Emit::Gun { .. }) {
                pass.set_viewport(0.0, 0.0, vw, vh, 0.0, 1.0);
            }
            // The sun's flare and washes go over the finished scene, before
            // the 2D (0x4d9770).
            if let Some(flare) = self.sky.as_ref().and_then(|s| s.flare.as_ref())
                && self.flares
            {
                if let Some(g) = &mut gpu_groups {
                    g.pass(&mut pass, Some(crate::frame_prof::Group::Flare));
                }
                flare.draw(&mut pass);
            }
            pass.set_bind_group(0, &self.camera_bg, &[]);

            // The world is resolved: add the shafts, blur it, add the glow
            // (and a CoD2 menu's blur) into the resolved world, run the post pass into the gamma
            // scene when it is on, and draw the HUD there single-sampled, so
            // the HUD never blooms, shafts or takes the grade and the MSAA
            // target is never reloaded.
            let shock = self.shock_blend;
            // A CoD4 session sets it every frame it draws.
            let cod4_shock = self.cod4_shock.take();
            // CoD2's r_glow, on at its default on every CoD2 map; a CoD4
            // level's own
            let sky_cube = self
                .sky
                .as_ref()
                .and_then(|s| s.farbox.as_ref())
                .and_then(|f| f.cube.as_ref());
            let glow = self
                .world
                .as_ref()
                .is_some_and(|w| w.cod2.is_some())
                .then(|| match &glow4 {
                    Some((p, _)) => Some(p.clone()),
                    None if self.level_look.is_some() => None,
                    None => crate::cod2_glow::plan(
                        &Default::default(),
                        self.config.width,
                        self.config.height,
                        sky_cube.is_some(),
                    ),
                })
                .flatten();
            // a `blurWorld` menu's blur, after the glow (cod2-materials.md 9.13)
            let world_blur = self
                .world
                .as_ref()
                .is_some_and(|w| w.cod2.is_some())
                .then(|| {
                    crate::cod2_glow::blur_plan(
                        self.world_blur,
                        self.config.width,
                        self.config.height,
                    )
                })
                .flatten();
            let after = bloom
                || shafts
                || post
                || shock.is_some()
                || glow.is_some()
                || world_blur.is_some();
            if after {
                drop(pass);
                if let Some(g) = &self.prof_gpu {
                    if !split_stamped {
                        g.stamp(prof_slot, &mut encoder, 4);
                        g.stamp(prof_slot, &mut encoder, 5);
                    }
                    g.stamp(prof_slot, &mut encoder, 6);
                    split_stamped = true;
                    post_stamped = true;
                }
                let world = if post {
                    self.post.world_view()
                } else {
                    self.gamma.scene_view()
                };
                use crate::frame_prof::Group;
                let mut group = |encoder: &mut wgpu::CommandEncoder, g| {
                    if let Some(gg) = &mut gpu_groups {
                        gg.enc(encoder, Some(g));
                    }
                };
                if let Some(d) = &dof4 {
                    group(&mut encoder, Group::Dof);
                    let c = d.consts(self.config.height, Z_NEAR, VM_NEAR);
                    self.dof.encode(
                        &self.device,
                        &self.queue,
                        &mut encoder,
                        self.post.world(),
                        self.post.world_view(),
                        &self.depth_view,
                        &c,
                    );
                    // the copy, five quarter steps and the composite
                    frame_write!(
                        self,
                        "r_dof (copy, 5 quarter passes, composite)",
                        2 * full(world_bpp) + 5 * full(world_bpp) / 16
                    );
                }
                if shafts {
                    group(&mut encoder, Group::Shafts);
                    self.shafts.encode(&mut encoder, world);
                    frame_write!(self, "vc_sunShafts (not sized)", 0);
                }
                #[cfg(test)]
                {
                    if let Some(g) = &glow {
                        let steps: usize = 1 + g.chains.iter().map(Vec::len).sum::<usize>();
                        frame_write!(
                            self,
                            format!("glow ({steps} quarter passes, {} applies)", g.applies.len()),
                            steps as u64 * full(4) / 16 + g.applies.len() as u64 * full(world_bpp)
                        );
                    }
                    if let Some(b) = &world_blur {
                        frame_write!(
                            self,
                            "blurWorld",
                            b.chain.len() as u64 * full(4) / 16 + full(world_bpp)
                        );
                    }
                }
                group(&mut encoder, Group::Glow);
                self.cod2_glow.encode(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    world,
                    sky_cube,
                    frame.view_proj.inverse(),
                    frame.eye,
                    glow.as_ref(),
                    world_blur.as_ref(),
                );
                if bloom {
                    group(&mut encoder, Group::Bloom);
                    self.bloom.set_params(
                        &self.queue,
                        self.bloom_intensity,
                        self.bloom_threshold,
                        self.identity_light,
                        hdr,
                    );
                    self.bloom.blur(&mut encoder);
                    frame_write!(self, "vc_bloom (not sized)", 0);
                }
                if post {
                    if bloom {
                        let mut glow =
                            single_pass(&mut encoder, self.post.world_view(), "bloom pass");
                        self.bloom.composite(&mut glow);
                    }
                    self.post.bind_glow(
                        &self.device,
                        glow4.as_ref().map(|_| self.cod2_glow.result()),
                    );
                    self.post.set_params(
                        &self.queue,
                        self.post_settings,
                        film,
                        glow4.as_ref().map_or(0.0, |g| g.1),
                        hdr,
                        self.identity_light,
                        frame.time,
                    );
                    group(&mut encoder, Group::PostFx);
                    pass = if gamma_direct {
                        // the post pass writes every texel
                        clear_pass(&mut encoder, &view, "hud pass")
                    } else {
                        single_pass(&mut encoder, self.gamma.scene_view(), "hud pass")
                    };
                    self.post.draw(&mut pass);
                    if gamma_direct {
                        frame_write!(self, "post pass into the swapchain, then the 2D", full(4));
                    } else {
                        frame_write!(self, "post pass into the gamma scene, then the 2D", full(4));
                    }
                } else {
                    pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
                    frame_write!(self, "2D pass (gamma scene)", full(4));
                    if bloom {
                        self.bloom.composite(&mut pass);
                    }
                }
            }
            // The shellshock's render commands 10 and 9 on the finished 3D
            // view, before the 2D, as the cgame queues them.
            use crate::play::cod4_shock::Screen as Cod4Screen;
            if (!matches!(cod4_shock, None | Some(Cod4Screen::Off)) || shock.is_some())
                && let Some(g) = &mut gpu_groups
            {
                g.pass(&mut pass, Some(crate::frame_prof::Group::Shock));
            }
            match (cod4_shock, shock) {
                // CoD4's flashbang: the first frame saves the still, the
                // later ones add it with the white (cod4-client-view.md 8).
                (Some(Cod4Screen::Save), _) => {
                    drop(pass);
                    self.screen_blend.capture(&mut encoder);
                    frame_write!(self, "screen capture", full(4));
                    pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
                }
                (Some(Cod4Screen::Flash { white, grab }), _) => {
                    self.screen_blend.flash(&self.queue, &mut pass, white, grab);
                }
                (Some(Cod4Screen::Blur { decay_ms }), _) => {
                    let since = self.shock_captured_at.map_or(i32::MAX, |t| {
                        i32::try_from(t.elapsed().as_millis()).unwrap_or(i32::MAX)
                    });
                    self.screen_blend.blend(
                        &self.queue,
                        &mut pass,
                        crate::screen_blend::alpha(since, decay_ms),
                        crate::screen_blend::COD2_DESATURATE,
                    );
                    drop(pass);
                    self.screen_blend.capture(&mut encoder);
                    frame_write!(self, "screen capture", full(4));
                    self.shock_captured_at = Some(std::time::Instant::now());
                    pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
                }
                (Some(Cod4Screen::Off), _) => self.screen_blend.drop_capture(),
                (None, Some(decay)) => {
                    let since = self.shock_captured_at.map_or(i32::MAX, |t| {
                        i32::try_from(t.elapsed().as_millis()).unwrap_or(i32::MAX)
                    });
                    // CoD2 draws the capture through `shellshock`
                    let desaturate = if self.world.as_ref().is_some_and(|w| w.cod2.is_some()) {
                        crate::screen_blend::COD2_DESATURATE
                    } else {
                        0.0
                    };
                    self.screen_blend.blend(
                        &self.queue,
                        &mut pass,
                        crate::screen_blend::alpha(since, decay),
                        desaturate,
                    );
                    drop(pass);
                    self.screen_blend.capture(&mut encoder);
                    frame_write!(self, "screen capture", full(4));
                    self.shock_captured_at = Some(std::time::Instant::now());
                    pass = single_pass(&mut encoder, self.gamma.scene_view(), "hud pass");
                }
                (None, None) => self.screen_blend.drop_capture(),
            }

            // HUD after the weapon so it sits on top, before the debug
            // overlay so F3 always wins.
            if let Some(g) = &mut gpu_groups {
                g.pass(&mut pass, Some(crate::frame_prof::Group::Hud));
            }
            if !self.hud_pass.runs.is_empty() {
                let n = self.hud_pass.draw(&mut pass, after);
                crate::frame_prof::count(crate::frame_prof::Count::HudDraws, n);
            }

            // Debug HUD text, over everything.
            if hud_quads > 0 {
                let hud = &self.hud;
                pass.set_pipeline(if after {
                    &hud.pipeline_1x
                } else {
                    &hud.pipeline
                });
                pass.set_bind_group(0, &hud.bind_group, &[]);
                pass.set_vertex_buffer(0, hud.vertex_buf.slice(..));
                pass.set_index_buffer(hud.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..(hud_quads * 6) as u32, 0, 0..1);
            }
        }
        if shadow_on && let Some(sp) = &mut self.shadows {
            sp.resolve_timer(&mut encoder);
        }
        if let Some(f) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            f.resolve(&mut encoder);
        }
        if let Some(t) = self.ssao.timer_mut() {
            t.resolve(&mut encoder, ssao_on);
        }
        if let Some(g) = &self.prof_gpu {
            // No post pass: the blend runs on into the HUD, all of it
            // `blend`.
            if !split_stamped {
                g.stamp(prof_slot, &mut encoder, 4);
                g.stamp(prof_slot, &mut encoder, 5);
            }
            if !post_stamped {
                g.stamp(prof_slot, &mut encoder, 6);
            }
            g.stamp(prof_slot, &mut encoder, 7);
        }
        if let Some(g) = &mut gpu_groups {
            g.enc(&mut encoder, None);
            g.resolve(&mut encoder);
        }
        if !self.gamma_skipped {
            self.gamma.draw(&mut encoder, &view);
            frame_write!(self, "gamma pass into the swapchain", full(4));
        }
        if let Some(g) = &mut self.prof_gpu {
            g.stamp(prof_slot, &mut encoder, 8);
            g.resolve(prof_slot, &mut encoder);
        }
        if let Some(f) = &mut self.frag_stats {
            f.resolve(&mut encoder);
        }
        // Before the ramp is the gamma scene, unless the frame drew
        // straight into the swapchain past an identity ramp.
        let readback = self.capture_wanted.take().map(|pre_gamma| {
            let tex = match (&surface_tex, &self.target) {
                _ if pre_gamma && !self.gamma_skipped => self.gamma.scene_view().texture(),
                (Some(t), _) => &t.texture,
                (None, Target::Offscreen(t)) => t,
                (None, Target::Surface(_)) => unreachable!("a surface frame holds its texture"),
            };
            capture::encode(&self.device, &mut encoder, tex)
        });
        prof.next(crate::frame_prof::Stage::Finish);
        let commands = encoder.finish();
        prof.next(crate::frame_prof::Stage::Submit);
        self.queue.submit([commands]);
        if let Some(sp) = &mut self.shadows {
            sp.after_submit();
        }
        self.sun_shadow.after_submit();
        if let Some(f) = self.sky.as_mut().and_then(|s| s.flare.as_mut()) {
            f.after_submit();
        }
        if let Some(rb) = readback {
            self.captured = Some(rb.and_then(|rb| capture::finish(&self.device, rb)));
        }
        if let Some(t) = surface_tex {
            self.queue.present(t);
        }
        if let Some(t) = self.ssao.timer_mut() {
            t.collect(&self.device);
        }
        if let Some(g) = &mut self.prof_gpu {
            g.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        if let Some(f) = &mut self.frag_stats {
            f.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        self.gpu_groups = gpu_groups;
        if let Some(g) = &mut self.gpu_groups {
            g.collect();
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
    }
}

/// A single-sample pass that clears `view` to black and stores it: the post
/// pass and the 2D straight into the swapchain.
fn clear_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A single-sample pass that loads and stores `view`: the bloom composite
/// and the HUD after the map pass.
fn single_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// The camera group (`camera_layout`): the Camera block, the fx lights,
/// `vc_shadows`' receiver inputs, `r_textureMode`'s samplers, CoD2's
/// dynamic shadow and CoD4's sun shadow map.
#[allow(clippy::too_many_arguments)]
fn camera_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera: &wgpu::Buffer,
    fx_lights: &wgpu::Buffer,
    recv: &shadows::Receive<'_>,
    tex: &TexSamplers,
    dyn_shadow: &wgpu::TextureView,
    sun_shadow: &crate::cod4_sunshadow::SunShadowPass,
) -> wgpu::BindGroup {
    let [r0, r1, r2, r3, r4, r5, r6] = recv.entries();
    let [m0, m1, m2] = sun_shadow.entries();
    let [s0, s1, s2, s3, s4, s5, s6, s7] = tex.entries();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("camera bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fx_lights.as_entire_binding(),
            },
            r0,
            r1,
            r2,
            r3,
            r4,
            r5,
            r6,
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
            wgpu::BindGroupEntry {
                binding: DYNAMIC_SHADOW_BINDING,
                resource: wgpu::BindingResource::TextureView(dyn_shadow),
            },
            m0,
            m1,
            m2,
        ],
    })
}

/// Whether a script-less `<type>@<image>` world material's
/// `shadertypes/world/<type>.stype` draws opaque: no stage alpha-tests and
/// the first one does not blend. Its image's alpha is then read by nothing
/// (wood@chateau_exteriordoor's is a gloss mask; docs/research/
/// cod11-shader-scripts.md, "World shader types").
/// How a static prop's skin draws when it has no `skins/` block, by its
/// `type@` template as the entity path resolves it (shader doc 8).
enum PropSkinDraw {
    /// More than one stage, or a blend: the stage path, `$texturename`
    /// bound to the skin.
    Stages(Box<Material>),
    /// One stage that alpha-tests: `fs_prop`'s cut.
    Cut,
    /// One opaque stage, or the implicit skin: nothing reads the alpha.
    Opaque,
}

fn prop_skin_draw(fs: &dyn AssetSource, skin: &str) -> PropSkinDraw {
    let Some(sh) =
        crate::entity_material::template(fs, skin, &mut vcod_common::shader::WarnSet::new())
    else {
        // a typed skin retail cannot build draws its default shader
        return if crate::entity_material::is_typed_skin(skin) {
            PropSkinDraw::Stages(Box::new(vcod_common::shader::default_material()))
        } else {
            PropSkinDraw::Opaque
        };
    };
    if sh.stages.len() > 1 || sh.stages[0].blend.is_some() {
        return PropSkinDraw::Stages(Box::new(Material::from_shader(
            &crate::entity_material::bind_texture_name(sh, &format!("skins/{skin}")),
        )));
    }
    if sh.stages[0].alpha_func.is_some() {
        PropSkinDraw::Cut
    } else {
        PropSkinDraw::Opaque
    }
}

/// Whether a static prop's skin draws both sides: its `type@` template
/// says `cull none` (UO's `*_cullnone` types). The implicit skin and every
/// other template keep `R_FindShader`'s zeroed cull type, which `GL_Cull`
/// (CoDMP.exe 0x4d5670) draws one-sided (cod11-client-perf.md 5).
fn prop_skin_two_sided(fs: &dyn AssetSource, skin: &str) -> bool {
    crate::entity_material::template(fs, skin, &mut vcod_common::shader::WarnSet::new())
        .is_some_and(|sh| sh.two_sided)
}

/// Whether an image as uploaded, `img` after `texture_settings::prepare`
/// and the levels it returned, has a texel under alpha 128 at any level:
/// only then can a 0.5 alpha test drop a filtered sample.
fn uploads_cutout(img: &Image, levels: &[Vec<u8>]) -> bool {
    assets::has_cutout_alpha_any_level(img)
        || levels
            .iter()
            .any(|l| l.iter().skip(3).step_by(4).any(|&a| a < 128))
}

fn world_type_draws_opaque(fs: &dyn AssetSource, name: &str) -> bool {
    crate::entity_material::skin_type(name)
        .and_then(|kind| {
            fs.load::<RawFile>(&format!("shadertypes/world/{kind}.stype"))
                .ok()
        })
        .is_some_and(|file| stype_draws_opaque(&file.text()))
}

fn stype_draws_opaque(text: &str) -> bool {
    let Some((_, body)) = vcod_common::shader::split_blocks(text).into_iter().next() else {
        return false;
    };
    let sh =
        vcod_common::shader::parse_shader("stype", &body, &mut vcod_common::shader::WarnSet::new());
    sh.stages.first().is_some_and(|st| st.blend.is_none())
        && sh.stages.iter().all(|st| st.alpha_func.is_none())
}

/// `vc_shadows`' casters from the world: the soups and props that draw
/// opaque, per [`ShadowCast`]; the deformed ones apart.
fn shadow_casters(device: &wgpu::Device, world: &WorldGpu) -> shadows::Casters {
    let casts = |r: &IndexRange| world.shadow_cast[r.batch as usize] != ShadowCast::None;
    let range = |r: &IndexRange, bounds| shadows::CasterRange {
        first: r.first,
        count: r.count,
        bounds,
        cut: match world.shadow_cast[r.batch as usize] {
            ShadowCast::Cut | ShadowCast::Stage(_) | ShadowCast::Deformed(_) => Some(r.batch),
            ShadowCast::None | ShadowCast::Solid => None,
        },
    };
    let mut models: Vec<Vec<shadows::CasterRange>> =
        world.model_soups.iter().map(|_| Vec::new()).collect();
    let deforms =
        |r: &IndexRange| matches!(world.shadow_cast[r.batch as usize], ShadowCast::Deformed(_));
    let (mut flat, mut deformed) = (Vec::new(), Vec::new());
    for (si, r) in world.soup_ranges.iter().enumerate() {
        let Some(r) = r.filter(casts).filter(|_| !world.coarse_lod[si]) else {
            continue;
        };
        let c = range(&r, world.soup_bounds[si]);
        match world.soup_model[si] as usize {
            _ if deforms(&r) => deformed.push(c),
            0 => flat.push(c),
            m => models[m].push(c),
        }
    }
    // LOD 0 alone: the static layer is cached across views, so it can't
    // follow the camera's pick.
    for (p, _, r) in world.prop_ranges.iter().filter(|(_, l, _)| *l == 0) {
        let bounds = world.prop_bounds[*p as usize];
        if casts(r) && bounds.0 != bounds.1 {
            if deforms(r) {
                deformed.push(range(r, bounds));
            } else {
                flat.push(range(r, bounds));
            }
        }
    }
    let count = |c: ShadowCast| world.shadow_cast.iter().filter(|&&s| s == c).count();
    let staged = world
        .shadow_cast
        .iter()
        .filter(|s| matches!(s, ShadowCast::Stage(_)))
        .count();
    log::info!(
        "shadow casters: {} solid, {} implicit cutout, {} staged cutout batches, {} deformed casters",
        count(ShadowCast::Solid),
        count(ShadowCast::Cut),
        staged,
        deformed.len()
    );
    shadows::Casters::build(device, &world.cpu_indices, &flat, &models, &deformed)
}

/// The smaller half-extent of a ring-ordered quad.
fn quad_half_extent(v: &[[f32; 3]; 4]) -> f32 {
    let [a, b, _, d] = v.map(glam::Vec3::from);
    0.5 * (b - a).length().min((d - a).length())
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: msaa_samples(),
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            // the SSAO passes read it between the frame's two halves
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_msaa_view(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::TextureView {
    // single-sampled the scene draws into its resolve target and never
    // touches this one (`SceneTargets::begin`): keep it a texel, not a
    // second full-screen colour target (cod4-client-perf.md 24)
    let (width, height) = match msaa_samples() {
        1 => (1, 1),
        _ => (width, height),
    };
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: msaa_samples(),
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_fx_pass(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    soft_layout: &wgpu::BindGroupLayout,
    copy_layout: &wgpu::BindGroupLayout,
) -> FxPass {
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fx vertices"),
        size: (MAX_FX_DRAWN * 4 * std::mem::size_of::<FxVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Quad n spans vertices 4n..4n+4. u32 because MAX_FX_DRAWN * 4 can
    // exceed u16::MAX.
    let indices: Vec<u32> = (0..MAX_FX_DRAWN)
        .flat_map(|q| {
            let b = (q * 4) as u32;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fx indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fx.wgsl"),
        source: wgpu::ShaderSource::Wgsl(FX_WGSL.into()),
    });
    let hard_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout)],
        immediate_size: 0,
    });
    let soft_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx soft pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout), Some(soft_layout)],
        immediate_size: 0,
    });
    let distort_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx distortion pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout), Some(copy_layout)],
        immediate_size: 0,
    });
    // The pipelines differ only in blend (with the depth write it implies),
    // layout and fragment entry, so runs can interleave within one pass.
    let make_in = |label, fs_entry, blend, layout: &wgpu::PipelineLayout| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FxVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // pos
                        1 => Float32x2, // uv
                        2 => Float32x4, // rgba
                        3 => Float32, // size
                        4 => Uint32, // flags
                        5 => Float32, // feather
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Decal tangent frames flip with the surface normal and
                // billboards face the camera; neither has a meaningful winding.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // The world must occlude fx quads, but overlapping blended
                // quads must all draw; an unblended stage writes depth.
                depth_write_enabled: Some(blend.is_none()),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
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
    };
    let make = |label, fs_entry, blend, soft: bool| {
        let layout = if soft {
            &soft_pipeline_layout
        } else {
            &hard_layout
        };
        make_in(label, fs_entry, blend, layout)
    };

    let [alpha, additive, screen, opaque] = [
        FxBlend::Alpha,
        FxBlend::Add,
        FxBlend::Screen,
        FxBlend::Opaque,
    ]
    .map(fx_blend_state);
    let pipelines = [
        make("fx pipeline", "fs_main", alpha, false),
        make("fx additive pipeline", "fs_additive", additive, false),
        make("fx screen pipeline", "fs_screen", screen, false),
        make("fx opaque pipeline", "fs_main", opaque, false),
    ];
    let soft_pipelines = [
        make("fx soft pipeline", "fs_soft", alpha, true),
        make(
            "fx soft additive pipeline",
            "fs_soft_additive",
            additive,
            true,
        ),
        make("fx soft screen pipeline", "fs_soft_screen", screen, true),
        // nothing to fade into: an opaque stage writes its own depth
        make("fx soft opaque pipeline", "fs_main", opaque, true),
    ];

    // distortion_scale_zfeather's emissive state: (SrcAlpha, InvSrcAlpha)
    let distort_pipeline = make_in(
        "fx distortion pipeline",
        "fs_distort",
        alpha,
        &distort_layout,
    );
    FxPass {
        pipelines,
        soft_pipelines,
        distort_pipeline,
        vertex_buf,
        index_buf,
        textures: HashMap::new(),
        runs: Vec::new(),
        clouds: fx_cloud::CloudPass::new(device, format, camera_layout, texture_layout),
    }
}

/// What a quad's vertex colour is scaled by on its way into the
/// framebuffer, from the material's first stage: the lit gens (`vertex`,
/// `identityLighting`, `constLighting`, `wave`) take `identityLight`,
/// `identity`, `exactVertex` and `const` none. A material with no script is
/// implicit 2D or fx, `rgbGen vertex` (0x4fc440 case -4); see
/// docs/research/cod11-gamma.md, section 5.
fn colour_scale(shader: Option<&Material>, identity_light: f32) -> f32 {
    match shader
        .and_then(|sh| sh.stages.first())
        .map(|st| &st.rgb_gen)
    {
        Some(RgbGen::Identity | RgbGen::ExactVertex | RgbGen::Const(_)) => 1.0,
        _ => identity_light,
    }
}

/// A HUD colour is a display value; `scale` (`colour_scale`) takes it into
/// the framebuffer, and the ramp's overbright doubling brings a lit gen back
/// on display. Alpha is a coverage weight.
fn hud_colour([r, g, b, a]: [f32; 4], scale: f32) -> [f32; 4] {
    [r * scale, g * scale, b * scale, a]
}

/// Resolves an fx or HUD shader name to a loadable texture path through
/// the shader scripts' image map and `assets::resolve_bundle_image`.
/// `load_path_image` never fails (a missing file becomes the default
/// image), so unresolvable names must be caught here or they draw
/// default-image quads.
fn resolve_fx_path(
    shader_images: &HashMap<String, String>,
    fs: &dyn AssetSource,
    name: &str,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let path = shader_images.get(name).map(String::as_str).unwrap_or(name);
    assets::resolve_bundle_image(fs, path)
}

/// How an effect's image loads. The efx `shaders` field registers through
/// `RE_RegisterShader` (refexport 0x4fca80, lightmap index -4), so an
/// effect image is mipmapped and takes `r_picmip` unless its shader says
/// `nomipmaps` / `nopicmip` (docs/research/cod11-front-end.md, section 14).
struct FxImage<'a> {
    gamma: Option<&'a [u8; 256]>,
    picmip: u32,
    store: texture_settings::Store,
    mipmap: bool,
}

/// `upload_levels` handles both RGBA sprites and BC `.dds` mip chains (many
/// effect textures are DXT5). Unresolvable names warn once; the caller
/// caches the `None`.
#[allow(clippy::too_many_arguments)]
fn resolve_fx_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    fs: &dyn AssetSource,
    shader_images: &HashMap<String, String>,
    name: &str,
    how: FxImage,
) -> Option<wgpu::BindGroup> {
    let Some(path) = resolve_fx_path(shader_images, fs, name) else {
        log::warn!("fx shader {name:?}: no texture found for it, dropping its quads");
        return None;
    };
    let mut img = assets::load_path_image(fs, &path);
    let load = texture_settings::path_levels(fs);
    let mips = if how.mipmap {
        texture_settings::Mips::chain(&path, &load)
    } else {
        texture_settings::Mips::single(&path, &load)
    };
    let levels = texture_settings::prepare(&mut img, how.picmip, how.gamma, how.store, mips);
    let view = upload_levels(device, queue, name, &img, &levels);
    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(name),
        layout: texture_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    }))
}

fn create_hud_text_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> HudTextPass {
    // Nearest sampling: glyphs draw at integer scales, filtering would blur them.
    let atlas = hud_text::build_atlas();
    let size = wgpu::Extent3d {
        width: hud_text::ATLAS_W as u32,
        height: hud_text::ATLAS_H as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud font atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &atlas,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(hud_text::ATLAS_W as u32),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("hud font sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hud text layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hud text"),
        layout: &bg_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    // Two quads per glyph (shadow + text); quad n spans vertices 4n..4n+4.
    const MAX_QUADS: usize = MAX_HUD_GLYPHS * 2;
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud text vertices"),
        size: (MAX_QUADS * 4 * std::mem::size_of::<HudVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let indices: Vec<u16> = (0..MAX_QUADS)
        .flat_map(|q| {
            let b = (q * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud text indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("hud_text.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud text pipeline layout"),
        bind_group_layouts: &[Some(&bg_layout)],
        immediate_size: 0,
    });
    // The map pass (MSAA or not) carries the depth target; the pass after
    // the bloom draws single-sample with no depth into the resolved scene.
    let make = |samples: u32, depth: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud text pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos (clip space)
                        1 => Float32x2, // uv
                        2 => Float32x4, // color
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // depth ignored, draws over everything
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let (pipeline, pipeline_1x) = (make(msaa_samples(), true), make(1, false));

    HudTextPass {
        pipeline,
        pipeline_1x,
        vertex_buf,
        index_buf,
        bind_group,
    }
}

/// What [`Renderer::set_hud_quads`] resolves and uploads HUD quads with,
/// apart so an offscreen test can draw a frame through the same HUD pass.
struct HudUpload<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    samplers: &'a TexSamplers,
    image_map: &'a HashMap<String, String>,
    lib: &'a MaterialLib,
    textures: &'a texture_settings::TextureSettings,
    image_gamma: Option<&'a [u8; 256]>,
    identity_light: f32,
    /// The target's size in px.
    size: (f32, f32),
}

impl HudUpload<'_> {
    /// Resolves each quad's texture into `pass`'s cache, writes the vertex
    /// buffer and the draw runs. Past [`MAX_HUD_QUADS`] the rest are dropped
    /// with one warning, `cap_warned` keeping it.
    fn upload(
        &self,
        pass: &mut HudPass,
        fs: &dyn AssetSource,
        quads: &[HudQuad],
        cap_warned: &mut bool,
    ) {
        let quads = if quads.len() > MAX_HUD_QUADS {
            if !*cap_warned {
                *cap_warned = true;
                log::warn!(
                    "hud: {} quads exceeds the {MAX_HUD_QUADS} cap, truncating",
                    quads.len()
                );
            }
            &quads[..MAX_HUD_QUADS]
        } else {
            quads
        };

        let (w, h) = self.size;
        let mut verts: Vec<HudVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<(String, usize, bool)> = Vec::new();
        for quad in quads {
            let (key, add) = match quad.texture.strip_prefix(hud::ADDITIVE) {
                Some(k) => (k, true),
                None => (quad.texture.as_str(), false),
            };
            // A default-if-missing quad that resolves draws as its own name.
            let name = default_shader_fallback(key).unwrap_or(key);
            if !pass.textures.contains_key(key) {
                let how = hud_image(name, self.lib.get(name), &pass.mip_names);
                let tex = resolve_hud_texture(
                    self.device,
                    self.queue,
                    &pass.texture_layout,
                    self.samplers,
                    fs,
                    self.image_map,
                    cod2_hud_image(fs, self.lib, name),
                    key,
                    how,
                    self.textures.world_picmip(!how.picmip),
                    self.image_gamma,
                    self.textures.store(),
                );
                pass.textures.insert(key.to_string(), tex);
            }

            // `black` shares `white`'s 1x1 texture; force the rgb here so it
            // draws black regardless of the element colour, alpha untouched.
            let default = matches!(
                pass.textures.get(key),
                Some(Some(t)) if t.default
            );
            let rgba = if key == "black" {
                [0.0, 0.0, 0.0, quad.rgba[3]]
            } else if default {
                [1.0; 4]
            } else {
                hud_colour(
                    quad.rgba,
                    colour_scale(self.lib.get(name), self.identity_light),
                )
            };

            for i in 0..4 {
                let [px, py] = quad.verts[i];
                verts.push(HudVert {
                    // pixel coords, origin top-left -> clip space
                    pos: [px / w * 2.0 - 1.0, 1.0 - py / h * 2.0],
                    uv: quad.uvs[i],
                    color: rgba,
                });
            }
            match runs.last_mut() {
                Some((name, count, a)) if name == key && *a == add => *count += 1,
                _ => runs.push((key.to_string(), 1, add)),
            }
        }

        self.queue
            .write_buffer(&pass.vertex_buf, 0, bytemuck::cast_slice(&verts));
        pass.quad_count = quads.len();
        pass.runs = runs;
    }
}

/// Like `create_hud_text_pass` but with `hud.wgsl`, which samples an image
/// with mipmaps through `r_textureMode`'s sampler and one without through
/// `GL_LINEAR` ([`HudTexture`]).
fn create_hud_pass(device: &wgpu::Device, format: wgpu::TextureFormat) -> HudPass {
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hud texture layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    // Quad n spans vertices 4n..4n+4.
    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud vertices"),
        size: (MAX_HUD_QUADS * 4 * std::mem::size_of::<HudVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let indices: Vec<u16> = (0..MAX_HUD_QUADS)
        .flat_map(|q| {
            let b = (q * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("hud.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud pipeline layout"),
        bind_group_layouts: &[Some(&texture_layout)],
        immediate_size: 0,
    });
    // The map pass (MSAA or not) carries the depth target; the pass after
    // the bloom draws single-sample with no depth into the resolved scene.
    let make = |samples: u32, depth: bool, blend: wgpu::BlendState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos (clip space)
                        1 => Float32x2, // uv
                        2 => Float32x4, // color
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // depth ignored, draws over everything
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let alpha = wgpu::BlendState::ALPHA_BLENDING;
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let add = wgpu::BlendState {
        color: add,
        alpha: add,
    };
    let (pipeline, pipeline_1x) = (make(msaa_samples(), true, alpha), make(1, false, alpha));
    let (additive, additive_1x) = (make(msaa_samples(), true, add), make(1, false, add));

    HudPass {
        pipeline,
        pipeline_1x,
        additive,
        additive_1x,
        vertex_buf,
        index_buf,
        texture_layout,
        textures: HashMap::new(),
        mip_names: HashSet::new(),
        runs: Vec::new(),
        quad_count: 0,
    }
}

/// Where a HUD texture name resolves. `white` and `$whiteimage` are the
/// engine built-in; `black` (pak4 `scripts/hud.shader`: `map $whiteimage`,
/// `rgbGen const 0`) shares the same texture, and `set_hud_quads` forces the
/// quad colour to black for it.
#[derive(Debug, PartialEq, Eq)]
enum HudTex {
    White,
    Black,
    /// [`crate::hud::DEFAULT_SHADER`], or a [`crate::hud::default_if_missing`]
    /// name with no image.
    Default,
    Path(String),
    Missing,
}

/// A material name (`hudStanceStand`, `hintHealth`) resolves through the
/// shader-script image map, same as fx; anything else is a bare pk3 path.
/// The material a prop skin draws with: CoD1's `skins/<name>` script
/// block, or the CoD2 material the model names.
fn prop_material<'a>(lib: &'a MaterialLib, cod2: bool, skin: &str) -> Option<&'a Material> {
    if cod2 {
        lib.get(skin)
    } else {
        props::skin_script(skin).and_then(|n| lib.get(&n))
    }
}

/// Whether a CoD2 static prop of `m` draws per pixel through its
/// `lprobe_smc*` technique: every stage in a variant the lit pipeline
/// takes (`cod2::draws_smodel`).
fn cod2_smodel(m: &Material) -> bool {
    !m.stages.is_empty()
        && (0..m.stages.len())
            .all(|si| stage_draw_facts(m, si).is_some_and(|(v, ..)| cod2::draws_smodel(m, v)))
}

/// Whether a CoD2 model material draws its model's vertex colours unlit:
/// one with stages and no light probe technique (`vertcol_mul` decals,
/// `effect_add` glows, the unlit and emissive sets).
fn cod2_model_colour(m: &Material) -> bool {
    !m.stages.is_empty() && m.model_light == ModelLight::None
}

/// The image a model surface's `$texturename` binds: its skin file, or on
/// CoD2 its material's colour map (the stages bind it by path already).
fn model_skin_image(
    fs: &Pk3Fs,
    lib: &MaterialLib,
    decoded: &assets::DecodedImages,
    skin: &str,
) -> Image {
    if fs.game() == vcod_common::game::Game::Cod2 {
        colour_map_image(fs, lib.get(skin), decoded)
    } else {
        assets::load_skin_image(fs, skin)
    }
}

/// Every image path `materials` may load: their textures as named and
/// their stages' bundle images as [`upload_bundle_view`] resolves them.
fn prefetch_paths<'m>(
    fs: &dyn AssetSource,
    materials: impl Iterator<Item = &'m Material>,
) -> Vec<String> {
    let mut out = Vec::new();
    let bundle = |p: &str, out: &mut Vec<String>| {
        if let Some(full) = assets::resolve_bundle_image(fs, p) {
            out.push(full);
        }
    };
    for m in materials {
        for t in &m.textures {
            if let ImageRef::Path(p) = &t.image {
                out.push(p.clone());
                bundle(p, &mut out);
            }
        }
        for b in m.stages.iter().flat_map(|st| st.bundles.iter()) {
            if let ImageRef::Path(p) = &b.image {
                bundle(p, &mut out);
            }
            for p in b.anim.iter().flat_map(|a| a.paths.iter()) {
                bundle(p, &mut out);
            }
        }
    }
    out
}

/// A CoD2 material's colour map: the image its base draw and cutout test
/// read, the default image without one.
fn colour_map_image(
    fs: &Pk3Fs,
    material: Option<&Material>,
    prefetched: &assets::DecodedImages,
) -> Image {
    match material.and_then(|m| m.texture(TexSemantic::Color)) {
        Some(ImageRef::Path(path)) => prefetched.load_path_image(fs, path),
        _ => {
            log::debug!(
                "cod2: {} has no colour map",
                material.map_or("?", |m| &m.name)
            );
            assets::default_image()
        }
    }
}

/// The materials `fs` draws with: CoD1's shader scripts, or CoD2's
/// compiled binary materials (empty, logged, when they fail).
fn material_lib(fs: &dyn AssetSource) -> MaterialLib {
    match fs.game() {
        // `compile_all` fails only on a search path with no `materials/`:
        // a CoD4 install's, whose zones hand theirs over (`set_material_lib`).
        vcod_common::game::Game::Cod2 => vcod_cod2::materials::load_lib(fs).unwrap_or_else(|e| {
            log::debug!("cod2 materials: {e:#}");
            MaterialLib::default()
        }),
        _ => ShaderLib::load(fs).materials(),
    }
}

/// `ShaderLib::load` folds every material name to lowercase, so the lookup
/// does too before falling back to the name as given.
fn hud_texture_path(
    name: &str,
    image_map: &HashMap<String, String>,
    fs: &dyn AssetSource,
) -> HudTex {
    match name {
        "white" | "$whiteimage" => return HudTex::White,
        "black" => return HudTex::Black,
        hud::DEFAULT_SHADER => return HudTex::Default,
        _ => {}
    }
    if let Some(rest) = default_shader_fallback(name) {
        return match hud_texture_path(rest, image_map, fs) {
            HudTex::Missing => HudTex::Default,
            found => found,
        };
    }
    let path = image_map
        .get(name)
        .or_else(|| image_map.get(&name.to_ascii_lowercase()))
        .map(String::as_str)
        .unwrap_or(name);
    match resolve_fx_path(&HashMap::new(), fs, path) {
        Some(p) => HudTex::Path(p),
        None => HudTex::Missing,
    }
}

/// A CoD2 HUD name that is a material (`fonts/gamefonts`): its colour map,
/// ahead of any loose image of the same name.
fn cod2_hud_image<'a>(fs: &dyn AssetSource, lib: &'a MaterialLib, name: &str) -> Option<&'a str> {
    if fs.game() != vcod_common::game::Game::Cod2 {
        return None;
    }
    match lib.get(name)?.texture(TexSemantic::Color)? {
        ImageRef::Path(p) => Some(p),
        _ => None,
    }
}

/// `img` with every texel's alpha at 255.
fn opaque(mut img: Image) -> Image {
    if let ImageData::Rgba8(px) = &mut img.data {
        px.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|t| t[3] = 255);
    }
    img
}

/// The name a [`crate::hud::default_if_missing`] quad asks for.
fn default_shader_fallback(name: &str) -> Option<&str> {
    name.strip_prefix(hud::DEFAULT_SHADER)?.strip_prefix(':')
}

/// How retail uploads a HUD or menu image: flag 1 (mipmapped), flag 2
/// (`r_picmip`) and clamped (flags 0x10 and 0x20,
/// docs/research/cod11-front-end.md, section 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HudImage {
    mip: bool,
    picmip: bool,
    clamp: bool,
}

/// A scripted shader's stage `map` sets the flags from its keywords whatever
/// registered it; an implicit one takes flags 3 (mipmapped, picmipped,
/// repeating) from `RE_RegisterShader` and 0x30 (clamped, neither) from
/// `RE_RegisterShaderNoMip`, and of the cgame's 2D images only those in
/// `mip_names` go through the former.
fn hud_image(name: &str, sh: Option<&Material>, mip_names: &HashSet<String>) -> HudImage {
    if let Some(sh) = sh.filter(|sh| !sh.stages.is_empty()) {
        let clamp = sh
            .stages
            .iter()
            .flat_map(|st| st.bundles.first())
            .next()
            .is_some_and(|b| b.clamp.any());
        return HudImage {
            mip: !sh.nomipmaps,
            picmip: !sh.nopicmip,
            clamp,
        };
    }
    let mip = mip_names.contains(&hud_image_key(name));
    HudImage {
        mip,
        picmip: mip,
        clamp: !mip,
    }
}

/// A 2D image name as `hud_mip_names` stores it: lowercase, no extension.
fn hud_image_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase().replace('\\', "/");
    match lower.rsplit_once('.') {
        Some((stem, "tga" | "jpg" | "dds")) => stem.to_string(),
        _ => lower,
    }
}

/// The implicit 2D images the cgame registers through trap 0x30
/// (`RE_RegisterShader`, mipmapped) rather than 0x58 (`NoMip`): the four
/// icons of each weapon in `weapons`, configstring 7's table
/// (`CG_RegisterWeapon`, cgame 0x30034cf0), and the fixed names of
/// `CG_RegisterGraphics` (0x30020da0).
fn hud_mip_names(weapons: &[Option<vcod_common::weapon::WeaponDef>]) -> HashSet<String> {
    const GRAPHICS: [&str; 5] = [
        "gfx/2d/net",
        "gfx/2d/backtile",
        "gfx/2d/select",
        "gfx/hud/hud@objective_friendly",
        "gfx/hud/hud@objective_friendly_chat",
    ];
    let mut names: HashSet<String> = GRAPHICS.iter().map(|n| n.to_string()).collect();
    for def in weapons.iter().flatten() {
        for icon in [
            &def.hud_icon,
            &def.kill_icon,
            &def.mode_icon,
            &def.ammo_icon,
        ]
        .into_iter()
        .flatten()
        {
            names.insert(hud_image_key(icon));
        }
    }
    names
}

/// The bind group for a HUD texture: the image, `r_textureMode`'s sampler
/// and the `GL_LINEAR` one, both of the image's address mode.
fn hud_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    tex: &TexSamplers,
    view: &wgpu::TextureView,
    clamp: bool,
) -> wgpu::BindGroup {
    let (mode, linear) = if clamp {
        (&tex.clamp, &tex.linear_clamp)
    } else {
        (&tex.repeat, &tex.linear)
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hud texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(mode),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(linear),
            },
        ],
    })
}

/// A HUD texture by name ([`hud_texture_path`]), uploaded as `how` says
/// with `picmip` levels dropped, `gamma` baked as retail's upload bakes it
/// and stored as `store` says. Unresolvable names warn once; the caller
/// caches the `None`.
#[allow(clippy::too_many_arguments)]
fn resolve_hud_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    tex: &TexSamplers,
    fs: &dyn AssetSource,
    image_map: &HashMap<String, String>,
    material_image: Option<&str>,
    name: &str,
    how: HudImage,
    picmip: u32,
    gamma: Option<&[u8; 256]>,
    store: texture_settings::Store,
) -> Option<HudTexture> {
    let found = match (hud_texture_path(name, image_map, fs), material_image) {
        // A CoD2 material's colour map wins, a default-if-missing name's too.
        (HudTex::Path(_) | HudTex::Missing | HudTex::Default, Some(p)) => {
            HudTex::Path(p.to_string())
        }
        (found, _) => found,
    };
    let default = found == HudTex::Default;
    let (mut img, path) = match found {
        HudTex::White | HudTex::Black => (assets::white_1x1(), None),
        // The default shader's one stage blends nothing (0x4fd780), so its
        // image's alpha of 32 never shows.
        HudTex::Default => (opaque(assets::default_image()), None),
        HudTex::Path(path) => (assets::load_path_image(fs, &path), Some(path)),
        HudTex::Missing => {
            log::warn!("hud: no texture found for {name:?}, dropping its quads");
            return None;
        }
    };
    let load = texture_settings::path_levels(fs);
    let level_name = path.as_deref().unwrap_or(name);
    let mips = if how.mip {
        texture_settings::Mips::chain(level_name, &load)
    } else {
        texture_settings::Mips::single(level_name, &load)
    };
    let levels = texture_settings::prepare(&mut img, picmip, gamma, store, mips);
    let view = upload_levels(device, queue, name, &img, &levels);
    let bg = hud_bind_group(device, texture_layout, tex, &view, how.clamp);
    Some(HudTexture {
        view,
        clamp: how.clamp,
        bg,
        default,
    })
}

/// The viewmodel's uniform, skin and bone layouts and buffers; its
/// pipelines are the entity materials' `EntPass::View` ones, built with the
/// dynamic pass.
fn create_vm_pass(device: &wgpu::Device, tex: &TexSamplers) -> VmPass {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = TexSamplers::layout_entries();
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel uniform layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(VM_UNIFORM_SIZE),
                },
                count: None,
            },
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
        ],
    });
    let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel skin layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let bone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel bone layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(VM_BONE_BUF_SIZE),
            },
            count: None,
        }],
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viewmodel uniform"),
        size: VM_UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_bg = vm_uniform_group(device, &uniform_layout, &uniform_buf, tex);

    VmPass {
        uniform_layout,
        uniform_buf,
        uniform_bg,
        skin_layout,
        bone_layout,
        models: Vec::new(),
        skins: Vec::new(),
        cache: HashMap::new(),
        first_uses: 0,
    }
}

/// The viewmodel's group 0: its uniform and `r_textureMode`'s samplers.
fn vm_uniform_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
    tex: &TexSamplers,
) -> wgpu::BindGroup {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = tex.entries();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewmodel uniform bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            },
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
        ],
    })
}

/// Group 0 is the camera, group 1 the viewmodel skin layout (the uploaded
/// models' bind groups are built against it), group 2 the bone storage
/// buffer, group 3 the stage's `StageParams`. Vertex buffer 1 is
/// instance-step `InstanceRaw`. The entity materials also build the
/// viewmodel's pipelines over `vm`'s layouts.
/// [`DynHandle::bone_boxes`] of `model`'s LOD 0.
fn bone_boxes(model: &model::Model) -> Vec<(u8, glam::Vec3, glam::Vec3)> {
    bone_boxes_of(model.lods.first().into_iter().flat_map(|l| &l.surfaces))
}

/// [`bone_boxes`] over `surfaces`' vertices.
fn bone_boxes_of<'a>(
    surfaces: impl Iterator<Item = &'a model::Surface>,
) -> Vec<(u8, glam::Vec3, glam::Vec3)> {
    let mut boxes: HashMap<u8, (glam::Vec3, glam::Vec3)> = HashMap::new();
    for v in surfaces.flat_map(|s| &s.verts) {
        let p = glam::Vec3::from(v.pos);
        for (&b, &w) in v.bone_indices.iter().zip(&v.bone_weights) {
            if w > 0.0 {
                let e = boxes.entry(b).or_insert((p, p));
                *e = (e.0.min(p), e.1.max(p));
            }
        }
    }
    let mut out: Vec<_> = boxes.into_iter().map(|(b, (lo, hi))| (b, lo, hi)).collect();
    out.sort_by_key(|e| e.0);
    out
}

/// The world box of placed instance `p` posed by its bone matrices
/// (`DynamicPass::bone_mats` from its `bone_base`), `None` for a mesh
/// with no vertex.
fn posed_box(d: &DynamicPass, p: &Placed) -> Option<(glam::Vec3, glam::Vec3)> {
    posed_box_of(d, p, &d.handles[p.handle].bone_boxes)
}

/// [`posed_box`] over `boxes`, a handle's bone boxes.
fn posed_box_of(
    d: &DynamicPass,
    p: &Placed,
    boxes: &[(u8, glam::Vec3, glam::Vec3)],
) -> Option<(glam::Vec3, glam::Vec3)> {
    let transform = glam::Mat4::from_cols_array(&p.raw.transform);
    let mut out: Option<(glam::Vec3, glam::Vec3)> = None;
    for &(b, lo, hi) in boxes {
        let skin = d
            .bone_mats
            .get(p.raw.bone_base as usize + usize::from(b))
            .map_or(glam::Mat4::IDENTITY, glam::Mat4::from_cols_array);
        let m = transform * skin;
        for c in 0..8 {
            let corner = glam::Vec3::new(
                if c & 1 == 0 { lo.x } else { hi.x },
                if c & 2 == 0 { lo.y } else { hi.y },
                if c & 4 == 0 { lo.z } else { hi.z },
            );
            let w = m.transform_point3(corner);
            out = Some(out.map_or((w, w), |(a, z)| (a.min(w), z.max(w))));
        }
    }
    out
}

/// Whether instance `k` can reach the view `frustum` cuts: its posed box
/// over every LOD (`DynHandle::view_boxes`), grown by a unit and a
/// hundredth of its reach from the instance's origin (a skinned vertex
/// whose weights sum short of 1 is pulled toward it), meets the frustum.
/// An instance with a stage whose vertex shader moves its vertices (a
/// deform, a flag) or without a box is always seen.
fn instance_seen(d: &DynamicPass, k: usize, frustum: &vcod_common::vis::Frustum) -> bool {
    let (m, _) = d.instances[k];
    if !d.models[m]
        .skins
        .iter()
        .all(|sk| sk.stages.iter().all(|st| st.rigid))
    {
        return true;
    }
    let p = &d.placed[d.instance_placed[k]];
    let Some((lo, hi)) = posed_box_of(d, p, &d.handles[p.handle].view_boxes) else {
        return true;
    };
    let origin = glam::Mat4::from_cols_array(&p.raw.transform)
        .w_axis
        .truncate();
    let reach = (lo - origin).abs().max((hi - origin).abs()).max_element();
    let grow = glam::Vec3::splat(1.0 + reach * 0.01);
    frustum.intersects_aabb(lo - grow, hi + grow)
}

fn create_dynamic_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    vm: &VmPass,
    white_view: &wgpu::TextureView,
    textures: TextureSettings,
) -> DynamicPass {
    let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic instance transforms"),
        size: MAX_DYNAMIC_INSTANCES as u64 * DYNAMIC_INSTANCE_STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Worst case: every instance skinned at the cap, plus the identity block.
    let storage = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    // Binding 1 holds the frame's light sets; a CoD2 probe-lit surface
    // reads its set in the fragment stage.
    let bone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("dynamic bone layout"),
        entries: &[
            storage(0, wgpu::ShaderStages::VERTEX),
            storage(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
        ],
    });
    let bone_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic bone matrices"),
        size: (MAX_DYNAMIC_INSTANCES + 1) as u64 * MAX_INSTANCE_BONES as u64 * 64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let light_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic light sets"),
        size: MAX_DYNAMIC_INSTANCES as u64 * std::mem::size_of::<GpuLightSet>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bone_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dynamic bone bind group"),
        layout: &bone_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: bone_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: light_buf.as_entire_binding(),
            },
        ],
    });

    let materials = EntityMaterials::new(
        device,
        queue,
        format,
        PassLayouts {
            camera: camera_layout,
            bones: &bone_layout,
            view_uniform: &vm.uniform_layout,
            view_bones: &vm.bone_layout,
        },
        &vm.skin_layout,
        textures,
        white_view,
    );

    DynamicPass {
        instance_buf,
        bone_layout,
        bone_buf,
        bone_bg,
        handles: Vec::new(),
        models: Vec::new(),
        uploads: 0,
        placed: Vec::new(),
        instances: Vec::new(),
        instance_src: Vec::new(),
        lod_counts: LodCounts::default(),
        bone_mats: Vec::new(),
        light_origins: Vec::new(),
        light_boxes: Vec::new(),
        instance_placed: Vec::new(),
        light_buf,
        lighting: None,
        cod2_light: None,
        cod4_light: None,
        scene_lights: Vec::new(),
        entity_scene_lights: Vec::new(),
        materials,
    }
}

/// The projection and motion transform; the light set follows at 128.
fn vm_uniform(model: glam::Mat4, fov_x: f32, aspect: f32) -> [f32; 32] {
    let proj = crate::camera::perspective(fov_x, aspect, VM_NEAR, VM_FAR);
    let mut out = [0.0f32; 32];
    out[..16].copy_from_slice(&proj.to_cols_array());
    out[16..32].copy_from_slice(&model.to_cols_array());
    out
}

/// `None` when nothing is drawable (empty buffers are invalid to create).
/// `load_image` resolves a material name; the callers name materials
/// differently (skin filenames vs `textures/...`).
fn upload_vm_model(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    vm: &VmPass,
    model_surfaces: &[model::Surface],
    materials: &[String],
    load_image: &dyn Fn(&str) -> PreparedImage,
) -> Option<VmModel> {
    let mut verts: Vec<VmVert> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut bind_groups = Vec::with_capacity(model_surfaces.len());
    let mut bound_skins: HashMap<&str, usize> = HashMap::new();
    let mut surfaces = Vec::with_capacity(model_surfaces.len());
    let mut skins = Vec::with_capacity(model_surfaces.len());
    let drawn = model_surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.verts.is_empty() && !s.indices.is_empty());
    let rebase = drawn.clone().map(|(_, s)| s.verts.len()).sum::<usize>() <= 1 << 16;
    for (src, surf) in drawn {
        let skin = materials
            .get(surf.material)
            .map(String::as_str)
            .unwrap_or_default();
        let bind_group = *bound_skins.entry(skin).or_insert_with(|| {
            let (img, levels) = load_image(skin);
            let view = upload_levels(device, queue, skin, &img, &levels);
            bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("viewmodel skin bind group"),
                layout: &vm.skin_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            }));
            bind_groups.len() - 1
        });
        skins.push(skin.to_string());
        let base = verts.len();
        surfaces.push(VmSurface {
            first_index: indices.len() as u32,
            index_count: surf.indices.len() as u32,
            base_vertex: if rebase { 0 } else { base as i32 },
            bind_group,
            src: src as u32,
        });
        verts.extend_from_slice(&surf.verts);
        match rebase {
            // `base + i` stays under 1 << 16: the total is.
            true => indices.extend(surf.indices.iter().map(|&i| (base + usize::from(i)) as u16)),
            false => indices.extend_from_slice(&surf.indices),
        }
    }
    if surfaces.is_empty() {
        return None;
    }
    // Identity bones: skinning is a no-op until an animation is sampled.
    let identity: Vec<[f32; 16]> = vec![glam::Mat4::IDENTITY.to_cols_array(); VM_BONE_COUNT];
    let bone_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("viewmodel bone matrices"),
        contents: bytemuck::cast_slice(&identity),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bone_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewmodel bone bind group"),
        layout: &vm.bone_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: bone_buf.as_entire_binding(),
        }],
    });
    Some(VmModel {
        vertex_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewmodel vertices"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        index_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewmodel indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        }),
        bind_groups,
        surfaces,
        skins,
        bone_buf,
        bone_bg,
    })
}

fn wgpu_format(f: assets::TextureFormat) -> wgpu::TextureFormat {
    match f {
        assets::TextureFormat::Bc1RgbaUnorm => wgpu::TextureFormat::Bc1RgbaUnorm,
        assets::TextureFormat::Bc2RgbaUnorm => wgpu::TextureFormat::Bc2RgbaUnorm,
        assets::TextureFormat::Bc3RgbaUnorm => wgpu::TextureFormat::Bc3RgbaUnorm,
    }
}

/// A running hash of every texture the load uploads, label and bytes in
/// upload order, for the load profile to show a faster load uploads the
/// same textures. Off unless a test turns it on.
#[cfg(test)]
pub(crate) mod upload_digest {
    use std::hash::Hasher;
    use std::sync::Mutex;

    static DIGEST: Mutex<Option<(std::collections::hash_map::DefaultHasher, usize)>> =
        Mutex::new(None);

    /// Starts a digest; [`take`] ends it.
    pub fn start() {
        *DIGEST.lock().unwrap() = Some(Default::default());
    }

    /// `(hash, textures)` since [`start`].
    pub fn take() -> Option<(u64, usize)> {
        DIGEST.lock().unwrap().take().map(|(h, n)| (h.finish(), n))
    }

    pub fn feed(label: &str, data: &[&[u8]]) {
        if let Some((h, n)) = DIGEST.lock().unwrap().as_mut() {
            h.write(label.as_bytes());
            for d in data {
                h.write(d);
            }
            *n += 1;
        }
    }

    pub fn feed_image(label: &str, img: &vcod_common::assets::Image, levels: &[Vec<u8>]) {
        use vcod_common::assets::ImageData;
        let mut data: Vec<&[u8]> = match &img.data {
            ImageData::Rgba8(px) => vec![px],
            ImageData::Bc { mips, .. } => mips.iter().map(Vec::as_slice).collect(),
        };
        data.extend(levels.iter().map(Vec::as_slice));
        feed(label, &data);
    }
}

/// An image after `texture_settings::prepare`, with its smaller levels.
type PreparedImage = (Image, Vec<Vec<u8>>);

/// A model skin's image decoded and mipped under the renderer's texture
/// settings, as a model upload binds it.
fn prepare_skin(
    fs: &Pk3Fs,
    lib: &MaterialLib,
    images: &assets::DecodedImages,
    textures: &TextureSettings,
    gamma: Option<&[u8; 256]>,
    skin: &str,
) -> PreparedImage {
    let mut img = model_skin_image(fs, lib, images, skin);
    let levels = texture_settings::prepare(
        &mut img,
        textures.picmip2,
        gamma,
        textures.store(),
        texture_settings::Mips::chain(skin, &texture_settings::skin_levels(fs)),
    );
    (img, levels)
}

/// The skins `upload_vm_model` binds for these meshes, each once, in
/// order: those of the surfaces it draws.
fn drawn_skins<'m>(
    meshes: impl Iterator<Item = (&'m [model::Surface], &'m [String])>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (surfaces, materials) in meshes {
        for s in surfaces
            .iter()
            .filter(|s| !s.verts.is_empty() && !s.indices.is_empty())
        {
            let skin = materials
                .get(s.material)
                .map(String::as_str)
                .unwrap_or_default();
            if seen.insert(skin) {
                out.push(skin.to_string());
            }
        }
    }
    out
}

/// BC mip chain as-is, or a single RGBA level.
pub(crate) fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    img: &Image,
) -> wgpu::TextureView {
    upload_levels(device, queue, label, img, &[])
}

/// `upload_image` with an RGBA image's smaller levels
/// (`texture_settings::prepare`'s chain) under level 0.
pub(crate) fn upload_levels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    img: &Image,
    levels: &[Vec<u8>],
) -> wgpu::TextureView {
    #[cfg(test)]
    upload_digest::feed_image(label, img, levels);
    match &img.data {
        ImageData::Rgba8(px) if !levels.is_empty() => {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1 + levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (level, data) in std::iter::once(px).chain(levels).enumerate() {
                let w = (img.width >> level).max(1);
                let h = (img.height >> level).max(1);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
            texture.create_view(&Default::default())
        }
        ImageData::Rgba8(px) => upload_rgba(device, queue, label, img.width, img.height, px),
        ImageData::Bc { format, mips } => {
            let block_size = format.block_size();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format(*format),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (level, data) in mips.iter().enumerate() {
                let w = (img.width >> level).max(1);
                let h = (img.height >> level).max(1);
                // Copy extents are whole blocks; the 2x2 and 1x1 tail mips
                // still occupy one 4x4 block each.
                let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bw * block_size),
                        rows_per_image: Some(bh),
                    },
                    wgpu::Extent3d {
                        width: bw * 4,
                        height: bh * 4,
                        depth_or_array_layers: 1,
                    },
                );
            }
            texture.create_view(&Default::default())
        }
    }
}

fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    px: &[u8],
) -> wgpu::TextureView {
    upload_texel4(
        device,
        queue,
        label,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        px,
    )
}

/// One mip of four-byte texels in `format`.
pub(crate) fn upload_texel4(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    px: &[u8],
) -> wgpu::TextureView {
    #[cfg(test)]
    upload_digest::feed(label, &[px]);
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&Default::default())
}

/// Resolves a bundle image path through the asset probe chain; a missing
/// file warns and lands in the default image. Views are deduped per load.
fn upload_bundle_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &Pk3Fs,
    cache: &mut BundleViews,
    path: &str,
) -> wgpu::TextureView {
    if let Some(v) = cache.views.get(path) {
        return v.clone();
    }
    let mut picmip = cache.picmip;
    let mut img = if path == "$dlight" {
        // engine-generated light blob, never a file on disk (research doc §8)
        assets::dlight_blob()
    } else if path == vcod_common::shader::DEFAULT_IMAGE {
        assets::default_image()
    } else if path == "textures/battleship/deckflag_np.tga" {
        // ships in no stock pak; retail binds its default image there too
        assets::white_1x1()
    } else {
        match assets::resolve_bundle_image(fs, path) {
            Some(full) => {
                // a CoD4 image's header can refuse the picmip
                if picmip > 0 && full.to_ascii_lowercase().ends_with(".iwi") {
                    let head = fs.read_unflagged(&full).unwrap_or_default();
                    picmip = texture_settings::iwi_picmip(&head, picmip);
                }
                match cache.prefetched {
                    Some(p) => p.load_path_image(fs, &full),
                    None => assets::load_path_image(fs, &full),
                }
            }
            None => {
                log::warn!("shader image {path} not found, using the default image");
                assets::default_image()
            }
        }
    };
    // Retail generates the blob unmipped at its native size, which skips
    // the texture gamma (docs/research/cod11-gamma.md, section 3).
    let load = texture_settings::path_levels(fs);
    let levels = if path == "$dlight" {
        Vec::new()
    } else {
        let mips = if cache.mipmap {
            texture_settings::Mips::chain(path, &load)
        } else {
            texture_settings::Mips::single(path, &load)
        };
        texture_settings::prepare(&mut img, picmip, cache.table, cache.store, mips)
    };
    let v = upload_levels(device, queue, path, &img, &levels);
    cache.views.insert(path.to_string(), v.clone());
    v
}

/// `r_dynamiclight` 1 keeps the world's dlight pass; any other value
/// zeroes its light count (0x4e9cb0).
fn lights_world(mode: i32) -> bool {
    mode == 1
}

/// Only `r_dynamiclight` 0 zeroes the light count the model light pick
/// reads, so 2 ("Models Only") still lights entities (0x4e9cb0; the
/// split between the two counts is INFERRED, light grid doc).
fn lights_models(mode: i32) -> bool {
    mode != 0
}

/// Shader-stage images uploaded during one world load, and the texture
/// gamma they load through (`r_ignorehwgamma 1`, `gamma::bake_image`).
struct BundleViews<'a> {
    views: HashMap<String, wgpu::TextureView>,
    table: Option<&'a [u8; 256]>,
    /// Levels the next new image drops, set per shader by the caller; an
    /// image already loaded keeps its first picmip, as retail's image
    /// cache does (0x4ee7ff warns on the mix).
    picmip: u32,
    /// Whether the next new image gets a mip chain: not under `nomipmaps`.
    mipmap: bool,
    /// `r_texturebits` and the mip filter.
    store: texture_settings::Store,
    /// What the world load decoded ahead ([`assets::DecodedImages`]).
    prefetched: Option<&'a assets::DecodedImages>,
}

fn fallback_pixels() -> Vec<u8> {
    match assets::default_image().data {
        ImageData::Rgba8(px) => px,
        _ => Vec::new(),
    }
}

/// True for `load_material_image`'s default-image placeholder.
fn is_fallback(img: &Image, fallback_px: &[u8]) -> bool {
    img.width == 16
        && img.height == 16
        && matches!(&img.data, ImageData::Rgba8(px) if px == fallback_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;
    use vcod_common::shader::{
        BlendFactor, Bundle, DepthFunc, PolygonOffset, SORT_ADDITIVE, Stage, TcMod, Wave, WaveForm,
    };

    #[test]
    fn gather_copies_visible_ranges_batch_by_batch() {
        use vcod_common::mesh::IndexRange;
        let src: Vec<u32> = (0..12).collect();
        let ranges = [
            IndexRange {
                batch: 1,
                first: 6,
                count: 3,
            },
            IndexRange {
                batch: 0,
                first: 0,
                count: 3,
            },
            IndexRange {
                batch: 1,
                first: 9,
                count: 3,
            },
        ];
        let mut out = Vec::new();
        let mut scratch = Vec::new();
        let draws = gather(&src, ranges, 3, &mut out, &mut scratch);
        assert_eq!(out, vec![0, 1, 2, 6, 7, 8, 9, 10, 11]);
        assert_eq!(
            draws,
            vec![
                DrawRange {
                    batch: 0,
                    first: 0,
                    count: 3
                },
                DrawRange {
                    batch: 1,
                    first: 3,
                    count: 6
                },
            ]
        );
        let draws = gather(&src, [], 3, &mut out, &mut scratch);
        assert!(draws.is_empty() && out.is_empty());
    }

    #[test]
    fn packs_identity_block_and_bone_bases() {
        let id = Mat4::IDENTITY.to_cols_array();
        let bones_a = vec![Mat4::from_translation(glam::Vec3::X); 3];
        let items = [
            (0usize, id, None),                // static: identity block
            (1, id, Some(bones_a.as_slice())), // skinned, 3 bones
            (0, id, None),
        ];
        let (raw, mats) = pack_instances(&items);
        assert_eq!(raw.len(), 3);
        assert_eq!(raw[0].bone_base, 0);
        assert_eq!(raw[2].bone_base, 0);
        assert_eq!(raw[1].bone_base, MAX_INSTANCE_BONES as u32);
        assert_eq!(mats.len(), MAX_INSTANCE_BONES + 3);
        assert_eq!(mats[0], Mat4::IDENTITY.to_cols_array());
        assert_eq!(mats[MAX_INSTANCE_BONES], bones_a[0].to_cols_array());
    }

    #[test]
    fn hud_colour_scales_rgb_by_identity_light_and_keeps_alpha() {
        assert_eq!(hud_colour([0.0, 1.0, 0.5, 0.6], 0.5), [0.0, 0.5, 0.25, 0.6]);
        assert_eq!(hud_colour([0.15; 4], 1.0), [0.15; 4]);
    }

    /// Lit gens and implicit materials take identityLight; identity,
    /// exactVertex and const draw their colour as it is (0x4ffa60).
    #[test]
    fn colour_scale_follows_the_first_stage_gen() {
        let with = |rgb_gen: RgbGen| {
            Material::from_shader(&Shader {
                name: "t".into(),
                stages: vec![Stage {
                    bundles: vec![],
                    blend: None,
                    depth_write: None,
                    alpha_func: None,
                    depth_func: DepthFunc::LessEqual,
                    rgb_gen,
                    alpha_gen: AlphaGen::Identity,
                }],
                ..Default::default()
            })
        };
        assert_eq!(colour_scale(None, 0.5), 0.5);
        assert_eq!(colour_scale(Some(&with(RgbGen::Vertex)), 0.5), 0.5);
        assert_eq!(colour_scale(Some(&with(RgbGen::ExactVertex)), 0.5), 1.0);
        assert_eq!(colour_scale(Some(&with(RgbGen::Identity)), 0.5), 1.0);
        assert_eq!(
            colour_scale(Some(&with(RgbGen::IdentityLighting)), 1.0),
            1.0
        );
    }

    /// `body` is a block's inside; newlines end a line as in a script.
    fn parse_fx(body: &str) -> Material {
        let text = format!("t {{\n{body}\n}}");
        let (name, toks) = vcod_common::shader::split_blocks(&text).remove(0);
        Material::from_shader(&vcod_common::shader::parse_shader(
            &name,
            &toks,
            &mut vcod_common::shader::WarnSet::new(),
        ))
    }

    /// The fx pass draws each stage on its own blendFunc and fogs it unless
    /// the material is `nofog`; only `GL_SRC_ALPHA GL_ONE` reads the
    /// texture's alpha among the additive ones, and an unblended stage is
    /// opaque (CoDMP.exe 0x4d57b0, 0x4fbc71).
    #[test]
    fn fx_stage_draw_follows_the_stage_blend_and_nofog() {
        let draw = |body: &str| {
            let sh = parse_fx(body);
            fx_stage_draw(&sh, &sh.stages[0])
        };
        assert_eq!(
            draw("{ map a.tga blendFunc blend rgbGen vertex }"),
            (FxBlend::Alpha, 0)
        );
        assert_eq!(
            draw("{ map a.tga blendFunc GL_ONE GL_ONE rgbGen vertex }"),
            (FxBlend::Add, 0)
        );
        assert_eq!(
            draw("{ map a.tga blendFunc add rgbGen vertex }"),
            (FxBlend::Add, 0)
        );
        assert_eq!(
            draw("{ map a.tga blendFunc GL_SRC_ALPHA GL_ONE rgbGen vertex }"),
            (FxBlend::Add, FX_VERT_TEX_ALPHA)
        );
        // UO's flamethrowerFire* and flameSmoke*
        assert_eq!(
            draw("nofog { map a.dds blendFunc GL_ONE GL_ONE_MINUS_SRC_COLOR rgbGen vertex }"),
            (FxBlend::Screen, FX_VERT_NOFOG)
        );
        assert_eq!(
            draw(
                "nofog { map a.dds blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA \
                 alphaGen vertex rgbGen vertex }"
            ),
            (FxBlend::Alpha, FX_VERT_NOFOG)
        );
        // UO's smoke/smk_t_fire_*: additive and nofog
        assert_eq!(
            draw("nofog { map a.dds blendFunc add rgbGen vertex }"),
            (FxBlend::Add, FX_VERT_NOFOG)
        );
        // gfx/impact/bark_gib and the other debris: unblended, cut at 128
        assert_eq!(
            draw("{ map a.tga rgbGen vertex alphaGen vertex alphaFunc GE128 depthWrite }"),
            (FxBlend::Opaque, FX_VERT_ALPHA_GE128)
        );
    }

    /// Every base stage draws, in order, on its own image and tcMods; a
    /// `perLight` stage is the dlight pass's and a `$lightmap` bundle 1 is
    /// not drawn (CoDMP.exe 0x500b30, 0x4f3699).
    #[test]
    fn fx_stage_draws_every_base_stage() {
        assert_eq!(fx_stage_draws(None, "gfx/x", 0.0).len(), 1);
        assert_eq!(fx_stage_draws(None, "gfx/x", 0.0)[0].image, "gfx/x");
        let clamp = FX_VERT_CLAMP_S | FX_VERT_CLAMP_T;
        assert_eq!(fx_stage_draws(None, "gfx/x", 0.0)[0].flags, clamp);
        // gfx/impact/bullethit_plaster: the perLight overlay stays out
        let hit = parse_fx(
            "{ map gfx/impact/bullethit_plaster blendfunc blend rgbGen vertex \
             nextbundle map $lightmap } \
             { perlight map gfx/impact/bullethit_plaster blendfunc GL_SRC_ALPHA GL_ONE \
             rgbGen vertex nextbundle map $dlight }",
        );
        let draws = fx_stage_draws(Some(&hit), "t", 0.0);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].image, "gfx/impact/bullethit_plaster");
        assert_eq!(draws[0].blend, FxBlend::Alpha);
        // no `clamp`: the image wraps
        assert_eq!(draws[0].flags, 0);
        // gfx/effects/smoke/roller: two blended stages, each turning
        let roller = parse_fx(
            "sort additive \
             { map clamp /gfx/effects/smoke/roller blendfunc blend rgbGen vertex tcMod rotate -90 } \
             { map clamp /gfx/effects/smoke/smoke_frnt blendfunc blend rgbGen vertex tcMod rotate 80 }",
        );
        let draws = fx_stage_draws(Some(&roller), "t", 0.25);
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].image, "gfx/effects/smoke/roller");
        assert_eq!(draws[1].image, "gfx/effects/smoke/smoke_frnt");
        assert_eq!(draws[0].flags, clamp);
        assert_eq!(draws[0].uv, bundle_affine(&[TcMod::Rotate(-90.0)], 0.25));
        assert_eq!(draws[1].uv, bundle_affine(&[TcMod::Rotate(80.0)], 0.25));
        // UO's impact/bullet_fabric_a: a blended stage and an additive glow
        let fabric = parse_fx(
            "{ map a.dds blendfunc blend rgbGen vertex nextbundle map $lightmap } \
             { map glow.dds blendFunc add rgbGen wave sin .90 .25 .5 .5 } \
             { perlight map a.dds blendfunc blend rgbGen identity nextbundle map $dlight }",
        );
        let blends: Vec<FxBlend> = fx_stage_draws(Some(&fabric), "t", 0.0)
            .iter()
            .map(|d| d.blend)
            .collect();
        assert_eq!(blends, [FxBlend::Alpha, FxBlend::Add]);
        // gfx/effects/animated/animfire: the frame at the shader time
        let fire = parse_fx(
            "{\nanimMap 4\nmap a.tga\nmap b.tga\nmap c.tga\nblendFunc add\nrgbGen vertex\n}",
        );
        let image = |t| fx_stage_draws(Some(&fire), "t", t)[0].image;
        assert_eq!(image(0.0), "a.tga");
        assert_eq!(image(0.5), "c.tga");
        assert_eq!(image(0.8), "a.tga");
    }

    /// A stage's colour follows its gens, not the sprite's colour alone
    /// (CoDMP.exe 0x4ffa60): identity is opaque white, vertex the sprite's
    /// colour times identityLight with its alpha, a wave ignores the sprite.
    #[test]
    fn fx_stage_rgba_follows_the_stage_gens() {
        let v = [0.8, 0.6, 0.4, 0.5];
        let rgba = |body: &str| {
            let sh = parse_fx(body);
            fx_stage_rgba(Some(&sh.stages[0]), v, 0.5, 0.0)
        };
        assert_eq!(fx_stage_rgba(None, v, 0.5, 0.0), [0.4, 0.3, 0.2, 0.5]);
        assert_eq!(
            rgba("{ map a.tga blendfunc blend rgbGen vertex }"),
            [0.4, 0.3, 0.2, 0.5]
        );
        assert_eq!(
            rgba("{ map a.tga blendfunc blend rgbGen exactVertex }"),
            [0.8, 0.6, 0.4, 0.5]
        );
        // UO's impact/bullet_sand_a: the sprite's fade never reaches it
        assert_eq!(
            rgba("{ map a.dds blendfunc blend rgbgen identity }"),
            [1.0, 1.0, 1.0, 1.0]
        );
        assert_eq!(
            rgba("{ map a.tga blendfunc blend rgbGen identityLighting }"),
            [0.5, 0.5, 0.5, 1.0]
        );
        assert_eq!(
            rgba("{ map a.tga blendfunc blend rgbGen vertex alphaGen const 0.25 }"),
            [0.4, 0.3, 0.2, 0.25]
        );
        // an `alphaGen identity` after the rgbGen stays; case 0 drops the
        // copied alpha unless identityLight is 1
        let late = parse_fx("{ map a.tga blendfunc blend rgbGen vertex alphaGen identity }");
        assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 0.5, 0.0)[3], 1.0);
        assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 1.0, 0.0)[3], 0.5);
        let late = parse_fx("{ map a.tga blendfunc blend rgbGen exactVertex alphaGen identity }");
        assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 1.0, 0.0)[3], 1.0);
        // GL_ONE GL_ONE reads no source alpha
        assert_eq!(
            rgba("{ map a.tga blendFunc add rgbGen vertex }"),
            [0.4, 0.3, 0.2, 1.0]
        );
        // a table wave is scaled by identityLight, then clamped; alpha 1
        let sh = parse_fx("{ map a.tga blendFunc add rgbGen wave sin .90 .25 .5 .5 }");
        let RgbGen::Wave(w) = &sh.stages[0].rgb_gen else {
            panic!("wave gen");
        };
        let k = (rgb_wave_value(w, 0.3) * 0.5).clamp(0.0, 1.0);
        assert_eq!(
            fx_stage_rgba(Some(&sh.stages[0]), v, 0.5, 0.3),
            [k, k, k, 1.0]
        );
    }

    /// Each fx pipeline's colour blend is retail's glBlendFunc for its
    /// `FxBlend`; the additive two leave the target's alpha alone.
    #[test]
    fn fx_blend_states_match_the_gl_pairs() {
        use wgpu::BlendFactor as W;
        let pair = |b: FxBlend| {
            let c = fx_blend_state(b).unwrap().color;
            (c.src_factor, c.dst_factor)
        };
        assert_eq!(pair(FxBlend::Alpha), (W::SrcAlpha, W::OneMinusSrcAlpha));
        assert_eq!(pair(FxBlend::Add), (W::One, W::One));
        assert_eq!(pair(FxBlend::Screen), (W::One, W::OneMinusSrc));
        assert_eq!(fx_blend_state(FxBlend::Opaque), None);
        for b in [FxBlend::Add, FxBlend::Screen] {
            let a = fx_blend_state(b).unwrap().alpha;
            assert_eq!((a.src_factor, a.dst_factor), (W::One, W::One));
        }
    }

    #[test]
    fn resolve_fx_path_drops_empty_and_missing_names_but_keeps_a_known_texture() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let no_mapping = HashMap::new();

        // An emitter with no `shaders` key hands over an empty string.
        assert_eq!(resolve_fx_path(&no_mapping, &fs, ""), None);

        assert_eq!(
            resolve_fx_path(&no_mapping, &fs, "fx/does/not/exist.tga"),
            None
        );

        assert_eq!(
            resolve_fx_path(&no_mapping, &fs, "gfx/impact/bullethole1.tga"),
            Some("gfx/impact/bullethole1.tga".to_string())
        );

        // Extensionless material name, as fx::sim hands over.
        assert_eq!(
            resolve_fx_path(&no_mapping, &fs, "gfx/impact/bullethole1"),
            Some("gfx/impact/bullethole1.tga".to_string())
        );

        assert_eq!(resolve_fx_path(&no_mapping, &fs, "fx/does/not/exist"), None);

        let mut mapped = HashMap::new();
        mapped.insert(
            "some_material".to_string(),
            "gfx/impact/bullethole1.tga".to_string(),
        );
        assert_eq!(
            resolve_fx_path(&mapped, &fs, "some_material"),
            Some("gfx/impact/bullethole1.tga".to_string())
        );
    }

    #[test]
    fn resolve_fx_path_resolves_default_hit_efx_shader_names() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let no_mapping = HashMap::new();
        for name in [
            "gfx/impact/bullethole1",
            "gfx/impact/dustlayer1",
            "gfx/impact/sparkflash",
            "gfx/impact/stone_piece1",
            "gfx/impact/stone_piece2",
        ] {
            assert!(
                resolve_fx_path(&no_mapping, &fs, name).is_some(),
                "{name} should resolve to a real texture, not drop its quads"
            );
        }
    }

    /// Weapon `killIcon`s name `.tga` while the art ships as `.dds`.
    #[test]
    fn resolve_fx_path_falls_back_to_the_real_extension_when_the_named_one_is_missing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let no_mapping = HashMap::new();
        assert_eq!(
            resolve_fx_path(&no_mapping, &fs, "gfx/hud/hud@death_kar98.tga"),
            Some("gfx/hud/hud@death_kar98.dds".to_string())
        );
    }

    /// `gfx/misc/tracer`'s image lives under a different path and
    /// `gfx/effects/whitesmoke`'s is written with a leading slash; both only
    /// resolve through the fxshaders material map.
    #[test]
    fn resolve_fx_path_uses_fxshaders_materials() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let shaders = vcod_common::assets::load_shaders(&fs);
        assert_eq!(
            resolve_fx_path(shaders.image_map(), &fs, "gfx/misc/tracer"),
            Some("textures/sfx/tracer.jpg".to_string())
        );
        assert_eq!(
            resolve_fx_path(shaders.image_map(), &fs, "gfx/effects/whitesmoke"),
            Some("gfx/effects/whitesmoke.tga".to_string())
        );
        for name in [
            "gfx/effects/muzflash2",
            "gfx/effects/muzflash2a",
            "gfx/effects/mg42a",
            "gfx/misc/tracer",
        ] {
            assert!(
                resolve_fx_path(shaders.image_map(), &fs, name).is_some(),
                "{name} should resolve"
            );
            assert!(shaders.is_additive(name), "{name} should be additive");
        }
    }

    #[test]
    fn resolve_fx_path_resolves_bc_compressed_effect_textures() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let no_mapping = HashMap::new();
        for name in ["gfx/effects/pjsmoke", "gfx/impact/cratered"] {
            let path = resolve_fx_path(&no_mapping, &fs, name);
            assert!(path.is_some(), "{name} should resolve to a real texture");
            let img = vcod_common::assets::load_path_image(&fs, path.as_ref().unwrap());
            assert!(
                matches!(img.data, ImageData::Bc { .. }),
                "{name} should decode as BC-compressed, exercising the upload_image path"
            );
        }
    }

    /// `white` and `$whiteimage` are the engine built-in; `black` shares its
    /// texture (pak4 `scripts/hud.shader`: both stage `map $whiteimage`,
    /// `black` differs only by `rgbGen const 0`, which `set_hud_quads`
    /// applies to the quad colour).
    /// A CoD2 font's material draws its colour map, not the loose
    /// `fonts/gamefonts.tga` beside the font files.
    #[test]
    fn cod2_hud_names_resolve_through_their_material() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let lib = material_lib(&fs);
        assert_eq!(
            cod2_hud_image(&fs, &lib, "fonts/gamefonts"),
            Some("images/gamefonts.iwi")
        );
        assert_eq!(cod2_hud_image(&fs, &lib, "fonts/no_such_material"), None);
    }

    #[test]
    fn hud_texture_path_resolves_the_white_and_black_built_ins() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let no_mapping = HashMap::new();
        assert_eq!(hud_texture_path("white", &no_mapping, &fs), HudTex::White);
        assert_eq!(
            hud_texture_path("$whiteimage", &no_mapping, &fs),
            HudTex::White
        );
        assert_eq!(hud_texture_path("black", &no_mapping, &fs), HudTex::Black);
    }

    /// UO's health back is `hud@health.dds` in pakuo00; 1.1's pak5
    /// `hud@health.tga`, the name hud.menu gives, is a white disc.
    #[test]
    fn uo_health_back_is_the_uo_dds() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        assert_eq!(
            hud_texture_path("gfx/hud/hud@health.tga", &HashMap::new(), &fs),
            HudTex::Path("gfx/hud/hud@health.dds".to_string())
        );
    }

    /// UO's `nofog` fx materials (`scripts/flamethrower.shader`,
    /// `fxshaders/smoke.shader`) draw unfogged, each on its own blend; the
    /// impacts draw their base stages, not the `perLight` overlay
    /// (`fxshaders/impact.shader`).
    #[test]
    fn uo_fx_materials_draw_their_base_stages_on_their_blends() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let lib = ShaderLib::load(&fs).materials();
        let alpha = (FxBlend::Alpha, 0);
        for (name, want) in [
            ("flamethrowerFire12", vec![(FxBlend::Screen, FX_VERT_NOFOG)]),
            ("flameSmoke1", vec![(FxBlend::Alpha, FX_VERT_NOFOG)]),
            ("smoke/smk_t_fire_a", vec![(FxBlend::Add, FX_VERT_NOFOG)]),
            ("impact/bullet_dirt_a", vec![alpha]),
            ("impact/bullet_sand_a", vec![alpha]),
            ("impact/bullet_fabric_a", vec![alpha, (FxBlend::Add, 0)]),
        ] {
            let sh = lib.get(name);
            assert!(sh.is_some(), "{name} has no script");
            let got: Vec<(FxBlend, u32)> = fx_stage_draws(sh, name, 0.0)
                .iter()
                .map(|d| (d.blend, d.flags & !(FX_VERT_CLAMP_S | FX_VERT_CLAMP_T)))
                .collect();
            assert_eq!(got, want, "{name}");
        }
        // rgbgen identity: white and opaque whatever the sprite's fade
        let sand = lib.get("impact/bullet_sand_a").unwrap();
        assert_eq!(
            fx_stage_rgba(Some(&sand.stages[0]), [0.5, 0.5, 0.5, 0.2], 0.5, 0.0),
            [1.0; 4]
        );
    }

    #[test]
    fn hud_texture_path_resolves_a_material_a_plain_path_and_a_miss() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let shaders = vcod_common::assets::load_shaders(&fs);
        assert_eq!(
            hud_texture_path("hudStanceStand", shaders.image_map(), &fs),
            HudTex::Path("gfx/hud/stance_stand.dds".to_string())
        );
        assert_eq!(
            hud_texture_path("gfx/hud/hud@compassback", &HashMap::new(), &fs),
            HudTex::Path("gfx/hud/hud@compassback.tga".to_string())
        );
        assert_eq!(
            hud_texture_path("gfx/hud/does_not_exist", &HashMap::new(), &fs),
            HudTex::Missing
        );
        // A style 3 window's image falls back to the default shader.
        let style3 = |n: &str| hud_texture_path(&hud::default_if_missing(n), &HashMap::new(), &fs);
        assert_eq!(style3("gfx/hud/does_not_exist"), HudTex::Default);
        assert_eq!(
            style3("gfx/hud/hud@compassback"),
            HudTex::Path("gfx/hud/hud@compassback.tga".to_string())
        );
    }

    #[test]
    fn the_default_shader_is_its_own_hud_texture() {
        let fs = Pk3Fs::empty();
        assert_eq!(
            hud_texture_path(hud::DEFAULT_SHADER, &HashMap::new(), &fs),
            HudTex::Default
        );
        let img = opaque(assets::default_image());
        let ImageData::Rgba8(px) = &img.data else {
            panic!("rgba")
        };
        assert!(px.as_chunks::<4>().0.iter().all(|t| t[3] == 255));
    }

    #[test]
    fn over_cap_bone_set_truncates() {
        let id = Mat4::IDENTITY.to_cols_array();
        let big = vec![Mat4::IDENTITY; MAX_INSTANCE_BONES + 40];
        let (raw, mats) = pack_instances(&[(0, id, Some(big.as_slice()))]);
        assert_eq!(raw[0].bone_base, MAX_INSTANCE_BONES as u32);
        assert_eq!(mats.len(), 2 * MAX_INSTANCE_BONES); // identity block + truncated set
    }

    // ---- StageParams ----

    #[test]
    fn stage_params_is_std140_shaped_and_pod_roundtrips() {
        assert_eq!(std::mem::size_of::<StageParams>(), 288);
        let p = StageParams {
            uv0: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            uv1: [7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
            turb01: [13.0, 14.0, 15.0, 16.0],
            tint: [17.0, 18.0, 19.0, 20.0],
            flags: STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_ALPHA,
            uv0_rate: [0.0; 3],
            vec0_s: [1.0, 0.0, 0.0, 0.0],
            vec0_t: [0.0, 1.0, 0.0, 0.0],
            vec1_s: [0.0, 0.0, 1.0, 0.0],
            vec1_t: [0.0, 0.0, 0.0, 1.0],
            eye_off: [9.0, 8.0, 7.0, 0.0],
            deform: [
                [1.0, 2.0, 3.0, 4.0],
                [5.0, 6.0, 7.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
            ],
            deform_kinds: [DEFORM_WAVE, DEFORM_BULGE, DEFORM_MOVE, 0],
            falloff: [[0.0; 4]; 2],
            fx: [0.0; 4],
        };
        // slot-shaped write/read, like the UBO dynamic-offset path
        let mut slot = [0u8; std::mem::size_of::<StageParams>()];
        slot[..bytemuck::bytes_of(&p).len()].copy_from_slice(bytemuck::bytes_of(&p));
        let q: &StageParams = bytemuck::try_from_bytes(&slot).unwrap();
        assert_eq!(*q, p);
    }

    /// Sky dome stages ride the same UBO slots with the eye-offset flag set.
    /// `tcGen environment` dots the reflection with the drawing frame's y
    /// and z, or with a static prop's model axes once the batch carries them.
    #[test]
    fn stage_params_carry_the_environment_axes() {
        let sh = Material::from_shader(&vcod_common::shader::parse_shader(
            "metal_env",
            &["{", "map", "env.jpg", "tcgen", "environment", "}"],
            &mut vcod_common::shader::WarnSet::new(),
        ));
        let p = stage_params(&sh, 0, 0.0).unwrap();
        assert_eq!(p.flags & STAGE_FLAG_BUNDLE0_ENV, STAGE_FLAG_BUNDLE0_ENV);
        assert_eq!(
            [p.vec0_s, p.vec0_t],
            [[0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
        );
        let turned = Material {
            model_axes: Some([[-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            ..sh
        };
        let p = stage_params(&turned, 0, 0.0).unwrap();
        assert_eq!(
            [p.vec0_s, p.vec0_t],
            [[-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
        );
    }

    /// A CoD4 reflex sight's stage carries its code and `detailScale` to
    /// viewmodel.wgsl, which projects the dot (cod4-client-view.md 4.5).
    #[test]
    fn stage_params_mark_a_reflex_sight() {
        let sh = Material::from_shader(&vcod_common::shader::parse_shader(
            "mc/mtl_weapon_reflex_red_dot",
            &["{", "map", "dot.jpg", "}"],
            &mut vcod_common::shader::WarnSet::new(),
        ));
        assert_eq!(stage_params(&sh, 0, 0.0).unwrap().deform_kinds[3], 0);
        let dot = Material {
            reflex: Some([25.0, 25.0]),
            ..sh
        };
        let p = stage_params(&dot, 0, 0.0).unwrap();
        assert_eq!(p.deform_kinds[3], DEFORM_COD4_REFLEX);
        assert_eq!(p.eye_off, [25.0, 25.0, 0.0, 0.0]);
    }

    #[test]
    fn stage_params_eye_offset_flag_marks_dome_draws() {
        let sh = Material::from_shader(&Shader {
            name: "test/skydome".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("clouds".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![TcMod::Scroll(0.004, 0.004), TcMod::Scale(3.0, 3.0)],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let mut p = stage_params(&sh, 0, 5.0).unwrap();
        p.flags |= STAGE_FLAG_EYE_OFFSET;
        p.eye_off = [100.0, 200.0, 300.0, 0.0];
        assert_eq!(p.flags & STAGE_FLAG_EYE_OFFSET, STAGE_FLAG_EYE_OFFSET);
        // the affine still carries scroll+scale for the dome's raw-radian uvs
        assert_eq!(
            p.uv0,
            bundle_affine(&[TcMod::Scroll(0.004, 0.004), TcMod::Scale(3.0, 3.0)], 5.0)
        );
        // world draws never set the flag; the default params stay put
        let world_p = stage_params(&sh, 0, 5.0).unwrap();
        assert_eq!(world_p.flags & STAGE_FLAG_EYE_OFFSET, 0);
        assert_eq!(world_p.eye_off, [0.0; 4]);
    }

    #[test]
    fn stages_past_the_shadow_mask_shade_themselves_unless_they_add_or_filter() {
        use BlendFactor::*;
        let flagged = |blend: Option<(BlendFactor, BlendFactor)>, sort: Option<f32>| {
            let sh = Material::from_shader(&Shader {
                stages: vec![Stage {
                    bundles: vec![Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    }],
                    blend,
                    depth_write: None,
                    alpha_func: None,
                    depth_func: DepthFunc::LessEqual,
                    rgb_gen: RgbGen::Identity,
                    alpha_gen: AlphaGen::Identity,
                }],
                sort,
                ..Default::default()
            });
            stage_params(&sh, 0, 0.0).unwrap().flags & STAGE_FLAG_SUN_SHADOW != 0
        };
        assert!(flagged(Some((SrcAlpha, OneMinusSrcAlpha)), None));
        assert!(flagged(Some((One, OneMinusSrcAlpha)), None));
        // opaque stages and decals sort before the mask, which darkens them
        assert!(!flagged(None, None));
        assert!(!flagged(
            Some((SrcAlpha, OneMinusSrcAlpha)),
            Some(SORT_DECAL)
        ));
        assert!(flagged(None, Some(vcod_common::shader::SORT_BANNER)));
        // additive light and filters over the darkened frame
        assert!(!flagged(Some((One, One)), None));
        assert!(!flagged(Some((SrcAlpha, One)), None));
        assert!(!flagged(Some((DstColor, Zero)), None));
        assert!(!flagged(Some((Zero, OneMinusSrcColor)), Some(SORT_BLEND)));
    }

    /// A world stage with `rgbGen vertex` or `exactVertex` and no earlier
    /// alphaGen reads the vertex alpha (CoDMP.exe 0x4f8d70): mp_bocage's
    /// rockface decals and mp_sicily's shoreline fade by it.
    #[test]
    fn stage_params_reads_vertex_alpha_under_a_vertex_rgbgen() {
        let flags = |body: &str| stage_params(&parse_fx(body), 0, 0.0).unwrap().flags;
        for body in [
            "{ map a.tga blendFunc blend rgbGen exactVertex }",
            "{ map a.tga alphaFunc GE128 rgbGen vertex }",
            "{ map a.tga blendFunc GL_SRC_ALPHA GL_ONE alphaGen identity rgbGen vertex }",
        ] {
            assert_ne!(flags(body) & STAGE_FLAG_VERTEX_ALPHA, 0, "{body}");
        }
        for body in [
            "{ map a.tga blendFunc blend rgbGen exactVertex alphaGen identity }",
            "{ map a.tga blendFunc blend alphaGen const 0.5 rgbGen vertex }",
            "{ map a.tga blendFunc blend rgbGen identity }",
        ] {
            assert_eq!(flags(body) & STAGE_FLAG_VERTEX_ALPHA, 0, "{body}");
        }
    }

    #[test]
    fn stage_params_encodes_const_gens_lightmap_and_alphafunc() {
        let sh = Material::from_shader(&Shader {
            name: "test/mat".into(),
            stages: vec![Stage {
                bundles: vec![
                    Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![TcMod::Scroll(0.25, 0.0)],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    },
                    Bundle {
                        image: ImageRef::Lightmap,
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    },
                ],
                blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                depth_write: None,
                alpha_func: Some(AlphaFunc::Ge128),
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Const([0.5, 0.25, 0.125]),
                alpha_gen: AlphaGen::Const(0.75),
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 2.0).unwrap();
        // an alpha blend sorts past the shadow mask and shades itself
        assert_eq!(
            p.flags,
            STAGE_FLAG_BUNDLE1_LIGHTMAP | STAGE_FLAG_ALPHAFUNC_GE128 | STAGE_FLAG_SUN_SHADOW
        );
        assert_eq!(p.uv0, bundle_affine(&[TcMod::Scroll(0.25, 0.0)], 2.0));
        assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.tint, [0.5, 0.25, 0.125, 0.75]);
        assert_eq!(p.turb01, [0.0; 4]);
    }

    /// Every stage blended onto `GL_ONE` fogs towards black (0x4d2ad0's
    /// `(state & 0xf0) == 0x20`), whatever its source factor; the rest fog
    /// towards the fog colour.
    #[test]
    fn stage_params_fog_a_stage_blended_onto_one_towards_black() {
        let with = |blend| {
            let sh = Material::from_shader(&Shader {
                name: "test/glow".into(),
                stages: vec![Stage {
                    bundles: vec![Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    }],
                    blend,
                    depth_write: None,
                    alpha_func: None,
                    depth_func: DepthFunc::LessEqual,
                    rgb_gen: RgbGen::Identity,
                    alpha_gen: AlphaGen::Identity,
                }],
                ..Default::default()
            });
            stage_params(&sh, 0, 0.0).unwrap().flags & STAGE_FLAG_FOG_BLACK != 0
        };
        use BlendFactor::*;
        assert!(with(Some((One, One))));
        assert!(with(Some((SrcAlpha, One))));
        assert!(with(Some((DstColor, One))));
        assert!(!with(Some((SrcAlpha, OneMinusSrcAlpha))));
        assert!(!with(Some((DstColor, Zero))));
        assert!(!with(None));
    }

    #[test]
    fn stage_params_vector_bundle_sets_flag_and_carries_basis() {
        let sh = Material::from_shader(&Shader {
            name: "test/vec".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::White,
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Vector([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Vertex,
                alpha_gen: AlphaGen::Vertex,
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 0.0).unwrap();
        assert_eq!(
            p.flags,
            STAGE_FLAG_BUNDLE0_VECTOR
                | STAGE_FLAG_VERTEX_RGB
                | STAGE_FLAG_VERTEX_RGB_HALF
                | STAGE_FLAG_VERTEX_ALPHA
        );
        assert_eq!(p.vec0_s, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(p.vec0_t, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.vec1_s, [0.0; 4]);
        // vertex gens leave tint white; the flags carry the instruction
        assert_eq!(p.tint, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn stage_params_wave_gens_evaluate_at_time() {
        let rgb_wave = Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.5,
            phase: 0.25,
            freq: 0.0,
        };
        let a_wave = Wave {
            form: WaveForm::Sawtooth,
            base: 0.0,
            amp: 1.0,
            phase: 0.25,
            freq: 0.0,
        };
        let sh = Material::from_shader(&Shader {
            name: "test/wave".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Wave(rgb_wave.clone()),
                alpha_gen: AlphaGen::Wave(a_wave.clone()),
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 123.0).unwrap();
        let rv = vcod_common::shader::wave_value(&rgb_wave, 123.0);
        let av = vcod_common::shader::wave_value(&a_wave, 123.0);
        // the rgb wave scales by identityLight in the shader, which clamps
        // after it (0x514c00): 1.5 reaches it whole, to draw 0.75 at an
        // identityLight of 0.5; alpha waves clamp here
        assert!((rv - 1.5).abs() < 1e-3, "{rv}");
        assert_eq!(p.tint[..3], [rv; 3]);
        assert!((p.tint[3] - av).abs() < 1e-6 && (av - 0.25).abs() < 1e-6);
        assert_eq!(p.flags, STAGE_FLAG_TINT_LIGHT);
    }

    /// One block of script text, parsed.
    fn parsed(text: &str) -> Material {
        use vcod_common::shader::{WarnSet, parse_shader, split_blocks};
        let (name, body) = split_blocks(text).into_iter().next().expect("a block");
        Material::from_shader(&parse_shader(&name, &body, &mut WarnSet::new()))
    }

    /// deform.wgsl's kind codes are the ones `stage_params` writes.
    #[test]
    fn deform_wgsl_codes_are_the_renderer_ones() {
        let src = include_str!("deform.wgsl");
        for (name, code) in [
            ("DEFORM_WAVE", DEFORM_WAVE),
            ("DEFORM_FLAP_S", DEFORM_FLAP_S),
            ("DEFORM_FLAP_T", DEFORM_FLAP_T),
            ("DEFORM_NORMAL", DEFORM_NORMAL),
            ("DEFORM_BULGE", DEFORM_BULGE),
            ("DEFORM_MOVE", DEFORM_MOVE),
        ] {
            assert!(
                src.contains(&format!("const {name}: u32 = {code}u;")),
                "{name}"
            );
        }
    }

    /// cod2_model.wgsl's stage flags are the ones `stage_params` writes.
    #[test]
    fn cod2_model_wgsl_flags_are_the_renderer_ones() {
        let src = include_str!("cod2_model.wgsl");
        for (name, flag) in [
            ("F_COD2_PROBE", STAGE_FLAG_COD2_PROBE),
            ("F_COD2_ALPHA", STAGE_FLAG_COD2_ALPHA),
            ("F_COD2_SPEC", STAGE_FLAG_COD2_SPEC),
            ("F_COD2_DETAIL", STAGE_FLAG_COD2_DETAIL),
            ("F_COD2_SCROLL", STAGE_FLAG_COD2_SCROLL),
            ("F_OBJECTIVE", STAGE_FLAG_OBJECTIVE),
        ] {
            assert!(
                src.contains(&format!("const {name}: u32 = {flag}u;")),
                "{name}"
            );
        }
        let flag = format!("const COD2_FLAG: u32 = {DEFORM_COD2_FLAG}u;");
        assert!(include_str!("dynamic_model.wgsl").contains(&flag));
        assert!(src.contains(&format!("const COD4_FLAG: u32 = {DEFORM_COD4_FLAG}u;")));
    }

    /// noise.wgsl spells out the noise tables `deformVertexes normal` reads
    /// on the GPU; they must be vcod_common's, entry for entry.
    #[test]
    fn noise_wgsl_tables_are_the_common_ones() {
        let src = include_str!("noise.wgsl");
        let array = |name: &str| -> Vec<String> {
            let start = src.find(&format!("var<private> {name}:")).expect(name);
            let body = &src[start..];
            let open = body.find(">(").unwrap() + 2;
            let close = body.find(");").unwrap();
            body[open..close]
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let perm: Vec<u8> = array("NOISE_PERM")
            .iter()
            .map(|t| t.trim_end_matches('u').parse().unwrap())
            .collect();
        let table: Vec<f32> = array("NOISE_TABLE")
            .iter()
            .map(|t| t.parse().unwrap())
            .collect();
        assert_eq!(perm, vcod_common::shader::NOISE_PERM);
        assert_eq!(table, vcod_common::shader::NOISE_TABLE);
    }

    /// The feather's depth copy lands before the first model surface, or
    /// at the split when no model comes first.
    #[test]
    fn feather_copy_goes_before_the_models() {
        use SortedItem::*;
        let items = [World(0), World(1), Model(0), World(2), Fx(0)];
        assert_eq!(feather_copy_at(&items, 4), 2);
        assert_eq!(feather_copy_at(&items, 1), 1);
        assert_eq!(feather_copy_at(&[World(0), Fx(0)], 1), 1);
    }

    /// A CoD2 `lprobe_flag*` probe stage carries the flag in the deform
    /// kinds' spare fourth slot, which deform.wgsl's three records never
    /// read; any other probe stage leaves it 0.
    #[test]
    fn stage_params_mark_a_flag_stage() {
        let mut sh = parsed("t\n{\n{\nmap a.tga\nrgbGen lightingDiffuse\n}\n}");
        let mut pr = vcod_common::material::ProbeLit {
            specular: true,
            alpha: true,
            detail: false,
            scroll: false,
            flag: true,
            detail_scale: [1.0, 1.0],
        };
        sh.probe = Some(pr);
        let p = stage_params(&sh, 0, 0.0).unwrap();
        assert_ne!(p.flags & STAGE_FLAG_COD2_PROBE, 0);
        assert_eq!(p.deform_kinds, [0, 0, 0, DEFORM_COD2_FLAG]);
        pr.flag = false;
        sh.probe = Some(pr);
        assert_eq!(stage_params(&sh, 0, 0.0).unwrap().deform_kinds, [0; 4]);
    }

    /// The deform records reach the shader in script order, each in its
    /// slot with retail's deform number (0x5149b0 runs them in order).
    #[test]
    fn stage_params_carry_the_deforms_in_script_order() {
        let sh = parsed(
            "t\n{\ndeformVertexes bulge 3 4 5\ndeformVertexes move 0 0 2 square 1 2 0 0\ndeformVertexes normal .5 2\n{\nmap a.tga\n}\n}",
        );
        assert!(stage_animated(&sh, 0));
        let p = stage_params(&sh, 0, 2.0).unwrap();
        assert_eq!(
            p.deform_kinds,
            [DEFORM_BULGE, DEFORM_MOVE, DEFORM_NORMAL, 0]
        );
        assert_eq!(p.deform[0], [3.0, 4.0, 10.0, 0.0]);
        // square at phase 0 reads +1: 2 * (1 + 2 * 1) along z
        assert_eq!(p.deform[1], [0.0, 0.0, 6.0, 0.0]);
        assert_eq!(p.deform[2], [0.5, 4.0, 0.0, 0.0]);
        // reordered, the slots follow
        let sh = parsed(
            "t\n{\ndeformVertexes normal .5 2\ndeformVertexes wave 64 sin 0 2 0 1\n{\nmap a.tga\n}\n}",
        );
        let p = stage_params(&sh, 0, 2.0).unwrap();
        assert_eq!(p.deform_kinds, [DEFORM_NORMAL, DEFORM_WAVE, 0, 0]);
        // a sprite holds a record slot but draws through its own flag
        let sh = parsed(
            "t\n{\ndeformVertexes autosprite\ndeformVertexes move 0 0 2 square 1 2 0 0\n{\nmap a.tga\n}\n}",
        );
        let p = stage_params(&sh, 0, 2.0).unwrap();
        assert_eq!(p.deform_kinds, [DEFORM_MOVE, 0, 0, 0]);
        let plain = parsed("t\n{\n{\nmap a.tga\n}\n}");
        let p = stage_params(&plain, 0, 2.0).unwrap();
        assert_eq!(p.deform_kinds, [0; 4]);
        assert!(!stage_animated(&plain, 0));
    }

    #[test]
    fn stage_params_noise_glow_flickers_without_identity_light() {
        // UO ba.shader's fire window (pakuo01)
        let sh = parsed(
            "textures/ardennes/windows/firewindow\n{\n{\nmap a.dds\nrgbgen wave noise 0.4 0.1 0 10\nblendfunc blend\n}\n}",
        );
        let tints: Vec<f32> = (0..20)
            .map(|i| {
                let p = stage_params(&sh, 0, i as f32 * 0.05).unwrap();
                assert_eq!(p.flags & STAGE_FLAG_TINT_LIGHT, 0);
                p.tint[0]
            })
            .collect();
        assert!(tints.iter().all(|v| (0.3..=0.5).contains(v)));
        assert!(tints.iter().any(|v| (v - 0.4).abs() > 0.02), "{tints:?}");
    }

    /// A table rgbGen wave clamps below at 0 here and above only after the
    /// shader's identityLight scale; a noise wave, unscaled, clamps here.
    #[test]
    fn stage_params_rgb_wave_clamps_after_identity_light() {
        let low = parsed("t\n{\n{\nmap a.tga\nrgbGen wave sin -1 0.5 0 0\n}\n}");
        assert_eq!(stage_params(&low, 0, 0.0).unwrap().tint[..3], [0.0; 3]);
        let high = parsed("t\n{\n{\nmap a.tga\nrgbGen wave sin 3 0 0 0\n}\n}");
        let p = stage_params(&high, 0, 0.0).unwrap();
        assert_eq!(p.tint[..3], [3.0; 3]);
        assert_ne!(p.flags & STAGE_FLAG_TINT_LIGHT, 0);
        let noise = parsed("t\n{\n{\nmap a.tga\nrgbGen wave noise 3 0 0 1\n}\n}");
        assert_eq!(stage_params(&noise, 0, 0.0).unwrap().tint[..3], [1.0; 3]);
    }

    #[test]
    fn stage_params_identity_gens_stay_white_without_flags() {
        for rgb in [RgbGen::Identity, RgbGen::ExactVertex] {
            let sh = Material::from_shader(&Shader {
                name: "test/id".into(),
                stages: vec![Stage {
                    bundles: vec![Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    }],
                    blend: None,
                    depth_write: None,
                    alpha_func: None,
                    depth_func: DepthFunc::LessEqual,
                    rgb_gen: rgb.clone(),
                    alpha_gen: AlphaGen::Identity,
                }],
                ..Default::default()
            });
            let p = stage_params(&sh, 0, 9.0).unwrap();
            assert_eq!(p.tint, [1.0, 1.0, 1.0, 1.0], "{rgb:?}");
            // ExactVertex routes through the vertex-colour flag like Vertex
            assert_eq!(
                p.flags,
                u32::from(rgb == RgbGen::ExactVertex) * STAGE_FLAG_VERTEX_RGB
            );
        }
    }

    /// identityLighting and constLighting take identityLight in the
    /// shader; const passes through (0x4ffa60's colour switch).
    #[test]
    fn stage_params_identity_light_flags_the_lighting_gens() {
        let cases: &[(RgbGen, [f32; 3], u32)] = &[
            (RgbGen::IdentityLighting, [1.0; 3], STAGE_FLAG_TINT_LIGHT),
            (
                RgbGen::ConstLighting([0.6, 0.65, 0.7]),
                [0.6, 0.65, 0.7],
                STAGE_FLAG_TINT_LIGHT,
            ),
            (RgbGen::Const([0.6, 0.65, 0.7]), [0.6, 0.65, 0.7], 0),
        ];
        for (rgb, want, flags) in cases {
            let sh = Material::from_shader(&Shader {
                name: "test/il".into(),
                stages: vec![Stage {
                    bundles: vec![Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: Clamp::NONE,
                        tcmods: vec![],
                        tc_gen: TcGen::Base,
                        env: TexEnv::Modulate,
                    }],
                    blend: None,
                    depth_write: None,
                    alpha_func: None,
                    depth_func: DepthFunc::LessEqual,
                    rgb_gen: rgb.clone(),
                    alpha_gen: AlphaGen::Identity,
                }],
                ..Default::default()
            });
            let p = stage_params(&sh, 0, 9.0).unwrap();
            assert_eq!(p.tint[..3], want[..], "{rgb:?}");
            assert_eq!(p.flags, *flags, "{rgb:?}");
        }
    }

    /// Every WGSL module parses and validates, so a shader edit fails here
    /// instead of at pipeline creation.
    #[test]
    fn quad_half_extent_is_the_shorter_half_side() {
        // a 2x6 stretch quad, ring order
        let v = [
            [0.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [6.0, 0.0, 2.0],
            [0.0, 0.0, 2.0],
        ];
        assert_eq!(quad_half_extent(&v), 1.0);
    }

    #[test]
    fn hud_images_take_retail_flags_by_registration() {
        use vcod_common::shader::{WarnSet, parse_shader, split_blocks};
        let text = "hudA\n{\nnomipmaps\n{\nmap clamp gfx/a.tga\n}\n}\n\
                    hudB\n{\n{\nmap gfx/b.tga\n}\n}\n";
        let lib: HashMap<String, Material> = split_blocks(text)
            .into_iter()
            .map(|(name, body)| {
                (
                    name.to_lowercase(),
                    Material::from_shader(&parse_shader(&name, &body, &mut WarnSet::new())),
                )
            })
            .collect();
        let mips: HashSet<String> = [hud_image_key("GFX/HUD/hud@death_kar98.tga")].into();
        let image = |n: &str| hud_image(n, lib.get(&n.to_lowercase()), &mips);
        let flags = |mip, picmip, clamp| HudImage { mip, picmip, clamp };
        assert_eq!(image("hudA"), flags(false, true, true));
        // a script's keywords count whatever trap registered it
        assert_eq!(image("hudB"), flags(true, true, false));
        // implicit: RE_RegisterShader's 3, RE_RegisterShaderNoMip's 0x30
        assert_eq!(
            image("gfx/hud/hud@death_kar98.tga"),
            flags(true, true, false)
        );
        assert_eq!(image("ui/assets/x.tga"), flags(false, false, true));
    }

    #[test]
    fn weapon_icons_register_mipmapped() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        // configstring 7's table, not every weapon file
        let weapons = vcod_common::weapon_table::from_configstring(&fs, "kar98k_mp luger_mp");
        let names = hud_mip_names(&weapons);
        assert!(names.contains("gfx/hud/hud@death_kar98"));
        assert!(!names.contains("gfx/hud/hud@death_m1carbine"));
        assert!(names.contains("gfx/2d/net"));
        assert!(hud_mip_names(&[]).contains("gfx/2d/net"));
    }

    #[test]
    fn wgsl_modules_validate() {
        use wgpu::naga;
        for (name, src) in [
            ("shader", WORLD_WGSL),
            ("dynamic_model", crate::entity_material::DYNAMIC_WGSL),
            ("viewmodel", crate::entity_material::VIEWMODEL_WGSL),
            ("fx", FX_WGSL),
            ("sky", SKY_WGSL),
            ("cod2_world", cod2::COD2_WORLD_WGSL),
            ("sky_cube", cod2::SKY_CUBE_WGSL),
            ("cod2_water", cod2_water::COD2_WATER_WGSL),
            ("gamma", include_str!("gamma.wgsl")),
            ("hud", include_str!("hud.wgsl")),
            ("hud_text", include_str!("hud_text.wgsl")),
            ("bloom", include_str!("bloom.wgsl")),
            ("post", include_str!("post.wgsl")),
            ("screen_blend", include_str!("screen_blend.wgsl")),
            ("ssao", include_str!("ssao.wgsl")),
            ("sun_flare", include_str!("sun_flare.wgsl")),
            ("sun_shafts", include_str!("sun_shafts.wgsl")),
            ("soft_particles", include_str!("soft_particles.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
        }
    }

    #[test]
    fn stage_params_single_bundle_leaves_slot_one_neutral() {
        let sh = Material::from_shader(&Shader {
            name: "test/one".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![TcMod::Scale(2.0, 3.0)],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: Some(AlphaFunc::Lt128),
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 1.0).unwrap();
        assert_eq!(p.flags, STAGE_FLAG_ALPHAFUNC_LT128);
        assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.turb01[2..], [0.0, 0.0]);
        assert_eq!(p.vec1_s, [0.0; 4]);
    }

    /// A `clampmap` bundle samples through the clamp sampler; images carry
    /// no sampler of their own.
    #[test]
    fn stage_params_flag_a_clamped_bundle() {
        let mut sh = Material::from_shader(&Shader {
            name: "test/clamp".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: Vec::new(),
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let clamp_flags = |sh: &Material| {
            stage_params(sh, 0, 0.0).unwrap().flags
                & (STAGE_FLAG_CLAMP_S
                    | STAGE_FLAG_CLAMP_T
                    | STAGE_FLAG_CLAMP1_S
                    | STAGE_FLAG_CLAMP1_T)
        };
        assert_eq!(clamp_flags(&sh), 0);
        sh.stages[0].bundles[0].clamp = Clamp::BOTH;
        assert_eq!(clamp_flags(&sh), STAGE_FLAG_CLAMP_S | STAGE_FLAG_CLAMP_T);
        // `clampy` on bundle 0 and `clampx` on bundle 1 each keep their axis
        // and their bundle (0x4ec6e9, 0x4ec70f)
        sh.stages[0].bundles[0].clamp = Clamp { s: false, t: true };
        let mut b1 = sh.stages[0].bundles[0].clone();
        b1.clamp = Clamp { s: true, t: false };
        sh.stages[0].bundles.push(b1);
        assert_eq!(clamp_flags(&sh), STAGE_FLAG_CLAMP_T | STAGE_FLAG_CLAMP1_S);
    }

    #[test]
    fn stage_params_turb_rides_separate_from_affine() {
        let tcmods = vec![
            TcMod::Scroll(0.5, 0.5),
            TcMod::Turb {
                amp: 3.0,
                phase: 0.1,
                freq: 0.5,
            },
        ];
        let sh = Material::from_shader(&Shader {
            name: "test/turb".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: tcmods.clone(),
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 4.0).unwrap();
        assert_eq!(p.uv0, bundle_affine(&tcmods, 4.0));
        assert_eq!(p.turb01, [3.0, 0.1 + 0.5 * 4.0, 0.0, 0.0]);
    }

    #[test]
    fn stage_params_flag_flap_and_sprites() {
        // skins/shipflag_np: flap s, the wave riding the first deform record
        let parse = |body: &str| {
            let (name, body) = vcod_common::shader::split_blocks(body)
                .into_iter()
                .next()
                .expect("one block");
            Material::from_shader(&vcod_common::shader::parse_shader(
                &name,
                &body,
                &mut vcod_common::shader::WarnSet::new(),
            ))
        };
        let flag = parse("f { cull none deformVertexes flap s 32 sin 4 8 0 .75 { map a.tga } }");
        let p = stage_params(&flag, 0, 2.0).expect("one stage");
        assert_eq!(p.deform_kinds, [DEFORM_FLAP_S, 0, 0, 0]);
        assert_eq!(p.deform[0], [4.0, 8.0, 1.5, 1.0 / 32.0]);
        assert!(stage_animated(&flag, 0));
        // a rate of 0 drops the spread (one phase for every vertex)
        let still = parse("f { deformVertexes flap t 32 sin 4 8 .5 0 { map a.tga } }");
        let p = stage_params(&still, 0, 2.0).expect("one stage");
        assert_eq!(p.deform_kinds, [DEFORM_FLAP_T, 0, 0, 0]);
        assert_eq!(p.deform[0], [4.0, 8.0, 0.5, 0.0]);
        // skins/light_sprite: autosprite, and its lightingDiffuse-free stage
        let sprite = parse(
            "s { deformvertexes autosprite { map a.tga blendfunc add rgbGen identityLighting } }",
        );
        let p = stage_params(&sprite, 0, 0.0).expect("one stage");
        assert_eq!(p.flags & STAGE_FLAG_AUTOSPRITE, STAGE_FLAG_AUTOSPRITE);
        assert!(!stage_animated(&sprite, 0));
        let beam = parse("b { deformVertexes autosprite2 { map a.tga } }");
        let p = stage_params(&beam, 0, 0.0).expect("one stage");
        assert_eq!(
            p.flags & (STAGE_FLAG_AUTOSPRITE | STAGE_FLAG_AUTOSPRITE2),
            STAGE_FLAG_AUTOSPRITE2
        );
    }

    #[test]
    fn stage_params_out_of_range_stage_is_none() {
        let sh = Material::from_shader(&Shader::default());
        assert_eq!(stage_params(&sh, 0, 0.0), None);
    }

    #[test]
    fn anim_frame_index_floors_wraps_and_clamps() {
        assert_eq!(anim_frame_index(5.0, 3, 0.0), 0);
        assert_eq!(anim_frame_index(5.0, 3, 0.21), 1);
        assert_eq!(anim_frame_index(5.0, 3, 0.999), 1); // floor(4.995)=4, 4%3
        assert_eq!(anim_frame_index(5.0, 3, 10.0), 2); // 50 % 3
        assert_eq!(anim_frame_index(0.0, 3, 5.0), 0);
        assert_eq!(anim_frame_index(-1.0, 3, 5.0), 0);
        assert_eq!(anim_frame_index(5.0, 0, 5.0), 0);
        assert_eq!(anim_frame_index(5.0, 3, -1.0), 0);
    }

    // ---- StageBatch classification ----

    use vcod_common::shader::{SkyParms, SurfaceBits};

    /// One-bundle shader from blend pair plus top-level knobs.
    fn script(
        blend: Option<(BlendFactor, BlendFactor)>,
        sort: Option<f32>,
        offset: bool,
    ) -> Material {
        Material::from_shader(&Shader {
            name: "test/script".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            sort,
            polygon_offset: offset.then_some(PolygonOffset::Single),
            ..Default::default()
        })
    }

    #[test]
    fn opaque_and_blend_variants_follow_class_and_depth_write() {
        let (v, _, _) = stage_draw_facts(&script(None, None, false), 0).unwrap();
        assert_eq!(v, StageVariant::Opaque);

        let alpha = Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha));
        let (v, _, _) = stage_draw_facts(&script(alpha.clone(), None, false), 0).unwrap();
        assert_eq!(v, StageVariant::Blend);

        // explicit depthWrite keeps writing
        let mut sh = script(alpha, None, false);
        sh.stages[0].depth_write = Some(true);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::BlendDepthWrite
        );

        // polygonOffset biases the stage
        assert!(stage_draw_facts(&script(None, None, true), 0).unwrap().2);
    }

    /// A CoD4 light shaft (`falloff_add`, cod4-world.md 9.5) draws its
    /// premultiplied colour (OneMinusDstColor, One) through
    /// `fs_stage_premul`, its angle falloff and eye offset in the params;
    /// the same pair from a CoD1 script keeps the faded additive.
    #[test]
    fn cod4_falloff_add_screens_its_premultiplied_colour() {
        let mut sh = script(
            Some((BlendFactor::OneMinusDstColor, BlendFactor::One)),
            None,
            false,
        );
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveFaded
        );
        sh.premultiplied = true;
        sh.falloff = Some(vcod_common::material::Falloff {
            begin: [0.5, 0.5, 0.5],
            end: [0.0, 0.0, 0.0],
            scale: 1.5,
            bias: -0.2,
            kind: FalloffKind::Angle,
        });
        sh.eye_offset = Some(4.0);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveInvDst
        );
        let (entry, blend, depth_write) = stage_variant_state(StageVariant::AdditiveInvDst);
        let c = blend.unwrap().color;
        assert_eq!(
            (entry, c.src_factor, c.dst_factor, depth_write),
            (
                "fs_stage_premul",
                wgpu::BlendFactor::OneMinusDst,
                wgpu::BlendFactor::One,
                false
            )
        );
        let p = stage_params(&sh, 0, 0.0).unwrap();
        assert_eq!(p.falloff, [[0.5, 0.5, 0.5, 1.5], [0.0, 0.0, 0.0, -0.2]]);
        assert_eq!(p.eye_off, [0.0, 0.0, 4.0, 1.0]);
    }

    /// A CoD4 doorway lightener (`hdrportal_lighten`, cod4-world.md 9.4)
    /// draws (DstColor, SrcColor) with its falloff in the stage params,
    /// never as the alpha blend that painted mp_crash's doorways white.
    #[test]
    fn cod4_distfalloff_doubles_the_framebuffer() {
        let mut sh = script(
            Some((BlendFactor::DstColor, BlendFactor::SrcColor)),
            None,
            false,
        );
        assert_eq!(stage_draw_facts(&sh, 0).unwrap().0, StageVariant::Blend);
        assert_eq!(stage_params(&sh, 0, 0.0).unwrap().falloff, [[0.0; 4]; 2]);
        sh.falloff = Some(vcod_common::material::Falloff {
            begin: [0.8, 0.8, 0.8],
            end: [0.5, 0.5, 0.5],
            scale: 0.006,
            bias: -0.18,
            kind: FalloffKind::Distance,
        });
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::Multiply2x
        );
        let (_, blend, depth_write) = stage_variant_state(StageVariant::Multiply2x);
        let c = blend.unwrap().color;
        assert_eq!(
            (c.src_factor, c.dst_factor, depth_write),
            (wgpu::BlendFactor::Dst, wgpu::BlendFactor::Src, false)
        );
        assert_eq!(
            stage_params(&sh, 0, 0.0).unwrap().falloff,
            [[0.8, 0.8, 0.8, 0.006], [0.5, 0.5, 0.5, -0.18]]
        );
    }

    /// Additive depthWrite must reach the pipeline (Task-6 carry-over): the
    /// variants exist instead of silently drawing without depth writes.
    #[test]
    fn additive_depth_write_selects_a_depth_writing_variant() {
        let one_one = Some((BlendFactor::One, BlendFactor::One));
        let src_one = Some((BlendFactor::SrcAlpha, BlendFactor::One));

        let mut sh = script(one_one, None, false);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveFixed
        );
        sh.stages[0].depth_write = Some(true);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveFixedDepthWrite
        );

        let mut sh = script(src_one, None, false);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveFaded
        );
        sh.stages[0].depth_write = Some(true);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::AdditiveFadedDepthWrite
        );
    }

    #[test]
    fn animated_params_track_tcmods_and_wave_gens() {
        let mut sh = script(None, None, false);
        assert!(!stage_animated(&sh, 0));
        sh.stages[0].bundles[0].tcmods.push(TcMod::Scroll(0.1, 0.0));
        assert!(stage_animated(&sh, 0));

        let mut sh = script(None, None, false);
        sh.stages[0].rgb_gen = RgbGen::Wave(Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.0,
            phase: 0.0,
            freq: 1.0,
        });
        assert!(stage_animated(&sh, 0));

        // scale/transform are static affines; no rewrite needed
        let mut sh = script(None, None, false);
        sh.stages[0].bundles[0].tcmods.push(TcMod::Scale(2.0, 2.0));
        assert!(!stage_animated(&sh, 0));
    }

    /// Sky materials classify through their block's surface bits so the sky
    /// pass can pick them up.
    #[test]
    fn surface_bits_carry_sky_and_nodraw_classification() {
        let mut bits = SurfaceBits::default();
        assert!(!bits.sky && !bits.nodraw);
        bits.sky = true;
        let sh = Material::from_shader(&Shader {
            name: "test/sky".into(),
            stages: vec![],
            sky: Some(SkyParms {
                env: "textures/skies/x".into(),
                cloud_height: 512.0,
            }),
            surface: bits.clone(),
            ..Default::default()
        });
        // either signal routes to Sky at load time
        assert!(sh.surface.sky || sh.sky.is_some());
        assert!(mesh::material_kind("textures/skies/x", true, false, true) == MaterialKind::Sky);
    }

    #[test]
    fn one_list_merges_world_marks_models_and_fx_by_sort_in_that_order_on_ties() {
        use SortedItem::{Fx, Mark, Model, World};
        use vcod_common::shader::SORT_DECAL;
        let world = [SORT_OPAQUE, SORT_DECAL, SORT_BLEND, SORT_ADDITIVE];
        // marks sort decal (polygonOffset2): after the opaques, before blends
        let marks = [SORT_DECAL];
        let models = [SORT_OPAQUE, SORT_BLEND];
        // an opaque effect material draws among the opaques, not after
        // every world blend
        let fx = [SORT_OPAQUE, SORT_BLEND, SORT_ADDITIVE, 22.0];
        assert_eq!(
            merge_by_sort(&world, &marks, &models, &fx),
            [
                World(0),
                Model(0),
                Fx(0),
                World(1),
                Mark(0),
                World(2),
                Model(1),
                Fx(1),
                World(3),
                Fx(2),
                Fx(3)
            ]
        );
        assert_eq!(merge_by_sort(&[], &[], &[], &[SORT_BLEND]), [Fx(0)]);
        assert_eq!(merge_by_sort(&[SORT_BLEND], &[], &[], &[]), [World(0)]);
    }

    #[test]
    fn shaders_of_one_sort_order_by_their_first_stage_images() {
        let e = |sort, images| SortEntry { sort, images };
        let places = sorted_places(&[
            // registered first, but blend sorts after every opaque
            e(SORT_BLEND, Some([1, 0])),
            e(SORT_OPAQUE, Some([9, 3])),
            e(SORT_OPAQUE, Some([4, 3])),
            // the same texture on a lower lightmap goes before it
            e(SORT_OPAQUE, Some([4, 2])),
            // equal images: after the earlier one
            e(SORT_OPAQUE, Some([4, 3])),
            e(vcod_common::shader::SORT_OCEAN, Some([20, 0])),
        ]);
        assert_eq!(places, [5, 4, 2, 1, 3, 0]);
        // a shader without bundle 1 does not hold back a newcomer with the
        // same bundle 0, nor does a stageless one of the same sort
        let places = sorted_places(&[
            e(SORT_OPAQUE, Some([4, 0])),
            e(SORT_OPAQUE, None),
            e(SORT_OPAQUE, Some([4, 7])),
        ]);
        assert_eq!(places, [2, 1, 0]);
    }

    #[test]
    fn image_numbers_follow_first_load_and_ignore_case_and_extension() {
        let mut n = ImageNumbers::default();
        assert_eq!(n.number("$white"), 1);
        assert_eq!(n.number("textures/a/Wall.tga"), 2);
        assert_eq!(n.number("textures/a/wall"), 2);
        assert_eq!(n.number("textures/a/floor.jpg"), 3);
    }

    /// The stock world types (pak4 `shadertypes/world/`): wood's gloss
    /// alpha is read by nothing, foliage and the masked types cut, a decal
    /// blends.
    #[test]
    fn a_world_type_draws_opaque_without_an_alpha_test_or_a_first_blend() {
        let wood = "{\n\tsurfaceparm wood\n\t{\n\t\tmap $texturename\n\t\trgbGen exactVertex\n\t}\n\t{\n\t\tmap $lightmap\n\t\tblendFunc filter\n\t}\n}\n";
        let foliage = "{\n\tsurfaceparm foliage\n\tsurfaceparm nomarks\n\t//cull disable\n\t{\n\t\tmap $texturename\n\t\trgbGen exactVertex\n\t\talphaFunc GE128\n\t\tdepthWrite\n\tnextbundle\n\t\tmap $lightmap\n\t}\n}\n";
        let decal = "{\n\tsurfaceparm nonsolid\n\tpolygonOffset\n\t{\n\t\tmap clamp $texturename\n\t\trgbGen exactVertex\n\t\tblendFunc blend\n\tnextbundle\n\t\tmap $lightmap\n\t}\n}\n";
        assert!(stype_draws_opaque(wood));
        assert!(!stype_draws_opaque(foliage));
        assert!(!stype_draws_opaque(decal));
        assert!(!stype_draws_opaque(""));
    }

    /// mp_chateau's glazed doors: the image's alpha masks the panes, and the
    /// wood type draws it opaque, so nothing of the door may cut.
    #[test]
    fn chateaus_door_draws_opaque() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let name = "textures/austria/doors/wood@chateau_exteriordoor";
        let mut img = assets::load_material_image(&fs, &assets::load_shaders(&fs), name);
        assert!(assets::has_cutout_alpha(&img));
        assert!(world_type_draws_opaque(&fs, name));
        assets::force_opaque(&mut img);
        assert!(!assets::has_cutout_alpha(&img));
        assert!(!world_type_draws_opaque(
            &fs,
            "textures/austria/background/foliage_clamp@truck_single"
        ));
    }

    /// A prop skin draws one-sided unless its template says `cull none`,
    /// as UO's `*_cullnone` types do.
    #[test]
    fn a_prop_skin_is_two_sided_only_by_its_template() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        assert!(!prop_skin_two_sided(&fs, "metal_masked@fordsteer.dds"));
        assert!(!prop_skin_two_sided(&fs, "kwagon.tga"));
        let cullnone = fs.list_prefix("skins/foliage_masked_cullnone@");
        if let Some(skin) = cullnone.first() {
            assert!(prop_skin_two_sided(&fs, &skin["skins/".len()..]), "{skin}");
        }
    }

    /// mp_chateau's staff car: `metal_env@staffcar.tga` keeps its
    /// reflectivity in alpha, which `fs_prop`'s cut read as a mask and so
    /// dropped the body. Its template blends the skin over the
    /// environment map, so it goes through the stages with the skin bound.
    #[test]
    fn a_prop_skin_draws_as_its_template_says() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let skin = "metal_env@staffcar.tga";
        assert!(assets::has_cutout_alpha(&assets::load_skin_image(
            &fs, skin
        )));
        let PropSkinDraw::Stages(sh) = prop_skin_draw(&fs, skin) else {
            panic!("metal_env takes the stage path");
        };
        assert_eq!(sh.stages.len(), 2);
        assert!(matches!(
            &sh.stages[1].bundles[0].image,
            ImageRef::Path(p) if p == "skins/metal_env@staffcar.tga"
        ));
        assert!(matches!(
            prop_skin_draw(&fs, "glass_blend_in@mercedeswindow.dds"),
            PropSkinDraw::Stages(_)
        ));
        assert!(matches!(
            prop_skin_draw(&fs, "metal_masked@fordsteer.dds"),
            PropSkinDraw::Cut
        ));
        assert!(matches!(
            prop_skin_draw(&fs, "metal@staffcartread.dds"),
            PropSkinDraw::Opaque
        ));
        assert!(matches!(
            prop_skin_draw(&fs, "kwagon.tga"),
            PropSkinDraw::Opaque
        ));
        // a typed skin without its template draws retail's default shader
        assert!(matches!(
            prop_skin_draw(&fs, "no_such_type@staffcar.tga"),
            PropSkinDraw::Stages(sh) if sh.name == "<default>"
        ));
    }

    #[test]
    fn world_draws_follow_the_sorted_list_with_a_shaders_stages_together() {
        let call = |pass| DrawCall {
            bind_group: 0,
            pass,
        };
        // batch 0: implicit opaque; 1: an opaque base with an additive glow
        // stage; 2: an alphaFunc cutout scripted `sort blend`; 3: a plain
        // opaque scripted shader whose texture loaded before batch 0's
        let calls = [
            call(Pass::Opaque),
            call(Pass::Stage),
            call(Pass::Stage),
            call(Pass::Stage),
        ];
        let stages = vec![vec![], vec![0, 1], vec![2], vec![3]];
        let keys = [
            (SORT_OPAQUE, 1),
            (SORT_OPAQUE, 2),
            (SORT_BLEND, 3),
            (SORT_OPAQUE, 0),
        ];
        let draws: Vec<DrawRange> = (0..4)
            .map(|batch| DrawRange {
                batch,
                first: 0,
                count: 3,
            })
            .collect();
        let mut out = Vec::new();
        order_draws(&keys, &calls, &stages, &draws, &mut out);
        let got: Vec<_> = out
            .iter()
            .map(|fd| match fd.kind {
                DrawRef::Legacy(_) => (fd.src, None, fd.sort),
                DrawRef::Stage(sb) => (fd.src, Some(sb), fd.sort),
            })
            .collect();
        assert_eq!(
            got,
            [
                (3, Some(3), SORT_OPAQUE),
                (0, None, SORT_OPAQUE),
                // the glow stage right after its base, at the shader's sort
                (1, Some(0), SORT_OPAQUE),
                (1, Some(1), SORT_OPAQUE),
                // the cutout at its own sort, after an entity of sort 5
                (2, Some(2), SORT_BLEND),
            ]
        );
        let world: Vec<f32> = out.iter().map(|fd| fd.sort).collect();
        let items = merge_by_sort(&world, &[], &[SORT_OPAQUE, SORT_BLEND], &[]);
        use SortedItem::{Model, World};
        assert_eq!(
            items,
            [
                World(0),
                World(1),
                World(2),
                World(3),
                Model(0),
                World(4),
                Model(1)
            ]
        );
    }

    #[test]
    fn entity_draws_sort_by_material_sort_then_load_order_then_entity() {
        let skin = |sort, order| EntSkin {
            stages: Vec::new(),
            lit: true,
            sort,
            order,
            cookie_caster: false,
            sun_caster: false,
            sun_cut: None,
        };
        let (blend, opaque_late, opaque_early) = (skin(15.0, 0), skin(5.0, 2), skin(5.0, 1));
        let surfs = [
            (0, 0, &blend),
            (0, 1, &opaque_late),
            (1, 0, &opaque_early),
            (2, 0, &opaque_late),
        ];
        let got: Vec<_> = sorted_ent_draws(surfs.into_iter(), std::iter::empty())
            .iter()
            .map(|d| (d.inst, d.surf))
            .collect();
        assert_eq!(got, [(1, 0), (0, 1), (2, 0), (0, 0)]);
    }

    #[test]
    fn the_gun_interleaves_with_the_world_and_entities_by_material() {
        let skin = |sort, order| EntSkin {
            stages: Vec::new(),
            lit: true,
            sort,
            order,
            cookie_caster: false,
            sun_caster: false,
            sun_cut: None,
        };
        let (flesh, rifle, glass, smoke) = (
            skin(SORT_OPAQUE, 0),
            skin(SORT_OPAQUE, 1),
            skin(SORT_BLEND, 2),
            skin(16.0, 3),
        );
        // a player in the rifle's material and a blend2 smoke card
        let ents = [(0, 0, &flesh), (0, 1, &rifle), (1, 0, &smoke)];
        // the gun: hands, the same rifle material, a blended scope lens
        let gun = [(0, 0, &glass), (0, 1, &rifle), (1, 0, &flesh)];
        let draws = sorted_ent_draws(ents.into_iter(), gun.into_iter());
        let got: Vec<_> = draws.iter().map(|d| (d.gun, d.inst, d.surf)).collect();
        assert_eq!(
            got,
            [
                // one material: the entities, then the gun (a later number)
                (false, 0, 0),
                (true, 1, 0),
                (false, 0, 1),
                (true, 0, 1),
                (true, 0, 0),
                (false, 1, 0),
            ]
        );

        // World ocean and opaque, then the models of sort 5, then an
        // opaque-class see-through cutout; the dlights after the last opaque
        // world draw, ahead of the models. The gun's lens (blend) draws
        // before a world blend2 and the smoke (blend2), not after every
        // world translucent.
        use SortedItem::{Model, World};
        let world = [3.0, SORT_OPAQUE, SORT_OPAQUE, 7.0, SORT_BLEND, 16.0];
        let sorts: Vec<f32> = draws.iter().map(|d| d.sort).collect();
        let items = merge_by_sort(&world, &[], &sorts, &[]);
        assert_eq!(
            items,
            [
                World(0),
                World(1),
                World(2),
                Model(0),
                Model(1),
                Model(2),
                Model(3),
                World(3),
                World(4),
                Model(4),
                World(5),
                Model(5)
            ]
        );
        assert_eq!(dlight_slot(&items, &world), 3);
        assert_eq!(dlight_slot(&[], &[]), 0);
    }

    /// The SSAO split follows the last draw of any list sorting up to
    /// decal, past an opaque effect, a mark run and a decal model, before every blend.
    #[test]
    fn ssao_splits_after_the_last_opaque_or_decal_draw() {
        let world = [SORT_OPAQUE, SORT_DECAL, SORT_BLEND];
        let models = [SORT_OPAQUE, SORT_DECAL, SORT_ADDITIVE];
        let fx = [SORT_OPAQUE, SORT_BLEND];
        let marks = [SORT_DECAL];
        let items = merge_by_sort(&world, &marks, &models, &fx);
        // world 0, model 0, fx 0, world 1, mark 0, model 1 | world 2, fx 1, model 2
        assert_eq!(ssao_split(&items, &world, &marks, &models, &fx), 6);
        assert_eq!(ssao_split(&[], &[], &[], &[], &[]), 0);
        let blends = [SORT_BLEND];
        let only = merge_by_sort(&blends, &[], &[], &[]);
        assert_eq!(ssao_split(&only, &blends, &[], &[], &[]), 0);
    }
}
