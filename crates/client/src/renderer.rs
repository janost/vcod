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
    SORT_DECAL, SORT_OPAQUE, ShaderClock, Sprite, Stage, TcGen, TcMod, TexEnv, TexSemantic, Wave,
    WaveForm, bundle_affine, bundle_turb, has_animated_tcmods, rgb_wave_value, wave_value,
};
use vcod_common::mesh::{self, Batch, IndexRange, MaterialKind};
use vcod_common::model::{self, Vertex as VmVert};
use vcod_common::net::FogParams;
use vcod_common::pk3::Pk3Fs;
use vcod_common::props;
use vcod_common::shader::{Shader, ShaderLib};
use vcod_common::static_light::{SceneLight, StaticLighting};
use vcod_common::vis::{Frustum, Visible, WorldVis};

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
mod bundles;
mod draw_list;
mod dynamic;
mod fog;
mod frame;
#[cfg(test)]
mod fx_feather;
mod fx_pass;
pub(crate) mod gpu_util;
pub(crate) mod headless;
mod hud_pass;
#[cfg(test)]
mod hud_shot;
mod mark_pass;
#[cfg(test)]
mod material_keys;
#[cfg(test)]
pub(crate) mod pipeline_check;
#[cfg(test)]
mod prop_blends;
mod prop_skin;
#[cfg(test)]
mod screenshot_live;
mod settings;
mod shadow_encode;
mod sky_pass;
mod stage;
mod upload;
mod world_encode;
mod world_load;
use bundles::*;
use draw_list::*;
pub(crate) use dynamic::*;
use fog::*;
use frame::*;
use fx_pass::*;
use hud_pass::*;
use prop_skin::*;
pub(crate) use settings::*;
use shadow_encode::*;
use sky_pass::*;
pub(crate) use stage::*;
pub(crate) use upload::*;
use world_encode::*;
use world_load::*;

// Ditto the viewmodel vertex stride.
const _: () = assert!(std::mem::size_of::<VmVert>() == 80);

pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// `r_aaSamples` to a count wgpu takes on every adapter: 1 stays 1, any
/// other value is 4.
pub fn aa_samples(cvar: f32) -> u32 {
    if cvar <= 1.0 { 1 } else { 4 }
}

/// A shader's multisampled depth binding, single-sampled when the scene is:
/// `textureLoad(t, p, 0)` reads sample 0 or mip 0 alike.
pub(crate) fn depth_wgsl(src: &str, samples: u32) -> std::borrow::Cow<'_, str> {
    if samples > 1 {
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

/// One GPU write of a frame (cod4-client-perf.md 24): a pass with the
/// bytes its attachments store (each attachment once, whatever the
/// overdraw), a copy or a buffer upload.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct FrameWrite {
    pub what: String,
    pub bytes: u64,
}

/// The shaders' seconds (`time_pad.x`, the view weapon's and the post
/// pass's): `time` wrapped at 3840 s, a whole number of every cycle those
/// shaders run off it (the flag and flap waves' 10 s, the scroll and
/// objective pulse's 1 s, the grain seed's 128/3 s); `time as f32` under it.
pub(crate) fn gpu_time(time: f64) -> f32 {
    ShaderClock(time).wrapped(3840.0)
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
    /// Seconds since start (f64): the stage terms read it through
    /// [`ShaderClock`], the shaders' own clock as [`gpu_time`].
    pub time: f64,
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

/// What a CoD4 vision set draws over the level (cod4-world.md 11).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelLook {
    pub film: Option<crate::post::Film>,
    pub glow: Option<crate::cod2_glow::Cod4Glow>,
}

#[cfg(test)]
mod fog_state_tests {
    use super::*;

    fn fog(far: f32, fade_ms: u32) -> FogParams {
        FogParams {
            near: 0.0,
            far,
            density: 1.0,
            color: [0.5; 3],
            fade_ms,
            start: 0.0,
        }
    }

