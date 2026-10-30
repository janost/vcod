//! CoD4's sun shadow map (`sm_enable`, technique 9): each frame two
//! partitions fitted around the view (`vcod_cod4::sunshadow`) take the
//! casting world surfaces' and static models' depth from the sun into one
//! 1024 x 2048 atlas with the entities' casting surfaces, and the lit world,
//! static model and entity shaders read it in place of the baked sun
//! visibility (cod4_sunshadow_recv.wgsl;
//! docs/research/cod4-world.md 3.6).

use bytemuck::Zeroable;
use glam::{Mat4, Vec3};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vcod_cod4::sunshadow::{self, PARTITION, SunShadow, View};
use wgpu::util::DeviceExt;

/// The caster shaders.
const CAST_WGSL: &str = include_str!("cod4_sunshadow.wgsl");
/// The camera group's bindings of the atlas and its uniform.
pub(crate) const ATLAS_BINDING: u32 = 18;
pub(crate) const UNIFORM_BINDING: u32 = 19;
/// The receivers' comparison sampler over the atlas.
pub(crate) const SAMPLER_BINDING: u32 = 20;
/// Where the spot lights' tiles (crate::cod4_spotshadow) sit in the atlas:
/// the column right of the sun's partitions, four 512 tiles down it, their
/// uniform the tail of binding 19's.
/// One atlas, as the world pipeline has no sampled texture slot left.
pub(crate) const SPOT_X: u32 = PARTITION;
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The world vertex's stride (`mapworld::DrawVert`).
const VERTEX_STRIDE: u64 = 44;

/// cod4_sunshadow_recv.wgsl's `SunShadow`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ReceiveUniform {
    pub lookup: [[f32; 4]; 4],
    pub switch_partition: [f32; 4],
    pub scale: [f32; 4],
    pub on: [f32; 4],
}

impl ReceiveUniform {
    pub fn of(sh: &SunShadow) -> ReceiveUniform {
        ReceiveUniform {
            lookup: sh.lookup.to_cols_array_2d(),
            switch_partition: sh.switch_partition.to_array(),
            scale: sh.scale.to_array(),
            on: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// cod4_sunshadow_recv.wgsl's spot fields, past [`ReceiveUniform`] in the
/// same buffer: the pixel adjust, then a shadowed light a tile.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SpotUniform {
    /// `spotShadowmapPixelAdjust`.
    pub adjust: [f32; 4],
    pub tiles: [SpotTile; crate::cod4_spotshadow::MAX_LIGHTS],
}

/// One shadowed spot light in [`SpotUniform`].
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SpotTile {
    /// The world to the map's x, y and clip z (not over w).
    pub lookup: [[f32; 4]; 4],
    /// The light's origin, w one over its far plane.
    pub origin: [f32; 4],
    /// x 1 while drawn, y the fade (`lightSpotFactors.w`), z the tile.
    pub on: [f32; 4],
}

/// The receivers' uniform buffer's size: the sun's fields, then the spot's.
const RECEIVE_SIZE: u64 =
    (std::mem::size_of::<ReceiveUniform>() + std::mem::size_of::<SpotUniform>()) as u64;

/// cod4_sunshadow.wgsl's `Partition`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PartitionUniform {
    view_proj: [[f32; 4]; 4],
    offset: [f32; 4],
}

/// The receivers' camera group entries.
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: ATLAS_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: UNIFORM_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: SAMPLER_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
    ]
}

/// The receivers' sampler: `shadowmapSamplerSun`'s point filter and clamp
/// (cod4-client-perf.md 28), the depth compare in the sampler: a tap is 1
/// where the stored depth is at or past the lookup's.
pub(crate) fn receive_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("cod4 sun shadow compare"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    })
}

/// A 1 x 1 atlas and a zeroed uniform, the receivers' inputs with no map
/// drawn: a test's camera group (or one that wants a given uniform).
#[cfg(test)]
pub(crate) fn off_resources(
    device: &wgpu::Device,
    uniform: ReceiveUniform,
) -> (wgpu::TextureView, wgpu::Buffer) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cod4 sun shadow off"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut contents = bytemuck::bytes_of(&uniform).to_vec();
    contents.resize(RECEIVE_SIZE as usize, 0);
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("cod4 sun shadow off"),
        contents: &contents,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    (tex.create_view(&Default::default()), buf)
}

