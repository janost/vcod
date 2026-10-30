//! Particle simulation: instantiates a parsed `.efx` [`efx::Effect`],
//! steps it, and flattens it into [`FxQuad`]/[`FxLight`] data. No wgpu, no
//! wire knowledge. Engine semantics: `docs/research/efx-grammar.md`, R1 to R10.

use crate::fx::efx::{self, Effect, Emitter, R1, R3};
use crate::fx::{cod2, cod2_cloud, cod2_tracer, cod4_trail};
use crate::marks::MarkRequest;
use glam::Vec3;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use vcod_common::asset_source::{Asset, AssetSource};
use vcod_common::collision::{CONTENTS_SOLID, CollisionWorld};
use vcod_common::pmove::aim::angles_to_axis_msvc;

pub const MAX_PARTICLES: usize = 2048;
/// How many effects deep a chain of `playfx`, `emitfx`, `impactfx` and
/// `deathfx` may run. Retail has no guard but its 1800-slot pool; the
/// stock chains go 5 deep at most, and `fx/explosions/moltov_hit.efx`
/// replays itself on impact (grammar doc R10).
pub const MAX_CHAIN_DEPTH: u8 = 8;
/// `CEmitter::Update` walks its path in 12 ms steps (CoDMP.exe 0x48fd10).
const EMIT_STEP_MS: i64 = 12;
/// Fx lights the prop vertex shader takes, nearest the camera (`vs_prop`).
pub const MAX_LIGHTS: usize = 8;
/// `RE_AddLightToScene`'s queue (0x4e9b00): later lights in a frame drop.
pub const MAX_SCENE_LIGHTS: usize = 32;

// Bullet tracers are hardcoded in the retail client, not an `.efx`:
// CG_Tracer @ cgame_mp_x86.dll 0x30039590, CG_DrawTracer @ 0x300390c0.
// Cvar table and segment setup: grammar doc R8. Addresses below are cgame VAs.

/// `cg_tracerchance` default, cvar row @ 0x30074e54.
const TRACER_CHANCE: f32 = 0.4;
/// `cg_tracerwidth` default, cvar row @ 0x30074e64. A half-width.
const TRACER_WIDTH: f32 = 0.8;
/// `cg_tracerSpeed` default, cvar row @ 0x30074e74. Units/sec.
const TRACER_SPEED: f32 = 4500.0;
/// `cg_tracerlength` default, cvar row @ 0x30074e84. The full streak length.
const TRACER_LENGTH: f32 = 160.0;
/// United Offensive's `cg_tracerchancelmg`, `cg_tracerlengthlmg` and
/// `cg_tracerwidthlmg` defaults (uo cgame vmCvars 0x30455900, 0x304506e0,
/// 0x304407c0): an lmg, hmg or umg round's tracer, its local entity flagged
/// 0x20 (0x300482f8, read at 0x3002ab06).
const TRACER_CHANCE_HEAVY: f32 = 0.7;
const TRACER_LENGTH_HEAVY: f32 = 220.0;
const TRACER_WIDTH_HEAVY: f32 = 1.6;
/// United Offensive's `cg_tracernightscale` default (vmCvar 0x3048c380):
/// the draw (0x30048460) widens every tracer by it while `sv_night` is set
/// (0x3004853d).
const TRACER_NIGHT_SCALE: f32 = 5.0;
/// Flesh branch only: shorter shots draw nothing (`ds:0x30069524` at 0x30039370).
const TRACER_FLESH_MIN_DIST: f32 = 100.0;
/// Flesh branch only: the segment's near end is drawn from
/// `[50, dist - 10)` (`ds:0x3006958c`, `ds:0x30069590` at 0x300393a0).
const TRACER_FLESH_NEAR: f32 = 50.0;
const TRACER_FLESH_SPREAD_CUT: f32 = 60.0;
/// Registered in CG_RegisterGraphics @ 0x30020da0.
pub(crate) const TRACER_SHADER: &str = "gfx/misc/tracer";

/// Where an effect is planted; decides emitter orientation.
#[derive(Clone, Copy, Debug)]
pub enum SpawnAt {
    /// No direction: `EV_PLAY_FX` and an `ET_FX` entity with a zero
    /// `origin2`, which play on [`UP_AXIS`].
    Point {
        pos: Vec3,
    },
    Surface {
        pos: Vec3,
        normal: Vec3,
    },
    Directed {
        pos: Vec3,
        dir: Vec3,
    },
    /// On a tag's own axis, `[forward, left, up]`: syscall 0xe5, what
    /// `EV_PLAY_FX_ON_TAG` plays through (docs/research/efx-grammar.md R10).
    Oriented {
        pos: Vec3,
        axis: [Vec3; 3],
    },
}

/// A tag `playFxOnTag` played on: the entity, and the event's parm, which
/// names the tag through configstring `CS_FX_TAGS + slot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TagBolt {
    pub entity: u32,
    pub slot: i32,
}

/// A tag's world pose this frame: origin and `[forward, left, up]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagPose {
    pub origin: Vec3,
    pub axis: [Vec3; 3],
}

/// What a played effect is bolted to (the bolt record of R10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Bolt {
    /// A shooter's muzzle, by [`FxSystem::set_bolts`] key.
    Muzzle(u32),
    Tag(TagBolt),
}

/// Verts in world space, ring order for the [0,1,2, 0,2,3] index pattern.
pub struct FxQuad {
    pub verts: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    pub rgba: [f32; 4],
    pub shader: Arc<str>,
}

pub struct FxLight {
    pub pos: [f32; 3],
    pub radius: f32,
    pub rgb: [f32; 3],
}

/// A drawn `Emitter` debris model (`CEmitter::Draw`, CoDMP.exe 0x48f9c0):
/// the model at `origin` with its axes `[forward, left, up]` scaled by
/// the element's `size` (grammar doc R9).
#[derive(Clone, Debug, PartialEq)]
pub struct FxModel {
    pub model: String,
    pub origin: Vec3,
    pub axis: [Vec3; 3],
}

/// A `Sound` block cue for the audio system.
#[derive(Clone, Debug, PartialEq)]
pub struct FxSound {
    pub alias: String,
    pub pos: Vec3,
    pub delay_s: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    Billboard,
    Decal,
    Stretch,
    Light,
    /// `Emitter`: an optional debris model, physics, no sprite (R9).
    Emitter,
    /// `FxRunner`: starts its `playfx` effects, draws nothing (R10).
    Runner,
    NoOp,
}

/// `OrientedParticle` and `Cylinder` draw as camera-facing sprites. `Line`'s
/// `origin2` is not parsed, so it stretches along velocity like `Tail`.
/// `Sound` is handled by [`spawn_sound_cues`]. An `Emitter` flies its
/// model and plays its `emitfx` along the way.
fn classify(kind: &str) -> Behavior {
    match kind {
        // CoD2's `Cloud` lives as a sprite does and draws through
        // `FxSystem::clouds` (docs/research/cod2-efx.md 5.1).
        "Particle" | "OrientedParticle" | "Cylinder" | "Cloud" => Behavior::Billboard,
        "Decal" => Behavior::Decal,
        "Tail" | "Line" => Behavior::Stretch,
        "Light" => Behavior::Light,
        "Emitter" => Behavior::Emitter,
        "FxRunner" => Behavior::Runner,
        _ => Behavior::NoOp,
    }
}

struct Particle {
    spawn: f32, // absolute seconds, after delay
    life: f32,  // seconds
    /// Bolt-local while `bolt` is set, like `vel` and `accel`.
    pos: Vec3,
    vel: Vec3,
    /// `acceleration` with `gravity` added on z, world space.
    accel: Vec3,
    rot: f32, // degrees
    rot_delta: f32,
    /// Half-extent at spawn and at death (grammar doc R4).
    size0: f32,
    size1: f32,
    /// A sprite's vertical half-extent at spawn and at death: `size2` under
    /// `nonUniformScale`, else `size` (grammar doc R4).
    height0: f32,
    height1: f32,
    len0: f32,
    len1: f32, // 0 = billboard, else stretch backwards along vel
    alpha0: f32,
    alpha1: f32,
    rgb0: Vec3,
    rgb1: Vec3,
    /// `useAlpha`: the alpha curve rides the vertex alpha; without it the
    /// curve scales the vertex rgb and the alpha byte is 255 (CoDMP.exe
    /// 0x48e92c..0x48e9a8).
    use_alpha: bool,
    physics: bool,
    impact_kills: bool,
    /// `bounce`: the share of the reflected velocity a hit keeps.
    elasticity: f32,
    /// Played where the particle hits, until it comes to rest.
    impact_fx: Option<String>,
    /// Played where its life runs out (R10).
    death_fx: Option<String>,
    shader: Arc<str>,
    is_light: bool,
    /// `Some` for an `Emitter` element, which draws no sprite.
    debris: Option<Debris>,
    /// An `Emitter`'s `emitfx` state.
    emit: Option<Emit>,
    /// A CoD4 element's `effectEmitted` state.
    emit4: Option<Emit4>,
    /// How many effects up the chain this particle's effect sits.
    depth: u8,
    /// A `relative` element of an effect played on a tag rides that tag
    /// (R10).
    bolt: Option<Bolt>,
    /// A delayed element of an effect played on a tag, held in the tag's
    /// frame until it falls due and posed there on the tag as it then
    /// stands (0x493f40), with its gravity.
    repose: Option<f32>,
    /// A CoD2 element's channels and spawn frame (`fx::cod2`).
    cod2: Option<Box<cod2::Live>>,
}

/// `CEmitter`'s emit cursor (CoDMP.exe 0x48fcd4, grammar doc R10): the
/// path it walks is its spawn origin and velocity under its current
/// acceleration, not its traced position, so bounces do not move it.
struct Emit {
    fx: String,
    pos: Vec3,
    vel: Vec3,
    last_ms: i64,
    /// Units to the next play.
    spacing: f32,
    density: f32,
    variance: f32,
}

/// A CoD4 element's `effectEmitted` cursor (docs/research/cod4-effects.md
/// 15): the effect plays at points spaced `gap + spread * rand` apart along
/// the element's path, each step's move walked from where the last one
/// ended.
struct Emit4 {
    fx: String,
    /// The spacing's fixed part: the element's `emitDist` draw plus
    /// `emitDistVariance.base`.
    gap: f32,
    /// `emitDistVariance.amplitude`, scaled by a fresh draw each spacing.
    spread: f32,
    /// The element's world position at the last step.
    at: Vec3,
    /// Units travelled since the last play, in 256ths of the widest
    /// spacing: retail keeps the leftover in a byte (`emitResidual`).
    owed: u8,
}

impl Emit4 {
    fn widest(&self) -> f32 {
        self.gap + self.spread
    }
}

/// An effect another one starts: an `FxRunner`'s `playfx`, an `Emitter`'s
/// `emitfx`, a hit's `impactfx`, a death's `deathfx`.
struct Chain {
    due: f32,
    path: String,
    at: SpawnAt,
    depth: u8,
    bolt: Option<Bolt>,
    /// `at` is in the bolt's frame, posed on it when the chain falls due
    /// (0x493f40).
    repose: bool,
}

/// A tag an effect was played on, this frame: its origin and axis. A
/// muzzle's axis is the frame [`tangent_basis`] builds on its forward, as
/// a `Directed` spawn has; a `playFxOnTag` tag's is its own.
#[derive(Clone, Copy)]
struct BoltFrame {
    origin: Vec3,
    axis: (Vec3, Vec3, Vec3),
}

impl BoltFrame {
    fn new((origin, dir): (Vec3, Vec3)) -> BoltFrame {
        BoltFrame {
            origin,
            axis: tangent_basis(dir.normalize_or_zero()),
        }
    }

    fn tag(pose: TagPose) -> BoltFrame {
        let [x, y, z] = pose.axis;
        BoltFrame {
            origin: pose.origin,
            axis: (x, y, z),
        }
    }

    fn rotate(&self, v: Vec3) -> Vec3 {
        let (x, y, z) = self.axis;
        x * v.x + y * v.y + z * v.z
    }

    fn unrotate(&self, v: Vec3) -> Vec3 {
        let (x, y, z) = self.axis;
        Vec3::new(v.dot(x), v.dot(y), v.dot(z))
    }

    /// `at` taken into this frame, or with `back` out of it.
    fn carry(&self, at: SpawnAt, back: bool) -> SpawnAt {
        let pos = |p: Vec3| match back {
            false => self.unrotate(p - self.origin),
            true => self.origin + self.rotate(p),
        };
        let dir = |d: Vec3| match back {
            false => self.unrotate(d),
            true => self.rotate(d),
        };
        match at {
            SpawnAt::Point { pos: p } => SpawnAt::Point { pos: pos(p) },
            SpawnAt::Surface { pos: p, normal } => SpawnAt::Surface {
                pos: pos(p),
                normal: dir(normal),
            },
            SpawnAt::Directed { pos: p, dir: d } => SpawnAt::Directed {
                pos: pos(p),
                dir: dir(d),
            },
            SpawnAt::Oriented { pos: p, axis } => SpawnAt::Oriented {
                pos: pos(p),
                axis: axis.map(dir),
            },
        }
    }
}

/// An `Emitter` element's own state (`CEmitter`, CoDMP.exe 0x4a02c0).
struct Debris {
    model: Option<String>,
    /// Euler angles, degrees.
    angles: Vec3,
    /// Degrees per 100 ms (0x490300 scales by `frametime * 0.01`).
    angle_delta: Vec3,
}

/// xorshift64. Also used by `audio::AudioSystem`.
pub(crate) struct Rng(u64);

impl Rng {
    /// `seed` is forced non-zero; xorshift64 is stuck at 0.
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub(crate) fn range(&mut self, a: f32, b: f32) -> f32 {
        a + self.next_f32() * (b - a)
    }
}

fn sample_r1(rng: &mut Rng, r: R1) -> f32 {
    rng.range(r.a, r.b)
}

fn sample_r3(rng: &mut Rng, r: R3) -> Vec3 {
    Vec3::new(
        rng.range(r.a[0], r.b[0]),
        rng.range(r.a[1], r.b[1]),
        rng.range(r.a[2], r.b[2]),
    )
}

/// Only `linear` animates; an unflagged curve holds `start` (grammar doc R3).
fn curve_animates(flags: &[String]) -> bool {
    flags.iter().any(|f| f == "linear")
}

/// Non-animating curves collapse onto `start`. Both ends are always sampled
/// so the rng stream does not depend on the flags.
fn sample_curve(rng: &mut Rng, c: &efx::Curve) -> (f32, f32) {
    let a = sample_r1(rng, c.start);
    let b = sample_r1(rng, c.end);
    if curve_animates(&c.flags) {
        (a, b)
    } else {
        (a, a)
    }
}

/// [`sample_curve`] for the `rgb` curve.
fn sample_curve_v3(rng: &mut Rng, c: &efx::CurveV3) -> (Vec3, Vec3) {
    let a = sample_r3(rng, c.start);
    let b = sample_r3(rng, c.end);
    if curve_animates(&c.flags) {
        (a, b)
    } else {
        (a, a)
    }
}

fn spawn_pos(at: SpawnAt) -> Vec3 {
    match at {
        SpawnAt::Point { pos }
        | SpawnAt::Surface { pos, .. }
        | SpawnAt::Directed { pos, .. }
        | SpawnAt::Oriented { pos, .. } => pos,
    }
}

/// The effect axis a direction plays on: `MakeNormalVectors` (CoDMP.exe
/// 0x42e1b0), which syscall 0xe4 (0x4a1800) runs on the forward before
/// `PlayEffect` (docs/research/efx-grammar.md R11). The right vector is the
/// forward's components rotated, `(z, -x, y)`, with the forward projected
/// out, and up is right x forward.
fn tangent_basis(axis: Vec3) -> (Vec3, Vec3, Vec3) {
    let r = Vec3::new(axis.z, -axis.x, axis.y);
    let right = (r - axis * r.dot(axis)).normalize_or_zero();
    (axis, right, right.cross(axis))
}

/// Syscall 0xe3's fixed axis (CoDMP.exe 0x4936c0), what an effect played
/// with no direction gets: forward up, then world x and y.
const UP_AXIS: (Vec3, Vec3, Vec3) = (Vec3::Z, Vec3::X, Vec3::Y);

/// Rotate the 2D basis `(x, y)` by `rad` radians in their shared plane.
fn rotate2(x: Vec3, y: Vec3, rad: f32) -> (Vec3, Vec3) {
    let (s, c) = rad.sin_cos();
    (x * c + y * s, y * c - x * s)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// orgOnSphere / orgOnCylinder placement (grammar doc R6); otherwise `base`.
fn distribute_origin(rng: &mut Rng, em: &Emitter, base: Vec3) -> Vec3 {
    use std::f32::consts::TAU;
    if em.spawn_flags.iter().any(|f| f == "orgOnSphere") {
        let radius = sample_r1(rng, em.radius);
        let theta = rng.next_f32() * TAU;
        // Uniform on the unit sphere: cos(phi) uniform in [-1, 1].
        let cos_phi = 1.0 - 2.0 * rng.next_f32();
        let sin_phi = (1.0 - cos_phi * cos_phi).max(0.0).sqrt();
        let dir = Vec3::new(sin_phi * theta.cos(), sin_phi * theta.sin(), cos_phi);
        base + dir * radius
    } else if em.spawn_flags.iter().any(|f| f == "orgOnCylinder") {
        let radius = sample_r1(rng, em.radius);
        let height = sample_r1(rng, em.height);
        let theta = rng.next_f32() * TAU;
        let z = rng.range(-height * 0.5, height * 0.5);
        base + Vec3::new(radius * theta.cos(), radius * theta.sin(), z)
    } else {
        base
    }
}

/// One [`FxSound`] per alias of a `Sound` block; origin and delay are
/// sampled once per block, since it has no `count`.
fn spawn_sound_cues(rng: &mut Rng, em: &Emitter, at: SpawnAt) -> Vec<FxSound> {
    if em.kind != "Sound" || em.sounds.is_empty() {
        return Vec::new();
    }
    let pos = spawn_pos(at) + sample_r3(rng, em.origin);
    let delay_s = sample_r1(rng, em.delay) / 1000.0;
    em.sounds
        .iter()
        .map(|alias| FxSound {
            alias: alias.clone(),
            pos,
            delay_s,
        })
        .collect()
}

/// A `Decal` element waiting for its delay.
struct PendingMark {
    due: f32,
    req: MarkRequest,
}

/// Where one spawn's elements land.
/// A camera's frustum as `FX_SetNextUpdateCamera` builds it: the view's
/// forward plane through the eye, then its four sides, each `(normal,
/// distance)` with the normal inward (cod4-effects.md 18).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    planes: [(Vec3, f32); 5],
}

