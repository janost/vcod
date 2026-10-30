//! Post-processing before the HUD: a CoD4 vision set's film tweak
//! (retail's `postfx_color`, docs/research/cod4-world.md 11) and its glow
//! added after it (`glow_apply_bloom`, the chain in `crate::cod2_glow`),
//! and the
//! vcod-only `vc_hdr` tonemap from the float world target, `vc_grade`,
//! `vc_vignette` and `vc_grain`. While any is on, the map pass resolves into this pass's world
//! target, the pass writes retail's framebuffer bytes into the gamma pass's
//! scene, and the HUD draws over that, so none of it touches the HUD. With
//! all four off it never runs and the frame is the retail path
//! (docs/research/cod11-gamma.md, section 9).

use crate::renderer::SCENE_FORMAT;

/// The user's settings, read every frame (`vc_hdr` only at renderer build).
#[derive(Clone, Copy, Default, PartialEq)]
pub struct PostSettings {
    pub grade: f32,
    pub vignette: f32,
    pub grain: f32,
}

impl PostSettings {
    /// Whether the pass runs: `vc_hdr` always needs the tonemap.
    pub fn active(&self, hdr: bool) -> bool {
        hdr || self.grade > 0.0 || self.vignette > 0.0 || self.grain > 0.0
    }
}

/// A CoD4 film tweak's `postfx_color` constants
/// (`vcod_cod4::look::FilmConsts`): `colorBias`, `colorTintBase`,
/// `colorTintDelta`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Film {
    pub bias: [f32; 4],
    pub base: [f32; 3],
    pub delta: [f32; 3],
}

impl Film {
    /// `R_UpdateColorManipulation`'s stores with the film off (iw3mp.exe
    /// 0x5f8e9c-0x5f8eec), which the glow's setup still reads.
    pub const OFF: Film = Film {
        bias: [0.0, 0.0, 0.0, 4095.0],
        base: [1.0 / 4096.0; 3],
        delta: [0.0; 3],
    };

    /// The glow setup's constants (`cod2_glow::Plan::film`), identityLight
    /// in the base's w.
    pub fn setup(&self, identity_light: f32) -> [[f32; 4]; 3] {
        let [b0, b1, b2] = self.base;
        let [d0, d1, d2] = self.delta;
        [self.bias, [b0, b1, b2, identity_light], [d0, d1, d2, 0.0]]
    }
}

/// Grain changes this many times a second, film's frame rate.
const GRAIN_RATE: f32 = 24.0;

pub struct PostPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    params: wgpu::Buffer,
    world: wgpu::Texture,
    world_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    /// Group 1: the CoD4 glow's quarter target, or `no_glow`.
    glow_layout: wgpu::BindGroupLayout,
    glow_sampler: wgpu::Sampler,
    no_glow: wgpu::TextureView,
    glow_group: wgpu::BindGroup,
    /// The glow target `glow_group` binds, `None` for `no_glow`: a frame
    /// that binds the same one keeps the group.
    glow_bound: Option<wgpu::TextureView>,
}

impl PostPass {
    /// `format` is the world's (`Renderer::world_format`).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let glow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post glow layout"),
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
        let glow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post glow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_glow = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("post no glow"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let glow_group = create_glow_group(device, &glow_layout, &no_glow, &glow_sampler);
        let shader = device.create_shader_module(wgpu::include_wgsl!("post.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post pipeline layout"),
            bind_group_layouts: &[Some(&layout), Some(&glow_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post pipeline"),
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
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
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
        let world = create_world(device, format, width, height);
        let world_view = world.create_view(&Default::default());
        let bind_group = create_bind_group(device, &layout, &world_view, &params);
        PostPass {
            pipeline,
            layout,
            format,
            params,
            world,
            world_view,
            bind_group,
            glow_layout,
            glow_sampler,
            no_glow,
            glow_group,
            glow_bound: None,
        }
    }

    /// The CoD4 glow's result to add this frame (`Cod2Glow::result`), or
    /// none.
    pub fn bind_glow(&mut self, device: &wgpu::Device, glow: Option<&wgpu::TextureView>) {
        if glow == self.glow_bound.as_ref() {
            return;
        }
        self.glow_bound = glow.cloned();
        let view = glow.unwrap_or(&self.no_glow);
        self.glow_group = create_glow_group(device, &self.glow_layout, view, &self.glow_sampler);
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.world = create_world(device, self.format, width, height);
        self.world_view = self.world.create_view(&Default::default());
        self.bind_group = create_bind_group(device, &self.layout, &self.world_view, &self.params);
    }

    /// Where the map pass resolves while the pass is active.
    pub fn world_view(&self) -> &wgpu::TextureView {
        &self.world_view
    }

    /// The world target itself, which CoD4's depth of field copies.
    pub fn world(&self) -> &wgpu::Texture {
        &self.world
    }

    /// `film` runs first, on the display value, as retail's pass does on
    /// its framebuffer before anything vcod adds; then the bound glow at
    /// `glow` (`glowApply`'s intensity, 0 for none), as `glow_apply_bloom`
    /// adds into it.
    #[allow(clippy::too_many_arguments)]
    pub fn set_params(
        &self,
        queue: &wgpu::Queue,
        s: PostSettings,
        film: Option<Film>,
        glow: f32,
        hdr: bool,
        identity_light: f32,
        time: f32,
    ) {
        let seed = (time * GRAIN_RATE).floor() % 1024.0;
        let f = film.unwrap_or(Film {
            bias: [0.0; 4],
            base: [0.0; 3],
            delta: [0.0; 3],
        });
        let on = if film.is_some() { 1.0 } else { 0.0 };
        let p = [
            identity_light,
            if hdr { 1.0 } else { 0.0 },
            s.grade.clamp(0.0, 2.0),
            s.vignette.clamp(0.0, 2.0),
            s.grain.clamp(0.0, 2.0),
            seed,
            glow,
            0.0,
            f.bias[0],
            f.bias[1],
            f.bias[2],
            f.bias[3],
            f.base[0],
            f.base[1],
            f.base[2],
            on,
            f.delta[0],
            f.delta[1],
            f.delta[2],
            0.0,
        ];
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&p));
    }

    /// Writes the processed world into `pass`, the gamma scene's.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(1, &self.glow_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn create_world(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("post world"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_glow_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post glow bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    world: &wgpu::TextureView,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(world),
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

    /// All four off is the retail path: the pass never runs.
    #[test]
    fn only_hdr_or_a_strength_runs_the_pass() {
        let off = PostSettings::default();
        assert!(!off.active(false));
        assert!(off.active(true));
        for s in [
            PostSettings { grade: 0.5, ..off },
            PostSettings {
                vignette: 1.0,
                ..off
            },
            PostSettings { grain: 1.0, ..off },
        ] {
            assert!(s.active(false));
        }
    }

    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        pollster::block_on(instance.request_adapter(&Default::default()))
            .ok()
            .filter(|a| a.get_info().backend != wgpu::Backend::Gl)
            .and_then(|a| pollster::block_on(a.request_device(&Default::default())).ok())
    }