/// The camera group's entries over an atlas view, a uniform and
/// [`receive_sampler`].
pub(crate) fn entries<'a>(
    view: &'a wgpu::TextureView,
    uniform: &'a wgpu::Buffer,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 3] {
    [
        wgpu::BindGroupEntry {
            binding: ATLAS_BINDING,
            resource: wgpu::BindingResource::TextureView(view),
        },
        wgpu::BindGroupEntry {
            binding: UNIFORM_BINDING,
            resource: uniform.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: SAMPLER_BINDING,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

/// The casting soups of one loaded world, over its vertex buffer: the
/// solid ones, then each alpha-tested material's run and its colour map.
pub(crate) struct Casters {
    index_buf: wgpu::Buffer,
    solid: Vec<Run>,
    /// Each with its colour map's group in `cut_bgs`.
    cuts: Vec<(Run, usize)>,
    cut_bgs: Vec<wgpu::BindGroup>,
    /// The world's bounds, which fix the partitions' depth range.
    bounds: (Vec3, Vec3),
    tris: u32,
    /// Unique per build, so a spot tile's cached static depth knows its
    /// casters.
    id: u64,
    /// The static models, each drawn at the LOD the camera picks
    /// (`sunshadow::smodel_casters`, cod4-world.md 3.6).
    smodels: Vec<SmodelRuns>,
}

/// A run of a static model's casting triangles in [`Casters`]' index
/// buffer, and its colour map's group in `cut_bgs` when alpha-tested.
type SmodelRun = (std::ops::Range<u32>, Option<usize>);

/// A static model's casting runs per LOD in [`Casters`]' index buffer
/// (each with its colour map's group in `cut_bgs` when alpha-tested), its
/// placement's LOD pick and its bounds.
struct SmodelRuns {
    lod: vcod_common::mapworld::StaticLod,
    lods: Vec<Vec<SmodelRun>>,
    lo: Vec3,
    hi: Vec3,
}

/// [`Casters::id`]'s source.
static CASTERS_ID: AtomicU64 = AtomicU64::new(1);

/// What a spot tile's cached static depth was drawn for: the casters,
/// the light, `r_spotLightSModelShadows` and the polygon offset's bits.
#[derive(Clone, Copy, PartialEq)]
struct SpotStatic {
    casters: u64,
    light: usize,
    smodels: bool,
    offset: [u32; 2],
}

/// What a sun partition's cached static depth was drawn for: everything
/// that decides its texels, bit for bit.
#[derive(Clone, Copy, PartialEq)]
struct SunStatic {
    casters: u64,
    view_proj: [u32; 16],
    viewport: [u32; 4],
    offset: [u32; 4],
    cull: [[u32; 4]; 20],
    cull_count: usize,
}

impl SunStatic {
    fn of(casters: u64, part: &sunshadow::Partition) -> SunStatic {
        let bits = |v: glam::Vec4| v.to_array().map(f32::to_bits);
        SunStatic {
            casters,
            view_proj: part.view_proj.to_cols_array().map(f32::to_bits),
            viewport: part.viewport,
            offset: bits(part.polygon_offset),
            cull: part.cull.planes.map(bits),
            cull_count: part.cull.count,
        }
    }
}

/// The static models a cached static layer was drawn with: every one
/// whose bounds it keeps, and its LOD pick then. While the layer's other
/// inputs hold, the same models are kept, so the layer holds while their
/// picks do: no cull test and no hash a frame (cod4-client-perf.md 38).
struct Picked(Vec<(u32, Option<u8>)>);

impl Picked {
    fn holds(&self, picks: &[Option<u8>]) -> bool {
        self.0
            .iter()
            .all(|&(i, l)| picks.get(i as usize) == Some(&l))
    }
}

/// The static model runs of `c` to draw at `picks` (each placement's LOD,
/// `None` past its last or its `cullDist`) where `keep` holds for the
/// model's bounds, and which models those are at which LODs.
fn picked_smodels<'a>(
    c: &'a Casters,
    picks: &[Option<u8>],
    keep: impl Fn(Vec3, Vec3) -> bool,
) -> (Vec<&'a SmodelRun>, Picked) {
    let mut kept = Vec::new();
    let mut runs = Vec::new();
    for (i, (m, &pick)) in c.smodels.iter().zip(picks).enumerate() {
        if !keep(m.lo, m.hi) {
            continue;
        }
        kept.push((i as u32, pick));
        if let Some(l) = pick {
            runs.extend(m.lods[usize::from(l)].iter());
        }
    }
    (runs, Picked(kept))
}

/// Draws `runs` (solid ones through `solid`, alpha-tested ones through
/// `cut` with their image group) over the bound casters' buffers; returns
/// the triangles.
fn draw_runs(
    pass: &mut wgpu::RenderPass<'_>,
    c: &Casters,
    (solid, cut): (&wgpu::RenderPipeline, &wgpu::RenderPipeline),
    runs: &[&SmodelRun],
) -> u32 {
    let mut tris = 0;
    pass.set_pipeline(solid);
    for (r, _) in runs.iter().filter(|r| r.1.is_none()) {
        pass.draw_indexed(r.clone(), 0, 0..1);
        tris += r.len() as u32 / 3;
    }
    pass.set_pipeline(cut);
    let mut bound = None;
    for (r, bg) in runs.iter().filter_map(|r| Some((&r.0, r.1?))) {
        if bound != Some(bg) {
            pass.set_bind_group(1, &c.cut_bgs[bg], &[]);
            bound = Some(bg);
        }
        pass.draw_indexed(r.clone(), 0, 0..1);
        tris += r.len() as u32 / 3;
    }
    tris
}

/// Draws the entity casters `keep` passes into `pass` (its group 0 bound)
/// and returns the draws. A depth-only pass keeps the nearest depth
/// whatever the order, so state is set only where it changes and an
/// instance's next surface on the same buffers and image, its indices
/// following on, joins the last draw: the same triangles, fewer calls
/// (cod4-client-perf.md 38). The instance comes in as `first_instance`
/// over the whole instance buffer, not a slice per draw.
fn draw_entities(
    pass: &mut wgpu::RenderPass<'_>,
    e: &Entities<'_>,
    [solid, cut]: &[wgpu::RenderPipeline; 2],
    keep: impl Fn(&EntityCaster<'_>) -> bool,
) -> usize {
    pass.set_bind_group(1, e.bones, &[]);
    pass.set_vertex_buffer(1, e.instance_buf.slice(..));
    let mut calls = 0;
    let mut tested = None;
    let mut image: Option<*const wgpu::BindGroup> = None;
    let mut buffers: Option<(*const wgpu::Buffer, *const wgpu::Buffer)> = None;
    // the draw being joined: indices, base vertex, instance, and its key
    let mut open: Option<(std::ops::Range<u32>, i32, u32)> = None;
    let flush = |pass: &mut wgpu::RenderPass<'_>, open: &mut Option<_>, calls: &mut usize| {
        if let Some((r, base, inst)) = open.take() {
            pass.draw_indexed(r, base, inst..inst + 1);
            *calls += 1;
        }
    };
    for d in e.draws.iter().filter(|d| keep(d)) {
        let bufs = (
            std::ptr::from_ref(d.vertex_buf),
            std::ptr::from_ref(d.index_buf),
        );
        let img = d.cut.map(std::ptr::from_ref);
        if let Some((r, base, inst)) = &mut open
            && *inst == d.instance
            && *base == d.base_vertex
            && r.end == d.indices.start
            && buffers == Some(bufs)
            && tested == Some(img.is_some())
            && (img.is_none() || image == img)
        {
            r.end = d.indices.end;
            continue;
        }
        flush(pass, &mut open, &mut calls);
        if tested != Some(img.is_some()) {
            pass.set_pipeline(if img.is_some() { cut } else { solid });
            tested = Some(img.is_some());
        }
        if let Some(bg) = d.cut
            && image != img
        {
            pass.set_bind_group(2, bg, &[]);
            image = img;
        }
        if buffers != Some(bufs) {
            pass.set_vertex_buffer(0, d.vertex_buf.slice(..));
            pass.set_index_buffer(d.index_buf.slice(..), wgpu::IndexFormat::Uint16);
            buffers = Some(bufs);
        }
        open = Some((d.indices.clone(), d.base_vertex, d.instance));
    }
    flush(pass, &mut open, &mut calls);
    calls
}

/// Copies a spot tile's cached static depth into the atlas (a fullscreen
/// triangle under the tile's viewport, writing its depth as it stands).
const SPOT_BLIT_WGSL: &str = "
@group(0) @binding(0) var t_static: texture_depth_2d;
@vertex fn vs_blit(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(q * 2.0 - 1.0, 0.0, 1.0);
}
// the tile is the atlas's 512 square from x 1024 at a multiple of 512 down
@fragment fn fs_blit(@builtin(position) pos: vec4<f32>) -> @builtin(frag_depth) f32 {
    let p = vec2<i32>(i32(pos.x) - 1024, i32(pos.y) % 512);
    return textureLoad(t_static, p, 0);
}
// a sun partition: the atlas's 1024 square from x 0 at a multiple of 1024
@fragment fn fs_blit_sun(@builtin(position) pos: vec4<f32>) -> @builtin(frag_depth) f32 {
    let p = vec2<i32>(i32(pos.x), i32(pos.y) % 1024);
    return textureLoad(t_static, p, 0);
}
";

/// A group's run in [`Casters`]' index buffer and its bounds.
struct Run {
    range: std::ops::Range<u32>,
    lo: Vec3,
    hi: Vec3,
}

/// One entity surface that casts this frame: its model's buffers, its
/// index range and base vertex, and its slot in the instance buffer.
pub(crate) struct EntityCaster<'a> {
    pub vertex_buf: &'a wgpu::Buffer,
    pub index_buf: &'a wgpu::Buffer,
    pub indices: std::ops::Range<u32>,
    pub base_vertex: i32,
    pub instance: u32,
    /// An alpha-tested surface's colour map, in `Entities::image_layout`.
    pub cut: Option<&'a wgpu::BindGroup>,
    /// The instance's posed world box, when known: a spot tile skips a
    /// surface whose box its light's frustum culls.
    pub bounds: Option<(Vec3, Vec3)>,
}

