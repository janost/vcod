//! CoD2 1.3 `.efx` effect files: Raven's `GenericParser2` text, read into
//! the engine's `PrimitiveTemplate`s. Every graph is a channel: a
//! piecewise-linear [`Curve`] over the element's life and a random `scale`.
//! Files in CoD1's `start`/`end` form are migrated into channels as the
//! engine does. Grammar, sampling and runtime: docs/research/cod2-efx.md.

use std::fmt;

use anyhow::anyhow;
use vcod_common::asset_source::{Asset, AssetKind, AssetSource};
use vcod_common::text::latin1;

/// `FX_ParseEffect` keeps at most this many primitives (Mac 0x5d218).
pub const MAX_PRIMITIVES: usize = 24;

/// A primitive block's type, `PrimitiveTemplate +0x40` (Mac 0x5d218).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Particle = 1,
    Line = 2,
    Tail = 3,
    Cylinder = 4,
    Emitter = 5,
    Decal = 6,
    OrientedParticle = 7,
    FxRunner = 8,
    Light = 9,
    CameraShake = 10,
    Flash = 11,
    Cloud = 12,
}

impl Kind {
    /// The block keyword, compared without case; anything else is skipped.
    pub fn from_name(name: &str) -> Option<Kind> {
        Some(match name.to_ascii_lowercase().as_str() {
            "particle" => Kind::Particle,
            "line" => Kind::Line,
            "tail" => Kind::Tail,
            "cylinder" => Kind::Cylinder,
            "emitter" => Kind::Emitter,
            "decal" => Kind::Decal,
            "orientedparticle" => Kind::OrientedParticle,
            "fxrunner" => Kind::FxRunner,
            "light" => Kind::Light,
            "camerashake" => Kind::CameraShake,
            "flash" => Kind::Flash,
            "cloud" => Kind::Cloud,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Particle => "Particle",
            Kind::Line => "Line",
            Kind::Tail => "Tail",
            Kind::Cylinder => "Cylinder",
            Kind::Emitter => "Emitter",
            Kind::Decal => "Decal",
            Kind::OrientedParticle => "OrientedParticle",
            Kind::FxRunner => "FxRunner",
            Kind::Light => "Light",
            Kind::CameraShake => "CameraShake",
            Kind::Flash => "Flash",
            Kind::Cloud => "Cloud",
        }
    }

    /// Particle, Tail and OrientedParticle need a material (Mac 0x5d3a3).
    fn needs_material(self) -> bool {
        matches!(self, Kind::Particle | Kind::Tail | Kind::OrientedParticle)
    }
}

/// `PrimitiveTemplate +0x90` bits, `_fxAttributeFlags` (Mac 0x36a800).
pub mod attr {
    pub const DEPTH_HACK: u32 = 0x1;
    pub const RELATIVE: u32 = 0x2;
    pub const SET_SHADER_TIME: u32 = 0x4;
    pub const USE_MODEL: u32 = 0x10;
    pub const USE_PHYSICS: u32 = 0x20;
    pub const USE_BBOX: u32 = 0x40;
    pub const USE_ALPHA: u32 = 0x80;
    pub const EMIT_FX: u32 = 0x100;
    pub const DEATH_FX: u32 = 0x200;
    pub const IMPACT_KILLS: u32 = 0x400;
    pub const IMPACT_FX: u32 = 0x800;
    pub const BLOCKS_SIGHT: u32 = 0x1000;
    pub const USE_RANDOM_COLORS: u32 = 0x2000;
    pub const USE_RANDOM_ALPHA: u32 = 0x4000;
    pub const USE_RANDOM_SIZE: u32 = 0x8000;
    pub const USE_RANDOM_SIZE2: u32 = 0x10000;
    pub const USE_RANDOM_LENGTH: u32 = 0x20000;
    pub const USE_RANDOM_ROTATION_DELTA: u32 = 0x40000;
    pub const USE_RANDOM_VELOCITY: u32 = 0x80000;
    pub const USE_RANDOM_VELOCITY2: u32 = 0x100000;
    pub const ABSOLUTE_VEL: u32 = 0x200000;
    pub const ABSOLUTE_VEL2: u32 = 0x400000;
    pub const AFFECTED_BY_WIND: u32 = 0x800000;
    pub const DISABLE_FAR_PLANE_CULLING: u32 = 0x2000000;

    /// The table in its binary order.
    pub const NAMES: [(&str, u32); 24] = [
        ("depthHack", DEPTH_HACK),
        ("setShaderTime", SET_SHADER_TIME),
        ("useModel", USE_MODEL),
        ("useBBox", USE_BBOX),
        ("usePhysics", USE_PHYSICS),
        ("impactKills", IMPACT_KILLS),
        ("impactFx", IMPACT_FX),
        ("deathFx", DEATH_FX),
        ("useAlpha", USE_ALPHA),
        ("useRandomColors", USE_RANDOM_COLORS),
        ("useRandomAlpha", USE_RANDOM_ALPHA),
        ("useRandomSize", USE_RANDOM_SIZE),
        ("useRandomSize2", USE_RANDOM_SIZE2),
        ("useRandomLength", USE_RANDOM_LENGTH),
        ("useRandomRotationDelta", USE_RANDOM_ROTATION_DELTA),
        ("useRandomVelocity", USE_RANDOM_VELOCITY),
        ("useRandomVelocity2", USE_RANDOM_VELOCITY2),
        ("absoluteVel", ABSOLUTE_VEL),
        ("absoluteVel2", ABSOLUTE_VEL2),
        ("affectedByWind", AFFECTED_BY_WIND),
        ("emitFx", EMIT_FX),
        ("relative", RELATIVE),
        ("blocksSight", BLOCKS_SIGHT),
        ("disableFarPlaneCulling", DISABLE_FAR_PLANE_CULLING),
    ];
}

/// `PrimitiveTemplate +0x94` bits, `_fxSpawnFlags` (Mac 0x36a760). Two of
/// its thirteen names set attribute bits instead: `absoluteVel` and
/// `absoluteAccel` (the latter [`attr::ABSOLUTE_VEL2`]).
pub mod spawn {
    pub const ORG_ON_SPHERE: u32 = 0x1;
    pub const AXIS_FROM_SPHERE: u32 = 0x2;
    pub const ORG_ON_CYLINDER: u32 = 0x4;
    pub const ORG2_FROM_TRACE: u32 = 0x8;
    pub const TRACE_IMPACT_FX: u32 = 0x10;
    pub const ORG2_IS_OFFSET: u32 = 0x20;
    pub const CHEAP_ORG_CALC: u32 = 0x40;
    pub const CHEAP_ORG2_CALC: u32 = 0x80;
    pub const RAND_ROT_AROUND_FWD: u32 = 0x100;
    pub const EVEN_DISTRIBUTION: u32 = 0x200;
    pub const FRUSTUM_CULL: u32 = 0x400;

