//! CoD2's glow, retail's `r_glow` (default 1; docs/research/cod2-materials.md
//! 9.9): `RB_ApplyLatePostEffectsCmd` (Mac 0x000da2f6) copies the finished
//! 3D view, `glow_setup` keeps its bright, non-sky part at quarter
//! resolution, `RB_GlowFilterImage` (0x0010654a) blurs it through the
//! `filter_symmetric_*` passes `RB_GenerateGaussianFilterChain` (0x00105d0a)
//! lays out, and `glow_apply_bloom` or `glow_apply_sky_bleed` adds it back
//! once per glow level. Then the blur of a `blurWorld` menu (9.13):
//! `RB_GaussianFilterImage` through the same passes, drawn back with
//! `feedbackblend`. Runs on the resolved world before the HUD and the
//! shellshock capture.
//!
//! CoD4's glow (docs/research/cod4-world.md 11.4) runs through the same
//! chain from its own setup, `glow_consistent_setup` (`plan_cod4`), and
//! no apply here: the post pass adds the result after the film tweak
//! (`crate::post`).

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// The four render targets the chain runs in are a quarter of the screen
/// (`R_InitRenderTargets` 0x000ea47c, shift 2).
const QUARTER_SHIFT: u32 = 2;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Passes a chain may take (`RB_GenerateGaussianFilterChain`'s 0x10).
const MAX_CHAIN: usize = 16;
/// Uniform slots: the setup, two chains, two applies, then the blur's
/// chain and its composite.
const SLOTS: usize = 1 + 2 * MAX_CHAIN + 2 + MAX_CHAIN + 1;
const SLOT_BYTES: u64 = 512;
/// The step groups `Cod2Glow` keeps before it starts the list over.
const MAX_GROUPS: usize = 16;

/// Below this a radius takes no pass (0x326454).
const MIN_RADIUS: f32 = 0.329_505_1;
/// The most a single 2D pass blurs (0x32644c), and the square a
/// downsample takes off past it (0x326450).
const MAX_2D_RADIUS: f32 = 1.389_560_5;
const MAX_2D_RADIUS_SQ: f32 = 1.930_878_3;
/// The most a 1D pass of eight half taps blurs, and its square
/// (0x326458, 0x32645c).
const MAX_1D_RADIUS: f32 = 6.497_750_3;
const MAX_1D_RADIUS_SQ: f32 = 42.220_76;
/// A trimmed tap's weight floor (0x3262f0) and the least weight sum the
/// normalisation takes (0x326210).
const TAP_FLOOR: f32 = 0.01;
const MIN_TOTAL: f32 = 0.001;

/// The glow dvars at their registered defaults (`gfx_d3d_mp_x86_s.dll`
/// 0x1000b75e..0x1000b883). A map's `setcvar` of them runs on the server
/// and never reaches a client of a dedicated one (9.9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowSettings {
    pub cutoff: f32,
    pub desaturation: f32,
    pub radius: [f32; 2],
    pub bloom: [f32; 2],
    pub sky_bleed: [f32; 2],
}

impl Default for GlowSettings {
    fn default() -> Self {
        GlowSettings {
            cutoff: 0.9,
            desaturation: 0.75,
            radius: [5.0, 12.0],
            bloom: [1.0, 1.0],
            sky_bleed: [0.5, 0.0],
        }
    }
}

/// One `filter_symmetric_N` pass: N taps of (s, t, 0, weight), each read
/// at `uv + (s, t)` and `uv - (s, t)`.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterPass {
    pub taps: Vec<[f32; 4]>,
}

/// What one frame draws: the chains in order (the second blurs the first's
/// result further) and one apply per level.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The setup's `glowSetup`: cutoff, `1 / (1 - cutoff)`, 0, desaturation.
    pub setup: [f32; 4],
    /// CoD4's setup (`glow_consistent_setup`) with its film tweak:
    /// `colorBias`, `colorTintBase` (w identityLight) and `colorTintDelta`.
    /// `None` runs CoD2's `glow_setup`.
    pub film: Option<[[f32; 4]; 3]>,
    pub chains: Vec<Vec<FilterPass>>,
    pub applies: Vec<Apply>,
}

