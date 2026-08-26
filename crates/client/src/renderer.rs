use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

use crate::camera::{Z_FAR, Z_NEAR};
use crate::fx::sim::{FxLight, FxQuad, MAX_LIGHTS};
use crate::hud::HudQuad;
use crate::hud_text::{self, HudVert};
use crate::sky;
use vcod_common::assets::{self, Image, ImageData};
use vcod_common::bsp::{self, Bsp, DrawVert};
use vcod_common::mesh::{self, Batch, IndexRange, MaterialKind};
use vcod_common::pk3::Pk3Fs;
use vcod_common::props;
use vcod_common::shader::{
    bundle_affine, bundle_turb, has_animated_tcmods, wave_value, AlphaFunc, AlphaGen, BlendFactor,
    DrawClass, ImageRef, RgbGen, Shader, ShaderLib, SORT_BLEND0, SORT_DECAL,
};
use vcod_common::vis::{Frustum, Visible, WorldVis};
use vcod_common::xmodel::{self, VmVert};

// The vertex layout below hardcodes the BSP drawvert stride.
const _: () = assert!(std::mem::size_of::<DrawVert>() == 44);
// Ditto the viewmodel vertex stride.
const _: () = assert!(std::mem::size_of::<VmVert>() == 52);

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MSAA_SAMPLES: u32 = 4;

/// The viewmodel has its own projection. CoD draws the weapon at a fixed FOV
/// regardless of the world's, with a much nearer near plane.
const VM_FOV_DEG: f32 = 65.0;
const VM_NEAR: f32 = 1.0;
const VM_FAR: f32 = 500.0;
/// Depth-range fraction the viewmodel is squeezed into, so the world cannot poke through it.
const VM_DEPTH_RANGE: f32 = 0.3;
/// `proj` (64) + `model` (64) + `light_dir` (16), std140-compatible as is.
const VM_UNIFORM_SIZE: u64 = 144;
/// Matches `array<mat4x4<f32>, 64>` in viewmodel.wgsl.
const VM_BONE_COUNT: usize = 64;
const VM_BONE_BUF_SIZE: u64 = (VM_BONE_COUNT * 64) as u64;

/// Debug-HUD glyph budget per frame; lines past it are cut.
const MAX_HUD_GLYPHS: usize = 1024;

/// `fx::sim::MAX_PARTICLES` + `fx::sim::MAX_DECALS`.
const MAX_FX_QUADS: usize = 2048 + 256;

const MAX_HUD_QUADS: usize = 4096;

/// Per-vertex rgba lets particles, decals and tracers share one pipeline.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FxVert {
    pos: [f32; 3],
    uv: [f32; 2],
    rgba: [f32; 4],
}

// The vertex layout below hardcodes this stride.
const _: () = assert!(std::mem::size_of::<FxVert>() == 36);

/// Render passes in draw order. Layers (polygonOffset terrain blends) and
/// overlays (decals, masked cutouts) are coplanar with the world and draw
/// depth-biased on top. `Stage` marks batches expanded into per-stage draws
/// instead; their placement comes from the stage bands.
#[derive(Copy, Clone, PartialEq)]
enum Pass {
    Opaque,
    Prop,
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
    /// Index into `WorldGpu::draws`, the gathered visible range.
    src: u32,
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

struct VmSurface {
    first_index: u32,
    index_count: u32,
    /// Surfaces share one vertex buffer; the parsed u16 indices are uploaded unrebased.
    base_vertex: i32,
    bind_group: usize,
}

struct VmModel {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    bind_groups: Vec<wgpu::BindGroup>,
    surfaces: Vec<VmSurface>,
    bone_buf: wgpu::Buffer,
    bone_bg: wgpu::BindGroup,
}

/// Built up front so the shader is validated at startup even with no viewmodel set.
struct VmPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    skin_layout: wgpu::BindGroupLayout,
    bone_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    models: Vec<VmModel>,
}

/// Each player is 4-7 part instances, so 32 players plus corpses and dropped
/// weapons land at 200-280.
const MAX_DYNAMIC_INSTANCES: usize = 512;
const DYNAMIC_INSTANCE_STRIDE: u64 = 80;
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
}

/// Per-instance vertex data. `bone_base` 0 is the shared identity block.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub transform: [f32; 16],
    pub bone_base: u32,
    pub _pad: [u32; 3],
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
            _pad: [0; 3],
        });
    }
    (raw, mats)
}

/// `bone_sets[i]` belongs to `set_viewmodel` model i. Absent or empty leaves
/// that model's GPU buffer as it was.
pub struct VmDraw {
    pub transform: glam::Mat4,
    pub bone_sets: Vec<Vec<glam::Mat4>>,
}

pub struct Frame {
    pub view_proj: glam::Mat4,
    /// Camera position, the cell the visibility walk starts from.
    pub eye: glam::Vec3,
    /// Seconds since start; drives tcMod and wave animation in the stage shaders.
    pub time: f32,
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
    pub gather_ms: f32,
}

/// Matches `FxLights` in shader.wgsl and dynamic_model.wgsl (std140, vec4 aligned).
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

/// Per-stage draw parameters, one dynamic-offset slot per stage batch. WGSL
/// mirror is `StageParams` in shader.wgsl; byte offsets:
/// uv0 @0 and uv1 @24 as mat3x2 (column-major, matching [`bundle_affine`]),
/// turb01 @48, tint @64, flags @80, pad @84..96, then the two tcGen vector
/// bases padded to vec4: vec0_s @96, vec0_t @112, vec1_s @128, vec1_t @144,
/// and the sky dome's view-origin offset @160.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StageParams {
    pub(crate) uv0: [f32; 6],
    pub(crate) uv1: [f32; 6],
    pub(crate) turb01: [f32; 4],
    pub(crate) tint: [f32; 4],
    pub(crate) flags: u32,
    pub(crate) _pad: [u32; 3],
    pub(crate) vec0_s: [f32; 4],
    pub(crate) vec0_t: [f32; 4],
    pub(crate) vec1_s: [f32; 4],
    pub(crate) vec1_t: [f32; 4],
    pub(crate) eye_off: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<StageParams>() == 176);
const STAGE_PARAMS_SIZE: u64 = std::mem::size_of::<StageParams>() as u64;

const UV_AFFINE_IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Evaluates one stage's draw parameters at time `t`: bundle affines/turb,
/// tint from rgbGen/alphaGen, and the flag bits the WGSL reads. `None` when
/// `idx` is out of range or the stage has no bundles.
fn stage_params(shader: &Shader, idx: usize, t: f32) -> Option<StageParams> {
    let st = shader.stages.get(idx)?;
    let b0 = st.bundles.first()?;
    let b1 = st.bundles.get(1);

    let mut flags = match &st.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => STAGE_FLAG_ALPHAFUNC_GT0,
        Some(AlphaFunc::Lt128) => STAGE_FLAG_ALPHAFUNC_LT128,
        Some(AlphaFunc::Ge128) => STAGE_FLAG_ALPHAFUNC_GE128,
    };
    if b0.image == ImageRef::Lightmap {
        flags |= STAGE_FLAG_BUNDLE0_LIGHTMAP;
    }
    if b1.is_some_and(|b| b.image == ImageRef::Lightmap) {
        flags |= STAGE_FLAG_BUNDLE1_LIGHTMAP;
    }

    let mut rgb = [1.0f32; 3];
    match &st.rgb_gen {
        RgbGen::Vertex | RgbGen::ExactVertex => flags |= STAGE_FLAG_VERTEX_RGB,
        RgbGen::Const(c) | RgbGen::ConstLighting(c) => rgb = *c,
        RgbGen::Wave(w) => {
            let v = wave_value(w, t);
            rgb = [v, v, v];
        }
        RgbGen::Identity | RgbGen::IdentityLighting => {}
    }
    let alpha = match &st.alpha_gen {
        AlphaGen::Vertex => {
            flags |= STAGE_FLAG_VERTEX_ALPHA;
            1.0
        }
        AlphaGen::Const(v) => *v,
        AlphaGen::Wave(w) => wave_value(w, t),
        AlphaGen::Identity => 1.0,
    };

    let mut p = StageParams {
        uv0: bundle_affine(&b0.tcmods, t),
        uv1: b1.map_or(UV_AFFINE_IDENTITY, |b| bundle_affine(&b.tcmods, t)),
        // [amp0, now0, amp1, now1]; the VS adds amp*sin(worldpos/1024 + now)
        turb01: {
            let tb0 = bundle_turb(&b0.tcmods, t);
            let tb1 = b1.map_or([0.0; 4], |b| bundle_turb(&b.tcmods, t));
            [tb0[0], tb0[1], tb1[0], tb1[1]]
        },
        tint: [rgb[0], rgb[1], rgb[2], alpha],
        flags,
        _pad: [0; 3],
        vec0_s: [0.0; 4],
        vec0_t: [0.0; 4],
        vec1_s: [0.0; 4],
        vec1_t: [0.0; 4],
        eye_off: [0.0; 4],
    };
    if let Some(v) = b0.vector.as_ref() {
        flags |= STAGE_FLAG_BUNDLE0_VECTOR;
        p.vec0_s = [v[0], v[1], v[2], 0.0];
        p.vec0_t = [v[3], v[4], v[5], 0.0];
    }
    if let Some(v) = b1.and_then(|b| b.vector.as_ref()) {
        flags |= STAGE_FLAG_BUNDLE1_VECTOR;
        p.vec1_s = [v[0], v[1], v[2], 0.0];
        p.vec1_t = [v[3], v[4], v[5], 0.0];
    }
    p.flags = flags;
    Some(p)
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
    /// Parent legacy batch whose gathered range this draw emits.
    batch: u32,
    /// Dynamic-offset slot of this (material, stage)'s params.
    slot: u32,
    variant: StageVariant,
    two_sided: bool,
    bias: bool,
    band: u8,
    /// group(1): bundle-0 image + lightmap page + samplers.
    mat: AnimFrames,
    /// group(2): params UBO slot + bundle-1 image.
    stage: AnimFrames,
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
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
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
}
const STAGE_VARIANTS: usize = 7;