    /// (name, attribute bits, spawn bits), in the binary's order.
    pub const NAMES: [(&str, u32, u32); 13] = [
        ("org2fromTrace", 0, ORG2_FROM_TRACE),
        ("traceImpactFx", 0, TRACE_IMPACT_FX),
        ("org2isOffset", 0, ORG2_IS_OFFSET),
        ("cheapOrgCalc", 0, CHEAP_ORG_CALC),
        ("cheapOrg2Calc", 0, CHEAP_ORG2_CALC),
        ("orgOnSphere", 0, ORG_ON_SPHERE),
        ("orgOnCylinder", 0, ORG_ON_CYLINDER),
        ("axisFromSphere", 0, AXIS_FROM_SPHERE),
        ("randrotaroundfwd", 0, RAND_ROT_AROUND_FWD),
        ("evenDistribution", 0, EVEN_DISTRIBUTION),
        ("frustumCull", 0, FRUSTUM_CULL),
        ("absoluteVel", super::attr::ABSOLUTE_VEL, 0),
        ("absoluteAccel", super::attr::ABSOLUTE_VEL2, 0),
    ];
}

/// The 24 channels, `PrimitiveTemplate +0x100 + 12 * id`, by group name
/// (`ParsePrimitiveInternal`, Mac 0x5f778). `*Rand` is the curve a
/// `useRandom*` element blends towards by its per-particle weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rgb = 0,
    RgbRand,
    Alpha,
    AlphaRand,
    Size,
    SizeRand,
    Size2,
    Size2Rand,
    Length,
    LengthRand,
    RotationDelta,
    RotationDeltaRand,
    VelocityX,
    VelocityY,
    VelocityZ,
    VelocityXRand,
    VelocityYRand,
    VelocityZRand,
    Velocity2X,
    Velocity2Y,
    Velocity2Z,
    Velocity2XRand,
    Velocity2YRand,
    Velocity2ZRand,
}

pub const CHANNELS: usize = 24;

impl Channel {
    /// Every group name `ParsePrimitiveInternal` takes, with its aliases.
    fn from_group(name: &str) -> Option<Channel> {
        use Channel::*;
        Some(match name.to_ascii_lowercase().as_str() {
            "rgb" => Rgb,
            "rgb2" | "rgbrand" => RgbRand,
            "alpha" => Alpha,
            "alpharand" => AlphaRand,
            "size" | "width" => Size,
            "sizerand" => SizeRand,
            "size2" | "width2" => Size2,
            "size2rand" => Size2Rand,
            "length" | "height" => Length,
            "lengthrand" => LengthRand,
            "rotationdelta" => RotationDelta,
            "rotationdeltarand" => RotationDeltaRand,
            "velocityx" => VelocityX,
            "velocityy" => VelocityY,
            "velocityz" => VelocityZ,
            "velocityxrand" => VelocityXRand,
            "velocityyrand" => VelocityYRand,
            "velocityzrand" => VelocityZRand,
            "velocity2x" => Velocity2X,
            "velocity2y" => Velocity2Y,
            "velocity2z" => Velocity2Z,
            "velocity2xrand" => Velocity2XRand,
            "velocity2yrand" => Velocity2YRand,
            "velocity2zrand" => Velocity2ZRand,
            _ => return None,
        })
    }

    /// Values per key: 3 for the two colour channels, 1 for the rest.
    pub fn dims(self) -> usize {
        if (self as usize) < 2 { 3 } else { 1 }
    }
}

/// A piecewise-linear curve over a life fraction: keys of `(t, v...)`, `t`
/// ascending from 0 to 1 (`FxCurve`, Mac 0xac320).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Values per key, 1 or 3.
    pub dims: usize,
    /// `(dims + 1)` floats per key.
    pub keys: Vec<f32>,
}

impl Curve {
    /// `FxCurve_AllocAndCreateWithKeys` (Mac 0xac320): a key at 0 is added
    /// with the first key's values when the first `t` is not 0, and one at
    /// 1 when the last is not 1, also with the first key's values as the
    /// decompile reads. No stock curve needs either.
    pub fn new(dims: usize, keys: &[f32]) -> Curve {
        let stride = dims + 1;
        if keys.len() < stride {
            // No key: the engine reads past its buffer; a flat 0 here.
            let mut k = vec![0.0; stride * 2];
            k[stride] = 1.0;
            return Curve { dims, keys: k };
        }
        let first = &keys[..stride];
        let last_t = keys[keys.len() - stride];
        let mut out = Vec::with_capacity(keys.len() + 2 * stride);
        if first[0] != 0.0 {
            out.push(0.0);
            out.extend_from_slice(&first[1..]);
        }
        out.extend_from_slice(&keys[..keys.len() / stride * stride]);
        if last_t != 1.0 {
            out.push(1.0);
            out.extend_from_slice(&first[1..]);
        }
        Curve { dims, keys: out }
    }

    /// Two keys, `a` at 0 and `b` at 1 (`FxChannel_CreateDefault`).
    pub fn line(dims: usize, a: f32, b: f32) -> Curve {
        let mut k = vec![0.0];
        k.extend(std::iter::repeat_n(a, dims));
        k.push(1.0);
        k.extend(std::iter::repeat_n(b, dims));
        Curve { dims, keys: k }
    }

    pub fn key_count(&self) -> usize {
        self.keys.len() / (self.dims + 1)
    }

    fn key(&self, i: usize) -> &[f32] {
        let s = self.dims + 1;
        &self.keys[i * s..i * s + s]
    }

    /// The segment `t` falls in: the first key pair whose second `t` is at
    /// or past it, the iterator's walk (`Particle::UpdateAlpha`, Mac
    /// 0xa5178). `t` is clamped to the curve's span.
    fn segment(&self, t: f32) -> (usize, f32) {
        let n = self.key_count();
        let mut i = 0;
        while i + 2 < n && self.key(i + 1)[0] < t {
            i += 1;
        }
        let (t0, t1) = (self.key(i)[0], self.key(i + 1)[0]);
        let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        (i, f.clamp(0.0, 1.0))
    }

