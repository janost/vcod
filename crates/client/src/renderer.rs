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
#[cfg(test)]
mod fx_feather;
mod fx_pass;
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
use bundles::*;
use draw_list::*;
pub(crate) use dynamic::*;
use fog::*;
use fx_pass::*;
use hud_pass::*;
use prop_skin::*;
pub(crate) use settings::*;
use shadow_encode::*;
use sky_pass::*;
pub(crate) use stage::*;
pub(crate) use upload::*;
use world_encode::*;

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

/// A shader's multisampled depth binding, single-sampled when the scene is:
/// `textureLoad(t, p, 0)` reads sample 0 or mip 0 alike.
pub(crate) fn depth_wgsl(src: &str) -> std::borrow::Cow<'_, str> {
    depth_wgsl_at(src, msaa_samples())
}

fn depth_wgsl_at(src: &str, samples: u32) -> std::borrow::Cow<'_, str> {
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

/// What `Renderer::cull` reads off the frame, bit for bit.
type CullKey = ([u32; 3], [u32; 16], [u32; 2]);

fn cull_key(eye: glam::Vec3, view_proj: &glam::Mat4, lod: (f32, f32)) -> CullKey {
    (
        eye.to_array().map(f32::to_bits),
        view_proj.to_cols_array().map(f32::to_bits),
        [lod.0.to_bits(), lod.1.to_bits()],
    )
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

    /// Draws the next map with `lib` in place of the game's materials (a
    /// CoD4 map carries its own).
    pub fn set_material_lib(&mut self, lib: MaterialLib) {
        self.shader_lib = lib;
        self.lib_warn_count = self.shader_lib.warn_count();
        // The mark shaders resolved through the old lib and search path.
        self.marks.reopen();
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
        log::info!(
            "{} batches ({} opaque, {} prop, {} layer, {} overlay), {} draw indices, {} vertices",
            batch_draws.len(),
            count(|p| matches!(p, Pass::Opaque | Pass::Solid)),
            count(|p| matches!(p, Pass::Prop { .. })),
            count(|p| p == Pass::Layer),
            count(|p| p == Pass::Overlay),
            indices.len(),
            vertices.len()
        );
        log::info!(
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
        // slotted after them (shader doc 8.2).
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
        log::info!(
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
        log::info!(
            "dlight pass: {} stage draws over {} batches",
            dlight_batches.len(),
            dlights_of_batch.iter().filter(|d| !d.is_empty()).count()
        );
        self.shader_dropped = dropped_stages;
        log::info!(
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
            // no box when outerbox[0] fails to load (`SkyBox`)
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
        // A material over several lightmaps or lights pushed its slot once
        // per batch; the frame writes each slot once, with the last push's
        // params as the repeated writes left it.
        let mut written = HashSet::new();
        animated.reverse();
        animated.retain(|(_, _, slot)| written.insert(*slot));
        animated.reverse();
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
        // The recorded runs hold the old map's buffers and bind groups.
        self.world_bundles.runs.clear();
        self.prepass_bundles.runs.clear();
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
                        let scene = model_scene_lights(
                            &self.dynamic.scene_lights,
                            &self.dynamic.entity_scene_lights,
                            self.dynamic_light,
                        );
                        let picked = l.entity_lights(draw.light_origin, &scene);
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
                // A frame with no surface still submits, so the writes it
                // queued above go now and do not pile up until one draws.
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    self.queue.submit([]);
                    return;
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.queue.submit([]);
                    self.reconfigure();
                    return;
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    log::warn!("dropped frame: surface validation error");
                    self.queue.submit([]);
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
            // Linear fog clears to the fog colour (docs/protocol-1.1.md,
            // configstring 12);
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
            // Shafts need a sun flare's direction (shader doc 8.7).
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
                            let n = ent_run(
                                &items,
                                j,
                                &ent_draws,
                                (&vm.surfaces, &model.skins),
                                &run_stops,
                            );
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
                        let n =
                            ent_run(&items, k, &ent_draws, (&vm.surfaces, &model.skins), &stops);
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
            // A CoD4 shellshock's capture and blends run single-sampled
            // with no depth, as the CoD1 and CoD2 one does.
            let cod4_screen = matches!(
                cod4_shock,
                Some(
                    crate::play::cod4_shock::Screen::Save
                        | crate::play::cod4_shock::Screen::Blur { .. }
                        | crate::play::cod4_shock::Screen::Flash { .. }
                )
            );
            let after = bloom
                || shafts
                || post
                || shock.is_some()
                || cod4_screen
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

/// Whether an image as uploaded, `img` after `texture_settings::prepare`
/// and the levels it returned, has a texel under alpha 128 at any level:
/// only then can a 0.5 alpha test drop a filtered sample.
fn uploads_cutout(img: &Image, levels: &[Vec<u8>]) -> bool {
    assets::has_cutout_alpha_any_level(img)
        || levels
            .iter()
            .any(|l| l.iter().skip(3).step_by(4).any(|&a| a < 128))
}

/// Whether a script-less `<type>@<image>` world material's
/// `shadertypes/world/<type>.stype` draws opaque: no stage alpha-tests and
/// the first one does not blend. Its image's alpha is then read by nothing
/// (wood@chateau_exteriordoor's is a gloss mask; docs/research/
/// cod11-shader-scripts.md, "World shader types").
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

    /// The view weapon's light pick reads the same list as every other
    /// model's: the cgame's and the entities' lights, capped, and none
    /// under `r_dynamiclight 0` (BUG-123).
    #[test]
    fn the_model_light_pick_takes_both_lists_under_the_cvar() {
        let light = |x: f32| SceneLight {
            origin: glam::Vec3::new(x, 0.0, 0.0),
            color: glam::Vec3::ONE,
            intensity: 1.0,
        };
        let scene = vec![light(1.0)];
        let ents = vec![light(2.0), light(3.0)];
        assert!(model_scene_lights(&scene, &ents, 0).is_empty());
        for mode in [1, 2] {
            let got = model_scene_lights(&scene, &ents, mode);
            assert_eq!(
                got.iter().map(|l| l.origin.x).collect::<Vec<_>>(),
                [1.0, 2.0, 3.0]
            );
        }
        let many = vec![light(0.0); crate::fx::sim::MAX_SCENE_LIGHTS + 5];
        assert_eq!(
            model_scene_lights(&many, &ents, 1).len(),
            crate::fx::sim::MAX_SCENE_LIGHTS
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

    /// `white` and `$whiteimage` are the engine built-in; `black` shares its
    /// texture (pak4 `scripts/hud.shader`: both stage `map $whiteimage`,
    /// `black` differs only by `rgbGen const 0`, which `set_hud_quads`
    /// applies to the quad colour).
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
            ("DEFORM_FORM_SHIFT", DEFORM_FORM_SHIFT),
            (
                "FORM_SQUARE",
                deform_form(&WaveForm::Square) >> DEFORM_FORM_SHIFT,
            ),
            (
                "FORM_TRIANGLE",
                deform_form(&WaveForm::Triangle) >> DEFORM_FORM_SHIFT,
            ),
            (
                "FORM_SAWTOOTH",
                deform_form(&WaveForm::Sawtooth) >> DEFORM_FORM_SHIFT,
            ),
            (
                "FORM_INVERSE_SAWTOOTH",
                deform_form(&WaveForm::InverseSawtooth) >> DEFORM_FORM_SHIFT,
            ),
            (
                "FORM_NOISE",
                deform_form(&WaveForm::Noise) >> DEFORM_FORM_SHIFT,
            ),
        ] {
            assert!(
                src.contains(&format!("const {name}: u32 = {code}u;")),
                "{name}"
            );
        }
        assert_eq!(deform_form(&WaveForm::Sin), 0, "table_entry's fallthrough");
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

    /// Every module the client builds, as its `create_shader_module` site
    /// builds it, parses and validates with no GPU; every `.wgsl` file under
    /// src is part of one of them.
    #[test]
    fn wgsl_modules_validate() {
        use wgpu::naga;
        let modules = [
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
            ("cod4_dof", include_str!("cod4_dof.wgsl")),
            ("cod4_sunshadow", crate::cod4_sunshadow::CAST_WGSL),
            ("cod4_spot_blit", crate::cod4_sunshadow::SPOT_BLIT_WGSL),
            ("cod2_glow", include_str!("cod2_glow.wgsl")),
            ("cod2_cookie", cod2_cookie::COOKIE_WGSL),
            ("fx_cloud", fx_cloud::FX_CLOUD_WGSL),
            ("scene_copy", include_str!("scene_copy.wgsl")),
            ("shadow_cast", include_str!("shadow_cast.wgsl")),
            ("shadow_cut", crate::shadows::SHADOW_CUT_WGSL),
            ("shadow_mask", include_str!("shadow_mask.wgsl")),
        ];
        for (name, src) in modules {
            for (samples, src) in [(4, depth_wgsl_at(src, 4)), (1, depth_wgsl_at(src, 1))] {
                let module = naga::front::wgsl::parse_str(&src)
                    .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::default(),
                )
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name} at {samples}x: {}", e.emit_to_string(&src)));
            }
        }
        let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut files = 0;
        while let Some(dir) = dirs.pop() {
            for e in std::fs::read_dir(&dir).unwrap() {
                let path = e.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|x| x == "wgsl") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        modules.iter().any(|(_, src)| src.contains(&text)),
                        "{} is in no module this test validates",
                        path.display()
                    );
                    files += 1;
                }
            }
        }
        assert!(files > 0, "no .wgsl file found");
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
        // the phase 0 + 2 * 0.75 goes up as its fraction: one table turn
        assert_eq!(p.deform[0], [4.0, 8.0, 0.5, 1.0 / 32.0]);
        // a square wave names its table in the kind's upper bits
        let square = parse("f { deformVertexes wave 100 square 1 2 0.25 3 { map a.tga } }");
        let p = stage_params(&square, 0, 1.0).expect("one stage");
        assert_eq!(p.deform_kinds[0], DEFORM_WAVE | 1 << DEFORM_FORM_SHIFT);
        assert_eq!(p.deform[0], [1.0, 2.0, 0.25, 0.01]);
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

    /// 1.7's order inside one sort: the batch whose soup comes first in
    /// the lit order draws first; a batch no soup reached, and the
    /// entries past the world's, keep their registration order after.
    #[test]
    fn cod4_batches_of_one_sort_draw_in_lit_order() {
        use vcod_common::mesh::IndexRange;
        let e = |sort| SortEntry { sort, images: None };
        let range = |batch| {
            Some(IndexRange {
                batch,
                first: 0,
                count: 3,
            })
        };
        let entries = [
            e(SORT_OPAQUE),
            e(SORT_OPAQUE),
            e(SORT_BLEND),
            e(SORT_OPAQUE),
            e(SORT_OPAQUE),
            e(SORT_OPAQUE),
        ];
        // batch 1's first soup is 2nd in the lit order, batch 0's 5th; a
        // cell's missing range takes no place
        let ranges = [range(0), range(1), None, range(0), range(3), range(2)];
        let order = [4, 1, 0, 7, 9, 3];
        assert_eq!(cod4_places(&entries, &ranges, &order), [1, 0, 5, 2, 3, 4]);
    }

    /// A fade lerps every field from where it stood; a change of mode or a
    /// zero fade snaps; a cleared fog's next target snaps too.
    #[test]
    fn fog_fades_within_a_mode_and_snaps_across_one() {
        let exp = |density: f32, fade_ms| FogParams {
            near: 0.0,
            far: 0.0,
            density,
            color: [0.0; 3],
            fade_ms,
            start: 0.0,
        };
        let linear = |far: f32, fade_ms| FogParams {
            near: 100.0,
            far,
            density: 1.0,
            color: [1.0; 3],
            fade_ms,
            start: 0.0,
        };
        let mut f = FogState::default();
        f.set_target(exp(0.2, 1000), 0.0);
        assert_eq!(f.live.density, 0.2, "the first fog snaps");
        f.set_target(exp(0.6, 1000), 1.0);
        f.advance(1.25);
        assert!((f.live.density - 0.3).abs() < 1e-6, "{}", f.live.density);
        assert!(f.target.is_some());
        // a new target mid-fade starts from what is displayed
        f.set_target(exp(0.0, 1000), 1.25);
        f.advance(1.75);
        assert!((f.live.density - 0.15).abs() < 1e-6, "{}", f.live.density);
        f.set_target(linear(2000.0, 1000), 2.0);
        assert!(f.live.linear && f.live.far == 2000.0, "a mode change snaps");
        assert!(f.target.is_none());
        f.set_target(linear(1000.0, 0), 3.0);
        assert_eq!(f.live.far, 1000.0, "no fade snaps");
        f.clear();
        assert!(!f.set);
        f.set_target(linear(3000.0, 1000), 4.0);
        assert_eq!(f.live.far, 3000.0, "the fog after a clear snaps");
    }

    fn vm_surface(
        first_index: u32,
        index_count: u32,
        base_vertex: i32,
        bind_group: usize,
    ) -> VmSurface {
        VmSurface {
            first_index,
            index_count,
            base_vertex,
            bind_group,
            src: 0,
        }
    }

    fn ent_skin(stages: usize) -> Rc<EntSkin> {
        let st = crate::entity_material::EntStageDraw {
            pipeline: 0,
            image: crate::entity_material::StageImage::Skin,
            slot: 0,
            params: 0,
            prepass: true,
            floatz: false,
            rigid: true,
        };
        Rc::new(EntSkin {
            stages: vec![st; stages],
            lit: true,
            sort: SORT_OPAQUE,
            order: 0,
            cookie_caster: false,
            sun_caster: false,
            sun_cut: None,
        })
    }

    /// An entity surface joins the next items while each is the same
    /// instance's next surface on the same one-stage skin, bind group and
    /// base vertex with its indices following on, and no stop or gun
    /// comes between; each broken condition ends the run there.
    #[test]
    fn an_entity_run_joins_only_contiguous_surfaces_of_one_skin() {
        let ent = |inst, surf, gun| EntDraw {
            inst,
            surf,
            sort: SORT_OPAQUE,
            order: 0,
            gun,
        };
        let skin = ent_skin(1);
        let base = || {
            (
                vec![
                    vm_surface(0, 6, 0, 0),
                    vm_surface(6, 3, 0, 0),
                    vm_surface(9, 12, 0, 0),
                    vm_surface(21, 3, 0, 0),
                ],
                vec![skin.clone(); 4],
                vec![
                    ent(0, 0, false),
                    ent(0, 1, false),
                    ent(0, 2, false),
                    ent(0, 3, false),
                ],
            )
        };
        let items: Vec<SortedItem> = (0..4).map(SortedItem::Model).collect();
        let run = |(surfs, skins, draws): &(Vec<VmSurface>, Vec<Rc<EntSkin>>, Vec<EntDraw>),
                   k,
                   stops: &[Option<usize>]| {
            ent_run(&items, k, draws, (surfs, skins), stops)
        };
        let all = base();
        assert_eq!(run(&all, 0, &[None]), 4);
        assert_eq!(run(&all, 1, &[None]), 3);
        assert_eq!(run(&all, 0, &[Some(2)]), 2, "a stop");
        let mut c = base();
        c.2[2].gun = true;
        assert_eq!(run(&c, 0, &[None]), 2, "the gun");
        let mut c = base();
        c.2[2].inst = 1;
        assert_eq!(run(&c, 0, &[None]), 2, "another instance");
        let mut c = base();
        c.2[2].surf = 3;
        assert_eq!(run(&c, 0, &[None]), 2, "not the next surface");
        let mut c = base();
        c.1[2] = ent_skin(1);
        assert_eq!(run(&c, 0, &[None]), 2, "an equal skin, not the same one");
        let mut c = base();
        c.0[2].bind_group = 1;
        assert_eq!(run(&c, 0, &[None]), 2, "another bind group");
        let mut c = base();
        c.0[2].base_vertex = 100;
        assert_eq!(run(&c, 0, &[None]), 2, "another base vertex");
        let mut c = base();
        c.0[2].first_index = 10;
        assert_eq!(run(&c, 0, &[None]), 2, "a gap in the indices");
        let mut c = base();
        c.1 = vec![ent_skin(2); 4];
        assert_eq!(run(&c, 0, &[None]), 1, "a two-stage skin draws alone");
        let items = [
            SortedItem::Model(0),
            SortedItem::Fx(0),
            SortedItem::Model(1),
        ];
        assert_eq!(
            ent_run(&items, 0, &all.2, (&all.0, &all.1), &[None]),
            1,
            "another kind of item"
        );
    }

    /// `keep` is by the model's surfaces, and an empty one uploads no
    /// surface, so the filter reads each uploaded surface's `src`.
    #[test]
    fn a_hidden_surface_is_picked_by_its_source_index() {
        let surfs: Vec<VmSurface> = [0, 2, 3, 5]
            .into_iter()
            .map(|src| VmSurface {
                src,
                ..vm_surface(0, 3, 0, 0)
            })
            .collect();
        assert_eq!(kept_surfaces(&surfs, &[]), [0, 1, 2, 3]);
        assert_eq!(kept_surfaces(&surfs, &[true, true, false, true]), [0, 2, 3]);
        assert_eq!(kept_surfaces(&surfs, &[false, false, true, false]), [1, 3]);
        assert!(kept_surfaces(&surfs, &[false; 6]).is_empty());
    }

    /// A run whose commands held from the last frame draws through a
    /// bundle made once; a changed run replays and drops its bundle; the
    /// runs past this frame's last go; a new target drops them all.
    #[test]
    fn world_runs_bundle_once_their_commands_hold() {
        let Some((device, queue)) = headless::test_device(headless::Need::Any) else {
            return;
        };
        set_msaa_samples(1);
        let tex = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let colour = tex(SCENE_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = tex(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let buf = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            })
        };
        let (a, b) = (buf(), buf());
        let rec = |v: &wgpu::Buffer| {
            let mut r = Recording::default();
            r.vertices(0, v.slice(..));
            r
        };
        let mut wb = WorldBundles::default();
        let frame = |wb: &mut WorldBundles, runs: &[&wgpu::Buffer]| {
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &colour,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations::default(),
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations::default()),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                wb.begin(Some(SCENE_FORMAT));
                for v in runs {
                    wb.draw(
                        &mut pass,
                        &device,
                        rec(v),
                        crate::frame_prof::Count::Rebundled,
                    );
                }
                wb.end();
            }
            queue.submit([enc.finish()]);
        };
        let bundled = |wb: &WorldBundles| wb.runs.iter().map(|r| r.1.is_some()).collect::<Vec<_>>();
        frame(&mut wb, &[&a, &b]);
        assert_eq!(bundled(&wb), [false, false], "a new run replays");
        frame(&mut wb, &[&a, &b]);
        assert_eq!(bundled(&wb), [true, true], "held runs bundle");
        frame(&mut wb, &[&a, &a]);
        assert_eq!(
            bundled(&wb),
            [true, false],
            "a changed run drops its bundle"
        );
        frame(&mut wb, &[&a]);
        assert_eq!(bundled(&wb), [true], "runs past the last go");
        wb.begin(None);
        assert!(wb.runs.is_empty(), "a new target drops every run");
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
