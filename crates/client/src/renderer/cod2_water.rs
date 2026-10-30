//! CoD2's water (docs/research/cod2-materials.md 9.12): a world batch whose
//! material carries [`WaterLit`] draws through `cod2_water.wgsl` over the
//! height map [`vcod_cod2::water::WaterSetup`] builds on the CPU each
//! frame, as retail's `RB_UploadWaterTexture` does.

use std::collections::HashMap;
use vcod_common::asset_source::AssetSource;
use wgpu::util::DeviceExt;

use super::cod2::{load_cube_faces, make_lit_pipeline, tex_entry, uniform_entry, upload_cube};
use super::{StageVariant, depth_bias};
use vcod_cod2::water::WaterSetup;
use vcod_common::assets::{Image, ImageData};
use vcod_common::material::{Material, PolygonOffset, WaterLit};
use vcod_common::rng::GlibcRand;

/// cod2_water.wgsl with the image samplers.
pub(super) const COD2_WATER_WGSL: &str = concat!(
    include_str!("../cod2_water.wgsl"),
    include_str!("../tex_samplers.wgsl")
);

/// cod2_water.wgsl's `Water`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WaterUniform {
    pub color: [f32; 4],
    pub flags: [u32; 4],
}

/// The material's fogged technique leaves the fog off.
const M_NOFOG: u32 = 8;

impl WaterUniform {
    pub fn new(w: &WaterLit, nofog: bool) -> Self {
        WaterUniform {
            color: w.color,
            flags: [if nofog { M_NOFOG } else { 0 }, 0, 0, 0],
        }
    }
}

/// Whether a world batch of `sh` draws through the water pipeline: a
/// [`WaterLit`] material in an opaque or alpha-blended stage.
pub fn draws_water(sh: &Material, v: StageVariant) -> bool {
    sh.water.is_some() && super::cod2::variant_state(v).is_some()
}

/// One water material's height map: its setup, the texture each frame's
/// levels go into, and group 2 (the sun, the map, its sampler).
pub(super) struct WaterSim {
    setup: WaterSetup,
    texture: wgpu::Texture,
    pub bg: wgpu::BindGroup,
    /// The time of the last upload; one a frame.
    time: Option<f32>,
}

impl WaterSim {
    /// Rebuilds the height map at `time` seconds, once per distinct time.
    pub fn update(&mut self, queue: &wgpu::Queue, time: f32) {
        if self.time == Some(time) {
            return;
        }
        self.time = Some(time);
        let levels = self.setup.levels(time);
        let kept = self.texture.mip_level_count() as usize;
        write_levels(queue, &self.texture, &levels[..kept]);
    }
}

pub(super) fn write_levels(queue: &wgpu::Queue, texture: &wgpu::Texture, levels: &[Vec<u8>]) {
    let (w, h) = (texture.width(), texture.height());
    for (level, bytes) in levels.iter().enumerate() {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(lw),
                rows_per_image: Some(lh),
            },
            wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// The loaded map's water draws.
pub(super) struct Waters {
    pipelines: HashMap<(usize, bool, bool), wgpu::RenderPipeline>,
    pub material_bgs: Vec<wgpu::BindGroup>,
    pub sims: Vec<WaterSim>,
}

impl Waters {
    pub fn pipeline(&self, v: StageVariant, two_sided: bool, bias: bool) -> &wgpu::RenderPipeline {
        let (slot, ..) = super::cod2::variant_state(v).expect("water draws take a lit variant");
        &self.pipelines[&(slot, two_sided, bias)]
    }

    /// Every water's height map at the frame's `time` seconds.
    pub fn update(&mut self, queue: &wgpu::Queue, time: f32) {
        for s in &mut self.sims {
            s.update(queue, time);
        }
    }
}

/// Builds [`Waters`] during a world load.
pub(super) struct WaterBuilder {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    material_layout: wgpu::BindGroupLayout,
    sim_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// One `rand()` stream for every setup, as retail's.
    rng: GlibcRand,
    material_of: HashMap<String, u32>,
    pipelines: HashMap<(usize, bool, bool), wgpu::RenderPipeline>,
    material_bgs: Vec<wgpu::BindGroup>,
    sims: Vec<WaterSim>,
}

fn cube_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
            multisampled: false,
        },
        count: None,
    }
}