/// Emission bands for the draw list: opaque (incl. cutouts), the coplanar
/// decal slot, see-through blends, back-to-front blends, additives last.
const BAND_OPAQUE: u8 = 0;
const BAND_DECAL: u8 = 1;
const BAND_SEETHROUGH: u8 = 2;
const BAND_BLEND: u8 = 3;
const BAND_ADDITIVE: u8 = 4;

/// Pipeline variant plus the draw facts a StageBatch needs. `None` when
/// `idx` is out of range.
fn stage_draw_facts(sh: &Shader, idx: usize) -> Option<(StageVariant, bool, bool, u8)> {
    let cs = sh.classify_stage(idx)?;
    let st = &sh.stages[idx];
    let variant = match cs.class {
        DrawClass::Opaque => StageVariant::Opaque,
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
    // class wins over sort except for the coplanar decal slot
    let sort = sh.sort_value();
    let band = if cs.class == DrawClass::Additive {
        BAND_ADDITIVE
    } else if sort == SORT_DECAL || cs.bias {
        BAND_DECAL
    } else if cs.class == DrawClass::Opaque {
        BAND_OPAQUE
    } else if sort < SORT_BLEND0 {
        BAND_SEETHROUGH
    } else {
        BAND_BLEND
    };
    Some((variant, cs.two_sided, cs.bias, band))
}

/// Whether the stage's params change over time and need per-frame rewrites.
fn stage_animated(sh: &Shader, idx: usize) -> bool {
    let Some(st) = sh.stages.get(idx) else {
        return false;
    };
    st.bundles.iter().any(|b| has_animated_tcmods(&b.tcmods))
        || matches!(st.rgb_gen, RgbGen::Wave(_))
        || matches!(st.alpha_gen, AlphaGen::Wave(_))
}

/// Group(1) and group(2) bind groups for one authored stage, shared by the
/// world stage-batch loop and the cloud dome. `lm_view` is the batch's
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
    lm_view: Option<&wgpu::TextureView>,
    diffuse_sampler: &wgpu::Sampler,
    lightmap_sampler: &wgpu::Sampler,
    bundle_views: &mut HashMap<String, wgpu::TextureView>,
    st: &vcod_common::shader::Stage,
) -> (AnimFrames, AnimFrames) {
    let b0 = &st.bundles[0];
    let b1 = st.bundles.get(1);
    // a clamped bundle needs ClampToEdge; the lightmap sampler is
    // already exactly that
    let clamp = b0.clamp || b1.is_some_and(|b| b.clamp);
    let diff_sampler = if clamp {
        lightmap_sampler
    } else {
        diffuse_sampler
    };
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
                    resource: wgpu::BindingResource::TextureView(lm_view.unwrap_or(white_view)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(diff_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(lightmap_sampler),
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

/// World-space fx quads. Two pipelines that differ only in blend state:
/// straight alpha, and `GL_ONE GL_ONE` for [`assets::Shaders::is_additive`]
/// materials. One draw call per contiguous same-shader run.
struct FxPass {
    pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer, // static quad pattern, MAX_FX_QUADS * 6, u32
    sampler: wgpu::Sampler,
    /// shader name -> bind group; None = failed to resolve, warned already.
    textures: HashMap<String, Option<wgpu::BindGroup>>,
    /// (shader, additive, quad count) runs in draw order.
    runs: Vec<(String, bool, usize)>,
}

/// Debug-HUD text over an embedded font atlas; drawn last, ignoring depth.
struct HudTextPass {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// On-screen HUD (chat, killfeed, scoreboard, status). Same lazy texture
/// cache and run pattern as [`FxPass`], straight alpha only, no depth test.
struct HudPass {
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    /// image name -> bind group; `None` = failed to resolve, warned already.
    textures: HashMap<String, Option<wgpu::BindGroup>>,
    /// (texture, quad count) runs in draw order.
    runs: Vec<(String, usize)>,
    /// For the F3 overlay.
    quad_count: usize,
}

/// Live snapshot entities: xmodels uploaded once with bind pose baked in,
/// drawn with per-instance transforms streamed through one instance-step
/// vertex buffer.
struct DynamicPass {
    pipeline: wgpu::RenderPipeline,
    instance_buf: wgpu::Buffer,
    /// Identity block first, then each skinned instance's bone set. Sized
    /// for every instance at the cap.
    bone_buf: wgpu::Buffer,
    bone_bg: wgpu::BindGroup,
    /// Indexed by [`ModelHandle`]; never shrinks.
    models: Vec<VmModel>,
    /// `(model index, packed instance)`; position here is the slot in `instance_buf`.
    instances: Vec<(usize, InstanceRaw)>,
    /// Written to `bone_buf` in [`Renderer::render`].
    bone_mats: Vec<[f32; 16]>,
}

/// Everything built from one map: buffers, material bind groups, batches,
/// visibility. Dropped whole on a map change.
struct WorldGpu {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    bind_groups: Vec<wgpu::BindGroup>,
    /// Merged CPU indices (world then props); `index_buf` holds the gathered visible subset.
    cpu_indices: Vec<u32>,
    soup_ranges: Vec<Option<IndexRange>>,
    prop_ranges: Vec<(u32, IndexRange)>,
    prop_bounds: Vec<(glam::Vec3, glam::Vec3)>,
    /// Static per-batch draw descriptors; `draws` is rebuilt each frame from them.
    batch_draws: Vec<DrawCall>,
    draws: Vec<DrawRange>,
    /// Authored stages expanded from their parent batches.
    stage_batches: Vec<StageBatch>,
    /// Parent batch id -> indices into `stage_batches`.
    stages_of_batch: Vec<Vec<u32>>,
    /// Animated stages: (shader clone, stage idx, params slot); rewritten each frame.
    animated: Vec<(Shader, usize, u32)>,
    /// Cloud-dome stages: like `animated` but every entry also gets the view
    /// origin stamped into its params each frame.
    sky_stages: Vec<(Shader, usize, u32)>,
    /// Per-legacy-batch vertex average, the blend-band sort key.
    centroids: Vec<glam::Vec3>,
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
    /// The frozen set and the prop flags it was computed with, so `Locked`
    /// holds props still too.
    locked: Option<(Visible, Vec<bool>)>,
    /// Last frame's mode, so `Locked` and `Off` skip re-uploading indices
    /// that cannot have changed.
    last_cull: Option<CullMode>,
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
}

/// Builds the farbox for the block's env name. Missing non-rt sides land in
/// `upload_bundle_view`'s checkerboard with a warn (once per path per load).
#[allow(clippy::too_many_arguments)]
fn build_sky_box(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    fs: &Pk3Fs,
    camera_layout: &wgpu::BindGroupLayout,
    bundle_views: &mut HashMap<String, wgpu::TextureView>,
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

    // ClampToEdge per ParseSkyParms' GL_CLAMP; linear filtering + mips like
    // every other colour texture.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sky box sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky side layout"),
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
    let mut side_bgs = Vec::with_capacity(6);
    for suffix in sky::AXIS_SUFFIX {
        let path = format!("{env}_{suffix}");
        let view = upload_bundle_view(device, queue, fs, bundle_views, &path);
        let label = format!("sky {sh_name}_{suffix}");
        side_bgs.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&label),
            layout: &tex_layout,
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

    let module = device.create_shader_module(wgpu::include_wgsl!("sky.wgsl"));
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
            count: MSAA_SAMPLES,
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
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    prop_pipeline: wgpu::RenderPipeline,
    layer_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    /// `[variant][two_sided][bias]`; built at startup so shader.wgsl validates
    /// even mapless.
    stage_pipelines: [[[wgpu::RenderPipeline; 2]; 2]; STAGE_VARIANTS],
    camera_layout: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    camera_bg: wgpu::BindGroup,
    fx_lights_buf: wgpu::Buffer,
    /// Device-stage state `load_world` builds map bind groups from.
    material_layout: wgpu::BindGroupLayout,
    stage_layout: wgpu::BindGroupLayout,
    diffuse_sampler: wgpu::Sampler,
    lightmap_sampler: wgpu::Sampler,
    white_view: wgpu::TextureView,
    world: Option<WorldGpu>,
    vis_counts: VisCounts,
    /// Stage draws emitted last frame; stages dropped at the current map's
    /// load; unique ShaderLib warnings. The F3 `shader:` line.
    shader_dropped: usize,
    lib_warn_count: usize,
    vm_pass: VmPass,
    dynamic: DynamicPass,
    fx: FxPass,
    hud: HudTextPass,
    hud_pass: HudPass,
    /// Kept past map load so later inline submodels resolve `textures/...`
    /// names the same way the world did.
    shaders: assets::Shaders,
    /// Parsed shader scripts; Task 7 classifies map materials against it.
    shader_lib: ShaderLib,
    /// The map's active sky (farbox + optional cloud dome); `None` without a
    /// matching sky block.
    sky: Option<SkyRender>,
    hud_quad_cap_warned: bool,
}

impl Renderer {
    pub fn new(window: Arc<winit::window::Window>, fs: &Pk3Fs) -> Result<Renderer> {
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

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vcod device"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
            // One stage-params UBO must stay reachable through dynamic offsets,
            // past the default 64 KiB uniform binding window.
            required_limits: wgpu::Limits {
                max_uniform_buffer_binding_size: adapter.limits().max_uniform_buffer_binding_size,
                ..Default::default()
            },
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("failed to create the wgpu device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let msaa_view = create_msaa_view(&device, format, width, height);
        let depth_view = create_depth_view(&device, width, height);

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Fx lights. The fx pass shares this layout without declaring
                // binding 1 in fx.wgsl, which wgpu allows.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                sampler_entry(2),
                sampler_entry(3),
            ],
        });
        // Group 2 of the stage pipelines: the per-stage params slot plus
        // bundle 1's image (white is bound when bundle 1 has none).
        let stage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                texture_entry(1),
            ],
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fx_lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx lights uniform"),
            size: std::mem::size_of::<FxLightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fx_lights_buf.as_entire_binding(),
                },
            ],
        });

        let diffuse_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("diffuse sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
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

        let shaders = assets::load_shaders(fs);

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
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
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
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
                    count: MSAA_SAMPLES,
                    mask: !0,
                    alpha_to_coverage_enabled: alpha_to_coverage,
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let bias = wgpu::DepthBiasState {
            constant: -2,
            slope_scale: -1.0,
            clamp: 0.0,
        };
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
        // Props take the opaque alpha-test path: `*_masked` skins are cutouts
        // on real geometry, not coplanar decals, so no overlay depth bias.
        // Unculled like the viewmodel (xmodel winding), and foliage cross
        // planes need both sides anyway. Known gap: shadow-decal props
        // (shadow_tree_*, shadow_crate) are coplanar decals and z-fight here.
        let prop_pipeline = make_pipeline(
            "prop pipeline",
            &pipeline_layout,
            "vs_main",
            Default::default(),
            "fs_main",
            false,
            None,
            true,
            None,
        );
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
        let one_one = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let srcalpha_one = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let variant_states = [
            (StageVariant::Opaque, None, true),
            (
                StageVariant::Blend,
                Some(wgpu::BlendState::ALPHA_BLENDING),
                false,
            ),
            (
                StageVariant::BlendDepthWrite,
                Some(wgpu::BlendState::ALPHA_BLENDING),
                true,
            ),
            (StageVariant::AdditiveFixed, Some(one_one), false),
            (StageVariant::AdditiveFaded, Some(srcalpha_one), false),
            // explicit depthWrite on additive stages
            (StageVariant::AdditiveFixedDepthWrite, Some(one_one), true),
            (
                StageVariant::AdditiveFadedDepthWrite,
                Some(srcalpha_one),
                true,
            ),
        ];
        let stage_pipelines: [[[wgpu::RenderPipeline; 2]; 2]; STAGE_VARIANTS] =
            std::array::from_fn(|v| {
                let (variant, blend, depth_write) = variant_states[v];
                [false, true].map(|two_sided| {
                    [false, true].map(|want_bias| {
                        make_pipeline(
                            &format!("stage {variant:?} twosided={two_sided} bias={want_bias}"),
                            &stage_pipeline_layout,
                            "vs_stage",
                            if want_bias { bias } else { Default::default() },
                            "fs_stage",
                            false,
                            blend,
                            depth_write,
                            if two_sided { None } else { cull_back },
                        )
                    })
                })
            });
        let vm_pass = create_vm_pass(&device, format);
        let dynamic = create_dynamic_pass(&device, format, &camera_layout, &vm_pass.skin_layout);
        let fx = create_fx_pass(&device, format, &camera_layout, &vm_pass.skin_layout);
        let hud = create_hud_text_pass(&device, &queue, format);
        let hud_pass = create_hud_pass(&device, format);

        let lib = ShaderLib::load(fs);
        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            msaa_view,
            depth_view,
            pipeline,
            prop_pipeline,
            layer_pipeline,
            overlay_pipeline,
            stage_pipelines,
            camera_layout,
            camera_buf,
            camera_bg,
            fx_lights_buf,
            material_layout,
            stage_layout,
            diffuse_sampler,
            lightmap_sampler,
            white_view,
            world: None,
            vis_counts: VisCounts::default(),
            shader_dropped: 0,
            lib_warn_count: lib.warn_count(),
            vm_pass,
            dynamic,
            fx,
            hud,
            hud_pass,
            hud_quad_cap_warned: false,
            shaders,
            shader_lib: lib,
            sky: None,
        })
    }

    /// Builds the map's GPU state; a previous map is dropped first.
    pub fn load_world(&mut self, bsp: &Bsp, fs: &Pk3Fs) -> Result<()> {
        self.unload_world();
        let device = &self.device;
        let queue = &self.queue;
        let material_layout = &self.material_layout;
        let diffuse_sampler = &self.diffuse_sampler;
        let lightmap_sampler = &self.lightmap_sampler;
        let white_view = &self.white_view;

        // Per-material draw decision: authored blocks classify from their
        // parsed surface bits, implicit names from the gameplay prefixes.
        let kinds: Vec<MaterialKind> = bsp
            .materials
            .iter()
            .map(|m| match self.shader_lib.get(&m.name) {
                Some(sh) => mesh::material_kind(
                    &m.name,
                    true,
                    sh.surface.nodraw,
                    sh.surface.sky || sh.sky.is_some(),
                ),
                None => mesh::material_kind(&m.name, false, false, false),
            })
            .collect();

        // Active sky: authored blocks named by this map's materials, ranked
        // by soup reference count, ties alphabetical.
        let norm = |s: &str| s.to_lowercase().replace('\\', "/");
        let mut material_idx: HashMap<String, u16> = HashMap::new();
        for (i, m) in bsp.materials.iter().enumerate() {
            material_idx.entry(norm(&m.name)).or_insert(i as u16);
        }
        let mut soup_refs: HashMap<u16, u32> = HashMap::new();
        for s in &bsp.soups {
            if kinds[s.material as usize] == MaterialKind::Sky {
                *soup_refs.entry(s.material).or_default() += 1;
            }
        }
        let candidates: Vec<(String, u16)> = self
            .shader_lib
            .sky_blocks()
            .filter_map(|sh| material_idx.get(&sh.name).map(|&mi| (sh.name.clone(), mi)))
            .collect();
        let active_sky = {
            let with_refs: Vec<(String, u32)> = candidates
                .iter()
                .map(|(n, mi)| (n.clone(), soup_refs.get(mi).copied().unwrap_or(0)))
                .collect();
            sky::pick_sky(&with_refs)
                .and_then(|picked| candidates.iter().find(|(n, _)| *n == picked))
                .cloned()
        };

        // Props (misc_model entities) are baked to world space on the CPU in
        // the same vertex format and extend the map's buffers.
        let (mut indices, batches, soup_ranges) = mesh::build_batches(bsp, &kinds);
        if batches.is_empty() {
            bail!("map has no drawable surfaces");
        }
        let props = props::build(fs, &bsp.entities);
        let prop_first_index = indices.len() as u32;
        let prop_first_vertex = bsp.verts.len() as u32;
        indices.extend(props.indices.iter().map(|i| i + prop_first_vertex));
        // prop batches follow the world's in `batch_draws`, prop indices follow
        // the world's in the merged buffer
        let prop_ranges: Vec<(u32, IndexRange)> = props
            .ranges
            .iter()
            .map(|&(p, r)| {
                (
                    p,
                    IndexRange {
                        batch: r.batch + batches.len() as u32,
                        first: r.first + prop_first_index,
                        count: r.count,
                    },
                )
            })
            .collect();
        let mut vertices = bsp.verts.clone();
        vertices.extend_from_slice(&props.verts);

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("map indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });

        let fallback_px = fallback_pixels();
        let shaders = assets::load_shaders(fs);
        let mut material_views: HashMap<u16, wgpu::TextureView> = HashMap::new();
        let (mut loaded, mut fallbacks) = (0usize, 0usize);
        for batch in &batches {
            if material_views.contains_key(&batch.material) {
                continue;
            }
            let name = &bsp.materials[batch.material as usize].name;
            let img = assets::load_material_image(fs, &shaders, name);
            if is_fallback(&img, &fallback_px) {
                fallbacks += 1;
            } else {
                loaded += 1;
            }
            material_views.insert(batch.material, upload_image(device, queue, name, &img));
        }

        // One RGBA lightmap page per BSP page, plus a white 1x1 for unlit soups.
        let lightmap_views: Vec<wgpu::TextureView> = bsp
            .lightmaps
            .iter()
            .enumerate()
            .map(|(i, page)| {
                let mut rgba = Vec::with_capacity(page.len() / 3 * 4);
                for px in page.as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                upload_rgba(
                    device,
                    queue,
                    &format!("lightmap {i}"),
                    bsp::LIGHTMAP_SIZE as u32,
                    bsp::LIGHTMAP_SIZE as u32,
                    &rgba,
                )
            })
            .collect();

        let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
        let mut cache: HashMap<(u16, u16), usize> = HashMap::new();
        let mut batch_draws = Vec::with_capacity(batches.len());
        for Batch {
            material, lightmap, ..
        } in &batches
        {
            let idx = *cache.entry((*material, *lightmap)).or_insert_with(|| {
                let lm = match *lightmap {
                    bsp::NO_LIGHTMAP => white_view,
                    i => lightmap_views.get(i as usize).unwrap_or(white_view),
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
                            resource: wgpu::BindingResource::TextureView(lm),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(diffuse_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(lightmap_sampler),
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
            } else {
                Pass::Opaque
            };
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass,
            });
        }

        // Prop materials are skin filenames, not `textures/...` names, so
        // `load_skin_image` resolves them.
        let mut skin_cache: HashMap<&str, usize> = HashMap::new();
        for batch in &props.batches {
            let idx = *skin_cache.entry(batch.skin.as_str()).or_insert_with(|| {
                let img = assets::load_skin_image(fs, &batch.skin);
                let view = upload_image(device, queue, &batch.skin, &img);
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
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(diffuse_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(lightmap_sampler),
                        },
                    ],
                }));
                bind_groups.len() - 1
            });
            batch_draws.push(DrawCall {
                bind_group: idx,
                pass: Pass::Prop,
            });
        }

        let count = |p: Pass| batch_draws.iter().filter(|d| d.pass == p).count();
        println!(
            "{} batches ({} opaque, {} prop, {} layer, {} overlay), {} draw indices, {} vertices",
            batch_draws.len(),
            count(Pass::Opaque),
            count(Pass::Prop),
            count(Pass::Layer),
            count(Pass::Overlay),
            indices.len(),
            vertices.len()
        );
        println!(
            "{} textures loaded, {} fallback checkerboards, {} lightmap pages",
            loaded,
            fallbacks,
            lightmap_views.len()
        );

        // One dynamic-offset slot per authored stage of every map material
        // that resolves in the shader lib; batches of one material share the
        // slot per stage. Implicit materials use no slots.
        let mut stage_slots: Vec<(&Shader, usize)> = Vec::new();
        let mut slot_of: HashMap<(u16, usize), u32> = HashMap::new();
        for (mi, mat) in bsp.materials.iter().enumerate() {
            if let Some(sh) = self.shader_lib.get(&mat.name) {
                for idx in 0..sh.stages.len() {
                    slot_of.insert((mi as u16, idx), stage_slots.len() as u32);
                    stage_slots.push((sh, idx));
                }
            }
        }
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stage_params_stride = STAGE_PARAMS_SIZE.div_ceil(align) * align;
        let stage_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage params"),
            size: (stage_slots.len() as u64 * stage_params_stride).max(stage_params_stride),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Provisional fill at t = 0 so every slot starts valid (static stages
        // keep these values; animated ones are rewritten each frame).
        for (i, (sh, idx)) in stage_slots.iter().enumerate() {
            if let Some(p) = stage_params(sh, *idx, 0.0) {
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

        // Expand authored materials: one StageBatch per parsed stage sharing
        // the parent batch's index range. Implicit materials and blocks with
        // no effective stages keep the legacy fast path.
        let mut stage_batches: Vec<StageBatch> = Vec::new();
        let mut stages_of_batch: Vec<Vec<u32>> = vec![Vec::new(); batches.len()];
        let mut animated: Vec<(Shader, usize, u32)> = Vec::new();
        let mut bundle_views: HashMap<String, wgpu::TextureView> = HashMap::new();
        // once per material, not per batch: a material can hold several batches
        let dropped_stages: usize = bsp
            .materials
            .iter()
            .zip(&kinds)
            .filter(|(_, k)| **k == MaterialKind::Draw)
            .filter_map(|(m, _)| self.shader_lib.get(&m.name))
            .map(|sh| sh.dropped_stages)
            .sum();
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
            let lm_view = match batch.lightmap {
                bsp::NO_LIGHTMAP => None,
                i => lightmap_views.get(i as usize),
            };
            for si in 0..sh.stages.len() {
                let Some((variant, two_sided, bias, band)) = stage_draw_facts(sh, si) else {
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
                    diffuse_sampler,
                    lightmap_sampler,
                    &mut bundle_views,
                    &sh.stages[si],
                );
                if stage_animated(sh, si) {
                    animated.push((sh.clone(), si, slot));
                }
                stages_of_batch[bi].push(stage_batches.len() as u32);
                stage_batches.push(StageBatch {
                    batch: bi as u32,
                    slot,
                    variant,
                    two_sided,
                    bias,
                    band,
                    mat,
                    stage,
                });
            }
        }
        for (bi, sbs) in stages_of_batch.iter().enumerate() {
            if !sbs.is_empty() {
                batch_draws[bi].pass = Pass::Stage;
            }
        }
        self.shader_dropped = dropped_stages;
        println!(
            "shader scripts: {} stage batches, {} dropped stages, {} lib warnings",
            stage_batches.len(),
            dropped_stages,
            self.shader_lib.warn_count()
        );

        // ---- sky pass: farbox now, cloud dome through the stage path ----
        self.sky = None;
        let mut sky_stages: Vec<(Shader, usize, u32)> = Vec::new();
        if let Some((name, mat_idx)) = active_sky {
            let sh = self
                .shader_lib
                .get(&name)
                .expect("sky candidate comes from the lib");
            let parms = sh.sky.as_ref().expect("sky_blocks only yields skyparms");
            log::info!(
                "sky: {name}  env {}  cloud height {}  stages {}",
                parms.env,
                parms.cloud_height,
                sh.stages.len()
            );
            // retail skips the whole box when outerbox[0] fails to load
            let mut farbox = None;
            if parms.env.is_empty() {
                log::info!("sky {name}: skyParms '-' outerbox, drawing no farbox");
            } else {
                let rt = format!("{}_rt", parms.env);
                if fs.contains(&rt) || assets::probe_image_path(fs, &rt).is_some() {
                    farbox = Some(build_sky_box(
                        device,
                        queue,
                        self.config.format,
                        fs,
                        &self.camera_layout,
                        &mut bundle_views,
                        &parms.env,
                        &name,
                    ));
                } else {
                    log::warn!("sky {name}: env image {rt} not found, drawing no farbox");
                }
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
                    let Some((variant, two_sided, _, _)) = stage_draw_facts(sh, si) else {
                        continue;
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
                        None,
                        diffuse_sampler,
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
                    println!(
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
            self.sky = Some(SkyRender { name, farbox, dome });
        }

        // Per-batch vertex average for blend-band sorting.
        let mut centroid_sum = vec![glam::Vec3::ZERO; batches.len()];
        let mut centroid_n = vec![0u64; batches.len()];
        for (si, r) in soup_ranges.iter().enumerate() {
            let Some(r) = r else { continue };
            let soup = &bsp.soups[si];
            let fv = soup.first_vertex as usize;
            for v in &bsp.verts[fv..fv + soup.vertex_count as usize] {
                centroid_sum[r.batch as usize] += glam::Vec3::from_slice(&v.pos);
            }
            centroid_n[r.batch as usize] += u64::from(soup.vertex_count);
        }
        let centroids = centroid_sum
            .into_iter()
            .zip(centroid_n)
            .map(|(s, n)| if n == 0 { s } else { s / n as f32 })
            .collect();

        self.world = Some(WorldGpu {
            vertex_buf,
            index_buf,
            bind_groups,
            cpu_indices: indices,
            soup_ranges,
            prop_ranges,
            prop_bounds: props.bounds,
            batch_draws,
            draws: Vec::new(),
            stage_batches,
            stages_of_batch,
            animated,
            sky_stages,
            centroids,
            frame_draws: Vec::new(),
            stage_draws_last: 0,
            stage_params_buf,
            stage_params_stride,
            vis: WorldVis::build(bsp),
            gathered: Vec::new(),
            gather_scratch: Vec::new(),
            locked: None,
            last_cull: None,
        });
        Ok(())
    }

    /// Drops the map and every model uploaded for it, so no `ModelHandle`
    /// from the old map can index a new upload.
    pub fn unload_world(&mut self) {
        self.world = None;
        self.sky = None;
        self.dynamic.models.clear();
        self.dynamic.instances.clear();
        self.dynamic.bone_mats.clear();
        self.vis_counts = VisCounts::default();
    }

    /// Quads in draw order (already back-to-front). Unseen shader names
    /// resolve lazily; an unresolvable one warns once and is cached as
    /// `None`. Additive materials (see `Shaders::is_additive`) go on the
    /// additive pipeline, everything else on straight alpha.
    pub fn set_fx_quads(&mut self, fs: &Pk3Fs, quads: Vec<FxQuad>) {
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

        let mut verts: Vec<FxVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<(String, bool, usize)> = Vec::new();
        for quad in quads {
            if !self.fx.textures.contains_key(&quad.shader) {
                let bg = resolve_fx_texture(
                    &self.device,
                    &self.queue,
                    &self.vm_pass.skin_layout,
                    &self.fx.sampler,
                    fs,
                    self.shaders.image_map(),
                    &quad.shader,
                );
                self.fx.textures.insert(quad.shader.clone(), bg);
            }

            for i in 0..4 {
                verts.push(FxVert {
                    pos: quad.verts[i],
                    uv: quad.uvs[i],
                    rgba: quad.rgba,
                });
            }
            match runs.last_mut() {
                Some((name, _, count)) if *name == quad.shader => *count += 1,
                _ => {
                    let additive = self.shaders.is_additive(&quad.shader);
                    runs.push((quad.shader.clone(), additive, 1));
                }
            }
        }

        self.queue
            .write_buffer(&self.fx.vertex_buf, 0, bytemuck::cast_slice(&verts));
        self.fx.runs = runs;
    }

    /// The sim already truncates to `MAX_LIGHTS`; an empty slice zeroes every slot.
    pub fn set_fx_lights(&mut self, lights: &[FxLight]) {
        let uniform = FxLightsUniform::from_lights(lights);
        self.queue
            .write_buffer(&self.fx_lights_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Quads in draw order, pixel coords with origin top-left. Textures
    /// resolve lazily by name; an unresolvable one warns once and is cached
    /// as `None`.
    pub fn set_hud_quads(&mut self, fs: &Pk3Fs, quads: Vec<HudQuad>) {
        let quads = if quads.len() > MAX_HUD_QUADS {
            if !self.hud_quad_cap_warned {
                self.hud_quad_cap_warned = true;
                log::warn!(
                    "hud: {} quads exceeds the {MAX_HUD_QUADS} cap, truncating",
                    quads.len()
                );
            }
            &quads[..MAX_HUD_QUADS]
        } else {
            &quads[..]
        };

        let (w, h) = (self.config.width as f32, self.config.height as f32);
        let mut verts: Vec<HudVert> = Vec::with_capacity(quads.len() * 4);
        let mut runs: Vec<(String, usize)> = Vec::new();
        for quad in quads {
            if !self.hud_pass.textures.contains_key(&quad.texture) {
                let bg = resolve_hud_texture(
                    &self.device,
                    &self.queue,
                    &self.hud_pass.texture_layout,
                    &self.hud_pass.sampler,
                    fs,
                    &quad.texture,
                );
                self.hud_pass.textures.insert(quad.texture.clone(), bg);
            }

            for i in 0..4 {
                let [px, py] = quad.verts[i];
                verts.push(HudVert {
                    // pixel coords, origin top-left -> clip space
                    pos: [px / w * 2.0 - 1.0, 1.0 - py / h * 2.0],
                    uv: quad.uvs[i],
                    color: quad.rgba,
                });
            }
            match runs.last_mut() {
                Some((name, count)) if *name == quad.texture => *count += 1,
                _ => runs.push((quad.texture.clone(), 1)),
            }
        }

        self.queue
            .write_buffer(&self.hud_pass.vertex_buf, 0, bytemuck::cast_slice(&verts));
        self.hud_pass.quad_count = quads.len();
        self.hud_pass.runs = runs;
    }

    /// Quads drawn last frame, for the F3 overlay.
    pub fn hud_quad_count(&self) -> usize {
        self.hud_pass.quad_count
    }

    /// Slice order is draw order (hands, then the gun). Replaces anything set before.
    pub fn set_viewmodel(&mut self, fs: &Pk3Fs, models: &[xmodel::XModel]) {
        let vm = &self.vm_pass;
        let uploaded: Vec<VmModel> = models
            .iter()
            .filter_map(|m| {
                let uploaded = upload_vm_model(
                    &self.device,
                    &self.queue,
                    vm,
                    &m.surfaces,
                    &m.materials,
                    &|skin| assets::load_skin_image(fs, skin),
                );
                if uploaded.is_none() {
                    log::warn!("viewmodel {}: no drawable surfaces, skipping it", m.lod);
                }
                uploaded
            })
            .collect();
        let surfaces: usize = uploaded.iter().map(|m| m.surfaces.len()).sum();
        println!(
            "viewmodel: {} models, {surfaces} drawn surfaces",
            uploaded.len()
        );
        self.vm_pass.models = uploaded;
    }

    /// Same upload as the viewmodel (bind pose baked into the mesh); an
    /// empty model returns `None`.
    pub fn upload_dynamic_model(
        &mut self,
        fs: &Pk3Fs,
        model: &xmodel::XModel,
    ) -> Option<ModelHandle> {
        let vm = &self.vm_pass;
        let uploaded = upload_vm_model(
            &self.device,
            &self.queue,
            vm,
            &model.surfaces,
            &model.materials,
            &|skin| assets::load_skin_image(fs, skin),
        )?;
        self.dynamic.models.push(uploaded);
        Some(ModelHandle(self.dynamic.models.len() - 1))
    }

    /// For inline BSP submodels ([`vcod_common::bsp::Bsp::submodel_mesh`]).
    /// Materials are `textures/...` names, resolved through the shader
    /// scripts rather than as skin filenames.
    pub fn upload_dynamic_mesh(
        &mut self,
        fs: &Pk3Fs,
        surfaces: &[xmodel::Surface],
        materials: &[String],
    ) -> Option<ModelHandle> {
        let vm = &self.vm_pass;
        let uploaded = upload_vm_model(
            &self.device,
            &self.queue,
            vm,
            surfaces,
            materials,
            &|name| assets::load_material_image(fs, &self.shaders, name),
        )?;
        self.dynamic.models.push(uploaded);
        Some(ModelHandle(self.dynamic.models.len() - 1))
    }

    /// Instances past `MAX_DYNAMIC_INSTANCES` are dropped (warned once); a
    /// stale handle is skipped. Written to the GPU in [`Self::render`].
    pub fn set_dynamic_models(&mut self, instances: &[DynamicModelInstance]) {
        static OVERFLOW_WARNED: std::sync::Once = std::sync::Once::new();

        let mut model_idxs = Vec::with_capacity(instances.len());
        let mut items = Vec::with_capacity(instances.len());
        for inst in instances {
            if model_idxs.len() == MAX_DYNAMIC_INSTANCES {
                OVERFLOW_WARNED.call_once(|| {
                    log::warn!(
                        "more than {MAX_DYNAMIC_INSTANCES} live entities this frame; dropping the rest"
                    );
                });
                break;
            }
            let ModelHandle(idx) = inst.model;
            if idx >= self.dynamic.models.len() {
                continue;
            }
            model_idxs.push(idx);
            items.push((idx, inst.transform.to_cols_array(), inst.bones.as_deref()));
        }
        let (raw, bone_mats) = pack_instances(&items);
        self.dynamic.instances = model_idxs.into_iter().zip(raw).collect();
        self.dynamic.bone_mats = bone_mats;
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.msaa_view = create_msaa_view(&self.device, self.config.format, w, h);
        self.depth_view = create_depth_view(&self.device, w, h);
    }

    /// For a lost or outdated swapchain.
    pub fn reconfigure(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        self.resize(w, h);
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
    pub fn shader_stats(&self) -> (usize, usize, usize) {
        (
            self.world.as_ref().map_or(0, |w| w.stage_draws_last),
            self.shader_dropped,
            self.lib_warn_count,
        )
    }

    /// Name of the map's active sky block, for the F3 `sky` field.
    pub fn sky_name(&self) -> Option<&str> {
        self.sky.as_ref().map(|s| s.name.as_str())
    }

    /// `[variant][two_sided][bias]`.
    fn stage_pipeline(
        &self,
        variant: StageVariant,
        two_sided: bool,
        bias: bool,
    ) -> &wgpu::RenderPipeline {
        &self.stage_pipelines[variant as usize][usize::from(two_sided)][usize::from(bias)]
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// Pixels.
    pub fn screen_size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    /// The visible set and the prop flags that go with it. A placement whose
    /// model failed to load has zero bounds and is never drawn or counted.
    fn cull(world: &WorldGpu, eye: glam::Vec3, frustum: &Frustum) -> (Visible, Vec<bool>) {
        let v = world.vis.visible(eye, frustum);
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
        // `proj` (64) + `time_pad` (16), matching Camera in the WGSL modules.
        let mut camera = [0.0f32; 20];
        camera[..16].copy_from_slice(&frame.view_proj.to_cols_array());
        camera[16] = frame.time;
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::cast_slice(&camera));
        if let Some(sky) = &self.sky {
            if let Some(farbox) = &sky.farbox {
                self.queue.write_buffer(
                    &farbox.eye_buf,
                    0,
                    bytemuck::bytes_of(&[frame.eye.x, frame.eye.y, frame.eye.z, 0.0f32]),
                );
            }
        }
        let t0 = std::time::Instant::now();
        let frustum = Frustum::from_view_proj(frame.view_proj);
        if let Some(world) = &mut self.world {
            let visible = match frame.cull {
                CullMode::Off => None,
                CullMode::Locked => Some(match world.locked.take() {
                    Some(v) => v,
                    None => {
                        // a fresh freeze: the index buffer holds some other set
                        world.last_cull = None;
                        Self::cull(world, frame.eye, &frustum)
                    }
                }),
                CullMode::On => Some(Self::cull(world, frame.eye, &frustum)),
            };
            let mut props_drawn = 0usize;
            let ranges: Vec<IndexRange> = match &visible {
                None => world
                    .soup_ranges
                    .iter()
                    .flatten()
                    .copied()
                    .chain(world.prop_ranges.iter().map(|&(_, r)| r))
                    .collect(),
                Some((v, prop_ok)) => {
                    props_drawn = prop_ok.iter().filter(|&&b| b).count();
                    world
                        .soup_ranges
                        .iter()
                        .zip(&v.soups)
                        .filter_map(|(r, &vis)| if vis { *r } else { None })
                        .chain(
                            world
                                .prop_ranges
                                .iter()
                                .filter(|(p, _)| prop_ok[*p as usize])
                                .map(|&(_, r)| r),
                        )
                        .collect()
                }
            };
            world.draws = gather(
                &world.cpu_indices,
                ranges,
                world.batch_draws.len(),
                &mut world.gathered,
                &mut world.gather_scratch,
            );
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
            // Dome stages always carry the view origin, animated or not.
            for (sh, si, slot) in &world.sky_stages {
                if let Some(mut p) = stage_params(sh, *si, frame.time) {
                    p.flags |= STAGE_FLAG_EYE_OFFSET;
                    p.eye_off = [frame.eye.x, frame.eye.y, frame.eye.z, 0.0];
                    self.queue.write_buffer(
                        &world.stage_params_buf,
                        u64::from(*slot) * world.stage_params_stride,
                        bytemuck::bytes_of(&p),
                    );
                }
            }
            // Emission order: opaque (legacy then staged cutouts), props, the
            // legacy biased passes, the staged decal slot, see-through blends,
            // back-to-front blends, additives. Sky soups are not in `draws`.
            let mut legacy: [Vec<u32>; 4] = Default::default();
            let mut bands: [Vec<(u32, u32)>; 5] = Default::default();
            for (di, d) in world.draws.iter().enumerate() {
                match world.batch_draws[d.batch as usize].pass {
                    Pass::Stage => {
                        for &sb in &world.stages_of_batch[d.batch as usize] {
                            let band = world.stage_batches[sb as usize].band;
                            bands[band as usize].push((sb, di as u32));
                        }
                    }
                    p => legacy[p as usize].push(di as u32),
                }
            }
            let eye = frame.eye;
            bands[BAND_BLEND as usize].sort_by(|(a, _), (b, _)| {
                let da = (world.centroids[world.stage_batches[*a as usize].batch as usize] - eye)
                    .length_squared();
                let db = (world.centroids[world.stage_batches[*b as usize].batch as usize] - eye)
                    .length_squared();
                db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
            });
            world.frame_draws.clear();
            for &di in &legacy[Pass::Opaque as usize] {
                world.frame_draws.push(FrameDraw {
                    kind: DrawRef::Legacy(Pass::Opaque),
                    src: di,
                });
            }
            for &(sb, di) in &bands[BAND_OPAQUE as usize] {
                world.frame_draws.push(FrameDraw {
                    kind: DrawRef::Stage(sb),
                    src: di,
                });
            }
            for pass in [Pass::Prop, Pass::Layer, Pass::Overlay] {
                for &di in &legacy[pass as usize] {
                    world.frame_draws.push(FrameDraw {
                        kind: DrawRef::Legacy(pass),
                        src: di,
                    });
                }
            }
            for band in [BAND_DECAL, BAND_SEETHROUGH, BAND_BLEND, BAND_ADDITIVE] {
                for &(sb, di) in &bands[band as usize] {
                    world.frame_draws.push(FrameDraw {
                        kind: DrawRef::Stage(sb),
                        src: di,
                    });
                }
            }
            world.stage_draws_last = world
                .frame_draws
                .iter()
                .filter(|fd| matches!(fd.kind, DrawRef::Stage(_)))
                .count();
            // in Locked and Off the gathered set is the same every frame
            let unchanged = frame.cull != CullMode::On && world.last_cull == Some(frame.cull);
            if !unchanged {
                self.queue
                    .write_buffer(&world.index_buf, 0, bytemuck::cast_slice(&world.gathered));
            }
            world.last_cull = Some(frame.cull);
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
                gather_ms: t0.elapsed().as_secs_f32() * 1000.0,
            };
            if frame.cull == CullMode::Locked {
                world.locked = visible;
            } else {
                world.locked = None;
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
            let raw: Vec<InstanceRaw> = self.dynamic.instances.iter().map(|(_, r)| *r).collect();
            self.queue
                .write_buffer(&self.dynamic.instance_buf, 0, bytemuck::cast_slice(&raw));
            self.queue.write_buffer(
                &self.dynamic.bone_buf,
                0,
                bytemuck::cast_slice(&self.dynamic.bone_mats),
            );
        }
        let draw_vm = match &vm {
            Some(draw) if !self.vm_pass.models.is_empty() => {
                let uniform = vm_uniform(draw.transform, self.aspect());
                self.queue.write_buffer(
                    &self.vm_pass.uniform_buf,
                    0,
                    bytemuck::cast_slice(&uniform),
                );
                for (i, model) in self.vm_pass.models.iter().enumerate() {
                    let Some(bones) = draw.bone_sets.get(i) else {
                        continue;
                    };
                    if bones.is_empty() {
                        continue;
                    }
                    debug_assert!(
                        bones.len() <= VM_BONE_COUNT,
                        "bone set exceeds the shader's {VM_BONE_COUNT}-matrix uniform"
                    );
                    let cols: Vec<[f32; 16]> =
                        bones.iter().map(glam::Mat4::to_cols_array).collect();
                    self.queue
                        .write_buffer(&model.bone_buf, 0, bytemuck::cast_slice(&cols));
                }
                true
            }
            _ => false,
        };

        let surface_tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::warn!("dropped frame: surface validation error");
                return;
            }
        };
        let view = surface_tex.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.35,
                            g: 0.42,
                            b: 0.50,
                            a: 1.0,
                        }),
                        // only the resolved single-sample image is needed
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
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
            pass.set_bind_group(0, &self.camera_bg, &[]);
            // The sky draws first so every world draw overwrites it; the box
            // and dome both ignore depth (cleared 1.0) and write none.
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
                        }
                        pass.set_bind_group(1, ds.mat.bind_group(frame.time), &[]);
                        pass.set_bind_group(
                            2,
                            ds.stage.bind_group(frame.time),
                            &[
                                u32::try_from(u64::from(ds.slot) * world.stage_params_stride)
                                    .unwrap_or(0),
                            ],
                        );
                        pass.draw_indexed(0..dome.index_count, 0, 0..1);
                    }
                }
            }
            if let Some(world) = &self.world {
                pass.set_vertex_buffer(0, world.vertex_buf.slice(..));
                pass.set_index_buffer(world.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                // The flat list mixes two pipeline layouts; switch only on change.
                #[derive(Copy, Clone, PartialEq)]
                enum Bound {
                    Legacy(Pass),
                    Stage(StageVariant, bool, bool),
                }
                let mut bound: Option<Bound> = None;
                for fd in &world.frame_draws {
                    let draw = &world.draws[fd.src as usize];
                    match fd.kind {
                        DrawRef::Legacy(want) => {
                            if bound != Some(Bound::Legacy(want)) {
                                pass.set_pipeline(match want {
                                    Pass::Opaque => &self.pipeline,
                                    Pass::Prop => &self.prop_pipeline,
                                    Pass::Layer => &self.layer_pipeline,
                                    Pass::Overlay => &self.overlay_pipeline,
                                    Pass::Stage => continue,
                                });
                                bound = Some(Bound::Legacy(want));
                            }
                            let call = &world.batch_draws[draw.batch as usize];
                            pass.set_bind_group(1, &world.bind_groups[call.bind_group], &[]);
                        }
                        DrawRef::Stage(sb_idx) => {
                            let sb = &world.stage_batches[sb_idx as usize];
                            let key = Bound::Stage(sb.variant, sb.two_sided, sb.bias);
                            if bound != Some(key) {
                                pass.set_pipeline(self.stage_pipeline(
                                    sb.variant,
                                    sb.two_sided,
                                    sb.bias,
                                ));
                                bound = Some(key);
                            }
                            // animMap stages pick their frame per draw
                            pass.set_bind_group(1, sb.mat.bind_group(frame.time), &[]);
                            pass.set_bind_group(
                                2,
                                sb.stage.bind_group(frame.time),
                                &[
                                    u32::try_from(u64::from(sb.slot) * world.stage_params_stride)
                                        .unwrap_or(0),
                                ],
                            );
                        }
                    }
                    pass.draw_indexed(draw.first..draw.first + draw.count, 0, 0..1);
                }
            }

            // Live entities draw after the world so they depth-test against it.
            if !self.dynamic.instances.is_empty() {
                let dynamic = &self.dynamic;
                pass.set_pipeline(&dynamic.pipeline);
                pass.set_bind_group(0, &self.camera_bg, &[]);
                pass.set_bind_group(2, &dynamic.bone_bg, &[]);
                for (i, (model_idx, _)) in dynamic.instances.iter().enumerate() {
                    let model = &dynamic.models[*model_idx];
                    let off = i as u64 * DYNAMIC_INSTANCE_STRIDE;
                    pass.set_vertex_buffer(0, model.vertex_buf.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        dynamic
                            .instance_buf
                            .slice(off..off + DYNAMIC_INSTANCE_STRIDE),
                    );
                    pass.set_index_buffer(model.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                    for s in &model.surfaces {
                        pass.set_bind_group(1, &model.bind_groups[s.bind_group], &[]);
                        pass.draw_indexed(
                            s.first_index..s.first_index + s.index_count,
                            s.base_vertex,
                            0..1,
                        );
                    }
                }
            }

            // Fx quads blend against whatever already drew, so after the
            // overlays and before the weapon. A run whose texture failed to
            // resolve is skipped.
            if !self.fx.runs.is_empty() {
                let fx = &self.fx;
                pass.set_bind_group(0, &self.camera_bg, &[]);
                pass.set_vertex_buffer(0, fx.vertex_buf.slice(..));
                pass.set_index_buffer(fx.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                // `current` skips redundant set_pipeline calls between same-blend runs.
                let mut current: Option<bool> = None;
                let mut first_quad = 0usize;
                for (shader, additive, count) in &fx.runs {
                    if let Some(Some(bg)) = fx.textures.get(shader) {
                        if current != Some(*additive) {
                            pass.set_pipeline(if *additive {
                                &fx.additive_pipeline
                            } else {
                                &fx.pipeline
                            });
                            current = Some(*additive);
                        }
                        pass.set_bind_group(1, bg, &[]);
                        let start = (first_quad * 6) as u32;
                        let end = ((first_quad + count) * 6) as u32;
                        pass.draw_indexed(start..end, 0, 0..1);
                    }
                    first_quad += count;
                }
            }

            if draw_vm {
                // The weapon is squeezed into the front slice of the depth
                // range so nothing the world wrote can clip through it.
                let (w, h) = (self.config.width as f32, self.config.height as f32);
                pass.set_viewport(0.0, 0.0, w, h, 0.0, VM_DEPTH_RANGE);
                pass.set_pipeline(&self.vm_pass.pipeline);
                pass.set_bind_group(0, &self.vm_pass.uniform_bg, &[]);
                for model in &self.vm_pass.models {
                    pass.set_vertex_buffer(0, model.vertex_buf.slice(..));
                    pass.set_index_buffer(model.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                    pass.set_bind_group(2, &model.bone_bg, &[]);
                    for s in &model.surfaces {
                        pass.set_bind_group(1, &model.bind_groups[s.bind_group], &[]);
                        pass.draw_indexed(
                            s.first_index..s.first_index + s.index_count,
                            s.base_vertex,
                            0..1,
                        );
                    }
                }
                pass.set_viewport(0.0, 0.0, w, h, 0.0, 1.0);
            }

            // HUD after the weapon so it sits on top, before the debug
            // overlay so F3 always wins.
            if !self.hud_pass.runs.is_empty() {
                let hud_pass = &self.hud_pass;
                pass.set_pipeline(&hud_pass.pipeline);
                pass.set_vertex_buffer(0, hud_pass.vertex_buf.slice(..));
                pass.set_index_buffer(hud_pass.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                let mut first_quad = 0usize;
                for (texture, count) in &hud_pass.runs {
                    if let Some(Some(bg)) = hud_pass.textures.get(texture) {
                        pass.set_bind_group(0, bg, &[]);
                        let start = (first_quad * 6) as u32;
                        let end = ((first_quad + count) * 6) as u32;
                        pass.draw_indexed(start..end, 0, 0..1);
                    }
                    first_quad += count;
                }
            }

            // Debug HUD text, over everything.
            if hud_quads > 0 {
                let hud = &self.hud;
                pass.set_pipeline(&hud.pipeline);
                pass.set_bind_group(0, &hud.bind_group, &[]);
                pass.set_vertex_buffer(0, hud.vertex_buf.slice(..));
                pass.set_index_buffer(hud.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..(hud_quads * 6) as u32, 0, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(surface_tex);
    }
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
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
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
) -> FxPass {
    // ClampToEdge: repeat would bleed the opposite edge into a sprite's rim.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("fx sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });

    let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fx vertices"),
        size: (MAX_FX_QUADS * 4 * std::mem::size_of::<FxVert>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Quad n spans vertices 4n..4n+4. u32 because MAX_FX_QUADS * 4 can
    // exceed u16::MAX.
    let indices: Vec<u32> = (0..MAX_FX_QUADS)
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

    let shader = device.create_shader_module(wgpu::include_wgsl!("fx.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fx pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(texture_layout)],
        immediate_size: 0,
    });
    // The two pipelines must differ only in blend and fragment entry so runs
    // can interleave within one pass.
    let make = |label, fs_entry, blend| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
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
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
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
                // The world must occlude fx quads, but overlapping quads must all draw.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
            cache: None,
        })
    };

    let pipeline = make("fx pipeline", "fs_main", wgpu::BlendState::ALPHA_BLENDING);
    // `GL_ONE GL_ONE`, as the effect materials' blendFunc. The fragment
    // writes alpha 0 so additive quads never inflate the target's alpha.
    let additive_pipeline = make(
        "fx additive pipeline",
        "fs_additive",
        wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
    );

    FxPass {
        pipeline,
        additive_pipeline,
        vertex_buf,
        index_buf,
        sampler,
        textures: HashMap::new(),
        runs: Vec::new(),
    }
}

/// Resolves an fx shader name to a loadable texture path. `load_path_image`
/// never fails (a missing file becomes the checkerboard), so unresolvable
/// names must be caught here or they draw checkerboard quads. `.efx` names
/// are usually extensionless material names, probed like
/// `assets::probe_image_path`. A name with a known extension is tried as-is
/// first; if that file is missing the other extensions are probed too, since
/// weapon `killIcon`s say `.tga` while the art ships as `.dds`
/// (docs/research/cod11-hud-protocol.md, section 2).
fn resolve_fx_path(
    shader_images: &HashMap<String, String>,
    fs: &Pk3Fs,
    name: &str,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let path = shader_images.get(name).map(String::as_str).unwrap_or(name);
    let ext_at = path.rfind('.');
    let has_known_ext = ext_at.is_some_and(|i| {
        assets::IMAGE_EXTS
            .iter()
            .any(|known| *known == path[i..].to_lowercase())
    });
    if has_known_ext {
        if fs.contains(path) {
            return Some(path.to_string());
        }
        let base = &path[..ext_at.unwrap()];
        return assets::probe_image_path(fs, base);
    }
    assets::probe_image_path(fs, path)
}

/// `upload_image` handles both RGBA sprites and BC `.dds` mip chains (many
/// effect textures are DXT5). Unresolvable names warn once; the caller
/// caches the `None`.
fn resolve_fx_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    fs: &Pk3Fs,
    shader_images: &HashMap<String, String>,
    name: &str,
) -> Option<wgpu::BindGroup> {
    let Some(path) = resolve_fx_path(shader_images, fs, name) else {
        log::warn!("fx shader {name:?}: no texture found for it, dropping its quads");
        return None;
    };
    let img = assets::load_path_image(fs, &path);
    let view = upload_image(device, queue, name, &img);
    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(name),
        layout: texture_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
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
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // depth ignored, draws over everything
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    });

    HudTextPass {
        pipeline,
        vertex_buf,
        index_buf,
        bind_group,
    }
}

