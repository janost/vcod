//! CoD2's shadow cookies (docs/research/cod2-materials.md 9.8): each frame
//! the scene's model entities nearest for their size (at most 24, the
//! cut-off sliding toward `sc_wantCount`) each draw their depth along the
//! sun into a 128 x 128 cookie, which `sc_blur` blurs. Every visible world
//! surface whose material names a `shadowcookie receiver` technique then
//! writes `1 - alpha * fade * clip` of each cookie into the screen's
//! dynamic shadow (`shadowcookie_receiver.hlsl`), which the lit world and
//! models multiply their sun by at the pixel. vcod keeps the 24 cookies as
//! tiles of one atlas and folds every cookie into one receiver draw per
//! world surface, in a pass ahead of the main one, over the world's depth
//! prepass when it runs; each draw carries the cookies its box can reach
//! ([`Cookies::receiver_masks`]), and its pixels test only those.

use glam::{Mat4, Vec3, Vec4};
use std::ops::Range;
use wgpu::util::DeviceExt;

/// The candidate list's length and so the most cookies a frame draws
/// (`R_GetShadowCandidates`' 24 slots, `gfx_d3d_mp_x86_s.dll` 0x100280f0).
pub(crate) const MAX_COOKIES: usize = 24;
/// `$shadow_cookie`'s side in texels (0x10022f80).
pub(crate) const COOKIE_SIZE: u32 = 128;
const ATLAS_COLS: u32 = 6;
const ATLAS_ROWS: u32 = 4;
pub(crate) const ATLAS_W: u32 = COOKIE_SIZE * ATLAS_COLS;
pub(crate) const ATLAS_H: u32 = COOKIE_SIZE * ATLAS_ROWS;
const _: () = assert!((ATLAS_COLS * ATLAS_ROWS) as usize >= MAX_COOKIES);
/// What a cookie is cleared to before its casters draw (0x10027d20).
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.5,
    g: 0.5,
    b: 0.5,
    a: 0.0,
};
/// The caster box's share of the cookie (`R_ShadowCookieMatrix`'s 0.9375,
/// 0x10027680) and the projection's scale, 126/128 of the side.
const BOX_FILL: f32 = 0.9375;
const PROJ_FILL: f32 = 0.984_375;
/// Where `R_ResetShadowCookies` puts the cut-off at a map load.
const START_CUTOFF: f32 = 10.0;

/// The `sc_*` dvars the cookies read, at the renderer's defaults
/// (`R_RegisterDvars`, Mac 0x000cb8b6, the same pushes in the DLL).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CookieSettings {
    pub enable: bool,
    /// Blur rounds, two passes each.
    pub blur: u32,
    pub want: i32,
    pub want_margin: i32,
    /// The share of the cut-off over which a cookie fades out.
    pub fade_range: f32,
    /// Per second, how fast the cut-off falls when too many cookies draw
    /// (`sc_shadowInRate`) and rises when too few (`sc_shadowOutRate`).
    pub in_rate: f32,
    pub out_rate: f32,
    /// Units of depth along the sun per half the cookie's depth range.
    pub length: f32,
    /// `sc_debugCasterCount`, `sc_debugReceiverCount`: the cookies drawn,
    /// and of those the ones whose receivers draw.
    pub casters: usize,
    pub receivers: usize,
}

impl Default for CookieSettings {
    fn default() -> Self {
        CookieSettings {
            enable: true,
            blur: 2,
            want: 12,
            want_margin: 1,
            fade_range: 0.25,
            in_rate: 2.0,
            out_rate: 5.0,
            length: 400.0,
            casters: MAX_COOKIES,
            receivers: MAX_COOKIES,
        }
    }
}

/// One scene entity that may cast: the refEntity origin the candidate
/// pick measures from and its instances, a range of the slice handed to
/// `Renderer::set_dynamic_models`.
#[derive(Clone, Debug, PartialEq)]
pub struct CookieEntity {
    pub origin: Vec3,
    pub parts: Range<usize>,
}

/// A candidate as the pick reads it: origin and the posed world box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// The cut-off on distance over size that sets how many cookies draw,
/// carried from frame to frame (`shadowCookieGlob`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CookieClock {
    pub cutoff: f32,
    last_ms: Option<i32>,
}

impl Default for CookieClock {
    fn default() -> Self {
        CookieClock {
            cutoff: START_CUTOFF,
            last_ms: None,
        }
    }
}

impl CookieClock {
    /// This frame's cookies, nearest for their size first: each candidate's
    /// index and its fade (`shadowParms.a`). Steps the cut-off toward
    /// `sc_wantCount` cookies by the time since the last call
    /// (0x10028330).
    pub fn pick(
        &mut self,
        s: &CookieSettings,
        view: Vec3,
        cands: &[Candidate],
        now_ms: i32,
    ) -> Vec<(usize, f32)> {
        let slots = candidates(view, cands);
        let cutoff = self.cutoff;
        let fade_from = (1.0 - s.fade_range) * cutoff;
        let fade_scale = 1.0 / (cutoff * s.fade_range);
        let mut out = Vec::new();
        // 1: too few cookies under the cut-off, -1: too many
        let mut step = -1;
        for (i, slot) in slots.iter().enumerate() {
            let Some((c, ratio)) = *slot else {
                step = 0;
                break;
            };
            if cutoff < ratio {
                let i = i as i32;
                step = if s.want + s.want_margin < i {
                    -1
                } else if i < s.want - s.want_margin {
                    1
                } else {
                    0
                };
                break;
            }
            let fade = if ratio <= fade_from || ratio == 0.0 {
                1.0
            } else {
                (cutoff - ratio) * fade_scale
            };
            out.push((c, fade));
        }
        let dt = self
            .last_ms
            .map_or(0.0, |l| (now_ms.wrapping_sub(l) as f32 * 0.001).max(0.0));
        self.last_ms = Some(now_ms);
        match step {
            1 => self.cutoff += dt * s.out_rate,
            -1 => self.cutoff -= dt * s.in_rate,
            _ => {}
        }
        self.cutoff = self.cutoff.max(0.0001);
        out
    }
}

/// The 24 candidates with the smallest distance from `view` over their
/// box's longest side, smallest first; `None` past the last
/// (`R_GetShadowCandidates`, 0x100280f0, then the sort by
/// `R_ShadowCandidatePred`). A flat or empty box never casts.
fn candidates(view: Vec3, cands: &[Candidate]) -> [Option<(usize, f32)>; MAX_COOKIES] {
    const EMPTY: f32 = 1e8;
    let mut slots = [(None, EMPTY); MAX_COOKIES];
    let mut worst = 0;
    for (i, c) in cands.iter().enumerate() {
        let size = c.maxs - c.mins;
        let sq = size * size;
        let mut axis = usize::from(sq.x < sq.y);
        if sq[axis] < sq.z {
            axis = 2;
        }
        let side = size[axis];
        if side == 0.0 {
            continue;
        }
        let ratio = (c.origin - view).length() / side;
        if ratio.is_nan() || ratio >= EMPTY || ratio > slots[worst].1 {
            continue;
        }
        slots[worst] = (Some(i), ratio);
        worst = 0;
        for j in 1..MAX_COOKIES {
            if slots[worst].1 < slots[j].1 {
                worst = j;
            }
        }
    }
    slots.sort_by(|a, b| a.1.total_cmp(&b.1));
    slots.map(|(c, r)| c.map(|c| (c, r)))
}

