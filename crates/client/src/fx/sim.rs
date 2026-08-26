//! Particle simulation: instantiates a parsed `.efx` [`efx::Effect`],
//! steps it, and flattens it into [`FxQuad`]/[`FxLight`] data. No wgpu, no
//! wire knowledge. Engine semantics: `docs/research/efx-grammar.md`, R1 to R8.

use crate::fx::efx::{self, Effect, Emitter, R1, R3};
use glam::Vec3;
use std::collections::{HashMap, HashSet, VecDeque};
use vcod_common::collision::CollisionWorld;
use vcod_common::pk3::Pk3Fs;

pub const MAX_PARTICLES: usize = 2048;
pub const MAX_DECALS: usize = 256;
pub const MAX_LIGHTS: usize = 8;

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
/// Shorter shots draw no tracer: `ds:0x30069524` guard at 0x30039370.
const TRACER_MIN_DIST: f32 = 100.0;
/// Near-end bias along the shot: `ds:0x3006958c` at 0x300393a8.
const TRACER_HEAD_START: f32 = 50.0;
/// Registered in CG_RegisterGraphics @ 0x30020da0.
const TRACER_SHADER: &str = "gfx/misc/tracer";

/// Where an effect is planted; decides emitter orientation.
#[derive(Clone, Copy, Debug)]
pub enum SpawnAt {
    // No caller yet; for spawners with neither a normal nor a direction.
    #[cfg_attr(not(test), allow(dead_code))]
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
}

/// Verts in world space, ring order for the [0,1,2, 0,2,3] index pattern.
pub struct FxQuad {
    pub verts: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    pub rgba: [f32; 4],
    pub shader: String,
}

pub struct FxLight {
    pub pos: [f32; 3],
    pub radius: f32,
    pub rgb: [f32; 3],
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
    NoOp,
}

/// `OrientedParticle` and `Cylinder` draw as camera-facing sprites. `Line`'s
/// `origin2` is not parsed, so it stretches along velocity like `Tail`.
/// `Sound` is handled by [`spawn_sound_cues`]; `Emitter` (xmodel debris,
/// `emitfx`) and `FxRunner` (`playfx`) chaining are not implemented.
fn classify(kind: &str) -> Behavior {
    match kind {
        "Particle" | "OrientedParticle" | "Cylinder" => Behavior::Billboard,
        "Decal" => Behavior::Decal,
        "Tail" | "Line" => Behavior::Stretch,
        "Light" => Behavior::Light,
        _ => Behavior::NoOp,
    }
}

struct Particle {
    spawn: f32, // absolute seconds, after delay
    life: f32,  // seconds
    pos: Vec3,
    vel: Vec3,
    accel: Vec3,
    gravity: f32,
    rot: f32, // degrees
    rot_delta: f32,
    /// Half-extent at spawn and at death (grammar doc R4).
    size0: f32,
    size1: f32,
    len0: f32,
    len1: f32, // 0 = billboard, else stretch backwards along vel
    alpha0: f32,
    alpha1: f32,
    rgb0: Vec3,
    rgb1: Vec3,
    physics: bool,
    impact_kills: bool,
    surface: Option<Vec3>, // Some(normal) = surface quad
    shader: String,
    cullrange: f32, // 0 = never culled
    is_light: bool,
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
        SpawnAt::Point { pos } | SpawnAt::Surface { pos, .. } | SpawnAt::Directed { pos, .. } => {
            pos
        }
    }
}

/// Orthonormal `(axis, tangent, bitangent)`. The seed swaps on near-vertical
/// axes to avoid a degenerate cross product.
fn tangent_basis(axis: Vec3) -> (Vec3, Vec3, Vec3) {
    let seed = if axis.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let t = axis.cross(seed).normalize_or_zero();
    let b = axis.cross(t);
    (axis, t, b)
}

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