/// Like `create_hud_text_pass` but with `hud.wgsl` and a linear sampler:
/// font pages and icons draw at scale and would alias under nearest sampling.
fn create_hud_pass(device: &wgpu::Device, format: wgpu::TextureFormat) -> HudPass {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("hud sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

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
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // depth ignored, draws over everything
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    });

    HudPass {
        pipeline,
        vertex_buf,
        index_buf,
        sampler,
        texture_layout,
        textures: HashMap::new(),
        runs: Vec::new(),
        quad_count: 0,
    }
}

/// HUD names are pk3 paths, not shader-script materials, so the alias map
/// is empty. Unresolvable names warn once; the caller caches the `None`.
fn resolve_hud_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    fs: &Pk3Fs,
    name: &str,
) -> Option<wgpu::BindGroup> {
    let Some(path) = resolve_fx_path(&HashMap::new(), fs, name) else {
        log::warn!("hud: no texture found for {name:?}, dropping its quads");
        return None;
    };
    let img = assets::load_path_image(fs, &path);
    let view = upload_image(device, queue, name, &img);
    Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(name),
        layout: texture_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    }))
}

/// Built at startup, viewmodel or not, so `viewmodel.wgsl` is validated on every launch.
fn create_vm_pass(device: &wgpu::Device, format: wgpu::TextureFormat) -> VmPass {
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel uniform layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            // light_dir is read in the fragment stage
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(VM_UNIFORM_SIZE),
            },
            count: None,
        }],
    });
    let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel skin layout"),
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
    let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewmodel uniform bind group"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("viewmodel skin sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 16,
        ..Default::default()
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("viewmodel.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewmodel pipeline layout"),
        bind_group_layouts: &[
            Some(&uniform_layout),
            Some(&skin_layout),
            Some(&bone_layout),
        ],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewmodel pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<VmVert>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, // pos
                    1 => Float32x3, // normal
                    2 => Float32x2, // uv
                    3 => Uint8x4,   // bone_indices
                    4 => Float32x4, // bone_weights
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
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
            // Unculled. Measured on the shipped models, the geometric normal
            // agrees with the vertex normals on ~99% of the kar98k's triangles
            // and ~78% of the hands', and on none of the bsp's: the gun is
            // wound opposite to the map and the hands inconsistently, so no
            // front_face works.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    });

    VmPass {
        pipeline,
        uniform_buf,
        uniform_bg,
        skin_layout,
        bone_layout,
        sampler,
        models: Vec::new(),
    }
}

