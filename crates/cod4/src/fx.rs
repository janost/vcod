//! CoD4 1.7's effects as a player reads them: an [`crate::assets::fx::Fx`]
//! element's raw `FxElemDef` (0xFC) and its velocity and visual samples
//! decoded into [`ElemDef`] (docs/research/cod4-effects.md).

use crate::assets::Assets;
use crate::assets::fx::{Fx, FxElem, FxVisual};

/// `FxElemDef.elemType` (cod4-effects.md 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElemType {
    BillboardSprite,
    OrientedSprite,
    Tail,
    Trail,
    Cloud,
    Model,
    OmniLight,
    SpotLight,
    Sound,
    Decal,
    Runner,
}

impl ElemType {
    pub fn from_u8(t: u8) -> Option<ElemType> {
        use ElemType::*;
        Some(match t {
            0 => BillboardSprite,
            1 => OrientedSprite,
            2 => Tail,
            3 => Trail,
            4 => Cloud,
            5 => Model,
            6 => OmniLight,
            7 => SpotLight,
            8 => Sound,
            9 => Decal,
            10 => Runner,
            _ => return None,
        })
    }
}

/// `FxElemDef.flags` bits (cod4-effects.md 2).
pub mod flags {
    /// `spawnOrigin` is in the effect's axes; clear, in the world's
    /// (`FX_GetSpawnOrigin`, iw3mp.exe 0x4a5cb0, cod4-effects.md 2).
    pub const SPAWN_RELATIVE_TO_EFFECT: u32 = 0x2;
    /// Culled against the view frustum at spawn by `spawnFrustumCullRadius`.
    pub const SPAWN_FRUSTUM_CULL: u32 = 0x4;
    /// Bits 4-5: the spawn offset, none, sphere (`0x10`) or cylinder (`0x20`).
    pub const SPAWN_OFFSET_MASK: u32 = 0x30;
    pub const SPAWN_OFFSET_SPHERE: u32 = 0x10;
    pub const SPAWN_OFFSET_CYLINDER: u32 = 0x20;
    /// Bits 6-7: what the element moves with: the world (0), its spawn
    /// (`0x40`), the effect (`0x80`), the effect's offset (`0xc0`).
    pub const RUN_MASK: u32 = 0xc0;
    pub const RUN_RELATIVE_TO_EFFECT: u32 = 0x80;
    pub const USE_COLLISION: u32 = 0x100;
    pub const DIE_ON_TOUCH: u32 = 0x200;
    /// The local and world velocity graphs carry motion.
    pub const HAS_VELOCITY_LOCAL: u32 = 0x100_0000;
    pub const HAS_VELOCITY_WORLD: u32 = 0x200_0000;
    pub const HAS_GRAVITY: u32 = 0x400_0000;
    /// `size[1]` is its own; otherwise it reads `size[0]`.
    pub const NONUNIFORM_SCALE: u32 = 0x1000_0000;
}

/// `FxFloatRange`: `base + amplitude * random`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub base: f32,
    pub amplitude: f32,
}

impl Range {
    /// The ends, `(base, base + amplitude)`.
    pub fn ends(self) -> (f32, f32) {
        (self.base, self.base + self.amplitude)
    }
}

/// One `FxElemVelStateInFrame` (0x30): velocity and the displacement
/// summed to this sample, each per axis, in units per millisecond of the
/// element's life divided by the interval count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VelState {
    pub velocity: [Range; 3],
    pub total_delta: [Range; 3],
}

/// One `FxElemVelStateSample` (0x60): the local frame's, then the world's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VelSample {
    pub local: VelState,
    pub world: VelState,
}

/// `FxElemVisualState` (0x18).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisState {
    pub color: [u8; 4],
    /// Radians per millisecond of life over the interval count.
    pub rotation_delta: f32,
    pub rotation_total: f32,
    pub size: [f32; 2],
    pub scale: f32,
}

/// `FxElemVisStateSample` (0x30): `base + amplitude * random`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisSample {
    pub base: VisState,
    pub amplitude: VisState,
}

/// `FxElemAtlas` (8).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Atlas {
    /// Bits 0-1: start at `index` (0), a random frame (1), the spawn's
    /// sequence (2); bit 2: the whole atlas over the life; bit 3: loop
    /// `loop_count` times only.
    pub behavior: u8,
    pub index: u8,
    pub fps: u8,
    pub loop_count: u8,
    pub col_bits: u8,
    pub row_bits: u8,
    pub entry_count: i16,
}

