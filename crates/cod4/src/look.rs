//! The look a level sets on top of its surfaces: the exponential fog of
//! configstring 9 and the vision sets (`vision/*.vision`) configstrings
//! 824 and 825 name, with their film tweak and glow. Facts and their
//! sources: docs/research/cod4-world.md section 11.

use crate::assets::misc::RawFile;
use std::collections::HashMap;
use vcod_common::net::FogParams;

/// `setExpFog`'s configstring.
pub const CS_FOG: u16 = 9;
/// `visionSetNaked`'s `"<name>" <ms>`.
pub const CS_VISION_NAKED: u16 = 824;
/// `visionSetNight`'s, read only under night vision.
pub const CS_VISION_NIGHT: u16 = 825;

/// The vision set a client draws before any configstring names one.
pub const DEFAULT_VISION: &str = "default";

/// One exponential fog: everything nearer than `start` stays clear, and
/// past it the colour keeps `exp(-density * (d - start))` of itself at
/// radial distance `d`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpFog {
    pub start: f32,
    pub density: f32,
    /// Bytes, as the renderer keeps it (round to nearest of `v * 255`).
    pub color: [u8; 3],
    /// The fade from the fog drawn before, in milliseconds.
    pub transition_ms: i32,
}

impl ExpFog {
    /// The renderer's fog: exp mode, the start in [`FogParams::start`].
    pub fn params(&self) -> FogParams {
        FogParams {
            near: 0.0,
            far: 1.0,
            density: self.density,
            color: self.color.map(|c| f32::from(c) / 255.0),
            fade_ms: self.transition_ms.max(0) as u32,
            start: self.start,
        }
    }
}

/// `setExpFog`'s density for a halfway distance: `ln 2 / halfwayDist`, in
/// floats.
pub fn density(halfway: f32) -> f32 {
    std::f32::consts::LN_2 / halfway
}

/// A colour component as a byte, rounded to nearest.
fn byte(v: f32) -> u8 {
    (v * 255.0).round_ties_even().clamp(0.0, 255.0) as u8
}

/// What configstring 9 says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FogCs {
    On(ExpFog),
    /// No fog, faded out over the first token's milliseconds.
    Off {
        transition_ms: i32,
    },
}

/// Configstring 9, `<start> <density> <r> <g> <b> <ms>`: fog on when a
/// second token follows the first, off otherwise.
pub fn parse_fog(cs: &str) -> FogCs {
    let mut t = cs.split_whitespace();
    let start = t.next().map_or(0.0, atof);
    let Some(density) = t.next().map(atof) else {
        return FogCs::Off {
            transition_ms: start as i32,
        };
    };
    let mut next = || t.next().map_or(0.0, atof);
    let color = [next(), next(), next()].map(byte);
    let transition_ms = next() as i32;
    FogCs::On(ExpFog {
        start,
        density,
        color,
        transition_ms,
    })
}

/// C's `atof`: the longest leading number, 0 when there is none.
fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0))
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let mut end = end;
    // an exponent, if one follows
    if let Some(rest) = s.get(end..)
        && let Some(e) = rest.strip_prefix(['e', 'E'])
    {
        let sign = usize::from(e.starts_with(['-', '+']));
        let digits = e[sign..].chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            end += 1 + sign + digits;
        }
    }
    s[..end].parse().unwrap_or(0.0)
}

/// The first live `setExpFog(start, halfway, r, g, b, time)` of a map
/// script (`maps/createart/<map>_art.gsc`), its arguments numbers or a
/// quotient of two, as the configstring it writes would read: fly mode
/// has no server to run it.
pub fn script_fog(gsc: &str) -> Option<ExpFog> {
    let code = strip_comments(gsc);
    let at = code.to_ascii_lowercase().find("setexpfog(")?;
    let args = &code[at + "setexpfog(".len()..];
    let args = &args[..args.find(')')?];
    let a: Vec<f32> = args
        .split(',')
        .map(|a| match a.split_once('/') {
            // mp_strike's `233/255`
            Some((n, d)) => Some(n.trim().parse::<f32>().ok()? / d.trim().parse::<f32>().ok()?),
            None => a.trim().parse().ok(),
        })
        .collect::<Option<_>>()?;
    let [start, halfway, r, g, b, time] = a[..] else {
        return None;
    };
    (halfway > 0.0).then(|| ExpFog {
        start,
        density: density(halfway),
        color: [r, g, b].map(byte),
        transition_ms: (time * 1000.0).round() as i32,
    })
}

