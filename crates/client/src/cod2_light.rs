//! CoD2 model lighting off the map's light grid (docs/research/
//! cod2-materials.md 9.2): an entity's grid sample packed into the light
//! set `dynamic_model.wgsl` reads, whose probe arm weights the eight corner
//! colours by the normal as the `lightprobe` techniques do and adds the
//! sun, and a static model's sample cached per model. An entity's probe
//! stage evaluates it per pixel at its normal mapped normal with the sun's
//! specular (cod2_model.wgsl), a static model's `lprobe_smc*` stage the
//! same off the cache (cod2_world.wgsl's `fs_smodel`); the other static
//! model sets (`ambient_*`, an additive pass) bake it per vertex.

use std::sync::Arc;
use vcod_common::asset_source::AssetSource;

use glam::Vec3;
use vcod_cod2::lightgrid::{self, GridSample, SIGHT_MASK, StaticModelKeys};
use vcod_cod2::world::SunLight;
use vcod_common::collision::CollisionWorld;
use vcod_common::mapworld::{LightGrid, MapWorld};
use vcod_common::props::Props;

use crate::entity_light::{GpuLight, GpuLightSet};

/// `LightSet.ambient.w` of a probe set: the shader's probe arm.
pub const PROBE_SET: f32 = -1.0;

/// The light set of an entity lit by `s`, its colours and the sun's
/// already times `identity_light` as the scene target holds them:
/// `ambient` the sun's colour times its visibility (`lightColor0` and its
/// alpha) with w [`PROBE_SET`]; light 0's five vectors and light 1's `pos`
/// `lightGridColorsR0`, `R1`, `G0`, `G1`, `B0`, `B1`; light 1's `diffuse`
/// the unit vector toward the sun (`lightPosition0`), its `ambient` the
/// sun's specular colour (`lightSpecular0`, the colour times
/// `r_specularColorScale` 1) with w the visibility (cod2_model.wgsl).
pub fn probe_set(s: &GridSample, sun: &SunLight, identity_light: f32) -> GpuLightSet {
    let k = identity_light;
    let half =
        |c: usize, h: usize| -> [f32; 4] { std::array::from_fn(|i| s.rgb[c][h * 4 + i] * k) };
    let col = Vec3::from_array(sun.colour) * (s.sun * k);
    let mut set = GpuLightSet {
        ambient: [col.x, col.y, col.z, PROBE_SET],
        ..GpuLightSet::default()
    };
    set.lights[0] = GpuLight {
        pos: half(0, 0),
        diffuse: half(0, 1),
        ambient: half(1, 0),
        atten: half(1, 1),
        spot_dir: half(2, 0),
    };
    let [x, y, z] = sun.dir;
    set.lights[1].pos = half(2, 1);
    set.lights[1].diffuse = [x, y, z, 0.0];
    let spec = Vec3::from_array(sun.colour) * k;
    set.lights[1].ambient = [spec.x, spec.y, spec.z, s.sun];
    set
}

/// The map's grid and sun, and the world the occluded corners' sight
/// traces run through.
pub struct Cod2Lighting {
    grid: LightGrid,
    sun: SunLight,
    clip: Arc<CollisionWorld>,
}

impl Cod2Lighting {
    /// `None` for a map without a light grid (every CoD1 map).
    #[cfg(test)]
    pub fn new(bsp: &MapWorld) -> Option<Self> {
        bsp.light_grid.as_ref()?;
        Self::with_clip(bsp, CollisionWorld::build(bsp, &[]))
    }

    /// [`Self::new`] over `clip`, `bsp`'s world built already
    /// (`CollisionWorld::build(bsp, &[])`).
    pub fn with_clip(bsp: &MapWorld, clip: CollisionWorld) -> Option<Self> {
        let grid = bsp.light_grid.clone()?;
        Some(Cod2Lighting {
            grid,
            sun: vcod_cod2::world::sun_light(&bsp.entities),
            clip: Arc::new(clip),
        })
    }

    /// The map's sun.
    pub fn sun(&self) -> &SunLight {
        &self.sun
    }

    /// The grid at `p`, a corner flagged occluded used only when the world
    /// does not cut the sight line to it.
    pub fn sample(&self, p: Vec3) -> GridSample {
        let blocked =
            |a: Vec3, b: Vec3| self.clip.point_trace(a, b, SIGHT_MASK, false).fraction < 1.0;
        lightgrid::sample(&self.grid, p, Some(&blocked))
    }

    /// An entity's light set, sampled at its lighting point.
    pub fn entity_set(&self, point: Vec3, identity_light: f32) -> GpuLightSet {
        probe_set(&self.sample(point), &self.sun, identity_light)
    }

    /// Rewrites the colour of every `lightingDiffuse` prop vertex
    /// (`vert_lit`) from its placement's cached sample, `vert_placement`
    /// naming each vertex's index into `props::placements`, whose world
    /// box (`bounds`) is sampled at its centre, times the colour the model
    /// stores (`vert_colour`). An unlit material's vertices keep the
    /// model's colours. A per pixel static model's (`vert_smodel`) keep
    /// them too and take their block's centre in the returned cache as
    /// their lightmap coords; `None` when no vertex is one.
    pub fn light_props(
        &self,
        fs: &dyn AssetSource,
        entities: &str,
        props: &mut Props,
    ) -> Option<SmodelCache> {
        self.light_props_on(fs, entities, props, |todo, f| {
            vcod_common::laps::par_map(todo, f)
        })
    }

    /// [`Self::light_props`] with the placements' samples taken by
    /// `map`, which runs `f` over every placement a vertex needs and
    /// keeps their order: on every core in a load, one at a time in the
    /// test that holds the two to the same bytes.
    fn light_props_on(
        &self,
        fs: &dyn AssetSource,
        entities: &str,
        props: &mut Props,
        map: impl FnOnce(&[usize], &(dyn Fn(&usize) -> GridSample + Sync)) -> Vec<GridSample>,
    ) -> Option<SmodelCache> {
        let blocks: Vec<_> = vcod_common::bsp::entity_blocks(entities)
            .into_iter()
            .filter(|b| b.get("classname").map(String::as_str) == Some("misc_model"))
            // the placements `props::placements` lists, in its order
            .filter(|b| {
                b.get("model")
                    .is_some_and(|m| m.replace('\\', "/").starts_with("xmodel/"))
            })
            .collect();
        let needs =
            |i: usize| props.vert_lit[i] || props.vert_smodel.get(i).copied().unwrap_or(false);
        let mut wanted = vec![false; props.bounds.len()];
        for i in (0..props.verts.len()).filter(|&i| needs(i)) {
            wanted[props.vert_placement[i] as usize] = true;
        }
        let todo: Vec<usize> = (0..wanted.len()).filter(|&pi| wanted[pi]).collect();
        let sample = |&pi: &usize| {
            let block = blocks.get(pi);
            let keys = block.map_or(
                StaticModelKeys {
                    ground: None,
                    direct: 1.0,
                    indirect: 1.0,
                },
                StaticModelKeys::of,
            );
            let ground_lit = block
                .and_then(|b| b.get("model"))
                .is_some_and(|m| ground_lit(fs, m));
            let (lo, hi) = props.bounds[pi];
            let blocked =
                |a: Vec3, b: Vec3| self.clip.point_trace(a, b, SIGHT_MASK, false).fraction < 1.0;
            lightgrid::static_model(
                &self.grid,
                (lo + hi) * 0.5,
                ground_lit,
                &keys,
                Some(&blocked),
            )
        };
        let samples = map(&todo, &sample);
        let mut cache: Vec<Option<GridSample>> = vec![None; props.bounds.len()];
        for (&pi, s) in todo.iter().zip(samples) {
            cache[pi] = Some(s);
        }
        let grid = SmodelCache::grid(props.bounds.len());
        let mut any_smodel = false;
        for (i, v) in props.verts.iter_mut().enumerate() {
            let smodel = props.vert_smodel.get(i).copied().unwrap_or(false);
            if !props.vert_lit[i] && !smodel {
                continue;
            }
            let pi = props.vert_placement[i] as usize;
            let s = cache[pi].expect("every needed placement sampled");
            if smodel {
                any_smodel = true;
                v.lm_uv = grid.centre(pi);
                continue;
            }
            let n = Vec3::from_array(v.normal);
            v.color = match props.vert_ambient[i] {
                false => prop_colour(&s, &self.sun, n, props.vert_colour[i]),
                true => ambient_prop_colour(&s, &self.sun, props.vert_colour[i]),
            };
        }
        any_smodel.then(|| SmodelCache::of(&cache))
    }
}

/// The static model lighting cache as vcod uploads it: retail's 3D
/// texture (`R_CacheStaticModelLighting`, Mac 0x107512) of a 2 x 2 x 2
/// block per model, model `i` at block (i % 128, i / 128) by its
/// placement index, split into its two z layers, each RGBA8 with the
/// corner's red, green and blue and the sun visibility in alpha
/// (cod2-materials.md 9.6).
pub struct SmodelCache {
    pub width: u32,
    pub height: u32,
    /// Layer 0 holds corners 0-3 (z -1), layer 1 corners 4-7.
    pub layers: [Vec<u8>; 2],
}

impl SmodelCache {
    /// Blocks along a row.
    pub const ROW: usize = 128;

    /// The cache of `samples`, by placement index; `None` leaves a block
    /// black.
    pub fn of(samples: &[Option<GridSample>]) -> SmodelCache {
        Self::grid(samples.len()).fill(samples)
    }

    /// An all black cache of `placements` blocks.
    fn grid(placements: usize) -> SmodelCache {
        let rows = placements.div_ceil(Self::ROW).max(1);
        let (width, height) = (2 * Self::ROW as u32, 2 * rows as u32);
        let size = (width * height * 4) as usize;
        SmodelCache {
            width,
            height,
            layers: [vec![0; size], vec![0; size]],
        }
    }

