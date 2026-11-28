//! United Offensive's flamethrower: the cgame's own chunk system, not an
//! `.efx` (uo cgame 0x30024050 `CG_FireFlameChunks` .. 0x30027d10
//! `CG_AddFlameChunks`, a reworked RTCW `cg_flamethrower.c`;
//! docs/research/coduo-diff.md section 19). A body firing a `gas` weapon
//! streams chunks off its `tag_flash` while its `eFlags` firing bit is up;
//! each chunk brakes by how far its stream's aim has turned, grows, sags
//! while fast and climbs once slow, bounces off the world and the bodies
//! in it ([`ChunkWorld`]) and plays through the 43 `flamethrowerFire`
//! sprite frames. The sound runs
//! `flamethrower_start`, then `flamethrower_fire` held while firing, then
//! `flamethrower_cooldown`.
//!
//! vcod moves the chunks by retail's rules, tracing ahead of each
//! (coduo-diff.md 19.3.2, 19.3.4), and draws them as its chain draw does,
//! merging a stream's overlapping chunks (19.3.5), at the places its
//! sprite add gives them (19.3.6); each section lists what it leaves out.
//! Each stream chunk's trail steps shed climbing trail flames and, by its
//! speed, smoke (19.3.3). A fire chunk's first bounce off the
//! world may leave a burning residue ([`Residue`]): a still emitter that
//! sheds small drifting, rising flames for a second or two, which burn as
//! the stream's do.
//!
//! The burn is the victim's to report: [`FlameSystem::report`] tests the
//! chunks against the snapshot's own player and yields the `wbuttons` bit
//! and the `fdc <owner>` command the server burns on (coduo-diff.md 19.3,
//! "The flame report").

use std::collections::HashMap;
use vcod_common::clipmap::SURF_NOIMPACT;

use glam::{Vec2, Vec3};
use vcod_common::collision::{
    CONTENTS_SOLID, CollisionWorld, MASK_PLAYERSOLID, Trace, sound_material,
};
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld, body_trace};

use super::sim::FxQuad;
use vcod_common::net::flags::{EF_FIRING, ET_PLAYER};
use vcod_common::net::protocol::Cod1Wire;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::weapon::WeaponDef;

/// Nozzle speed, units per second (900 per 1000 half-ms ticks).
const START_SPEED: f32 = 1800.0;
/// The friction step (0x30025da0 into 0x30023c30): `+0xbc` times 1800 per
/// 1000 ticks of speed per 1000 ticks, here units per second lost per ms,
/// and the floor a speed that is not 0 is held to (30 per 1000 ticks).
const FRICTION: f32 = 7.2;
const MIN_SPEED: f32 = 60.0;
/// A stream chunk's friction once its owner's stream has ended or moved
/// on (0x3007c310, a double).
const LOOSE_FRICTION: f32 = 0.35;
/// A stream chunk's `+0xa8`: it sags or climbs by `+0xa8` per 1000 ticks
/// squared, scaled by its speed and size (0x30025990).
const STREAM_A8: f32 = 60.0;
/// A chunk's life, ms: 2332 ticks, `(1.8 + 1) / 2` of 1666 truncated, read
/// before the call's `+0xb8` drops to 1 (0x3002445b..0x3002447d).
const LIFE_MS: f32 = 1166.0;
const START_SIZE: f32 = 16.0;
const MAX_SIZE: f32 = 290.0;
/// Sprite frames `flamethrowerFire2` .. `flamethrowerFire44`.
const FRAMES: f32 = 43.0;
/// Chunk spacing, ms per chunk before the view scale: `16 / 900` of 1000
/// ticks.
const SPACING_MS: f32 = 8.888_889;
/// A remote body's chain lives on this long without a frame of firing.
const REMOTE_GRACE_MS: f32 = 75.0;
/// More chunks due than this in a frame spread this many over the gap
/// (0x3007bd48, 0x3007c0f0).
const MAX_DUE: f64 = 180.0;
/// A chunk's move traces this far ahead (350 ticks, 0x30026049) and
/// rebases only once more than this has passed since its base (20 ticks,
/// 0x3007c128).
const LOOK_AHEAD_MS: f64 = 175.0;
const REBASE_MS: f64 = 10.0;

/// A body streaming flame this frame: its nozzle (`tag_flash`), and
/// whether it is the view's own (first person spaces its chunks wider).
#[derive(Clone, Copy, Debug)]
pub struct Nozzle {
    pub owner: u32,
    pub pos: Vec3,
    pub dir: Vec3,
    pub own_view: bool,
    /// The view's own body: `cg.snap`'s `ps.velocity`, which tilts its
    /// chunks; zero for another body.
    pub ps_velocity: Vec3,
}

/// The sounds a frame's flames start; the caller plays them on `owner`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlameSound {
    /// A new stream: `flamethrower_start` once.
    Start { owner: u32, pos: Vec3 },
    /// Still firing: keep `flamethrower_fire` (a looping row) going.
    Fire { owner: u32, pos: Vec3 },
    /// Released: stop the loop, `flamethrower_cooldown` once.
    Stop { owner: u32, pos: Vec3 },
}

#[derive(Clone, Copy)]
struct Owner {
    /// When the last chunk left the nozzle, and the last frame it fired.
    last_emit: f64,
    last_fire: f64,
    pos: Vec3,
    /// Its stream (`centFlameInfo` 0x300ab7e0, one more per new stream),
    /// the nozzle's aim on its last firing frame, and that frame's count.
    chain: u32,
    aim: Vec2,
    fired_frame: u64,
    /// Last frame's share of the owner's velocity in its chunks' direction
    /// ([`owner_velocity`]), in retail's units per 1000 ticks.
    vel: Vec3,
    /// How long the stream has fired (0x300ab7a8, at most 1000 ms
    /// counted), and the aim's turn rate, degrees a second on its faster
    /// axis, at most 200 (0x3007bf44).
    run_ms: f32,
    turn_rate: f32,
}

/// A chunk's type (`+0x2c`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Type 0: a stream chunk off the nozzle.
    Stream,
    /// Type 2 with `+0x14` -1: a residue's flame, tested by the report
    /// (0x300268e0).
    ResidueFlame,
    /// Type 2 with `+0x14` 0 or 1: a stream chunk's trail flame, never
    /// tested.
    Trail,
    /// Type 5: smoke, `flameSmoke{n}`.
    Smoke(u8),
}

#[derive(Clone, Copy)]
struct Chunk {
    /// The body whose nozzle shed it, the entity `fdc` names.
    owner: u32,
    kind: Kind,
    /// The drawn origin `+0xd8`, [`place`]d off the base `+0x70` at its
    /// time `+0x80`.
    pos: Vec3,
    base: Vec3,
    base_at: f64,
    /// How far the look-ahead has traced, and when the chunk gets there
    /// (`+0xc0`, `+0xd0`): a contact is acted on once the clock passes it.
    seen: Vec3,
    seen_at: f64,
    dir: Vec3,
    speed: f32,
    size: f32,
    size_max: f32,
    /// `+0x60`, the size draw a stream chunk's cap is 140 less
    /// (0x30024b12..0x30024bd2, 0x30023c30).
    size_rand: f32,
    born: f64,
    life: f32,
    roll: f32,
    /// When it last moved.
    moved: f64,
    /// The owner is the view's own player (`cg.snap`'s `clientNum`).
    own: bool,
    /// A stream chunk's next trail step and its share of the life to the
    /// one after (`+0x130`, `+0x138`; 0x30027f22..0x3002905e).
    trail: Option<(f64, f64)>,
    /// A stream chunk may leave a residue on its first bounce (`+0x14` 1
    /// at the nozzle, 0x30024c04; `+0x124` the bounce, 0x30025da0).
    may_residue: bool,
    /// `+0xa8`: 0x30025990 lowers the chunk by `a8 * dt * age` (a stream
    /// chunk's scaled by its speed and size, [`lift`]), `dt` and `age` in
    /// units of 1000 ticks (500 ms) since the last move and the birth, so
    /// a negative one climbs.
    a8: f32,
    /// The friction `+0xbc`: 1 on a stream chunk, 0 on a residue flame,
    /// 0.5 once either has bounced (0x30025da0).
    friction: f32,
    /// A stream chunk's stream and the nozzle's aim and place as it left
    /// (`+0x128`, `+0x100`, `+0x10c`): while that stream fires on, the
    /// chunk brakes by how far the aim has moved since ([`chain_brake`]).
    chain: Option<(u32, Vec2, Vec3)>,
    /// This frame's chain draw (0x300272b0) draws it, and so the report
    /// may test it.
    drawn: bool,
    /// `+0x118`: the face its look-ahead last struck, zero until one
    /// (0x300261b0..0x3002621f); copies keep it.
    normal: Vec3,
}

/// A burning residue (a type 3 chunk, uo cgame 0x30027ad0 spawned off a
/// bounce in 0x30025da0): invisible and still where a stream chunk struck,
/// shedding a residue flame every 37.5..112.5 ms until it dies
/// (0x30027d10's trail loop).
#[derive(Clone, Copy)]
struct Residue {
    owner: u32,
    pos: Vec3,
    /// The struck face's normal (`+0x118`).
    normal: Vec3,
    size: f32,
    born: f64,
    death: f64,
    next_emit: f64,
    /// Its `+0xa8`: -0.2 at the bounce, redrawn each step on a wall.
    sink: f32,
}

/// 0x30025da0's bounce: the velocity clipped against the struck face with
/// an overbounce of 1.1 (0x30008390), a slide rather than a mirror.
fn clip_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let d = v.dot(normal);
    v - normal * if d < 0.0 { d * 1.1 } else { d / 1.1 }
}

/// The friction step (0x30025da0 into 0x30023c30, vcod's speed being
/// retail's per 1000 ticks doubled): above 2 a chunk loses [`FRICTION`]
/// times its `+0xbc` times `brake` a ms, and a result under 60 that is
/// not 0 becomes 60.
fn brake(speed: f32, friction: f32, brake: f32, dt_ms: f32) -> f32 {
    if speed <= 2.0 || friction == 0.0 {
        return speed;
    }
    let slowed = speed - FRICTION * friction * brake * dt_ms;
    if slowed != 0.0 && slowed < MIN_SPEED {
        MIN_SPEED
    } else {
        slowed
    }
}

/// Q3's `vectoangles` pitch and yaw, degrees.
fn aim_of(dir: Vec3) -> Vec2 {
    let pitch = -dir.z.atan2(dir.x.hypot(dir.y)).to_degrees();
    Vec2::new(pitch, dir.y.atan2(dir.x).to_degrees())
}

/// Q3's `AngleVectors` forward off a pitch and yaw in degrees.
fn dir_of(aim: Vec2) -> Vec3 {
    let (sp, cp) = aim.x.to_radians().sin_cos();
    let (sy, cy) = aim.y.to_radians().sin_cos();
    Vec3::new(cp * cy, cp * sy, -sp)
}

/// The owner's velocity a stream chunk's direction takes in, per 1000
/// ticks (0x30024289..0x3002439d): for the view's own body moving faster
/// than 1, its `ps.velocity` times 0.75; else, while the stream fires on,
/// the nozzle's own move since the last firing frame, per 1000 ticks with
/// the gap held to 20 ticks; else nothing. Halved.
fn owner_velocity(n: &Nozzle, last: Option<(Vec3, f64)>, now: f64) -> Vec3 {
    if n.own_view && n.ps_velocity.length() > 1.0 {
        return n.ps_velocity * 0.75 * 0.5;
    }
    match last {
        Some((pos, at)) => {
            let ticks = ((now - at) * 2.0).max(20.0) as f32;
            (n.pos - pos) * (1000.0 / ticks) * 0.5
        }
        None => Vec3::ZERO,
    }
}

/// A stream chunk's share of the friction while its stream fires on
/// (0x30025e45..0x30025f8f): the larger of how far the aim has turned
/// since it left, in degrees, and a twentieth of how far the nozzle has
/// moved, `m`, gives `clamp((m + 6.8) / 37.3, 0, 1)`
/// (0x3007c320, 0x3007c318, 0x3007c328). A steady aim brakes at 0.18.
fn chain_brake(fired: (Vec2, Vec3), now: (Vec2, Vec3)) -> f32 {
    let wrap = |a: f32| (a + 180.0).rem_euclid(360.0) - 180.0;
    let turn = now.0 - fired.0;
    let m = Vec2::new(wrap(turn.x), wrap(turn.y))
        .length()
        .max(0.05 * now.1.distance(fired.1));
    let f = if m >= 30.5 {
        0.0
    } else {
        1.0 - (f64::from(m) + 6.8) * 0.026_809_651_337_438_734
    };
    (1.0 - f) as f32
}

/// 0x30025990's `+0xa8` for a stream chunk: `a8 * (1 - 1.5 * (1 - p))`,
/// `p` the clamped `0.35 * (1 - size / 290) + 0.65 * (speed / 900)^2` in
/// retail's units (0x3007c298, 0x3007c0e4, 0x3007c1fc, 0x3007c29c). A
/// fast chunk sags, a slow, large one climbs.
fn lift(c: &Chunk) -> f32 {
    if c.kind != Kind::Stream {
        return c.a8;
    }
    let v = c.speed / START_SPEED;
    let p = (0.35 * (1.0 - c.size * 0.003_448_276) + 0.65 * v * v).clamp(0.0, 1.0);
    c.a8 * (1.0 - 1.5 * (1.0 - p))
}

/// 0x3004d200: `sin(a * t) * cos(b * t)`, `t` in 1000-tick turns.
fn wave(a: f32, b: f32, ticks: f64) -> f32 {
    let t = (ticks / 1000.0) as f32 * std::f32::consts::TAU;
    (a * t).sin() * (b * t).cos()
}

/// A chunk's move traces this box about it (0x300851c4, 0x300851d0).
const CHUNK_BOX: Vec3 = Vec3::splat(2.0);