fn spawn_emitter(
    rng: &mut Rng,
    particles: &mut VecDeque<Particle>,
    decals: &mut VecDeque<Particle>,
    em: &Emitter,
    at: SpawnAt,
    now: f32,
) {
    let behavior = classify(&em.kind);
    if behavior == Behavior::NoOp {
        return;
    }
    // Clamped so a malformed count (1e9) does not spin the loop before
    // push_capped evicts.
    let count = (sample_r1(rng, em.count).round().max(0.0) as usize).min(MAX_PARTICLES);
    for _ in 0..count {
        let delay = sample_r1(rng, em.delay) / 1000.0;
        let life = sample_r1(rng, em.life) / 1000.0;
        let spawn = now + delay;
        if life <= 0.0 {
            // Some retail blocks have no `life` (fx/explosions/boom_dirt.efx);
            // the default 0 would make the age fraction 0/0.
            continue;
        }

        let mut vel = sample_r3(rng, em.velocity);
        let mut accel = sample_r3(rng, em.accel);
        // Emitter-frame vectors rotate onto `dir`/`normal` unless the
        // absoluteVel/absoluteAccel spawnFlags (not flags) keep them
        // world-space (grammar doc R6). Decals are oriented separately below.
        let frame_axis: Option<Vec3> = match at {
            SpawnAt::Directed { dir, .. } => Some(dir),
            SpawnAt::Surface { normal, .. } if behavior != Behavior::Decal => Some(normal),
            _ => None,
        };
        let frame = frame_axis.map(|axis| tangent_basis(axis.normalize_or_zero()));
        if let Some((x, y, z)) = frame {
            if !em.spawn_flags.iter().any(|f| f == "absoluteVel") {
                vel = vel.x * x + vel.y * y + vel.z * z;
            }
            if !em.spawn_flags.iter().any(|f| f == "absoluteAccel") {
                accel = accel.x * x + accel.y * y + accel.z * z;
            }
        }

        // `origin` rotates into the same frame unless cheapOrgCalc (R6).
        // evenDistribution is not implemented.
        let mut origin_offset = sample_r3(rng, em.origin);
        if let Some((x, y, z)) = frame {
            if !em.spawn_flags.iter().any(|f| f == "cheapOrgCalc") {
                origin_offset = origin_offset.x * x + origin_offset.y * y + origin_offset.z * z;
            }
        }
        let base = spawn_pos(at) + origin_offset;
        let center = base;
        let base = distribute_origin(rng, em, base);

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

        let (pos, surface) = if behavior == Behavior::Decal {
            let normal = match at {
                SpawnAt::Surface { normal, .. } => normal,
                SpawnAt::Directed { dir, .. } => dir,
                SpawnAt::Point { .. } => Vec3::Z,
            }
            .normalize_or_zero();
            // Keep a decal's velocity out of the surface it sits on.
            let d = vel.dot(normal);
            if d < 0.0 {
                vel -= 2.0 * d * normal;
            }
            const DECAL_NUDGE: f32 = 0.25;
            (base + normal * DECAL_NUDGE, Some(normal))
        } else {
            (base, None)
        };

        let gravity = sample_r1(rng, em.gravity);
        let rot = sample_r1(rng, em.rotation);
        let rot_delta = sample_r1(rng, em.rotation_delta);
        let (size0, size1) = sample_curve(rng, &em.size);
        // `length` is a tail-only field (grammar doc R3); sprites carrying
        // one (fx/muzzleflashes/heavy.efx) stay square.
        let (len0, len1) = match sample_curve(rng, &em.length) {
            l if behavior == Behavior::Stretch => l,
            _ => (0.0, 0.0),
        };
        let (alpha0, alpha1) = sample_curve(rng, &em.alpha);
        let (rgb0, rgb1) = sample_curve_v3(rng, &em.rgb);
        let physics = em.flags.iter().any(|f| f == "usePhysics");
        let impact_kills = em.flags.iter().any(|f| f == "impactKills");
        let shader = if em.shaders.is_empty() {
            String::new()
        } else {
            let idx =
                ((rng.next_f32() * em.shaders.len() as f32) as usize).min(em.shaders.len() - 1);
            em.shaders[idx].clone()
        };

        let p = Particle {
            spawn,
            life,
            pos,
            vel,
            accel,
            gravity,
            rot,
            rot_delta,
            size0,
            size1,
            len0,
            len1,
            alpha0,
            alpha1,
            rgb0,
            rgb1,
            physics,
            impact_kills,
            surface,
            shader,
            cullrange: em.cullrange,
            is_light: behavior == Behavior::Light,
        };

        if behavior == Behavior::Decal {
            push_capped(decals, p, MAX_DECALS);
        } else {
            push_capped(particles, p, MAX_PARTICLES);
        }
    }
}

