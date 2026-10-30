//! `vc_ssao`: screen-space ambient occlusion, a vcod-only extra (retail
//! has none). The frame's merged draw list is split after its last opaque
//! or decal draw; between the halves, `encode` computes GTAO at half
//! resolution from the MSAA depth and multiplies it into the MSAA colour, so
//! it darkens the lightmapped and ambient-lit surfaces drawn so far and
//! nothing that draws after: the blends, additives and fx, and the HUD. The
//! sky (cleared depth) and the viewmodel (its squeezed depth range) are
//! masked by depth. docs/research/cod11-gamma.md, section 6.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The half-resolution intermediates: linear depth, then (visibility, depth).
/// Depth stays f32: at half precision a flat floor's steps would occlude it.
const LINZ_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
/// The occlusion search radius, world units (a player is 70 tall).
const RADIUS: f32 = 40.0;
/// `vc_ssao` is the exponent on the visibility (1 is GTAO's own, which
/// lightmapped corners already darkened read as subtle); past this it only crushes.
const MAX_STRENGTH: f32 = 4.0;

/// `Params` in ssao.wgsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    proj: [f32; 4],
    size: [f32; 4],
    ao: [f32; 4],
    fog: [f32; 4],
}

/// What `Renderer::render` passes in each frame.
pub struct FrameParams {
    pub view_proj: glam::Mat4,
    pub near: f32,
    pub far: f32,
    /// The camera uniform's fog fields: mode, density, linear near and far.
    pub fog_mode: f32,
    pub fog_density: f32,
    pub fog_near: f32,
    pub fog_far: f32,
}

pub struct SsaoPass {
    layout: wgpu::BindGroupLayout,
    depth_pipeline: wgpu::RenderPipeline,
    ao_pipeline: wgpu::RenderPipeline,
    blur_h_pipeline: wgpu::RenderPipeline,
    blur_v_pipeline: wgpu::RenderPipeline,
    apply_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    targets: Targets,
    strength: f32,
    timer: Option<GpuTimer>,
}

/// Everything sized to the window.
struct Targets {
    size: (u32, u32),
    half: (u32, u32),
    linz: wgpu::TextureView,
    raw: wgpu::TextureView,
    blur_h: wgpu::TextureView,
    blurred: wgpu::TextureView,
    /// Reads the blurred AO: the depth pass (which ignores it) and the apply.
    bg_blurred: wgpu::BindGroup,
    bg_linz: wgpu::BindGroup,
    bg_raw: wgpu::BindGroup,
    bg_blur_h: wgpu::BindGroup,
}

