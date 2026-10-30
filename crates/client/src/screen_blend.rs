//! The shellshock screen blend, retail's render commands 9 and 10
//! (CoDMP.exe `0x4d8620`, `0x4d86b0`; docs/research/cod11-shellshock.md,
//! "Screen blend"): each frame of a shock draws the last capture of the
//! finished 3D view over this one, then captures the result, so the view
//! trails. Both run on the gamma scene after the world's post effects and
//! before the HUD, in retail's byte space. CoD2 draws the capture through
//! its `shellshock` material (`shell_shock.hlsl`), a quarter of the way to
//! its luminance (docs/research/cod2-materials.md 9.10), and so does CoD4.
//! CoD4's flashbang draws its `shellshock_flashed` material instead
//! ([`ScreenBlend::flash`]): a still captured on the shock's first frame,
//! scaled and added to every later frame with white (cod4-client-view.md
//! 8).

use crate::renderer::SCENE_FORMAT;

pub struct ScreenBlend {
    blend: wgpu::RenderPipeline,
    flash: wgpu::RenderPipeline,
    copy: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    capture: wgpu::TextureView,
    /// Samples the capture, for the blend.
    from_capture: wgpu::BindGroup,
    /// Samples the scene, for the capture.
    from_scene: wgpu::BindGroup,
    /// The capture holds a frame (render command 9 has run since the last
    /// resize).
    held: bool,
}

impl ScreenBlend {
    pub fn new(device: &wgpu::Device, scene: &wgpu::TextureView, width: u32, height: u32) -> Self {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen blend layout"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen blend params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("screen_blend.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("screen blend pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let blend = pipeline("fs_blend", Some(wgpu::BlendState::ALPHA_BLENDING));
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let flash = pipeline(
            "fs_flash",
            Some(wgpu::BlendState {
                color: add,
                alpha: add,
            }),
        );
        let copy = pipeline("fs_copy", None);
        let capture = create_capture(device, width, height);
        let from_capture = bind(device, &layout, &capture, &params);
        let from_scene = bind(device, &layout, scene, &params);
        ScreenBlend {
            blend,
            flash,
            copy,
            layout,
            params,
            capture,
            from_capture,
            from_scene,
            held: false,
        }
    }

    /// The scene target was rebuilt; the old capture is gone with it.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        scene: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        self.capture = create_capture(device, width, height);
        self.from_capture = bind(device, &self.layout, &self.capture, &self.params);
        self.from_scene = bind(device, &self.layout, scene, &self.params);
        self.held = false;
    }

    /// Forgets the capture: a frame without the blend.
    pub fn drop_capture(&mut self) {
        self.held = false;
    }

    /// Render command 10 into `pass` (on the scene) at `alpha` (0..1),
    /// the capture taken `desaturate` of the way to its luminance (0 on
    /// CoD1, [`COD2_DESATURATE`] on CoD2); skipped with no capture or a
    /// zero alpha.
    pub fn blend(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        alpha: f32,
        desaturate: f32,
    ) {
        if !self.held || alpha <= 0.0 {
            return;
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[alpha, desaturate, 0.0, 0.0]),
        );
        pass.set_pipeline(&self.blend);
        pass.set_bind_group(0, &self.from_capture, &[]);
        pass.draw(0..3, 0..1);
    }

    /// CoD4's `RB_BlendSavedScreenFlashedCmd` into `pass`: the capture
    /// times `grab` plus `white`, added; skipped with no capture.
    pub fn flash(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        white: f32,
        grab: f32,
    ) {
        if !self.held {
            return;
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[grab, white, 0.0, 0.0]),
        );
        pass.set_pipeline(&self.flash);
        pass.set_bind_group(0, &self.from_capture, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Render command 9: the scene as it stands into the capture.
    pub fn capture(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("screen blend capture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.capture,
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
        });
        pass.set_pipeline(&self.copy);
        pass.set_bind_group(0, &self.from_scene, &[]);
        pass.draw(0..3, 0..1);
        self.held = true;
    }
}

