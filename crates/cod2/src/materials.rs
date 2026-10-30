//! CoD2 materials compiled into the neutral [`Material`] the renderer draws
//! today (docs/research/cod2-materials.md): each binary material, the lit
//! technique its set names (4.1) and that technique's first pass statemap
//! applied to its `stateBits` (3, 6) become one CoD1-style stage, the
//! colour map modulated by the lightmap for the `lmap*` techniques and by
//! the vertex colour, with the blend, alpha test, depth and cull the state
//! gives. Whether it fogs comes from the fogged technique a fogged scene
//! draws it with instead (4.2). `textures` keeps every map by semantic; the stage draw ignores the
//! normal, specular and detail maps, the sun, `detailScale` and texture
//! scroll. An `lmap` technique also carries its switches in
//! `Material::phong` ([`phong_of`]) and the sky its cube's path in
//! `sky.env`, which the client's CoD2 pipelines draw instead.
//!
//! Images are named `images/<name>.iwi`, which `assets::load_path_image`
//! decodes (cod2-materials.md 10).

use crate::material::{
    Fog, MaterialFile, StateBits, StateMap, Technique as Tech, TechniqueSet, TechniqueType,
    TextureDef, TextureImage, semantic,
};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use vcod_common::asset_source::{Asset, AssetKind, AssetSource};
use vcod_common::material::{
    AlphaFunc, AlphaGen, BlendFactor, Bundle, Clamp, Cull, DepthFunc, ImageRef, Material,
    MaterialLib, ModelLight, PhongLit, PolygonOffset, ProbeLit, RgbGen, SORT_DECAL, SORT_OPAQUE,
    SkyParms, Stage, SurfaceBits, TcGen, Technique, TexEnv, TexSemantic, TextureBinding, WaterLit,
};

/// Every stock material: the library the renderer draws.
pub fn load_lib(fs: &dyn AssetSource) -> Result<MaterialLib> {
    Ok(Compiled::find(fs, "")?.lib)
}

/// Every material of a source compiled ([`compile_all`]), the name unused
/// (`""`). [`load_lib`] builds it through [`Asset::find`], not the cached
/// [`vcod_common::asset_source::load`]: the renderer owns its library, and
/// the source's cache would hold a second copy for the level.
pub struct Compiled {
    pub lib: MaterialLib,
    pub report: Report,
}

impl Asset for Compiled {
    const KIND: AssetKind = AssetKind::Cod2Materials;

    fn find(src: &dyn AssetSource, _name: &str) -> Result<Self> {
        let (lib, report) = compile_all(src)?;
        Ok(Compiled { lib, report })
    }
}

/// What [`compile_all`] met on the way.
#[derive(Debug, Default)]
pub struct Report {
    /// Materials compiled.
    pub compiled: usize,
    /// Files that did not parse, with the reason; they are left out.
    pub errors: Vec<String>,
    /// Materials drawn by the colour-map-only fallback, by technique set.
    pub fallbacks: BTreeMap<String, usize>,
    /// Unique warnings (also in the library's `warn_count`).
    pub warnings: BTreeSet<String>,
}

/// The material files under `materials/`, without the technique tree.
fn material_paths(fs: &dyn AssetSource) -> Vec<String> {
    const SKIP: [&str; 4] = ["techniques/", "techniquesets/", "statemaps/", "shaders/"];
    fs.list("materials", "")
        .into_iter()
        .filter(|p| {
            let rel = &p["materials/".len()..];
            !rel.is_empty() && !rel.ends_with('/') && !SKIP.iter().any(|s| rel.starts_with(s))
        })
        .collect()
}

/// Parses each text file once, by name; a missing or bad one is `None`.
struct Cache<T> {
    dir: &'static str,
    ext: &'static str,
    parse: fn(&str) -> Result<T>,
    got: HashMap<String, Option<T>>,
}

impl<T> Cache<T> {
    fn new(dir: &'static str, ext: &'static str, parse: fn(&str) -> Result<T>) -> Self {
        Self {
            dir,
            ext,
            parse,
            got: HashMap::new(),
        }
    }

    fn get(&mut self, fs: &dyn AssetSource, name: &str, warn: &mut BTreeSet<String>) -> Option<&T> {
        let (dir, ext, parse) = (self.dir, self.ext, self.parse);
        self.got
            .entry(name.to_string())
            .or_insert_with(|| {
                let path = format!("{dir}{name}{ext}");
                let Some(raw) = fs.read(&path) else {
                    warn.insert(format!("{path} is missing"));
                    return None;
                };
                match parse(&String::from_utf8_lossy(&raw)) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn.insert(format!("{path}: {e:#}"));
                        None
                    }
                }
            })
            .as_ref()
    }
}