impl Frustum {
    /// The frustum of an eye at `pos` along `fwd`, `left` and `up`,
    /// `tan_x` and `tan_y` the tangents of its half fovs.
    pub fn new(pos: Vec3, (fwd, left, up): (Vec3, Vec3, Vec3), tan_x: f32, tan_y: f32) -> Frustum {
        let side = |t: f32, axis: Vec3| {
            let c = 1.0 / (t * t + 1.0).sqrt();
            (fwd * (t * c) + axis * c, fwd * (t * c) - axis * c)
        };
        let (l, r) = side(tan_x, left);
        let (u, d) = side(tan_y, up);
        let plane = |n: Vec3| (n, n.dot(pos));
        Frustum {
            planes: [plane(fwd), plane(l), plane(r), plane(u), plane(d)],
        }
    }

    /// `FX_CullSphere` (iw3mp.exe 0x4a6290): the sphere lies wholly behind
    /// one plane.
    pub fn culls(&self, centre: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .any(|(n, d)| n.dot(centre) - d <= -radius)
    }
}

struct SpawnOut<'a> {
    particles: &'a mut VecDeque<Particle>,
    marks: &'a mut Vec<PendingMark>,
    chains: &'a mut Vec<Chain>,
    trails: &'a mut Vec<(Bolt, cod4_trail::Trail)>,
}

/// What one effect spawn shares between its elements.
#[derive(Clone, Copy)]
struct SpawnCx {
    at: SpawnAt,
    now: f32,
    /// The camera, for `cullrange`.
    view: Option<Vec3>,
    /// The camera's frustum, for `frustumCull` (CoD4 only).
    frustum: Option<Frustum>,
    depth: u8,
    /// The tag the effect was played on, and its frame when it is posed.
    bolt: Option<(Bolt, Option<BoltFrame>)>,
}

/// The emitter frame of `at`: forward along its direction or normal.
fn spawn_frame(at: SpawnAt) -> Option<(Vec3, Vec3, Vec3)> {
    match at {
        SpawnAt::Directed { dir, .. } => Some(tangent_basis(dir.normalize_or_zero())),
        SpawnAt::Surface { normal, .. } => Some(tangent_basis(normal.normalize_or_zero())),
        SpawnAt::Point { .. } => Some(UP_AXIS),
        SpawnAt::Oriented {
            axis: [x, y, z], ..
        } => Some((x, y, z)),
    }
}

/// `at` moved to `pos`, its direction or normal kept.
fn spawn_moved(at: SpawnAt, pos: Vec3) -> SpawnAt {
    match at {
        SpawnAt::Point { .. } => SpawnAt::Point { pos },
        SpawnAt::Surface { normal, .. } => SpawnAt::Surface { pos, normal },
        SpawnAt::Directed { dir, .. } => SpawnAt::Directed { pos, dir },
        SpawnAt::Oriented { axis, .. } => SpawnAt::Oriented { pos, axis },
    }
}

/// The sampled `origin` in the emitter frame unless cheapOrgCalc (R6),
/// then spread by orgOnSphere/orgOnCylinder: (centre, spawn point).
fn element_origin(
    rng: &mut Rng,
    em: &Emitter,
    at: SpawnAt,
    frame: Option<(Vec3, Vec3, Vec3)>,
) -> (Vec3, Vec3) {
    let mut offset = sample_r3(rng, em.origin);
    if let Some((x, y, z)) = frame
        && !em.spawn_flags.iter().any(|f| f == "cheapOrgCalc")
    {
        offset = offset.x * x + offset.y * y + offset.z * z;
    }
    let center = spawn_pos(at) + offset;
    (center, distribute_origin(rng, em, center))
}

fn spawn_emitter(rng: &mut Rng, out: &mut SpawnOut, em: &Emitter, cx: &SpawnCx) {
    let at = cx.at;
    let now = cx.now;
    let behavior = classify(&em.kind);
    if behavior == Behavior::NoOp {
        return;
    }
    // A CoD4 trail drops its cross-sections as the effect's tag moves; an
    // effect on no tag never moves, so it draws none.
    if let Some(def) = &em.cod4_trail {
        if let (Some((id, Some(f))), Some(el)) = (cx.bolt, &em.cod2) {
            let shader = pick(rng, &em.shaders).cloned().unwrap_or_default();
            let (x, y, z) = f.axis;
            out.trails.push((
                id,
                cod4_trail::Trail::start(
                    el.clone(),
                    def.clone(),
                    shader,
                    (f.origin, [x, y, z]),
                    now,
                    rng,
                ),
            ));
            if out.trails.len() > MAX_TRAILS {
                out.trails.remove(0);
            }
        }
        return;
    }
    // The effect runner skips an element when the effect is played past
    // its `cullrange` from the view, and never tests it again (CoDMP.exe
    // 0x493a1f, grammar doc R9).
    if em.cullrange > 0.0
        && let Some(view) = cx.view
        && (spawn_pos(at) - view).length() > em.cullrange
    {
        return;
    }
    // A `relative` element rides the tag; with the tag gone it is not
    // created (0x494050's 0x200000 branch, R10).
    let bolt = match cx.bolt {
        Some((id, frame)) if em.flags.iter().any(|f| f == "relative") => match frame {
            Some(frame) => Some((id, frame)),
            None => return,
        },
        _ => None,
    };
    // The tag's frame a delayed element is held in until it falls due.
    let posed = match cx.bolt {
        Some((id, Some(frame))) => Some((id, frame)),
        _ => None,
    };
    // CoD2's `spawnRange` also has a near end (cod2-efx.md 4.1).
    if let Some(el) = &em.cod2
        && el.prim.spawn_range.0 > 0.0
        && let Some(view) = cx.view
        && (spawn_pos(at) - view).length() < el.prim.spawn_range.0
    {
        return;
    }
    // `FX_CullElemForSpawn` (iw3mp.exe 0x4a0b90): a `frustumCull` element
    // spawns only when a sphere of `spawnFrustumCullRadius` round the
    // effect's origin meets the camera's frustum (cod4-effects.md 18).
    if let Some(el) = &em.cod2
        && el.prim.has_spawn(vcod_cod2::efx::spawn::FRUSTUM_CULL)
        && let Some(f) = &cx.frustum
        && f.culls(spawn_pos(at), el.prim.spawn_frustum_cull_radius)
    {
        return;
    }
    // Clamped so a malformed count (1e9) does not spin the loop before
    // push_capped evicts.
    let count = (sample_r1(rng, em.count).round().max(0.0) as usize).min(MAX_PARTICLES);
    // CoD2's `evenDistribution` spreads the delays over the range by the
    // spawn's index instead of drawing them (FxScheduler::PlayEffect).
    let even = em
        .cod2
        .as_ref()
        .filter(|el| el.prim.has_spawn(vcod_cod2::efx::spawn::EVEN_DISTRIBUTION))
        .map(|el| (el.prim.delay.1 - el.prim.delay.0).abs() / count.max(1) as f32);
    for index in 0..count {
        let delay = match even {
            Some(step) => (index as f32 * step).trunc() / 1000.0,
            None => sample_r1(rng, em.delay) / 1000.0,
        };
        let spawn = now + delay;
        if behavior == Behavior::Runner {
            // Case 10 of the spawner: the element's origin and frame, then
            // PlayEffect on one `playfx` entry (0x492780 picks).
            let (_, pos) = element_origin(rng, em, at, spawn_frame(at));
            if let Some(fx) = pick(rng, &em.play_fx) {
                let mut at = spawn_moved(at, pos);
                let tag = posed.filter(|_| spawn > now);
                if let Some((_, f)) = tag {
                    at = f.carry(at, false);
                }
                out.chains.push(Chain {
                    due: spawn,
                    path: efx::chained_path(fx),
                    at,
                    depth: cx.depth + 1,
                    bolt: cx.bolt.map(|(id, _)| id),
                    repose: tag.is_some(),
                });
            }
            continue;
        }
        let life = sample_r1(rng, em.life) / 1000.0;
        if behavior == Behavior::Decal {
            out.marks.push(PendingMark {
                due: spawn,
                req: decal_request(rng, em, at, life),
            });
            continue;
        }
        if life <= 0.0 {
            // An explicit `life 0` would make the age fraction 0/0.
            continue;
        }

        let mut vel = sample_r3(rng, em.velocity);
        let mut accel = sample_r3(rng, em.accel);
        // Emitter-frame vectors rotate onto `dir`/`normal` unless the
        // absoluteVel/absoluteAccel spawnFlags (not flags) keep them
        // world-space (grammar doc R6).
        let frame = spawn_frame(at);
        if let Some((x, y, z)) = frame {
            if !em.spawn_flags.iter().any(|f| f == "absoluteVel") {
                vel = vel.x * x + vel.y * y + vel.z * z;
            }
            if !em.spawn_flags.iter().any(|f| f == "absoluteAccel") {
                accel = accel.x * x + accel.y * y + accel.z * z;
            }
        }

        // evenDistribution is not implemented.
        let (center, base) = element_origin(rng, em, at, frame);

        // axisFromSphere: velocity points radially out from the spawn center,
        // magnitude kept. Without orgOnSphere/orgOnCylinder, base == center.
        if em.spawn_flags.iter().any(|f| f == "axisFromSphere")
            && em
                .spawn_flags
                .iter()
                .any(|f| f == "orgOnSphere" || f == "orgOnCylinder")
        {
            let radial = (base - center).normalize_or_zero();
            if radial != Vec3::ZERO {
                vel = radial * vel.length();
            }
        }

        let mut pos = base;

        // Gravity joins the acceleration on world z (0x494050).
        let gravity = sample_r1(rng, em.gravity);
        accel.z += gravity;
        let rot = sample_r1(rng, em.rotation);
        let rot_delta = sample_r1(rng, em.rotation_delta);
        let (size0, size1) = sample_curve(rng, &em.size);
        // The constructor copies `size` into `size2` without the flag
        // (CoDMP.exe 0x49f360); only a sprite reads it (0x5101d0, 0x5103c0).
        let (height0, height1) = if em.non_uniform_scale {
            sample_curve(rng, &em.size2)
        } else {
            (size0, size1)
        };
        // `length` is a tail-only field (grammar doc R3); sprites carrying
        // one (fx/muzzleflashes/heavy.efx) stay square.
        let (len0, len1) = match sample_curve(rng, &em.length) {
            l if behavior == Behavior::Stretch => l,
            _ => (0.0, 0.0),
        };
        let (alpha0, alpha1) = sample_curve(rng, &em.alpha);
        let (rgb0, rgb1) = sample_curve_v3(rng, &em.rgb);
        // `bounce` and an `impactfx` list each turn physics on (R9). A
        // bolted particle does not trace: vcod integrates it in the tag's
        // frame.
        let physics = bolt.is_none()
            && (em.flags.iter().any(|f| f == "usePhysics")
                || em.bounce.is_some()
                || !em.impact_fx.is_empty());
        let impact_kills = em.flags.iter().any(|f| f == "impactKills");
        let elasticity = em.bounce.map_or(0.0, |b| sample_r1(rng, b));
        let impact_fx = pick(rng, &em.impact_fx).cloned();
        let death_fx = pick(rng, &em.death_fx).map(|f| efx::chained_path(f));
        let shader = pick(rng, &em.shaders).cloned().unwrap_or_default();
        let debris = (behavior == Behavior::Emitter).then(|| {
            // The models list turns `useModel` on (0x49cae0).
            let model = pick(rng, &em.models).cloned();
            let mut angles = sample_r3(rng, em.angle);
            // A bolted model's angles are in the tag's frame, whose forward
            // is the spawn direction.
            if let Some((x, _, _)) = frame.filter(|_| bolt.is_none()) {
                angles += forward_angles(x);
            }
            Debris {
                model,
                angles,
                angle_delta: sample_r3(rng, em.angle_delta),
            }
        });
        // `CEmitter` (0x4a02c0) starts its cursor at its own origin and
        // velocity; the first spacing is drawn like every later one.
        let emit = match pick(rng, &em.emit_fx) {
            Some(fx) if behavior == Behavior::Emitter && em.cod4_emit.is_none() => {
                let density = sample_r1(rng, em.density);
                let variance = sample_r1(rng, em.variance);
                Some(Emit {
                    fx: efx::chained_path(fx),
                    pos,
                    vel,
                    last_ms: (spawn * 1000.0).round() as i64,
                    spacing: emit_spacing(rng, density, variance),
                    density,
                    variance,
                })
            }
            _ => None,
        };
        let mut emit = emit;
        let mut debris = debris;
        let emit4 = em.cod4_emit.and_then(|c| {
            let fx = pick(rng, &em.emit_fx)?;
            Some(Emit4 {
                fx: efx::chained_path(fx),
                gap: sample_r1(rng, c.dist) + c.variance_base,
                spread: c.variance_amp,
                at: pos,
                owed: 0,
            })
        });
        // A CoD2 element moves on its velocity curves from here, on the
        // effect's axis or, under axisFromSphere, the spread's.
        let mut live = em.cod2.as_ref().map(|el| {
            let (fx, fy, fz) = frame.unwrap_or(UP_AXIS);
            let mut axis = [fx, fy, fz];
            if el.prim.has_spawn(vcod_cod2::efx::spawn::AXIS_FROM_SPHERE)
                && let Some(radial) = (base - center).try_normalize()
            {
                let (x, y, z) = tangent_basis(radial);
                axis = [x, y, z];
            }
            let l = cod2::Live::spawn(
                el.clone(),
                rng,
                (pos, axis),
                Vec3::Z,
                life * 1000.0,
                &shader,
                index as u32,
            );
            vel = (l.position(0.001) - l.position(0.0)) / 0.001;
            if let Some(e) = &mut emit {
                e.vel = vel;
            }
            Box::new(l)
        });
        // A delayed non-`relative` element waits in the tag's frame.
        let repose = posed.filter(|_| bolt.is_none() && spawn > now);
        if let Some((_, frame)) = repose {
            if let Some(l) = &mut live {
                l.carry(|p| frame.unrotate(p - frame.origin), |d| frame.unrotate(d));
            }
            pos = frame.unrotate(pos - frame.origin);
            vel = frame.unrotate(vel);
            accel = frame.unrotate(accel - Vec3::Z * gravity);
            if let Some(e) = &mut emit {
                e.pos = pos;
                e.vel = vel;
            }
            if let Some(d) = &mut debris {
                d.angles -= forward_angles(frame.axis.0);
            }
        }
        if let Some((_, frame)) = bolt {
            if let Some(l) = &mut live {
                l.carry(|p| frame.unrotate(p - frame.origin), |d| frame.unrotate(d));
            }
            pos = frame.unrotate(pos - frame.origin);
            vel = frame.unrotate(vel);
            accel = frame.unrotate(accel);
            if let Some(e) = &mut emit {
                e.pos = pos;
                e.vel = vel;
            }
        }

        let p = Particle {
            spawn,
            life,
            pos,
            vel,
            accel,
            rot,
            rot_delta,
            size0,
            size1,
            height0,
            height1,
            len0,
            len1,
            alpha0,
            alpha1,
            rgb0,
            rgb1,
            use_alpha: em.flags.iter().any(|f| f == "useAlpha"),
            physics,
            impact_kills,
            elasticity,
            impact_fx,
            death_fx,
            shader,
            is_light: behavior == Behavior::Light,
            debris,
            emit,
            emit4,
            depth: cx.depth,
            bolt: bolt.or(repose).map(|(id, _)| id),
            repose: repose.map(|_| gravity),
            cod2: live,
        };

        push_capped(out.particles, p, MAX_PARTICLES);
    }
}

/// The next `emitfx` spacing: `density` plus `variance` times a draw in
/// [-1, 1) (0x4902a9; `CEmitter`'s constructor draws the first the same way).
fn emit_spacing(rng: &mut Rng, density: f32, variance: f32) -> f32 {
    density + variance * (2.0 * rng.next_f32() - 1.0)
}

/// What a `Decal` element hands `CG_ImpactMark` (CoDMP.exe 0x496896): a
/// shader from its list, the start `rgb`, `alpha` and `size` (the radius),
/// `rotation`, `life`, along the surface normal, alpha fading. Decals carry
/// no `origin` or `velocity` in the corpus.
fn decal_request(rng: &mut Rng, em: &Emitter, at: SpawnAt, life: f32) -> MarkRequest {
    let dir = match at {
        SpawnAt::Surface { normal, .. } => normal,
        SpawnAt::Directed { dir, .. } => dir,
        SpawnAt::Oriented { axis, .. } => axis[0],
        SpawnAt::Point { .. } => Vec3::Z,
    };
    let orientation = sample_r1(rng, em.rotation);
    let (radius, _) = sample_curve(rng, &em.size);
    let (alpha, _) = sample_curve(rng, &em.alpha);
    let (rgb, _) = sample_curve_v3(rng, &em.rgb);
    let shader = if em.shaders.is_empty() {
        String::new()
    } else {
        let idx = ((rng.next_f32() * em.shaders.len() as f32) as usize).min(em.shaders.len() - 1);
        em.shaders[idx].to_string()
    };
    MarkRequest {
        shader,
        origin: spawn_pos(at),
        dir,
        orientation,
        rgba: [rgb.x, rgb.y, rgb.z, alpha],
        alpha_fade: true,
        radius,
        life_ms: (life * 1000.0).round() as i32,
    }
}

/// One entry at random, as the shader, model and impact lists are drawn.
fn pick<'a, T>(rng: &mut Rng, list: &'a [T]) -> Option<&'a T> {
    if list.is_empty() {
        return None;
    }
    let idx = ((rng.next_f32() * list.len() as f32) as usize).min(list.len() - 1);
    list.get(idx)
}