/// The fog a map's scripts set: its art script's
/// (`maps/createart/<map>_art.gsc`, which `maps/mp/<map>.gsc` calls),
/// else the map script's own, out of the map zone's rawfiles.
pub fn map_fog(raw: &[RawFile], map: &str) -> Option<ExpFog> {
    [
        format!("maps/createart/{map}_art.gsc"),
        format!("maps/mp/{map}.gsc"),
    ]
    .iter()
    .find_map(|path| {
        let r = raw.iter().find(|r| r.name.eq_ignore_ascii_case(path))?;
        script_fog(&String::from_utf8_lossy(&r.data))
    })
}

/// The vision sets among a zone's rawfiles (`common_mp`'s), by their
/// lower-case name without `vision/` and `.vision`.
pub fn visions(raw: &[RawFile]) -> HashMap<String, Vision> {
    raw.iter()
        .filter_map(|r| {
            let n = r.name.to_ascii_lowercase();
            let name = n.strip_prefix("vision/")?.strip_suffix(".vision")?;
            Some((
                name.to_string(),
                Vision::parse(&String::from_utf8_lossy(&r.data)),
            ))
        })
        .collect()
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("//") {
            rest = r.find('\n').map_or("", |i| &r[i..]);
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.find("*/").map_or("", |i| &r[i + 2..]);
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A vision set's glow (`r_glow*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glow {
    pub enable: bool,
    pub bloom_cutoff: f32,
    pub bloom_desaturation: f32,
    pub bloom_intensity: [f32; 2],
    pub radius: [f32; 2],
    pub sky_bleed_intensity: [f32; 2],
}

/// A vision set's film tweak (`r_film*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Film {
    pub enable: bool,
    pub brightness: f32,
    pub contrast: f32,
    pub desaturation: f32,
    pub invert: bool,
    pub light_tint: [f32; 3],
    pub dark_tint: [f32; 3],
}

/// One `vision/<name>.vision` rawfile.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vision {
    pub glow: Glow,
    pub film: Film,
}

/// Where a vision key's value goes, and how it reads: `%i` for a
/// switch, `%f` for a number, `%f %f %f` for a colour (cod4-world.md 11.2).
#[derive(Clone, Copy)]
enum Slot {
    Switch(fn(&mut Vision) -> &mut bool),
    Number(fn(&mut Vision) -> &mut f32),
    Colour(fn(&mut Vision) -> &mut [f32; 3]),
}

impl Slot {
    /// Reads `text` into `v`; false, and `v` untouched, when it does not
    /// read.
    fn store(self, v: &mut Vision, text: &str) -> bool {
        match self {
            Slot::Switch(at) => read_int(text).map(|i| *at(v) = i != 0),
            Slot::Number(at) => read_float(text.trim_start()).map(|x| *at(v) = x),
            Slot::Colour(at) => {
                let rgb: Option<Vec<f32>> =
                    text.split_whitespace().take(3).map(read_float).collect();
                rgb.and_then(|c| <[f32; 3]>::try_from(c).ok())
                    .map(|c| *at(v) = c)
            }
        }
        .is_some()
    }
}

/// The vision file's keys, in the order and of the kinds of iw3mp.exe's
/// field table at 0x727610 (`tools/re/cod4_vision_fields.py`,
/// cod4-world.md 11.2), and where each lands.
const FIELDS: [(&str, Slot); 16] = [
    ("r_glow", Slot::Switch(|v| &mut v.glow.enable)),
    (
        "r_glowBloomCutoff",
        Slot::Number(|v| &mut v.glow.bloom_cutoff),
    ),
    (
        "r_glowBloomDesaturation",
        Slot::Number(|v| &mut v.glow.bloom_desaturation),
    ),
    (
        "r_glowBloomIntensity0",
        Slot::Number(|v| &mut v.glow.bloom_intensity[0]),
    ),
    (
        "r_glowBloomIntensity1",
        Slot::Number(|v| &mut v.glow.bloom_intensity[1]),
    ),
    ("r_glowRadius0", Slot::Number(|v| &mut v.glow.radius[0])),
    ("r_glowRadius1", Slot::Number(|v| &mut v.glow.radius[1])),
    (
        "r_glowSkyBleedIntensity0",
        Slot::Number(|v| &mut v.glow.sky_bleed_intensity[0]),
    ),
    (
        "r_glowSkyBleedIntensity1",
        Slot::Number(|v| &mut v.glow.sky_bleed_intensity[1]),
    ),
    ("r_filmEnable", Slot::Switch(|v| &mut v.film.enable)),
    ("r_filmBrightness", Slot::Number(|v| &mut v.film.brightness)),
    ("r_filmContrast", Slot::Number(|v| &mut v.film.contrast)),
    (
        "r_filmDesaturation",
        Slot::Number(|v| &mut v.film.desaturation),
    ),
    ("r_filmInvert", Slot::Switch(|v| &mut v.film.invert)),
    ("r_filmLightTint", Slot::Colour(|v| &mut v.film.light_tint)),
    ("r_filmDarkTint", Slot::Colour(|v| &mut v.film.dark_tint)),
];

