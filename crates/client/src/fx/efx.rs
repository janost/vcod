//! Parser for CoD's `.efx` particle-script text. Curves are parsed, not
//! evaluated; that is `fx::sim`. Grammar and defaults: `docs/research/efx-grammar.md`.

use vcod_common::asset_source::{Asset, AssetKind, AssetSource};
use vcod_common::game::Game;
use vcod_common::text::atoi_msvc;

pub struct Effect {
    pub emitters: Vec<Emitter>,
}

/// The effect at `path` as the fx system plays it: read at
/// [`chained_path`], then parsed in the source game's dialect (a CoD2
/// install's through [`crate::fx::cod2::load`]). Built through
/// [`Asset::find`], uncached: `FxSystem` keeps its own cache.
impl Asset for Effect {
    const KIND: AssetKind = AssetKind::Effect;

    fn find(src: &dyn AssetSource, path: &str) -> anyhow::Result<Self> {
        let Some(bytes) = src.read(&chained_path(path)) else {
            anyhow::bail!("not found");
        };
        match src.game() {
            Game::Cod2 => crate::fx::cod2::load(src, path),
            _ => parse(&String::from_utf8_lossy(&bytes)),
        }
        .map_err(anyhow::Error::msg)
    }
}

/// Uniform-random scalar range; one value in the file means b == a.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R1 {
    pub a: f32,
    pub b: f32,
}

/// Vec3 range: 3 values = fixed, 6 = min/max corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R3 {
    pub a: [f32; 3],
    pub b: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub start: R1,
    pub end: R1,
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurveV3 {
    pub start: R3,
    pub end: R3,
    pub flags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Emitter {
    /// Block keyword as written, e.g. "Particle".
    pub kind: String,
    pub name: Option<String>,
    pub flags: Vec<String>,
    pub spawn_flags: Vec<String>,
    pub count: R1,
    pub life: R1,  // ms
    pub delay: R1, // ms
    pub radius: R1,
    pub height: R1,
    pub origin: R3, // offset from the trigger point
    pub velocity: R3,
    pub accel: R3,
    pub gravity: R1,        // units/sec^2, on z
    pub rotation: R1,       // degrees
    pub rotation_delta: R1, // deg/sec
    pub cullrange: f32,     // 0 = never culled
    /// `Emitter` only: initial euler angles and their rate, degrees and
    /// degrees per 100 ms (grammar doc R9).
    pub angle: R3,
    pub angle_delta: R3,
    /// Restitution. Its handler also turns on `usePhysics` (R7, R9), so
    /// `Some` means physics whatever `flags` says.
    pub bounce: Option<R1>,
    /// United Offensive's `CameraShake` strength. Retail parses it into
    /// `bounce`'s slot (CoDUOMP.exe 0x4ac0c0); stock files write `bounce`
    /// first, so this one wins.
    pub intensity: Option<R1>,
    /// `Emitter` debris models, `xmodel/...` paths.
    pub models: Vec<String>,
    /// Effects a physics particle plays where it hits; like `bounce`, the
    /// list turns on `usePhysics` and `impactFx` (R9).
    pub impact_fx: Vec<String>,
    /// `Emitter` only: effects it plays every `density` units along its
    /// path; the list turns on `emitFx` (grammar doc R10).
    pub emit_fx: Vec<String>,
    /// Effects an element plays where its life runs out; the list turns on
    /// `deathFx` (R10).
    pub death_fx: Vec<String>,
    /// `FxRunner` only: the effects it starts (R10).
    pub play_fx: Vec<String>,
    /// `Emitter` only: units between two `emitfx` plays, and the jitter
    /// on that spacing, both in units (R10).
    pub density: R1,
    pub variance: R1,
    pub size: Curve,
    /// `nonUniformScale`'s vertical half-extent (grammar doc R4); read only
    /// when [`Self::non_uniform_scale`] is set.
    pub size2: Curve,
    /// `nonUniformScale`: `atol` of its value, non-zero on (CoDMP.exe
    /// 0x49d8b0, template `+0xc0`).
    pub non_uniform_scale: bool,
    pub length: Curve,
    pub alpha: Curve,
    pub rgb: CurveV3,
    /// Shared with every particle and quad drawn from them.
    pub shaders: Vec<std::sync::Arc<str>>,
    pub sounds: Vec<String>,
    pub unknown_keys: Vec<String>,
    /// A CoD2 element: the primitive its channels are evaluated from
    /// (`fx::cod2`); `None` for every CoD1 file.
    pub cod2: Option<std::sync::Arc<crate::fx::cod2::Element>>,
    /// A CoD4 element's `effectEmitted` spacing: any element type plays
    /// [`Self::emit_fx`] along the path it moved (`FX_ProcessEmitting`,
    /// docs/research/cod4-effects.md 15); `None` for CoD1 and CoD2.
    pub cod4_emit: Option<Cod4Emit>,
    /// A CoD4 trail element (type 3): its cross-section and life, drawn by
    /// `fx::cod4_trail` on a moving effect (cod4-effects.md 16).
    pub cod4_trail: Option<crate::fx::cod4_trail::TrailDef>,
}

/// `FX_ProcessEmitting`'s spacing: an element draws `dist` once, adds
/// `variance_base`, and each play adds a fresh share of `variance_amp`;
/// units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cod4Emit {
    pub dist: R1,
    pub variance_base: f32,
    pub variance_amp: f32,
}

