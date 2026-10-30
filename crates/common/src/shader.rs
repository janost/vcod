//! Q3-style shader scripts: the type model, the block splitter and the stage
//! parser shared by every later parsing pass.

use std::collections::{HashMap, HashSet};

use crate::asset_source::{Asset, AssetKind, AssetSource};
pub use crate::material::{
    AlphaFunc, AlphaGen, AnimSpec, BlendFactor, Bulge, Bundle, Clamp, ClassedStage, DEFAULT_IMAGE,
    DLIGHT_IMAGE, Deform, DeformNormal, DepthFunc, DlightDraw, DrawClass, Flap, ImageRef,
    NOISE_PERM, NOISE_TABLE, PolygonOffset, RgbGen, SORT_ADDITIVE, SORT_BANNER, SORT_BLEND,
    SORT_BOATHULL, SORT_DECAL, SORT_OCEAN, SORT_OPAQUE, SORT_SEETHROUGH, SORT_UNDERWATER,
    SORT_WATER, SkyParms, Sprite, Stage, SurfaceBits, TcGen, TcMod, TexEnv, Wave, WaveForm,
    bundle_affine, bundle_turb, has_animated_tcmods, noise_get4f, rgb_wave_value, wave_value,
};
use crate::material::{
    Cull, Material, MaterialLib, Technique, TexSemantic, TextureBinding, norm_key,
};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shader {
    pub name: String,
    pub stages: Vec<Stage>,
    /// `perLight` stages: never in the base pass, drawn once per dynamic
    /// light by the dlight pass (`Shader::dlight_draw`; shader doc 3).
    pub per_light: Vec<Stage>,
    /// Stage blocks killed by a failed `requires` or an unsupported tcGen;
    /// diagnostic only.
    pub dropped_stages: usize,
    pub two_sided: bool,
    pub sort: Option<f32>,
    /// `polygonOffset`, `polygonOffset2` or `polygonOffsetConst`.
    pub polygon_offset: Option<PolygonOffset>,
    /// `nofog`: shader flag 0x400 (CoDMP.exe 0x4faf39).
    pub nofog: bool,
    pub nopicmip: bool,
    pub nomipmaps: bool,
    pub sky: Option<SkyParms>,
    pub sunfile: Option<String>,
    /// The shader's `deformVertexes` records in script order, at most
    /// [`MAX_SHADER_DEFORMS`]; the runtime runs them one after another on
    /// the same vertices (CoDMP.exe 0x5149b0; shader doc 8).
    pub deforms: Vec<Deform>,
    /// `deformVertexes autosprite` / `autosprite2`: camera-facing quads
    /// (0x514080 / 0x514330; shader doc 8).
    pub sprite: Option<Sprite>,
    pub surface: SurfaceBits,
    /// Set by the renderer on a static prop's batch: the model's left and
    /// up axes in the world, so `tcGen environment` reflects in the model's
    /// frame on world-baked vertices (shader doc 8, "Static props").
    pub model_axes: Option<[[f32; 3]; 2]>,
}

/// Retail's deform record cap (`MAX_SHADER_DEFORMS`, CoDMP.exe 0x4f9d20).
pub const MAX_SHADER_DEFORMS: usize = 3;

/// The line-break token [`tokens`] emits and block bodies keep.
pub const NEWLINE: &str = "\n";

/// Script tokens: comments (`//` to end of line, `/* */` across lines) gone,
/// glued braces split off as standalone `{`/`}` tokens, every line break a
/// [`NEWLINE`] (a `/* */` comment spanning lines yields none, as Q3's
/// `COM_ParseExt` skips it). Slices point into `text`; delimiters are ASCII,
/// so token boundaries are always char edges.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    std::iter::from_fn(move || {
        loop {
            match bytes.get(i) {
                None => return None,
                Some(b'/') if bytes.get(i + 1) == Some(&b'/') => {
                    i = bytes[i + 2..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(bytes.len(), |p| i + 2 + p);
                }
                Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                    i = text[i + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |p| i + 2 + p + 2);
                }
                // a line break is a token: keyword arguments end with their line
                Some(b'\n') => {
                    i += 1;
                    return Some(NEWLINE);
                }
                Some(c) if c.is_ascii_whitespace() => i += 1,
                Some(b'{') | Some(b'}') => {
                    let tok = &text[i..i + 1];
                    i += 1;
                    return Some(tok);
                }
                Some(_) => {
                    let start = i;
                    while let Some(&c) = bytes.get(i) {
                        // a comment opener ends the word even without whitespace
                        if c.is_ascii_whitespace() || c == b'{' || c == b'}' {
                            break;
                        }
                        if c == b'/' && matches!(bytes.get(i + 1), Some(b'/' | b'*')) {
                            break;
                        }
                        i += 1;
                    }
                    return Some(&text[start..i]);
                }
            }
        }
    })
}

/// Split a shader script into `(name, body)` pairs in file order.
///
/// Names are lowercased with `\` normalized to `/`; bodies are verbatim token
/// slices of `text` with comments gone and braces flattened into the stream as
/// standalone `{`/`}` tokens (same approach as `assets.rs`'s brace handling).
/// Anonymous top-level blocks come out with an empty name; duplicate names are
/// both returned (last-wins dedupe happens at map level). An unterminated
/// block at EOF is dropped.
pub fn split_blocks(text: &str) -> Vec<(String, Vec<&str>)> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut pending: Option<String> = None;
    let mut name: Option<String> = None;
    let mut body: Vec<&str> = Vec::new();
    for tok in tokens(text) {
        match tok {
            NEWLINE if depth == 0 || body.last().is_none_or(|&t| t == NEWLINE) => {}
            "{" => {
                depth += 1;
                if depth == 1 {
                    name = pending.take();
                } else {
                    body.push(tok);
                }
            }
            "}" => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if body.last() == Some(&NEWLINE) {
                            body.pop();
                        }
                        out.push((name.take().unwrap_or_default(), std::mem::take(&mut body)));
                    } else {
                        body.push(tok);
                    }
                }
            }
            _ if depth == 0 => pending = Some(tok.to_lowercase().replace('\\', "/")),
            _ => body.push(tok),
        }
    }
    out
}

/// Sort-name to draw-order mapping, evaluated at parse time; numeric tokens
/// parse directly and win. CoD 1.1's own enum, not RTCW's: the name table
/// the shader parser walks at CoDMP.exe 0x4fa5e9..0x4fa85a (shader doc 5).
pub fn map_sort_token(tok: &str) -> Option<f32> {
    if let Ok(v) = tok.parse::<f32>() {
        return Some(v);
    }
    match tok.to_ascii_lowercase().as_str() {
        "portal" => Some(1.0),
        "sky" => Some(2.0),
        "ocean" => Some(SORT_OCEAN),
        "boathull" => Some(SORT_BOATHULL),
        "opaque" => Some(SORT_OPAQUE),
        "decal" => Some(SORT_DECAL),
        "seethrough" => Some(SORT_SEETHROUGH),
        "banner" => Some(SORT_BANNER),
        "underwater" => Some(SORT_UNDERWATER),
        "water" => Some(SORT_WATER),
        "corona" => Some(12.0),
        "innerblend" => Some(13.0),
        "outerblend" => Some(14.0),
        "blend" => Some(SORT_BLEND),
        "blend2" => Some(16.0),
        "blend3" => Some(17.0),
        "blend4" => Some(18.0),
        "additive" => Some(SORT_ADDITIVE),
        "nearest" => Some(22.0),
        _ => None,
    }
}

impl Shader {
    /// Whether some bundle takes `tcGen environment`.
    pub fn reflects(&self) -> bool {
        self.stages
            .iter()
            .flat_map(|st| &st.bundles)
            .any(|b| b.tc_gen == TcGen::Environment)
    }

    /// Explicit `sort` wins; otherwise CoDMP.exe's FinishShader defaults
    /// (0x4fb9d0..): sky, then polygon offset, then a blended stage 0
    /// (see-through while it writes depth, additive onto `GL_ONE`, blend
    /// otherwise), then opaque.
    pub fn sort_value(&self) -> f32 {
        self.sort.unwrap_or_else(|| {
            if self.sky.is_some() {
                2.0
            } else if self.polygon_offset.is_some() {
                SORT_DECAL
            } else {
                match self.stages.first() {
                    Some(st @ Stage { blend: Some(_), .. }) if st.writes_depth() => SORT_SEETHROUGH,
                    Some(Stage {
                        blend: Some((_, BlendFactor::One)),
                        ..
                    }) => SORT_ADDITIVE,
                    Some(Stage { blend: Some(_), .. }) => SORT_BLEND,
                    _ => SORT_OPAQUE,
                }
            }
        })
    }

    /// What the dlight pass draws for this block once per dynamic light
    /// touching it (CoDMP.exe 0x500d3a, 0x500a10; shader doc 3). `None`
    /// when the pass skips it: sky and `nodlight` surfaces, sorts past
    /// opaque. Otherwise its `perLight` stages, the automatic copy of a
    /// texture-times-lightmap stage (0x4fb3c0), or `dlightshader`.
    pub fn dlight_draw(&self) -> Option<DlightDraw> {
        if self.sky.is_some()
            || self.surface.sky
            || self.surface.nodlight
            || self.sort_value() > SORT_OPAQUE
        {
            return None;
        }
        if !self.per_light.is_empty() {
            return Some(DlightDraw::Stages(self.per_light.clone()));
        }
        Some(match self.dlight_copy() {
            Some(st) => DlightDraw::Stages(vec![st]),
            None => DlightDraw::Blob,
        })
    }

    /// 0x4fb3c0: a block left with one stage, texture in bundle 0 and the
    /// lightmap in bundle 1, both modulating, under one of five blends,
    /// gets a `perLight` copy with bundle 1 bound to the dlight image,
    /// blended `GL_SRC_ALPHA GL_ONE` when the stage's source factor is
    /// `GL_SRC_ALPHA` and `GL_ONE GL_ONE` otherwise.
    fn dlight_copy(&self) -> Option<Stage> {
        let mut st = match self.stages.as_slice() {
            [st] => st.clone(),
            [a, b] => collapse(a, b)?,
            _ => return None,
        };
        let [b0, b1] = st.bundles.as_mut_slice() else {
            return None;
        };
        if b0.image == ImageRef::Lightmap
            || b1.image != ImageRef::Lightmap
            || b1.env != TexEnv::Modulate
        {
            return None;
        }
        use BlendFactor::*;
        let src_alpha = match st.blend {
            None | Some((One, Zero)) | Some((One, One)) => false,
            Some((SrcAlpha, One)) | Some((SrcAlpha, OneMinusSrcAlpha)) => true,
            _ => return None,
        };
        b1.image = ImageRef::Path(DLIGHT_IMAGE.to_string());
        st.blend = Some(if src_alpha {
            (SrcAlpha, One)
        } else {
            (One, One)
        });
        Some(st)
    }

    /// The first stage as R_SortNewShader (0x4fb790) reads it, after the
    /// texture-and-lightmap pair collapses (`collapse`): its bundle images
    /// order shaders of one sort (shader doc 5).
    pub fn sorting_stage(&self) -> Option<Stage> {
        match self.stages.as_slice() {
            [] => None,
            [a] => Some(a.clone()),
            [a, b, ..] => Some(collapse(a, b).unwrap_or_else(|| a.clone())),
        }
    }

    /// The polygon offset the shader draws at: its keyword's, else the
    /// single one for a shader sorted decal.
    pub fn depth_bias(&self) -> Option<PolygonOffset> {
        // exact compare is sound: both sides come from the same 4.0 literal
        // or its exact decimal parse
        self.polygon_offset
            .or((self.sort == Some(SORT_DECAL)).then_some(PolygonOffset::Single))
    }

    /// Per-stage draw facts; `None` when `idx` is out of range.
    pub fn classify_stage(&self, idx: usize) -> Option<ClassedStage> {
        let st = self.stages.get(idx)?;
        let class = match st.blend {
            None => DrawClass::Opaque,
            Some((_, BlendFactor::One)) | Some((_, BlendFactor::SrcAlphaSaturate)) => {
                DrawClass::Additive
            }
            Some(_) => DrawClass::Blend,
        };
        // exact compare is sound: both sides come from the same 4.0 literal
        // or its exact decimal parse
        Some(ClassedStage {
            class,
            depth_write: st.writes_depth(),
            bias: self.depth_bias().is_some(),
            two_sided: self.two_sided,
        })
    }
}

impl Material {
    /// Compiles a parsed script: its FinishShader sort and polygon offset,
    /// its dlight draw, its sorting stage's images, its stages as written.
    pub fn from_shader(sh: &Shader) -> Material {
        let technique = if sh.sky.is_some() {
            Technique::Sky
        } else if sh.stages.is_empty() {
            Technique::None
        } else {
            Technique::Stages
        };
        let sorting = sh.sorting_stage();
        let mut textures = Vec::new();
        if let Some(st) = &sorting {
            if let Some(b) = st
                .bundles
                .iter()
                .find(|b| matches!(b.image, ImageRef::Path(_)))
            {
                textures.push(TextureBinding {
                    semantic: TexSemantic::Color,
                    image: b.image.clone(),
                });
            }
            if st.bundles.iter().any(|b| b.image == ImageRef::Lightmap) {
                textures.push(TextureBinding {
                    semantic: TexSemantic::Lightmap,
                    image: ImageRef::Lightmap,
                });
            }
        }
        Material {
            name: sh.name.clone(),
            technique,
            sort: sh.sort_value(),
            surface: sh.surface.clone(),
            cull: if sh.two_sided { Cull::None } else { Cull::Back },
            depth_bias: sh.depth_bias(),
            nofog: sh.nofog,
            nopicmip: sh.nopicmip,
            nomipmaps: sh.nomipmaps,
            sky: sh.sky.clone(),
            sunfile: sh.sunfile.clone(),
            deforms: sh.deforms.clone(),
            sprite: sh.sprite,
            textures,
            stages: sh.stages.clone(),
            dlight: sh.dlight_draw(),
            sort_images: sorting
                .map(|st| st.bundles.iter().take(2).map(|b| b.image.clone()).collect()),
            dropped_stages: sh.dropped_stages,
            model_axes: sh.model_axes,
            phong: None,
            vertex_multiply: false,
            falloff: None,
            premultiplied: false,
            eye_offset: None,
            reflex: None,
            model_light: crate::material::ModelLight::None,
            probe: None,
            smodel: None,
            feather: None,
            feather_view_depth: false,
            floatz: None,
            entity_cull: false,
            distortion: None,
            cookie: crate::material::CookieRole::default(),
            water: None,
        }
    }
}

