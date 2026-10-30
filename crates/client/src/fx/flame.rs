//! United Offensive's flamethrower: the cgame's own chunk system, not an
//! `.efx` (uo cgame 0x30024050 `CG_FireFlameChunks` .. 0x30027d10
//! `CG_AddFlameChunks`, a reworked RTCW `cg_flamethrower.c`;
//! docs/research/coduo-diff.md section 12). A body firing a `gas` weapon
//! streams chunks off its `tag_flash` while its `eFlags` firing bit is up;
//! each chunk slows, grows, bounces off the world and the bodies in it
//! ([`ChunkWorld`]) and plays through the
//! 43 `flamethrowerFire` sprite frames, shedding smoke on the way. The
//! sound runs `flamethrower_start`, then `flamethrower_fire` held while
//! firing, then `flamethrower_cooldown`.
//!
//! vcod keeps the retail constants it was given (speeds, friction, life,
//! sizes, spacing, the frame pick) and simplifies the rest: no chain
//! merging, one smoke puff per chunk. A fire chunk's first bounce off the
//! world may leave a burning residue ([`Residue`]): a still emitter that
//! sheds small drifting, rising flames for a second or two, which burn as
//! the stream's do.
//!
//! The burn is the victim's to report: [`FlameSystem::report`] tests the
//! chunks against the snapshot's own player and yields the `wbuttons` bit
//! and the `fdc <owner>` command the server burns on (coduo-diff.md 19,
//! "The flame report").

use std::collections::HashMap;

use glam::Vec3;
use vcod_common::collision::{
    CONTENTS_SOLID, CollisionWorld, MASK_PLAYERSOLID, Trace, sound_material,
};
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld, body_trace};

use super::sim::FxQuad;
use vcod_common::net::flags::{EF_FIRING, ET_PLAYER};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::weapon::WeaponDef;

/// Nozzle speed, units per second (900 per 1000 half-ms ticks).
const START_SPEED: f32 = 1800.0;
const MIN_SPEED: f32 = 30.0;
/// Speed lost per second.
const FRICTION: f32 = 3600.0;
/// A chunk's life, ms (1666 ticks).
const LIFE_MS: f32 = 833.0;
const START_SIZE: f32 = 16.0;
const MAX_SIZE: f32 = 290.0;
/// Sprite frames `flamethrowerFire2` .. `flamethrowerFire44`.
const FRAMES: f32 = 43.0;
/// Chunk spacing, ms per chunk before the view scale: `16 / 900` of 1000
/// ticks.
const SPACING_MS: f32 = 8.888_889;
/// A remote body's chain lives on this long without a frame of firing.
const REMOTE_GRACE_MS: f32 = 75.0;
const MAX_PER_FRAME: usize = 180;
/// The upward drift the chunks gather, units per second squared.
const RISE: f32 = 60.0;

/// A body streaming flame this frame: its nozzle (`tag_flash`), and
/// whether it is the view's own (first person spaces its chunks wider).
#[derive(Clone, Copy, Debug)]
pub struct Nozzle {
    pub owner: u32,
    pub pos: Vec3,
    pub dir: Vec3,
    pub own_view: bool,
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
    last_emit: f32,
    last_fire: f32,
    pos: Vec3,
}

#[derive(Clone, Copy)]
struct Chunk {
    /// The body whose nozzle shed it, the entity `fdc` names.
    owner: u32,
    pos: Vec3,
    dir: Vec3,
    speed: f32,
    rise: f32,
    size: f32,
    size_max: f32,
    born: f32,
    life: f32,
    roll: f32,
    smoke: Option<u8>,
    /// A fire chunk sheds its smoke once.
    shed: bool,
    /// A stream chunk may leave a residue on its first bounce (`+0x14` 1
    /// at the nozzle, 0x30024c04; `+0x124` the bounce, 0x30025da0).
    may_residue: bool,
    /// A residue's flame: drifting, and tested by the report (type 2 with
    /// `+0x14` -1, 0x300268e0).
    residue: bool,
    /// A residue flame's lift, `-(+0xa8)`: 0x30025990 raises it by
    /// `lift * dt * age` in units of 500 ms, `dt` since the last move.
    lift: f32,
    /// A residue flame's friction `+0xbc`: 0 at birth, 0.5 once it has
    /// bounced (0x30025da0).
    braked: bool,
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
    born: f32,
    death: f32,
    next_emit: f32,
    /// Its `+0xa8`: -0.2 at the bounce, redrawn each step on a wall.
    sink: f32,
}

