//! CoD4's depth of field (cod4-client-view.md 12): the cgame's
//! `GfxDepthOfField` for the drawn view, the scripted one off the
//! playerstate or the sight's eased toward its targets, and the constants
//! retail's `postfx_dof` chain reads, which `crate::renderer`'s
//! [`DofPass`] draws with.

use crate::cod4_viewmodel::ViewPs;

/// `PM_DEAD`; at or past it the sight's targets ease in as with the gun up.
const PM_DEAD: u32 = 7;
/// `r_dof_bias`' default (iw3mp.exe 0x6be648).
const BIAS: f32 = 0.5;

/// `GfxDepthOfField`: distances in world units, blurs in 640x480 pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dof {
    pub viewmodel_start: f32,
    pub viewmodel_end: f32,
    pub near_start: f32,
    pub near_end: f32,
    pub far_start: f32,
    pub far_end: f32,
    pub near_blur: f32,
    pub far_blur: f32,
}

impl Dof {
    /// `R_UsingDepthOfField` (cod4-client-view.md 12): some range wider
    /// than a unit, the far one only with a far blur.
    pub fn used(&self) -> bool {
        let wide = |from: f32, to: f32| to > from + 1.0;
        wide(self.viewmodel_start, self.viewmodel_end)
            || wide(self.near_start, self.near_end)
            || (self.far_blur > 0.0 && wide(self.far_start, self.far_end))
    }

    /// The shader constants for a scene `height` pixels tall, the world's
    /// near plane at `z_near` and the view model's at `vm_near`
    /// (`RB_ApplyMergedPostEffects`, cod4-client-view.md 12).
    pub fn consts(&self, height: u32, z_near: f32, vm_near: f32) -> DofConsts {
        let blur = self.near_blur.max(4.0);
        let near = Ramp::near(self.near_start, self.near_end, z_near);
        let far = Ramp::far(self.far_start, self.far_end, z_near);
        let gun = Ramp::near(self.viewmodel_start, self.viewmodel_end, vm_near);
        let biased = |x: f32| x.powf(BIAS);
        // the small and medium blurs as fractions of the large, at 640x480
        let fraction = |pixels: f32| biased(pixels * 480.0 / height.max(1) as f32 / blur);
        let (lerp_scale, lerp_bias) = blur_lerp(fraction(1.4), fraction(3.6));
        DofConsts {
            scene: [near.scale, far.scale, near.bias, far.bias],
            viewmodel: [gun.scale, 0.0, gun.bias, biased(self.far_blur / blur)],
            lerp_scale,
            lerp_bias,
            // a quarter-size gaussian of the 640x480 radius scaled to the
            // scene (`RB_GetDepthOfFieldInputImages`)
            sigma: blur * 0.25 * height as f32 / 480.0,
        }
    }
}

/// A blur ramp over depth `z`: `scale * z + bias`.
#[derive(Clone, Copy, Default)]
struct Ramp {
    scale: f32,
    bias: f32,
}

impl Ramp {
    /// 1 at `blurred`, 0 at `sharp` (`RB_GetNearDepthOfFieldEquation`).
    /// A range that does not clear the near plane falls back to blurring
    /// what lies under half of it.
    fn near(blurred: f32, sharp: f32, near_plane: f32) -> Ramp {
        let (blurred, sharp) = if blurred.max(near_plane) >= sharp {
            (0.0, near_plane * 0.5)
        } else {
            (blurred, sharp)
        };
        Ramp {
            scale: 1.0 / (blurred - sharp),
            bias: sharp / (sharp - blurred),
        }
    }

    /// 0 at `sharp`, 1 at `blurred`; none when the range starts at or past
    /// its end once the near plane clamps its start.
    fn far(sharp: f32, blurred: f32, near_plane: f32) -> Ramp {
        if sharp.max(near_plane) < blurred {
            Ramp {
                scale: 1.0 / (blurred - sharp),
                bias: sharp / (sharp - blurred),
            }
        } else {
            Ramp::default()
        }
    }
}