/// The sixteen keys alone, in the same order.
pub const KEYS: [&str; 16] = {
    let mut keys = [""; 16];
    let mut i = 0;
    while i < keys.len() {
        keys[i] = FIELDS[i].0;
        i += 1;
    }
    keys
};

/// `%i` as the vision reader takes it: an optional sign, then digits.
fn read_int(text: &str) -> Option<i64> {
    let t = text.trim_start();
    let sign = usize::from(t.starts_with(['+', '-']));
    let len = sign + t[sign..].bytes().take_while(u8::is_ascii_digit).count();
    t[..len].parse().ok()
}

/// `%f`: a word that starts a number reads as `atof` does.
fn read_float(word: &str) -> Option<f32> {
    word.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
        .then(|| atof(word))
}

impl Vision {
    /// A vision file: a `<key> "<value>"` line per setting, the keys
    /// case-blind. A key takes the first of its lines that reads; unknown
    /// keys (`cg_fov`) are skipped, and a key the file never sets reads 0.
    pub fn parse(text: &str) -> Vision {
        let mut v = Vision::default();
        let mut set = [false; FIELDS.len()];
        for line in text.lines() {
            let mut words = tokens(line);
            let Some(key) = words.next() else { continue };
            let Some(row) = FIELDS
                .iter()
                .position(|(name, _)| key.eq_ignore_ascii_case(name))
            else {
                continue;
            };
            if !set[row] {
                set[row] = FIELDS[row]
                    .1
                    .store(&mut v, &words.next().unwrap_or_default());
            }
        }
        v
    }

    /// `to` reached `frac` of the way from `from` along the smooth lerp
    /// `visionSetNaked` starts: `sin(frac * pi / 2)` on every number, the
    /// switches at `to`'s from the start.
    pub fn lerp(from: &Vision, to: &Vision, frac: f32) -> Vision {
        let s = (frac.clamp(0.0, 1.0) * std::f32::consts::PI * 0.5).sin();
        let l = |a: f32, b: f32| (b - a) * s + a;
        let l3 = |a: [f32; 3], b: [f32; 3]| [0, 1, 2].map(|i| l(a[i], b[i]));
        let l2 = |a: [f32; 2], b: [f32; 2]| [0, 1].map(|i| l(a[i], b[i]));
        let (fg, tg, ff, tf) = (&from.glow, &to.glow, &from.film, &to.film);
        Vision {
            glow: Glow {
                enable: tg.enable,
                bloom_cutoff: l(fg.bloom_cutoff, tg.bloom_cutoff),
                bloom_desaturation: l(fg.bloom_desaturation, tg.bloom_desaturation),
                bloom_intensity: l2(fg.bloom_intensity, tg.bloom_intensity),
                radius: l2(fg.radius, tg.radius),
                sky_bleed_intensity: l2(fg.sky_bleed_intensity, tg.sky_bleed_intensity),
            },
            film: Film {
                enable: tf.enable,
                brightness: l(ff.brightness, tf.brightness),
                contrast: l(ff.contrast, tf.contrast),
                desaturation: l(ff.desaturation, tf.desaturation),
                invert: tf.invert,
                light_tint: l3(ff.light_tint, tf.light_tint),
                dark_tint: l3(ff.dark_tint, tf.dark_tint),
            },
        }
    }
}

/// A line's tokens: words, or a quoted string's inside.
fn tokens(line: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = line;
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with("//") {
            return None;
        }
        if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            let tok = r[..end].to_string();
            rest = r.get(end + 1..).unwrap_or("");
            return Some(tok);
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tok = rest[..end].to_string();
        rest = &rest[end..];
        Some(tok)
    })
}

/// Configstring 824 or 825, `"<name>" <ms>`: the set's name and the fade
/// to it.
pub fn parse_vision_cs(cs: &str) -> (String, i32) {
    let mut t = tokens(cs);
    let name = t.next().unwrap_or_default();
    let ms = t.next().map_or(0.0, |s| atof(&s)) as i32;
    (name, ms)
}