/// Group 0 is the camera, group 1 the viewmodel skin layout (the uploaded
/// models' bind groups are built against it), group 2 the bone storage
/// buffer. Vertex buffer 1 is instance-step `InstanceRaw`.
fn create_dynamic_pass(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    skin_layout: &wgpu::BindGroupLayout,
) -> DynamicPass {
    let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic instance transforms"),
        size: MAX_DYNAMIC_INSTANCES as u64 * DYNAMIC_INSTANCE_STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Worst case: every instance skinned at the cap, plus the identity block.
    let bone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("dynamic bone layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bone_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic bone matrices"),
        size: (MAX_DYNAMIC_INSTANCES + 1) as u64 * MAX_INSTANCE_BONES as u64 * 64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bone_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dynamic bone bind group"),
        layout: &bone_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: bone_buf.as_entire_binding(),
        }],
    });

    let shader = device.create_shader_module(wgpu::include_wgsl!("dynamic_model.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("dynamic pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(skin_layout), Some(&bone_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("dynamic model pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<VmVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // pos
                        1 => Float32x3, // normal
                        2 => Float32x2, // uv
                        3 => Uint8x4,   // bone_indices
                        4 => Float32x4, // bone_weights
                    ],
                }),
                // `InstanceRaw`: the transform's four columns, then
                // bone_base; the padding needs no attribute.
                Some(wgpu::VertexBufferLayout {
                    array_stride: DYNAMIC_INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        5 => Float32x4,
                        6 => Float32x4,
                        7 => Float32x4,
                        8 => Float32x4,
                        9 => Uint32,
                    ],
                }),
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
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
            // xmodel winding, same as the viewmodel; cull nothing.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    });

    DynamicPass {
        pipeline,
        instance_buf,
        bone_buf,
        bone_bg,
        models: Vec::new(),
        instances: Vec::new(),
        bone_mats: Vec::new(),
    }
}

