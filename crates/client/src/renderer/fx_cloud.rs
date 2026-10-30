//! CoD2's particle clouds (`fx::cod2_cloud`): every cloud draws the one
//! shared 1024-flake buffer through `fx_cloud.wgsl`, the port of
//! `lib/particle_cloud.hlsl`, with its world matrix, `particleCloudMatrix`
//! and colour as instance data, and the outdoor test against the map's
//! `$outdoor` image (`vcod_cod2::outdoor`). Cloud runs sit in the fx run
//! list by material sort (`FxRun::cloud`); cod2-materials.md 9.14.

use super::*;
use crate::fx::cod2_cloud::{self, Clouds, FLAKES};
use glam::{Mat4, Vec3};
use std::ops::Range;
use std::sync::Arc;
use std::thread::JoinHandle;
use vcod_cod2::collision::ClipMap;
use vcod_cod2::outdoor::{OutdoorMap, SIZE};
use vcod_common::asset_source::AssetSource;
use wgpu::util::DeviceExt;

/// Clouds drawn a frame, stage draws included; stock snow loops 12 a
/// `Cloud` element.
const MAX_CLOUDS: usize = 512;

/// The `fx.wgsl` pass plus the cloud vertex and fragment entries.
const FX_CLOUD_WGSL: &str = concat!(
    include_str!("../fx.wgsl"),
    include_str!("../fx_cloud.wgsl"),
    include_str!("../tex_samplers.wgsl")
);

/// One cloud of one stage draw: `fx_cloud.wgsl`'s `CloudIn` from location 2.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CloudInstance {
    /// The world matrix's rows, the origin in `w`.
    m: [[f32; 4]; 3],
    matrix: [f32; 4],
    rgba: [f32; 4],
    flags: u32,
    outdoor: u32,
    _pad: [u32; 2],
}

/// `fx_cloud.wgsl`'s `CloudParams`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudParams {
    mins: [f32; 4],
    inv_extent: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
}

/// What a CoD2 load hands the outdoor build: the level's one `ClipMap`
/// (the one prediction traces) and the box the image spans
/// (`MapFile::outdoor_bounds`), so the build reads no map again.
pub struct OutdoorSource {
    pub map: String,
    pub clip: Arc<ClipMap>,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// The map's `$outdoor` image: built on a worker the first frame a map
/// draws a cloud, as no cloud draws before it is.
enum Outdoor {
    None,
    Building {
        map: String,
        job: JoinHandle<Option<OutdoorMap>>,
    },
    /// Uploaded, or `None` when the build failed: clouds then draw untested.
    Ready {
        map: String,
        span: Option<(Vec3, Vec3)>,
    },
}

pub(super) struct CloudPass {
    /// Indexed by `FxBlend as usize`, as the fx pass's.
    pipelines: [wgpu::RenderPipeline; 4],
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    /// Each instance's `Cloud::Cull` sphere.
    spheres: Vec<(Vec3, f32)>,
    outdoor_tex: wgpu::Texture,
    params_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    outdoor: Outdoor,
    /// The loaded level's, until the next load.
    source: Option<OutdoorSource>,
}

/// The buffer `R_CreateParticleCloudBuffer` fills at renderer start, from
/// a fixed seed.
fn flake_buffer() -> (Vec<cod2_cloud::CloudVertex>, Vec<u16>) {
    let mut rng = crate::fx::sim::Rng::new(0x2005_c10d);
    cod2_cloud::buffer(|| rng.next_f32())
}

impl CloudPass {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> CloudPass {
        let (verts, indices) = flake_buffer();
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cloud flakes"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cloud flake indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cloud instances"),
            size: (MAX_CLOUDS * std::mem::size_of::<CloudInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let outdoor_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("$outdoor"),
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cloud params"),
            size: std::mem::size_of::<CloudParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cloud outdoor layout"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // the code sampler's filter is not read off retail: linear, clamped
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("$outdoor sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cloud outdoor"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &outdoor_tex.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx_cloud.wgsl"),
            source: wgpu::ShaderSource::Wgsl(FX_CLOUD_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cloud pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(texture_layout),
                None,
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let make = |label, fs_entry, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_cloud"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<cod2_cloud::CloudVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x3, // the flake in the cube
                                1 => Float32x2, // the corner's texcoord
                            ],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<CloudInstance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![
                                2 => Float32x4,
                                3 => Float32x4,
                                4 => Float32x4,
                                5 => Float32x4, // particleCloudMatrix
                                6 => Float32x4, // particleCloudColor
                                7 => Uint32, // FX_VERT_* flags
                                8 => Uint32, // the outdoor test on
                            ],
                        }),
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs_entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(blend.is_none()),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
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
        let blend = |b| fx_blend_state(b);
        let pipelines = [
            make("cloud pipeline", "fs_cloud", blend(FxBlend::Alpha)),
            make(
                "cloud additive pipeline",
                "fs_cloud_additive",
                blend(FxBlend::Add),
            ),
            make(
                "cloud screen pipeline",
                "fs_cloud_screen",
                blend(FxBlend::Screen),
            ),
            make("cloud opaque pipeline", "fs_cloud", blend(FxBlend::Opaque)),
        ];
        CloudPass {
            pipelines,
            vertex_buf,
            index_buf,
            instance_buf,
            spheres: Vec::new(),
            outdoor_tex,
            params_buf,
            bind_group,
            outdoor: Outdoor::None,
            source: None,
        }
    }