impl WaterBuilder {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod2_water.wgsl"),
            source: wgpu::ShaderSource::Wgsl(COD2_WATER_WGSL.into()),
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 water material layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                cube_entry(1),
            ],
        });
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cod2 water height layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                super::cod2::texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cod2 water pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&material_layout),
                Some(&sim_layout),
            ],
            immediate_size: 0,
        });
        // sampler byte 6: linear min and mag, point mip, wrap (2.2)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cod2 water height"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        WaterBuilder {
            module,
            layout,
            material_layout,
            sim_layout,
            sampler,
            rng: GlibcRand::new(1),
            material_of: HashMap::new(),
            pipelines: HashMap::new(),
            material_bgs: Vec::new(),
            sims: Vec::new(),
        }
    }

    /// Binds a water batch of `sh`, which [`draws_water`] accepted: the
    /// material's group (and its height map, same index) and the pipeline.
    #[allow(clippy::too_many_arguments)]
    pub fn bind(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        fs: &dyn AssetSource,
        sun_buf: &wgpu::Buffer,
        sh: &Material,
        (v, two_sided, bias): (StageVariant, bool, bool),
    ) -> u32 {
        let (slot, blend, depth_write) = super::cod2::variant_state(v).expect("a lit variant");
        let key = (slot, two_sided, bias);
        if !self.pipelines.contains_key(&key) {
            let p = make_lit_pipeline(
                device,
                &self.module,
                &self.layout,
                format,
                blend,
                depth_write,
                two_sided,
                ("fs_main", None),
                if bias {
                    depth_bias(sh.depth_bias.unwrap_or(PolygonOffset::Single))
                } else {
                    Default::default()
                },
            );
            self.pipelines.insert(key, p);
        }
        if let Some(&i) = self.material_of.get(&sh.name) {
            return i;
        }
        let w = sh.water.as_ref().expect("draws_water checked");
        let cube = load_cube_faces(fs, &w.cube)
            .and_then(|faces| upload_cube(device, queue, &w.cube, &faces))
            .unwrap_or_else(|| {
                // no cube: alpha 0 keeps the water colour alone
                let faces: Vec<Image> = (0..6)
                    .map(|_| Image {
                        width: 1,
                        height: 1,
                        data: ImageData::Rgba8(vec![0; 4]),
                    })
                    .collect();
                upload_cube(device, queue, "no water cube", &faces).expect("a 1x1 cube")
            });
        self.add(device, queue, sun_buf, sh, w, &cube, true)
    }

    /// The material and height map groups of `w` over `cube`; without
    /// `mips` the height map keeps its top level only (the tests' exact
    /// sampling).
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sun_buf: &wgpu::Buffer,
        sh: &Material,
        w: &WaterLit,
        cube: &wgpu::TextureView,
        mips: bool,
    ) -> u32 {
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cod2 water"),
            contents: bytemuck::bytes_of(&WaterUniform::new(w, sh.nofog)),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        self.material_bgs
            .push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("cod2 water {}", sh.name)),
                layout: &self.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ubuf.as_entire_binding(),
                    },
                    tex_entry(1, cube),
                ],
            }));
        let setup = WaterSetup::new(w, &mut self.rng);
        let side = setup.width as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("watersetup {}", sh.name)),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: if mips { side.ilog2() + 1 } else { 1 },
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let levels = setup.levels(0.0);
        let kept = texture.mip_level_count() as usize;
        write_levels(queue, &texture, &levels[..kept]);
        let view = texture.create_view(&Default::default());
        let bg = self.sim_group(device, sun_buf, &view);
        self.sims.push(WaterSim {
            setup,
            texture,
            bg,
            time: Some(0.0),
        });
        let i = self.material_bgs.len() as u32 - 1;
        self.material_of.insert(sh.name.clone(), i);
        i
    }

    fn sim_group(
        &self,
        device: &wgpu::Device,
        sun_buf: &wgpu::Buffer,
        height: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cod2 water height"),
            layout: &self.sim_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sun_buf.as_entire_binding(),
                },
                tex_entry(1, height),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    pub fn finish(self) -> Waters {
        if !self.sims.is_empty() {
            log::info!("cod2 water: {} materials", self.sims.len());
        }
        Waters {
            pipelines: self.pipelines,
            material_bgs: self.material_bgs,
            sims: self.sims,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cod2::SunUniform;
    use super::super::cod2::tests::{camera_bytes, camera_group, camera_layout, headless};
    use super::*;
    use glam::{Vec3, Vec4};
    use vcod_common::pk3::Pk3Fs;

    const W: usize = 48;
    const H: usize = 32;
    /// World units per texture repeat on the test plane.
    const UV_SPAN: f32 = 2048.0;

    /// A smooth periodic height map, so neighbouring samples of a pixel
    /// agree and the comparison measures the shading, not aliasing.
    fn heights() -> Vec<u8> {
        let mut out = Vec::with_capacity(64 * 64);
        for y in 0..64 {
            for x in 0..64 {
                let (fx, fy) = (x as f32 / 64.0, y as f32 / 64.0);
                let v = 128.0
                    + 90.0
                        * (std::f32::consts::TAU * 4.0 * fx).sin()
                        * (std::f32::consts::TAU * 2.0 * fy).cos();
                out.push(v.round() as u8);
            }
        }
        out
    }

    /// Bilinear, wrapped, at `uv`, in [0, 1].
    fn sample(tex: &[u8], uv: [f32; 2]) -> f32 {
        let at = |x: i32, y: i32| {
            f32::from(tex[(y.rem_euclid(64) * 64 + x.rem_euclid(64)) as usize]) / 255.0
        };
        let (x, y) = (uv[0] * 64.0 - 0.5, uv[1] * 64.0 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// The cube face a direction reads (wgpu's layer order +x -x +y -y +z
    /// -z), and whether a second axis comes within 2% of it.
    fn face(d: Vec3) -> (usize, bool) {
        let a = d.abs();
        let (axis, m) = if a.x >= a.y && a.x >= a.z {
            (0, a.x)
        } else if a.y >= a.z {
            (1, a.y)
        } else {
            (2, a.z)
        };
        let second = [a.x, a.y, a.z]
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != axis)
            .map(|(_, &v)| v)
            .fold(0.0f32, f32::max);
        let neg = [d.x, d.y, d.z][axis] < 0.0;
        (2 * axis + usize::from(neg), second > 0.98 * m)
    }

    /// `lib/water.hlsl`'s `ps_main` worked on the CPU at world point `p`
    /// seen from `eye`, then identityLight 1 and the exp fog, its keep
    /// interpolated from the vertices of the plane's triangle under `p`
    /// (`Fog_VertexCalc`, cod2-client-perf.md 9).
    #[allow(clippy::too_many_arguments)]
    fn hlsl(
        tex: &[u8],
        faces: &[[u8; 4]; 6],
        color: [f32; 3],
        sun: (Vec3, Vec3),
        p: Vec3,
        eye: Vec3,
        fog: Option<([f32; 3], f32)>,
    ) -> ([f32; 3], bool) {
        let uv = [p.x / UV_SPAN, p.y / UV_SPAN];
        let eye_dir = (p - eye).normalize();
        let mut h = 0.5f32;
        let err = h - sample(tex, [uv[0] * 0.5, uv[1] * 0.5]);
        h -= err;
        let _ = h;
        let uv = [
            uv[0] + err * eye_dir.x * (1.5 / 64.0),
            uv[1] + err * eye_dir.y * (1.5 / 64.0),
        ];
        let turb = |uv: [f32; 2]| {
            let (mut t, mut scale, mut at) = (0.0, 1.0, uv);
            for _ in 0..3 {
                t += sample(tex, at) * scale;
                at = [at[0] * 3.7, at[1] * 3.7];
                scale *= 0.6;
            }
            t
        };
        let h00 = turb(uv);
        let h10 = turb([uv[0] + 0.5 / 128.0, uv[1]]);
        let h01 = turb([uv[0], uv[1] + 0.5 / 128.0]);
        let n = Vec3::new(h10 - h00, h01 - h00, 1.0).normalize();
        let mut r = eye_dir - 2.0 * n.dot(eye_dir) * n;
        let spec = r.dot(sun.0).max(0.0);
        r.z = r.z.abs();
        let (f, seam) = face(r);
        let env = faces[f].map(|b| f32::from(b) / 255.0);
        let mut out = [0.0; 3];
        for k in 0..3 {
            let diffuse = color[k] * n.z;
            out[k] = diffuse + (env[k] - diffuse) * env[3] + spec.powf(64.0) * sun.1[k];
        }
        if let Some((fc, density)) = fog {
            // the two triangles of `water_plane_matches_the_hlsl`'s quad,
            // split along y = x; world barycentrics are what the
            // perspective-correct interpolation gives
            let s = 6000.0;
            let (a, b, c) = if p.y <= p.x {
                (
                    Vec3::new(-s, -s, 0.0),
                    Vec3::new(s, -s, 0.0),
                    Vec3::new(s, s, 0.0),
                )
            } else {
                (
                    Vec3::new(-s, -s, 0.0),
                    Vec3::new(s, s, 0.0),
                    Vec3::new(-s, s, 0.0),
                )
            };
            let area = |u: Vec3, v: Vec3, w: Vec3| (v - u).cross(w - u).z;
            let whole = area(a, b, c);
            let (wa, wb) = (area(p, b, c) / whole, area(a, p, c) / whole);
            let k = |v: Vec3| (-density * (v - eye).length()).exp();
            let keep = wa * k(a) + wb * k(b) + (1.0 - wa - wb) * k(c);
            for k in 0..3 {
                out[k] = fc[k] + (out[k] - fc[k]) * keep;
            }
        }
        (out, seam)
    }

    /// Renders a water plane on a software adapter against the HLSL worked
    /// on the CPU at each pixel centre: within 4 of 255 on 99% of the
    /// channels off a cube seam, under no fog and exp fog. Vulkan or Metal
    /// only; passes vacuously without an adapter.
    #[test]
    fn water_plane_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let camera = camera_layout(&device);
        let water = WaterLit {
            color: [0.2, 0.3, 0.4, 1.0],
            cube: String::new(),
            width: 64,
            lengths: [37.0, 37.0],
            amplitude: 0.06,
            wind_speed: 76.0,
            wind_direction: [1.0, 0.0],
        };
        let sh = Material {
            name: "water_test".into(),
            water: Some(water.clone()),
            nofog: false,
            ..Material::from_shader(&vcod_common::shader::Shader::default())
        };
        let faces: [[u8; 4]; 6] = [
            [200, 40, 40, 255],
            [40, 200, 40, 128],
            [40, 40, 200, 64],
            [200, 200, 40, 200],
            [120, 160, 220, 180],
            [10, 10, 10, 0],
        ];
        let cube_faces: Vec<Image> = faces
            .iter()
            .map(|c| Image {
                width: 64,
                height: 64,
                data: ImageData::Rgba8(c.repeat(64 * 64)),
            })
            .collect();
        let cube = upload_cube(&device, &queue, "test cube", &cube_faces).unwrap();
        let sun_dir = Vec3::new(1.0, 0.2, 0.6).normalize();
        let sun_col = Vec3::new(0.6, 0.5, 0.4);
        let sun_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&SunUniform {
                dir: sun_dir.extend(0.0).to_array(),
                color: sun_col.extend(1.0).to_array(),
                specular: sun_col.extend(1.0).to_array(),
                ..bytemuck::Zeroable::zeroed()
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let format = super::super::SCENE_FORMAT;
        let mut b = WaterBuilder::new(&device, &camera);
        let _ = b.bind(
            &device,
            &queue,
            format,
            &Pk3Fs::empty(),
            &sun_buf,
            &sh,
            (StageVariant::Opaque, true, false),
        );
        // the bound group's map, rebuilt without mips over the test heights
        b.material_of.clear();
        b.material_bgs.clear();
        b.sims.clear();
        let i = b.add(&device, &queue, &sun_buf, &sh, &water, &cube, false);
        let tex = heights();
        write_levels(
            &queue,
            &b.sims[i as usize].texture,
            std::slice::from_ref(&tex),
        );
        let waters = b.finish();

        let eye = Vec3::new(0.0, 0.0, 120.0);
        let target = Vec3::new(100.0, 30.0, 0.0);
        let view_proj =
            glam::camera::rh::proj::directx::perspective(1.2, W as f32 / H as f32, 4.0, 8192.0)
                * glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Z);
        // a plane well past the view on z = 0
        let verts: Vec<vcod_common::mapworld::DrawVert> = [
            [-6000.0, -6000.0],
            [6000.0, -6000.0],
            [6000.0, 6000.0],
            [-6000.0, -6000.0],
            [6000.0, 6000.0],
            [-6000.0, 6000.0],
        ]
        .map(|[x, y]: [f32; 2]| vcod_common::mapworld::DrawVert {
            pos: [x, y, 0.0],
            uv: [x / UV_SPAN, y / UV_SPAN],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        })
        .to_vec();
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let tangents = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0u8; 24 * 6],
            usage: wgpu::BufferUsages::VERTEX,
        });
        let inv = view_proj.inverse();
        for fog in [None, Some(([0.5, 0.55, 0.6], 0.0008))] {
            let cam = camera_group(&device, &queue, &camera, &camera_bytes(view_proj, eye, fog));
            let img = render(&device, &queue, format, |pass| {
                pass.set_pipeline(waters.pipeline(StageVariant::Opaque, true, false));
                pass.set_bind_group(0, &cam, &[]);
                pass.set_bind_group(1, &waters.material_bgs[i as usize], &[]);
                pass.set_bind_group(2, &waters.sims[i as usize].bg, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.set_vertex_buffer(1, tangents.slice(..));
                pass.draw(0..6, 0..1);
            });
            let (mut n, mut close, mut worst) = (0usize, 0usize, 0.0f32);
            for py in 0..H {
                for px in 0..W {
                    let ndc = [
                        (px as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                        1.0 - (py as f32 + 0.5) / H as f32 * 2.0,
                    ];
                    let un = |z: f32| {
                        let v = inv * Vec4::new(ndc[0], ndc[1], z, 1.0);
                        v.truncate() / v.w
                    };
                    let (a, bb) = (un(0.0), un(1.0));
                    let d = bb - a;
                    assert!(d.z < 0.0, "pixel {px} {py} misses the plane");
                    let p = a + d * (-a.z / d.z);
                    let (want, seam) = hlsl(
                        &tex,
                        &faces,
                        [0.2, 0.3, 0.4],
                        (sun_dir, sun_col),
                        p,
                        eye,
                        fog,
                    );
                    if seam {
                        continue;
                    }
                    let got = &img[(py * W + px) * 4..][..3];
                    for k in 0..3 {
                        let e = (f32::from(got[k]) - (want[k].clamp(0.0, 1.0) * 255.0)).abs();
                        n += 1;
                        close += usize::from(e <= 4.0);
                        worst = worst.max(e);
                    }
                }
            }
            eprintln!("fog {fog:?}: {close}/{n} within 4, worst {worst}");
            assert!(n > W * H * 3 / 2, "{n}");
            assert!(close * 100 >= n * 99, "{close}/{n}, worst {worst}");
        }
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
    }

    /// Runs `draw` into a `W` x `H` multisampled target of `format` and
    /// returns its resolved texels, four bytes each, row by row.
    fn render(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        draw: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) -> Vec<u8> {
        let target = |samples, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: W as u32,
                    height: H as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let att = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let samples = super::super::msaa_samples();
        let msaa = target(samples, format, att);
        let depth = target(samples, super::super::DEPTH_FORMAT, att);
        let resolved = target(1, format, att | wgpu::TextureUsages::COPY_SRC);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * H as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (mv, dv, rv) = (
            msaa.create_view(&Default::default()),
            depth.create_view(&Default::default()),
            resolved.create_view(&Default::default()),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: if samples > 1 { &mv } else { &rv },
                    depth_slice: None,
                    resolve_target: (samples > 1).then_some(&rv),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv,
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
            draw(&mut pass);
        }
        enc.copy_texture_to_buffer(
            resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(H as u32),
                },
            },
            wgpu::Extent3d {
                width: W as u32,
                height: H as u32,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        let mut out = Vec::with_capacity(W * H * 4);
        for row in 0..H {
            out.extend_from_slice(&data[row * 256..][..W * 4]);
        }
        out
    }
}