/// The frame's entity casters and what they draw through: the instance
/// buffer and its stride, the bones' group and its layout, and the colour
/// maps' layout.
pub(crate) struct Entities<'a> {
    pub draws: Vec<EntityCaster<'a>>,
    pub instance_buf: &'a wgpu::Buffer,
    pub bones: &'a wgpu::BindGroup,
    pub bone_layout: &'a wgpu::BindGroupLayout,
    pub image_layout: &'a wgpu::BindGroupLayout,
}

/// GPU timestamps around the two partitions' passes.
struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Timestamps resolved and not yet read.
    pending: bool,
    /// The readback's map was asked for.
    mapping: bool,
    mapped: Arc<AtomicBool>,
    period_ns: f32,
}

/// The atlas, the receivers' uniform and the caster passes.
pub(crate) struct SunShadowPass {
    /// Read back by a test; the view keeps it alive otherwise.
    #[cfg_attr(not(test), allow(dead_code))]
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    receive_sampler: wgpu::Sampler,
    parts: [(wgpu::Buffer, wgpu::BindGroup); 2],
    part_layout: wgpu::BindGroupLayout,
    cut_layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    /// The skinned casters, solid and alpha-tested, made the first frame
    /// an entity casts.
    entity: Option<[wgpu::RenderPipeline; 2]>,
    solid: wgpu::RenderPipeline,
    cut: wgpu::RenderPipeline,
    timer: Option<Timer>,
    /// Smoothed GPU ms of both passes; 0 until measured.
    gpu_ms: f32,
    /// Whether this frame drew the map.
    drawn: bool,
    /// The world casters' triangles drawn into both partitions this
    /// frame, and all of them.
    tris: (u32, u32),
    /// Entity surfaces drawn into it this frame.
    entity_draws: usize,
    /// Each spot tile's pass uniform and group.
    spot_parts: [(wgpu::Buffer, wgpu::BindGroup); crate::cod4_spotshadow::MAX_LIGHTS],
    /// The shadowed lights, a tile each.
    spot: crate::cod4_spotshadow::History,
    /// The static casters' triangles the spot tiles redrew this frame.
    spot_tris: u32,
    /// Each spot tile's static casters' depth, drawn when its light (or
    /// what it draws) changes and copied into the atlas every frame
    /// before the entities (`spot_blit`); and what each was drawn for.
    spot_static: [(wgpu::TextureView, wgpu::BindGroup); crate::cod4_spotshadow::MAX_LIGHTS],
    spot_static_key: [Option<(SpotStatic, Picked)>; crate::cod4_spotshadow::MAX_LIGHTS],
    spot_blit: wgpu::RenderPipeline,
    /// Static tiles redrawn since the map's start.
    spot_redraws: u32,
    /// Each sun partition's static casters' depth, redrawn only when the
    /// partition's matrix, viewport, offset, cull planes or casters change
    /// (the origin is snapped to its texels, so a still or slow view keeps
    /// them), copied into the atlas every frame before the entities.
    sun_static: [(wgpu::TextureView, wgpu::BindGroup); 2],
    sun_static_key: [Option<(SunStatic, Picked)>; 2],
    sun_blit: wgpu::RenderPipeline,
    /// Partition static layers redrawn since the map's start.
    sun_redraws: u32,
    /// Each caster static model's LOD this frame ([`Self::pick_smodel_lods`]).
    picks: Vec<Option<u8>>,
}