    /// Whether `map`'s outdoor image is in place, starting or finishing
    /// its build; a frame without a map draws untested.
    fn outdoor_ready(&mut self, queue: &wgpu::Queue, map: Option<&str>) -> bool {
        let Some(map) = map else {
            self.outdoor = Outdoor::None;
            return true;
        };
        let current = match &self.outdoor {
            Outdoor::Building { map: m, .. } | Outdoor::Ready { map: m, .. } => m == map,
            Outdoor::None => false,
        };
        if !current {
            let src = self
                .source
                .as_ref()
                .filter(|s| s.map == map)
                .map(|s| (s.clip.clone(), s.mins, s.maxs));
            let name = map.to_string();
            let job = std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let built = src
                    .ok_or_else(|| anyhow::anyhow!("the load kept no collision"))
                    .map(|(clip, mins, maxs)| OutdoorMap::generate(&clip, mins, maxs));
                match built {
                    Ok(m) => {
                        log::info!(
                            "cod2: {name}'s outdoor map in {:.0} ms",
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                        Some(m)
                    }
                    Err(e) => {
                        log::warn!("cod2: no outdoor map for {name}, clouds draw untested: {e:#}");
                        None
                    }
                }
            });
            self.outdoor = Outdoor::Building {
                map: map.to_string(),
                job,
            };
        }
        if matches!(&self.outdoor, Outdoor::Building { job, .. } if job.is_finished()) {
            let Outdoor::Building { map, job } =
                std::mem::replace(&mut self.outdoor, Outdoor::None)
            else {
                unreachable!()
            };
            let built = job.join().ok().flatten();
            if let Some(m) = &built {
                queue.write_texture(
                    self.outdoor_tex.as_image_copy(),
                    &m.heights,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE[0]),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: SIZE[0],
                        height: SIZE[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.outdoor = Outdoor::Ready {
                map,
                span: built.map(|m| (m.mins, m.maxs)),
            };
        }
        matches!(self.outdoor, Outdoor::Ready { .. })
    }

    /// Writes the frame's instances and the view's right and up; the
    /// instances' order is the runs'.
    fn upload(&mut self, queue: &wgpu::Queue, instances: &[CloudInstance], right: Vec3, up: Vec3) {
        let (mins, maxs) = match &self.outdoor {
            Outdoor::Ready {
                span: Some(span), ..
            } => *span,
            _ => (Vec3::ZERO, Vec3::ONE),
        };
        let params = CloudParams {
            mins: mins.extend(0.0).to_array(),
            inv_extent: (1.0 / (maxs - mins)).extend(0.0).to_array(),
            right: right.extend(0.0).to_array(),
            up: up.extend(0.0).to_array(),
        };
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
    }

    fn has_outdoor(&self) -> bool {
        matches!(self.outdoor, Outdoor::Ready { span: Some(_), .. })
    }

    /// Draws instances `range` with `blend`, binding everything it needs;
    /// a cloud whose sphere `Cloud::Cull` puts outside the view's sides or
    /// behind its near plane is skipped.
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        camera_bg: &wgpu::BindGroup,
        texture_bg: &wgpu::BindGroup,
        blend: FxBlend,
        range: Range<u32>,
        view_proj: Mat4,
    ) {
        let frustum = Frustum::from_view_proj(view_proj);
        let seen = |i: u32| {
            let (c, r) = self.spheres[i as usize];
            frustum.planes[..5].iter().all(|(n, d)| n.dot(c) + d >= -r)
        };
        pass.set_pipeline(&self.pipelines[blend as usize]);
        pass.set_bind_group(0, camera_bg, &[]);
        pass.set_bind_group(1, texture_bg, &[]);
        pass.set_bind_group(3, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_vertex_buffer(1, self.instance_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint16);
        let mut i = range.start;
        while i < range.end {
            if !seen(i) {
                i += 1;
                continue;
            }
            let first = i;
            while i < range.end && seen(i) {
                i += 1;
            }
            pass.draw_indexed(0..(FLAKES * 6) as u32, 0, first..i);
        }
    }
}

impl Renderer {
    /// The loaded CoD2 level's collision for the `$outdoor` build, or None
    /// for a load without one (no session: no map name, no outdoor test).
    pub fn set_outdoor_source(&mut self, src: Option<OutdoorSource>) {
        self.fx.clouds.source = src;
    }

