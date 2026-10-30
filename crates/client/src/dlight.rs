//! The world's dynamic-light pass, the CPU half: which soups a light
//! reaches, the per-vertex tangent axis the projection needs, and the
//! per-light uniform. The projection itself is `vs_dlight` in shader.wgsl
//! (CoDMP.exe 0x4ff690; docs/research/cod11-shader-scripts.md, section 3).

use crate::renderer::{DEPTH_FORMAT, msaa_samples, upload_image};
use glam::Vec3;
use vcod_common::assets;
use vcod_common::mapworld::{DrawVert, MapWorld, TriangleSoup};
use vcod_common::material::{BlendFactor, DLIGHT_IMAGE, DlightDraw, Material, Stage};
use vcod_common::static_light::SceneLight;

/// `RE_AddLightToScene`'s queue (0x4e9b00), which is also the dlight
/// mask's width.
pub const MAX_DLIGHTS: usize = 32;

/// One light as the dlight pass reads it: the record's origin, its
/// intensity (`+0x74`, the projection radius) and its raw colour (`+0x04`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dlight {
    pub origin: Vec3,
    pub radius: f32,
    pub color: Vec3,
}

/// The frame's scene lights in queue order, at most [`MAX_DLIGHTS`]; a
/// light at or below zero intensity was never queued (0x4e9b00).
pub fn from_scene(lights: &[SceneLight]) -> Vec<Dlight> {
    lights
        .iter()
        .filter(|l| l.intensity > 0.0)
        .take(MAX_DLIGHTS)
        .map(|l| Dlight {
            origin: l.origin,
            radius: l.intensity,
            color: l.color,
        })
        .collect()
}

/// Whether `light` can touch a box: the brush model test at 0x4b59f0,
/// which keeps a light whose origin is within its radius of the box on
/// every axis. The world's node walk is assumed to cut the same way.
pub fn reaches(light: &Dlight, origin: Vec3, min: Vec3, max: Vec3) -> bool {
    let r = light.radius;
    (0..3).all(|i| origin[i] - max[i] <= r && min[i] - origin[i] <= r)
}

/// Per vertex of `bsp.verts`, the s axis of the tangent frame the loader
/// builds for every soup (0x4da2e0 calling 0x515680): each triangle's
/// direction of constant t, `(t1 - t2) P0 + (t2 - t0) P1 + (t0 - t1) P2`
/// normalized (0x5155d0), summed into its three vertices, and the sum
/// normalized where more than one triangle met. The t axis is `N x S`,
/// normalized, which the vertex shader rebuilds.
pub fn soup_s_axes(soups: &[TriangleSoup], verts: &[DrawVert], indices: &[u16]) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; verts.len()];
    let mut count = vec![0u32; verts.len()];
    for soup in soups {
        let fv = soup.first_vertex as usize;
        let fi = soup.first_index as usize;
        let Some(idx) = indices.get(fi..fi + soup.index_count as usize) else {
            continue;
        };
        for tri in idx.as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|k| fv + tri[k] as usize);
            if [a, b, c].iter().any(|&v| v >= verts.len()) {
                continue;
            }
            let p = |v: usize| Vec3::from_array(verts[v].pos);
            let t = |v: usize| verts[v].uv[1];
            let dir = ((t(b) - t(c)) * p(a) + (t(c) - t(a)) * p(b) + (t(a) - t(b)) * p(c))
                .normalize_or_zero();
            for v in [a, b, c] {
                out[v] = (Vec3::from_array(out[v]) + dir).to_array();
                count[v] += 1;
            }
        }
    }
    for (s, &n) in out.iter_mut().zip(&count) {
        if n > 1 {
            *s = Vec3::from_array(*s).normalize_or_zero().to_array();
        }
    }
    out
}