/// What a chunk's move clips (uo cgame 0x30025cd0): `CG_Trace` of a
/// [`CHUNK_BOX`] over the world and the snapshot's solid entities but the
/// chunk's owner, mask
/// [`MASK_PLAYERSOLID`], then, unless the owner is `cg.snap`'s own
/// player, that player's capsule (a temp capsule model off the
/// playerstate's `mins` and `maxs` at its `origin`, contents
/// [`CONTENTS_BODY`]), kept when it starts solid or is the nearer.
pub struct ChunkWorld<'a> {
    pub world: &'a CollisionWorld,
    /// The snapshot's solid entities but the local player
    /// (`play::predict::solid_bodies`).
    pub bodies: &'a [Body],
    pub local: Option<Victim>,
    /// `sv_maxclients`: a struck entity under it is a client.
    pub max_clients: u32,
}

/// `sv_maxclients` off the serverinfo, [`ChunkWorld::max_clients`]; all
/// 64 slots when it is missing.
pub fn max_clients(serverinfo: &str) -> u32 {
    vcod_common::net::info_value_for_key(serverinfo, "sv_maxclients")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}

impl ChunkWorld<'_> {
    /// The trace and the entity it stopped on.
    fn trace(&self, start: Vec3, end: Vec3, owner: u32) -> (Trace, u32) {
        let w = MoveWorld::new(self.world, self.bodies, owner);
        let t = w.box_trace(start, end, -CHUNK_BOX, CHUNK_BOX, MASK_PLAYERSOLID);
        let hit = (t, w.entity_num(&t));
        let Some(v) = self.local.filter(|v| v.client != owner) else {
            return hit;
        };
        let body = Body {
            entity: v.client,
            origin: v.origin,
            mins: v.mins,
            maxs: v.maxs,
            contents: CONTENTS_BODY,
        };
        let own = body_trace(start, end, -CHUNK_BOX, CHUNK_BOX, &body);
        if own.startsolid || own.fraction < t.fraction {
            (own, v.client)
        } else {
            hit
        }
    }
}

/// A stream chunk's first trail share `+0x138` (0x30024db8..0x30025045):
/// `0.35 / d` held to 0.06 (own view) or 0.04 and to 100, `d` the larger
/// of `0.75 + 0.25 * (1 - run / 1000)` and the aim's turn rate times 0.25
/// (own view, the rate held to 400) or 0.125 (held to 100), at most 20.
fn trail_share(own: bool, run_ms: f32, turn_rate: f32) -> f64 {
    let (k, cap, floor) = if own {
        (0.25, 400.0, 0.06)
    } else {
        (0.125, 100.0, 0.04)
    };
    let held = 0.75 + 0.25 * (1.0 - f64::from(run_ms.min(1000.0)) * 0.001);
    let turning = (k * f64::from(turn_rate.min(cap))).min(20.0);
    (0.35 / held.max(turning)).clamp(floor, 100.0)
}

/// The streams fired this frame or the last, by owner: the stream, the
/// nozzle's aim and its place (0x30025e1e..0x30025e3f).
type Streams = HashMap<u32, (u32, Vec2, Vec3)>;

/// 0x30025990: where a chunk is at `t`, off its base: along its direction
/// at its present speed, lowered by [`lift`] times the time since the base
/// and its age, both in 1000 ticks. A brake since the base moves the whole
/// span.
fn place(c: &Chunk, t: f64) -> Vec3 {
    let dt = (t - c.base_at) as f32;
    let age = (t - c.born) as f32 / 500.0;
    c.base + c.dir * c.speed * dt / 1000.0 - Vec3::Z * (lift(c) * dt / 500.0 * age)
}

/// 0x30027505..0x3002760a: a drawn stream chunk takes in the next older
/// one of its chain while that one lies within `2 * ((r + 1) / 2)^1.5 *
/// size` times 0.1 (the view's own stream) or 0.25 of it, `r` its size
/// over 72.5 held to 1, and was born within 50 ticks of it.
fn overlaps(c: &Chunk, n: &Chunk) -> bool {
    let k = if c.own { 0.1 } else { 0.25 };
    let r = f64::from((c.size / 72.5).min(1.0));
    let reach = 2.0 * ((r + 1.0) * 0.5).powf(1.5) * f64::from(c.size) * k;
    reach > f64::from(c.pos.distance(n.pos)) && ((c.born - n.born).abs() as f32) < 25.0
}

/// 0x300257e0: `c` takes in `n`, the next older chunk of its chain. When
/// `n` is the larger, `c` takes its size, cap, size draw, clocks, life,
/// roll, place, `+0xa8`, direction and speed, and keeps its own
/// look-ahead, chain angles and friction; either way it takes `n`'s
/// residue chance when it has none.
fn merge(c: &mut Chunk, n: &Chunk) {
    if n.size > c.size {
        (c.size, c.size_max, c.size_rand) = (n.size, n.size_max, n.size_rand);
        (c.moved, c.born, c.life, c.roll) = (n.moved, n.born, n.life, n.roll);
        (c.base, c.base_at, c.pos) = (n.base, n.base_at, n.pos);
        (c.a8, c.dir, c.speed) = (n.a8, n.dir, n.speed);
    }
    c.may_residue |= n.may_residue;
}

/// 0x30027632..0x300277a7: in another body's live stream a chunk whose
/// size is within a tenth of the last drawn one's is skipped when `(1 -
/// frac)^3 * min(size / 30, 1) * 32` beats `(0.1 + 0.9 * (1 - |cos|)) *
/// |eye - last| * |chunk - last| / min(size, 30)`, the cosine between the
/// two lines off the last drawn chunk.
fn hidden_behind(c: &Chunk, last: &Chunk, now: f64, eye: Vec3) -> bool {
    if (last.size - c.size).abs() >= c.size / 10.0 {
        return false;
    }
    let (to_c, to_eye) = (c.pos - last.pos, eye - last.pos);
    let cos = f64::from(
        to_c.normalize_or_zero()
            .dot(to_eye.normalize_or_zero())
            .abs(),
    );
    let spread = (1.0 - cos) * 0.9 + 0.1;
    let apart = f64::from(to_eye.length() / c.size.min(30.0) * to_c.length());
    let frac = (now - c.born) / f64::from(c.life);
    let fade = (1.0 - frac).powi(3) * f64::from((c.size / 30.0).min(1.0)) * 32.0;
    fade > spread * apart
}

/// The last look-ahead trace of any chunk: where it ended and its fraction
/// (0x300a85bc, 0x300a85b8).
type LastTrace = Option<(Vec3, f32)>;

/// One chunk's move from its last on to `now` (0x30025da0): it grows
/// (0x30025c60), brakes, and, once more than [`REBASE_MS`] has passed since
/// its base, moves on. Unless smoke (0x3002603b) and before the clock
/// reaches its last trace, it traces [`LOOK_AHEAD_MS`] ahead at its present
/// speed as [`ChunkWorld`] clips, from where the last trace ended; a miss
/// within 2 of the last trace's end, when that one missed, is taken for
/// a miss without a trace (0x3002605d..0x300260d1). Until the clock passes the contact the
/// chunk only rebases where it is; past it, it bounces at the contact
/// itself, but not off a `SURF_NOIMPACT` face, and stuck inside it stops
/// (0x300261b0..0x3002656a). Returns the struck face's normal when the
/// strike may leave a residue.
fn move_chunk(
    c: &mut Chunk,
    now: f64,
    world: Option<&ChunkWorld>,
    streams: &Streams,
    last: &mut LastTrace,
) -> Option<Vec3> {
    let dt_ms = ((now - c.moved) as f32).max(0.0);
    c.moved = now;
    // 0x30023b70: a stream chunk grows slower the faster it goes.
    let slow = match c.kind {
        Kind::Stream => {
            let v = c.speed / START_SPEED;
            2.0 * v * v * v + 0.8
        }
        _ => 1.0,
    };
    if c.size < c.size_max {
        c.size = (c.size + c.size_max / c.life / slow * dt_ms).min(c.size_max);
    }
    c.size = c.size.min(MAX_SIZE);
    if c.speed > 2.0 && c.friction != 0.0 {
        let share = match (c.chain, streams.get(&c.owner)) {
            (Some((chain, aim, at)), Some(&(now_chain, now_aim, now_at))) if now_chain == chain => {
                chain_brake((aim, at), (now_aim, now_at))
            }
            _ if c.kind == Kind::Stream => LOOSE_FRICTION,
            _ => 1.0,
        };
        c.speed = brake(c.speed, c.friction, share, dt_ms);
        // 0x30023c30 sets a stream chunk's cap by its speed.
        if c.kind == Kind::Stream {
            c.size_max = ((1.0 - c.speed / START_SPEED) + 140.0 - c.size_rand).min(MAX_SIZE);
        }
    }
    if (c.speed <= 2.0 && c.a8 == 0.0) || now - c.base_at <= REBASE_MS {
        c.pos = place(c, now);
        return None;
    }
    let rebase = |c: &mut Chunk| {
        c.base = place(c, now);
        c.base_at = now;
        c.pos = c.base;
    };
    if now <= c.seen_at || matches!(c.kind, Kind::Smoke(_)) {
        rebase(c);
        return None;
    }
    let ahead = now + LOOK_AHEAD_MS;
    let end = place(c, ahead);
    let traced = match (*last, world) {
        (Some((at, frac)), _) if frac == 1.0 && at.distance(end) < CHUNK_BOX.x => {
            c.seen = at;
            c.seen_at = ahead;
            None
        }
        (_, Some(w)) => {
            let (mut tr, entity) = w.trace(c.seen, end, c.owner);
            if tr.allsolid {
                tr.fraction = 0.0;
                tr.endpos = c.seen;
            }
            *last = Some((tr.endpos, tr.fraction));
            c.seen_at += f64::from(tr.fraction) * (ahead - c.seen_at);
            c.seen = tr.endpos;
            Some((tr, entity, w.max_clients))
        }
        (_, None) => {
            *last = Some((end, 1.0));
            c.seen = end;
            c.seen_at = ahead;
            None
        }
    };
    if now < c.seen_at {
        rebase(c);
        return None;
    }
    let Some((tr, entity, max_clients)) = traced else {
        c.pos = place(c, now);
        return None;
    };
    // Any face struck, a `SURF_NOIMPACT` one too.
    if tr.fraction < 1.0 && tr.normal != Vec3::ZERO {
        c.normal = tr.normal;
    }
    if tr.allsolid {
        c.dir = Vec3::ZERO;
        c.speed = 0.0;
    } else if tr.surface_flags & SURF_NOIMPACT == 0 {
        let v = clip_velocity(c.dir * c.speed, tr.normal);
        c.dir = v.normalize_or_zero();
        c.speed = v.length() * 0.25;
        c.friction = 0.5;
        (c.base, c.seen, c.base_at, c.seen_at) = (tr.endpos, tr.endpos, now, now);
        c.pos = tr.endpos;
        if std::mem::take(&mut c.may_residue)
            && entity >= max_clients
            && !NO_RESIDUE_SURFACES.contains(&sound_material(tr.surface_flags))
        {
            return Some(tr.normal);
        }
        return None;
    }
    c.pos = place(c, now);
    None
}

/// A residue lives `RESIDUE_LIFE_MS` times 1 on a floor up to 1.5 on a
/// wall (`1 + (1 - |normal z|) / 2`), plus up to `RESIDUE_LIFE_MS` more
/// at random (2000 ticks, 0x3007bd28, 0x3007c300, 0x3007bee4).
const RESIDUE_LIFE_MS: f32 = 1000.0;
/// It sheds a flame every `RESIDUE_STEP_MS` on average, half to one and a
/// half of it apart (150 ticks over its life, 0x3007c2f0; the step's
/// `/ 2 + rand * step`, 0x3007bde8).
const RESIDUE_STEP_MS: f32 = 75.0;
/// A chunk larger than this leaves a residue of this size (0x3007c308).
const RESIDUE_MAX_SIZE: f32 = 145.0;
/// One owner leaves a residue at most this often (200 ticks), and not
/// within [`RESIDUE_NEAR`] of its last inside [`RESIDUE_NEAR_MS`] (6000
/// ticks, 0x3007c0f8; 0x3007bdd0).
const RESIDUE_EVERY_MS: f32 = 100.0;
const RESIDUE_NEAR: f32 = 32.0;
const RESIDUE_NEAR_MS: f32 = 3000.0;
/// A residue flame lives this long (400 + 2400 ticks, 0x3007c3b8,
/// 0x3007c3b0), started up to [`RESIDUE_FLAME_LAG_MS`] into it (600
/// ticks, 0x3007c3a8).
const RESIDUE_FLAME_LIFE_MS: f32 = 1400.0;
const RESIDUE_FLAME_LAG_MS: f32 = 300.0;
/// A residue's `+0xa8` at the bounce (0x30027ad0's third argument).
const RESIDUE_SINK: f32 = -0.2;
/// While more than this is left of a residue's life (1300 ticks,
/// 0x3007c3d0), each flame it sheds tests its face: a box 4 units into it
/// (0x3007be40) that meets nothing kills it on the spot (0x300281c6..
/// 0x300282b2).
const RESIDUE_FACE_MS: f64 = 650.0;
const RESIDUE_FACE_DEPTH: f32 = 4.0;
/// Surfaces that take no residue: ice, snow, water (0x1300000, 0x1400000
/// and 0xc00000 of the struck `surfaceFlags`).
const NO_RESIDUE_SURFACES: [i32; 3] = [12, 19, 20];

/// The report's check runs at most once per this many ms per client
/// (100 of the chunk clock's half-ms ticks, uo cgame 0x30027d10).
const CHECK_MS: f32 = 50.0;
/// `fdc` goes out at most once per this many ms per client (0x300291c0).
const FDC_MS: f32 = 2000.0;
/// A chunk's reach is its size times this, halved for the victim's own
/// flame, never under [`MIN_REACH`] (0x300265c0, 0x3007162c, 0x3007c0a0).
const REACH: f32 = 0.4;
const MIN_REACH: f32 = 14.0;
/// Chunks this far through their life are not tested (0x300268e0,
/// 0x3007c038), nor the victim's own before [`OWN_MIN_FRAC`] (0x3007c098).
const MAX_FRAC: f32 = 0.9;
const OWN_MIN_FRAC: f32 = 0.4;