/// One level's apply, `glowApply` (sky bleed, 0, 0, bloom): the sky bleed
/// material when its sky bleed is not 0, else the bloom one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Apply {
    pub sky_bleed: f32,
    pub bloom: f32,
}

/// `RB_GaussianFilterPoints1D` (0x00105878): up to `half_taps` bilinear
/// pairs of a Gaussian of `radius` texels (sigma), each pair's offset in
/// the source's texcoords and its weight, the weights summing to 0.5 per
/// side, trimmed after the first one under 0.01. The texel grid is offset
/// half a texel when the source is an even multiple of the destination.
pub fn points_1d(radius: f32, src: u32, dst: u32, half_taps: usize) -> Vec<[f32; 2]> {
    let size = src as f32;
    let ratio = (size / dst as f32 + 0.5).floor() as i32;
    let off = if ratio & 1 == 0 { 0.5 } else { 0.0 };
    let k = -0.5 / (radius * radius);
    let mut taps = Vec::with_capacity(half_taps);
    let mut total = 0.0f32;
    for i in 0..half_taps {
        let x0 = (2 * i) as f32 + off;
        let x1 = (2 * i + 1) as f32 + off;
        let mut w0 = (k * x0 * x0).exp();
        let w1 = (k * x1 * x1).exp();
        // the centre texel is shared by the + and - reads
        if i == 0 && off == 0.0 {
            w0 *= 0.5;
        }
        let w = w0 + w1;
        let at = if w != 0.0 {
            (x0 * w0 + x1 * w1) / (w * size)
        } else {
            (x0 + x1) * 0.5 / size
        };
        taps.push([at, w]);
        total += w;
    }
    if taps.is_empty() || total <= MIN_TOTAL {
        taps.truncate(1);
        if let Some(t) = taps.first_mut() {
            t[1] = 0.5;
        }
        return taps;
    }
    let norm = 0.5 / total;
    let mut count = taps.len();
    for i in (0..taps.len()).rev() {
        taps[i][1] *= norm;
        if taps[i][1] < TAP_FLOOR {
            count = i + 1;
        }
    }
    taps.truncate(count);
    taps
}

/// `RB_GenerateGaussianFilter2D` (0x00105ac0): one radius, two pairs per
/// axis from a `src`-sized source into a `dst`-sized target, every
/// quadrant, eight taps.
fn filter_2d(r: f32, src: (u32, u32), dst: (u32, u32)) -> FilterPass {
    let x = points_1d(r, src.0, dst.0, 2);
    let y = points_1d(r, src.1, dst.1, 2);
    let mut taps = Vec::with_capacity(8);
    for yj in &y {
        for xi in &x {
            let wt = xi[1] * yj[1];
            taps.push([-xi[0], yj[0], 0.0, wt]);
            taps.push([xi[0], yj[0], 0.0, wt]);
        }
    }
    FilterPass { taps }
}

/// `RB_GenerateGaussianFilterChain` on a same-size target (the glow's:
/// no downsample pass).
pub fn chain(rx: f32, ry: f32, w: u32, h: u32) -> Vec<FilterPass> {
    chain_scaled(rx, ry, (w, h), (w, h))
}