/// One element, decoded.
#[derive(Clone, Debug)]
pub struct ElemDef {
    pub elem_type: Option<ElemType>,
    pub flags: u32,
    /// Looping: `(intervalMsec, count)`; one-shot: the count's
    /// `(base, amplitude)`.
    pub spawn: (i32, i32),
    pub spawn_range: Range,
    pub fade_in_range: Range,
    pub fade_out_range: Range,
    pub spawn_frustum_cull_radius: f32,
    /// `(base, amplitude)` ms.
    pub spawn_delay_msec: (i32, i32),
    pub life_span_msec: (i32, i32),
    pub spawn_origin: [Range; 3],
    pub spawn_offset_radius: Range,
    pub spawn_offset_height: Range,
    /// Radians.
    pub spawn_angles: [Range; 3],
    /// Radians per millisecond.
    pub angular_velocity: [Range; 3],
    /// Radians.
    pub initial_rotation: Range,
    /// Times 800 units per second squared, down.
    pub gravity: Range,
    pub reflection_factor: Range,
    pub atlas: Atlas,
    pub vel: Vec<VelSample>,
    pub vis: Vec<VisSample>,
    pub coll_mins: [f32; 3],
    pub coll_maxs: [f32; 3],
    pub emit_dist: Range,
    pub emit_dist_variance: Range,
    pub sort_order: u8,
    pub lighting_frac: u8,
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn range(b: &[u8], o: usize) -> Range {
    Range {
        base: f32_at(b, o),
        amplitude: f32_at(b, o + 4),
    }
}

fn ranges3(b: &[u8], o: usize) -> [Range; 3] {
    std::array::from_fn(|i| range(b, o + 8 * i))
}

/// An `FxElemVec3Range` (0x18): three bases, then three amplitudes.
fn vec3_range(b: &[u8], o: usize) -> [Range; 3] {
    std::array::from_fn(|i| Range {
        base: f32_at(b, o + 4 * i),
        amplitude: f32_at(b, o + 12 + 4 * i),
    })
}

fn vel_state(b: &[u8], o: usize) -> VelState {
    VelState {
        velocity: vec3_range(b, o),
        total_delta: vec3_range(b, o + 0x18),
    }
}

fn vis_state(b: &[u8], o: usize) -> VisState {
    VisState {
        color: b[o..o + 4].try_into().unwrap(),
        rotation_delta: f32_at(b, o + 4),
        rotation_total: f32_at(b, o + 8),
        size: [f32_at(b, o + 12), f32_at(b, o + 16)],
        scale: f32_at(b, o + 20),
    }
}

impl ElemDef {
    /// Element `e` of an effect, its samples read when the walk kept them.
    pub fn of(e: &FxElem) -> ElemDef {
        let b = &e.raw;
        let vel = e
            .vel_samples
            .as_deref()
            .map(|v| {
                v.as_chunks::<0x60>()
                    .0
                    .iter()
                    .map(|c| VelSample {
                        local: vel_state(c, 0),
                        world: vel_state(c, 0x30),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let vis = e
            .vis_samples
            .as_deref()
            .map(|v| {
                v.as_chunks::<0x30>()
                    .0
                    .iter()
                    .map(|c| VisSample {
                        base: vis_state(c, 0),
                        amplitude: vis_state(c, 0x18),
                    })
                    .collect()
            })
            .unwrap_or_default();
        ElemDef {
            elem_type: ElemType::from_u8(e.elem_type),
            flags: i32_at(b, 0) as u32,
            spawn: (i32_at(b, 4), i32_at(b, 8)),
            spawn_range: range(b, 12),
            fade_in_range: range(b, 20),
            fade_out_range: range(b, 28),
            spawn_frustum_cull_radius: f32_at(b, 36),
            spawn_delay_msec: (i32_at(b, 40), i32_at(b, 44)),
            life_span_msec: (i32_at(b, 48), i32_at(b, 52)),
            spawn_origin: ranges3(b, 56),
            spawn_offset_radius: range(b, 80),
            spawn_offset_height: range(b, 88),
            spawn_angles: ranges3(b, 96),
            angular_velocity: ranges3(b, 120),
            initial_rotation: range(b, 144),
            gravity: range(b, 152),
            reflection_factor: range(b, 160),
            atlas: Atlas {
                behavior: b[168],
                index: b[169],
                fps: b[170],
                loop_count: b[171],
                col_bits: b[172],
                row_bits: b[173],
                entry_count: i16::from_le_bytes([b[174], b[175]]),
            },
            vel,
            vis,
            coll_mins: [f32_at(b, 192), f32_at(b, 196), f32_at(b, 200)],
            coll_maxs: [f32_at(b, 204), f32_at(b, 208), f32_at(b, 212)],
            emit_dist: range(b, 228),
            emit_dist_variance: range(b, 236),
            sort_order: b[248],
            lighting_frac: b[249],
        }
    }

    /// The velocity graph's interval count (`velIntervalCount`).
    pub fn vel_intervals(&self) -> usize {
        self.vel.len().saturating_sub(1)
    }

    /// The visual graph's interval count (`visStateIntervalCount`).
    pub fn vis_intervals(&self) -> usize {
        self.vis.len().saturating_sub(1)
    }
}

/// An effect's elements by role: `FxEffectDef` lists the looping ones
/// first, then the one-shot, then those only another element emits.
pub fn roles(fx: &Fx) -> impl Iterator<Item = (Role, &FxElem)> {
    let looping = fx.looping.max(0) as usize;
    let one_shot = fx.one_shot.max(0) as usize;
    fx.elems.iter().enumerate().map(move |(i, e)| {
        let role = if i < looping {
            Role::Looping
        } else if i < looping + one_shot {
            Role::OneShot
        } else {
            Role::Emitted
        };
        (role, e)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Looping,
    OneShot,
    Emitted,
}

/// The material names an element draws (one per visual).
pub fn materials(a: &Assets, e: &FxElem) -> Vec<String> {
    e.visuals
        .iter()
        .filter_map(|v| match v {
            FxVisual::Material(Some(id)) | FxVisual::Mark([Some(id), _]) => {
                Some(crate::map::stub_name(&a.materials[id.index()].name).to_string())
            }
            _ => None,
        })
        .collect()
}

/// The map's createfx ambience as a 1.7 client reads it at load
/// (`clientSideEffects`, cod4-effects.md 11): `maps/mp/<map>_fx.gsc`'s
/// `level._effect[...] = loadfx(...)` lines name the effects, and
/// `maps/createfx/<map>_fx.gsc` places the one-shot effects and loop
/// sounds.
pub mod createfx {
    /// `CG_AddPairToMap`'s limit (`MAX_CLIENT_EFFECT_DEFS`).
    pub const MAX_EFFECT_DEFS: usize = 32;
    /// `MAX_CLIENT_ENT_SOUNDS`.
    pub const MAX_SOUNDS: usize = 128;
    /// The sound entity number of client sound `i` is `1024 + i`.
    pub const SOUND_ENTITY_BASE: u32 = 1024;

    /// One `createOneshotEffect` entry.
    #[derive(Clone, Debug, PartialEq)]
    pub struct OneShot {
        pub origin: [f32; 3],
        /// Degrees, through `AnglesToAxis`.
        pub angles: [f32; 3],
        /// The `level._effect` name.
        pub fxid: String,
        /// Seconds; the effect starts at `delay * 1000` ms of the client's
        /// clock, so a negative delay starts it long settled.
        pub delay: f32,
    }

    /// What the createfx file places: the effects in file order, and the
    /// sounds (`createLoopSound`'s and those a one-shot entry names).
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Placed {
        pub effects: Vec<OneShot>,
        pub sounds: Vec<([f32; 3], String)>,
        /// Why the parse stopped early, as retail's `Com_PrintError` would
        /// say; what was read before stays.
        pub error: Option<String>,
    }

    /// The rawfile's text, up to its terminating NUL.
    pub fn text(data: &[u8]) -> String {
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        String::from_utf8_lossy(&data[..end]).into_owned()
    }

    /// `(name, file)` per `level._effect[ "name" ] = loadfx( "file" )`
    /// line, the first mapping of a name kept, at most
    /// [`MAX_EFFECT_DEFS`]. Any other line is skipped.
    pub fn effect_map(text: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for line in text.lines().map(str::trim) {
            let Some(rest) = line.strip_prefix("level._effect[") else {
                continue;
            };
            let Some((name, rest)) = quoted(rest.trim_start()) else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix(']').map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('=').map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix("loadfx").map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('(').map(str::trim_start) else {
                continue;
            };
            let Some((file, _)) = quoted(rest) else {
                continue;
            };
            if out.iter().any(|(n, _)| *n == name) || out.len() == MAX_EFFECT_DEFS {
                continue;
            }
            out.push((name, file));
        }
        out
    }

    /// A `"..."` string at the start of `s`, and what follows it.
    fn quoted(s: &str) -> Option<(String, &str)> {
        let s = s.strip_prefix('"')?;
        let end = s.find('"')?;
        Some((s[..end].to_string(), &s[end + 1..]))
    }

    /// `( x, y, z );` after its key.
    fn vec3(s: &str) -> Option<[f32; 3]> {
        let s = s.trim_start().strip_prefix('(')?;
        let s = &s[..s.find(')')?];
        let mut it = s.split(',').map(|v| v.trim().parse::<f32>().ok());
        let v = [it.next()??, it.next()??, it.next()??];
        Some(v)
    }

    const ONESHOT: &str = "ent = maps\\mp\\_utility::createOneshotEffect( ";
    const LOOPSOUND: &str = "ent = maps\\mp\\_createfx::createLoopSound();";

    /// The createfx file's entries, read as `CG_ParseClientEffects` reads
    /// them: the generated header, then each entry's keys in their fixed
    /// order (origin, angles, fxid, delay and an optional soundalias for an
    /// effect; origin, angles, soundalias for a sound). The first line out
    /// of that order ends the parse.
    pub fn placed(text: &str) -> Placed {
        let mut out = Placed::default();
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .peekable();
        for want in ["//_createfx generated. Do not touch!!", "main()", "{"] {
            if lines.next() != Some(want) {
                out.error = Some(format!("expected '{want}'"));
                return out;
            }
        }
        if lines.peek().is_some_and(|l| l.starts_with("//")) {
            lines.next();
        }
        // `ent.v[ "key" ] = value;`, the value's text.
        let key = |line: Option<&str>, k: &str| -> Option<String> {
            let rest = line?.strip_prefix(&format!("ent.v[ \"{k}\" ] = "))?;
            Some(rest.trim_end_matches(';').trim().to_string())
        };
        let string = |v: Option<String>| -> Option<String> { Some(quoted(&v?)?.0) };
        while let Some(line) = lines.next() {
            if line.starts_with('}') {
                return out;
            }
            if line.starts_with(LOOPSOUND) {
                let origin = key(lines.next(), "origin").and_then(|v| vec3(&v));
                let angles = key(lines.next(), "angles").and_then(|v| vec3(&v));
                let alias = string(key(lines.next(), "soundalias"));
                match (origin, angles, alias) {
                    (Some(o), Some(_), Some(a)) => push_sound(&mut out, o, a),
                    _ => {
                        out.error = Some(format!("a bad loop sound after {line:?}"));
                        return out;
                    }
                }
            } else if line.starts_with(ONESHOT) {
                let origin = key(lines.next(), "origin").and_then(|v| vec3(&v));
                let angles = key(lines.next(), "angles").and_then(|v| vec3(&v));
                let fxid = string(key(lines.next(), "fxid"));
                let delay = key(lines.next(), "delay").and_then(|v| v.parse::<f32>().ok());
                let (Some(origin), Some(angles), Some(fxid), Some(delay)) =
                    (origin, angles, fxid, delay)
                else {
                    out.error = Some(format!("a bad effect after {line:?}"));
                    return out;
                };
                if lines
                    .peek()
                    .is_some_and(|l| l.starts_with("ent.v[ \"soundalias\" ] = "))
                {
                    match string(key(lines.next(), "soundalias")) {
                        Some(a) => push_sound(&mut out, origin, a),
                        None => {
                            out.error = Some(format!("a bad soundalias after {line:?}"));
                            return out;
                        }
                    }
                }
                out.effects.push(OneShot {
                    origin,
                    angles,
                    fxid,
                    delay,
                });
            } else {
                out.error = Some(format!("unexpected {line:?}"));
                return out;
            }
        }
        out
    }

    fn push_sound(out: &mut Placed, origin: [f32; 3], alias: String) {
        if out.sounds.len() < MAX_SOUNDS {
            out.sounds.push((origin, alias));
        }
    }
}