/// The flame test's `CG_Trace` (uo cgame 0x30035310, mask 0x2802031, no
/// pass entity): the world, then every solid entity of the snapshot. It
/// returns the fraction and the entity it stopped on.
pub type ShotTrace<'a> = &'a dyn Fn(Vec3, Vec3) -> (f32, u32);

/// The player a flame report tests: the snapshot playerstate's
/// `clientNum`, origin and box.
#[derive(Clone, Copy, Debug)]
pub struct Victim {
    pub client: u32,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// A frame's flame report: the in-flame flag the next cmds carry as
/// `wbuttons` 0x1, and the owner to name in an `fdc` command.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlameReport {
    pub in_flame: bool,
    pub fdc: Option<u32>,
}

/// One client slot's report clocks (`centFlameInfo`'s 0x300ab7bc and
/// 0x300ab7f8 rows).
#[derive(Clone, Copy)]
struct ReportClock {
    next_check: f64,
    last_fdc: f64,
}

pub struct FlameSystem {
    owners: HashMap<u32, Owner>,
    chunks: Vec<Chunk>,
    residues: Vec<Residue>,
    /// Each owner's last residue, when and where (`centFlameInfo` rows
    /// 0x300ab7f0, 0x300ab7e4).
    last_residue: HashMap<u32, (f64, Vec3)>,
    rng: u32,
    report_clocks: HashMap<u32, ReportClock>,
    in_flame: bool,
    /// The last chunk tested and when: a nearby chunk in the same frame is
    /// skipped (0x300a8710, 0x300d9760).
    last_test: Option<(f64, Vec3)>,
    /// Frames stepped, and streams started (each owner's `centFlameInfo`
    /// 0x300ab750 and 0x300ab7e0 count against these).
    frame: u64,
    chains: u32,
    last_trace: LastTrace,
}

impl Default for FlameSystem {
    fn default() -> Self {
        FlameSystem {
            owners: HashMap::new(),
            chunks: Vec::new(),
            residues: Vec::new(),
            last_residue: HashMap::new(),
            rng: 0x1234_5678,
            report_clocks: HashMap::new(),
            in_flame: false,
            last_test: None,
            frame: 0,
            chains: 0,
            last_trace: None,
        }
    }
}

impl FlameSystem {
    fn rand(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// [`Streams`]: each owner that fired this frame or the last.
    fn streams(&self) -> Streams {
        self.owners
            .iter()
            .filter(|(_, o)| o.fired_frame + 1 >= self.frame)
            .map(|(&owner, o)| (owner, (o.chain, o.aim, o.pos)))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.owners.is_empty() && self.residues.is_empty()
    }

    /// Emits from this frame's `nozzles`, moves every chunk `dt_ms` on to
    /// `now` (ms), and reports the sound transitions. `eye` scales a remote
    /// body's spacing with its distance.
    pub fn step(
        &mut self,
        now: f64,
        dt_ms: f32,
        nozzles: &[Nozzle],
        eye: Vec3,
        world: Option<&ChunkWorld>,
    ) -> Vec<FlameSound> {
        let mut sounds = Vec::new();
        self.frame += 1;
        for n in nozzles {
            let started = match self.owners.get(&n.owner) {
                Some(o) => (now - o.last_fire) as f32 > dt_ms.max(1.0) + REMOTE_GRACE_MS,
                None => true,
            };
            // Last frame's nozzle, aim and velocity share, and the last
            // chunk's birth, which each chunk of this frame lerps from.
            let last = self
                .owners
                .get(&n.owner)
                .filter(|_| !started)
                .map(|o| (o.pos, o.aim, o.vel, o.last_emit, o.last_fire));
            let vel = owner_velocity(n, last.map(|l| (l.0, l.4)), now);
            if started {
                sounds.push(FlameSound::Start {
                    owner: n.owner,
                    pos: n.pos,
                });
                self.chains += 1;
                self.owners.insert(
                    n.owner,
                    Owner {
                        last_emit: now,
                        last_fire: now,
                        pos: n.pos,
                        chain: self.chains,
                        aim: aim_of(n.dir),
                        fired_frame: self.frame,
                        vel,
                        run_ms: -dt_ms,
                        turn_rate: 0.0,
                    },
                );
            }
            if let Some(o) = self.owners.get_mut(&n.owner) {
                let aim = aim_of(n.dir);
                let wrap = |a: f32| ((a + 180.0).rem_euclid(360.0) - 180.0).abs();
                let turn = wrap(aim.x - o.aim.x).max(wrap(aim.y - o.aim.y));
                o.turn_rate = (turn / (dt_ms.max(0.1) / 1000.0)).min(200.0);
                o.run_ms = (o.run_ms + dt_ms).min(1000.0);
                o.aim = aim;
                o.pos = n.pos;
                o.fired_frame = self.frame;
            }
            sounds.push(FlameSound::Fire {
                owner: n.owner,
                pos: n.pos,
            });
            let k = if n.own_view { 0.75 } else { 0.35 };
            let far = if n.own_view {
                1.0
            } else {
                (n.pos.distance(eye).clamp(2048.0, 6000.0)) / 2048.0
            };
            let mut spacing = f64::from((k * SPACING_MS * far).max(1.0));
            // A new stream's frame lays only its type 1 head, never drawn
            // or tested, whose birth the next frame's chunks space from
            // (0x30024538..0x30024555, 0x3002521b).
            let last_emit = self.owners[&n.owner].last_emit;
            if (now - last_emit) / spacing > MAX_DUE {
                spacing = (now - last_emit) / MAX_DUE;
            }
            let mut t = match started {
                true => f64::INFINITY,
                false => last_emit + spacing,
            };
            let aim = aim_of(n.dir);
            while t <= now {
                // 0x30024810..0x300249e0: a chunk takes the nozzle and
                // velocity share lerped from last frame's by its time
                // between the last chunk and now, and the two frames'
                // forwards lerped, normalized; its chain angles lerp as
                // plain angles, no wrap (0x300248e4..0x30024947).
                let (pos, at, fwd, v) = match last {
                    Some((pos0, aim0, vel0, born0, _)) if now > born0 => {
                        let f = ((now - t) / (now - born0)).clamp(0.0, 1.0) as f32;
                        let fwd = dir_of(aim0).lerp(dir_of(aim), 1.0 - f);
                        (
                            pos0.lerp(n.pos, 1.0 - f),
                            aim0.lerp(aim, 1.0 - f),
                            fwd.normalize_or(dir_of(aim)),
                            vel0.lerp(vel, 1.0 - f),
                        )
                    }
                    _ => (n.pos, aim, dir_of(aim), vel),
                };
                let at_t = Nozzle { pos, ..*n };
                self.emit(&at_t, at, fwd, v, t, world);
                t += spacing;
            }
            if let Some(o) = self.owners.get_mut(&n.owner) {
                if !started {
                    o.last_emit = t - spacing;
                }
                o.last_fire = now;
                o.pos = n.pos;
                o.vel = vel;
            }
        }
        let streams = self.streams();
        // Released: no nozzle this frame and past the grace.
        let firing: Vec<u32> = nozzles.iter().map(|n| n.owner).collect();
        self.owners.retain(|&owner, o| {
            let keep = firing.contains(&owner) || (now - o.last_fire) as f32 <= REMOTE_GRACE_MS;
            if !keep {
                sounds.push(FlameSound::Stop { owner, pos: o.pos });
            }
            keep
        });

        self.shed_residue_flames(now, dt_ms, world);
        let mut struck = Vec::new();
        let mut born = Vec::new();
        for i in 0..self.chunks.len() {
            let mut c = self.chunks[i];
            if now < c.born + f64::from(c.life) {
                self.trail(&mut c, now, eye, world, &streams, &mut born);
            }
            if let Some(normal) = move_chunk(&mut c, now, world, &streams, &mut self.last_trace) {
                struck.push((i, normal));
            }
            self.chunks[i] = c;
        }
        self.chunks.extend(born);
        for (i, normal) in struck {
            let c = self.chunks[i];
            self.leave_residue(&c, normal, now);
        }
        self.residues.retain(|r| now < r.death);
        self.draw_chains(now, eye);
        sounds
    }

    /// A stream chunk's trail steps due by `now`, run before its own move
    /// (0x30027f22..0x3002905e). Each sheds a trail flame and may shed a
    /// smoke puff, both into `born`; the steps grow apart with the life
    /// spent, and, for another body's stream, with the eye's nearness.
    fn trail(
        &mut self,
        c: &mut Chunk,
        now: f64,
        eye: Vec3,
        world: Option<&ChunkWorld>,
        streams: &Streams,
        born: &mut Vec<Chunk>,
    ) {
        let Some((mut next, mut share)) = c.trail else {
            return;
        };
        let span = f64::from(c.life);
        while share != 0.0 && next < now {
            let frac = ((next - c.born) / span) as f32;
            // A type 2 copy moved to the step (0x30027f8a..0x3002800c).
            let mut flame = Chunk {
                kind: Kind::Trail,
                trail: None,
                may_residue: false,
                ..*c
            };
            move_chunk(&mut flame, next, world, streams, &mut self.last_trace);
            flame.chain = None;
            // It is older than its parent by a share of the life left
            // (0x30028026..0x300288a8); the clamp draws anew per test.
            let lag_share = |r: f32| ((2.0 * r - 1.0) * 0.15 + 1.5 * (1.0 - frac).powi(3)) * 0.3;
            let lag_share = if lag_share(self.rand()) < 0.0 {
                0.0
            } else if lag_share(self.rand()) > 0.8 {
                0.8
            } else {
                lag_share(self.rand())
            };
            let lag = (c.born + span - next) * f64::from(lag_share);
            if c.own {
                flame.born -= (f64::from(frac).sqrt() + 1.0) * lag;
            } else {
                flame.born -= lag;
                flame.life -= lag as f32;
            }
            // 0x30028911..0x30028be7: it climbs, its cap opens, it brakes
            // at 0.15 and slows.
            flame.a8 = -2.25
                * (c.a8 / STREAM_A8)
                * (3.0 * self.rand() + 16.0 * (flame.size / MAX_SIZE).powi(2) + 10.0 * frac);
            // A draw times 0 (0x30028a4b).
            self.rand();
            flame.size_rand =
                ((0.25 * flame.size_rand + 105.0) * 0.75 * (0.5 * (1.0 - frac).powi(5) + 0.2))
                    .clamp(0.0, MAX_SIZE);
            if streams.contains_key(&c.owner) {
                flame.size += (1.0 - (1.0 - frac).powi(4)) * 20.0;
            }
            flame.size_max = 140.0 - flame.size_rand;
            flame.friction = 0.15;
            flame.roll += (self.rand() * 15.0).floor();
            flame.speed *= if c.own { frac * 0.2 + 0.1 } else { 0.25 };
            if 0.02 < frac && frac < 0.8 {
                self.smoke(c, &flame, frac, next, born);
            }
            move_chunk(&mut flame, now, world, streams, &mut self.last_trace);
            born.push(flame);
            share *= f64::from(2.0 * frac + 1.0);
            let mut step = span * share;
            let dist = eye.distance(c.pos).max(2.0);
            if !c.own && dist < c.size * 4.0 {
                step *= f64::from(c.size * 4.0 / dist);
            }
            next += step / 2.0 + f64::from(self.rand()) * step;
        }
        c.trail = Some((next, share));
    }

    /// A trail step's smoke (0x30028c32..0x30028eeb): with a chance of
    /// the stream chunk's `(speed / 900)^2` halved, halved again for
    /// another body's stream, a copy of the step's `flame` as type 5, born
    /// at the step and living 800 to 2100 ms (1600 + rand * 2600 ticks),
    /// its cap 3.5 times the flame's size plus 20 and its start a
    /// twentieth of that, set 6 plus 0.19 of the stream chunk's size
    /// above it and climbing at 2 to 15 (`+0xa8`). It is not moved again
    /// until the next frame.
    fn smoke(&mut self, c: &Chunk, flame: &Chunk, frac: f32, at: f64, born: &mut Vec<Chunk>) {
        let v = c.speed / START_SPEED;
        let own = if c.own { 1.0 } else { 0.5 };
        if v * v * own * 0.5 <= self.rand() {
            return;
        }
        let shade = (self.rand() * 6.0) as u8 + 1;
        let size_max = (flame.size * 3.5 + 20.0).min(MAX_SIZE);
        let roll = (2.0 * self.rand() - 1.0) * 25.0 + 180.0;
        let life = (1600.0 + self.rand() * 2600.0) / 2.0;
        let pos = flame.pos + Vec3::Z * (c.size * 0.19 + 6.0);
        born.push(Chunk {
            kind: Kind::Smoke(shade.min(6)),
            pos,
            // The rise goes on the base's z alone (0x30028e1d, 0x30028e23).
            base: flame.base.with_z(pos.z),
            size: size_max / 20.0,
            size_max,
            born: at,
            life,
            roll,
            moved: at,
            a8: ((frac * 1.85 + 0.55) * flame.a8).clamp(-15.0, -2.0),
            ..*flame
        });
    }

    /// 0x30025da0's residue off a stream chunk's first bounce: unless its
    /// owner left one in the last 100 ms, or within 32 units of one left
    /// in the last 3 s, a residue of the chunk's size (at most 145) at the
    /// contact, living one to two and a half seconds by the face's slope
    /// (0x30027ad0 and the death and trail it is given there).
    fn leave_residue(&mut self, c: &Chunk, normal: Vec3, now: f64) {
        if let Some(&(t, at)) = self.last_residue.get(&c.owner)
            && (((now - t) as f32) < RESIDUE_EVERY_MS
                || (t <= now
                    && (now - t) as f32 <= RESIDUE_NEAR_MS
                    && at.distance(c.pos) <= RESIDUE_NEAR))
        {
            return;
        }
        self.last_residue.insert(c.owner, (now, c.pos));
        let life =
            RESIDUE_LIFE_MS * (1.0 + (1.0 - normal.z.abs()) * 0.5) + self.rand() * RESIDUE_LIFE_MS;
        let next_emit = now + f64::from(self.rand() * RESIDUE_STEP_MS);
        self.residues.push(Residue {
            owner: c.owner,
            pos: c.pos,
            normal,
            size: c.size.min(RESIDUE_MAX_SIZE),
            born: now,
            death: now + f64::from(life),
            next_emit,
            sink: RESIDUE_SINK,
        });
    }

    /// Every residue's flames due by `now` (0x30027d10, a type 3 parent's
    /// type 2 children): each lives [`RESIDUE_FLAME_LIFE_MS`], started up
    /// to [`RESIDUE_FLAME_LAG_MS`] into it, at a size of 1.5 times the
    /// residue's held to 60..120 plus up to 25, from half of that. It
    /// drifts along the face at 4 units a second or more, off a wall
    /// as well, and rises ever faster ([`Chunk::a8`]). Such a flame
    /// never trails a smoke (type 2, 0x30028bfb), and a residue whose face
    /// went dies ([`RESIDUE_FACE_MS`]).
    fn shed_residue_flames(&mut self, now: f64, dt_ms: f32, world: Option<&ChunkWorld>) {
        for i in 0..self.residues.len() {
            while self.residues[i].next_emit < now.min(self.residues[i].death) {
                // A wall's residue redraws its `+0xa8` each step (|z| of
                // the normal under 0.2, 0x3007c3c0).
                if self.residues[i].normal.z.abs() < 0.2 {
                    self.residues[i].sink = -0.3 - 2.0 * self.rand();
                }
                let r = self.residues[i];
                let lag = self.rand() * RESIDUE_FLAME_LAG_MS;
                let size_max =
                    ((r.size * 1.5).clamp(60.0, 120.0) + self.rand() * 25.0).min(MAX_SIZE);
                // The direction: a random share of `right` off the face's
                // angles on each axis, plus, off a wall, 0.25..0.7 of the
                // normal on each axis; normalized (0x3004a000 vectoangles,
                // 0x3004a200 AngleVectors). A floor's `right` is -y.
                let n = r.normal;
                let yaw = if n.x == 0.0 && n.y == 0.0 {
                    0.0
                } else {
                    n.y.atan2(n.x)
                };
                let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
                let mut dir = Vec3::from_array(
                    [right.x, right.y, right.z].map(|a| (2.0 * self.rand() - 1.0) * a),
                );
                let wall = 1.0 - n.z.abs();
                dir += Vec3::from_array(
                    [n.x, n.y, n.z].map(|a| wall * (self.rand() * 0.45 + 0.25) * a),
                );
                let frac = ((r.next_emit - r.born) / (r.death - r.born)) as f32;
                // Units per 1000 ticks (500 ms): `(1 - frac) * rand * |+0xa8|
                // + 2`.
                let speed = ((1.0 - frac) * self.rand() * r.sink.abs() + 2.0) * 2.0;
                let roll = self.rand() * 360.0;
                self.chunks.push(Chunk {
                    owner: r.owner,
                    kind: Kind::ResidueFlame,
                    pos: r.pos,
                    // Based at the step (0x30028816); its look-ahead goes
                    // on from the contact the residue was left at.
                    base: r.pos,
                    base_at: r.next_emit,
                    seen: r.pos,
                    seen_at: r.born,
                    dir: dir.normalize_or_zero(),
                    speed,
                    size: size_max * 0.5,
                    size_max,
                    size_rand: 0.0,
                    born: r.next_emit - f64::from(lag),
                    life: RESIDUE_FLAME_LIFE_MS,
                    roll,
                    moved: now - f64::from(dt_ms),
                    own: false,
                    trail: None,
                    may_residue: false,
                    // The parent's less 20, less 5 times `1 - normal z`.
                    a8: (r.sink - 20.0) - (1.0 - n.z) * 5.0,
                    friction: 0.0,
                    chain: None,
                    drawn: false,
                    // A copy of the residue, a copy of the chunk at the
                    // contact.
                    normal: n,
                });
                if let Some(w) = world
                    && r.death - now >= RESIDUE_FACE_MS
                {
                    let (tr, _) = w.trace(r.pos, r.pos - n * RESIDUE_FACE_DEPTH, r.owner);
                    if !tr.allsolid && tr.fraction == 1.0 {
                        self.residues[i].death = now;
                    }
                }
                let step = RESIDUE_STEP_MS * (0.5 + self.rand());
                self.residues[i].next_emit += f64::from(step);
            }
        }
    }

    /// The chain draw (0x30027d10's last pass, 0x300272b0 from each chain's
    /// newest chunk): a stream's chunks link newest to oldest, and a dead
    /// one ends its chain, the older ones going with it (0x30027357;
    /// 0x300256e0 frees the tail). A live chain, its owner's latest that
    /// fired this frame, ages its chunks from its newest's birth, any other
    /// from the clock, and a chunk no older than that is not drawn
    /// (0x300274d2..0x300274f7). A drawn chunk first takes in the older
    /// ones it [`overlaps`]; another body's live stream then skips one
    /// [`hidden_behind`] the last drawn. Every other chunk is a chain of
    /// its own.
    fn draw_chains(&mut self, now: f64, eye: Vec3) {
        let mut chains: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, c) in self.chunks.iter_mut().enumerate() {
            match c.chain {
                Some((id, ..)) if c.kind == Kind::Stream => chains.entry(id).or_default().push(i),
                _ => c.drawn = now > c.born,
            }
        }
        let alive = |c: &Chunk| ((now - c.born) as f32) < c.life;
        let mut gone = vec![false; self.chunks.len()];
        for (id, links) in chains {
            let mut links: Vec<usize> = links.into_iter().rev().collect();
            if let Some(cut) = links.iter().position(|&i| !alive(&self.chunks[i])) {
                for &i in &links[cut..] {
                    gone[i] = true;
                }
                links.truncate(cut);
            }
            let Some(&newest) = links.first() else {
                continue;
            };
            let head = self.chunks[newest];
            let live = self
                .owners
                .get(&head.owner)
                .is_some_and(|o| o.chain == id && o.fired_frame == self.frame);
            let t0 = if live { head.born } else { now };
            let mut last: Option<Chunk> = None;
            let mut k = 0;
            while let Some(&i) = links.get(k) {
                k += 1;
                let mut c = self.chunks[i];
                c.drawn = false;
                if t0 - c.born > 0.0 {
                    while let Some(&j) = links.get(k)
                        && overlaps(&c, &self.chunks[j])
                    {
                        merge(&mut c, &self.chunks[j]);
                        gone[j] = true;
                        k += 1;
                    }
                    c.drawn =
                        !(live && !c.own && last.is_some_and(|l| hidden_behind(&c, &l, now, eye)));
                    if c.drawn {
                        last = Some(c);
                    }
                }
                self.chunks[i] = c;
            }
        }
        let mut i = 0;
        self.chunks.retain(|c| {
            i += 1;
            !gone[i - 1] && alive(c)
        });
    }