/// Compiles every material under `materials/`; fails only when there is
/// none, so not a CoD2 file system.
pub fn compile_all(fs: &dyn AssetSource) -> Result<(MaterialLib, Report)> {
    let paths = material_paths(fs);
    ensure!(!paths.is_empty(), "no CoD2 materials under materials/");
    let mut sets = Cache::new("materials/techniquesets/", ".techset", TechniqueSet::parse);
    let mut techs = Cache::new("materials/techniques/", ".tech", Tech::parse);
    let mut maps = Cache::new("materials/statemaps/", ".sm", StateMap::parse);
    let mut report = Report::default();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let file = match fs.read(&path).map(|b| MaterialFile::parse(&b)) {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                report.errors.push(format!("{path}: {e:#}"));
                continue;
            }
            None => {
                report.errors.push(format!("{path}: unreadable"));
                continue;
            }
        };
        let w = &mut report.warnings;
        let set = sets.get(fs, &file.technique_set, w);
        let model_light = set.map_or(ModelLight::None, model_light);
        let pass = match set {
            None => Err(format!("technique set {} not found", file.technique_set)),
            Some(set) => match pick_technique(set) {
                None => Ok(None),
                Some((ty, tech_name)) => {
                    let tech_name = tech_name.to_string();
                    let sm = techs
                        .get(fs, &tech_name, w)
                        .and_then(|t| t.passes.first())
                        .map(|p| p.state_map.clone());
                    let bits = match sm.and_then(|sm| maps.get(fs, &sm, w).map(|m| (sm, m))) {
                        Some((sm, m)) => m.apply(file.state_bits).unwrap_or_else(|g| {
                            w.insert(format!(
                                "{}: no rule in statemap {sm} group {g}, using the material's state",
                                file.name
                            ));
                            file.state_bits
                        }),
                        None => file.state_bits,
                    };
                    // the scene's fog picks the fogged variant of the same
                    // type (4.1); its passes decide whether the draw fogs
                    let mut fogs_in = |fog: Fog| {
                        let name = fog_technique(set, ty, fog)?;
                        let tech = techs.get(fs, name, w)?;
                        let first = tech.passes.first()?;
                        let bits = maps
                            .get(fs, &first.state_map, w)
                            .and_then(|m| m.apply(file.state_bits).ok())
                            .unwrap_or(file.state_bits);
                        Some(technique_fogs(tech, bits))
                    };
                    let (linear, exp) = (fogs_in(Fog::Linear), fogs_in(Fog::Exp));
                    if linear != exp {
                        w.insert(format!(
                            "{}: linear fog {linear:?} and exp fog {exp:?} differ, drawing exp's",
                            file.name
                        ));
                    }
                    Ok(Some(LitPass {
                        ty,
                        technique: tech_name,
                        bits,
                        fogs: exp.or(linear).unwrap_or(false),
                    }))
                }
            },
        };
        let (mut m, fallback) = match pass {
            Ok(p) => compile(&file, p.as_ref(), w),
            Err(why) => {
                w.insert(format!("{}: {why}", file.name));
                (fallback_material(&file, file.state_bits, w), true)
            }
        };
        // a model draws the set's probe technique, a world surface its
        // lightmap one (cod2-materials.md 9.2)
        m.model_light = model_light;
        m.cookie = vcod_common::material::CookieRole {
            caster: set.is_some_and(|s| s.get(TechniqueType::ShadowcookieCaster).is_some()),
            receiver: set.is_some_and(|s| s.get(TechniqueType::ShadowcookieReceiver).is_some()),
            sun_caster: false,
        };
        if model_light == ModelLight::Probe {
            m.probe = set
                .and_then(|s| s.get(TechniqueType::LightprobeNoFog))
                .and_then(|t| probe_of(&file, t));
            m.smodel = set
                .and_then(|s| s.get(TechniqueType::LightprobeSmodelCachedNoFog))
                .and_then(|t| smodel_of(&file, t));
        }
        if fallback {
            *report
                .fallbacks
                .entry(file.technique_set.clone())
                .or_default() += 1;
        }
        report.compiled += 1;
        let image = m.texture(TexSemantic::Color).and_then(|i| match i {
            ImageRef::Path(p) => Some(p.clone()),
            _ => None,
        });
        out.push((m, image));
    }
    let fallbacks: usize = report.fallbacks.values().sum();
    log::info!(
        "cod2 materials: {} compiled, {} errors, {} fallbacks over {} sets, {} warnings",
        report.compiled,
        report.errors.len(),
        fallbacks,
        report.fallbacks.len(),
        report.warnings.len()
    );
    for (set, n) in &report.fallbacks {
        match fallback_use(set) {
            FallbackUse::Visible(why) => {
                log::warn!(
                    "cod2 materials: {n} in technique set {set} draw the colour map only: {why}"
                )
            }
            FallbackUse::Elsewhere(why) | FallbackUse::Unused(why) => {
                log::debug!("cod2 materials: technique set {set} ({n}) has no stage form: {why}")
            }
        }
    }
    for e in &report.errors {
        log::warn!("cod2 materials: {e}");
    }
    for w in &report.warnings {
        log::debug!("cod2 materials: {w}");
    }
    let lib = MaterialLib::from_materials(out, report.warnings.len());
    Ok((lib, report))
}