/// Retail's `<default>` shader compiled ([`default_shader`]).
pub fn default_material() -> Material {
    Material::from_shader(&default_shader())
}

/// Retail's `<default>` shader (CoDMP.exe 0x4fd780): one opaque stage of
/// `*default` that writes depth, its rgbGen left 0, which 0x4ffa60's default
/// case fills with identityLight. `R_FindShader` returns it for a skin it
/// cannot build (shader doc 8, "Model skin scripts").
pub fn default_shader() -> Shader {
    Shader {
        name: "<default>".to_string(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path(DEFAULT_IMAGE.to_string()),
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
            rgb_gen: RgbGen::IdentityLighting,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    }
}

/// CollapseMultitexture (0x4fb530) for the pairs the dlight copy can
/// take: an unblended single-bundle stage followed by a single-bundle
/// `filter` stage becomes one opaque stage, texture first, lightmap second.
/// Every other row of its table (0x5404d8) leaves a blend or a texture
/// environment 0x4fb3c0 refuses, so they stay uncollapsed here.
fn collapse(a: &Stage, b: &Stage) -> Option<Stage> {
    if a.bundles.len() != 1 || b.bundles.len() != 1 {
        return None;
    }
    if a.blend.is_some() || b.blend != Some((BlendFactor::DstColor, BlendFactor::Zero)) {
        return None;
    }
    let same_gens = a.rgb_gen == b.rgb_gen && a.alpha_gen == b.alpha_gen;
    if !same_gens && (b.rgb_gen != RgbGen::Identity || b.alpha_gen != AlphaGen::Identity) {
        return None;
    }
    let mut out = a.clone();
    let mut second = b.bundles[0].clone();
    second.env = TexEnv::Modulate;
    if out.bundles[0].takes_lightmap_coords() {
        std::mem::swap(&mut out.bundles[0], &mut second);
    }
    out.bundles.push(second);
    Some(out)
}

/// Dedupes warnings by `(shader name, message)` so a malformed script shape
/// logs once no matter how many shaders hit it.
pub struct WarnSet {
    seen: HashSet<String>,
    noted: HashSet<String>,
}

impl WarnSet {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
            noted: HashSet::new(),
        }
    }

    /// A debug line, not a warning: script damage retail's parser warns
    /// about too and vcod answers the same way, or a keyword vcod reads
    /// and deliberately approximates. Not counted by [`len`](Self::len).
    pub fn note_once(&mut self, name: &str, msg: &str) {
        if self.noted.insert(format!("{name}\0{msg}")) {
            log::debug!("{name}: {msg}");
        }
    }

    pub fn warn_once(&mut self, name: &str, msg: &str) {
        if self.seen.insert(format!("{name}\0{msg}")) {
            log::warn!("{name}: {msg}");
        }
    }

    /// Unique warnings fired so far.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Every `(shader name, message)` fired so far, in no order.
    pub fn messages(&self) -> impl Iterator<Item = (&str, &str)> {
        self.seen.iter().filter_map(|k| k.split_once('\0'))
    }

    #[cfg(test)]
    fn fired(&self, name: &str, msg: &str) -> bool {
        self.seen.contains(&format!("{name}\0{msg}"))
    }

    #[cfg(test)]
    fn noted(&self, name: &str, msg: &str) -> bool {
        self.noted.contains(&format!("{name}\0{msg}"))
    }
}

impl Default for WarnSet {
    fn default() -> Self {
        Self::new()
    }
}

/// The genfunc names of CoDMP.exe 0x4f1980; `None` for any other.
pub fn wave_form(tok: &str) -> Option<WaveForm> {
    Some(match tok.to_ascii_lowercase().as_str() {
        "sin" => WaveForm::Sin,
        "square" => WaveForm::Square,
        "triangle" => WaveForm::Triangle,
        "sawtooth" => WaveForm::Sawtooth,
        "invsawtooth" | "inversesawtooth" => WaveForm::InverseSawtooth,
        "noise" => WaveForm::Noise,
        _ => return None,
    })
}

/// A wave's genfunc and four numbers. Missing numbers read 0, as retail's
/// parser leaves them (0x4f1a40). `None` for an unknown genfunc.
pub fn parse_wave(args: &[&str]) -> Option<Wave> {
    let form = wave_form(args.first()?)?;
    let num = |i: usize| args.get(i + 1).map_or(0.0, |t| fnum(t));
    Some(Wave {
        form,
        base: num(0),
        amp: num(1),
        phase: num(2),
        freq: num(3),
    })
}

/// [`parse_wave`] as a stage or deform reads it: an unknown genfunc is
/// retail's sin with a warning of its own (0x4f1a29), noted here.
fn script_wave(args: &[&str], sname: &str, warns: &mut WarnSet) -> Wave {
    if let Some(w) = parse_wave(args) {
        return w;
    }
    let tok = args.first().copied().unwrap_or("");
    warns.note_once(sname, &format!("invalid genfunc {tok:?}, read as sin"));
    let mut patched = vec!["sin"];
    patched.extend(args.iter().skip(1));
    parse_wave(&patched).expect("sin parses")
}

/// Accepts `GL_`-prefixed and case/underscore variants.
pub fn parse_blend_factor(tok: &str) -> Option<BlendFactor> {
    let t = tok.to_ascii_lowercase();
    let t = t.strip_prefix("gl_").unwrap_or(&t);
    Some(match t.replace('_', "").as_str() {
        "zero" => BlendFactor::Zero,
        "one" => BlendFactor::One,
        "dstcolor" => BlendFactor::DstColor,
        "oneminusdstcolor" => BlendFactor::OneMinusDstColor,
        "srccolor" => BlendFactor::SrcColor,
        "oneminussrccolor" => BlendFactor::OneMinusSrcColor,
        "srcalpha" => BlendFactor::SrcAlpha,
        "oneminussrcalpha" => BlendFactor::OneMinusSrcAlpha,
        "dstalpha" => BlendFactor::DstAlpha,
        "oneminusdstalpha" => BlendFactor::OneMinusDstAlpha,
        "srcalphasaturate" => BlendFactor::SrcAlphaSaturate,
        _ => return None,
    })
}

const CAPS_ABSENT: [&str; 6] = [
    "gl_nv_texture_shader",
    "gl_nv_register_combiners",
    "gl_ati_fragment_shader",
    "gl_arb_texture_cube_map",
    "gl_arb_texture_env_combine",
    "gl_arb_texture_env_dot3",
];

/// What `requires GL_MAX_TEXTURE_UNITS_ARB` reads: the bundles a stage can
/// draw (`nextbundle` opens the second and last).
const TEXTURE_UNITS: f32 = 2.0;

fn is_relop(t: &str) -> bool {
    matches!(t, "<" | "<=" | ">" | ">=" | "=" | "==" | "!=")
}

fn compare(a: f32, op: &str, b: f32) -> bool {
    match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        "=" | "==" => a == b,
        "!=" => a != b,
        _ => true,
    }
}

fn eval_atom(raw: &[&str]) -> bool {
    // `!GL_x` arrives either glued or as two tokens; normalize first.
    let mut ts: Vec<&str> = Vec::with_capacity(raw.len());
    for t in raw {
        match t.strip_prefix('!') {
            Some("") => ts.push("!"),
            Some(rest) => {
                ts.push("!");
                ts.push(rest);
            }
            None => ts.push(t),
        }
    }
    let mut neg = false;
    let mut k = 0;
    if ts.first() == Some(&"!") {
        neg = true;
        k = 1;
    }
    let val = match ts.get(k) {
        Some(&"cvar") => {
            let name = ts.get(k + 1).copied().unwrap_or("");
            let op = ts.get(k + 2).copied().unwrap_or("");
            match (name.eq_ignore_ascii_case("sys_cpumhz"), op) {
                (true, ">=") => false,
                (true, "<") => true,
                _ => true,
            }
        }
        Some(ident) => match ts[k + 1..]
            .iter()
            .position(|t| is_relop(t))
            .map(|p| k + 1 + p)
        {
            Some(p) => {
                let n = ts.get(p + 1).map_or(0.0, |t| fnum(t));
                // the only numeric atom CoD scripts use is the texture-unit
                // count; a stage draws at most two bundles here, so the gates
                // pick their two-unit stages (shader doc 7)
                if ident.eq_ignore_ascii_case("gl_max_texture_units_arb") {
                    compare(TEXTURE_UNITS, ts[p], n)
                } else {
                    true
                }
            }
            None => !CAPS_ABSENT.iter().any(|c| c.eq_ignore_ascii_case(ident)),
        },
        None => true,
    };
    val != neg
}

/// OR of `||`-separated atoms; an empty line is vacuously satisfied.
fn eval_requires(toks: &[&str]) -> bool {
    toks.is_empty() || toks.split(|t| *t == "||").any(eval_atom)
}

fn is_known_kw(tok: &str) -> bool {
    let t = tok.to_ascii_lowercase();
    t.starts_with("qer_")
        || t.starts_with("q3map_")
        || matches!(
            &*t,
            "map"
                | "clampmap"
                | "animmap"
                | "nextbundle"
                | "blendfunc"
                | "alphafunc"
                | "depthwrite"
                | "depthfunc"
                | "perlight"
                | "tcmod"
                | "tcgen"
                | "texgen"
                | "rgbgen"
                | "alphagen"
                | "requires"
                | "nopicmip"
                | "nomipmaps"
                | "polygonoffset"
                | "polygonoffset2"
                | "polygonoffsetconst"
                | "cull"
                | "sort"
                | "surfaceparm"
                | "skyparms"
                | "sunfile"
                | "deformvertexes"
                | "nofog"
                | "entitymergable"
                | "skyfogvars"
                | "waterfogvars"
                | "fogvars"
                | "tesssize"
                | "light"
        )
}

fn is_delim(tok: &str) -> bool {
    tok == "{" || tok == "}" || tok == NEWLINE || is_known_kw(tok)
}

/// A keyword's arguments: what follows it on its line, as retail's parser
/// reads them with `COM_ParseExt(text, qfalse)` (CoDMP.exe 0x449fc0). A
/// brace ends them too.
fn line_args<'a, 'b>(rest: &'b [&'a str]) -> &'b [&'a str] {
    let n = rest
        .iter()
        .position(|&t| t == NEWLINE || t == "{" || t == "}")
        .unwrap_or(rest.len());
    &rest[..n]
}

fn until_keyword(args: &[&str]) -> usize {
    args.iter().position(|t| is_delim(t)).unwrap_or(args.len())
}

fn fnum(tok: &str) -> f32 {
    tok.trim_matches(|c| c == '(' || c == ')')
        .parse()
        .unwrap_or(0.0)
}

fn norm_path(p: &str) -> String {
    p.trim_start_matches('/').replace('\\', "/")
}

fn image_ref(tok: &str) -> ImageRef {
    match tok {
        "$lightmap" => ImageRef::Lightmap,
        "$whiteimage" | "*white" => ImageRef::White,
        p => ImageRef::Path(norm_path(p)),
    }
}

#[derive(Default)]
struct StageBuf {
    alive: bool,
    bundles: Vec<Bundle>,
    target: usize,
    bundles_closed: bool,
    blend: Option<(BlendFactor, BlendFactor)>,
    depth_write: Option<bool>,
    alpha_func: Option<AlphaFunc>,
    depth_func: DepthFunc,
    rgb_gen: Option<RgbGen>,
    alpha_gen: Option<AlphaGen>,
    /// `tcGen lightmap` seen before bundle 0 existed.
    want_lm: bool,
    /// `perLight`: drawn only in the dlight pass (shader doc 3).
    per_light: bool,
    /// The `nextbundle`'s `blendFunc`, applied to bundle 1 at the end.
    env1: TexEnv,
    /// `animMap <fps>` per bundle: the `map` lines after it are its frames.
    anim_fps: [Option<f32>; 2],
    /// `multiplyImage`'s four factors, folded into the gens at the end.
    multiply: Option<[ImageFactor; 4]>,
}

/// One `multiplyImage` argument (CoDMP.exe 0x4f946e): a number, or
/// `identityLighting` for the renderer's identityLight.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ImageFactor {
    IdentityLight,
    Num(f32),
}

impl StageBuf {
    /// The stage and whether it is a `perLight` one.
    fn finish(mut self, sname: &str, warns: &mut WarnSet) -> Option<(Stage, bool)> {
        if !self.alive || self.bundles.is_empty() {
            return None;
        }
        if let Some(b) = self.bundles.get_mut(1) {
            b.env = self.env1;
        }
        if self.want_lm && !self.bundles.iter().any(|b| b.image == ImageRef::Lightmap) {
            self.bundles[0].image = ImageRef::Lightmap;
        }
        // RTCW FinishShader rule for stages without rgbGen
        let rgb_gen = self.rgb_gen.unwrap_or(match self.blend {
            Some((BlendFactor::One, _)) | Some((BlendFactor::SrcAlpha, _)) | None => {
                RgbGen::IdentityLighting
            }
            Some(_) => RgbGen::Identity,
        });
        let mut alpha_gen = self.alpha_gen.unwrap_or(AlphaGen::Identity);
        let mut rgb_gen = rgb_gen;
        if let Some(m) = self.multiply {
            fold_multiply(m, &mut rgb_gen, &mut alpha_gen, sname, warns);
        }
        Some((
            Stage {
                bundles: self.bundles,
                blend: self.blend,
                depth_write: self.depth_write,
                alpha_func: self.alpha_func,
                depth_func: self.depth_func,
                rgb_gen,
                alpha_gen,
            },
            self.per_light,
        ))
    }