/// `dofLerpScale` and `dofLerpBias` for the small blur at fraction
/// `small` and the medium at `medium` of the large one.
fn blur_lerp(small: f32, medium: f32) -> ([f32; 4], [f32; 4]) {
    let to_small = medium - small;
    let to_large = 1.0 - medium;
    (
        [
            -1.0 / small,
            -1.0 / to_small,
            -1.0 / to_large,
            1.0 / to_large,
        ],
        [1.0, medium / to_small, 1.0 / to_large, -medium / to_large],
    )
}

/// What `cod4_dof.wgsl` reads: `dofEquationScene`,
/// `dofEquationViewModelAndFarBlur`, `dofLerpScale`, `dofLerpBias` and
/// the quarter target's gaussian sigma in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DofConsts {
    pub scene: [f32; 4],
    pub viewmodel: [f32; 4],
    pub lerp_scale: [f32; 4],
    pub lerp_bias: [f32; 4],
    pub sigma: f32,
}

/// The cgame's `refdef.dof`, kept from frame to frame as the sight eases
/// it (`CG_UpdateSceneDepthOfField`, `CG_UpdateAdsDof`).
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewDof {
    dof: Dof,
}

/// The drawn playerstate's six scripted `dof*` fields
/// (`setDepthOfField`) and its view model pair, in `GfxDepthOfField`'s
/// order past the view model: near start, near end, far start, far end,
/// near blur, far blur; then view model start and end.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PsDof {
    pub scripted: [f32; 6],
    pub viewmodel: [f32; 2],
}

/// Off the sight: nothing near, the far plane at 5000, which draws no
/// blur (cod4-client-view.md 12).
const HIP: [f32; 6] = [0.0, 0.0, 5000.0, 5000.0, 6.0, 0.0];

/// How far each of the six may move per 50 ms as it eases: near
/// distances 50, far 400, blurs 0.1 (`CG_UpdateAdsDofValue`).
const EASE_PER_50MS: [f32; 6] = [50.0, 50.0, 400.0, 400.0, 0.1, 0.1];

impl Dof {
    /// The scene's six in `PsDof::scripted`'s order.
    fn scene(&self) -> [f32; 6] {
        [
            self.near_start,
            self.near_end,
            self.far_start,
            self.far_end,
            self.near_blur,
            self.far_blur,
        ]
    }

    fn set_scene(&mut self, v: [f32; 6]) {
        [
            self.near_start,
            self.near_end,
            self.far_start,
            self.far_end,
            self.near_blur,
            self.far_blur,
        ] = v;
    }
}

impl ViewDof {
    /// One frame: `ps` the drawn playerstate, `dof` its fields, `ads` the
    /// held weapon's `adsDofStart` and `adsDofEnd` (`None` with no view
    /// weapon drawn), `trace` the distance the view's forward trace ran
    /// (8192 open), `dt` the frame's seconds. A script's distances win
    /// over the sight's (`CG_UpdateSceneDepthOfField`); the view model's
    /// range moves from the playerstate's to the weapon's as the sight
    /// comes up (`CG_AddViewWeapon`).
    pub fn frame(
        &mut self,
        ps: &ViewPs,
        dof: &PsDof,
        ads: Option<[f32; 2]>,
        trace: f32,
        dt: f32,
    ) -> Dof {
        let scripted = dof.scripted[..4].iter().any(|&d| d != 0.0);
        let scene = if scripted {
            dof.scripted
        } else {
            sight_dof(self.dof.scene(), ps, trace, dt)
        };
        self.dof.set_scene(scene);
        let up = ps.pos_frac;
        let [base_start, base_end] = dof.viewmodel;
        let gun = ads.map_or([0.0, 0.0], |[start, end]| {
            [
                (start - base_start) * up + base_start,
                (end - base_end) * up + base_end,
            ]
        });
        [self.dof.viewmodel_start, self.dof.viewmodel_end] = gun;
        self.dof
    }
}

