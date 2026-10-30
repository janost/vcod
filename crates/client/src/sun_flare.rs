//! The sun flare and the blind and glare screen washes a `.sun` file
//! configures (docs/research/cod11-shader-scripts.md section 8). Retail adds
//! the flare while the sky draws (0x4e83f0 -> 0x4e81e0), measures it by
//! reading the depth buffer around the sun (0x4d9310) and draws it and the
//! washes in screen space after the scene (0x4d9770). vcod measures with an
//! occlusion query instead of a depth read and uses the newest finished
//! result, a frame or two old; the fades absorb the lag.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use vcod_common::sun::{SunFade, SunFile};

use crate::renderer::{DEPTH_FORMAT, Frame, msaa_samples};
use crate::texture_settings::TexSamplers;

/// Window sizes are in 640-wide virtual pixels (0x568fe4 is 1/640).
const VIRTUAL_WIDTH: f32 = 640.0;
/// The depth window is at most 65 pixels across (0x4d9310).
const MAX_WINDOW: i32 = 65;
/// A new flare's first fade step is 10 ms (0x4d8ca0 backdates it).
const FIRST_STEP_MS: f32 = 10.0;

/// The `.sun` values the flare reads, angles turned into cosines as
/// 0x4e7be0 stores them.
#[derive(Clone, Debug, PartialEq)]
pub struct FlareParams {
    pub sprite_size: f32,
    pub min_size: f32,
    pub max_size: f32,
    pub cos_min: f32,
    pub cos_max: f32,
    pub max_alpha: f32,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
    pub blind: Wash,
    pub glare: Wash,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wash {
    pub cos_min: f32,
    pub cos_max: f32,
    pub max: f32,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
}

/// Seconds to whole milliseconds as 0x4e7be0 rounds them.
fn ms(s: f32) -> f32 {
    (s * 1000.0).round()
}

impl Wash {
    fn of(f: &SunFade) -> Self {
        Wash {
            cos_min: f.min_angle.to_radians().cos(),
            cos_max: f.max_angle.to_radians().cos(),
            max: f.max,
            fade_in_ms: ms(f.fade_in),
            fade_out_ms: ms(f.fade_out),
        }
    }
}

impl FlareParams {
    pub fn of(sf: &SunFile) -> Self {
        FlareParams {
            sprite_size: sf.sprite_size,
            min_size: sf.flare_min_size,
            max_size: sf.flare_max_size,
            cos_min: sf.flare_min_angle.to_radians().cos(),
            cos_max: sf.flare_max_angle.to_radians().cos(),
            max_alpha: sf.flare_max_alpha,
            fade_in_ms: ms(sf.flare_fade_in),
            fade_out_ms: ms(sf.flare_fade_out),
            blind: Wash::of(&sf.blind),
            glare: Wash::of(&sf.glare),
        }
    }
}

/// How far into an angle band `dot` (view forward against the sun) sits:
/// 0 at or outside `cos_min`, 1 inside `cos_max`, linear in the cosine
/// between (0x4e81e0, 0x4e8270).
pub fn band(dot: f32, cos_min: f32, cos_max: f32) -> f32 {
    if dot <= cos_min {
        0.0
    } else if dot >= cos_max {
        1.0
    } else {
        (dot - cos_min) / (cos_max - cos_min)
    }
}

/// 0x4d2d90: steps `cur` toward `target` at 1/`in_ms` per ms rising and
/// 1/`out_ms` falling; a time of 0 jumps.
pub fn fade(cur: f32, target: f32, in_ms: f32, out_ms: f32, dt_ms: f32) -> f32 {
    if target > cur {
        if in_ms > 0.0 {
            return (cur + dt_ms / in_ms).min(target);
        }
    } else if target < cur && out_ms > 0.0 {
        return (cur - dt_ms / out_ms).max(target);
    }
    target
}

/// A pixel rectangle, bottom-left origin like GL window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// What one frame of the flare does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameOut {
    /// The window to measure this frame and its full side in pixels (the
    /// visible fraction divides by the side squared even where the screen
    /// clips the window).
    pub query: Option<(Rect, i32)>,
    pub flare: Option<FlareDraw>,
    /// The blind's darkening and the glare's lightening, 0..max.
    pub darken: f32,
    pub lighten: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlareDraw {
    /// Window pixels, bottom-left origin.
    pub center: [f32; 2],
    pub half: f32,
    /// The flare's colour (its rgb all `max_alpha * band`) and faded
    /// visibility, the vertex colour's alpha.
    pub color: f32,
    pub intensity: f32,
}

/// One flare's state across frames (retail's flare list holds one per
/// light and view; the sun's has id -1).
#[derive(Clone, Debug, PartialEq)]
struct Flare {
    intensity: f32,
    last_ms: f32,
    /// Window centre, pixels, as last added.
    center: [f32; 2],
    half: f32,
    color: f32,
}

/// The flare's logic without the GPU.
#[derive(Clone, Debug, Default)]
pub struct FlareSim {
    flare: Option<Flare>,
    blind: f32,
    glare: f32,
    wash_ms: Option<f32>,
}

/// One frame's view of the sun.
pub struct FrameIn {
    pub now_ms: f32,
    /// View forward against the sprite's sun direction (0x16c57d0).
    pub dot: f32,
    /// The flare direction projected to window pixels (bottom-left
    /// origin), `None` behind the eye.
    pub window: Option<[f32; 2]>,
    pub width: f32,
    pub height: f32,
    /// The newest measured fraction of the window showing sky.
    pub measured: f32,
    /// The sky drew and `r_drawSun` is on, so 0x4e83f0 ran this frame.
    pub sun: bool,
}

impl FlareSim {
    pub fn step(&mut self, p: &FlareParams, f: &FrameIn) -> FrameOut {
        let mut out = FrameOut::default();
        let scale = f.width / VIRTUAL_WIDTH;
        let radius = (p.sprite_size * scale).round() as i32;
        // 0x4e81e0 adds the flare inside the band
        let added = f
            .window
            .filter(|_| f.sun && f.dot > p.cos_min)
            .and_then(|c| {
                let frac = band(f.dot, p.cos_min, p.cos_max);
                let r = radius as f32;
                let fresh = self.flare.is_none();
                // 0x4d8ca0 drops a new flare whose window is all off screen
                if fresh
                    && (c[0] + r < 0.0
                        || c[0] - r >= f.width
                        || c[1] + r < 0.0
                        || c[1] - r >= f.height)
                {
                    return None;
                }
                let fl = self.flare.get_or_insert(Flare {
                    intensity: 0.0,
                    last_ms: f.now_ms - FIRST_STEP_MS,
                    center: c,
                    half: 0.0,
                    color: 0.0,
                });
                fl.center = c;
                fl.half = (p.min_size * 0.5 + frac * p.max_size * 0.5) * scale;
                fl.color = p.max_alpha * frac;
                Some(())
            });
        // 0x4d9310: a window off the screen reads 1 while the flare is
        // added and 0 once it fades out; a fading flare's depth of 2
        // fails every pixel
        let mut sun_vis = 0.0;
        if let Some(fl) = &mut self.flare {
            let n = (radius * 2 + 1).clamp(1, MAX_WINDOW);
            let (cx, cy) = (fl.center[0].round() as i32, fl.center[1].round() as i32);
            let off = cx + radius < 0
                || cx - radius >= f.width as i32
                || cy + radius < 0
                || cy - radius >= f.height as i32;
            let vis = if off {
                if added.is_some() { 1.0 } else { 0.0 }
            } else if added.is_some() {
                let rect = Rect {
                    x: cx - n / 2,
                    y: cy - n / 2,
                    w: n,
                    h: n,
                };
                out.query = Some((rect, n));
                f.measured
            } else {
                0.0
            };
            sun_vis = vis;
            fl.intensity = fade(
                fl.intensity,
                vis,
                p.fade_in_ms,
                p.fade_out_ms,
                f.now_ms - fl.last_ms,
            );
            fl.last_ms = f.now_ms;
            if fl.intensity == 0.0 {
                self.flare = None;
            } else {
                out.flare = Some(FlareDraw {
                    center: fl.center,
                    half: fl.half,
                    color: fl.color,
                    intensity: fl.intensity,
                });
            }
        }
        // 0x4e8270, every frame
        let dt = self.wash_ms.map_or(FIRST_STEP_MS, |t| f.now_ms - t);
        self.wash_ms = Some(f.now_ms);
        let wash = |cur: &mut f32, w: &Wash| {
            if w.max <= 0.0 {
                return 0.0;
            }
            let target = band(f.dot, w.cos_min, w.cos_max) * sun_vis;
            *cur = fade(*cur, target, w.fade_in_ms, w.fade_out_ms, dt);
            *cur * w.max
        };
        out.darken = wash(&mut self.blind, &p.blind);
        out.lighten = wash(&mut self.glare, &p.glare);
        out
    }
}

// ---- GPU side ----

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

/// Six vertices per quad: the depth probe, the wash, the flare.
const QUADS: usize = 3;

/// One mappable copy of the query result; `state` is 0 idle, 1 in flight,
/// 2 mapped.
struct Readback {
    buf: wgpu::Buffer,
    state: Arc<AtomicU8>,
    samples: u64,
}

pub struct SunFlareGpu {
    params: FlareParams,
    /// Retail aims the flare along the lump-30 sun light (0x4d9250 reads
    /// `world+0x114`'s direction) and gates it on the sprite direction.
    flare_dir: glam::Vec3,
    view_dir: glam::Vec3,
    sim: FlareSim,
    measured: f32,
    probe: wgpu::RenderPipeline,
    wash: wgpu::RenderPipeline,
    flare: wgpu::RenderPipeline,
    image: wgpu::BindGroup,
    image_layout: wgpu::BindGroupLayout,
    image_view: wgpu::TextureView,
    verts: wgpu::Buffer,
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readbacks: Vec<Readback>,
    /// This frame's readback slot and the probe's full sample count.
    issued: Option<(usize, u64)>,
    draw_wash: bool,
    draw_flare: bool,
}

impl SunFlareGpu {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        params: FlareParams,
        flare_dir: glam::Vec3,
        view_dir: glam::Vec3,
        image: &wgpu::TextureView,
        samplers: &TexSamplers,
    ) -> Self {
        let module = device.create_shader_module(wgpu::include_wgsl!("sun_flare.wgsl"));
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun flare layout"),
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
                sampler_entry(1),
                sampler_entry(2),
            ],
        });
        let image_view = image.clone();
        let image = image_group(device, &tex_layout, &image_view, samplers);
        let textured = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun flare pipeline layout"),
            bind_group_layouts: &[Some(&tex_layout)],
            immediate_size: 0,
        });
        let plain = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun probe pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = |label, layout, fs, blend, write_mask, depth_compare| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vert>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x2, 2 => Float32x4
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: msaa_samples(),
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let blend = |src, dst| {
            let c = wgpu::BlendComponent {
                src_factor: src,
                dst_factor: dst,
                operation: wgpu::BlendOperation::Add,
            };
            Some(wgpu::BlendState { color: c, alpha: c })
        };
        // a sample passes where the scene left the cleared far depth: sky
        let probe = pipeline(
            "sun probe pipeline",
            &plain,
            "fs_wash",
            None,
            wgpu::ColorWrites::empty(),
            wgpu::CompareFunction::LessEqual,
        );
        // state 0x10062: GL_ONE, GL_ONE_MINUS_SRC_ALPHA, no depth test
        let wash = pipeline(
            "sun wash pipeline",
            &plain,
            "fs_wash",
            blend(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrcAlpha),
            wgpu::ColorWrites::ALL,
            wgpu::CompareFunction::Always,
        );
        // the stock flare blocks: `blendFunc gl_src_alpha gl_one`
        let flare = pipeline(
            "sun flare pipeline",
            &textured,
            "fs_flare",
            blend(wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::One),
            wgpu::ColorWrites::ALL,
            wgpu::CompareFunction::Always,
        );
        let verts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun flare vertices"),
            size: (QUADS * 6 * std::mem::size_of::<Vert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("sun probe query"),
            ty: wgpu::QueryType::Occlusion,
            count: 1,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sun probe resolve"),
            size: 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..3)
            .map(|_| Readback {
                buf: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sun probe readback"),
                    size: 8,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(0)),
                samples: 1,
            })
            .collect();
        SunFlareGpu {
            params,
            flare_dir: flare_dir.normalize_or_zero(),
            view_dir: view_dir.normalize_or_zero(),
            sim: FlareSim::default(),
            measured: 0.0,
            probe,
            wash,
            flare,
            image,
            image_layout: tex_layout,
            image_view,
            verts,
            queries,
            resolve,
            readbacks,
            issued: None,
            draw_wash: false,
            draw_flare: false,
        }
    }

    /// Before the scene pass: collects finished probes, steps the flare and
    /// writes this frame's quads. `id_light` is the renderer's identityLight.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        (width, height): (f32, f32),
        id_light: f32,
        sun: bool,
    ) {
        let _ = device.poll(wgpu::PollType::Poll);
        for rb in &mut self.readbacks {
            if rb.state.load(Ordering::Acquire) != 2 {
                continue;
            }
            if let Ok(view) = rb.buf.slice(..).get_mapped_range() {
                let passed = u64::from_le_bytes(view[..8].try_into().expect("8 bytes"));
                self.measured = (passed as f32 / rb.samples as f32).min(1.0);
            }
            rb.buf.unmap();
            rb.state.store(0, Ordering::Release);
        }
        // the flare direction is a point at infinity (w 0, 0x4d8ca0)
        let clip = frame.view_proj * self.flare_dir.extend(0.0);
        let window = (clip.w > 0.0).then(|| {
            [
                (clip.x / clip.w * 0.5 + 0.5) * width,
                (clip.y / clip.w * 0.5 + 0.5) * height,
            ]
        });
        let out = self.sim.step(
            &self.params,
            &FrameIn {
                now_ms: frame.time * 1000.0,
                dot: frame.fwd.dot(self.view_dir),
                window,
                width,
                height,
                measured: self.measured,
                sun,
            },
        );
        let slot = self
            .readbacks
            .iter()
            .position(|rb| rb.state.load(Ordering::Acquire) == 0);
        self.issued = None;
        let mut quads = [[Vert::zeroed(); 6]; QUADS];
        if let (Some((rect, n)), Some(slot)) = (out.query, slot) {
            let samples = (n * n) as u64 * u64::from(msaa_samples());
            self.issued = Some((slot, samples));
            let (x0, y0) = (rect.x as f32, rect.y as f32);
            let (x1, y1) = (x0 + rect.w as f32, y0 + rect.h as f32);
            quads[0] = quad([x0, y0, x1, y1], 1.0, [0.0; 4], width, height);
        }
        self.draw_wash = out.darken != 0.0 || out.lighten != 0.0;
        if self.draw_wash {
            let c = [out.lighten, out.lighten, out.lighten, out.darken];
            quads[1] = quad([0.0, 0.0, width, height], 0.0, c, width, height);
        }
        self.draw_flare = out.flare.is_some();
        if let Some(fl) = &out.flare {
            // vertex rgb is identityLight * colour; `rgbGen vertex` takes
            // identityLight off it again (0x4d94b0, cod11-gamma.md 5)
            let rgb = fl.color * id_light * id_light;
            let [cx, cy] = fl.center;
            quads[2] = quad(
                [cx - fl.half, cy - fl.half, cx + fl.half, cy + fl.half],
                0.0,
                [rgb, rgb, rgb, fl.intensity],
                width,
                height,
            );
        }
        queue.write_buffer(&self.verts, 0, bytemuck::cast_slice(&quads));
    }

    /// The flare's direction, a point at infinity.
    /// `r_textureMode` changed: a mipmapped flare samples through its new
    /// sampler.
    pub fn set_samplers(&mut self, device: &wgpu::Device, samplers: &TexSamplers) {
        self.image = image_group(device, &self.image_layout, &self.image_view, samplers);
    }

    pub fn dir(&self) -> glam::Vec3 {
        self.flare_dir
    }

    /// The query set the scene pass needs when a probe runs this frame.
    pub fn query_set(&self) -> Option<&wgpu::QuerySet> {
        self.issued.map(|_| &self.queries)
    }

    /// Inside the scene pass, after every 3D draw: the probe, then the wash,
    /// then the flare, as 0x4d9770 orders them.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.issued.is_none() && !self.draw_wash && !self.draw_flare {
            return;
        }
        pass.set_vertex_buffer(0, self.verts.slice(..));
        if self.issued.is_some() {
            pass.set_pipeline(&self.probe);
            pass.begin_occlusion_query(0);
            pass.draw(0..6, 0..1);
            pass.end_occlusion_query();
        }
        if self.draw_wash {
            pass.set_pipeline(&self.wash);
            pass.draw(6..12, 0..1);
        }
        if self.draw_flare {
            pass.set_pipeline(&self.flare);
            pass.set_bind_group(0, &self.image, &[]);
            pass.draw(12..18, 0..1);
        }
    }

    /// After the scene pass: copies the probe's count out.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some((slot, samples)) = self.issued else {
            return;
        };
        let rb = &mut self.readbacks[slot];
        rb.samples = samples;
        encoder.resolve_query_set(&self.queries, 0..1, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &rb.buf, 0, 8);
    }

    /// After the submit: maps this frame's copy.
    pub fn after_submit(&mut self) {
        let Some((slot, _)) = self.issued.take() else {
            return;
        };
        let rb = &self.readbacks[slot];
        rb.state.store(1, Ordering::Release);
        let state = rb.state.clone();
        rb.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            state.store(if r.is_ok() { 2 } else { 0 }, Ordering::Release);
        });
    }
}