    fn place_bundle(&mut self, b: Bundle) {
        if self.bundles_closed {
            return;
        }
        if self.target < self.bundles.len() {
            let slot = &mut self.bundles[self.target];
            slot.image = b.image;
            slot.anim = b.anim;
            slot.clamp = b.clamp;
            slot.tc_gen = b.tc_gen;
        } else {
            self.bundles.push(b);
        }
    }
}

/// `multiplyImage` scales the stage's texels as retail loads them (0x4f3410).
/// A factor of at most 1 scales the stage's colour the same way, so it
/// folds into the gens; the stock use, `multiplyImage identityLighting`
/// under `rgbGen identity`, is the fallback stage's `rgbGen
/// identityLighting` exactly.
fn fold_multiply(
    m: [ImageFactor; 4],
    rgb_gen: &mut RgbGen,
    alpha_gen: &mut AlphaGen,
    sname: &str,
    warns: &mut WarnSet,
) {
    use ImageFactor::*;
    let rgb_folded = match (m[0], m[1], m[2]) {
        _ if m[..3].iter().all(|&f| f == Num(1.0)) => true,
        (IdentityLight, IdentityLight, IdentityLight) => match rgb_gen {
            RgbGen::Identity => {
                *rgb_gen = RgbGen::IdentityLighting;
                true
            }
            RgbGen::Const(c) => {
                *rgb_gen = RgbGen::ConstLighting(*c);
                true
            }
            _ => false,
        },
        (Num(r), Num(g), Num(b)) if [r, g, b].iter().all(|v| (0.0..=1.0).contains(v)) => {
            match rgb_gen {
                RgbGen::Identity => {
                    *rgb_gen = RgbGen::Const([r, g, b]);
                    true
                }
                RgbGen::IdentityLighting => {
                    *rgb_gen = RgbGen::ConstLighting([r, g, b]);
                    true
                }
                RgbGen::Const(c) | RgbGen::ConstLighting(c) => {
                    *c = [c[0] * r, c[1] * g, c[2] * b];
                    true
                }
                _ => false,
            }
        }
        _ => false,
    };
    let alpha_folded = match (m[3], &mut *alpha_gen) {
        (Num(1.0), _) => true,
        (Num(a), AlphaGen::Identity) if (0.0..=1.0).contains(&a) => {
            *alpha_gen = AlphaGen::Const(a);
            true
        }
        (Num(a), AlphaGen::Const(c)) if (0.0..=1.0).contains(&a) => {
            *c *= a;
            true
        }
        _ => false,
    };
    if !rgb_folded || !alpha_folded {
        warns.warn_once(sname, "multiplyImage not folded into the stage's gens");
    }
}

/// `multiplyImage` (CoDMP.exe 0x4f946e): up to four factors, before the
/// stage's first image. One factor scales rgb, three set rgb, four rgba.
fn multiply_image(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    if sb.bundles.get(sb.target).is_some() {
        warns.warn_once(sname, "multiplyImage after the images");
        return args.len();
    }
    let mut f = Vec::new();
    for t in args.iter().take(4) {
        if t.eq_ignore_ascii_case("identitylighting") {
            f.push(ImageFactor::IdentityLight);
        } else if t.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '+') {
            f.push(ImageFactor::Num(fnum(t)));
        } else {
            warns.warn_once(sname, &format!("bad multiplyImage argument {t}"));
            return args.len();
        }
    }
    let one = ImageFactor::Num(1.0);
    sb.multiply = match f[..] {
        [] => {
            warns.warn_once(sname, "multiplyImage without arguments");
            return 0;
        }
        [v] => Some([v, v, v, one]),
        [v, a] => Some([v, v, v, a]),
        [r, g, b] => Some([r, g, b, one]),
        [r, g, b, a, ..] => Some([r, g, b, a]),
    };
    f.len()
}

/// Handles one stage token at `body[i]`; returns how many of the following
/// arg tokens it consumed.
fn stage_token(
    sb: &mut StageBuf,
    kw: &str,
    args: &[&str],
    sname: &str,
    warns: &mut WarnSet,
) -> usize {
    match kw.to_ascii_lowercase().as_str() {
        "requires" => {
            let n = until_keyword(args);
            sb.alive &= eval_requires(&args[..n]);
            n
        }
        "map" => add_image(sb, false, args, sname, warns),
        "clampmap" => add_image(sb, true, args, sname, warns),
        "animmap" => add_anim(sb, args, sname, warns),
        "multiplyimage" => multiply_image(sb, args, sname, warns),
        // the stage flag 2 and its fog bit (0x4f89fe, 0x4f8a4b): no
        // stock MP stage draws differently for either
        "detail" => 0,
        "fog" => usize::from(!args.is_empty()),
        "nextbundle" => {
            if sb.target == 0 && !sb.bundles_closed {
                sb.target = 1;
            } else {
                sb.bundles_closed = true;
                warns.warn_once(sname, "multiple nextbundle");
            }
            0
        }
        "blendfunc" => {
            let short = match args.first().map(|t| t.to_ascii_lowercase()).as_deref() {
                Some("add") => Some((BlendFactor::One, BlendFactor::One)),
                Some("filter") => Some((BlendFactor::DstColor, BlendFactor::Zero)),
                Some("blend") => Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                _ => None,
            };
            match short {
                // shorthands carry a single argument
                Some(pair) => {
                    set_blend(sb, pair, sname, warns);
                    usize::from(!args.is_empty())
                }
                None if args.len() < 2 => {
                    // "missing parm for localFunc" (0x4f8b5e): blend unset
                    warns.note_once(sname, "blendFunc short of its second factor");
                    args.len()
                }
                None => {
                    let mut factor =
                        |i: usize| match args.get(i).and_then(|t| parse_blend_factor(t)) {
                            Some(f) => f,
                            None => {
                                warns.warn_once(
                                    sname,
                                    &format!(
                                        "unknown blend factor {}",
                                        args.get(i).copied().unwrap_or("")
                                    ),
                                );
                                BlendFactor::One
                            }
                        };
                    let pair = (factor(0), factor(1));
                    set_blend(sb, pair, sname, warns);
                    2.min(args.len())
                }
            }
        }
        "alphafunc" => {
            if let Some(m) = args.first().map(|t| t.to_ascii_uppercase()) {
                sb.alpha_func = match m.as_str() {
                    "GT0" => Some(AlphaFunc::Gt0),
                    "LT128" => Some(AlphaFunc::Lt128),
                    "GE128" => Some(AlphaFunc::Ge128),
                    _ => {
                        warns.warn_once(sname, &format!("unknown alphaFunc {m}"));
                        sb.alpha_func.clone()
                    }
                };
            }
            usize::from(!args.is_empty())
        }
        "depthwrite" => {
            sb.depth_write = Some(true);
            0
        }
        "depthfunc" => {
            match args.first().map(|t| t.to_ascii_lowercase()).as_deref() {
                Some("lequal") => sb.depth_func = DepthFunc::LessEqual,
                Some("equal") => sb.depth_func = DepthFunc::Equal,
                Some("always") => sb.depth_func = DepthFunc::Always,
                // retail warns and keeps the depth function (0x4f89e9)
                other => warns.note_once(sname, &format!("unknown depthFunc {other:?}")),
            }
            usize::from(!args.is_empty())
        }
        "perlight" => {
            if sb.target == 0 {
                sb.per_light = true;
            } else {
                warns.warn_once(sname, "'perLight' not allowed in nextbundle");
            }
            0
        }
        "tcmod" => tc_mod(sb, args, sname, warns),
        // `texGen` is Q3's alias (`dlightshader` spells it so)
        "tcgen" | "texgen" => tc_gen(sb, args, sname, warns),
        "rgbgen" => apply_gen(sb, true, args, sname, warns),
        "alphagen" => apply_gen(sb, false, args, sname, warns),
        _ => {
            warns.warn_once(sname, &format!("unknown token {kw}"));
            0
        }
    }
}

fn add_image(
    sb: &mut StageBuf,
    clamp_kw: bool,
    args: &[&str],
    sname: &str,
    warns: &mut WarnSet,
) -> usize {
    let mut clamp = if clamp_kw { Clamp::BOTH } else { Clamp::NONE };
    let mut k = 0;
    // image flags before the name (CoDMP.exe 0x4f34ad..0x4f3626)
    while let Some(m) = args.get(k).map(|t| t.to_ascii_lowercase()) {
        match m.as_str() {
            "clamp" => clamp = Clamp::BOTH,
            // flags 0x10 and 0x20, one axis each
            "clampx" | "clamps" => clamp.s = true,
            "clampy" | "clampt" => clamp.t = true,
            // flag 0x40 and its scale
            "heighttonormal" => {
                warns.note_once(sname, "heightToNormal ignored");
                k += 1;
            }
            // no effect here
            "noopt" | "only16bit" | "only32bit" | "picmip" | "nopicmip" | "mipmaps"
            | "nomipmaps" => {}
            _ => break,
        }
        k += 1;
    }
    let Some(path) = args.get(k) else { return k };
    // after `animMap <fps>` each `map` adds a frame (0x4f84c1)
    let fps = sb.anim_fps.get(sb.target).copied().flatten();
    if let Some(b) = sb.bundles.get_mut(sb.target).filter(|_| fps.is_some()) {
        if let ImageRef::Path(p) = image_ref(path) {
            match (&mut b.anim, &b.image) {
                (Some(anim), _) => anim.paths.push(p),
                // an `animMap` after the first `map`
                (None, ImageRef::Path(first)) => {
                    b.anim = Some(AnimSpec {
                        fps: fps.unwrap_or(0.0),
                        paths: vec![first.clone(), p],
                    })
                }
                (None, _) => {}
            }
        }
        return k + 1;
    }
    if sb.bundles.get(sb.target).is_some() && !sb.bundles_closed {
        warns.warn_once(sname, "multiple map without animMap");
    }
    let image = image_ref(path);
    let anim = match (&image, fps) {
        (ImageRef::Path(p), Some(fps)) => Some(AnimSpec {
            fps,
            paths: vec![p.clone()],
        }),
        _ => None,
    };
    sb.place_bundle(Bundle {
        image,
        anim,
        clamp,
        tcmods: Vec::new(),
        tc_gen: TcGen::Base,
        env: TexEnv::Modulate,
    });
    k + 1
}

/// A stage's `blendFunc`, or inside a `nextbundle` the bundle's texture
/// environment (CoDMP.exe 0x4f8ba3, 0x4f8bce, 0x4f8bf3): `filter` modulates, `add`
/// adds, `blend` decals; the stage's blend stays as it was.
fn set_blend(
    sb: &mut StageBuf,
    pair: (BlendFactor, BlendFactor),
    sname: &str,
    warns: &mut WarnSet,
) {
    use BlendFactor::*;
    if sb.target == 0 {
        sb.blend = Some(pair);
        return;
    }
    match pair {
        (DstColor, Zero) => sb.env1 = TexEnv::Modulate,
        (One, One) => sb.env1 = TexEnv::Add,
        (SrcAlpha, OneMinusSrcAlpha) => sb.env1 = TexEnv::Decal,
        _ => warns.warn_once(sname, "bad blendFunc for nextbundle"),
    }
}

/// CoD's `animMap <fps>` (0x4f85a0, 0x4f9926): the rest of its line is skipped and
/// the `map` lines that follow are the frames.
fn add_anim(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    let Some(fps) = args.first() else {
        warns.note_once(sname, "animMap without its rate");
        return 0;
    };
    if args.len() > 1 {
        warns.note_once(sname, "animMap frames on its own line, skipped");
    }
    if let Some(slot) = sb.anim_fps.get_mut(sb.target) {
        *slot = Some(fnum(fps));
    }
    args.len()
}

/// `tcMod` (CoDMP.exe 0x4f1b40). Retail reads each number off the line and
/// drops the whole tcMod, with a warning of its own, when one is missing:
/// stock water's three-number `turb` is no turbulence at all.
fn tc_mod(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    let Some(sub) = args.first().map(|t| t.to_ascii_lowercase()) else {
        return 0;
    };
    let g = |i: usize| args.get(i + 1).map_or(0.0, |t| fnum(t));
    let (need, tcmod) = match sub.as_str() {
        "scroll" => (2, TcMod::Scroll(g(0), g(1))),
        "scale" => (2, TcMod::Scale(g(0), g(1))),
        "rotate" => (1, TcMod::Rotate(g(0))),
        "stretch" => (5, TcMod::Stretch(script_wave(&args[1..], sname, warns))),
        // base amplitude is parsed but unused, like RTCW
        "turb" => (
            4,
            TcMod::Turb {
                amp: g(1),
                phase: g(2),
                freq: g(3),
            },
        ),
        "transform" => (6, TcMod::Transform([g(0), g(1), g(2), g(3), g(4), g(5)])),
        _ => {
            warns.warn_once(sname, &format!("unknown tcMod {sub}"));
            return args.len();
        }
    };
    if args.len() < 1 + need {
        warns.note_once(sname, &format!("tcMod {sub} short of parms, dropped"));
    } else if let Some(b) = sb.bundles.get_mut(sb.target) {
        b.tcmods.push(tcmod);
    }
    1 + need
}

fn tc_gen(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    let Some(form) = args.first().map(|t| t.to_ascii_lowercase()) else {
        return 0;
    };
    match form.as_str() {
        "lightmap" => {
            if sb.bundles.iter().any(|b| b.image == ImageRef::Lightmap) {
                warns.warn_once(sname, "redundant tcGen lightmap");
            } else if let Some(b) = sb.bundles.first_mut() {
                b.image = ImageRef::Lightmap;
            } else {
                sb.want_lm = true;
            }
            1
        }
        // q3map_globaltexture idiom: dot world position with the basis in the
        // VS (renderer side); the parser only stores it.
        "vector" => {
            let mut vals = [0.0f32; 6];
            let mut bad = false;
            let mut k = 1;
            for slot in vals.iter_mut() {
                while matches!(args.get(k), Some(&"(") | Some(&")")) {
                    k += 1;
                }
                match args.get(k) {
                    Some(t) => match t.trim_matches(|c| c == '(' || c == ')').parse::<f32>() {
                        Ok(v) => *slot = v,
                        Err(_) => bad = true,
                    },
                    None => bad = true,
                }
                k += 1;
            }
            while matches!(args.get(k), Some(&")")) {
                k += 1;
            }
            if bad {
                warns.warn_once(sname, "malformed tcGen vector");
            }
            if let Some(b) = sb.bundles.get_mut(sb.target) {
                b.tc_gen = TcGen::Vector(vals);
            }
            k
        }
        "environment" => {
            if let Some(b) = sb.bundles.get_mut(sb.target) {
                b.tc_gen = TcGen::Environment;
            }
            1
        }
        _ => {
            warns.warn_once(sname, "unsupported tcGen");
            sb.alive = false;
            1 + until_keyword(&args[1..])
        }
    }
}

