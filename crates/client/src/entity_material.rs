//! Entity model materials. A skin named `<type>@<image>` takes the stages of
//! `shadertypes/model/<type>.stype` with `$texturename` bound to the skin; a
//! skin without one takes its `skins/<name>` script block when there is one
//! (the S&D bomb's timer face), else the implicit model skin, one opaque
//! `lightingDiffuse` stage (0x4fc440). Every model resolves its skins the
//! same way, props included (docs/research/cod11-shader-scripts.md, section
//! 8, "Model skin scripts"). Only `lightingDiffuse` stages take the light grid; the
//! rest draw their constant or wave colour
//! (docs/research/cod11-light-grid-and-leaf-lights.md, section 13).

use std::collections::HashMap;
use std::rc::Rc;
use vcod_common::asset_source::{AssetSource, RawFile};

use crate::renderer::{
    DEPTH_FORMAT, DYNAMIC_INSTANCE_STRIDE, STAGE_PARAMS_SIZE, msaa_samples, stage_animated,
    stage_params, upload_levels,
};
use crate::texture_settings::{Mips, TextureSettings};
use vcod_common::assets;
use vcod_common::material::{
    AlphaGen, BlendFactor, Bundle, Clamp, DEFAULT_IMAGE, DepthFunc, ImageRef, Material,
    MaterialLib, ModelLight, RgbGen, SORT_OPAQUE, Stage, TcGen, TexEnv, TexSemantic,
};
use vcod_common::model::Vertex;
use vcod_common::pk3::Pk3Fs;
pub use vcod_common::props::{is_typed_skin, skin_script, skin_type};
use vcod_common::shader::{Shader, WarnSet, default_material, parse_shader, split_blocks};

/// The skin's own image in a template stage.
pub const TEXTURE_NAME: &str = "$texturename";

/// The implicit model skin: one opaque stage lit from the grid.
pub fn implicit(skin: &str) -> Material {
    Material::from_shader(&Shader {
        name: skin.to_string(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path(TEXTURE_NAME.to_string()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: Vec::new(),
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::LightingDiffuse,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    })
}

/// Parses a `.stype` body. When a stage drops (an unsupported form), a
/// blended stage left first draws opaque instead of over nothing.
pub fn from_stype(skin: &str, text: &str, warns: &mut WarnSet) -> Option<Shader> {
    let (_, body) = split_blocks(text).into_iter().next()?;
    let mut sh = parse_shader(skin, &body, warns);
    if sh.stages.is_empty() {
        return None;
    }
    if sh.dropped_stages > 0 {
        sh.stages[0].blend = None;
        sh.stages[0].depth_write = None;
    }
    Some(sh)
}

/// The skin's material: its `skins/` script block or the implicit skin,
/// or for a `<type>@<image>` skin its template, retail's default shader
/// when that cannot be built (0x4fc5c0). A CoD2 model names its compiled
/// material directly ([`cod2`]).
pub fn load(fs: &dyn AssetSource, lib: &MaterialLib, skin: &str, warns: &mut WarnSet) -> Material {
    if fs.game() == vcod_common::game::Game::Cod2 {
        // CoD4's search path is iwds too, so `fs` reads as CoD2's: a CoD4
        // set is told by its compiled `Cod4Lit`
        return match lib.get(skin) {
            Some(m) if is_cod4_set(m) && !m.stages.is_empty() => cod4(m),
            _ => cod2(lib, skin),
        };
    }
    if is_typed_skin(skin) {
        return template(fs, skin, warns)
            .map_or_else(default_material, |sh| Material::from_shader(&sh));
    }
    match skin_script(skin).and_then(|n| lib.get(&n)) {
        Some(sh) if fs.game() == vcod_common::game::Game::Cod4 && !sh.stages.is_empty() => cod4(sh),
        Some(sh) if !sh.stages.is_empty() => sh.clone(),
        _ => implicit(skin),
    }
}

/// A CoD4 model material as the entity and view model pipelines draw it:
/// a lit model set's (`mc_l_*`, `mc_ambient_t0c0`, cod4-world.md 3.2,
/// 3.3) stage takes the light grid per pixel, through cod2_model.wgsl's
/// probe path, which reads the entity's block (`entity_light::pack_cod4`)
/// and the set's normal, specular and detail maps; every other set as
/// compiled.
pub fn cod4(m: &Material) -> Material {
    let Some(p) = m.phong.filter(|_| lights_cod4_grid(m)) else {
        return m.clone();
    };
    let mut m = lit_stages(m);
    m.probe = Some(vcod_common::material::ProbeLit {
        specular: p.specular,
        alpha: p.alpha,
        detail: p.detail,
        scroll: false,
        flag: false,
        detail_scale: p.detail_scale,
    });
    m
}

/// Whether `m` was compiled from a CoD4 technique set
/// (`vcod_cod4::materials::compile`): only those carry a `Cod4Lit`.
fn is_cod4_set(m: &Material) -> bool {
    m.phong.and_then(|p| p.cod4).is_some()
}

/// Whether `m` is a CoD4 lit model set drawn off the model lighting
/// volume: an `lp_*` technique under an opaque, alpha-tested or blended
/// state (`vcod_cod4::materials::compile` gives such a set its `Cod4Lit`).
pub fn lights_cod4_grid(m: &Material) -> bool {
    m.phong.and_then(|p| p.cod4).is_some_and(|c| {
        matches!(
            c.light,
            vcod_common::material::Cod4Light::Grid | vcod_common::material::Cod4Light::GridAmbient
        )
    })
}

/// A CoD2 model surface's material, by its own name (`mtl_*`), drawn by
/// its technique set's `lprobe` technique where it has one: the colour map
/// times the vertex colour times the grid sample and the sun, no lightmap
/// bundle; the vertex alpha stays, which the `_a` techniques output. An
/// `ambient_*` set's entity technique is `vertcol_shaded`, the stages as
/// compiled (cod2-materials.md 9.2). A name the library lacks draws
/// retail's default material.
fn cod2(lib: &MaterialLib, name: &str) -> Material {
    let Some(m) = lib.get(name).filter(|m| !m.stages.is_empty()) else {
        return default_material();
    };
    if m.model_light == ModelLight::Probe {
        return lit_stages(m);
    }
    m.clone()
}

/// A lit CoD2 model material's stages as `lightingDiffuse`, the colour map
/// alone: an entity's light set or a static prop's baked light, which the
/// stage path rescales from identityLight 0.5 (shader.wgsl's `F_LIT`).
pub fn lit_stages(m: &Material) -> Material {
    let mut m = m.clone();
    for st in &mut m.stages {
        st.rgb_gen = RgbGen::LightingDiffuse;
        // the library compiled the set's lightmap pass, if it has one
        st.bundles.truncate(1);
    }
    m
}

/// A `<type>@<image>` skin's `shadertypes/model/<type>.stype`, its
/// `$texturename` still unbound. `None`, for which retail draws its default
/// shader (0x4fc0c0 returns 0), without an `@`, with a slash after it,
/// without the file or a stage, or when a `$texturename` stage's image is
/// missing ("shadertype '%s' couldn't load image for '%s'").
pub fn template(fs: &dyn AssetSource, skin: &str, warns: &mut WarnSet) -> Option<Shader> {
    let sh = skin_type(skin)
        .and_then(|kind| {
            fs.load::<RawFile>(&format!("shadertypes/model/{kind}.stype"))
                .ok()
        })
        .and_then(|file| from_stype(skin, &file.text(), warns))?;
    let names_skin = sh
        .stages
        .iter()
        .flat_map(|st| &st.bundles)
        .any(|b| matches!(&b.image, ImageRef::Path(p) if p.eq_ignore_ascii_case(TEXTURE_NAME)));
    if names_skin && !assets::skin_image_exists(fs, skin) {
        warns.warn_once(
            skin,
            "shadertype couldn't load image, drawing the default shader",
        );
        return None;
    }
    Some(sh)
}

/// `sh` with every `$texturename` bundle naming `path` instead: how a
/// static prop's stage path, which binds images by name, takes its skin.
pub fn bind_texture_name(mut sh: Shader, path: &str) -> Shader {
    for bu in sh.stages.iter_mut().flat_map(|st| &mut st.bundles) {
        if matches!(&bu.image, ImageRef::Path(p) if p.eq_ignore_ascii_case(TEXTURE_NAME)) {
            bu.image = ImageRef::Path(path.to_string());
        }
    }
    sh
}

/// Whether some stage takes the light grid: retail picks an entity's lights
/// only then (0x50e3e0, the `+0x54 & 0x18` test).
pub fn is_lit(sh: &Material) -> bool {
    sh.stages
        .iter()
        .any(|st| st.rgb_gen == RgbGen::LightingDiffuse)
}

// ---- GPU side: per-stage pipelines and params for entities and the viewmodel ----

/// Which pass a pipeline draws in: snapshot entities (`dynamic_model.wgsl`,
/// instanced) or the first-person viewmodel (`viewmodel.wgsl`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EntPass {
    World = 0,
    View = 1,
}

/// A stage's image: the surface's skin, or one the material names
/// (`$whiteimage`, an environment map), by index into `EntityMaterials::images`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StageImage {
    Skin,
    Own(usize),
}