impl Default for Emitter {
    /// Defaults for keys absent from the file: grammar doc section 4 and R7.
    /// The template initializer (CoDMP.exe 0x49a460) sets `life` to 1 ms.
    fn default() -> Self {
        let zero1 = R1 { a: 0.0, b: 0.0 };
        let zero3 = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        Emitter {
            kind: String::new(),
            name: None,
            flags: Vec::new(),
            spawn_flags: Vec::new(),
            count: R1 { a: 1.0, b: 1.0 },
            life: R1 { a: 1.0, b: 1.0 },
            delay: zero1,
            // Only read under the orgOnSphere/orgOnCylinder spawnFlags.
            radius: R1 { a: 1.0, b: 1.0 },
            height: R1 { a: 1.0, b: 1.0 },
            origin: zero3,
            velocity: zero3,
            accel: zero3,
            gravity: zero1,
            rotation: zero1,
            rotation_delta: zero1,
            cullrange: 0.0,
            angle: zero3,
            angle_delta: zero3,
            bounce: None,
            intensity: None,
            models: Vec::new(),
            impact_fx: Vec::new(),
            emit_fx: Vec::new(),
            death_fx: Vec::new(),
            play_fx: Vec::new(),
            density: zero1,
            variance: zero1,
            size: Curve {
                start: zero1,
                end: zero1,
                flags: Vec::new(),
            },
            size2: Curve {
                start: zero1,
                end: zero1,
                flags: Vec::new(),
            },
            non_uniform_scale: false,
            length: Curve {
                start: zero1,
                end: zero1,
                flags: Vec::new(),
            },
            alpha: Curve {
                start: R1 { a: 1.0, b: 1.0 },
                end: R1 { a: 1.0, b: 1.0 },
                flags: Vec::new(),
            },
            rgb: CurveV3 {
                start: R3 {
                    a: [1.0; 3],
                    b: [1.0; 3],
                },
                end: R3 {
                    a: [1.0; 3],
                    b: [1.0; 3],
                },
                flags: Vec::new(),
            },
            shaders: Vec::new(),
            sounds: Vec::new(),
            unknown_keys: Vec::new(),
            cod2: None,
            cod4_emit: None,
            cod4_trail: None,
        }
    }
}

/// Token with its 1-based source line.
#[derive(Clone, Copy)]
struct Tok<'a> {
    line: usize,
    text: &'a str,
}

