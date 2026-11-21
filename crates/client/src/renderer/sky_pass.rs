//! The map's sky: the farbox cube, the cloud dome's stages and the sunfile
//! sprite.

use super::*;
use crate::renderer::gpu_util;

/// One cloud-dome draw: a stage of the active sky block over the whole dome
/// mesh, through the shared stage pipelines and params slots.
pub(super) struct DomeStage {
    pub(super) variant: StageVariant,
    pub(super) two_sided: bool,
    pub(super) slot: u32,
    pub(super) mat: AnimFrames,
    pub(super) stage: AnimFrames,
}

pub(super) struct DomeLayer {
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer,
    pub(super) index_count: u32,
    pub(super) stages: Vec<DomeStage>,
}

/// The farbox cube. Absent from [`SkyRender`] when the env art is missing:
/// RTCW-MP gates the whole box on `outerbox[0]` having loaded
/// (RB_StageIteratorSky tr_sky.c :994, cod11-shader-scripts.md 6) and draws
/// nothing otherwise; unverified against retail.
pub(super) struct SkyBox {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) index_buf: wgpu::Buffer,
    /// View origin per frame; the cube never rotates (sky.wgsl).
    pub(super) eye_buf: wgpu::Buffer,
    pub(super) eye_bg: wgpu::BindGroup,
    /// Per-axis bind groups in [`sky::AXIS_SUFFIX`] order.
    pub(super) side_bgs: Vec<wgpu::BindGroup>,
    /// Index ranges per axis into `index_buf`.
    pub(super) sides: [(u32, u32); 6],
    /// A CoD2 sky's cube, which the glow's sky bleed samples too.
    pub(super) cube: Option<wgpu::TextureView>,
}

/// The map's active sky: the farbox cube plus optional cloud dome. Built at
/// load, drawn FIRST every frame with depth-always / write-off / cull-off so
/// the world overdraws it everywhere geometry exists (retail draws at depth
/// range 1.0 before everything; same net effect).
pub(super) struct SkyRender {
    /// Chosen block name, for the F3 overlay and the load log.
    pub(super) name: String,
    pub(super) farbox: Option<SkyBox>,
    pub(super) dome: Option<DomeLayer>,
    /// `sunfile` glow sprite; needs a resolvable `.sun` sprite image and a
    /// lump-30 sun light.
    pub(super) sun: Option<SunSprite>,
    /// The `.sun` file's flare and screen washes (`sun_flare.rs`).
    pub(super) flare: Option<SunFlareGpu>,
}

/// The `sunfile` disc: an additive billboard along `bsp::sun_direction`
/// at farbox distance, rebuilt per frame around the view origin.
pub(super) struct SunSprite {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) vertex_buf: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) dir: glam::Vec3,
    pub(super) half_extent: f32,
}

/// Builds the farbox for the block's env name. Missing non-rt sides draw
/// flat white, warned per load: RTCW-MP's tr.defaultImage (tr_shader.c:1170),
/// unverified against retail.
#[expect(clippy::too_many_arguments)]
pub(super) fn build_sky_box(
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
        entries: &[gpu_util::texture_2d(0)],
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
        entries: &[gpu_util::uniform(0, wgpu::ShaderStages::VERTEX)],
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
pub(super) const SUN_SPRITE_SCALE: f32 = 0.001_311_092_9;

/// Builds the sunfile sprite for a parsed `.sun` file along the sun
/// direction. `None` when the sprite image does not resolve.
#[expect(clippy::too_many_arguments)]
pub(super) fn build_sun_sprite(
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
        entries: &[gpu_util::texture_2d(0)],
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

impl Renderer {
    /// Name of the map's active sky block, for the F3 `sky` field.
    pub fn sky_name(&self) -> Option<&str> {
        self.sky.as_ref().map(|s| s.name.as_str())
    }
}
