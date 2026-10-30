//! `vc_softParticles`: effect sprites fade out where they meet the opaque
//! scene, a vcod-only extra (retail draws them with a plain depth test).
//!
//! The frame splits after its last opaque or decal draw (the same split
//! `vc_ssao` and `vc_shadows` use). There `encode` copies sample 0 of the
//! MSAA depth into a single-sample linear depth target, which the fx
//! pipelines read for every run after the split. The MSAA depth itself
//! cannot be bound there: the second half keeps it as a writable
//! attachment (blended models and depth-writing blend stages still write
//! it). Sample 0, not an average, so a silhouette pixel reads a real
//! surface. docs/research/cod11-gamma.md, section 8.
//!
//! The same copy is CoD2's `floatZ` for its `zfeather*` effects, which
//! retail fades by the depth gap (cod2-materials.md 9.5): the fx pass
//! splits and reads it for a feathered run whatever `vc_softParticles` is.

/// The copy's target: linear view depth, world units.
pub(crate) const LINZ_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Past this the value only washes sprites out.
const MAX_SCALE: f32 = 8.0;

/// `SoftParams` in fx.wgsl and `Params` in soft_particles.wgsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    /// near, far, `vc_softParticles`, unused
    p: [f32; 4],
}

pub struct SoftParticles {
    /// Group 2 of the soft fx pipelines: params and the linear depth.
    pub fx_layout: wgpu::BindGroupLayout,
    copy_layout: wgpu::BindGroupLayout,
    copy_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    linz: wgpu::TextureView,
    copy_bg: wgpu::BindGroup,
    fx_bg: wgpu::BindGroup,
    scale: f32,
}

impl SoftParticles {
    /// `depth` is the frame's MSAA depth view; it needs `TEXTURE_BINDING`.
    pub fn new(
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> SoftParticles {
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let copy_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("soft particles copy layout"),
            entries: &[
                uniform_entry,
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
            ],
        });
        let fx_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("soft particles fx layout"),
            entries: &[
                uniform_entry,
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
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
            label: Some("soft_particles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::depth_wgsl(include_str!(
                "soft_particles.wgsl"
            ))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("soft particles copy pipeline layout"),
            bind_group_layouts: &[Some(&copy_layout)],
            immediate_size: 0,
        });
        let copy_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("soft particles depth copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_copy"),
                compilation_options: Default::default(),
                targets: &[Some(LINZ_FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // near and far from the start: CoD2's feather reads the copy with
        // vc_softParticles off
        let uniform = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("soft particles params"),
                contents: bytemuck::bytes_of(&Params {
                    p: [crate::camera::Z_NEAR, crate::camera::Z_FAR, 0.0, 0.0],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let (linz, copy_bg, fx_bg) = targets(
            device,
            &copy_layout,
            &fx_layout,
            &uniform,
            depth,
            width,
            height,
        );
        SoftParticles {
            fx_layout,
            copy_layout,
            copy_pipeline,
            uniform,
            linz,
            copy_bg,
            fx_bg,
            scale: 0.0,
        }
    }

    /// The window's new size, with the depth view recreated at it.
    pub fn resize(&mut self, device: &wgpu::Device, depth: &wgpu::TextureView, w: u32, h: u32) {
        (self.linz, self.copy_bg, self.fx_bg) = targets(
            device,
            &self.copy_layout,
            &self.fx_layout,
            &self.uniform,
            depth,
            w,
            h,
        );
    }

    /// `vc_softParticles`; 0 (or less) turns the copy and the fade off.
    pub fn set_scale(&mut self, queue: &wgpu::Queue, s: f32) {
        let s = if s.is_finite() {
            s.clamp(0.0, MAX_SCALE)
        } else {
            0.0
        };
        if s == self.scale {
            return;
        }
        self.scale = s;
        let params = Params {
            p: [crate::camera::Z_NEAR, crate::camera::Z_FAR, s, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
    }

    pub fn on(&self) -> bool {
        self.scale > 0.0
    }

    pub fn fx_bind_group(&self) -> &wgpu::BindGroup {
        &self.fx_bg
    }

    /// The linear depth the copy writes, which `crate::scene_copy` binds
    /// as the distortion effects' `floatZ`.
    pub fn linz(&self) -> &wgpu::TextureView {
        &self.linz
    }

    /// Copies the MSAA depth as it stands into the linear target. Call
    /// between the frame's two halves, with no pass holding the depth.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("soft particles depth copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.linz,
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
        });
        pass.set_pipeline(&self.copy_pipeline);
        pass.set_bind_group(0, &self.copy_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn targets(
    device: &wgpu::Device,
    copy_layout: &wgpu::BindGroupLayout,
    fx_layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    depth: &wgpu::TextureView,
    w: u32,
    h: u32,
) -> (wgpu::TextureView, wgpu::BindGroup, wgpu::BindGroup) {
    let linz = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("soft particles depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LINZ_FORMAT,
            // copied out by the floatZ tests (cod4-client-perf.md 35)
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let bg = |label, layout, view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
            ],
        })
    };
    let copy_bg = bg("soft particles copy", copy_layout, depth);
    let fx_bg = bg("soft particles fx", fx_layout, &linz);
    (linz, copy_bg, fx_bg)
}