    /// The value at life fraction `t`, linear between the keys either side.
    /// Unused components of a 1-wide curve read 0.
    pub fn sample(&self, t: f32) -> [f32; 3] {
        let (i, f) = self.segment(t);
        let (a, b) = (self.key(i), self.key(i + 1));
        let mut out = [0.0; 3];
        for d in 0..self.dims.min(3) {
            out[d] = a[d + 1] + (b[d + 1] - a[d + 1]) * f;
        }
        out
    }

    /// [`Self::sample`]'s first component.
    pub fn sample1(&self, t: f32) -> f32 {
        self.sample(t)[0]
    }

    /// The area under a 1-wide curve from 0 to `t` (`FxCurve_Integrate`,
    /// Mac 0xb9fde): whole segments by the trapezoid rule, then the
    /// partial one up to `t`.
    pub fn integrate(&self, t: f32) -> f32 {
        let n = self.key_count();
        let mut sum = 0.0;
        let mut i = 0;
        while i + 2 < n && self.key(i + 1)[0] < t {
            let (a, b) = (self.key(i), self.key(i + 1));
            sum += (a[1] + b[1]) * 0.5 * (b[0] - a[0]);
            i += 1;
        }
        let (a, b) = (self.key(i), self.key(i + 1));
        let dt = t - a[0];
        let span = b[0] - a[0];
        let at_t = if span > 0.0 {
            a[1] + (b[1] - a[1]) * dt / span
        } else {
            a[1]
        };
        sum + dt * (a[1] + at_t) * 0.5
    }
}

/// One channel: its curve and the range a particle's scale is drawn from.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDef {
    pub curve: Curve,
    pub scale: (f32, f32),
}

/// A uniform range, `FxRange`; one value in the file means both ends.
pub type Range = (f32, f32);

/// One `PrimitiveTemplate`, the fields the runtime reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    pub kind: Kind,
    /// The block's index in the file, `+0x44`.
    pub index: usize,
    pub name: String,
    /// [`attr`] bits.
    pub flags: u32,
    /// [`spawn`] bits.
    pub spawn_flags: u32,
    pub count: Range,
    /// Milliseconds.
    pub life: Range,
    pub delay: Range,
    /// Camera distance outside which the primitive is not spawned; 0 for
    /// either end means unbounded (`cullrange` sets the far end alone).
    pub spawn_range: Range,
    /// `bounce` or `intensity`, 0..1; either turns `usePhysics` on.
    pub bounce: Option<Range>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Per axis, `(min, max)`.
    pub angle: [Range; 3],
    pub angle_delta: [Range; 3],
    pub origin: [Range; 3],
    pub origin2: [Range; 3],
    pub radius: Range,
    pub height: Range,
    pub wind: Range,
    /// Degrees.
    pub rotation: Range,
    /// Units per second squared, up.
    pub gravity: Range,
    pub density: Range,
    pub variance: Range,
    pub non_uniform_scale: bool,
    pub use_length: bool,
    pub shader_impact: String,
    /// 0 fixed frame, 1 random, 2 the spawn's index.
    pub sequence_start_frame_mode: u32,
    /// 1-based.
    pub sequence_fixed_frame: i32,
    /// 0 fixed rate, 1 the whole atlas over the life.
    pub sequence_play_rate_mode: u32,
    /// Frames per second.
    pub sequence_fixed_fps: f32,
    pub sequence_loop_mode: u32,
    pub sequence_loop_times: i32,
    pub spawn_frustum_cull_radius: f32,
    /// A CoD4 element's `fadeInRange` and `fadeOutRange` as `(base,
    /// amplitude)`, the camera distances its alpha fades over; CoD2's are
    /// 0 (no fade).
    pub fade_ranges: [Range; 2],
    pub materials: Vec<String>,
    pub models: Vec<String>,
    pub impact_fx: Vec<String>,
    pub death_fx: Vec<String>,
    pub emit_fx: Vec<String>,
    pub play_fx: Vec<String>,
    pub channels: Vec<ChannelDef>,
}

impl Primitive {
    /// `PrimitiveTemplate::Init` (Mac 0x5d9b4).
    pub fn new(kind: Kind, index: usize) -> Primitive {
        let one = (1.0, 1.0);
        let zero = (0.0, 0.0);
        let channels = (0..CHANNELS)
            .map(|i| {
                let (dims, a) = match i {
                    0 | 1 => (3, 1.0),
                    2..=9 => (1, 1.0),
                    _ => (1, 0.0),
                };
                ChannelDef {
                    curve: Curve::line(dims, a, 0.0),
                    scale: one,
                }
            })
            .collect();
        Primitive {
            kind,
            index,
            name: String::new(),
            flags: 0,
            spawn_flags: 0,
            count: one,
            life: one,
            delay: zero,
            spawn_range: zero,
            bounce: None,
            min: [0.0; 3],
            max: [0.0; 3],
            angle: [zero; 3],
            angle_delta: [zero; 3],
            origin: [zero; 3],
            origin2: [zero; 3],
            radius: one,
            height: one,
            wind: zero,
            rotation: zero,
            gravity: zero,
            density: (10.0, 10.0),
            variance: one,
            non_uniform_scale: false,
            use_length: false,
            shader_impact: String::new(),
            sequence_start_frame_mode: 0,
            sequence_fixed_frame: 1,
            sequence_play_rate_mode: 0,
            sequence_fixed_fps: 1.0,
            sequence_loop_mode: 0,
            sequence_loop_times: 1,
            spawn_frustum_cull_radius: 0.0,
            fade_ranges: [zero; 2],
            materials: Vec::new(),
            models: Vec::new(),
            impact_fx: Vec::new(),
            death_fx: Vec::new(),
            emit_fx: Vec::new(),
            play_fx: Vec::new(),
            channels,
        }
    }

    pub fn channel(&self, c: Channel) -> &ChannelDef {
        &self.channels[c as usize]
    }

    pub fn has(&self, bits: u32) -> bool {
        self.flags & bits != 0
    }

    pub fn has_spawn(&self, bits: u32) -> bool {
        self.spawn_flags & bits != 0
    }
}

/// A parsed effect: its primitives in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub primitives: Vec<Primitive>,
}

/// Why the engine refuses a file; it then plays `misc/missing_fx`.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(msg: String) -> Result<T, ParseError> {
    Err(ParseError(msg))
}

// ---------------------------------------------------------------------
// GenericParser2: the text into groups, pairs and lists.

/// A value: a pair's text to the end of its line, or a `[ ... ]` list.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    List(Vec<String>),
}