/// `{ } [ ]` are split out as standalone tokens even when not
/// whitespace-isolated. `//` comment lines are dropped: two retail files
/// (`fx/atmosphere/rockets_altocloud.efx`, `fx/atmosphere/missile_skyflash.efx`)
/// carry one, which the grammar doc does not mention.
fn tokenize(text: &str) -> Vec<Tok<'_>> {
    let mut toks = Vec::new();
    for (i, raw_line) in text.split('\n').enumerate() {
        let line = i + 1;
        // The corpus is CRLF throughout (grammar doc, section 0).
        let content = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if content.trim_start().starts_with("//") {
            continue;
        }
        let mut start: Option<usize> = None;
        for (pos, c) in content.char_indices() {
            match c {
                c if c.is_whitespace() => {
                    if let Some(s) = start.take() {
                        toks.push(Tok {
                            line,
                            text: &content[s..pos],
                        });
                    }
                }
                '{' | '}' | '[' | ']' => {
                    if let Some(s) = start.take() {
                        toks.push(Tok {
                            line,
                            text: &content[s..pos],
                        });
                    }
                    toks.push(Tok {
                        line,
                        text: &content[pos..pos + c.len_utf8()],
                    });
                }
                _ => {
                    if start.is_none() {
                        start = Some(pos);
                    }
                }
            }
        }
        if let Some(s) = start.take() {
            toks.push(Tok {
                line,
                text: &content[s..],
            });
        }
    }
    toks
}

struct Cursor<'a> {
    toks: Vec<Tok<'a>>,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(toks: Vec<Tok<'a>>) -> Self {
        Cursor { toks, pos: 0 }
    }

    fn peek(&self) -> Option<Tok<'a>> {
        self.toks.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<Tok<'a>> {
        let t = self.toks.get(self.pos).copied()?;
        self.pos += 1;
        Some(t)
    }

    fn eof_line(&self) -> usize {
        self.toks.last().map(|t| t.line).unwrap_or(1)
    }

    /// A keyval's values sit on the key's own line throughout the corpus.
    fn take_same_line_values(&mut self, line: usize) -> Vec<&'a str> {
        let mut out = Vec::new();
        while let Some(t) = self.peek() {
            if t.line != line || matches!(t.text, "{" | "}" | "[" | "]") {
                break;
            }
            out.push(t.text);
            self.pos += 1;
        }
        out
    }

    /// Skip a balanced region whose opener was already consumed.
    fn skip_balanced(&mut self, open: &str, close: &str) -> Result<(), String> {
        let mut depth = 1u32;
        loop {
            match self.next() {
                Some(t) if t.text == open => depth += 1,
                Some(t) if t.text == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some(_) => {}
                None => {
                    return Err(format!(
                        "line {}: unexpected end of input, expected '{close}'",
                        self.eof_line()
                    ));
                }
            }
        }
    }
}

fn parse_f32(s: &str, line: usize) -> Result<f32, String> {
    s.parse::<f32>()
        .map_err(|_| format!("line {line}: invalid number '{s}'"))
}

fn parse_f32_list(vals: &[&str], line: usize) -> Result<Vec<f32>, String> {
    vals.iter().map(|s| parse_f32(s, line)).collect()
}

fn r1_from_values(v: &[f32], line: usize) -> Result<R1, String> {
    match v {
        [a] => Ok(R1 { a: *a, b: *a }),
        [a, b] => Ok(R1 { a: *a, b: *b }),
        _ => Err(format!(
            "line {line}: expected 1 or 2 numbers, got {}",
            v.len()
        )),
    }
}

fn r3_from_values(v: &[f32], line: usize) -> Result<R3, String> {
    match v {
        [x, y, z] => Ok(R3 {
            a: [*x, *y, *z],
            b: [*x, *y, *z],
        }),
        [x0, y0, z0, x1, y1, z1] => Ok(R3 {
            a: [*x0, *y0, *z0],
            b: [*x1, *y1, *z1],
        }),
        _ => Err(format!(
            "line {line}: expected 3 or 6 numbers, got {}",
            v.len()
        )),
    }
}

fn r1_from_tokens(vals: &[&str], line: usize) -> Result<R1, String> {
    r1_from_values(&parse_f32_list(vals, line)?, line)
}

fn r3_from_tokens(vals: &[&str], line: usize) -> Result<R3, String> {
    r3_from_values(&parse_f32_list(vals, line)?, line)
}

/// `cullrange` is arity 1 throughout the corpus (grammar doc, section 2a).
fn scalar_from_tokens(vals: &[&str], line: usize) -> Result<f32, String> {
    match vals {
        [a] => parse_f32(a, line),
        _ => Err(format!(
            "line {line}: expected exactly 1 number, got {}",
            vals.len()
        )),
    }
}