fn read_triple(args: &[&str], start: usize) -> ([f32; 3], usize) {
    let mut k = start;
    let paren = args.get(k) == Some(&"(");
    if paren {
        k += 1;
    }
    let mut c = [0.0f32; 3];
    for (j, slot) in c.iter_mut().enumerate() {
        if let Some(t) = args.get(k + j) {
            *slot = fnum(t);
        }
    }
    k += 3.min(args.len().saturating_sub(k));
    if paren && args.get(k) == Some(&")") {
        k += 1;
    }
    (c, k)
}

/// `rgbGen vertex` and `exactVertex` give the stage `alphaGen vertex` while
/// its alphaGen is still unset, and `alphaGen identity` stores unset, so
/// it is overridden too; a later `alphaGen` line wins (CoDMP.exe
/// 0x4f8d70..0x4f8d7e, 0x4f9009; shader doc 3).
fn vertex_alpha_unless_set(sb: &mut StageBuf) {
    if matches!(sb.alpha_gen, None | Some(AlphaGen::Identity)) {
        sb.alpha_gen = Some(AlphaGen::Vertex);
    }
}

/// rgbGen/alphaGen share vertex, identity and wave; the const forms differ in
/// arity, so `rgb` picks both the target field and the spelling set.
fn apply_gen(
    sb: &mut StageBuf,
    rgb: bool,
    args: &[&str],
    sname: &str,
    warns: &mut WarnSet,
) -> usize {
    let Some(form_tok) = args.first() else {
        return 0;
    };
    let form = form_tok.to_ascii_lowercase();
    let kind = if rgb { "rgbGen" } else { "alphaGen" };
    match form.as_str() {
        "vertex" => {
            if rgb {
                sb.rgb_gen = Some(RgbGen::Vertex);
                vertex_alpha_unless_set(sb);
            } else {
                sb.alpha_gen = Some(AlphaGen::Vertex);
            }
            1
        }
        "identity" => {
            if rgb {
                sb.rgb_gen = Some(RgbGen::Identity);
            } else {
                sb.alpha_gen = Some(AlphaGen::Identity);
            }
            1
        }
        "wave" => {
            // args[0] is the `wave` keyword itself; the genfunc follows
            let w = script_wave(&args[1..], sname, warns);
            if rgb {
                sb.rgb_gen = Some(RgbGen::Wave(w));
            } else {
                sb.alpha_gen = Some(AlphaGen::Wave(w));
            }
            6
        }
        "exactvertex" if rgb => {
            sb.rgb_gen = Some(RgbGen::ExactVertex);
            vertex_alpha_unless_set(sb);
            1
        }
        "identitylighting" if rgb => {
            sb.rgb_gen = Some(RgbGen::IdentityLighting);
            1
        }
        "lightingdiffuse" | "lightingambient" if rgb => {
            sb.rgb_gen = Some(RgbGen::LightingDiffuse);
            1
        }
        "lightingprecalc" if rgb => {
            sb.rgb_gen = Some(RgbGen::LightingPrecalc);
            1
        }
        "const" | "constant" | "constlighting" if rgb => {
            let (c, k) = read_triple(args, 1);
            sb.rgb_gen = Some(if form == "constlighting" {
                RgbGen::ConstLighting(c)
            } else {
                RgbGen::Const(c)
            });
            k
        }
        // alphaGen const takes a single float
        "const" => {
            if let Some(t) = args.get(1) {
                sb.alpha_gen = Some(AlphaGen::Const(fnum(t)));
            }
            2.min(args.len())
        }
        "entity" if !rgb => {
            sb.alpha_gen = Some(AlphaGen::Entity);
            1
        }
        _ => {
            // the forms CoDMP.exe 0x4f8c21..0x4f8e5b knows; any other it warns about
            // and skips, keeping the stage's earlier gen
            let retail_knows: &[&str] = if rgb {
                &["entity", "oneminusentity", "oneminusvertex"]
            } else {
                &[
                    "constlighting",
                    "oneminusentity",
                    "lightingspecular",
                    "oneminusvertex",
                    "portal",
                    "dot",
                    "oneminusdot",
                    "oneplusdot",
                    "negativedot",
                ]
            };
            if retail_knows.contains(&form.as_str()) {
                warns.warn_once(sname, &format!("unsupported {kind} {form}"));
            } else {
                warns.note_once(sname, &format!("unknown {kind} {form}, skipped"));
            }
            args.len()
        }
    }
}

/// Handles one top-level token; returns how many arg tokens it consumed.
fn top_token(
    sh: &mut Shader,
    kw: &str,
    args: &[&str],
    sname: &str,
    warns: &mut WarnSet,
    pending_req: &mut bool,
) -> usize {
    let lower = kw.to_ascii_lowercase();
    let one_arg = || args.first().filter(|t| !is_delim(t));
    match lower.as_str() {
        "requires" => {
            let n = until_keyword(args);
            *pending_req &= eval_requires(&args[..n]);
            n
        }
        "skyparms" => {
            let mut n = 0;
            while n < 3 && n < args.len() && !is_delim(args[n]) {
                n += 1;
            }
            let env = match args.first() {
                Some(&"-") | None => String::new(),
                Some(p) => norm_path(p),
            };
            let cloud = if n > 1 {
                args[1].parse::<f32>().ok()
            } else {
                None
            };
            sh.sky = Some(SkyParms {
                env,
                cloud_height: cloud.filter(|&h| h > 0.0).unwrap_or(512.0),
            });
            n
        }
        "cull" => {
            sh.two_sided = matches!(
                one_arg().map(|t| t.to_ascii_lowercase()).as_deref(),
                Some("none") | Some("disable") | Some("twosided")
            );
            usize::from(one_arg().is_some())
        }
        "sort" => {
            if let Some(t) = one_arg() {
                match map_sort_token(t) {
                    Some(v) => sh.sort = Some(v),
                    None => warns.warn_once(sname, &format!("unknown sort token {t}")),
                }
                1
            } else {
                0
            }
        }
        "surfaceparm" => {
            if let Some(t) = one_arg() {
                match t.to_ascii_lowercase().as_str() {
                    "sky" => sh.surface.sky = true,
                    "nodraw" => sh.surface.nodraw = true,
                    "trans" => sh.surface.trans = true,
                    "water" => sh.surface.water = true,
                    "nonsolid" => sh.surface.nonsolid = true,
                    "nolightmap" => sh.surface.nolightmap = true,
                    "nodlight" => sh.surface.nodlight = true,
                    _ => {} // physics/material data already carried by the BSP lump
                }
            }
            usize::from(one_arg().is_some())
        }
        "sunfile" => {
            if let Some(t) = one_arg() {
                sh.sunfile = Some(norm_path(t));
            }
            usize::from(one_arg().is_some())
        }
        "deformvertexes" => {
            // The 1.1 parser (CoDMP.exe 0x4f9d20, shader doc 8) accepts
            // projectionShadow, autosprite, autosprite2, text0-7, bulge,
            // wave, flap, normal, syncNormal and move, three records at
            // most. vcod draws all but projectionShadow, text0-7 and
            // syncNormal, which have no stock user and hold their slot
            // inertly.
            let kind = args.first().map(|t| t.to_ascii_lowercase());
            if kind.is_some() && sh.deforms.len() == MAX_SHADER_DEFORMS {
                warns.warn_once(sname, "MAX_SHADER_DEFORMS");
                return until_keyword(args);
            }
            let spread = |t: Option<&&str>| match t.map(|t| fnum(t)) {
                Some(d) if d != 0.0 => 1.0 / d,
                // retail warns "illegal div value of 0" and stores 100
                _ => 100.0,
            };
            let (record, used) = match kind.as_deref() {
                Some("wave") => match parse_wave(args.get(2..).unwrap_or(&[])) {
                    Some(wave) => (
                        Some(Deform::Wave {
                            wave,
                            spread: spread(args.get(1)),
                        }),
                        7.min(args.len()),
                    ),
                    None => {
                        warns.warn_once(sname, "unknown deformVertexes wave form");
                        (Some(Deform::Other), until_keyword(args))
                    }
                },
                Some("flap") => {
                    let axis = args.get(1).map(|t| t.to_ascii_lowercase());
                    let along_t = match axis.as_deref() {
                        Some("s" | "x") => Some(false),
                        Some("t" | "y") => Some(true),
                        _ => None,
                    };
                    match (along_t, parse_wave(args.get(3..).unwrap_or(&[]))) {
                        (Some(along_t), Some(wave)) => (
                            Some(Deform::Flap(Flap {
                                along_t,
                                wave,
                                spread: spread(args.get(2)),
                            })),
                            8.min(args.len()),
                        ),
                        _ => {
                            warns.warn_once(sname, "bad deformVertexes flap");
                            (None, until_keyword(args))
                        }
                    }
                }
                Some("autosprite") => {
                    sh.sprite = Some(Sprite::Auto);
                    (Some(Deform::Other), 1)
                }
                Some("autosprite2") => {
                    sh.sprite = Some(Sprite::Auto2);
                    (Some(Deform::Other), 1)
                }
                // width height speed (deform 6, record +0x28..+0x30)
                Some("bulge") => {
                    let n = until_keyword(args);
                    if n < 4 {
                        warns.warn_once(sname, "missing deformVertexes bulge parm");
                        (None, n)
                    } else {
                        let b = Bulge {
                            width: fnum(args[1]),
                            height: fnum(args[2]),
                            speed: fnum(args[3]),
                        };
                        (Some(Deform::Bulge(b)), 4)
                    }
                }
                // x y z, then the wave (deform 7)
                Some("move") => {
                    let n = until_keyword(args);
                    match parse_wave(args.get(4..n).unwrap_or(&[])) {
                        Some(wave) if n >= 5 => {
                            let by = [fnum(args[1]), fnum(args[2]), fnum(args[3])];
                            (Some(Deform::Move { by, wave }), 9.min(n))
                        }
                        _ => {
                            warns.warn_once(sname, "bad deformVertexes move");
                            (None, n)
                        }
                    }
                }
                // amplitude frequency (deform 4, record +0x18 and +0x20)
                Some("normal") => {
                    let n = until_keyword(args);
                    if n < 3 {
                        warns.warn_once(sname, "missing deformVertexes parm");
                        (None, n)
                    } else {
                        let d = DeformNormal {
                            amp: fnum(args[1]),
                            freq: fnum(args[2]),
                        };
                        (Some(Deform::Normal(d)), 3)
                    }
                }
                // a copy of the wave before it, its amplitude scaled
                Some("syncnormal") => {
                    let n = until_keyword(args);
                    let after_wave = matches!(sh.deforms.last(), Some(Deform::Wave { .. }));
                    (after_wave.then_some(Deform::Other).filter(|_| n >= 2), n)
                }
                Some("projectionshadow") => (Some(Deform::Other), 1),
                Some(k)
                    if k.len() == 5
                        && k.starts_with("text")
                        && (b'0'..=b'7').contains(&k.as_bytes()[4]) =>
                {
                    (Some(Deform::Other), 1)
                }
                _ => (None, until_keyword(args)),
            };
            sh.deforms.extend(record);
            used
        }
        "skyfogvars" | "waterfogvars" | "fogvars" | "entitymergable" => until_keyword(args),
        // the rest of the line skipped, as CoDMP.exe 0x4fa950 does for each
        "tesssize" | "radialnormals" | "fogparms" => args.len(),
        _ if lower.starts_with("qer") || lower.starts_with("q3map") => args.len(),
        "light" | "clamptime" => usize::from(!args.is_empty()),
        // shader flag 0x200; vcod has no picmip
        "picmip2" => 0,
        // United Offensive's shader flag 0x2000 (CoDUOMP.exe 0x5014d4), on
        // the flamethrower's sprites; its reader is not traced
        "nolightscale" => 0,
        // shader flag 2 and sort 1 (0x4fae63); vcod draws no portal view
        "portal" => {
            sh.sort = Some(1.0);
            0
        }
        // shader flags 0x100 and 0x80 (CoDMP.exe 0x4fad32, 0x4fad13);
        // unlike Q3, `nomipmaps` leaves picmip alone
        "nopicmip" => {
            sh.nopicmip = true;
            0
        }
        "nomipmaps" => {
            sh.nomipmaps = true;
            0
        }
        // All three set one of shader flags 0x10/0x20/0x40, each clearing
        // the other two, and any of them sorts an unsorted block decal
        // (CoDMP.exe 0x4fad60..0x4fadc9, 0x4fba4d).
        "polygonoffset" => {
            sh.polygon_offset = Some(PolygonOffset::Single);
            0
        }
        "polygonoffset2" => {
            sh.polygon_offset = Some(PolygonOffset::Double);
            0
        }
        "polygonoffsetconst" => {
            sh.polygon_offset = Some(PolygonOffset::Const);
            0
        }
        "nofog" => {
            sh.nofog = true;
            0
        }
        _ => {
            warns.warn_once(sname, &format!("unknown token {kw}"));
            0
        }
    }
}