/// The sun's frame for the cookies (0x100275a0): the light's travel
/// `-sun`, then a side and an up off world up, or off x when the sun is
/// within about 18 degrees of overhead.
pub(crate) fn light_axes(sun: Vec3) -> [Vec3; 3] {
    let f = -sun;
    let up = if f.x * f.x + f.y * f.y >= 0.1 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let side = up.cross(f).normalize();
    [f, -side, f.cross(side)]
}

/// One cookie's two matrices, column vectors on world points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CookieMatrices {
    /// To the caster pass's clip space: x and y over the box at 0.9375 of
    /// the cookie, z the depth along the sun about the box's middle over
    /// `2 * sc_length`, 0.5 at the middle.
    pub caster: Mat4,
    /// To the cookie's texture: u, v (0..1 over the whole 128 texels) and
    /// the same depth (`shadowLookupMatrix`).
    pub lookup: Mat4,
}

/// The cookie over the world box `mins..maxs` (`R_ShadowCookieMatrix`
/// 0x10027680 and the lookup `R_AddShadowCookie` 0x10027ac0 makes of
/// it); `None` for a box flat across the sun, which retail divides by
/// zero on.
pub(crate) fn cookie_matrices(
    axes: [Vec3; 3],
    mins: Vec3,
    maxs: Vec3,
    length: f32,
) -> Option<CookieMatrices> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p = Vec3::new(
            if corner & 1 == 0 { mins.x } else { maxs.x },
            if corner & 2 == 0 { mins.y } else { maxs.y },
            if corner & 4 == 0 { mins.z } else { maxs.z },
        );
        for (k, a) in axes.iter().enumerate() {
            let d = p.dot(*a);
            lo[k] = lo[k].min(d);
            hi[k] = hi[k].max(d);
        }
    }
    let row = |k: usize| -> Option<Vec4> {
        let scale = 2.0 / (hi[k] - lo[k]) * BOX_FILL;
        scale
            .is_finite()
            .then(|| axes[k].extend(-(lo[k] + hi[k]) * 0.5) * scale * PROJ_FILL)
    };
    let x = row(1)?;
    let y = row(2)?;
    let z = axes[0].extend(-(lo[0] + hi[0]) * 0.5) * (0.5 / length) + Vec4::W * 0.5;
    let w = Vec4::W;
    let caster = Mat4::from_cols(x, y, z, w).transpose();
    let lookup = Mat4::from_cols(x * 0.5 + w * 0.5, y * -0.5 + w * 0.5, z, w).transpose();
    Some(CookieMatrices { caster, lookup })
}

/// The top-left texel of cookie `i`'s atlas tile.
pub(crate) fn tile_origin(i: usize) -> (u32, u32) {
    let i = i as u32;
    (
        (i % ATLAS_COLS) * COOKIE_SIZE,
        (i / ATLAS_COLS) * COOKIE_SIZE,
    )
}

/// [`Cookies::receiver_masks`] over `lookups`.
fn box_mask(lookups: &[Mat4], bounds: Option<(Vec3, Vec3)>) -> u32 {
    let all = (1u32 << lookups.len()) - 1;
    let Some((lo, hi)) = bounds else {
        return all;
    };
    let (mid, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    let reaches = |r: Vec4| {
        let at = r.truncate().dot(mid) + r.w;
        let ext = r.truncate().abs().dot(half);
        // NaN (an empty box) keeps the cookie
        !(at + ext < -MASK_MARGIN || at - ext > 1.0 + MASK_MARGIN)
    };
    lookups
        .iter()
        .enumerate()
        .filter(|(_, l)| reaches(l.row(0)) && reaches(l.row(1)))
        .fold(0, |m, (i, _)| m | 1 << i)
}

/// The cookie WGSL (`cod2_cookie.wgsl`).
pub(super) const COOKIE_WGSL: &str = include_str!("../cod2_cookie.wgsl");
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// `$dynamic_shadows`' format, as the frame buffer's (0x10023110).
const DYNAMIC_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// One caster matrix per dynamic-offset slot.
const SLOT: u64 = 256;
/// How far outside a cookie's square, in its texture's 0..1, a box still
/// counts as reaching it ([`Cookies::receiver_masks`]): room for the float
/// rounding of the shader's lookup, about 1e-5 at a map's far corners.
const MASK_MARGIN: f32 = 1.0 / 1024.0;
/// `Cookies` in the WGSL: a count, then 24 of a matrix and a vec4.
const RECV_BYTES: u64 = 16 + MAX_COOKIES as u64 * 80;

/// One cookie this frame: the candidate it was cast for, its matrices and
/// its fade.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameCookie {
    pub candidate: usize,
    pub matrices: CookieMatrices,
    pub fade: f32,
}

/// One entity surface drawn into cookie `cookie`.
pub(crate) struct CasterDraw<'a> {
    pub cookie: usize,
    pub vertex_buf: &'a wgpu::Buffer,
    pub index_buf: &'a wgpu::Buffer,
    pub indices: Range<u32>,
    pub base_vertex: i32,
    /// The instance's slot in `instance_buf`.
    pub instance: u32,
}

/// The cookies' state across frames and their GPU side.
pub(crate) struct Cookies {
    pub settings: CookieSettings,
    pub clock: CookieClock,
    /// This frame's entities from the scene.
    pub entities: Vec<CookieEntity>,
    /// Cookies drawn this frame, and of those the ones that darken.
    live: usize,
    receive: usize,
    /// The receiving cookies' lookups, for [`Self::receiver_masks`].
    lookups: Vec<Mat4>,
    atlas: [wgpu::TextureView; 2],
    depth: wgpu::TextureView,
    caster_buf: wgpu::Buffer,
    caster_bg: wgpu::BindGroup,
    caster_pipeline: wgpu::RenderPipeline,
    blur_bgs: [wgpu::BindGroup; 2],
    blur_pipeline: wgpu::RenderPipeline,
    recv_buf: wgpu::Buffer,
    recv_bg: wgpu::BindGroup,
    recv_pipeline: wgpu::RenderPipeline,
    /// The receivers over the scene's world depth prepass: LessEqual, no
    /// depth write, as retail's (`shadowcookie_receiver.sm`).
    recv_on_prepass: wgpu::RenderPipeline,
    /// The receivers' per world set view (`view_proj`, `model`), one
    /// dynamic-offset slot each; grown on demand.
    view_layout: wgpu::BindGroupLayout,
    view_buf: Option<(wgpu::Buffer, wgpu::BindGroup, usize)>,
}