/// `vectoangles`: pitch (positive down) and yaw of `dir`, roll 0.
fn forward_angles(dir: Vec3) -> Vec3 {
    let yaw = dir.y.atan2(dir.x).to_degrees();
    let pitch = -dir.z.atan2(dir.truncate().length()).to_degrees();
    Vec3::new(pitch, yaw, 0.0)
}

fn push_capped(q: &mut VecDeque<Particle>, p: Particle, cap: usize) {
    q.push_back(p);
    while q.len() > cap {
        q.pop_front();
    }
}

/// Per-frame inputs [`step_pool`] shares between particles.
struct StepCx<'a> {
    dt: f32,
    now: f32,
    world: Option<&'a CollisionWorld>,
    bolts: &'a HashMap<Bolt, BoltFrame>,
}

/// Delayed particles wait, expired ones drop. Effects the step starts
/// (impacts, emits, deaths) land in `chains`.
fn step_pool(pool: &mut VecDeque<Particle>, cx: &StepCx, rng: &mut Rng, chains: &mut Vec<Chain>) {
    let (dt, now) = (cx.dt, cx.now);
    pool.retain_mut(|p| {
        if now < p.spawn {
            return true;
        }
        if let Some(gravity) = p.repose.take() {
            // Posed on the tag as it stands now; a tag that no longer poses
            // spawns nothing (INFERRED, 0x493f40's call of 0x49e8d0).
            let Some(f) = p.bolt.take().and_then(|id| cx.bolts.get(&id).copied()) else {
                return false;
            };
            p.pos = f.origin + f.rotate(p.pos);
            p.vel = f.rotate(p.vel);
            p.accel = f.rotate(p.accel) + Vec3::Z * gravity;
            if let Some(l) = &mut p.cod2 {
                l.carry(|v| f.origin + f.rotate(v), |d| f.rotate(d));
            }
            if let Some(e) = &mut p.emit {
                e.pos = p.pos;
                e.vel = p.vel;
            }
            if let Some(e) = &mut p.emit4 {
                e.at = f.origin + f.rotate(p.pos);
            }
            if let Some(d) = &mut p.debris {
                d.angles += forward_angles(f.axis.0);
            }
        }
        let frame = match p.bolt {
            None => None,
            Some(id) => match cx.bolts.get(&id) {
                Some(&f) => Some(f),
                // The tag is gone: `Update` fails and `Die` runs with the
                // `impactKills` bit still set (0x49f1e0, R10).
                None => {
                    if !p.impact_kills {
                        death_chain(p, None, now, rng, chains);
                    }
                    return false;
                }
            },
        };
        if p.life <= 0.0 || now - p.spawn > p.life {
            death_chain(p, frame, now, rng, chains);
            return false;
        }
        let before = p.pos;
        if !update_origin(p, dt, cx.world, now, chains) {
            return false;
        }
        if let Some(l) = &p.cod2 {
            // Degrees per 100 ms (cod2-efx.md 4.3).
            p.rot_delta = l.rotation_delta(((now - p.spawn) / p.life).clamp(0.0, 1.0)) * 10.0;
        }
        p.rot += p.rot_delta * dt;
        if let Some(d) = &mut p.debris {
            // A resting model's spin dies away (CEmitter::Update, 0x48fa2f).
            if p.pos == before {
                d.angle_delta *= 0.7;
            }
            d.angles += d.angle_delta * (dt * 1000.0 * 0.01);
        }
        if let Some(mut e) = p.emit.take() {
            let now_ms = (now * 1000.0).round() as i64;
            let forward = p.debris.as_ref().map_or(Vec3::X, |d| {
                Vec3::from(angles_to_axis_msvc(d.angles.to_array())[0])
            });
            for pos in emit_along(&mut e, p.accel, now_ms, rng) {
                let (pos, dir) = match frame {
                    Some(f) => (f.origin + f.rotate(pos), f.rotate(forward)),
                    None => (pos, forward),
                };
                chains.push(Chain {
                    due: now,
                    path: e.fx.clone(),
                    at: SpawnAt::Directed { pos, dir },
                    depth: p.depth + 1,
                    bolt: p.bolt,
                    repose: false,
                });
            }
            p.emit = Some(e);
        }
        if let Some(e) = &mut p.emit4 {
            let end = frame.map_or(p.pos, |f| f.origin + f.rotate(p.pos));
            for (pos, dir) in emit4_along(e, end, rng) {
                chains.push(Chain {
                    due: now,
                    path: e.fx.clone(),
                    at: SpawnAt::Directed { pos, dir },
                    depth: p.depth + 1,
                    bolt: None,
                    repose: false,
                });
            }
        }
        true
    });
}

/// Most plays one element's emission makes in one step.
const MAX_EMITS_PER_STEP: usize = 64;

/// Walks `e` to `end` and returns where its effect plays on the way, each
/// with the move's direction. A spacing that ends exactly at `end` plays;
/// one shorter than what was already owed plays at the move's start.
fn emit4_along(e: &mut Emit4, end: Vec3, rng: &mut Rng) -> Vec<(Vec3, Vec3)> {
    let from = std::mem::replace(&mut e.at, end);
    let (dir, len) = (end - from).normalize_and_length();
    let widest = e.widest();
    // No move, or a spacing that could never advance.
    if len == 0.0 || widest <= 0.0 {
        return Vec::new();
    }
    // Distances along the move; the carried units put the walk's origin
    // behind the move's start. vcod caps a step's plays against a
    // near-zero spacing.
    let owed = f32::from(e.owed) / 256.0 * widest;
    let (gap, spread) = (e.gap, e.spread);
    let marks: Vec<f32> = std::iter::successors(Some(-owed), |&d| {
        let next = d + gap + spread * rng.next_f32();
        (next <= len).then_some(next.max(0.0))
    })
    .skip(1)
    .take(MAX_EMITS_PER_STEP)
    .collect();
    let since = len - marks.last().copied().unwrap_or(-owed);
    e.owed = (since / widest * 256.0).round().clamp(0.0, 255.0) as u8;
    marks.into_iter().map(|d| (from + dir * d, dir)).collect()
}

/// `Die` (CoDMP.exe 0x48dda0): a `deathfx` plays at the particle along a
/// random direction, unless an `impactKills` hit ended it. The pool clears
/// that bit before it retires an expired particle (0x49f1e0), so only a
/// hit skips the effect; vcod drops a hit particle in [`update_origin`].
fn death_chain(
    p: &Particle,
    frame: Option<BoltFrame>,
    now: f32,
    rng: &mut Rng,
    chains: &mut Vec<Chain>,
) {
    let Some(fx) = &p.death_fx else {
        return;
    };
    let dir = Vec3::new(
        2.0 * rng.next_f32() - 1.0,
        2.0 * rng.next_f32() - 1.0,
        2.0 * rng.next_f32() - 1.0,
    )
    .normalize_or_zero();
    let pos = frame.map_or(p.pos, |f| f.origin + f.rotate(p.pos));
    chains.push(Chain {
        due: now,
        path: fx.clone(),
        at: SpawnAt::Directed { pos, dir },
        depth: p.depth + 1,
        bolt: None,
        repose: false,
    });
}

/// `CEmitter::Update`'s `emitFx` loop (CoDMP.exe 0x48fcd4..0x4902ef): walk
/// the cursor's path in 12 ms steps up to `now_ms`; once a step lands
/// `spacing` or more from the last play, a Newton step on the squared
/// distance (`3` at 0x5690c8, `0.25` at 0x568ed0, `1000` at 0x569090)
/// pulls the time back to where the distance is `spacing`, no earlier
/// than the step before. The effect plays there, the cursor moves to it
/// and a new spacing is drawn. Returns the play positions.
fn emit_along(e: &mut Emit, accel: Vec3, now_ms: i64, rng: &mut Rng) -> Vec<Vec3> {
    let mut out = Vec::new();
    let at = |e: &Emit, ms: i64| {
        let t = ms as f32 * 0.001;
        e.pos + e.vel * t + 0.5 * accel * t * t
    };
    let mut clock = e.last_ms;
    let mut s = 0;
    while clock < now_ms {
        s += EMIT_STEP_MS;
        let p = at(e, s);
        let thresh2 = e.spacing * e.spacing;
        if (p - e.pos).length_squared() < thresh2 {
            clock += EMIT_STEP_MS;
            continue;
        }
        let t = s as f32 * 0.001;
        let (v2, va, a2) = (
            e.vel.length_squared(),
            e.vel.dot(accel),
            accel.length_squared(),
        );
        let slope = 2.0 * t * v2 + 3.0 * t * t * va + t * t * t * a2;
        if slope != 0.0 {
            let excess = t * t * v2 + t * t * t * va + 0.25 * t * t * t * t * a2 - thresh2;
            // Never at or before the last play, or a zero spacing would
            // spin here; retail has no such floor.
            s = (s - (excess / slope * 1000.0).round() as i64)
                .max(clock - e.last_ms)
                .max(1);
        }
        let p = at(e, s);
        out.push(p);
        e.vel += accel * (s as f32 * 0.001);
        e.pos = p;
        e.last_ms += s;
        clock = e.last_ms;
        s = 0;
        e.spacing = emit_spacing(rng, e.density, e.variance);
    }
    out
}

/// `CParticle::UpdateOrigin` (CoDMP.exe 0x48df20, grammar doc R9): the
/// velocity takes the frame's acceleration first, then the position moves
/// on the new velocity plus half the acceleration. A physics particle
/// traces that move against solid brushes; a hit plays its impact effect,
/// kills an `impactKills` particle, or reflects the velocity scaled by
/// `bounce` and parks the particle at the hit. False when it dies.
fn update_origin(
    p: &mut Particle,
    dt: f32,
    world: Option<&CollisionWorld>,
    now: f32,
    chains: &mut Vec<Chain>,
) -> bool {
    let new_pos = match p.cod2.as_deref().filter(|l| l.moving) {
        // A CoD2 element sits where its velocity integrals put it.
        Some(l) => {
            let at = l.position(now - p.spawn);
            if dt > 0.0 {
                p.vel = (at - p.pos) / dt;
            }
            at
        }
        None => {
            p.vel += p.accel * dt;
            p.pos + p.vel * dt + 0.5 * p.accel * dt * dt
        }
    };
    if p.physics
        && let Some(world) = world
    {
        let tr = world.point_trace(p.pos, new_pos, CONTENTS_SOLID, false);
        if tr.fraction < 1.0 && !tr.startsolid && !tr.allsolid {
            if let Some(fx) = &p.impact_fx {
                chains.push(Chain {
                    due: now,
                    path: efx::chained_path(fx),
                    at: SpawnAt::Surface {
                        pos: tr.endpos,
                        normal: tr.normal,
                    },
                    depth: p.depth + 1,
                    bolt: None,
                    repose: false,
                });
            }
            if p.impact_kills {
                return false;
            }
            // A bounced CoD2 element falls on as a CoD1 one does from here:
            // `gravity * s^2` is an acceleration of twice `gravity`.
            if let Some(l) = &mut p.cod2
                && l.moving
            {
                l.moving = false;
                p.accel = l.up * (2.0 * l.gravity);
            }
            p.vel += p.accel * (tr.fraction * dt);
            p.vel = (p.vel - 2.0 * p.vel.dot(tr.normal) * tr.normal) * p.elasticity;
            // Too slow off a floor: it rests, and stops tracing.
            if tr.normal.z > 0.0 && p.vel.z < 4.0 {
                p.vel = Vec3::ZERO;
                p.accel = Vec3::ZERO;
                p.physics = false;
                p.impact_fx = None;
            }
            p.pos = p.pos.lerp(new_pos, tr.fraction);
            return true;
        }
    }
    p.pos = new_pos;
    true
}

/// Farthest from `cam_pos` first, the order `Renderer::set_fx_quads` draws in.
pub fn sort_back_to_front(quads: &mut Vec<FxQuad>, cam_pos: Vec3) {
    // each distance once, not twice per comparison
    let mut keyed: Vec<(f32, FxQuad)> = quads
        .drain(..)
        .map(|q| ((quad_center(&q) - cam_pos).length_squared(), q))
        .collect();
    // total_cmp: a NaN must not panic here.
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    quads.extend(keyed.into_iter().map(|(_, q)| q));
}

fn quad_center(q: &FxQuad) -> Vec3 {
    q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0
}

fn make_quad(center: Vec3, x: Vec3, y: Vec3, rgb: Vec3, alpha: f32, shader: &Arc<str>) -> FxQuad {
    let mut verts = [[0.0f32; 3]; 4];
    let mut uvs = [[0.0f32; 2]; 4];
    for (i, (u, v)) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        verts[i] = (center + x * (u * 2.0 - 1.0) + y * (v * 2.0 - 1.0)).to_array();
        uvs[i] = [u, v];
    }
    FxQuad {
        verts,
        uvs,
        rgba: [rgb.x, rgb.y, rgb.z, alpha],
        shader: shader.clone(),
    }
}

/// Unit vector perpendicular to `axis`, preferring `view_dir`. The fallbacks
/// cover a segment aimed at the camera, which would collapse the quad.
fn stretch_side_dir(axis: Vec3, view_dir: Vec3, cam_up: Vec3, cam_right: Vec3) -> Vec3 {
    for candidate in [view_dir, cam_up, cam_right] {
        let c = axis.cross(candidate);
        if c.length_squared() > 1e-8 {
            return c.normalize();
        }
    }
    Vec3::ZERO
}

/// A particle's world origin and velocity; `None` when its tag is gone.
fn world_state(p: &Particle, bolts: &HashMap<Bolt, BoltFrame>) -> Option<(Vec3, Vec3)> {
    match p.bolt {
        None => Some((p.pos, p.vel)),
        Some(id) => {
            let f = *bolts.get(&id)?;
            Some((f.origin + f.rotate(p.pos), f.rotate(p.vel)))
        }
    }
}

fn live(p: &Particle, now: f32) -> bool {
    now >= p.spawn && p.life > 0.0 && now - p.spawn <= p.life
}

/// A particle's extents at life fraction `t`: half-width, half-height,
/// and the tail's length for a stretched one.
struct Shape {
    half: f32,
    height: f32,
    len: Option<f32>,
}

impl Shape {
    fn at(p: &Particle, t: f32) -> Shape {
        match &p.cod2 {
            // `size2` draws only under `nonUniformScale`, as on CoD1.
            Some(l) => Shape {
                half: l.size(t),
                height: if l.prim().non_uniform_scale {
                    l.size2(t)
                } else {
                    l.size(t)
                },
                len: l.stretches().then(|| l.length(t)),
            },
            None => Shape {
                half: lerp(p.size0, p.size1, t),
                height: lerp(p.height0, p.height1, t),
                len: (p.len0 != 0.0 || p.len1 != 0.0).then(|| lerp(p.len0, p.len1, t)),
            },
        }
    }
}

/// A sprite's vertex colour at `t` through its life: the rgb curve, then
/// the alpha curve clamped to 0..1, into the alpha (`useAlpha`) or onto the
/// rgb with an opaque alpha (0x48e640 then 0x48e7d0).
/// [`vertex_colour`] at `pos`, a CoD4 element's alpha scaled by its
/// distance fade from `cam_pos` ([`cod2::Live::distance_fade`]).
fn faded_colour(p: &Particle, t: f32, pos: Vec3, cam_pos: Vec3) -> (Vec3, f32) {
    let (rgb, alpha) = vertex_colour(p, t);
    let fade = p
        .cod2
        .as_ref()
        .map_or(1.0, |l| l.distance_fade(pos.distance(cam_pos)));
    if p.use_alpha {
        (rgb, alpha * fade)
    } else {
        (rgb * fade, alpha)
    }
}

fn vertex_colour(p: &Particle, t: f32) -> (Vec3, f32) {
    let (rgb, alpha) = match &p.cod2 {
        Some(l) => (l.rgb(t), l.alpha(t)),
        None => (
            p.rgb0.lerp(p.rgb1, t),
            lerp(p.alpha0, p.alpha1, t).clamp(0.0, 1.0),
        ),
    };
    if p.use_alpha {
        (rgb, alpha)
    } else {
        (rgb * alpha, 1.0)
    }
}

/// A CoD2 `Cloud`, drawn by [`FxSystem::clouds`] instead of as a quad.
fn is_cloud(p: &Particle) -> bool {
    p.cod2.as_ref().is_some_and(|l| l.cloud_axis.is_some())
}

fn push_quads(
    pool: &VecDeque<Particle>,
    bolts: &HashMap<Bolt, BoltFrame>,
    cam_pos: Vec3,
    cam_right: Vec3,
    cam_up: Vec3,
    now: f32,
    out: &mut Vec<FxQuad>,
) {
    for p in pool {
        // An element with no `shaders` list draws retail's shader handle 0,
        // `<default>`, at its size; every stock one is zero-sized, so it
        // draws nothing (grammar doc R4, "An element with no shaders").
        if p.is_light || p.debris.is_some() || p.shader.is_empty() || !live(p, now) || is_cloud(p) {
            continue;
        }
        let Some((pos, vel)) = world_state(p, bolts) else {
            continue;
        };
        let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
        let shape = Shape::at(p, t);
        // `size` is a half-extent (grammar doc R4).
        let half = shape.half;
        let (rgb, alpha) = faded_colour(p, t, pos, cam_pos);
        let rad = p.rot.to_radians();
        let uvs = p
            .cod2
            .as_ref()
            .and_then(|l| l.uvs((now - p.spawn) * 1000.0));
        let mut push = |mut q: FxQuad| {
            if let Some(uvs) = uvs {
                q.uvs = uvs;
            }
            out.push(q);
        };

        if let Some(len) = shape.len {
            // Anchored at the head, trailing `length` back along travel,
            // `2 * size` wide (grammar doc R5).
            let head = pos;
            let tail = head - vel.normalize_or_zero() * len;
            let mid = (head + tail) * 0.5;
            let axis = (head - tail) * 0.5;
            let view_dir = (cam_pos - mid).normalize_or_zero();
            let side = stretch_side_dir(axis, view_dir, cam_up, cam_right) * half;
            push(make_quad(mid, axis, side, rgb, alpha, &p.shader));
        } else {
            // `size` across the view, `size2` up it, both turned by the
            // rotation (0x5101d0).
            let height = shape.height;
            let (rx, ry) = rotate2(cam_right, cam_up, rad);
            push(make_quad(
                pos,
                rx * half,
                ry * height,
                rgb,
                alpha,
                &p.shader,
            ));
        }
    }
}