/// Curve sub-block contents before `start`/`end` are folded into `R1`/`R3`.
#[derive(Default)]
struct CurveRaw {
    start: Option<Vec<f32>>,
    end: Option<Vec<f32>>,
    flags: Vec<String>,
}

/// Parse a curve sub-block body after its `{`, through its `}`.
fn parse_curve_body(p: &mut Cursor, em: &mut Emitter) -> Result<CurveRaw, String> {
    let mut raw = CurveRaw::default();
    loop {
        let key_tok = p.next().ok_or_else(|| {
            format!(
                "line {}: unexpected end of input, expected '}}'",
                p.eof_line()
            )
        })?;
        if key_tok.text == "}" {
            return Ok(raw);
        }
        let key = key_tok.text;
        let line = key_tok.line;
        match p.peek().map(|t| t.text) {
            Some("{") => {
                p.next();
                p.skip_balanced("{", "}")?;
                em.unknown_keys.push(key.to_string());
            }
            Some("[") => {
                p.next();
                p.skip_balanced("[", "]")?;
                em.unknown_keys.push(key.to_string());
            }
            _ => {
                let vals = p.take_same_line_values(line);
                match key {
                    "start" => raw.start = Some(parse_f32_list(&vals, line)?),
                    "end" => raw.end = Some(parse_f32_list(&vals, line)?),
                    "flags" => raw.flags = vals.iter().map(|s| s.to_string()).collect(),
                    "parm" => {} // wave-mode parameters, not modeled
                    _ => em.unknown_keys.push(key.to_string()),
                }
            }
        }
    }
}

/// `missing_default` fills either endpoint the file omits. The engine shares
/// one default per curve between `start` and `end` (grammar doc R7), so a
/// `linear` curve with no `end` ramps to the default, not to `start`.
fn curve_from_raw(raw: CurveRaw, missing_default: f32, line: usize) -> Result<Curve, String> {
    let fallback = R1 {
        a: missing_default,
        b: missing_default,
    };
    let start = match raw.start {
        Some(v) => r1_from_values(&v, line)?,
        None => fallback,
    };
    let end = match raw.end {
        Some(v) => r1_from_values(&v, line)?,
        None => fallback,
    };
    Ok(Curve {
        start,
        end,
        flags: raw.flags,
    })
}

/// `curve_from_raw` for the 3-wide `rgb` curve.
fn curve_v3_from_raw(
    raw: CurveRaw,
    missing_default: [f32; 3],
    line: usize,
) -> Result<CurveV3, String> {
    let fallback = R3 {
        a: missing_default,
        b: missing_default,
    };
    let start = match raw.start {
        Some(v) => r3_from_values(&v, line)?,
        None => fallback,
    };
    let end = match raw.end {
        Some(v) => r3_from_values(&v, line)?,
        None => fallback,
    };
    Ok(CurveV3 {
        start,
        end,
        flags: raw.flags,
    })
}

fn dispatch_curve(p: &mut Cursor, em: &mut Emitter, key: &str, line: usize) -> Result<(), String> {
    let raw = parse_curve_body(p, em)?;
    // Per-curve defaults: grammar doc, section 4.
    match key {
        "rgb" => em.rgb = curve_v3_from_raw(raw, [1.0; 3], line)?,
        "alpha" => em.alpha = curve_from_raw(raw, 1.0, line)?,
        "size" => em.size = curve_from_raw(raw, 0.0, line)?,
        "length" => em.length = curve_from_raw(raw, 0.0, line)?,
        "size2" => em.size2 = curve_from_raw(raw, 0.0, line)?,
        _ => em.unknown_keys.push(key.to_string()),
    }
    Ok(())
}

fn dispatch_list(em: &mut Emitter, key: &str, items: Vec<String>) {
    match key {
        "shaders" => em.shaders = items.into_iter().map(Into::into).collect(),
        "sounds" => em.sounds = items,
        "models" => em.models = items,
        "impactfx" => em.impact_fx = items,
        "emitfx" => em.emit_fx = items,
        "deathfx" => em.death_fx = items,
        "playfx" => em.play_fx = items,
        _ => em.unknown_keys.push(key.to_string()),
    }
}