/// 0x30025da0's bounce: the velocity clipped against the struck face with
/// an overbounce of 1.1 (0x30008390), a slide rather than a mirror.
fn clip_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let d = v.dot(normal);
    v - normal * if d < 0.0 { d * 1.1 } else { d / 1.1 }
}

/// A bounced residue flame's friction (0x30025da0 into 0x30023c30, vcod's
/// speed being retail's per 1000 ticks doubled): above 2 it loses `+0xbc`
/// 0.5 times 1800 per 1000 ticks, and a result under 60 that is not 0
/// becomes 60.
fn residue_brake(speed: f32, dt_ms: f32) -> f32 {
    if speed <= 2.0 {
        return speed;
    }
    let slowed = speed - 0.5 * 1800.0 * 2.0 * (dt_ms * 2.0 / 1000.0);
    if slowed != 0.0 && slowed < 60.0 {
        60.0
    } else {
        slowed
    }
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
/// Surfaces that take no residue: ice, snow, water (0x1300000, 0x1400000
/// and 0xc00000 of the struck `surfaceFlags`).
const NO_RESIDUE_SURFACES: [i32; 3] = [12, 19, 20];
/// A struck surface with this flag stops nothing (0x300a85d4 & 0x10).
const SURF_NOIMPACT: u32 = 0x10;

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
    next_check: f32,
    last_fdc: f32,
}

