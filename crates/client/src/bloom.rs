//! `vc_bloom`, a vcod-only glow on bright pixels (retail has none). The
//! bright-pass reads the resolved scene before the HUD draws, a dual-filter
//! chain (Bjørge 2015) blurs it at half resolution and below, and the
//! composite adds it into the resolved scene, where the HUD then draws
//! single-sampled on top. The threshold is on the display value at gamma 1, framebuffer /
//! `identityLight`, so windowed and full screen bloom the same pixels
//! (docs/research/cod11-gamma.md, section 4). With `vc_hdr` the scene is
//! `crate::post`'s float world target and the knee sits past display white,
//! so only what retail would clip glows: stacked blends (fx, flares, the
//! sun), and full screen the overbright part of a surface.

use crate::renderer::gpu_util;

const CHAIN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Most levels below the half-resolution one, and the smallest side a
/// level may have.
const MAX_LEVELS: usize = 6;
const MIN_SIDE: u32 = 8;
/// The composite gain at `vc_bloom 1`.
const GAIN: f32 = 0.25;
/// With `vc_hdr`, where the knee sits above `vc_bloomThreshold` on the
/// display value: the default 0.8 puts it at white.
const HDR_KNEE_OFFSET: f32 = 0.2;

pub struct Bloom {
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// Half resolution first, each one half the last.
    levels: Vec<wgpu::TextureView>,
    /// Reads the scene; `level_bgs[i]` reads `levels[i]`.
    scene_bg: wgpu::BindGroup,
    level_bgs: Vec<wgpu::BindGroup>,
}

impl Bloom {
    /// `scene_format` is the world's (`Renderer::world_format`): the
    /// composite adds into the target the chain reads.
    pub fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        scene: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Bloom {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom layout"),
            entries: &[
                gpu_util::texture_2d(0),
                gpu_util::filtering_sampler(1),
                gpu_util::uniform(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let sampler = gpu_util::linear_clamp_sampler(device, Some("bloom sampler"));
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bloom params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("bloom.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let pipeline =
            |entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
                gpu_util::Fullscreen::new(
                    Some(entry),
                    Some(&pipeline_layout),
                    &shader,
                    "vs_main",
                    entry,
                    wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::COLOR,
                    },
                )
                .build(device)
            };
        let prefilter = pipeline("fs_prefilter", CHAIN_FORMAT, None);
        let down = pipeline("fs_down", CHAIN_FORMAT, None);
        let up = pipeline("fs_up", CHAIN_FORMAT, Some(add));
        let composite = pipeline("fs_composite", scene_format, Some(add));
        let (levels, scene_bg, level_bgs) =
            create_chain(device, &layout, &sampler, &params, scene, width, height);
        Bloom {
            prefilter,
            down,
            up,
            composite,
            layout,
            sampler,
            params,
            levels,
            scene_bg,
            level_bgs,
        }
    }

    /// After the scene target was recreated at the new size.
    pub fn resize(&mut self, device: &wgpu::Device, scene: &wgpu::TextureView, w: u32, h: u32) {
        (self.levels, self.scene_bg, self.level_bgs) = create_chain(
            device,
            &self.layout,
            &self.sampler,
            &self.params,
            scene,
            w,
            h,
        );
    }

    /// `intensity` is `vc_bloom`, `threshold` the display value where the
    /// glow starts, `identity_light` the frame's (`gamma::identity_light`),
    /// `hdr` whether the scene is the float world target.
    pub fn set_params(
        &self,
        queue: &wgpu::Queue,
        intensity: f32,
        threshold: f32,
        identity_light: f32,
        hdr: bool,
    ) {
        let scale = intensity * GAIN * identity_light / self.levels.len().max(1) as f32;
        let p = if hdr {
            [
                (threshold + HDR_KNEE_OFFSET).max(0.05),
                1.0 / identity_light,
                scale,
                1.0,
            ]
        } else {
            [threshold.clamp(0.0, 0.99), 1.0 / identity_light, scale, 0.0]
        };
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&p));
    }

    /// The bright-pass and blur chain, reading the resolved scene.
    pub fn blur(&self, encoder: &mut wgpu::CommandEncoder) {
        let n = self.levels.len();
        if n == 0 {
            return;
        }
        self.draw(
            encoder,
            &self.prefilter,
            &self.scene_bg,
            &self.levels[0],
            true,
        );
        for i in 0..n - 1 {
            self.draw(
                encoder,
                &self.down,
                &self.level_bgs[i],
                &self.levels[i + 1],
                true,
            );
        }
        for i in (0..n - 1).rev() {
            self.draw(
                encoder,
                &self.up,
                &self.level_bgs[i + 1],
                &self.levels[i],
                false,
            );
        }
    }

    /// Adds the blurred glow into `pass`, the resolved scene's pass that
    /// draws the HUD next.
    pub fn composite(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.levels.is_empty() {
            return;
        }
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &self.level_bgs[0], &[]);
        pass.draw(0..3, 0..1);
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        src: &wgpu::BindGroup,
        target: &wgpu::TextureView,
        clear: bool,
    ) {
        let mut pass = gpu_util::color_pass(
            encoder,
            Some("bloom pass"),
            target,
            if clear {
                wgpu::LoadOp::Clear(wgpu::Color::BLACK)
            } else {
                wgpu::LoadOp::Load
            },
        );
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, src, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The level sizes for a `width` x `height` scene: half resolution, then
/// halved until a side would drop under `MIN_SIDE`.
fn level_sizes(width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut w, mut h) = (width / 2, height / 2);
    while out.len() < MAX_LEVELS && w >= MIN_SIDE && h >= MIN_SIDE {
        out.push((w, h));
        w /= 2;
        h /= 2;
    }
    out
}

type Chain = (
    Vec<wgpu::TextureView>,
    wgpu::BindGroup,
    Vec<wgpu::BindGroup>,
);

fn create_chain(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
    scene: &wgpu::TextureView,
    width: u32,
    height: u32,
) -> Chain {
    let bind = |view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom bind group"),
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
            ],
        })
    };
    let levels: Vec<_> = level_sizes(width, height)
        .into_iter()
        .map(|(w, h)| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("bloom level"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: CHAIN_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        })
        .collect();
    let level_bgs = levels.iter().map(bind).collect();
    (levels, bind(scene), level_bgs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_down_to_the_floor() {
        assert_eq!(
            level_sizes(1920, 1080),
            [
                (960, 540),
                (480, 270),
                (240, 135),
                (120, 67),
                (60, 33),
                (30, 16)
            ]
        );
        assert_eq!(level_sizes(40, 20), [(20, 10)]);
        assert!(level_sizes(10, 10).is_empty());
    }
}