use bytemuck::Zeroable;

/// Two triangles over a window-pixel rectangle `[x0, y0, x1, y1]`
/// (bottom-left origin) at NDC depth `z`. uv runs as 0x4d94b0's: t = 0 at
/// the bottom.
fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The flare image with `r_textureMode`'s clamp sampler and the `GL_LINEAR`
/// one; the shader picks by the image's level count, as retail filters an
/// image by its flag 1 (0x4ec07e).
fn image_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    samplers: &TexSamplers,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sun flare image"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&samplers.clamp),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&samplers.linear_clamp),
            },
        ],
    })
}

fn quad(r: [f32; 4], z: f32, color: [f32; 4], width: f32, height: f32) -> [Vert; 6] {
    let v = |x: f32, y: f32, s: f32, t: f32| Vert {
        pos: [x / width * 2.0 - 1.0, y / height * 2.0 - 1.0, z],
        uv: [s, t],
        color,
    };
    let [x0, y0, x1, y1] = r;
    let (a, b, c, d) = (
        v(x0, y0, 0.0, 0.0),
        v(x0, y1, 0.0, 1.0),
        v(x1, y1, 1.0, 1.0),
        v(x1, y0, 1.0, 0.0),
    );
    [a, b, c, a, c, d]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harbor() -> FlareParams {
        // scripts/mp_harbor.sun
        FlareParams::of(&SunFile::parse(
            "r_sunsprite_shader \"0\"\nr_sunsprite_size \"16\"\n\
             r_sunflare_shader \"gfx/sun/global_flare\"\nr_sunflare_min_size \"12\"\n\
             r_sunflare_min_angle \"90\"\nr_sunflare_max_size \"600\"\n\
             r_sunflare_max_angle \"2\"\nr_sunflare_max_alpha \".50\"\n\
             r_sunflare_fadein \"1\"\nr_sunflare_fadeout \"1\"\n\
             r_sunblind_min_angle \"30\"\nr_sunblind_max_angle \"5\"\n\
             r_sunblind_max_darken \".3\"\nr_sunblind_fadein \".5\"\n\
             r_sunblind_fadeout \"3\"\nr_sunglare_min_angle \"30\"\n\
             r_sunglare_max_angle \"5\"\nr_sunglare_max_lighten \".1\"\n\
             r_sunglare_fadein \".5\"\nr_sunglare_fadeout \"3\"\n",
        ))
    }

    fn frame(now_ms: f32, dot: f32, window: Option<[f32; 2]>, measured: f32) -> FrameIn {
        FrameIn {
            now_ms,
            dot,
            window,
            width: 1280.0,
            height: 960.0,
            measured,
            sun: true,
        }
    }

    #[test]
    fn no_sky_drawn_adds_no_flare_and_the_washes_fade() {
        let p = harbor();
        let mut sim = FlareSim::default();
        let c = [640.0, 480.0];
        sim.step(&p, &frame(0.0, 1.0, Some(c), 1.0));
        sim.step(&p, &frame(1000.0, 1.0, Some(c), 1.0));
        // the sky stage did not run (no sky surface, or r_drawSun 0)
        let out = sim.step(
            &p,
            &FrameIn {
                sun: false,
                ..frame(1250.0, 1.0, Some(c), 1.0)
            },
        );
        assert_eq!(out.query, None);
        assert_eq!(out.flare.expect("fading").intensity, 0.75);
        assert!(sim.glare < 1.0 && sim.blind < 1.0);
    }

    #[test]
    fn band_and_fade_follow_retail() {
        let p = harbor();
        assert_eq!(band(p.cos_min, p.cos_min, p.cos_max), 0.0);
        assert_eq!(band(1.0, p.cos_min, p.cos_max), 1.0);
        let mid = (p.cos_min + p.cos_max) * 0.5;
        assert!((band(mid, p.cos_min, p.cos_max) - 0.5).abs() < 1e-6);
        // 1 s in, 1 s out: 250 ms moves a quarter
        assert_eq!(fade(0.0, 1.0, 1000.0, 1000.0, 250.0), 0.25);
        assert_eq!(fade(0.9, 1.0, 1000.0, 1000.0, 250.0), 1.0);
        assert_eq!(fade(1.0, 0.0, 1000.0, 3000.0, 1500.0), 0.5);
        assert_eq!(fade(0.2, 0.7, 0.0, 0.0, 1.0), 0.7);
    }

    #[test]
    fn sun_in_view_and_unoccluded_fades_in_and_probes_its_window() {
        let p = harbor();
        let mut sim = FlareSim::default();
        let c = [640.0, 480.0];
        let out = sim.step(&p, &frame(1000.0, 1.0, Some(c), 1.0));
        // r_sunsprite_size 16 at 1280 wide: radius 32, window 65 (capped)
        let (rect, n) = out.query.expect("on screen");
        assert_eq!((n, rect.x, rect.y, rect.w), (65, 640 - 32, 480 - 32, 65));
        let fl = out.flare.expect("drawn");
        // first step 10 ms of a 1 s fade
        assert!((fl.intensity - 0.01).abs() < 1e-6);
        // dead on the sun: half 6 + 300 virtual pixels, colour max_alpha
        assert_eq!((fl.half, fl.color), ((6.0 + 300.0) * 2.0, 0.5));
        // glare and blind ride the same visibility
        assert!(out.darken > 0.0 && out.lighten > 0.0);
        let out = sim.step(&p, &frame(2000.0, 1.0, Some(c), 1.0));
        assert_eq!(out.flare.expect("drawn").intensity, 1.0);
        assert!((out.darken - 0.3).abs() < 1e-6 && (out.lighten - 0.1).abs() < 1e-6);
    }

    #[test]
    fn occluded_sun_fades_out_and_the_flare_goes() {
        let p = harbor();
        let mut sim = FlareSim::default();
        let c = [640.0, 480.0];
        sim.step(&p, &frame(0.0, 1.0, Some(c), 1.0));
        sim.step(&p, &frame(1000.0, 1.0, Some(c), 1.0));
        let out = sim.step(&p, &frame(1500.0, 1.0, Some(c), 0.0));
        assert_eq!(out.flare.expect("fading").intensity, 0.5);
        let out = sim.step(&p, &frame(2500.0, 1.0, Some(c), 0.0));
        assert_eq!(out.flare, None);
        assert!(sim.flare.is_none());
    }

    #[test]
    fn leaving_the_band_fades_without_probing() {
        let p = harbor();
        let mut sim = FlareSim::default();
        let c = [640.0, 480.0];
        sim.step(&p, &frame(0.0, 1.0, Some(c), 1.0));
        sim.step(&p, &frame(1000.0, 1.0, Some(c), 1.0));
        // facing away (dot below cos 90): not added, no probe, fades out
        let out = sim.step(&p, &frame(1250.0, -0.5, None, 1.0));
        assert_eq!(out.query, None);
        assert_eq!(out.flare.expect("fading").intensity, 0.75);
        assert_eq!(out.lighten, sim.glare * 0.1);
    }

    #[test]
    fn a_new_flare_off_screen_is_not_added_but_an_old_one_reads_visible() {
        let p = harbor();
        let mut sim = FlareSim::default();
        let out = sim.step(&p, &frame(0.0, 0.5, Some([-200.0, 480.0]), 0.0));
        assert_eq!((out.query, out.flare), (None, None));
        sim.step(&p, &frame(10.0, 1.0, Some([640.0, 480.0]), 1.0));
        // the window slides off screen while the sun stays in the band
        let out = sim.step(&p, &frame(510.0, 0.5, Some([-200.0, 480.0]), 0.0));
        assert_eq!(out.query, None);
        assert!(out.flare.expect("still added").intensity > 0.5);
    }
}