/// One stage of an entity material.
#[derive(Clone, Copy)]
pub(crate) struct EntStageDraw {
    /// Into `EntityMaterials::pipelines`.
    pub(crate) pipeline: usize,
    pub(crate) image: StageImage,
    /// `StageParams` slot in `EntityMaterials::params_buf`.
    pub(crate) slot: u32,
    /// Group 3: `EntityMaterials::param_groups`, its extra maps; 0 the
    /// defaults.
    pub(crate) params: usize,
    /// A first stage the depth prepass takes (`EntityMaterials::prepass`):
    /// unblended, depth written, LessEqual, no alpha test.
    pub(crate) prepass: bool,
    /// A first stage the `floatZ` prepass takes (cod4-client-perf.md 35):
    /// a CoD4 material 1.7's prepass draws (`Material::floatz`), which
    /// alpha tests there as its stage does.
    pub(crate) floatz: bool,
    /// No deform and no flag wave: `vs_main`'s position reads nothing of
    /// the stage, so the depth prepass may join its surface to the next.
    pub(crate) rigid: bool,
}

/// The model vertex and the `InstanceRaw` instance step every entity
/// pipeline reads (the shadow casters too).
pub(crate) fn vertex_layouts() -> [wgpu::VertexBufferLayout<'static>; 2] {
    const VERTEX: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x3, // pos
        1 => Float32x3, // normal
        2 => Float32x2, // uv
        3 => Uint8x4,   // bone_indices
        4 => Float32x4, // bone_weights
        12 => Unorm8x4, // color
        13 => Float32x3, // tangent
        14 => Float32x3, // binormal
    ];
    // the transform's four columns, then bone_base, light_set and
    // time_offset; the padding needs no attribute
    const INSTANCE: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Uint32,
        10 => Uint32,
        11 => Float32,
    ];
    [
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX,
        },
        wgpu::VertexBufferLayout {
            array_stride: DYNAMIC_INSTANCE_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE,
        },
    ]
}

/// A skin's material on the GPU.
pub(crate) struct EntSkin {
    /// In stage order; a surface draws them back to back, as retail's stage
    /// iterator does.
    pub(crate) stages: Vec<EntStageDraw>,
    /// Some stage takes the light grid.
    pub(crate) lit: bool,
    /// The material's sort (`Material::sort`), retail's first sort key.
    pub(crate) sort: f32,
    /// Load order: stands in for retail's tie-break between equal sorts
    /// (R_SortNewShader, CoDMP.exe 0x4fb790).
    pub(crate) order: u32,
    /// A CoD2 material whose set names `shadowcookie caster`: the surface
    /// draws into its entity's shadow cookie (`renderer::cod2_cookie`).
    pub(crate) cookie_caster: bool,
    /// A CoD4 material that casts into the sun's shadow map
    /// (`crate::cod4_sunshadow`).
    pub(crate) sun_caster: bool,
    /// An alpha-tested caster's colour map: its first alpha-tested
    /// stage's image, which the cast tests at 128.
    pub(crate) sun_cut: Option<StageImage>,
}

/// One `StageParams` slot per entity material stage, a multiple of the
/// widest uniform offset alignment so no device limit is needed.
pub(crate) const ENT_STAGE_STRIDE: u64 = 512;
const MAX_ENT_STAGES: u32 = 1024;
const _: () = assert!(STAGE_PARAMS_SIZE <= ENT_STAGE_STRIDE);

/// A stage's blend pair, depth write, depth test and whether a world
/// entity's draw culls back faces (`Material::entity_cull`).
pub(crate) type EntPipelineKey = (Option<(BlendFactor, BlendFactor)>, bool, DepthFunc, bool);

/// Every pipeline key a stock CoD2 xmodel's stages draw with (the census
/// `cod2_model_pipelines_cover_every_stock_model` holds): built with the
/// level's first models, so a later level's new model (sd's
/// `mp_tntbomb_obj`, a `SrcAlpha One` glow) builds none in play
/// (cod2-client-perf.md, "The sd level's first frame").
pub(crate) const COD2_MODEL_PIPELINES: [EntPipelineKey; 5] = [
    (None, true, DepthFunc::LessEqual, false),
    (
        Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
        false,
        DepthFunc::LessEqual,
        false,
    ),
    (
        Some((BlendFactor::SrcAlpha, BlendFactor::One)),
        false,
        DepthFunc::LessEqual,
        false,
    ),
    (
        Some((BlendFactor::Zero, BlendFactor::SrcColor)),
        false,
        DepthFunc::LessEqual,
        false,
    ),
    (
        Some((BlendFactor::One, BlendFactor::One)),
        false,
        DepthFunc::LessEqual,
        false,
    ),
];