/// A `{ }` group. Pairs and subgroups are kept apart, each in file order,
/// as `GPGroup` keeps them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub name: String,
    pub pairs: Vec<(String, Value)>,
    pub groups: Vec<Group>,
}

/// `GetToken` (Mac 0xab80c) over bytes. A byte is blank when it is below
/// `!` as a signed char, so every byte from 0x80 up is blank too.
struct Lexer<'a> {
    s: &'a [u8],
    pos: usize,
}

fn blank(c: u8) -> bool {
    (c as i8) < b'!' as i8
}

impl Lexer<'_> {
    /// Skip blanks and `//` and `/* */` comments; false at the end.
    fn skip(&mut self) -> bool {
        loop {
            while self.pos < self.s.len() && blank(self.s[self.pos]) {
                if self.s[self.pos] == 0 {
                    return false;
                }
                self.pos += 1;
            }
            if self.pos >= self.s.len() {
                return false;
            }
            let rest = &self.s[self.pos..];
            if rest.starts_with(b"//") {
                while self.pos < self.s.len() && self.s[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else if rest.starts_with(b"/*") {
                self.pos += 2;
                while self.pos < self.s.len() && !self.s[self.pos..].starts_with(b"*/") {
                    self.pos += 1;
                }
                self.pos = (self.pos + 2).min(self.s.len());
            } else {
                return true;
            }
        }
    }

    /// A blank-delimited word (a key, or `}`); a quoted one runs to its
    /// closing quote. Empty at the end.
    fn word(&mut self) -> String {
        if !self.skip() {
            return String::new();
        }
        let start = self.pos;
        if self.s[self.pos] == b'"' {
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.s.len() && self.s[self.pos] != b'"' && self.s[self.pos] != 0 {
                self.pos += 1;
            }
            let w = latin1(&self.s[start..self.pos]);
            self.pos = (self.pos + 1).min(self.s.len());
            return w;
        }
        while self.pos < self.s.len() && !blank(self.s[self.pos]) {
            self.pos += 1;
        }
        latin1(&self.s[start..self.pos])
    }

    /// The rest of the line from the next non-blank byte (a value): up to a
    /// newline, a carriage return or a comment, trailing blanks dropped,
    /// one pair of surrounding quotes removed. Empty at the end.
    fn rest_of_line(&mut self) -> String {
        if !self.skip() {
            return String::new();
        }
        let start = self.pos;
        while self.pos < self.s.len() {
            let r = &self.s[self.pos..];
            if r[0] == b'\n' || r[0] == b'\r' || r.starts_with(b"//") || r.starts_with(b"/*") {
                break;
            }
            self.pos += 1;
        }
        let mut end = self.pos;
        while end > start && blank(self.s[end - 1]) {
            end -= 1;
        }
        let mut v = &self.s[start..end];
        if v.first() == Some(&b'"') {
            v = &v[1..];
            if v.last() == Some(&b'"') {
                v = &v[..v.len() - 1];
            }
        }
        latin1(v)
    }
}

/// `GPGroup::Parse` (Mac 0xac080): `key {` opens a subgroup, `key [`
/// a list of one value a line up to `]`, anything else is a pair whose
/// value is the rest of the line. A group ends at `}` or the text's end;
/// a list left open fails the parse.
fn parse_group(lx: &mut Lexer, name: String) -> Result<Group, ParseError> {
    let mut g = Group {
        name,
        ..Group::default()
    };
    loop {
        let key = lx.word();
        if key.is_empty() || key == "}" {
            return Ok(g);
        }
        let v = lx.rest_of_line();
        if v == "{" {
            let sub = parse_group(lx, key)?;
            g.groups.push(sub);
        } else if v == "[" {
            let mut items = Vec::new();
            loop {
                let item = lx.rest_of_line();
                if item.is_empty() {
                    return err(format!("list '{key}' not closed"));
                }
                if item == "]" {
                    break;
                }
                items.push(item);
            }
            g.pairs.push((key, Value::List(items)));
        } else {
            g.pairs.push((key, Value::Text(v)));
        }
    }
}

/// The file as `GenericParser2` reads it, its top level a group.
pub fn parse_tree(text: &[u8]) -> Result<Group, ParseError> {
    let mut lx = Lexer { s: text, pos: 0 };
    parse_group(&mut lx, "Top Level".into())
}

// ---------------------------------------------------------------------
// The primitive's keys.

/// `sscanf("%f %f ...")`: the leading numbers, stopping at the first word
/// that is not one.
fn floats(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    for w in s.split_ascii_whitespace() {
        match lead_float(w) {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out
}

/// `%f` on one word: the longest leading float, `None` without one.
fn lead_float(w: &str) -> Option<f32> {
    let ends: Vec<usize> = w.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    ends.iter().rev().find_map(|&e| w[..e].parse::<f32>().ok())
}

/// `atoi`.
fn int(s: &str) -> i32 {
    let t = s.trim_start();
    let (sign, digits) = match t.as_bytes().first() {
        Some(b'-') => (-1, &t[1..]),
        Some(b'+') => (1, &t[1..]),
        _ => (1, t),
    };
    let n: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
    sign * n.parse::<i32>().unwrap_or(0)
}

/// One or two numbers: `(a, b)` with a lone `a` doubled.
fn range(s: &str) -> Option<Range> {
    match floats(s).as_slice() {
        [] => None,
        [a] => Some((*a, *a)),
        [a, b, ..] => Some((*a, *b)),
    }
}

/// Three or six numbers: per axis `(min, max)`; four or five fail.
fn range3(s: &str) -> Option<[Range; 3]> {
    let v = floats(s);
    match v.len() {
        3 => Some([0, 1, 2].map(|i| (v[i], v[i]))),
        n if n >= 6 => Some([0, 1, 2].map(|i| (v[i], v[i + 3]))),
        _ => None,
    }
}

fn sorted((a, b): Range) -> Range {
    if b < a { (b, a) } else { (a, b) }
}

/// `ParseFlags` (Mac 0x5e5c4): every word must name an entry.
fn parse_attr_flags(p: &mut Primitive, s: &str) -> Result<(), String> {
    for w in s.split_ascii_whitespace() {
        let (_, bits) = attr::NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(w))
            .ok_or_else(|| format!("unknown flag '{w}'"))?;
        p.flags |= bits;
    }
    Ok(())
}

fn parse_spawn_flags(p: &mut Primitive, s: &str) -> Result<(), String> {
    for w in s.split_ascii_whitespace() {
        let (_, a, sp) = spawn::NAMES
            .iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(w))
            .ok_or_else(|| format!("unknown spawn flag '{w}'"))?;
        p.flags |= a;
        p.spawn_flags |= sp;
    }
    Ok(())
}

/// The CoD1 form of one channel (`BackCompatibleParameters`, 0x40 bytes a
/// channel): `start`/`end` ranges per component, `parm`, the group flags.
#[derive(Clone, Copy, Debug)]
struct Legacy {
    start: [Range; 3],
    end: [Range; 3],
    parm: Range,
    flags: u32,
    present: bool,
}

impl Default for Legacy {
    fn default() -> Self {
        Legacy {
            start: [(1.0, 1.0); 3],
            end: [(1.0, 1.0); 3],
            parm: (1.0, 1.0),
            flags: 0,
            present: false,
        }
    }
}

/// `ParseGroupFlags`' words (Mac 0x5d79c).
mod group_flag {
    pub const LINEAR: u32 = 1;
    pub const RANDOM: u32 = 2;
    pub const NONLINEAR: u32 = 4;
    pub const WAVE: u32 = 8;
    pub const CLAMP: u32 = 0xc;
}

fn mid((a, b): Range) -> f32 {
    a + (b - a) * 0.5
}

/// `ParseChannel` (Mac 0x5e8d0): `curve`, `scale`, and CoD1's `start`,
/// `end`, `parm` and `flags`.
fn parse_channel(
    p: &mut Primitive,
    legacy: &mut Legacy,
    g: &Group,
    c: Channel,
) -> Result<(), String> {
    let colour = c.dims() == 3;
    for (key, v) in &g.pairs {
        let text = match v {
            Value::Text(t) => t.as_str(),
            Value::List(items) if key.eq_ignore_ascii_case("curve") => {
                let per = if colour { 4 } else { 2 };
                let mut keys = Vec::new();
                for row in items {
                    let mut f = floats(row);
                    f.resize(per, 0.0);
                    f.truncate(per);
                    keys.extend(f);
                }
                p.channels[c as usize].curve = Curve::new(c.dims(), &keys);
                continue;
            }
            Value::List(_) => continue,
        };
        match key.to_ascii_lowercase().as_str() {
            "scale" => {
                if let Some(r) = range(text) {
                    p.channels[c as usize].scale = r;
                }
            }
            k @ ("start" | "end") => {
                let slot = if k == "start" {
                    &mut legacy.start
                } else {
                    &mut legacy.end
                };
                if colour {
                    if let Some(r) = range3(text) {
                        *slot = r;
                        legacy.present = true;
                    }
                } else if let Some(r) = range(text) {
                    slot[0] = sorted(r);
                    legacy.present = true;
                }
            }
            "parm" | "parms" => {
                if let Some(r) = range(text) {
                    legacy.parm = r;
                    legacy.present = true;
                }
            }
            "flags" | "flag" => {
                let mut bits = 0;
                let mut ok = true;
                for w in text.split_ascii_whitespace() {
                    bits |= match w.to_ascii_lowercase().as_str() {
                        "linear" => group_flag::LINEAR,
                        "nonlinear" => group_flag::NONLINEAR,
                        "wave" => group_flag::WAVE,
                        "random" => group_flag::RANDOM,
                        "clamp" => group_flag::CLAMP,
                        _ => {
                            ok = false;
                            0
                        }
                    };
                }
                if ok {
                    legacy.flags |= bits;
                    legacy.present = true;
                }
            }
            // "Unknown key parsing a channel" is printed and skipped.
            _ => {}
        }
    }
    Ok(())
}

/// `FxChannel_CreateViaMigration` (Mac 0xac4e4): a CoD1 channel as a
/// curve. Values are normalised by the larger of the start and end
/// midpoints (times 4/3) into the scale, except for alpha. `linear` is a
/// line from start to end, `nonlinear` holds start until `parm` percent
/// of the life, no flag holds start; `wave` and `clamp` are sampled at 16
/// keys. `random` bakes a draw at load; vcod takes the value it draws
/// below (docs/research/cod2-efx.md 3.4).
fn migrate(l: &Legacy, c: Channel, life: Range) -> ChannelDef {
    let dims = c.dims();
    let alpha = c == Channel::Alpha;
    let (mut norm, mut inv) = (1.0f32, 1.0f32);
    if !alpha {
        let m = mid(l.end[0]).max(mid(l.start[0])) * (4.0 / 3.0);
        if m >= 1.0 {
            norm = m;
            inv = 1.0 / m;
        }
    }
    let half = (l.start[0].1 - l.start[0].0) * 0.5;
    let scale = (norm - half, norm + half);
    let start: Vec<f32> = (0..dims).map(|i| mid(l.start[i]) * inv).collect();
    let end: Vec<f32> = (0..dims).map(|i| mid(l.end[i]) * inv).collect();
    let p = mid(l.parm) * inv;
    let p_frac = p * 0.01;
    let p_wave = p * std::f32::consts::PI * 0.001;
    let linear = l.flags & group_flag::LINEAR != 0;
    let shape = l.flags & 0xc;
    let key = |t: f32, v: &[f32]| {
        let mut k = vec![t];
        k.extend_from_slice(v);
        k
    };
    let curve = if !linear && shape == group_flag::NONLINEAR {
        let mut k = key(0.0, &start);
        k.extend(key(p_frac, &start));
        k.extend(key(1.0, &end));
        Curve::new(dims, &k)
    } else if linear && shape == 0 {
        let mut k = key(0.0, &start);
        k.extend(key(1.0, &end));
        Curve::new(dims, &k)
    } else {
        // Sixteen keys at t = i / 15; `w` is start's weight.
        let mut k = Vec::new();
        for i in 0..16 {
            let t = (i as f32 / 15.0).min(1.0);
            let base = if linear { 1.0 - t } else { 1.0 };
            let w = match shape {
                group_flag::WAVE => (p_wave * mid(life) * t).cos() * base,
                group_flag::CLAMP => {
                    let c = if t < p_frac {
                        (p_frac - t) / p_frac
                    } else {
                        0.0
                    };
                    if linear { c * 0.5 + base * 0.5 } else { c }
                }
                group_flag::NONLINEAR => {
                    let n = if p_frac < t {
                        1.0 - (t - p_frac) / (1.0 - p_frac)
                    } else {
                        1.0
                    };
                    n * 0.5 + base * 0.5
                }
                _ => base,
            };
            k.push(t);
            for d in 0..dims {
                k.push((w * start[d] + (1.0 - w) * end[d]).max(0.0));
            }
        }
        Curve::new(dims, &k)
    };
    ChannelDef { curve, scale }
}

/// `ParseVelocity` (Mac 0x5ed92): a CoD1 `velocity` as flat velocity
/// curves at the minimum, normalised by the largest magnitude, and when
/// the range is not a point `useRandomVelocity` with flat rand curves at
/// the maximum.
fn legacy_velocity(p: &mut Primitive, v: [Range; 3]) {
    legacy_vector(p, v, Channel::VelocityX, Channel::VelocityXRand, 1.0);
    if v.iter().any(|(a, b)| a != b) {
        p.flags |= attr::USE_RANDOM_VELOCITY;
    }
}

/// `ParseAcceleration` (Mac 0x5f244): a CoD1 `acceleration` as velocity2
/// lines from 0 to the acceleration over the longest life, so the second
/// velocity grows as the acceleration would have made it.
fn legacy_acceleration(p: &mut Primitive, v: [Range; 3]) {
    let secs = p.life.1 * 0.001;
    legacy_vector(p, v, Channel::Velocity2X, Channel::Velocity2XRand, secs);
    if v.iter().any(|(a, b)| a != b) {
        p.flags |= attr::USE_RANDOM_VELOCITY2;
    }
}

fn legacy_vector(p: &mut Primitive, v: [Range; 3], first: Channel, rand: Channel, ramp: f32) {
    let v = v.map(sorted);
    let norm = v
        .iter()
        .map(|&(a, b)| a.abs().max(b.abs()))
        .fold(0.0f32, f32::max)
        * ramp;
    for (axis, &(lo, hi)) in v.iter().enumerate() {
        let div = |x: f32| if norm != 0.0 { x * ramp / norm } else { 0.0 };
        let curve = |x: f32| {
            if ramp == 1.0 {
                Curve::line(1, div(x), div(x))
            } else {
                Curve::line(1, 0.0, div(x))
            }
        };
        p.channels[first as usize + axis] = ChannelDef {
            curve: curve(lo),
            scale: (norm, norm),
        };
        p.channels[rand as usize + axis] = ChannelDef {
            curve: curve(hi),
            scale: (norm, norm),
        };
    }
}

/// `ParseRotationDelta` and `CreateBackCompatibleRotationDeltaCurve` (Mac
/// 0x5e79e, 0x5e16e): CoD1's `rotationDelta a b` as twenty keys that
/// decay by `life / 20 * 0.00065` a key, scaled by the larger magnitude.
fn legacy_rotation_delta(p: &mut Primitive, r: Range) {
    let (a, b) = sorted(r);
    let scale = (a.trunc().abs()).max(b);
    let curve = |v: f32, life: f32| {
        let k = life / 20.0 * 0.00065;
        let first = if scale != 0.0 { v / scale } else { v };
        let decay = if k <= 0.0 {
            1.0
        } else if k < 1.0 {
            1.0 - k
        } else {
            0.0
        };
        let mut keys = Vec::new();
        let mut val = first;
        for i in 0..20 {
            keys.push(i as f32 / 19.0);
            keys.push(val);
            val *= decay;
        }
        Curve::new(1, &keys)
    };
    p.channels[Channel::RotationDelta as usize] = ChannelDef {
        curve: curve(a, p.life.0),
        scale: (scale, scale),
    };
    if a != b {
        p.flags |= attr::USE_RANDOM_ROTATION_DELTA;
    }
    p.channels[Channel::RotationDeltaRand as usize] = ChannelDef {
        curve: curve(b, p.life.1),
        scale: (1.0, 1.0),
    };
}

fn list(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => items.clone(),
        Value::Text(t) => t.split_ascii_whitespace().map(String::from).collect(),
    }
}