impl SsaoPass {
    /// `depth` is the frame's MSAA depth view; it needs `TEXTURE_BINDING`.
    /// `timestamps` when the device has `TIMESTAMP_QUERY`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: &wgpu::TextureView,
        scene_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        timestamps: bool,
    ) -> SsaoPass {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: crate::renderer::msaa_samples() > 1,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ssao.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::depth_wgsl(include_str!(
                "ssao.wgsl"
            ))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ssao pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label, vs, entry, format, blend, samples, depth| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let half = |label, entry, format| pipeline(label, "vs_main", entry, format, None, 1, None);
        let depth_pipeline = half("ssao depth", "fs_depth", LINZ_FORMAT);
        let ao_pipeline = half("ssao ao", "fs_ao", AO_FORMAT);
        let blur_h_pipeline = half("ssao blur h", "fs_blur_h", AO_FORMAT);
        let blur_v_pipeline = half("ssao blur v", "fs_blur_v", AO_FORMAT);
        // dst * src on the colour, alpha kept
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let apply_pipeline = pipeline(
            "ssao apply",
            "vs_apply",
            "fs_apply",
            scene_format,
            Some(multiply),
            crate::renderer::msaa_samples(),
            Some(wgpu::DepthStencilState {
                format: crate::renderer::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ssao params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let targets = Targets::new(device, &layout, &uniform, depth, width, height);
        SsaoPass {
            layout,
            depth_pipeline,
            ao_pipeline,
            blur_h_pipeline,
            blur_v_pipeline,
            apply_pipeline,
            uniform,
            targets,
            strength: 0.0,
            timer: timestamps.then(|| GpuTimer::new(device, queue)),
        }
    }

    /// The window's new size, with the depth view recreated at it.
    pub fn resize(&mut self, device: &wgpu::Device, depth: &wgpu::TextureView, w: u32, h: u32) {
        self.targets = Targets::new(device, &self.layout, &self.uniform, depth, w, h);
    }

    /// `vc_ssao`; 0 (or less) turns the passes off.
    pub fn set_strength(&mut self, s: f32) {
        self.strength = if s.is_finite() {
            s.clamp(0.0, MAX_STRENGTH)
        } else {
            0.0
        };
    }

    pub fn strength(&self) -> f32 {
        self.strength
    }

    /// Writes the frame's uniform; call before the frame's encoder is submitted.
    pub fn prepare(&self, queue: &wgpu::Queue, f: &FrameParams) {
        if self.strength <= 0.0 {
            return;
        }
        // A perspective's x and y scales are the lengths of view_proj's
        // first two rows: the view is a rotation and a translation.
        let (w, h) = self.targets.size;
        let (hw, hh) = self.targets.half;
        let params = Params {
            proj: [
                f.view_proj.row(0).truncate().length(),
                f.view_proj.row(1).truncate().length(),
                f.near,
                f.far,
            ],
            size: [w as f32, h as f32, hw as f32, hh as f32],
            ao: [self.strength, RADIUS, f.fog_mode, f.fog_density],
            fog: [f.fog_near, f.fog_far, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
    }

    /// The five passes: half-res linear depth, AO, two blurs, and the multiply
    /// into `msaa_colour`, which must hold the frame so far (stored, not
    /// resolved). `depth` is the view bound at `new`/`resize`.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        msaa_colour: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let t = &self.targets;
        let timer = self.timer.as_ref();
        let half_pass =
            |encoder: &mut wgpu::CommandEncoder,
             target: &wgpu::TextureView,
             pipeline: &wgpu::RenderPipeline,
             bg: &wgpu::BindGroup,
             stamp: Option<wgpu::RenderPassTimestampWrites<'_>>| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ssao half pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: stamp,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..3, 0..1);
            };
        half_pass(
            encoder,
            &t.linz,
            &self.depth_pipeline,
            &t.bg_blurred,
            timer.map(|tm| tm.writes(Some(GpuTimer::AO_BEGIN), None)),
        );
        half_pass(encoder, &t.raw, &self.ao_pipeline, &t.bg_linz, None);
        half_pass(encoder, &t.blur_h, &self.blur_h_pipeline, &t.bg_raw, None);
        half_pass(
            encoder,
            &t.blurred,
            &self.blur_v_pipeline,
            &t.bg_blur_h,
            None,
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ssao apply"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: msaa_colour,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            // read-only, so the pass may sample it too
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            timestamp_writes: timer.map(|tm| tm.writes(None, Some(GpuTimer::AO_END))),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.apply_pipeline);
        pass.set_bind_group(0, &t.bg_blurred, &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn timer(&self) -> Option<&GpuTimer> {
        self.timer.as_ref()
    }

    pub fn timer_mut(&mut self) -> Option<&mut GpuTimer> {
        self.timer.as_mut()
    }
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        depth: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> Targets {
        let half = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
        let tex = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: half.0,
                        height: half.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let linz = tex("ssao depth", LINZ_FORMAT);
        let raw = tex("ssao raw", AO_FORMAT);
        let blur_h = tex("ssao blur h", AO_FORMAT);
        let blurred = tex("ssao blurred", AO_FORMAT);
        let bg = |input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao bind group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                ],
            })
        };
        Targets {
            size: (w, h),
            half,
            bg_blurred: bg(&blurred),
            bg_linz: bg(&linz),
            bg_raw: bg(&raw),
            bg_blur_h: bg(&blur_h),
            linz,
            raw,
            blur_h,
            blurred,
        }
    }
}