/// The film tweak's three pixel shader constants (`postfx_color.hlsl`):
/// `out = (c * bias.w + lum) * (base + delta * lum) + bias.rgb`, `lum` the
/// texel's Rec. 601 luma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilmConsts {
    /// `colorBias`: the bias on rgb, the desaturation scale in w.
    pub bias: [f32; 4],
    /// `colorTintBase`.
    pub base: [f32; 3],
    /// `colorTintDelta`.
    pub delta: [f32; 3],
}

impl Film {
    /// The film's three shader constants, `None` with the film off
    /// (cod4-world.md 11.3): `R_SetFilmInfo` (0x5f9070) folds in
    /// `r_desaturation`, `r_contrast` and `r_brightness` at their defaults,
    /// then `R_UpdateColorManipulation` (0x5f8e90) takes the desaturation
    /// no lower than 1/4096 as the gain on the tints.
    pub fn consts(&self) -> Option<FilmConsts> {
        const R_DESATURATION: f32 = 1.0;
        const R_CONTRAST: f32 = 1.0;
        const R_BRIGHTNESS: f32 = 0.0;
        if !self.enable {
            return None;
        }
        let desat = self.desaturation;
        let kept = (desat * ((1.0 - desat) * R_DESATURATION + desat)).max(1.0 / 4096.0);
        let contrast = self.contrast * R_CONTRAST;
        let mut gain = contrast * kept;
        let mut lift = self.brightness + R_BRIGHTNESS + 0.5 - contrast * 0.5;
        if self.invert {
            gain = -gain;
            lift += 1.0;
        }
        Some(FilmConsts {
            bias: [lift, lift, lift, 1.0 / kept - 1.0],
            base: self.dark_tint.map(|dark| dark * gain),
            delta: std::array::from_fn(|i| (self.light_tint[i] - self.dark_tint[i]) * gain),
        })
    }
}

impl FilmConsts {
    /// `postfx_color.hlsl` on one framebuffer colour, unclamped.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let lum = c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114;
        [0, 1, 2].map(|i| {
            (c[i] * self.bias[3] + lum) * (self.delta[i] * lum + self.base[i]) + self.bias[i]
        })
    }
}

/// The naked vision set as the cgame blends it: a set named with a fade
/// eases there from what is drawn (`Vision::lerp`); the first set, or one
/// with no fade, snaps.
#[derive(Clone, Debug, Default)]
pub struct VisionBlend {
    from: Vision,
    to: Vision,
    /// Start and length of the running fade, in the cgame's milliseconds.
    fade: Option<(i32, i32)>,
    set: bool,
}

impl VisionBlend {
    /// Starts toward `v` at `now_ms` over `ms`.
    pub fn set(&mut self, v: Vision, ms: i32, now_ms: i32) {
        if ms <= 0 || !self.set {
            self.fade = None;
        } else {
            self.from = self.at(now_ms);
            self.fade = Some((now_ms, ms));
        }
        self.to = v;
        self.set = true;
    }