/// What default play does with a set whose technique has no stage form
/// (cod2-materials.md 9.9..9.13); only [`FallbackUse::Visible`] sets show
/// their colour-map stand-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackUse {
    /// vcod draws it another way: a renderer pass's code material, which no
    /// surface names, or a set the model path draws itself.
    Elsewhere(&'static str),
    /// Drawn by nothing a stock MP level, effect, weapon or menu reaches,
    /// or only by a pass that is off by default.
    Unused(&'static str),
    /// Drawn in default play and not ported.
    Visible(&'static str),
}

/// [`FallbackUse`] of technique set `set`.
pub fn fallback_use(set: &str) -> FallbackUse {
    use FallbackUse::*;
    match set {
        "glow_setup" | "glow_apply_bloom" | "glow_apply_sky_bleed" => {
            Elsewhere("r_glow's pass, vcod's cod2_glow")
        }
        s if s.starts_with("filter_symmetric_") => {
            Elsewhere("the glow and blur passes' filter, vcod's cod2_glow")
        }
        "shell_shock" => Elsewhere("the shellshock blend, vcod's screen_blend"),
        "shadowcookieblur" => Elsewhere("the shadow cookies' blur, vcod's cod2_cookie"),
        "shadowclear" => Elsewhere("the floatZ clear quad; vcod's depth copy needs none"),
        "shadowcookieoverlay" => Unused("sc_showOverlay only"),
        "shadowoverlay" => Unused("a set no pak holds, named by no code slot"),
        "objective" => Elsewhere("models draw objective_base's pulse; no stock world surface"),
        "passthru_vertalpha" => Elsewhere("feedbackblend, the menu blur, vcod's cod2_glow"),
        "passthru_alpha" => Unused("feedbackreplace, RB_TouchAllImages' warm-up draw"),
        "color_channel_mixer" => Unused("r_showFbColorDebug and sc_showDebug only"),
        "clear_alpha_stencil" => Unused("the point light pass's colourless clear"),
        "stencil_plane" => Unused("R_AddCmdSetStencilPlanes, which nothing calls"),
        "grain_overlay" => Unused("overlay_grain, named by nothing in MP"),
        "distortion" => Unused("no stock MP map, weapon or impact effect loads one"),
        "water" => Elsewhere("water_l_sun without a water def; vcod's cod2_water draws the rest"),
        _ => Visible("not ported"),
    }
}

/// The pass a material draws in: its lit technique and that pass's state.
#[derive(Clone, Debug, PartialEq)]
pub struct LitPass {
    pub ty: TechniqueType,
    /// The no-fog technique; its name picks the stage family and the
    /// phong switches, which the fogged variants share.
    pub technique: String,
    /// The material's `stateBits` after the no-fog pass's statemap.
    pub bits: StateBits,
    /// Whether the draw fogs in a fogged scene ([`technique_fogs`] of the
    /// `ty.with_fog` technique). Every stock material answers the same
    /// for linear and exp fog, so one answer serves both; a scene without
    /// fog draws the no-fog technique, which never fogs.
    pub fogs: bool,
}

/// `stateBits` word 0's fog enable (`D3DRS_FOGENABLE`, cod2-materials.md 3).
pub const FOG_BIT: u32 = 0x2000_0000;

/// The technique a material of base type `ty` draws with under the
/// scene's `fog`: `RB_RenderDrawSurfList` adds `RB_FogOffset()` to the
/// no-fog type (4.1). `None` when the set names none there, and retail
/// skips the draw.
pub fn fog_technique(set: &TechniqueSet, ty: TechniqueType, fog: Fog) -> Option<&str> {
    set.get(ty.with_fog(fog))
}

/// Whether a draw through `tech` fogs, `bits` being the material's
/// `stateBits` after the technique's first pass statemap: the fog bit
/// survives it (`fogged.sm` passes the material's through, `nofog.sm`
/// clears it), or the technique carries a `multiply_fog` pass, a
/// multiply decal's additive fog fix-up (cod2-materials.md 6, 9.4).
pub fn technique_fogs(tech: &Tech, bits: StateBits) -> bool {
    bits[0] & FOG_BIT != 0 || tech.passes.iter().any(|p| p.state_map == "multiply_fog")
}

/// The technique the scene draws a material with: the lit pass's lightmap
/// or light probe type, else the emissive pass, the sky pass, or unlit
/// (the 2D and fullbright path), each the no-fog type the scene's fog
/// offsets ([`fog_technique`], 4.1). The emissive pass is the feathered
/// type, as retail's dx9 path runs it under its default `r_zFeather 1`
/// (cod2-materials.md 9.5); a set without one falls back to plain
/// emissive. `None` when the set has none of these (tools), so the
/// material draws nothing.
pub fn pick_technique(set: &TechniqueSet) -> Option<(TechniqueType, &str)> {
    use TechniqueType as T;
    [
        T::LightmapNoFog,
        T::LightprobeNoFog,
        T::EmissiveFeatheredNoFog,
        T::EmissiveNoFog,
        T::Sky,
        T::UnlitNoFog,
    ]
    .into_iter()
    .find_map(|t| set.get(t).map(|n| (t, n)))
}

/// How a model drawn with `set` is lit: the `ambient_*` sets' static
/// technique is `lprobe_smc_amb*` (their entity one `vertcol_shaded`), any
/// other set with a `lightprobe` technique is probe lit (cod2-materials.md
/// 9.2).
fn model_light(set: &TechniqueSet) -> ModelLight {
    let cached = set.get(TechniqueType::LightprobeSmodelCachedNoFog);
    let probe = set.get(TechniqueType::LightprobeNoFog);
    if cached.is_some_and(|t| t.starts_with("lprobe_smc_amb")) {
        ModelLight::Ambient
    } else if probe == Some("objective_base") {
        ModelLight::Objective
    } else if probe.is_some() {
        ModelLight::Probe
    } else {
        ModelLight::None
    }
}

/// How a technique draws in the stage path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// `lmap*`: colour times vertex colour times the lightmap.
    Lightmap,
    /// Probe-lit models, unlit, 2D and effects: colour times vertex colour.
    Vertex,
    Sky,
    /// `water_l_sun*`: the renderer's water pipeline (9.12).
    Water,
    /// Anything else: the colour map only.
    Fallback,
}

fn family(technique: &str) -> Family {
    const VERTEX: [&str; 9] = [
        "lprobe",
        "zfeather",
        "vertcol_simple",
        "vertcol_shaded",
        "vertcol_mul",
        "effect_add",
        "textured_simple",
        "unlit",
        // the colour map times `particleCloudColor`, which the fx_cloud
        // pass hands in as the vertex colour (cod2-materials.md 9.14)
        "particle_cloud",
    ];
    if technique.starts_with("lmap") {
        Family::Lightmap
    } else if technique == "sky" {
        Family::Sky
    } else if technique.starts_with("water_l_sun") {
        Family::Water
    } else if VERTEX.iter().any(|p| technique.starts_with(p)) {
        Family::Vertex
    } else {
        Family::Fallback
    }
}

/// Compiles one material for `pass`; the bool is true when it took the
/// colour-map-only fallback. `pass` `None` draws nothing.
pub fn compile(
    file: &MaterialFile,
    pass: Option<&LitPass>,
    warn: &mut BTreeSet<String>,
) -> (Material, bool) {
    let mut m = base_material(file, file.state_bits);
    let Some(pass) = pass else {
        return (m, false);
    };
    if m.surface.nodraw {
        return (m, false);
    }
    match family(&pass.technique) {
        Family::Water if let Some(w) = water_of(file) => {
            // the stage keeps the colour-map draw for every path that
            // reads stages; the CoD2 world pipeline draws `water`
            let mut m = fallback_material(file, pass.bits, warn);
            m.nofog = !pass.fogs;
            m.water = Some(w);
            (m, false)
        }
        Family::Water | Family::Fallback => {
            warn.insert(format!(
                "{}: technique {} has no stage form, drawing the colour map only",
                file.name, pass.technique
            ));
            let mut m = fallback_material(file, pass.bits, warn);
            m.nofog = !pass.fogs;
            (m, true)
        }
        Family::Sky => {
            m.technique = Technique::Sky;
            // the sky type takes no fog offset (4.1)
            m.nofog = true;
            m.surface.sky = true;
            m.sort = 2.0;
            // `env` carries the cube's image path: the renderer's CoD2 sky
            // samples it along the view direction (cod2-materials.md 9, Sky)
            let (cube, _) = color_image(file);
            m.sky = Some(SkyParms {
                env: match cube {
                    ImageRef::Path(p) => p,
                    _ => String::new(),
                },
                cloud_height: 512.0,
            });
            (m, false)
        }
        f => {
            let st = state_of(pass.bits, &file.name, warn);
            let mut stage = stage_for(&st, color_image(file), f == Family::Lightmap);
            stage.rgb_gen = RgbGen::ExactVertex;
            stage.alpha_gen = AlphaGen::Vertex;
            finish(&mut m, file, &st, stage, f == Family::Lightmap);
            m.nofog = !pass.fogs;
            if f == Family::Lightmap {
                m.phong = phong_of(file, &pass.technique);
            }
            // mul.hlsl folds the vertex alpha into the colour; the blend
            // (Zero, SrcColor) then multiplies (cod2-materials.md 9)
            m.vertex_multiply = pass.technique.starts_with("vertcol_mul") && st.blend.is_some();
            // z_feather.hlsl scales the depth gap by featherParms.x (9.5)
            if pass.technique.starts_with("zfeather") {
                m.feather = Some(file.constant("featherParms").map_or(0.0, |c| c.literal[0]));
            }
            (m, false)
        }
    }
}

/// The switches of an `lmap` technique whose name is `lmap` plus any of
/// the `_a`, `_s` and `_dtl` suffixes (cod2-materials.md 9); `None` for
/// another (`lmap_add`).
pub fn phong_of(file: &MaterialFile, technique: &str) -> Option<PhongLit> {
    let mut parts = technique.split('_');
    if parts.next() != Some("lmap") {
        return None;
    }
    let mut lit = PhongLit {
        specular: false,
        detail: false,
        alpha: false,
        detail_scale: [1.0, 1.0],
        cod4: None,
    };
    for p in parts {
        match p {
            "a" => lit.alpha = true,
            "s" => lit.specular = true,
            "dtl" => lit.detail = true,
            _ => return None,
        }
    }
    if let Some(c) = file.constant("detailScale") {
        lit.detail_scale = [c.literal[0], c.literal[1]];
    }
    Some(lit)
}

/// The switches of an entity's `lprobe` technique: `lprobe` plus any of
/// `_flag`, `_a`, `_s`, `_dtl` and `_scroll` (cod2-materials.md 9.4, 9.7);
/// `None` for another family.
pub fn probe_of(file: &MaterialFile, technique: &str) -> Option<ProbeLit> {
    switches(file, technique.strip_prefix("lprobe")?)
}

/// A static model's `lprobe_smc*` technique's switches, as [`probe_of`]
/// reads an entity's; `None` for any other cached technique (the ambient
/// sets' `lprobe_smc_amb*`).
pub fn smodel_of(file: &MaterialFile, technique: &str) -> Option<ProbeLit> {
    let lit = switches(file, technique.strip_prefix("lprobe_smc")?)?;
    (!lit.flag && !lit.scroll).then_some(lit)
}

/// The `_`-separated switches after a technique's family name.
fn switches(file: &MaterialFile, rest: &str) -> Option<ProbeLit> {
    let mut lit = ProbeLit {
        specular: false,
        alpha: false,
        detail: false,
        scroll: false,
        flag: false,
        detail_scale: [1.0, 1.0],
    };
    if !rest.is_empty() && !rest.starts_with('_') {
        return None;
    }
    for p in rest.split('_').skip(1) {
        match p {
            "a" => lit.alpha = true,
            "s" => lit.specular = true,
            "dtl" => lit.detail = true,
            "scroll" => lit.scroll = true,
            "flag" => lit.flag = true,
            _ => return None,
        }
    }
    if let Some(c) = file.constant("detailScale") {
        lit.detail_scale = [c.literal[0], c.literal[1]];
    }
    Some(lit)
}

/// `water_l_sun`'s inputs: `waterColor` (0 without one), the
/// `cubicEnvMap` and the `normalMap`'s water def (cod2-materials.md 9.12);
/// `None` without a water def.
pub fn water_of(file: &MaterialFile) -> Option<WaterLit> {
    let def = file.textures.iter().find_map(|t| match &t.image {
        TextureImage::Water(w) => Some(w),
        TextureImage::Image(_) => None,
    })?;
    Some(WaterLit {
        color: file.constant("waterColor").map_or([0.0; 4], |c| c.literal),
        cube: file
            .texture("cubicEnvMap")
            .and_then(TextureDef::image_name)
            .map_or_else(String::new, image_path),
        width: def.texture_width,
        lengths: [def.horizontal_world_length, def.vertical_world_length],
        amplitude: def.amplitude,
        wind_speed: def.wind_speed,
        wind_direction: def.wind_direction,
    })
}

/// The colour-map-only draw: the colour map (or a `waterColor` constant on
/// white) under the state's blend, depth and cull.
fn fallback_material(
    file: &MaterialFile,
    bits: StateBits,
    warn: &mut BTreeSet<String>,
) -> Material {
    let mut m = base_material(file, bits);
    if m.surface.nodraw {
        return m;
    }
    let st = state_of(bits, &file.name, warn);
    let color = color_image(file);
    let white = color.0 == ImageRef::White;
    let mut stage = stage_for(&st, color, false);
    if white && let Some(c) = file.constant("waterColor") {
        stage.rgb_gen = RgbGen::Const([c.literal[0], c.literal[1], c.literal[2]]);
        stage.alpha_gen = AlphaGen::Const(c.literal[3]);
    }
    finish(&mut m, file, &st, stage, false);
    m
}

/// A material's own fields, drawing nothing yet.
fn base_material(file: &MaterialFile, bits: StateBits) -> Material {
    let sf = file.surface_flags;
    Material {
        name: file.name.clone(),
        technique: Technique::None,
        sort: SORT_OPAQUE,
        surface: SurfaceBits {
            sky: sf & 0x4 != 0 || file.game_flags & 0x8 != 0,
            nodraw: sf & 0x80 != 0,
            trans: false,
            water: file.contents & 0x20 != 0,
            nonsolid: sf & 0x4000 != 0,
            nolightmap: sf & 0x400 != 0,
            nodlight: sf & 0x20000 != 0,
        },
        cull: Cull::Back,
        depth_bias: None,
        nofog: bits[0] & FOG_BIT == 0,
        nopicmip: false,
        nomipmaps: false,
        sky: None,
        sunfile: None,
        deforms: Vec::new(),
        sprite: None,
        textures: textures_of(file),
        stages: Vec::new(),
        dlight: None,
        sort_images: None,
        dropped_stages: 0,
        model_axes: None,
        phong: None,
        vertex_multiply: false,
        falloff: None,
        premultiplied: false,
        eye_offset: None,
        reflex: None,
        model_light: ModelLight::None,
        probe: None,
        smodel: None,
        feather: None,
        feather_view_depth: false,
        floatz: None,
        entity_cull: false,
        distortion: None,
        cookie: Default::default(),
        water: None,
    }
}

/// `images/<name>.iwi`, the file `Image_LoadFromFile` reads (10).
pub fn image_path(name: &str) -> String {
    format!("images/{name}.iwi")
}

fn semantic_of(t: &TextureDef) -> Option<TexSemantic> {
    Some(match t.name.as_str() {
        "colorMap" => TexSemantic::Color,
        "detailMap" => TexSemantic::Detail,
        "normalMap" => TexSemantic::Normal,
        "specularMap" => TexSemantic::Specular,
        _ => return None,
    })
}

fn textures_of(file: &MaterialFile) -> Vec<TextureBinding> {
    file.textures
        .iter()
        .filter_map(|t| {
            Some(TextureBinding {
                semantic: semantic_of(t)?,
                image: ImageRef::Path(image_path(t.image_name()?)),
            })
        })
        .collect()
}

/// The image the stage draws and its texture def: `colorMap`, else the
/// first image, else white.
fn color_image(file: &MaterialFile) -> (ImageRef, Option<&TextureDef>) {
    let def = file
        .texture("colorMap")
        .filter(|t| t.image_name().is_some())
        .or_else(|| file.textures.iter().find(|t| t.image_name().is_some()));
    match def.and_then(|t| Some((t, t.image_name()?))) {
        Some((t, n)) => (ImageRef::Path(image_path(n)), Some(t)),
        None => (ImageRef::White, None),
    }
}

/// The state a pass draws with, read off its two words (3).
#[derive(Clone, Debug, PartialEq)]
pub struct PassState {
    pub blend: Option<(BlendFactor, BlendFactor)>,
    pub alpha_func: Option<AlphaFunc>,
    pub cull: Cull,
    pub depth_func: DepthFunc,
    pub depth_write: bool,
    pub depth_bias: Option<PolygonOffset>,
    pub fog: bool,
}

fn factor(v: u32) -> Option<BlendFactor> {
    use BlendFactor as F;
    Some(match v {
        1 => F::Zero,
        2 => F::One,
        3 => F::SrcColor,
        4 => F::OneMinusSrcColor,
        5 => F::SrcAlpha,
        6 => F::OneMinusSrcAlpha,
        7 => F::DstAlpha,
        8 => F::OneMinusDstAlpha,
        9 => F::DstColor,
        10 => F::OneMinusDstColor,
        _ => return None,
    })
}

/// Maps a pass's `stateBits` onto the stage state; what the stage path has
/// no form for (blend ops other than add, the blend factor, front-face
/// cull, stencil, colour masks, the separate alpha blend) warns once and
/// takes the nearest.
pub fn state_of(bits: StateBits, name: &str, warn: &mut BTreeSet<String>) -> PassState {
    let [w0, w1] = bits;
    let op = (w0 >> 8) & 7;
    let blend = if op == 0 {
        None
    } else {
        if op != 1 {
            warn.insert(format!("{name}: blend op {op} drawn as add"));
        }
        let (s, d) = (w0 & 0xf, (w0 >> 4) & 0xf);
        let (fs, fd) = (factor(s), factor(d));
        if fs.is_none() || fd.is_none() {
            warn.insert(format!("{name}: blend factors {s}/{d} drawn as one"));
        }
        match (
            fs.unwrap_or(BlendFactor::One),
            fd.unwrap_or(BlendFactor::One),
        ) {
            (BlendFactor::One, BlendFactor::Zero) => None,
            pair => Some(pair),
        }
    };
    let alpha_func = match w0 & 0x3800 {
        0x1000 => Some(AlphaFunc::Gt0),
        0x2000 => Some(AlphaFunc::Lt128),
        0x3000 => Some(AlphaFunc::Ge128),
        _ => None,
    };
    let cull = match w0 & 0xc000 {
        0x8000 => Cull::Back,
        0xc000 => {
            warn.insert(format!("{name}: front-face cull drawn two-sided"));
            Cull::None
        }
        _ => Cull::None,
    };
    let (depth_func, test) = match w1 & 0xe {
        8 => (DepthFunc::Equal, true),
        0xc => (DepthFunc::Always, true),
        2 => (DepthFunc::Always, false),
        _ => (DepthFunc::LessEqual, true),
    };
    let depth_bias = match w1 & 0x30 {
        0x10 => Some(PolygonOffset::Single),
        0x20 | 0x30 => Some(PolygonOffset::Double),
        _ => None,
    };
    PassState {
        blend,
        alpha_func,
        cull,
        depth_func,
        // D3D9's disabled z test writes no depth either
        depth_write: test && w1 & 1 != 0,
        depth_bias,
        fog: w0 & FOG_BIT != 0,
    }
}

fn stage_for(st: &PassState, color: (ImageRef, Option<&TextureDef>), lightmap: bool) -> Stage {
    let (image, def) = color;
    let clamp = def.map_or(Clamp::NONE, |t| Clamp {
        s: t.sampler.clamp_u(),
        t: t.sampler.clamp_v(),
    });
    let bundle = |image, clamp| Bundle {
        image,
        anim: None,
        clamp,
        tcmods: Vec::new(),
        tc_gen: TcGen::Base,
        env: TexEnv::Modulate,
    };
    let mut bundles = vec![bundle(image, clamp)];
    if lightmap {
        bundles.push(bundle(ImageRef::Lightmap, Clamp::NONE));
    }
    Stage {
        bundles,
        blend: st.blend.clone(),
        depth_write: Some(st.depth_write),
        alpha_func: st.alpha_func.clone(),
        depth_func: st.depth_func,
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
    }
}

/// CoD1 sort values that keep CoD2's `sortKey` order (2): unblended draws
/// opaque, blended keys up to 16 (world decals) just under `SORT_DECAL`,
/// the effect and 2D keys above it from 15 up.
pub fn sort_of(sort_key: u8, blended: bool) -> f32 {
    let k = f32::from(sort_key);
    match (blended, sort_key) {
        (false, _) => SORT_OPAQUE,
        (true, 0..=16) => SORT_DECAL - (16.0 - k) * 0.01,
        (true, _) => 15.0 + (k - 17.0) * 0.1,
    }
}

fn finish(m: &mut Material, file: &MaterialFile, st: &PassState, stage: Stage, lightmap: bool) {
    let (_, def) = color_image(file);
    m.technique = Technique::Stages;
    m.sort = sort_of(file.sort_key, st.blend.is_some());
    m.cull = st.cull;
    m.depth_bias = st.depth_bias;
    m.nofog = !st.fog;
    if let Some(t) = def {
        m.nopicmip = t.semantic == semantic::TWO_D;
        // a sampler naming its own filter with no mip filter has no mips
        m.nomipmaps = t.sampler.0 & 0xf != 0 && t.sampler.mip() == crate::material::Filter::None;
    }
    if lightmap {
        m.textures.push(TextureBinding {
            semantic: TexSemantic::Lightmap,
            image: ImageRef::Lightmap,
        });
    }
    m.sort_images = Some(stage.bundles.iter().map(|b| b.image.clone()).collect());
    m.stages = vec![stage];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{ConstantDef, SamplerState, TextureImage};

    /// A statemap of the stock `fogged.sm` shape: GT0 for an untested alpha
    /// blend, depth writes only without blending, the rest passed through.
    const FOGGED: &str = "
        alphaTest { mtlAlphaTest == Always && mtlBlendOp == Add && mtlSrcBlend == SrcAlpha && mtlDestBlend == InvSrcAlpha: GT0; default: passthrough; }
        blendFunc { default: passthrough; }
        separateAlphaBlendFunc { mtlBlendOp == Disable: Disable, One, Zero; default: Add, InvDestAlpha, One; }
        cullFace { default: passthrough; }
        depthTest { default: passthrough; }
        depthWrite { mtlBlendOp == Disable: Enable; default: Disable; }
        colorWrite { default: Enable, Enable; }
        fog { default: passthrough; }
        polygonOffset { default: passthrough; }
        stencil { default: passthrough; }
        wireframe { default: passthrough; }
    ";

    fn file(bits: StateBits, sort_key: u8, textures: Vec<TextureDef>) -> MaterialFile {
        MaterialFile {
            name: "test".into(),
            ref_image_name: String::new(),
            hash_index: 0,
            sorted_index: 0,
            game_flags: 3,
            sort_key,
            atlas_rows: 1,
            atlas_columns: 1,
            max_deform_move: 0.0,
            deform_flags: 0,
            usage: 0,
            tool_flags: 0,
            locale: 0,
            auto_tex_scale: [0; 2],
            tess_size: 0.0,
            surface_flags: 0,
            contents: 1,
            state_bits: bits,
            technique_set: "phong_replace".into(),
            textures,
            constants: Vec::new(),
        }
    }

    fn tex(name: &str, sem: u8, sampler: u8, image: &str) -> TextureDef {
        TextureDef {
            name: name.into(),
            sampler: SamplerState(sampler),
            semantic: sem,
            unused: [0; 2],
            image: TextureImage::Image(image.into()),
        }
    }

    fn lit(technique: &str, bits: StateBits) -> LitPass {
        LitPass {
            ty: TechniqueType::LightmapNoFog,
            technique: technique.into(),
            bits,
            fogs: bits[0] & FOG_BIT != 0,
        }
    }

    #[test]
    fn opaque_phong_is_colour_times_lightmap() {
        let bits = [0x2812_8812, 5];
        let f = file(
            bits,
            3,
            vec![
                tex("colorMap", semantic::COLOR, 0x30, "brick"),
                tex("normalMap", semantic::NORMAL, 0, "~brick_n-gggr"),
                tex("specularMap", semantic::SPECULAR, 0, "brick_s"),
            ],
        );
        let sm = StateMap::parse(FOGGED).unwrap();
        let pass = lit("lmap_s", sm.apply(bits).unwrap());
        let mut w = BTreeSet::new();
        let (m, fb) = compile(&f, Some(&pass), &mut w);
        assert!(!fb && w.is_empty(), "{w:?}");
        assert_eq!(m.technique, Technique::Stages);
        assert_eq!(m.sort, SORT_OPAQUE);
        assert_eq!(m.cull, Cull::Back);
        assert!(!m.nofog && m.depth_bias.is_none());
        let st = &m.stages[0];
        assert_eq!(st.blend, None);
        assert_eq!(st.alpha_func, None);
        assert_eq!(st.depth_write, Some(true));
        assert_eq!(st.depth_func, DepthFunc::LessEqual);
        assert_eq!(
            st.bundles[0].image,
            ImageRef::Path("images/brick.iwi".into())
        );
        assert_eq!(st.bundles[0].clamp, Clamp::BOTH);
        assert_eq!(st.bundles[1].image, ImageRef::Lightmap);
        assert_eq!(st.bundles[1].env, TexEnv::Modulate);
        assert_eq!(
            m.texture(TexSemantic::Normal),
            Some(&ImageRef::Path("images/~brick_n-gggr.iwi".into()))
        );
        assert_eq!(m.texture(TexSemantic::Lightmap), Some(&ImageRef::Lightmap));
        let p = m.phong.expect("lmap_s is a lit phong");
        assert!(p.specular && !p.detail && !p.alpha);
    }

    #[test]
    fn phong_switches_follow_the_technique_name() {
        let mut f = file([0x2812_8812, 5], 3, Vec::new());
        f.constants.push(ConstantDef {
            name: "detailScale".into(),
            literal: [8.0, 4.0, 0.0, 0.0],
        });
        let p = phong_of(&f, "lmap_a_s_dtl").unwrap();
        assert!(p.alpha && p.specular && p.detail);
        assert_eq!(p.detail_scale, [8.0, 4.0]);
        let p = phong_of(&f, "lmap").unwrap();
        assert!(!p.alpha && !p.specular && !p.detail);
        assert_eq!(phong_of(&f, "lmap_add"), None);
        assert_eq!(phong_of(&f, "lprobe_s"), None);
    }

    #[test]
    fn sky_names_its_cube_as_the_env() {
        let mut f = file(
            [0x2812_8812, 5],
            4,
            vec![tex("colorMap", 1, 0, "sky_toujane")],
        );
        f.game_flags = 8;
        let pass = LitPass {
            ty: TechniqueType::Sky,
            technique: "sky".into(),
            bits: f.state_bits,
            fogs: true,
        };
        let (m, _) = compile(&f, Some(&pass), &mut BTreeSet::new());
        assert_eq!(m.technique, Technique::Sky);
        assert_eq!(m.sky.unwrap().env, "images/sky_toujane.iwi");
        assert_eq!(m.phong, None);
        // the sky type takes no fog offset, whatever the pass says
        assert!(m.nofog);
    }

    #[test]
    fn decal_blend_takes_gt0_and_no_depth_write_from_the_statemap() {
        // SrcAlpha InvSrcAlpha, test Always, polygon offset 1, no depth write
        let bits = [0x2812_8965, 0x14];
        let sm = StateMap::parse(FOGGED).unwrap();
        let out = sm.apply(bits).unwrap();
        let f = file(bits, 9, vec![tex("colorMap", 2, 0, "decal")]);
        let (m, _) = compile(&f, Some(&lit("lmap", out)), &mut BTreeSet::new());
        let st = &m.stages[0];
        assert_eq!(
            st.blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha))
        );
        assert_eq!(st.alpha_func, Some(AlphaFunc::Gt0));
        assert_eq!(st.depth_write, Some(false));
        assert_eq!(m.depth_bias, Some(PolygonOffset::Single));
        assert!(m.sort > SORT_OPAQUE && m.sort <= SORT_DECAL);
    }

    #[test]
    fn multiply_2d_and_foliage_states() {
        let mut w = BTreeSet::new();
        // effect_multiply: Zero SrcColor
        let mul = state_of([0x2812_9131, 0x14], "m", &mut w);
        assert_eq!(mul.blend, Some((BlendFactor::Zero, BlendFactor::SrcColor)));
        // 2D: alpha blend, GT0, cull none, depth test off
        let hud = state_of([0x2812_5165, 2], "h", &mut w);
        assert_eq!(hud.alpha_func, Some(AlphaFunc::Gt0));
        assert_eq!(hud.cull, Cull::None);
        assert_eq!(hud.depth_func, DepthFunc::Always);
        assert!(!hud.depth_write);
        // foliage: GE128 and LT128, opaque
        let ge = state_of([0x2812_b012, 5], "f", &mut w);
        assert_eq!(ge.alpha_func, Some(AlphaFunc::Ge128));
        assert_eq!(ge.blend, None);
        let lt = state_of([0x2812_a012, 5], "f", &mut w);
        assert_eq!(lt.alpha_func, Some(AlphaFunc::Lt128));
        assert!(w.is_empty(), "{w:?}");
        // a subtract op and front cull warn and draw their nearest
        let odd = state_of([0x2812_c222, 5], "odd", &mut w);
        assert_eq!(odd.blend, Some((BlendFactor::One, BlendFactor::One)));
        assert_eq!(odd.cull, Cull::None);
        assert_eq!(w.len(), 2);
    }

    /// `vertcol_mul` marks the multiply; a blend layer is no multiply.
    #[test]
    fn multiply_decals_are_marked() {
        let mut w = BTreeSet::new();
        let f = file([0x0812_8931, 0x14], 11, Vec::new());
        let pass = LitPass {
            ty: TechniqueType::EmissiveNoFog,
            technique: "vertcol_mul".into(),
            bits: f.state_bits,
            fogs: true,
        };
        let (m, _) = compile(&f, Some(&pass), &mut w);
        assert!(m.vertex_multiply && m.phong.is_none() && !m.nofog);
        assert_eq!(
            m.stages[0].blend,
            Some((BlendFactor::Zero, BlendFactor::SrcColor))
        );
        let f = file([0x0812_9965, 0x14], 8, Vec::new());
        let (m, _) = compile(&f, Some(&lit("lmap_a", f.state_bits)), &mut w);
        assert!(!m.vertex_multiply && m.phong.is_some_and(|p| p.alpha));
    }

    #[test]
    fn unknown_technique_falls_back_to_the_colour_map() {
        let mut f = file([0x2812_8812, 5], 3, Vec::new());
        f.constants.push(ConstantDef {
            name: "waterColor".into(),
            literal: [0.1, 0.2, 0.3, 0.5],
        });
        let mut w = BTreeSet::new();
        let (m, fb) = compile(&f, Some(&lit("water_l_sun", f.state_bits)), &mut w);
        assert!(fb);
        assert_eq!(w.len(), 1);
        assert_eq!(m.stages[0].bundles[0].image, ImageRef::White);
        assert_eq!(m.stages[0].rgb_gen, RgbGen::Const([0.1, 0.2, 0.3]));
        // no technique for the scene's passes: nothing to draw
        let (none, fb) = compile(&f, None, &mut w);
        assert!(!fb && none.technique == Technique::None && none.stages.is_empty());
    }

    /// A `water_l_sun` material with a water def carries `WaterLit` and is
    /// no fallback; the stage keeps the colour map (cod2-materials.md 9.12).
    #[test]
    fn water_carries_its_def_and_cube() {
        use crate::material::{SamplerState, WaterDef};
        let tex = |name: &str, image| TextureDef {
            name: name.into(),
            sampler: SamplerState(0),
            semantic: 2,
            unused: [0; 2],
            image,
        };
        let mut f = file(
            [0x2812_8812, 5],
            3,
            vec![
                tex("colorMap", TextureImage::Image("case64blue".into())),
                tex(
                    "cubicEnvMap",
                    TextureImage::Image("elalamein_water_ft".into()),
                ),
                tex(
                    "normalMap",
                    TextureImage::Water(WaterDef {
                        texture_width: 64,
                        horizontal_world_length: 37.0,
                        vertical_world_length: 37.0,
                        amplitude: 0.06,
                        wind_speed: 76.0,
                        wind_direction: [1.0, 0.0],
                        map: None,
                    }),
                ),
            ],
        );
        f.constants.push(ConstantDef {
            name: "waterColor".into(),
            literal: [0.2, 0.3, 0.4, 1.0],
        });
        let mut w = BTreeSet::new();
        let (m, fb) = compile(&f, Some(&lit("water_l_sun", f.state_bits)), &mut w);
        assert!(!fb && w.is_empty() && !m.nofog, "{w:?}");
        let water = m.water.expect("water");
        assert_eq!(water.cube, "images/elalamein_water_ft.iwi");
        assert_eq!(water.color, [0.2, 0.3, 0.4, 1.0]);
        assert_eq!(
            (water.width, water.lengths, water.wind_direction),
            (64, [37.0; 2], [1.0, 0.0])
        );
        assert_eq!(m.stages.len(), 1);
    }

    /// The scene's fog picks the set's fogged type, and that technique's
    /// first statemap decides: `fogged` keeps the material's own fog bit,
    /// `nofog` clears it, and a `multiply_fog` pass fogs a decal whose
    /// passes both clear it (cod2-materials.md 4.1, 6).
    #[test]
    fn the_scene_fog_picks_the_fogged_technique() {
        let set = TechniqueSet::parse(
            "\"lightmap no fog\": lmap;\n\"lightmap linear fog\": lmap_lfog;\n\
             \"lightmap exp fog\": lmap_efog;\n\
             \"unlit no fog\":\n\"unlit linear fog\":\n\"unlit exp fog\": vertcol_simple;\n",
        )
        .unwrap();
        use TechniqueType as T;
        assert_eq!(
            fog_technique(&set, T::LightmapNoFog, Fog::Off),
            Some("lmap")
        );
        assert_eq!(
            fog_technique(&set, T::LightmapNoFog, Fog::Linear),
            Some("lmap_lfog")
        );
        assert_eq!(
            fog_technique(&set, T::LightmapNoFog, Fog::Exp),
            Some("lmap_efog")
        );
        assert_eq!(
            fog_technique(&set, T::UnlitNoFog, Fog::Exp),
            Some("vertcol_simple")
        );
        assert_eq!(fog_technique(&set, T::EmissiveNoFog, Fog::Exp), None);

        let pass = |sm: &str| {
            format!(
                "{{ stateMap \"{sm}\"; vertexShader 2.0 \"a\" {{ }} pixelShader 2.0 \"a\" {{ }} }}"
            )
        };
        let fogged = Tech::parse(&pass("fogged")).unwrap();
        let mul = Tech::parse(&(pass("nofog") + &pass("multiply_fog"))).unwrap();
        let fog_sm = StateMap::parse(FOGGED).unwrap();
        let nofog_sm = StateMap::parse(
            &FOGGED.replace("fog { default: passthrough; }", "fog { default: Disable; }"),
        )
        .unwrap();
        let on = [0x2812_8812, 5];
        let off = [0x0812_8812, 5];
        assert!(technique_fogs(&fogged, fog_sm.apply(on).unwrap()));
        assert!(!technique_fogs(&fogged, fog_sm.apply(off).unwrap()));
        assert!(!technique_fogs(&fogged, nofog_sm.apply(on).unwrap()));
        assert!(technique_fogs(&mul, nofog_sm.apply(on).unwrap()));

        // compiled: a fogging pass draws fogged, any other does not
        let f = file(on, 3, Vec::new());
        let mut w = BTreeSet::new();
        let mut p = lit("lmap", nofog_sm.apply(on).unwrap());
        p.fogs = true;
        assert!(!compile(&f, Some(&p), &mut w).0.nofog);
        p.fogs = false;
        assert!(compile(&f, Some(&p), &mut w).0.nofog);
    }

    #[test]
    fn sort_keeps_cod2_order() {
        assert_eq!(sort_of(3, false), SORT_OPAQUE);
        assert!(sort_of(8, true) < sort_of(16, true));
        assert!(sort_of(16, true) <= SORT_DECAL);
        assert!(sort_of(26, true) < sort_of(27, true));
        assert!(sort_of(17, true) > SORT_DECAL);
    }
}