    /// The cgame's flame report for this frame at `now` (ms), run after
    /// [`Self::step`] as `CG_AddFlameChunks` (uo cgame 0x30027d10) tests
    /// while it draws. Once `victim`'s check is due the flag drops, and
    /// each drawn fire chunk short of 0.9 of its life is tested one in five
    /// (0x300268e0) until one reaches the victim's box (0x300265c0); a hit
    /// raises the flag and, two seconds past the last, names the chunk's
    /// owner for `fdc` (0x300291c0). The next check is 50 ms on, and the
    /// flag holds in between. `None` for a victim keeps the flag down.
    /// `fdc` is subject to the server's flood window, so while
    /// `flood_open` the report holds it back for a later check.
    pub fn report(
        &mut self,
        now: f64,
        victim: Option<Victim>,
        flood_open: bool,
        trace: Option<ShotTrace>,
    ) -> FlameReport {
        let Some(v) = victim else {
            self.in_flame = false;
            return FlameReport::default();
        };
        let clock = *self.report_clocks.entry(v.client).or_insert(ReportClock {
            next_check: f64::NEG_INFINITY,
            last_fdc: f64::NEG_INFINITY,
        });
        let mut fdc = None;
        if clock.next_check < now {
            self.in_flame = false;
            for i in 0..self.chunks.len() {
                if self.in_flame {
                    break;
                }
                let c = self.chunks[i];
                if !c.drawn || c.kind == Kind::Trail || (self.rand() * 5.0) as u32 != 0 {
                    continue;
                }
                let frac = (now - c.born) as f32 / c.life;
                if frac >= MAX_FRAC || !self.reaches(&c, frac, now, &v, trace) {
                    continue;
                }
                self.in_flame = true;
                if clock.last_fdc <= now - f64::from(FDC_MS) && !flood_open {
                    self.report_clocks.get_mut(&v.client).unwrap().last_fdc = now;
                    fdc = Some(c.owner);
                }
            }
            self.report_clocks.get_mut(&v.client).unwrap().next_check = now + f64::from(CHECK_MS);
        }
        FlameReport {
            in_flame: self.in_flame,
            fdc,
        }
    }

    /// 0x300265c0's test of one chunk: within its reach of the nearest
    /// point of the victim's box, with a line to it from the box's axis at
    /// the chunk's height (clamped a unit inside the box) that is clear or
    /// ends on the victim itself.
    fn reaches(
        &mut self,
        c: &Chunk,
        frac: f32,
        now: f64,
        v: &Victim,
        trace: Option<ShotTrace>,
    ) -> bool {
        if c.pos == Vec3::ZERO {
            return false;
        }
        let own = c.owner == v.client;
        let mut reach = c.size * REACH;
        if own {
            reach *= 0.5;
        }
        if reach < MIN_REACH {
            reach = MIN_REACH;
        }
        if let Some((t, at)) = self.last_test
            && t == now
            && at.distance(c.pos) < reach
        {
            return false;
        }
        self.last_test = Some((now, c.pos));
        if own && frac <= OWN_MIN_FRAC {
            return false;
        }
        let near = v.origin + (c.pos - v.origin).clamp(v.mins, v.maxs);
        if near.distance(c.pos) >= reach {
            return false;
        }
        let dz = c.pos.z - v.origin.z;
        let z = if dz > v.maxs.z - 1.0 {
            v.origin.z + v.maxs.z - 1.0
        } else if dz < v.mins.z + 1.0 {
            v.origin.z + v.mins.z + 1.0
        } else {
            c.pos.z
        };
        let start = Vec3::new(v.origin.x, v.origin.y, z);
        trace.is_none_or(|t| {
            let (fraction, entity) = t(start, c.pos);
            fraction >= 1.0 || entity == v.client
        })
    }

    /// One stream chunk leaving `n` at `born` along `dir`, its chain angles
    /// `aim` (0x30024050's chain branch, `+0xb8` 1): 1800 units a second
    /// with the owner's velocity share `vel` taken in (0x30024d13..
    /// 0x30024d5d, the speed kept), a cap of 140 less its size draw and a
    /// start of 16 less 0.9 (own view) or 0.6 of that draw's share of 140
    /// (0x30024b12..0x30024c1d).
    fn emit(
        &mut self,
        n: &Nozzle,
        aim: Vec2,
        dir: Vec3,
        vel: Vec3,
        born: f64,
        world: Option<&ChunkWorld>,
    ) {
        let size_rand = self.size_rand(born);
        let k = if n.own_view { 0.9 } else { 0.6 };
        let roll = self.rand() * 360.0;
        // A nozzle inside a wall (`CG_FireFlameChunks`' ±2 box trace) lays
        // the chunk at the wall, still, for half a life.
        let blocked = world.map(|w| {
            let tr = w
                .world
                .point_trace(n.pos - dir * 16.0, n.pos, CONTENTS_SOLID, false);
            tr.fraction < 1.0
        });
        let (speed, life) = match blocked {
            Some(true) => (0.0, LIFE_MS * 0.5),
            _ => (START_SPEED, LIFE_MS),
        };
        let o = self.owners[&n.owner];
        let step_k = trail_share(n.own_view, o.run_ms, o.turn_rate);
        let first = born + f64::from(self.rand()) * f64::from(life) * step_k;
        self.chunks.push(Chunk {
            owner: n.owner,
            kind: Kind::Stream,
            pos: n.pos,
            base: n.pos,
            base_at: born,
            seen: n.pos,
            seen_at: born,
            dir: (dir * START_SPEED * 0.5 + vel).normalize_or(dir),
            speed,
            size: (1.0 - k * size_rand / 140.0) * START_SIZE,
            size_max: 140.0 - size_rand,
            size_rand,
            born,
            life,
            roll,
            moved: born,
            own: n.own_view,
            trail: Some((first, step_k)),
            may_residue: true,
            a8: STREAM_A8,
            friction: 1.0,
            chain: Some((o.chain, aim, n.pos)),
            drawn: false,
            normal: Vec3::ZERO,
        });
    }

    /// A stream chunk's size draw `+0x60` at its birth: `1.1 * 140` times
    /// two waves of the chunk clock and a draw, at most 110
    /// (0x30024b01..0x30024bb0; 0x3004d200 is [`wave`]).
    fn size_rand(&mut self, born: f64) -> f32 {
        let ticks = born * 2.0;
        let r = self.rand() * 0.5;
        let a = wave(4.723_876, 6.522_44, ticks);
        let b = wave(10.453_76, 11.476_52, ticks);
        ((0.75 + 0.35) * ((b + 1.0) * 0.125 + (a * 0.5 + 1.0) * 0.25 + r) * 140.0).min(110.0)
    }

    /// Camera-facing sprites of the chunks the chain draw drew, placed by
    /// [`sprite`]: `flamethrowerFire{2..44}` by the chunk's share of its
    /// life (0x300268e0), smoke as `flameSmoke{1..6}` ([`colour`]).
    /// `halve` is the cgame's `r_fullscreen` and `r_overbrightbits` test.
    pub fn quads(&self, now: f64, view: &SpriteView, halve: bool) -> Vec<FxQuad> {
        let (cam_right, cam_up) = (view.right, view.up);
        self.chunks
            .iter()
            .filter(|c| c.drawn)
            .filter_map(|c| {
                let (p, r) = sprite(c, view.eye, view.forward)?;
                let frac = ((now - c.born) as f32 / c.life).clamp(0.0, 1.0);
                let shader = match c.kind {
                    Kind::Smoke(n) => format!("flameSmoke{n}"),
                    _ => {
                        let frame = (frac * FRAMES).floor().clamp(0.0, FRAMES - 1.0) as i32 + 2;
                        format!("flamethrowerFire{frame}")
                    }
                };
                let (s, co) = c.roll.to_radians().sin_cos();
                let right = (cam_right * co + cam_up * s) * r;
                let up = (cam_up * co - cam_right * s) * r;
                Some(FxQuad {
                    verts: [
                        (p - right + up).to_array(),
                        (p + right + up).to_array(),
                        (p + right - up).to_array(),
                        (p - right - up).to_array(),
                    ],
                    uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    rgba: colour(c.kind, frac, halve),
                    shader: shader.into(),
                })
            })
            .collect()
    }
}