    /// The texture coords of placement `pi`'s block centre: retail's
    /// `baseLightingCoords` in x and y (its z is the layers' middle).
    pub fn centre(&self, pi: usize) -> [f32; 2] {
        let (bx, by) = ((pi % Self::ROW) as f32, (pi / Self::ROW) as f32);
        [
            (2.0 * bx + 1.0) / self.width as f32,
            (2.0 * by + 1.0) / self.height as f32,
        ]
    }

    fn fill(mut self, samples: &[Option<GridSample>]) -> SmodelCache {
        for (pi, s) in samples.iter().enumerate() {
            let Some(s) = s else { continue };
            let (bx, by) = (2 * (pi % Self::ROW), 2 * (pi / Self::ROW));
            for j in 0..8 {
                let (x, y) = (bx + (j & 1), by + ((j >> 1) & 1));
                let at = (y * self.width as usize + x) * 4;
                self.layers[j >> 2][at..at + 4].copy_from_slice(&smodel_texel(s, j));
            }
        }
        self
    }
}

/// Corner `j`'s cached texel: each value `floor(v * 255 + 0.5)` clamped
/// to a byte, as `R_CacheStaticModelLighting` stores them.
pub fn smodel_texel(s: &GridSample, j: usize) -> [u8; 4] {
    let byte = |v: f32| (v * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
    [
        byte(s.rgb[0][j]),
        byte(s.rgb[1][j]),
        byte(s.rgb[2][j]),
        byte(s.sun),
    ]
}

/// Whether a model's descriptor sets flag bit 0, which lets `gndLt` light
/// it.
fn ground_lit(fs: &dyn AssetSource, model: &str) -> bool {
    let name = model.replace('\\', "/");
    fs.read(&name)
        .and_then(|d| vcod_cod2::xmodel::descriptor_flags(&d))
        .is_some_and(|f| f & 1 != 0)
}

/// A static model's vertex colour as the prop path stores it (display
/// value times identityLight 0.5, `vs_prop` scales by the current one):
/// the cached probe at the normal plus the sun's diffuse by its cached
/// visibility, times the colour the model stores (`lprobe_smc.hlsl`,
/// `lib/lightprobe_sm.hlsl`: the colour map times the vertex colour is
/// the diffuse), and the model's alpha, which the `_a` techniques output
/// (`lib/lightprobe_sm.hlsl` under `USE_ALPHA`).
pub fn prop_colour(s: &GridSample, sun: &SunLight, n: Vec3, model: [u8; 4]) -> [u8; 4] {
    let probe = s.light(&lightgrid::static_weights(n));
    let ndl = n.dot(Vec3::from_array(sun.dir)).max(0.0);
    stored(probe + Vec3::from_array(sun.colour) * (s.sun * ndl), model)
}

/// [`prop_colour`] for an `ambient_*` set's `lprobe_smc_amb`
/// (`lib/lightprobe_sm_ambient.hlsl`): the lighting texture read at the
/// model's base coords, the middle of its 2 x 2 x 2 block, so the eight
/// corners' mean, plus the sun's colour by the visibility, no `N.L`.
pub fn ambient_prop_colour(s: &GridSample, sun: &SunLight, model: [u8; 4]) -> [u8; 4] {
    let mean = s.light(&[0.125; 8]);
    stored(mean + Vec3::from_array(sun.colour) * s.sun, model)
}

/// `light` times the model's colour, at identityLight 0.5 in bytes, and
/// the model's alpha.
fn stored(light: Vec3, model: [u8; 4]) -> [u8; 4] {
    let c = light * Vec3::new(model[0].into(), model[1].into(), model[2].into()) / 255.0;
    let byte = |v: f32| (v * 0.5 * 255.0).clamp(0.0, 255.0) as u8;
    [byte(c.x), byte(c.y), byte(c.z), model[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GridSample {
        let mut s = GridSample {
            rgb: [[0.0; 8]; 3],
            sun: 0.5,
        };
        for j in 0..8 {
            s.rgb[0][j] = j as f32 / 10.0;
            s.rgb[1][j] = 0.05;
            s.rgb[2][j] = 1.0 - j as f32 / 10.0;
        }
        s
    }

    fn sun() -> SunLight {
        SunLight {
            dir: [0.0, 0.0, 1.0],
            colour: [1.0, 0.5, 0.25],
        }
    }

    #[test]
    fn probe_set_packs_the_six_constants_and_the_sun() {
        let set = probe_set(&sample(), &sun(), 0.5);
        let near = |got: [f32; 4], want: [f32; 4]| {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
                "{got:?} {want:?}"
            )
        };
        near(set.ambient, [0.25, 0.125, 0.0625, PROBE_SET]);
        let l = &set.lights;
        near(l[0].pos, [0.0, 0.05, 0.1, 0.15]);
        near(l[0].diffuse, [0.2, 0.25, 0.3, 0.35]);
        near(l[0].ambient, [0.025; 4]);
        near(l[0].atten, [0.025; 4]);
        near(l[0].spot_dir, [0.5, 0.45, 0.4, 0.35]);
        near(l[1].pos, [0.3, 0.25, 0.2, 0.15]);
        near(l[1].diffuse, [0.0, 0.0, 1.0, 0.0]);
        // the sun's colour at half scale and its visibility 0.5
        near(l[1].ambient, [0.5, 0.25, 0.125, 0.5]);
    }

    /// A normal straight up reads corner 7 and the whole sun; straight down
    /// corner 0 and no sun.
    #[test]
    fn prop_colour_reads_the_corner_and_the_sun() {
        let s = sample();
        let up = prop_colour(&s, &sun(), Vec3::Z, [255; 4]);
        // (0.7 + 0.5) / 2 * 255
        assert_eq!(up, [153, 38, 54, 255]);
        let down = prop_colour(&s, &sun(), -Vec3::Z, [255; 4]);
        assert_eq!(down, [0, 6, 127, 255]);
    }

    /// The model's stored colour scales the whole lit colour, the probe
    /// and the sun alike (`lprobe_smc.hlsl`): the crate's 125 grey keeps
    /// 125/255 of the white model's.
    #[test]
    fn prop_colour_multiplies_the_model_colour() {
        let s = sample();
        let n = Vec3::new(0.3, -0.4, 0.866).normalize();
        let white = prop_colour(&s, &sun(), n, [255; 4]);
        let grey = prop_colour(&s, &sun(), n, [125, 125, 125, 255]);
        let probe = s.light(&lightgrid::static_weights(n)) + Vec3::new(1.0, 0.5, 0.25) * 0.5 * n.z;
        for k in 0..3 {
            let want = probe[k] * 125.0 / 255.0 * 0.5 * 255.0;
            assert!((f32::from(grey[k]) - want).abs() <= 1.0, "{grey:?} {probe}");
            assert!(grey[k] < white[k] || white[k] == 0, "{grey:?} {white:?}");
        }
        assert_eq!(grey[3], 255);
        let tint = prop_colour(&s, &sun(), Vec3::Z, [255, 0, 128, 255]);
        assert_eq!(tint, [153, 0, 27, 255]);
    }

    /// `lprobe_smc_amb`: the corners' mean and the sun by its visibility,
    /// whatever the normal, times the model's colour.
    #[test]
    fn ambient_prop_colour_reads_the_block_centre() {
        let s = sample();
        // red's corners average 0.35, plus the sun's 1.0 * 0.5
        let c = ambient_prop_colour(&s, &sun(), [255; 4]);
        assert_eq!(c, [108, 38, 98, 255]);
        let half = ambient_prop_colour(&s, &sun(), [128, 128, 128, 255]);
        assert_eq!(half, [54, 19, 49, 255]);
    }

    /// A device off a headless adapter (Vulkan or Metal), `None` where the
    /// machine has none.
    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let info = adapter.get_info();
        eprintln!("adapter: {} {:?}", info.name, info.backend);
        if info.backend == wgpu::Backend::Gl {
            return None;
        }
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    /// `src` between the line holding `from` and the one holding `to`.
    fn cut<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap();
        let b = a + src[a..].find(to).unwrap();
        &src[a..b]
    }

    const W: u32 = 128;
    const H: u32 = 64;

    /// The sphere's normal under pixel (x, y) of a `H` x `H` half, seen
    /// from -y: screen right +x, screen up +z. `None` off the sphere.
    fn sphere_normal(x: u32, y: u32) -> Option<Vec3> {
        let u = ((x % H) as f32 + 0.5) / H as f32 * 2.0 - 1.0;
        let v = 1.0 - (y as f32 + 0.5) / H as f32 * 2.0;
        let r = u * u + v * v;
        (r < 0.9).then(|| Vec3::new(u, -(1.0 - r).sqrt(), v))
    }

    /// Two spheres drawn offscreen through `dynamic_model.wgsl`'s own
    /// `probe_lighting` and `stage_colour`, each lit by one light set over
    /// a white colour map: every covered pixel is `lprobe.hlsl`'s colour
    /// computed on the CPU, the vertex colour times the probe at its normal
    /// plus the sun, in display units at identityLight 1. The first sphere
    /// stores white, the second the 125 grey of `prop_crate_dak1`'s body.
    /// With `COD2_DIR` the sets are mp_carentan's sunlit street and
    /// upstairs room (lightgrid_maps.rs), else two made-up samples.
    /// `VCOD_LIGHTGRID_TGA` names a TGA to write the picture to.
    #[test]
    fn probe_shader_lights_a_model_as_the_cpu_does() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let (samples, sun) = match vcod_common::testing::cod2_fs() {
            Some(fs) => {
                let data = fs.read("maps/mp/mp_carentan.d3dbsp").unwrap();
                let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
                let l = Cod2Lighting::new(&bsp).unwrap();
                let pts = [Vec3::new(984.0, -736.0, 32.0), Vec3::new(0.0, 696.0, 192.0)];
                (pts.map(|p| l.sample(p)), l.sun)
            }
            None => {
                let mut dark = sample();
                dark.sun = 0.0;
                ([sample(), dark.scaled(1.0, 0.3)], sun())
            }
        };
        let sets: Vec<GpuLightSet> = samples.iter().map(|s| probe_set(s, &sun, 1.0)).collect();

        let src = include_str!("dynamic_model.wgsl");
        let wgsl = format!(
            "{}{}\n@group(0) @binding(0) var<storage, read> light_sets: array<LightSet>;\n{}{}{}",
            cut(src, "struct EntLight", "// The frame's light sets"),
            cut(src, "const F_AF_GT0", "// GL_LIGHTING's vertex colour"),
            cut(
                include_str!("cod2_model.wgsl"),
                "fn probe_grid",
                "// The texcoords `TexScroll`"
            ),
            cut(src, "fn stage_colour", "// alphaFunc decode"),
            r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let h = 64.0;
    let half = u32(at.x / h);
    let u = (at.x - f32(half) * h) / h * 2.0 - 1.0;
    let v = 1.0 - at.y / h * 2.0;
    let r = u * u + v * v;
    if (r >= 0.9) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let n = vec3<f32>(u, -sqrt(1.0 - r), v);
    let vcol = select(vec4<f32>(1.0), vec4<f32>(vec3<f32>(125.0 / 255.0), 1.0), half == 1u);
    let c = stage_colour(vec4<f32>(1.0), probe_lighting(light_sets[half], n), vcol, F_LIT | F_TINT_LIGHT, vec4<f32>(1.0), 1.0);
    return vec4<f32>(c.rgb, 1.0);
}
"#
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("probe test"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        use wgpu::util::DeviceExt;
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&sets),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            }],
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(W * 4 * H),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = target.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
        let px = readback.slice(..).get_mapped_range().unwrap().to_vec();

        let sun_dir = Vec3::from_array(sun.dir);
        let mut checked = 0;
        for y in 0..H {
            for x in 0..W {
                let Some(n) = sphere_normal(x, y) else {
                    continue;
                };
                // the rim, where the pixel centre's normal is steep, is
                // left to the GPU's rounding
                if n.y > -0.2 {
                    continue;
                }
                let s = &samples[(x / H) as usize];
                let vcol = if x < H { 1.0 } else { 125.0 / 255.0 };
                let want = vcol
                    * (s.light(&lightgrid::entity_weights(n))
                        + Vec3::from_array(sun.colour) * (s.sun * n.dot(sun_dir).max(0.0)));
                let at = ((y * W + x) * 4) as usize;
                for k in 0..3 {
                    let w = (want[k].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + k]) - w).abs() <= 2.0,
                        "({x}, {y}) n {n}: {:?} vs {want}",
                        &px[at..at + 3]
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 3000, "{checked}");
        // the two models differ, the first (the street) the brighter
        let mean = |half: u32| {
            let (mut sum, mut n) = (0u32, 0u32);
            for y in 0..H {
                for x in half * H..(half + 1) * H {
                    if sphere_normal(x, y).is_some() {
                        let at = ((y * W + x) * 4) as usize;
                        sum += u32::from(px[at]) + u32::from(px[at + 1]) + u32::from(px[at + 2]);
                        n += 3;
                    }
                }
            }
            sum as f32 / n as f32
        };
        assert!(mean(0) > mean(1) * 2.0, "{} {}", mean(0), mean(1));
        if let Ok(path) = std::env::var("VCOD_LIGHTGRID_TGA") {
            image::save_buffer(&path, &px, W, H, image::ColorType::Rgba8).unwrap();
            eprintln!("wrote {path}");
        }
    }

    /// The placements sampled on every core light the props to the same
    /// bytes as sampled one at a time: every vertex colour and lightmap
    /// coord and the static model cache. Needs `COD2_DIR`.
    #[test]
    fn props_lit_on_every_core_match_one_at_a_time() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        for map in ["mp_toujane", "mp_carentan", "mp_burgundy"] {
            let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
            let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
            // Every vertex lit, every other placement's a static model's.
            let build = || {
                let mut lighting = vcod_common::static_light::StaticLighting::new(&bsp);
                let mut props = vcod_common::props::build(&fs, &bsp, &mut lighting, &|_| {
                    vcod_common::props::SkinDraw::default()
                });
                props.vert_lit.iter_mut().for_each(|l| *l = true);
                props.vert_smodel = props.vert_placement.iter().map(|p| p % 2 == 0).collect();
                props
            };
            let (mut props, mut one) = (build(), build());
            let l = Cod2Lighting::new(&bsp).unwrap();
            let a = l.light_props(&fs, &bsp.entities, &mut props).unwrap();
            let b = l
                .light_props_on(&fs, &bsp.entities, &mut one, |todo, f| {
                    todo.iter().map(f).collect()
                })
                .unwrap();
            let digest = |p: &Props| {
                p.verts
                    .iter()
                    .map(|v| (v.color, v.lm_uv.map(f32::to_bits)))
                    .collect::<Vec<_>>()
            };
            assert!(digest(&props) == digest(&one), "{map}");
            assert!(a.layers == b.layers && (a.width, a.height) == (b.width, b.height));
        }
    }

    /// mp_toujane's props lit from the grid: every `lightingDiffuse` vertex
    /// takes its placement's sample, the colours vary over the map, and a
    /// placement's vertices facing the sun outshine those facing away.
    /// Needs `COD2_DIR`.
    #[test]
    fn toujane_props_take_the_grid() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        let mut lighting = vcod_common::static_light::StaticLighting::new(&bsp);
        let mut props = vcod_common::props::build(&fs, &bsp, &mut lighting, &|_| {
            vcod_common::props::SkinDraw::default()
        });
        let l = Cod2Lighting::new(&bsp).unwrap();
        let before = props.verts.clone();
        let t = std::time::Instant::now();
        l.light_props(&fs, &bsp.entities, &mut props);
        eprintln!(
            "{} prop vertices lit in {:.0} ms",
            props.verts.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(props.vert_placement.len(), props.verts.len());
        let lit: Vec<_> = props
            .verts
            .iter()
            .zip(&props.vert_lit)
            .filter_map(|(v, &l)| l.then_some(v))
            .collect();
        assert!(
            lit.len() * 2 > props.verts.len(),
            "{} of {}",
            lit.len(),
            props.verts.len()
        );
        let changed = props
            .verts
            .iter()
            .zip(&before)
            .filter(|(a, b)| a.color != b.color)
            .count();
        assert!(changed * 2 > lit.len(), "{changed}");
        let first = lit[0].color;
        assert!(lit.iter().any(|v| v.color != first));
        let sun = Vec3::from_array(l.sun.dir);
        let (mut toward, mut away) = ((0u64, 0u64), (0u64, 0u64));
        for v in &lit {
            let n = Vec3::from_array(v.normal);
            let sum = u64::from(v.color[0]) + u64::from(v.color[1]) + u64::from(v.color[2]);
            if n.dot(sun) > 0.5 {
                toward = (toward.0 + sum, toward.1 + 1);
            } else if n.dot(sun) < -0.5 {
                away = (away.0 + sum, away.1 + 1);
            }
        }
        let (t, a) = (
            toward.0 as f64 / toward.1 as f64,
            away.0 as f64 / away.1 as f64,
        );
        eprintln!("facing the sun {t:.1}, away {a:.1}");
        assert!(t > a, "{t} {a}");
    }

    /// The world material with the most near-vertical area within 384
    /// units of `p`: the walls around a spot.
    fn nearby_wall(bsp: &MapWorld, p: Vec3) -> String {
        let mut area: std::collections::HashMap<u16, f32> = Default::default();
        for soup in &bsp.soups {
            let v0 = soup.first_vertex as usize;
            let idx = &bsp.indices
                [soup.first_index as usize..soup.first_index as usize + soup.index_count as usize];
            for t in idx.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| Vec3::from_array(bsp.verts[v0 + i as usize].pos));
                let cross = (b - a).cross(c - a);
                let n = cross.normalize_or_zero();
                if n.z.abs() < 0.3 && ((a + b + c) / 3.0).distance(p) < 384.0 {
                    *area.entry(soup.material).or_default() += cross.length() * 0.5;
                }
            }
        }
        let (m, _) = area
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("walls near the spot");
        bsp.materials[m as usize].name.clone()
    }

    /// The axis default soldier at mp_toujane's sunlit dm spawn (2326,
    /// 485), posed by the client's body path: every part draws its own
    /// colour map, probe lit, under the grid sample at his lighting
    /// origin. He is drawn offscreen facing the sun, in front of a quad of
    /// the alley's wall material lit by the same sample. Retail draws both
    /// as colour map times light (`lightprobe.hlsl`; the wall's lightmap
    /// bakes the sun and bounce the grid stores), so the body's share of
    /// the wall's luminance is their colour maps' albedo ratio, 0.34 for
    /// the field grey against `toujane_plasterwall_white`, a little less
    /// lit (0.29: his sides turn from the sun, the wall clamps). The
    /// default image the parts drew before (mean 24 of 255) gave 0.11, the
    /// near-black silhouette, so the floor is 0.2. Needs `COD2_DIR`;
    /// `VCOD_BODY_TGA` names a TGA to write the picture to.
    #[test]
    fn toujane_sunlit_body_takes_its_colour_maps() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let Some((device, queue)) = headless_bc() else {
            eprintln!("no adapter with BC textures; skipped");
            return;
        };
        vcod_cod2::model::register();
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
        let l = Cod2Lighting::new(&bsp).unwrap();
        let origin = Vec3::new(2326.0, 485.0, 69.0);
        let frame = vcod_cod2::player::BodyFrame {
            origin,
            ..Default::default()
        };
        let set = l.entity_set(frame.light_origin(), 1.0);
        let sample = l.sample(frame.light_origin());
        assert!(sample.sun > 0.9, "the spawn is sunlit: {sample:?}");

        // the team's default soldier, posed by the client's own body path
        let mut players = crate::cod2_players::Cod2Players::new();
        let view = crate::cod2_players::Cod2PlayerView {
            number: 0,
            frame: vcod_cod2::player::BodyFrame {
                legs_anim: 127,
                ..frame
            },
            character: None,
            team: 1,
            weapon: None,
            gun: None,
        };
        let posed = players.posed(&fs, &[view], 1000, 50, 1000);
        let (_, rig, drawn) = &posed[0];
        let mut warns = vcod_common::shader::WarnSet::default();
        let image_of = |name: &str, warns: &mut vcod_common::shader::WarnSet| {
            let m = crate::entity_material::load(&fs, &lib, name, warns);
            assert!(crate::entity_material::is_lit(&m), "{name}: not probe lit");
            match &m.stages[0].bundles[..] {
                [b] => match &b.image {
                    vcod_common::material::ImageRef::Path(p) => p.clone(),
                    other => panic!("{name}: binds {other:?}"),
                },
                other => panic!("{name}: {} bundles", other.len()),
            }
        };
        // (texture path, vertices, indices) per surface; the wall last
        let mut draws: Vec<(String, Vec<[f32; 8]>, Vec<u32>)> = Vec::new();
        let sun = Vec3::from_array(l.sun.dir);
        let toward = Vec3::new(sun.x, sun.y, 0.0).normalize();
        // faces the sun, about the body's own origin
        let rot = glam::Quat::from_rotation_z(toward.y.atan2(toward.x));
        for ((name, model), (_, skin)) in rig.models.iter().zip(&drawn.models) {
            let (surfaces, materials) = model.lod_mesh(0).unwrap();
            for surf in surfaces {
                let path = image_of(&materials[surf.material], &mut warns);
                assert!(!path.contains("default"), "{name}: {path}");
                let verts = surf
                    .verts
                    .iter()
                    .map(|v| {
                        // dynamic_model.wgsl's skinning
                        let (mut p, mut n) = (Vec3::ZERO, Vec3::ZERO);
                        for (&b, &w) in v.bone_indices.iter().zip(&v.bone_weights) {
                            let m = skin[usize::from(b)];
                            p += w * m.transform_point3(Vec3::from_array(v.pos));
                            n += w * m.transform_vector3(Vec3::from_array(v.normal));
                        }
                        let (p, n) = (rot * p, rot * n);
                        [p.x, p.y, p.z, n.x, n.y, n.z, v.uv[0], v.uv[1]]
                    })
                    .collect();
                draws.push((
                    path,
                    verts,
                    surf.indices.iter().map(|&i| u32::from(i)).collect(),
                ));
            }
        }
        let wall = nearby_wall(&bsp, origin);
        let wall_image = match lib
            .get(&wall)
            .and_then(|m| m.texture(vcod_common::material::TexSemantic::Color))
        {
            Some(vcod_common::material::ImageRef::Path(p)) => p.clone(),
            other => panic!("{wall}: {other:?}"),
        };
        eprintln!("wall {wall}: {wall_image}");
        {
            // a 128-unit square 40 behind the body, facing the sun
            let right = Vec3::Z.cross(toward);
            let back = -toward * 40.0;
            let corner = |x: f32, z: f32| {
                let p = back + right * x + Vec3::Z * z;
                [
                    p.x,
                    p.y,
                    p.z,
                    toward.x,
                    toward.y,
                    toward.z,
                    x / 128.0,
                    -z / 128.0,
                ]
            };
            draws.push((
                wall_image,
                vec![
                    corner(-64.0, -24.0),
                    corner(64.0, -24.0),
                    corner(64.0, 104.0),
                    corner(-64.0, 104.0),
                ],
                vec![0, 1, 2, 0, 2, 3],
            ));
        }

        let src = include_str!("dynamic_model.wgsl");
        let wgsl = format!(
            "{}\n@group(0) @binding(0) var<storage, read> light_sets: array<LightSet>;\n{}{}",
            cut(src, "struct EntLight", "// The frame's light sets"),
            cut(
                include_str!("cod2_model.wgsl"),
                "fn probe_grid",
                "// The texcoords `TexScroll`"
            ),
            r#"
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) light: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