/// The models are unlit; the fixed key light stands in for the engine's light grid.
fn vm_uniform(model: glam::Mat4, aspect: f32) -> [f32; 36] {
    let proj = glam::camera::rh::proj::directx::perspective(
        VM_FOV_DEG.to_radians(),
        aspect,
        VM_NEAR,
        VM_FAR,
    );
    // upper-left key light, view space
    let light = glam::Vec4::new(-0.4, 0.8, 0.4, 0.0).normalize();
    let mut out = [0.0f32; 36];
    out[..16].copy_from_slice(&proj.to_cols_array());
    out[16..32].copy_from_slice(&model.to_cols_array());
    out[32..].copy_from_slice(&light.to_array());
    out
}

/// `None` when nothing is drawable (empty buffers are invalid to create).
/// `load_image` resolves a material name; the callers name materials
/// differently (skin filenames vs `textures/...`).
fn upload_vm_model(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    vm: &VmPass,
    model_surfaces: &[xmodel::Surface],
    materials: &[String],
    load_image: &dyn Fn(&str) -> Image,
) -> Option<VmModel> {
    let mut verts: Vec<VmVert> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut bind_groups = Vec::with_capacity(model_surfaces.len());
    let mut surfaces = Vec::with_capacity(model_surfaces.len());
    for surf in model_surfaces {
        if surf.verts.is_empty() || surf.indices.is_empty() {
            continue;
        }
        let skin = materials
            .get(surf.material)
            .map(String::as_str)
            .unwrap_or_default();
        let img = load_image(skin);
        let view = upload_image(device, queue, skin, &img);
        bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewmodel skin bind group"),
            layout: &vm.skin_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&vm.sampler),
                },
            ],
        }));
        surfaces.push(VmSurface {
            first_index: indices.len() as u32,
            index_count: surf.indices.len() as u32,
            base_vertex: verts.len() as i32,
            bind_group: bind_groups.len() - 1,
        });
        verts.extend_from_slice(&surf.verts);
        indices.extend_from_slice(&surf.indices);
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
        bone_buf,
        bone_bg,
    })
}

