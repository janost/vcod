//! A player clone's own move, the frame CoD2 and CoD4 1.7 share:
//! `cloneplayer` throws it under gravity and each entity pass moves it,
//! its fall first and then its death anim's root motion, until it rests
//! (`G_RunCorpseMove`: CoD2 lnxded 0x8107846, `cod4_lnxded` 0x80b97e0;
//! docs/research/cod2-combat.md 6.2, cod4-combat.md 5.2). What differs
//! between the two games, their trace and their trajectory numbers, comes
//! in through [`CorpseWorld`]; `game::cod2_corpse` and `game::cod4::corpse`
//! run it each frame.

use glam::{DVec3, Vec3};
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::Protocol;

use super::bodies::{Body, Cod2Corpse};

/// The clone's `clipmask`: CoD2's `ent+0x184`, 1.7's +0x190 (0x80ad1ce).
/// VERIFIED.
pub const CLIPMASK: u32 = 0x1_0001;
/// The dying player's box, its top lowered to 30 by `player_die`.
pub const MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
pub const MAXS: Vec3 = Vec3::new(15.0, 15.0, 30.0);
/// `g_clonePlayerMaxVelocity`'s default in both games. VERIFIED.
pub const MAX_VELOCITY: f32 = 80.0;
/// `CONTENTS_NODROP`: a clone that stops with its origin in it is freed.
pub const CONTENTS_NODROP: u32 = 0x8000_0000;
/// Half of `g_gravity`'s 800, the fall's `t * t` factor.
const HALF_GRAVITY: f32 = 400.0;

/// What a frame did to a clone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Moved {
    /// Still falling, or at rest.
    Kept,
    /// Stopped in `CONTENTS_NODROP` and freed.
    Freed,
}

/// The clone's death anim on its own tree: the wire anim it plays and the
/// leaf's node time. The anim update after the move starts a new anim at
/// 0; each later frame steps it before the move (CoD2 lnxded 0x8107d30).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorpseAnim {
    pub wire: i32,
    pub time: f32,
}

/// One sweep of the clone's box as a game's trace reports it.
pub struct Sweep {
    /// How far the box got; 0 where the game's trace reads it stuck.
    pub fraction: f32,
    /// Where the clone ends up.
    pub end: Vec3,
    pub normal: Vec3,
    /// In solid all along: it rests where it is, its angles as they were.
    pub embedded: bool,
    /// The entity it came down on.
    pub ground: i32,
}

/// The trajectory types a clone rests and falls with.
#[derive(Clone, Copy)]
pub struct Kinds {
    pub rest: i32,
    pub fall: i32,
}

/// A game's side of the move.
pub trait CorpseWorld {
    /// The rest and fall types for a clone whose `pos.trType` is `now`.
    fn kinds(&self, now: i32) -> Kinds;
    /// The box swept under [`CLIPMASK`]; `fall` for the trajectory's sweep,
    /// as against the root motion's drag.
    fn sweep(&self, from: Vec3, to: Vec3, fall: bool) -> Sweep;
    /// Whether the box swept from `from` to `to` meets nothing.
    fn clear(&self, from: Vec3, to: Vec3) -> bool;
    /// Whether `at` is in `CONTENTS_NODROP`.
    fn no_drop(&self, at: Vec3) -> bool;
}

/// `from` moved `fraction` of the way to `to` on the x87 stack, stored
/// once (`Vec3Lerp`).
pub fn lerp(from: Vec3, to: Vec3, fraction: f32) -> Vec3 {
    let from = from.as_dvec3();
    (from + (to.as_dvec3() - from) * f64::from(fraction)).as_vec3()
}

/// `PlayerCmd_ClonePlayer`'s trajectory: `fall` from `origin` at `now_ms`
/// at the player's velocity, its x and y capped above (not below) at
/// `max_velocity`, no ground, no death anim yet (CoD2 cod2-combat.md 6.2;
/// 1.7's stores 0x80ad004..0x80ad082). VERIFIED.
pub fn launch(
    body: &mut Body,
    origin: Vec3,
    velocity: Vec3,
    now_ms: i32,
    max_velocity: f32,
    fall: i32,
    p: &Protocol,
) {
    let capped = Vec3::new(
        velocity.x.min(max_velocity),
        velocity.y.min(max_velocity),
        velocity.z,
    );
    let e = &mut body.state;
    put(e, p, "pos.trType", fall);
    put(e, p, "pos.trTime", now_ms);
    put_vec(e, p, "pos.trBase", origin);
    put_vec(e, p, "pos.trDelta", capped);
    put(e, p, "groundEntityNum", 1023);
    body.cod2 = Some(Cod2Corpse { origin, anim: None });
}

/// Where this frame takes the clone, and at what pace a drag that leaves
/// the ground drops it.
enum Goal {
    Fall(Vec3),
    Drag { to: Vec3, pace: Vec3 },
}