/// The view the flame sprites face: `cg.refdef`'s `vieworg` and axes.
#[derive(Clone, Copy, Debug)]
pub struct SpriteView {
    pub eye: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

/// Where the sprite add (0x300268e0) centres a chunk's sprite and its
/// radius, `None` when it is not drawn. The radius is half the size. A
/// chunk whose last struck face looks down sits `20 * |normal z|` lower
/// (0x300269e1..0x30026a47). The view's own chunk is pulled its radius
/// toward the eye and shrunk alike, so it covers the same screen; one
/// within its radius of the eye goes 20 behind it (0x30026a4b..0x30026ae5).
/// The radius is held to 0.5 (0x30026aef..0x30026b00), a sprite whose
/// centre is not ahead of the eye is dropped (0x30026d41..0x30026d88),
/// and the quad is raised by `0.55 * min(radius, 72.5) - 8.8`
/// (0x30027005..0x300270cb).
fn sprite(c: &Chunk, eye: Vec3, forward: Vec3) -> Option<(Vec3, f32)> {
    let mut p = c.pos;
    let mut r = c.size * 0.5;
    if c.normal != Vec3::ZERO && c.normal.z < 0.0 {
        p.z -= c.normal.z.abs() * 20.0;
    }
    if c.own {
        let to = p - eye;
        let len = to.length();
        let pull = if len <= r { -len - 20.0 } else { -r };
        p += to.normalize_or_zero() * pull;
        r *= pull / len + 1.0;
    }
    r = r.max(0.5);
    if forward.dot(p - eye) <= 0.0 {
        return None;
    }
    p.z += r.min(72.5) * 0.55 - 8.8;
    Some((p, r))
}

/// A drawn chunk's vertex colour (0x30026b08..0x30026c4f): white for a
/// stream chunk and its trail; a residue's flame greys up from black over
/// the first half of its life; smoke is `g` times (249, 239, 219) / 255,
/// `g` 100 over its first half falling to 20, with an alpha of 76.5 times
/// the chain draw's half tent of its life (0x300277a9..0x30027819).
/// Truncated to bytes as `_ftol` does. `halve` shifts all but smoke's
/// grey bytes right by one (0x30026c53..0x30026c84), on top of the
/// renderer's `rgbGen vertex` shift.
fn colour(kind: Kind, frac: f32, halve: bool) -> [f32; 4] {
    let byte = |v: f32| f32::from(v as u8 >> u8::from(halve)) / 255.0;
    match kind {
        Kind::Stream | Kind::Trail => {
            let g = byte(255.0);
            [g, g, g, 1.0]
        }
        Kind::ResidueFlame => {
            let g = byte((2.0 * frac).min(1.0) * 255.0);
            [g, g, g, 1.0]
        }
        Kind::Smoke(_) => {
            let byte = |v: f32| f32::from(v as u8) / 255.0;
            let g = f32::from(((1.0 - 2.0 * (frac - 0.5).max(0.0)) * 80.0 + 20.0) as u8);
            let tent = if frac < 0.5 {
                2.0 * frac
            } else {
                2.0 - 2.0 * frac
            };
            [
                byte(g * 0.976_470_6),
                byte(g * 0.937_254_9),
                byte(g * 0.858_823_5),
                byte(tent * 0.5 * 76.5),
            ]
        }
    }
}

/// This frame's flamethrower nozzles on a United Offensive connection: the
/// ridden body off the playerstate, every other player body off its
/// entity, each whose `eFlags` firing bit is up with a `gas` weapon in
/// hand, at its muzzle (the view's for the ridden body).
pub fn nozzles(
    p: &Cod1Wire,
    snap: &Snapshot,
    ps_client: i32,
    weapons: &[Option<WeaponDef>],
    muzzles: &HashMap<u32, (Vec3, Vec3)>,
) -> Vec<Nozzle> {
    if !p.is_uo() {
        return Vec::new();
    }
    let gas = |w: i32| {
        usize::try_from(w)
            .ok()
            .and_then(|w| weapons.get(w)?.as_ref())
            .is_some_and(|d| d.weapon_type == "gas")
    };
    let mut out = Vec::new();
    let mut add = |owner: u32, key: u32, own_view: bool| {
        if let Some(&(pos, dir)) = muzzles.get(&key) {
            out.push(Nozzle {
                owner,
                pos,
                dir,
                own_view,
                ps_velocity: match own_view {
                    true => Vec3::from_array(
                        ["velocity[0]", "velocity[1]", "velocity[2]"]
                            .map(|f| snap.ps.field_f32(p, f)),
                    ),
                    false => Vec3::ZERO,
                },
            });
        }
    };
    let ridden = u32::try_from(ps_client).ok();
    if let Some(own) = ridden
        && snap.ps.field_i32(p, "eFlags") & EF_FIRING != 0
        && gas(snap.ps.field_i32(p, "weapon"))
    {
        add(own, own, true);
    }
    for (&num, ent) in &snap.entities {
        if Some(num) != ridden
            && ent.field_i32(p, "eType") == ET_PLAYER
            && ent.field_i32(p, "eFlags") & EF_FIRING != 0
            && gas(ent.field_i32(p, "weapon"))
        {
            add(num, num, false);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with no bodies and no local player.
    fn bare(world: &CollisionWorld) -> ChunkWorld<'_> {
        ChunkWorld {
            world,
            bodies: &[],
            local: None,
            max_clients: 64,
        }
    }

    fn nozzle(owner: u32) -> Nozzle {
        Nozzle {
            owner,
            pos: Vec3::ZERO,
            dir: Vec3::X,
            own_view: true,
            ps_velocity: Vec3::ZERO,
        }
    }

    fn fire(owner: u32, pos: Vec3, size: f32, born: f64, life: f32) -> Chunk {
        Chunk {
            owner,
            kind: Kind::Stream,
            pos,
            base: pos,
            base_at: born,
            seen: pos,
            seen_at: born,
            dir: Vec3::X,
            speed: 0.0,
            size,
            size_max: size,
            size_rand: 0.0,
            born,
            life,
            roll: 0.0,
            moved: born,
            own: false,
            trail: None,
            may_residue: false,
            a8: 0.0,
            friction: 0.0,
            chain: None,
            drawn: true,
            normal: Vec3::ZERO,
        }
    }

    /// A standing player at the origin, slot 2.
    const VICTIM: Victim = Victim {
        client: 2,
        origin: Vec3::ZERO,
        mins: Vec3::new(-15.0, -15.0, 0.0),
        maxs: Vec3::new(15.0, 15.0, 70.0),
    };

    /// 0x300265c0: a chunk reaches the player when it is within 0.4 of its
    /// size (half that for the player's own, never under 14) of the box's
    /// nearest point, and the player's own only past 0.4 of its life.
    #[test]
    fn a_chunk_reaches_the_box_within_its_reach() {
        let mut f = FlameSystem::default();
        let mut reaches = |owner: u32, pos: Vec3, size: f32, frac: f32, now: f64| {
            let c = fire(owner, pos, size, 0.0, 1000.0);
            f.reaches(&c, frac, now, &VICTIM, None)
        };
        // Size 100 reaches 40: 35 off the side is in, 45 is out.
        assert!(reaches(7, Vec3::new(50.0, 0.0, 30.0), 100.0, 0.1, 1.0));
        assert!(!reaches(7, Vec3::new(60.0, 0.0, 30.0), 100.0, 0.1, 2.0));
        // Over the head the box's top is the nearest point.
        assert!(reaches(7, Vec3::new(0.0, 0.0, 105.0), 100.0, 0.1, 3.0));
        assert!(!reaches(7, Vec3::new(0.0, 0.0, 115.0), 100.0, 0.1, 4.0));
        // A small chunk still reaches 14.
        assert!(reaches(7, Vec3::new(28.0, 0.0, 30.0), 10.0, 0.1, 5.0));
        assert!(!reaches(7, Vec3::new(30.0, 0.0, 30.0), 10.0, 0.1, 6.0));
        // The player's own flame reaches half as far, and only once past
        // 0.4 of its life.
        assert!(!reaches(2, Vec3::new(50.0, 0.0, 30.0), 100.0, 0.5, 7.0));
        assert!(reaches(2, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.5, 8.0));
        assert!(!reaches(2, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.4, 9.0));
        // The world origin is never tested.
        assert!(!reaches(7, Vec3::ZERO, 100.0, 0.1, 10.0));
        // In one frame a chunk near the last one tested is skipped.
        assert!(reaches(7, Vec3::new(40.0, 0.0, 30.0), 100.0, 0.1, 11.0));
        assert!(!reaches(7, Vec3::new(40.0, 5.0, 30.0), 100.0, 0.1, 11.0));
        assert!(reaches(7, Vec3::new(40.0, 5.0, 30.0), 100.0, 0.1, 12.0));
    }

    /// 0x30027d10 and 0x300291c0: checks every 50 ms raise the flag the
    /// cmds carry and name the owner for `fdc` at most every 2 s; a check
    /// with nothing in reach drops the flag, and inside the flood window
    /// `fdc` waits for a later check.
    #[test]
    fn the_report_holds_the_flag_and_spaces_fdc_two_seconds() {
        let mut f = FlameSystem::default();
        f.chunks
            .push(fire(7, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.0, 1e9));
        let mut sent = Vec::new();
        let mut flagged = 0;
        let mut checks = 0;
        let mut now = 10_000.0;
        while now < 15_000.0 {
            let due = f
                .report_clocks
                .get(&VICTIM.client)
                .is_none_or(|c| c.next_check < now);
            let r = f.report(now, Some(VICTIM), false, None);
            checks += due as u32;
            flagged += r.in_flame as u32;
            if let Some(owner) = r.fdc {
                assert_eq!(owner, 7);
                sent.push(now);
            }
            now += 16.0;
        }
        // A check every 64 ms at a 16 ms frame (the first frame past 50).
        assert_eq!(checks, 79);
        assert!(flagged > 0);
        assert!(sent.len() >= 2, "{sent:?}");
        assert!(
            sent.windows(2).all(|w| w[1] - w[0] >= f64::from(FDC_MS)),
            "{sent:?}"
        );

        // The flag holds between checks and drops at the next one.
        f.chunks.clear();
        let held = f.report(now, Some(VICTIM), false, None);
        let mut t = now;
        while f.report(t, Some(VICTIM), false, None).in_flame {
            t += 16.0;
        }
        assert!(
            t - now <= f64::from(CHECK_MS) + 16.0,
            "{held:?} {}",
            t - now
        );

        // Inside the flood window the flag rises but `fdc` waits.
        let mut f = FlameSystem::default();
        f.chunks
            .push(fire(7, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.0, 1e9));
        let mut now = 10_000.0;
        let mut flag = false;
        while now < 12_000.0 {
            let r = f.report(now, Some(VICTIM), true, None);
            assert_eq!(r.fdc, None);
            flag |= r.in_flame;
            now += 16.0;
        }
        assert!(flag);
        let mut fdc = None;
        while fdc.is_none() && now < 14_000.0 {
            fdc = f.report(now, Some(VICTIM), false, None).fdc;
            now += 16.0;
        }
        assert_eq!(fdc, Some(7));
    }

    /// A retail UO client's frames in a flame stream (coduo-diff.md 19.3.1):
    /// with fire in reach on the frames whose cmd carried the flag, the
    /// report raises it on the same frames and sends `fdc` on the same ones,
    /// 2014 to 2127 ms apart on the 56 ms check grid of 8 ms frames.
    #[test]
    fn the_report_replays_a_retail_clients_flag_and_fdc_frames() {
        let text = include_str!("../../tests/fixtures/uo_fdc_retail.txt");
        let rows =
            |key: &str| -> String { text.lines().filter_map(|l| l.strip_prefix(key)).collect() };
        let start: f64 = rows("start ").trim().parse().unwrap();
        let mut times = vec![start];
        for c in rows("steps ").chars() {
            times.push(times.last().unwrap() + f64::from(c.to_digit(10).unwrap()));
        }
        let flags: Vec<bool> = rows("flags ").chars().map(|c| c == '1').collect();
        let sent: Vec<f64> = rows("fdc ")
            .split(' ')
            .map(|t| t.parse().unwrap())
            .collect();
        assert_eq!(times.len(), flags.len());
        let mut f = FlameSystem::default();
        let mut got = Vec::new();
        for (&now, &flag) in times.iter().zip(&flags) {
            f.chunks.clear();
            if flag {
                f.chunks
                    .extend((0..64).map(|_| fire(7, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.0, 1e9)));
            }
            let r = f.report(now, Some(VICTIM), false, None);
            assert_eq!(r.in_flame, flag, "flag at {now}");
            if r.fdc.is_some() {
                got.push(now);
            }
        }
        assert_eq!(got, sent);
    }

    /// 0x3002693c..0x3002698c: a trail flame never burns, nor any chunk
    /// at 0.9 of its life; smoke does.
    #[test]
    fn trail_flames_and_spent_chunks_never_report_but_smoke_does() {
        let burns = |c: Chunk| {
            let mut f = FlameSystem::default();
            f.chunks.push(c);
            (0..200).any(|i| {
                f.report(900.0 + 16.0 * f64::from(i), Some(VICTIM), false, None)
                    .in_flame
            })
        };
        let near = fire(7, Vec3::new(20.0, 0.0, 30.0), 100.0, 0.0, 1e9);
        assert!(burns(near));
        assert!(burns(Chunk {
            kind: Kind::Smoke(1),
            ..near
        }));
        assert!(!burns(Chunk {
            kind: Kind::Trail,
            ..near
        }));
        assert!(!burns(Chunk {
            life: 1000.0,
            ..near
        }));
    }

    #[test]
    fn a_stream_starts_holds_and_cools_down() {
        let mut f = FlameSystem::default();
        let start = f.step(1000.0, 16.0, &[nozzle(3)], Vec3::ZERO, None);
        assert!(matches!(
            start[..],
            [
                FlameSound::Start { owner: 3, .. },
                FlameSound::Fire { owner: 3, .. }
            ]
        ));
        // The start frame lays only its type 1 head, which is never drawn.
        assert!(f.chunks.is_empty());
        let held = f.step(1016.0, 16.0, &[nozzle(3)], Vec3::ZERO, None);
        assert!(matches!(held[..], [FlameSound::Fire { owner: 3, .. }]));
        // Two at a 6.7 ms spacing from the head's birth in a 16 ms frame.
        let born: Vec<_> = f.chunks.iter().map(|c| c.born).collect();
        assert_eq!(born.len(), 2);
        assert!((born[0] - (1000.0 + 0.75 * f64::from(SPACING_MS))).abs() < 1e-4);
        // Released: the grace runs out, then the cooldown.
        assert!(f.step(1032.0, 16.0, &[], Vec3::ZERO, None).is_empty());
        let stop = f.step(1100.0, 16.0, &[], Vec3::ZERO, None);
        assert!(matches!(stop[..], [FlameSound::Stop { owner: 3, .. }]));
    }

    /// 0x30024289..0x300249e0: the view's own moving body tilts its chunks
    /// by 0.375 of `ps.velocity` against the nozzle's 900 a 1000 ticks;
    /// another body's chunks take its nozzle's move, and each chunk of a
    /// frame takes the nozzle and that share lerped from the last frame's
    /// by its birth, and the two frames' forwards lerped. The speed stays
    /// the nozzle's.
    #[test]
    fn a_stream_chunk_takes_in_its_owners_velocity_and_the_frames_lerp() {
        let mut f = FlameSystem::default();
        let own = Nozzle {
            ps_velocity: Vec3::new(0.0, 200.0, 0.0),
            ..nozzle(1)
        };
        f.step(100.0, 16.0, &[own], Vec3::ZERO, None);
        f.step(116.0, 16.0, &[own], Vec3::ZERO, None);
        let c = f.chunks[0];
        assert!((c.dir - Vec3::new(900.0, 75.0, 0.0).normalize()).length() < 1e-6);
        // The nozzle's speed, braked since its birth as a steady stream's.
        let braked = START_SPEED - FRICTION * 0.182_305_6 * (116.0 - c.born) as f32;
        assert!((c.speed - braked).abs() < 1e-2, "{}", c.speed);

        let mut f = FlameSystem::default();
        let mut far = Nozzle {
            own_view: false,
            ..nozzle(3)
        };
        f.step(100.0, 20.0, &[far], Vec3::ZERO, None);
        far.pos = Vec3::new(0.0, 20.0, 0.0);
        far.dir = Vec3::Y;
        f.step(120.0, 20.0, &[far], Vec3::ZERO, None);
        // 20 units in 40 ticks is 500 a 1000 ticks, halved; the start
        // frame's share was 0.
        let vel = 250.0;
        let fresh: Vec<_> = f.chunks.iter().filter(|c| c.kind == Kind::Stream).collect();
        assert!(fresh.len() > 2);
        for c in fresh {
            let share = ((c.born - 100.0) / 20.0) as f32;
            let (_, aim, at) = c.chain.unwrap();
            assert!((at.y - 20.0 * share).abs() < 1e-4, "{at} at {share}");
            assert!((aim.y - 90.0 * share).abs() < 1e-3, "{aim} at {share}");
            let fwd = Vec3::X.lerp(Vec3::Y, share).normalize();
            let want = (fwd * 900.0 + Vec3::Y * vel * share).normalize();
            assert!((c.dir - want).length() < 1e-5, "{} against {want}", c.dir);
        }
    }

    /// 0x300248e4..0x30024947: a frame's chunks lerp their chain angles as
    /// plain angles, so an aim crossing yaw 180 sweeps them the long way
    /// round through 0, and those brake at the whole friction against the
    /// latest aim, while their direction lerps the two forwards the short
    /// way (0x30024969..0x300249e0).
    #[test]
    fn an_aim_across_yaw_180_sweeps_the_chain_angles_the_long_way() {
        let mut f = FlameSystem::default();
        let mut n = Nozzle {
            own_view: false,
            dir: dir_of(Vec2::new(0.0, 170.0)),
            ..nozzle(3)
        };
        f.step(100.0, 20.0, &[n], Vec3::ZERO, None);
        n.dir = dir_of(Vec2::new(0.0, -170.0));
        f.step(120.0, 20.0, &[n], Vec3::ZERO, None);
        let mid = f
            .chunks
            .iter()
            .filter(|c| c.kind == Kind::Stream)
            .min_by(|a, b| (a.born - 110.0).abs().total_cmp(&(b.born - 110.0).abs()))
            .unwrap();
        let share = ((mid.born - 100.0) / 20.0) as f32;
        let (_, aim, at) = mid.chain.unwrap();
        assert!((aim.y - (170.0 - 340.0 * share)).abs() < 1e-3, "{aim}");
        assert!(aim.y.abs() < 20.0, "{aim}");
        assert!(aim_of(mid.dir).y.abs() > 170.0, "{}", mid.dir);
        let latest = (Vec2::new(0.0, -170.0), at);
        assert_eq!(chain_brake((aim, at), latest), 1.0);
    }

    /// The fx clock is f64: 22 h in, a held stream sheds the
    /// chunks it sheds 1 s in, where f32 ms spaced them 8 ms apart at best.
    #[test]
    fn a_stream_sheds_alike_a_day_into_the_clock() {
        let shed = |base: f64| {
            let mut f = FlameSystem::default();
            for k in 0..30 {
                f.step(
                    base + 16.0 * f64::from(k),
                    16.0,
                    &[nozzle(3)],
                    Vec3::ZERO,
                    None,
                );
            }
            f.chunks.iter().filter(|c| c.kind == Kind::Stream).count()
        };
        assert_eq!(shed(80e6), shed(1000.0));
    }

    /// A held stream's chunk brakes at 0.18 of the full friction while its
    /// stream fires on and its aim holds (0x30025e45), grows toward its
    /// cap, sags while fast (0x30025990) and plays through its frames.
    #[test]
    fn chunks_slow_grow_play_through_their_frames_and_die() {
        let mut f = FlameSystem::default();
        f.step(990.0, 10.0, &[nozzle(1)], Vec3::ZERO, None);
        f.step(1000.0, 10.0, &[nozzle(1)], Vec3::ZERO, None);
        let first = f.chunks[0];
        f.step(1200.0, 200.0, &[], Vec3::ZERO, None);
        let c = f
            .chunks
            .iter()
            .find(|c| c.kind == Kind::Stream && c.born == first.born)
            .unwrap();
        let share = chain_brake((Vec2::ZERO, Vec3::ZERO), (Vec2::ZERO, Vec3::ZERO));
        assert!((share - 0.182_305_6).abs() < 1e-6, "{share}");
        // Released last frame, its stream still holds.
        let age = (1200.0 - c.born) as f32;
        assert!((c.speed - (START_SPEED - FRICTION * share * age)).abs() < 1e-2);
        assert!(c.size > first.size && c.size <= c.size_max);
        // Placed off its birth at its present speed.
        let x = c.speed * age / 1000.0;
        assert!((c.pos.x - x).abs() < 1e-2, "{} {x}", c.pos.x);
        assert!(c.pos.z < -1.0, "{}", c.pos.z);
        let view = SpriteView {
            eye: Vec3::ZERO,
            forward: Vec3::X,
            right: Vec3::NEG_Y,
            up: Vec3::Z,
        };
        let q = f.quads(1200.0, &view, false);
        assert!(
            q.iter().any(|q| &*q.shader == "flamethrowerFire9"),
            "{:?}",
            q.iter().map(|q| &q.shader).collect::<Vec<_>>()
        );
        f.step(2200.0, 1000.0, &[], Vec3::ZERO, None);
        assert!(f.chunks.iter().all(|c| matches!(c.kind, Kind::Smoke(_))));
    }

    /// 0x30025e45..0x30025f8f: the share of the friction grows with the
    /// aim's turn (or a twentieth of the nozzle's move) since the chunk
    /// left, to the whole at 30.5; a chunk of a stream that has ended
    /// brakes at 0.35, a residue flame at the whole.
    #[test]
    fn a_stream_chunk_brakes_harder_as_its_aim_turns_and_once_released() {
        let held = (Vec2::new(0.0, 90.0), Vec3::ZERO);
        let at = |pitch: f32, yaw: f32, x: f32| (Vec2::new(pitch, yaw), Vec3::new(x, 0.0, 0.0));
        let share = |now| chain_brake(held, now);
        assert!((share(at(0.0, 105.0, 0.0)) - 21.8 / 37.3).abs() < 1e-5);
        assert!((share(at(0.0, 90.0, 200.0)) - 16.8 / 37.3).abs() < 1e-5);
        assert_eq!(share(at(30.5, 90.0, 0.0)), 1.0);
        // Across the wrap: -175 to 175 is a turn of 10.
        let wrap = chain_brake((Vec2::new(0.0, -175.0), Vec3::ZERO), at(0.0, 175.0, 0.0));
        assert!((wrap - 16.8 / 37.3).abs() < 1e-5);

        let chunk = Chunk {
            speed: 1000.0,
            friction: 1.0,
            chain: Some((4, held.0, held.1)),
            ..fire(7, Vec3::ZERO, 50.0, 0.0, LIFE_MS)
        };
        let slowed = |c: Chunk, streams: &Streams| {
            let mut c = c;
            move_chunk(&mut c, 10.0, None, streams, &mut None);
            (1000.0 - c.speed) / (FRICTION * 10.0)
        };
        let firing = Streams::from([(7, (4, held.0, held.1))]);
        assert!((slowed(chunk, &firing) - 0.182_305_6).abs() < 1e-4);
        let next = Streams::from([(7, (5, held.0, held.1))]);
        assert!((slowed(chunk, &next) - LOOSE_FRICTION).abs() < 1e-4);
        assert!((slowed(chunk, &Streams::new()) - LOOSE_FRICTION).abs() < 1e-4);
        let residue = Chunk {
            kind: Kind::ResidueFlame,
            chain: None,
            ..chunk
        };
        assert!((slowed(residue, &firing) - 1.0).abs() < 1e-4);
    }

    /// 0x30025990: a fresh stream chunk at the nozzle's speed sags at
    /// nearly its whole `+0xa8`; one slowed to the floor and grown large
    /// climbs.
    #[test]
    fn a_fast_stream_chunk_sags_and_a_slow_large_one_climbs() {
        let c = |speed: f32, size: f32| Chunk {
            speed,
            a8: STREAM_A8,
            ..fire(7, Vec3::ZERO, size, 0.0, LIFE_MS)
        };
        assert!(lift(&c(START_SPEED, 16.0)) > 55.0);
        assert!(lift(&c(MIN_SPEED, 140.0)) < -10.0);
        let flame = Chunk {
            kind: Kind::ResidueFlame,
            a8: -20.2,
            ..c(MIN_SPEED, 140.0)
        };
        assert_eq!(lift(&flame), -20.2);
    }

    /// 0x30024b01..0x30024c1d: the size draw is `154 * (0.125 * (b + 1) +
    /// 0.25 * (a / 2 + 1) + rand / 2)` held to 110, `a` and `b` the chunk
    /// clock's two waves, both 0 at tick 0; the cap is 140 less it, the
    /// start 16 less 0.9 (own view) or 0.6 of its share of 140.
    #[test]
    fn a_stream_chunks_size_follows_the_two_waves() {
        assert_eq!(wave(4.723_876, 6.522_44, 0.0), 0.0);
        let mut f = FlameSystem::default();
        let mut remote = nozzle(3);
        remote.own_view = false;
        f.step(0.0, 0.0, &[nozzle(1), remote], Vec3::ZERO, None);
        for c in &f.chunks {
            assert!((57.75..=110.0).contains(&c.size_rand), "{}", c.size_rand);
            assert_eq!(c.size_max, 140.0 - c.size_rand);
            let k = if c.owner == 1 { 0.9 } else { 0.6 };
            assert_eq!(c.size, (1.0 - k * c.size_rand / 140.0) * START_SIZE);
        }
    }

    /// 0x30024db8..0x30025045: a steady stream's first trail share is
    /// 0.35 over 1 at its start, 0.75 a second on; a fast turn spaces the
    /// steps to their floor, 0.06 for the view's own stream, 0.04 for
    /// another body's.
    #[test]
    fn a_stream_chunks_first_trail_share_follows_the_run_and_the_turn() {
        assert_eq!(trail_share(true, 0.0, 0.0), 0.35);
        assert!((trail_share(false, 1000.0, 0.0) - 0.35 / 0.75).abs() < 1e-12);
        assert!((trail_share(false, 4000.0, 0.0) - 0.35 / 0.75).abs() < 1e-12);
        assert!((trail_share(false, 0.0, 16.0) - 0.35 / 2.0).abs() < 1e-12);
        assert_eq!(trail_share(true, 0.0, 200.0), 0.06);
        assert_eq!(trail_share(false, 0.0, 200.0), 0.04);
    }

    /// A stream chunk at the nozzle's speed with its next trail step at
    /// half its life.
    fn trailing(own: bool, speed: f32, share: f64) -> Chunk {
        Chunk {
            speed,
            own,
            a8: STREAM_A8,
            friction: 1.0,
            size_rand: 80.0,
            trail: Some((1000.0 + f64::from(LIFE_MS) * 0.5, share)),
            ..fire(7, Vec3::ZERO, 50.0, 1000.0, LIFE_MS)
        }
    }

    /// 0x30027f22..0x3002905e: a step sheds a trail flame older than its
    /// stream chunk, slower and climbing, and the next step comes `step /
    /// 2 + rand * step` on, `step` the life times the share grown by `2 *
    /// frac + 1`.
    #[test]
    fn a_trail_step_sheds_an_older_climbing_flame_and_spaces_the_next() {
        for seed in 1..50u32 {
            let mut f = FlameSystem {
                rng: seed.wrapping_mul(0x9e37_79b9),
                ..FlameSystem::default()
            };
            let mut c = trailing(seed % 2 == 0, START_SPEED, 0.3);
            let (at, share) = c.trail.unwrap();
            let mut born = Vec::new();
            let now = at + 1.0;
            let eye = Vec3::X * 5000.0;
            f.trail(&mut c, now, eye, None, &Streams::new(), &mut born);
            let flame = born.iter().find(|b| b.kind == Kind::Trail).unwrap();
            assert_eq!(born.iter().filter(|b| b.kind == Kind::Trail).count(), 1);
            assert!(flame.born <= c.born && flame.born + f64::from(flame.life) <= at + 583.0);
            assert!(flame.a8 < 0.0 && flame.friction == 0.15);
            let slowed = if c.own { 0.5 * 0.2 + 0.1 } else { 0.25 };
            assert!(flame.speed <= START_SPEED * slowed);
            let (next, grown) = c.trail.unwrap();
            assert!((grown - share * 2.0).abs() < 1e-9);
            let step = f64::from(LIFE_MS) * grown;
            assert!(
                (at + step / 2.0..=at + step * 1.5).contains(&next),
                "{next}"
            );
        }
        // Another body's stream chunk within four of its sizes of the eye
        // steps that much farther.
        let mut f = FlameSystem::default();
        let mut c = trailing(false, START_SPEED, 0.3);
        let at = c.trail.unwrap().0;
        f.trail(
            &mut c,
            at + 1.0,
            Vec3::X * 20.0,
            None,
            &Streams::new(),
            &mut Vec::new(),
        );
        let step = f64::from(LIFE_MS) * 0.6 * 10.0;
        assert!((at + step / 2.0..=at + step * 1.5).contains(&c.trail.unwrap().0));
    }

    /// 0x30028bed..0x30028eeb: smoke comes off a trail step only between
    /// 0.02 and 0.8 of the stream chunk's life, with a chance of
    /// `(speed / 900)^2 / 2`, a half again for another body's stream; it
    /// is born at the step, lives 800 to 2100 ms and starts at a
    /// twentieth of its cap.
    #[test]
    fn a_trail_step_sheds_smoke_by_the_stream_chunks_speed() {
        let run = |own: bool, speed: f32, at_frac: f64| {
            let mut puffs = Vec::new();
            for seed in 1..2001u32 {
                let mut f = FlameSystem {
                    rng: seed.wrapping_mul(0x9e37_79b9),
                    ..FlameSystem::default()
                };
                let mut c = trailing(own, speed, 10.0);
                let at = 1000.0 + f64::from(LIFE_MS) * at_frac;
                c.trail = Some((at, 10.0));
                let mut born = Vec::new();
                f.trail(
                    &mut c,
                    at + 1.0,
                    Vec3::ZERO,
                    None,
                    &Streams::new(),
                    &mut born,
                );
                for b in born.iter().filter(|b| matches!(b.kind, Kind::Smoke(_))) {
                    assert_eq!((b.born, b.moved), (at, at));
                    assert!((800.0..=2100.0).contains(&b.life), "{}", b.life);
                    assert_eq!(b.size, b.size_max / 20.0);
                    assert!((-15.0..=-2.0).contains(&b.a8));
                    puffs.push(*b);
                }
            }
            puffs.len() as f32 / 2000.0
        };
        let own = run(true, START_SPEED, 0.5);
        assert!((0.45..0.55).contains(&own), "{own}");
        let remote = run(false, START_SPEED, 0.5);
        assert!((0.2..0.3).contains(&remote), "{remote}");
        let slow = run(true, START_SPEED / 2.0, 0.5);
        assert!((0.1..0.15).contains(&slow), "{slow}");
        assert_eq!(run(true, START_SPEED, 0.01), 0.0);
        assert_eq!(run(true, START_SPEED, 0.85), 0.0);
    }

    /// A stream at a wall: its chunks' first bounces leave residues at the
    /// face, at most one per owner each 100 ms and none within 32 units of
    /// the last for 3 s, so the one spot holds one residue; it sheds still
    /// flames that burn a player standing in them after the stream is
    /// released, and it dies inside 2.5 s, its flames 1.4 s later. An ice
    /// wall takes none.
    #[test]
    fn a_residue_whose_face_is_gone_dies_and_its_flames_trail_no_smoke() {
        use vcod_common::collision::synthetic_world;
        let wall = synthetic_world(
            &[("textures/test/wall", CONTENTS_SOLID, 0)],
            &[(0, [100.0, -512.0, -64.0], [116.0, 512.0, 256.0])],
        );
        // The face moved off: only a brush far away.
        let empty = synthetic_world(
            &[("textures/test/wall", CONTENTS_SOLID, 0)],
            &[(0, [2000.0, -512.0, -64.0], [2016.0, 512.0, 256.0])],
        );
        let residue = Residue {
            owner: 7,
            pos: Vec3::new(97.0, 0.0, 30.0),
            normal: Vec3::NEG_X,
            size: 60.0,
            born: 1000.0,
            death: 3000.0,
            next_emit: 1000.0,
            sink: RESIDUE_SINK,
        };
        let run = |world: &CollisionWorld, from: f64| {
            let mut f = FlameSystem::default();
            f.residues.push(Residue {
                next_emit: from,
                ..residue
            });
            let mut now = from;
            let mut smoked = false;
            while now < 3200.0 && !f.residues.is_empty() {
                f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(world)));
                smoked |= f.chunks.iter().any(|c| matches!(c.kind, Kind::Smoke(_)));
                now += 16.0;
            }
            (now, smoked)
        };
        // On its wall it lives to its death and sheds no smoke.
        assert_eq!(run(&wall, 1000.0), (3016.0, false));
        // Its face gone, the first flame's test kills it.
        assert_eq!(run(&empty, 1000.0).0, 1032.0);
        // Inside its last 650 ms nothing tests.
        assert!(run(&empty, 2400.0).0 > 3000.0);
    }