    /// CoD1's frame asks for the same fog every frame: the fade still runs
    /// to its end, and the repeat starts no new one.
    #[test]
    fn a_fog_asked_for_every_frame_fades_to_its_end() {
        let mut f = FogState::default();
        f.set_target(fog(1000.0, 0), 0.0);
        f.advance(0.0);
        let to = fog(2000.0, 500);
        for i in 1..=40 {
            let t = i as f32 * 0.016;
            f.set_target(to, t);
            f.advance(t);
        }
        assert_eq!(f.live.far, 2000.0);
        assert!(f.target.is_none());
        f.set_target(to, 1.0);
        f.advance(1.0);
        assert!(f.target.is_none());
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

/// Where a frame lands: the window's swapchain, or a texture for a
/// headless renderer (tests, probes).
enum Target {
    Surface(wgpu::Surface<'static>),
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
    swap_direct: bool,
    /// `SCENE_FORMAT`, or `HDR_FORMAT` with `vc_hdr`: what every world,
    /// entity, fx and sky pipeline draws into. Fixed for the renderer's life.
    world_format: wgpu::TextureFormat,
    /// The scene's sample count, `r_aaSamples` through [`aa_samples`] (1 or
    /// 4): every scene target and pipeline takes it. Fixed for the
    /// renderer's life.
    samples: u32,
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
    /// Model skins and their stage images decoded for the upload batch
    /// running (prefetched on CoD2 and CoD4), or the single uploads since
    /// the last one: emptied at each batch's end and with the map, as the
    /// GPU holds what was taken.
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
    world_fx_keep: cod4_worldfx::WorldFxKeep,
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
    prepass_drawn: std::cell::Cell<usize>,
    /// Whether the last frame's prepass wrote `floatZ`.
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
    shaders: Arc<ShaderLib>,
    /// Parsed shader scripts; map materials are classified against it.
    shader_lib: MaterialLib,
    /// The map's active sky (farbox + optional cloud dome); `None` without a
    /// matching sky block.
    sky: Option<SkyRender>,
    /// Global fog from configstring 12; the F3 `fog:` line.
    fog: FogState,
    hud_quad_cap_warned: bool,
    /// The fx quad and stage-draw caps warn once, not every frame.
    fx_cap_warned: bool,
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
        let _ = crate::frontend::GPU_INFO.set(crate::frontend::GpuInfo {
            vendor: info.driver.clone(),
            version: info.driver_info.clone(),
            renderer: info.name.clone(),
            color_bits: config.format.block_copy_size(None).unwrap_or(4) * 8,
            depth_bits: DEPTH_FORMAT.block_copy_size(None).unwrap_or(4) * 8,
            stencil_bits: 0,
            extensions: adapter
                .features()
                .iter_names()
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
                .join(" "),
        });
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
    #[expect(clippy::too_many_arguments)]
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
        let world_format = if hdr { HDR_FORMAT } else { SCENE_FORMAT };
        let (width, height, format) = (config.width, config.height, config.format);
        let msaa_view = create_msaa_view(&device, world_format, width, height, samples);
        let depth_view = create_depth_view(&device, width, height, samples);
        let ssao = crate::ssao::SsaoPass::new(
            &device,
            &queue,
            &depth_view,
            world_format,
            width,
            height,
            timestamps,
            samples,
        );

        let [r0, r1, r2, r3, r4, r5, r6] = shadows::receive_layout_entries();
        let [s0, s1, s2, s3, s4, s5, s6, s7] = TexSamplers::layout_entries();
        let [m0, m1, m2] = crate::cod4_sunshadow::layout_entries();
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[
                gpu_util::uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                // Fx lights. The fx pass shares this layout without declaring
                // binding 1 in fx.wgsl, which wgpu allows. Props light
                // per vertex from them.
                gpu_util::uniform(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
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
                gpu_util::texture_2d(DYNAMIC_SHADOW_BINDING),
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

        let shaders = ShaderLib::load(fs);

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
                (world_format, samples),
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
        // offset (`depth_bias`); their alpha drives MSAA coverage.
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
        // StageBatch from its ClassedStage.
        // The stage table and the dlight pass compile on their own threads
        // while this one builds the entity pass (`par_build`).
        let (stage_pipelines, dlight, vm_pass, dynamic) = std::thread::scope(|s| {
            let stages = s.spawn(|| {
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
                dlight::DlightPass::new(
                    &device,
                    &queue,
                    &shader,
                    [&camera_layout, &material_layout, &stage_layout],
                    &lightmap_sampler,
                    bias,
                    (world_format, samples),
                )
            });
            let vm_pass = create_vm_pass(&device, &tex_samplers);
            let dynamic = create_dynamic_pass(
                &device,
                &queue,
                (world_format, samples),
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
        let soft =
            crate::soft_particles::SoftParticles::new(&device, &depth_view, width, height, samples);
        let scene_copy =
            crate::scene_copy::SceneCopy::new(&device, world_format, soft.linz(), width, height);
        let fx = create_fx_pass(
            &device,
            world_format,
            &camera_layout,
            &vm_pass.skin_layout,
            &soft.fx_layout,
            &scene_copy.layout,
            samples,
        );
        let cookies = cod2_cookie::Cookies::new(&device, &dynamic.bone_layout);
        let hud = create_hud_text_pass(&device, &queue, SCENE_FORMAT, samples);
        let hud_pass = create_hud_pass(&device, SCENE_FORMAT, samples);
        let gamma = GammaPass::new(&device, format, width, height);
        let post = crate::post::PostPass::new(&device, world_format, width, height);
        let dof = crate::cod4_dof::DofPass::new(&device, world_format, width, height, samples);
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
            samples,
        );
        let screen_blend =
            crate::screen_blend::ScreenBlend::new(&device, gamma.scene_view(), width, height);

        let lib = material_lib(fs);
        let marks = mark_pass::MarkPass::new(
            &device,
            shader.clone(),
            stage_pipeline_layout.clone(),
            (world_format, samples),
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
            swap_direct: true,
            world_format,
            samples,
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
            world_fx_keep: Default::default(),
            identity_light: 0.5,
            image_gamma: None,
            textures,
            view_aspect: None,
            dynamic_light: 1,
            lod_cvars: (1.0, 0.0),
            cod4_options: Cod4Options::default(),
            depth_prepass: false,
            cookies_on_prepass: true,
            prepass_drawn: Default::default(),
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
            fx_cap_warned: false,
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
}

/// The camera group (`camera_layout`): the Camera block, the fx lights,
/// `vc_shadows`' receiver inputs, `r_textureMode`'s samplers, CoD2's
/// dynamic shadow and CoD4's sun shadow map.
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

fn create_depth_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    samples: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
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
    samples: u32,
) -> wgpu::TextureView {
    // single-sampled the scene draws into its resolve target and never
    // touches this one (`SceneTargets::begin`): keep it a texel, not a
    // second full-screen colour target (cod4-client-perf.md 24)
    let (width, height) = match samples {
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
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests;