fn push_capped(q: &mut VecDeque<Particle>, p: Particle, cap: usize) {
    q.push_back(p);
    while q.len() > cap {
        q.pop_front();
    }
}

/// Delayed particles wait, expired ones drop. Motion is the closed-form
/// constant-acceleration solution, so results are exact for any `dt` split.
fn step_pool(pool: &mut VecDeque<Particle>, dt: f32, now: f32, world: Option<&CollisionWorld>) {
    pool.retain_mut(|p| {
        if now < p.spawn {
            return true;
        }
        if p.life <= 0.0 || now - p.spawn > p.life {
            return false;
        }
        let a = p.accel + Vec3::new(0.0, 0.0, p.gravity);
        let mut new_pos = p.pos + p.vel * dt + 0.5 * a * dt * dt;
        let mut new_vel = p.vel + a * dt;
        if p.physics {
            if let Some(world) = world {
                let trace = world.box_trace(p.pos, new_pos, Vec3::ZERO, Vec3::ZERO);
                if trace.fraction < 1.0 {
                    if p.impact_kills {
                        return false;
                    }
                    new_pos = trace.endpos + trace.normal * 0.25;
                    new_vel = bounce(new_vel, trace.normal);
                }
            }
        }
        p.pos = new_pos;
        p.vel = new_vel;
        p.rot += p.rot_delta * dt;
        true
    });
}

/// Mirror-reflect off unit `normal`, damped to 30%.
fn bounce(vel: Vec3, normal: Vec3) -> Vec3 {
    (vel - 2.0 * vel.dot(normal) * normal) * 0.3
}

fn quad_center(q: &FxQuad) -> Vec3 {
    q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0
}

