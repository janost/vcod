//! `r_gamma` (the options Brightness slider). Retail loads a 256-entry
//! hardware gamma ramp; vcod draws the frame offscreen and maps it through
//! the same table in a final pass. Formula, clamps and addresses:
//! docs/research/cod11-gamma.md.

/// Retail's ramp shift with `r_overBrightBits 1` in full screen; vcod's
/// lighting already bakes this one bit into the frame.
const OVERBRIGHT_BITS: u32 = 1;

/// Retail's `r_gamma` clamp: values outside are written back to the cvar.
pub const GAMMA_MIN: f32 = 0.5;
pub const GAMMA_MAX: f32 = 3.0;

/// Retail's gamma table (CoDMP.exe 0x4f0780): entry i is
/// `ftol(255 * pow(i / 255, 1 / gamma) + 0.5) << shift`, clamped to 0..255;
/// gamma 1 skips the pow.
pub fn ramp(gamma: f32, shift: u32) -> [u8; 256] {
    let mut t = [0u8; 256];
    let inv255 = f64::from(1.0f32 / 255.0);
    for (i, out) in t.iter_mut().enumerate() {
        let v = if gamma == 1.0 {
            i as i64
        } else {
            (255.0 * (i as f64 * inv255).powf(1.0 / f64::from(gamma)) + 0.5) as i64
        };
        *out = (v << shift).clamp(0, 255) as u8;
    }
    t
}

/// Offscreen frame target and the pass that maps it through the ramp onto
/// the swapchain. Skipped at gamma 1, where the ramp is the identity on
/// vcod's frame.
pub struct GammaPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    ramp_tex: wgpu::Texture,
    ramp_view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    scene_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    gamma: f32,
}

impl GammaPass {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> GammaPass {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gamma layout"),
            entries: &[
                texture_entry(0, false),
                texture_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gamma ramp sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let ramp_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gamma ramp"),
            size: wgpu::Extent3d {
                width: 256,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let ramp_view = ramp_tex.create_view(&Default::default());

        let shader = device.create_shader_module(wgpu::include_wgsl!("gamma.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gamma pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let srgb = [("SRGB", if format.is_srgb() { 1.0 } else { 0.0 })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gamma pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &srgb,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let scene_view = create_scene_view(device, format, width, height);
        let bind_group = create_bind_group(device, &layout, &scene_view, &ramp_view, &sampler);
        GammaPass {
            pipeline,
            layout,
            sampler,
            ramp_tex,
            ramp_view,
            format,
            scene_view,
            bind_group,
            gamma: 1.0,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.scene_view = create_scene_view(device, self.format, width, height);
        self.bind_group = create_bind_group(
            device,
            &self.layout,
            &self.scene_view,
            &self.ramp_view,
            &self.sampler,
        );
    }

    /// Rebuilds the ramp when `gamma` changed; retail rebuilds it on the
    /// frame after `r_gamma` is modified.
    pub fn set_gamma(&mut self, queue: &wgpu::Queue, gamma: f32) {
        if gamma == self.gamma {
            return;
        }
        self.gamma = gamma;
        queue.write_texture(
            self.ramp_tex.as_image_copy(),
            &ramp(gamma, OVERBRIGHT_BITS),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 256,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The frame renders here instead of the swapchain while this is set.
    pub fn active(&self) -> bool {
        self.gamma != 1.0
    }

    pub fn scene_view(&self) -> &wgpu::TextureView {
        &self.scene_view
    }

    /// Maps the offscreen frame through the ramp onto `target`.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gamma pass"),
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
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_scene_view(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("gamma scene"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene: &wgpu::TextureView,
    ramp: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gamma bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(ramp),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At gamma 1 the table is the identity shifted by one bit, which is the
    /// x2 vcod's frame already carries: the pass would change nothing.
    #[test]
    fn gamma_one_is_the_overbright_shift() {
        let t = ramp(1.0, 1);
        assert_eq!(
            (t[0], t[64], t[127], t[128], t[255]),
            (0, 128, 254, 255, 255)
        );
        assert!(
            ramp(1.0, 0)
                .iter()
                .enumerate()
                .all(|(i, &v)| v as usize == i)
        );
    }

    /// Hand-computed rows of retail's formula.
    #[test]
    fn ramp_matches_retail_formula() {
        // 255 * (64/255)^(1/2) + 0.5 = 128.25 -> 128, << 1 = 256 -> 255
        assert_eq!(ramp(2.0, 1)[64], 255);
        // 255 * (16/255)^(1/2) + 0.5 = 64.37 -> 64, << 1 = 128
        assert_eq!(ramp(2.0, 1)[16], 128);
        // 255 * (100/255)^2 + 0.5 = 39.72 -> 39, << 1 = 78
        assert_eq!(ramp(0.5, 1)[100], 78);
        // 255 * (128/255)^(1/1.3) + 0.5 = 150.57 -> 150
        assert_eq!(ramp(1.3, 0)[128], 150);
        assert_eq!(ramp(3.0, 1)[0], 0);
    }
}