    fn mp_crash_film() -> vcod_cod4::look::FilmConsts {
        vcod_cod4::look::Film {
            enable: true,
            contrast: 1.4,
            brightness: 0.134,
            desaturation: 0.265,
            invert: false,
            light_tint: [1.179, 1.086, 1.024],
            dark_tint: [0.7, 1.0215, 1.265],
        }
        .consts()
        .unwrap()
    }

    /// One pixel of `display` through the pass at identityLight `il`
    /// (the world target holds display * identityLight), with a 1x1 glow
    /// target of `glow` added at `intensity`; the gamma scene's bytes.
    fn one_pixel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        display: [f32; 3],
        film: Option<Film>,
        glow: Option<([u8; 4], f32)>,
        il: f32,
    ) -> [u8; 4] {
        let mut pass = PostPass::new(device, SCENE_FORMAT, 1, 1);
        let one = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };
        let glow_view = glow.map(|(px, _)| {
            wgpu::util::DeviceExt::create_texture_with_data(
                device,
                queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: one,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &px,
            )
            .create_view(&Default::default())
        });
        pass.bind_glow(device, glow_view.as_ref());
        let mut enc = device.create_command_encoder(&Default::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: pass.world_view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(display[0] * il),
                        g: f64::from(display[1] * il),
                        b: f64::from(display[2] * il),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: one,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        pass.set_params(
            queue,
            PostSettings::default(),
            film,
            glow.map_or(0.0, |g| g.1),
            false,
            il,
            0.0,
        );
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.draw(&mut rp);
        }
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            out.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            one,
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let px = buf.slice(..).get_mapped_range().unwrap();
        [px[0], px[1], px[2], px[3]]
    }

    /// The post pass draws a CoD4 film as `FilmConsts::apply` computes it:
    /// mp_crash's on a mid grey and a saturated red, identityLight 0.5 as
    /// full screen. Vulkan or Metal only; under lavapipe set
    /// `GALLIVM_PERF=nopt`.
    #[test]
    fn film_runs_as_retail_postfx_color() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let c = mp_crash_film();
        let film = Film {
            bias: c.bias,
            base: c.base,
            delta: c.delta,
        };
        let il = 0.5;
        for display in [[0.5, 0.5, 0.5], [0.8, 0.1, 0.1]] {
            let px = one_pixel(&device, &queue, display, Some(film), None, il);
            let want = c.apply(display).map(|v| v.clamp(0.0, 1.0) * il * 255.0);
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 2.0,
                    "{display:?}: {px:?} vs {want:?}"
                );
            }
        }
    }

    /// `glow_apply_bloom` adds the glow target at the intensity into the
    /// framebuffer after `postfx_color` wrote it clamped: mp_bog's 1.42 on
    /// a glow of 0.2, over the film and without one.
    #[test]
    fn glow_adds_after_the_film() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let c = mp_crash_film();
        let film = Film {
            bias: c.bias,
            base: c.base,
            delta: c.delta,
        };
        let (il, g, i) = (0.5, 51u8, 1.42171f32);
        let glow = f32::from(g) / 255.0 * i;
        for (display, film) in [([0.5, 0.5, 0.5], Some(film)), ([0.3, 0.2, 0.9], None)] {
            let px = one_pixel(
                &device,
                &queue,
                display,
                film,
                Some(([g, g, g, 255], i)),
                il,
            );
            let before = match film {
                Some(_) => c.apply(display),
                None => display,
            };
            let want = before.map(|v| (v.clamp(0.0, 1.0) + glow).min(1.0) * il * 255.0);
            for k in 0..3 {
                assert!(
                    (f32::from(px[k]) - want[k]).abs() <= 2.0,
                    "{display:?}: {px:?} vs {want:?}"
                );
            }
        }
    }
}