fn dispatch_keyval(em: &mut Emitter, key: &str, vals: &[&str], line: usize) -> Result<(), String> {
    match key {
        "name" => em.name = Some(vals.join(" ")),
        "flags" => em.flags = vals.iter().map(|s| s.to_string()).collect(),
        "spawnFlags" => em.spawn_flags = vals.iter().map(|s| s.to_string()).collect(),
        "count" => em.count = r1_from_tokens(vals, line)?,
        "life" => em.life = r1_from_tokens(vals, line)?,
        "delay" => em.delay = r1_from_tokens(vals, line)?,
        "radius" => em.radius = r1_from_tokens(vals, line)?,
        "height" => em.height = r1_from_tokens(vals, line)?,
        "velocity" => em.velocity = r3_from_tokens(vals, line)?,
        "acceleration" => em.accel = r3_from_tokens(vals, line)?,
        "gravity" => em.gravity = r1_from_tokens(vals, line)?,
        "rotation" => em.rotation = r1_from_tokens(vals, line)?,
        "rotationDelta" => em.rotation_delta = r1_from_tokens(vals, line)?,
        "cullrange" => em.cullrange = scalar_from_tokens(vals, line)?,
        "origin" => em.origin = r3_from_tokens(vals, line)?,
        "angle" => em.angle = r3_from_tokens(vals, line)?,
        "angleDelta" => em.angle_delta = r3_from_tokens(vals, line)?,
        "bounce" => em.bounce = Some(r1_from_tokens(vals, line)?),
        "intensity" => em.intensity = Some(r1_from_tokens(vals, line)?),
        "density" => em.density = r1_from_tokens(vals, line)?,
        "variance" => em.variance = r1_from_tokens(vals, line)?,
        "nonUniformScale" => {
            em.non_uniform_scale = atoi_msvc(vals.first().copied().unwrap_or("")) != 0
        }
        // Census keys without a field, kept out of unknown_keys.
        "origin2" | "wind" => {}
        _ => em.unknown_keys.push(key.to_string()),
    }
    Ok(())
}

fn parse_block_body(p: &mut Cursor, em: &mut Emitter) -> Result<(), String> {
    loop {
        let key_tok = p.next().ok_or_else(|| {
            format!(
                "line {}: unexpected end of input, expected '}}'",
                p.eof_line()
            )
        })?;
        if key_tok.text == "}" {
            return Ok(());
        }
        let key = key_tok.text;
        let line = key_tok.line;
        match p.peek().map(|t| t.text) {
            Some("{") => {
                p.next();
                dispatch_curve(p, em, key, line)?;
            }
            Some("[") => {
                p.next();
                let mut items = Vec::new();
                loop {
                    let t = p.next().ok_or_else(|| {
                        format!(
                            "line {}: unexpected end of input, expected ']'",
                            p.eof_line()
                        )
                    })?;
                    if t.text == "]" {
                        break;
                    }
                    items.push(t.text.to_string());
                }
                dispatch_list(em, key, items);
            }
            _ => {
                let vals = p.take_same_line_values(line);
                dispatch_keyval(em, key, &vals, line)?;
            }
        }
    }
}

/// The file a chained-effect list entry (`impactfx`, `emitfx`, `deathfx`,
/// `playfx`) names. Entries are registered up to their first `.` and read
/// back as `.efx` (CoDMP.exe 0x492eb0, grammar doc R9).
pub fn chained_path(name: &str) -> String {
    let stem = name.split('.').next().unwrap_or(name);
    format!("{stem}.efx")
}