/// `CG_UpdateAdsDof` with no aim assist target on screen
/// (cod4-client-view.md 12): last frame's six `now` moved toward the
/// sight's targets. Alive at the hip they are [`HIP`]; with the sight
/// part way up they are the hip's and the targets' mix; fully up or dead
/// each eases toward its target.
fn sight_dof(now: [f32; 6], ps: &ViewPs, trace: f32, dt: f32) -> [f32; 6] {
    let up = ps.pos_frac;
    let dead = ps.pm_type >= PM_DEAD;
    if up == 0.0 && !dead {
        return HIP;
    }
    // blur near things up to 30 short of what the view meets, at most
    // 256 out; push the far blur out past it, from 2500
    let sharp_from = if trace < 256.0 { trace - 30.0 } else { 256.0 }.max(1.0);
    let far_from = if trace > 2500.0 { trace } else { 2500.0 };
    let target = [1.0, sharp_from, far_from, far_from * 4.0, 6.0, 0.0];
    std::array::from_fn(|k| {
        if up == 1.0 || dead {
            step_toward(now[k], target[k], EASE_PER_50MS[k], dt)
        } else {
            up * target[k] + (1.0 - up) * HIP[k]
        }
    })
}

/// `CG_UpdateAdsDofValue`: half the gap to `target`, no less than a unit
/// and no more than `per_50ms` scaled to `dt`, never past it.
fn step_toward(from: f32, target: f32, per_50ms: f32, dt: f32) -> f32 {
    let limit = per_50ms / 0.05 * dt;
    let step = ((target - from).abs() * 0.5).max(1.0).min(limit);
    match from.partial_cmp(&target) {
        Some(std::cmp::Ordering::Less) => (from + step).min(target),
        Some(std::cmp::Ordering::Greater) => (from - step).max(target),
        _ => from,
    }
}

/// The quarter targets' format: the near blur rides their alpha.
const QUARTER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct Targets {
    /// The resolved world, copied out so the composite can write it.
    scene: wgpu::Texture,
    scene_view: wgpu::TextureView,
    /// Retail's `postEffect1` (the downsample, later its small blur),
    /// the gaussian's half way, `postEffect0` (the gaussian) and
    /// `pingPong0` (the near blur widened).
    quarter: [wgpu::TextureView; 4],
}

/// The GPU half: the world target in, blurred by its depth in place.
pub struct DofPass {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 6],
    params: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Targets,
    format: wgpu::TextureFormat,
    /// The six steps' groups and the depth view they read: they bind
    /// only `targets` and the depth, so they last until a resize.
    groups: Option<(wgpu::TextureView, Vec<wgpu::BindGroup>)>,
}

const DOWNSAMPLE: usize = 0;
const BLUR_H: usize = 1;
const BLUR_V: usize = 2;
const NEAR_COC: usize = 3;
const SMALL_BLUR: usize = 4;
const COMPOSITE: usize = 5;
const PE1: usize = 0;
const HALF: usize = 1;
const PE0: usize = 2;
const PING: usize = 3;