/// `RB_GenerateGaussianFilterChain` (0x00105d0a): a radius pair of `rx`,
/// `ry` texels of `src`. A smaller `dst` first takes a 2D downsample pass
/// that blurs `min(rx, ry, MAX_2D_RADIUS)` off both in quadrature, the rest
/// scaled to `dst`'s texels. Then on `dst`: one 2D pass at their mean when
/// both are small and alike, else 1D passes along the wider axis, each
/// taking at most `MAX_1D_RADIUS` off it in quadrature.
pub fn chain_scaled(mut rx: f32, mut ry: f32, src: (u32, u32), dst: (u32, u32)) -> Vec<FilterPass> {
    let mut passes = Vec::new();
    if src != dst {
        let least = rx.min(ry);
        let (r, off) = if least > MAX_2D_RADIUS {
            (MAX_2D_RADIUS, MAX_2D_RADIUS_SQ)
        } else {
            (least, least * least)
        };
        rx = (rx * rx - off).sqrt() * dst.0 as f32 / src.0 as f32;
        ry = (ry * ry - off).sqrt() * dst.1 as f32 / src.1 as f32;
        passes.push(filter_2d(r, src, dst));
    }
    let (w, h) = dst;
    while passes.len() < MAX_CHAIN {
        if rx < MIN_RADIUS && ry < MIN_RADIUS {
            break;
        }
        if (rx - ry).abs() < MIN_RADIUS && (rx + ry) * 0.5 <= MAX_2D_RADIUS {
            passes.push(filter_2d((rx + ry) * 0.5, dst, dst));
            break;
        }
        let (r, size, y_axis) = if rx <= ry {
            (&mut ry, h, true)
        } else {
            (&mut rx, w, false)
        };
        let sigma = r.min(MAX_1D_RADIUS);
        *r = if *r > MAX_1D_RADIUS {
            (*r * *r - MAX_1D_RADIUS_SQ).sqrt()
        } else {
            0.0
        };
        let taps = points_1d(sigma, size, size, 8)
            .into_iter()
            .map(|[at, wt]| {
                if y_axis {
                    [0.0, at, 0.0, wt]
                } else {
                    [at, 0.0, 0.0, wt]
                }
            })
            .collect();
        passes.push(FilterPass { taps });
    }
    passes
}

/// The late post effects' blur (`RB_ApplyLatePostEffectsCmd`'s second
/// half, docs/research/cod2-materials.md 9.13): the scene as it was
/// before the glow, through `chain`, drawn back over the scene with
/// `feedbackblend` at `alpha`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlurPlan {
    pub chain: Vec<FilterPass>,
    pub alpha: f32,
}

/// The blur of `radius` (`r_blur` and the view's, in 480-high virtual
/// units) on a `width` x `height` view, `None` at 0. Under three real
/// pixels the blur stays at three and fades in through the composite's
/// alpha, its byte rounded.
pub fn blur_plan(radius: f32, width: u32, height: u32) -> Option<BlurPlan> {
    if radius <= 0.0 {
        return None;
    }
    // `GetVirtualWidthFromRealWidth(3.0)`: 480 / height a pixel
    let least = 3.0 * 480.0 / height as f32;
    let (radius, alpha) = if radius < least {
        (least, (radius / least * 255.0 + 0.5).floor() / 255.0)
    } else {
        (radius, 1.0)
    };
    // `RB_GaussianFilterImage`: virtual units to the view's pixels, x
    // times `aspectRatioPixel` (1 on square pixels)
    let r = radius * height as f32 / 480.0;
    let chain = chain_scaled(r, r, (width, height), quarter(width, height));
    Some(BlurPlan { chain, alpha })
}

/// The frame's glow for a `width`-wide view (`RB_ApplyLatePostEffectsCmd`
/// and `RB_GlowFilterImage`): `None` when no level glows. `sky` is whether
/// the level has a sky, without which no sky bleed counts.
pub fn plan(s: &GlowSettings, width: u32, height: u32, sky: bool) -> Option<Plan> {
    let mut r: [f32; 2] = std::array::from_fn(|i| {
        if s.bloom[i] > 0.0 || (sky && s.sky_bleed[i] > 0.0) {
            s.radius[i]
        } else {
            0.0
        }
    });
    // the smaller radius blurs first; `swap` picks which level's
    // intensities the first apply takes
    let swap = !(r[1] == 0.0 || (r[0] != 0.0 && r[0] <= r[1]));
    if swap {
        r.swap(0, 1);
    }
    if r[0] == 0.0 {
        return None;
    }
    let (qw, qh) = quarter(width, height);
    let scale = qw as f32 / width as f32;
    let (r0, r1) = (r[0] * scale, r[1] * scale);
    let mut chains = vec![chain(r0, r0, qw, qh)];
    if r1 != 0.0 {
        let more = (r1 * r1 - r0 * r0).sqrt();
        chains.push(chain(more, more, qw, qh));
    }
    // both levels read the one quarter target the last chain ended in
    let applies = (0..chains.len())
        .map(|i| {
            let set = usize::from(swap) ^ (i & 1);
            Apply {
                sky_bleed: if sky { s.sky_bleed[set] } else { 0.0 },
                bloom: s.bloom[set],
            }
        })
        .collect();
    Some(Plan {
        setup: [s.cutoff, 1.0 / (1.0 - s.cutoff), 0.0, s.desaturation],
        film: None,
        chains,
        applies,
    })
}