/// Errors carry the offending line number.
pub fn parse(text: &str) -> Result<Effect, String> {
    let mut p = Cursor::new(tokenize(text));
    let mut emitters = Vec::new();
    while let Some(kind_tok) = p.next() {
        let kind = kind_tok.text.to_string();
        match p.next() {
            Some(t) if t.text == "{" => {}
            Some(t) => {
                return Err(format!(
                    "line {}: expected '{{' after block '{kind}', found '{}'",
                    t.line, t.text
                ));
            }
            None => {
                return Err(format!(
                    "line {}: expected '{{' after block '{kind}', found end of input",
                    p.eof_line()
                ));
            }
        }
        let mut emitter = Emitter {
            kind,
            ..Emitter::default()
        };
        parse_block_body(&mut p, &mut emitter)?;
        emitters.push(emitter);
    }
    Ok(Effect { emitters })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim body of fx/tagged/tracers.efx from main/pak5.pk3.
    const TRACER: &str = r#"
Tail
{
	name				tracer

	flags				useAlpha

	spawnFlags			cheapOrgCalc absoluteAccel evenDistribution

	life				3700 4300

	delay				300 100

	radius				45 55

	height				100 10

	velocity			1.19e+004 0 0 1.18e+004 0 0

	rgb
	{
		flags			linear
	}

	alpha
	{
		flags			linear
	}

	size
	{
		start			5
		end				10
		flags			linear
	}

	length
	{
		start			60 90
		end				120
		flags			random linear
	}

	shaders
	[
		gfx/effects/antiaircraft_tracer
	]
}
"#;

    #[test]
    fn parses_tracer_emitter() {
        let e = parse(TRACER).unwrap();
        assert_eq!(e.emitters.len(), 1);
        let em = &e.emitters[0];
        assert_eq!(em.kind, "Tail");
        assert_eq!(em.name.as_deref(), Some("tracer"));
        assert_eq!(em.flags, vec!["useAlpha"]);
        assert_eq!(
            em.life,
            R1 {
                a: 3700.0,
                b: 4300.0
            }
        );
        assert_eq!(em.velocity.a, [1.19e4, 0.0, 0.0]);
        assert_eq!(em.velocity.b, [1.18e4, 0.0, 0.0]);
        assert_eq!(em.size.start, R1 { a: 5.0, b: 5.0 });
        assert_eq!(em.size.end, R1 { a: 10.0, b: 10.0 });
        assert_eq!(em.length.end, R1 { a: 120.0, b: 120.0 });
        assert_eq!(
            em.shaders,
            ["gfx/effects/antiaircraft_tracer"].map(std::sync::Arc::<str>::from)
        );
    }

    /// `fx/impacts/blast_flash2.efx`'s flash: `nonUniformScale 1` and its
    /// `size2` curve are kept.
    #[test]
    fn non_uniform_scale_keeps_size2() {
        let text = "Particle\n{\n\tnonUniformScale\t\t1\n\tsize2\n\t{\n\t\tstart\t\t\t122\n\t\tend\t\t\t\t2\n\t\tflags\t\t\tlinear\n\t}\n}\n";
        let em = &parse(text).unwrap().emitters[0];
        assert!(em.non_uniform_scale);
        assert_eq!(em.size2.start, R1 { a: 122.0, b: 122.0 });
        assert_eq!(em.size2.end, R1 { a: 2.0, b: 2.0 });
        assert_eq!(em.size2.flags, vec!["linear"]);
        assert!(
            !parse("Particle\n{\n\tnonUniformScale 0\n}\n")
                .unwrap()
                .emitters[0]
                .non_uniform_scale
        );
    }

    #[test]
    fn sound_block_keeps_its_alias_list() {
        let text = "Sound\n{\n\tsounds\n\t[\n\t\tglass_break\n\t]\n}\n";
        let e = parse(text).unwrap();
        assert_eq!(e.emitters.len(), 1);
        assert_eq!(e.emitters[0].kind, "Sound");
        assert_eq!(e.emitters[0].sounds, vec!["glass_break".to_string()]);
        assert!(e.emitters[0].unknown_keys.is_empty());
    }

    #[test]
    fn two_particle_blocks_parse_as_two_emitters() {
        let two = format!("{TRACER}\n{TRACER}");
        assert_eq!(parse(&two).unwrap().emitters.len(), 2);
    }

    #[test]
    fn an_absent_life_reads_one_millisecond() {
        let s = TRACER.replace("life\t\t\t\t3700 4300", "");
        let e = parse(&s).unwrap();
        assert_eq!(e.emitters[0].life, R1 { a: 1.0, b: 1.0 });
    }

    #[test]
    fn unknown_key_is_recorded_not_fatal() {
        let s = TRACER.replace("radius\t\t\t\t45 55", "frobnicate 1 2 3");
        let e = parse(&s).unwrap();
        assert!(
            e.emitters[0]
                .unknown_keys
                .contains(&"frobnicate".to_string())
        );
    }

    #[test]
    fn chained_lists_and_emit_spacing_parse() {
        let text = "Emitter\n{\n\tflags\temitFx\n\tdensity\t4\n\tvariance\t1 5\n\temitfx\n\t[\n\t\tfx/smoke/a\n\t\tfx/smoke/b\n\t]\n\tdeathfx\n\t[\n\t\tfx/c\n\t]\n}\nFxRunner\n{\n\tplayfx\n\t[\n\t\tfx/d\n\t]\n}\n";
        let e = parse(text).unwrap();
        let em = &e.emitters[0];
        assert_eq!(em.emit_fx, vec!["fx/smoke/a", "fx/smoke/b"]);
        assert_eq!(em.death_fx, vec!["fx/c"]);
        assert_eq!(em.density, R1 { a: 4.0, b: 4.0 });
        assert_eq!(em.variance, R1 { a: 1.0, b: 5.0 });
        assert_eq!(e.emitters[1].play_fx, vec!["fx/d"]);
        assert_eq!(chained_path("fx/d"), "fx/d.efx");
        assert_eq!(chained_path("fx/d.efx"), "fx/d.efx");
    }

    #[test]
    fn unbalanced_brace_is_an_error_with_line() {
        let s = TRACER.replacen('}', "", 1);
        assert!(parse(&s).is_err());
    }

    #[test]
    fn every_retail_efx_parses() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let names = fs.names_with_suffix(".efx");
        assert!(
            names.len() > 100,
            "expected the retail effect set, got {}",
            names.len()
        );
        let mut unknown: std::collections::BTreeMap<String, usize> = Default::default();
        let mut chained = 0;
        for name in &names {
            let text = String::from_utf8_lossy(&fs.read(name).unwrap()).into_owned();
            let eff = parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            for em in &eff.emitters {
                for k in &em.unknown_keys {
                    *unknown.entry(k.clone()).or_default() += 1;
                }
                let lists = [&em.impact_fx, &em.emit_fx, &em.death_fx, &em.play_fx];
                for target in lists.into_iter().flatten() {
                    chained += 1;
                    let path = chained_path(target);
                    assert!(fs.read(&path).is_some(), "{name}: {path} is missing");
                }
            }
        }
        // The census is the full key set; any unknown key is a parser gap.
        assert!(unknown.is_empty(), "unhandled keys: {unknown:?}");
        // Grammar doc R10: 260 entries in the 1.1 paks.
        assert!(chained >= 260, "{chained}");
    }

    /// `fx::sim` draws no quad for an element without `shaders`, where
    /// retail draws `<default>` at the element's size (grammar doc R4):
    /// the two agree only while every such stock element is zero-sized.
    #[test]
    fn every_stock_element_without_shaders_is_zero_sized() {
        let paks = [
            vcod_common::testing::game_fs(),
            vcod_common::testing::uo_fs(),
        ];
        let mut seen = 0;
        for fs in paks.iter().flatten() {
            for name in fs.names_with_suffix(".efx") {
                let text = String::from_utf8_lossy(&fs.read(&name).unwrap()).into_owned();
                let eff = parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
                let drawn = ["Particle", "OrientedParticle", "Cylinder", "Tail", "Line"];
                for em in eff
                    .emitters
                    .iter()
                    .filter(|em| drawn.contains(&em.kind.as_str()))
                {
                    assert!(em.shaders.iter().all(|s| !s.is_empty()), "{name}");
                    if !em.shaders.is_empty() {
                        continue;
                    }
                    seen += 1;
                    let zero =
                        |c: &Curve| [c.start, c.end].iter().all(|r| r.a == 0.0 && r.b == 0.0);
                    assert!(zero(&em.size) && zero(&em.length), "{name}: {:?}", em.name);
                }
            }
        }
        // 1.1: boom_dirt's `Particle {}` and fx/test/modeltest.efx.
        assert!(paks[0].is_none() || seen >= 2, "{seen}");
    }
}