    #[test]
    fn a_stream_at_a_wall_leaves_a_residue_that_burns_after_release() {
        use vcod_common::collision::synthetic_world;
        let wall = |surface: u32| {
            synthetic_world(
                &[("textures/test/wall", CONTENTS_SOLID, surface)],
                &[(0, [100.0, -512.0, -64.0], [116.0, 512.0, 256.0])],
            )
        };
        let nozzle = Nozzle {
            owner: 7,
            pos: Vec3::new(0.0, 0.0, 30.0),
            dir: Vec3::X,
            own_view: false,
            ps_velocity: Vec3::ZERO,
        };
        let world = wall(0);
        let mut f = FlameSystem::default();
        let mut now = 1000.0;
        while now < 1500.0 {
            f.step(now, 16.0, &[nozzle], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        assert_eq!(f.residues.len(), 1);
        let r = f.residues[0];
        assert!(
            // The chunk's 2-unit box stops it short of the face.
            (r.pos.x - 97.0).abs() < 1.0 && r.pos.y.abs() < 1.0,
            "{}",
            r.pos
        );
        assert!(r.death - r.born <= 1500.0 + 1000.0, "{}", r.death);
        // Released; a player stands against the wall where it burns.
        let victim = Victim {
            client: 2,
            origin: Vec3::new(70.0, 0.0, 0.0),
            ..VICTIM
        };
        let mut burned = 0;
        while now < 3000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            if f.chunks.iter().any(|c| c.kind == Kind::Stream) {
                now += 16.0;
                continue;
            }
            burned += f.report(now, Some(victim), false, None).in_flame as u32;
            now += 16.0;
        }
        assert!(burned > 0);
        assert!(f.chunks.iter().any(|c| c.kind == Kind::ResidueFlame));
        while now < 6000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        assert!(f.residues.is_empty() && !f.chunks.iter().any(|c| c.kind == Kind::ResidueFlame));

        let ice = wall(12 << 20);
        let mut f = FlameSystem::default();
        let mut now = 1000.0;
        while now < 1500.0 {
            f.step(now, 16.0, &[nozzle], Vec3::ZERO, Some(&bare(&ice)));
            now += 16.0;
        }
        assert!(f.residues.is_empty());
    }

    /// The flame test's trace may end on the victim itself, never on
    /// another body or the world.
    #[test]
    fn a_body_between_the_chunk_and_the_victim_blocks_the_burn() {
        let chunk = fire(7, Vec3::new(30.0, 0.0, 30.0), 100.0, 0.0, 1e9);
        let burns = |hit: (f32, u32)| {
            let mut f = FlameSystem::default();
            f.chunks.push(chunk);
            let t = move |_: Vec3, _: Vec3| hit;
            (0..200).any(|i| {
                f.report(
                    10_000.0 + 16.0 * f64::from(i),
                    Some(VICTIM),
                    false,
                    Some(&t),
                )
                .in_flame
            })
        };
        assert!(burns((1.0, 1023)));
        assert!(burns((0.2, VICTIM.client)));
        assert!(!burns((0.5, 9)));
        assert!(!burns((0.5, 1022)));
    }

    /// The retail UO capture (`net/snapshots-uo-flame.bin`): the followed
    /// shooter's flamethrower (configstring 7's 16) fires with `eFlags`
    /// 0x200 up, which streams from the view's nozzle, and nothing else
    /// does.
    #[test]
    fn the_uo_capture_streams_while_its_shooter_fires() {
        use vcod_common::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let mut weapons = vec![None; 17];
        weapons[16] = Some(WeaponDef {
            weapon_type: "gas".into(),
            ..WeaponDef::default()
        });
        let frames = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-flame.bin",
            "net/snapshots-uo-flame.bin",
        );
        let mut streaming = 0;
        for f in &frames {
            let s = &f.snapshot;
            let own = s.ps.field_i32(p, "clientNum");
            let muzzles = HashMap::from([(own as u32, (Vec3::ZERO, Vec3::X))]);
            let n = nozzles(p, s, own, &weapons, &muzzles);
            assert!(n.iter().all(|n| n.owner == own as u32 && n.own_view));
            streaming += n.len();
        }
        assert!(
            streaming >= 5,
            "{streaming} frames streaming of {}",
            frames.len()
        );
    }

