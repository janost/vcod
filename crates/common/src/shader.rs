//! Q3-style shader scripts: the type model, the block splitter and the stage
//! parser shared by every later parsing pass.

use std::collections::{HashMap, HashSet};

use crate::pk3::Pk3Fs;

#[derive(Debug, Clone, PartialEq)]
pub enum BlendFactor {
    Zero,
    One,
    DstColor,
    OneMinusDstColor,
    OneMinusSrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WaveForm {
    Sin,
    Square,
    Triangle,
    Sawtooth,
    InverseSawtooth,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wave {
    pub form: WaveForm,
    pub base: f32,
    pub amp: f32,
    pub phase: f32,
    pub freq: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TcMod {
    Scroll(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    Stretch(Wave),
    Turb { amp: f32, phase: f32, freq: f32 },
    Transform([f32; 6]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImageRef {
    Path(String),
    Lightmap,
    White,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimSpec {
    pub fps: f32,
    pub paths: Vec<String>,
}

/// One `map`/`animMap` group inside a stage; a stage has one or more.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub image: ImageRef,
    pub anim: Option<AnimSpec>,
    pub clamp: bool,
    pub tcmods: Vec<TcMod>,
    /// `tcGen vector` basis, sx sy sz tx ty tz; renderer dots world position.
    pub vector: Option<[f32; 6]>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaFunc {
    Gt0,
    Lt128,
    Ge128,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RgbGen {
    IdentityLighting,
    Identity,
    ExactVertex,
    Vertex,
    Const([f32; 3]),
    ConstLighting([f32; 3]),
    Wave(Wave),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaGen {
    Identity,
    Vertex,
    Const(f32),
    Wave(Wave),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub bundles: Vec<Bundle>,
    pub blend: Option<(BlendFactor, BlendFactor)>,
    /// `depthWrite` seen explicitly; absent means stage-default.
    pub depth_write: Option<bool>,
    pub alpha_func: Option<AlphaFunc>,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyParms {
    pub env: String,
    pub cloud_height: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceBits {
    pub sky: bool,
    pub nodraw: bool,
    pub trans: bool,
    pub water: bool,
    pub nonsolid: bool,
    pub nolightmap: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shader {
    pub name: String,
    pub stages: Vec<Stage>,
    pub two_sided: bool,
    pub sort: Option<f32>,
    pub polygon_offset: bool,
    pub nopicmip: bool,
    pub nomipmaps: bool,
    pub sky: Option<SkyParms>,
    pub sunfile: Option<String>,
    pub surface: SurfaceBits,
}

/// Script tokens: comments (`//` to end of line, `/* */` across lines) gone,
/// glued braces split off as standalone `{`/`}` tokens. Slices point into
/// `text`; delimiters are ASCII, so token boundaries are always char edges.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    std::iter::from_fn(move || loop {
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
/// parse directly and win.
pub fn map_sort_token(tok: &str) -> Option<f32> {
    if let Ok(v) = tok.parse::<f32>() {
        return Some(v);
    }
    match tok.to_ascii_lowercase().as_str() {
        "portal" => Some(1.0),
        "sky" => Some(2.0),
        "opaque" => Some(3.0),
        "decal" => Some(4.0),
        "seethrough" | "see" => Some(5.0),
        "banner" => Some(6.0),
        "underwater" => Some(8.0),
        "water" | "ocean" => Some(8.75),
        "outer" | "outerblend" => Some(9.0),
        "inner" | "innerblend" | "additive" => Some(10.0),
        "almostnearest" => Some(14.0),
        "nearest" => Some(15.0),
        _ => None,
    }
}

pub const SORT_OPAQUE: f32 = 3.0;
pub const SORT_DECAL: f32 = 4.0;
pub const SORT_SEETHROUGH: f32 = 5.0;
pub const SORT_BANNER: f32 = 6.0;
pub const SORT_WATER: f32 = 8.75;
pub const SORT_BLEND0: f32 = 9.0;
pub const SORT_ADDITIVE: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawClass {
    Opaque,
    Blend,
    Additive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassedStage {
    pub class: DrawClass,
    pub depth_write: bool,
    pub bias: bool,
    pub two_sided: bool,
}

/// Start true; a blend clears it unless it degenerates to (One, Zero); an
/// explicit `depthWrite` keyword wins over both.
fn depth_write(st: &Stage) -> bool {
    if let Some(dw) = st.depth_write {
        return dw;
    }
    match &st.blend {
        None => true,
        Some(pair) => *pair == (BlendFactor::One, BlendFactor::Zero),
    }
}

impl Shader {
    /// Explicit `sort` wins; otherwise the RTCW FinishShader defaults in
    /// sky > polygon offset > stage-0 blend order.
    pub fn sort_value(&self) -> f32 {
        self.sort.unwrap_or_else(|| {
            if self.sky.is_some() {
                2.0
            } else if self.polygon_offset {
                SORT_DECAL
            } else {
                match self.stages.first().map(|s| &s.blend) {
                    Some(Some(_)) if depth_write(self.stages.first().unwrap()) => SORT_SEETHROUGH,
                    Some(Some(_)) => SORT_BLEND0,
                    _ => SORT_OPAQUE,
                }
            }
        })
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
        let bias = self.polygon_offset || self.sort == Some(SORT_DECAL);
        Some(ClassedStage {
            class,
            depth_write: depth_write(st),
            bias,
            two_sided: self.two_sided,
        })
    }
}

/// Dedupes warnings by `(shader name, message)` so a malformed script shape
/// logs once no matter how many shaders hit it.
pub struct WarnSet {
    seen: HashSet<String>,
}

impl WarnSet {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
        }
    }

    pub fn warn_once(&mut self, name: &str, msg: &str) {
        if self.seen.insert(format!("{name}\0{msg}")) {
            log::warn!("{name}: {msg}");
        }
    }

    #[cfg(test)]
    fn fired(&self, name: &str, msg: &str) -> bool {
        self.seen.contains(&format!("{name}\0{msg}"))
    }

    #[cfg(test)]
    fn entries(&self) -> usize {
        self.seen.len()
    }
}

impl Default for WarnSet {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_wave(args: &[&str]) -> Option<Wave> {
    let form = match args.first()?.to_ascii_lowercase().as_str() {
        "sin" => WaveForm::Sin,
        "square" => WaveForm::Square,
        "triangle" => WaveForm::Triangle,
        "sawtooth" => WaveForm::Sawtooth,
        "invsawtooth" | "inversesawtooth" => WaveForm::InverseSawtooth,
        _ => return None,
    };
    let num = |i: usize| args.get(i + 1).map_or(0.0, |t| fnum(t));
    Some(Wave {
        form,
        base: num(0),
        amp: num(1),
        phase: num(2),
        freq: num(3),
    })
}

/// Accepts `GL_`-prefixed and case/underscore variants; `SrcColor` has no
/// contract variant, so it falls through to the unknown-factor path.
pub fn parse_blend_factor(tok: &str) -> Option<BlendFactor> {
    let t = tok.to_ascii_lowercase();
    let t = t.strip_prefix("gl_").unwrap_or(&t);
    Some(match t.replace('_', "").as_str() {
        "zero" => BlendFactor::Zero,
        "one" => BlendFactor::One,
        "dstcolor" => BlendFactor::DstColor,
        "oneminusdstcolor" => BlendFactor::OneMinusDstColor,
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
                // the only numeric atom CoD scripts use is the texture-unit count
                if ident.eq_ignore_ascii_case("gl_max_texture_units_arb") {
                    compare(4.0, ts[p], n)
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
                | "tcmod"
                | "tcgen"
                | "rgbgen"
                | "alphagen"
                | "requires"
                | "nopicmip"
                | "nomipmaps"
                | "polygonoffset"
                | "cull"
                | "sort"
                | "surfaceparm"
                | "skyparms"
                | "sunfile"
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
    tok == "{" || tok == "}" || is_known_kw(tok)
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
    rgb_gen: Option<RgbGen>,
    alpha_gen: Option<AlphaGen>,
    /// `tcGen lightmap` seen before bundle 0 existed.
    want_lm: bool,
}

impl StageBuf {
    fn finish(mut self) -> Option<Stage> {
        if !self.alive || self.bundles.is_empty() {
            return None;
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
        let alpha_gen = self.alpha_gen.unwrap_or(AlphaGen::Identity);
        Some(Stage {
            bundles: self.bundles,
            blend: self.blend,
            depth_write: self.depth_write,
            alpha_func: self.alpha_func,
            rgb_gen,
            alpha_gen,
        })
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
            slot.vector = b.vector;
        } else {
            self.bundles.push(b);
        }
    }
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
                    sb.blend = Some(pair);
                    usize::from(!args.is_empty())
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
                    sb.blend = Some((factor(0), factor(1)));
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
        "tcmod" => tc_mod(sb, args, sname, warns),
        "tcgen" => tc_gen(sb, args, sname, warns),
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
    let mut clamp = clamp_kw;
    let mut k = 0;
    while let Some(m) = args.get(k).map(|t| t.to_ascii_lowercase()) {
        match m.as_str() {
            "clamp" => clamp = true,
            "clampy" => {
                clamp = true;
                warns.warn_once(sname, "clampY approximated");
            }
            "heighttonormal" => warns.warn_once(sname, "heightToNormal ignored"),
            _ => break,
        }
        k += 1;
    }
    let Some(path) = args.get(k) else { return k };
    sb.place_bundle(Bundle {
        image: image_ref(path),
        anim: None,
        clamp,
        tcmods: Vec::new(),
        vector: None,
    });
    k + 1
}

fn add_anim(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    let fps = args.first().map_or(0.0, |t| fnum(t));
    let n = until_keyword(&args[1.min(args.len())..]);
    let paths: Vec<String> = args[1.min(args.len())..1 + n]
        .iter()
        .map(|p| norm_path(p))
        .collect();
    if paths.is_empty() {
        warns.warn_once(sname, "animMap without frames");
        return 1.min(args.len());
    }
    sb.place_bundle(Bundle {
        image: ImageRef::Path(paths[0].clone()),
        anim: Some(AnimSpec { fps, paths }),
        clamp: false,
        tcmods: Vec::new(),
        vector: None,
    });
    1 + n
}

fn tc_mod(sb: &mut StageBuf, args: &[&str], sname: &str, warns: &mut WarnSet) -> usize {
    let Some(sub) = args.first().map(|t| t.to_ascii_lowercase()) else {
        return 0;
    };
    let g = |i: usize| args.get(i + 1).map_or(0.0, |t| fnum(t));
    let push = |sb: &mut StageBuf, m: TcMod| {
        if let Some(b) = sb.bundles.get_mut(sb.target) {
            b.tcmods.push(m);
        }
    };
    let (tcmod, n) = match sub.as_str() {
        "scroll" => (Some(TcMod::Scroll(g(0), g(1))), 2),
        "scale" => (Some(TcMod::Scale(g(0), g(1))), 2),
        "rotate" => (Some(TcMod::Rotate(g(0))), 1),
        "stretch" => (
            parse_wave(&args[1..]).map(TcMod::Stretch),
            4.min(args.len().saturating_sub(1)),
        ),
        // base amplitude is parsed but unused, like RTCW
        "turb" => (
            Some(TcMod::Turb {
                amp: g(1),
                phase: g(2),
                freq: g(3),
            }),
            4,
        ),
        "transform" => (
            Some(TcMod::Transform([g(0), g(1), g(2), g(3), g(4), g(5)])),
            6,
        ),
        _ => {
            warns.warn_once(sname, &format!("unknown tcMod {sub}"));
            return 1 + until_keyword(&args[1..]);
        }
    };
    match tcmod {
        Some(m) => push(sb, m),
        None => warns.warn_once(sname, "unknown wave form"),
    }
    1 + n
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
                b.vector = Some(vals);
            }
            k
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
            // args[0] is the `wave` keyword itself; parse_wave wants the form
            match parse_wave(&args[1..]) {
                Some(w) => {
                    if rgb {
                        sb.rgb_gen = Some(RgbGen::Wave(w));
                    } else {
                        sb.alpha_gen = Some(AlphaGen::Wave(w));
                    }
                }
                None => warns.warn_once(sname, &format!("unknown wave form in {kind}")),
            }
            1 + 4.min(args.len().saturating_sub(1))
        }
        "exactvertex" if rgb => {
            sb.rgb_gen = Some(RgbGen::ExactVertex);
            1
        }
        "identitylighting" if rgb => {
            sb.rgb_gen = Some(RgbGen::IdentityLighting);
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
        _ => {
            warns.warn_once(sname, &format!("unknown {kind} {form}"));
            1 + until_keyword(&args[1..])
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
        "skyfogvars" | "waterfogvars" | "fogvars" | "nofog" | "entitymergable" | "tesssize"
        | "light" => until_keyword(args),
        _ if lower.starts_with("qer_") || lower.starts_with("q3map_") => until_keyword(args),
        "nopicmip" => {
            sh.nopicmip = true;
            0
        }
        "nomipmaps" => {
            sh.nomipmaps = true;
            sh.nopicmip = true;
            0
        }
        "polygonoffset" => {
            sh.polygon_offset = true;
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
                        tok if sb.alive && depth == 1 => {
                            // kw plus however many args the handler ate
                            i += 1 + stage_token(&mut sb, tok, &body[i + 1..], name, warns);
                            continue;
                        }
                        _ => {} // failed requires: skip straight to the closing brace
                    }
                    i += 1;
                }
                if let Some(st) = sb.finish() {
                    sh.stages.push(st);
                }
            }
            "}" => i += 1,
            tok => {
                i += 1 + top_token(&mut sh, tok, &body[i + 1..], name, warns, &mut pending_req);
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
}

impl ShaderLib {
    pub fn load(fs: &Pk3Fs) -> Self {
        let mut lib = Self {
            by_name: HashMap::new(),
            images: HashMap::new(),
        };
        // one set for the whole scan: repeated damage across many files
        // warns once per (shader, message)
        let mut warns = WarnSet::new();
        for path in fs.names_with_suffix(".shader") {
            let Some(raw) = fs.read(&path) else { continue };
            let text = String::from_utf8_lossy(&raw);
            for (name, body) in split_blocks(&text) {
                // glued `}{` continuation blocks carry no name of their own
                if name.is_empty() {
                    continue;
                }
                let sh = parse_shader(&name, &body, &mut warns);
                // from the raw body, not the parsed stages: a gated-out or
                // dropped stage must still yield the material's texture
                if let Some(img) = body_image(&body) {
                    lib.images.insert(name.clone(), img);
                }
                lib.by_name.insert(name, sh);
            }
        }
        lib
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
        self.get(name).is_some_and(|s| s.polygon_offset)
    }

    pub fn sky_blocks(&self) -> impl Iterator<Item = &Shader> {
        self.by_name.values().filter(|s| s.sky.is_some())
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

fn norm_key(name: &str) -> String {
    name.to_lowercase().replace('\\', "/")
}

/// The first `map`/`clampmap` image of a raw block body, normalized the way
/// the minimal parser stored them: lowercased, leading slash and known image
/// extension gone, `$lightmap`/`*white` skipped, `clamp`/`clampY`/
/// `heightToNormal` treated as modifiers before the path.
fn body_image(body: &[&str]) -> Option<String> {
    let mut want = false;
    for &tok in body {
        let t = tok.to_ascii_lowercase();
        if want {
            want = false;
            if matches!(t.as_str(), "clamp" | "clampy" | "heighttonormal") {
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

// ---- tcMod runtime evaluation ----
// Math pinned to RTCW-MP src/renderer/tr_shade_calc.c; table build loop is
// tr_init.c R_Init.

const FUNCTABLE_SIZE: usize = 1024;
const FUNCTABLE_MASK: i64 = (FUNCTABLE_SIZE - 1) as i64;

/// Compile-time stand-in for libm sin(): quadrant reduction then Maclaurin.
const fn sin_deg(deg: f64) -> f64 {
    let mut x = deg % 360.0;
    if x > 180.0 {
        x -= 360.0;
    } else if x < -180.0 {
        x += 360.0;
    }
    if x > 90.0 {
        x = 180.0 - x;
    } else if x < -90.0 {
        x = -180.0 - x;
    }
    let r = x * std::f64::consts::PI / 180.0;
    let sq = -r * r;
    let mut term = r;
    let mut sum = r;
    let mut k = 2.0f64;
    while k < 30.0 {
        term *= sq / (k * (k + 1.0));
        sum += term;
        k += 2.0;
    }
    sum
}

const fn build_sin_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        // RTCW spans the table over SIZE-1 degrees of arc, so entry 256 is
        // sin(90.088deg), not exactly 1
        t[i] = sin_deg(i as f64 * 360.0 / (FUNCTABLE_SIZE - 1) as f64) as f32;
        i += 1;
    }
    t
}

const fn build_square_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = if i < FUNCTABLE_SIZE / 2 { 1.0 } else { -1.0 };
        i += 1;
    }
    t
}

const fn build_sawtooth_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = i as f32 / FUNCTABLE_SIZE as f32;
        i += 1;
    }
    t
}

const fn build_inverse_sawtooth_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = 1.0 - i as f32 / FUNCTABLE_SIZE as f32;
        i += 1;
    }
    t
}

const fn build_triangle_table() -> [f32; FUNCTABLE_SIZE] {
    let mut t = [0.0f32; FUNCTABLE_SIZE];
    let mut i = 0usize;
    while i < FUNCTABLE_SIZE {
        t[i] = if i < FUNCTABLE_SIZE / 2 {
            if i < FUNCTABLE_SIZE / 4 {
                i as f32 / (FUNCTABLE_SIZE / 4) as f32
            } else {
                1.0 - t[i - FUNCTABLE_SIZE / 4]
            }
        } else {
            -t[i - FUNCTABLE_SIZE / 2]
        };
        i += 1;
    }
    t
}

const SIN_TABLE: [f32; FUNCTABLE_SIZE] = build_sin_table();
const SQUARE_TABLE: [f32; FUNCTABLE_SIZE] = build_square_table();
const TRIANGLE_TABLE: [f32; FUNCTABLE_SIZE] = build_triangle_table();
const SAWTOOTH_TABLE: [f32; FUNCTABLE_SIZE] = build_sawtooth_table();
const INVERSE_SAWTOOTH_TABLE: [f32; FUNCTABLE_SIZE] = build_inverse_sawtooth_table();

/// WAVEVALUE (tr_shade_calc.c:34): base + table[idx]*amp with
/// idx = ftol(phase + t*freq, scaled by 1024) & 1023. myftol is x87 fistp,
/// which rounds to nearest even, so 511.9 indexes 512 where an `as` cast
/// would truncate to 511.
pub fn wave_value(w: &Wave, t: f32) -> f32 {
    let table = match w.form {
        WaveForm::Sin => &SIN_TABLE,
        WaveForm::Square => &SQUARE_TABLE,
        WaveForm::Triangle => &TRIANGLE_TABLE,
        WaveForm::Sawtooth => &SAWTOOTH_TABLE,
        WaveForm::InverseSawtooth => &INVERSE_SAWTOOTH_TABLE,
    };
    let x = (w.phase + t * w.freq) * FUNCTABLE_SIZE as f32;
    let idx = x.round_ties_even() as i64 & FUNCTABLE_MASK;
    w.base + table[idx as usize] * w.amp
}

/// Column-major like wgpu mat3x2: [a,b,c,d,tx,ty] means
/// s' = a*s + c*t + tx ; t' = b*s + d*t + ty.
type Affine = [f32; 6];

const AFFINE_IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// outer applied after inner.
fn compose_affine(o: &Affine, i: &Affine) -> Affine {
    [
        o[0] * i[0] + o[2] * i[1],
        o[1] * i[0] + o[3] * i[1],
        o[0] * i[2] + o[2] * i[3],
        o[1] * i[2] + o[3] * i[3],
        o[0] * i[4] + o[2] * i[5] + o[4],
        o[1] * i[4] + o[3] * i[5] + o[5],
    ]
}

/// Folds the tcMods of one bundle left from identity in listed order.
/// Turb is not affine and rides separately in [`bundle_turb`].
pub fn bundle_affine(tcmods: &[TcMod], t: f32) -> [f32; 6] {
    let mut acc = AFFINE_IDENTITY;
    for m in tcmods {
        let next = match m {
            TcMod::Scroll(sx, sy) => {
                // RB_CalcScrollTexCoords keeps the fractional part only
                let fx = sx * t;
                let fy = sy * t;
                [1.0, 0.0, 0.0, 1.0, fx - fx.floor(), fy - fy.floor()]
            }
            TcMod::Scale(sx, sy) => [*sx, 0.0, 0.0, *sy, 0.0, 0.0],
            TcMod::Rotate(dps) => {
                // RB_CalcRotateTexCoords: negated degrees, index truncated
                // toward zero before masking, cos read a quarter ahead
                let degs = -dps * t;
                let idx = (degs * (FUNCTABLE_SIZE as f32 / 360.0)) as i64 & FUNCTABLE_MASK;
                let sin_v = SIN_TABLE[idx as usize];
                let cos_v =
                    SIN_TABLE[(idx + FUNCTABLE_SIZE as i64 / 4) as usize & FUNCTABLE_MASK as usize];
                [
                    cos_v,
                    sin_v,
                    -sin_v,
                    cos_v,
                    0.5 - 0.5 * cos_v + 0.5 * sin_v,
                    0.5 - 0.5 * sin_v - 0.5 * cos_v,
                ]
            }
            TcMod::Stretch(w) => {
                // RB_CalcStretchTexCoords: uniform 1/waveform about centre
                let p = 1.0 / wave_value(w, t);
                [p, 0.0, 0.0, p, 0.5 - 0.5 * p, 0.5 - 0.5 * p]
            }
            // args stored m00 m01 m10 m11 tS tT map verbatim onto this layout
            TcMod::Transform(a) => *a,
            TcMod::Turb { .. } => continue,
        };
        acc = compose_affine(&next, &acc);
    }
    acc
}

/// [amp0, now0, amp1, now1] with now = phase + t*freq for the last Turb of
/// the list; zeros when there is none. The VS adds
/// amp*sin(worldpos_axis/1024 + now) per axis.
pub fn bundle_turb(tcmods: &[TcMod], t: f32) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for m in tcmods {
        if let TcMod::Turb { amp, phase, freq } = m {
            let now = phase + t * freq;
            out = [*amp, now, *amp, now];
        }
    }
    out
}

/// Scale/Transform are static; everything else animates with time.
pub fn has_animated_tcmods(tcmods: &[TcMod]) -> bool {
    tcmods
        .iter()
        .any(|m| !matches!(m, TcMod::Scale(_, _) | TcMod::Transform(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tcMod runtime evaluation ----

    fn apply(m: &[f32; 6], s: f32, t: f32) -> (f32, f32) {
        (m[0] * s + m[2] * t + m[4], m[1] * s + m[3] * t + m[5])
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn assert_mat6(got: &[f32; 6], want: [f32; 6], eps: f32) {
        for i in 0..6 {
            assert!(
                close(got[i], want[i], eps),
                "[{i}] got {} want {}",
                got[i],
                want[i]
            );
        }
    }

    #[test]
    fn scroll_translates_fractional_speed_times_time() {
        // 0.5 * 2.3 = 1.15, wrapped to 0.15
        assert_mat6(
            &bundle_affine(&[TcMod::Scroll(0.5, 0.0)], 2.3),
            [1.0, 0.0, 0.0, 1.0, 0.15, 0.0],
            1e-6,
        );
        // negative speeds wrap upward: -0.75 -> 0.25, -0.25 -> 0.75
        assert_mat6(
            &bundle_affine(&[TcMod::Scroll(-0.75, -0.25)], 1.0),
            [1.0, 0.0, 0.0, 1.0, 0.25, 0.75],
            1e-6,
        );
    }

    #[test]
    fn rotate_negates_degrees_per_second() {
        // 90 deg/s for 1s: degs = -90, quarter turn about (0.5, 0.5) sends
        // UV point (1, 0.5) to (0.5, 0)
        let m = bundle_affine(&[TcMod::Rotate(90.0)], 1.0);
        assert_mat6(&m, [0.0, -1.0, 1.0, 0.0, 0.0, 1.0], 1e-4);
        let (sp, tp) = apply(&m, 1.0, 0.5);
        assert!(close(sp, 0.5, 1e-4) && close(tp, 0.0, 1e-4));

        // small positive time, CCW-positive speed: (1, 0.5) drifts toward
        // negative t (RB_CalcRotateTexCoords negates first)
        let m = bundle_affine(&[TcMod::Rotate(90.0)], 0.01);
        let (sp, tp) = apply(&m, 1.0, 0.5);
        assert!(sp > 0.99 && sp < 1.01, "s' {sp}");
        assert!(tp < 0.499, "t' must drop below 0.5, got {tp}");
    }

    #[test]
    fn stretch_inverts_wave_about_centre() {
        // phase+freq*t lands exactly on table index 256: sin ~= 1,
        // waveform = 1 + 0.5 ~= 1.5, p = 1/1.5 = 2/3
        let w = Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.5,
            phase: 0.0,
            freq: 0.25,
        };
        let m = bundle_affine(&[TcMod::Stretch(w)], 1.0);
        let p = m[0];
        assert!(close(p, 2.0 / 3.0, 1e-4), "p {p}");
        assert_mat6(&m, [p, 0.0, 0.0, p, 0.5 - 0.5 * p, 0.5 - 0.5 * p], 1e-6);
        // centre is a fixed point
        let (sp, tp) = apply(&m, 0.5, 0.5);
        assert!(close(sp, 0.5, 1e-5) && close(tp, 0.5, 1e-5));
    }

    #[test]
    fn transform_matrix_applies_verbatim_args() {
        let ident = bundle_affine(&[TcMod::Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])], 7.0);
        assert_eq!(ident, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

        // args arrive m00 m01 m10 m11 tS tT; lone transform comes back
        // verbatim in [a,b,c,d,tx,ty] layout
        let args = [2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let m = bundle_affine(&[TcMod::Transform(args)], 7.0);
        assert_eq!(m, args);
        // s' = m00*s + m10*t + tS ; t' = m01*s + m11*t + tT
        let (sp, tp) = apply(&m, 0.3, 0.8);
        assert!(close(sp, 2.0 * 0.3 + 4.0 * 0.8 + 6.0, 1e-5));
        assert!(close(tp, 3.0 * 0.3 + 5.0 * 0.8 + 7.0, 1e-5));
    }

    #[test]
    fn tcmod_composition_order_matters() {
        // scroll-then-scale carries the translation through the scale
        let a = bundle_affine(&[TcMod::Scroll(0.5, 0.0), TcMod::Scale(2.0, 2.0)], 2.3);
        assert_mat6(&a, [2.0, 0.0, 0.0, 2.0, 0.3, 0.0], 1e-6);
        // scale-then-scroll does not
        let b = bundle_affine(&[TcMod::Scale(2.0, 2.0), TcMod::Scroll(0.5, 0.0)], 2.3);
        assert_mat6(&b, [2.0, 0.0, 0.0, 2.0, 0.15, 0.0], 1e-6);
    }

    #[test]
    fn square_wave_flips_at_half_cycle_with_nearest_index() {
        let w = Wave {
            form: WaveForm::Square,
            base: 0.0,
            amp: 1.0,
            phase: 0.0,
            freq: 1.0,
        };
        assert!(close(wave_value(&w, 0.499), 1.0, 1e-6));
        // 0.4999 * 1024 = 511.898: myftol rounds to nearest, indexing 512
        // (truncation would stay on 511 and read +1)
        assert!(close(wave_value(&w, 0.4999), -1.0, 1e-6));
        assert!(close(wave_value(&w, 0.5), -1.0, 1e-6));
        assert!(close(wave_value(&w, 1.0), 1.0, 1e-6));
    }

    #[test]
    fn wave_value_matches_hand_computed_table_entries() {
        // sawtooth table[i] = i/1024: index 256 reads 0.25 exactly
        let saw = Wave {
            form: WaveForm::Sawtooth,
            base: 0.0,
            amp: 1.0,
            phase: 0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&saw, 123.0), 0.25, 1e-6));
        // inverse sawtooth mirrors it
        let inv = Wave {
            form: WaveForm::InverseSawtooth,
            ..saw.clone()
        };
        assert!(close(wave_value(&inv, 123.0), 0.75, 1e-6));
        // triangle peaks at the quarter, bottoms past three quarters
        let tri = |phase: f32| Wave {
            form: WaveForm::Triangle,
            base: 0.0,
            amp: 1.0,
            phase,
            freq: 0.0,
        };
        assert!(close(wave_value(&tri(0.125), 0.0), 0.5, 1e-6));
        assert!(close(wave_value(&tri(0.25), 0.0), 1.0, 1e-6));
        assert!(close(wave_value(&tri(0.75), 0.0), -1.0, 1e-6));
        // sin near its peak: sinTable[256] = sin(256*360/1023 deg)
        let sin = Wave {
            form: WaveForm::Sin,
            base: 100.0,
            amp: 20.0,
            phase: 0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&sin, 0.0), 120.0, 1e-4));
        // negative phases index through the mask, not negative slots
        let neg = Wave {
            form: WaveForm::Sawtooth,
            base: 0.0,
            amp: 1.0,
            phase: -0.25,
            freq: 0.0,
        };
        assert!(close(wave_value(&neg, 0.0), 0.75, 1e-6));
    }

    #[test]
    fn sin_table_matches_libm_over_full_range() {
        for (i, v) in SIN_TABLE.iter().enumerate() {
            let want = (i as f64 * 360.0 / (FUNCTABLE_SIZE - 1) as f64)
                .to_radians()
                .sin() as f32;
            assert!((v - want).abs() < 1e-5, "SIN_TABLE[{i}] {v} vs libm {want}");
        }
    }

    #[test]
    fn wave_value_is_periodic_over_one_cycle() {
        let forms = [
            WaveForm::Sin,
            WaveForm::Square,
            WaveForm::Triangle,
            WaveForm::Sawtooth,
            WaveForm::InverseSawtooth,
        ];
        for form in forms {
            let label = format!("{form:?}");
            let w = Wave {
                form: form.clone(),
                base: 0.3,
                amp: 0.8,
                phase: 0.11,
                freq: 1.0,
            };
            let mut t = 0.0f32;
            while t < 1.0 {
                assert!(
                    close(wave_value(&w, t), wave_value(&w, t + 1.0), 1e-4),
                    "{label} at t={t}"
                );
                t += 0.037;
            }
        }
    }

    #[test]
    fn turb_bundle_folds_time_into_now_last_wins() {
        assert_eq!(bundle_turb(&[], 3.0), [0.0; 4]);
        assert_eq!(bundle_turb(&[TcMod::Scroll(1.0, 1.0)], 3.0), [0.0; 4]);
        // now = phase + freq*t, unwrapped; same now for both axes
        let turb = TcMod::Turb {
            amp: 2.0,
            phase: 0.25,
            freq: 0.5,
        };
        assert_eq!(
            bundle_turb(std::slice::from_ref(&turb), 3.0),
            [2.0, 1.75, 2.0, 1.75]
        );
        // the affine pass leaves turbulence untouched
        assert_eq!(
            bundle_affine(
                &[TcMod::Turb {
                    amp: 2.0,
                    phase: 0.25,
                    freq: 0.5
                }],
                3.0
            ),
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        );
        // later Turb replaces earlier
        let late = TcMod::Turb {
            amp: 3.0,
            phase: 1.0,
            freq: 1.0,
        };
        assert_eq!(
            bundle_turb(&[turb, TcMod::Scale(2.0, 2.0), late], 2.0),
            [3.0, 3.0, 3.0, 3.0]
        );
    }

    #[test]
    fn has_animated_tcmods_truth_table() {
        assert!(!has_animated_tcmods(&[]));
        assert!(!has_animated_tcmods(&[TcMod::Scale(2.0, 2.0)]));
        assert!(!has_animated_tcmods(&[TcMod::Transform([1.0; 6])]));
        assert!(has_animated_tcmods(&[TcMod::Scroll(0.1, 0.0)]));
        assert!(has_animated_tcmods(&[TcMod::Rotate(30.0)]));
        assert!(has_animated_tcmods(&[TcMod::Stretch(Wave {
            form: WaveForm::Sin,
            base: 1.0,
            amp: 0.1,
            phase: 0.0,
            freq: 1.0
        })]));
        assert!(has_animated_tcmods(&[TcMod::Turb {
            amp: 1.0,
            phase: 0.0,
            freq: 1.0
        }]));
        // static mods alongside an animated one still animate
        assert!(has_animated_tcmods(&[
            TcMod::Scale(2.0, 2.0),
            TcMod::Scroll(0.1, 0.0)
        ]));
    }

    #[test]
    fn empty_tcmods_yield_identity_affine() {
        assert_eq!(bundle_affine(&[], 5.0), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

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
            vec!["nopicmip", "map", r"textures\detail\detail.tga"]
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
        assert_eq!(warns.entries(), 0);
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
            b0.vector,
            Some([0.001953125, 0.0, 0.0, 0.0, 0.001953125, 0.0])
        );
        assert_eq!(st.bundles[1].image, ImageRef::Lightmap);
        assert_eq!(st.rgb_gen, RgbGen::ExactVertex);
        // controller ruling: the sort name maps at parse time
        assert_eq!(sh.sort, Some(8.75));
        assert!(sh.surface.trans && sh.surface.water);
    }

    #[test]
    fn texture_unit_comparisons_use_profile_four_units() {
        let kept = parse_one("t { requires GL_MAX_TEXTURE_UNITS_ARB >= 4 { map a.tga } }");
        assert_eq!(kept.stages.len(), 1);
        let dropped = parse_one("t { requires GL_MAX_TEXTURE_UNITS_ARB < 4 { map a.tga } }");
        assert!(dropped.stages.is_empty());
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
    fn anim_map_reads_fps_then_paths_until_keyword() {
        let sh = parse_one(
            r#"t { { animMap 12 tex/a.tga tex\b.tga blendFunc add nextbundle map $lightmap } }"#,
        );
        let st = &sh.stages[0];
        let anim = st.bundles[0].anim.as_ref().unwrap();
        assert_eq!(anim.fps, 12.0);
        assert_eq!(
            anim.paths,
            vec!["tex/a.tga".to_string(), "tex/b.tga".to_string()]
        );
        assert_eq!(st.bundles[0].image, ImageRef::Path("tex/a.tga".to_string()));
        // parsing stopped at the keyword, so the blend belongs to the stage
        assert_eq!(st.blend, Some((BlendFactor::One, BlendFactor::One)));
        assert_eq!(st.bundles[1].image, ImageRef::Lightmap);
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
        assert_eq!(w.entries(), 1);
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
    fn nomipmaps_implies_nopicmip() {
        let sh = parse_one("t { nomipmaps { map a.tga } }");
        assert!(sh.nomipmaps && sh.nopicmip);
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
                    animMap zzz a.tga
                }
            }"#,
        );
        let st = &sh.stages[0];
        assert_eq!(st.bundles[0].tcmods, vec![TcMod::Scroll(0.0, 0.0)]);
        assert_eq!(st.rgb_gen, RgbGen::Const([0.0; 3]));
        assert_eq!(sh.sky.as_ref().unwrap().cloud_height, 512.0);
        assert_eq!(st.blend, Some((BlendFactor::One, BlendFactor::One)));
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
        assert_eq!(st.bundles[0].vector, None);
        assert_eq!(st.bundles[1].vector, Some([0.5, 0.0, 0.0, 0.0, 0.5, 0.0]));
        assert_eq!(w.entries(), 0, "supported form must not warn");
    }

    #[test]
    fn tcgen_vector_malformed_defaults_to_zero_with_warning() {
        let (name, body) = first_block("t { { map a.tga tcGen vector ( x y z ) ( 0 q 0 ) } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].bundles[0].vector, Some([0.0; 6]));
        assert!(w.fired(&name, "malformed tcGen vector"));
        assert_eq!(w.entries(), 1);
    }

    #[test]
    fn unsupported_tcgen_forms_drop_the_stage() {
        let (name, body) = first_block("t { { map a.tga tcGen environment } { map b.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages.len(), 1);
        assert!(w.fired(&name, "unsupported tcGen"));
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

    #[test]
    fn malformed_gen_wave_warns_once_and_keeps_default() {
        let (name, body) = first_block("t { { map a.tga rgbGen wave bogus 1 2 3 } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert_eq!(sh.stages[0].rgb_gen, RgbGen::IdentityLighting);
        assert!(w.fired(&name, "unknown wave form in rgbGen"));
        assert_eq!(w.entries(), 1);
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
        assert!(sh.stages[0].bundles[0].clamp);
        assert!(w.fired(&name, "clampY approximated"));

        let (name, body) = first_block("t { { map heightToNormal tex/b.tga } }");
        let mut w = WarnSet::new();
        let sh = parse_shader(&name, &body, &mut w);
        assert!(!sh.stages[0].bundles[0].clamp);
        assert!(w.fired(&name, "heightToNormal ignored"));
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
    fn sort_tokens_map_per_controller_table() {
        for (tok, v) in [
            ("portal", 1.0),
            ("sky", 2.0),
            ("opaque", 3.0),
            ("decal", 4.0),
            ("seethrough", 5.0),
            ("see", 5.0),
            ("banner", 6.0),
            ("underwater", 8.0),
            ("water", 8.75),
            ("ocean", 8.75),
            ("outer", 9.0),
            ("outerblend", 9.0),
            ("inner", 10.0),
            ("innerblend", 10.0),
            ("additive", 10.0),
            ("almostnearest", 14.0),
            ("nearest", 15.0),
        ] {
            assert_eq!(map_sort_token(tok), Some(v), "{tok}");
        }
        // numeric parse wins over the name table
        assert_eq!(map_sort_token("16"), Some(16.0));
        assert_eq!(map_sort_token("WATER"), Some(8.75));
        assert_eq!(map_sort_token("bogus"), None);
    }

    // ---- classification and sort mapping ----

    fn stage_of(blend: Option<(BlendFactor, BlendFactor)>, depth_write: Option<bool>) -> Stage {
        Stage {
            bundles: Vec::new(),
            blend,
            depth_write,
            alpha_func: None,
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
    fn sort_constants_match_the_controller_table() {
        assert_eq!(
            (
                SORT_OPAQUE,
                SORT_DECAL,
                SORT_SEETHROUGH,
                SORT_BANNER,
                SORT_WATER,
                SORT_BLEND0,
                SORT_ADDITIVE
            ),
            (3.0, 4.0, 5.0, 6.0, 8.75, 9.0, 10.0)
        );
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
        sh.polygon_offset = true;
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
        decal.polygon_offset = true;
        assert_eq!(decal.sort_value(), SORT_DECAL);
        // sky outranks polygon offset
        let mut sky_decal = sky;
        sky_decal.polygon_offset = true;
        assert_eq!(sky_decal.sort_value(), 2.0);

        // stage0 blended and still writing depth -> see-through
        let seethrough = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            Some(true),
        )]);
        assert_eq!(seethrough.sort_value(), SORT_SEETHROUGH);
        // stage0 blended without depth write -> blend0
        let blend0 = shader_of(vec![stage_of(
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            None,
        )]);
        assert_eq!(blend0.sort_value(), SORT_BLEND0);
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
        decal_blend.polygon_offset = true;
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
        sh.polygon_offset = true;
        assert!(sh.classify_stage(0).unwrap().bias);

        let mut named = shader_of(vec![stage_of(None, None)]);
        named.sort = map_sort_token("decal");
        assert!(named.classify_stage(0).unwrap().bias);
        assert_eq!(named.sort_value(), SORT_DECAL);

        let mut num = shader_of(vec![stage_of(None, None)]);
        num.sort = Some(4.0);
        assert!(num.classify_stage(0).unwrap().bias);

        let mut other = shader_of(vec![stage_of(None, None)]);
        other.sort = Some(4.25);
        assert!(!other.classify_stage(0).unwrap().bias);
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
        assert!(lib
            .image("textures/x/dupe")
            .is_some_and(|p| p.ends_with("b")));
    }
}