/// Retail's moving tracer: `CG_SpawnTracer` @ 0x30038f30 starts a linear
/// trajectory at the muzzle, `CG_AddLocalEntities` @ 0x300201b0 draws
/// `[p, p + length*dir]` off it (0x3001ffe0) until it expires, so the tail
/// leaves the muzzle. Lifetime in seconds: the tail's travel to
/// `impact - length*dir`, truncated to whole ms.
fn moving_tracer_life(muzzle: Vec3, impact: Vec3, len: f32) -> f32 {
    let dir = (impact - muzzle).normalize_or_zero();
    let travel = (impact - dir * len - muzzle).length();
    (travel * 1000.0 / TRACER_SPEED).trunc() / 1000.0
}

/// A tracer as a stretch particle: `head` leads, the tail trails `len` back
/// along `vel`.
fn tracer_particle(spawn: f32, life: f32, head: Vec3, vel: Vec3, len: f32, width: f32) -> Particle {
    Particle {
        spawn,
        life,
        pos: head,
        vel,
        accel: Vec3::ZERO,
        rot: 0.0,
        rot_delta: 0.0,
        size0: width,
        size1: width,
        height0: width,
        height1: width,
        len0: len,
        len1: len,
        alpha0: 1.0,
        alpha1: 1.0,
        rgb0: Vec3::ONE,
        rgb1: Vec3::ONE,
        use_alpha: false,
        physics: false,
        impact_kills: false,
        elasticity: 0.0,
        impact_fx: None,
        death_fx: None,
        shader: TRACER_SHADER.into(),
        is_light: false,
        debris: None,
        emit: None,
        emit4: None,
        depth: 0,
        bolt: None,
        repose: None,
        cod2: None,
    }
}

pub struct FxSystem {
    cache: HashMap<String, Option<Effect>>,
    warned: HashSet<String>,
    particles: VecDeque<Particle>,
    marks: Vec<PendingMark>,
    /// Effects other effects started, for [`Self::spawn_chained`].
    chains: Vec<Chain>,
    /// The camera, for `cullrange`.
    view: Option<Vec3>,
    /// The camera's frustum, for `frustumCull`; see [`Self::set_frustum`].
    frustum: Option<Frustum>,
    /// This frame's tags effects are played on, see [`Self::set_bolts`]
    /// and [`Self::set_tag_bolts`].
    bolts: HashMap<Bolt, BoltFrame>,
    rng: Rng,
    /// `CameraShake` elements played since [`Self::take_shakes`].
    shakes: Vec<Shake>,
    /// A United Offensive `F` waiting for the next step, ms.
    rewind_ms: Option<f32>,
    /// United Offensive's tracer rules, see [`Self::set_tracer_rules`].
    tracer_rules: TracerRules,
    /// CoD2's tracers in flight ([`Self::spawn_cod2_tracer`]).
    cod2_tracers: Vec<cod2_tracer::Tracer>,
    /// CoD4 trails on their effects' tags.
    trails: Vec<(Bolt, cod4_trail::Trail)>,
}

/// Most CoD4 trails running at once; a new one past it ends the oldest.
const MAX_TRAILS: usize = 64;

/// How a United Offensive cgame draws a tracer where 1.1's and 1.5's do
/// not (docs/research/coduo-diff.md 19, "The tracer cvars").
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TracerRules {
    /// The moving streak needs a shot at least as long as the tracer
    /// (uo cgame 0x300482ff); 1.1's and 1.5's spawn one for any length.
    pub min_length: bool,
    /// `sv_night` (systeminfo): every tracer drawn
    /// [`TRACER_NIGHT_SCALE`] times as wide.
    pub night: bool,
}

/// A United Offensive `CameraShake` element (CoDUOMP.exe 0x4a908a): the
/// engine hands the cgame's shake core (uo cgame 0x3001b420) a random
/// `life` in ms, `radius`, and `intensity` scaled by 0.05, at the effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shake {
    pub scale: f32,
    pub duration_ms: f32,
    pub radius: f32,
    pub origin: Vec3,
}

impl Default for FxSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FxSystem {
    pub fn new() -> FxSystem {
        FxSystem {
            cache: HashMap::new(),
            warned: HashSet::new(),
            particles: VecDeque::new(),
            marks: Vec::new(),
            chains: Vec::new(),
            view: None,
            frustum: None,
            bolts: HashMap::new(),
            rng: Rng::new(0x9E3779B97F4A7C15),
            shakes: Vec::new(),
            rewind_ms: None,
            tracer_rules: TracerRules::default(),
            cod2_tracers: Vec::new(),
            trails: Vec::new(),
        }
    }

    /// A CoD2 tracer from `muzzle` to `hit`, spawned at `now` less
    /// `back_ms`; the event path rolled its chance.
    pub fn spawn_cod2_tracer(&mut self, muzzle: Vec3, hit: Vec3, now: f32, back_ms: i32) {
        crate::frame_digest::effect("", muzzle);
        crate::frame_digest::effect("", hit);
        crate::frame_digest::u32(crate::frame_digest::Part::Fx, back_ms as u32);
        // A full pool reuses the oldest (`CG_AllocLocalEntity`).
        if self.cod2_tracers.len() >= cod2_tracer::MAX_LOCAL_ENTITIES {
            self.cod2_tracers.remove(0);
        }
        self.cod2_tracers
            .push(cod2_tracer::Tracer::new(muzzle, hit, now, back_ms));
    }

    /// A CoD4 tracer from `muzzle` to `hit` (`CG_SpawnTracer` under 1.7's
    /// dvars, [`cod2_tracer::COD4`]), back-dated `back_ms`; the caller
    /// rolled its chance.
    pub fn spawn_cod4_tracer(&mut self, muzzle: Vec3, hit: Vec3, now: f32, back_ms: i32) {
        if self.cod2_tracers.len() >= cod2_tracer::MAX_LOCAL_ENTITIES {
            self.cod2_tracers.remove(0);
        }
        self.cod2_tracers.push(cod2_tracer::Tracer::with(
            cod2_tracer::COD4,
            muzzle,
            hit,
            now,
            back_ms,
        ));
    }

    /// The connection's tracer rules: a United Offensive server's, with
    /// its systeminfo `sv_night`, or the plain ones.
    pub fn set_tracer_rules(&mut self, rules: TracerRules) {
        self.tracer_rules = rules;
    }

    /// United Offensive's `F <ms>` (`rewindFx`; uo cgame trap 0xf0,
    /// CoDUOMP.exe 0x4af950): the next step frees every element spawned in
    /// the last `ms`, and the chained effects due in it.
    pub fn rewind(&mut self, ms: f32) {
        self.rewind_ms = Some(ms);
    }

    /// The camera shakes the effects played since the last call started.
    pub fn take_shakes(&mut self) -> Vec<Shake> {
        std::mem::take(&mut self.shakes)
    }

    fn ensure_cached(&mut self, fs: &dyn AssetSource, path: &str) {
        if self.cache.contains_key(path) {
            return;
        }
        // Registration keeps a name up to its first `.` and reads it back as
        // `.efx` (0x492eb0, efx-grammar.md R9), which United Offensive's
        // `worldFlashEffect fx/weapon/muzzleflash/mf_kar98.efx.efx` needs
        // (`efx::chained_path`, in `Effect`'s find).
        let effect = match efx::Effect::find(fs, path) {
            Ok(e) => Some(e),
            Err(err) => {
                if self.warned.insert(path.to_string()) {
                    log::warn!("fx {path}: {err}");
                }
                None
            }
        };
        self.cache.insert(path.to_string(), effect);
    }