/// `shell_shock.hlsl`'s `lerp(colour, luminance, 0.25)`.
pub const COD2_DESATURATE: f32 = 0.25;

/// Render command 10's alpha (CoDMP.exe `0x4d86b0`): `0.01` raised to the
/// wall time since the capture over the decay, capped at 0.99 and rounded
/// to a byte; nothing once the capture is older than the decay.
pub fn alpha(since_capture_ms: i32, decay_ms: i32) -> f32 {
    if decay_ms <= 0 || since_capture_ms >= decay_ms {
        return 0.0;
    }
    let a = 0.01f64
        .powf(f64::from(since_capture_ms) / f64::from(decay_ms))
        .min(f64::from(0.99f32));
    let byte = (a as f32 * 255.0 + 9.313226e-10).round_ties_even();
    byte / 255.0
}

fn create_capture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("screen blend capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    src: &wgpu::TextureView,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("screen blend bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 60 Hz frame under the 400 ms decay keeps 83% of the last frame;
    /// a capture the same millisecond keeps 0.99's byte.
    #[test]
    fn alpha_decays_to_a_hundredth_over_the_decay() {
        assert_eq!(alpha(0, 400), 252.0 / 255.0);
        let a = alpha(16, 400);
        assert_eq!(a, (0.01f64.powf(0.04) as f32 * 255.0).round() / 255.0);
        assert_eq!(alpha(400, 400), 0.0);
        assert_eq!(alpha(10, 0), 0.0);
    }

    /// CoD2's blend through `shell_shock.hlsl` on a software adapter: a
    /// capture of one image blended over another at a byte alpha matches
    /// `lerp(capture, luminance, 0.25)` drawn SrcAlpha, InvSrcAlpha.
    #[test]
    fn cod2_blend_desaturates_the_capture_as_the_hlsl_does() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Some(adapter) = pollster::block_on(instance.request_adapter(&Default::default())).ok()
        else {
            eprintln!("no adapter; skipped");
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        const N: u32 = 64;
        let scene = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: N,
                height: N,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = scene.create_view(&Default::default());
        let image = |seed: u32| -> Vec<u8> {
            (0..N * N * 4)
                .map(|i| ((i * 37 + seed * 101) % 256) as u8)
                .collect()
        };
        let put = |px: &[u8]| {
            queue.write_texture(
                scene.as_image_copy(),
                px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(N * 4),
                    rows_per_image: None,
                },
                scene.size(),
            )
        };
        let (shot, now) = (image(1), image(2));
        let mut blend = ScreenBlend::new(&device, &view, N, N);
        put(&shot);
        let mut enc = device.create_command_encoder(&Default::default());
        blend.capture(&mut enc);
        queue.submit([enc.finish()]);
        put(&now);
        let alpha = super::alpha(100, 400);
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            });
            blend.blend(&queue, &mut pass, alpha, COD2_DESATURATE);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(N * N * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            scene.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(N * 4),
                    rows_per_image: None,
                },
            },
            scene.size(),
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let got = readback.slice(..).get_mapped_range().unwrap().to_vec();
        let mut max_err = 0.0f32;
        for p in 0..(N * N) as usize {
            let c = |img: &[u8], k: usize| f32::from(img[p * 4 + k]) / 255.0;
            let lum = 0.299 * c(&shot, 0) + 0.587 * c(&shot, 1) + 0.114 * c(&shot, 2);
            for k in 0..3 {
                let src = c(&shot, k) + (lum - c(&shot, k)) * 0.25;
                let want = (src * alpha + c(&now, k) * (1.0 - alpha)) * 255.0;
                max_err = max_err.max((f32::from(got[p * 4 + k]) - want).abs());
            }
        }
        eprintln!("max error {max_err} of 255");
        assert!(max_err <= 1.0, "{max_err}");
    }
}