/// `ParsePrimitiveInternal` (Mac 0x5f778): the pairs, then the channel
/// groups, then the CoD1 channels migrated. Any key it does not know, or
/// a value it cannot read, refuses the whole effect.
fn parse_primitive(kind: Kind, index: usize, g: &Group) -> Result<Primitive, String> {
    let mut p = Primitive::new(kind, index);
    let mut legacy = [Legacy::default(); CHANNELS];
    let bad = |k: &str| format!("bad value for '{k}'");
    for (key, v) in &g.pairs {
        let lk = key.to_ascii_lowercase();
        let text = match v {
            Value::Text(t) => t.as_str(),
            Value::List(_) => "",
        };
        match lk.as_str() {
            "count" => p.count = range(text).ok_or_else(|| bad(key))?,
            "shaders" | "shader" => p.materials = list(v),
            "models" | "model" => p.models = list(v),
            "impactfx" => {
                p.impact_fx = list(v);
                p.flags |= attr::IMPACT_FX | attr::USE_PHYSICS;
            }
            "deathfx" => {
                p.death_fx = list(v);
                p.flags |= attr::DEATH_FX;
            }
            "emitfx" => {
                p.emit_fx = list(v);
                p.flags |= attr::EMIT_FX;
            }
            "playfx" => p.play_fx = list(v),
            "life" => p.life = range(text).ok_or_else(|| bad(key))?,
            "cullrange" => p.spawn_range.1 = floats(text).first().copied().unwrap_or(0.0),
            "spawnrange" => {
                let r = range(text).ok_or_else(|| bad(key))?;
                if r.1 < r.0 {
                    return Err(bad(key));
                }
                p.spawn_range = r;
            }
            "delay" => p.delay = range(text).ok_or_else(|| bad(key))?,
            "bounce" | "intensity" => {
                let r = range(text).ok_or_else(|| bad(key))?;
                if r.1 < r.0 || !(0.0..=1.0).contains(&r.0) || !(0.0..=1.0).contains(&r.1) {
                    return Err(bad(key));
                }
                p.bounce = Some(r);
                p.flags |= attr::USE_PHYSICS;
            }
            "min" | "max" => {
                let v = floats(text);
                if v.len() < 3 || v.len() == 4 || v.len() == 5 {
                    return Err(bad(key));
                }
                let dst = if lk == "min" { &mut p.min } else { &mut p.max };
                *dst = [v[0], v[1], v[2]];
                p.flags |= attr::USE_BBOX | attr::USE_PHYSICS;
            }
            "angle" | "angles" => p.angle = range3(text).ok_or_else(|| bad(key))?,
            "angledelta" => p.angle_delta = range3(text).ok_or_else(|| bad(key))?,
            // These three build channels off `life` as parsed so far.
            "velocity" | "vel" => legacy_velocity(&mut p, range3(text).ok_or_else(|| bad(key))?),
            "acceleration" | "accel" => {
                legacy_acceleration(&mut p, range3(text).ok_or_else(|| bad(key))?)
            }
            "gravity" => p.gravity = range(text).ok_or_else(|| bad(key))?,
            "density" => p.density = range(text).ok_or_else(|| bad(key))?,
            "variance" => p.variance = range(text).ok_or_else(|| bad(key))?,
            "origin" => p.origin = range3(text).ok_or_else(|| bad(key))?,
            "origin2" => p.origin2 = range3(text).ok_or_else(|| bad(key))?,
            "radius" => p.radius = range(text).ok_or_else(|| bad(key))?,
            "height" => p.height = range(text).ok_or_else(|| bad(key))?,
            "wind" => p.wind = range(text).ok_or_else(|| bad(key))?,
            "rotation" => p.rotation = range(text).ok_or_else(|| bad(key))?,
            "rotationdelta" => legacy_rotation_delta(&mut p, range(text).ok_or_else(|| bad(key))?),
            "flags" | "flag" => parse_attr_flags(&mut p, text)?,
            "spawnflags" | "spawnflag" => parse_spawn_flags(&mut p, text)?,
            "nonuniformscale" => p.non_uniform_scale = int(text) != 0,
            "uselength" => p.use_length = int(text) != 0,
            "name" => p.name = text.chars().take(31).collect(),
            "shaderimpact" => p.shader_impact = text.chars().take(31).collect(),
            "sequencestartframemode" => {
                let m = int(text);
                if !(0..=2).contains(&m) {
                    return Err(bad(key));
                }
                p.sequence_start_frame_mode = m as u32;
            }
            "sequencefixedframevalue" => {
                p.sequence_fixed_frame = int(text);
                if p.sequence_fixed_frame < 1 {
                    return Err(bad(key));
                }
            }
            "sequenceplayratemode" => {
                let m = int(text);
                if !(0..=1).contains(&m) {
                    return Err(bad(key));
                }
                p.sequence_play_rate_mode = m as u32;
            }
            "sequencefixedfpsvalue" => {
                p.sequence_fixed_fps = floats(text).first().copied().unwrap_or(0.0);
                if p.sequence_fixed_fps < 0.0 {
                    return Err(bad(key));
                }
            }
            "sequenceloopmode" => {
                let m = int(text);
                if !(0..=1).contains(&m) {
                    return Err(bad(key));
                }
                p.sequence_loop_mode = m as u32;
            }
            "sequencelooptimes" => {
                p.sequence_loop_times = int(text);
                if p.sequence_loop_times < 0 {
                    return Err(bad(key));
                }
            }
            "spawnfrustumcullradius" => {
                let r = floats(text).first().copied().unwrap_or(0.0);
                if r < 0.0 {
                    return Err(bad(key));
                }
                p.spawn_frustum_cull_radius = r;
            }
            _ => return Err(format!("unknown key '{key}'")),
        }
    }
    if p.min.iter().zip(&p.max).any(|(a, b)| a > b) {
        return Err("bounding box mins / maxs invalid".into());
    }
    let (w, d, h) = (
        p.max[0] - p.min[0],
        p.max[1] - p.min[1],
        p.max[2] - p.min[2],
    );
    if h < w || h < d {
        return Err("bounding box width or depth larger than height".into());
    }
    for sub in &g.groups {
        let c = Channel::from_group(&sub.name)
            .ok_or_else(|| format!("unknown group key '{}'", sub.name))?;
        parse_channel(&mut p, &mut legacy[c as usize], sub, c)?;
    }
    for (i, l) in legacy.iter().enumerate() {
        if l.present {
            let c = channel_at(i);
            p.channels[i] = migrate(l, c, p.life);
        }
    }
    Ok(p)
}