    /// A stream's farthest fire chunk along +x after 300 ms from `from`,
    /// on a bare floor with `bodies` and `local` to clip.
    fn stream_reach(bodies: &[Body], local: Option<Victim>, owner: u32, from: Vec3) -> f32 {
        let world = vcod_common::collision::test_world(&[]);
        let w = ChunkWorld {
            world: &world,
            bodies,
            local,
            max_clients: 20,
        };
        let n = Nozzle {
            owner,
            pos: from,
            dir: Vec3::X,
            own_view: false,
            ps_velocity: Vec3::ZERO,
        };
        let mut f = FlameSystem::default();
        let mut now = 1000.0;
        while now < 1300.0 {
            f.step(now, 16.0, &[n], Vec3::ZERO, Some(&w));
            now += 16.0;
        }
        f.chunks
            .iter()
            .filter(|c| c.kind == Kind::Stream)
            .map(|c| c.pos.x)
            .fold(f32::MIN, f32::max)
    }

    fn standing(entity: u32, x: f32) -> Body {
        Body {
            entity,
            origin: Vec3::new(x, 0.0, 0.0),
            mins: VICTIM.mins,
            maxs: VICTIM.maxs,
            contents: CONTENTS_BODY,
        }
    }

    /// 0x30025cd0: a chunk clips the snapshot's bodies but its owner's,
    /// and `cg.snap`'s own player's capsule unless the chunk is its own.
    #[test]
    fn chunks_strike_bodies_and_the_local_player_but_not_their_owner() {
        let nozzle = Vec3::new(0.0, 0.0, 30.0);
        assert!(stream_reach(&[], None, 7, nozzle) > 300.0);
        // A body 200 on turns the stream at its capsule.
        let body = standing(5, 200.0);
        assert!(stream_reach(&[body], None, 7, nozzle) < 200.0 - 14.0);
        assert!(stream_reach(&[body], None, 5, nozzle) > 300.0);
        // The local player is no snapshot entity: it clips as its own
        // capsule, and its own stream, started inside it, passes.
        let local = Victim {
            origin: Vec3::new(200.0, 0.0, 0.0),
            ..VICTIM
        };
        assert!(stream_reach(&[], Some(local), 7, nozzle) < 200.0 - 14.0);
        let inside = local.origin + Vec3::Z * 30.0;
        assert!(stream_reach(&[], Some(local), VICTIM.client, inside) > 500.0);
    }

    /// 0x30025da0: a first bounce off a client leaves no residue and spends
    /// the chunk's one chance; off another entity it may leave one.
    #[test]
    fn a_bounce_off_a_client_leaves_no_residue() {
        let world = vcod_common::collision::test_world(&[]);
        let residues = |entity: u32| {
            let bodies = [standing(entity, 100.0)];
            let w = ChunkWorld {
                world: &world,
                bodies: &bodies,
                local: None,
                max_clients: 20,
            };
            let n = Nozzle {
                owner: 7,
                pos: Vec3::new(0.0, 0.0, 30.0),
                dir: Vec3::X,
                own_view: false,
                ps_velocity: Vec3::ZERO,
            };
            let mut f = FlameSystem::default();
            let mut now = 1000.0;
            while now < 1100.0 {
                f.step(now, 16.0, &[n], Vec3::ZERO, Some(&w));
                now += 16.0;
            }
            f.residues.len()
        };
        assert_eq!(residues(5), 0);
        assert!(residues(40) > 0);
    }

    /// One residue's flames after a second of shedding.
    fn residue_flames(pos: Vec3, normal: Vec3) -> (FlameSystem, Residue) {
        let world = vcod_common::collision::test_world(&[]);
        let mut f = FlameSystem::default();
        f.residues.push(Residue {
            owner: 7,
            pos,
            normal,
            size: 100.0,
            born: 1000.0,
            death: 3000.0,
            next_emit: 1000.0,
            sink: RESIDUE_SINK,
        });
        let r = f.residues[0];
        let mut now = 1000.0;
        while now < 2000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        (f, r)
    }