impl SunShadowPass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> SunShadowPass {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cod4 sun shadow atlas"),
            size: wgpu::Extent3d {
                // the sun's two partitions, then the spot's tile
                width: PARTITION + crate::cod4_spotshadow::TILE,
                height: 2 * PARTITION,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ATLAS_FORMAT,
            // copied out by a test
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&Default::default());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cod4 sun shadow receive"),
            contents: &vec![0u8; RECEIVE_SIZE as usize],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let part_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod4 sun shadow partition"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let cut_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod4 sun shadow cut"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cod4 sun shadow cut"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let part_uniform = |label| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<PartitionUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &part_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            (buf, bg)
        };
        let spot_parts =
            [0; crate::cod4_spotshadow::MAX_LIGHTS].map(|_| part_uniform("cod4 spot shadow"));
        let static_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod4 spot static"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let spot_static = [0; crate::cod4_spotshadow::MAX_LIGHTS].map(|_| {
            let tile = crate::cod4_spotshadow::TILE;
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("cod4 spot static"),
                    size: wgpu::Extent3d {
                        width: tile,
                        height: tile,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: ATLAS_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cod4 spot static"),
                layout: &static_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            (view, bg)
        });
        let blit = |entry: &str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("cod4 spot blit"),
                source: wgpu::ShaderSource::Wgsl(SPOT_BLIT_WGSL.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cod4 spot blit"),
                bind_group_layouts: &[Some(&static_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cod4 spot blit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_blit"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: ATLAS_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let spot_blit = blit("fs_blit");
        let sun_blit = blit("fs_blit_sun");
        // each sun partition's static casters, as the spot tiles' cache
        let sun_static = [0, 1].map(|_| {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("cod4 sun static"),
                    size: wgpu::Extent3d {
                        width: PARTITION,
                        height: PARTITION,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: ATLAS_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cod4 sun static"),
                layout: &static_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            (view, bg)
        });
        let parts = [0, 1].map(|p| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if p == 0 {
                    "cod4 sun shadow near"
                } else {
                    "cod4 sun shadow far"
                }),
                size: std::mem::size_of::<PartitionUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cod4 sun shadow partition"),
                layout: &part_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            (buf, bg)
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod4_sunshadow.wgsl"),
            source: wgpu::ShaderSource::Wgsl(CAST_WGSL.into()),
        });
        let pipeline = |cut: bool| {
            let groups = if cut {
                vec![Some(&part_layout), Some(&cut_layout)]
            } else {
                vec![Some(&part_layout)]
            };
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cod4 sun shadow caster"),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if cut {
                    "cod4 sun shadow cut caster"
                } else {
                    "cod4 sun shadow caster"
                }),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_cast"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(if cut { "fs_cast_cut" } else { "fs_cast" }),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                // every stock caster's state draws both faces (`Cull::None`)
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: ATLAS_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        SunShadowPass {
            atlas,
            atlas_view,
            uniform,
            receive_sampler: receive_sampler(device),
            parts,
            solid: pipeline(false),
            cut: pipeline(true),
            part_layout,
            cut_layout,
            module,
            entity: None,
            timer: Timer::new(device, queue),
            gpu_ms: 0.0,
            drawn: false,
            tris: (0, 0),
            entity_draws: 0,
            spot_parts,
            spot: Default::default(),
            spot_tris: 0,
            spot_static,
            spot_static_key: Default::default(),
            spot_blit,
            spot_redraws: 0,
            sun_static,
            sun_static_key: [None, None],
            sun_blit,
            sun_redraws: 0,
            picks: Vec::new(),
        }
    }

    /// The skinned caster pipeline over the entities' vertex layouts, the
    /// alpha-tested one reading a colour map in `image`'s layout.
    fn entity_pipeline(
        &self,
        device: &wgpu::Device,
        bones: &wgpu::BindGroupLayout,
        image: Option<&wgpu::BindGroupLayout>,
    ) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod4 sun shadow entity caster"),
            bind_group_layouts: &[Some(&self.part_layout), Some(bones), image],
            immediate_size: 0,
        });
        let [vertex, instance] = crate::entity_material::vertex_layouts();
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cod4 sun shadow entity caster"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_cast_entity"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex), Some(instance)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some(if image.is_some() {
                    "fs_cast_entity_cut"
                } else {
                    "fs_cast"
                }),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: ATLAS_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The camera group's three entries.
    pub fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        entries(&self.atlas_view, &self.uniform, &self.receive_sampler)
    }

    /// The entity caster pipelines, made the first frame an entity casts.
    fn ensure_entity(&mut self, device: &wgpu::Device, e: &Entities<'_>) {
        if self.entity.is_none() {
            self.entity = Some([
                self.entity_pipeline(device, e.bone_layout, None),
                self.entity_pipeline(device, e.bone_layout, Some(e.image_layout)),
            ]);
        }
    }

    /// The casters of `bsp`'s world and static model soups (`soup_model` 0)
    /// whose material casts, grouped (`sunshadow::caster_groups`), `None` on a map with no caster list (not
    /// CoD4). `image` uploads an alpha-tested caster's colour map.
    pub fn casters(
        &self,
        device: &wgpu::Device,
        bsp: &vcod_common::mapworld::MapWorld,
        soup_model: &[u32],
        mut image: impl FnMut(&str) -> wgpu::TextureView,
    ) -> Option<Casters> {
        if bsp.sun_casters.is_empty() {
            return None;
        }
        let groups = sunshadow::caster_groups(bsp, soup_model, sunshadow::CASTER_CELL);
        let mut tested = 0;
        let mut indices = Vec::new();
        let (mut solid, mut cuts) = (Vec::new(), Vec::new());
        let mut cut_bgs: Vec<wgpu::BindGroup> = Vec::new();
        let mut last_cut = None;
        for g in groups {
            let cut = g.cut;
            let first = indices.len() as u32;
            indices.extend(g.indices);
            let run = Run {
                range: first..indices.len() as u32,
                lo: g.lo,
                hi: g.hi,
            };
            let Some(m) = cut else {
                solid.push(run);
                continue;
            };
            if last_cut != Some(m) {
                let Some(Some(c)) = bsp.sun_casters.get(usize::from(m)) else {
                    continue;
                };
                let view = image(&c.image);
                cut_bgs.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("cod4 sun shadow cut image"),
                    layout: &self.cut_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    }],
                }));
                last_cut = Some(m);
                tested += 1;
            }
            cuts.push((run, cut_bgs.len() - 1));
        }
        // the static models' runs, their cut images shared with the world's
        let mut cut_of: std::collections::HashMap<u16, usize> = Default::default();
        let mut smodels = Vec::new();
        for sm in sunshadow::smodel_casters(bsp) {
            let mut lods = Vec::with_capacity(sm.lods.len());
            for runs in sm.lods {
                let mut out = Vec::with_capacity(runs.len());
                for (cut, idx) in runs {
                    let bg = match cut {
                        None => None,
                        Some(m) => {
                            if let Some(&b) = cut_of.get(&m) {
                                Some(b)
                            } else {
                                let Some(Some(c)) = bsp.sun_casters.get(usize::from(m)) else {
                                    continue;
                                };
                                let view = image(&c.image);
                                cut_bgs.push(device.create_bind_group(
                                    &wgpu::BindGroupDescriptor {
                                        label: Some("cod4 sun shadow cut image"),
                                        layout: &self.cut_layout,
                                        entries: &[wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::TextureView(&view),
                                        }],
                                    },
                                ));
                                tested += 1;
                                cut_of.insert(m, cut_bgs.len() - 1);
                                Some(cut_bgs.len() - 1)
                            }
                        }
                    };
                    let first = indices.len() as u32;
                    indices.extend(idx);
                    out.push((first..indices.len() as u32, bg));
                }
                lods.push(out);
            }
            smodels.push(SmodelRuns {
                lod: sm.lod,
                lods,
                lo: sm.lo,
                hi: sm.hi,
            });
        }
        if indices.is_empty() {
            return None;
        }
        let m0 = bsp.models.first()?;
        let tris = indices.len() as u32 / 3;
        log::info!(
            "cod4 sun shadow casters: {tris} triangles in {} cells, {tested} alpha-tested materials",
            solid.len() + cuts.len()
        );
        Some(Casters {
            id: CASTERS_ID.fetch_add(1, Ordering::Relaxed),
            smodels,
            index_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cod4 sun shadow casters"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            }),
            solid,
            cuts,
            cut_bgs,
            bounds: (Vec3::from(m0.mins), Vec3::from(m0.maxs)),
            tris,
        })
    }

    /// Fits the partitions around the view (`view_proj` from `eye` along
    /// `fwd`, near plane `z_near`) under a sun toward `to_sun`, draws the
    /// casters over their world's vertex buffer into the atlas and points
    /// the receivers at it. `None` for `casters` turns the receivers off.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        casters: Option<(&Casters, &wgpu::Buffer)>,
        entities: Option<&Entities<'_>>,
        (view_proj, eye, fwd, z_near): (Mat4, Vec3, Vec3, f32),
        to_sun: Vec3,
        settings: &sunshadow::Settings,
    ) {
        let Some((c, vertex_buf)) = casters else {
            if self.drawn {
                queue.write_buffer(
                    &self.uniform,
                    0,
                    bytemuck::bytes_of(&ReceiveUniform::zeroed()),
                );
            }
            self.drawn = false;
            self.sun_static_key = [None, None];
            self.tris = (0, 0);
            self.entity_draws = 0;
            return;
        };
        let entities = entities.filter(|e| !e.draws.is_empty());
        if let Some(e) = entities {
            self.ensure_entity(device, e);
        }
        let view = View::of(view_proj, eye, fwd, z_near);
        let sh = sunshadow::setup(&view, to_sun, c.bounds, settings);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&ReceiveUniform::of(&sh)),
        );
        for (p, part) in sh.partitions.iter().enumerate() {
            let mut offset = part.polygon_offset.to_array();
            // depth over w, half a 1024 texel
            offset[2] = 0.0;
            offset[3] = 1.0 / PARTITION as f32;
            let u = PartitionUniform {
                view_proj: part.view_proj.to_cols_array_2d(),
                offset,
            };
            queue.write_buffer(&self.parts[p].0, 0, bytemuck::bytes_of(&u));
        }
        let timer = self.timer.as_ref().filter(|t| !t.pending);
        let mut drawn = 0;
        let mut calls = 0;
        // Each partition's static casters into its own cache when what
        // decides their texels changed: the 1024 square, the casters cut
        // to the viewport retail fits it in, the rest cleared (lit). Then
        // one pass a partition copies the cache into the atlas and draws
        // the entities over it.
        for (p, part) in sh.partitions.iter().enumerate() {
            let key = SunStatic::of(c.id, part);
            if let Some((k, picked)) = &self.sun_static_key[p]
                && *k == key
                && picked.holds(&self.picks)
            {
                continue;
            }
            let (smodel_runs, picked) =
                picked_smodels(c, &self.picks, |lo, hi| !part.cull.culls(lo, hi));
            self.sun_static_key[p] = Some((key, picked));
            self.sun_redraws += 1;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cod4 sun static"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.sun_static[p].0,
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
            let [vx, vy, vw, vh] = part.viewport;
            if vw == 0 || vh == 0 {
                continue;
            }
            pass.set_scissor_rect(vx, vy, vw.min(PARTITION - vx), vh.min(PARTITION - vy));
            pass.set_bind_group(0, &self.parts[p].1, &[]);
            pass.set_vertex_buffer(0, vertex_buf.slice(..));
            pass.set_index_buffer(c.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            let cull = &part.cull;
            pass.set_pipeline(&self.solid);
            for r in c.solid.iter().filter(|r| !cull.culls(r.lo, r.hi)) {
                pass.draw_indexed(r.range.clone(), 0, 0..1);
                drawn += r.range.len() as u32 / 3;
            }
            pass.set_pipeline(&self.cut);
            let mut bound = None;
            for (r, bg) in c.cuts.iter().filter(|(r, _)| !cull.culls(r.lo, r.hi)) {
                if bound != Some(*bg) {
                    pass.set_bind_group(1, &c.cut_bgs[*bg], &[]);
                    bound = Some(*bg);
                }
                pass.draw_indexed(r.range.clone(), 0, 0..1);
                drawn += r.range.len() as u32 / 3;
            }
            drawn += draw_runs(&mut pass, c, (&self.solid, &self.cut), &smodel_runs);
        }
        for (p, part) in sh.partitions.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cod4 sun shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.atlas_view,
                    depth_ops: Some(wgpu::Operations {
                        load: if p == 0 {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: timer.map(|t| wgpu::RenderPassTimestampWrites {
                    query_set: &t.queries,
                    beginning_of_pass_write_index: (p == 0).then_some(0),
                    end_of_pass_write_index: (p == 1).then_some(1),
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let y = p as u32 * PARTITION;
            pass.set_viewport(0.0, y as f32, PARTITION as f32, PARTITION as f32, 0.0, 1.0);
            let [vx, vy, vw, vh] = part.viewport;
            if vw == 0 || vh == 0 {
                continue;
            }
            pass.set_scissor_rect(vx, y + vy, vw.min(PARTITION - vx), vh.min(PARTITION - vy));
            pass.set_pipeline(&self.sun_blit);
            pass.set_bind_group(0, &self.sun_static[p].1, &[]);
            pass.draw(0..3, 0..1);
            calls += 1;
            // an entity whose posed box the partition's planes cull casts
            // nothing into it
            let cull = &part.cull;
            if let (Some(e), Some(pipes)) = (entities, &self.entity) {
                pass.set_bind_group(0, &self.parts[p].1, &[]);
                calls += draw_entities(&mut pass, e, pipes, |d| {
                    d.bounds.is_none_or(|(lo, hi)| !cull.culls(lo, hi))
                });
            }
        }
        crate::frame_prof::count(crate::frame_prof::Count::ShadowDraws, calls);
        self.entity_draws = entities.map_or(0, |e| e.draws.len());
        if let Some(t) = self.timer.as_mut().filter(|t| !t.pending) {
            encoder.resolve_query_set(&t.queries, 0..2, &t.resolve, 0);
            encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.readback, 0, None);
            t.pending = true;
            t.mapped.store(false, Ordering::Release);
        }
        self.drawn = true;
        self.tris = (drawn, c.tris);
    }

    /// Each caster static model's LOD from the camera at `eye` through
    /// the rigid ramp `(scale, bias)`, as the sun and spot shadow passes
    /// pick it (iw3mp.exe 0x63b3d0, cod4-world.md 9.10, 3.6). Call before
    /// [`Self::encode`] each frame.
    pub fn pick_smodel_lods(&mut self, casters: Option<&Casters>, eye: Vec3, ramp: (f32, f32)) {
        self.picks.clear();
        if let Some(c) = casters {
            self.picks.extend(c.smodels.iter().map(|m| {
                m.lod
                    .pick(eye.into(), ramp.0, ramp.1)
                    .and_then(|l| u8::try_from(l).ok())
            }));
        }
    }

    /// The spot lights' shadow maps (crate::cod4_spotshadow): steps the
    /// history of shadowed lights over `lights` (the candidates the spots
    /// `used` marks, scored from `eye` along `fwd`, `time` seconds), draws
    /// `casters` and `entities` from each into its tile of the atlas (after
    /// [`Self::encode`], whose clear it keeps) and points the receivers at
    /// them; with `settings.on` false or no casters the history empties
    /// and the receivers read none.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_spot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        casters: Option<(&Casters, &wgpu::Buffer)>,
        entities: Option<&Entities<'_>>,
        (lights, used): (&[Option<vcod_common::mapworld::ModelLightSource>], &[bool]),
        (eye, fwd, time): (Vec3, Vec3, f32),
        settings: &crate::cod4_spotshadow::Settings,
    ) {
        use crate::cod4_spotshadow as spot;
        let off = std::mem::size_of::<ReceiveUniform>() as u64;
        let Some((c, vertex_buf)) = casters.filter(|_| settings.on) else {
            if !self.spot.entries.is_empty() {
                queue.write_buffer(
                    &self.uniform,
                    off,
                    bytemuck::bytes_of(&SpotUniform::zeroed()),
                );
            }
            self.spot = Default::default();
            self.spot_static_key = Default::default();
            self.spot_tris = 0;
            return;
        };
        let before: Vec<usize> = self.spot.entries.iter().map(|e| e.light).collect();
        let picked = spot::candidates(lights, used, eye, fwd, settings);
        self.spot.update(&picked, used, time, settings);
        let now: Vec<usize> = self.spot.entries.iter().map(|e| e.light).collect();
        if now != before {
            log::debug!("cod4 spot shadow: lights {now:?}");
        }
        let entities = entities.filter(|e| settings.entities && !e.draws.is_empty());
        if let Some(e) = entities {
            self.ensure_entity(device, e);
        }
        let mut u = SpotUniform {
            adjust: spot::PIXEL_ADJUST,
            ..SpotUniform::zeroed()
        };
        let mut drawn = 0;
        let mut calls = 0;
        for (t, e) in self.spot.entries.iter().enumerate() {
            let Some(light) = lights.get(e.light).and_then(Option::as_ref) else {
                continue;
            };
            let v = spot::light_view(light, t as u32);
            u.tiles[t] = SpotTile {
                lookup: v.lookup.to_cols_array_2d(),
                origin: v.origin.extend(v.inv_far).to_array(),
                on: [1.0, e.fade, t as f32, 0.0],
            };
            // `shadowmapPolygonOffset`: (bias / 4, scale) in the stored
            // units (clip z), here over the far plane; half a 512 texel
            let part = PartitionUniform {
                view_proj: v.view_proj.to_cols_array_2d(),
                offset: [
                    settings.offset_bias * 0.25 * v.inv_far,
                    settings.offset_scale,
                    v.inv_far,
                    1.0 / spot::TILE as f32,
                ],
            };
            queue.write_buffer(&self.spot_parts[t].0, 0, bytemuck::bytes_of(&part));
            let size = spot::TILE;
            // the static casters (world surfaces and, under
            // `r_spotLightSModelShadows`, static models) only when the
            // tile's light or what it draws changed: they never move
            // `r_spotLightSModelShadows` 0 leaves the static models out
            let key = SpotStatic {
                casters: c.id,
                light: e.light,
                smodels: settings.smodels,
                offset: [
                    settings.offset_bias.to_bits(),
                    settings.offset_scale.to_bits(),
                ],
            };
            let held = matches!(&self.spot_static_key[t],
                Some((k, picked)) if *k == key && picked.holds(&self.picks));
            if !held {
                let (smodel_runs, picked) = picked_smodels(c, &self.picks, |lo, hi| {
                    settings.smodels && !spot::culls(&v.planes, lo, hi)
                });
                self.spot_static_key[t] = Some((key, picked));
                self.spot_redraws += 1;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("cod4 spot static"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.spot_static[t].0,
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
                pass.set_bind_group(0, &self.spot_parts[t].1, &[]);
                pass.set_vertex_buffer(0, vertex_buf.slice(..));
                pass.set_index_buffer(c.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                let keep = |r: &Run| !spot::culls(&v.planes, r.lo, r.hi);
                pass.set_pipeline(&self.solid);
                for r in c.solid.iter().filter(|r| keep(r)) {
                    pass.draw_indexed(r.range.clone(), 0, 0..1);
                    drawn += r.range.len() as u32 / 3;
                }
                pass.set_pipeline(&self.cut);
                let mut bound = None;
                for (r, bg) in c.cuts.iter().filter(|(r, _)| keep(r)) {
                    if bound != Some(*bg) {
                        pass.set_bind_group(1, &c.cut_bgs[*bg], &[]);
                        bound = Some(*bg);
                    }
                    pass.draw_indexed(r.range.clone(), 0, 0..1);
                    drawn += r.range.len() as u32 / 3;
                }
                drawn += draw_runs(&mut pass, c, (&self.solid, &self.cut), &smodel_runs);
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cod4 spot shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.atlas_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let y = t as u32 * size;
            pass.set_viewport(SPOT_X as f32, y as f32, size as f32, size as f32, 0.0, 1.0);
            pass.set_scissor_rect(SPOT_X, y, size, size);
            // the cached static depth, then the entities in the light's frustum
            pass.set_pipeline(&self.spot_blit);
            pass.set_bind_group(0, &self.spot_static[t].1, &[]);
            pass.draw(0..3, 0..1);
            calls += 1;
            if let (Some(e), Some(pipes)) = (entities, &self.entity) {
                pass.set_bind_group(0, &self.spot_parts[t].1, &[]);
                calls += draw_entities(&mut pass, e, pipes, |d| {
                    d.bounds
                        .is_none_or(|(lo, hi)| !spot::culls(&v.planes, lo, hi))
                });
            }
        }
        queue.write_buffer(&self.uniform, off, bytemuck::bytes_of(&u));
        crate::frame_prof::count(crate::frame_prof::Count::ShadowDraws, calls);
        self.spot_tris = drawn;
    }

    /// The shadowed spot lights and their fades, a tile each.
    #[cfg(test)]
    pub fn spots(&self) -> Vec<(usize, f32)> {
        self.spot
            .entries
            .iter()
            .map(|e| (e.light, e.fade))
            .collect()
    }

    /// The F3 line of the spot lights' maps.
    pub fn spot_line(&self) -> String {
        if self.spot.entries.is_empty() {
            return "spotshadow off".into();
        }
        let lights: Vec<String> = self
            .spot
            .entries
            .iter()
            .map(|e| format!("{}:{:.2}", e.light, e.fade))
            .collect();
        format!(
            "spotshadow lights {}  static {}k tris  redraws {}",
            lights.join(" "),
            self.spot_tris / 1000,
            self.spot_redraws
        )
    }

    /// Starts the timestamps' readback; call after the frame's submit.
    pub fn after_submit(&mut self) {
        let Some(t) = self.timer.as_mut().filter(|t| t.pending) else {
            return;
        };
        if t.mapped.load(Ordering::Acquire) {
            let ticks: Option<Vec<u64>> = t
                .readback
                .get_mapped_range(..)
                .ok()
                .map(|v| bytemuck::cast_slice(&v).to_vec());
            t.readback.unmap();
            t.mapped.store(false, Ordering::Release);
            t.pending = false;
            t.mapping = false;
            if let Some(ticks) = ticks {
                let ms = ticks[1].wrapping_sub(ticks[0]) as f32 * t.period_ns / 1e6;
                self.gpu_ms = if self.gpu_ms == 0.0 {
                    ms
                } else {
                    self.gpu_ms * 0.9 + ms * 0.1
                };
            }
            return;
        }
        if t.mapping {
            return;
        }
        t.mapping = true;
        let flag = t.mapped.clone();
        t.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            if r.is_ok() {
                flag.store(true, Ordering::Release);
            }
        });
    }

    /// The F3 line: on or off, the world casters' triangles drawn into both
    /// partitions of all, the entity surfaces drawn into each, and both
    /// passes' GPU ms.
    pub fn debug_line(&self) -> String {
        if !self.drawn {
            return "sunshadow off".into();
        }
        let gpu = if self.timer.is_some() {
            format!("{:.2}ms", self.gpu_ms)
        } else {
            "-".into()
        };
        format!(
            "sunshadow sm_enable  casters {}k/{}k tris + {} ents  redraws {}  gpu {gpu}",
            self.tris.0 / 1000,
            self.tris.1 / 1000,
            self.entity_draws,
            self.sun_redraws
        )
    }

    /// The partitions' static layers redrawn so far, for a test.
    #[cfg(test)]
    pub fn sun_redraws(&self) -> u32 {
        self.sun_redraws
    }

    /// Whether the last encode drew the sun's atlas, and the sun and spot
    /// static layers redrawn so far, for the frame write log.
    #[cfg(test)]
    pub fn redraws(&self) -> (bool, u32, u32) {
        (self.drawn, self.sun_redraws, self.spot_redraws)
    }

    /// The atlas, for a test reading it back.
    #[cfg(test)]
    pub fn atlas(&self) -> &wgpu::Texture {
        &self.atlas
    }
}

impl Timer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Timer> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        Some(Timer {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("cod4 sun shadow timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cod4 sun shadow resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cod4 sun shadow readback"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            pending: false,
            mapping: false,
            mapped: Arc::new(AtomicBool::new(false)),
            period_ns: queue.get_timestamp_period(),
        })
    }
}