/// GPU timestamps around the scene passes and the SSAO passes, read back
/// a few frames late without stalling; the F3 `ssao` line.
pub struct GpuTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 idle, 1 copy submitted, 2 mapped, 3 waiting on the map.
    state: Arc<AtomicU8>,
    /// Whether the pending readback's frame ran the SSAO passes.
    pending_ao: bool,
    ns_per_tick: f32,
    /// Smoothed (scene, ssao) milliseconds; ssao `None` until measured.
    ms: (f32, Option<f32>),
}

impl GpuTimer {
    pub const SCENE_BEGIN: u32 = 0;
    pub const SCENE_END: u32 = 1;
    const AO_BEGIN: u32 = 2;
    const AO_END: u32 = 3;
    const COUNT: u32 = 4;

    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTimer {
        let bytes = u64::from(Self::COUNT) * 8;
        GpuTimer {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("frame timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: Self::COUNT,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp resolve"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            read: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("timestamp read"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(0)),
            pending_ao: false,
            ns_per_tick: queue.get_timestamp_period(),
            ms: (0.0, None),
        }
    }

    pub fn writes(
        &self,
        begin: Option<u32>,
        end: Option<u32>,
    ) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: begin,
            end_of_pass_write_index: end,
        }
    }

    /// After the frame's passes, before submit: copies the stamps out
    /// unless the previous copy is still being read.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder, ran_ao: bool) {
        if self.state.load(Ordering::Acquire) != 0 {
            return;
        }
        encoder.resolve_query_set(&self.set, 0..Self::COUNT, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, None);
        self.pending_ao = ran_ao;
        self.state.store(1, Ordering::Release);
    }

    /// After submit: maps a fresh copy, or folds in one the map finished.
    pub fn collect(&mut self, device: &wgpu::Device) {
        match self.state.load(Ordering::Acquire) {
            1 => {
                self.state.store(3, Ordering::Release);
                let state = self.state.clone();
                self.read.map_async(wgpu::MapMode::Read, .., move |r| {
                    state.store(if r.is_ok() { 2 } else { 0 }, Ordering::Release);
                });
            }
            2 => {
                let ticks: Vec<u64> = match self.read.get_mapped_range(..) {
                    Ok(view) => bytemuck::pod_collect_to_vec(&view[..]),
                    Err(_) => vec![0; Self::COUNT as usize],
                };
                self.read.unmap();
                self.state.store(0, Ordering::Release);
                let ms = |a: u32, b: u32| {
                    let (a, b) = (ticks[a as usize], ticks[b as usize]);
                    (b > a).then(|| (b - a) as f32 * self.ns_per_tick / 1.0e6)
                };
                let ease = |old: f32, new: f32| {
                    if old > 0.0 {
                        old * 0.9 + new * 0.1
                    } else {
                        new
                    }
                };
                if let Some(scene) = ms(Self::SCENE_BEGIN, Self::SCENE_END) {
                    self.ms.0 = ease(self.ms.0, scene);
                }
                self.ms.1 = if self.pending_ao {
                    ms(Self::AO_BEGIN, Self::AO_END)
                        .map(|ao| ease(self.ms.1.unwrap_or(0.0), ao))
                        .or(self.ms.1)
                } else {
                    None
                };
            }
            _ => {}
        }
        let _ = device.poll(wgpu::PollType::Poll);
    }

    /// Smoothed GPU ms: the scene passes (SSAO included), and the SSAO
    /// passes alone while they run.
    pub fn ms(&self) -> (f32, Option<f32>) {
        self.ms
    }
}