/// Parses one shader block body into a `Shader`, reporting tolerable damage
/// through `warns`. Bodies come from `split_blocks`: braces flattened to
/// standalone tokens, names normalized but body paths still verbatim.
pub fn parse_shader(name: &str, body: &[&str], warns: &mut WarnSet) -> Shader {
    let mut sh = Shader {
        name: name.to_string(),
        ..Default::default()
    };
    let mut pending_req = true;
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            "{" => {
                i += 1;
                let mut sb = StageBuf {
                    alive: pending_req,
                    ..Default::default()
                };
                pending_req = true;
                let mut depth = 1u32;
                while i < body.len() && depth > 0 {
                    match body[i] {
                        "{" => depth += 1,
                        "}" => depth -= 1,
                        NEWLINE => {}
                        tok if sb.alive && depth == 1 => {
                            // kw plus however many args the handler ate
                            let args = line_args(&body[i + 1..]);
                            let n = stage_token(&mut sb, tok, args, name, warns);
                            i += 1 + n.min(args.len());
                            continue;
                        }
                        _ => {} // failed requires: skip straight to the closing brace
                    }
                    i += 1;
                }
                let alive = sb.alive;
                if let Some((st, per_light)) = sb.finish(name, warns) {
                    if per_light {
                        sh.per_light.push(st);
                    } else {
                        sh.stages.push(st);
                    }
                } else if !alive {
                    sh.dropped_stages += 1;
                }
            }
            "}" | NEWLINE => i += 1,
            tok => {
                let args = line_args(&body[i + 1..]);
                let n = top_token(&mut sh, tok, args, name, warns, &mut pending_req);
                i += 1 + n.min(args.len());
            }
        }
    }
    sh
}

// ---- library ----

/// Every `.shader` script in the mod's paks, parsed and keyed by material
/// name. Later archives win because `Pk3Fs` resolves each script path to its
/// highest layer already, and a name repeated inside one script takes the
/// last block, like RTCW's hash insert.
pub struct ShaderLib {
    by_name: HashMap<String, Shader>,
    /// First texture path per material (lowercased, known image extension
    /// stripped): what the implicit-material loader used to read off the
    /// minimal parser in `assets.rs`.
    images: HashMap<String, String>,
    /// Unique parse warnings fired during the scan.
    warns: usize,
}

/// The library by [`crate::asset_source::load`]: every script parsed once
/// per source, until [`crate::pk3::Pk3Fs::clear_references`] or an overlay drops the
/// cache. A `.shader` read marks no pak. The name is unused (`""`).
impl Asset for ShaderLib {
    const KIND: AssetKind = AssetKind::ShaderScripts;

    fn find(src: &dyn AssetSource, _name: &str) -> anyhow::Result<Self> {
        // one set for the whole scan: repeated damage across many files
        // warns once per (shader, message)
        Ok(Self::load_with(src, &mut WarnSet::new()))
    }
}

impl ShaderLib {
    /// The source's library, parsed on the first ask ([`Asset`] above).
    pub fn load(fs: &dyn AssetSource) -> Arc<Self> {
        crate::asset_source::load(fs, "").expect("a shader scan never fails")
    }

    /// The scan behind [`load`](Self::load), uncached, firing its warnings
    /// into `warns`.
    pub fn load_with(fs: &dyn AssetSource, warns: &mut WarnSet) -> Self {
        let mut lib = Self {
            by_name: HashMap::new(),
            images: HashMap::new(),
            warns: 0,
        };
        // every directory: effects keep theirs in fxshaders/
        for path in fs.list("", ".shader") {
            let Some(raw) = fs.read(&path) else { continue };
            let text = String::from_utf8_lossy(&raw);
            for (name, body) in split_blocks(&text) {
                // glued `}{` continuation blocks carry no name of their own
                if name.is_empty() {
                    continue;
                }
                let sh = parse_shader(&name, &body, warns);
                // from the raw body, not the parsed stages: a gated-out or
                // dropped stage must still yield the material's texture
                if let Some(img) = body_image(&body) {
                    lib.images.insert(name.clone(), img);
                }
                lib.by_name.insert(name, sh);
            }
        }
        lib.warns = warns.len();
        lib
    }

    /// Every block compiled into a [`Material`].
    pub fn materials(&self) -> MaterialLib {
        MaterialLib {
            by_name: self
                .by_name
                .iter()
                .map(|(k, sh)| (k.clone(), Material::from_shader(sh)))
                .collect(),
            images: self.images.clone(),
            warns: self.warns,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Shader> {
        self.by_name.get(&norm_key(name))
    }

    /// First `map`/`clampmap` image path of the shader (see [`body_image`]);
    /// `None` when the block references no real texture.
    pub fn image(&self, name: &str) -> Option<&str> {
        self.images.get(&norm_key(name)).map(String::as_str)
    }

    /// The whole material->image table, for helpers that take the map form.
    pub fn image_map(&self) -> &HashMap<String, String> {
        &self.images
    }

    /// Whether the fx quad pass draws this material on the additive
    /// pipeline: the stage carrying the material's first image blends onto
    /// GL_ONE (`add`, `GL_ONE GL_ONE`, `GL_SRC_ALPHA GL_ONE`). Scoping to
    /// the image's stage keeps two-stage decal scripts (alpha base plus a
    /// perlight overlay) from being retagged by the later stage; scoping to
    /// any stage instead flips 58 stock materials' rendering.
    pub fn is_additive(&self, name: &str) -> bool {
        self.get(name).is_some_and(|s| {
            s.stages
                .iter()
                .find(|st| {
                    st.bundles
                        .iter()
                        .any(|b| matches!(b.image, ImageRef::Path(_)))
                })
                .is_some_and(|st| matches!(st.blend, Some((_, BlendFactor::One))))
        })
    }

    pub fn uses_polygon_offset(&self, name: &str) -> bool {
        self.get(name).is_some_and(|s| s.polygon_offset.is_some())
    }

    /// Unique parse warnings fired while building the library.
    pub fn warn_count(&self) -> usize {
        self.warns
    }

    pub fn sky_blocks(&self) -> impl Iterator<Item = &Shader> {
        self.by_name.values().filter(|s| s.sky.is_some())
    }

    /// Every block's (normalized) name, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The first `map`/`clampmap` image of a raw block body, normalized the way
/// the minimal parser stored them: lowercased, leading slash and known image
/// extension gone, `$lightmap`/`*white` skipped, the clamps and
/// `heightToNormal` treated as modifiers before the path.
fn body_image(body: &[&str]) -> Option<String> {
    let mut want = false;
    for &tok in body.iter().filter(|&&t| t != NEWLINE) {
        let t = tok.to_ascii_lowercase();
        if want {
            want = false;
            if matches!(
                t.as_str(),
                "clamp" | "clampx" | "clampy" | "clamps" | "clampt" | "heighttonormal"
            ) {
                want = true;
            } else if !t.starts_with(['$', '*']) {
                let stripped = [".tga", ".jpg", ".dds"]
                    .iter()
                    .find_map(|e| t.strip_suffix(e))
                    .unwrap_or(&t);
                return Some(stripped.trim_start_matches('/').to_string());
            }
            continue;
        }
        want = t == "map" || t == "clampmap";
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::SunFile;

    // ---- stage parser ----

    const SAMPLE: &str = r#"
textures\walls\big // the big wall
{
    map textures/walls/big.tga
    blendFunc GL_ONE GL_ONE
}{
    map $lightmap
}
/*
textures/walls/gone
{
    pb_never
}
*/
common\detail
{
    nopicmip
    map textures\detail\detail.tga
}
"#;

    #[test]
    fn splits_blocks_with_comments_and_glued_braces() {
        let blocks = split_blocks(SAMPLE);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            (
                "textures/walls/big".to_string(),
                vec![
                    "map",
                    "textures/walls/big.tga",
                    NEWLINE,
                    "blendFunc",
                    "GL_ONE",
                    "GL_ONE"
                ]
            )
        );
        // glued `}{`: anonymous block keeps its body
        assert_eq!(blocks[1].0, "");
        assert_eq!(blocks[1].1, vec!["map", "$lightmap"]);
        // block-commented shader never appears
        assert!(blocks.iter().all(|(n, _)| n != "textures/walls/gone"));
        assert!(!blocks.iter().any(|(_, b)| b.contains(&"pb_never")));
    }

    #[test]
    fn normalizes_names_to_lowercase_and_slashes() {
        let blocks = split_blocks(SAMPLE);
        assert_eq!(blocks[2].0, "common/detail");
        assert_eq!(
            blocks[2].1,
            vec!["nopicmip", NEWLINE, "map", r"textures\detail\detail.tga"]
        );
    }

    #[test]
    fn duplicate_names_both_come_out() {
        let blocks = split_blocks("a { one }\na\n{\n two \n}");
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|(n, _)| n == "a"));
        assert_eq!(blocks[0].1, vec!["one"]);
        assert_eq!(blocks[1].1, vec!["two"]);
    }

    #[test]
    fn nested_braces_flatten_into_body() {
        let blocks = split_blocks("s { { map x.tga } }");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "s");
        assert_eq!(blocks[0].1, vec!["{", "map", "x.tga", "}"]);
    }

    #[test]
    fn unterminated_block_dropped() {
        assert!(split_blocks("x { never closed").is_empty());
    }

    #[test]
    fn stray_closer_ignored() {
        let blocks = split_blocks("} x { y }");
        assert_eq!(blocks, vec![("x".to_string(), vec!["y"])]);
    }

    // ---- stage parser ----

    const WATER_FALLBACK: &str = r#"
textures/sfx/test_water
{
    qer_editorimage textures/sfx/damwater.dds
    surfaceparm trans
    surfaceparm water
    sort water
    {
        requires GL_MAX_TEXTURE_UNITS_ARB < 4 || !GL_NV_texture_shader || !GL_NV_register_combiners
        map textures/sfx/damwater.jpg
        tcgen vector ( .001953125 0 0 ) ( 0 .001953125 0 )
        tcMod Scroll .05 0
        tcMod scale 4 4
        rgbGen exactVertex
        nextbundle map $lightmap
    }
    {
        requires GL_MAX_TEXTURE_UNITS_ARB >= 4
        requires GL_NV_texture_shader
        waterMap 64 64 37 37 76 1 0 .06
        rgbGen vertex
    }
}
"#;

    fn first_block(text: &str) -> (String, Vec<&str>) {
        split_blocks(text).into_iter().next().unwrap()
    }

    fn parse_one(text: &str) -> Shader {
        let (name, body) = first_block(text);
        let mut warns = WarnSet::new();
        parse_shader(&name, &body, &mut warns)
    }