/// Whether `sh`'s world entity draws cull back faces
/// (`Material::entity_cull`); `VCOD_C4_ENTITY_CULL=0` draws them two-sided
/// as before, for an A/B (cod4-client-perf.md 37).
fn entity_cull(sh: &Material) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    sh.entity_cull
        && *ON.get_or_init(|| std::env::var_os("VCOD_C4_ENTITY_CULL").is_none_or(|v| v != "0"))
}

/// One pass's shader and pipeline layout.
struct PassShader {
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
}

/// Entity and viewmodel materials on the GPU: their pipelines, stage params
/// and the images stages name.
pub(crate) struct EntityMaterials {
    passes: [PassShader; 2],
    format: wgpu::TextureFormat,
    pub(crate) skin_layout: wgpu::BindGroupLayout,
    /// Group 3's layout, the stage params.
    pub(crate) params_layout: wgpu::BindGroupLayout,
    /// One pipeline pair (indexed by [`EntPass`]) per key; entry 0 is opaque.
    pub(crate) pipelines: Vec<(EntPipelineKey, [wgpu::RenderPipeline; 2])>,
    /// `r_vc_depthprepass`' entity pipelines: the depth-only one, then the
    /// opaque key's colour pipeline under EQUAL with no depth write;
    /// two-sided, then culled as a `Material::entity_cull` stage is.
    pub(crate) prepass: [[wgpu::RenderPipeline; 2]; 2],
    /// The depth-only one's twin that writes `floatZ` too
    /// (cod4-client-perf.md 35), two-sided and culled.
    pub(crate) floatz: [wgpu::RenderPipeline; 2],
    pub(crate) params_buf: wgpu::Buffer,
    /// Group 3 with the default extra maps (flat normal, black specular,
    /// grey detail): what every stage but a CoD2 probe stage binds.
    pub(crate) params_bg: wgpu::BindGroup,
    /// Group 3 per CoD2 probe stage's maps, by (normal, specular, detail)
    /// path; entry 0 is `params_bg`.
    pub(crate) param_groups: Vec<wgpu::BindGroup>,
    param_index: HashMap<[String; 3], usize>,
    /// Each of `param_groups`' normal, specular and detail map, an index
    /// into `views` (`None` the default), to rebuild it by.
    param_maps: Vec<[Option<usize>; 3]>,
    /// The flat normal, black and grey defaults, then `$specularity`.
    default_views: [wgpu::TextureView; 4],
    /// Group 3's binding 5: the CoD4 map's reflection probes as a cube
    /// array (`set_probes`), else one black cube.
    probe_view: wgpu::TextureView,
    /// Group 3's binding 6: CoD4 entities' model lighting volume.
    pub(crate) cod4_volume: Cod4Volume,
    /// Group 3's binding 7: CoD2's light grid weight cubes.
    grid_weights: wgpu::TextureView,
    /// Stage images; entry 0 is white.
    pub(crate) images: Vec<wgpu::BindGroup>,
    /// `images`' views, index for index.
    views: Vec<wgpu::TextureView>,
    /// By path.
    image_index: HashMap<String, usize>,
    /// By skin name; shared by every model that names the skin.
    by_skin: HashMap<String, Rc<EntSkin>>,
    /// Stages whose params move with time; rewritten each frame.
    pub(crate) animated: Vec<(Material, usize, u32)>,
    next_slot: u32,
    warns: WarnSet,
    /// What stage images load with: `r_picmip2` unless the shader says
    /// `nopicmip`, and `r_texturebits`.
    textures: TextureSettings,
}

/// The layouts the two passes bind before group 3, the stage params.
pub(crate) struct PassLayouts<'a> {
    pub(crate) camera: &'a wgpu::BindGroupLayout,
    pub(crate) bones: &'a wgpu::BindGroupLayout,
    pub(crate) view_uniform: &'a wgpu::BindGroupLayout,
    pub(crate) view_bones: &'a wgpu::BindGroupLayout,
}