/// Per soup, the bounds of its vertices (spawn pose).
pub fn soup_bounds(bsp: &MapWorld) -> Vec<(Vec3, Vec3)> {
    (0..bsp.soups.len())
        .map(|s| vcod_common::vis::soup_bounds(bsp, s))
        .collect()
}

/// The single-stage shaders the pass draws over a material's surfaces
/// (`Material::dlight`), each keeping the material's deform and
/// culling. Fog is the drawn stage's own state bit (0x200000, set at
/// 0x4fbc71 unless the shader is `nofog`): a `perLight` stage or the copy
/// keeps the material's, `dlightshader`'s blob takes that shader's
/// `nofog` (shader doc 3).
pub fn stage_shaders(material: &Material, blob: Option<&Material>) -> Vec<Material> {
    let single = |st: Stage, nofog: bool| Material {
        nofog,
        ..material.with_stages(vec![st])
    };
    match &material.dlight {
        None => Vec::new(),
        Some(DlightDraw::Stages(st)) => st
            .iter()
            .map(|st| single(st.clone(), material.nofog))
            .collect(),
        Some(DlightDraw::Blob) => blob
            .and_then(|b| Some(single(b.stages.first()?.clone(), b.nofog)))
            .into_iter()
            .collect(),
    }
}

/// `Dlight` in shader.wgsl, one dynamic-offset slot per light.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DlightUniform {
    pub origin_radius: [f32; 4],
    pub color: [f32; 4],
}

impl DlightUniform {
    /// `quality` is `r_dlightQuality`, carried in `color.w`.
    pub fn of(l: &Dlight, quality: bool) -> Self {
        Self {
            origin_radius: [l.origin.x, l.origin.y, l.origin.z, l.radius],
            color: [
                l.color.x,
                l.color.y,
                l.color.z,
                f32::from(u8::from(quality)),
            ],
        }
    }
}

/// The pass's blends (shader doc 3): the `perLight` stages and the
/// automatic copy add onto `GL_ONE`, `dlightshader` is `GL_DST_COLOR
/// GL_ONE` under `depthFunc equal`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DlightBlend {
    One,
    SrcAlpha,
    DstColor,
}

impl DlightBlend {
    pub fn of(st: &Stage) -> Option<Self> {
        match st.blend {
            Some((BlendFactor::One, BlendFactor::One)) => Some(Self::One),
            Some((BlendFactor::SrcAlpha, BlendFactor::One)) => Some(Self::SrcAlpha),
            Some((BlendFactor::DstColor, BlendFactor::One)) => Some(Self::DstColor),
            _ => None,
        }
    }
}

const UNIFORM_SIZE: u64 = std::mem::size_of::<DlightUniform>() as u64;

/// The pass's device state: `vs_dlight`/`fs_dlight` pipelines over the
/// stage pipelines' groups 0..2 plus group 3, the light slots (one per
/// dynamic offset) and the dlight image. Nothing writes depth; the blob's
/// `depthFunc equal` relies on `@invariant` positions.
pub struct DlightPass {
    /// `[blend][two_sided][bias]`
    pipelines: [[[wgpu::RenderPipeline; 2]; 2]; 3],
    pub buf: wgpu::Buffer,
    pub bg: wgpu::BindGroup,
    pub stride: u64,
}