impl Cookies {
    /// `bones` is the dynamic models' bone group's layout.
    pub fn new(device: &wgpu::Device, bones: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod2 cookie shader"),
            source: wgpu::ShaderSource::Wgsl(COOKIE_WGSL.into()),
        });
        let texture = |label, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: ATLAS_W,
                        height: ATLAS_H,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let colour = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST;
        let atlas = [
            texture("cod2 cookie atlas", ATLAS_FORMAT, colour),
            texture("cod2 cookie blur", ATLAS_FORMAT, colour),
        ];
        let depth = texture(
            "cod2 cookie depth",
            super::DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cod2 cookie sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = |binding, visibility, dynamic, size| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
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
        let samp = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        // casters
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 cookie caster layout"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX, true, 64)],
        });
        let caster_buf = buffer("cod2 cookie casters", SLOT * MAX_COOKIES as u64);
        let caster_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cod2 cookie casters"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &caster_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 cookie caster pipeline layout"),
            bind_group_layouts: &[Some(&caster_layout), Some(bones)],
            immediate_size: 0,
        });
        let [vertex, instance] = crate::entity_material::vertex_layouts();
        let caster_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cod2 cookie casters"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_caster"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex), Some(instance)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_caster"),
                compilation_options: Default::default(),
                targets: &[Some(ATLAS_FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: super::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // blur
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 cookie blur layout"),
            entries: &[
                tex(0),
                samp(1),
                uniform(2, wgpu::ShaderStages::FRAGMENT, false, 16),
            ],
        });
        // the first pass of a round reads the atlas half a texel right
        // and down, the second the blur target half a texel back
        let blur_bgs = [(0, 0.5f32), (1, -0.5)].map(|(src, shift)| {
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cod2 cookie blur shift"),
                contents: bytemuck::cast_slice(&[shift, 0.0, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cod2 cookie blur"),
                layout: &blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&atlas[src]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buf.as_entire_binding(),
                    },
                ],
            })
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 cookie blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cod2 cookie blur"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_blur"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_blur"),
                compilation_options: Default::default(),
                targets: &[Some(ATLAS_FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // receivers, into the screen's dynamic shadow
        let recv_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 cookie receiver layout"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT, false, RECV_BYTES),
                tex(1),
                samp(2),
            ],
        });
        let recv_buf = buffer("cod2 cookie receivers", RECV_BYTES);
        let recv_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cod2 cookie receivers"),
            layout: &recv_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: recv_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 cookie receiver view layout"),
            entries: &[uniform(0, wgpu::ShaderStages::VERTEX, true, 128)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 cookie receiver pipeline layout"),
            bind_group_layouts: &[Some(&view_layout), Some(&recv_layout)],
            immediate_size: 0,
        });
        let recv = |write: bool, compare| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cod2 cookie receivers"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_receive"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        // the world's vertex (renderer::cod2's lit pipeline)
                        array_stride: 44,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_receive"),
                    compilation_options: Default::default(),
                    targets: &[Some(DYNAMIC_FORMAT.into())],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: super::DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // on its own depth, the nearest receiver's keep
        let recv_pipeline = recv(true, wgpu::CompareFunction::Less);
        let recv_on_prepass = recv(false, wgpu::CompareFunction::LessEqual);
        Cookies {
            settings: CookieSettings::default(),
            clock: CookieClock::default(),
            entities: Vec::new(),
            live: 0,
            receive: 0,
            lookups: Vec::new(),
            atlas,
            depth,
            caster_buf,
            caster_bg,
            caster_pipeline,
            blur_bgs,
            blur_pipeline,
            recv_buf,
            recv_bg,
            recv_pipeline,
            recv_on_prepass,
            view_layout,
            view_buf: None,
        }
    }

    /// Picks this frame's cookies off `cands` (indexed as `entities`) and
    /// writes their uniforms; none when off.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        sun: Vec3,
        view: Vec3,
        cands: &[Candidate],
        now_ms: i32,
    ) -> Vec<FrameCookie> {
        let s = self.settings;
        let mut out = Vec::new();
        if s.enable {
            let axes = light_axes(sun);
            for (candidate, fade) in self.clock.pick(&s, view, cands, now_ms) {
                let c = &cands[candidate];
                if let Some(matrices) = cookie_matrices(axes, c.mins, c.maxs, s.length) {
                    out.push(FrameCookie {
                        candidate,
                        matrices,
                        fade,
                    });
                }
            }
            out.truncate(s.casters.min(MAX_COOKIES));
        }
        self.live = out.len();
        self.receive = out.len().min(s.receivers);
        self.lookups = out[..self.receive]
            .iter()
            .map(|c| c.matrices.lookup)
            .collect();
        let mut casters = vec![0u8; (SLOT * MAX_COOKIES as u64) as usize];
        let mut recv = vec![0u8; RECV_BYTES as usize];
        recv[..4].copy_from_slice(&(self.receive as u32).to_le_bytes());
        for (i, c) in out.iter().enumerate() {
            let at = i * SLOT as usize;
            casters[at..at + 64]
                .copy_from_slice(bytemuck::cast_slice(&c.matrices.caster.to_cols_array()));
            let at = 16 + i * 80;
            recv[at..at + 64]
                .copy_from_slice(bytemuck::cast_slice(&c.matrices.lookup.to_cols_array()));
            let (x, y) = tile_origin(i);
            recv[at + 64..at + 80]
                .copy_from_slice(bytemuck::cast_slice(&[x as f32, y as f32, c.fade, 0.0]));
        }
        if self.live > 0 {
            queue.write_buffer(&self.caster_buf, 0, &casters[..self.live * SLOT as usize]);
        }
        queue.write_buffer(&self.recv_buf, 0, &recv);
        out
    }

    /// The receiving cookies whose square a receiver inside the world box
    /// `lo..hi` can fall in, bit `i` cookie `i`: a receiver draw's first
    /// instance, which `fs_receive` tests in place of every cookie. The
    /// lookup is affine, so the box's reach along its u and v rows is
    /// exact; a cookie left out would have skipped every pixel of the box,
    /// so the dynamic shadow is the same. Every receiving cookie without
    /// a box. The closure holds this frame's lookups, apart from `self`.
    pub fn receiver_masks(&self) -> impl Fn(Option<(Vec3, Vec3)>) -> u32 + use<> {
        let lookups = self.lookups.clone();
        move |bounds| box_mask(&lookups, bounds)
    }

    /// The cookies drawn this frame.
    #[cfg(test)]
    pub fn live(&self) -> usize {
        self.live
    }

    /// Clears the atlas, draws each cookie's casters into its tile's inner
    /// 126 texels (`R_DrawShadowCookies`' viewport 1, 1, 126, 126) and
    /// runs `sc_blur` rounds over the tiles.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[CasterDraw<'_>],
        instance_buf: &wgpu::Buffer,
        instance_stride: u64,
        bones: &wgpu::BindGroup,
    ) {
        if self.live == 0 {
            return;
        }
        {
            let mut pass = self.atlas_pass(encoder, 0, true);
            pass.set_pipeline(&self.caster_pipeline);
            pass.set_bind_group(1, bones, &[]);
            let mut cookie = None;
            for d in draws.iter().filter(|d| d.cookie < self.live) {
                if cookie != Some(d.cookie) {
                    cookie = Some(d.cookie);
                    let (x, y) = tile_origin(d.cookie);
                    let inner = (COOKIE_SIZE - 2) as f32;
                    pass.set_viewport(x as f32 + 1.0, y as f32 + 1.0, inner, inner, 0.0, 1.0);
                    pass.set_scissor_rect(x + 1, y + 1, COOKIE_SIZE - 2, COOKIE_SIZE - 2);
                    pass.set_bind_group(0, &self.caster_bg, &[(d.cookie as u64 * SLOT) as u32]);
                }
                let off = u64::from(d.instance) * instance_stride;
                pass.set_vertex_buffer(0, d.vertex_buf.slice(..));
                pass.set_vertex_buffer(1, instance_buf.slice(off..off + instance_stride));
                pass.set_index_buffer(d.index_buf.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(d.indices.clone(), d.base_vertex, 0..1);
            }
        }
        self.encode_blur(encoder);
    }

    /// `sc_blur` rounds of two passes, atlas to blur target and back.
    fn encode_blur(&self, encoder: &mut wgpu::CommandEncoder) {
        for _ in 0..self.settings.blur {
            self.blur_pass(encoder, 0);
            self.blur_pass(encoder, 1);
        }
    }

    /// One blur pass out of atlas `from` into the other.
    fn blur_pass(&self, encoder: &mut wgpu::CommandEncoder, from: usize) {
        let mut pass = self.atlas_pass(encoder, 1 - from, false);
        pass.set_pipeline(&self.blur_pipeline);
        pass.set_bind_group(0, &self.blur_bgs[from], &[]);
        pass.draw(0..6, 0..self.live as u32);
    }

    /// A pass on atlas `which`, cleared to the cookies' clear colour; with
    /// the depth when `depth`.
    fn atlas_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        which: usize,
        depth: bool,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cod2 cookies"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.atlas[which],
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth.then_some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Draws the screen's dynamic shadow (`$dynamic_shadows`, render
    /// target 3): cleared, then the nearest receiving world surface at
    /// each pixel writes the share of the sun every cookie together leaves.
    /// `views` are each world set's view-projection and pose; `draw` binds
    /// the world's buffers and draws set `n`'s visible receivers, each
    /// draw's first instance its [`Self::receiver_masks`]. With no
    /// cookie the target is only cleared, and only when the last frame
    /// wrote it. With `prepass`, the scene's depth after its world
    /// prepass, a receiver draws where it is no farther than that depth,
    /// as retail's do over theirs, and writes none; without, the
    /// receivers keep a depth of their own, the nearest one's keep.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_receivers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &mut DynamicShadows,
        prepass: Option<&wgpu::TextureView>,
        views: &[(Mat4, Mat4)],
        mut draw: impl FnMut(&mut wgpu::RenderPass<'_>, usize),
    ) {
        if self.receive == 0 {
            if target.dirty {
                target.dirty = false;
                drop(target.pass(encoder, None));
            }
            return;
        }
        if self.view_buf.as_ref().is_none_or(|v| v.2 < views.len()) {
            let n = views.len().next_power_of_two().max(8);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cod2 cookie receiver views"),
                size: SLOT * n as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cod2 cookie receiver views"),
                layout: &self.view_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(128),
                    }),
                }],
            });
            self.view_buf = Some((buf, bg, n));
        }
        let (buf, bg, _) = self.view_buf.as_ref().expect("made above");
        let mut bytes = vec![0u8; views.len() * SLOT as usize];
        for (i, (view_proj, m)) in views.iter().enumerate() {
            let at = i * SLOT as usize;
            bytes[at..at + 64].copy_from_slice(bytemuck::cast_slice(&view_proj.to_cols_array()));
            bytes[at + 64..at + 128].copy_from_slice(bytemuck::cast_slice(&m.to_cols_array()));
        }
        queue.write_buffer(buf, 0, &bytes);
        target.dirty = true;
        let mut pass = target.pass(encoder, prepass);
        pass.set_pipeline(match prepass {
            Some(_) => &self.recv_on_prepass,
            None => &self.recv_pipeline,
        });
        pass.set_bind_group(1, &self.recv_bg, &[]);
        for set in 0..views.len() {
            pass.set_bind_group(0, bg, &[(set as u64 * SLOT) as u32]);
            draw(&mut pass, set);
        }
    }
}