    /// What draws at `now_ms`.
    pub fn at(&self, now_ms: i32) -> Vision {
        match self.fade {
            Some((start, ms)) if start + ms >= now_ms => {
                let frac = (now_ms - start) as f32 / ms as f32;
                Vision::lerp(&self.from, &self.to, frac.max(0.0))
            }
            _ => self.to,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prematch capture's configstring 9, `setExpFog(500, 3500,
    /// 0.501961, 0.501961, 0.45098, 0)` on mp_crash (cod4-handshake.md 5).
    #[test]
    fn the_captured_fog_parses_and_the_script_writes_it() {
        let cs = "500 0.000198042 0.501961 0.501961 0.45098 0";
        let FogCs::On(f) = parse_fog(cs) else {
            panic!("fog off")
        };
        assert_eq!(f.start, 500.0);
        assert_eq!(f.density, 0.000198042);
        assert_eq!(f.color, [128, 128, 115]);
        assert_eq!(f.transition_ms, 0);
        let art = "main()\n{\n\t// setExpFog( 1, 2, 3, 4, 5, 6 );\n\tsetExpFog( 500, 3500, 0.501961, 0.501961, 0.45098, 0 );\n}\n";
        let s = script_fog(art).unwrap();
        assert_eq!(s.start, 500.0);
        // `%g` of the script's density is the configstring's
        assert_eq!(format!("{:.5e}", s.density), "1.98042e-4");
        assert_eq!(s.color, f.color);
        // the fog keeps half the colour at start + halfway
        let keep = |d: f32| (-s.density * (d - s.start)).exp().min(1.0);
        assert!((keep(4000.0) - 0.5).abs() < 1e-5);
        assert_eq!(keep(400.0), 1.0);
        assert_eq!(parse_fog(""), FogCs::Off { transition_ms: 0 });
        assert_eq!(
            parse_fog("2000"),
            FogCs::Off {
                transition_ms: 2000
            }
        );
    }

    /// The capture's vision configstrings (cod4-gsc.md, cod4-hud.md 13).
    #[test]
    fn vision_configstrings_name_a_set_and_a_fade() {
        assert_eq!(
            parse_vision_cs("\"mp_crash\" 2000"),
            ("mp_crash".into(), 2000)
        );
        assert_eq!(parse_vision_cs("\"mpintro\" 0"), ("mpintro".into(), 0));
        assert_eq!(parse_vision_cs(""), (String::new(), 0));
    }

    const MP_CRASH: &str = "r_glow\t\t\t\t\t\t\"0\"\nr_glowRadius0\t\t\t\t\"7\"\nr_glowRadius1\t\t\t\t\"7\"\nr_glowBloomCutoff\t\t\t\"0.99\"\nr_glowBloomDesaturation\t\t\"0.65\"\nr_glowBloomIntensity0\t\t\"0.36\"\nr_glowBloomIntensity1\t\t\"0.36\"\nr_glowSkyBleedIntensity0\t\"0.29\"\nr_glowSkyBleedIntensity1\t\"0.29\"\n\nr_filmEnable\t\t\t\t\"1\"\nr_filmContrast\t\t\t\t\"1.4\"\nr_filmBrightness\t\t\t\"0.134\"\nr_filmDesaturation\t\t\t\"0.265\"\nr_filmInvert\t\t\t\t\"0\"\nr_filmLightTint\t\t\t\t\"1.179 1.086 1.024\"\nr_filmDarkTint\t\t\t\t\"0.7 1.0215 1.265\"\n";

    #[test]
    fn mp_crash_vision_parses() {
        let v = Vision::parse(MP_CRASH);
        assert!(!v.glow.enable);
        assert_eq!(v.glow.radius, [7.0, 7.0]);
        assert_eq!(v.glow.bloom_cutoff, 0.99);
        assert_eq!(v.glow.bloom_intensity, [0.36, 0.36]);
        assert_eq!(v.glow.sky_bleed_intensity, [0.29, 0.29]);
        let f = v.film;
        assert!(f.enable && !f.invert);
        assert_eq!(
            (f.contrast, f.brightness, f.desaturation),
            (1.4, 0.134, 0.265)
        );
        assert_eq!(f.light_tint, [1.179, 1.086, 1.024]);
        assert_eq!(f.dark_tint, [0.7, 1.0215, 1.265]);
        // a key read once keeps its first value; unknown keys are skipped
        let v = Vision::parse("r_filmContrast \"2\"\nR_FILMCONTRAST \"3\"\ncg_fov \"80\"\n");
        assert_eq!(v.film.contrast, 2.0);
    }

    /// `R_UpdateColorManipulation` on mp_crash's film, and what it does to
    /// black, white and a mid grey.
    #[test]
    fn mp_crash_film_constants() {
        let c = Vision::parse(MP_CRASH).film.consts().unwrap();
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // bias = brightness + 0.5 - contrast / 2
        assert!(close(c.bias[0], 0.134 + 0.5 - 0.7), "{c:?}");
        assert!(close(c.bias[3], 1.0 / 0.265 - 1.0));
        let scale = 1.4 * 0.265;
        assert!(close(c.base[0], 0.7 * scale));
        assert!(close(c.delta[2], (1.024 - 1.265) * scale));
        // black lifts to the bias, white to the light tint's contrast
        let black = c.apply([0.0; 3]);
        assert!(close(black[1], 0.134 + 0.5 - 0.7));
        let white = c.apply([1.0; 3]);
        for i in 0..3 {
            let want = [1.179, 1.086, 1.024][i] * 1.4 + (0.134 + 0.5 - 0.7);
            assert!(close(white[i], want), "{white:?}");
        }
        // off is no constants
        assert_eq!(Film::default().consts(), None);
        // mpintro: grey, contrast 1.8
        let intro = Film {
            enable: true,
            contrast: 1.8,
            brightness: 0.05,
            desaturation: 1.0,
            light_tint: [1.0; 3],
            dark_tint: [1.0; 3],
            ..Film::default()
        };
        let c = intro.consts().unwrap();
        let o = c.apply([1.0, 0.0, 0.0]);
        assert!(close(o[0], o[1]) && close(o[1], o[2]), "{o:?}");
        assert!(close(o[0], 0.299 * 1.8 + 0.05 + 0.5 - 0.9));
    }

    #[test]
    fn a_fade_eases_and_the_first_set_snaps() {
        let intro = Vision {
            film: Film {
                enable: true,
                contrast: 1.8,
                ..Film::default()
            },
            ..Vision::default()
        };
        let map = Vision::parse(MP_CRASH);
        let mut b = VisionBlend::default();
        b.set(intro, 2000, 100);
        assert_eq!(b.at(100), intro);
        b.set(map, 2000, 1000);
        assert_eq!(b.at(1000).film.contrast, 1.8);
        let half = b.at(2000).film.contrast;
        let s = (0.5f32 * std::f32::consts::FRAC_PI_2).sin();
        assert!((half - (1.8 + (1.4 - 1.8) * s)).abs() < 1e-6);
        assert_eq!(b.at(3001), map);
        b.set(intro, 0, 4000);
        assert_eq!(b.at(4000), intro);
    }

    /// FNV-1a over a stream of words: the pins below hash every output's bits.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// The film constants of a spread of films, inverted ones and a
    /// desaturation under the 1/4096 floor included, bit for bit.
    #[test]
    fn pins_the_film_constants() {
        let mut out = Vec::new();
        for (k, invert) in (0..40).zip([false, true].into_iter().cycle()) {
            let k = k as f32;
            let f = Film {
                enable: true,
                brightness: k * 0.037 - 0.6,
                contrast: 0.3 + k * 0.071,
                desaturation: [0.0, 1e-5, 0.265, 0.5, 1.0, 1.7][k as usize % 6],
                invert,
                light_tint: [1.179, 1.086 + k * 0.01, 1.024],
                dark_tint: [0.7, 1.0215, 1.265 - k * 0.013],
            };
            let c = f.consts().unwrap();
            out.extend(c.bias.map(f32::to_bits));
            out.extend(c.base.map(f32::to_bits));
            out.extend(c.delta.map(f32::to_bits));
        }
        assert_eq!(fnv(out), PIN_FILM);
    }
    const PIN_FILM: u64 = 4427718347994114573;

    /// What a vision file's odd lines read: a malformed value leaves its
    /// key to a later line, `%i` stops at the first non-digit, a vec3
    /// short a number is skipped, a comment ends a line.
    #[test]
    fn pins_the_vision_parse_edges() {
        let v = Vision::parse(concat!(
            "r_glow \"x\"\n",
            "R_GLOW \"-3\"\n",
            "r_glow \"0\"\n",
            "r_glowBloomCutoff abc\n",
            "r_glowBloomCutoff .5e1junk\n",
            "r_glowRadius0 \"+7.25\" // trailing\n",
            "// r_glowRadius1 \"9\"\n",
            "r_glowRadius1\n",
            "r_glowRadius1 \"2\"\n",
            "r_filmEnable \"0x1\"\n",
            "r_filmInvert \"12z\"\n",
            "r_filmLightTint \"1 2\"\n",
            "r_filmLightTint \"1 2 x\"\n",
            "r_filmLightTint \"1.5 -2 .25 9\"\n",
            "r_filmDarkTint 0.5 0.25 0.125\n",
            "r_filmDarkTint \"0.5 0.25 0.125\"\n",
            "r_filmContrast \"\"\n",
            "r_filmContrast \" 3\"\n",
            "  r_filmBrightness\t\"-.5\"\n",
            "r_glowSkyBleedIntensity1 \"1e-2\"\n",
        ));
        assert!(v.glow.enable);
        assert_eq!(v.glow.bloom_cutoff, 5.0);
        assert_eq!(v.glow.radius, [7.25, 2.0]);
        assert_eq!(v.glow.sky_bleed_intensity, [0.0, 0.01]);
        assert!(!v.film.enable);
        assert!(v.film.invert);
        assert_eq!(v.film.light_tint, [1.5, -2.0, 0.25]);
        assert_eq!(v.film.dark_tint, [0.5, 0.25, 0.125]);
        assert_eq!(v.film.contrast, 3.0);
        assert_eq!(v.film.brightness, -0.5);
    }
}