/// One frame of the clone's move at `now_ms`, `root` its death anim's
/// root motion since the last (`XAnimCalcDelta`).
///
/// A falling clone sweeps to its gravity point. Stopped on ground (a
/// normal with z above 0) or embedded, it rests there with the ground's
/// entity, its angles turned onto the surface unless embedded; stopped on
/// a wall, it is nudged a unit off along the normal and falls again from
/// there. A resting clone moves only when the root moved more than a unit
/// (`Vec3LengthSq > 1`, 1.7 0x80b9c3c): it sweeps to `pos.trBase` plus that
/// motion along its forward and left, rests at the end of a clear sweep,
/// and drops at the motion's pace when nothing is a unit below
/// (0x80b9b4c). A stop with the origin in `CONTENTS_NODROP` frees it.
pub fn step(
    world: &impl CorpseWorld,
    body: &mut Body,
    now_ms: i32,
    root: Vec3,
    p: &Protocol,
) -> Moved {
    let Some(mut corpse) = body.cod2 else {
        return Moved::Kept;
    };
    let e = &mut body.state;
    let tr_type = field(e, p, "pos.trType");
    let kinds = world.kinds(tr_type);
    let goal = if tr_type == kinds.fall {
        let t = (now_ms - field(e, p, "pos.trTime")) as f32 * 0.001;
        let mut at = field_vec(e, p, "pos.trBase") + field_vec(e, p, "pos.trDelta") * t;
        at.z -= HALF_GRAVITY * t * t;
        Goal::Fall(at)
    } else if root.as_dvec3().length_squared() > 1.0 {
        let (forward, left) = facing(field_vec(e, p, "apos.trBase"));
        let along = |from: Vec3| {
            let moved = from.as_dvec3() + forward * f64::from(root.x);
            let moved = moved.as_vec3().as_dvec3() + left * f64::from(root.y);
            moved.as_vec3()
        };
        Goal::Drag {
            to: along(field_vec(e, p, "pos.trBase")),
            pace: along(Vec3::ZERO),
        }
    } else {
        return Moved::Kept;
    };
    let to = match goal {
        Goal::Fall(to) | Goal::Drag { to, .. } => to,
    };
    let sweep = world.sweep(corpse.origin, to, matches!(goal, Goal::Fall(_)));
    corpse.origin = sweep.end;
    if sweep.fraction < 1.0 {
        if world.no_drop(corpse.origin) {
            body.cod2 = None;
            return Moved::Freed;
        }
        if let Goal::Fall(_) = goal {
            put_vec(e, p, "pos.trDelta", Vec3::ZERO);
            if sweep.embedded || sweep.normal.z > 0.0 {
                settle(e, p, kinds.rest, corpse.origin);
                put(e, p, "groundEntityNum", sweep.ground);
                if !sweep.embedded {
                    let angles = field_vec(e, p, "apos.trBase").to_array();
                    let on = super::spawn::align_to_surface(angles, sweep.normal.to_array(), false);
                    put_vec(e, p, "apos.trBase", Vec3::from(on));
                }
            } else {
                corpse.origin += sweep.normal;
                put_vec(e, p, "pos.trBase", corpse.origin);
                put(e, p, "pos.trTime", now_ms);
            }
        }
    } else if let Goal::Drag { pace, .. } = goal {
        settle(e, p, kinds.rest, corpse.origin);
        if world.clear(corpse.origin, to - Vec3::Z) {
            put(e, p, "pos.trType", kinds.fall);
            put_vec(e, p, "pos.trDelta", pace);
            put(e, p, "pos.trTime", now_ms);
        }
    }
    body.cod2 = Some(corpse);
    Moved::Kept
}

/// The clone at rest at `at`: `rest`, no time, no velocity.
fn settle(e: &mut EntityState, p: &Protocol, rest: i32, at: Vec3) {
    put(e, p, "pos.trType", rest);
    put(e, p, "pos.trTime", 0);
    put_vec(e, p, "pos.trBase", at);
    put_vec(e, p, "pos.trDelta", Vec3::ZERO);
}

/// `AngleVectors` of the clone's angles: its forward and its left (the
/// right reversed), each normalized with the length stored
/// (`Vec3Normalize`).
fn facing(angles: Vec3) -> (DVec3, DVec3) {
    let [forward, right, _] = vcod_common::x87::angle_vectors(angles);
    let unit = |v: Vec3| vcod_common::x87::normalize_stored(v).as_dvec3();
    (unit(forward), unit(-right))
}

/// A field of the clone's entity by its netfield name, 0 where the
/// protocol has none.
pub fn field(e: &EntityState, p: &Protocol, name: &str) -> i32 {
    EntityState::field_index(p, name).map_or(0, |i| e.fields[i])
}

pub fn put(e: &mut EntityState, p: &Protocol, name: &str, v: i32) {
    if let Some(i) = EntityState::field_index(p, name) {
        e.fields[i] = v;
    }
}

/// A float vector field (`pos.trBase` and kin) by its `[0]`..`[2]` names.
pub fn field_vec(e: &EntityState, p: &Protocol, name: &str) -> Vec3 {
    Vec3::from_array(std::array::from_fn(|k| {
        f32::from_bits(field(e, p, &format!("{name}[{k}]")) as u32)
    }))
}

pub fn put_vec(e: &mut EntityState, p: &Protocol, name: &str, v: Vec3) {
    for (k, x) in v.to_array().into_iter().enumerate() {
        put(e, p, &format!("{name}[{k}]"), x.to_bits() as i32);
    }
}