fn channel_at(i: usize) -> Channel {
    use Channel::*;
    const ALL: [Channel; CHANNELS] = [
        Rgb,
        RgbRand,
        Alpha,
        AlphaRand,
        Size,
        SizeRand,
        Size2,
        Size2Rand,
        Length,
        LengthRand,
        RotationDelta,
        RotationDeltaRand,
        VelocityX,
        VelocityY,
        VelocityZ,
        VelocityXRand,
        VelocityYRand,
        VelocityZRand,
        Velocity2X,
        Velocity2Y,
        Velocity2Z,
        Velocity2XRand,
        Velocity2YRand,
        Velocity2ZRand,
    ];
    ALL[i]
}

/// `FX_ParseEffect` (Mac 0x5d218): every top-level group a known kind
/// names becomes a primitive, others are skipped. One that fails, or a
/// Particle, Tail or OrientedParticle with no material, fails the file.
pub fn parse(text: &[u8]) -> Result<Effect, ParseError> {
    let tree = parse_tree(text)?;
    let mut primitives = Vec::new();
    for (index, g) in tree.groups.iter().enumerate() {
        let Some(kind) = Kind::from_name(&g.name) else {
            continue;
        };
        let p = parse_primitive(kind, index, g)
            .map_err(|e| ParseError(format!("{} block {index}: {e}", g.name)))?;
        if kind.needs_material() && p.materials.is_empty() {
            return err(format!("{} block {index}: no materials", g.name));
        }
        if primitives.len() < MAX_PRIMITIVES {
            primitives.push(p);
        }
    }
    Ok(Effect { primitives })
}