/// The screen's dynamic shadow the lit world and models read their sun
/// through (`dynamicShadowSampler`, lib/lightmap.hlsl and
/// lib/lightprobe.hlsl), and its depth; screen sized, single sample. It
/// holds retail's, the share of the sun left, so a cleared or new one
/// (white) takes none; the lit shaders read it through screen coordinates
/// their vertex writes, as retail's `tex2Dproj` (cod2-client-perf.md 10).
pub(crate) struct DynamicShadows {
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// Some receiver wrote it since its last clear.
    dirty: bool,
}

impl DynamicShadows {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Self {
        use wgpu::util::DeviceExt;
        let desc = |label, format, usage| wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        let att = wgpu::TextureUsages::RENDER_ATTACHMENT;
        DynamicShadows {
            // white: no shadow
            view: device
                .create_texture_with_data(
                    queue,
                    &desc(
                        "cod2 dynamic shadows",
                        DYNAMIC_FORMAT,
                        att | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    ),
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &vec![255; (4 * width * height) as usize],
                )
                .create_view(&Default::default()),
            depth: device
                .create_texture(&desc("cod2 dynamic shadow depth", super::DEPTH_FORMAT, att))
                .create_view(&Default::default()),
            dirty: false,
        }
    }

    /// What the lit shaders sample.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Its pixels the cookies take any sun at.
    #[cfg(test)]
    pub fn shaded(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> usize {
        let t = self.view.texture();
        let got = tests::read_rgba(device, queue, t, (t.width(), t.height()));
        got.chunks(4).filter(|p| p[0] < 255).count()
    }

    /// The pass the receivers draw in: on `prepass`, kept, or on its own
    /// depth, cleared.
    fn pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        prepass: Option<&wgpu::TextureView>,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cod2 dynamic shadows"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: prepass.unwrap_or(&self.depth),
                depth_ops: Some(match prepass {
                    Some(_) => wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    None => wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    },
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The receiver's darkening at a world point: what retail's blend
    /// multiplies the scene by, one cookie's `texel` (rgba, 0..1) being
    /// the bilinear sample at the point's cookie texture position
    /// (`shadowcookie_receiver.hlsl`).
    fn receiver_keep(depth: f32, texel: [f32; 4], fade: f32) -> f32 {
        let clip = ((depth - texel[2]) * 64.0 + 0.5).clamp(0.0, 1.0);
        1.0 - texel[3] * fade * clip
    }

    /// A box's mask holds every cookie whose square a point in it falls
    /// in, as `fs_receive` tests it, and leaves out ones it cannot reach.
    #[test]
    fn receiver_mask_keeps_every_cookie_a_point_in_the_box_reaches() {
        let axes = light_axes(Vec3::new(-0.4, 0.3, 0.86).normalize());
        let lookups: Vec<Mat4> = [Vec3::new(100.0, 40.0, 0.0), Vec3::new(-900.0, 300.0, 20.0)]
            .iter()
            .map(|&o| {
                let m = cookie_matrices(
                    axes,
                    o - Vec3::new(16.0, 16.0, 0.0),
                    o + Vec3::new(16.0, 16.0, 72.0),
                    400.0,
                );
                m.expect("a box").lookup
            })
            .collect();
        let near = (Vec3::new(40.0, -20.0, -8.0), Vec3::new(160.0, 100.0, 0.0));
        assert_eq!(box_mask(&lookups, Some(near)), 0b01);
        let far = (
            Vec3::new(3000.0, 3000.0, 0.0),
            Vec3::new(3100.0, 3100.0, 8.0),
        );
        assert_eq!(box_mask(&lookups, Some(far)), 0);
        assert_eq!(box_mask(&lookups, None), 0b11);
        // points scattered over boxes around the casters
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..2000 {
            let lo = Vec3::new(
                next() * 2400.0 - 1400.0,
                next() * 800.0 - 300.0,
                next() * 200.0 - 100.0,
            );
            let hi = lo + Vec3::new(next(), next(), next()) * 300.0;
            let mask = box_mask(&lookups, Some((lo, hi)));
            for _ in 0..8 {
                let p = lo + (hi - lo) * Vec3::new(next(), next(), next());
                for (i, l) in lookups.iter().enumerate() {
                    let t = (*l * p.extend(1.0)).truncate().truncate() * 128.0;
                    let inside =
                        t.cmpge(glam::Vec2::ZERO).all() && t.cmple(glam::Vec2::splat(128.0)).all();
                    assert!(
                        !inside || mask & 1 << i != 0,
                        "cookie {i} reaches {p} in {lo}..{hi}"
                    );
                }
            }
        }
    }

    fn cand(origin: Vec3, half: f32) -> Candidate {
        Candidate {
            origin,
            mins: origin - Vec3::splat(half),
            maxs: origin + Vec3::splat(half),
        }
    }

    /// Thirty boxes of one size in a row: the 24 nearest come back
    /// nearest first; a flat box never does.
    #[test]
    fn the_24_nearest_for_their_size_are_the_candidates() {
        let mut cands: Vec<Candidate> = (0..30)
            .rev()
            .map(|i| cand(Vec3::X * (100.0 + 10.0 * i as f32), 8.0))
            .collect();
        cands.push(Candidate {
            origin: Vec3::X,
            mins: Vec3::X,
            maxs: Vec3::X,
        });
        let slots = candidates(Vec3::ZERO, &cands);
        let picked: Vec<usize> = slots.iter().map(|s| s.unwrap().0).collect();
        assert_eq!(picked, (6..30).rev().collect::<Vec<_>>());
        assert_eq!(slots[0].unwrap().1, 100.0 / 16.0);
    }

    /// The cut-off at 10 takes the boxes up to ten of their sides away,
    /// fading the last quarter; with fewer than `sc_wantCount - margin`
    /// under it, it rises at 5 a second, with all 24 under it it falls
    /// at 2.
    #[test]
    fn the_cutoff_fades_the_far_cookies_and_slides_toward_want() {
        let s = CookieSettings::default();
        let cands: Vec<Candidate> = [4.0, 8.0, 9.0, 11.0]
            .iter()
            .map(|r| cand(Vec3::X * 16.0 * r, 8.0))
            .collect();
        let mut clock = CookieClock::default();
        let picked = clock.pick(&s, Vec3::ZERO, &cands, 1000);
        assert_eq!(picked.len(), 3);
        assert_eq!(picked[0], (0, 1.0));
        assert!((picked[1].1 - 0.8).abs() < 1e-6, "{picked:?}");
        assert!((picked[2].1 - 0.4).abs() < 1e-6, "{picked:?}");
        // the first frame has no time step
        assert_eq!(clock.cutoff, 10.0);
        clock.pick(&s, Vec3::ZERO, &cands, 1500);
        assert_eq!(clock.cutoff, 12.5);

        let many: Vec<Candidate> = (0..30)
            .map(|i| cand(Vec3::X * (20.0 + i as f32), 8.0))
            .collect();
        let mut clock = CookieClock::default();
        clock.pick(&s, Vec3::ZERO, &many, 0);
        assert_eq!(clock.pick(&s, Vec3::ZERO, &many, 250).len(), 24);
        assert_eq!(clock.cutoff, 9.5);
        // twelve under the cut-off holds it
        let mut clock = CookieClock::default();
        clock.pick(&s, Vec3::ZERO, &many[..12], 0);
        let mut more = many[..12].to_vec();
        more.push(cand(Vec3::X * 16.0 * 20.0, 8.0));
        clock.pick(&s, Vec3::ZERO, &more, 1000);
        assert_eq!(clock.cutoff, 10.0);
    }

    /// The axes are the light's travel, a right and an up seen from the
    /// sun (up x right is the travel), and the
    /// two matrices put the box's corners where the caster pass and the
    /// receiver lookup expect them: x and y within 0.9375 * 126/128 of the
    /// clip square, the texture's v down, the depth 0.5 at the middle and
    /// one `sc_length` away at 0 and 1.
    #[test]
    fn the_matrices_fit_the_box_and_agree_on_depth() {
        let sun = Vec3::new(-0.5, 0.3, 0.8).normalize();
        let axes = light_axes(sun);
        assert!((axes[0] + sun).length() < 1e-6);
        assert!((axes[2].cross(axes[1]) - axes[0]).length() < 1e-5);
        let (mins, maxs) = (Vec3::new(10.0, -20.0, 0.0), Vec3::new(40.0, 10.0, 72.0));
        let m = cookie_matrices(axes, mins, maxs, 400.0).unwrap();
        let mut hi = Vec3::ZERO;
        for c in 0..8 {
            let p = Vec3::new(
                if c & 1 == 0 { mins.x } else { maxs.x },
                if c & 2 == 0 { mins.y } else { maxs.y },
                if c & 4 == 0 { mins.z } else { maxs.z },
            );
            let clip = m.caster.project_point3(p);
            hi = hi.max(clip.abs());
            let uv = m.lookup.project_point3(p);
            assert!((uv.x - (0.5 + 0.5 * clip.x)).abs() < 1e-6);
            assert!((uv.y - (0.5 - 0.5 * clip.y)).abs() < 1e-6);
            assert_eq!(uv.z, clip.z);
        }
        assert!((hi.x - BOX_FILL * PROJ_FILL).abs() < 1e-5, "{hi}");
        assert!((hi.y - BOX_FILL * PROJ_FILL).abs() < 1e-5, "{hi}");
        let mid = (mins + maxs) * 0.5;
        assert!((m.caster.project_point3(mid).z - 0.5).abs() < 1e-6);
        let far = m.caster.project_point3(mid + axes[0] * 400.0).z;
        assert!((far - 1.0).abs() < 1e-5, "{far}");
        // up the side axis is right on the cookie, up the up axis the top
        let r = m.lookup.project_point3(mid + axes[1] * 5.0);
        assert!(r.x > 0.5 && (r.y - 0.5).abs() < 1e-6);
        let u = m.lookup.project_point3(mid + axes[2] * 5.0);
        assert!(u.y < 0.5 && (u.x - 0.5).abs() < 1e-6);
        // a sun near overhead takes x for the up hint
        let top = light_axes(Vec3::new(0.1, 0.0, 1.0).normalize());
        assert!((top[2].cross(top[1]) - top[0]).length() < 1e-5);
        assert!(cookie_matrices(axes, mins, mins, 400.0).is_none());
    }

    #[test]
    fn a_receiver_behind_its_caster_darkens_by_the_cookie() {
        assert_eq!(receiver_keep(0.6, [0.5, 0.5, 0.5, 1.0], 1.0), 0.0);
        assert_eq!(receiver_keep(0.5, [0.5, 0.5, 0.5, 1.0], 1.0), 0.5);
        assert_eq!(receiver_keep(0.4, [0.5, 0.5, 0.5, 1.0], 1.0), 1.0);
        assert_eq!(receiver_keep(0.6, [0.5, 0.5, 0.5, 0.0], 1.0), 1.0);
        assert_eq!(receiver_keep(0.6, [0.5, 0.5, 0.5, 0.5], 0.5), 0.75);
    }

    // ---- on a device: lavapipe, or any non-GL adapter ----

    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let info = adapter.get_info();
        eprintln!("adapter: {} {:?}", info.name, info.backend);
        if info.backend == wgpu::Backend::Gl {
            return None;
        }
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    /// The dynamic models' bone group layout the caster pipeline takes.
    fn bones_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[storage(0), storage(1)],
        })
    }

    /// `tex`, `w` x `h` RGBA bytes.
    pub(super) fn read_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tex: &wgpu::Texture,
        (w, h): (u32, u32),
    ) -> Vec<u8> {
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap();
        (0..h as usize)
            .flat_map(|y| data[y * row as usize..][..w as usize * 4].to_vec())
            .collect()
    }

    fn write_atlas(queue: &wgpu::Queue, tex: &wgpu::Texture, bytes: &[u8]) {
        queue.write_texture(
            tex.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W * 4),
                rows_per_image: Some(ATLAS_H),
            },
            wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Bilinear, clamped to the atlas's edge, at `q` in texel units (texel
    /// `k`'s centre at `k + 0.5`), of RGBA bytes as 0..1.
    fn bilinear(atlas: &[u8], q: [f32; 2]) -> [f32; 4] {
        let (x, y) = (q[0] - 0.5, q[1] - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |ix: f32, iy: f32, c: usize| {
            let ix = (ix as i32).clamp(0, ATLAS_W as i32 - 1) as usize;
            let iy = (iy as i32).clamp(0, ATLAS_H as i32 - 1) as usize;
            f32::from(atlas[(iy * ATLAS_W as usize + ix) * 4 + c]) / 255.0
        };
        std::array::from_fn(|c| {
            let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1.0, y0, c) * fx;
            let bot = at(x0, y0 + 1.0, c) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0, c) * fx;
            top * (1.0 - fy) + bot * fy
        })
    }

    /// One blur pass on the CPU: each live tile's inner texels the mean
    /// of four bilinear taps a texel off, read `shift` texels along both
    /// axes (the stretch's half texel), the rest `ring`.
    fn cpu_blur(src: &[u8], live: usize, shift: f32, ring: [u8; 4]) -> Vec<u8> {
        let mut out: Vec<u8> = ring.repeat((ATLAS_W * ATLAS_H) as usize);
        for t in 0..live {
            let (ox, oy) = tile_origin(t);
            for y in oy + 1..oy + COOKIE_SIZE - 1 {
                for x in ox + 1..ox + COOKIE_SIZE - 1 {
                    let c = [x as f32 + 0.5 + shift, y as f32 + 0.5 + shift];
                    let taps = [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]]
                        .map(|[dx, dy]| bilinear(src, [c[0] + dx, c[1] + dy]));
                    for ch in 0..4 {
                        let v = taps.iter().map(|t| t[ch]).sum::<f32>() * 0.25;
                        out[((y * ATLAS_W + x) * 4) as usize + ch] = (v * 255.0).round() as u8;
                    }
                }
            }
        }
        out
    }

    /// Two blur rounds over two tiles of a hard-edged caster: each pass
    /// matches the CPU's over the same input to a byte (the second of a
    /// round reading half a texel back), the ring keeps the clear.
    #[test]
    fn the_blur_matches_the_cpu_passes() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let mut c = Cookies::new(&device, &bones_layout(&device));
        c.live = 2;
        c.settings.blur = 2;
        let clear = [128u8, 128, 128, 0];
        let mut atlas: Vec<u8> = clear.repeat((ATLAS_W * ATLAS_H) as usize);
        for t in 0..3 {
            let (ox, oy) = tile_origin(t);
            for y in 30..90 {
                for x in 20 + t as u32 * 10..70 {
                    let z = 100 + (x + y) as u8 / 4;
                    let at = (((oy + y) * ATLAS_W + ox + x) * 4) as usize;
                    atlas[at..at + 4].copy_from_slice(&[z, z, z, 255]);
                }
            }
        }
        write_atlas(&queue, c.atlas[0].texture(), &atlas);
        // pass by pass, each against the CPU's pass over the GPU's input
        let mut input = atlas.clone();
        let mut worst = 0;
        for (pass, from) in [0, 1, 0, 1].into_iter().enumerate() {
            let mut enc = device.create_command_encoder(&Default::default());
            c.blur_pass(&mut enc, from);
            queue.submit([enc.finish()]);
            let out = read_rgba(
                &device,
                &queue,
                c.atlas[1 - from].texture(),
                (ATLAS_W, ATLAS_H),
            );
            let ring: [u8; 4] = out[..4].try_into().unwrap();
            assert!(ring[3] == 0 && ring[0].abs_diff(128) <= 1, "{ring:?}");
            let shift = if from == 0 { 0.5 } else { -0.5 };
            let want = cpu_blur(&input, 2, shift, ring);
            let off = out
                .iter()
                .zip(&want)
                .map(|(g, w)| g.abs_diff(*w))
                .max()
                .unwrap();
            eprintln!("pass {pass}: off by {off}");
            worst = worst.max(off);
            input = out;
        }
        assert!(worst <= 1, "blur off by {worst}");
        // the whole run, through encode_blur, lands where the passes did
        write_atlas(&queue, c.atlas[0].texture(), &atlas);
        let mut enc = device.create_command_encoder(&Default::default());
        c.encode_blur(&mut enc);
        queue.submit([enc.finish()]);
        let got = read_rgba(&device, &queue, c.atlas[0].texture(), (ATLAS_W, ATLAS_H));
        assert!(got == input, "encode_blur differs from its passes");
        // the blur spread the caster's edge over two texels each way
        let (ox, oy) = tile_origin(1);
        let edge = |x: u32| got[(((oy + 60) * ATLAS_W + ox + x) * 4 + 3) as usize];
        assert!(edge(29) > 0 && edge(29) < 255 && edge(27) > 0 && edge(32) < 255);
    }

    /// A tilted square, drawn as an entity surface into cookie 0 with the
    /// sun overhead, fills the texels its footprint covers with its depth
    /// along the sun (rgb) and full alpha, as the caster matrix puts it in
    /// the tile's inner 126 texels; the rest of the tile keeps the clear.
    #[test]
    fn a_caster_writes_its_depth_where_it_covers() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let bones_layout = bones_layout(&device);
        let mut c = Cookies::new(&device, &bones_layout);
        c.settings.blur = 0;
        // corners at (+-24, +-16) rising 1 in 2 along x, at 1000 up
        let corner = |x: f32, y: f32| Vec3::new(x, y, 1000.0 + 0.5 * x);
        let quad = [
            corner(-24.0, -16.0),
            corner(24.0, -16.0),
            corner(24.0, 16.0),
            corner(-24.0, 16.0),
        ];
        let verts: Vec<vcod_common::model::Vertex> = quad
            .iter()
            .map(|p| vcod_common::model::Vertex {
                pos: p.to_array(),
                normal: [0.0, 0.0, 1.0],
                uv: [0.0; 2],
                bone_indices: [0; 4],
                bone_weights: [1.0, 0.0, 0.0, 0.0],
                color: [255; 4],
                tangent: [0.0; 3],
                binormal: [0.0; 3],
            })
            .collect();
        let init = |contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            })
        };
        let vb = init(bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let ib = init(
            bytemuck::cast_slice(&[0u16, 1, 2, 0, 2, 3]),
            wgpu::BufferUsages::INDEX,
        );
        let instance = super::super::InstanceRaw {
            transform: Mat4::IDENTITY.to_cols_array(),
            bone_base: 0,
            light_set: 0,
            time_offset: 0.0,
            _pad: 0,
        };
        let inst = init(bytemuck::bytes_of(&instance), wgpu::BufferUsages::VERTEX);
        let storage = wgpu::BufferUsages::STORAGE;
        let bone_buf = init(
            bytemuck::cast_slice(&Mat4::IDENTITY.to_cols_array()),
            storage,
        );
        let light_buf = init(&[0u8; 256], storage);
        let bones = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bones_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: bone_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buf.as_entire_binding(),
                },
            ],
        });
        let (mins, maxs) = (
            quad.iter().fold(Vec3::INFINITY, |a, p| a.min(*p)),
            quad.iter().fold(Vec3::NEG_INFINITY, |a, p| a.max(*p)),
        );
        let cand = Candidate {
            origin: (mins + maxs) * 0.5,
            mins,
            maxs,
        };
        let cookies = c.prepare(&queue, Vec3::Z, cand.origin - Vec3::X * 100.0, &[cand], 0);
        assert_eq!(cookies.len(), 1);
        let m = cookies[0].matrices;
        let mut enc = device.create_command_encoder(&Default::default());
        c.encode(
            &mut enc,
            &[CasterDraw {
                cookie: 0,
                vertex_buf: &vb,
                index_buf: &ib,
                indices: 0..6,
                base_vertex: 0,
                instance: 0,
            }],
            &inst,
            super::super::DYNAMIC_INSTANCE_STRIDE,
            &bones,
        );
        queue.submit([enc.finish()]);
        let got = read_rgba(&device, &queue, c.atlas[0].texture(), (ATLAS_W, ATLAS_H));
        // the square in the tile's texels, and its depth over them
        let to_texel = |p: Vec3| {
            let clip = m.caster.project_point3(p);
            let tx = 1.0 + (clip.x + 1.0) * 0.5 * 126.0;
            let ty = 1.0 + (1.0 - clip.y) * 0.5 * 126.0;
            (Vec3::new(tx, ty, 0.0), clip.z)
        };
        let t: Vec<(Vec3, f32)> = quad.iter().map(|p| to_texel(*p)).collect();
        let (lo, hi) = t
            .iter()
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(a, b), (p, _)| {
                (a.min(*p), b.max(*p))
            });
        // the box fills 0.9375 of the projection's 126/128, over the
        // viewport's 126 texels: retail scales twice (9.8)
        assert!(
            (hi.x - lo.x - 126.0 * BOX_FILL * PROJ_FILL).abs() < 0.01,
            "{lo} {hi}"
        );
        let (mut inside, mut outside) = (0, 0);
        for y in 0..COOKIE_SIZE {
            for x in 0..COOKIE_SIZE {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let texel = &got[((y * ATLAS_W + x) * 4) as usize..][..4];
                let margin = |v: f32, a: f32, b: f32| (v - a).min(b - v);
                let m = margin(px, lo.x, hi.x).min(margin(py, lo.y, hi.y));
                if m > 0.75 {
                    // depth is affine over the square
                    let z = depth_at(&t, px, py);
                    let want = (z * 255.0).round() as u8;
                    assert!(
                        texel[3] == 255 && texel[0].abs_diff(want) <= 1,
                        "({x}, {y}): {texel:?} want {want}"
                    );
                    assert_eq!(texel[0], texel[2]);
                    inside += 1;
                } else if m < -0.75 {
                    assert_eq!(texel[3], 0, "({x}, {y})");
                    outside += 1;
                }
            }
        }
        assert!(inside > 8000 && outside > 1000, "{inside} {outside}");
        // past the live tile nothing drew
        let (ox, oy) = tile_origin(1);
        assert_eq!(got[(((oy + 64) * ATLAS_W + ox + 64) * 4 + 3) as usize], 0);
    }

    /// The depth at texel point (x, y) of the plane through the first
    /// three texel corners `t` (texel position, depth).
    fn depth_at(t: &[(Vec3, f32)], x: f32, y: f32) -> f32 {
        let (a, b, c) = (t[0], t[1], t[2]);
        let (e1, e2) = (b.0 - a.0, c.0 - a.0);
        let det = e1.x * e2.y - e1.y * e2.x;
        let (dx, dy) = (x - a.0.x, y - a.0.y);
        let u = (dx * e2.y - dy * e2.x) / det;
        let v = (e1.x * dy - e1.y * dx) / det;
        a.1 + u * (b.1 - a.1) + v * (c.1 - a.1)
    }

    /// Two overlapping cookies over a flat floor, viewed straight down:
    /// each pixel of the dynamic shadow takes the sun but the product of
    /// every cookie's `1 - alpha * fade * clip` at its floor point, as the
    /// CPU reads the atlas bilinearly, to two bytes; off both cookies it
    /// takes none.
    #[test]
    fn the_receivers_write_each_cookies_keep() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let mut c = Cookies::new(&device, &bones_layout(&device));
        // two casters standing over the floor, the sun slanting in
        let sun = Vec3::new(0.3, 0.2, 1.0).normalize();
        let cands = [
            Candidate {
                origin: Vec3::new(-10.0, 0.0, 40.0),
                mins: Vec3::new(-24.0, -12.0, 4.0),
                maxs: Vec3::new(4.0, 12.0, 76.0),
            },
            Candidate {
                origin: Vec3::new(10.0, 10.0, 30.0),
                mins: Vec3::new(-4.0, -6.0, 2.0),
                maxs: Vec3::new(24.0, 26.0, 58.0),
            },
        ];
        let cookies = c.prepare(&queue, sun, Vec3::new(0.0, -400.0, 300.0), &cands, 0);
        assert_eq!(cookies.len(), 2);
        assert!(
            cookies[1].fade < 1.0 && cookies[1].fade > 0.0,
            "{cookies:?}"
        );
        // tile 0: an alpha ramp, tile 1: a solid disc; depth one byte
        // short of the middle (the casters' own)
        let clear = [128u8, 128, 128, 0];
        let mut atlas: Vec<u8> = clear.repeat((ATLAS_W * ATLAS_H) as usize);
        for t in 0..2 {
            let (ox, oy) = tile_origin(t);
            for y in 8..120u32 {
                for x in 8..120u32 {
                    let a = if t == 0 {
                        (x * 2) as u8
                    } else if (x as f32 - 64.0).hypot(y as f32 - 60.0) < 40.0 {
                        255
                    } else {
                        0
                    };
                    let at = (((oy + y) * ATLAS_W + ox + x) * 4) as usize;
                    atlas[at..at + 4].copy_from_slice(&[127, 127, 127, a]);
                }
            }
        }
        write_atlas(&queue, c.atlas[0].texture(), &atlas);
        // the floor at z 0 over x, y in -64..64, seen from above, and a
        // second set, the floor posed 1000 under it, hidden behind it
        const SIDE: u32 = 64;
        let view_proj = Mat4::from_cols(
            Vec4::X / 64.0,
            Vec4::Y / 64.0,
            Vec4::new(0.0, 0.0, -0.0001, 0.0),
            Vec4::new(0.0, 0.0, 0.5, 1.0),
        );
        let floor: Vec<f32> = [[-64.0, -64.0], [64.0, -64.0], [64.0, 64.0], [-64.0, 64.0]]
            .iter()
            .flat_map(|[x, y]| {
                let mut v = vec![*x, *y, 0.0];
                v.extend([0.0; 8]);
                v
            })
            .collect();
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&floor),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2]),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mut target = DynamicShadows::new(&device, &queue, SIDE, SIDE);
        let mut enc = device.create_command_encoder(&Default::default());
        let poses = [Mat4::IDENTITY, Mat4::from_translation(Vec3::Z * -1000.0)];
        // the floor's box, then no box for the posed copy
        let mask_of = c.receiver_masks();
        let masks = [
            mask_of(Some((
                Vec3::new(-64.0, -64.0, 0.0),
                Vec3::new(64.0, 64.0, 0.0),
            ))),
            mask_of(None),
        ];
        assert_eq!(masks[0], masks[1], "every cookie reaches the floor");
        c.encode_receivers(
            &device,
            &queue,
            &mut enc,
            &mut target,
            None,
            &poses.map(|m| (view_proj, m)),
            |pass, set| {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..12, 0, masks[set]..masks[set] + 1);
            },
        );
        queue.submit([enc.finish()]);
        let got = read_rgba(&device, &queue, target.view.texture(), (SIDE, SIDE));
        let (mut worst, mut shaded) = (0u8, 0);
        for y in 0..SIDE {
            for x in 0..SIDE {
                // the pixel's centre on the floor: y up the view is y up
                let p = Vec3::new(
                    (x as f32 + 0.5) * 2.0 - 64.0,
                    64.0 - (y as f32 + 0.5) * 2.0,
                    0.0,
                );
                let mut keep = 1.0;
                for (i, ck) in cookies.iter().enumerate() {
                    let lp = ck.matrices.lookup.project_point3(p);
                    let t = [lp.x * 128.0, lp.y * 128.0];
                    if t.iter().any(|v| !(0.0..=128.0).contains(v)) {
                        continue;
                    }
                    let (ox, oy) = tile_origin(i);
                    let q = [
                        t[0].clamp(0.5, 127.5) + ox as f32,
                        t[1].clamp(0.5, 127.5) + oy as f32,
                    ];
                    keep *= receiver_keep(lp.z, bilinear(&atlas, q), ck.fade);
                }
                if keep < 0.99 {
                    shaded += 1;
                }
                let want = (keep * 255.0).round() as u8;
                worst = worst.max(got[((y * SIDE + x) * 4) as usize].abs_diff(want));
            }
        }
        assert!(shaded > 400, "{shaded} pixels shaded");
        assert!(worst <= 2, "receiver off by {worst}");
        // no cookie: the next frame clears it once
        c.live = 0;
        c.receive = 0;
        let mut enc = device.create_command_encoder(&Default::default());
        c.encode_receivers(
            &device,
            &queue,
            &mut enc,
            &mut target,
            None,
            &poses.map(|m| (view_proj, m)),
            |_, _| {},
        );
        queue.submit([enc.finish()]);
        assert!(!target.dirty);
        let got = read_rgba(&device, &queue, target.view.texture(), (SIDE, SIDE));
        assert!(got.chunks(4).all(|p| p[0] == 255));
    }
}