    /// The frame's CoD2 clouds, far to near, after [`Self::set_fx_quads`]
    /// (which drops the last frame's). `map` names the level whose
    /// outdoor image the snow is tested against; until it is built no
    /// cloud draws.
    pub fn set_fx_clouds(
        &mut self,
        fs: &dyn AssetSource,
        map: Option<&str>,
        clouds: &Clouds,
        time: f32,
    ) {
        if clouds.list.is_empty() {
            return;
        }
        if !self.fx.clouds.outdoor_ready(&self.queue, map) {
            return;
        }
        let outdoor = u32::from(self.fx.clouds.has_outdoor());
        let mut instances: Vec<CloudInstance> = Vec::new();
        let mut spheres: Vec<(Vec3, f32)> = Vec::new();
        let mut runs: Vec<FxRun> = Vec::new();
        let mut rest = &clouds.list[..];
        'runs: while let Some(first) = rest.first() {
            let name = first.shader.as_str();
            let len = rest.iter().take_while(|c| c.shader == name).count();
            let (group, tail) = rest.split_at(len);
            rest = tail;
            let sh = self.shader_lib.get(name);
            let sort = sh.map_or(SORT_BLEND, |sh| sh.sort);
            for draw in fx_stage_draws(sh, name, time) {
                if instances.len() + group.len() > MAX_CLOUDS {
                    log::warn!("fx: clouds exceed the {MAX_CLOUDS} cap, truncating");
                    break 'runs;
                }
                if !self.fx.textures.contains_key(draw.image) {
                    let no_map = HashMap::new();
                    let images = if draw.stage.is_some() {
                        &no_map
                    } else {
                        self.shaders.image_map()
                    };
                    let bg = resolve_fx_texture(
                        &self.device,
                        &self.queue,
                        &self.vm_pass.skin_layout,
                        fs,
                        images,
                        draw.image,
                        FxImage {
                            gamma: self.image_gamma.as_ref(),
                            picmip: self.textures.world_picmip(sh.is_some_and(|sh| sh.nopicmip)),
                            store: self.textures.store(),
                            mipmap: !sh.is_some_and(|sh| sh.nomipmaps),
                        },
                    );
                    self.fx.textures.insert(draw.image.to_string(), bg);
                }
                let start = instances.len() as u32;
                for c in group {
                    instances.push(instance(
                        c,
                        draw.stage,
                        draw.flags,
                        outdoor,
                        self.identity_light,
                        time,
                    ));
                    spheres.push((c.origin, c.cull_radius));
                }
                runs.push(FxRun {
                    image: draw.image.to_string(),
                    blend: draw.blend,
                    quads: 0,
                    sort,
                    feather: false,
                    view_depth: false,
                    distort: false,
                    cloud: Some(start..instances.len() as u32),
                });
            }
        }
        self.fx
            .clouds
            .upload(&self.queue, &instances, clouds.right, clouds.up);
        self.fx.clouds.spheres = spheres;
        // after every run of their sort, in their own far-to-near order
        for run in runs {
            let at = self.fx.runs.partition_point(|r| r.sort <= run.sort);
            self.fx.runs.insert(at, run);
        }
    }
}