impl EntityMaterials {
    /// Group 3's params buffer and both passes' pipeline layouts. Builds the
    /// opaque pipeline pair up front so both shaders validate at startup.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        layouts: PassLayouts,
        skin_layout: &wgpu::BindGroupLayout,
        textures: TextureSettings,
        white_view: &wgpu::TextureView,
    ) -> Self {
        let map_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("entity stage params layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                    },
                    count: None,
                },
                // cod2_model.wgsl's normal, specular, detail maps and
                // `$specularity`
                map_entry(1),
                map_entry(2),
                map_entry(3),
                map_entry(4),
                // cod2_model.wgsl's CoD4 reflection probes
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::CubeArray,
                        multisampled: false,
                    },
                    count: None,
                },
                // cod2_model.wgsl's CoD4 entity model lighting volume
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                // cod2_model.wgsl's `$lightgridweights0` and `1`
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::CubeArray,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("entity stage params"),
            size: u64::from(MAX_ENT_STAGES) * ENT_STAGE_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rgba = |label, px: [u8; 4]| {
            plain_view(
                device,
                queue,
                label,
                (1, 1),
                wgpu::TextureFormat::Rgba8Unorm,
                &px,
            )
        };
        let default_views = [
            // .wy 0.5: the identity normal map
            rgba("flat normal", [128; 4]),
            rgba("black specular", [0; 4]),
            // detail.rgb - 0.5 adds nothing
            rgba("grey detail", [128; 4]),
            plain_view(
                device,
                queue,
                "$specularity",
                (32, 256),
                wgpu::TextureFormat::R8Unorm,
                &crate::renderer::specularity_map(),
            ),
        ];
        let probe_view = probe_array(device, queue, &[]);
        let cod4_volume = Cod4Volume::new(device);
        let grid_weights = grid_weight_cubes(device, queue);
        let params_bg = params_group(
            device,
            &params_layout,
            &params_buf,
            [&default_views[0], &default_views[1], &default_views[2]],
            &default_views[3],
            [&probe_view, &cod4_volume.view, &grid_weights],
        );
        let pass = |label, shader: wgpu::ShaderModule, g0, g2| PassShader {
            layout: device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(g0), Some(skin_layout), Some(g2), Some(&params_layout)],
                immediate_size: 0,
            }),
            shader,
        };
        let passes = [
            pass(
                "dynamic pipeline layout",
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("dynamic_model.wgsl"),
                    source: wgpu::ShaderSource::Wgsl(DYNAMIC_WGSL.into()),
                }),
                layouts.camera,
                layouts.bones,
            ),
            pass(
                "viewmodel pipeline layout",
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("viewmodel.wgsl"),
                    source: wgpu::ShaderSource::Wgsl(VIEWMODEL_WGSL.into()),
                }),
                layouts.view_uniform,
                layouts.view_bones,
            ),
        ];
        let prepass = [
            ((None, true, DepthFunc::LessEqual), false),
            ((None, false, DepthFunc::Equal), true),
        ];
        let prepass = [false, true].map(|cull| {
            prepass.clone().map(|((b, w, d), color)| {
                ent_pipeline(
                    device,
                    &passes,
                    format,
                    EntPass::World,
                    &(b, w, d, cull),
                    color,
                )
            })
        });
        let floatz = [false, true].map(|cull| ent_floatz_pipeline(device, &passes, cull));
        let mut materials = Self {
            prepass,
            floatz,
            passes,
            format,
            skin_layout: skin_layout.clone(),
            params_layout,
            pipelines: Vec::new(),
            params_buf,
            param_groups: vec![params_bg.clone()],
            params_bg,
            param_index: HashMap::new(),
            param_maps: vec![[None; 3]],
            default_views,
            probe_view,
            cod4_volume,
            grid_weights,
            images: Vec::new(),
            views: Vec::new(),
            image_index: HashMap::new(),
            by_skin: HashMap::new(),
            animated: Vec::new(),
            next_slot: 0,
            warns: WarnSet::new(),
            textures,
        };
        materials
            .images
            .push(materials.image_group(device, white_view));
        materials.views.push(white_view.clone());
        // pipeline 0 is opaque, the implicit skin's
        materials.pipeline(device, (None, true, DepthFunc::LessEqual, false));
        materials
    }

    /// Forgets every material and stage image, for a map change (retail
    /// reloads them too).
    pub(crate) fn clear(&mut self) {
        self.by_skin.clear();
        self.animated.clear();
        self.images.truncate(1);
        self.views.truncate(1);
        self.image_index.clear();
        self.param_groups.truncate(1);
        self.param_maps.truncate(1);
        self.param_index.clear();
        self.next_slot = 0;
    }

    /// A stage image's group 1; a `clampmap` bundle picks its sampler
    /// through `STAGE_FLAG_CLAMP_S` / `_T` instead.
    fn image_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entity stage image"),
            layout: &self.skin_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        })
    }

    /// The pipeline pair for a stage's key, built on first use.
    fn pipeline(&mut self, device: &wgpu::Device, key: EntPipelineKey) -> usize {
        if let Some(i) = self.pipelines.iter().position(|(k, _)| *k == key) {
            return i;
        }
        let pair = [EntPass::World, EntPass::View].map(|p| self.build_pipeline(device, p, &key));
        self.pipelines.push((key, pair));
        self.pipelines.len() - 1
    }

    /// Builds the pipeline pairs of `keys` not built yet, each pass on
    /// its own thread ([`crate::renderer::par_build`]): a driver that
    /// compiles on the calling thread (lavapipe) takes about 110 ms a pair.
    pub(crate) fn prewarm(&mut self, device: &wgpu::Device, keys: &[EntPipelineKey]) {
        let mut fresh: Vec<EntPipelineKey> = Vec::new();
        for k in keys {
            if !self.pipelines.iter().any(|(b, _)| b == k) && !fresh.contains(k) {
                fresh.push(k.clone());
            }
        }
        if fresh.is_empty() {
            return;
        }
        let passes = [EntPass::World, EntPass::View];
        let (shaders, format) = (&self.passes, self.format);
        let mut built = crate::renderer::par_build(fresh.len() * passes.len(), |i| {
            let (pass, key) = (passes[i % passes.len()], &fresh[i / passes.len()]);
            ent_pipeline(device, shaders, format, pass, key, true)
        })
        .into_iter();
        for key in fresh {
            let (Some(world), Some(view)) = (built.next(), built.next()) else {
                unreachable!("par_build returns one pipeline a job");
            };
            self.pipelines.push((key, [world, view]));
        }
    }

    /// The pipeline pairs built so far.
    /// The pipeline keys [`Self::skin`] would build for `skins` not
    /// built yet, read off their materials as it reads them, for
    /// [`Self::prewarm`] to build at once.
    pub(crate) fn skin_pipeline_keys(
        &mut self,
        fs: &Pk3Fs,
        lib: &MaterialLib,
        skins: &[String],
    ) -> Vec<EntPipelineKey> {
        let mut slot = self.next_slot.max(1);
        let mut keys = Vec::new();
        for skin in skins
            .iter()
            .filter(|s| !self.by_skin.contains_key(s.as_str()))
        {
            let sh = load(fs, lib, skin, &mut self.warns);
            for (i, st) in sh.stages.iter().enumerate() {
                if slot == MAX_ENT_STAGES {
                    return keys;
                }
                slot += 1;
                let depth_write = sh.classify_stage(i).is_some_and(|c| c.depth_write);
                keys.push((
                    st.blend.clone(),
                    depth_write,
                    st.depth_func,
                    entity_cull(&sh),
                ));
            }
        }
        keys
    }

    #[cfg(test)]
    pub(crate) fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    fn build_pipeline(
        &self,
        device: &wgpu::Device,
        pass: EntPass,
        key: &EntPipelineKey,
    ) -> wgpu::RenderPipeline {
        ent_pipeline(device, &self.passes, self.format, pass, key, true)
    }
}

/// An entity or view model stage pipeline for `key`; without `color` the
/// depth prepass's (no fragment stage; `vs_main`'s position is
/// `@invariant`, so its depths are the colour pipelines' bit for bit).
fn ent_pipeline(
    device: &wgpu::Device,
    passes: &[PassShader; 2],
    format: wgpu::TextureFormat,
    pass: EntPass,
    key: &EntPipelineKey,
    color: bool,
) -> wgpu::RenderPipeline {
    ent_pipeline_to(device, passes, (format, "fs_main"), pass, key, color)
}

/// The depth prepass's entity pipeline that writes `floatZ` (`fs_floatz`,
/// cod4-client-perf.md 35) into the R32F target beside the depth.
fn ent_floatz_pipeline(
    device: &wgpu::Device,
    passes: &[PassShader; 2],
    cull: bool,
) -> wgpu::RenderPipeline {
    let key = (None, true, DepthFunc::LessEqual, cull);
    let target = (crate::soft_particles::LINZ_FORMAT, "fs_floatz");
    ent_pipeline_to(device, passes, target, EntPass::World, &key, true)
}