    /// Parses `paths` and every effect they chain to (impact, emitted,
    /// death and runner effects) now, as a level's registration does,
    /// so no first play reads a file in its frame. Returns the shaders
    /// they draw: `Decal` elements' mark materials, then every other
    /// element's sprites.
    pub fn preload<'a>(
        &mut self,
        fs: &dyn AssetSource,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> (Vec<Arc<str>>, Vec<Arc<str>>) {
        let mut todo: Vec<String> = paths.into_iter().map(str::to_string).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let (mut marks, mut sprites) = (Vec::new(), Vec::new());
        while let Some(path) = todo.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            self.ensure_cached(fs, &path);
            let Some(Some(e)) = self.cache.get(&path) else {
                continue;
            };
            for m in &e.emitters {
                let set = if m.kind == "Decal" {
                    &mut marks
                } else {
                    &mut sprites
                };
                set.extend(m.shaders.iter().cloned());
                for chained in [&m.impact_fx, &m.emit_fx, &m.death_fx, &m.play_fx] {
                    todo.extend(chained.iter().filter(|p| !seen.contains(*p)).cloned());
                }
            }
        }
        for v in [&mut marks, &mut sprites] {
            v.sort();
            v.dedup();
        }
        (marks, sprites)
    }

    /// CoD1's [`Self::preload`]: parses `paths` and every effect they chain
    /// to (`playfx`, `emitfx`, `impactfx`, `deathfx`, read as
    /// [`efx::chained_path`] names them) now, and returns the shaders their elements
    /// draw: `Decal` elements' mark shaders, then every other's. A CoD1
    /// level reads its effects this way with the gamestate, not in an
    /// effect's first frame (cod11-client-soak.md). The third set is the
    /// xmodels their `Model` elements throw (brass, debris).
    pub fn preload_cod1<'a>(
        &mut self,
        fs: &dyn AssetSource,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
        let mut todo: Vec<String> = paths.into_iter().map(str::to_string).collect();
        let mut seen = HashSet::new();
        let (mut marks, mut sprites, mut models) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        while let Some(path) = todo.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            self.ensure_cached(fs, &path);
            let Some(Some(effect)) = self.cache.get(&path) else {
                continue;
            };
            for em in &effect.emitters {
                let set = if em.kind == "Decal" {
                    &mut marks
                } else {
                    &mut sprites
                };
                set.extend(em.shaders.iter().map(|s| s.to_string()));
                models.extend(em.models.iter().cloned());
                for fx in em
                    .play_fx
                    .iter()
                    .chain(&em.emit_fx)
                    .chain(&em.impact_fx)
                    .chain(&em.death_fx)
                {
                    todo.push(efx::chained_path(fx));
                }
            }
        }
        (marks, sprites, models)
    }

    /// An effect built elsewhere (CoD4's zone effects, `cod4_fx`), held
    /// under `path` as [`Self::spawn`] and the chains look it up.
    pub fn insert(&mut self, path: &str, effect: Effect) {
        self.cache.insert(path.to_string(), Some(effect));
    }

    /// A download reopened the search path: effect files parse again, and a
    /// miss may now resolve.
    pub fn reopen(&mut self) {
        self.cache.clear();
        self.warned.clear();
    }

    /// A new map: world-space particles and marks are meaningless there.
    pub fn clear(&mut self) {
        self.particles.clear();
        self.marks.clear();
        self.chains.clear();
        self.bolts.clear();
        self.cod2_tracers.clear();
        self.trails.clear();
    }

    /// The camera this frame's spawns are culled against. Drops the last
    /// frustum: a caller that culls by one sets it after this.
    pub fn set_view(&mut self, pos: Vec3) {
        self.view = Some(pos);
        self.frustum = None;
    }

    /// The camera's frustum this frame's `frustumCull` spawns are culled
    /// against (cod4-effects.md 18). Only CoD4's frame sets one.
    pub fn set_frustum(&mut self, frustum: Frustum) {
        self.frustum = Some(frustum);
    }

    /// The camera [`Self::set_view`] last set.
    #[cfg(test)]
    pub fn view(&self) -> Option<Vec3> {
        self.view
    }

    /// This frame's tags, as (origin, forward) by key: the shooters'
    /// muzzles, `u32::MAX` the viewmodel's. A `relative` element of an
    /// effect played on one rides it, and dies with it.
    pub fn set_bolts(&mut self, bolts: &HashMap<u32, (Vec3, Vec3)>) {
        self.bolts.retain(|b, _| !matches!(b, Bolt::Muzzle(_)));
        self.bolts.extend(
            bolts
                .iter()
                .map(|(&id, &f)| (Bolt::Muzzle(id), BoltFrame::new(f))),
        );
    }

    /// This frame's poses of the `playFxOnTag` tags [`Self::tag_bolts`]
    /// named. A `relative` element on a tag missing here dies, as a bolt
    /// that fails to pose does (R10).
    pub fn set_tag_bolts(&mut self, tags: &HashMap<TagBolt, TagPose>) {
        self.bolts.retain(|b, _| !matches!(b, Bolt::Tag(_)));
        self.bolts.extend(
            tags.iter()
                .map(|(&t, &pose)| (Bolt::Tag(t), BoltFrame::tag(pose))),
        );
    }

    /// The `playFxOnTag` tags a live element, a pending chain or a CoD4
    /// trail rides, which the caller poses for [`Self::set_tag_bolts`] each
    /// frame.
    pub fn tag_bolts(&self) -> HashSet<TagBolt> {
        let particles = self.particles.iter().filter_map(|p| p.bolt);
        let chains = self.chains.iter().filter_map(|c| c.bolt);
        let trails = self
            .trails
            .iter()
            .filter(|(_, t)| !t.ended)
            .map(|(b, _)| *b);
        particles
            .chain(chains)
            .chain(trails)
            .filter_map(|b| match b {
                Bolt::Tag(t) => Some(t),
                Bolt::Muzzle(_) => None,
            })
            .collect()
    }

    /// `CG_PlayFxOnTag` (cgame 0x3001db80): the effect on the tag's pose
    /// through syscall 0xe5, bolted to it. Only its `relative` elements
    /// follow the tag after this frame (`PlayEffect` 0x493870, R10).
    pub fn spawn_on_tag(
        &mut self,
        fs: &dyn AssetSource,
        path: &str,
        tag: TagBolt,
        pose: TagPose,
        now: f32,
    ) -> Vec<FxSound> {
        crate::frame_digest::effect(path, pose.origin);
        self.bolts.insert(Bolt::Tag(tag), BoltFrame::tag(pose));
        let at = SpawnAt::Oriented {
            pos: pose.origin,
            axis: pose.axis,
        };
        let first_chain = self.chains.len();
        let mut sounds = self.spawn_effect(fs, path, at, now, 0, Some(Bolt::Tag(tag)));
        sounds.extend(self.drain_chains(fs, now, first_chain));
        sounds
    }

    /// Missing or broken files warn once and no-op. Returns the `Sound`
    /// cues for the audio system.
    pub fn spawn(
        &mut self,
        fs: &dyn AssetSource,
        path: &str,
        at: SpawnAt,
        now: f32,
    ) -> Vec<FxSound> {
        self.spawn_marked(fs, path, at, None, now, true)
    }

    /// [`Self::spawn`] on the tag `bolt` (a [`Self::set_bolts`] key) when
    /// it is set; with `marks` false its `Decal` elements, and those of
    /// the effects it starts at once, place nothing, as
    /// `EV_ROCKET_EXPLODE_NOMARKS` has the cgame refuse them.
    pub fn spawn_marked(
        &mut self,
        fs: &dyn AssetSource,
        path: &str,
        at: SpawnAt,
        bolt: Option<u32>,
        now: f32,
        marks: bool,
    ) -> Vec<FxSound> {
        crate::frame_digest::effect(path, spawn_pos(at));
        let first_mark = self.marks.len();
        let first_chain = self.chains.len();
        let mut sounds = self.spawn_effect(fs, path, at, now, 0, bolt.map(Bolt::Muzzle));
        // An `FxRunner` without a delay plays inside PlayEffect (0x493870).
        sounds.extend(self.drain_chains(fs, now, first_chain));
        if !marks {
            self.marks.truncate(first_mark);
        }
        sounds
    }

    fn spawn_effect(
        &mut self,
        fs: &dyn AssetSource,
        path: &str,
        at: SpawnAt,
        now: f32,
        depth: u8,
        bolt: Option<Bolt>,
    ) -> Vec<FxSound> {
        if depth > MAX_CHAIN_DEPTH {
            return Vec::new();
        }
        self.ensure_cached(fs, path);
        let cx = SpawnCx {
            at,
            now,
            view: self.view,
            frustum: self.frustum,
            depth,
            bolt: bolt.map(|id| (id, self.bolts.get(&id).copied())),
        };
        let mut sounds = Vec::new();
        if let Some(Some(effect)) = self.cache.get(path) {
            let mut out = SpawnOut {
                particles: &mut self.particles,
                marks: &mut self.marks,
                chains: &mut self.chains,
                trails: &mut self.trails,
            };
            for em in &effect.emitters {
                if em.kind == "CameraShake" && em.cod2.is_some() {
                    // `FX_AddCameraShake` (Mac 0x59fec) hands the cgame
                    // origin, intensity and radius; not drawn yet.
                    log::debug!("fx {path}: CoD2 CameraShake at {:?}", spawn_pos(at));
                    continue;
                }
                if em.kind == "CameraShake" {
                    let origin = spawn_pos(at);
                    let culled = em.cullrange > 0.0
                        && self
                            .view
                            .is_some_and(|v| (origin - v).length() > em.cullrange);
                    if !culled && let Some(strength) = em.intensity.or(em.bounce) {
                        self.shakes.push(Shake {
                            scale: sample_r1(&mut self.rng, strength) * 0.05,
                            duration_ms: sample_r1(&mut self.rng, em.life),
                            radius: sample_r1(&mut self.rng, em.radius),
                            origin,
                        });
                    }
                    continue;
                }
                spawn_emitter(&mut self.rng, &mut out, em, &cx);
                sounds.extend(spawn_sound_cues(&mut self.rng, em, at));
            }
        }
        sounds
    }

    /// Spawns the chained effects due by `now` from `chains[from..]`,
    /// and those they start in turn.
    fn drain_chains(&mut self, fs: &dyn AssetSource, now: f32, from: usize) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        let mut i = from;
        while i < self.chains.len() {
            if self.chains[i].due > now {
                i += 1;
                continue;
            }
            let mut c = self.chains.remove(i);
            if c.repose {
                let frame = c.bolt.and_then(|id| self.bolts.get(&id).copied());
                let Some(f) = frame else {
                    continue;
                };
                c.at = f.carry(c.at, true);
            }
            sounds.extend(self.spawn_effect(fs, &c.path, c.at, now, c.depth, c.bolt));
        }
        sounds
    }

    /// Plays the effects due by `now` that others started: delayed
    /// `playfx`, and the `impactfx`, `emitfx` and `deathfx` the steps
    /// since the last call raised.
    /// The chained effects waiting to start, by path, for the gates.
    #[cfg(test)]
    pub(crate) fn pending_chains(&self) -> Vec<&str> {
        self.chains.iter().map(|c| c.path.as_str()).collect()
    }

    pub fn spawn_chained(&mut self, fs: &dyn AssetSource, now: f32) -> Vec<FxSound> {
        self.drain_chains(fs, now, 0)
    }

    /// One tracer from `muzzle` towards `impact`, per the retail client's
    /// hardcoded tracer (grammar doc R8). `flesh` is `surfType == 7`, which
    /// retail draws as a one-frame segment instead of a moving streak.
    /// `heavy` is a United Offensive lmg, hmg or umg round, whose chance,
    /// length and width are the `*lmg` cvars'; `roll` false skips the
    /// chance (a server-rolled tracer). `frame_dt` is the frame's length in
    /// seconds, for the spawn back-date. False when the chance roll, the
    /// flesh distance gate or United Offensive's length gate
    /// ([`TracerRules`]) drops the shot.
    pub fn spawn_tracer(
        &mut self,
        (muzzle, impact): (Vec3, Vec3),
        flesh: bool,
        (heavy, roll): (bool, bool),
        now: f32,
        frame_dt: f32,
    ) -> bool {
        crate::frame_digest::effect("", muzzle);
        crate::frame_digest::effect("", impact);
        let flags = u32::from(flesh) | u32::from(heavy) << 1 | u32::from(roll) << 2;
        crate::frame_digest::u32(crate::frame_digest::Part::Fx, flags);
        let (chance, len, mut width) = match heavy {
            true => (TRACER_CHANCE_HEAVY, TRACER_LENGTH_HEAVY, TRACER_WIDTH_HEAVY),
            false => (TRACER_CHANCE, TRACER_LENGTH, TRACER_WIDTH),
        };
        if roll && self.rng.next_f32() >= chance {
            return false;
        }
        if self.tracer_rules.night {
            width *= TRACER_NIGHT_SCALE;
        }
        let delta = impact - muzzle;
        let dist = delta.length();
        let dir = delta.normalize_or_zero();
        if !flesh && self.tracer_rules.min_length && dist < len {
            return false;
        }
        let p = if flesh {
            if dist < TRACER_FLESH_MIN_DIST {
                return false;
            }
            let near = TRACER_FLESH_NEAR + (dist - TRACER_FLESH_SPREAD_CUT) * self.rng.next_f32();
            let far = (near + len).min(dist);
            tracer_particle(
                now,
                // Drawn on the spawn frame only; `step` drops it before it moves.
                f32::MIN_POSITIVE,
                muzzle + dir * far,
                dir,
                far - near,
                width,
            )
        } else {
            // `(rand() % cg.frametime) / 2` in whole ms.
            let frame_ms = (frame_dt * 1000.0) as u64;
            let back = if frame_ms == 0 {
                0.0
            } else {
                ((self.rng.next_u64() % frame_ms) / 2) as f32 / 1000.0
            };
            let vel = dir * TRACER_SPEED;
            tracer_particle(
                now - back,
                moving_tracer_life(muzzle, impact, len),
                muzzle + dir * len + vel * back,
                vel,
                len,
                width,
            )
        };
        push_capped(&mut self.particles, p, MAX_PARTICLES);
        true
    }

    #[cfg(test)]
    pub fn spawn_effect_for_test(&mut self, effect: Effect, at: SpawnAt, now: f32) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        let cx = SpawnCx {
            at,
            now,
            view: self.view,
            frustum: self.frustum,
            depth: 0,
            bolt: None,
        };
        let mut out = SpawnOut {
            particles: &mut self.particles,
            marks: &mut self.marks,
            chains: &mut self.chains,
            trails: &mut self.trails,
        };
        for em in &effect.emitters {
            spawn_emitter(&mut self.rng, &mut out, em, &cx);
            sounds.extend(spawn_sound_cues(&mut self.rng, em, at));
        }
        sounds
    }

    /// Advance the simulation. `world` enables usePhysics collision. Call
    /// [`Self::spawn_chained`] after it for the effects the step started.
    pub fn step(&mut self, dt: f32, now: f32, world: Option<&CollisionWorld>) {
        if let Some(ms) = self.rewind_ms.take() {
            let since = now - ms / 1000.0;
            self.particles.retain(|p| p.spawn <= since);
            self.chains.retain(|c| c.due <= since);
        }
        let cx = StepCx {
            dt,
            now,
            world,
            bolts: &self.bolts,
        };
        step_pool(&mut self.particles, &cx, &mut self.rng, &mut self.chains);
        for (id, t) in &mut self.trails {
            let frame = self.bolts.get(id).map(|f| {
                let (x, y, z) = f.axis;
                (f.origin, [x, y, z])
            });
            t.advance(frame, now, &mut self.rng);
        }
        self.trails.retain(|(_, t)| !t.done(now));
        self.cod2_tracers.retain(|t| now < t.end);
    }

    /// This frame's `Emitter` debris models.
    pub fn models(&self, now: f32) -> Vec<FxModel> {
        self.particles
            .iter()
            .filter(|p| live(p, now))
            .filter_map(|p| {
                let d = p.debris.as_ref()?;
                let model = d.model.clone()?;
                let (origin, _) = world_state(p, &self.bolts)?;
                let frame = p.bolt.and_then(|id| self.bolts.get(&id)).copied();
                let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
                let scale = Shape::at(p, t).half;
                let axis = angles_to_axis_msvc(d.angles.to_array()).map(|a| {
                    let a = Vec3::from(a) * scale;
                    frame.map_or(a, |f| f.rotate(a))
                });
                Some(FxModel {
                    model,
                    origin,
                    axis,
                })
            })
            .collect()
    }

    /// This frame's quads, far to near from `cam_pos`; the renderer draws
    /// in order.
    pub fn build_quads(
        &self,
        cam_pos: Vec3,
        cam_right: Vec3,
        cam_up: Vec3,
        now: f32,
    ) -> Vec<FxQuad> {
        let mut out = Vec::new();
        push_quads(
            &self.particles,
            &self.bolts,
            cam_pos,
            cam_right,
            cam_up,
            now,
            &mut out,
        );
        let cam_fwd = cam_up.cross(cam_right);
        for t in self.cod2_tracers.iter().filter(|t| t.live(now)) {
            t.quads(now, cam_pos, cam_fwd, cam_right, cam_up, &mut out);
        }
        for (_, t) in &self.trails {
            t.quads(now, cam_pos, &mut out);
        }
        sort_back_to_front(&mut out, cam_pos);
        out
    }

    /// This frame's CoD2 clouds (`fx::cod2_cloud`), far to near from
    /// `cam_pos`.
    pub fn clouds(
        &self,
        cam_pos: Vec3,
        cam_right: Vec3,
        cam_up: Vec3,
        now: f32,
    ) -> cod2_cloud::Clouds {
        let mut out: Vec<cod2_cloud::FxCloud> = Vec::new();
        for p in &self.particles {
            let Some((l, axis)) = p.cod2.as_ref().and_then(|l| Some((l, l.cloud_axis?))) else {
                continue;
            };
            if p.shader.is_empty() || !live(p, now) {
                continue;
            }
            let Some((pos, vel)) = world_state(p, &self.bolts) else {
                continue;
            };
            let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
            let size = l.size(t);
            let width = if l.prim().non_uniform_scale {
                l.size2(t)
            } else {
                size
            };
            let length = if l.prim().use_length {
                l.length(t)
            } else {
                width
            };
            let (rgb, alpha) = faded_colour(p, t, pos, cam_pos);
            out.push(cod2_cloud::FxCloud::new(
                pos,
                vel,
                (size, width, length),
                p.rot,
                axis,
                [rgb.x, rgb.y, rgb.z, alpha],
                &p.shader,
                (cam_right, cam_up),
            ));
        }
        let d = |c: &cod2_cloud::FxCloud| c.origin.distance_squared(cam_pos);
        out.sort_by(|a, b| d(b).total_cmp(&d(a)));
        cod2_cloud::Clouds {
            list: out,
            right: cam_right,
            up: cam_up,
        }
    }

    /// Live Light-kind particles as point lights: the first
    /// `MAX_SCENE_LIGHTS` in spawn order, as retail queues them while it
    /// draws, then sorted nearest `cam_pos` first.
    pub fn lights(&self, cam_pos: Vec3, now: f32) -> Vec<FxLight> {
        let mut candidates: Vec<(f32, FxLight)> = self
            .particles
            .iter()
            .filter(|p| p.is_light && live(p, now))
            .filter_map(|p| {
                let (pos, _) = world_state(p, &self.bolts)?;
                let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
                let radius = Shape::at(p, t).half;
                let rgb = match &p.cod2 {
                    Some(l) => l.rgb(t),
                    None => p.rgb0.lerp(p.rgb1, t),
                };
                let dist_sq = (pos - cam_pos).length_squared();
                Some((
                    dist_sq,
                    FxLight {
                        pos: pos.to_array(),
                        radius,
                        rgb: rgb.to_array(),
                    },
                ))
            })
            .take(MAX_SCENE_LIGHTS)
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        candidates.into_iter().map(|(_, l)| l).collect()
    }

    /// The `Decal` elements due by `now`. Their `cullrange` was tested
    /// at spawn.
    pub fn take_marks(&mut self, now: f32) -> Vec<MarkRequest> {
        let mut out = Vec::new();
        self.marks.retain(|m| {
            if m.due > now {
                return true;
            }
            out.push(m.req.clone());
            false
        });
        out
    }

    /// (live particles, pending marks, lights) for the F3 overlay's `fx` line.
    pub fn counts(&self) -> (usize, usize, usize) {
        let lights = self.particles.iter().filter(|p| p.is_light).count();
        let particles = self.particles.len() - lights;
        (particles, self.marks.len(), lights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::efx::{Curve, Emitter, R1};
    use glam::Vec3;
    use vcod_common::pk3::Pk3Fs;

    /// A minimal one-particle emitter: no ranges, so behavior is exact.
    #[allow(clippy::field_reassign_with_default)]
    fn simple_emitter() -> Emitter {
        let mut e = Emitter::default();
        e.kind = "Particle".into();
        e.count = R1 { a: 1.0, b: 1.0 };
        e.life = R1 {
            a: 1000.0,
            b: 1000.0,
        }; // 1s
        e.velocity.a = [10.0, 0.0, 0.0];
        e.velocity.b = [10.0, 0.0, 0.0];
        e.gravity = R1 {
            a: -100.0,
            b: -100.0,
        };
        e.size = Curve {
            start: R1 { a: 2.0, b: 2.0 },
            end: R1 { a: 4.0, b: 4.0 },
            flags: vec!["linear".into()],
        };
        e.alpha = Curve {
            start: R1 { a: 1.0, b: 1.0 },
            end: R1 { a: 0.0, b: 0.0 },
            flags: vec!["linear".into()],
        };
        e.shaders = vec!["gfx/effects/test".into()];
        // World-space vectors, so a `Point` spawn's up axis leaves them be.
        e.spawn_flags = vec!["absoluteVel".into(), "absoluteAccel".into()];
        e
    }

    fn sys_with(e: Emitter, at: SpawnAt) -> FxSystem {
        let mut s = FxSystem::new();
        s.spawn_effect_for_test(Effect { emitters: vec![e] }, at, 0.0);
        s
    }

    #[test]
    fn particle_integrates_velocity_and_gravity_and_expires() {
        let mut s = sys_with(simple_emitter(), SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.5, 0.5, None);
        let q = s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, 0.5);
        assert_eq!(q.len(), 1);
        // Retail's order: v = (10, 0, -50) first, then x = 10*0.5 = 5,
        // z = -50*0.5 - 0.5*100*0.25 = -37.5. The quad center is the mean
        // of its verts.
        let c = quad_center(&q[0]);
        assert!(
            (c.x - 5.0).abs() < 1e-3 && (c.z + 37.5).abs() < 1e-3,
            "{c:?}"
        );
        // half-size at mid-life: 2 + (4-2)*0.5 = 3
        assert!((quad_half_x(&q[0]) - 3.0).abs() < 1e-3);
        // No `useAlpha`: the half-faded alpha scales the rgb.
        assert_eq!(q[0].rgba, [0.5, 0.5, 0.5, 1.0]);
        s.step(0.6, 1.1, None); // past 1s life
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 1.1).is_empty());
    }

    /// `useAlpha` keeps the alpha curve in the vertex alpha and the rgb
    /// curve whole; without it the clamped alpha scales the rgb and the
    /// alpha is opaque (CoDMP.exe 0x48e7d0). `grenade2.efx`'s flash rgb,
    /// white going to (0.76, 0.44, 0.24), half-way through its life.
    #[test]
    fn use_alpha_picks_where_the_alpha_curve_goes() {
        let mut e = still_particle();
        e.rgb.end = R3 {
            a: [0.7608, 0.4353, 0.2392],
            b: [0.7608, 0.4353, 0.2392],
        };
        e.rgb.flags = vec!["linear".into()];
        e.alpha.start = R1 { a: 1.5, b: 1.5 };
        e.alpha.end = R1 { a: -0.5, b: -0.5 };
        let colour = |e: &Emitter, t: f32| {
            let mut s = sys_with(e.clone(), SpawnAt::Point { pos: Vec3::ZERO });
            s.step(t, t, None);
            s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, t)[0].rgba
        };
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        let rgb = [0.8804, 0.71765, 0.6196];
        let a = colour(&e, 0.5);
        assert!(
            near(a, [rgb[0] * 0.5, rgb[1] * 0.5, rgb[2] * 0.5, 1.0]),
            "{a:?}"
        );
        // The curve passes 1 early on: clamped first.
        let a = colour(&e, 0.1);
        assert!(a[3] == 1.0 && a[0] > 0.97, "{a:?}");
        e.flags = vec!["useAlpha".into()];
        let a = colour(&e, 0.5);
        assert!(near(a, [rgb[0], rgb[1], rgb[2], 0.5]), "{a:?}");
    }

    #[test]
    fn a_particle_without_shaders_lives_but_draws_no_quad() {
        let mut e = simple_emitter();
        e.shaders.clear();
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.1, 0.1, None);
        assert_eq!(s.counts().0, 1);
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.1).is_empty());
    }

    #[test]
    fn delay_holds_particle_invisible() {
        let mut e = simple_emitter();
        e.delay = R1 { a: 500.0, b: 500.0 };
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.25, 0.25, None);
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.25).is_empty());
        s.step(0.5, 0.75, None);
        assert_eq!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.75).len(), 1);
    }

    #[test]
    fn malformed_count_is_clamped_at_spawn_not_just_evicted_after() {
        let mut e = simple_emitter();
        e.count = R1 { a: 1e9, b: 1e9 };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        assert_eq!(s.counts().0, MAX_PARTICLES);
    }

    #[test]
    fn particle_cap_evicts_oldest() {
        let mut e = simple_emitter();
        e.count = R1 { a: 1.0, b: 1.0 };
        let mut s = FxSystem::new();
        for i in 0..(MAX_PARTICLES + 10) {
            s.spawn_effect_for_test(
                Effect {
                    emitters: vec![e.clone()],
                },
                SpawnAt::Point {
                    pos: Vec3::new(i as f32, 0.0, 0.0),
                },
                0.0,
            );
        }
        assert_eq!(s.counts().0, MAX_PARTICLES);
    }

    #[test]
    fn quads_sorted_far_to_near() {
        let mut s = FxSystem::new();
        for x in [10.0f32, 200.0, 50.0] {
            s.spawn_effect_for_test(
                Effect {
                    emitters: vec![simple_emitter()],
                },
                SpawnAt::Point {
                    pos: Vec3::new(x, 0.0, 0.0),
                },
                0.0,
            );
        }
        let q = s.build_quads(Vec3::ZERO, Vec3::Y, Vec3::Z, 0.0);
        let xs: Vec<f32> = q.iter().map(|q| quad_center(q).x).collect();
        assert_eq!(xs, vec![200.0, 50.0, 10.0]);
    }

    fn quad_center(q: &FxQuad) -> Vec3 {
        q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0
    }
    fn quad_half_x(q: &FxQuad) -> f32 {
        (Vec3::from(q.verts[1]) - Vec3::from(q.verts[0])).length() / 2.0
    }

    #[test]
    fn sound_and_trigger_kinds_spawn_nothing() {
        for kind in ["Sound", "FxRunner"] {
            let mut e = simple_emitter();
            e.kind = kind.into();
            let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
            assert_eq!(s.counts(), (0, 0, 0), "kind {kind} should be a no-op");
        }
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn spawn_returns_sound_cues_at_the_spawn_point() {
        let mut em = Emitter::default();
        em.kind = "Sound".to_string();
        em.sounds = vec!["kaboom".to_string()];
        em.delay = R1 { a: 250.0, b: 250.0 };
        let effect = Effect { emitters: vec![em] };
        let mut fx = FxSystem::new();
        let cues = fx.spawn_effect_for_test(
            effect,
            SpawnAt::Point {
                pos: Vec3::new(1.0, 2.0, 3.0),
            },
            0.0,
        );
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].alias, "kaboom");
        assert_eq!(cues[0].pos, Vec3::new(1.0, 2.0, 3.0));
        assert!((cues[0].delay_s - 0.25).abs() < 1e-6);
        assert_eq!(fx.counts(), (0, 0, 0));
    }

    #[test]
    fn decal_kind_hands_the_mark_code_a_request_not_a_quad() {
        let mut e = simple_emitter();
        e.kind = "Decal".into();
        e.rotation = R1 { a: 90.0, b: 90.0 };
        e.cullrange = 1200.0;
        let mut s = sys_with(
            e,
            SpawnAt::Surface {
                pos: Vec3::new(0.0, 0.0, 10.0),
                normal: Vec3::Z,
            },
        );
        assert_eq!(s.counts(), (0, 1, 0));
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Y, 0.0).is_empty());
        let marks = s.take_marks(0.0);
        assert_eq!(
            marks,
            vec![MarkRequest {
                shader: "gfx/effects/test".into(),
                origin: Vec3::new(0.0, 0.0, 10.0),
                dir: Vec3::Z,
                orientation: 90.0,
                rgba: [1.0; 4],
                alpha_fade: true,
                radius: 2.0,
                life_ms: 1000,
            }]
        );
        assert_eq!(s.counts(), (0, 0, 0));
    }

    #[test]
    fn a_nomarks_spawn_drops_its_decals_and_keeps_the_rest() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let at = SpawnAt::Surface {
            pos: Vec3::ZERO,
            normal: Vec3::Z,
        };
        let path = "fx/explosions/grenade2.efx";
        let mut s = FxSystem::new();
        s.spawn_marked(&fs, path, at, None, 0.0, false);
        assert_eq!(s.counts().1, 0);
        assert!(s.counts().0 > 0);
        s.spawn(&fs, path, at, 0.0);
        assert_eq!(s.take_marks(0.0).len(), 1);
    }

    /// `CG_MapRestart` frees every live effect: a particle in flight and a
    /// decal not yet handed to the mark pool both go, so no mark from the
    /// last round lands in the new one.
    #[test]
    fn a_map_restart_drops_particles_and_pending_decals() {
        let mut decal = simple_emitter();
        decal.kind = "Decal".into();
        let mut s = FxSystem::new();
        s.spawn_effect_for_test(
            Effect {
                emitters: vec![simple_emitter(), decal],
            },
            SpawnAt::Surface {
                pos: Vec3::ZERO,
                normal: Vec3::Z,
            },
            0.0,
        );
        assert_eq!(s.counts(), (1, 1, 0));
        s.clear();
        assert_eq!(s.counts(), (0, 0, 0));
        assert!(s.take_marks(0.0).is_empty());
    }

    #[test]
    fn a_decal_spawned_past_its_cullrange_places_no_mark() {
        let mut e = simple_emitter();
        e.kind = "Decal".into();
        e.cullrange = 100.0;
        let mut s = FxSystem::new();
        s.set_view(Vec3::new(0.0, 101.0, 0.0));
        s.spawn_effect_for_test(
            Effect { emitters: vec![e] },
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        assert!(s.take_marks(0.0).is_empty());
        assert_eq!(s.counts(), (0, 0, 0));
    }

    #[test]
    fn light_kind_contributes_a_light_not_a_quad() {
        let mut e = simple_emitter();
        e.kind = "Light".into();
        let s = sys_with(
            e,
            SpawnAt::Point {
                pos: Vec3::new(1.0, 2.0, 3.0),
            },
        );
        assert_eq!(s.counts(), (0, 0, 1));
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).is_empty());
        let lights = s.lights(Vec3::ZERO, 0.0);
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].pos, [1.0, 2.0, 3.0]);
        assert!((lights[0].radius - 2.0).abs() < 1e-3); // size.start at t=0
        assert_eq!(lights[0].rgb, [1.0, 1.0, 1.0]); // rgb absent -> white
    }

    #[test]
    fn lights_are_returned_nearest_cam_pos_first() {
        let mut e = simple_emitter();
        e.kind = "Light".into();
        let mut s = FxSystem::new();
        for x in [50.0f32, 5.0, 20.0] {
            s.spawn_effect_for_test(
                Effect {
                    emitters: vec![e.clone()],
                },
                SpawnAt::Point {
                    pos: Vec3::new(x, 0.0, 0.0),
                },
                0.0,
            );
        }
        let lights = s.lights(Vec3::ZERO, 0.0);
        let xs: Vec<f32> = lights.iter().map(|l| l.pos[0]).collect();
        assert_eq!(xs, vec![5.0, 20.0, 50.0]);
    }

    /// Retail's scene queue keeps the first 32 lights added, near or far.
    #[test]
    fn lights_keep_the_first_32_spawned() {
        let mut e = simple_emitter();
        e.kind = "Light".into();
        let mut s = FxSystem::new();
        for i in 0..40 {
            s.spawn_effect_for_test(
                Effect {
                    emitters: vec![e.clone()],
                },
                SpawnAt::Point {
                    pos: Vec3::new(1000.0 - i as f32, 0.0, 0.0),
                },
                0.0,
            );
        }
        let lights = s.lights(Vec3::ZERO, 0.0);
        assert_eq!(lights.len(), MAX_SCENE_LIGHTS);
        assert_eq!(lights[0].pos[0], 1000.0 - 31.0);
    }

    /// Syscall 0xe4's `MakeNormalVectors` and 0xe3's fixed axis: a forward
    /// along +x gets right -y and up +z, and no direction plays forward up,
    /// right +x, up +y.
    #[test]
    fn spawn_frames_are_retails_axes() {
        let (f, r, u) = tangent_basis(Vec3::X);
        assert_eq!((f, r, u), (Vec3::X, Vec3::NEG_Y, Vec3::Z));
        let d = Vec3::new(0.6, 0.0, 0.8);
        let (_, r, u) = tangent_basis(d);
        // (z, -x, y) = (0.8, -0.6, 0) less its 0.48 along d, normalized.
        let want = (Vec3::new(0.8, -0.6, 0.0) - d * 0.48).normalize();
        assert!((r - want).length() < 1e-6, "{r:?}");
        assert!((u - r.cross(d)).length() < 1e-6);
        assert!(r.dot(d).abs() < 1e-6 && u.dot(d).abs() < 1e-6);
        let point = spawn_frame(SpawnAt::Point { pos: Vec3::ZERO });
        assert_eq!(point, Some((Vec3::Z, Vec3::X, Vec3::Y)));
    }

    #[test]
    fn directed_spawn_rotates_velocity_into_dir_frame() {
        let mut e = simple_emitter();
        e.spawn_flags.clear();
        e.velocity = R3 {
            a: [1.0, 0.0, 0.0],
            b: [1.0, 0.0, 0.0],
        };
        let s = sys_with(
            e,
            SpawnAt::Directed {
                pos: Vec3::ZERO,
                dir: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        let p = &s.particles[0];
        assert!((p.vel - Vec3::Z).length() < 1e-4, "{:?}", p.vel);
    }

    #[test]
    fn directed_spawn_rotates_origin_into_dir_frame() {
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.origin = R3 {
            a: [10.0, 0.0, 0.0],
            b: [10.0, 0.0, 0.0],
        };
        let s = sys_with(
            e,
            SpawnAt::Directed {
                pos: Vec3::ZERO,
                dir: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        let p = &s.particles[0];
        assert!((p.pos - Vec3::Z * 10.0).length() < 1e-4, "{:?}", p.pos);
    }

    #[test]
    fn cheap_org_calc_keeps_origin_offset_in_world_space() {
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.origin = R3 {
            a: [10.0, 0.0, 0.0],
            b: [10.0, 0.0, 0.0],
        };
        e.spawn_flags = vec!["cheapOrgCalc".into()];
        let s = sys_with(
            e,
            SpawnAt::Directed {
                pos: Vec3::ZERO,
                dir: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        let p = &s.particles[0];
        assert!((p.pos - Vec3::X * 10.0).length() < 1e-4, "{:?}", p.pos);
    }

    #[test]
    fn accel_integrates_in_retails_frame_order() {
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0, 0.0, 0.0],
            b: [0.0, 0.0, 0.0],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.accel = R3 {
            a: [5.0, 0.0, 0.0],
            b: [5.0, 0.0, 0.0],
        };
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.5, 0.5, None);
        s.step(0.5, 1.0, None);
        // v 2.5 then x 1.25 + 0.625; v 5 then x += 2.5 + 0.625: 5, not
        // the closed form's 2.5 (CoDMP.exe 0x48df20).
        let q = s.build_quads(Vec3::new(100.0, 0.0, 0.0), Vec3::Y, Vec3::Z, 1.0);
        let c = quad_center(&q[0]);
        assert!((c.x - 5.0).abs() < 1e-4, "{c:?}");
    }

    #[test]
    fn stretch_quad_axis_is_parallel_to_velocity_with_curve_length() {
        let mut e = simple_emitter();
        e.kind = "Tail".into();
        e.velocity = R3 {
            a: [0.0, 10.0, 0.0],
            b: [0.0, 10.0, 0.0],
        };
        e.length = Curve {
            start: R1 { a: 6.0, b: 6.0 },
            end: R1 { a: 12.0, b: 12.0 },
            flags: vec!["linear".into()],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        // t = 0.5 (life is 1s, spawn at 0): length(t) = 6 + (12-6)*0.5 = 9
        let q = s.build_quads(Vec3::new(100.0, 0.0, 0.0), Vec3::X, Vec3::Z, 0.5);
        assert_eq!(q.len(), 1);
        let edge = Vec3::from(q[0].verts[1]) - Vec3::from(q[0].verts[0]);
        // Long edge parallel to velocity, exactly length(t) (grammar doc R5).
        assert!((edge.normalize() - Vec3::Y).length() < 1e-4, "{edge:?}");
        assert!((edge.length() - 9.0).abs() < 1e-3, "{edge:?}");
    }

    #[test]
    fn stretch_quad_is_anchored_at_the_head_and_trails_backwards() {
        let mut e = simple_emitter();
        e.kind = "Tail".into();
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.velocity = R3 {
            a: [0.0, 10.0, 0.0],
            b: [0.0, 10.0, 0.0],
        };
        e.length = Curve {
            start: R1 { a: 8.0, b: 8.0 },
            end: R1 { a: 8.0, b: 8.0 },
            flags: vec![],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        let q = s.build_quads(Vec3::new(100.0, 0.0, 0.0), Vec3::X, Vec3::Z, 0.0);
        assert_eq!(q.len(), 1);
        let ys: Vec<f32> = q[0].verts.iter().map(|v| v[1]).collect();
        let max = ys.iter().cloned().fold(f32::MIN, f32::max);
        let min = ys.iter().cloned().fold(f32::MAX, f32::min);
        // Head sits exactly on the particle (y = 0), tail 8 units behind it.
        assert!(max.abs() < 1e-4, "{ys:?}");
        assert!((min + 8.0).abs() < 1e-4, "{ys:?}");
    }

    /// `fx/muzzleflashes/heavy.efx` carries a `Particle` with `length 12 ->
    /// 275`; retail evaluates `length` for tails only (grammar doc R3), so
    /// the sprite stays `2 * size` square instead of trailing back down the
    /// barrel.
    #[test]
    fn length_curve_on_a_particle_does_not_stretch_it() {
        let mut e = simple_emitter();
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.length = Curve {
            start: R1 { a: 12.0, b: 12.0 },
            end: R1 { a: 275.0, b: 275.0 },
            flags: vec!["linear".into()],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        let q = s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, 0.5);
        assert_eq!(q.len(), 1);
        let w = (Vec3::from(q[0].verts[1]) - Vec3::from(q[0].verts[0])).length();
        let h = (Vec3::from(q[0].verts[2]) - Vec3::from(q[0].verts[1])).length();
        // size at t = 0.5 is 3, so 6 across both ways.
        assert!((w - 6.0).abs() < 1e-3, "{w}");
        assert!((h - 6.0).abs() < 1e-3, "{h}");
    }

    #[test]
    fn curve_without_linear_flag_holds_its_start_value() {
        let mut e = simple_emitter();
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.size = Curve {
            start: R1 { a: 3.0, b: 3.0 },
            end: R1 { a: 40.0, b: 40.0 },
            flags: vec![],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        for now in [0.0f32, 0.5, 0.99] {
            let q = s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, now);
            assert_eq!(q.len(), 1);
            assert!((quad_half_x(&q[0]) - 3.0).abs() < 1e-4, "{now}");
        }
    }

    #[test]
    fn sprite_size_is_a_half_extent() {
        let mut e = simple_emitter();
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.velocity = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        e.size = Curve {
            start: R1 { a: 3.0, b: 3.0 },
            end: R1 { a: 3.0, b: 3.0 },
            flags: vec![],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        let q = s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, 0.0);
        assert_eq!(q.len(), 1);
        let width = (Vec3::from(q[0].verts[1]) - Vec3::from(q[0].verts[0])).length();
        let height = (Vec3::from(q[0].verts[2]) - Vec3::from(q[0].verts[1])).length();
        assert!((width - 6.0).abs() < 1e-4, "{width}");
        assert!((height - 6.0).abs() < 1e-4, "{height}");
    }

    /// Retries past the 0.4 chance roll; false if 50 rolls all failed.
    fn spawn_tracer_retrying(
        s: &mut FxSystem,
        impact: Vec3,
        flesh: bool,
        now: f32,
        frame_dt: f32,
    ) -> bool {
        (0..50).any(|_| s.spawn_tracer((Vec3::ZERO, impact), flesh, (false, true), now, frame_dt))
    }

    /// The one tracer's (tail, head) x along a +x shot, and its width.
    fn tracer_extent(s: &FxSystem, now: f32) -> Option<(f32, f32, f32)> {
        let q = s.build_quads(Vec3::new(0.0, -500.0, 0.0), Vec3::X, Vec3::Z, now);
        let q = q.first()?;
        assert_eq!(&*q.shader, "gfx/misc/tracer");
        let xs = q.verts.map(|v| v[0]);
        let width = (Vec3::from(q.verts[2]) - Vec3::from(q.verts[1])).length();
        Some((
            xs.iter().cloned().fold(f32::MAX, f32::min),
            xs.iter().cloned().fold(f32::MIN, f32::max),
            width,
        ))
    }

    #[test]
    fn tracer_tail_leaves_the_muzzle_and_the_head_leads_by_its_length() {
        let mut s = FxSystem::new();
        let impact = Vec3::new(4000.0, 0.0, 0.0);
        assert!(spawn_tracer_retrying(&mut s, impact, false, 0.0, 0.0));
        let (tail, head, width) = tracer_extent(&s, 0.0).unwrap();
        assert!(tail.abs() < 1e-3, "tail {tail} must sit on the muzzle");
        assert!((head - 160.0).abs() < 1e-3, "{head}");
        assert!((width - 1.6).abs() < 1e-3, "{width}");
        // p(t) = muzzle + 4500 * t along the shot, head = p + 160.
        let mut now = 0.0;
        for _ in 0..40 {
            s.step(0.02, now + 0.02, None);
            now += 0.02;
            let Some((tail, head, _)) = tracer_extent(&s, now) else {
                break;
            };
            assert!((tail - 4500.0 * now).abs() < 1e-2, "{now}: {tail}");
            assert!((head - tail - 160.0).abs() < 1e-2, "{now}: {head}");
            assert!(head <= impact.x + 1e-2, "{now}: {head} past the impact");
        }
    }

    #[test]
    fn tracer_lives_until_its_tail_reaches_length_short_of_the_impact() {
        let mut s = FxSystem::new();
        // (4000 - 160) / 4500 s = 853.33 ms, truncated to 853.
        assert!(spawn_tracer_retrying(
            &mut s,
            Vec3::new(4000.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        s.step(0.853, 0.853, None);
        let (_, head, _) = tracer_extent(&s, 0.853).unwrap();
        assert!((head - (0.853 * 4500.0 + 160.0)).abs() < 1e-1, "{head}");
        s.step(0.002, 0.855, None);
        assert!(tracer_extent(&s, 0.855).is_none());
    }

    #[test]
    fn tracer_backdate_moves_the_tail_ahead_never_behind() {
        let mut s = FxSystem::new();
        let impact = Vec3::new(4000.0, 0.0, 0.0);
        for i in 0..200 {
            s.clear();
            let now = i as f32;
            assert!(spawn_tracer_retrying(&mut s, impact, false, now, 0.016));
            let (tail, head, _) = tracer_extent(&s, now).unwrap();
            // `(rand() % 16) / 2` ms, so at most 7 ms of travel.
            assert!((0.0..=4500.0 * 0.007 + 1e-2).contains(&tail), "{tail}");
            assert!((head - tail - 160.0).abs() < 1e-2);
        }
    }

    #[test]
    fn tracer_short_of_its_length_still_starts_at_the_muzzle() {
        // Retail's moving branch has no distance gate.
        let mut s = FxSystem::new();
        assert!(spawn_tracer_retrying(
            &mut s,
            Vec3::new(80.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        let (tail, head, _) = tracer_extent(&s, 0.0).unwrap();
        assert!(
            tail.abs() < 1e-3 && (head - 160.0).abs() < 1e-3,
            "{tail} {head}"
        );
    }

    /// United Offensive draws no moving streak for a shot shorter than the
    /// tracer (0x300482ff), and under `sv_night` every tracer is five
    /// times as wide; the flesh segment keeps its own gate.
    #[test]
    fn uo_tracers_need_their_length_and_widen_at_night() {
        let mut s = FxSystem::new();
        s.set_tracer_rules(TracerRules {
            min_length: true,
            night: false,
        });
        assert!(!spawn_tracer_retrying(
            &mut s,
            Vec3::new(159.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        assert_eq!(s.counts().0, 0);
        assert!(spawn_tracer_retrying(
            &mut s,
            Vec3::new(160.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        assert!(!s.spawn_tracer(
            (Vec3::ZERO, Vec3::new(219.0, 0.0, 0.0)),
            false,
            (true, false),
            0.0,
            0.0
        ));
        assert!(spawn_tracer_retrying(
            &mut s,
            Vec3::new(150.0, 0.0, 0.0),
            true,
            0.0,
            0.0
        ));
        s.clear();
        s.set_tracer_rules(TracerRules {
            min_length: true,
            night: true,
        });
        assert!(spawn_tracer_retrying(
            &mut s,
            Vec3::new(4000.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        let (_, _, width) = tracer_extent(&s, 0.0).unwrap();
        assert!((width - 8.0).abs() < 1e-3, "{width}");
    }

    #[test]
    fn flesh_tracer_is_a_one_frame_segment_inside_the_shot() {
        let mut s = FxSystem::new();
        for i in 0..100 {
            s.clear();
            let now = i as f32;
            assert!(spawn_tracer_retrying(
                &mut s,
                Vec3::new(1000.0, 0.0, 0.0),
                true,
                now,
                0.016
            ));
            let (near, far, _) = tracer_extent(&s, now).unwrap();
            // near in [50, 1000 - 10), far = min(near + 160, 1000).
            assert!((50.0..990.0).contains(&near), "{near}");
            assert!(
                (far - (near + 160.0).min(1000.0)).abs() < 1e-2,
                "{near} {far}"
            );
            s.step(0.016, now + 0.016, None);
            assert!(tracer_extent(&s, now + 0.016).is_none());
        }
    }

    #[test]
    fn flesh_tracer_is_skipped_for_short_shots() {
        let mut s = FxSystem::new();
        assert!(!spawn_tracer_retrying(
            &mut s,
            Vec3::new(80.0, 0.0, 0.0),
            true,
            0.0,
            0.0
        ));
        assert_eq!(s.counts().0, 0);
    }

    #[test]
    fn clear_drops_particles_and_marks_but_keeps_the_cache() {
        let mut fx = FxSystem::new();
        assert!(spawn_tracer_retrying(
            &mut fx,
            Vec3::new(4000.0, 0.0, 0.0),
            false,
            0.0,
            0.0
        ));
        fx.cache.insert("x".into(), None);
        fx.clear();
        assert_eq!(fx.counts(), (0, 0, 0));
        assert!(fx.cache.contains_key("x"));
    }

    #[test]
    fn stretch_quad_falls_back_to_cam_up_when_velocity_is_parallel_to_view_dir() {
        let mut e = simple_emitter();
        e.kind = "Tail".into();
        e.velocity = R3 {
            a: [0.0, 0.0, 10.0],
            b: [0.0, 0.0, 10.0],
        };
        e.length = Curve {
            start: R1 { a: 6.0, b: 6.0 },
            end: R1 { a: 6.0, b: 6.0 },
            flags: vec![],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        // Camera along the velocity axis degenerates the primary cross product.
        let q = s.build_quads(Vec3::new(0.0, 0.0, 100.0), Vec3::X, Vec3::Y, 0.0);
        assert_eq!(q.len(), 1);
        let side_edge = Vec3::from(q[0].verts[2]) - Vec3::from(q[0].verts[1]);
        // size.start is 2; the side edge is 2 * size, not collapsed.
        assert!((side_edge.length() - 4.0).abs() < 1e-3, "{side_edge:?}");
    }

    #[test]
    fn life_zero_emitter_does_not_panic_and_produces_no_quads() {
        // Spawn, step and build at the same `now`, as main.rs does.
        let mut e = simple_emitter();
        e.life = R1 { a: 0.0, b: 0.0 };
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.0, 0.0, None);
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).is_empty());
        assert_eq!(s.counts(), (0, 0, 0));
    }

    #[test]
    fn directed_spawn_with_absolute_vel_keeps_world_space_velocity() {
        // absoluteVel is a spawnFlags token, not a flags token.
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [1.0, 0.0, 0.0],
            b: [1.0, 0.0, 0.0],
        };
        e.spawn_flags = vec!["absoluteVel".into()];
        let s = sys_with(
            e,
            SpawnAt::Directed {
                pos: Vec3::ZERO,
                dir: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        let p = &s.particles[0];
        assert!((p.vel - Vec3::X).length() < 1e-4, "{:?}", p.vel);
    }

    #[test]
    fn surface_spawn_rotates_non_decal_velocity_into_normal_hemisphere() {
        let mut e = simple_emitter();
        e.spawn_flags.clear();
        e.velocity = R3 {
            a: [1.0, 0.0, 0.0],
            b: [1.0, 0.0, 0.0],
        };
        let s = sys_with(
            e,
            SpawnAt::Surface {
                pos: Vec3::ZERO,
                normal: Vec3::Z,
            },
        );
        let p = &s.particles[0];
        assert!(p.vel.dot(Vec3::Z) > 0.0, "{:?}", p.vel);
        assert!((p.vel - Vec3::Z).length() < 1e-4, "{:?}", p.vel);
    }

    #[test]
    fn org_on_sphere_distributes_spawn_positions_on_a_sphere() {
        let mut e = simple_emitter();
        e.count = R1 { a: 8.0, b: 8.0 };
        e.spawn_flags = vec!["orgOnSphere".into()];
        e.radius = R1 { a: 50.0, b: 50.0 };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        assert_eq!(s.counts().0, 8);
        let positions: Vec<Vec3> = s.particles.iter().map(|p| p.pos).collect();
        assert!(positions.windows(2).any(|w| (w[0] - w[1]).length() > 1.0));
        for p in &positions {
            assert!(p.length() <= 50.0 + 1e-3, "{p:?}");
        }
    }

    /// fx/impacts/default_hit.efx's debris: `orgOnSphere axisFromSphere`,
    /// `velocity 700 0 0`.
    #[test]
    fn axis_from_sphere_points_velocity_radially_outward() {
        let mut e = simple_emitter();
        e.count = R1 { a: 8.0, b: 8.0 };
        e.spawn_flags = vec!["orgOnSphere".into(), "axisFromSphere".into()];
        e.radius = R1 { a: 50.0, b: 50.0 };
        e.velocity = R3 {
            a: [700.0, 0.0, 0.0],
            b: [700.0, 0.0, 0.0],
        };
        let s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        assert_eq!(s.counts().0, 8);
        for p in &s.particles {
            let radial = p.pos.normalize_or_zero(); // center is the origin
            assert!((p.vel.length() - 700.0).abs() < 1e-2, "{:?}", p.vel);
            assert!(
                (p.vel.normalize_or_zero() - radial).length() < 1e-3,
                "vel={:?} pos={:?}",
                p.vel,
                p.pos
            );
        }
    }

    /// The level preload reads every effect the stock events and weapons
    /// play, and what they chain to: spawning each of them afterwards, and
    /// stepping until their chains ran, reads no file (an empty search
    /// path would cache a miss).
    #[test]
    fn a_preloaded_level_plays_its_effects_without_reading_a_file() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        crate::fx::registry::init(&fs);
        let p = vcod_common::net::protocol::PROTOCOL_V1;
        let weapons: Vec<_> = [
            "kar98k_mp",
            "thompson_mp",
            "fraggrenade_mp",
            "panzerfaust_mp",
        ]
        .iter()
        .map(|w| vcod_common::weapon::load(&fs, w).ok())
        .collect();
        let paths = crate::fx::registry::level_effects(&[], &p.cs, &weapons, false);
        assert!(paths.len() > 20, "{paths:?}");
        let mut s = FxSystem::new();
        let (marks, sprites, _) = s.preload_cod1(&fs, paths.iter().map(String::as_str));
        assert!(!marks.is_empty() && !sprites.is_empty());
        let cached: HashSet<String> = s.cache.keys().cloned().collect();
        let empty = Pk3Fs::empty();
        let at = SpawnAt::Surface {
            pos: Vec3::ZERO,
            normal: Vec3::Z,
        };
        for (i, path) in paths.iter().enumerate() {
            s.spawn(&empty, path, at, i as f32);
        }
        let mut now = paths.len() as f32;
        for _ in 0..200 {
            now += 0.05;
            s.step(0.05, now, None);
            s.spawn_chained(&empty, now);
        }
        let read: Vec<_> = s.cache.keys().filter(|k| !cached.contains(*k)).collect();
        assert!(read.is_empty(), "read in play: {read:?}");
    }

    #[test]
    fn spawn_loads_and_caches_a_real_effect_file_and_warns_once_on_missing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut s = FxSystem::new();
        s.spawn(
            &fs,
            "fx/impacts/default_hit.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        let (particles, marks, _lights) = s.counts();
        assert!(particles + marks > 0);
        // Warn-once is exercised, not asserted.
        s.spawn(
            &fs,
            "fx/does/not/exist.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        s.spawn(
            &fs,
            "fx/does/not/exist.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        assert_eq!(s.counts(), (particles, marks, _lights));
    }

    #[test]
    fn physics_particle_bounces_off_the_floor_and_damps() {
        let world = vcod_common::collision::test_world(&[]);
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0, 0.0, -50.0],
            b: [0.0, 0.0, -50.0],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.flags = vec!["usePhysics".into()];
        // floor top face is at z=0 (see collision::test_world)
        let mut s = sys_with(
            e,
            SpawnAt::Point {
                pos: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        s.step(0.1, 0.1, Some(&world));
        let q = s.build_quads(Vec3::new(0.0, 0.0, 100.0), Vec3::X, Vec3::Y, 0.1);
        assert_eq!(q.len(), 1);
        let c = quad_center(&q[0]);
        // stopped just above the floor, not fallen through it
        assert!(c.z > 0.0 && c.z < 1.0, "{c:?}");
    }

    #[test]
    fn physics_particle_with_impact_kills_is_dropped_on_collision() {
        let world = vcod_common::collision::test_world(&[]);
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0, 0.0, -50.0],
            b: [0.0, 0.0, -50.0],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.flags = vec!["usePhysics".into(), "impactKills".into()];
        let mut s = sys_with(
            e,
            SpawnAt::Point {
                pos: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        s.step(0.1, 0.1, Some(&world));
        assert_eq!(s.counts().0, 0);
    }

    /// Retail tests `cullrange` once, when the effect plays (0x493a1f).
    #[test]
    fn cullrange_is_tested_at_spawn_only_and_zero_never_culls() {
        let far = SpawnAt::Point {
            pos: Vec3::new(1000.0, 0.0, 0.0),
        };
        let mut e = simple_emitter();
        e.cullrange = 10.0;
        let spawn_seen_from = |view: Vec3, e: Emitter| {
            let mut s = FxSystem::new();
            s.set_view(view);
            s.spawn_effect_for_test(Effect { emitters: vec![e] }, far, 0.0);
            s
        };
        let s = spawn_seen_from(Vec3::ZERO, e.clone());
        assert_eq!(s.counts().0, 0);
        // Spawned in range, it draws from anywhere afterwards.
        let s = spawn_seen_from(Vec3::new(995.0, 0.0, 0.0), e);
        assert_eq!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).len(), 1);
        let s = spawn_seen_from(Vec3::ZERO, simple_emitter());
        assert_eq!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).len(), 1);
    }

    /// `FX_CullSphere` against `FX_SetNextUpdateCamera`'s planes: a
    /// sphere is culled once it lies wholly behind the eye's forward plane
    /// or a side.
    #[test]
    fn frustum_culls_a_sphere_wholly_outside_one_plane() {
        // along +x, 90 degrees each way
        let f = Frustum::new(Vec3::ZERO, (Vec3::X, Vec3::Y, Vec3::Z), 1.0, 1.0);
        assert!(!f.culls(Vec3::new(100.0, 0.0, 0.0), 0.0));
        assert!(f.culls(Vec3::new(-100.0, 0.0, 0.0), 50.0));
        assert!(!f.culls(Vec3::new(-100.0, 0.0, 0.0), 150.0));
        // 35.4 past the 45 degree side, either way
        for p in [Vec3::new(100.0, 150.0, 0.0), Vec3::new(100.0, -150.0, 0.0)] {
            assert!(f.culls(p, 30.0) && !f.culls(p, 40.0), "{p}");
        }
        assert!(f.culls(Vec3::new(100.0, 0.0, -150.0), 30.0));
    }

    /// `fx/shellejects/pistol.efx` from `pak5.pk3`, verbatim but for the
    /// line endings.
    const PISTOL_BRASS: &str = "Emitter\n{\n\tflags\t\t\t\tuseModel usePhysics\n\tlife\t\t\t\t5000\n\tcullrange\t\t\t100\n\tbounce\t\t\t\t0.5\n\tangle\t\t\t\t360 360 360 -360 -360 -360\n\tangleDelta\t\t\t133 133 133 -133 -133 -133\n\tvelocity\t\t\t100 0 0\n\tacceleration\t\t132 132 132 -132 -132 -132\n\tgravity\t\t\t\t-800\n\tsize\n\t{\n\t\tstart\t\t\t0.09\n\t\tend\t\t\t\t0.09\n\t\tflags\t\t\tlinear\n\t}\n\tmodels\n\t[\n\t\txmodel/shellcasing\n\t]\n}\n\nEmitter\n{\n\tname\t\t\t\tCopy of Unnamed Emitter 0\n\tflags\t\t\t\tusePhysics impactKills impactFx\n\tlife\t\t\t\t5000\n\tbounce\t\t\t\t0.6\n\tangle\t\t\t\t360 360 360 -360 -360 -360\n\tangleDelta\t\t\t133 133 133 -133 -133 -133\n\tvelocity\t\t\t100 0 0\n\tgravity\t\t\t\t-800\n\tsize\n\t{\n\t\tstart\t\t\t0.09\n\t\tend\t\t\t\t0.09\n\t\tflags\t\t\tlinear\n\t}\n\timpactfx\n\t[\n\t\tfx/shellejects/shelleject_pistolsound\n\t]\n}\n";

    fn brass_at(pos: Vec3, dir: Vec3) -> FxSystem {
        let mut s = FxSystem::new();
        s.set_view(pos);
        s.spawn_effect_for_test(
            efx::parse(PISTOL_BRASS).unwrap(),
            SpawnAt::Directed { pos, dir },
            0.0,
        );
        s
    }

    #[test]
    fn brass_flies_one_case_model_and_one_unseen_sound_particle() {
        let s = brass_at(Vec3::new(0.0, 0.0, 40.0), Vec3::Y);
        assert_eq!(s.counts(), (2, 0, 0));
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).is_empty());
        let models = s.models(0.0);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "xmodel/shellcasing");
        for a in models[0].axis {
            assert!((a.length() - 0.09).abs() < 1e-5, "{a:?}");
        }
    }

    #[test]
    fn brass_leaves_along_the_tag_forward_and_falls() {
        let mut s = brass_at(Vec3::new(0.0, 0.0, 40.0), Vec3::Y);
        s.step(0.05, 0.05, None);
        let p = s.models(0.05)[0].origin;
        // 100 u/s out along +Y, give or take the +-132 random acceleration.
        assert!(p.y > 4.0 && p.y < 6.0 && p.x.abs() < 0.5, "{p:?}");
        assert!(p.z < 40.0, "{p:?}");
    }

    #[test]
    fn brass_bounces_then_rests_on_the_floor_and_its_spin_dies_away() {
        let world = vcod_common::collision::test_world(&[]);
        let mut s = brass_at(Vec3::new(0.0, 0.0, 20.0), Vec3::X);
        let mut now = 0.0;
        let mut top_after_first_hit: Option<f32> = None;
        let mut hit = false;
        for _ in 0..200 {
            now += 0.01;
            s.step(0.01, now, Some(&world));
            let z = s.models(now)[0].origin.z;
            if z < 1.0 {
                hit = true;
            }
            if hit {
                top_after_first_hit = Some(top_after_first_hit.map_or(z, |t: f32| t.max(z)));
            }
        }
        // It bounced back up off the floor, below where it started.
        let top = top_after_first_hit.expect("reached the floor");
        assert!(top > 1.0 && top < 20.0, "{top}");
        let p = &s.particles.iter().find(|p| p.debris.is_some()).unwrap();
        assert_eq!(p.vel, Vec3::ZERO);
        assert!(!p.physics, "a resting case stops tracing");
        assert!(p.pos.z >= 0.0 && p.pos.z < 0.5, "{:?}", p.pos);
        let spin = p.debris.as_ref().unwrap().angle_delta.length();
        assert!(spin < 1e-3, "{spin}");
    }

    #[test]
    fn brass_sound_particle_queues_its_impact_effect_and_dies() {
        let world = vcod_common::collision::test_world(&[]);
        let mut s = brass_at(Vec3::new(0.0, 0.0, 10.0), Vec3::X);
        let mut now = 0.0;
        while s.chains.is_empty() && now < 2.0 {
            now += 0.01;
            s.step(0.01, now, Some(&world));
        }
        assert_eq!(s.chains.len(), 1);
        let Chain { path, at, .. } = &s.chains[0];
        assert_eq!(path, "fx/shellejects/shelleject_pistolsound.efx");
        match at {
            SpawnAt::Surface { pos, normal } => {
                assert!(pos.z.abs() < 0.5, "{pos:?}");
                assert_eq!(*normal, Vec3::Z);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            s.counts().0,
            1,
            "the case flies on, the sound particle is gone"
        );
    }

    #[test]
    fn brass_past_its_cullrange_from_the_view_spawns_nothing() {
        let mut s = FxSystem::new();
        s.set_view(Vec3::new(500.0, 0.0, 0.0));
        s.spawn_effect_for_test(
            efx::parse(PISTOL_BRASS).unwrap(),
            SpawnAt::Directed {
                pos: Vec3::ZERO,
                dir: Vec3::X,
            },
            0.0,
        );
        // The model element culls at 100; the sound element has no cullrange.
        assert_eq!(s.counts().0, 1);
        assert!(s.models(0.0).is_empty());
    }

    // Chaining (grammar doc R10).

    /// A system whose cache holds `effects` by path, as if read from a pak.
    fn sys_with_files(effects: &[(&str, Vec<Emitter>)]) -> FxSystem {
        let mut s = FxSystem::new();
        for (path, emitters) in effects {
            s.cache.insert(
                path.to_string(),
                Some(Effect {
                    emitters: emitters.clone(),
                }),
            );
        }
        s
    }

    #[allow(clippy::field_reassign_with_default)]
    fn runner(play: &str) -> Emitter {
        let mut e = Emitter::default();
        e.kind = "FxRunner".into();
        e.play_fx = vec![play.into()];
        e
    }

    fn still_particle() -> Emitter {
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e
    }

    fn centers(s: &FxSystem, now: f32) -> Vec<Vec3> {
        s.build_quads(Vec3::new(0.0, 0.0, 1000.0), Vec3::X, Vec3::Y, now)
            .iter()
            .map(quad_center)
            .collect()
    }

    #[test]
    fn runner_plays_its_playfx_count_times_at_its_origin_in_the_spawn_frame() {
        let mut r = runner("fx/child");
        r.count = R1 { a: 3.0, b: 3.0 };
        r.origin = R3 {
            a: [10.0, 0.0, 0.0],
            b: [10.0, 0.0, 0.0],
        };
        let mut s = sys_with_files(&[
            ("fx/parent.efx", vec![r]),
            ("fx/child.efx", vec![still_particle()]),
        ]);
        let at = SpawnAt::Directed {
            pos: Vec3::new(1.0, 2.0, 3.0),
            dir: Vec3::Y,
        };
        s.spawn(&Pk3Fs::empty(), "fx/parent.efx", at, 0.0);
        let c = centers(&s, 0.0);
        assert_eq!(c.len(), 3);
        for p in c {
            assert!((p - Vec3::new(1.0, 12.0, 3.0)).length() < 1e-4, "{p:?}");
        }
    }

    #[test]
    fn runner_delay_holds_its_effect_until_due() {
        let mut r = runner("fx/child");
        r.delay = R1 { a: 500.0, b: 500.0 };
        let mut s = sys_with_files(&[
            ("fx/parent.efx", vec![r]),
            ("fx/child.efx", vec![still_particle()]),
        ]);
        let fs = Pk3Fs::empty();
        s.spawn(
            &fs,
            "fx/parent.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        assert_eq!(s.counts().0, 0);
        s.spawn_chained(&fs, 0.4);
        assert_eq!(s.counts().0, 0);
        s.spawn_chained(&fs, 0.5);
        assert_eq!(s.counts().0, 1);
        assert!(s.chains.is_empty());
    }

    #[test]
    fn a_runner_past_its_cullrange_starts_nothing() {
        let mut r = runner("fx/child");
        r.cullrange = 100.0;
        let mut s = sys_with_files(&[
            ("fx/parent.efx", vec![r]),
            ("fx/child.efx", vec![still_particle()]),
        ]);
        s.set_view(Vec3::new(0.0, 200.0, 0.0));
        s.spawn(
            &Pk3Fs::empty(),
            "fx/parent.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        assert_eq!(s.counts().0, 0);
    }

    /// `fx/explosions/moltov_hit.efx` replays itself; vcod stops the chain.
    #[test]
    fn a_self_playing_effect_stops_at_the_chain_depth() {
        let mut s = sys_with_files(&[("fx/loop.efx", vec![runner("fx/loop"), still_particle()])]);
        s.spawn(
            &Pk3Fs::empty(),
            "fx/loop.efx",
            SpawnAt::Point { pos: Vec3::ZERO },
            0.0,
        );
        assert_eq!(s.counts().0, MAX_CHAIN_DEPTH as usize + 1);
        assert!(s.chains.is_empty());
    }

    /// An `Emitter` flying 200 u/s with `density 10` plays its `emitfx`
    /// about every 10 units: the 12 ms walk passes 10 units at 60 ms and
    /// the Newton step pulls the play back to 51 ms (10.2 units). The
    /// second play at 102 ms comes out of the 100 ms frame.
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn emitter_plays_its_emitfx_every_density_units_along_its_path() {
        let mut e = Emitter::default();
        e.kind = "Emitter".into();
        e.life = R1 { a: 100.0, b: 100.0 };
        e.velocity = R3 {
            a: [200.0, 0.0, 0.0],
            b: [200.0, 0.0, 0.0],
        };
        e.density = R1 { a: 10.0, b: 10.0 };
        e.emit_fx = vec!["fx/puff".into()];
        let mut s = sys_with_files(&[("fx/emit.efx", vec![e])]);
        let at = SpawnAt::Directed {
            pos: Vec3::ZERO,
            dir: Vec3::X,
        };
        s.spawn(&Pk3Fs::empty(), "fx/emit.efx", at, 0.0);
        let mut plays = Vec::new();
        for i in 1..=10 {
            let now = i as f32 * 0.01;
            s.step(0.01, now, None);
            for c in s.chains.drain(..) {
                assert_eq!(c.path, "fx/puff.efx");
                assert_eq!(c.depth, 1);
                plays.push((now, spawn_pos(c.at)));
            }
        }
        assert_eq!(plays.len(), 2, "{plays:?}");
        // The walk steps past `now` once: the 50 ms frame samples 60 ms.
        assert!((plays[0].0 - 0.05).abs() < 1e-4, "{plays:?}");
        assert!((plays[0].1 - Vec3::new(10.2, 0.0, 0.0)).length() < 1e-3);
        assert!((plays[1].1 - Vec3::new(20.4, 0.0, 0.0)).length() < 1e-3);
    }

    /// A CoD4 sprite carrying `effectEmitted` (no `Emitter` needed) flying
    /// 200 u/s plays it every `emitDist` (10) units it moved, along the
    /// move, the spacing carried from step to step (cod4-effects.md 15).
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn a_cod4_element_emits_every_emit_distance_it_moves() {
        let mut e = simple_emitter();
        e.gravity = R1 { a: 0.0, b: 0.0 };
        e.velocity = R3 {
            a: [200.0, 0.0, 0.0],
            b: [200.0, 0.0, 0.0],
        };
        e.emit_fx = vec!["fx/puff".into()];
        e.cod4_emit = Some(efx::Cod4Emit {
            dist: R1 { a: 10.0, b: 10.0 },
            variance_base: 0.0,
            variance_amp: 0.0,
        });
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        let mut plays = Vec::new();
        // 7 ms steps, 1.4 units each, to 98 units.
        for i in 1..=70 {
            let now = i as f32 * 0.007;
            s.step(0.007, now, None);
            for c in s.chains.drain(..) {
                assert_eq!(c.path, "fx/puff.efx");
                let SpawnAt::Directed { pos, dir } = c.at else {
                    panic!("{:?}", c.at);
                };
                assert!((dir - Vec3::X).length() < 1e-5);
                plays.push(pos.x);
            }
        }
        assert_eq!(plays.len(), 9, "{plays:?}");
        // The units carried between steps are kept in 256ths of a
        // spacing, as retail keeps them: each play lands a little short.
        for (k, x) in plays.iter().enumerate() {
            assert!((x - 10.0 * (k + 1) as f32).abs() < 0.5, "{plays:?}");
        }
        for w in plays.windows(2) {
            assert!((w[1] - w[0] - 10.0).abs() < 0.1, "{plays:?}");
        }
    }

    #[test]
    fn deathfx_plays_where_the_particle_expires() {
        let mut e = simple_emitter();
        e.life = R1 { a: 100.0, b: 100.0 };
        e.death_fx = vec!["fx/smoke/whitesmoke".into()];
        let mut s = sys_with(e, SpawnAt::Point { pos: Vec3::ZERO });
        s.step(0.1, 0.1, None);
        assert!(s.chains.is_empty());
        let last = s.particles[0].pos;
        s.step(0.01, 0.11, None);
        assert_eq!(s.counts().0, 0);
        assert_eq!(s.chains.len(), 1);
        let c = &s.chains[0];
        assert_eq!(c.path, "fx/smoke/whitesmoke.efx");
        assert_eq!(spawn_pos(c.at), last);
        assert!(matches!(c.at, SpawnAt::Directed { dir, .. } if (dir.length() - 1.0).abs() < 1e-4));
    }

    #[test]
    fn an_impact_kill_plays_no_deathfx() {
        let world = vcod_common::collision::test_world(&[]);
        let mut e = simple_emitter();
        e.velocity = R3 {
            a: [0.0, 0.0, -50.0],
            b: [0.0, 0.0, -50.0],
        };
        e.flags = vec!["usePhysics".into(), "impactKills".into()];
        e.death_fx = vec!["fx/x".into()];
        let mut s = sys_with(
            e,
            SpawnAt::Point {
                pos: Vec3::new(0.0, 0.0, 1.0),
            },
        );
        s.step(0.1, 0.1, Some(&world));
        assert_eq!(s.counts().0, 0);
        assert!(s.chains.is_empty());
    }

    /// A `relative` element of an effect played on a tag rides it; the
    /// others stay where they spawned.
    #[test]
    fn relative_elements_ride_their_tag_and_die_with_it() {
        let mut rel = still_particle();
        rel.flags = vec!["relative".into()];
        rel.origin = R3 {
            a: [5.0, 0.0, 0.0],
            b: [5.0, 0.0, 0.0],
        };
        let mut fixed = rel.clone();
        fixed.flags.clear();
        let mut s = sys_with_files(&[("fx/flash.efx", vec![rel, fixed])]);
        let mut bolts = HashMap::from([(7, (Vec3::ZERO, Vec3::X))]);
        s.set_bolts(&bolts);
        let at = SpawnAt::Directed {
            pos: Vec3::ZERO,
            dir: Vec3::X,
        };
        s.spawn_marked(&Pk3Fs::empty(), "fx/flash.efx", at, Some(7), 0.0, true);
        assert_eq!(s.counts().0, 2);
        bolts.insert(7, (Vec3::new(100.0, 0.0, 0.0), Vec3::Y));
        s.set_bolts(&bolts);
        s.step(0.01, 0.01, None);
        let mut c = centers(&s, 0.01);
        c.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert!((c[0] - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-4, "{c:?}");
        assert!((c[1] - Vec3::new(100.0, 5.0, 0.0)).length() < 1e-4, "{c:?}");
        s.set_bolts(&HashMap::new());
        s.step(0.01, 0.02, None);
        assert_eq!(s.counts().0, 1);
    }

    /// `playFxOnTag` plays on the tag's own axis (syscall 0xe5); its
    /// `relative` elements ride the tag while the caller poses it and die
    /// once it stops, and the rest stay where they spawned.
    #[test]
    fn an_effect_on_a_tag_plays_on_its_axis_and_rides_it() {
        let mut rel = still_particle();
        rel.flags = vec!["relative".into()];
        rel.origin = R3 {
            a: [5.0, 0.0, 0.0],
            b: [5.0, 0.0, 0.0],
        };
        let mut fixed = rel.clone();
        fixed.flags.clear();
        let mut s = sys_with_files(&[("fx/tag.efx", vec![rel, fixed])]);
        let tag = TagBolt {
            entity: 70,
            slot: 1,
        };
        // Forward +y, left -x, up +z.
        let axis = [Vec3::Y, -Vec3::X, Vec3::Z];
        let pose = TagPose {
            origin: Vec3::new(10.0, 0.0, 0.0),
            axis,
        };
        s.spawn_on_tag(&Pk3Fs::empty(), "fx/tag.efx", tag, pose, 0.0);
        assert_eq!(s.tag_bolts(), HashSet::from([tag]));
        for c in centers(&s, 0.0) {
            assert!((c - Vec3::new(10.0, 5.0, 0.0)).length() < 1e-4, "{c:?}");
        }
        let moved = TagPose {
            origin: Vec3::new(0.0, 100.0, 0.0),
            axis,
        };
        s.set_tag_bolts(&HashMap::from([(tag, moved)]));
        s.step(0.01, 0.01, None);
        let mut c = centers(&s, 0.01);
        c.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert!((c[0] - Vec3::new(10.0, 5.0, 0.0)).length() < 1e-4, "{c:?}");
        assert!((c[1] - Vec3::new(0.0, 105.0, 0.0)).length() < 1e-4, "{c:?}");
        s.set_tag_bolts(&HashMap::new());
        s.step(0.01, 0.02, None);
        assert_eq!(s.counts().0, 1);
        assert!(s.tag_bolts().is_empty());
    }

    /// A delayed element of an effect on a tag spawns on the tag as it
    /// stands when it falls due (0x493f40), not where the play found it;
    /// one whose tag no longer poses spawns nothing.
    #[test]
    fn a_delayed_element_on_a_tag_spawns_at_the_tag_when_due() {
        let mut late = still_particle();
        late.origin = R3 {
            a: [5.0, 0.0, 0.0],
            b: [5.0, 0.0, 0.0],
        };
        late.delay = R1 { a: 100.0, b: 100.0 };
        let late_clone = late.clone();
        let mut s = sys_with_files(&[("fx/late.efx", vec![late])]);
        let tag = TagBolt {
            entity: 70,
            slot: 1,
        };
        let pose = |origin: Vec3, axis| TagPose { origin, axis };
        let play = pose(Vec3::new(10.0, 0.0, 0.0), [Vec3::X, Vec3::Y, Vec3::Z]);
        s.spawn_on_tag(&Pk3Fs::empty(), "fx/late.efx", tag, play, 0.0);
        assert_eq!(s.tag_bolts(), HashSet::from([tag]));
        // Turned to face +y and moved before the element falls due.
        let due = pose(Vec3::new(0.0, 100.0, 0.0), [Vec3::Y, -Vec3::X, Vec3::Z]);
        s.set_tag_bolts(&HashMap::from([(tag, due)]));
        s.step(0.05, 0.05, None);
        assert!(centers(&s, 0.05).is_empty());
        s.step(0.06, 0.11, None);
        let c = centers(&s, 0.11);
        assert_eq!(c.len(), 1);
        assert!((c[0] - Vec3::new(0.0, 105.0, 0.0)).length() < 1e-4, "{c:?}");
        // Spawned, it no longer rides the tag.
        assert!(s.tag_bolts().is_empty());

        let mut s = sys_with_files(&[("fx/late.efx", vec![late_clone])]);
        s.spawn_on_tag(&Pk3Fs::empty(), "fx/late.efx", tag, play, 0.0);
        assert_eq!(s.counts().0, 1);
        s.set_tag_bolts(&HashMap::new());
        s.step(0.2, 0.2, None);
        assert_eq!(s.counts().0, 0);
    }

    #[test]
    fn a_relative_element_on_an_unposed_tag_is_not_created() {
        let mut rel = still_particle();
        rel.flags = vec!["relative".into()];
        let mut s = sys_with_files(&[("fx/flash.efx", vec![rel, still_particle()])]);
        let at = SpawnAt::Point { pos: Vec3::ZERO };
        s.spawn_marked(&Pk3Fs::empty(), "fx/flash.efx", at, Some(7), 0.0, true);
        assert_eq!(s.counts().0, 1);
    }

    /// `grenade2.efx`'s `FxRunner` plays `fx/impacts/blast_flash2` at
    /// once, and its `Emitter` plays `radstones1sml` along its path.
    #[test]
    fn retail_grenade_runs_its_playfx_and_emitfx() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let at = SpawnAt::Surface {
            pos: Vec3::new(0.0, 0.0, 1.0),
            normal: Vec3::Z,
        };
        let mut alone = FxSystem::new();
        alone.set_view(Vec3::ZERO);
        alone.spawn_effect_for_test(
            efx::parse(&String::from_utf8_lossy(
                &fs.read("fx/explosions/grenade2.efx").unwrap(),
            ))
            .unwrap(),
            at,
            0.0,
        );
        let mut s = FxSystem::new();
        s.set_view(Vec3::ZERO);
        s.spawn(&fs, "fx/explosions/grenade2.efx", at, 0.0);
        assert!(s.counts().0 > alone.counts().0, "the runner's flash");
        let mut emitted = 0;
        for i in 1..=10 {
            let now = i as f32 * 0.016;
            s.step(0.016, now, None);
            emitted += s.chains.len();
            s.spawn_chained(&fs, now);
        }
        assert!(emitted > 0);
    }

    /// A `nonUniformScale` sprite spans `size` across the view and `size2`
    /// up it, each on its own curve (CoDMP.exe 0x5101d0); without the flag
    /// the sprite is square on `size` whatever `size2` says (0x49f360).
    #[test]
    fn a_non_uniform_sprite_takes_size2_up_the_view() {
        let mut e = still_particle();
        e.size2 = Curve {
            start: R1 { a: 122.0, b: 122.0 },
            end: R1 { a: 2.0, b: 2.0 },
            flags: vec!["linear".into()],
        };
        let extents = |e: &Emitter| {
            let mut s = sys_with_files(&[("fx/flash.efx", vec![e.clone()])]);
            let at = SpawnAt::Point { pos: Vec3::ZERO };
            s.spawn(&Pk3Fs::empty(), "fx/flash.efx", at, 0.0);
            let q = &s.build_quads(Vec3::new(0.0, -100.0, 0.0), Vec3::X, Vec3::Z, 0.5)[0];
            let [a, _, c, _] = q.verts.map(Vec3::from);
            ((c.x - a.x).abs() / 2.0, (c.z - a.z).abs() / 2.0)
        };
        // Half-way through its life: size 2 -> 4, size2 122 -> 2.
        let (w, h) = extents(&e);
        assert!((w - 3.0).abs() < 1e-4 && (h - 3.0).abs() < 1e-4, "{w} {h}");
        e.non_uniform_scale = true;
        let (w, h) = extents(&e);
        assert!((w - 3.0).abs() < 1e-4 && (h - 62.0).abs() < 1e-3, "{w} {h}");
    }

    /// A UO `CameraShake` element plays nothing on screen and hands its
    /// shake over: `intensity` times 0.05, `life` ms, `radius`.
    #[test]
    fn a_camera_shake_element_becomes_a_shake() {
        let mut fs = Pk3Fs::empty();
        fs.overlay(
            "fx/test/shake.efx",
            b"CameraShake\n{\n\tlife 200\n\tbounce 1\n\tradius 2500\n\tintensity 6\n}\n".to_vec(),
        );
        let mut fx = FxSystem::new();
        let pos = Vec3::new(1.0, 2.0, 3.0);
        fx.spawn(&fs, "fx/test/shake.efx", SpawnAt::Point { pos }, 0.0);
        assert_eq!(
            fx.take_shakes(),
            [Shake {
                scale: 6.0 * 0.05,
                duration_ms: 200.0,
                radius: 2500.0,
                origin: pos,
            }]
        );
        assert!(fx.particles.is_empty());
        assert!(fx.take_shakes().is_empty());
    }

    #[test]
    fn a_rewind_frees_what_spawned_inside_it() {
        let mut fx = FxSystem::new();
        let effect = || {
            let mut em = simple_emitter();
            em.life = R1 {
                a: 5000.0,
                b: 5000.0,
            };
            Effect { emitters: vec![em] }
        };
        fx.spawn_effect_for_test(effect(), SpawnAt::Point { pos: Vec3::ZERO }, 1.0);
        fx.spawn_effect_for_test(effect(), SpawnAt::Point { pos: Vec3::ZERO }, 2.5);
        fx.rewind(1000.0);
        fx.step(0.0, 3.0, None);
        assert_eq!(fx.particles.len(), 1);
        assert_eq!(fx.particles[0].spawn, 1.0);
    }
}
