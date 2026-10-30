//! CoD4's `resolvedPostSun`: a copy of the scene's colour taken at the
//! frame's split, after the opaque and decal draws and before the
//! effects, which the `distortion_*` effects bend (`fs_distort` in
//! fx.wgsl). Retail resolves it after the lit pass and before the
//! emissive one under `r_distortion` (cod4-effects.md 17).
//!
//! With MSAA the copy is the multisampled colour resolved by an empty
//! pass; single-sampled, a blit of the scene target. Group 2 of the fx
//! pass's distortion pipeline is the copy, the soft particles' linear
//! depth (the shader's `floatZ`) and a linear clamped sampler. Group 3 of
//! the world's feather and distortion pipelines (`renderer::cod4_worldfx`)
//! binds the same three at 5, 6 and 7, clear of the dlight pass's group 3
//! in shader.wgsl.

pub struct SceneCopy {
    /// Group 2 of `FxPass::distort_pipeline`.
    pub layout: wgpu::BindGroupLayout,
    /// Group 3 of the world fx pipelines.
    pub world_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    copy: wgpu::TextureView,
    bg: wgpu::BindGroup,
    world_bg: wgpu::BindGroup,
    /// The single-sample blit's group and the scene view it reads, kept
    /// while the frame copies the same target.
    blit_bg: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

impl SceneCopy {
    /// `format` is the world's target format, `linz` the soft particles'
    /// depth copy at the same size.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        linz: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> SceneCopy {
        let tex = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene copy layout"),
            entries: &[
                tex(1, false),
                tex(2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let world_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene copy world layout"),
            entries: &[
                tex(5, false),
                tex(6, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene copy blit layout"),
            entries: &[tex(0, false)],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_copy.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene_copy.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene copy blit pipeline layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene copy blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // `R_InitCodeImages`' sampler for it: linear, clamped
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene copy sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (copy, bg, world_bg) = targets(
            device,
            [&layout, &world_layout],
            &sampler,
            format,
            linz,
            width,
            height,
        );
        SceneCopy {
            layout,
            world_layout,
            world_bg,
            blit_bg: None,
            blit_layout,
            blit,
            sampler,
            format,
            copy,
            bg,
        }
    }

    /// The window's new size, with the soft particles' depth recreated.
    pub fn resize(&mut self, device: &wgpu::Device, linz: &wgpu::TextureView, w: u32, h: u32) {
        (self.copy, self.bg, self.world_bg) = targets(
            device,
            [&self.layout, &self.world_layout],
            &self.sampler,
            self.format,
            linz,
            w,
            h,
        );
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bg
    }

    /// Group 3 of the world fx pipelines.
    pub fn world_bind_group(&self) -> &wgpu::BindGroup {
        &self.world_bg
    }

    /// Copies `scene`, the frame's colour as it stands: the MSAA target
    /// when the frame is multisampled, else the single-sample one. Call
    /// between the frame's two halves, with no pass holding it.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
    ) {
        if crate::renderer::msaa_samples() > 1 {
            // an empty pass whose end resolves the samples into the copy
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene copy resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene,
                    depth_slice: None,
                    resolve_target: Some(&self.copy),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            return;
        }
        if self.blit_bg.as_ref().is_none_or(|(v, _)| v != scene) {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene copy blit"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                }],
            });
            self.blit_bg = Some((scene.clone(), bg));
        }
        let (_, bg) = self.blit_bg.as_ref().expect("built above");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene copy blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.copy,
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
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn targets(
    device: &wgpu::Device,
    [layout, world_layout]: [&wgpu::BindGroupLayout; 2],
    sampler: &wgpu::Sampler,
    format: wgpu::TextureFormat,
    linz: &wgpu::TextureView,
    w: u32,
    h: u32,
) -> (wgpu::TextureView, wgpu::BindGroup, wgpu::BindGroup) {
    let copy = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene copy"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene copy"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(linz),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&copy),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    let world_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene copy world"),
        layout: world_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(linz),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&copy),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (copy, bg, world_bg)
}