/// A CoD4 vision set's glow as `R_SetGlowInfo` (iw3mp.exe 0x5f9190) keeps
/// it: `glowBloomIntensity0` and `glowRadius0` (cod4-world.md 11.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cod4Glow {
    pub cutoff: f32,
    pub desaturation: f32,
    pub intensity: f32,
    pub radius: f32,
}

/// The frame's CoD4 glow on a `width` x `height` view, `None` when it is
/// off (`R_UsingGlow`: intensity and radius not 0): the setup at a
/// quarter of the view, then the chain `RB_ApplyGlowFilter` (0x651860)
/// lays out on the quarter target. The radius, in 480-high virtual units,
/// is scaled by the quarter's width ratio in `RB_GlowFilterImage`
/// (0x651970) and again by 0.25 in `RB_ApplyGlowFilter` before it becomes
/// pixels; x times the pixel aspect, 1 on square pixels. `film` is the
/// setup's `postfx_color` constants (`colorBias`, `colorTintBase` with
/// identityLight in w, `colorTintDelta`). The result is
/// [`Cod2Glow::result`], for the post pass to add at `intensity`.
pub fn plan_cod4(g: &Cod4Glow, film: [[f32; 4]; 3], width: u32, height: u32) -> Option<Plan> {
    if g.intensity == 0.0 || g.radius == 0.0 {
        return None;
    }
    let (qw, qh) = quarter(width, height);
    let r = g.radius * (qw as f32 / width as f32) * 0.25;
    let ry = height as f32 * r / 480.0;
    Some(Plan {
        setup: [g.cutoff, 1.0 / (1.0 - g.cutoff), 0.0, g.desaturation],
        film: Some(film),
        chains: vec![chain(ry, ry, qw, qh)],
        applies: Vec::new(),
    })
}

/// The quarter target's size (`R_InitFullscreenRenderTargetImage`'s shift,
/// at least 1).
pub fn quarter(width: u32, height: u32) -> (u32, u32) {
    (
        (width >> QUARTER_SHIFT).max(1),
        (height >> QUARTER_SHIFT).max(1),
    )
}

/// One pass's uniform slot (`cod2_glow.wgsl`'s `Params`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    taps: [[f32; 4]; 8],
    /// x tap count, y z the destination's 1 / size.
    count: [f32; 4],
    /// The setup's `glowSetup`, or an apply's `glowApply`.
    glow: [f32; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    /// CoD4's setup: `colorBias`, `colorTintBase` (w identityLight),
    /// `colorTintDelta`.
    film: [[f32; 4]; 3],
}

pub struct Cod2Glow {
    setup: wgpu::RenderPipeline,
    /// CoD4's `glow_consistent_setup`.
    setup4: wgpu::RenderPipeline,
    filter: wgpu::RenderPipeline,
    bloom: wgpu::RenderPipeline,
    sky_bleed: wgpu::RenderPipeline,
    /// `feedbackblend`: the blur drawn back over the scene.
    blur_composite: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// Bound when the level has no sky cube.
    no_sky: wgpu::TextureView,
    /// Retail's targets 8, 9, 11 and 12: the blur's result, the glow's
    /// and the ping-pong pair.
    targets: [wgpu::TextureView; 4],
    size: (u32, u32),
    /// A step's group by the views it reads (source, sky cube): the
    /// params ride a dynamic offset, so a group outlives its frame.
    groups: Vec<(wgpu::TextureView, wgpu::TextureView, wgpu::BindGroup)>,
}