    /// 0x30027d10's residue flames: on a floor they drift along -y or +y
    /// only (`right` off a vertical normal's angles) and rise ever faster
    /// (`+0xa8` -20.2); off a wall they drift out from it as well.
    #[test]
    fn residue_flames_drift_along_the_face_and_rise() {
        // Where a chunk's box comes to rest on the floor.
        let (f, r) = residue_flames(Vec3::new(0.0, 0.0, 3.125), Vec3::Z);
        let flames: Vec<&Chunk> = f
            .chunks
            .iter()
            .filter(|c| c.kind == Kind::ResidueFlame)
            .collect();
        assert!(flames.len() > 5);
        assert!(flames.iter().all(|c| c.pos.x == r.pos.x && c.a8 == -20.2));
        assert!(flames.iter().any(|c| (c.pos.y - r.pos.y).abs() > 1.0));
        // The oldest, a second old plus up to 300 ms of lag, have risen
        // 20.2 * age^2 / 2 in 500 ms units: 40 to 68.
        let top = flames.iter().map(|c| c.pos.z).fold(f32::MIN, f32::max);
        assert!((35.0..70.0).contains(&(top - r.pos.z)), "{top}");

        let (f, r) = residue_flames(Vec3::new(500.0, 0.0, 40.0), -Vec3::X);
        let flames: Vec<&Chunk> = f
            .chunks
            .iter()
            .filter(|c| c.kind == Kind::ResidueFlame)
            .collect();
        assert!(flames.iter().all(|c| c.pos.x < r.pos.x));
        assert!(flames.iter().all(|c| (-27.3..=-25.3).contains(&c.a8)));
    }

    /// 0x30025da0's bounce clips a residue flame along the face (0x30008390,
    /// overbounce 1.1) at a quarter of the clipped speed, and the friction
    /// it then carries (`+0xbc` 0.5) pins a speed above 2 at 60
    /// (0x30023c30).
    #[test]
    fn a_bounced_residue_flame_slides_along_the_face_and_brakes_to_60() {
        use vcod_common::collision::synthetic_world;
        let world = synthetic_world(
            &[("textures/test/wall", CONTENTS_SOLID, 0)],
            &[(0, [100.0, -512.0, -64.0], [116.0, 512.0, 256.0])],
        );
        let mut f = FlameSystem::default();
        f.chunks.push(Chunk {
            dir: Vec3::new(1.0, 1.0, 0.0).normalize(),
            speed: 20.0,
            kind: Kind::ResidueFlame,
            ..fire(7, Vec3::new(97.0, 0.0, 40.0), 60.0, 1000.0, 5000.0)
        });
        let mut now = 1000.0;
        while f.chunks[0].dir.x > 0.0 && now < 2000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        let c = f.chunks[0];
        assert_eq!(c.friction, 0.5);
        assert!(c.dir.y > 0.99 && c.dir.x < 0.0, "{:?}", c.dir);
        assert!((3.0..4.0).contains(&c.speed), "{}", c.speed);
        for _ in 0..3 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        assert_eq!(f.chunks[0].speed, 60.0);
        let head_on = clip_velocity(Vec3::X * 4.0, -Vec3::X);
        assert!(
            head_on.abs_diff_eq(Vec3::new(-0.4, 0.0, 0.0), 1e-5),
            "{head_on}"
        );
    }

    /// 0x30025da0's look-ahead: the first move traces 175 ms ahead at the
    /// chunk's speed and, as it missed, traces nothing more until the clock
    /// reaches that span's end, so a wall that comes up inside it is met
    /// only after (here a 2000-tick span, the chunk long past the wall's
    /// face). A contact is acted on once the clock passes it: the chunk
    /// bounces at the contact itself, a quarter as fast, and passes a
    /// `SURF_NOIMPACT` face untouched. Either keeps the face's normal.
    #[test]
    fn a_chunk_meets_only_what_its_look_ahead_traced() {
        use vcod_common::collision::synthetic_world;
        let wall = |surface: u32| {
            synthetic_world(
                &[("textures/test/wall", CONTENTS_SOLID, surface)],
                &[(0, [100.0, -512.0, -64.0], [116.0, 512.0, 256.0])],
            )
        };
        let open = synthetic_world(
            &[("textures/test/wall", CONTENTS_SOLID, 0)],
            &[(0, [3000.0, -512.0, -64.0], [3016.0, 512.0, 256.0])],
        );
        let fast = Chunk {
            speed: START_SPEED,
            ..fire(7, Vec3::ZERO, 16.0, 0.0, LIFE_MS)
        };
        let run = |c: &mut Chunk, worlds: &[(&CollisionWorld, f64)]| {
            let mut last = None;
            for &(w, now) in worlds {
                move_chunk(c, now, Some(&bare(w)), &Streams::new(), &mut last);
            }
        };
        let solid = wall(0);
        // Traced in the open first: on through the wall until 191 ms.
        let mut c = fast;
        run(&mut c, &[(&open, 16.0), (&solid, 64.0), (&solid, 128.0)]);
        assert!(c.pos.x > 200.0 && c.dir == Vec3::X, "{} {}", c.pos, c.dir);
        assert!((c.seen_at - 191.0).abs() < 1e-9, "{}", c.seen_at);

        // Traced against it: placed on until the contact's time (some 54
        // ms in), then bounced at the contact.
        let mut c = fast;
        run(&mut c, &[(&solid, 16.0), (&solid, 48.0)]);
        assert!(c.pos.x < 98.0 && c.dir == Vec3::X, "{}", c.pos);
        assert!((50.0..60.0).contains(&c.seen_at), "{}", c.seen_at);
        run(&mut c, &[(&solid, 64.0)]);
        assert!(c.dir.x < 0.0 && c.speed < START_SPEED * 0.25, "{}", c.dir);
        assert!((97.5..98.0).contains(&c.pos.x), "{}", c.pos);
        assert_eq!((c.base, c.base_at), (c.pos, 64.0));
        assert_eq!(c.normal, Vec3::NEG_X);

        let mut c = fast;
        let noimpact = wall(SURF_NOIMPACT);
        run(
            &mut c,
            &[(&noimpact, 16.0), (&noimpact, 64.0), (&noimpact, 96.0)],
        );
        assert!(c.dir == Vec3::X && c.pos.x > 116.0, "{}", c.pos);
        assert_eq!(c.normal, Vec3::NEG_X);
    }

    /// 0x300269e1..0x300270cb, the eye at the origin looking along +x: a
    /// sprite's radius is half the size and it is raised by `0.55 *
    /// min(radius, 72.5) - 8.8`; a chunk off a face that looks down sits
    /// `20 * |normal z|` lower. The view's own chunk comes its radius
    /// nearer the eye at `1 - radius / distance` of the radius, one inside
    /// its radius goes 20 behind the eye, and nothing behind it is drawn.
    #[test]
    fn the_sprite_add_pulls_the_views_own_chunks_in_and_lowers_one_off_a_ceiling() {
        let at = |pos: Vec3, size: f32, own: bool, normal: Vec3| {
            let c = Chunk {
                own,
                normal,
                ..fire(4, pos, size, 0.0, LIFE_MS)
            };
            sprite(&c, Vec3::ZERO, Vec3::X).map(|(p, r)| (p.to_array(), r))
        };
        let near = |got: Option<([f32; 3], f32)>, want: ([f32; 3], f32)| {
            let (p, r) = got.expect("drawn");
            let close = p.iter().zip(want.0).all(|(a, b)| (a - b).abs() < 1e-4);
            assert!(close && (r - want.1).abs() < 1e-4, "{p:?} {r} != {want:?}");
        };
        let ahead = Vec3::new(200.0, 0.0, 0.0);
        near(
            at(ahead, 40.0, false, Vec3::ZERO),
            ([200.0, 0.0, 2.2], 20.0),
        );
        near(at(ahead, 40.0, true, Vec3::ZERO), ([180.0, 0.0, 1.1], 18.0));
        near(
            at(ahead, 290.0, false, Vec3::ZERO),
            ([200.0, 0.0, 31.075], 145.0),
        );
        assert_eq!(at(Vec3::new(10.0, 0.0, 0.0), 40.0, true, Vec3::ZERO), None);
        assert_eq!(
            at(Vec3::new(-50.0, 0.0, 0.0), 40.0, false, Vec3::ZERO),
            None
        );
        let high = Vec3::new(200.0, 0.0, 100.0);
        near(
            at(high, 40.0, false, Vec3::NEG_Z),
            ([200.0, 0.0, 82.2], 20.0),
        );
        near(at(high, 40.0, false, Vec3::Z), ([200.0, 0.0, 102.2], 20.0));
        let slant = Vec3::new(0.6, 0.0, -0.8);
        near(at(high, 40.0, false, slant), ([200.0, 0.0, 86.2], 20.0));
    }

    /// 0x300272b0 on a chain not live: newest first, a chunk takes in the
    /// next older one it overlaps when born within 25 ms, keeping the
    /// larger's size, birth and place; one born 30 ms apart stays. A dead
    /// chunk ends its chain: the older ones go with it.
    #[test]
    fn the_chain_draw_merges_overlapping_chunks_and_drops_the_tail_behind_a_dead_one() {
        let link = |chain: u32, x: f32, size: f32, born: f64, life: f32| Chunk {
            chain: Some((chain, Vec2::ZERO, Vec3::ZERO)),
            drawn: false,
            ..fire(5, Vec3::new(x, 0.0, 0.0), size, born, life)
        };
        // Oldest first, as laid.
        let mut f = FlameSystem {
            chunks: vec![
                link(1, 203.0, 100.0, 950.0, LIFE_MS),
                link(1, 200.0, 100.0, 980.0, LIFE_MS),
                link(1, 0.0, 120.0, 990.0, LIFE_MS),
                link(1, 10.0, 100.0, 1000.0, LIFE_MS),
                link(2, 500.0, 50.0, 0.0, 2000.0),
                link(2, 600.0, 50.0, 100.0, 100.0),
                link(2, 700.0, 50.0, 500.0, 2000.0),
            ],
            ..FlameSystem::default()
        };
        f.draw_chains(1010.0, Vec3::ZERO);
        let left: Vec<_> = f
            .chunks
            .iter()
            .map(|c| (c.chain.unwrap().0, c.pos.x, c.size, c.born, c.drawn))
            .collect();
        assert_eq!(
            left,
            [
                (1, 203.0, 100.0, 950.0, true),
                (1, 200.0, 100.0, 980.0, true),
                (1, 0.0, 120.0, 990.0, true),
                (2, 700.0, 50.0, 500.0, true),
            ]
        );
    }

    /// 0x300274d2..0x300277a7 on a live chain: the newest is not drawn,
    /// its age from its own birth being 0; in another body's stream a
    /// chunk alike the last drawn and in line with it from the eye is
    /// skipped, one off the line is drawn, and the view's own stream draws
    /// both.
    #[test]
    fn another_bodys_live_stream_skips_a_chunk_hidden_behind_the_last_drawn() {
        let drawn = |own: bool| {
            let link = |pos: Vec3, born: f64| Chunk {
                chain: Some((3, Vec2::ZERO, Vec3::ZERO)),
                own,
                drawn: false,
                ..fire(6, pos, 40.0, born, LIFE_MS)
            };
            let mut f = FlameSystem {
                frame: 9,
                ..FlameSystem::default()
            };
            f.owners.insert(
                6,
                Owner {
                    last_emit: 1010.0,
                    last_fire: 1010.0,
                    pos: Vec3::ZERO,
                    chain: 3,
                    aim: Vec2::ZERO,
                    fired_frame: 9,
                    vel: Vec3::ZERO,
                    run_ms: 0.0,
                    turn_rate: 0.0,
                },
            );
            f.chunks = vec![
                link(Vec3::new(100.0, 30.0, 0.0), 910.0),
                link(Vec3::new(101.0, 0.0, 0.0), 940.0),
                link(Vec3::new(100.0, 0.0, 0.0), 970.0),
                link(Vec3::new(0.0, 0.0, 0.0), 1010.0),
            ];
            f.draw_chains(1010.0, Vec3::ZERO);
            f.chunks.iter().map(|c| c.drawn).collect::<Vec<_>>()
        };
        assert_eq!(drawn(false), [true, false, true, false]);
        assert_eq!(drawn(true), [true, true, true, false]);
    }

    /// 0x30026b08..0x30026c4f: smoke greys from 100 to 20 over its second
    /// half, tinted (249, 239, 219), its alpha 76.5 times half the tent of
    /// its life; a residue's flame greys up from black over its first half.
    /// Halved (0x30026c53..0x30026c84), all but smoke shift their grey a
    /// bit down and keep their alpha.
    #[test]
    fn smoke_fades_by_a_tent_of_its_life_and_a_residue_flame_greys_up() {
        let bytes = |c: [f32; 4]| c.map(|v| (v * 255.0).round() as u8);
        for halve in [false, true] {
            let smoke = |frac| bytes(colour(Kind::Smoke(1), frac, halve));
            assert_eq!(smoke(0.25), [97, 93, 85, 19]);
            assert_eq!(smoke(0.5), [97, 93, 85, 38]);
            assert_eq!(smoke(0.75), [58, 56, 51, 19]);
        }
        let rest = |kind, frac, halve| bytes(colour(kind, frac, halve));
        assert_eq!(rest(Kind::ResidueFlame, 0.25, false), [127, 127, 127, 255]);
        assert_eq!(rest(Kind::ResidueFlame, 0.7, false), [255; 4]);
        assert_eq!(colour(Kind::Stream, 0.3, false), [1.0; 4]);
        assert_eq!(rest(Kind::ResidueFlame, 0.25, true), [63, 63, 63, 255]);
        assert_eq!(rest(Kind::ResidueFlame, 0.7, true), [127, 127, 127, 255]);
        assert_eq!(rest(Kind::Stream, 0.3, true), [127, 127, 127, 255]);
        assert_eq!(rest(Kind::Trail, 0.3, true), [127, 127, 127, 255]);
    }
}
