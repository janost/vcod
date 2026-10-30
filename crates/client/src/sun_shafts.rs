//! `vc_sunShafts`, a vcod-only extra (retail has none): light shafts
//! radiating from the sun's screen position. The sky's pixels near the sun
//! (the cleared far plane in the MSAA depth) are blurred radially toward it
//! at half resolution and added into the resolved scene after the 3D draws,
//! before `vc_bloom`'s blur reads it and before the HUD. The sun's position
//! is the flare's direction (`SunFlareGpu`), so a map without a sun flare
//! casts none; occlusion comes from the per-pixel sky mask, so a hidden disc
//! behind a gappy roof still shafts through the gaps. The strength fades
//! out as the sun leaves the screen.

use glam::{Mat4, Vec2, Vec3};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// `vc_sunShafts` past this only washes the screen out.
const MAX_STRENGTH: f32 = 4.0;
/// The shafts are full while the sun's NDC lies within +-1 and gone at this
/// distance off centre.
const FADE_EDGE: f32 = 1.6;
/// The composite gain at `vc_sunShafts 1`.
const GAIN: f32 = 0.6;

/// The sun's uv (top-left origin) and the fade for a sun direction `dir`
/// (a point at infinity), or `None` behind the eye or past the fade.
pub fn sun_screen(view_proj: Mat4, dir: Vec3) -> Option<(Vec2, f32)> {
    let clip = view_proj * dir.extend(0.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = Vec2::new(clip.x, clip.y) / clip.w;
    let edge = ndc.x.abs().max(ndc.y.abs());
    let fade = ((FADE_EDGE - edge) / (FADE_EDGE - 1.0)).clamp(0.0, 1.0);
    (fade > 0.0).then(|| {
        (
            Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5),
            fade * fade * (3.0 - 2.0 * fade),
        )
    })
}

/// `Params` in sun_shafts.wgsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    sun: [f32; 4],
    tone: [f32; 4],
}

pub struct SunShafts {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    mask: wgpu::RenderPipeline,
    rays: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Targets,
    strength: f32,
}

/// Everything sized to the window.
struct Targets {
    mask: wgpu::TextureView,
    rays: wgpu::TextureView,
    bg_scene: wgpu::BindGroup,
    bg_mask: wgpu::BindGroup,
    bg_rays: wgpu::BindGroup,
}

impl SunShafts {
    /// `scene` is the resolved scene target in `scene_format`
    /// (`Renderer::world_format`), `depth` the frame's MSAA depth (it needs
    /// `TEXTURE_BINDING`).
    pub fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> SunShafts {
        let tex = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun shafts layout"),
            entries: &[
                tex(
                    0,
                    wgpu::TextureSampleType::Float { filterable: true },
                    false,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(
                    3,
                    wgpu::TextureSampleType::Depth,
                    crate::renderer::msaa_samples() > 1,
                ),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sun shafts sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun shafts params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sun_shafts.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::depth_wgsl(include_str!(
                "sun_shafts.wgsl"
            ))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun shafts pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format, blend| {
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
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let mask = pipeline("fs_mask", FORMAT, None);
        let rays = pipeline("fs_rays", FORMAT, None);
        let composite = pipeline("fs_composite", scene_format, Some(add));
        let targets = Targets::new(
            device, &layout, &sampler, &params, scene, depth, width, height,
        );
        SunShafts {
            layout,
            sampler,
            params,
            mask,
            rays,
            composite,
            targets,
            strength: 0.0,
        }
    }

    /// After the scene and depth targets were recreated at the new size.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) {
        self.targets = Targets::new(
            device,
            &self.layout,
            &self.sampler,
            &self.params,
            scene,
            depth,
            w,
            h,
        );
    }

    /// `vc_sunShafts`; 0 (or less) turns the passes off.
    pub fn set_strength(&mut self, s: f32) {
        self.strength = if s.is_finite() {
            s.clamp(0.0, MAX_STRENGTH)
        } else {
            0.0
        };
    }

    /// This frame's parameters, or `false` when nothing would draw: off,
    /// or the sun behind the eye or too far off screen.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        sun_dir: Vec3,
        (width, height): (f32, f32),
        identity_light: f32,
    ) -> bool {
        if self.strength <= 0.0 {
            return false;
        }
        let Some((uv, fade)) = sun_screen(view_proj, sun_dir) else {
            return false;
        };
        let p = Params {
            sun: [
                uv.x,
                uv.y,
                width / height.max(1.0),
                self.strength * GAIN * fade,
            ],
            tone: [1.0 / identity_light, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&p));
        true
    }

    /// The three passes, the last adding into `scene` (the resolved scene,
    /// the view bound at `new`/`resize`).
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, scene: &wgpu::TextureView) {
        let t = &self.targets;
        let draw = |encoder: &mut wgpu::CommandEncoder,
                    target: &wgpu::TextureView,
                    pipeline: &wgpu::RenderPipeline,
                    bg: &wgpu::BindGroup,
                    load: wgpu::LoadOp<wgpu::Color>| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sun shafts pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        draw(encoder, &t.mask, &self.mask, &t.bg_scene, clear);
        draw(encoder, &t.rays, &self.rays, &t.bg_mask, clear);
        draw(
            encoder,
            scene,
            &self.composite,
            &t.bg_rays,
            wgpu::LoadOp::Load,
        );
    }
}

impl Targets {
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        params: &wgpu::Buffer,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Targets {
        let half = || {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("sun shafts half"),
                    size: wgpu::Extent3d {
                        width: width.div_ceil(2).max(1),
                        height: height.div_ceil(2).max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let bind = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sun shafts bind group"),
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
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                ],
            })
        };
        let mask = half();
        let rays = half();
        Targets {
            bg_scene: bind(scene),
            bg_mask: bind(&mask),
            bg_rays: bind(&rays),
            mask,
            rays,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_proj(fwd: Vec3) -> Mat4 {
        crate::camera::perspective(90.0, 16.0 / 9.0, 4.0, 60000.0)
            * glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, Vec3::Z)
    }

    #[test]
    fn sun_ahead_is_centred_and_full() {
        let (uv, fade) = sun_screen(view_proj(Vec3::X), Vec3::X).unwrap();
        assert!((uv - Vec2::splat(0.5)).length() < 1e-4, "{uv}");
        assert_eq!(fade, 1.0);
    }

    #[test]
    fn sun_above_the_view_sits_high_on_screen() {
        let dir = Vec3::new(1.0, 0.0, 0.3).normalize();
        let (uv, fade) = sun_screen(view_proj(Vec3::X), dir).unwrap();
        assert!(uv.y < 0.5 && (uv.x - 0.5).abs() < 1e-4, "{uv}");
        assert_eq!(fade, 1.0);
    }

    #[test]
    fn fades_out_past_the_edge_and_behind() {
        let vp = view_proj(Vec3::X);
        assert!(sun_screen(vp, Vec3::NEG_X).is_none());
        // 90 degree fov: 45 degrees off axis is the screen's edge (ndc 1),
        // a little past it fades, far past it is gone
        let at = |deg: f32| {
            let r = deg.to_radians();
            sun_screen(vp, Vec3::new(r.cos(), -r.sin(), 0.0)).map(|s| s.1)
        };
        assert_eq!(at(44.0), Some(1.0));
        let partial = at(50.0).unwrap();
        assert!(partial > 0.0 && partial < 1.0, "{partial}");
        assert!(at(70.0).is_none());
    }
}