impl Cod2Glow {
    /// `scene_format` is the world's: the applies add into it.
    pub fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Cod2Glow {
        let tex = |binding, dim| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 glow layout"),
            entries: &[
                tex(0, wgpu::TextureViewDimension::D2),
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
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Params>() as u64
                        ),
                    },
                    count: None,
                },
                tex(3, wgpu::TextureViewDimension::Cube),
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("cod2_glow.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 glow pipeline layout"),
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
        // the apply materials' blends, the same on alpha as D3D9 without a
        // separate alpha blend: `glow_apply_bloom` One, One;
        // `glow_apply_sky_bleed` One, InvSrcAlpha (9.9)
        let both = |dst_factor| {
            let c = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor,
                operation: wgpu::BlendOperation::Add,
            };
            Some(wgpu::BlendState { color: c, alpha: c })
        };
        let setup = pipeline("fs_setup", TARGET_FORMAT, None);
        let setup4 = pipeline("fs_setup4", TARGET_FORMAT, None);
        let filter = pipeline("fs_filter", TARGET_FORMAT, None);
        let bloom = pipeline("fs_bloom", scene_format, both(wgpu::BlendFactor::One));
        let sky_bleed = pipeline(
            "fs_sky_bleed",
            scene_format,
            both(wgpu::BlendFactor::OneMinusSrcAlpha),
        );
        // `feedbackblend`'s stateBits 0x28128865: SrcAlpha, InvSrcAlpha
        let blur_composite = pipeline(
            "fs_blur_composite",
            scene_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cod2 glow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cod2 glow params"),
            size: SLOTS as u64 * SLOT_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let no_sky = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("cod2 glow no sky"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            });
        Cod2Glow {
            setup,
            setup4,
            filter,
            bloom,
            sky_bleed,
            blur_composite,
            layout,
            sampler,
            params,
            no_sky,
            targets: create_targets(device, width, height),
            size: (width, height),
            groups: Vec::new(),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.size != (width, height) {
            self.targets = create_targets(device, width, height);
            self.size = (width, height);
            self.groups.clear();
        }
    }

    /// Where a glow's last pass ends (retail's target 9), which a CoD4
    /// plan leaves for the post pass to add.
    pub fn result(&self) -> &wgpu::TextureView {
        &self.targets[1]
    }

    /// The late post effects on `scene`: the blur's chain from the scene
    /// as it stands (retail's copy into target 2 before the glow) into
    /// target 8; the glow of `glow` (the setup into target 11, each chain
    /// ping-ponging 11 and 12 and ending in 9, the applies reading 9 and
    /// adding into the scene); then the blur drawn over the scene. `sky` is
    /// the level's sky cube, sampled along each pixel's view ray through
    /// `inv_view_proj` from `eye`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        sky: Option<&wgpu::TextureView>,
        inv_view_proj: Mat4,
        eye: Vec3,
        glow: Option<&Plan>,
        blur: Option<&BlurPlan>,
    ) {
        let blur = blur.filter(|b| !b.chain.is_empty());
        if glow.is_none() && blur.is_none() {
            return;
        }
        let (qw, qh) = quarter(self.size.0, self.size.1);
        let inv_q = [0.0, 1.0 / qw as f32, 1.0 / qh as f32, 0.0];
        let inv_full = [0.0, 1.0 / self.size.0 as f32, 1.0 / self.size.1 as f32, 0.0];
        let cube = sky.unwrap_or(&self.no_sky);
        let blank = Params {
            taps: [[0.0; 4]; 8],
            count: inv_q,
            glow: [0.0; 4],
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            eye: [eye.x, eye.y, eye.z, 0.0],
            film: [[0.0; 4]; 3],
        };
        let filter = |pass: &FilterPass| {
            let mut p = blank;
            for (dst, tap) in p.taps.iter_mut().zip(&pass.taps) {
                *dst = *tap;
            }
            p.count[0] = pass.taps.len() as f32;
            p
        };
        let [t8, t9, t11, t12] = &self.targets;
        // a chain of `n` passes ping-pongs 11 and 12 and ends in `last`
        let pong = |i: usize, n: usize, last| {
            if i + 1 == n {
                last
            } else if i & 1 == 0 {
                t11
            } else {
                t12
            }
        };
        struct Step<'a> {
            pipeline: &'a wgpu::RenderPipeline,
            src: &'a wgpu::TextureView,
            dst: &'a wgpu::TextureView,
            load: bool,
            params: Params,
        }
        let mut steps: Vec<Step> = Vec::new();
        if let Some(b) = blur {
            let mut src = scene;
            for (i, pass) in b.chain.iter().enumerate() {
                let dst = pong(i, b.chain.len(), t8);
                steps.push(Step {
                    pipeline: &self.filter,
                    src,
                    dst,
                    load: false,
                    params: filter(pass),
                });
                src = dst;
            }
        }
        if let Some(plan) = glow {
            // the setup is the first chain's first pass
            // (`RB_ApplyGlowFilter` from the resolved scene)
            let mut src = scene;
            for (c, passes) in plan.chains.iter().enumerate() {
                let setup = (c == 0).then_some((
                    if plan.film.is_some() {
                        &self.setup4
                    } else {
                        &self.setup
                    },
                    Params {
                        glow: plan.setup,
                        film: plan.film.unwrap_or_default(),
                        ..blank
                    },
                ));
                let total = passes.len() + usize::from(c == 0);
                let all = setup
                    .into_iter()
                    .chain(passes.iter().map(|p| (&self.filter, filter(p))));
                for (i, (pipeline, params)) in all.enumerate() {
                    let dst = pong(i, total, t9);
                    steps.push(Step {
                        pipeline,
                        src,
                        dst,
                        load: false,
                        params,
                    });
                    src = dst;
                }
            }
            for a in &plan.applies {
                let pipeline = if a.sky_bleed != 0.0 {
                    &self.sky_bleed
                } else {
                    &self.bloom
                };
                steps.push(Step {
                    pipeline,
                    src: t9,
                    dst: scene,
                    load: true,
                    params: Params {
                        count: inv_full,
                        glow: [a.sky_bleed, 0.0, 0.0, a.bloom],
                        ..blank
                    },
                });
            }
        }
        if let Some(b) = blur {
            steps.push(Step {
                pipeline: &self.blur_composite,
                src: t8,
                dst: scene,
                load: true,
                params: Params {
                    count: inv_full,
                    glow: [b.alpha, 0.0, 0.0, 0.0],
                    ..blank
                },
            });
        }
        let mut bytes = vec![0u8; steps.len() * SLOT_BYTES as usize];
        for (i, step) in steps.iter().enumerate() {
            let at = i * SLOT_BYTES as usize;
            bytes[at..at + std::mem::size_of::<Params>()]
                .copy_from_slice(bytemuck::bytes_of(&step.params));
        }
        queue.write_buffer(&self.params, 0, &bytes);
        // a view the frame no longer reads (a resize, a new level's sky)
        // keeps its group until the list fills
        if self.groups.len() > MAX_GROUPS {
            self.groups.clear();
        }
        for (slot, step) in steps.iter().enumerate() {
            let at = match self
                .groups
                .iter()
                .position(|(src, sky, _)| src == step.src && sky == cube)
            {
                Some(at) => at,
                None => {
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("cod2 glow bind group"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(step.src),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.params,
                                    offset: 0,
                                    size: wgpu::BufferSize::new(
                                        std::mem::size_of::<Params>() as u64
                                    ),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(cube),
                            },
                        ],
                    });
                    self.groups.push((step.src.clone(), cube.clone(), bg));
                    self.groups.len() - 1
                }
            };
            let bg = &self.groups[at].2;
            let load = if step.load {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cod2 glow"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: step.dst,
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
            pass.set_pipeline(step.pipeline);
            pass.set_bind_group(0, bg, &[slot as u32 * SLOT_BYTES as u32]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn create_targets(device: &wgpu::Device, width: u32, height: u32) -> [wgpu::TextureView; 4] {
    let (w, h) = quarter(width, height);
    std::array::from_fn(|_| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("cod2 glow target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    })
}

#[cfg(test)]
mod tests;