/// [`ent_pipeline`] drawing fragment `entry` into a `format` target.
fn ent_pipeline_to(
    device: &wgpu::Device,
    passes: &[PassShader; 2],
    (format, entry): (wgpu::TextureFormat, &str),
    pass: EntPass,
    key: &EntPipelineKey,
    color: bool,
) -> wgpu::RenderPipeline {
    let floatz = entry == "fs_floatz";
    let constants = [
        ("FLOATZ_NEAR", f64::from(crate::camera::Z_NEAR)),
        ("FLOATZ_FAR", f64::from(crate::camera::Z_FAR)),
    ];
    let (blend, depth_write, depth_func, cull) = key;
    let blend_state = blend.as_ref().map(|(src, dst)| {
        let c = wgpu::BlendComponent {
            src_factor: wgpu_blend_factor(src),
            dst_factor: wgpu_blend_factor(dst),
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState { color: c, alpha: c }
    });
    let [vertex, instance] = vertex_layouts().map(Some);
    let buffers = match pass {
        EntPass::World => vec![vertex, instance],
        EntPass::View => vec![vertex],
    };
    let ps = &passes[pass as usize];
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend: blend_state,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match (pass, color) {
            (_, false) => "dynamic model depth prepass",
            _ if floatz => "dynamic model floatZ prepass",
            (EntPass::World, true) => "dynamic model pipeline",
            (EntPass::View, true) => "viewmodel pipeline",
        }),
        layout: Some(&ps.layout),
        vertex: wgpu::VertexState {
            module: &ps.shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        fragment: color.then(|| wgpu::FragmentState {
            module: &ps.shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: if floatz { &constants } else { &[] },
                ..Default::default()
            },
            targets: &targets,
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Unculled: the gun is wound opposite to the map and the
            // hands inconsistently, so no front_face works. A CoD4 world
            // entity's culled material culls as 1.7 does.
            front_face: wgpu::FrontFace::Cw,
            cull_mode: (*cull && pass == EntPass::World).then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(*depth_write),
            // GL_LEQUAL by default, so a later stage passes over the
            // depths an earlier one wrote (GL state 0x4d57b0)
            depth_compare: Some(match depth_func {
                DepthFunc::LessEqual => wgpu::CompareFunction::LessEqual,
                DepthFunc::Equal => wgpu::CompareFunction::Equal,
                DepthFunc::Always => wgpu::CompareFunction::Always,
            }),
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
}

impl EntityMaterials {
    /// A stage image the material names, loaded on first use through the
    /// world's resolution and texture gamma.
    #[allow(clippy::too_many_arguments)]
    fn own_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (fs, decoded): (&Pk3Fs, &assets::DecodedImages),
        gamma: Option<&[u8; 256]>,
        path: &str,
        nopicmip: bool,
        nomipmaps: bool,
    ) -> usize {
        let key = path.to_string();
        if let Some(&i) = self.image_index.get(&key) {
            return i;
        }
        let mut img = match assets::resolve_bundle_image(fs, path) {
            _ if path == DEFAULT_IMAGE => assets::default_image(),
            Some(full) => decoded.load_path_image(fs, &full),
            None => {
                log::warn!("entity stage image {path} not found, using the default image");
                assets::default_image()
            }
        };
        let load = crate::texture_settings::path_levels(fs);
        let mips = if nomipmaps {
            Mips::single(path, &load)
        } else {
            Mips::chain(path, &load)
        };
        let levels = crate::texture_settings::prepare(
            &mut img,
            self.textures.model_picmip(nopicmip),
            gamma,
            self.textures.store(),
            mips,
        );
        let view = upload_levels(device, queue, path, &img, &levels);
        let group = self.image_group(device, &view);
        self.images.push(group);
        self.views.push(view);
        self.image_index.insert(key, self.images.len() - 1);
        self.images.len() - 1
    }

    /// A skin's material, built on first use. Past `MAX_ENT_STAGES` a new
    /// skin draws as the implicit one at slot 0.
    pub(crate) fn skin(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (fs, decoded): (&Pk3Fs, &assets::DecodedImages),
        lib: &MaterialLib,
        gamma: Option<&[u8; 256]>,
        skin: &str,
    ) -> Rc<EntSkin> {
        if let Some(d) = self.by_skin.get(skin) {
            return d.clone();
        }
        if self.next_slot == 0 {
            // slot 0: the implicit skin, the fallback
            self.write_params(queue, &implicit(""), 0, 0);
            self.next_slot = 1;
        }
        let sh = load(fs, lib, skin, &mut self.warns);
        let mut lit = is_lit(&sh);
        let mut sort = sh.sort;
        let mut stages = Vec::with_capacity(sh.stages.len());
        for (i, st) in sh.stages.iter().enumerate() {
            if self.next_slot == MAX_ENT_STAGES {
                static FULL: std::sync::Once = std::sync::Once::new();
                FULL.call_once(|| {
                    log::warn!(
                        "more than {MAX_ENT_STAGES} entity material stages; the rest draw plain"
                    )
                });
                stages.clear();
                break;
            }
            let b0 = &st.bundles[0];
            let image = match &b0.image {
                ImageRef::White => StageImage::Own(0),
                ImageRef::Path(p) if p.eq_ignore_ascii_case(TEXTURE_NAME) => StageImage::Skin,
                ImageRef::Path(p) => {
                    let (nopicmip, nomipmaps) = (sh.nopicmip, sh.nomipmaps);
                    StageImage::Own(self.own_image(
                        device,
                        queue,
                        (fs, decoded),
                        gamma,
                        p,
                        nopicmip,
                        nomipmaps,
                    ))
                }
                ImageRef::Lightmap => {
                    self.warns
                        .warn_once(skin, "entity stage $lightmap draws white");
                    StageImage::Own(0)
                }
            };
            let slot = self.next_slot;
            self.next_slot += 1;
            self.write_params(queue, &sh, i, slot);
            let depth_write = sh.classify_stage(i).is_some_and(|c| c.depth_write);
            let pipeline = self.pipeline(
                device,
                (
                    st.blend.clone(),
                    depth_write,
                    st.depth_func,
                    entity_cull(&sh),
                ),
            );
            let params = match sh.probe {
                Some(_) if st.rgb_gen == RgbGen::LightingDiffuse => {
                    self.probe_params(device, queue, (fs, decoded), &sh)
                }
                _ => 0,
            };
            let prepass = i == 0
                && st.blend.is_none()
                && depth_write
                && st.depth_func == DepthFunc::LessEqual
                && st.alpha_func.is_none();
            let floatz = i == 0 && sh.floatz.is_some();
            let rigid = stage_params(&sh, i, 0.0).is_none_or(|p| p.deform_kinds == [0; 4]);
            stages.push(EntStageDraw {
                pipeline,
                image,
                slot,
                params,
                prepass,
                floatz,
                rigid,
            });
        }
        if stages.is_empty() {
            stages.push(EntStageDraw {
                pipeline: 0,
                image: StageImage::Skin,
                slot: 0,
                params: 0,
                prepass: true,
                floatz: true,
                rigid: true,
            });
            lit = true;
            sort = SORT_OPAQUE;
        }
        // `stages` is the material's, one for one (or the fallback alone)
        let sun_cut = sh
            .stages
            .iter()
            .zip(&stages)
            .find(|(st, _)| st.alpha_func.is_some())
            .map(|(_, d)| d.image)
            .filter(|_| sh.cookie.sun_caster);
        let out = Rc::new(EntSkin {
            stages,
            lit,
            sort,
            order: self.by_skin.len() as u32,
            cookie_caster: sh.cookie.caster,
            sun_caster: sh.cookie.sun_caster,
            sun_cut,
        });
        self.by_skin.insert(skin.to_string(), out.clone());
        out
    }

    /// A CoD2 probe stage's group 3: the material's normal, specular and
    /// detail maps, each loaded raw (no texture gamma), the default where
    /// it names none.
    fn probe_params(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (fs, decoded): (&Pk3Fs, &assets::DecodedImages),
        sh: &Material,
    ) -> usize {
        let path = |sem| match sh.texture(sem) {
            Some(ImageRef::Path(p)) => p.clone(),
            _ => String::new(),
        };
        let key = [
            path(TexSemantic::Normal),
            path(TexSemantic::Specular),
            path(TexSemantic::Detail),
        ];
        if let Some(&i) = self.param_index.get(&key) {
            return i;
        }
        let views: Vec<Option<usize>> = key
            .iter()
            .map(|p| {
                (!p.is_empty()).then(|| {
                    let src = (fs, decoded);
                    self.own_image(device, queue, src, None, p, sh.nopicmip, sh.nomipmaps)
                })
            })
            .collect();
        let maps = [views[0], views[1], views[2]];
        let group = self.params_group_of(device, maps);
        self.param_groups.push(group);
        self.param_maps.push(maps);
        self.param_index.insert(key, self.param_groups.len() - 1);
        self.param_groups.len() - 1
    }

    /// Group 3 over `maps` (indices into `views`, `None` the default).
    fn params_group_of(&self, device: &wgpu::Device, maps: [Option<usize>; 3]) -> wgpu::BindGroup {
        let view = |k: usize| match maps[k] {
            None => &self.default_views[k],
            Some(i) => &self.views[i],
        };
        params_group(
            device,
            &self.params_layout,
            &self.params_buf,
            [view(0), view(1), view(2)],
            &self.default_views[3],
            [&self.probe_view, &self.cod4_volume.view, &self.grid_weights],
        )
    }

    /// Binds a CoD4 map's reflection probes (cod4-world.md 9.7) to every
    /// group 3, layer `i` probe `i`; an empty list binds one black cube.
    pub(crate) fn set_probes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        probes: &[vcod_common::mapworld::ProbeCube],
    ) {
        self.probe_view = probe_array(device, queue, probes);
        self.params_bg = self.params_group_of(device, [None; 3]);
        self.param_groups = (0..self.param_maps.len())
            .map(|i| self.params_group_of(device, self.param_maps[i]))
            .collect();
    }

    fn write_params(&mut self, queue: &wgpu::Queue, sh: &Material, idx: usize, slot: u32) {
        if let Some(p) = stage_params(sh, idx, 0.0) {
            queue.write_buffer(
                &self.params_buf,
                u64::from(slot) * ENT_STAGE_STRIDE,
                bytemuck::bytes_of(&p),
            );
        }
        if stage_animated(sh, idx) {
            self.animated.push((sh.clone(), idx, slot));
        }
    }
}