pub struct FlameSystem {
    owners: HashMap<u32, Owner>,
    chunks: Vec<Chunk>,
    residues: Vec<Residue>,
    /// Each owner's last residue, when and where (`centFlameInfo` rows
    /// 0x300ab7f0, 0x300ab7e4).
    last_residue: HashMap<u32, (f32, Vec3)>,
    rng: u32,
    report_clocks: HashMap<u32, ReportClock>,
    in_flame: bool,
    /// The last chunk tested and when: a nearby chunk in the same frame is
    /// skipped (0x300a8710, 0x300d9760).
    last_test: Option<(f32, Vec3)>,
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

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.owners.is_empty() && self.residues.is_empty()
    }

    /// Emits from this frame's `nozzles`, moves every chunk `dt_ms` on to
    /// `now` (ms), and reports the sound transitions. `eye` scales a remote
    /// body's spacing with its distance.
    pub fn step(
        &mut self,
        now: f32,
        dt_ms: f32,
        nozzles: &[Nozzle],
        eye: Vec3,
        world: Option<&ChunkWorld>,
    ) -> Vec<FlameSound> {
        let mut sounds = Vec::new();
        for n in nozzles {
            let started = match self.owners.get(&n.owner) {
                Some(o) => now - o.last_fire > dt_ms.max(1.0) + REMOTE_GRACE_MS,
                None => true,
            };
            if started {
                sounds.push(FlameSound::Start {
                    owner: n.owner,
                    pos: n.pos,
                });
                self.owners.insert(
                    n.owner,
                    Owner {
                        last_emit: now,
                        last_fire: now,
                        pos: n.pos,
                    },
                );
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
            let spacing = (k * SPACING_MS * far).max(1.0);
            let speed_scale = if n.own_view { 1.0 } else { 1.8 };
            // A new stream's first chunk leaves now.
            let mut t = match started {
                true => now,
                false => self.owners[&n.owner].last_emit + spacing,
            };
            let mut emitted = 0;
            while t <= now && emitted < MAX_PER_FRAME {
                self.emit(n, t, speed_scale, now, world);
                t += spacing;
                emitted += 1;
            }
            if let Some(o) = self.owners.get_mut(&n.owner) {
                o.last_emit = if emitted == MAX_PER_FRAME {
                    now
                } else {
                    t - spacing
                };
                o.last_fire = now;
                o.pos = n.pos;
            }
        }
        // Released: no nozzle this frame and past the grace.
        let firing: Vec<u32> = nozzles.iter().map(|n| n.owner).collect();
        self.owners.retain(|&owner, o| {
            let keep = firing.contains(&owner) || now - o.last_fire <= REMOTE_GRACE_MS;
            if !keep {
                sounds.push(FlameSound::Stop { owner, pos: o.pos });
            }
            keep
        });

        self.shed_residue_flames(now);
        let dt = dt_ms / 1000.0;
        let mut smoke = Vec::new();
        let mut struck = Vec::new();
        for (i, c) in self.chunks.iter_mut().enumerate() {
            if c.smoke.is_some() {
                c.pos.z += 40.0 * dt;
                c.size = (c.size + 30.0 * dt).min(c.size_max);
                continue;
            }
            let to = if c.residue {
                if c.braked {
                    c.speed = residue_brake(c.speed, dt_ms);
                }
                // 0x30025c60: half its cap to the whole over half its life
                // at the stream's speed scale `+0xb8` of 1.
                c.size = (c.size + c.size_max / c.life * dt_ms).min(c.size_max);
                let age = (now - c.born) / 500.0;
                c.pos + c.dir * c.speed * dt + Vec3::Z * (c.lift * dt_ms / 500.0 * age)
            } else {
                c.speed = (c.speed - FRICTION * dt).max(MIN_SPEED);
                let v = c.speed / START_SPEED;
                let rate = c.size_max / LIFE_MS / (2.0 * v * v * v + 0.8);
                c.size = (c.size + rate * dt_ms).min(c.size_max);
                c.rise += RISE * dt;
                c.pos + c.dir * c.speed * dt + Vec3::Z * c.rise * dt
            };
            match world.map(|w| (w.trace(c.pos, to, c.owner), w.max_clients)) {
                // 0x30025da0: stuck inside, the chunk loses its speed.
                Some(((tr, _), _)) if tr.allsolid => {
                    c.dir = Vec3::ZERO;
                    c.speed = 0.0;
                }
                Some(((tr, entity), max_clients)) if tr.fraction < 1.0 => {
                    let v = clip_velocity(c.dir * c.speed, tr.normal);
                    c.dir = v.normalize_or_zero();
                    c.speed = v.length() * 0.25;
                    c.braked |= c.residue;
                    c.pos = tr.endpos + tr.normal;
                    if std::mem::take(&mut c.may_residue)
                        && entity >= max_clients
                        && tr.surface_flags & SURF_NOIMPACT == 0
                        && !NO_RESIDUE_SURFACES.contains(&sound_material(tr.surface_flags))
                    {
                        struck.push((i, tr.normal));
                    }
                }
                _ => c.pos = to,
            }
            let frac = (now - c.born) / c.life;
            if !c.shed && frac >= 0.5 {
                c.shed = true;
                smoke.push((c.owner, c.pos, c.size));
            }
        }
        for (i, normal) in struck {
            let c = self.chunks[i];
            self.leave_residue(&c, normal, now);
        }
        for (owner, pos, size) in smoke {
            if self.rand() < 0.3 {
                let frame = (self.rand() * 6.0) as u8 + 1;
                let life = 1600.0 + self.rand() * 2600.0;
                let roll = self.rand() * 360.0;
                self.chunks.push(Chunk {
                    owner,
                    pos,
                    dir: Vec3::Z,
                    speed: 0.0,
                    rise: 0.0,
                    size: size * 0.5,
                    size_max: size * 1.5,
                    born: now,
                    life,
                    roll,
                    smoke: Some(frame.min(6)),
                    shed: true,
                    may_residue: false,
                    residue: false,
                    lift: 0.0,
                    braked: false,
                });
            }
        }
        self.chunks.retain(|c| now - c.born < c.life);
        self.residues.retain(|r| now < r.death);
        sounds
    }

    /// 0x30025da0's residue off a stream chunk's first bounce: unless its
    /// owner left one in the last 100 ms, or within 32 units of one left
    /// in the last 3 s, a residue of the chunk's size (at most 145) at the
    /// contact, living one to two and a half seconds by the face's slope
    /// (0x30027ad0 and the death and trail it is given there).
    fn leave_residue(&mut self, c: &Chunk, normal: Vec3, now: f32) {
        if let Some(&(t, at)) = self.last_residue.get(&c.owner)
            && (now - t < RESIDUE_EVERY_MS
                || (t <= now && now - t <= RESIDUE_NEAR_MS && at.distance(c.pos) <= RESIDUE_NEAR))
        {
            return;
        }
        self.last_residue.insert(c.owner, (now, c.pos));
        let life =
            RESIDUE_LIFE_MS * (1.0 + (1.0 - normal.z.abs()) * 0.5) + self.rand() * RESIDUE_LIFE_MS;
        let next_emit = now + self.rand() * RESIDUE_STEP_MS;
        self.residues.push(Residue {
            owner: c.owner,
            pos: c.pos,
            normal,
            size: c.size.min(RESIDUE_MAX_SIZE),
            born: now,
            death: now + life,
            next_emit,
            sink: RESIDUE_SINK,
        });
    }

    /// Every residue's flames due by `now` (0x30027d10, a type 3 parent's
    /// type 2 children): each lives [`RESIDUE_FLAME_LIFE_MS`], started up
    /// to [`RESIDUE_FLAME_LAG_MS`] into it, at a size of 1.5 times the
    /// residue's held to 60..120 plus up to 25, from half of that. It
    /// drifts along the face at 4 units a second or more, off a wall
    /// as well, and rises ever faster ([`Chunk::lift`]).
    fn shed_residue_flames(&mut self, now: f32) {
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
                let frac = (r.next_emit - r.born) / (r.death - r.born);
                // Units per 1000 ticks (500 ms): `(1 - frac) * rand * |+0xa8|
                // + 2`.
                let speed = ((1.0 - frac) * self.rand() * r.sink.abs() + 2.0) * 2.0;
                let roll = self.rand() * 360.0;
                self.chunks.push(Chunk {
                    owner: r.owner,
                    pos: r.pos,
                    dir: dir.normalize_or_zero(),
                    speed,
                    rise: 0.0,
                    size: size_max * 0.5,
                    size_max,
                    born: r.next_emit - lag,
                    life: RESIDUE_FLAME_LIFE_MS,
                    roll,
                    smoke: None,
                    shed: false,
                    may_residue: false,
                    residue: true,
                    // `+0xa8` is the parent's less 20, less 5 times
                    // `1 - normal z`.
                    lift: -((r.sink - 20.0) - (1.0 - n.z) * 5.0),
                    braked: false,
                });
                let step = RESIDUE_STEP_MS * (0.5 + self.rand());
                self.residues[i].next_emit += step;
            }
        }
    }

    /// The cgame's flame report for this frame at `now` (ms), run after
    /// [`Self::step`] as `CG_AddFlameChunks` (uo cgame 0x30027d10) tests
    /// while it draws. Once `victim`'s check is due the flag drops, and
    /// each live fire chunk short of 0.9 of its life is tested one in five
    /// (0x300268e0) until one reaches the victim's box (0x300265c0); a hit
    /// raises the flag and, two seconds past the last, names the chunk's
    /// owner for `fdc` (0x300291c0). The next check is 50 ms on, and the
    /// flag holds in between. `None` for a victim keeps the flag down.
    /// `fdc` is subject to the server's flood window, so while
    /// `flood_open` the report holds it back for a later check.
    pub fn report(
        &mut self,
        now: f32,
        victim: Option<Victim>,
        flood_open: bool,
        trace: Option<ShotTrace>,
    ) -> FlameReport {
        let Some(v) = victim else {
            self.in_flame = false;
            return FlameReport::default();
        };
        let clock = *self.report_clocks.entry(v.client).or_insert(ReportClock {
            next_check: f32::NEG_INFINITY,
            last_fdc: f32::NEG_INFINITY,
        });
        let mut fdc = None;
        if clock.next_check < now {
            self.in_flame = false;
            for i in 0..self.chunks.len() {
                if self.in_flame {
                    break;
                }
                let c = self.chunks[i];
                if c.smoke.is_some() || (self.rand() * 5.0) as u32 != 0 {
                    continue;
                }
                let frac = (now - c.born) / c.life;
                if frac >= MAX_FRAC || !self.reaches(&c, frac, now, &v, trace) {
                    continue;
                }
                self.in_flame = true;
                if clock.last_fdc <= now - FDC_MS && !flood_open {
                    self.report_clocks.get_mut(&v.client).unwrap().last_fdc = now;
                    fdc = Some(c.owner);
                }
            }
            self.report_clocks.get_mut(&v.client).unwrap().next_check = now + CHECK_MS;
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
        now: f32,
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

    fn emit(
        &mut self,
        n: &Nozzle,
        born: f32,
        speed_scale: f32,
        now: f32,
        world: Option<&ChunkWorld>,
    ) {
        let size_rand = (self.rand() * 140.0).min(110.0);
        let k = if n.own_view { 0.75 } else { 0.35 };
        let size_max = ((140.0 - size_rand) * (0.75 * speed_scale + 0.25)).min(MAX_SIZE);
        let roll = self.rand() * 360.0;
        // A nozzle inside a wall (`CG_FireFlameChunks`' ±2 box trace) lays
        // the chunk at the wall, still, for half a life.
        let blocked = world.map(|w| {
            let tr = w
                .world
                .point_trace(n.pos - n.dir * 16.0, n.pos, CONTENTS_SOLID, false);
            tr.fraction < 1.0
        });
        let (speed, life) = match blocked {
            Some(true) => (0.0, LIFE_MS * 0.5),
            _ => (START_SPEED, LIFE_MS),
        };
        // Chunks due earlier in this frame start along their path.
        let ahead = (now - born).max(0.0) / 1000.0 * speed;
        self.chunks.push(Chunk {
            owner: n.owner,
            pos: n.pos + n.dir * ahead,
            dir: n.dir.normalize_or_zero(),
            speed,
            rise: 0.0,
            size: (1.0 - k * size_rand / 140.0) * START_SIZE,
            size_max: size_max.max(START_SIZE),
            born,
            life,
            roll,
            smoke: None,
            shed: false,
            may_residue: true,
            residue: false,
            lift: 0.0,
            braked: false,
        });
    }

    /// Camera-facing sprites: `flamethrowerFire{2..44}` by the chunk's
    /// share of its life (0x300268e0), at a radius of half its size and no
    /// less than 6; smoke as `flameSmoke{1..6}`, fading out.
    pub fn quads(&self, now: f32, cam_right: Vec3, cam_up: Vec3) -> Vec<FxQuad> {
        self.chunks
            .iter()
            .map(|c| {
                let frac = ((now - c.born) / c.life).clamp(0.0, 1.0);
                let (shader, a) = match c.smoke {
                    Some(n) => (format!("flameSmoke{n}"), 0.4 * (1.0 - frac)),
                    None => {
                        let frame = (frac * FRAMES).floor().clamp(0.0, FRAMES - 1.0) as i32 + 2;
                        (format!("flamethrowerFire{frame}"), 1.0)
                    }
                };
                let r = (c.size / 2.0).max(6.0);
                let (s, co) = c.roll.to_radians().sin_cos();
                let right = (cam_right * co + cam_up * s) * r;
                let up = (cam_up * co - cam_right * s) * r;
                let p = c.pos;
                FxQuad {
                    verts: [
                        (p - right + up).to_array(),
                        (p + right + up).to_array(),
                        (p + right - up).to_array(),
                        (p - right - up).to_array(),
                    ],
                    uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    rgba: [a; 4],
                    shader: shader.into(),
                }
            })
            .collect()
    }
}

/// This frame's flamethrower nozzles on a United Offensive connection: the
/// ridden body off the playerstate, every other player body off its
/// entity, each whose `eFlags` firing bit is up with a `gas` weapon in
/// hand, at its muzzle (the view's for the ridden body).
pub fn nozzles(
    p: &Protocol,
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
        }
    }

    fn fire(owner: u32, pos: Vec3, size: f32, born: f32, life: f32) -> Chunk {
        Chunk {
            owner,
            pos,
            dir: Vec3::X,
            speed: 0.0,
            rise: 0.0,
            size,
            size_max: size,
            born,
            life,
            roll: 0.0,
            smoke: None,
            shed: false,
            may_residue: false,
            residue: false,
            lift: 0.0,
            braked: false,
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
        let mut reaches = |owner: u32, pos: Vec3, size: f32, frac: f32, now: f32| {
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
        assert!(sent.windows(2).all(|w| w[1] - w[0] >= FDC_MS), "{sent:?}");

        // The flag holds between checks and drops at the next one.
        f.chunks.clear();
        let held = f.report(now, Some(VICTIM), false, None);
        let mut t = now;
        while f.report(t, Some(VICTIM), false, None).in_flame {
            t += 16.0;
        }
        assert!(t - now <= CHECK_MS + 16.0, "{held:?} {}", t - now);

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

    /// Smoke never burns, nor a chunk at 0.9 of its life.
    #[test]
    fn smoke_and_spent_chunks_never_report() {
        let mut f = FlameSystem::default();
        let mut smoke = fire(7, Vec3::new(20.0, 0.0, 30.0), 100.0, 0.0, 1e9);
        smoke.smoke = Some(1);
        f.chunks.push(smoke);
        f.chunks
            .push(fire(7, Vec3::new(20.0, 0.0, 30.0), 100.0, 0.0, 1000.0));
        let mut now = 900.0;
        while now < 3_000.0 {
            assert_eq!(
                f.report(now, Some(VICTIM), false, None),
                FlameReport::default()
            );
            now += 16.0;
        }
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
        let held = f.step(1016.0, 16.0, &[nozzle(3)], Vec3::ZERO, None);
        assert!(matches!(held[..], [FlameSound::Fire { owner: 3, .. }]));
        // One at the start, two more at a 6.7 ms spacing in a 16 ms frame.
        let live = f.chunks.iter().filter(|c| c.smoke.is_none()).count();
        assert_eq!(live, 3);
        // Released: the grace runs out, then the cooldown.
        assert!(f.step(1032.0, 16.0, &[], Vec3::ZERO, None).is_empty());
        let stop = f.step(1100.0, 16.0, &[], Vec3::ZERO, None);
        assert!(matches!(stop[..], [FlameSound::Stop { owner: 3, .. }]));
    }

    #[test]
    fn chunks_slow_grow_play_through_their_frames_and_die() {
        let mut f = FlameSystem::default();
        f.step(1000.0, 10.0, &[nozzle(1)], Vec3::ZERO, None);
        let first = f.chunks[0];
        f.step(1200.0, 200.0, &[], Vec3::ZERO, None);
        let c = f.chunks.iter().find(|c| c.born == first.born).unwrap();
        // The emitting frame moved it 10 ms too.
        assert_eq!(c.speed, START_SPEED - FRICTION * 0.21);
        assert!(c.size > first.size && c.size <= c.size_max);
        assert!(
            (c.pos.x - (1764.0 * 0.01 + 1044.0 * 0.2)).abs() < 1e-2,
            "{}",
            c.pos.x
        );
        let q = f.quads(1200.0, Vec3::Y, Vec3::Z);
        assert!(
            q.iter().any(|q| &*q.shader == "flamethrowerFire12"),
            "{:?}",
            q.iter().map(|q| &q.shader).collect::<Vec<_>>()
        );
        f.step(2100.0, 900.0, &[], Vec3::ZERO, None);
        assert!(f.chunks.iter().all(|c| c.smoke.is_some()));
    }

    /// A stream at a wall: its chunks' first bounces leave residues at the
    /// face, at most one per owner each 100 ms and none within 32 units of
    /// the last for 3 s, so the one spot holds one residue; it sheds still
    /// flames that burn a player standing in them after the stream is
    /// released, and it dies inside 2.5 s, its flames 1.4 s later. An ice
    /// wall takes none.
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
        assert!(r.death - 1000.0 <= 1500.0 + 1000.0 + 16.0, "{}", r.death);
        // Released; a player stands against the wall where it burns.
        let victim = Victim {
            client: 2,
            origin: Vec3::new(70.0, 0.0, 0.0),
            ..VICTIM
        };
        let mut burned = 0;
        while now < 3000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            if f.chunks.iter().any(|c| !c.residue && c.smoke.is_none()) {
                now += 16.0;
                continue;
            }
            burned += f.report(now, Some(victim), false, None).in_flame as u32;
            now += 16.0;
        }
        assert!(burned > 0);
        assert!(f.chunks.iter().any(|c| c.residue));
        while now < 6000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        assert!(f.residues.is_empty() && !f.chunks.iter().any(|c| c.residue));

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
                f.report(10_000.0 + 16.0 * i as f32, Some(VICTIM), false, Some(&t))
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
        };
        let mut f = FlameSystem::default();
        let mut now = 1000.0;
        while now < 1300.0 {
            f.step(now, 16.0, &[n], Vec3::ZERO, Some(&w));
            now += 16.0;
        }
        f.chunks
            .iter()
            .filter(|c| c.smoke.is_none())
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
        let flames: Vec<&Chunk> = f.chunks.iter().filter(|c| c.residue).collect();
        assert!(flames.len() > 5);
        assert!(flames.iter().all(|c| c.pos.x == r.pos.x && c.lift == 20.2));
        assert!(flames.iter().any(|c| (c.pos.y - r.pos.y).abs() > 1.0));
        // The oldest, a second old plus up to 300 ms of lag, have risen
        // 20.2 * age^2 / 2 in 500 ms units: 40 to 68.
        let top = flames.iter().map(|c| c.pos.z).fold(f32::MIN, f32::max);
        assert!((35.0..70.0).contains(&(top - r.pos.z)), "{top}");

        let (f, r) = residue_flames(Vec3::new(500.0, 0.0, 40.0), -Vec3::X);
        let flames: Vec<&Chunk> = f.chunks.iter().filter(|c| c.residue).collect();
        assert!(flames.iter().all(|c| c.pos.x < r.pos.x));
        assert!(flames.iter().all(|c| (25.3..=27.3).contains(&c.lift)));
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
            residue: true,
            ..fire(7, Vec3::new(97.0, 0.0, 40.0), 60.0, 1000.0, 5000.0)
        });
        let mut now = 1000.0;
        while f.chunks[0].dir.x > 0.0 && now < 2000.0 {
            f.step(now, 16.0, &[], Vec3::ZERO, Some(&bare(&world)));
            now += 16.0;
        }
        let c = f.chunks[0];
        assert!(c.braked);
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
}