    #[test]
    fn water_fallback_kept_and_hw_stage_dropped_by_requires() {
        let (name, body) = first_block(WATER_FALLBACK);
        let mut warns = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut warns);
        assert_eq!(sh.stages.len(), 1, "hw stage must drop via requires");
        assert!(
            !warns.fired(&name, "unknown token waterMap"),
            "dropped stage must not reach field parsing"
        );
        assert_eq!(warns.len(), 0);
        let st = &sh.stages[0];
        assert_eq!(st.bundles.len(), 2);
        let b0 = &st.bundles[0];
        assert_eq!(
            b0.image,
            ImageRef::Path("textures/sfx/damwater.jpg".to_string())
        );
        assert_eq!(
            b0.tcmods,
            vec![TcMod::Scroll(0.05, 0.0), TcMod::Scale(4.0, 4.0)]
        );
        // tcGen vector basis, sx sy sz tx ty tz as written
        assert_eq!(
            b0.tc_gen,
            TcGen::Vector([0.001953125, 0.0, 0.0, 0.0, 0.001953125, 0.0])
        );
        assert_eq!(st.bundles[1].image, ImageRef::Lightmap);
        assert_eq!(st.rgb_gen, RgbGen::ExactVertex);
        // the sort name maps at parse time, CoD numbering
        assert_eq!(sh.sort, Some(SORT_WATER));
        assert!(sh.surface.trans && sh.surface.water);
    }

    #[test]
    fn texture_unit_comparisons_read_two_units() {
        let kept = parse_one("t { requires GL_MAX_TEXTURE_UNITS_ARB == 2 { map a.tga } }");
        assert_eq!(kept.stages.len(), 1);
        let dropped = parse_one("t { requires GL_MAX_TEXTURE_UNITS_ARB > 2 { map a.tga } }");
        assert!(dropped.stages.is_empty());
    }

    /// A `perLight` stage is left out: retail draws it only once per dlight
    /// touching the surface. Inside a nextbundle the keyword is ignored.
    #[test]
    fn perlight_stages_leave_the_base_pass() {
        let sh = parse_one(
            "t { { map a.tga nextbundle map $lightmap } \
             { perlight map a.tga blendFunc add nextbundle map $dlight blendFunc filter } \
             { map b.tga nextbundle perLight map $lightmap } }",
        );
        assert_eq!(sh.stages.len(), 2);
        assert_eq!(sh.dropped_stages, 0);
        assert_eq!(
            sh.stages[1].bundles[0].image,
            ImageRef::Path("b.tga".into())
        );
        assert_eq!(sh.per_light.len(), 1);
        let pl = &sh.per_light[0];
        assert_eq!(pl.blend, Some((BlendFactor::One, BlendFactor::One)));
        assert!(pl.bundles[1].takes_lightmap_coords());
        assert_eq!(
            sh.dlight_draw(),
            Some(DlightDraw::Stages(sh.per_light.clone()))
        );
    }

    /// A `nextbundle`'s `blendFunc` is the bundle's texture environment;
    /// the stage keeps its own blend.
    #[test]
    fn nextbundle_blendfunc_sets_the_texture_environment() {
        let sh = parse_one(
            "t { { map a.tga blendFunc blend nextbundle map $lightmap blendFunc filter } \
             { map a.tga nextbundle map b.tga blendFunc add } \
             { map a.tga nextbundle map b.tga blendFunc blend } }",
        );
        let st = &sh.stages;
        assert_eq!(
            st[0].blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha))
        );
        assert_eq!(st[0].bundles[1].env, TexEnv::Modulate);
        assert_eq!(st[1].blend, None);
        assert_eq!(st[1].bundles[1].env, TexEnv::Add);
        assert_eq!(st[2].bundles[1].env, TexEnv::Decal);
    }

    #[test]
    fn dlight_draw_copies_a_texture_times_lightmap_stage() {
        let one_one = Some((BlendFactor::One, BlendFactor::One));
        let src_one = Some((BlendFactor::SrcAlpha, BlendFactor::One));
        let draw = |text: &str| parse_one(text).dlight_draw();
        let copy_of = |d: Option<DlightDraw>| match d {
            Some(DlightDraw::Stages(st)) if st.len() == 1 => st[0].clone(),
            other => panic!("not a copy: {other:?}"),
        };
        // one stage, opaque: GL_ONE GL_ONE over the dlight image
        let c = copy_of(draw(
            "t { { map a.tga rgbGen identity nextbundle map $lightmap } }",
        ));
        assert_eq!(c.blend, one_one);
        assert_eq!(c.bundles[0].image, ImageRef::Path("a.tga".into()));
        assert_eq!(c.bundles[1].image, ImageRef::Path(DLIGHT_IMAGE.into()));
        assert_eq!(c.rgb_gen, RgbGen::Identity);
        // a GL_SRC_ALPHA source keeps it (sorted opaque; blend alone is 15)
        let c = copy_of(draw(
            "t { sort opaque { map a.tga blendFunc blend nextbundle map $lightmap } }",
        ));
        assert_eq!(c.blend, src_one);
        // the two-stage lightmap-then-filter form collapses first
        let c = copy_of(draw(
            "t { { map $lightmap } { map a.tga blendFunc filter } }",
        ));
        assert_eq!(c.bundles[0].image, ImageRef::Path("a.tga".into()));
        assert_eq!(c.blend, one_one);
        // a filter blend, an added lightmap or a third stage: dlightshader
        for text in [
            "t { sort opaque { map a.tga blendFunc filter nextbundle map $lightmap } }",
            "t { { map a.tga nextbundle map $lightmap blendFunc add } }",
            "t { { map $lightmap } { map a.tga blendFunc filter } { map g.tga blendFunc add } }",
            "t { { map a.tga } }",
        ] {
            assert_eq!(draw(text), Some(DlightDraw::Blob), "{text}");
        }
    }

    #[test]
    fn the_sorting_stage_reads_the_collapsed_pair() {
        let images = |text: &str| {
            parse_one(text)
                .sorting_stage()
                .map(|st| st.bundles.into_iter().map(|b| b.image).collect::<Vec<_>>())
        };
        let (a, lm) = (ImageRef::Path("a.tga".into()), ImageRef::Lightmap);
        assert_eq!(
            images(
                "t { { map $lightmap } { map a.tga blendFunc filter } { map g.tga blendFunc add } }"
            ),
            Some(vec![a.clone(), lm.clone()])
        );
        assert_eq!(
            images("t { { map $lightmap } { map a.tga blendFunc add } }"),
            Some(vec![lm])
        );
        assert_eq!(images("t { { map a.tga } }"), Some(vec![a]));
        assert_eq!(images("t { surfaceparm nodraw }"), None);
    }

    #[test]
    fn dlight_draw_skips_sky_nodlight_and_sorts_past_opaque() {
        for text in [
            "t { surfaceparm nodlight { map a.tga nextbundle map $lightmap } }",
            "t { surfaceparm sky { map a.tga } }",
            "t { sort decal { map a.tga } }",
            "t { { map a.tga blendFunc blend } }",
            "t { polygonOffset { map a.tga } }",
        ] {
            assert_eq!(parse_one(text).dlight_draw(), None, "{text}");
        }
        assert_eq!(
            parse_one("t { sort ocean { map a.tga } }").dlight_draw(),
            Some(DlightDraw::Blob)
        );
    }

    /// Dropped stages are counted on the shader for the F3 overlay.
    #[test]
    fn dropped_stages_are_counted() {
        let sh = parse_one(
            "t { requires GL_MAX_TEXTURE_UNITS_ARB > 2 { map a.tga } \
             { tcGen bogus { map b.tga } } { map c.tga } }",
        );
        assert_eq!(sh.stages.len(), 1);
        assert_eq!(sh.dropped_stages, 2);
    }

    #[test]
    fn top_level_requires_attaches_to_next_stage_only() {
        let sh = parse_one("t { requires GL_NV_texture_shader { map a.tga } { map b.tga } }");
        assert_eq!(sh.stages.len(), 1);
        assert_eq!(
            sh.stages[0].bundles[0].image,
            ImageRef::Path("b.tga".to_string())
        );
    }

    #[test]
    fn cvar_requires_rules() {
        let dropped = parse_one("t { requires cvar sys_cpuMHz >= 500 { map a.tga } }");
        assert!(dropped.stages.is_empty());
        let kept = parse_one("t { requires cvar sys_cpuMHz < 500 { map a.tga } }");
        assert_eq!(kept.stages.len(), 1);
        let other_cvar_true = parse_one("t { requires cvar sys_vidcap >= 1 { map a.tga } }");
        assert_eq!(other_cvar_true.stages.len(), 1);
    }

    #[test]
    fn blend_func_shorthands() {
        for (tok, src, dst) in [
            ("add", BlendFactor::One, BlendFactor::One),
            ("filter", BlendFactor::DstColor, BlendFactor::Zero),
            (
                "blend",
                BlendFactor::SrcAlpha,
                BlendFactor::OneMinusSrcAlpha,
            ),
        ] {
            let text = format!("t\n{{\n {{\n blendFunc {tok}\n map tex/a.tga\n }}\n}}\n");
            let sh = parse_one(&text);
            assert_eq!(sh.stages[0].blend, Some((src, dst)), "{tok}");
        }
    }

    #[test]
    fn degenerate_blend_one_zero_is_recorded() {
        let sh = parse_one("t { { blendFunc GL_ONE GL_ZERO map a.tga } }");
        assert_eq!(
            sh.stages[0].blend,
            Some((BlendFactor::One, BlendFactor::Zero))
        );
    }

    #[test]
    fn unknown_blend_factor_becomes_one_with_warning() {
        let (name, body) = first_block("t { { blendFunc GL_ONE GL_BOGUS map a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(
            sh.stages[0].blend,
            Some((BlendFactor::One, BlendFactor::One))
        );
        assert!(w.fired(&name, "unknown blend factor GL_BOGUS"));
    }

    #[test]
    fn anim_map_takes_its_frames_from_the_map_lines_after_it() {
        let sh = parse_one(
            "t {\n{\nanimMap 12\nmap tex/a.tga\nmap clamp tex\\b.tga\nblendFunc add\nnextbundle\nmap $lightmap\n}\n}",
        );
        let st = &sh.stages[0];
        let anim = st.bundles[0].anim.as_ref().unwrap();
        assert_eq!(anim.fps, 12.0);
        assert_eq!(
            anim.paths,
            vec!["tex/a.tga".to_string(), "tex/b.tga".to_string()]
        );
        assert_eq!(st.bundles[0].image, ImageRef::Path("tex/a.tga".to_string()));
        assert_eq!(st.blend, Some((BlendFactor::One, BlendFactor::One)));
        assert_eq!(st.bundles[1].image, ImageRef::Lightmap);
        assert_eq!(st.bundles[1].anim, None);
    }

    #[test]
    fn anim_map_skips_frames_on_its_own_line() {
        // retail's SkipRestOfLine after the rate: the stage has no image
        let (name, body) = first_block("t {\n{\nanimMap 15 a.dds b.dds\nblendFunc add\n}\n}");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert!(sh.stages.is_empty());
        assert!(w.is_empty());
    }

    #[test]
    fn alpha_func_ge128() {
        let sh = parse_one("t { { map a.tga alphaFunc GE128 } }");
        assert_eq!(sh.stages[0].alpha_func, Some(AlphaFunc::Ge128));
    }

    #[test]
    fn depth_write_flips() {
        let sh = parse_one("t { { map a.tga depthWrite } }");
        assert_eq!(sh.stages[0].depth_write, Some(true));
    }

    #[test]
    fn unknown_token_warns_exactly_once_for_repeat_occurrences() {
        let (name, body) = first_block("t { fooBar fooBar { map a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages.len(), 1);
        assert!(w.fired(&name, "unknown token fooBar"));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn sky_parms_env_and_cloud_height() {
        let sh = parse_one("t { skyParms /env/starsky 512 - { map a.tga } }");
        assert_eq!(
            sh.sky,
            Some(SkyParms {
                env: "env/starsky".to_string(),
                cloud_height: 512.0
            })
        );
        let suppressed = parse_one("t { skyParms - - - { map a.tga } }");
        assert_eq!(
            suppressed.sky,
            Some(SkyParms {
                env: String::new(),
                cloud_height: 512.0
            })
        );
    }

    #[test]
    fn nomipmaps_leaves_picmip_on() {
        let sh = parse_one("t { nomipmaps { map a.tga } }");
        assert!(sh.nomipmaps && !sh.nopicmip);
        let sh = parse_one("t { nopicmip { map a.tga } }");
        assert!(sh.nopicmip && !sh.nomipmaps);
    }

    #[test]
    fn cull_none_is_two_sided() {
        let sh = parse_one("t { cull none { map a.tga } }");
        assert!(sh.two_sided);
        let front = parse_one("t { cull front { map a.tga } }");
        assert!(!front.two_sided);
    }

    #[test]
    fn malformed_numbers_fall_back_without_panicking() {
        let sh = parse_one(
            r#"t {
                skyParms env/x xyz -
                {
                    map first.jpg
                    tcMod scroll abc def
                    rgbGen const ( x y z )
                    blendFunc GL_ONE GL_BOGUS
                    animMap zzz
                    map a.tga
                }
            }"#,
        );
        let st = &sh.stages[0];
        assert_eq!(st.bundles[0].tcmods, vec![TcMod::Scroll(0.0, 0.0)]);
        assert_eq!(st.rgb_gen, RgbGen::Const([0.0; 3]));
        assert_eq!(sh.sky.as_ref().unwrap().cloud_height, 512.0);
        assert_eq!(st.blend, Some((BlendFactor::One, BlendFactor::One)));
        // `animMap` after the first `map` animates it
        assert_eq!(st.bundles[0].anim.as_ref().unwrap().fps, 0.0);
    }

    #[test]
    fn tcgen_lightmap_promotes_bundle_zero() {
        let sh = parse_one("t { { map tex/a.tga tcGen lightmap } }");
        assert_eq!(sh.stages[0].bundles[0].image, ImageRef::Lightmap);
    }

    #[test]
    fn explicit_lightmap_makes_tcgen_lightmap_redundant() {
        let (name, body) =
            first_block("t { { map $lightmap nextbundle map tex/a.tga tcGen lightmap } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(
            sh.stages[0].bundles[1].image,
            ImageRef::Path("tex/a.tga".to_string())
        );
        assert!(w.fired(&name, "redundant tcGen lightmap"));
    }

    #[test]
    fn tcgen_vector_lands_in_current_fill_target_without_warning() {
        let (name, body) = first_block(
            r#"t { { map a.tga nextbundle map $lightmap tcGen vector (.5 0 0) (0 .5 0) } }"#,
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        let st = &sh.stages[0];
        assert_eq!(st.bundles[0].tc_gen, TcGen::Base);
        assert_eq!(
            st.bundles[1].tc_gen,
            TcGen::Vector([0.5, 0.0, 0.0, 0.0, 0.5, 0.0])
        );
        assert_eq!(w.len(), 0, "supported form must not warn");
    }

    #[test]
    fn tcgen_vector_malformed_defaults_to_zero_with_warning() {
        let (name, body) = first_block("t { { map a.tga tcGen vector ( x y z ) ( 0 q 0 ) } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].bundles[0].tc_gen, TcGen::Vector([0.0; 6]));
        assert!(w.fired(&name, "malformed tcGen vector"));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn unsupported_tcgen_forms_drop_the_stage() {
        let (name, body) = first_block("t { { map a.tga tcGen bogus } { map b.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages.len(), 1);
        assert!(w.fired(&name, "unsupported tcGen"));
    }

    #[test]
    fn tcgen_environment_marks_the_current_bundle() {
        let (name, body) = first_block("t { { map env.jpg tcgen environment rgbgen identity } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].bundles[0].tc_gen, TcGen::Environment);
        assert_eq!(w.len(), 0);
    }

    #[test]
    fn depthfunc_and_image_flags_parse() {
        let (name, body) = first_block(
            "t { { map noopt $texturename } { depthfunc equal map $whiteimage blendFunc add } \
             { depthFunc always map a.tga } { depthfunc lequal map a.tga } }",
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(w.len(), 0);
        assert_eq!(
            sh.stages[0].bundles[0].image,
            ImageRef::Path("$texturename".into())
        );
        let funcs: Vec<_> = sh.stages.iter().map(|s| s.depth_func).collect();
        assert_eq!(
            funcs,
            [
                DepthFunc::LessEqual,
                DepthFunc::Equal,
                DepthFunc::Always,
                DepthFunc::LessEqual
            ]
        );
    }

    #[test]
    fn vertex_rgbgen_takes_vertex_alpha_unless_alphagen_was_set() {
        let alpha = |s: &str| {
            parse_one(&format!("t {{ {{ map a.tga {s} }} }}")).stages[0]
                .alpha_gen
                .clone()
        };
        assert_eq!(alpha("rgbGen vertex"), AlphaGen::Vertex);
        assert_eq!(alpha("rgbGen exactVertex"), AlphaGen::Vertex);
        // identity stores retail's unset 0, so the rgbGen still overrides it
        assert_eq!(alpha("alphaGen identity rgbGen vertex"), AlphaGen::Vertex);
        assert_eq!(alpha("rgbGen vertex alphaGen identity"), AlphaGen::Identity);
        assert_eq!(
            alpha("alphaGen const 0.5 rgbGen vertex"),
            AlphaGen::Const(0.5)
        );
        assert_eq!(
            alpha("rgbGen exactVertex alphaGen const 0.5"),
            AlphaGen::Const(0.5)
        );
        assert_eq!(alpha("rgbGen identity"), AlphaGen::Identity);
        assert_eq!(alpha("rgbGen identityLighting"), AlphaGen::Identity);
    }

    #[test]
    fn rgbgen_and_alphagen_wave_parse_end_to_end() {
        let sh = parse_one("t { { map a.tga rgbGen wave sin 0.5 0.25 0 1 } }");
        assert_eq!(
            sh.stages[0].rgb_gen,
            RgbGen::Wave(Wave {
                form: WaveForm::Sin,
                base: 0.5,
                amp: 0.25,
                phase: 0.0,
                freq: 1.0
            })
        );
        let sh = parse_one("t { { map a.tga alphaGen wave square 1 1 0.5 2 } }");
        assert_eq!(
            sh.stages[0].alpha_gen,
            AlphaGen::Wave(Wave {
                form: WaveForm::Square,
                base: 1.0,
                amp: 1.0,
                phase: 0.5,
                freq: 2.0
            })
        );
    }

    /// Parses `text` and returns the shader with the warnings it fired.
    fn parse_warned(text: &str) -> (Shader, WarnSet) {
        let (name, body) = first_block(text);
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        (sh, w)
    }

    /// A keyword's arguments end with its line: stock water's three-number
    /// `turb` neither eats the next line's `rgbGen` nor makes a tcMod.
    #[test]
    fn arguments_end_with_the_line() {
        let (sh, w) = parse_warned(
            "t\n{\n{\nmap a.jpg\ntcMod scroll -0.01 .01\ntcMod turb 1.03 0.2 1.03\nrgbGen exactVertex\nblendFunc add\n}\n}",
        );
        let st = &sh.stages[0];
        assert_eq!(st.bundles[0].tcmods, vec![TcMod::Scroll(-0.01, 0.01)]);
        assert_eq!(st.rgb_gen, RgbGen::ExactVertex);
        assert_eq!(st.blend, Some((BlendFactor::One, BlendFactor::One)));
        assert!(w.is_empty());
        assert!(w.noted("t", "tcMod turb short of parms, dropped"));
    }

    /// `rgbGen wave` takes its genfunc and all four numbers: ui_hud.shader's
    /// `rgbGen wave triangle 0.4 0.1 0 1.2` left `1.2` behind.
    #[test]
    fn gen_wave_consumes_its_frequency() {
        let (sh, w) = parse_warned("t\n{\n{\nmap a.tga\nrgbGen wave triangle 0.4 0.1 0 1.2\n}\n}");
        let RgbGen::Wave(wave) = &sh.stages[0].rgb_gen else {
            panic!("not a wave: {:?}", sh.stages[0].rgb_gen);
        };
        assert_eq!(wave.freq, 1.2);
        assert!(w.is_empty());
    }

    /// surfacelight.shader's three-unit stage: `multiplyImage
    /// identityLighting` under `rgbGen identity` draws as the two-unit
    /// fallback's `rgbGen identityLighting`.
    #[test]
    fn multiply_image_identity_lighting_folds_into_the_gen() {
        let (sh, w) = parse_warned(
            "t\n{\n{\nmultiplyImage identityLighting\nmap a.tga\nblendFunc add\nrgbGen identity\n}\n}",
        );
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::IdentityLighting);
        assert_eq!(sh.stages[0].alpha_gen, AlphaGen::Identity);
        assert!(w.is_empty());
        let (sh, _) = parse_warned("t\n{\n{\nmultiplyImage 0.5 0.25 1 0.5\nmap a.tga\n}\n}");
        assert_eq!(
            sh.stages[0].rgb_gen,
            RgbGen::ConstLighting([0.5, 0.25, 1.0])
        );
        assert_eq!(sh.stages[0].alpha_gen, AlphaGen::Const(0.5));
    }

    #[test]
    fn stock_forms_retail_reads() {
        // ui_menus_mp.shader's tab gradient
        let (sh, w) = parse_warned("t\n{\n{\nmap a.tga\nblendFunc GL_ZERO GL_SRC_COLOR\n}\n}");
        assert_eq!(
            sh.stages[0].blend,
            Some((BlendFactor::Zero, BlendFactor::SrcColor))
        );
        assert!(w.is_empty());
        // common.shader's mirror sorts as a portal
        let (sh, w) = parse_warned("t\n{\nportal\n{\nmap $whiteimage\n}\n}");
        assert_eq!(sh.sort, Some(1.0));
        assert!(w.is_empty());
        // gfx.shader's viewFadeBlack
        let (sh, w) = parse_warned("t\n{\n{\nmap *white\nalphaGen entity\n}\n}");
        assert_eq!(sh.stages[0].alpha_gen, AlphaGen::Entity);
        assert!(w.is_empty());
        // UO: the flamethrower's flag, `depthFunc less` (retail keeps lequal)
        // and a noise wave
        let (sh, w) = parse_warned(
            "t\n{\nnolightscale\n{\nmap a.dds\ndepthFunc less\nrgbgen wave noise 0.4 0.1 0 10\n}\n}",
        );
        let st = &sh.stages[0];
        assert_eq!(st.depth_func, DepthFunc::LessEqual);
        assert!(matches!(&st.rgb_gen, RgbGen::Wave(w) if w.form == WaveForm::Noise));
        assert!(w.is_empty());
    }

    #[test]
    fn unknown_genfunc_reads_as_sin() {
        let (name, body) = first_block("t { { map a.tga rgbGen wave bogus 1 2 3 } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(
            sh.stages[0].rgb_gen,
            RgbGen::Wave(Wave {
                form: WaveForm::Sin,
                base: 1.0,
                amp: 2.0,
                phase: 3.0,
                freq: 0.0
            })
        );
        assert!(w.noted(&name, "invalid genfunc \"bogus\", read as sin"));
        assert!(w.is_empty());
    }

    #[test]
    fn unset_rgbgen_defaults_follow_finish_rule() {
        let plain = parse_one("t { { map a.tga } }");
        assert_eq!(plain.stages[0].rgb_gen, RgbGen::IdentityLighting);
        let src_alpha = parse_one("t { { blendFunc GL_SRC_ALPHA GL_ONE map a.tga } }");
        assert_eq!(src_alpha.stages[0].rgb_gen, RgbGen::IdentityLighting);
        let dst_color = parse_one("t { { blendFunc GL_DST_COLOR GL_ONE map a.tga } }");
        assert_eq!(dst_color.stages[0].rgb_gen, RgbGen::Identity);
    }

    #[test]
    fn map_modifiers_clamp_clampy_heighttonormal() {
        let (name, body) = first_block("t { { map clampY tex/a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].bundles[0].clamp, Clamp { s: false, t: true });
        let (name, body) =
            first_block("t { { map clampx clampY tex/a.tga } { map clampS tex/b.tga } }");
        let sh = parse_shader(&name, &body, &mut WarnSet::new());
        assert_eq!(sh.stages[0].bundles[0].clamp, Clamp::BOTH);
        assert_eq!(sh.stages[1].bundles[0].clamp, Clamp { s: true, t: false });

        let (name, body) = first_block("t { { map heightToNormal 2 tex/b.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].bundles[0].clamp, Clamp::NONE);
        assert!(w.noted(&name, "heightToNormal ignored"));
    }

    #[test]
    fn third_bundle_is_warned_and_ignored() {
        let (name, body) =
            first_block("t { { map a.tga nextbundle map b.tga nextbundle map c.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        let st = &sh.stages[0];
        assert_eq!(st.bundles.len(), 2);
        assert_eq!(st.bundles[1].image, ImageRef::Path("b.tga".to_string()));
        assert!(w.fired(&name, "multiple nextbundle"));
    }

    #[test]
    fn sunfile_and_fogvars_args_are_consumed() {
        let (name, body) = first_block(
            "t { fogvars ( 0.1 0.2 0.3 ) 4 512 sunfile sun/sun.tga nopicmip { map a.tga } }",
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.sunfile.as_deref(), Some("sun/sun.tga"));
        assert!(sh.nopicmip);
        assert_eq!(sh.stages.len(), 1);
        assert!(!w.fired(&name, "unknown token 512"));
    }

    #[test]
    fn sun_file_parses_sprite_cvars() {
        // scripts/mp_harbor.sun: disc disabled, flare only
        let harbor = SunFile::parse("r_sunsprite_shader \"0\"\nr_sunsprite_size \"16\"\n");
        assert_eq!(harbor.sprite, None);
        assert_eq!(harbor.sprite_size, 16.0);
        // scripts/ship.sun: the moon at size 32, and its flare keys
        let ship = SunFile::parse(
            "r_sunsprite_shader \"gfx/sun/moon_full\"\n\nr_sunsprite_size \"32\"\n\
             r_sunflare_shader \"gfx/sun/moon_flare\"\nr_sunflare_min_size \"0\"\n\
             r_sunflare_min_angle \"45\"\nr_sunflare_max_size \"120\"\n\
             r_sunflare_max_angle \"2\"\nr_sunflare_max_alpha \"1\"\n\
             r_sunflare_fadein \"1\"\nr_sunflare_fadeout \".2\"\n\
             r_sunblind_min_angle \"1\"\nr_sunblind_max_darken \".01\"\n\
             r_sunglare_max_lighten \".005\"\r\nr_sunglare_fadein \".1\"\n",
        );
        assert_eq!(ship.sprite.as_deref(), Some("gfx/sun/moon_full"));
        assert_eq!(ship.sprite_size, 32.0);
        assert_eq!(ship.flare.as_deref(), Some("gfx/sun/moon_flare"));
        assert_eq!((ship.flare_max_size, ship.flare_fade_out), (120.0, 0.2));
        assert_eq!((ship.blind.min_angle, ship.blind.max), (1.0, 0.01));
        // untouched keys keep the registration defaults, CRLF lines parse
        assert_eq!((ship.blind.max_angle, ship.blind.fade_out), (5.0, 3.0));
        assert_eq!((ship.glare.max, ship.glare.fade_in), (0.005, 0.1));
        // partial file keeps the size default; unknown keys are ignored
        let partial = SunFile::parse("r_sunflare_shader \"gfx/sun/global_flare\"\nr_bogus 3\n");
        assert_eq!(partial.sprite, None);
        assert_eq!(partial.sprite_size, SunFile::DEFAULT_SPRITE_SIZE);
        assert_eq!(partial.flare_max_size, 2500.0);
    }

    #[test]
    fn parse_deform_vertexes_wave() {
        let (name, body) =
            first_block("t { deformVertexes wave 1600 sin 16 16 0 .25 { map a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        let [Deform::Wave { wave: wv, spread }] = sh.deforms.as_slice() else {
            panic!("ocean wave parsed: {:?}", sh.deforms);
        };
        let (wv, spread) = (wv.clone(), *spread);
        assert_eq!(wv.form, WaveForm::Sin);
        assert_eq!(
            (wv.base, wv.amp, wv.phase, wv.freq),
            (16.0, 16.0, 0.0, 0.25)
        );
        assert!((spread - 1.0 / 1600.0).abs() < 1e-9);
        assert!(!w.fired(&name, "unknown token"), "args consumed cleanly");
    }

    #[test]
    fn parse_deform_vertexes_bulge_move_normal() {
        let (name, body) = first_block(
            "t { deformVertexes bulge 3 4 5\ndeformVertexes move 0 0 2 sin 1 2 0.5 0.25\ndeformVertexes normal .5 2 { map a.tga } }",
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        let [
            Deform::Bulge(bulge),
            Deform::Move { by, wave },
            Deform::Normal(normal),
        ] = sh.deforms.as_slice()
        else {
            panic!("bulge, move, normal in order: {:?}", sh.deforms);
        };
        assert_eq!(
            *bulge,
            Bulge {
                width: 3.0,
                height: 4.0,
                speed: 5.0
            }
        );
        let (by, wave) = (*by, wave.clone());
        assert_eq!(by, [0.0, 0.0, 2.0]);
        assert_eq!(
            (wave.form, wave.base, wave.amp, wave.phase, wave.freq),
            (WaveForm::Sin, 1.0, 2.0, 0.5, 0.25)
        );
        assert_eq!(
            *normal,
            DeformNormal {
                amp: 0.5,
                freq: 2.0
            }
        );
        assert!(w.is_empty(), "args consumed cleanly");
        // short ones warn as retail does and deform nothing
        let (name, body) =
            first_block("t { deformVertexes bulge 3 4\ndeformVertexes normal 1 { map a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert!(sh.deforms.is_empty());
        assert!(w.fired(&name, "missing deformVertexes bulge parm"));
        assert!(w.fired(&name, "missing deformVertexes parm"));
    }

    #[test]
    fn parse_deform_vertexes_flap_and_sprites() {
        // skins/shipflag_np (jeff.shader)
        let (name, body) =
            first_block("t { cull none deformVertexes flap s 32 sin 4 8 0 .75 { map a.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        let [Deform::Flap(flap)] = sh.deforms.as_slice() else {
            panic!("flap parsed: {:?}", sh.deforms);
        };
        assert!(!flap.along_t);
        assert_eq!(
            (
                flap.wave.base,
                flap.wave.amp,
                flap.wave.phase,
                flap.wave.freq
            ),
            (4.0, 8.0, 0.0, 0.75)
        );
        assert!((flap.spread - 1.0 / 32.0).abs() < 1e-9);
        assert!(sh.two_sided && sh.stages.len() == 1);
        // the parser maps y with t and x with s
        let (name, body) = first_block("t { deformVertexes flap y 0 sin 1 2 0 1 { map a.tga } }");
        let sh = parse_shader(&name, &body, &mut WarnSet::new());
        let [Deform::Flap(flap)] = sh.deforms.as_slice() else {
            panic!("flap parsed: {:?}", sh.deforms);
        };
        assert!(flap.along_t);
        assert_eq!(flap.spread, 100.0, "div 0 stores 100");
        // skins/light_sprite and skins/searchlight_beam, either case
        let (name, body) = first_block("t { deformvertexes autosprite { map a.tga } }");
        let sh = parse_shader(&name, &body, &mut WarnSet::new());
        assert_eq!(sh.sprite, Some(Sprite::Auto));
        assert_eq!(sh.stages.len(), 1);
        let (name, body) = first_block("t { DeformVertexes autosprite2 { map a.tga } }");
        let sh = parse_shader(&name, &body, &mut WarnSet::new());
        assert_eq!(sh.sprite, Some(Sprite::Auto2));
        // stalingradwater_pj's syncNormal rides along inertly
        let (name, body) = first_block(
            "t { deformVertexes wave 100 sin 2 2 0 .25 deformVertexes syncNormal 0.25 { map a.tga } }",
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert!(matches!(
            sh.deforms.as_slice(),
            [Deform::Wave { .. }, Deform::Other]
        ));
        assert!(sh.sprite.is_none());
        assert!(!w.fired(&name, "unknown token"));
    }

    /// Records keep script order, a sprite or an inert one holds a slot, and
    /// a fourth warns `MAX_SHADER_DEFORMS` and is dropped (0x4f9d20).
    #[test]
    fn deforms_keep_script_order_up_to_three() {
        let (name, body) = first_block(
            "t { deformVertexes move 0 0 1 sin 0 1 0 1\ndeformVertexes autosprite\ndeformVertexes wave 64 sin 0 2 0 1\ndeformVertexes normal 1 1 { map a.tga } }",
        );
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert!(matches!(
            sh.deforms.as_slice(),
            [Deform::Move { .. }, Deform::Other, Deform::Wave { .. }]
        ));
        assert_eq!(sh.sprite, Some(Sprite::Auto));
        assert!(w.fired(&name, "MAX_SHADER_DEFORMS"));
        assert!(
            !w.fired(&name, "unknown token"),
            "the fourth's args consumed"
        );
        assert_eq!(sh.stages.len(), 1);
        // a syncNormal off a wave takes a slot; one with nothing before it none
        let (name, body) = first_block(
            "t { deformVertexes syncNormal 0.5\ndeformVertexes bulge 1 2 3 { map a.tga } }",
        );
        let sh = parse_shader(&name, &body, &mut WarnSet::new());
        assert!(matches!(sh.deforms.as_slice(), [Deform::Bulge(_)]));
    }

    #[test]
    fn parse_wave_forms_and_defaults() {
        let w = parse_wave(&["sin", "0.5", "1.5", "0.25", "2"]).unwrap();
        assert_eq!(w.form, WaveForm::Sin);
        assert_eq!((w.base, w.amp, w.phase, w.freq), (0.5, 1.5, 0.25, 2.0));
        let inv = parse_wave(&["invsawtooth", "0", "1", "0", "1"]).unwrap();
        assert_eq!(inv.form, WaveForm::InverseSawtooth);
        assert_eq!(parse_wave(&["bogus", "1", "1", "1", "1"]), None);
        assert!(parse_wave(&[]).is_none());
    }

    #[test]
    fn sort_tokens_map_per_the_cod_name_table() {
        for (tok, v) in [
            ("portal", 1.0),
            ("sky", 2.0),
            ("ocean", 3.0),
            ("boathull", 4.0),
            ("opaque", 5.0),
            ("decal", 6.0),
            ("seeThrough", 7.0),
            ("banner", 8.0),
            ("underwater", 10.0),
            ("water", 11.0),
            ("corona", 12.0),
            ("innerBlend", 13.0),
            ("outerBlend", 14.0),
            ("blend", 15.0),
            ("blend2", 16.0),
            ("blend3", 17.0),
            ("blend4", 18.0),
            ("additive", 19.0),
            ("nearest", 22.0),
        ] {
            assert_eq!(map_sort_token(tok), Some(v), "{tok}");
        }
        // numeric parse wins over the name table
        assert_eq!(map_sort_token("16"), Some(16.0));
        assert_eq!(map_sort_token("WATER"), Some(11.0));
        // RTCW-only names are not in CoD's table
        assert_eq!(map_sort_token("almostNearest"), None);
        assert_eq!(map_sort_token("bogus"), None);
    }

    // ---- classification and sort mapping ----

    fn stage_of(blend: Option<(BlendFactor, BlendFactor)>, depth_write: Option<bool>) -> Stage {
        Stage {
            bundles: Vec::new(),
            blend,
            depth_write,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }
    }

    fn shader_of(stages: Vec<Stage>) -> Shader {
        Shader {
            name: "t".to_string(),
            stages,
            ..Default::default()
        }
    }

    #[test]
    fn explicit_sort_beats_every_derived_default() {
        let mut sh = shader_of(vec![stage_of(
            Some((BlendFactor::One, BlendFactor::One)),
            None,
        )]);
        sh.sky = Some(SkyParms {
            env: String::new(),
            cloud_height: 512.0,
        });
        sh.polygon_offset = Some(PolygonOffset::Single);
        for v in [1.0, 2.0, SORT_OPAQUE, SORT_DECAL, 15.0] {
            sh.sort = Some(v);
            assert_eq!(sh.sort_value(), v, "{v}");
        }
    }

    #[test]
    fn derived_sort_defaults_follow_finish_rule_order() {
        assert_eq!(shader_of(Vec::new()).sort_value(), SORT_OPAQUE);
        assert_eq!(
            shader_of(vec![stage_of(None, None)]).sort_value(),
            SORT_OPAQUE
        );

        let mut sky = shader_of(vec![stage_of(None, None)]);
        sky.sky = Some(SkyParms {
            env: String::new(),
            cloud_height: 512.0,
        });
        assert_eq!(sky.sort_value(), 2.0);

        let mut decal = shader_of(vec![stage_of(None, None)]);
        decal.polygon_offset = Some(PolygonOffset::Single);
        assert_eq!(decal.sort_value(), SORT_DECAL);
        // sky outranks polygon offset
        let mut sky_decal = sky;
        sky_decal.polygon_offset = Some(PolygonOffset::Single);
        assert_eq!(sky_decal.sort_value(), 2.0);

        // stage0 blended and still writing depth -> see-through
        let seethrough = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            Some(true),
        )]);
        assert_eq!(seethrough.sort_value(), SORT_SEETHROUGH);
        // stage0 blended without depth write -> blend, or additive onto GL_ONE
        let blend = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            None,
        )]);
        assert_eq!(blend.sort_value(), SORT_BLEND);
        let add = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::One)),
            None,
        )]);
        assert_eq!(add.sort_value(), SORT_ADDITIVE);
        // (One, Zero) is still a blend here but keeps depth write
        let degen = shader_of(vec![stage_of(
            Some((BlendFactor::One, BlendFactor::Zero)),
            None,
        )]);
        assert_eq!(degen.sort_value(), SORT_SEETHROUGH);
        assert!(degen.classify_stage(0).unwrap().depth_write);
        // polygon offset outranks the stage-derived keys
        let mut decal_blend = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            None,
        )]);
        decal_blend.polygon_offset = Some(PolygonOffset::Single);
        assert_eq!(decal_blend.sort_value(), SORT_DECAL);
    }

    #[test]
    fn stage_depth_write_rules() {
        for (blend, explicit, want) in [
            (None, None, true),
            (None, Some(true), true),
            (None, Some(false), false),
            (
                Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                None,
                false,
            ),
            (
                Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                Some(true),
                true,
            ),
            (Some((BlendFactor::One, BlendFactor::Zero)), None, true),
            (
                Some((BlendFactor::One, BlendFactor::Zero)),
                Some(false),
                false,
            ),
        ] {
            let sh = shader_of(vec![stage_of(blend.clone(), explicit)]);
            let cs = sh.classify_stage(0).unwrap();
            assert_eq!(cs.depth_write, want, "{blend:?} {explicit:?}");
        }
    }

    #[test]
    fn draw_class_from_dst_factor() {
        for (blend, want) in [
            (None, DrawClass::Opaque),
            (
                Some((BlendFactor::One, BlendFactor::One)),
                DrawClass::Additive,
            ),
            (
                Some((BlendFactor::SrcAlpha, BlendFactor::One)),
                DrawClass::Additive,
            ),
            (
                Some((BlendFactor::DstColor, BlendFactor::SrcAlphaSaturate)),
                DrawClass::Additive,
            ),
            (
                Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
                DrawClass::Blend,
            ),
            (
                Some((BlendFactor::One, BlendFactor::Zero)),
                DrawClass::Blend,
            ),
            (
                Some((BlendFactor::One, BlendFactor::OneMinusSrcColor)),
                DrawClass::Blend,
            ),
        ] {
            let sh = shader_of(vec![stage_of(blend.clone(), None)]);
            assert_eq!(sh.classify_stage(0).unwrap().class, want, "{blend:?}");
        }
    }

    #[test]
    fn out_of_range_stage_is_none() {
        let sh = shader_of(vec![stage_of(None, None)]);
        assert_eq!(sh.classify_stage(1), None);
        assert_eq!(shader_of(Vec::new()).classify_stage(0), None);
    }

    #[test]
    fn bias_and_two_sided_flags() {
        let mut sh = shader_of(vec![stage_of(None, None)]);
        let cs = sh.classify_stage(0).unwrap();
        assert!(!cs.bias);
        assert!(!cs.two_sided);
        sh.two_sided = true;
        assert!(sh.classify_stage(0).unwrap().two_sided);

        sh.two_sided = false;
        sh.polygon_offset = Some(PolygonOffset::Single);
        assert!(sh.classify_stage(0).unwrap().bias);

        let mut named = shader_of(vec![stage_of(None, None)]);
        named.sort = map_sort_token("decal");
        assert!(named.classify_stage(0).unwrap().bias);
        assert_eq!(named.sort_value(), SORT_DECAL);

        let mut num = shader_of(vec![stage_of(None, None)]);
        num.sort = Some(6.0);
        assert!(num.classify_stage(0).unwrap().bias);

        let mut other = shader_of(vec![stage_of(None, None)]);
        other.sort = Some(6.25);
        assert!(!other.classify_stage(0).unwrap().bias);
    }

    #[test]
    fn every_polygon_offset_form_sorts_decal_and_biases() {
        for (kw, form) in [
            ("polygonOffset", PolygonOffset::Single),
            ("polygonOffset2", PolygonOffset::Double),
            ("polygonOffsetConst", PolygonOffset::Const),
        ] {
            let sh = parse_one(&format!("t {{ {kw} {{ map a.tga blendFunc blend }} }}"));
            assert_eq!(sh.sort_value(), SORT_DECAL, "{kw}");
            assert!(sh.classify_stage(0).unwrap().bias, "{kw}");
            assert_eq!(sh.depth_bias(), Some(form), "{kw}");
        }
        // the last keyword wins, as each clears the other two bits
        let sh = parse_one("t { polygonOffset2 polygonOffset { map a.tga } }");
        assert_eq!(sh.depth_bias(), Some(PolygonOffset::Single));
        // a decal sort without a keyword draws the single offset
        let sh = parse_one("t { sort decal { map a.tga } }");
        assert_eq!(sh.depth_bias(), Some(PolygonOffset::Single));
    }

    #[test]
    fn nofog_is_parsed() {
        assert!(parse_one("markShadow { nofog polygonOffset { map a.tga } }").nofog);
        assert!(!parse_one("t { { map a.tga } }").nofog);
    }

    #[test]
    fn classify_end_to_end_through_the_parser() {
        let sh = parse_one(
            "t { cull none polygonoffset sort decal \
             { blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA map a.tga depthWrite } }",
        );
        assert_eq!(sh.sort_value(), SORT_DECAL);
        assert_eq!(
            sh.classify_stage(0),
            Some(ClassedStage {
                class: DrawClass::Blend,
                depth_write: true,
                bias: true,
                two_sided: true,
            })
        );
    }

    // ---- material compile ----

    #[test]
    fn a_compiled_material_resolves_what_the_script_leaves_implicit() {
        let m = Material::from_shader(&parse_one(
            "t { cull none sort decal { map a.tga } { map $lightmap blendFunc filter } }",
        ));
        assert_eq!(m.technique, Technique::Stages);
        assert_eq!(m.sort, SORT_DECAL);
        assert_eq!(m.depth_bias, Some(PolygonOffset::Single));
        assert_eq!(m.cull, Cull::None);
        assert_eq!(m.stages.len(), 2);
        // sorted past opaque: no dlight pass
        assert_eq!(m.dlight, None);
        // the collapsed pair sorts by texture then lightmap
        assert_eq!(
            m.sort_images,
            Some(vec![ImageRef::Path("a.tga".into()), ImageRef::Lightmap])
        );
        assert_eq!(
            m.texture(TexSemantic::Color),
            Some(&ImageRef::Path("a.tga".into()))
        );
        assert_eq!(m.texture(TexSemantic::Lightmap), Some(&ImageRef::Lightmap));

        let blend = Material::from_shader(&parse_one("t { { map a.tga blendFunc add } }"));
        assert_eq!(blend.sort, SORT_ADDITIVE);
        assert_eq!(blend.depth_bias, None);
        assert_eq!(blend.cull, Cull::Back);
        assert_eq!(blend.texture(TexSemantic::Lightmap), None);

        let sky = parse_one("s { skyparms env/sky 512 - }");
        assert_eq!(Material::from_shader(&sky).technique, Technique::Sky);
        assert_eq!(
            Material::from_shader(&parse_one("n { surfaceparm nodraw }")).technique,
            Technique::None
        );
    }

    // ---- library ----

    fn make_pk3(dir: &std::path::Path, file: &str, script: &str) {
        use std::io::Write;
        let f = std::fs::File::create(dir.join(file)).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file("scripts/s.shader", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(script.as_bytes()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn later_archive_wins_and_anonymous_blocks_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(
            dir.path(),
            "pak0.pk3",
            "textures/x/dupe\n{\n\t{\n\t\tmap textures/x/a.tga\n\t}\n}\n",
        );
        // the glued `}{` block is a nameless continuation, not a material
        make_pk3(
            dir.path(),
            "pak1.pk3",
            "textures/x/dupe\n{\n\t{\n\t\tmap textures/x/b.tga\n\t}\n}{\n\tmap $lightmap\n}\n",
        );
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let lib = ShaderLib::load(&fs);
        assert_eq!(lib.len(), 1);
        let sh = lib.get("textures/x/dupe").unwrap();
        assert_eq!(
            sh.stages[0].bundles[0].image,
            ImageRef::Path("textures/x/b.tga".to_string())
        );
        assert!(
            lib.image("textures/x/dupe")
                .is_some_and(|p| p.ends_with("b"))
        );
    }

    #[test]
    fn the_library_parses_once_until_the_cache_clears() {
        let dir = tempfile::tempdir().unwrap();
        make_pk3(dir.path(), "pak0.pk3", "a\n{\n}\n");
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let a = ShaderLib::load(&fs);
        assert!(Arc::ptr_eq(&a, &ShaderLib::load(&fs)));
        // a script read marks no pak (FS_FOpenFileRead skips `.shader`)
        assert_eq!(fs.referenced_paks().len(), 0);
        fs.clear_references();
        let b = ShaderLib::load(&fs);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(b.len(), 1);
    }
}