fn wgpu_format(f: assets::TextureFormat) -> wgpu::TextureFormat {
    match f {
        assets::TextureFormat::Bc1RgbaUnormSrgb => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
        assets::TextureFormat::Bc2RgbaUnormSrgb => wgpu::TextureFormat::Bc2RgbaUnormSrgb,
        assets::TextureFormat::Bc3RgbaUnormSrgb => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
    }
}

/// BC mip chain as-is, or a single RGBA level.
fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    img: &Image,
) -> wgpu::TextureView {
    match &img.data {
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
/// file warns and lands in the checkerboard. Views are deduped per load.
fn upload_bundle_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &Pk3Fs,
    cache: &mut HashMap<String, wgpu::TextureView>,
    path: &str,
) -> wgpu::TextureView {
    if let Some(v) = cache.get(path) {
        return v.clone();
    }
    // script paths may carry a real extension; try as-is, then probe exts
    let img = if fs.contains(path) {
        assets::load_path_image(fs, path)
    } else {
        match assets::probe_image_path(fs, path) {
            Some(full) => assets::load_path_image(fs, &full),
            None => {
                log::warn!("shader image {path} not found, using checkerboard");
                assets::checkerboard()
            }
        }
    };
    let v = upload_image(device, queue, path, &img);
    cache.insert(path.to_string(), v.clone());
    v
}

fn fallback_pixels() -> Vec<u8> {
    match assets::checkerboard().data {
        ImageData::Rgba8(px) => px,
        _ => Vec::new(),
    }
}

/// True for `load_material_image`'s checkerboard placeholder.
fn is_fallback(img: &Image, fallback_px: &[u8]) -> bool {
    img.width == 64
        && img.height == 64
        && matches!(&img.data, ImageData::Rgba8(px) if px == fallback_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;
    use vcod_common::shader::{BlendFactor, Bundle, Stage, TcMod, Wave, WaveForm};

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
        assert_eq!(std::mem::size_of::<StageParams>(), 176);
        let p = StageParams {
            uv0: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            uv1: [7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
            turb01: [13.0, 14.0, 15.0, 16.0],
            tint: [17.0, 18.0, 19.0, 20.0],
            flags: STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_ALPHA,
            _pad: [0; 3],
            vec0_s: [1.0, 0.0, 0.0, 0.0],
            vec0_t: [0.0, 1.0, 0.0, 0.0],
            vec1_s: [0.0, 0.0, 1.0, 0.0],
            vec1_t: [0.0, 0.0, 0.0, 1.0],
            eye_off: [9.0, 8.0, 7.0, 0.0],
        };
        // slot-shaped write/read, like the UBO dynamic-offset path
        let mut slot = [0u8; std::mem::size_of::<StageParams>()];
        slot[..bytemuck::bytes_of(&p).len()].copy_from_slice(bytemuck::bytes_of(&p));
        let q: &StageParams = bytemuck::try_from_bytes(&slot).unwrap();
        assert_eq!(*q, p);
    }

    /// Sky dome stages ride the same UBO slots with the eye-offset flag set.
    #[test]
    fn stage_params_eye_offset_flag_marks_dome_draws() {
        let sh = Shader {
            name: "test/skydome".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("clouds".into()),
                    anim: None,
                    clamp: false,
                    tcmods: vec![TcMod::Scroll(0.004, 0.004), TcMod::Scale(3.0, 3.0)],
                    vector: None,
                }],
                blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                depth_write: None,
                alpha_func: None,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        };
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
    fn stage_params_encodes_const_gens_lightmap_and_alphafunc() {
        let sh = Shader {
            name: "test/mat".into(),
            stages: vec![Stage {
                bundles: vec![
                    Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: false,
                        tcmods: vec![TcMod::Scroll(0.25, 0.0)],
                        vector: None,
                    },
                    Bundle {
                        image: ImageRef::Lightmap,
                        anim: None,
                        clamp: false,
                        tcmods: vec![],
                        vector: None,
                    },
                ],
                blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                depth_write: None,
                alpha_func: Some(AlphaFunc::Ge128),
                rgb_gen: RgbGen::Const([0.5, 0.25, 0.125]),
                alpha_gen: AlphaGen::Const(0.75),
            }],
            ..Default::default()
        };
        let p = stage_params(&sh, 0, 2.0).unwrap();
        assert_eq!(
            p.flags,
            STAGE_FLAG_BUNDLE1_LIGHTMAP | STAGE_FLAG_ALPHAFUNC_GE128
        );
        assert_eq!(p.uv0, bundle_affine(&[TcMod::Scroll(0.25, 0.0)], 2.0));
        assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.tint, [0.5, 0.25, 0.125, 0.75]);
        assert_eq!(p.turb01, [0.0; 4]);
    }

    #[test]
    fn stage_params_vector_bundle_sets_flag_and_carries_basis() {
        let sh = Shader {
            name: "test/vec".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::White,
                    anim: None,
                    clamp: false,
                    tcmods: vec![],
                    vector: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                rgb_gen: RgbGen::Vertex,
                alpha_gen: AlphaGen::Vertex,
            }],
            ..Default::default()
        };
        let p = stage_params(&sh, 0, 0.0).unwrap();
        assert_eq!(
            p.flags,
            STAGE_FLAG_BUNDLE0_VECTOR | STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_ALPHA
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
        let sh = Shader {
            name: "test/wave".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: false,
                    tcmods: vec![],
                    vector: None,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                rgb_gen: RgbGen::Wave(rgb_wave.clone()),
                alpha_gen: AlphaGen::Wave(a_wave.clone()),
            }],
            ..Default::default()
        };
        let p = stage_params(&sh, 0, 123.0).unwrap();
        let rv = vcod_common::shader::wave_value(&rgb_wave, 123.0);
        let av = vcod_common::shader::wave_value(&a_wave, 123.0);
        assert!(
            (p.tint[0] - rv).abs() < 1e-6 && (rv - 1.5).abs() < 1e-3,
            "{p:?}"
        );
        assert_eq!(p.tint[1], rv);
        assert_eq!(p.tint[2], rv);
        assert!((p.tint[3] - av).abs() < 1e-6 && (av - 0.25).abs() < 1e-6);
        assert_eq!(p.flags, 0);
    }

    #[test]
    fn stage_params_identity_gens_stay_white_without_flags() {
        for rgb in [
            RgbGen::Identity,
            RgbGen::IdentityLighting,
            RgbGen::ExactVertex,
        ] {
            let sh = Shader {
                name: "test/id".into(),
                stages: vec![Stage {
                    bundles: vec![Bundle {
                        image: ImageRef::Path("a".into()),
                        anim: None,
                        clamp: false,
                        tcmods: vec![],
                        vector: None,
                    }],
                    blend: None,
                    depth_write: None,
                    alpha_func: None,
                    rgb_gen: rgb.clone(),
                    alpha_gen: AlphaGen::Identity,
                }],
                ..Default::default()
            };
            let p = stage_params(&sh, 0, 9.0).unwrap();
            assert_eq!(p.tint, [1.0, 1.0, 1.0, 1.0], "{rgb:?}");
            // ExactVertex routes through the vertex-colour flag like Vertex
            assert_eq!(
                p.flags,
                u32::from(rgb == RgbGen::ExactVertex) * STAGE_FLAG_VERTEX_RGB
            );
        }
    }

    #[test]
    fn stage_params_single_bundle_leaves_slot_one_neutral() {
        let sh = Shader {
            name: "test/one".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: false,
                    tcmods: vec![TcMod::Scale(2.0, 3.0)],
                    vector: None,
                }],
                blend: None,
                depth_write: None,
                alpha_func: Some(AlphaFunc::Lt128),
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        };
        let p = stage_params(&sh, 0, 1.0).unwrap();
        assert_eq!(p.flags, STAGE_FLAG_ALPHAFUNC_LT128);
        assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.turb01[2..], [0.0, 0.0]);
        assert_eq!(p.vec1_s, [0.0; 4]);
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
        let sh = Shader {
            name: "test/turb".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: false,
                    tcmods: tcmods.clone(),
                    vector: None,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        };
        let p = stage_params(&sh, 0, 4.0).unwrap();
        assert_eq!(p.uv0, bundle_affine(&tcmods, 4.0));
        assert_eq!(p.turb01, [3.0, 0.1 + 0.5 * 4.0, 0.0, 0.0]);
    }

    #[test]
    fn stage_params_out_of_range_stage_is_none() {
        let sh = Shader::default();
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
    ) -> Shader {
        Shader {
            name: "test/script".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: false,
                    tcmods: vec![],
                    vector: None,
                }],
                blend,
                depth_write: None,
                alpha_func: None,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            sort,
            polygon_offset: offset,
            ..Default::default()
        }
    }

    #[test]
    fn opaque_and_blend_variants_follow_class_and_depth_write() {
        let (v, _, _, band) = stage_draw_facts(&script(None, None, false), 0).unwrap();
        assert_eq!(v, StageVariant::Opaque);
        assert_eq!(band, BAND_OPAQUE);

        let alpha = Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha));
        // plain blends land in the back-to-front band
        let (v, _, _, band) = stage_draw_facts(&script(alpha.clone(), None, false), 0).unwrap();
        assert_eq!(v, StageVariant::Blend);
        assert_eq!(band, BAND_BLEND);

        // explicit depthWrite keeps writing
        let mut sh = script(alpha, None, false);
        sh.stages[0].depth_write = Some(true);
        assert_eq!(
            stage_draw_facts(&sh, 0).unwrap().0,
            StageVariant::BlendDepthWrite
        );

        // default sorts put unblended-but-offset scripts in the decal slot
        let (_, _, _, band) = stage_draw_facts(&script(None, None, true), 0).unwrap();
        assert_eq!(band, BAND_DECAL);
        let (_, _, _, band) =
            stage_draw_facts(&script(None, Some(SORT_DECAL - 1.0), false), 0).unwrap();
        assert_eq!(band, BAND_OPAQUE);
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
        // additives always emit last regardless of sort
        assert_eq!(stage_draw_facts(&sh, 0).unwrap().3, BAND_ADDITIVE);
    }

    #[test]
    fn sort_bands_split_seethrough_from_blend() {
        let alpha = Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha));
        for (sort, want) in [
            (Some(4.0), BAND_DECAL),
            (Some(4.5), BAND_SEETHROUGH),
            (Some(8.75), BAND_SEETHROUGH),
            (Some(9.0), BAND_BLEND),
            (None, BAND_BLEND),
        ] {
            let (_, _, _, band) = stage_draw_facts(&script(alpha.clone(), sort, false), 0).unwrap();
            assert_eq!(band, want, "sort {sort:?}");
        }
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
        let sh = Shader {
            name: "test/sky".into(),
            stages: vec![],
            sky: Some(SkyParms {
                env: "textures/skies/x".into(),
                cloud_height: 512.0,
            }),
            surface: bits.clone(),
            ..Default::default()
        };
        // either signal routes to Sky at load time
        assert!(sh.surface.sky || sh.sky.is_some());
        assert!(mesh::material_kind("textures/skies/x", true, false, true) == MaterialKind::Sky);
    }
}