impl DlightPass {
    /// `groups` are the camera, material and stage layouts; `clamp` a
    /// clamping sampler, `bias` the decal depth bias.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shader: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        clamp: &wgpu::Sampler,
        bias: wgpu::DepthBiasState,
        format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dlight layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = UNIFORM_SIZE.div_ceil(align) * align;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dlight uniform"),
            size: stride * MAX_DLIGHTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blob = upload_image(device, queue, DLIGHT_IMAGE, &assets::dlight_blob());
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dlight bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&blob),
                },
                // the image's black rim covers whatever lies off it
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(clamp),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dlight pipeline layout"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let onto_one = |src_factor, src_alpha| wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: src_alpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let blends = [
            DlightBlend::One,
            DlightBlend::SrcAlpha,
            DlightBlend::DstColor,
        ];
        let mut built = crate::renderer::par_build(12, |i| {
            let blend = blends[i / 4];
            let (two_sided, want_bias) = (i & 2 != 0, i & 1 != 0);
            let (state, compare) = match blend {
                DlightBlend::One => (
                    onto_one(wgpu::BlendFactor::One, wgpu::BlendFactor::One),
                    wgpu::CompareFunction::LessEqual,
                ),
                DlightBlend::SrcAlpha => (
                    onto_one(wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::SrcAlpha),
                    wgpu::CompareFunction::LessEqual,
                ),
                DlightBlend::DstColor => (
                    onto_one(wgpu::BlendFactor::Dst, wgpu::BlendFactor::DstAlpha),
                    wgpu::CompareFunction::Equal,
                ),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&format!(
                    "dlight {blend:?} twosided={two_sided} bias={want_bias}"
                )),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_dlight"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 44,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x3,
                                1 => Float32x2,
                                2 => Float32x2,
                                3 => Float32x3,
                                4 => Unorm8x4,
                            ],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 12,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![5 => Float32x3],
                        }),
                    ],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_dlight"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(state),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: (!two_sided).then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: if want_bias { bias } else { Default::default() },
                }),
                multisample: wgpu::MultisampleState {
                    count: msaa_samples(),
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        })
        .into_iter();
        let pipelines = std::array::from_fn(|_| {
            std::array::from_fn(|_| std::array::from_fn(|_| built.next().unwrap()))
        });
        Self {
            pipelines,
            buf,
            bg,
            stride,
        }
    }

    pub fn pipeline(
        &self,
        blend: DlightBlend,
        two_sided: bool,
        bias: bool,
    ) -> &wgpu::RenderPipeline {
        &self.pipelines[blend as usize][usize::from(two_sided)][usize::from(bias)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vert(pos: [f32; 3], uv: [f32; 2]) -> DrawVert {
        DrawVert {
            pos,
            uv,
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        }
    }

    fn s_axes(verts: Vec<DrawVert>, indices: Vec<u16>) -> Vec<[f32; 3]> {
        let soup = TriangleSoup {
            material: 0,
            lightmap: 0,
            first_vertex: 0,
            vertex_count: verts.len() as u16,
            index_count: indices.len() as u16,
            first_index: 0,
        };
        soup_s_axes(&[soup], &verts, &indices)
    }

    /// A floor quad textured with s along x and t along y: the axis of
    /// constant t is x, summed over both triangles and normalized; a
    /// vertex in one triangle keeps that triangle's axis as is.
    #[test]
    fn s_axis_runs_along_constant_t() {
        let s = s_axes(
            vec![
                vert([0.0, 0.0, 0.0], [0.0, 0.0]),
                vert([64.0, 0.0, 0.0], [1.0, 0.0]),
                vert([64.0, 64.0, 0.0], [1.0, 1.0]),
                vert([0.0, 64.0, 0.0], [0.0, 1.0]),
            ],
            vec![0, 1, 2, 0, 2, 3],
        );
        for (i, axis) in s.iter().enumerate() {
            let a = Vec3::from_array(*axis);
            assert!((a.abs() - Vec3::X).length() < 1e-6, "vertex {i}: {a}");
        }
        // the two triangles agree in sign, so the shared corner stays unit
        assert!((Vec3::from_array(s[0]).length() - 1.0).abs() < 1e-6);
    }

    /// A triangle whose t never changes has no axis: retail's normalize
    /// leaves the zero vector, so the projection reads s = 0.5 there.
    #[test]
    fn a_flat_t_triangle_has_no_axis() {
        let s = s_axes(
            vec![
                vert([0.0, 0.0, 0.0], [0.0, 0.5]),
                vert([64.0, 0.0, 0.0], [1.0, 0.5]),
                vert([64.0, 64.0, 0.0], [1.0, 0.5]),
            ],
            vec![0, 1, 2],
        );
        assert!(s.iter().all(|a| *a == [0.0; 3]));
    }

    #[test]
    fn a_light_reaches_a_box_within_its_radius_per_axis() {
        let l = Dlight {
            origin: Vec3::ZERO,
            radius: 100.0,
            color: Vec3::ONE,
        };
        let (min, max) = (Vec3::new(50.0, -10.0, -10.0), Vec3::new(80.0, 10.0, 10.0));
        assert!(reaches(&l, l.origin, min, max));
        assert!(reaches(
            &l,
            l.origin,
            min + Vec3::X * 50.0,
            max + Vec3::X * 50.0
        ));
        assert!(!reaches(
            &l,
            l.origin,
            min + Vec3::X * 51.0,
            max + Vec3::X * 51.0
        ));
        // the corner of a cube, not a sphere
        let corner = Vec3::splat(99.0);
        assert!(reaches(&l, l.origin, corner, corner + 1.0));
    }

    fn shader(text: &str) -> Material {
        use vcod_common::shader::{WarnSet, parse_shader, split_blocks};
        let (name, body) = split_blocks(text).into_iter().next().expect("a block");
        Material::from_shader(&parse_shader(&name, &body, &mut WarnSet::new()))
    }

    /// The copy and `perLight` stages fog as their material does; the blob
    /// is `dlightshader`'s stage under that shader's `nofog`, whatever the
    /// material says.
    #[test]
    fn a_dlight_stage_fogs_by_its_own_shader() {
        let blob = shader(
            "dlightshader { nofog { map $dlight rgbGen exactVertex \
             blendFunc GL_DST_COLOR GL_ONE texGen lightmap depthFunc equal } }",
        );
        let copy = "{ map $lightmap rgbGen identity } \
                    { map a.tga blendFunc filter rgbGen identity }";
        let fogged = stage_shaders(&shader(&format!("t {{ {copy} }}")), Some(&blob));
        assert_eq!(fogged.len(), 1);
        assert!(!fogged[0].nofog);
        assert_eq!(
            fogged[0].stages[0].bundles[1].image,
            vcod_common::material::ImageRef::Path(DLIGHT_IMAGE.into())
        );
        let unfogged = stage_shaders(&shader(&format!("t {{ nofog {copy} }}")), Some(&blob));
        assert!(unfogged[0].nofog);
        // three stages: no copy, so the blob, unfogged on a fogged material
        let three = shader(
            "t { { map $lightmap } { map a.tga blendFunc filter } { map g.tga blendFunc add } }",
        );
        let b = stage_shaders(&three, Some(&blob));
        assert_eq!(b.len(), 1);
        assert!(b[0].nofog && !three.nofog);
        assert_eq!(b[0].stages[0].blend, blob.stages[0].blend);
        assert!(stage_shaders(&three, None).is_empty());
        // a perLight stage keeps the material's fog
        let per = shader("t { { map a.tga } { map b.tga blendFunc GL_ONE GL_ONE perLight } }");
        let p = stage_shaders(&per, Some(&blob));
        assert_eq!(p.len(), 1);
        assert!(!p[0].nofog);
        assert!(
            stage_shaders(&shader("t { surfaceparm sky { map a.tga } }"), Some(&blob)).is_empty()
        );
    }

    #[test]
    fn unqueued_and_surplus_lights_drop() {
        let light = |intensity| SceneLight {
            origin: Vec3::ZERO,
            color: Vec3::ONE,
            intensity,
        };
        let mut lights = vec![light(0.0), light(-5.0)];
        lights.extend((0..40).map(|_| light(200.0)));
        let got = from_scene(&lights);
        assert_eq!(got.len(), MAX_DLIGHTS);
        assert!(got.iter().all(|l| l.radius == 200.0));
    }
}