// orthographic, looking back along `toward`, x right, z up
@vertex
fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) uv: vec2<f32>) -> Out {
    let toward = normalize(light_sets[0].lights[1].diffuse.xyz * vec3<f32>(1.0, 1.0, 0.0));
    let right = cross(vec3<f32>(0.0, 0.0, 1.0), toward);
    var o: Out;
    o.clip = vec4<f32>(dot(p, right) / 48.0, (p.z - 38.0) / 48.0, 0.5 - dot(p, toward) / 200.0, 1.0);
    o.light = probe_lighting(light_sets[0], normalize(n));
    o.uv = uv;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, i.uv).rgb * i.light, 1.0);
}
"#
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("body test"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let depth_format = wgpu::TextureFormat::Depth32Float;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                })],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        use wgpu::util::DeviceExt;
        let sets = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&set),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let light_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sets.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let gpu: Vec<_> = draws
            .iter()
            .map(|(path, verts, indices)| {
                let img = vcod_common::assets::load_path_image(&fs, path);
                let view = crate::renderer::upload_levels(&device, &queue, path, &img, &[]);
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(1),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                (group, vb, ib, indices.len() as u32)
            })
            .collect();

        const BW: u32 = 256;
        const BH: u32 = 256;
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: BW,
                    height: BH,
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
        let colour = target(
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(BW * 4 * BH),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // pass 0 draws the body alone, for its mask; pass 1 the whole picture
        let mut pictures = Vec::new();
        for with_wall in [false, true] {
            let view = colour.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                        view: &depth_view,
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
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &light_group, &[]);
                let n = if with_wall { gpu.len() } else { gpu.len() - 1 };
                for (group, vb, ib, count) in &gpu[..n] {
                    pass.set_bind_group(1, group, &[]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, 0, 0..1);
                }
            }
            enc.copy_texture_to_buffer(
                colour.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(BW * 4),
                        rows_per_image: Some(BH),
                    },
                },
                wgpu::Extent3d {
                    width: BW,
                    height: BH,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            pictures.push(readback.slice(..).get_mapped_range().unwrap().to_vec());
            readback.unmap();
        }
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
        let (body, full) = (&pictures[0], &pictures[1]);
        let luma = |px: &[u8]| {
            0.299 * f32::from(px[0]) + 0.587 * f32::from(px[1]) + 0.114 * f32::from(px[2])
        };
        let (mut on, mut off) = ((0.0f32, 0u32), (0.0f32, 0u32));
        for px in 0..(BW * BH) as usize {
            let at = px * 4;
            let y = luma(&full[at..at + 4]);
            if body[at..at + 3] != [0, 0, 0] {
                on = (on.0 + y, on.1 + 1);
            } else if full[at..at + 3] != [0, 0, 0] {
                off = (off.0 + y, off.1 + 1);
            }
        }
        assert!(
            on.1 > 2000 && off.1 > 2000,
            "{} body, {} wall pixels",
            on.1,
            off.1
        );
        let (body_y, wall_y) = (on.0 / on.1 as f32, off.0 / off.1 as f32);
        eprintln!("body {body_y:.1} wall {wall_y:.1}: {:.2}", body_y / wall_y);
        if let Ok(path) = std::env::var("VCOD_BODY_TGA") {
            image::save_buffer(&path, full, BW, BH, image::ColorType::Rgba8).unwrap();
            eprintln!("wrote {path}");
        }
        assert!(body_y > wall_y * 0.2, "body {body_y} wall {wall_y}");
    }

    /// Draws `wgsl`'s full-screen `fs` into a `W` x `H` RGBA8 target with
    /// group 0 binding 0 the light sets, bindings 9..16 one linear sampler,
    /// group 3 binding 0 a zeroed stage params block but `eye_off` (the
    /// detail scale, 1) and bindings 1..4 the given textures, each `(w, h,
    /// format, bytes)`.
    fn draw_harness(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wgsl: &str,
        sets: &[GpuLightSet],
        maps: [(u32, u32, wgpu::TextureFormat, Vec<u8>); 4],
    ) -> Vec<u8> {
        let mut params: crate::renderer::StageParams = bytemuck::Zeroable::zeroed();
        params.eye_off = [1.0, 1.0, 0.0, 0.0];
        draw_harness_with(device, queue, wgsl, sets, maps, params)
    }

    /// [`draw_harness`] under the stage params `params`.
    fn draw_harness_with(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wgsl: &str,
        sets: &[GpuLightSet],
        maps: [(u32, u32, wgpu::TextureFormat, Vec<u8>); 4],
        params: crate::renderer::StageParams,
    ) -> Vec<u8> {
        draw_harness_probes(device, queue, wgsl, sets, maps, params, &[[[0; 4]; 6]], &[])
    }

    /// [`draw_harness_with`] with `t_probes` a cube array of 8 x 8 faces
    /// of one colour each, `probes[layer][face]` in D3D's face order (one
    /// texel would blend its neighbours across the seams), and the entity
    /// volume holding `blocks` from block 0 on; binding 7 the light grid
    /// weight cubes.
    #[allow(clippy::too_many_arguments)]
    fn draw_harness_probes(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wgsl: &str,
        sets: &[GpuLightSet],
        maps: [(u32, u32, wgpu::TextureFormat, Vec<u8>); 4],
        params: crate::renderer::StageParams,
        probes: &[[[u8; 4]; 6]],
        blocks: &[[[u8; 4]; 64]],
    ) -> Vec<u8> {
        use crate::entity_light::{COD4_VOLUME_BLOCKS, COD4_VOLUME_ROW, COD4_VOLUME_ROWS};
        use wgpu::util::DeviceExt;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cod2 model harness"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let set_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(sets),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: set_buf.as_entire_binding(),
        }];
        for b in 9..=16 {
            entries.push(wgpu::BindGroupEntry {
                binding: b,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        let g0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views: Vec<_> = maps
            .iter()
            .map(|(w, h, f, data)| {
                device
                    .create_texture_with_data(
                        queue,
                        &wgpu::TextureDescriptor {
                            label: None,
                            size: wgpu::Extent3d {
                                width: *w,
                                height: *h,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: *f,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::util::TextureDataOrder::LayerMajor,
                        data,
                    )
                    .create_view(&Default::default())
            })
            .collect();
        let cube = device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 8,
                        height: 8,
                        depth_or_array_layers: 6 * probes.len() as u32,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &probes
                    .as_flattened()
                    .iter()
                    .flat_map(|c| c.repeat(64))
                    .collect::<Vec<u8>>(),
            )
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                ..Default::default()
            });
        let mut full = blocks.to_vec();
        full.resize(COD4_VOLUME_BLOCKS as usize, [[0; 4]; 64]);
        let volume = device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 4 * COD4_VOLUME_ROW,
                        height: 4 * COD4_VOLUME_ROWS,
                        depth_or_array_layers: 4,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &crate::entity_light::cod4_volume_rows(&full).1,
            )
            .create_view(&Default::default());
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: params_buf.as_entire_binding(),
        }];
        let weights = crate::entity_material::grid_weight_cubes(device, queue);
        for (i, v) in views.iter().chain([&cube, &volume, &weights]).enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32 + 1,
                resource: wgpu::BindingResource::TextureView(v),
            });
        }
        let g3 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(3),
            entries: &entries,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(W * 4 * H),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = target.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &g0, &[]);
            pass.set_bind_group(3, &g3, &[]);
            pass.draw(0..3, 0..1);
        }
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
        readback.slice(..).get_mapped_range().unwrap().to_vec()
    }

    /// The harness's declarations: what dynamic_model.wgsl gives
    /// cod2_model.wgsl, and the image samplers.
    fn harness_wgsl(fs: &str) -> String {
        let dm = include_str!("dynamic_model.wgsl");
        format!(
            "{}{}\n@group(0) @binding(0) var<storage, read> light_sets: array<LightSet>;\n{}\n{}{}{}{}",
            cut(dm, "struct EntLight", "// The frame's light sets"),
            cut(
                dm,
                "// The stage's `renderer::StageParams`",
                "const F_AF_GT0"
            ),
            "@group(1) @binding(0) var t_unused: texture_2d<f32>;",
            concat!(
                include_str!("cod2_model.wgsl"),
                include_str!("cod4_sunshadow_none.wgsl")
            ),
            include_str!("tex_samplers.wgsl"),
            r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
"#,
            fs
        )
    }

    /// The tangent frame the harness gives a sphere normal: the tangent
    /// round the z axis, the binormal completing it, both unit.
    fn sphere_frame(n: Vec3) -> (Vec3, Vec3) {
        let t = Vec3::Z.cross(n).normalize_or(Vec3::X);
        (t, n.cross(t))
    }

    /// `$specularity` sampled bilinearly, clamped, at (u, v).
    fn specularity_at(map: &[u8], u: f32, v: f32) -> f32 {
        let (w, h) = (32usize, 256usize);
        let texel = |x: i32, y: i32| {
            let x = x.clamp(0, w as i32 - 1) as usize;
            let y = y.clamp(0, h as i32 - 1) as usize;
            f32::from(map[y * w + x]) / 255.0
        };
        let (x, y) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = texel(x0, y0) * (1.0 - fx) + texel(x0 + 1, y0) * fx;
        let bot = texel(x0, y0 + 1) * (1.0 - fx) + texel(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bot * fy
    }

    /// Two spheres through cod2_model.wgsl's `cod2_probe`, lit by one light
    /// set under a constant normal map texel and specular texel: the left
    /// with `_s`, the right without. Every pixel is `lprobe_s.hlsl`'s
    /// colour computed on the CPU: the normal `N + (2x - 1)T + (2y - 1)B`
    /// normalized, the weight cubes read bilinearly and the sun's `N.L` there,
    /// times the colour map times the vertex colour, plus `lerp(0.3, 1,
    /// vis) * lightSpecular0 * spec.rgb * $specularity(spec.a, N.H)`, with
    /// the eye along -y (lib/lightprobe.hlsl, lib/phong.hlsl,
    /// lib/normalmap.hlsl). With `COD2_DIR` the set is mp_carentan's sunlit
    /// street (as the sphere test), else a made-up sample with the sun
    /// overhead and to the side.
    #[test]
    fn probe_stage_lights_a_normal_mapped_sphere_as_the_hlsl_does() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let (sample, sun) = match vcod_common::testing::cod2_fs() {
            Some(fs) => {
                let data = fs.read("maps/mp/mp_carentan.d3dbsp").unwrap();
                let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
                let l = Cod2Lighting::new(&bsp).unwrap();
                (l.sample(Vec3::new(984.0, -736.0, 32.0)), l.sun)
            }
            None => (
                sample(),
                SunLight {
                    dir: Vec3::new(0.4, -0.5, 0.77).normalize().to_array(),
                    colour: [1.0, 0.9, 0.7],
                },
            ),
        };
        let set = probe_set(&sample, &sun, 1.0);
        // the normal map's .w (x) and .y, the spec map's rgb and exponent
        let (nx, ny) = (191u8, 89u8);
        let spec = [204u8, 153, 102, 140];
        let vcol = [255u8, 204, 153, 255];
        let spec_map = crate::renderer::specularity_map();
        let fs = format!(
            r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {{
    let h = 64.0;
    let half = u32(at.x / h);
    let u = (at.x - f32(half) * h) / h * 2.0 - 1.0;
    let v = 1.0 - at.y / h * 2.0;
    let r = u * u + v * v;
    let n = vec3<f32>(u, -sqrt(max(1.0 - r, 0.0)), v);
    var t = cross(vec3<f32>(0.0, 0.0, 1.0), n);
    t = select(vec3<f32>(1.0, 0.0, 0.0), normalize(t), dot(t, t) > 1e-12);
    let b = cross(n, t);
    let ls = light_sets[0];
    let half_angle = normalize(vec3<f32>(0.0, -1.0, 0.0) + ls.lights[1].diffuse.xyz);
    let flags = select(F_COD2_PROBE | F_COD2_SPEC, F_COD2_PROBE, half == 1u);
    let vcol = vec4<f32>({}, {}, {}, 1.0);
    let c = cod2_probe(ls, vec4<f32>(1.0), vcol, vec2<f32>(0.5), cod2_frame(n, t, b), half_angle, flags);
    if (r >= 0.9) {{
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }}
    return vec4<f32>(c.rgb, 1.0);
}}
"#,
            f32::from(vcol[0]) / 255.0,
            f32::from(vcol[1]) / 255.0,
            f32::from(vcol[2]) / 255.0,
        );
        use wgpu::TextureFormat as F;
        let px = draw_harness(
            &device,
            &queue,
            &harness_wgsl(&fs),
            &[set],
            [
                (1, 1, F::Rgba8Unorm, vec![0, ny, 0, nx]),
                (1, 1, F::Rgba8Unorm, spec.to_vec()),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (32, 256, F::R8Unorm, spec_map.clone()),
            ],
        );
        let l = Vec3::from_array(sun.dir);
        let h_vec = (Vec3::NEG_Y + l).normalize();
        let unorm = |b: u8| f32::from(b) / 255.0;
        let (x_n, y_n) = (unorm(nx), unorm(ny));
        let colour = Vec3::from_array(sun.colour);
        let cubes = lightgrid::weight_cubes();
        let (mut checked, mut spec_seen) = (0, 0.0f32);
        for y in 0..H {
            for x in 0..W {
                let Some(n) = sphere_normal(x, y) else {
                    continue;
                };
                // the rim and the pole, where the frame turns fast, are
                // left to the GPU's rounding
                if n.y > -0.2 || n.x.abs() + n.y.abs() < 0.05 {
                    continue;
                }
                let (t, b) = sphere_frame(n);
                let nm = (n - t - b + x_n * 2.0 * t + y_n * 2.0 * b).normalize();
                let light = sample.light(&lightgrid::sampled_entity_weights(&cubes, nm))
                    + colour * (sample.sun * nm.dot(l).max(0.0));
                let diffuse = Vec3::new(unorm(vcol[0]), unorm(vcol[1]), unorm(vcol[2]));
                let mut want = diffuse * light;
                if x < H {
                    let strength = specularity_at(&spec_map, unorm(spec[3]), nm.dot(h_vec));
                    let s = (0.3 + 0.7 * sample.sun)
                        * colour
                        * Vec3::new(unorm(spec[0]), unorm(spec[1]), unorm(spec[2]))
                        * strength;
                    spec_seen = spec_seen.max(s.max_element());
                    want += s;
                }
                let at = ((y * W + x) * 4) as usize;
                for k in 0..3 {
                    let w = (want[k].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + k]) - w).abs() <= 2.0,
                        "({x}, {y}) n {n} mapped {nm}: {:?} vs {want}",
                        &px[at..at + 3]
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 3000, "{checked}");
        // the highlight shows
        assert!(spec_seen > 0.05, "{spec_seen}");
        if let Ok(path) = std::env::var("VCOD_PROBE_STAGE_TGA") {
            image::save_buffer(&path, &px, W, H, image::ColorType::Rgba8).unwrap();
            eprintln!("wrote {path}");
        }
    }

    /// A sphere through cod2_model.wgsl's probe path under a CoD4 entity's
    /// set (`entity_light::pack_cod4`), whose `cod4_model` is
    /// `lp_sun_r0c0n0s0_sm3.hlsl` (cod4-world.md 3.2, 3.3): every pixel is
    /// the block read on the CPU as the volume's linear filter reads it,
    /// 1.5 texels from its centre along the normal mapped normal over its
    /// longest axis, twice its colour plus its alpha on the sun's `N.L`,
    /// times the vertex colour, plus the sun's glint and the set's
    /// reflection probe (layer 1 of two, its face along the reflected
    /// view, times its alpha) times the specular map and envMapParms'
    /// fresnel on the left half; the eye along -y. Every texel of the
    /// block and every face of the probe differs, so a misplaced word
    /// shows.
    #[test]
    fn cod4_entity_set_lights_a_normal_mapped_sphere_as_the_hlsl_does() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let texel = |x: usize, y: usize, z: usize| -> [u8; 4] {
            [
                (20 + 30 * x + 9 * y + 3 * z) as u8,
                (200 - 25 * y - 7 * z - 2 * x) as u8,
                (40 + 35 * z + 5 * x + 11 * y) as u8,
                180,
            ]
        };
        let block: [[u8; 4]; 64] = std::array::from_fn(|t| texel(t & 3, (t >> 2) & 3, t >> 4));
        let sun_dir = Vec3::new(0.3, -0.8, 0.5).normalize();
        let sun = Vec3::new(0.7, 0.6, 0.5);
        let light = vcod_common::mapworld::EntityLight {
            texels: block,
            probe: 1,
            primary: 1,
        };
        let set = crate::entity_light::pack_cod4(&light, 0, sun_dir, sun, None, 1.0);
        // layer 0 white, so a wrong layer shows; layer 1 a colour a face
        let faces: [[u8; 4]; 6] = std::array::from_fn(|f| {
            let f = f as u8;
            [10 + 30 * f, 120 - 15 * f, 60 + 20 * f, 255 - 30 * f]
        });
        let probes = [[[255; 4]; 6], faces];
        let env = [0.25f32, 0.9, 3.0, 1.5];
        let mut params: crate::renderer::StageParams = bytemuck::Zeroable::zeroed();
        params.eye_off = [1.0, 1.0, 0.0, 0.0];
        params.vec0_s = env;
        params.vec0_t = [1.0, 0.0, 0.0, 0.0];
        let (nx, ny) = (150u8, 112u8);
        let spec = [200u8, 180, 160, 140];
        let vcol = [255u8, 204, 153, 255];
        let fs = format!(
            r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {{
    let h = 64.0;
    let half = u32(at.x / h);
    let u = (at.x - f32(half) * h) / h * 2.0 - 1.0;
    let v = 1.0 - at.y / h * 2.0;
    let r = u * u + v * v;
    let n = vec3<f32>(u, -sqrt(max(1.0 - r, 0.0)), v);
    var t = cross(vec3<f32>(0.0, 0.0, 1.0), n);
    t = select(vec3<f32>(1.0, 0.0, 0.0), normalize(t), dot(t, t) > 1e-12);
    let b = cross(n, t);
    let flags = select(F_COD2_PROBE | F_COD2_SPEC, F_COD2_PROBE, half == 1u);
    let vcol = vec4<f32>({}, {}, {}, 1.0);
    let c = cod2_probe(light_sets[0], vec4<f32>(1.0), vcol, vec2<f32>(0.5), cod2_frame(n, t, b), vec3<f32>(0.0, 2.0, 0.0), flags);
    if (r >= 0.9) {{
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }}
    return vec4<f32>(c.rgb, 1.0);
}}
"#,
            f32::from(vcol[0]) / 255.0,
            f32::from(vcol[1]) / 255.0,
            f32::from(vcol[2]) / 255.0,
        );
        use wgpu::TextureFormat as F;
        let px = draw_harness_probes(
            &device,
            &queue,
            &harness_wgsl(&fs),
            &[set],
            [
                (1, 1, F::Rgba8Unorm, vec![0, ny, 0, nx]),
                (1, 1, F::Rgba8Unorm, spec.to_vec()),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (32, 256, F::R8Unorm, crate::renderer::specularity_map()),
            ],
            params,
            &probes,
            &[block],
        );
        let unorm = |b: u8| f32::from(b) / 255.0;
        let nt = (unorm(nx) * 4.08 - 2.08, unorm(ny) * 4.064_516 - 2.064_516);
        // the block's linear filter in texels, centres at i + 0.5, clamped
        // to the block (a lookup along an axis sits on its outer centres)
        let volume = |p: Vec3| -> [f32; 4] {
            let q = (p - 0.5).clamp(Vec3::ZERO, Vec3::splat(3.0));
            let i0 = q.floor().min(Vec3::splat(2.0));
            let f = q - i0;
            std::array::from_fn(|c| {
                let mut sum = 0.0;
                for (dx, dy, dz) in (0..8).map(|i| (i & 1, (i >> 1) & 1, i >> 2)) {
                    let w = [f.x, f.y, f.z]
                        .iter()
                        .zip([dx, dy, dz])
                        .map(|(&f, d)| if d == 1 { f } else { 1.0 - f })
                        .product::<f32>();
                    let at = |k: f32, d: usize| k as usize + d;
                    sum += w * unorm(texel(at(i0.x, dx), at(i0.y, dy), at(i0.z, dz))[c]);
                }
                sum
            })
        };
        let v = Vec3::Y;
        let (mut checked, mut spec_seen) = (0, 0.0f32);
        for y in 0..H {
            for x in 0..W {
                let Some(n) = sphere_normal(x, y) else {
                    continue;
                };
                if n.y > -0.2 || n.x.abs() + n.y.abs() < 0.05 {
                    continue;
                }
                let (t, b) = sphere_frame(n);
                let nm = (n + nt.0 * t + nt.1 * b).normalize();
                let ml = volume(Vec3::splat(2.0) + 1.5 * nm / nm.abs().max_element());
                let n_dot_l = nm.dot(sun_dir).clamp(0.0, 1.0);
                let vn = v.dot(nm);
                let r = v - 2.0 * vn * nm;
                let power = (unorm(spec[3]) * 9.377_518).exp2() + 7.0;
                let glint = ((r.dot(sun_dir) - 0.99925) * power).exp().clamp(0.0, 1.0);
                let fresnel = env[0] + (env[1] - env[0]) * (1.0 - vn.abs()).powf(env[2]);
                // the cube face along r: +x -x +y -y +z -z; skip its seams
                let ra = r.abs().to_array();
                let major = (0..3).max_by(|&a, &b| ra[a].total_cmp(&ra[b])).unwrap();
                if (0..3).any(|k| k != major && ra[major] - ra[k] < 0.3 * ra[major]) {
                    continue;
                }
                let face = faces[2 * major + usize::from(r[major] < 0.0)];
                let want: [f32; 3] = std::array::from_fn(|k| {
                    let light = 2.0 * ml[k] + ml[3] * sun[k] * n_dot_l;
                    let mut c = unorm(vcol[k]) * light;
                    if x < H {
                        let probe = unorm(face[k]) * unorm(face[3]);
                        let glint = ml[3] * glint * env[3] * sun[k];
                        let s = (glint + probe) * unorm(spec[k]) * fresnel;
                        spec_seen = spec_seen.max(s);
                        c += s;
                    }
                    c
                });
                let at = ((y * W + x) * 4) as usize;
                for k in 0..3 {
                    let w = (want[k].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + k]) - w).abs() <= 2.0,
                        "({x}, {y}) n {n} mapped {nm}: {:?} vs {want:?}",
                        &px[at..at + 3]
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 3000, "{checked}");
        assert!(spec_seen > 0.05, "{spec_seen}");
    }

    /// Two spheres of radius 50 at the origin through cod2_model.wgsl's
    /// probe path under CoD4 entity sets that settled on a spot light (the
    /// left, with the specular map) and an omni light (the right), each
    /// pixel `lp_spot_r0c0n0s0_sm3.hlsl` and `lp_omni_r0c0n0s0_sm3.hlsl`
    /// on the CPU (cod4-world.md 3.5): the light toward its origin from the
    /// surface, its colour by its attenuation row read linearly at the
    /// distance over its radius, a spot's by its cone factors, all by the
    /// block's alpha, on `N.L` and the glint off the reflected view, plus
    /// twice the block; the eye 200 down -y. Flat normal map, black probe.
    #[test]
    fn cod4_spot_and_omni_light_a_sphere_as_the_hlsl_does() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        use vcod_common::mapworld::{EntityLight, ModelLightSource};
        let block: [[u8; 4]; 64] = [[30, 40, 50, 200]; 64];
        let light = EntityLight {
            texels: block,
            probe: 0,
            primary: 2,
        };
        let row: Vec<[u8; 4]> = (0..32u8)
            .map(|i| [255 - 7 * i, 250 - 6 * i, 240 - 5 * i, 255])
            .collect();
        let origin = [60.0f32, -90.0, 40.0];
        let target = Vec3::new(0.0, -50.0, 0.0);
        let dir = (Vec3::from(origin) - target).normalize().to_array();
        let (outer, inner) = (0.8f32, 0.95f32);
        let scale = 1.0 / (inner - outer);
        let spot = ModelLightSource {
            spot: true,
            can_shadow: true,
            origin,
            inv_radius: 1.0 / 180.0,
            colour: [0.9, 0.7, 0.5],
            dir,
            spot_factors: [scale, -scale * outer, 2.0],
            attenuation: row.clone(),
        };
        let omni = ModelLightSource {
            spot: false,
            can_shadow: true,
            ..spot.clone()
        };
        let sun = (Vec3::Z, Vec3::splat(5.0));
        let sets = [
            crate::entity_light::pack_cod4(&light, 0, sun.0, sun.1, Some(&spot), 1.0),
            crate::entity_light::pack_cod4(&light, 0, sun.0, sun.1, Some(&omni), 1.0),
        ];
        let env = [0.25f32, 0.9, 3.0, 1.5];
        let mut params: crate::renderer::StageParams = bytemuck::Zeroable::zeroed();
        params.eye_off = [1.0, 1.0, 0.0, 0.0];
        params.vec0_s = env;
        params.vec0_t = [1.0, 0.0, 0.0, 0.0];
        let spec = [200u8, 180, 160, 140];
        let fs = r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let h = 64.0;
    let half = u32(at.x / h);
    let u = (at.x - f32(half) * h) / h * 2.0 - 1.0;
    let v = 1.0 - at.y / h * 2.0;
    let r = u * u + v * v;
    let n = vec3<f32>(u, -sqrt(max(1.0 - r, 0.0)), v);
    var t = cross(vec3<f32>(0.0, 0.0, 1.0), n);
    t = select(vec3<f32>(1.0, 0.0, 0.0), normalize(t), dot(t, t) > 1e-12);
    let b = cross(n, t);
    let flags = select(F_COD2_PROBE | F_COD2_SPEC, F_COD2_PROBE, half == 1u);
    let eye = vec3<f32>(0.0, -200.0, 0.0);
    let c = cod2_probe_sided(light_sets[half], vec4<f32>(1.0), vec4<f32>(1.0), vec2<f32>(0.5), cod2_frame(n, t, b), 50.0 * n - eye, flags, false, 1.0, eye);
    if (r >= 0.9) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return vec4<f32>(c.rgb, 1.0);
}
"#;
        use wgpu::TextureFormat as F;
        let px = draw_harness_probes(
            &device,
            &queue,
            &harness_wgsl(fs),
            &sets,
            [
                (1, 1, F::Rgba8Unorm, vec![0, 128, 0, 128]),
                (1, 1, F::Rgba8Unorm, spec.to_vec()),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (32, 256, F::R8Unorm, crate::renderer::specularity_map()),
            ],
            params,
            &[[[0; 4]; 6]],
            &[block],
        );
        let unorm = |b: u8| f32::from(b) / 255.0;
        let eye = Vec3::new(0.0, -200.0, 0.0);
        let (mut checked, mut lit_spot, mut lit_omni) = (0, 0, 0);
        for y in 0..H {
            for x in 0..W {
                let Some(n) = sphere_normal(x, y) else {
                    continue;
                };
                let left = x < H;
                let l = if left { &spot } else { &omni };
                let world = 50.0 * n;
                // the flat map's (0.5, 0.5) decodes to a hair off zero
                let (t, b) = sphere_frame(n);
                let nt = (unorm(128) * 4.08 - 2.08, unorm(128) * 4.064_516 - 2.064_516);
                let nm = (n + nt.0 * t + nt.1 * b).normalize();
                let lv = Vec3::from(l.origin) - world;
                let ld = lv.normalize();
                let u = (lv.length() * l.inv_radius).clamp(0.0, 1.0);
                let xa = (u * 32.0 - 0.5).clamp(0.0, 31.0);
                let (i0, f) = (xa.floor() as usize, xa.fract());
                let i1 = (i0 + 1).min(31);
                let mut att: [f32; 3] =
                    std::array::from_fn(|k| unorm(row[i0][k]) * (1.0 - f) + unorm(row[i1][k]) * f);
                if l.spot {
                    let s = (ld.dot(Vec3::from(l.dir)) * l.spot_factors[0] + l.spot_factors[1])
                        .clamp(0.0, 1.0);
                    let sf = if s <= 0.0 {
                        0.0
                    } else {
                        s.powf(l.spot_factors[2])
                    };
                    att = att.map(|a| a * sf);
                }
                let vis = unorm(200);
                let n_dot_l = nm.dot(ld).clamp(0.0, 1.0);
                let v = (world - eye).normalize();
                let vn = v.dot(nm);
                let r = v - 2.0 * vn * nm;
                let power = (unorm(spec[3]) * 9.377_518).exp2() + 7.0;
                let glint = ((r.dot(ld) - 0.99925) * power).exp().clamp(0.0, 1.0);
                let fresnel = env[0] + (env[1] - env[0]) * (1.0 - vn.abs()).powf(env[2]);
                let want: [f32; 3] = std::array::from_fn(|k| {
                    let col = vis * att[k] * l.colour[k];
                    let mut c = 2.0 * unorm(block[0][k]) + col * n_dot_l;
                    if left {
                        c += col * glint * env[3] * unorm(spec[k]) * fresnel;
                    }
                    c
                });
                let at = ((y * W + x) * 4) as usize;
                for k in 0..3 {
                    let w = (want[k].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + k]) - w).abs() <= 2.0,
                        "({x}, {y}) n {n}: {:?} vs {want:?}",
                        &px[at..at + 3]
                    );
                }
                let base = 2.0 * unorm(block[0][0]);
                if want[0] > base + 0.05 {
                    if left {
                        lit_spot += 1;
                    } else {
                        lit_omni += 1;
                    }
                }
                checked += 1;
            }
        }
        assert!(checked > 5000, "{checked}");
        // the cone lights part of its sphere, the omni more of its own
        assert!(
            lit_spot > 100 && lit_omni > lit_spot,
            "{lit_spot} {lit_omni}"
        );
    }

    /// cod2_model.wgsl's `objective_colour` against `objective_base.hlsl`
    /// on the CPU: the facing is the view-space normal's z toward the eye,
    /// here the world normal against the view forward +y, at a clock of
    /// 0.3 s, over a colour map texel of (0.6, 0.4, 0.2, 0.7).
    #[test]
    fn objective_pulse_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let fs = r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let h = 64.0;
    let u = (at.x % h) / h * 2.0 - 1.0;
    let v = 1.0 - at.y / h * 2.0;
    let r = u * u + v * v;
    let n = vec3<f32>(u, -sqrt(max(1.0 - r, 0.0)), v);
    let c = objective_colour(vec4<f32>(0.6, 0.4, 0.2, 0.7), n, vec3<f32>(0.0, 1.0, 0.0), 0.3 + f32(u32(at.x / h)));
    // binds what the harness binds
    let unused = cod2_probe(light_sets[0], vec4<f32>(1.0), vec4<f32>(1.0), vec2<f32>(0.5), cod2_frame(n, n, n), n, 0u);
    if (r >= 0.9) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return c + unused * 0.0;
}
"#;
        use wgpu::TextureFormat as F;
        let px = draw_harness(
            &device,
            &queue,
            &harness_wgsl(fs),
            &[GpuLightSet::default()],
            [
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::Rgba8Unorm, vec![0; 4]),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::R8Unorm, vec![0]),
            ],
        );
        let lum = 0.333 * (0.6 + 0.4 + 0.2);
        let (lo, hi) = (Vec3::new(0.25, 0.15, 0.0), Vec3::new(1.0, 1.0, 0.5));
        let mut checked = 0;
        let (mut darkest, mut brightest) = (f32::MAX, 0.0f32);
        for y in 0..H {
            for x in 0..W {
                let Some(n) = sphere_normal(x, y) else {
                    continue;
                };
                let facing = -n.y;
                // the clock's fraction is 0.3 on both halves
                // the HLSL's own literal for pi
                #[allow(clippy::approx_constant)]
                let k = 0.5 - ((facing * 0.5 + 0.3) * 3.14159 * 2.0).sin() * 0.5;
                let want = (lo + (hi - lo) * k) * (lum + 0.5);
                let at = ((y * W + x) * 4) as usize;
                for c in 0..3 {
                    let w = (want[c].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + c]) - w).abs() <= 2.0,
                        "({x}, {y}) facing {facing}: {:?} vs {want}",
                        &px[at..at + 3]
                    );
                }
                assert!(px[at + 3].abs_diff(179) <= 1, "alpha {}", px[at + 3]);
                darkest = darkest.min(want.x);
                brightest = brightest.max(want.x);
                checked += 1;
            }
        }
        assert!(checked > 3000, "{checked}");
        // the pulse spans the face
        assert!(brightest - darkest > 0.3, "{darkest} {brightest}");
    }

    /// `lib/flag.hlsl`'s `Flap` on the CPU, its macros expanded: the
    /// offset along the normal and the new normal's tangent, binormal and
    /// normal parts for texcoords `uv`, vertex alpha `a`, at `time` s.
    #[allow(clippy::approx_constant)]
    fn hlsl_flap(uv: [f32; 2], a: f32, time: f32) -> (f32, [f32; 3]) {
        const PI: f32 = 3.141_592_7; // the HLSL's 3.14159265 as an f32
        let (u, v) = (1.0 - uv[0], 1.0 - uv[1]);
        let waves = [
            (1.73f32, 4.0f32, 0.9f32, 1.5f32),
            (0.75, -10.0, 1.5, 0.6),
            (0.31, 15.0, 1.2, 0.34),
        ];
        let (mut off, mut nu, mut nv) = (0.0f32, 0.0f32, 0.0f32);
        for (scale, deg, speed, amp) in waves {
            let ang = deg * (PI / 180.0);
            let su = 2.0 * PI / scale * ang.cos();
            let sv = 2.0 * PI / scale * ang.sin();
            let w = su * u + sv * v + 2.0 * PI * time * speed;
            off += amp * w.sin();
            nu += amp * su * w.cos();
            nv += amp * sv * w.cos();
        }
        let nu = (nu / (2.0 * PI)).atan() * a;
        let nv = (nv / (2.0 * PI)).atan() * a;
        // `sqrt` compiles to RSQ and RCP, which take the size
        (
            5.0 * a * off,
            [nu, nv, (1.0 - (nu * nu + nv * nv)).abs().sqrt()],
        )
    }

    /// cod2_model.wgsl's `cod2_flap` against [`hlsl_flap`] over a grid of
    /// texcoords and vertex alphas, its colour swizzle, and the flag
    /// techniques' two-sided sun (cod2-materials.md 9.7). Vulkan or Metal
    /// only.
    #[test]
    fn flag_flap_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let fs = r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(at.x / 128.0 * 3.0 - 1.0, at.y / 32.0);
    let a = select(1.0, 0.4, at.y >= 32.0);
    let t = vec3<f32>(1.0, 0.0, 0.0);
    let b = vec3<f32>(0.0, 1.0, 0.0);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let f = cod2_flap(vec3<f32>(0.0), n, t, b, uv, vec4<f32>(0.2, 0.4, 0.6, a), 7.37);
    // binds what the harness binds, and the two-sided sun from below
    var ls = light_sets[0];
    ls.lights[1].diffuse = vec4<f32>(0.0, 0.0, -1.0, 0.0);
    let below = probe_lighting_sided(ls, n, true, 1.0) - probe_lighting(ls, n);
    let unused = cod2_probe(light_sets[0], vec4<f32>(1.0), vec4<f32>(1.0), vec2<f32>(0.5), cod2_frame(n, n, n), n, 0u);
    if (at.x < 1.0 && at.y < 1.0) {
        return vec4<f32>(f.vcol.rgb, f.vcol.a * 0.5) + vec4<f32>(below, 0.0) + unused * 0.0;
    }
    return vec4<f32>(f.pos.z / 32.0 + 0.5, f.normal.x * 0.5 + 0.5, f.normal.y * 0.5 + 0.5, f.normal.z);
}
"#;
        use wgpu::TextureFormat as F;
        // the sun's colour 0.25 grey at full visibility, the grid black
        let mut set = GpuLightSet {
            ambient: [0.25, 0.25, 0.25, PROBE_SET],
            ..GpuLightSet::default()
        };
        set.lights[1].ambient = [0.0, 0.0, 0.0, 1.0];
        let px = draw_harness(
            &device,
            &queue,
            &harness_wgsl(fs),
            &[set],
            [
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::Rgba8Unorm, vec![0; 4]),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::R8Unorm, vec![0]),
            ],
        );
        // the colour hands on red, blue, green, alpha 1; the sun from
        // straight below lights the face by its full colour
        for (c, want) in [115u8, 217, 166, 128].into_iter().enumerate() {
            assert!(px[c].abs_diff(want) <= 1, "{:?}", &px[0..4]);
        }
        let (mut checked, mut lo, mut hi) = (0, f32::MAX, f32::MIN);
        for y in 0..H {
            for x in 0..W {
                if x == 0 && y == 0 {
                    continue;
                }
                let uv = [
                    (x as f32 + 0.5) / 128.0 * 3.0 - 1.0,
                    (y as f32 + 0.5) / 32.0,
                ];
                let a = if y >= 32 { 0.4 } else { 1.0 };
                let (off, n) = hlsl_flap(uv, a, 7.37);
                let want = [off / 32.0 + 0.5, n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2]];
                let at = ((y * W + x) * 4) as usize;
                for c in 0..4 {
                    let w = (want[c].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + c]) - w).abs() <= 1.0,
                        "({x}, {y}) {c}: {:?} vs {want:?}",
                        &px[at..at + 4]
                    );
                }
                (lo, hi) = (lo.min(off), hi.max(off));
                checked += 1;
            }
        }
        assert!(checked > 8000, "{checked}");
        // the waves move the cloth by units, not fractions
        assert!(hi - lo > 10.0, "{lo} {hi}");
    }

    /// `lp_flag_sun_s_tc0n0_dtex_sm3.hlsl`'s waves (cod4-world.md 3.4),
    /// read off the bytecode in its own order: the phases of (1 - u,
    /// 1 - v) and gameTime.w wrapped by `frc` into [-pi, pi), their sines'
    /// push along the normal and cosines' slopes times the vertex alpha,
    /// the slopes times 0.025 along the tangent and cross(N, T) times the
    /// handedness. Returns the push and the tilted normal (unnormalized).
    fn hlsl_cod4_wave(uv: [f32; 2], a: f32, hand: f32, time: f32) -> (f32, [f32; 3]) {
        use std::f32::consts::{FRAC_1_PI, PI, TAU};
        let (fu, fv) = (1.0 - uv[0], 1.0 - uv[1]);
        let c19 = [3.623_052, 0.253_348_47, 8.250_306, -1.454_751_6];
        let x = fv * c19[1] + fu * c19[0] + time * 5.654_866_7;
        let y = fv * c19[3] + fu * c19[2] + time * 9.424_778;
        let z = fu * 19.577_713 + fv * 5.245_832_4 + time * 7.539_822_6;
        let wrap = |p: f32| (p * FRAC_1_PI / 2.0 + 0.5).fract() * TAU - PI;
        let (sa, ca) = wrap(x).sin_cos();
        let (sb, cb) = wrap(y).sin_cos();
        let (sc, cc) = wrap(z).sin_cos();
        let push = (sb * 3.0 + sa * 7.5 + sc * 1.7) * a;
        let st = (cb * 24.750_92 + ca * 27.172_89 + cc * 33.282_11) * a * 0.025;
        let sbn = (cb * -4.364_254_5 + ca * 1.900_113_5 + cc * 8.917_915) * a * 0.025;
        // N (0, 0, 1), T (1, 0, 0): cross(N, T) is +y
        (push, [st, sbn * hand, 1.0])
    }

    /// cod4_flag.wgsl's `cod4_flag_wave` against [`hlsl_cod4_wave`] over
    /// a grid of texcoords, two vertex alphas and both handednesses: the
    /// push, the tilted normal and the binormal handed on, cross(tilted N,
    /// T) times the handedness. Vulkan or Metal only.
    #[test]
    fn cod4_flag_wave_matches_the_hlsl() {
        let Some((device, queue)) = headless() else {
            eprintln!("no adapter; skipped");
            return;
        };
        let fs = format!(
            "{}{}",
            include_str!("cod4_flag.wgsl"),
            r#"
@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(at.x / 128.0 * 3.0 - 1.0, (at.y % 32.0) / 32.0);
    let a = select(1.0, 0.4, at.x >= 64.0);
    let hand = select(1.0, -1.0, at.y >= 32.0);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let f = cod4_flag_wave(vec3<f32>(0.0), n, vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, hand, 0.0), uv, a, 1234.37);
    // binds what the harness binds
    let unused = cod2_probe(light_sets[0], vec4<f32>(1.0), vec4<f32>(1.0), vec2<f32>(0.5), cod2_frame(n, n, n), n, 0u);
    let bz = f.binormal.z * hand;
    return vec4<f32>(f.pos.z / 32.0 + 0.5, f.normal.x * 0.2 + 0.5, f.normal.y * 0.2 + 0.5, f.binormal.y * hand * 0.25 + bz * 0.2 + 0.5) + unused * 0.0;
}
"#
        );
        use wgpu::TextureFormat as F;
        let px = draw_harness(
            &device,
            &queue,
            &harness_wgsl(&fs),
            &[GpuLightSet::default()],
            [
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::Rgba8Unorm, vec![0; 4]),
                (1, 1, F::Rgba8Unorm, vec![128; 4]),
                (1, 1, F::R8Unorm, vec![0]),
            ],
        );
        let (mut checked, mut lo, mut hi) = (0, f32::MAX, f32::MIN);
        for y in 0..H {
            for x in 0..W {
                let uv = [
                    (x as f32 + 0.5) / 128.0 * 3.0 - 1.0,
                    ((y % 32) as f32 + 0.5) / 32.0,
                ];
                let a = if x >= 64 { 0.4 } else { 1.0 };
                let hand = if y >= 32 { -1.0 } else { 1.0 };
                let (push, n) = hlsl_cod4_wave(uv, a, hand, 1234.37);
                // cross(n, T) = (0, n.z, -n.y), times the handedness
                let (by, bz) = (n[2] * hand, -n[1] * hand);
                let want = [
                    push / 32.0 + 0.5,
                    n[0] * 0.2 + 0.5,
                    n[1] * 0.2 + 0.5,
                    by * hand * 0.25 + bz * hand * 0.2 + 0.5,
                ];
                let at = ((y * W + x) * 4) as usize;
                for c in 0..4 {
                    let w = (want[c].clamp(0.0, 1.0) * 255.0).round();
                    assert!(
                        (f32::from(px[at + c]) - w).abs() <= 1.0,
                        "({x}, {y}) {c}: {:?} vs {want:?}",
                        &px[at..at + 4]
                    );
                }
                (lo, hi) = (lo.min(push), hi.max(push));
                checked += 1;
            }
        }
        assert!(checked > 8000, "{checked}");
        assert!(hi - lo > 10.0, "{lo} {hi}");
    }

    /// [`headless`] with BC textures, which CoD2's colour maps are.
    fn headless_bc() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let bc = wgpu::Features::TEXTURE_COMPRESSION_BC;
        if adapter.get_info().backend == wgpu::Backend::Gl || !adapter.features().contains(bc) {
            return None;
        }
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: bc,
            ..Default::default()
        }))
        .ok()
    }
}