/// A cloud's instance for one stage draw: its colour through the stage's
/// gens, never fogged (`particle_cloud*.tech`'s `nofog` statemap; the
/// HLSL computes no fog).
fn instance(
    c: &cod2_cloud::FxCloud,
    stage: Option<&Stage>,
    flags: u32,
    outdoor: u32,
    identity_light: f32,
    time: f32,
) -> CloudInstance {
    let row = |a: Vec3, o: f32| [a.x, a.y, a.z, o];
    CloudInstance {
        m: [
            row(c.axis[0], c.origin.x),
            row(c.axis[1], c.origin.y),
            row(c.axis[2], c.origin.z),
        ],
        matrix: c.matrix,
        rgba: fx_stage_rgba(stage, c.rgba, identity_light, time),
        flags: flags | FX_VERT_NOFOG,
        outdoor,
        _pad: [0; 2],
    }
}

/// The pass's own pipelines on a software adapter against
/// `lib/particle_cloud.hlsl` evaluated on the CPU: every flake's corners
/// through `FxCloud::corner` (`vs_main`), rasterized here with
/// perspective-correct texcoords, shaded as `ps_main` with the outdoor
/// variant's test and blended `SrcAlpha, InvSrcAlpha` in buffer order.
/// Vulkan or Metal only; passes vacuously on a machine with no adapter.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::cod2_cloud::FxCloud;
    use glam::{Vec2, Vec4};

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

    const N: u32 = 128;
    /// The 2 x 2 colour map, rgba bytes, row-major.
    const TEX: [[u8; 4]; 4] = [
        [255, 40, 0, 255],
        [0, 255, 60, 128],
        [30, 0, 255, 200],
        [255, 255, 255, 64],
    ];

    /// The outdoor map's box: rows under y 0 (the view's right half)
    /// roofed at the top, the rest open to the floor.
    fn outdoor() -> OutdoorMap {
        let heights = (0..SIZE[1])
            .flat_map(|row| std::iter::repeat_n(if row < 256 { 255 } else { 0 }, 512))
            .collect();
        OutdoorMap {
            mins: Vec3::new(100.0, -100.0, -100.0),
            maxs: Vec3::new(200.0, 100.0, 100.0),
            heights,
        }
    }

    /// Bilinear on the 512 grid, edges clamped, as the linear sampler.
    fn height(map: &OutdoorMap, uv: Vec2) -> f32 {
        let at = |x: i32, y: i32| {
            let (x, y) = (x.clamp(0, 511) as usize, y.clamp(0, 511) as usize);
            f32::from(map.heights[y * 512 + x]) / 255.0
        };
        let p = uv * 512.0 - 0.5;
        let (x0, y0) = (p.x.floor(), p.y.floor());
        let (fx, fy) = (p.x - x0, p.y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    #[test]
    fn a_cloud_draws_its_flakes_as_the_hlsl_does() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        set_msaa_samples(1);
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        for binding in 9..=16 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let mut pass = CloudPass::new(&device, SCENE_FORMAT, &camera_layout, &texture_layout);

        // a view down +x: right is -y, up is +z
        let vp = crate::camera::view_proj_from(Vec3::ZERO, 0.0, 0.0, 0.0, 90.0, 1.0);
        let (right, up) = (Vec3::NEG_Y, Vec3::Z);
        let mut cam = [0f32; 36];
        cam[..16].copy_from_slice(&vp.to_cols_array());
        let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&cam),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // nearest everywhere, so the CPU's texel pick is exact
        let nearest = device.create_sampler(&Default::default());
        let mut cam_entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: cam_buf.as_entire_binding(),
        }];
        for binding in 9..=16 {
            cam_entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(&nearest),
            });
        }
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &camera_layout,
            entries: &cam_entries,
        });
        let tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 2,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Default::default(),
            TEX.as_flattened(),
        );
        let tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &texture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&tex.create_view(&Default::default())),
            }],
        });

        let map = outdoor();
        queue.write_texture(
            pass.outdoor_tex.as_image_copy(),
            &map.heights,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE[0]),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
        );
        pass.outdoor = Outdoor::Ready {
            map: "test".into(),
            span: Some((map.mins, map.maxs)),
        };
        // a turned cloud whose flakes stretch along its motion
        let cloud = FxCloud::new(
            Vec3::new(150.0, 0.0, 0.0),
            Vec3::new(0.0, -20.0, 60.0),
            (50.0, 3.0, 8.0),
            25.0,
            Vec3::new(1.0, 2.0, 3.0).normalize(),
            [0.9, 0.8, 0.7, 0.85],
            "test",
            (right, up),
        );
        assert_ne!(cloud.matrix[2], 0.0, "{:?}", cloud.matrix);
        let flags = FX_VERT_CLAMP_S | FX_VERT_CLAMP_T;
        let inst = instance(&cloud, None, flags, 1, 1.0, 0.0);
        pass.upload(&queue, &[inst], right, up);
        pass.spheres = vec![(cloud.origin, cloud.cull_radius)];

        let target = device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let depth = create_depth_view(&device, N, N);
        let mut enc = device.create_command_encoder(&Default::default());
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
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.draw(&mut rp, &cam_bg, &tex_bg, FxBlend::Alpha, 0..1, vp);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(N * N * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(N * 4),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: N,
                height: N,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let px = readback.slice(..).get_mapped_range().unwrap().to_vec();

        // The CPU side: each triangle in index order onto an 8-bit target.
        let (verts, indices) = flake_buffer();
        let n = N as usize;
        let mut img = vec![Vec3::ZERO; n * n];
        let mut unsure = vec![false; n * n];
        let mut killed = 0;
        let quantize = |c: Vec3| (c * 255.0).round() / 255.0;
        for tri in indices.chunks(3) {
            let vs: Vec<&cod2_cloud::CloudVertex> =
                tri.iter().map(|&i| &verts[i as usize]).collect();
            let flake = cloud.flake(vs[0]);
            let o = map.lookup(flake);
            let h = height(&map, Vec2::new(o.x, o.y));
            let keep = if o.z < h { 0.0 } else { 1.0 };
            let flake_unsure = (o.z - h).abs() < 0.02;
            killed += usize::from(keep == 0.0);
            // clip space, then pixels (y down)
            let clip: Vec<Vec4> = vs
                .iter()
                .map(|v| vp * cloud.corner(v, right, up).extend(1.0))
                .collect();
            let scr: Vec<Vec2> = clip
                .iter()
                .map(|c| {
                    let ndc = c.truncate() / c.w;
                    Vec2::new(
                        (ndc.x + 1.0) * 0.5 * N as f32,
                        (1.0 - ndc.y) * 0.5 * N as f32,
                    )
                })
                .collect();
            let edge = |a: Vec2, b: Vec2, p: Vec2| (b - a).perp_dot(p - a);
            let area = edge(scr[0], scr[1], scr[2]);
            if area.abs() < 1e-6 {
                continue;
            }
            let lo = scr
                .iter()
                .fold(Vec2::splat(f32::MAX), |m, &p| m.min(p))
                .floor();
            let hi = scr
                .iter()
                .fold(Vec2::splat(f32::MIN), |m, &p| m.max(p))
                .ceil();
            for y in (lo.y.max(0.0) as usize)..(hi.y.min(N as f32) as usize) {
                for x in (lo.x.max(0.0) as usize)..(hi.x.min(N as f32) as usize) {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w = [
                        edge(scr[1], scr[2], p) / area,
                        edge(scr[2], scr[0], p) / area,
                        edge(scr[0], scr[1], p) / area,
                    ];
                    let near_edge = (0..3).any(|k| {
                        let (a, b) = (scr[(k + 1) % 3], scr[(k + 2) % 3]);
                        (edge(a, b, p) / (b - a).length()).abs() < 0.01
                    });
                    if w.iter().any(|&w| w < 0.0) {
                        if near_edge {
                            unsure[y * n + x] = true;
                        }
                        continue;
                    }
                    // perspective-correct texcoord
                    let iw: Vec<f32> = clip.iter().map(|c| 1.0 / c.w).collect();
                    let den: f32 = (0..3).map(|k| w[k] * iw[k]).sum();
                    let uv = (0..3)
                        .map(|k| Vec2::new(vs[k][3], vs[k][4]) * w[k] * iw[k])
                        .sum::<Vec2>()
                        / den;
                    let t = uv * 2.0;
                    let on_seam = (t - t.round()).abs().min_element() < 1e-3;
                    if near_edge || on_seam || flake_unsure {
                        unsure[y * n + x] = true;
                    }
                    let (tx, ty) = (t.x.floor().clamp(0.0, 1.0), t.y.floor().clamp(0.0, 1.0));
                    let texel = TEX[ty as usize * 2 + tx as usize].map(|b| f32::from(b) / 255.0);
                    let rgba = Vec4::from(inst.rgba);
                    let src = Vec3::from_slice(&texel[..3]) * rgba.truncate();
                    let a = texel[3] * rgba.w * keep;
                    let dst = img[y * n + x];
                    img[y * n + x] = quantize(src * a + dst * (1.0 - a));
                }
            }
        }
        let (mut checked, mut drawn, mut max_err) = (0, [0usize; 2], 0.0f32);
        for y in 0..n {
            for x in 0..n {
                let got = Vec3::new(
                    f32::from(px[(y * n + x) * 4]),
                    f32::from(px[(y * n + x) * 4 + 1]),
                    f32::from(px[(y * n + x) * 4 + 2]),
                ) / 255.0;
                if got != Vec3::ZERO {
                    drawn[usize::from(x >= n / 2)] += 1;
                }
                if unsure[y * n + x] {
                    continue;
                }
                checked += 1;
                let e = (got - img[y * n + x]).abs().max_element();
                max_err = max_err.max(e);
            }
        }
        eprintln!(
            "checked {checked} px, max err {:.2}/255, drawn left/right {drawn:?}, {killed} triangles indoors",
            max_err * 255.0
        );
        assert!(checked > (n * n) * 9 / 10, "{checked}");
        // llvmpipe blends an 8-bit target in 8-bit fixed point: 2 steps
        // off the float blend where flakes stack
        assert!(max_err <= 2.5 / 255.0, "{}", max_err * 255.0);
        // the left half (+y, open) shows flakes; the right (-y) is roofed
        assert!(killed > 800, "{killed}");
        assert!(drawn[0] > 1000 && drawn[1] < drawn[0] / 4, "{drawn:?}");
    }
}