/// The effect `name` names ([`effect_key`]), its `.efx` parsed; an error
/// carries the engine's reason for refusing it. Built through
/// [`Asset::find`], uncached: the client's fx system keeps its own cache.
impl Asset for Effect {
    const KIND: AssetKind = AssetKind::Cod2Effect;

    fn find(src: &dyn AssetSource, name: &str) -> anyhow::Result<Self> {
        let key = effect_key(name).ok_or_else(|| anyhow!("'{name}' is not under fx/"))?;
        let bytes = src
            .read(&format!("{key}.efx"))
            .ok_or_else(|| anyhow!("not found"))?;
        parse(&bytes).map_err(|e| anyhow!("{e}"))
    }
}

/// The name `FX_RegisterEffect` (Mac 0x5d5fa) keys an effect by: one
/// leading slash dropped, the extension stripped, lowercased; `None`
/// without the `fx/` prefix, which the engine refuses.
pub fn effect_key(name: &str) -> Option<String> {
    let n = name
        .strip_prefix(['/', '\\'])
        .unwrap_or(name)
        .replace('\\', "/");
    if !n.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("fx/")) {
        return None;
    }
    let stem = match n.rfind('.') {
        Some(dot) if !n[dot..].contains('/') => &n[..dot],
        _ => &n[..],
    };
    Some(stem.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// Three keys: 0 -> 10 over the first quarter, then down to 2.
    fn three() -> Curve {
        Curve::new(1, &[0.0, 0.0, 0.25, 10.0, 1.0, 2.0])
    }

    #[test]
    fn samples_are_linear_between_keys() {
        let c = three();
        assert!(close(c.sample1(0.0), 0.0));
        assert!(close(c.sample1(0.125), 5.0));
        assert!(close(c.sample1(0.25), 10.0));
        // 10 + (2 - 10) * (0.625 - 0.25) / 0.75 = 6
        assert!(close(c.sample1(0.625), 6.0));
        assert!(close(c.sample1(1.0), 2.0));
        // Past the ends the end keys hold.
        assert!(close(c.sample1(-1.0), 0.0));
        assert!(close(c.sample1(2.0), 2.0));
    }

    #[test]
    fn colour_curves_sample_every_component() {
        let c = Curve::new(3, &[0.0, 1.0, 0.5, 0.0, 1.0, 0.0, 0.5, 1.0]);
        let v = c.sample(0.25);
        assert!(close(v[0], 0.75) && close(v[1], 0.5) && close(v[2], 0.25));
    }

    #[test]
    fn integral_is_the_area_under_the_line() {
        let c = three();
        // First segment: triangle 0.25 * 10 / 2 = 1.25.
        assert!(close(c.integrate(0.25), 1.25));
        // Half of it: 0.125 * 5 / 2.
        assert!(close(c.integrate(0.125), 0.3125));
        // Whole: 1.25 + 0.75 * (10 + 2) / 2 = 5.75.
        assert!(close(c.integrate(1.0), 5.75));
        // A flat 3 integrates to 3 t.
        let flat = Curve::line(1, 3.0, 3.0);
        assert!(close(flat.integrate(0.4), 1.2));
    }

    #[test]
    fn missing_end_keys_are_padded() {
        let c = Curve::new(1, &[0.5, 4.0]);
        assert_eq!(c.keys, vec![0.0, 4.0, 0.5, 4.0, 1.0, 4.0]);
        assert!(close(c.sample1(0.7), 4.0));
    }

    #[test]
    fn high_bytes_are_blank_to_the_lexer() {
        // fx/muzzleflashes/panzerschreck_flash.efx closes a block with
        // `}` and 0xf6.
        let text = b"Particle\r\n{\r\n\tname a\r\n\tshaders\r\n\t[\r\n\t\tgfx_a\r\n\t]\r\n}\xf6\r\nTail\r\n{\r\n\tshaders\r\n\t[\r\n\t\tgfx_b\r\n\t]\r\n}\r\n";
        let e = parse(text).unwrap();
        assert_eq!(e.primitives.len(), 2);
        assert_eq!(e.primitives[1].kind, Kind::Tail);
    }

    #[test]
    fn a_curve_block_and_its_scale() {
        let text = b"Particle\n{\n\tflags useAlpha useRandomSize\n\tcount 2 3\n\tshaders\n\t[\n\t\tgfx_x\n\t]\n\tsize\n\t{\n\t\tcurve\n\t\t[\n\t\t\t0 0.5 0 0\n\t\t\t1 1 0 0\n\t\t]\n\t\tscale 8 16\n\t}\n}\n";
        let e = parse(text).unwrap();
        let p = &e.primitives[0];
        assert_eq!(p.count, (2.0, 3.0));
        assert!(p.has(attr::USE_ALPHA | attr::USE_RANDOM_SIZE));
        let s = p.channel(Channel::Size);
        assert_eq!(s.scale, (8.0, 16.0));
        assert!(close(s.curve.sample1(0.5), 0.75));
        // An untouched channel keeps Init's default: alpha 1 down to 0.
        assert!(close(p.channel(Channel::Alpha).curve.sample1(0.5), 0.5));
    }

    #[test]
    fn unknown_keys_and_flags_refuse_the_file() {
        let bad_key = b"Particle\n{\n\tshaders\n\t[\n\t\tx\n\t]\n\tfoo 1\n}\n";
        assert!(parse(bad_key).is_err());
        // fx/impacts/wall_impact_small_dirt.efx's spawn flag.
        let bad_flag =
            b"Particle\n{\n\tshaders\n\t[\n\t\tx\n\t]\n\tspawnFlags rgbComponentInterpolation\n}\n";
        assert!(parse(bad_flag).is_err());
        let no_material = b"Tail\n{\n\tlife 10\n}\n";
        assert!(parse(no_material).is_err());
        // An unknown block is skipped.
        assert!(parse(b"Sound\n{\n\tsounds\n\t[\n\t\ta\n\t]\n}\n").is_ok());
    }

    #[test]
    fn cod1_channels_migrate() {
        let text = b"Light\n{\n\tlife 1000\n\tsize\n\t{\n\t\tstart 30\n\t\tend 10\n\t\tflags linear\n\t}\n\talpha\n\t{\n\t\tstart 0.8\n\t}\n}\n";
        let p = &parse(text).unwrap().primitives[0];
        let s = p.channel(Channel::Size);
        // Normalised by 30 * 4/3 = 40 into the scale.
        assert_eq!(s.scale, (40.0, 40.0));
        assert!(close(s.curve.sample1(0.0) * 40.0, 30.0));
        assert!(close(s.curve.sample1(0.5) * 40.0, 20.0));
        // No flag holds the start; alpha is not normalised.
        let a = p.channel(Channel::Alpha);
        assert!(close(a.curve.sample1(0.7) * a.scale.0, 0.8));
    }

    #[test]
    fn cod1_velocity_is_flat_curves_scaled() {
        let text = b"Emitter\n{\n\tvelocity 0 0 100 0 0 300\n}\n";
        let p = &parse(text).unwrap().primitives[0];
        let z = p.channel(Channel::VelocityZ);
        let zr = p.channel(Channel::VelocityZRand);
        assert!(close(z.curve.sample1(0.3) * z.scale.0, 100.0));
        assert!(close(zr.curve.sample1(0.3) * zr.scale.0, 300.0));
        assert!(p.has(attr::USE_RANDOM_VELOCITY));
    }

    #[test]
    fn effect_keys_fold_like_registration() {
        assert_eq!(
            effect_key("/fx/Impacts/default_hit").as_deref(),
            Some("fx/impacts/default_hit")
        );
        assert_eq!(
            effect_key("fx/impacts/default_hit.efx").as_deref(),
            Some("fx/impacts/default_hit")
        );
        assert_eq!(effect_key("impacts/x.efx"), None);
    }
}