impl DofPass {
    /// `format` is the world target's (`Renderer::world_format`).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> DofPass {
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod4 dof layout"),
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
                tex(1),
                tex(2),
                tex(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: crate::renderer::msaa_samples() > 1,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod4_dof.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::renderer::depth_wgsl(include_str!(
                "cod4_dof.wgsl"
            ))),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod4 dof pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, target: wgpu::TextureFormat| {
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
                    targets: &[Some(target.into())],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [
            pipeline("fs_downsample", QUARTER_FORMAT),
            pipeline("fs_blur_h", QUARTER_FORMAT),
            pipeline("fs_blur_v", QUARTER_FORMAT),
            pipeline("fs_near_coc", QUARTER_FORMAT),
            pipeline("fs_small_blur", QUARTER_FORMAT),
            pipeline("fs_composite", format),
        ];
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cod4 dof params"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cod4 dof sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        DofPass {
            layout,
            pipelines,
            params,
            sampler,
            targets: targets(device, format, w, h),
            format,
            groups: None,
        }
    }

    /// The window's new size.
    pub fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        self.targets = targets(device, self.format, w, h);
        self.groups = None;
    }

    /// Blurs `world` (the resolved world target, `world_view` its view)
    /// by `depth`, the frame's depth, under `c`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        world: &wgpu::Texture,
        world_view: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        c: &DofConsts,
    ) {
        use crate::camera::{Z_FAR, Z_NEAR};
        use crate::renderer::{VM_DEPTH_RANGE, VM_FAR, VM_NEAR};
        let p: [[f32; 4]; 6] = [
            c.scene,
            c.viewmodel,
            c.lerp_scale,
            c.lerp_bias,
            [c.sigma, Z_NEAR, Z_FAR, VM_FAR],
            [VM_NEAR, VM_DEPTH_RANGE, 0.0, 0.0],
        ];
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&p));
        let t = &self.targets;
        encoder.copy_texture_to_texture(
            world.as_image_copy(),
            t.scene.as_image_copy(),
            t.scene.size(),
        );
        let q = &t.quarter;
        let steps: [(usize, [&wgpu::TextureView; 3], &wgpu::TextureView); 6] = [
            (DOWNSAMPLE, [&t.scene_view, &q[PE0], &q[PE0]], &q[PE1]),
            (BLUR_H, [&q[PE1], &q[PE0], &q[PE0]], &q[HALF]),
            (BLUR_V, [&q[HALF], &q[PE1], &q[PE1]], &q[PE0]),
            (NEAR_COC, [&q[PE0], &q[PE1], &q[HALF]], &q[PING]),
            (SMALL_BLUR, [&q[PING], &q[PE0], &q[PE0]], &q[PE1]),
            (COMPOSITE, [&t.scene_view, &q[PE1], &q[PE0]], world_view),
        ];
        if self.groups.as_ref().is_none_or(|(d, _)| d != depth) {
            let tex = |b, v| wgpu::BindGroupEntry {
                binding: b,
                resource: wgpu::BindingResource::TextureView(v),
            };
            let groups = steps
                .iter()
                .map(|(_, src, _)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("cod4 dof"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: self.params.as_entire_binding(),
                            },
                            tex(1, src[0]),
                            tex(2, src[1]),
                            tex(3, src[2]),
                            tex(4, depth),
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    })
                })
                .collect();
            self.groups = Some((depth.clone(), groups));
        }
        let (_, groups) = self.groups.as_ref().expect("built above");
        for ((pipe, _, dst), bg) in steps.into_iter().zip(groups) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cod4 dof"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
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
            pass.set_pipeline(&self.pipelines[pipe]);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn targets(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> Targets {
    let make = |label, w: u32, h: u32, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let scene = make(
        "cod4 dof scene",
        w,
        h,
        format,
        wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let scene_view = scene.create_view(&Default::default());
    // retail's post effect targets are a quarter of the scene each way
    let (qw, qh) = (w.div_ceil(4), h.div_ceil(4));
    let quarter = std::array::from_fn(|_| {
        make(
            "cod4 dof quarter",
            qw,
            qh,
            QUARTER_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&Default::default())
    });
    Targets {
        scene,
        scene_view,
        quarter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps(frac: f32) -> ViewPs {
        ViewPs {
            pm_type: 0,
            pos_frac: frac,
            ..Default::default()
        }
    }

    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    #[test]
    fn dof_consts_pin() {
        let dofs = [
            Dof::default(),
            Dof {
                near_start: 1.0,
                near_end: 170.0,
                far_start: 2500.0,
                far_end: 10000.0,
                near_blur: 6.0,
                ..Default::default()
            },
            Dof {
                viewmodel_start: 2.0,
                viewmodel_end: 8.0,
                near_start: 10.0,
                near_end: 60.0,
                far_start: 1000.0,
                far_end: 7000.0,
                near_blur: 6.0,
                far_blur: 1.8,
            },
            // near range inside the near plane, far start under it
            Dof {
                viewmodel_start: 0.05,
                viewmodel_end: 0.08,
                near_start: 3.0,
                near_end: 3.5,
                far_start: 2.0,
                far_end: 3000.0,
                near_blur: 2.0,
                far_blur: 9.0,
            },
            // reversed ranges
            Dof {
                near_start: 80.0,
                near_end: 20.0,
                far_start: 900.0,
                far_end: 400.0,
                near_blur: 10.0,
                far_blur: 0.5,
                ..Default::default()
            },
        ];
        let mut s = String::new();
        for d in &dofs {
            s += &format!("{d:?} used {}\n", d.used());
            for (h, z, vm) in [(480, 4.0, 0.1), (1080, 4.0, 0.1), (0, 1.0, 0.5)] {
                s += &format!("  {h} {z} {vm} => {:?}\n", d.consts(h, z, vm));
            }
        }
        pin(
            "consts",
            &s,
            r##"Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 0.0, near_end: 0.0, far_start: 0.0, far_end: 0.0, near_blur: 0.0, far_blur: 0.0 } used false
  480 4 0.1 => DofConsts { scene: [-0.5, 0.0, 1.0, 0.0], viewmodel: [-20.0, 0.0, 1.0, 0.0], lerp_scale: [-1.6903085, -2.80053, -19.486818, 19.486818], lerp_bias: [1.0, 2.656816, 19.486818, -18.486818], sigma: 1.0 }
  1080 4 0.1 => DofConsts { scene: [-0.5, 0.0, 1.0, 0.0], viewmodel: [-20.0, 0.0, 1.0, 0.0], lerp_scale: [-2.5354626, -4.2007947, -2.7207592, 2.7207592], lerp_bias: [1.0, 2.6568158, 2.7207592, -1.7207592], sigma: 2.25 }
  0 1 0.5 => DofConsts { scene: [-2.0, 0.0, 1.0, 0.0], viewmodel: [-4.0, 0.0, 1.0, 0.0], lerp_scale: [-0.07715168, -0.12782611, 0.05054434, -0.05054434], lerp_bias: [1.0, 2.6568158, -0.05054434, 1.0505444], sigma: 0.0 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 1.0, near_end: 170.0, far_start: 2500.0, far_end: 10000.0, near_blur: 6.0, far_blur: 0.0 } used true
  480 4 0.1 => DofConsts { scene: [-0.00591716, 0.00013333333, 1.0059172, -0.33333334], viewmodel: [-20.0, 0.0, 1.0, 0.0], lerp_scale: [-2.0701966, -3.429935, -4.436491, 4.436491], lerp_bias: [1.0, 2.656816, 4.436491, -3.436491], sigma: 1.5 }
  1080 4 0.1 => DofConsts { scene: [-0.00591716, 0.00013333333, 1.0059172, -0.33333334], viewmodel: [-20.0, 0.0, 1.0, 0.0], lerp_scale: [-3.105295, -5.1449018, -2.067815, 2.067815], lerp_bias: [1.0, 2.6568158, 2.067815, -1.0678152], sigma: 3.375 }
  0 1 0.5 => DofConsts { scene: [-0.00591716, 0.00013333333, 1.0059172, -0.33333334], viewmodel: [-4.0, 0.0, 1.0, 0.0], lerp_scale: [-0.094491124, -0.15655437, 0.0626152, -0.0626152], lerp_bias: [1.0, 2.6568158, -0.0626152, 1.0626152], sigma: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 10.0, near_end: 60.0, far_start: 1000.0, far_end: 7000.0, near_blur: 6.0, far_blur: 1.8 } used true
  480 4 0.1 => DofConsts { scene: [-0.02, 0.00016666666, 1.2, -0.16666667], viewmodel: [-0.16666667, 0.0, 1.3333334, 0.5477225], lerp_scale: [-2.0701966, -3.429935, -4.436491, 4.436491], lerp_bias: [1.0, 2.656816, 4.436491, -3.436491], sigma: 1.5 }
  1080 4 0.1 => DofConsts { scene: [-0.02, 0.00016666666, 1.2, -0.16666667], viewmodel: [-0.16666667, 0.0, 1.3333334, 0.5477225], lerp_scale: [-3.105295, -5.1449018, -2.067815, 2.067815], lerp_bias: [1.0, 2.6568158, 2.067815, -1.0678152], sigma: 3.375 }
  0 1 0.5 => DofConsts { scene: [-0.02, 0.00016666666, 1.2, -0.16666667], viewmodel: [-0.16666667, 0.0, 1.3333334, 0.5477225], lerp_scale: [-0.094491124, -0.15655437, 0.0626152, -0.0626152], lerp_bias: [1.0, 2.6568158, -0.0626152, 1.0626152], sigma: 0.0 }
Dof { viewmodel_start: 0.05, viewmodel_end: 0.08, near_start: 3.0, near_end: 3.5, far_start: 2.0, far_end: 3000.0, near_blur: 2.0, far_blur: 9.0 } used true
  480 4 0.1 => DofConsts { scene: [-0.5, 0.0003335557, 1.0, -0.0006671114], viewmodel: [-20.0, 0.0, 1.0, 1.5], lerp_scale: [-1.6903085, -2.80053, -19.486818, 19.486818], lerp_bias: [1.0, 2.656816, 19.486818, -18.486818], sigma: 1.0 }
  1080 4 0.1 => DofConsts { scene: [-0.5, 0.0003335557, 1.0, -0.0006671114], viewmodel: [-20.0, 0.0, 1.0, 1.5], lerp_scale: [-2.5354626, -4.2007947, -2.7207592, 2.7207592], lerp_bias: [1.0, 2.6568158, 2.7207592, -1.7207592], sigma: 2.25 }
  0 1 0.5 => DofConsts { scene: [-2.0, 0.0003335557, 7.0, -0.0006671114], viewmodel: [-4.0, 0.0, 1.0, 1.5], lerp_scale: [-0.07715168, -0.12782611, 0.05054434, -0.05054434], lerp_bias: [1.0, 2.6568158, -0.05054434, 1.0505444], sigma: 0.0 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 80.0, near_end: 20.0, far_start: 900.0, far_end: 400.0, near_blur: 10.0, far_blur: 0.5 } used false
  480 4 0.1 => DofConsts { scene: [-0.5, 0.0, 1.0, 0.0], viewmodel: [-20.0, 0.0, 1.0, 0.2236068], lerp_scale: [-2.6726124, -4.4280267, -2.4999998, 2.4999998], lerp_bias: [1.0, 2.656816, 2.4999998, -1.4999998], sigma: 2.5 }
  1080 4 0.1 => DofConsts { scene: [-0.5, 0.0, 1.0, 0.0], viewmodel: [-20.0, 0.0, 1.0, 0.2236068], lerp_scale: [-4.008919, -6.642039, -1.6666666, 1.6666666], lerp_bias: [1.0, 2.6568155, 1.6666666, -0.6666666], sigma: 5.625 }
  0 1 0.5 => DofConsts { scene: [-2.0, 0.0, 1.0, 0.0], viewmodel: [-4.0, 0.0, 1.0, 0.2236068], lerp_scale: [-0.121987514, -0.20211078, 0.08233609, -0.08233609], lerp_bias: [1.0, 2.6568153, -0.08233609, 1.0823361], sigma: 0.0 }
"##,
        );
    }

    #[test]
    fn dof_frames_pin() {
        let pd = PsDof::default();
        let script = PsDof {
            scripted: [0.0, 100.0, 500.0, 2000.0, 6.0, 1.8],
            viewmodel: [1.0, 3.0],
        };
        let far_only = PsDof {
            scripted: [0.0, 0.0, 0.0, 4000.0, 4.0, 0.0],
            viewmodel: [0.0, 0.0],
        };
        let at = |frac: f32, pm_type: u32| ViewPs {
            pm_type,
            pos_frac: frac,
            ..Default::default()
        };
        // (ps, dof, ads, trace, dt)
        let frames: Vec<(ViewPs, &PsDof, _, f32, f32)> = vec![
            (at(0.0, 0), &pd, Some([2.0, 8.0]), 8192.0, 0.016),
            (at(0.25, 0), &pd, Some([2.0, 8.0]), 8192.0, 0.016),
            (at(0.6, 0), &pd, Some([2.0, 8.0]), 20.0, 0.016),
            (at(1.0, 0), &pd, Some([2.0, 8.0]), 20.0, 0.016),
            (at(1.0, 0), &pd, Some([2.0, 8.0]), 20.0, 0.05),
            (at(1.0, 0), &pd, Some([2.0, 8.0]), 3000.0, 0.1),
            (at(1.0, 0), &pd, Some([2.0, 8.0]), 3000.0, 0.0),
            (at(1.0, 0), &pd, Some([2.0, 8.0]), 600.0, 0.033),
            (at(1.0, 0), &script, Some([2.0, 8.0]), 600.0, 0.016),
            (at(0.5, 0), &script, None, 600.0, 0.016),
            (at(0.0, 7), &pd, None, 150.0, 0.016),
            (at(0.0, 7), &pd, None, 150.0, 0.016),
            (at(0.3, 8), &far_only, Some([4.0, 4.0]), 150.0, 0.016),
            (at(0.0, 0), &pd, Some([2.0, 8.0]), 8192.0, 0.016),
        ];
        let mut v = ViewDof::default();
        let mut s = String::new();
        for (ps, dof, ads, trace, dt) in frames {
            s += &format!("{:?}\n", v.frame(&ps, dof, ads, trace, dt));
        }
        // ease up toward the sight's targets and hold
        for _ in 0..40 {
            s += &format!(
                "{:?}\n",
                v.frame(&at(1.0, 0), &pd, Some([2.0, 8.0]), 400.0, 0.016)
            );
        }
        pin(
            "frames",
            &s,
            r##"Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 0.0, near_end: 0.0, far_start: 5000.0, far_end: 5000.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 0.5, viewmodel_end: 2.0, near_start: 0.25, near_end: 64.0, far_start: 5798.0, far_end: 11942.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 1.2, viewmodel_end: 4.8, near_start: 0.6, near_end: 0.6, far_start: 3500.0, far_end: 8000.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 1.0, far_start: 3372.0, far_end: 8128.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 1.0, far_start: 2972.0, far_end: 8528.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 101.0, far_start: 2986.0, far_end: 9328.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 101.0, far_start: 2986.0, far_end: 9328.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 134.0, far_start: 2743.0, far_end: 9592.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 0.0, near_end: 100.0, far_start: 500.0, far_end: 2000.0, near_blur: 6.0, far_blur: 1.8 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 0.0, near_end: 100.0, far_start: 500.0, far_end: 2000.0, near_blur: 6.0, far_blur: 1.8 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 1.0, near_end: 110.0, far_start: 628.0, far_end: 2128.0, near_blur: 6.0, far_blur: 1.768 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 1.0, near_end: 115.0, far_start: 756.0, far_end: 2256.0, near_blur: 6.0, far_blur: 1.7360001 }
Dof { viewmodel_start: 1.2, viewmodel_end: 1.2, near_start: 0.0, near_end: 0.0, far_start: 0.0, far_end: 4000.0, near_blur: 4.0, far_blur: 0.0 }
Dof { viewmodel_start: 0.0, viewmodel_end: 0.0, near_start: 0.0, near_end: 0.0, far_start: 5000.0, far_end: 5000.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 16.0, far_start: 4872.0, far_end: 5128.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 32.0, far_start: 4744.0, far_end: 5256.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 48.0, far_start: 4616.0, far_end: 5384.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 64.0, far_start: 4488.0, far_end: 5512.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 80.0, far_start: 4360.0, far_end: 5640.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 96.0, far_start: 4232.0, far_end: 5768.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 112.0, far_start: 4104.0, far_end: 5896.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 128.0, far_start: 3976.0, far_end: 6024.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 144.0, far_start: 3848.0, far_end: 6152.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 160.0, far_start: 3720.0, far_end: 6280.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 176.0, far_start: 3592.0, far_end: 6408.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 192.0, far_start: 3464.0, far_end: 6536.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 208.0, far_start: 3336.0, far_end: 6664.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 224.0, far_start: 3208.0, far_end: 6792.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 240.0, far_start: 3080.0, far_end: 6920.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 248.0, far_start: 2952.0, far_end: 7048.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 252.0, far_start: 2824.0, far_end: 7176.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 254.0, far_start: 2696.0, far_end: 7304.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 255.0, far_start: 2598.0, far_end: 7432.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2549.0, far_end: 7560.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2524.5, far_end: 7688.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2512.25, far_end: 7816.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2506.125, far_end: 7944.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2503.0625, far_end: 8072.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2501.5313, far_end: 8200.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.5313, far_end: 8328.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 8456.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 8584.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 8712.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 8840.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 8968.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9096.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9224.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9352.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9480.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9608.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9736.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9864.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9932.0, near_blur: 6.0, far_blur: 0.0 }
Dof { viewmodel_start: 2.0, viewmodel_end: 8.0, near_start: 1.0, near_end: 256.0, far_start: 2500.0, far_end: 9966.0, near_blur: 6.0, far_blur: 0.0 }
"##,
        );
    }

    #[test]
    fn the_hip_view_has_none() {
        let mut v = ViewDof::default();
        let d = v.frame(&ps(0.0), &PsDof::default(), Some([0.0, 0.0]), 8192.0, 0.016);
        assert!(!d.used(), "{d:?}");
        assert_eq!((d.far_start, d.far_end, d.near_blur), (5000.0, 5000.0, 6.0));
    }

    #[test]
    fn the_sight_blurs_up_to_30_short_of_the_wall() {
        let mut v = ViewDof::default();
        let pd = PsDof::default();
        // halfway up: the hip and sighted values lerped
        let d = v.frame(&ps(0.5), &pd, Some([2.0, 8.0]), 200.0, 0.016);
        assert_eq!((d.near_start, d.near_end), (0.5, 85.0));
        assert_eq!((d.viewmodel_start, d.viewmodel_end), (1.0, 4.0));
        assert!(d.used());
        // up: eased to the targets
        for _ in 0..200 {
            v.frame(&ps(1.0), &pd, Some([2.0, 8.0]), 200.0, 0.016);
        }
        let d = v.frame(&ps(1.0), &pd, Some([2.0, 8.0]), 200.0, 0.016);
        assert_eq!((d.near_start, d.near_end), (1.0, 170.0));
        assert_eq!((d.far_start, d.far_end), (2500.0, 10000.0));
        assert_eq!((d.near_blur, d.far_blur), (6.0, 0.0));
    }

    #[test]
    fn a_script_dof_overrides_the_sight() {
        let mut v = ViewDof::default();
        let pd = PsDof {
            scripted: [0.0, 100.0, 500.0, 2000.0, 6.0, 1.8],
            viewmodel: [0.0, 0.0],
        };
        let d = v.frame(&ps(0.0), &pd, None, 8192.0, 0.016);
        assert_eq!((d.near_end, d.far_start, d.far_blur), (100.0, 500.0, 1.8));
        assert!(d.used());
    }

    #[test]
    fn a_step_moves_half_way_at_most_its_rate() {
        // 50 units per 50 ms: 16 in a 16 ms frame
        assert_eq!(step_toward(0.0, 100.0, 50.0, 0.016), 16.0);
        assert_eq!(step_toward(0.0, 10.0, 50.0, 0.016), 5.0);
        assert_eq!(step_toward(9.5, 10.0, 50.0, 0.016), 10.0);
        assert_eq!(step_toward(10.0, 0.0, 50.0, 0.016), 5.0);
    }

    /// The sighted defaults at 480 lines: `postfx_dof`'s lerp constants
    /// put the small blur at 1.4 / 6 and the medium at 3.6 / 6 under the
    /// 0.5 bias.
    #[test]
    fn consts_follow_the_blur_fractions() {
        let d = Dof {
            near_start: 1.0,
            near_end: 170.0,
            far_start: 2500.0,
            far_end: 10000.0,
            near_blur: 6.0,
            ..Default::default()
        };
        let c = d.consts(480, 4.0, 0.1);
        let small = (1.4f32 / 6.0).sqrt();
        let medium = (3.6f32 / 6.0).sqrt();
        assert!((c.lerp_scale[0] + 1.0 / small).abs() < 1e-5);
        assert!((c.lerp_bias[3] + medium / (1.0 - medium)).abs() < 1e-5);
        // the near blur is 1 at 1 and 0 at 170, the far 0 at 2500 and 1
        // at 10000
        let near = |z: f32| (c.scene[0] * z + c.scene[2]).clamp(0.0, 1.0);
        let far = |z: f32| (c.scene[1] * z + c.scene[3]).clamp(0.0, 1.0);
        assert!((near(1.0) - 1.0).abs() < 1e-5 && near(170.0).abs() < 1e-5);
        assert!(far(2500.0).abs() < 1e-5 && (far(10000.0) - 1.0).abs() < 1e-5);
        assert_eq!(c.viewmodel[3], 0.0);
        assert!((c.sigma - 1.5).abs() < 1e-5);
    }
}