fn make_quad(center: Vec3, x: Vec3, y: Vec3, rgb: Vec3, alpha: f32, shader: &str) -> FxQuad {
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
        shader: shader.to_string(),
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

fn push_quads(
    pool: &VecDeque<Particle>,
    cam_pos: Vec3,
    cam_right: Vec3,
    cam_up: Vec3,
    now: f32,
    out: &mut Vec<FxQuad>,
) {
    for p in pool {
        if p.is_light || now < p.spawn || p.life <= 0.0 || now - p.spawn > p.life {
            continue;
        }
        if p.cullrange > 0.0 && (p.pos - cam_pos).length() > p.cullrange {
            continue;
        }
        let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
        // `size` is a half-extent (grammar doc R4).
        let half = lerp(p.size0, p.size1, t);
        let alpha = lerp(p.alpha0, p.alpha1, t);
        let rgb = p.rgb0.lerp(p.rgb1, t);
        let rad = p.rot.to_radians();

        if let Some(n) = p.surface {
            let (_, t_axis, b_axis) = tangent_basis(n);
            let (rt, rb) = rotate2(t_axis, b_axis, rad);
            out.push(make_quad(
                p.pos,
                rt * half,
                rb * half,
                rgb,
                alpha,
                &p.shader,
            ));
        } else if p.len0 != 0.0 || p.len1 != 0.0 {
            // Anchored at the head, trailing `length` back along travel,
            // `2 * size` wide (grammar doc R5).
            let len = lerp(p.len0, p.len1, t);
            let head = p.pos;
            let tail = head - p.vel.normalize_or_zero() * len;
            let mid = (head + tail) * 0.5;
            let axis = (head - tail) * 0.5;
            let view_dir = (cam_pos - mid).normalize_or_zero();
            let side = stretch_side_dir(axis, view_dir, cam_up, cam_right) * half;
            out.push(make_quad(mid, axis, side, rgb, alpha, &p.shader));
        } else {
            let (rx, ry) = rotate2(cam_right, cam_up, rad);
            out.push(make_quad(
                p.pos,
                rx * half,
                ry * half,
                rgb,
                alpha,
                &p.shader,
            ));
        }
    }
}

pub struct FxSystem {
    cache: HashMap<String, Option<Effect>>,
    warned: HashSet<String>,
    particles: VecDeque<Particle>,
    decals: VecDeque<Particle>,
    rng: Rng,
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
            decals: VecDeque::new(),
            rng: Rng::new(0x9E3779B97F4A7C15),
        }
    }

    fn ensure_cached(&mut self, fs: &Pk3Fs, path: &str) {
        if self.cache.contains_key(path) {
            return;
        }
        let effect = match fs.read(path) {
            None => {
                if self.warned.insert(path.to_string()) {
                    log::warn!("fx {path}: not found");
                }
                None
            }
            Some(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                match efx::parse(&text) {
                    Ok(e) => Some(e),
                    Err(err) => {
                        if self.warned.insert(path.to_string()) {
                            log::warn!("fx {path}: {err}");
                        }
                        None
                    }
                }
            }
        };
        self.cache.insert(path.to_string(), effect);
    }

    /// A new map: world-space particles and decals are meaningless there.
    pub fn clear(&mut self) {
        self.particles.clear();
        self.decals.clear();
    }

    /// Missing or broken files warn once and no-op. Returns the `Sound`
    /// cues for the audio system.
    pub fn spawn(&mut self, fs: &Pk3Fs, path: &str, at: SpawnAt, now: f32) -> Vec<FxSound> {
        self.ensure_cached(fs, path);
        let mut sounds = Vec::new();
        if let Some(Some(effect)) = self.cache.get(path) {
            for em in &effect.emitters {
                spawn_emitter(
                    &mut self.rng,
                    &mut self.particles,
                    &mut self.decals,
                    em,
                    at,
                    now,
                );
                sounds.extend(spawn_sound_cues(&mut self.rng, em, at));
            }
        }
        sounds
    }

    /// One tracer streak from `muzzle` towards `impact`, per the retail
    /// client's hardcoded tracer (grammar doc R8). False when the chance
    /// roll or the distance gate drops the shot.
    pub fn spawn_tracer(&mut self, muzzle: Vec3, impact: Vec3, now: f32) -> bool {
        let delta = impact - muzzle;
        let dist = delta.length();
        if dist < TRACER_MIN_DIST || self.rng.next_f32() >= TRACER_CHANCE {
            return false;
        }
        let dir = delta / dist;
        let travel = dist - TRACER_HEAD_START;
        if travel <= 0.0 {
            return false;
        }
        push_capped(
            &mut self.particles,
            Particle {
                spawn: now,
                life: travel / TRACER_SPEED,
                pos: muzzle + dir * TRACER_HEAD_START,
                vel: dir * TRACER_SPEED,
                accel: Vec3::ZERO,
                gravity: 0.0,
                rot: 0.0,
                rot_delta: 0.0,
                size0: TRACER_WIDTH,
                size1: TRACER_WIDTH,
                len0: TRACER_LENGTH,
                len1: TRACER_LENGTH,
                alpha0: 1.0,
                alpha1: 1.0,
                rgb0: Vec3::ONE,
                rgb1: Vec3::ONE,
                physics: false,
                impact_kills: false,
                surface: None,
                shader: TRACER_SHADER.to_string(),
                cullrange: 0.0,
                is_light: false,
            },
            MAX_PARTICLES,
        );
        true
    }

    #[cfg(test)]
    pub fn spawn_effect_for_test(&mut self, effect: Effect, at: SpawnAt, now: f32) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        for em in &effect.emitters {
            spawn_emitter(
                &mut self.rng,
                &mut self.particles,
                &mut self.decals,
                em,
                at,
                now,
            );
            sounds.extend(spawn_sound_cues(&mut self.rng, em, at));
        }
        sounds
    }

    /// Advance the simulation. `world` enables usePhysics collision.
    pub fn step(&mut self, dt: f32, now: f32, world: Option<&CollisionWorld>) {
        step_pool(&mut self.particles, dt, now, world);
        step_pool(&mut self.decals, dt, now, world);
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
        push_quads(&self.particles, cam_pos, cam_right, cam_up, now, &mut out);
        push_quads(&self.decals, cam_pos, cam_right, cam_up, now, &mut out);
        out.sort_by(|a, b| {
            let da = (quad_center(a) - cam_pos).length_squared();
            let db = (quad_center(b) - cam_pos).length_squared();
            // total_cmp: a NaN must not panic here.
            db.total_cmp(&da)
        });
        out
    }

    /// Live Light-kind particles as point lights, nearest `cam_pos` first,
    /// capped at `MAX_LIGHTS`.
    pub fn lights(&self, cam_pos: Vec3, now: f32) -> Vec<FxLight> {
        let mut candidates: Vec<(f32, FxLight)> = self
            .particles
            .iter()
            .filter(|p| p.is_light && now >= p.spawn && p.life > 0.0 && now - p.spawn <= p.life)
            .map(|p| {
                let t = ((now - p.spawn) / p.life).clamp(0.0, 1.0);
                let radius = lerp(p.size0, p.size1, t);
                let rgb = p.rgb0.lerp(p.rgb1, t);
                let dist_sq = (p.pos - cam_pos).length_squared();
                (
                    dist_sq,
                    FxLight {
                        pos: p.pos.to_array(),
                        radius,
                        rgb: rgb.to_array(),
                    },
                )
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        candidates.truncate(MAX_LIGHTS);
        candidates.into_iter().map(|(_, l)| l).collect()
    }

    /// (live particles, decals, lights) for the F3 overlay's `fx` line.
    pub fn counts(&self) -> (usize, usize, usize) {
        let lights = self.particles.iter().filter(|p| p.is_light).count();
        let particles = self.particles.len() - lights;
        (particles, self.decals.len(), lights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::efx::{Curve, Emitter, R1};
    use glam::Vec3;

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
        // x = v*t = 5, z = 0.5*g*t^2 = -12.5; the quad center is the mean of its verts
        let c = quad_center(&q[0]);
        assert!(
            (c.x - 5.0).abs() < 1e-3 && (c.z + 12.5).abs() < 1e-3,
            "{c:?}"
        );
        // half-size at mid-life: 2 + (4-2)*0.5 = 3
        assert!((quad_half_x(&q[0]) - 3.0).abs() < 1e-3);
        assert!((q[0].rgba[3] - 0.5).abs() < 1e-3);
        s.step(0.6, 1.1, None); // past 1s life
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 1.1).is_empty());
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
    fn sound_and_model_and_trigger_kinds_spawn_nothing() {
        for kind in ["Sound", "Emitter", "FxRunner"] {
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
    fn decal_kind_spawns_a_surface_quad_not_a_particle() {
        let mut e = simple_emitter();
        e.kind = "Decal".into();
        e.velocity = R3 {
            a: [0.0; 3],
            b: [0.0; 3],
        };
        let s = sys_with(
            e,
            SpawnAt::Surface {
                pos: Vec3::new(0.0, 0.0, 10.0),
                normal: Vec3::Z,
            },
        );
        assert_eq!(s.counts(), (0, 1, 0));
        let q = s.build_quads(Vec3::new(0.0, 0.0, 100.0), Vec3::X, Vec3::Y, 0.0);
        assert_eq!(q.len(), 1);
        // nudged off the surface along the normal
        assert!(quad_center(&q[0]).z > 10.0);
        assert_eq!(q[0].shader, "gfx/effects/test");
        assert_eq!(q[0].uvs, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
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

    #[test]
    fn directed_spawn_rotates_velocity_into_dir_frame() {
        let mut e = simple_emitter();
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
    fn accel_is_integrated_exactly_across_two_steps() {
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
        // 0.5 * 5 * 1^2 = 2.5, regardless of the step split.
        let q = s.build_quads(Vec3::new(100.0, 0.0, 0.0), Vec3::Y, Vec3::Z, 1.0);
        let c = quad_center(&q[0]);
        assert!((c.x - 2.5).abs() < 1e-4, "{c:?}");
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

    #[test]
    fn tracer_uses_the_clients_own_dimensions() {
        let mut s = FxSystem::new();
        let muzzle = Vec3::ZERO;
        let impact = Vec3::new(4000.0, 0.0, 0.0);
        // Chance roll is 0.4; retry until one lands.
        let mut spawned = false;
        for _ in 0..50 {
            if s.spawn_tracer(muzzle, impact, 0.0) {
                spawned = true;
                break;
            }
        }
        assert!(spawned);
        let q = s.build_quads(Vec3::new(0.0, -500.0, 0.0), Vec3::X, Vec3::Z, 0.0);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].shader, "gfx/misc/tracer");
        let len = (Vec3::from(q[0].verts[1]) - Vec3::from(q[0].verts[0])).length();
        let width = (Vec3::from(q[0].verts[2]) - Vec3::from(q[0].verts[1])).length();
        assert!((len - 160.0).abs() < 1e-3, "{len}");
        assert!((width - 1.6).abs() < 1e-3, "{width}");
        // Head starts 50 units out from the muzzle and travels at 4500 u/s.
        let xs: Vec<f32> = q[0].verts.iter().map(|v| v[0]).collect();
        let head = xs.iter().cloned().fold(f32::MIN, f32::max);
        assert!((head - 50.0).abs() < 1e-3, "{head}");
        s.step(0.01, 0.01, None);
        let q = s.build_quads(Vec3::new(0.0, -500.0, 0.0), Vec3::X, Vec3::Z, 0.01);
        let head2 = q[0].verts.iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        assert!((head2 - (50.0 + 45.0)).abs() < 1e-2, "{head2}");
    }

    #[test]
    fn tracer_is_skipped_for_short_shots() {
        let mut s = FxSystem::new();
        for _ in 0..50 {
            assert!(!s.spawn_tracer(Vec3::ZERO, Vec3::new(80.0, 0.0, 0.0), 0.0));
        }
        assert_eq!(s.counts().0, 0);
    }

    #[test]
    fn clear_drops_particles_and_decals_but_keeps_the_cache() {
        let mut fx = FxSystem::new();
        // Chance roll is 0.4; retry until a tracer lands.
        for _ in 0..50 {
            if fx.spawn_tracer(Vec3::ZERO, Vec3::new(4000.0, 0.0, 0.0), 0.0) {
                break;
            }
        }
        assert!(fx.counts().0 > 0);
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
        let (particles, decals, _lights) = s.counts();
        assert!(particles + decals > 0);
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
        assert_eq!(s.counts(), (particles, decals, _lights));
    }

    #[test]
    fn bounce_reflects_and_damps() {
        let v = bounce(Vec3::new(0.0, 0.0, -100.0), Vec3::Z);
        assert!((v - Vec3::new(0.0, 0.0, 30.0)).length() < 1e-4);
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

    #[test]
    fn cullrange_hides_far_particles_but_zero_never_culls() {
        let mut e = simple_emitter();
        e.cullrange = 10.0;
        let s = sys_with(
            e,
            SpawnAt::Point {
                pos: Vec3::new(1000.0, 0.0, 0.0),
            },
        );
        assert!(s.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).is_empty());

        let s2 = sys_with(
            simple_emitter(),
            SpawnAt::Point {
                pos: Vec3::new(1000.0, 0.0, 0.0),
            },
        );
        assert_eq!(s2.build_quads(Vec3::ZERO, Vec3::X, Vec3::Z, 0.0).len(), 1);
    }
}