/// Group 3: the params buffer at its dynamic slot, then cod2_model.wgsl's
/// normal, specular and detail maps and `$specularity`.
fn params_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
    maps: [&wgpu::TextureView; 3],
    specularity: &wgpu::TextureView,
    [probes, volume, grid_weights]: [&wgpu::TextureView; 3],
) -> wgpu::BindGroup {
    let tex = |binding, view| wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("entity stage params bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(STAGE_PARAMS_SIZE),
                }),
            },
            tex(1, maps[0]),
            tex(2, maps[1]),
            tex(3, maps[2]),
            tex(4, specularity),
            tex(5, probes),
            tex(6, volume),
            tex(7, grid_weights),
        ],
    })
}

/// CoD2's `$lightgridweights0` and `1` (vcod_cod2::lightgrid::weight_cubes,
/// A8R8G8B8 as retail builds them, so B8G8R8A8 here) as one cube array,
/// layer 0 corners 0-3 and layer 1 corners 4-7; one level, as
/// `Image_Setup`'s flags give them (cod2-materials.md 9.2).
pub(crate) fn grid_weight_cubes(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let side = vcod_cod2::lightgrid::WEIGHT_CUBE_SIZE as u32;
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("$lightgridweights"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 12,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &vcod_cod2::lightgrid::weight_cubes().concat(),
        )
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            ..Default::default()
        })
}

/// CoD4 entities' model lighting volume, 1.7's `modelLightingSampler`
/// (`R_InitModelLightingImage`: 256 wide, four deep, no mips; cod4-world.md
/// 3.2): each frame's lit models take a 4 x 4 x 4 block from 0 on, the
/// view model the last ([`crate::entity_light::COD4_VOLUME_BLOCKS`]).
pub(crate) struct Cod4Volume {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Cod4Volume {
    fn new(device: &wgpu::Device) -> Self {
        use crate::entity_light::{COD4_VOLUME_ROW, COD4_VOLUME_ROWS};
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cod4 entity model lighting volume"),
            size: wgpu::Extent3d {
                width: 4 * COD4_VOLUME_ROW,
                height: 4 * COD4_VOLUME_ROWS,
                depth_or_array_layers: 4,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }

    /// Writes blocks `0..blocks.len()` (at most all but the last), whole
    /// rows of them.
    pub(crate) fn write_blocks(&self, queue: &wgpu::Queue, blocks: &[[[u8; 4]; 64]]) {
        use crate::entity_light::{COD4_VOLUME_BLOCKS, COD4_VOLUME_ROW};
        let blocks = &blocks[..blocks.len().min(COD4_VOLUME_BLOCKS as usize - 1)];
        let (rows, rgba) = crate::entity_light::cod4_volume_rows(blocks);
        self.write_box(queue, (0, 0), (4 * COD4_VOLUME_ROW, 4 * rows), &rgba);
    }

    /// Writes the view model's block, the last.
    pub(crate) fn write_view_block(&self, queue: &wgpu::Queue, block: &[[u8; 4]; 64]) {
        use crate::entity_light::{COD4_VOLUME_BLOCKS, COD4_VOLUME_ROW};
        let i = COD4_VOLUME_BLOCKS - 1;
        let at = (4 * (i % COD4_VOLUME_ROW), 4 * (i / COD4_VOLUME_ROW));
        // a block's texels in order are its 4 x 4 x 4 box's rows
        self.write_box(queue, at, (4, 4), block.as_flattened());
    }

    fn write_box(&self, queue: &wgpu::Queue, (x, y): (u32, u32), (w, h): (u32, u32), rgba: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 4,
            },
        );
    }
}

/// The probes as one cube array, layer `i` probe `i`'s six faces in D3D's
/// order with its mip chain; a probe off the first one's size or level
/// count stays black, and an empty list gives one black cube.
fn probe_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    probes: &[vcod_common::mapworld::ProbeCube],
) -> wgpu::TextureView {
    let levels_of = |p: &vcod_common::mapworld::ProbeCube| p.faces.first().map_or(1, Vec::len);
    let first = probes.iter().find(|p| p.size > 0 && p.faces.len() == 6);
    let (size, levels) = first.map_or((1, 1), |p| (p.size, levels_of(p)));
    // A cube array holds six layers a probe; mp_cargoship has more probes
    // than wgpu's default 256 layers hold (cod4-world.md 9.7). Probes past
    // the device's limit stay unbound.
    let probes = &probes[..probes
        .len()
        .min(device.limits().max_texture_array_layers as usize / 6)];
    let layers = probes.len().max(1) as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("entity reflection probes"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6 * layers,
        },
        mip_level_count: levels as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let fits = |p: &&vcod_common::mapworld::ProbeCube| {
        p.size == size && p.faces.len() == 6 && p.faces.iter().all(|f| f.len() == levels)
    };
    for (i, p) in probes.iter().enumerate().filter(|(_, p)| fits(p)) {
        for (face, chain) in p.faces.iter().enumerate() {
            for (level, px) in chain.iter().enumerate() {
                let side = (size >> level).max(1);
                if px.len() != (4 * side * side) as usize {
                    continue;
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: (6 * i + face) as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    px,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * side),
                        rows_per_image: Some(side),
                    },
                    wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::CubeArray),
        ..Default::default()
    })
}

/// A one-level texture from tightly packed rows.
fn plain_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    data: &[u8],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
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
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            data,
        )
        .create_view(&Default::default())
}

/// dynamic_model.wgsl with the CoD2 and CoD4 model techniques, the
/// deforms, `vc_shadows`' receiver tail, CoD4's sun shadow map and the
/// image samplers.
pub(crate) const DYNAMIC_WGSL: &str = concat!(
    include_str!("dynamic_model.wgsl"),
    include_str!("cod2_model.wgsl"),
    include_str!("cod4_flag.wgsl"),
    include_str!("noise.wgsl"),
    include_str!("deform.wgsl"),
    include_str!("shadow_receive.wgsl"),
    include_str!("cod4_sunshadow_recv.wgsl"),
    include_str!("tex_samplers.wgsl")
);

/// viewmodel.wgsl with the deforms and the image samplers; the view model
/// reads no sun shadow map.
pub(crate) const VIEWMODEL_WGSL: &str = concat!(
    include_str!("viewmodel.wgsl"),
    include_str!("cod2_model.wgsl"),
    include_str!("cod4_sunshadow_none.wgsl"),
    include_str!("noise.wgsl"),
    include_str!("deform.wgsl"),
    include_str!("tex_samplers.wgsl")
);

pub(crate) fn wgpu_blend_factor(f: &BlendFactor) -> wgpu::BlendFactor {
    match f {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::DstColor => wgpu::BlendFactor::Dst,
        BlendFactor::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        BlendFactor::SrcColor => wgpu::BlendFactor::Src,
        BlendFactor::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        BlendFactor::SrcAlphaSaturate => wgpu::BlendFactor::SrcAlphaSaturated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::shader::{AlphaFunc, BlendFactor, WaveForm};

    const OBJECTIVE: &str = "{ { map $texturename rgbGen lightingDiffuse } \
        { map $whiteimage rgbgen constLighting ( 0.20 0.158 0.062 ) blendFunc add } \
        { map $whiteimage rgbgen constLighting ( 0.8 0.632 0.252  ) \
          alphaGen wave sin -0.1 1.1 0 .5 blendFunc GL_SRC_ALPHA GL_ONE } }";

    /// A typed skin retail cannot build draws its default shader: a
    /// missing `.stype`, a missing image, a slash after the `@` (0x4fc0c0).
    /// Every typed skin of every stock model builds.
    #[test]
    fn a_typed_skin_retail_cannot_build_draws_the_default_shader() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let lib = vcod_common::shader::ShaderLib::load(&fs).materials();
        let mut w = WarnSet::new();
        let is_default = |sh: &Material| sh.name == "<default>";
        // metal_env@staffcar.tga ships with its template
        assert!(!is_default(&load(
            &fs,
            &lib,
            "metal_env@staffcar.tga",
            &mut w
        )));
        assert!(is_default(&load(
            &fs,
            &lib,
            "no_such_type@staffcar.tga",
            &mut w
        )));
        assert!(is_default(&load(
            &fs,
            &lib,
            "metal_env@no_such_skin.tga",
            &mut w
        )));
        assert!(is_default(&load(
            &fs,
            &lib,
            "metal_env@sub/staffcar.tga",
            &mut w
        )));
        // an untyped skin keeps the implicit one, image or not
        assert_eq!(
            load(&fs, &lib, "no_such_skin.tga", &mut w).name,
            "no_such_skin.tga"
        );
        let d = load(&fs, &lib, "no_such_type@x.tga", &mut w);
        assert_eq!(d.stages[0].rgb_gen, RgbGen::IdentityLighting);
        assert_eq!(
            d.stages[0].bundles[0].image,
            ImageRef::Path(DEFAULT_IMAGE.into())
        );
        assert!(!is_lit(&d));

        let mut failed = Vec::new();
        for name in fs.list_prefix("xmodel/") {
            let Ok(m) = vcod_common::model::load(&fs, &name["xmodel/".len()..]) else {
                continue;
            };
            for skin in m.materials().iter().filter(|s| is_typed_skin(s)) {
                if template(&fs, skin, &mut w).is_none() {
                    failed.push(format!("{name}: {skin}"));
                }
            }
        }
        failed.sort();
        failed.dedup();
        assert!(failed.is_empty(), "{failed:#?}");
    }

    #[test]
    fn objective_is_a_lit_base_and_two_additive_stages() {
        let sh = from_stype("objective@x.dds", OBJECTIVE, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages.len(), 3);
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::LightingDiffuse);
        assert_eq!(sh.stages[0].blend, None);
        assert_eq!(sh.stages[1].bundles[0].image, ImageRef::White);
        assert_eq!(
            sh.stages[1].rgb_gen,
            RgbGen::ConstLighting([0.20, 0.158, 0.062])
        );
        assert_eq!(
            sh.stages[1].blend,
            Some((BlendFactor::One, BlendFactor::One))
        );
        assert_eq!(
            sh.stages[2].blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::One))
        );
        let AlphaGen::Wave(w) = &sh.stages[2].alpha_gen else {
            panic!("alphaGen wave");
        };
        assert_eq!((w.form.clone(), w.base, w.amp), (WaveForm::Sin, -0.1, 1.1));
        assert!(is_lit(&Material::from_shader(&sh)));
    }

    #[test]
    fn unlit_types_carry_no_lit_stage() {
        let incomplete =
            "{ { map $textureName rgbgen constLighting ( 0.40 0.316 0.124 ) blendFunc add } }";
        let sh = from_stype("objective_incomplete@x", incomplete, &mut WarnSet::new()).unwrap();
        assert!(!is_lit(&Material::from_shader(&sh)));
        let light = "{ surfaceparm glass surfaceparm noshadow { map $texturename rgbGen identityLighting } }";
        let sh = from_stype("glass_light@x", light, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::IdentityLighting);
        assert!(!is_lit(&Material::from_shader(&sh)));
        assert!(is_lit(&implicit("plain.dds")));
    }

    #[test]
    fn an_env_material_reflects_under_its_masked_texture() {
        let env = "{ surfaceparm metal nomipmaps \
            { map textures/sfx/environmap_1day.jpg tcgen environment rgbgen lightingdiffuse } \
            { map $texturename blendFunc GL_ONE_MINUS_SRC_ALPHA GL_SRC_ALPHA rgbgen lightingdiffuse } }";
        let sh = from_stype("metal_env@x", env, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages.len(), 2);
        assert_eq!(sh.stages[0].bundles[0].tc_gen, TcGen::Environment);
        assert_eq!(
            sh.stages[0].bundles[0].image,
            ImageRef::Path("textures/sfx/environmap_1day.jpg".into())
        );
        assert_eq!(sh.stages[0].blend, None);
        // the skin's alpha picks texture over reflection
        assert_eq!(
            sh.stages[1].blend,
            Some((BlendFactor::OneMinusSrcAlpha, BlendFactor::SrcAlpha))
        );
        assert_eq!(sh.sort_value(), SORT_OPAQUE);
    }

    #[test]
    fn masked_glow_stages_test_depth_equal() {
        let masked = "{ { map $texturename alphafunc GE128 rgbGen lightingDiffuse } \
            { depthfunc equal map $whiteimage rgbgen constLighting ( 0.20 0.158 0.062 ) blendFunc add } }";
        let sh = from_stype("objective_masked@x", masked, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages[0].depth_func, DepthFunc::LessEqual);
        assert_eq!(sh.stages[1].depth_func, DepthFunc::Equal);
    }

    #[test]
    fn masked_types_alpha_test() {
        let masked = "{ surfaceparm cloth { map $texturename rgbGen lightingDiffuse alphaFunc GE128 depthWrite } }";
        let sh = from_stype("cloth_masked@x", masked, &mut WarnSet::new()).unwrap();
        assert_eq!(sh.stages[0].alpha_func, Some(AlphaFunc::Ge128));
        assert_eq!(implicit("x").stages[0].alpha_func, None);
    }

    /// The stock templates the snapshot entities name: pickup glow,
    /// objective pulse, the unlit objective and the lamp glass.
    #[test]
    fn stock_entity_templates_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let lib = vcod_common::shader::ShaderLib::load(&fs).materials();
        let mut warns = WarnSet::new();
        let pickup = load(&fs, &lib, "pickup@medpack.dds", &mut warns);
        assert_eq!(pickup.stages.len(), 2);
        assert!(matches!(pickup.stages[1].rgb_gen, RgbGen::Wave(_)));
        assert_eq!(
            pickup.stages[1].blend,
            Some((BlendFactor::One, BlendFactor::One))
        );
        assert_eq!(
            load(&fs, &lib, "objective@tnt_block.dds", &mut warns)
                .stages
                .len(),
            3
        );
        let incomplete = load(&fs, &lib, "objective_incomplete@tnt_block.dds", &mut warns);
        assert!(!is_lit(&incomplete));
        assert!(!is_lit(&load(
            &fs,
            &lib,
            "glass_light@bulb.dds",
            &mut warns
        )));
        let plain = load(&fs, &lib, "metal@staffcartread.dds", &mut warns);
        assert_eq!(plain.stages.len(), 1);
        assert!(is_lit(&plain));
    }

    /// The S&D bomb's `timer_face.dds` takes `skins/timer_face`: the face,
    /// then the clamped hand turning under an alpha test, both lit. A skin
    /// with no block stays the implicit one.
    #[test]
    fn bomb_timer_face_takes_its_skins_block() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let lib = vcod_common::shader::ShaderLib::load(&fs).materials();
        let mut warns = WarnSet::new();
        let model = vcod_common::model::load(&fs, "mp_bomb1").unwrap();
        assert!(model.materials().iter().any(|m| m == "timer_face.dds"));
        let sh = load(&fs, &lib, "timer_face.dds", &mut warns);
        assert_eq!(sh.stages.len(), 2);
        let hand = &sh.stages[1];
        assert_eq!(
            hand.bundles[0].image,
            ImageRef::Path("gfx/misc/timer_hand.dds".into())
        );
        assert_eq!(hand.bundles[0].clamp, Clamp::BOTH);
        assert!(!hand.bundles[0].tcmods.is_empty());
        assert_eq!(hand.alpha_func, Some(AlphaFunc::Ge128));
        assert!(is_lit(&sh));
        assert!(crate::renderer::stage_animated(&sh, 1));
        let plain = load(&fs, &lib, "no_block_here.dds", &mut warns);
        assert_eq!(plain.stages.len(), 1);
        assert_eq!(
            plain.stages[0].bundles[0].image,
            ImageRef::Path(TEXTURE_NAME.into())
        );
    }

    #[test]
    fn skin_type_is_the_name_before_the_at() {
        assert_eq!(skin_type("Pickup@medpack.dds").as_deref(), Some("pickup"));
        assert_eq!(
            skin_type("skins/objective@a.tga").as_deref(),
            Some("objective")
        );
        assert_eq!(skin_type("plain.dds"), None);
    }

    /// More probes than a device's array layers hold (mp_cargoship's,
    /// under wgpu's default 256 layers) build a valid array of the ones
    /// that fit, not an invalid texture every entity bind group fails on
    /// (the map sweep's 2012 validation errors).
    #[test]
    fn probes_past_the_layer_limit_leave_a_valid_array() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no adapter; skipped");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };
        let cube = vcod_common::mapworld::ProbeCube {
            size: 1,
            faces: vec![vec![vec![9; 4]]; 6],
        };
        let n = device.limits().max_texture_array_layers as usize / 6 + 5;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _view = probe_array(&device, &queue, &vec![cube; n]);
        let err = pollster::block_on(scope.pop());
        assert!(err.is_none(), "{err:?}");
    }

    /// Every stage of every material a stock CoD2 xmodel names draws with
    /// one of [`COD2_MODEL_PIPELINES`], and each of them is used. Needs
    /// `COD2_DIR`.
    #[test]
    fn cod2_model_pipelines_cover_every_stock_model() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        vcod_cod2::model::register();
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let mut skins = std::collections::BTreeSet::new();
        for name in fs.list_prefix("xmodel/") {
            let name = name.strip_prefix("xmodel/").unwrap();
            if let Ok(m) = vcod_common::model::load(&fs, name) {
                skins.extend(m.lods.iter().flat_map(|l| l.materials.iter().cloned()));
            }
        }
        assert!(skins.len() > 500, "{} model materials", skins.len());
        let mut used = [false; COD2_MODEL_PIPELINES.len()];
        let mut w = WarnSet::new();
        for skin in &skins {
            let sh = load(&fs, &lib, skin, &mut w);
            for (i, st) in sh.stages.iter().enumerate() {
                let depth_write = sh.classify_stage(i).is_some_and(|c| c.depth_write);
                let key = (
                    st.blend.clone(),
                    depth_write,
                    st.depth_func,
                    entity_cull(&sh),
                );
                let at = COD2_MODEL_PIPELINES.iter().position(|k| *k == key);
                assert!(at.is_some(), "{skin} stage {i}: {key:?}");
                used[at.unwrap()] = true;
            }
        }
        assert!(used.iter().all(|u| *u), "unused keys: {used:?}");
    }
}
