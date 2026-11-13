//! Grenades in flight: `fire_grenade`, `G_RunMissile` and the explode,
//! read out in `docs/research/cod11-combat.md` sections 11 to 13. Every
//! constant here is a pointer into those sections, and where they and the
//! retail capture
//! `crates/server/tests/fixtures/playerstate/mp_carentan-tdm-grenade.txt`
//! disagree the capture wins; `crates/server/tests/missile_ab.rs` is what
//! holds the flight to it.
//!
//! Contains routines ported from the RTCW-MP GPL source, Copyright (C)
//! 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE. The
//! shapes are `g_missile.c`'s; the constants, the damping split and the
//! ordering are retail CoD 1.1's, which differ in most of them.

use crate::game::entity::ObjectTable;
use crate::spectate::{ClientSim, PmType};
use glam::Vec3;
use vcod_common::collision::{CONTENTS_WATER, CollisionWorld, sound_material};
use vcod_common::net::events::dir_to_byte;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD, Protocol};
use vcod_common::net::trajectory::{
    DEFAULT_GRAVITY, TR_GRAVITY, TR_LINEAR, TR_STATIONARY, Trajectory,
};
use vcod_common::pmove::PlayerState;
use vcod_common::pmove::cmd::EventRing;
use vcod_common::weapon::WeaponDef;
use vcod_common::x87;
use vcod_gsc::{Cx, EntId, ErrorKind, Value};

pub use vcod_common::net::event_ids::EV_GRENADE_BOUNCE;
pub use vcod_common::net::event_ids::EV_GRENADE_EXPLODE;
/// `ET_GENERAL`, what the explode flips `eType` to (13.1).
pub use vcod_common::net::flags::ET_GENERAL;
/// `ET_MISSILE` (11.1).
pub use vcod_common::net::flags::ET_MISSILE;

/// The `eFlags` bit `G_ExplodeMissile` ORs in (13.2). The two bounce bits
/// `fire_grenade` sets, `0x03000000`, are above the 24 the `eFlags` netfield
/// carries and never reach a client: the capture's flying missile reads
/// `eFlags=0` and its exploded one `eFlags=256`.
const EFLAGS_EXPLODED: i32 = 0x100;

/// 12.4's damping for a grenade, which is the one arm with both bounce bits
/// set: sliding friction on the tangential component and restitution along
/// the normal.
const BOUNCE_FRICTION: f32 = 0.75;
const BOUNCE_RESTITUTION: f32 = 0.3;
/// The same function's arm for a contact with a live player or with water.
const BOUNCE_SOFT: f32 = 0.125;
/// A surface this far from vertical counts as ground, for the rest test, the
/// ground snap and `groundEntityNum`.
const GROUND_NORMAL_Z: f32 = 0.7;
/// Below this speed a grenade on the ground stops (12.4).
const REST_SPEED: f32 = 20.0;
/// How much a contact has to change the velocity before it is worth an
/// event; the return value of `G_BounceMissile` gates 177 on it (12.4).
const BOUNCE_EVENT_SPEED: f32 = 100.0;
/// The per-frame snap that keeps a rolling grenade on the floor (12.2).
const GROUND_SNAP: f32 = 1.5;
/// The lift off the surface a bounce that did not rest takes (12.4).
const BOUNCE_NUDGE: f32 = 0.1;
/// The sky's surface bits (every stock sky material reads 0x434): 1.1's and
/// 1.5's `G_RunMissile` frees a missile whose move ends on 0x10 (1.1
/// 0x5432f, 1.5b 0x550e4); UO's skips that test and the impact on 0x4 and
/// pushes the contact 2 units into the sky (0x5f247, 0x5f533), combat doc
/// 12.3.
const SURF_SKY: u32 = 0x4;
pub(crate) const SURF_NOIMPACT: u32 = 0x10;
/// UO's push along `-normal` (`.rodata 0xa180c`, -2.0).
const UO_SKY_PUSH: f32 = 2.0;
/// The sky brushes' contents bit: UO's `G_MissileTrace` (0x5edf4) reads a
/// trace that starts inside one as clear.
const CONTENTS_SKY: u32 = 0x800;
/// How far the explode looks down for the normal its `eventParm` packs
/// (13.2).
const EXPLODE_TRACE_DOWN: f32 = 16.0;
/// The clipmask `fire_grenade` gives a missile (combat doc 11.1), as UO's
/// `fire_grenade` and `fire_rocket` do (game.mp.uo 0x5fc74): what a script
/// model's 0x2080 and a vehicle's collision surfaces meet.
pub(crate) const MISSILE_MODEL_MASK: u32 = vcod_common::collision::MASK_MISSILE_MOVE;
/// `0x14`, the surface type the water branches of 12.1 and 13.2 both use.
const WATER_SURF_TYPE: i32 = 0x14;
/// 12.1: a move whose `trDelta[2]` is past this (`.rodata 0x75B50`) and
/// that starts out of water traces water too, and splashes on it.
const WATER_ENTRY_SPEED: f32 = 30.0;
/// The surface type's bits in `surfaceFlags`.
const SURF_TYPE_MASK: u32 = 0x1F0_0000;
// How long an exploded missile stays on the wire before it is freed: the
// capture's three explodes each ride exactly 300 ms of snapshots past their
// event and are gone on the next (entity 176, serverTime 225150 to 225450).
use crate::game::temp_entity::EVENT_VALID_MS;
/// United Offensive's `G_BounceMissile` (game.mp.uo 0x5d6e5) scales the
/// whole reflected velocity, 0.4 off a surface and 0.1 in water or off a
/// body, where 1.1
/// splits it into a tangential and a normal part (docs/research/coduo-diff.md
/// 21).
const UO_BOUNCE: f32 = 0.4;
/// 1.5's `G_BounceMissile` (1.5b 0x53d71) scales the whole reflected
/// velocity by 0.5 off a surface, on either `eFlags` arm (combat doc 12.4).
const BOUNCE_15: f32 = 0.5;
const UO_BOUNCE_WATER: f32 = 0.1;
/// How far UO's `G_ExplodeMissile` looks up for the blast's centre
/// (0x5e2b5), and how far off the struck face `G_MissileImpact` (0x5db67)
/// puts a shell's.
const UO_BLAST_LIFT: f32 = 10.0;
/// Where the water branch of UO's `G_ExplodeMissile` starts its trace for
/// the surface, above the missile (.rodata 0xa17d0).
const UO_WATER_TRACE_UP: f32 = 500.0;
const UO_IMPACT_LIFT: f32 = 4.0;
/// `G_ExplodeSmokeGrenade` (0x5eb72): the popped grenade is freed this long
/// after its event.
pub const SMOKE_LIFE_MS: i32 = 15_000;
/// `projExplosionType smoke`'s index.
const SMOKE: i32 = 1;
/// `projExplosionType`'s names in enum order (game.mp.uo 0xab540); the
/// index rides an explode's `es.scale`.
pub const EXPLOSION_TYPES: [&str; 9] = [
    "grenade",
    "smoke",
    "rocket",
    "molotov",
    "artillery",
    "mortar",
    "tank",
    "b17",
    "none",
];

/// A weapon file's `projExplosionType` as its enum index; 1.1's files have
/// none, which reads as `grenade`.
pub fn explosion_type(name: &str) -> i32 {
    EXPLOSION_TYPES
        .iter()
        .position(|t| *t == name)
        .map_or(0, |i| i as i32)
}

/// The fuse `fire_grenade` falls back to when the thrower has no client or a
/// zero `grenadeTimeLeft` (11.1).
const DEFAULT_FUSE_MS: i32 = 2500;
/// The tumble `fire_grenade` draws (11.2): `flrand(-45, 45)` on top of these
/// two, in degrees per second, pitch and roll, never yaw.
const TUMBLE_PITCH: f32 = 720.0;
const TUMBLE_ROLL: f32 = 360.0;
const TUMBLE_SPREAD: f32 = 45.0;
/// `vectoangles(velocity)` has this taken off its pitch before it becomes the
/// launch angles (11.2).
const LAUNCH_PITCH_LEAD: f32 = 120.0;
/// `G_MissileLandAngles` (12.7): a contact whose normal's z is not above
/// this is a wall, which only nudges the pitch rate; past it the pitch is
/// laid onto the surface by how far it is off it.
const LAND_FLOOR_Z: f32 = 0.1;
const LAND_FLAT_OFF: f32 = 45.0;
const LAND_TILT_OFF: f32 = 80.0;

/// What one `G_RunMissile` did.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Stepped {
    /// It flew, or it was already at rest and did not move at all.
    Flew,
    /// It hit something and the contact was loud enough for event 177.
    Bounced,
    /// It hit something quietly, or came to rest on it.
    Settled,
    /// A shell struck something; the caller explodes it on the contact.
    Struck(Contact),
    /// Its move ended on the sky: `G_FreeEntity`, no event, no think.
    Freed,
}

/// Where a shell struck (`G_MissileImpact`'s trace).
#[derive(PartialEq, Debug, Clone, Copy)]
pub struct Contact {
    pub at: Vec3,
    pub normal: Vec3,
    pub material: i32,
    /// The client it struck, which takes the direct hit.
    pub player: Option<usize>,
    /// The entity the trace stopped on (`trace.entityNum`): a vehicle's
    /// collmap names its vehicle.
    pub entity: u32,
    /// It met an xmodel's collision (a vehicle's hull), whose bone name
    /// the trace hands back at +42.
    pub model: bool,
}

/// An artillery shell's own state (`fire_artillery`, 0x5ff4a): it hangs
/// unsent where it was spawned until `G_DropArtillery` (0x5eabe) drops it,
/// then explodes on contact rather than bouncing.
#[derive(Clone, Copy, Debug)]
pub struct Shell {
    /// When it drops; `None` once it has.
    pub drop_at_ms: Option<i32>,
    /// `projectileSpeed`, the drop's downward speed.
    pub speed: f32,
    /// `damage`, charged whole to what it struck (`Damage_Falloff` with the
    /// stock file's 100 `minDamagePercent` leaves it whole).
    pub damage: i32,
    /// A UO vehicle's main gun fired it (`Weapon_Artillery_Fire` 0x7b594,
    /// `fire_rocket`): the vehicle is the inflictor and the means of death
    /// `MOD_PROJECTILE`, not an artillery strike's.
    pub vehicle: Option<EntId>,
}

/// One grenade in the air. The entity is the object table's, so a script
/// walking `getEntArray("grenade", "classname")` finds it, but what reaches
/// the wire is built here rather than by `crate::game::wire`: a missile's
/// state is a trajectory and an event ring, none of which lives in the
/// script fields.
pub struct Missile {
    pub id: EntId,
    /// The thrower's client slot, `r.ownerNum`: what the move trace passes
    /// through and who the blast is charged to. `None` once the thrower has
    /// left: `G_FreeEntity`'s sweep stores 0x3ff there and NULL in `parent`
    /// (combat doc 13.7), so nothing is passed and nothing is charged.
    pub owner: Option<usize>,
    /// With no owner, `parent` is `&g_entities[1023]` rather than NULL: a
    /// shell a barrage hung after its caller left (`fire_artillery` takes
    /// the barrage's swept 0x3ff). Only the fuse's `G_ExplodeMissile` reads
    /// `parent`, and walks its splash charged to it (combat doc 13.7).
    parent_unspawned: bool,
    /// `s.weapon`, the 1-based configstring 7 index. The client draws the
    /// projectile off it.
    pub weapon: u8,
    /// The projectile model's configstring index. Retail's `fire_grenade`
    /// writes no `s.index` -- every `!missile` line reads `index=0` -- so
    /// this never reaches the wire; the registration it comes from is what
    /// puts the model in the client's table.
    pub model: i32,
    pub traj: Trajectory,
    pub apos: Trajectory,
    /// `nextthink`: the frame at or past which `G_ExplodeMissile` runs.
    pub explode_at_ms: i32,
    /// `r.currentOrigin`, where the last step left it.
    pub origin: Vec3,
    pub events: EventRing,
    /// `level.previousTime` as this missile sees it: the frame it was last
    /// run on, which is what the bounce interpolates the impact time inside.
    last_run_ms: i32,
    surf_type: i32,
    ground: i32,
    /// When the explode event went on the ring, if it has.
    exploded_ms: Option<i32>,
    /// `G_ExplodeMissile` set it off (the fuse), which ORs in
    /// [`EFLAGS_EXPLODED`]; `G_MissileImpact` sets no such bit.
    fused: bool,
    /// United Offensive's missile rules: its bounce, its explode's type in
    /// `es.scale` and no concussion entity.
    pub uo: bool,
    /// 1.5's and UO's `G_RunMissile` opens with a fall test (1.5b 0x54ad7,
    /// UO 0x5ee9c): a resting missile whose ground is not the world (0x3fe)
    /// traces 1.5 down and, with nothing there, takes `TR_GRAVITY` from
    /// where it lies (docs/research/cod11-combat.md 12.6). 1.1 has none.
    /// The same two snap a missile down onto the world only (12.2).
    pub falls: bool,
    /// `projExplosionType`'s index, sent on the explode under UO.
    pub explosion_type: i32,
    /// `eFlags` the spawn set (a shell's 0x8000, in 1.1's layout).
    pub eflags: i32,
    /// A shell, which explodes on contact.
    pub shell: Option<Shell>,
    /// `fire_rocket`'s: its blast and direct hit are `MOD_PROJECTILE`'s.
    rocket: bool,
    /// A popped smoke grenade's free; it stays a missile until then.
    free_at_ms: Option<i32>,
    /// UO's shootable grenade, the satchel charge.
    pub shootable: Option<Shootable>,
    /// Dropped live by a thrower killed with it cooking (combat doc 5.1
    /// step 5), not thrown: bots by him see it fall.
    pub dropped: bool,
    /// The water entry splash this step raised (12.1), which the pool
    /// hands its caller to spawn.
    splash: Option<crate::game::temp_entity::TempEntity>,
}

/// What UO's `fire_grenade` (0x5f7ad) gives a `takeDamage` grenade whose
/// fuse runs past 500 ms: a box with contents 0x2000 and 1 health that
/// `G_GrenadeDie` (0x5f683) answers, and `G_GrenadeLeaveOwnerThink`
/// (0x5f6b7) as its think until it is clear of every body
/// (coduo-diff.md 21).
#[derive(Clone, Copy, Debug)]
pub struct Shootable {
    /// `takedamage`, which `G_GrenadeDie` clears.
    pub takedamage: bool,
    /// `r.ownerNum` is still the thrower: his traces pass it and it passes
    /// him. The leave-owner think makes it its own owner.
    pub owner_passes: bool,
    /// The think is still `G_GrenadeLeaveOwnerThink`.
    leave_owner: bool,
    /// `+0x32c`, the fuse the think hands back once the grenade is clear.
    fuse_at_ms: i32,
}

/// The shootable grenade's box and contents (`fire_grenade` 0x5f7ad).
pub const SHOOTABLE_MINS: Vec3 = Vec3::new(-6.0, -6.0, 0.0);
pub const SHOOTABLE_MAXS: Vec3 = Vec3::new(6.0, 6.0, 6.0);
pub const SHOOTABLE_CONTENTS: i32 = 0x2000;
/// `G_GrenadeLeaveOwnerThink`'s retry while a body still holds the grenade,
/// and how close to the fuse it gives up; `G_GrenadeDie`'s delay.
const LEAVE_OWNER_RETRY_MS: i32 = 100;
const GRENADE_DIE_MS: i32 = 100;

/// One blast for the radius damage pass to charge.
pub struct Explosion {
    /// `parent` (the splash's attacker) and `r.ownerNum` (the direct hit's):
    /// `None` when the thrower left before it went off, and then
    /// `G_RadiusDamage` returns before its walk (combat doc 13.7).
    /// [`crate::game::host::ZEROED_ENTITY`] for a fuse's blast charged to
    /// `&g_entities[1023]`, and a UO vehicle shell's fuse blast names the
    /// vehicle's entity number.
    pub owner: Option<usize>,
    pub inflictor: EntId,
    pub weapon: u8,
    pub at: Vec3,
    /// The splash means of death: `MOD_GRENADE_SPLASH` for a thrown
    /// grenade, `MOD_ARTILLERY_SPLASH` for a shell (`fire_artillery`'s
    /// `+0x25c`).
    pub mod_: &'static str,
    /// A shell's direct hit: the client it struck, which the walk skips,
    /// and what it takes under [`Explosion::direct_mod`] first.
    pub direct: Option<(usize, i32)>,
    /// `MOD_ARTILLERY`, or a vehicle shell's `MOD_PROJECTILE`.
    pub direct_mod: &'static str,
    /// 1.1's `G_ExplodeMissile` spawns a concussion entity; UO's does not.
    pub concussive: bool,
    /// The entity a shell struck, `None` for a fuse's blast. A vehicle
    /// takes the direct hit as a client does, and the splash skips it.
    pub struck: Option<u32>,
    /// Where the shell struck (`r.currentOrigin` at the contact) and the
    /// way it flew there (`BG_EvaluateTrajectoryDelta`): the point and
    /// the direction `G_MissileImpact` hands the struck entity's
    /// `G_Damage` (game.mp.uo 0x5db67); UO's trigger walk tests the point
    /// alone (combat doc 14.8).
    pub contact: Vec3,
    pub flight: Vec3,
    /// A shell's `damage`, which the struck entity takes whole.
    pub shell_damage: i32,
}

/// What one frame of the missile pass produced. `temp` holds the water
/// entry splashes (12.1) only: both explode paths write the event onto the
/// missile's own entity rather than spawning a temp entity (13).
pub struct MissileFrame {
    pub exploded: Vec<Explosion>,
    pub temp: Vec<crate::game::temp_entity::TempEntity>,
    /// Missiles whose ridden-out event freed them this frame. `Missiles` has
    /// no `&mut GameHost` to free them itself, so the caller does it through
    /// `GameHost::free_entity`, the one place an entity going away is wired
    /// up to the tables that hang off it (triggers today).
    pub freed: Vec<EntId>,
}

/// What one missile's turn did ([`Missiles::run_one`]).
pub enum Turn {
    Flew,
    /// It went off this turn; the blast walk is the caller's.
    Exploded(Explosion),
    /// Its event aged out and it left the pool; the caller frees the entity.
    Freed,
}

#[derive(Default)]
pub struct Missiles {
    live: Vec<Missile>,
    /// The server speaks United Offensive's protocol: every missile takes
    /// its rules.
    pub uo: bool,
    /// The server speaks 1.5's or UO's protocol: a resting missile falls
    /// once its floor is gone ([`Missile::falls`]).
    pub falls: bool,
    /// The launch tumble's and the bounces' draws.
    draws: Draws,
    /// Water entry splashes raised since the caller last took them
    /// ([`Missiles::take_splashes`]).
    splashes: Vec<crate::game::temp_entity::TempEntity>,
    /// Test-facing: the `trDelta` the next rockets leave with, a retail
    /// capture's, in place of the spread's ([`Missiles::pin_rocket_deltas`]).
    rocket_deltas: std::collections::VecDeque<Vec3>,
}

/// Section 11.3: where a throw starts and how fast it leaves, from the
/// thrower's aim, which is `FireWeapon`'s same `client+0x220c` pair as a
/// bullet's (`ClientSim::aim_angles`). Returns `(origin, velocity)`.
pub fn throw_velocity(ps: &PlayerState, aim: [f32; 2], def: &WeaponDef) -> (Vec3, Vec3) {
    let origin = crate::game::combat::muzzle_point(ps);
    let (yaw, pitch) = crate::game::combat::aim_radians(aim);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    // `projectileSpeed` times each axis stored, `projectileSpeedUp` added
    // to z on the stack (`FireWeapon` 0x6900c). `fire_grenade` truncates;
    // the launch angles read it unrounded.
    let speed = def.projectile_speed;
    let z = f64::from(speed) * f64::from(forward.z) + f64::from(def.projectile_speed_up);
    (
        origin,
        Vec3::new(speed * forward.x, speed * forward.y, z as f32),
    )
}

/// `fire_rocket` normalizes the direction it is handed once more before it
/// scales it (1.1d 0x54627, 1.5b 0x5559a, UO 0x5fc8c): a launcher's
/// already normalized one can move a last bit under 1.5b's and UO's
/// stored `VectorNormalize` (combat doc 13.6). `stored` is 1.5b's and UO's.
pub(crate) fn rocket_launch_dir(dir: Vec3, stored: bool) -> Vec3 {
    if stored {
        vcod_common::x87::normalize_stored(dir)
    } else {
        vcod_common::x87::normalize_in_place(dir)
    }
}

/// `BG_EvaluateTrajectoryDelta` for the two types a missile ever carries:
/// `delta.z - dt * 800` on the x87 stack, rounded once. `kept` is 1.1d's
/// build (`.so` 0x2c964), whose `ms * 0.001` stays on the stack; 1.5b and
/// UO store it as a float first (1.5b 0x1c01d, UO alike).
fn delta_at(traj: &Trajectory, at_ms: i32, kept: bool) -> Vec3 {
    match traj.tr_type {
        TR_GRAVITY => {
            let dt = if kept {
                f64::from(at_ms - traj.tr_time) * f64::from(0.001f32)
            } else {
                f64::from((at_ms - traj.tr_time) as f32 * 0.001)
            };
            let z = f64::from(traj.delta.z) - dt * f64::from(DEFAULT_GRAVITY);
            traj.delta.with_z(z as f32)
        }
        TR_LINEAR => traj.delta,
        _ => Vec3::ZERO,
    }
}

/// `vectoangles` (1.1d 0x3de94, UO's alike): pitch down positive, both
/// angles in [0, 360) (combat doc 11.2, 13.6).
fn vectoangles(v: Vec3) -> Vec3 {
    Vec3::from(crate::game::builtins::math::vectoangles(v.into()))
}

/// 1.1d's `G_BounceMissile` damping (0x53828-0x53942) for a grenade, both
/// bounce bits set: the reflection `v - 2 (v . n) n` with the dot on the
/// x87 stack, each component stored; then off a surface the stored
/// reflection plus `n (v . n)` (the tangential part) times 0.75 less
/// `n (v . n) 0.3`, all on the stack and each component rounded once, or
/// the stored reflection times 0.125 off a body or in water.
fn bounce_kept(v: Vec3, n: Vec3, soft: bool) -> Vec3 {
    let dot = x87::dot(v, n);
    let (vd, nd) = (v.as_dvec3(), n.as_dvec3());
    let reflected = (vd + nd * (-2.0 * dot)).as_vec3();
    if soft {
        return reflected * BOUNCE_SOFT;
    }
    let r = reflected.as_dvec3();
    let friction = f64::from(BOUNCE_FRICTION);
    let restitution = dot * f64::from(BOUNCE_RESTITUTION);
    ((r + nd * dot) * friction - nd * restitution).as_vec3()
}

impl Missile {
    /// The client its move trace passes, `r.ownerNum`: the thrower, until a
    /// shootable grenade's leave-owner think makes it its own owner.
    fn passes(&self) -> usize {
        match self.shootable {
            Some(s) if !s.owner_passes => usize::MAX,
            _ => self.owner.unwrap_or(usize::MAX),
        }
    }

    /// `G_GrenadeLeaveOwnerThink` (0x5f6b7): a point test of the grenade's
    /// origin against every body (mask 0x2000000, no pass entity); while one
    /// holds it and the fuse is more than 100 ms off, again 100 ms on, else
    /// the thrower stops passing and the fuse comes back.
    fn leave_owner(&mut self, sims: &[(usize, &ClientSim)], now_ms: i32) {
        let Some(s) = self.shootable.as_mut() else {
            return;
        };
        let p = self.origin;
        let held = sims.iter().any(|(_, sim)| {
            sim.pm_type == PmType::Normal
                && !sim.dead
                && (sim.ps.origin + sim.ps.mins()).cmple(p).all()
                && (sim.ps.origin + sim.ps.maxs()).cmpge(p).all()
        });
        if now_ms < s.fuse_at_ms - LEAVE_OWNER_RETRY_MS && held {
            self.explode_at_ms = now_ms + LEAVE_OWNER_RETRY_MS;
            return;
        }
        self.explode_at_ms = s.fuse_at_ms;
        s.owner_passes = false;
        s.leave_owner = false;
    }

    /// `G_RunMissile`'s move trace under the clipmask every protocol's
    /// `fire_grenade` and `fire_rocket` store, 0x2802091 (1.1d 0x544a0,
    /// read at 0x5409a; UO 0x5f7ad, read at 0x5ee7d; combat doc 11.1),
    /// which meets the 0x2080 kerb brushes too.
    fn trace(&self, world: &CollisionWorld, from: Vec3, to: Vec3) -> vcod_common::collision::Trace {
        let tr = world.missile_trace(from, to);
        // UO's `G_MissileTrace`: start-solid in a sky brush is a clean miss,
        // which is how a UO missile pushed into the sky flies on above it.
        if self.uo && tr.startsolid && world.point_contents(from) & CONTENTS_SKY != 0 {
            return vcod_common::collision::Trace::clear(to);
        }
        tr
    }

    /// `G_RunMissile`'s `trap_LocationalTrace` from `from` to `to`: the
    /// world, script models and UO's vehicles, and every live body but the
    /// passed owner's on its posed bones under `bulletPriorityMap` (1.1
    /// 0x54095, UO `G_MissileTrace` 0x5edc0), bouncing off the struck face.
    fn full_trace(
        &self,
        world: &CollisionWorld,
        sims: &[(usize, &ClientSim)],
        models: &[crate::game::combat::PlacedModel],
        bones: Option<&mut crate::game::combat::BoneTraceCtx>,
        from: Vec3,
        to: Vec3,
    ) -> MissileHit {
        let travel = to - from;
        // Script models and UO's vehicles: the move trace is locational
        // under the clipmask 0x2802091 (`fire_grenade`, `fire_rocket`),
        // which meets a script model's 0x2080 and a vehicle's 0xa00000, and
        // passes `r.ownerNum`: a shell's own vehicle, or the thrower (while
        // he passes) and what he drives.
        let pass = self
            .shell
            .and_then(|s| s.vehicle)
            .map(|v| v.0)
            .or_else(|| (self.passes() != usize::MAX).then_some(self.passes() as u32));
        let mut hit = MissileHit {
            tr: self.trace(world, from, to),
            player: None,
            model: None,
            model_takes_damage: false,
            model_body: false,
        };
        for m in models {
            if pass.is_some_and(|p| m.id.0 == p || m.owner == Some(p)) {
                continue;
            }
            if let Some((f, n, flags)) = m.clip(from, to, MISSILE_MODEL_MASK, hit.tr.fraction) {
                hit.tr.fraction = f;
                hit.tr.endpos = from + travel * f;
                hit.tr.normal = n;
                hit.tr.surface_flags = flags;
                hit.model = Some(m.id.0);
                hit.model_takes_damage = m.takes_damage;
                hit.model_body = m.body;
            }
        }
        let bodies: Vec<crate::game::combat::HitBody> = sims
            .iter()
            .filter_map(|(slot, sim)| sim.hit_body(*slot))
            .collect();
        if let Some(h) = crate::game::combat::trace_bodies_faced(
            from,
            to,
            self.passes(),
            &bodies,
            hit.tr.fraction,
            &crate::game::combat::BULLET_PRIORITY,
            bones,
        ) {
            hit.tr.fraction = h.fraction;
            let arith = if self.kept() {
                vcod_common::patch::Arith::Kept
            } else {
                vcod_common::patch::Arith::Stored
            };
            hit.tr.endpos = crate::game::combat::trace_point(from, to, h.fraction, arith);
            hit.tr.normal = h.normal;
            hit.tr.surface_flags = 0;
            hit.player = Some(h.slot);
            hit.model = None;
            hit.model_body = false;
        }
        // 1.5's and UO's `G_MissileTrace` (1.5b 0x54a5e, UO 0x5ee1e): any
        // other start-solid reads as a contact at the start, its normal
        // back along the move (combat doc 12.3).
        if self.falls && hit.tr.startsolid {
            hit.tr.fraction = 0.0;
            hit.tr.normal = (from - to).normalize_or_zero();
        }
        hit
    }

    /// 12.1's water entry splash, `G_TempEntity` at `r.currentOrigin`
    /// before the move: `EV_BULLET_HIT_SMALL` (UO's 178) with the water
    /// face's normal, the flight with its z made non-negative in `scale`,
    /// surface type 0x14 and the missile's own number; UO's carries the
    /// weapon too (1.1 0x54115, 1.5b 0x54d8d, UO 0x5f14d).
    fn splash_at(&self, at: Vec3, normal: Vec3) -> crate::game::temp_entity::TempEntity {
        let mut d = self.traj.delta.normalize_or_zero();
        d.z = d.z.abs();
        crate::game::temp_entity::TempEntity {
            event: vcod_common::net::event_ids::EV_BULLET_HIT_SMALL,
            parm: dir_to_byte(normal.into()),
            surf_type: WATER_SURF_TYPE,
            other: self.id.0,
            attacker: 0,
            weapon: if self.uo { self.weapon as i32 } else { 0 },
            client_num: 0,
            scale: dir_to_byte(d.into()),
            origin: at.into(),
            scope: crate::game::temp_entity::Scope::Pvs,
            quake: None,
            origin2: None,
        }
    }

    /// One `G_RunMissile` (12): fly to where the arc says this frame, then
    /// bounce, rest or carry on. The caller checks the fuse afterwards, the
    /// way `G_RunThink` closes retail's frame.
    pub fn step(
        &mut self,
        world: Option<&CollisionWorld>,
        sims: &[(usize, &ClientSim)],
        models: &[crate::game::combat::PlacedModel],
        mut bones: Option<&mut crate::game::combat::BoneTraceCtx>,
        draws: &mut Draws,
        now_ms: i32,
    ) -> Stepped {
        let prev_ms = std::mem::replace(&mut self.last_run_ms, now_ms);
        if let Some(shell) = self.shell.as_mut()
            && let Some(at) = shell.drop_at_ms
        {
            // `fire_artillery`'s `groundEntityNum` 0x3fe keeps the hanging
            // shell off `G_RunMissile`'s fall test; `G_DropArtillery` is its
            // think, due at the spawn plus the delay.
            if now_ms < at {
                return Stepped::Flew;
            }
            shell.drop_at_ms = None;
            // `G_DropArtillery` (0x5eabe) makes `G_ExplodeMissile` the think
            // 15 s on (0x5eadc): a shell that strikes nothing goes off then.
            self.explode_at_ms = now_ms + SHELL_FALLBACK_MS;
            self.traj = Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now_ms - FRAME_BEHIND_MS,
                tr_duration: 0,
                base: self.traj.base,
                delta: Vec3::new(0.0, 0.0, -shell.speed),
            };
            // `G_RunThink` closes the frame: the drop moves it next frame.
            return Stepped::Flew;
        }
        if self.falls
            && self.traj.tr_type == TR_STATIONARY
            && self.ground != ENTITYNUM_WORLD as i32
            && let Some(world) = world
        {
            let below = self.origin - Vec3::Z * GROUND_SNAP;
            let down = self.full_trace(
                world,
                sims,
                models,
                bones.as_deref_mut(),
                self.origin,
                below,
            );
            if down.tr.fraction >= 1.0 {
                self.traj = Trajectory {
                    tr_type: TR_GRAVITY,
                    tr_time: now_ms,
                    tr_duration: 0,
                    base: self.origin,
                    delta: Vec3::ZERO,
                };
            }
        }
        let kept = self.kept();
        let to = if kept {
            self.traj.evaluate_kept(now_ms)
        } else {
            self.traj.evaluate_stored(now_ms)
        };
        let travel = to - self.origin;
        // 12.1: a move shorter than this is not traced at all, which is what
        // makes a resting missile free. 1.1d's `VectorNormalize` leaves the
        // length on the stack (0x54034); 1.5b's and UO's store it (1.5b
        // 0x54c61, `x87::normalize_stored`).
        let moved = if kept {
            x87::dot(travel, travel).sqrt()
        } else {
            f64::from(f64::from(x87::dot(travel, travel) as f32).sqrt() as f32)
        };
        if moved < f64::from(0.001f32) {
            return Stepped::Flew;
        }
        let Some(world) = world else {
            self.origin = to;
            return Stepped::Flew;
        };
        // 12.1's move trace, then 12.2's ground snap into the same trace
        // struct: 12.3 reads the downward trace and not the move, which is
        // what makes a rolling grenade bounce off the floor below it every
        // frame.
        // 12.1: a fast move out of water traces water too (mask | 0x20);
        // one that meets water first splashes and is traced again without
        // it, which is the plain trace.
        let wet = self.traj.delta.z.abs() > WATER_ENTRY_SPEED
            && world.point_contents(self.origin) & CONTENTS_WATER == 0;
        let MissileHit {
            mut tr,
            mut player,
            mut model,
            mut model_takes_damage,
            mut model_body,
        } = self.full_trace(world, sims, models, bones.as_deref_mut(), self.origin, to);
        if wet
            && let Some(w) = world.water_hit(self.origin, to)
            && w.fraction < tr.fraction
            && w.surface_flags & SURF_TYPE_MASK == (WATER_SURF_TYPE as u32) << 20
        {
            self.splash = Some(self.splash_at(self.origin, w.normal));
            // UO frees a smoke grenade on the splash (0x5f1f3).
            if self.uo && self.explosion_type == SMOKE {
                return Stepped::Freed;
            }
        }
        if self.uo && tr.surface_flags & SURF_SKY != 0 {
            tr.endpos -= tr.normal * UO_SKY_PUSH;
        }
        self.origin = tr.endpos;
        // `r.currentOrigin` takes the end, then a start-solid move reads as
        // a contact (1.1 0x5420c, UO 0x5f367).
        if tr.startsolid {
            tr.fraction = 0.0;
        }
        // A shell carries no bounce bits (`fire_rocket` sets only 0x10000).
        if self.shell.is_none() && (tr.fraction >= 1.0 || tr.normal.z > GROUND_NORMAL_Z) {
            // The snap is a `trap_LocationalTrace` under `bulletPriorityMap`
            // too (0x54285, the call at 0x542a4): on 1.1 a grenade on a
            // player's bones snaps to them; 1.5 and UO snap only onto the
            // world (1.5b 0x54ffe, UO 0x5f44d, 12.2).
            let below = self.origin - Vec3::Z * GROUND_SNAP;
            let down = self.full_trace(world, sims, models, bones, self.origin, below);
            let onto = !self.falls
                || (down.player.is_none()
                    && down.model.is_none()
                    && world.entity_num(&down.tr) == ENTITYNUM_WORLD);
            if down.tr.fraction < 1.0 && onto {
                // The snapped height stays on the stack into both stores on
                // all three builds (1.1d 0x542bd, 1.5b 0x5501f, UO 0x5f477).
                let z = f64::from(down.tr.endpos.z) + f64::from(GROUND_SNAP);
                let base = (z - f64::from(self.origin.z)) + f64::from(self.traj.base.z);
                self.traj.base.z = base as f32;
                self.origin.z = z as f32;
                MissileHit {
                    tr,
                    player,
                    model,
                    model_takes_damage,
                    model_body,
                } = down;
            }
        }
        if tr.fraction >= 1.0 {
            // 12.3: nothing was hit, and a moving missile has no ground.
            if self.traj.delta != Vec3::ZERO {
                self.ground = ENTITYNUM_NONE as i32;
            }
            return Stepped::Flew;
        }
        // 12.3: on UO a sky contact neither frees nor strikes, and the
        // missile flies on past the sky; 1.1 and 1.5 free it silently.
        if self.uo && tr.surface_flags & SURF_SKY != 0 {
            return Stepped::Flew;
        }
        if !self.uo && tr.surface_flags & SURF_NOIMPACT != 0 {
            return Stepped::Freed;
        }
        // The material goes in the event parm and nowhere else: 13.1 step 5
        // leaves `s.surfType` to the explode, and the client keys the bounce
        // alias off `surfType` (`cod11-sound-system.md` section 7a).
        let material = sound_material(tr.surface_flags);
        if self.shell.is_some() {
            return Stepped::Struck(Contact {
                at: tr.endpos,
                normal: tr.normal,
                material,
                player,
                entity: player
                    .map(|slot| slot as u32)
                    .or(model)
                    .unwrap_or_else(|| world.entity_num(&tr)),
                model: model.is_some(),
            });
        }
        let in_water = world.point_contents(self.origin) & CONTENTS_WATER != 0;
        // `tr.entityNum`: the player, the script model or vehicle, or the
        // brush entity struck.
        let ground = player
            .map(|slot| slot as u32)
            .or(model)
            .unwrap_or_else(|| world.entity_num(&tr));
        // The bounce keeps the soft scale in water and off a body
        // (`tr.contents & 0x2000000`): a player, or a script model a UO
        // script gave those contents (12.4).
        let silent = in_water || player.is_some();
        let soft = silent || model_body;
        if self.uo {
            // UO's `G_MissileImpact` bounces off a player or a vehicle that
            // takes damage with no event (0x5d841).
            let quiet = player.is_some() || (model.is_some() && model_takes_damage);
            let loud = self.bounce(prev_ms, now_ms, &tr, ground, soft, silent, draws) && !quiet;
            if loud {
                self.events.add(EV_GRENADE_BOUNCE, material);
                return Stepped::Bounced;
            }
            return Stepped::Settled;
        }
        let loud = self.bounce(prev_ms, now_ms, &tr, ground, soft, silent, draws);
        if loud {
            self.events.add(EV_GRENADE_BOUNCE, material);
            Stepped::Bounced
        } else {
            Stepped::Settled
        }
    }

    /// `G_BounceMissile` (12.4). Returns whether the contact was loud enough
    /// for the caller to raise 177.
    #[allow(clippy::too_many_arguments)]
    fn bounce(
        &mut self,
        prev_ms: i32,
        now_ms: i32,
        tr: &vcod_common::collision::Trace,
        entity: u32,
        soft: bool,
        silent: bool,
        draws: &mut Draws,
    ) -> bool {
        // The impact time is interpolated in whole milliseconds, and the
        // fraction is whichever trace 12.3 read -- the downward one on a
        // ground snap, where it is a fraction of 1.5 units and not of the
        // frame. That is retail's arithmetic, and the capture's bounce
        // deltas only come out on it. All three builds keep the product on
        // the x87 stack and truncate it (1.1d 0x537f4, 1.5b 0x53de6).
        let kept = self.kept();
        let at = prev_ms + (f64::from(now_ms - prev_ms) * f64::from(tr.fraction)) as i32;
        let v = delta_at(&self.traj, at, kept);
        let n = tr.normal;
        if n.z > GROUND_NORMAL_Z {
            self.ground = entity as i32;
        }
        let damped = if kept {
            bounce_kept(v, n, soft)
        } else {
            // 1.5b and UO store the dot, then round each component of
            // `dot * -2 * n + v` once (1.5b 0x53e33-0x53e87, UO 0x5d7a7).
            let dot = f64::from(x87::dot(v, n) as f32);
            let reflected = (v.as_dvec3() + n.as_dvec3() * (dot * -2.0)).as_vec3();
            if self.uo {
                reflected * if soft { UO_BOUNCE_WATER } else { UO_BOUNCE }
            } else if soft {
                reflected * BOUNCE_SOFT
            } else {
                // 1.5: `falls` without `uo`.
                reflected * BOUNCE_15
            }
        };
        self.traj.delta = damped;
        // The squares summed on the stack, the root stored (1.1d 0x5396e,
        // 1.5b 0x53fec).
        let speed = x87::dot(damped, damped).sqrt() as f32;
        if n.z > GROUND_NORMAL_Z && speed < REST_SPEED {
            // `G_SetOrigin` and `G_SetAngle`: trType 0, trTime 0, no delta.
            self.traj = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base: self.origin,
                delta: Vec3::ZERO,
            };
            let base = self.land_angles(prev_ms, now_ms, tr, true, draws);
            self.apos = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base,
                delta: Vec3::ZERO,
            };
            return false;
        }
        let mut nudge = n * BOUNCE_NUDGE;
        nudge.z = nudge.z.min(0.0);
        self.origin += nudge;
        self.traj.base = self.origin;
        self.traj.tr_time = now_ms;
        self.apos.base = self.land_angles(prev_ms, now_ms, tr, false, draws);
        self.apos.tr_time = now_ms;
        // Each component of the change stored, their squares summed on the
        // stack and the root stored (1.1d 0x53a5e, 1.5b 0x541c2).
        let d = damped - v;
        let change = x87::dot(d, d).sqrt() as f32;
        !silent && change > BOUNCE_EVENT_SPEED
    }

    /// `G_MissileLandAngles` (1.1d 0x535d0, 1.5b 0x53ad0, UO 0x5d444; 12.7):
    /// the angles a contact leaves, off the tumble at the impact time. Yaw
    /// and roll come back as the tumble had them, unwrapped. On a wall the
    /// pitch rate takes a whole-degree step; on a floor it is scaled (and
    /// reversed when the pitch lies within 80 of the surface's) and the
    /// pitch laid flat within 45, eased a quarter of the way within 80.
    /// `rest` is `G_BounceMissile`'s call before `G_SetAngle`: no draw, and
    /// the pitch always laid flat.
    fn land_angles(
        &mut self,
        prev_ms: i32,
        now_ms: i32,
        tr: &vcod_common::collision::Trace,
        rest: bool,
        draws: &mut Draws,
    ) -> Vec3 {
        use vcod_common::pmove::aim::{angle_normalize_180, angle_normalize_360, angle_subtract};
        // Its own impact time, on the x87 stack in all three builds.
        let at = prev_ms + (f64::from(now_ms - prev_ms) * f64::from(tr.fraction)) as i32;
        let mut out = if self.kept() {
            self.apos.evaluate_kept(at)
        } else {
            self.apos.evaluate_stored(at)
        };
        if tr.normal.z <= LAND_FLOOR_Z {
            if !rest {
                let rate = draws.land_step() + f64::from(self.apos.delta.x);
                self.apos.delta.x = angle_normalize_360(rate as f32);
            }
            return out;
        }
        let flat = vcod_common::pmove::pitch_for_yaw_on_normal(out.y, tr.normal);
        let diff = angle_subtract(flat, out.x);
        let off = diff.abs();
        if !rest {
            let rate = draws.land_factor(!self.falls) * f64::from(self.apos.delta.x);
            self.apos.delta.x = if off < LAND_TILT_OFF { -rate } else { rate } as f32;
        }
        let a = angle_normalize_180(out.x);
        out.x = if rest || off < LAND_FLAT_OFF {
            // Upside down lands the other way up.
            if a.abs() > 90.0 {
                angle_normalize_360(flat + 180.0)
            } else {
                angle_normalize_360(flat)
            }
        } else if off < LAND_TILT_OFF {
            angle_normalize_360((f64::from(diff) * 0.25 + f64::from(a)) as f32)
        } else {
            angle_normalize_360(a)
        };
        out
    }

    /// 1.1d's arithmetic: its server build keeps the bounce and the arc on
    /// the x87 stack where 1.5b's and UO's -O0 builds store every float
    /// variable (cod11-combat.md 12.5).
    fn kept(&self) -> bool {
        !self.falls && !self.uo
    }

    /// `G_ExplodeMissile` (13.2): the origin truncated, `eType` 0, the event
    /// with the downward trace's normal packed into its parm, and the
    /// entity kept on the wire until [`EVENT_VALID_MS`] has passed.
    fn explode(&mut self, world: Option<&CollisionWorld>, now_ms: i32) -> Explosion {
        let org = self.traj.evaluate(now_ms).trunc();
        self.origin = org;
        self.traj = Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: 0,
            tr_duration: 0,
            base: org,
            delta: Vec3::ZERO,
        };
        // 13.2 writes the surface unconditionally, so a blast in mid-air
        // takes 0 rather than whatever the last bounce left. A trace that
        // hits nothing reports a zero normal, which is the `eventParm` 0
        // retail packs for exactly that case.
        let mut normal = Vec3::ZERO;
        self.surf_type = 0;
        if let Some(world) = world {
            // `trap_Trace` on a zero box: no static models, so a frag resting
            // on a crate's mesh packs a zero normal, as retail's does.
            let down = world.point_trace(
                org,
                org - Vec3::Z * EXPLODE_TRACE_DOWN,
                vcod_common::collision::MASK_MISSILE,
                false,
            );
            normal = down.normal;
            self.surf_type = sound_material(down.surface_flags);
            if world.point_contents(org) & CONTENTS_WATER != 0 {
                self.surf_type = WATER_SURF_TYPE;
                // UO's water branch (0x5e2b5) puts the explode on the
                // surface: a water-only trace from 500 above to the
                // downward trace's end.
                let (from, to) = (
                    org + Vec3::Z * UO_WATER_TRACE_UP,
                    org - Vec3::Z * EXPLODE_TRACE_DOWN,
                );
                if self.uo
                    && let Some(f) = world.water_trace(from, to)
                {
                    let surface = from + (to - from) * f;
                    self.origin = surface;
                    self.traj.base = surface;
                }
            }
        }
        self.events
            .add(EV_GRENADE_EXPLODE, dir_to_byte(normal.into()));
        self.exploded_ms = Some(now_ms);
        self.fused = true;
        // UO's blast centre is the end of a trace 10 up (0x5e2b5).
        let org = self.origin;
        let at = match world {
            Some(w) if self.uo => {
                w.point_trace(
                    org,
                    org + Vec3::Z * UO_BLAST_LIFT,
                    vcod_common::collision::MASK_MISSILE,
                    false,
                )
                .endpos
            }
            _ => org,
        };
        // The splash is charged to `parent` (UO 0x5e2fc): a shell hung
        // after its caller left has `&g_entities[1023]` there, which walks,
        // and a vehicle's shell the vehicle itself, never its driver: only a
        // turret's `parent` is swapped for its gunner (13.7). A freed
        // vehicle swept `parent` to NULL with the owner. Its means of death
        // is the spawn's `splashMethodOfDeath` (`+0x25c`: `fire_rocket` 6,
        // `fire_artillery` 14).
        let owner = match (self.owner, self.shell.and_then(|s| s.vehicle)) {
            (None, _) if self.parent_unspawned => Some(crate::game::host::ZEROED_ENTITY),
            (Some(_), Some(vehicle)) => Some(vehicle.0 as usize),
            (owner, _) => owner,
        };
        let (mod_, direct_mod) = if self.rocket {
            ("MOD_PROJECTILE_SPLASH", "MOD_PROJECTILE")
        } else if self.shell.is_some() {
            ("MOD_ARTILLERY_SPLASH", "MOD_ARTILLERY")
        } else {
            ("MOD_GRENADE_SPLASH", "MOD_GRENADE")
        };
        Explosion {
            owner,
            inflictor: self.id,
            weapon: self.weapon,
            at,
            mod_,
            direct: None,
            direct_mod,
            concussive: !self.uo,
            struck: None,
            contact: org,
            flight: Vec3::ZERO,
            shell_damage: 0,
        }
    }

    /// `G_ExplodeSmokeGrenade` (0x5eb72): 185 with the downward trace's
    /// normal and type 1, no blast, `eType` and the flight left alone, and
    /// the free [`SMOKE_LIFE_MS`] on.
    fn pop_smoke(&mut self, world: Option<&CollisionWorld>, now_ms: i32) {
        let org = self.origin;
        let mut normal = Vec3::ZERO;
        self.surf_type = 0;
        if let Some(world) = world {
            let down = world.point_trace(
                org,
                org - Vec3::Z * EXPLODE_TRACE_DOWN,
                vcod_common::collision::MASK_MISSILE,
                false,
            );
            normal = down.normal;
            self.surf_type = sound_material(down.surface_flags);
            if world.point_contents(org) & CONTENTS_WATER != 0 {
                self.surf_type = WATER_SURF_TYPE;
            }
        }
        self.events
            .add(EV_GRENADE_EXPLODE, dir_to_byte(normal.into()));
        self.exploded_ms = Some(now_ms);
        self.free_at_ms = Some(now_ms + SMOKE_LIFE_MS);
    }

    /// `G_MissileImpact`'s explode arm (0x5db67) for a shell: the origin
    /// snapped toward its base, `eType` 0, 185 with the struck face's normal
    /// (186 off a player), and the blast 4 off the face. 1.1's (0x53ab4,
    /// combat doc 13.1) raises 179 (180 off a player) and blasts at the
    /// contact itself.
    fn impact(&mut self, world: Option<&CollisionWorld>, c: Contact, now_ms: i32) -> Explosion {
        // `BG_EvaluateTrajectoryDelta` at the contact, ahead of the stop.
        let mut flight = self.traj.delta;
        if self.traj.tr_type == vcod_common::net::trajectory::TR_GRAVITY {
            flight.z -= vcod_common::net::trajectory::DEFAULT_GRAVITY
                * (now_ms - self.traj.tr_time) as f32
                * 0.001;
        }
        let base = self.traj.base;
        let org = Vec3::from_array([0, 1, 2].map(|i| snap_towards(c.at[i], base[i])));
        self.origin = org;
        self.traj = Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: 0,
            tr_duration: 0,
            base: org,
            delta: Vec3::ZERO,
        };
        self.surf_type = c.material;
        let mut at = if self.uo {
            c.at + c.normal * UO_IMPACT_LIFT
        } else {
            c.at
        };
        if let Some(world) = world
            && self.uo
        {
            if world.point_contents(c.at) & CONTENTS_WATER != 0 {
                self.surf_type = WATER_SURF_TYPE;
            }
            at = world
                .point_trace(c.at, at, vcod_common::collision::MASK_MISSILE, false)
                .endpos;
        }
        // 186 off a player, or off anything the trace names a bone of
        // (`trace+42`, 0x5dfa6): a vehicle's hull.
        let event = if c.player.is_some() || c.model {
            vcod_common::net::event_ids::EV_ROCKET_EXPLODE_NOMARKS
        } else if self.uo {
            EV_GRENADE_EXPLODE
        } else {
            vcod_common::net::event_ids::EV_ROCKET_EXPLODE
        };
        self.events.add(event, dir_to_byte(c.normal.into()));
        self.exploded_ms = Some(now_ms);
        let damage = self.shell.map_or(0, |s| s.damage);
        let vehicle = self.shell.and_then(|s| s.vehicle);
        let (mod_, direct_mod) = if self.rocket {
            ("MOD_PROJECTILE_SPLASH", "MOD_PROJECTILE")
        } else {
            ("MOD_ARTILLERY_SPLASH", "MOD_ARTILLERY")
        };
        Explosion {
            owner: self.owner,
            inflictor: vehicle.unwrap_or(self.id),
            weapon: self.weapon,
            at,
            mod_,
            direct: c.player.map(|p| (p, damage)),
            direct_mod,
            concussive: false,
            struck: Some(c.entity),
            contact: c.at,
            flight,
            shell_damage: damage,
        }
    }

    /// The entity state one client is sent. `index` is left at 0: retail's
    /// `fire_grenade` writes no model index and every `!missile` line in the
    /// capture reads `index=0`, the client drawing the projectile off
    /// `s.weapon` instead. The owner rides `r.ownerNum` and `parent`, which
    /// are server-side, so no client-facing field carries it.
    fn to_entity(&self, p: &Protocol) -> EntityState {
        let mut e = EntityState::null(p);
        e.number = self.id.0;
        let mut set = |name: &str, v: i32| {
            if let Some(i) = EntityState::field_index(p, name) {
                e.fields[i] = v;
            }
        };
        let exploded = self.exploded_ms.is_some();
        // A popped smoke grenade stays a missile; an impact sets no explode
        // flag, the fuse does, a shell's or a rocket's too (UO 0x5e435,
        // 1.5b 0x547dc; combat doc 13.7).
        let general = exploded && self.free_at_ms.is_none();
        set("eType", if general { ET_GENERAL } else { ET_MISSILE });
        let flag = general && self.fused;
        set(
            "eFlags",
            self.eflags | if flag { EFLAGS_EXPLODED } else { 0 },
        );
        set("weapon", i32::from(self.weapon));
        set("surfType", self.surf_type);
        if exploded && self.uo {
            set("_union.scale", self.explosion_type);
        }
        set("groundEntityNum", self.ground);
        self.events.write(&mut set);
        self.traj.write(&mut e, p, "pos");
        self.apos.write(&mut e, p, "apos");
        e
    }
}

/// What [`Missile::full_trace`] struck: the trace, and the body or the
/// model it stopped on over the world.
struct MissileHit {
    tr: vcod_common::collision::Trace,
    player: Option<usize>,
    model: Option<u32>,
    model_takes_damage: bool,
    /// The model struck carries `CONTENTS_BODY` (`PlacedModel::body`).
    model_body: bool,
}

/// `SnapVectorTowards` on one axis: `floor` when `to` is not above, else
/// `ceil` (1.1 0x692d8 under the x87 round-down and round-up modes, 1.5b
/// 0x6dd91 and UO 0x7a745 through `floor` and `ceil`). Not Q3's truncate
/// and add one, which lands a unit off on a negative axis (combat doc 13.1).
fn snap_towards(v: f32, to: f32) -> f32 {
    if to <= v { v.floor() } else { v.ceil() }
}

/// UO's `fire_grenade` gives a `takeDamage` grenade a box only when its
/// fuse runs past this, and marks it with `eFlags` 0x10.
const SHOOTABLE_FUSE_MS: i32 = 500;
const SHOOTABLE_EFLAGS: i32 = 0x10;

/// `fire_artillery` adds 1000 to the point it is handed.
const SHELL_LIFT: f32 = 1000.0;
/// `fire_artillery`'s `eFlags |= 0x8000`, in UO's own layout.
const SHELL_EFLAGS_V22: i32 = 0x8000;

/// `G_DropArtillery` backs the fall's `trTime` one frame off the drop.
const FRAME_BEHIND_MS: i32 = 50;
/// `G_DropArtillery`'s `G_ExplodeMissile` think past the drop (0x5eadc).
const SHELL_FALLBACK_MS: i32 = 15_000;
/// A rocket's `G_ExplodeMissile` think after the launch: UO's 15 s, 1.1's
/// and 1.5's 30 s (combat doc 13.6).
const ROCKET_LIFE_MS: i32 = 15_000;
const ROCKET_LIFE_MS_V1: i32 = 30_000;
/// 1.1's `fire_rocket` `eFlags |= 0x10000` (0x54683), kept through the
/// explode.
const ROCKET_EFLAGS_V1: i32 = 0x10000;

impl Missiles {
    /// `G_FreeEntity`'s ground sweep over the missiles
    /// (`GameHost::free_ground_refs`).
    pub fn free_ground_refs(&mut self, num: i32) {
        for m in &mut self.live {
            if m.ground == num {
                m.ground = crate::game::host::ENTITYNUM_NONE;
            }
        }
    }

    /// `G_FreeEntity`'s owner sweep for a client that left (1.1d
    /// 0x66add..0x66af7, 1.5b and UO `G_FreeEntityRefs`; combat doc 13.7):
    /// `parent` NULL and `r.ownerNum` 0x3ff on whatever he threw or fired,
    /// so its traces stop passing his slot and its blast charges nobody. A
    /// UO vehicle's shell is owned by the vehicle, not the driver, and keeps
    /// him.
    pub fn free_owner_refs(&mut self, slot: usize) {
        for m in &mut self.live {
            if m.owner == Some(slot) && m.shell.is_none_or(|s| s.vehicle.is_none()) {
                m.owner = None;
            }
        }
    }

    /// The same sweep for a freed UO vehicle: its shells' `r.ownerNum` (the
    /// vehicle) goes to 0x3ff and `parent` to NULL, so they charge nobody.
    /// The driver they keep in `+0x330` is read only through a live owner
    /// (`G_MissileImpact`, game.mp.uo 0x5dc8e..0x5dd33; combat doc 13.7).
    pub fn free_vehicle_refs(&mut self, vehicle: EntId) {
        for m in &mut self.live {
            if m.shell.is_some_and(|s| s.vehicle == Some(vehicle)) {
                m.owner = None;
            }
        }
    }

    /// `G_RunMissile` ends in `G_RunThink` (1.1d 0x54050, 0x54398, UO's
    /// alike), and a missile's fuse is its think: a `G_FreeEntity` think set
    /// on its entity (a script `delete`, the 1.1 drop ring's eviction)
    /// replaces the fuse, and the missile flies on until `at_ms`, then goes
    /// unexploded (combat doc 13.8). A popped smoke grenade's own free is
    /// such a think too; one that exploded rides out its event.
    pub fn schedule_free(&mut self, id: EntId, at_ms: i32) {
        if let Some(m) = self
            .live
            .iter_mut()
            .find(|m| m.id == id && (m.exploded_ms.is_none() || m.free_at_ms.is_some()))
        {
            m.free_at_ms = Some(at_ms);
        }
    }

    /// `r.ownerNum` of a live missile: `Some(None)` once nobody owns it.
    pub fn owner_of(&self, id: EntId) -> Option<Option<usize>> {
        self.live.iter().find(|m| m.id == id).map(|m| m.owner)
    }

    /// A throw's `trDelta`: `velocity` truncated per axis, plus the
    /// thrower's `carried` velocity along it. 1.1d's `FireWeapon`
    /// (0x6900c-0x6909c) normalizes `velocity` in place and keeps the dot
    /// and each product on the x87 stack; 1.5b's and UO's
    /// `weapon_grenadelauncher_fire` (UO 0x7b2c4) store the dot.
    fn carry(&self, velocity: Vec3, carried: Vec3) -> Vec3 {
        let delta = velocity.trunc();
        if carried == Vec3::ZERO {
            return delta;
        }
        let stored = self.falls;
        let n = if stored {
            x87::normalize_stored(velocity)
        } else {
            x87::normalize_in_place(velocity)
        };
        let dot = x87::dot(n, carried);
        let dot = if stored { f64::from(dot as f32) } else { dot };
        (delta.as_dvec3() + n.as_dvec3() * dot).as_vec3()
    }

    /// `fire_grenade` (11): the entity, the two trajectories and the fuse.
    /// `velocity` is the caller's, unrounded: `trDelta` takes it truncated
    /// and the launch angles take it as it is (11.2). `carried` is the
    /// thrower's own velocity, whose part along the throw `FireWeapon`
    /// adds to `trDelta` once the spawn returns (11.3); zero for a drop.
    /// `now_ms` is the `level.time` the throw ran at, which is the frame
    /// *before* the one the missile first moves on: retail executes a
    /// usercmd in `SV_ClientThink`, between frames, so the first
    /// `G_RunMissile` after a throw is a full frame of flight (the capture's
    /// first `!missile` line is already past its first bounce).
    #[allow(clippy::too_many_arguments)]
    pub fn fire_grenade(
        &mut self,
        ents: &mut ObjectTable,
        hold: &mut vcod_common::rng::HoldRand,
        cx: &mut Cx,
        model: i32,
        owner: usize,
        weapon: u8,
        origin: Vec3,
        velocity: Vec3,
        carried: Vec3,
        fuse_left_ms: i32,
        now_ms: i32,
    ) -> Result<EntId, ErrorKind> {
        let id = ents.spawn(cx)?;
        // The classname a script's `getEntArray` looks for. `crate::game::wire`
        // puts nothing on the wire for it, which is what keeps the entity
        // from being sent twice.
        if let crate::game::fields::Route::Engine { slot, .. } =
            crate::game::fields::route_entity("classname")
            && let Some(e) = ents.get_mut(id)
        {
            e.engine[slot] = Value::String(cx.intern_exact("grenade"));
        }
        set_script_origin(ents, id, origin);
        let mut angles = vectoangles(velocity);
        angles.x = vcod_common::pmove::aim::angle_normalize_360(angles.x - LAUNCH_PITCH_LEAD);
        let delta = self.carry(velocity, carried);
        let tumble = Vec3::new(
            self.draws.tumble(hold, TUMBLE_PITCH),
            0.0,
            self.draws.tumble(hold, TUMBLE_ROLL),
        );
        let fuse = if fuse_left_ms > 0 {
            fuse_left_ms
        } else {
            DEFAULT_FUSE_MS
        };
        self.live.push(Missile {
            id,
            owner: Some(owner),
            parent_unspawned: false,
            weapon,
            model,
            traj: Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now_ms,
                tr_duration: 0,
                base: origin,
                delta,
            },
            apos: Trajectory {
                tr_type: TR_LINEAR,
                tr_time: now_ms,
                tr_duration: 0,
                base: angles,
                delta: tumble,
            },
            explode_at_ms: now_ms + fuse,
            origin,
            events: EventRing::default(),
            last_run_ms: now_ms,
            surf_type: 0,
            ground: ENTITYNUM_NONE as i32,
            exploded_ms: None,
            fused: false,
            uo: self.uo,
            falls: self.falls,
            explosion_type: 0,
            eflags: 0,
            shell: None,
            rocket: false,
            free_at_ms: None,
            shootable: None,
            dropped: false,
            splash: None,
        });
        Ok(id)
    }

    /// Where a grenade [`Self::fire_grenade`] would throw from `origin` at
    /// `velocity` (the thrower moving at `carried`) stands after each frame
    /// of its flight up to `fuse_ms`, flown by [`Missile::step`] against
    /// the world alone: no bodies, no script models, no launch tumble.
    /// Cut short where its move ends on the sky. A bot's lob probe reads
    /// the burst off it (`Server::lob_probe`).
    pub fn predict_flight(
        &self,
        world: &CollisionWorld,
        weapon: u8,
        origin: Vec3,
        velocity: Vec3,
        carried: Vec3,
        fuse_ms: i32,
    ) -> Vec<Vec3> {
        let mut m = Missile {
            id: EntId(0, 0),
            owner: None,
            parent_unspawned: false,
            weapon,
            model: 0,
            traj: Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: 0,
                tr_duration: 0,
                base: origin,
                delta: self.carry(velocity, carried),
            },
            apos: Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base: Vec3::ZERO,
                delta: Vec3::ZERO,
            },
            explode_at_ms: fuse_ms,
            origin,
            events: EventRing::default(),
            last_run_ms: 0,
            surf_type: 0,
            ground: ENTITYNUM_NONE as i32,
            exploded_ms: None,
            fused: false,
            uo: self.uo,
            falls: self.falls,
            explosion_type: 0,
            eflags: 0,
            shell: None,
            rocket: false,
            free_at_ms: None,
            shootable: None,
            dropped: false,
            splash: None,
        };
        let mut draws = Draws::default();
        let mut at = Vec::new();
        let mut now = 0;
        while now < fuse_ms {
            now += crate::server::FRAME_MS;
            if m.step(Some(world), &[], &[], None, &mut draws, now) == Stepped::Freed {
                break;
            }
            at.push(m.origin);
        }
        at
    }

    /// Sets what UO's `fire_grenade` (0x5f7ad) reads off the weapon file
    /// for a missile just armed: `projExplosionType`'s index (type 1 arms
    /// `G_ExplodeSmokeGrenade`) and, for a `takeDamage` grenade whose fuse
    /// runs past 500 ms, the shootable box, its `eFlags` 0x10 and the
    /// leave-owner think 500 ms on. Returns whether it is shootable.
    pub fn set_uo_kind(&mut self, id: EntId, kind: i32, take_damage: bool, now_ms: i32) -> bool {
        let Some(m) = self.live.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        m.explosion_type = kind;
        if !(take_damage && m.explode_at_ms > now_ms + SHOOTABLE_FUSE_MS) {
            return false;
        }
        m.eflags |= SHOOTABLE_EFLAGS;
        m.shootable = Some(Shootable {
            takedamage: true,
            owner_passes: true,
            leave_owner: true,
            fuse_at_ms: m.explode_at_ms,
        });
        m.explode_at_ms = now_ms + SHOOTABLE_FUSE_MS;
        true
    }

    /// `G_Damage` on a shootable grenade that still takes it: its 1 health
    /// goes, and `G_GrenadeDie` (0x5f683) brings its think 100 ms on at
    /// `level_ms` and clears `takedamage`. Whether `id` took it.
    pub fn kill(&mut self, id: EntId, level_ms: i32) -> bool {
        let Some(m) = self.live.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        match m.shootable.as_mut() {
            Some(s) if s.takedamage && m.exploded_ms.is_none() => {
                s.takedamage = false;
                m.explode_at_ms = level_ms + GRENADE_DIE_MS;
                // The think is one slot: the explode replaces a pending free.
                m.free_at_ms = None;
                true
            }
            _ => false,
        }
    }

    /// The live shootable grenades a round can stop on: entity, box at its
    /// origin, and the thrower its traces still pass (Q3's
    /// `SV_ClipMoveToEntities` skips an entity the pass entity owns).
    pub fn shot_targets(&self) -> Vec<crate::game::combat::ShotTarget> {
        self.live
            .iter()
            .filter(|m| m.exploded_ms.is_none())
            .filter_map(|m| {
                let s = m.shootable?;
                Some(crate::game::combat::ShotTarget {
                    id: m.id,
                    lo: m.origin + SHOOTABLE_MINS,
                    hi: m.origin + SHOOTABLE_MAXS,
                    owner: m.owner.filter(|_| s.owner_passes),
                })
            })
            .collect()
    }

    /// The shootable grenades a blast can damage: entity and origin.
    pub fn blast_targets(&self) -> Vec<(EntId, Vec3)> {
        self.live
            .iter()
            .filter(|m| m.exploded_ms.is_none() && m.shootable.is_some_and(|s| s.takedamage))
            .map(|m| (m.id, m.origin))
            .collect()
    }

    /// A shootable grenade's link: its origin, while it is in the pool.
    pub fn shootable_origin(&self, id: EntId) -> Option<Vec3> {
        self.live
            .iter()
            .find(|m| m.id == id && m.shootable.is_some())
            .map(|m| m.origin)
    }

    /// `fire_artillery` (0x5ff4a): a shell hanging 1000 above `at`, sent to
    /// nobody, dropped `delay_ms` on at `shell.speed` straight down. `at` is
    /// already the barrage's 1000 up from the strike point. `owner` is the
    /// barrage's: `None` when its caller left, and the shell then charges
    /// nobody on contact (combat doc 13.7).
    #[allow(clippy::too_many_arguments)]
    pub fn fire_artillery(
        &mut self,
        ents: &mut ObjectTable,
        cx: &mut Cx,
        owner: Option<usize>,
        weapon: u8,
        at: Vec3,
        shell: Shell,
        explosion_type: i32,
        now_ms: i32,
    ) -> Result<EntId, ErrorKind> {
        let id = ents.spawn(cx)?;
        if let crate::game::fields::Route::Engine { slot, .. } =
            crate::game::fields::route_entity("classname")
            && let Some(e) = ents.get_mut(id)
        {
            e.engine[slot] = Value::String(cx.intern_exact("rocket"));
        }
        let origin = at + Vec3::Z * SHELL_LIFT;
        set_script_origin(ents, id, origin);
        let stationary = |base| Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: 0,
            tr_duration: 0,
            base,
            delta: Vec3::ZERO,
        };
        self.live.push(Missile {
            id,
            owner,
            parent_unspawned: owner.is_none(),
            weapon,
            model: 0,
            traj: Trajectory {
                delta: Vec3::new(0.0, 0.0, -shell.speed),
                ..stationary(origin)
            },
            // `vectoangles` of straight down.
            apos: stationary(Vec3::new(90.0, 0.0, 0.0)),
            // `G_DropArtillery` arms `G_ExplodeMissile` 15 s past the drop.
            explode_at_ms: i32::MAX,
            origin,
            events: EventRing::default(),
            last_run_ms: now_ms,
            surf_type: 0,
            ground: ENTITYNUM_WORLD as i32,
            exploded_ms: None,
            fused: false,
            uo: true,
            falls: true,
            explosion_type,
            eflags: vcod_common::net::flags::eflags_v22_to_v1(SHELL_EFLAGS_V22),
            shell: Some(shell),
            rocket: false,
            free_at_ms: None,
            shootable: None,
            dropped: false,
            splash: None,
        });
        Ok(id)
    }

    /// `fire_rocket` (UO 0x5fc74): a `rocket` from `start` along `dir` at
    /// the weapon's `projectileSpeed`, the speed truncated per axis, on
    /// `trType` 2 a frame behind, exploding on contact with
    /// `projExplosionType` or 15 s on. With `arc`, as `Weapon_Artillery_Fire`
    /// (0x7b594) calls it for a UO vehicle's main gun, it is rewritten to
    /// `trType` 5 (z falls 400 t²); `owner` is the driver, credited through
    /// the vehicle (docs/research/coduo-vehicles.md 13). A player's
    /// launcher (`Weapon_RocketLauncher_Fire`, 0x7b3da) keeps the line.
    /// 1.1's (0x54614, combat doc 13.6) flies straight for 30 s.
    #[allow(clippy::too_many_arguments)]
    pub fn fire_rocket(
        &mut self,
        ents: &mut ObjectTable,
        cx: &mut Cx,
        owner: usize,
        weapon: u8,
        start: Vec3,
        dir: Vec3,
        shell: Shell,
        explosion_type: i32,
        arc: bool,
        now_ms: i32,
    ) -> Result<EntId, ErrorKind> {
        let id = ents.spawn(cx)?;
        if let crate::game::fields::Route::Engine { slot, .. } =
            crate::game::fields::route_entity("classname")
            && let Some(e) = ents.get_mut(id)
        {
            e.engine[slot] = Value::String(cx.intern_exact("rocket"));
        }
        set_script_origin(ents, id, start);
        let dir = rocket_launch_dir(dir, self.falls);
        let delta = self
            .rocket_deltas
            .pop_front()
            .unwrap_or_else(|| (dir * shell.speed).trunc());
        let (tr_type, life_ms, eflags) = if self.uo {
            (
                if arc { TR_GRAVITY } else { TR_LINEAR },
                ROCKET_LIFE_MS,
                vcod_common::net::flags::eflags_v22_to_v1(SHELL_EFLAGS_V22),
            )
        } else {
            (TR_LINEAR, ROCKET_LIFE_MS_V1, ROCKET_EFLAGS_V1)
        };
        self.live.push(Missile {
            id,
            owner: Some(owner),
            parent_unspawned: false,
            weapon,
            model: 0,
            traj: Trajectory {
                tr_type,
                tr_time: now_ms - FRAME_BEHIND_MS,
                tr_duration: 0,
                base: start,
                delta,
            },
            apos: Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base: vectoangles(delta),
                delta: Vec3::ZERO,
            },
            explode_at_ms: now_ms + life_ms,
            origin: start,
            events: EventRing::default(),
            last_run_ms: now_ms,
            surf_type: 0,
            ground: ENTITYNUM_NONE as i32,
            exploded_ms: None,
            fused: false,
            uo: self.uo,
            falls: self.falls,
            explosion_type,
            eflags,
            shell: Some(Shell {
                drop_at_ms: None,
                ..shell
            }),
            rocket: true,
            free_at_ms: None,
            shootable: None,
            dropped: false,
            splash: None,
        });
        Ok(id)
    }

    /// One frame of every missile: the move, then its own fuse, then the
    /// exploded ones that have ridden out their event.
    pub fn run(
        &mut self,
        world: Option<&CollisionWorld>,
        sims: &[(usize, &ClientSim)],
        models: &[crate::game::combat::PlacedModel],
        mut bones: Option<&mut crate::game::combat::BoneTraceCtx>,
        now_ms: i32,
    ) -> MissileFrame {
        let mut frame = MissileFrame {
            exploded: Vec::new(),
            temp: Vec::new(),
            freed: Vec::new(),
        };
        let ids: Vec<EntId> = self.live.iter().map(|m| m.id).collect();
        for id in ids {
            match self.run_one(id, world, sims, models, bones.as_deref_mut(), now_ms) {
                Some(Turn::Exploded(x)) => frame.exploded.push(x),
                Some(Turn::Freed) => frame.freed.push(id),
                _ => {}
            }
        }
        frame.temp = self.take_splashes();
        frame
    }

    /// A thrown grenade's `r.currentOrigin` and `s.weapon`: the missiles
    /// whose `methodOfDeath` is `MOD_GRENADE`, which `G_RunMissile` walks
    /// for grenade-touch triggers. `None` for a shell or a rocket.
    pub fn grenade_at(&self, id: EntId) -> Option<(Vec3, u8)> {
        self.live
            .iter()
            .find(|m| m.id == id && !m.rocket && m.shell.is_none() && m.exploded_ms.is_none())
            .map(|m| (m.origin, m.weapon))
    }

    /// `G_RunEntity`'s missile arm for one entity (`eType` 4, 0x50375), on
    /// its turn in `G_RunFrame`'s entity loop. `None` when `id` is no live
    /// missile.
    pub fn run_one(
        &mut self,
        id: EntId,
        world: Option<&CollisionWorld>,
        sims: &[(usize, &ClientSim)],
        models: &[crate::game::combat::PlacedModel],
        bones: Option<&mut crate::game::combat::BoneTraceCtx>,
        now_ms: i32,
    ) -> Option<Turn> {
        let i = self.live.iter().position(|m| m.id == id)?;
        let m = &mut self.live[i];
        if let Some(free_at) = m.free_at_ms {
            // A popped smoke grenade, or one whose fuse a free replaced
            // ([`Missiles::schedule_free`]), flies on; `G_FreeEntity` is its
            // think.
            let stepped = m.step(world, sims, models, bones, &mut self.draws, now_ms);
            self.splashes.extend(m.splash.take());
            if let Stepped::Struck(c) = stepped {
                // A shell still goes off on contact and rides out its event.
                m.free_at_ms = None;
                return Some(Turn::Exploded(m.impact(world, c, now_ms)));
            }
            if now_ms >= free_at || stepped == Stepped::Freed {
                self.live.remove(i);
                return Some(Turn::Freed);
            }
            return Some(Turn::Flew);
        }
        if let Some(at) = m.exploded_ms {
            // `freeAfterEvent`: the entity rides the wire until its event
            // has aged out, and is freed with it.
            if now_ms.wrapping_sub(at) > EVENT_VALID_MS {
                self.live.remove(i);
                return Some(Turn::Freed);
            }
            return Some(Turn::Flew);
        }
        let stepped = m.step(world, sims, models, bones, &mut self.draws, now_ms);
        self.splashes.extend(m.splash.take());
        let m = &mut self.live[i];
        match stepped {
            Stepped::Struck(c) => return Some(Turn::Exploded(m.impact(world, c, now_ms))),
            Stepped::Freed => {
                self.live.remove(i);
                return Some(Turn::Freed);
            }
            _ => {}
        }
        if now_ms >= m.explode_at_ms {
            if m.shootable.is_some_and(|s| s.leave_owner) {
                m.leave_owner(sims, now_ms);
                return Some(Turn::Flew);
            }
            if m.uo && m.explosion_type == SMOKE {
                m.pop_smoke(world, now_ms);
                return Some(Turn::Flew);
            }
            return Some(Turn::Exploded(m.explode(world, now_ms)));
        }
        Some(Turn::Flew)
    }

    /// `G_MissileImpact` on a vehicle (game.mp.uo 0x6db67): the explode
    /// carries UO's `eFlags` 0x100000, which turns the client to the
    /// impact row's `vehicle` column (coduo-diff.md 19).
    pub fn struck_vehicle(&mut self, id: EntId) {
        if let Some(m) = self.live.iter_mut().find(|m| m.id == id) {
            m.eflags |= vcod_common::net::flags::EF_IN_VEHICLE_V22;
        }
    }

    /// The water entry splashes the turns since the last call raised, in
    /// order: `G_TempEntity`s the caller spawns on the missile's turn.
    pub fn take_splashes(&mut self) -> Vec<crate::game::temp_entity::TempEntity> {
        std::mem::take(&mut self.splashes)
    }

    /// The grenades still to go off, as `(origin, weapon, owner, dropped)`
    /// ([`Missile::dropped`]): what a bot runs from
    /// (`crate::bots::BotView::frag_near`). Shells and the ones already
    /// gone off are left out.
    pub fn armed(&self) -> impl Iterator<Item = (Vec3, u8, Option<usize>, bool)> + '_ {
        self.live
            .iter()
            .filter(|m| m.shell.is_none() && m.exploded_ms.is_none())
            .map(|m| (m.origin, m.weapon, m.owner, m.dropped))
    }

    /// Marks a grenade [`Missile::dropped`].
    pub fn mark_dropped(&mut self, id: EntId) {
        if let Some(m) = self.live.iter_mut().find(|m| m.id == id) {
            m.dropped = true;
        }
    }

    /// Whether `id` is a missile this pool runs.
    pub fn contains(&self, id: EntId) -> bool {
        self.live.iter().any(|m| m.id == id)
    }

    /// One missile's `r.currentOrigin` into its script `origin`, as
    /// [`Missiles::sync_origins`] does for all of them.
    /// A missile's `s.weapon`, gone off or not.
    pub fn weapon_of(&self, id: EntId) -> Option<u8> {
        self.live.iter().find(|m| m.id == id).map(|m| m.weapon)
    }

    /// A script `origin` write on a live missile: `Scr_SetOrigin`'s
    /// `G_SetOrigin` (1.1d 0x5c61c, UO 0x6e801) puts `trType` 0 at the
    /// point with no delta and moves `r.currentOrigin`. 1.5's and UO's
    /// `G_RunMissile` then drops it from there when nothing is under it,
    /// so a write every frame holds it (combat doc 13.7). `false` when `id`
    /// is no live missile.
    pub fn set_origin(&mut self, id: EntId, origin: Vec3) -> bool {
        let Some(m) = self.live.iter_mut().find(|m| m.id == id) else {
            return false;
        };
        m.origin = origin;
        m.traj = Trajectory {
            tr_type: TR_STATIONARY,
            tr_time: 0,
            tr_duration: 0,
            base: origin,
            delta: Vec3::ZERO,
        };
        true
    }

    /// A live missile's `r.currentOrigin`.
    pub fn origin_of(&self, id: EntId) -> Option<Vec3> {
        self.live.iter().find(|m| m.id == id).map(|m| m.origin)
    }

    pub fn sync_origin(&self, ents: &mut ObjectTable, id: EntId) {
        if let Some(m) = self.live.iter().find(|m| m.id == id) {
            set_script_origin(ents, m.id, m.origin);
        }
    }

    /// Every live missile's `r.currentOrigin` into its script `origin`, as
    /// `G_RunMissile` leaves it each frame (its stores at 0x54206 and 0x542e6
    /// are to `ent+0x13c`, the `origin` field's z).
    pub fn sync_origins(&self, ents: &mut ObjectTable) {
        for m in &self.live {
            set_script_origin(ents, m.id, m.origin);
        }
    }

    /// The live missiles, by entity number, for the snapshot build. They are
    /// `SVF_BROADCAST` (11.1), so the caller adds them past its own PVS cull.
    pub fn entities<'a>(
        &'a self,
        p: &'a Protocol,
    ) -> impl Iterator<Item = (u32, EntityState)> + 'a {
        // A hanging shell is `SVF_NOCLIENT` until its drop.
        self.live
            .iter()
            .filter(|m| m.shell.is_none_or(|s| s.drop_at_ms.is_none()))
            .map(move |m| (m.id.0, m.to_entity(p)))
    }

    #[cfg(test)]
    fn missiles(&self) -> &[Missile] {
        &self.live
    }

    #[cfg(test)]
    fn missiles_mut(&mut self) -> impl Iterator<Item = &mut Missile> {
        self.live.iter_mut()
    }

    /// Test-facing: the draws the next throws and bounces take, in order,
    /// before the pool's own state takes over. A tumble draw is the rate
    /// itself, a floor contact's the factor its pitch rate is scaled by, a
    /// wall's the whole degrees added to it ([`Missile::land_angles`]).
    pub fn pin_draws(&mut self, draws: impl IntoIterator<Item = f64>) {
        self.draws.pinned = draws.into_iter().collect();
    }

    /// Test-facing: the next rockets' `trDelta`, in order, before the
    /// launch spread takes over again: each server draws its own spread, so
    /// a gate flies retail's rocket by pinning the delta its capture shows.
    pub fn pin_rocket_deltas(&mut self, deltas: impl IntoIterator<Item = Vec3>) {
        self.rocket_deltas = deltas.into_iter().collect();
    }
}

/// Retail's `rand()` for the bounces (12.7): the pool's own state, so a
/// throw draws the same numbers whatever else the frame did, or a gate's
/// pins. The pins cover the launch tumble too, which draws `holdrand`.
pub struct Draws {
    rng: u64,
    pinned: std::collections::VecDeque<f64>,
}

impl Default for Draws {
    fn default() -> Draws {
        Draws {
            // xorshift degenerates on a zero state, so an unseeded pool
            // would bounce every grenade identically.
            rng: crate::game::host::RNG_SEED,
            pinned: std::collections::VecDeque::new(),
        }
    }
}

impl Draws {
    /// `flrand(-spread, spread) + rate` off the level's `holdrand`,
    /// `fire_grenade`'s tumble (11.2); a pin takes the value, the draw
    /// still made.
    fn tumble(&mut self, hold: &mut vcod_common::rng::HoldRand, rate: f32) -> f32 {
        let drawn = hold.flrand(-TUMBLE_SPREAD, TUMBLE_SPREAD) + rate;
        self.pinned.pop_front().map_or(drawn, |v| v as f32)
    }

    /// A floor contact's pitch rate factor (12.7): 1.1d's
    /// `0.85 - 0.3 * rand() / 2^31`, 1.5b's and UO's `0.85 + 0.3 * rand() /
    /// 2^31`, on the x87 stack.
    fn land_factor(&mut self, v11: bool) -> f64 {
        if let Some(v) = self.pinned.pop_front() {
            return v;
        }
        let r = f64::from(crate::game::host::rand_int(&mut self.rng)) / 2_147_483_648.0;
        let r = if v11 { -r } else { r };
        r * f64::from(0.3f32) + f64::from(0.85f32)
    }

    /// A wall contact's pitch rate step (12.7): `(rand() & 0x7f) - 63`.
    fn land_step(&mut self) -> f64 {
        if let Some(v) = self.pinned.pop_front() {
            return v;
        }
        f64::from((crate::game::host::rand_int(&mut self.rng) & 0x7f) - 63)
    }
}

/// What a script reads as a missile's `origin`.
fn set_script_origin(ents: &mut ObjectTable, id: EntId, origin: Vec3) {
    if let crate::game::fields::Route::Engine { slot, .. } =
        crate::game::fields::route_entity("origin")
        && let Some(e) = ents.get_mut(id)
    {
        e.engine[slot] = Value::Vector(origin.to_array());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::collision::SURFACE_CLIP_EPSILON;
    use vcod_common::net::protocol::PROTOCOL_V1;

    /// The sound-surface index of grass (`cod11-events-and-fx.md` section 4).
    const GRASS: i32 = 10;

    /// The frag as the paks ship it, for the tests that do not mount them:
    /// `projectileSpeed` 960, `projectileSpeedUp` 120, `fuseTime` 4
    /// (`vcod_common::weapon`'s own parser test).
    fn frag() -> WeaponDef {
        WeaponDef {
            projectile_speed: 960.0,
            projectile_speed_up: 120.0,
            fuse_time: 4.0,
            weapon_type: "grenade".into(),
            ..Default::default()
        }
    }

    fn armed(
        ms: &mut Missiles,
        host: &mut crate::game::host::GameHost,
        vm: &mut vcod_gsc::Vm,
        origin: Vec3,
        velocity: Vec3,
        now: i32,
    ) -> EntId {
        let (ents, hold) = (&mut host.ents, &mut host.holdrand);
        vm.with_cx(|cx| {
            ms.fire_grenade(
                ents,
                hold,
                cx,
                0,
                0,
                8,
                origin,
                velocity,
                Vec3::ZERO,
                4000,
                now,
            )
        })
        .unwrap()
    }

    /// A grenade thrown across a floor bounces, loses speed and settles a
    /// ground snap above the surface: retail's rest is `endpos.z + 1.5`
    /// (section 12.2), and `endpos` sits `SURFACE_CLIP_EPSILON` off the
    /// plane.
    ///
    /// The bounce also carries the surface it struck as its event parm and
    /// leaves `s.surfType` alone: the client formats
    /// `grenade_bounce_<surfType>` and `iw_sound.csv` has only the `_default`
    /// row, so a bounce that writes the material into `surfType` is silent
    /// (section 13.1 step 5, `cod11-sound-system.md` section 7a).
    #[test]
    fn a_grenade_bounces_and_comes_to_rest_on_a_floor() {
        // A grass floor: the material rides surface-flag bits 20..24.
        let world = vcod_common::collision::synthetic_world(
            &[(
                "textures/test/grass",
                vcod_common::collision::CONTENTS_SOLID,
                (GRASS as u32) << 20,
            )],
            &[(0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0])],
        );
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 64.0),
            Vec3::new(300.0, 0.0, 0.0),
            0,
        );
        let mut bounces = 0;
        for f in 1..=80 {
            for m in ms.missiles_mut() {
                if m.step(Some(&world), &[], &[], None, &mut Draws::default(), f * 50)
                    == Stepped::Bounced
                {
                    bounces += 1;
                }
            }
        }
        let m = &ms.missiles()[0];
        assert!(bounces >= 1, "the floor bounced it at least once");
        assert_eq!(
            m.events.parms[0], GRASS,
            "the bounce event carries the surface it struck"
        );
        assert_eq!(
            m.surf_type, 0,
            "a bounce leaves `s.surfType` alone; only the explode writes it"
        );
        assert_eq!(m.traj.tr_type, TR_STATIONARY, "it came to rest");
        let rest = SURFACE_CLIP_EPSILON + GROUND_SNAP;
        assert!(
            (m.traj.base.z - rest).abs() < 0.5,
            "rested at {} rather than {rest}",
            m.traj.base.z
        );
        assert_eq!(m.apos.tr_type, TR_STATIONARY, "the tumble stopped too");
    }

    /// A script reads a grenade's `origin` where the missile pass left it:
    /// at the muzzle on the frame it is thrown, along its flight after.
    #[test]
    fn a_grenades_script_origin_follows_its_flight() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let start = Vec3::new(0.0, 0.0, 64.0);
        let id = armed(&mut ms, &mut host, &mut vm, start, Vec3::X * 300.0, 0);
        use vcod_gsc::Host;
        let read = |host: &mut crate::game::host::GameHost, vm: &mut vcod_gsc::Vm| {
            vm.with_cx(|cx| {
                let f = cx.intern_folded("origin");
                host.get_field(cx, id, f)
            })
        };
        assert_eq!(read(&mut host, &mut vm), Value::Vector(start.to_array()));
        ms.run(None, &[], &[], None, 500);
        ms.sync_origins(&mut host.ents);
        let flown = ms.missiles()[0].origin;
        assert!(flown.x > 100.0, "it flew to {flown}");
        assert_eq!(read(&mut host, &mut vm), Value::Vector(flown.to_array()));
    }

    /// The launch tumble is drawn, not fixed: `flrand` runs off the pool's
    /// own seeded state, so two throws leave with different pitch and roll
    /// rates (11.2). An unseeded state would hand every grenade the same
    /// pair.
    #[test]
    fn two_throws_draw_different_tumbles() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        for _ in 0..2 {
            armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::X * 900.0, 0);
        }
        let (a, b) = (ms.missiles()[0].apos.delta, ms.missiles()[1].apos.delta);
        assert_ne!(a.x, b.x, "both throws tumbled at pitch {}", a.x);
        assert_ne!(a.z, b.z, "both throws tumbled at roll {}", a.z);
        for t in [a, b] {
            assert!((t.x - TUMBLE_PITCH).abs() <= TUMBLE_SPREAD, "pitch {}", t.x);
            assert!((t.z - TUMBLE_ROLL).abs() <= TUMBLE_SPREAD, "roll {}", t.z);
        }
    }

    /// The fuse is absolute: armed at `t` with `f` left, it explodes on the
    /// first frame at or past `t + f` and not before.
    #[test]
    fn the_fuse_is_absolute() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let id = armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::ZERO, 10_000);
        assert!(ms.run(None, &[], &[], None, 13_950).exploded.is_empty());
        let frame = ms.run(None, &[], &[], None, 14_000);
        assert_eq!(frame.exploded.len(), 1);
        assert_eq!(frame.exploded[0].inflictor, id);
        assert_eq!(frame.exploded[0].owner, Some(0));
        assert!(
            frame.temp.is_empty(),
            "the explode rides the missile's ring"
        );
    }

    /// `G_FreeEntity`'s owner sweep (13.7): a thrower who leaves is passed
    /// by nothing his grenade traces and charged with nothing it does; a
    /// UO vehicle's shell is the vehicle's and keeps its driver.
    #[test]
    fn a_leaver_s_grenade_loses_its_owner_and_a_vehicle_s_shell_does_not() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::ZERO, 10_000);
        let shell = Shell {
            drop_at_ms: None,
            speed: 3000.0,
            damage: 300,
            vehicle: Some(EntId(200, 0)),
        };
        let ents = &mut host.ents;
        vm.with_cx(|cx| {
            ms.fire_rocket(ents, cx, 0, 42, Vec3::Z * 40.0, Vec3::X, shell, 6, true, 0)
        })
        .unwrap();
        ms.free_owner_refs(1);
        assert_eq!(
            ms.missiles()[0].passes(),
            0,
            "another slot's leave cleared it"
        );
        ms.free_owner_refs(0);
        assert_eq!(ms.missiles()[0].owner, None);
        assert_eq!(ms.missiles()[0].passes(), usize::MAX);
        assert_eq!(ms.missiles()[1].owner, Some(0));
        let frame = ms.run(None, &[], &[], None, 14_000);
        assert_eq!(frame.exploded[0].owner, None);
    }

    /// The vehicle is the shell's `r.ownerNum`: freeing it sweeps that to
    /// 0x3ff, and `G_MissileImpact` reads the driver only through a live
    /// owner (UO 0x5dc8e), so the shell charges nobody (13.7).
    #[test]
    fn a_shell_whose_vehicle_was_freed_charges_no_one() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let tank = EntId(200, 0);
        let shell = Shell {
            drop_at_ms: None,
            speed: 3000.0,
            damage: 300,
            vehicle: Some(tank),
        };
        let ents = &mut host.ents;
        vm.with_cx(|cx| {
            ms.fire_rocket(ents, cx, 0, 42, Vec3::Z * 40.0, Vec3::X, shell, 6, true, 0)
        })
        .unwrap();
        ms.free_vehicle_refs(EntId(201, 0));
        assert_eq!(ms.missiles()[0].owner, Some(0), "another entity's free");
        ms.free_vehicle_refs(tank);
        assert_eq!(ms.missiles()[0].owner, None);
    }

    /// Section 13 and the capture: the explode flips `eType` to 0, adds 178
    /// on the missile's own ring and leaves the entity on the wire for
    /// another 300 ms. Fixture: entity 176 reads `eType=0 eFlags=256 ...
    /// events=177,177,178,0` from serverTime 225150 to 225450 and is gone at
    /// 225500.
    #[test]
    fn the_explode_rides_the_missiles_own_entity_and_is_freed_three_hundred_ms_later() {
        let p = &PROTOCOL_V1;
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let id = armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::ZERO, 0);
        ms.run(None, &[], &[], None, 4000);
        let e = ms.entities(p).next().unwrap().1;
        assert_eq!(e.field_i32(p, "eType"), ET_GENERAL);
        assert_eq!(e.field_i32(p, "eFlags"), EFLAGS_EXPLODED);
        assert_eq!(e.field_i32(p, "events[0]"), EV_GRENADE_EXPLODE);
        assert_eq!(e.field_i32(p, "eventSequence"), 1);
        assert!(
            host.ents.get(id).is_some(),
            "the entity rides its own event"
        );

        let frame = ms.run(None, &[], &[], None, 4300);
        assert!(frame.freed.is_empty(), "not aged out yet");
        assert_eq!(ms.entities(p).count(), 1, "it rides one more 300 ms");
        let frame = ms.run(None, &[], &[], None, 4350);
        assert_eq!(frame.freed, vec![id]);
        for freed in frame.freed {
            host.free_entity(freed);
        }
        assert_eq!(ms.entities(p).count(), 0, "and leaves the wire after that");
        assert!(host.ents.get(id).is_none(), "freed with it");
        // The number goes back to the pool: the next spawn takes it.
        let next = vm.with_cx(|cx| host.ents.spawn(cx)).unwrap();
        assert_eq!(next.0, id.0);
    }

    /// A bounce raises 177 only when the contact changed the velocity by
    /// more than 100 units/s (12.4's return value). A grenade rolling on a
    /// floor bounces every frame off the ground snap and is silent.
    #[test]
    fn a_soft_contact_raises_no_bounce_event() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 64.0),
            Vec3::new(300.0, 0.0, 0.0),
            0,
        );
        let mut loud = 0;
        for f in 1..=80 {
            for m in ms.missiles_mut() {
                if m.step(Some(&world), &[], &[], None, &mut Draws::default(), f * 50)
                    == Stepped::Bounced
                {
                    loud += 1;
                }
            }
        }
        let m = &ms.missiles()[0];
        assert_eq!(
            m.events.seq, loud,
            "every event on the ring is a loud bounce and no other"
        );
        assert!(
            m.events.seq < 8,
            "the roll-out is silent, {} events",
            m.events.seq
        );
    }

    /// A grenade that reaches a live player bounces off it rather than
    /// detonating: the stock frag's weapon-file `damage` is 0, and 13.1's
    /// arm for "the thing it hit takes damage but the missile does none" is
    /// `G_BounceMissile` and a return with no event at all. The damping is
    /// 12.4's soft one, an eighth of the incoming speed.
    #[test]
    fn a_live_player_bounces_a_grenade_softly_and_silently() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let target = {
            let mut sim = ClientSim::spectator([200.0, 0.0, 0.0], 180.0, [0; 3]);
            sim.become_player([200.0, 0.0, 0.0], 180.0, [0; 3]);
            sim
        };
        let sims = [(1usize, &target)];
        armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 40.0),
            Vec3::new(800.0, 0.0, 0.0),
            0,
        );
        let mut stepped = Stepped::Flew;
        for f in 1..=6 {
            stepped = ms.missiles_mut().next().unwrap().step(
                Some(&world),
                &sims,
                &[],
                None,
                &mut Draws::default(),
                f * 50,
            );
            if stepped != Stepped::Flew {
                break;
            }
        }
        assert_eq!(
            stepped,
            Stepped::Settled,
            "a player contact raises no event"
        );
        let m = &ms.missiles()[0];
        assert_eq!(m.events.seq, 0, "and nothing on the ring");
        assert!(
            m.traj.delta.x < 0.0 && m.traj.delta.length() < 800.0 * BOUNCE_SOFT * 1.2,
            "bounced back at an eighth of its speed, {}",
            m.traj.delta
        );
    }

    /// The throw's origin and velocity, against the retail capture's own
    /// numbers: standing at (1200, 1760, 144.125) with a 60-unit view height
    /// and yaw 90, the fixture's grenade leaves `pos.trBase`
    /// (1200, 1760, 204) with `trDelta` (0, 960, 120) -- the eye truncated
    /// toward zero, `forward * projectileSpeed` and `projectileSpeedUp` on z
    /// (section 11.3). Reconstructed from the first `!missile` line's bounce:
    /// `trDelta` (0, -288, 75.6) is `-0.3 * 960` and `0.75 * (120 - 800 *
    /// 0.024)`.
    #[test]
    fn the_throw_leaves_the_eye_at_the_weapon_files_speed() {
        let ps = PlayerState::spawn(Vec3::new(1200.0, 1760.0, 144.125), 90.0);
        let (origin, velocity) = throw_velocity(&ps, [0.0, 90.0], &frag());
        assert!(
            (origin - Vec3::new(1200.0, 1760.0, 204.0)).length() < 0.01,
            "{origin}"
        );
        assert!(
            (velocity - Vec3::new(0.0, 960.0, 120.0)).length() < 0.01,
            "{velocity}"
        );
    }

    /// A `setPlayerAngles` pitch of 30 up reaches the throw exact only on
    /// the first cmd after it; every later cmd's pmove has put the view on
    /// the short grid (-5461 shorts, 29.998169), and the frag leaves at
    /// `trDelta.z` 599, not 600. Retail UO does the same (combat doc 12.5).
    #[test]
    fn a_held_pitch_of_30_throws_600_on_the_first_cmd_and_599_after() {
        let ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 45.0);
        let delta = |aim| {
            let (_, v) = throw_velocity(&ps, aim, &frag());
            Missiles::default().carry(v, Vec3::ZERO)
        };
        assert_eq!(delta([-30.0, 45.0]), Vec3::new(587.0, 587.0, 600.0));
        let short = vcod_common::pmove::angle_shorts(-30.0) as i16;
        let quantized = f32::from(short) * (360.0 / 65536.0);
        assert_eq!(quantized, -29.998169);
        assert_eq!(delta([quantized, 45.0]), Vec3::new(587.0, 587.0, 599.0));
    }

    /// The thrower's own motion along the throw is added after the truncation
    /// (11.3), so a runner's grenade goes further than a stander's; the
    /// direction it is measured along is the unrounded throw's.
    #[test]
    fn the_throwers_speed_along_the_throw_is_added() {
        let velocity = Vec3::new(117.8, 117.8, 1065.4);
        let carried = Vec3::new(190.0, 0.0, 0.0);
        let delta = Missiles::default().carry(velocity, carried);
        let n = velocity.normalize();
        let want = Vec3::new(117.0, 117.0, 1065.0) + n * 190.0 * n.x;
        assert!((delta - want).length() < 0.001, "{delta} want {want}");
        assert_eq!(
            Missiles::default().carry(velocity, Vec3::ZERO),
            Vec3::new(117.0, 117.0, 1065.0)
        );
    }

    /// 1.5's and UO's fall test: a frag resting on a player who has left
    /// takes `TR_GRAVITY` from where it lies on its next run, its ground
    /// still his number, and falls from the run after; on the world it
    /// stays, and 1.1 has no test at all (combat doc 12.6).
    #[test]
    fn a_frag_left_hanging_falls_on_15_and_uo_only() {
        let world = vcod_common::collision::test_world(&[]);
        for (falls, ground, drops) in [
            (true, 0, true),
            (true, ENTITYNUM_WORLD as i32, false),
            (false, 0, false),
        ] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                falls,
                ..Missiles::default()
            };
            let at = Vec3::new(0.0, 0.0, 200.0);
            armed(&mut ms, &mut host, &mut vm, at, Vec3::ZERO, 0);
            let m = ms.missiles_mut().next().unwrap();
            m.traj = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base: at,
                delta: Vec3::ZERO,
            };
            m.ground = ground;
            m.step(Some(&world), &[], &[], None, &mut Draws::default(), 1000);
            let want = if drops { TR_GRAVITY } else { TR_STATIONARY };
            assert_eq!(m.traj.tr_type, want, "falls {falls} ground {ground}");
            assert_eq!((m.origin, m.ground), (at, ground));
            m.step(Some(&world), &[], &[], None, &mut Draws::default(), 1050);
            assert_eq!(m.origin.z < at.z, drops, "falls {falls} ground {ground}");
        }
    }

    /// A frag that lands on a script model's top: 1.1's ground snap lifts
    /// it 1.5 onto the model, as it does onto a player's bones; 1.5's and
    /// UO's snap only onto the world, so it stays where it struck (combat
    /// doc 12.2).
    #[test]
    fn the_15_ground_snap_takes_the_world_only() {
        let world = vcod_common::collision::test_world(&[]);
        let (a, b, c, d) = (
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, -64.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(-64.0, 64.0, 0.0),
        );
        let face = |tri| vcod_common::collision::ModelTri {
            tri,
            contents: 1,
            surface_flags: 0,
        };
        let top = crate::game::combat::PlacedModel {
            id: EntId(200, 0),
            origin: Vec3::new(0.0, 0.0, 100.0),
            axis: glam::Mat3::IDENTITY,
            tris: std::sync::Arc::from(vec![face([a, b, c]), face([a, c, d])]),
            surfs: Default::default(),
            owner: None,
            takes_damage: false,
            body: false,
            bounds: None,
        };
        for falls in [false, true] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                falls,
                ..Missiles::default()
            };
            let at = Vec3::new(0.0, 0.0, 110.0);
            armed(
                &mut ms,
                &mut host,
                &mut vm,
                at,
                Vec3::new(0.0, 0.0, -300.0),
                1000,
            );
            let m = ms.missiles_mut().next().unwrap();
            let models = std::slice::from_ref(&top);
            m.step(Some(&world), &[], models, None, &mut Draws::default(), 1050);
            assert_eq!(m.ground, 200, "falls {falls}: {}", m.origin);
            // The clip leaves the strike an eighth off the face.
            let lift = if falls { 0.0 } else { GROUND_SNAP };
            assert!(
                (m.origin.z - (100.125 + lift)).abs() < 0.01,
                "falls {falls}: {}",
                m.origin
            );
        }
    }

    /// A script model whose contents carry `CONTENTS_BODY` (a UO script's
    /// `setContents`) bounces a frag off its top at UO's soft tenth, as a
    /// player does, and a plain one at the surface's 0.4 (combat doc 12.4).
    #[test]
    fn a_body_contents_model_bounces_a_uo_frag_softly() {
        let world = vcod_common::collision::test_world(&[]);
        let (a, b, c, d) = (
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, -64.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(-64.0, 64.0, 0.0),
        );
        let face = |tri| vcod_common::collision::ModelTri {
            tri,
            contents: 1,
            surface_flags: 0,
        };
        for (body, scale) in [(false, UO_BOUNCE), (true, UO_BOUNCE_WATER)] {
            let top = crate::game::combat::PlacedModel {
                id: EntId(200, 0),
                origin: Vec3::new(0.0, 0.0, 100.0),
                axis: glam::Mat3::IDENTITY,
                tris: std::sync::Arc::from(vec![face([a, b, c]), face([a, c, d])]),
                surfs: Default::default(),
                owner: None,
                takes_damage: false,
                body,
                bounds: None,
            };
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                uo: true,
                falls: true,
                ..Missiles::default()
            };
            armed(
                &mut ms,
                &mut host,
                &mut vm,
                Vec3::new(0.0, 0.0, 110.0),
                Vec3::new(300.0, 0.0, -300.0),
                1000,
            );
            let m = ms.missiles_mut().next().unwrap();
            m.step(
                Some(&world),
                &[],
                std::slice::from_ref(&top),
                None,
                &mut Draws::default(),
                1050,
            );
            let d = m.traj.delta;
            assert!((d.x - 300.0 * scale).abs() < 0.5, "body {body}: {d}");
        }
    }

    /// UO's bounce off a floor keeps 0.4 of the whole reflected velocity,
    /// tangential and normal alike (game.mp.uo 0x5d6e5), where 1.1's keeps
    /// 0.75 along the floor and 0.3 off it.
    #[test]
    fn a_uo_bounce_keeps_four_tenths_of_the_reflection() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(300.0, 0.0, -400.0),
            0,
        );
        let m = ms.missiles_mut().next().unwrap();
        let mut f = 1;
        while m.step(Some(&world), &[], &[], None, &mut Draws::default(), f * 50) == Stepped::Flew {
            f += 1;
        }
        let d = m.traj.delta;
        assert!((d.x - 120.0).abs() < 0.5, "along the floor: {d}");
        assert!(d.z > 0.0 && d.z < 0.4 * 450.0, "off the floor: {d}");
    }

    /// 1.5's bounce off a floor keeps half the whole reflected velocity
    /// (1.5b 0x53d71), tangential and normal alike.
    #[test]
    fn a_15_bounce_keeps_half_the_reflection() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            falls: true,
            ..Missiles::default()
        };
        armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(300.0, 0.0, -400.0),
            0,
        );
        let m = ms.missiles_mut().next().unwrap();
        let mut f = 1;
        while m.step(Some(&world), &[], &[], None, &mut Draws::default(), f * 50) == Stepped::Flew {
            f += 1;
        }
        let d = m.traj.delta;
        assert!((d.x - 150.0).abs() < 0.5, "along the floor: {d}");
        assert!(d.z > 0.0 && d.z < 0.5 * 450.0, "off the floor: {d}");
    }

    /// A slab of sky 200..210 over the test floor: the stock sky materials'
    /// contents 0x801 and surface bits 0x434.
    fn sky_world() -> CollisionWorld {
        vcod_common::collision::synthetic_world(
            &[
                (
                    "textures/test/solid",
                    vcod_common::collision::CONTENTS_SOLID,
                    0,
                ),
                ("textures/sky/test", 0x801, 0x434),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [-1024.0, -1024.0, 200.0], [1024.0, 1024.0, 210.0]),
            ],
        )
    }

    /// 12.3: a 1.1 or 1.5 missile whose move ends on the sky is freed on
    /// that turn with no event and no explode, as retail 1.1d's rocket in
    /// `client-probes/probe_skyshot` leaves the wire.
    #[test]
    fn a_missile_at_the_sky_is_freed_silently() {
        let world = sky_world();
        for falls in [false, true] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                falls,
                ..Missiles::default()
            };
            let id = armed(
                &mut ms,
                &mut host,
                &mut vm,
                Vec3::new(0.0, 0.0, 60.0),
                Vec3::new(0.0, 0.0, 900.0),
                0,
            );
            let mut freed = None;
            for f in 1..20 {
                match ms.run_one(id, Some(&world), &[], &[], None, f * 50) {
                    Some(Turn::Freed) => {
                        freed = Some(f);
                        break;
                    }
                    Some(Turn::Exploded(_)) => panic!("it went off at the sky"),
                    _ => {}
                }
                let m = ms.missiles().iter().find(|m| m.id == id).unwrap();
                assert_eq!(m.events.seq, 0, "no bounce off the sky");
            }
            assert!(freed.is_some(), "never freed");
            assert!(!ms.contains(id));
        }
    }

    /// 12.3 on UO: a sky contact is pushed 2 units into the sky, neither
    /// frees nor strikes, and the trace that then starts inside the sky
    /// brush reads clear, so the missile flies on above it (retail UO's
    /// bazooka round in `client-probes/probe_skyshot`, 2 above the face on
    /// the contact frame) and back down through it.
    #[test]
    fn a_uo_missile_flies_through_the_sky() {
        let world = sky_world();
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let id = armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 60.0),
            Vec3::new(0.0, 0.0, 600.0),
            0,
        );
        let mut zs = Vec::new();
        for f in 1..40 {
            let turn = ms.run_one(id, Some(&world), &[], &[], None, f * 50);
            assert!(!matches!(turn, Some(Turn::Freed)), "freed at the sky");
            let m = ms.missiles().iter().find(|m| m.id == id).unwrap();
            zs.push(m.origin.z);
            if m.ground == ENTITYNUM_WORLD as i32 {
                break;
            }
        }
        let contact = zs
            .iter()
            .copied()
            .find(|&z| z > 200.0)
            .expect("never reached the sky");
        assert!(
            (contact - (200.0 - SURFACE_CLIP_EPSILON + UO_SKY_PUSH)).abs() < 1e-3,
            "pushed 2 into the sky: {zs:?}"
        );
        assert!(zs.iter().any(|&z| z > 210.0), "above the sky: {zs:?}");
        assert!(zs.last().unwrap() < &10.0, "back down on the floor: {zs:?}");
    }

    /// A UO smoke grenade pops on its fuse with type 1, stays a missile
    /// (`eType` 4, no explode flag), raises no blast and is freed
    /// [`SMOKE_LIFE_MS`] after the pop.
    #[test]
    fn a_uo_smoke_grenade_pops_and_is_freed_fifteen_seconds_on() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let id = armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::ZERO, 0);
        ms.set_uo_kind(id, explosion_type("smoke"), false, 0);
        let frame = ms.run(None, &[], &[], None, 4000);
        assert!(frame.exploded.is_empty(), "no blast");
        let e = ms.entities(p).next().unwrap().1;
        assert_eq!(e.field_i32(p, "eType"), ET_MISSILE);
        assert_eq!(e.field_i32(p, "eFlags"), 0);
        assert_eq!(e.field_i32(p, "_union.scale"), 1);
        assert_eq!(e.field_i32(p, "events[0]"), EV_GRENADE_EXPLODE);
        assert!(
            ms.run(None, &[], &[], None, 4000 + SMOKE_LIFE_MS - 50)
                .freed
                .is_empty()
        );
        assert_eq!(
            ms.run(None, &[], &[], None, 4000 + SMOKE_LIFE_MS).freed,
            vec![id]
        );
    }

    /// UO's explode under water moves to the surface the water-only trace
    /// from 500 above finds (`G_ExplodeMissile` 0x5e2b5), and blasts from
    /// there; 1.1's stays where the fuse found it. Both send type 20.
    #[test]
    fn a_uo_explode_in_water_comes_up_to_the_surface() {
        use vcod_common::collision::{CONTENTS_SOLID, synthetic_world};
        let world = synthetic_world(
            &[
                ("textures/test/solid", CONTENTS_SOLID, 0),
                ("textures/test/water", CONTENTS_WATER, 0),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [-512.0, -512.0, 0.0], [512.0, 512.0, 64.0]),
            ],
        );
        for uo in [false, true] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                uo,
                ..Missiles::default()
            };
            armed(
                &mut ms,
                &mut host,
                &mut vm,
                Vec3::new(0.0, 0.0, 8.0),
                Vec3::ZERO,
                0,
            );
            let frame = ms.run(Some(&world), &[], &[], None, 4000);
            let x = &frame.exploded[0];
            let m = &ms.missiles()[0];
            assert_eq!(m.surf_type, WATER_SURF_TYPE);
            if uo {
                let top = 64.0 + SURFACE_CLIP_EPSILON;
                assert!((m.origin.z - top).abs() < 1e-3, "{}", m.origin);
                assert!((x.at.z - (top + UO_BLAST_LIFT)).abs() < 1e-3, "{}", x.at);
            } else {
                assert!(m.origin.z <= 8.0, "1.1 stays under: {}", m.origin);
            }
        }
    }

    /// A pool of water 64 deep on a floor, its material water's surface
    /// type, for 12.1's splash.
    fn pool() -> CollisionWorld {
        use vcod_common::collision::{CONTENTS_SOLID, synthetic_world};
        synthetic_world(
            &[
                ("textures/test/solid", CONTENTS_SOLID, 0),
                (
                    "textures/test/water",
                    CONTENTS_WATER,
                    SURF_TYPE_MASK & 0x140_0000,
                ),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [-512.0, -512.0, 0.0], [512.0, 512.0, 64.0]),
            ],
        )
    }

    /// 12.1: a missile whose `trDelta` z passes 30 and that starts out of
    /// water raises `EV_BULLET_HIT_SMALL` on the move that enters it, at the
    /// origin before that move, then flies on as if the water were not
    /// there; one thrown slower does not, and neither does the next move,
    /// which starts in the water.
    #[test]
    fn a_missile_entering_water_fast_splashes() {
        let world = pool();
        for (vz, splashes) in [(-600.0, true), (-20.0, false)] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles::default();
            let start = Vec3::new(0.0, 0.0, 100.0);
            let id = armed(
                &mut ms,
                &mut host,
                &mut vm,
                start,
                Vec3::new(0.0, 100.0, vz),
                0,
            );
            let mut temps = Vec::new();
            let mut before = start;
            let mut entered = None;
            for f in 1..20 {
                let was = ms.missiles()[0].origin;
                let frame = ms.run(Some(&world), &[], &[], None, f * 50);
                if !frame.temp.is_empty() && entered.is_none() {
                    entered = Some(f);
                    before = was;
                }
                temps.extend(frame.temp);
            }
            if !splashes {
                assert!(temps.is_empty(), "{} splashes", temps.len());
                continue;
            }
            assert_eq!(temps.len(), 1);
            let t = &temps[0];
            assert_eq!(t.event, vcod_common::net::event_ids::EV_BULLET_HIT_SMALL);
            assert_eq!(t.surf_type, WATER_SURF_TYPE);
            assert_eq!(t.other, id.0);
            assert_eq!(t.origin, before.to_array());
            assert!(before.z > 64.0, "{before}");
            assert_eq!(t.parm, dir_to_byte([0.0, 0.0, 1.0]));
            let d = Vec3::new(0.0, 100.0, 600.0).normalize();
            assert_eq!(t.scale, dir_to_byte(d.into()));
            assert_eq!(t.weapon, 0);
        }
    }

    /// UO's splash carries the weapon, and frees a smoke grenade on the
    /// spot (game.mp.uo 0x5f1f3).
    #[test]
    fn a_uo_smoke_grenade_is_freed_on_its_splash() {
        let world = pool();
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let id = armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, -600.0),
            0,
        );
        ms.set_uo_kind(id, explosion_type("smoke"), false, 0);
        let mut freed = None;
        for f in 1..10 {
            let frame = ms.run(Some(&world), &[], &[], None, f * 50);
            if !frame.temp.is_empty() {
                assert_eq!(frame.temp[0].weapon, 8);
                assert_eq!(frame.freed, vec![id]);
                freed = Some(f);
                break;
            }
        }
        assert!(freed.is_some());
        assert!(!ms.contains(id));
    }

    /// `SnapVectorTowards` floors or ceils toward the base; a negative axis
    /// snaps toward the base by less than a unit (combat doc 13.1).
    #[test]
    fn snap_towards_floors_and_ceils() {
        assert_eq!(snap_towards(-191.875, 212.0), -191.0);
        assert_eq!(snap_towards(-6647.35, -7036.0), -6648.0);
        assert_eq!(snap_towards(10.5, 0.0), 10.0);
        assert_eq!(snap_towards(10.5, 20.0), 11.0);
        assert_eq!(snap_towards(-4.0, 20.0), -4.0);
    }

    /// 1.5's and UO's `G_MissileTrace` reads a start-solid trace as a
    /// contact at the start whose normal points back along the move; 1.1's
    /// `G_RunMissile` zeroes only the fraction (combat doc 12.3).
    #[test]
    fn a_start_solid_move_is_a_contact() {
        use vcod_common::collision::{CONTENTS_SOLID, synthetic_world};
        let world = synthetic_world(
            &[("textures/test/solid", CONTENTS_SOLID, 0)],
            &[(0, [-64.0, -64.0, -64.0], [64.0, 64.0, 64.0])],
        );
        for falls in [false, true] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                falls,
                ..Missiles::default()
            };
            let from = Vec3::new(0.0, 0.0, 32.0);
            let to = Vec3::new(0.0, 200.0, 32.0);
            armed(&mut ms, &mut host, &mut vm, from, Vec3::ZERO, 0);
            let m = &ms.missiles()[0];
            let hit = m.full_trace(&world, &[], &[], None, from, to);
            assert!(hit.tr.startsolid);
            assert_eq!(hit.tr.fraction == 0.0, falls, "{:?}", hit.tr);
            if falls {
                assert_eq!(hit.tr.normal, -Vec3::Y);
            }
        }
    }

    /// Every protocol's move trace runs under `fire_grenade`'s clipmask
    /// 0x2802091, so a 1.1 missile stops on a 0x2080 kerb brush as a UO
    /// one does (combat doc 11.1).
    #[test]
    fn a_kerb_brush_stops_a_missile_on_every_protocol() {
        use vcod_common::collision::synthetic_world;
        let world = synthetic_world(
            &[("textures/test/kerb", 0x2080, 0)],
            &[(0, [-64.0, 64.0, -64.0], [64.0, 128.0, 64.0])],
        );
        for uo in [false, true] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                uo,
                ..Missiles::default()
            };
            let from = Vec3::new(0.0, 0.0, 32.0);
            let to = Vec3::new(0.0, 200.0, 32.0);
            armed(&mut ms, &mut host, &mut vm, from, Vec3::ZERO, 0);
            let hit = ms.missiles()[0].full_trace(&world, &[], &[], None, from, to);
            assert!(
                (hit.tr.endpos.y - 64.0).abs() < 0.2,
                "uo {uo}: {:?}",
                hit.tr
            );
        }
    }

    fn standing(at: [f32; 3]) -> ClientSim {
        let mut sim = ClientSim::spectator(at, 0.0, [0; 3]);
        sim.become_player(at, 0.0, [0; 3]);
        sim
    }

    /// A satchel (`takeDamage`, a fuse past 500 ms) is shootable: its
    /// thrower's traces pass it until `G_GrenadeLeaveOwnerThink`, from
    /// 500 ms on and every 100 ms after, finds no body over it, and the
    /// fuse it was thrown with stands. A thrower who never steps off holds
    /// it until 100 ms before the fuse.
    #[test]
    fn a_satchel_passes_its_thrower_until_he_steps_off() {
        let world = vcod_common::collision::test_world(&[]);
        for leaves_at in [Some(1000), None] {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let mut ms = Missiles {
                uo: true,
                falls: true,
                ..Missiles::default()
            };
            let id = armed(
                &mut ms,
                &mut host,
                &mut vm,
                Vec3::new(0.0, 0.0, 8.0),
                Vec3::ZERO,
                0,
            );
            assert!(ms.set_uo_kind(id, explosion_type("rocket"), true, 0));
            let (on, off) = (standing([0.0, 0.0, 0.0]), standing([300.0, 0.0, 0.0]));
            let mut cleared = None;
            let mut blew = None;
            for f in 1..=90 {
                let t = f * 50;
                let thrower = if leaves_at.is_some_and(|l| t >= l) {
                    &off
                } else {
                    &on
                };
                let frame = ms.run(Some(&world), &[(0, thrower)], &[], None, t);
                if cleared.is_none() && ms.shot_targets()[0].owner.is_none() {
                    cleared = Some(t);
                }
                if blew.is_none() && !frame.exploded.is_empty() {
                    blew = Some(t);
                }
            }
            assert_eq!(cleared, Some(leaves_at.unwrap_or(3900)), "{leaves_at:?}");
            assert_eq!(blew, Some(4000), "{leaves_at:?}");
        }
    }

    /// `G_Damage` on a satchel clear of its thrower: `G_GrenadeDie` blows it
    /// 100 ms on and it takes no more; a frag never had the box.
    #[test]
    fn a_shot_satchel_blows_100_ms_on() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let frag = armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(50.0, 0.0, 8.0),
            Vec3::ZERO,
            0,
        );
        ms.set_uo_kind(frag, 0, false, 0);
        let id = armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 8.0),
            Vec3::ZERO,
            0,
        );
        ms.set_uo_kind(id, explosion_type("rocket"), true, 0);
        for f in 1..=20 {
            assert!(
                ms.run(Some(&world), &[], &[], None, f * 50)
                    .exploded
                    .is_empty()
            );
        }
        assert_eq!(ms.shot_targets().len(), 1);
        assert!(!ms.kill(frag, 1000));
        assert!(ms.kill(id, 1000));
        assert!(!ms.kill(id, 1000), "takedamage is gone");
        assert!(
            ms.run(Some(&world), &[], &[], None, 1050)
                .exploded
                .is_empty()
        );
        let frame = ms.run(Some(&world), &[], &[], None, 1100);
        assert_eq!(frame.exploded.len(), 1);
        assert_eq!(frame.exploded[0].inflictor, id);
    }

    /// A satchel gone off on its fuse keeps retail's `takedamage`, but no
    /// blast or round sets it off again: `G_ExplodeMissile` zeroes its
    /// contents, and `freeAfterEvent` keeps `G_RunFrame` from running the
    /// think a second `G_GrenadeDie` would set (coduo-diff.md 21). It goes
    /// out once and is freed when its event ages out.
    #[test]
    fn a_satchel_gone_off_never_goes_off_again() {
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let id = armed(
            &mut ms,
            &mut host,
            &mut vm,
            Vec3::new(0.0, 0.0, 8.0),
            Vec3::ZERO,
            0,
        );
        ms.set_uo_kind(id, explosion_type("rocket"), true, 0);
        let mut blew = Vec::new();
        for f in 1..=100 {
            let t = f * 50;
            // Every frame from the fuse's on, a blast or a round reaches it.
            if ms.contains(id) && !ms.blast_targets().iter().any(|(b, _)| *b == id) {
                assert!(!ms.kill(id, t), "a gone-off satchel took damage at {t}");
                assert!(ms.shot_targets().is_empty(), "a round stops on it at {t}");
            }
            let frame = ms.run(Some(&world), &[], &[], None, t);
            blew.extend(frame.exploded.iter().map(|_| t));
            if frame.freed.contains(&id) {
                assert_eq!(t, blew[0] + EVENT_VALID_MS + 50, "freed with its event");
            }
        }
        assert_eq!(blew, [4000], "one explode, on the fuse");
        assert!(!ms.contains(id));
    }

    /// A tank shell striking a vehicle's collmap names the vehicle as what
    /// it struck, and once the caller marks the hit the explode carries
    /// UO's 0x100000 for the impact row's `vehicle` column.
    #[test]
    fn a_shell_on_a_vehicle_names_it_and_marks_the_explode() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let world = vcod_common::collision::submodel_test_world(
            "{\n\"classname\" \"script_vehicle_collmap\"\n\"model\" \"*1\"\n}\n",
            &[([-32.0, -32.0, 0.0], [32.0, 32.0, 64.0])],
        );
        world.set_model_entity(1, 300);
        world.set_model_linked(1, true);
        world.set_model_pose(1, Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let shell = Shell {
            drop_at_ms: None,
            speed: 3000.0,
            damage: 300,
            vehicle: Some(EntId(200, 0)),
        };
        let ents = &mut host.ents;
        let id = vm
            .with_cx(|cx| {
                ms.fire_rocket(
                    ents,
                    cx,
                    0,
                    42,
                    Vec3::new(0.0, 0.0, 40.0),
                    Vec3::X,
                    shell,
                    6,
                    true,
                    0,
                )
            })
            .unwrap();
        let x = (1..=10)
            .find_map(
                |f| match ms.run_one(id, Some(&world), &[], &[], None, f * 50) {
                    Some(Turn::Exploded(x)) => Some(x),
                    _ => None,
                },
            )
            .expect("it struck the hull");
        assert_eq!(x.struck, Some(300));
        assert!((x.at.x - 368.0).abs() < 8.0, "{}", x.at);
        ms.struck_vehicle(id);
        let e = ms.entities(p).next().unwrap().1;
        assert_eq!(
            vcod_common::net::flags::eflags_v1_to_v22(e.field_i32(p, "eFlags")),
            SHELL_EFLAGS_V22 | 0x100000
        );
    }

    /// An artillery shell hangs unsent until its drop, falls at its speed
    /// from 2000 over the strike and explodes on the floor as an `eType` 0
    /// entity with type 4, `eFlags` 0x8000 in UO's layout and the splash
    /// means of death.
    #[test]
    fn a_shell_hangs_drops_and_explodes_on_contact() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let world = vcod_common::collision::test_world(&[]);
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let shell = Shell {
            drop_at_ms: Some(1750),
            speed: 10_000.0,
            damage: 2000,
            vehicle: None,
        };
        let ents = &mut host.ents;
        let id = vm
            .with_cx(|cx| {
                ms.fire_artillery(
                    ents,
                    cx,
                    Some(0),
                    6,
                    Vec3::new(0.0, 0.0, 1000.0),
                    shell,
                    4,
                    0,
                )
            })
            .unwrap();
        assert_eq!(ms.entities(p).count(), 0, "unsent while it hangs");
        let mut x = None;
        for f in 1..=60 {
            if let Some(Turn::Exploded(e)) = ms.run_one(id, Some(&world), &[], &[], None, f * 50) {
                x = Some((f * 50, e));
                break;
            }
        }
        let (at, x) = x.expect("it struck the floor");
        assert!((1800..2100).contains(&at), "struck at {at}");
        assert_eq!(
            (x.mod_, x.direct, x.concussive),
            ("MOD_ARTILLERY_SPLASH", None, false)
        );
        assert!(x.at.z > 0.0 && x.at.z <= UO_IMPACT_LIFT + 0.2, "{}", x.at);
        let e = ms.entities(p).next().unwrap().1;
        assert_eq!(e.field_i32(p, "eType"), ET_GENERAL);
        assert_eq!(e.field_i32(p, "_union.scale"), 4);
        assert_eq!(
            vcod_common::net::flags::eflags_v1_to_v22(e.field_i32(p, "eFlags")),
            SHELL_EFLAGS_V22
        );
    }

    /// `G_DropArtillery` (0x5eabe) arms `G_ExplodeMissile` 15 s past the
    /// drop: a shell that strikes nothing goes off then, its splash charged
    /// to `parent` (13.7): the caller, NULL once his leave swept it, and
    /// `&g_entities[1023]` for a shell a barrage hung after he left.
    #[test]
    fn a_shell_that_strikes_nothing_goes_off_fifteen_seconds_past_its_drop() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let shell = Shell {
            drop_at_ms: Some(100),
            speed: 1000.0,
            damage: 2000,
            vehicle: None,
        };
        let ents = &mut host.ents;
        let ids: Vec<EntId> = [Some(0), Some(1), None]
            .into_iter()
            .map(|owner| {
                vm.with_cx(|cx| {
                    ms.fire_artillery(ents, cx, owner, 6, Vec3::Z * 1000.0, shell, 4, 0)
                })
                .unwrap()
            })
            .collect();
        ms.free_owner_refs(1);
        let mut went = Vec::new();
        for f in 1..=400 {
            for &id in &ids {
                if let Some(Turn::Exploded(x)) = ms.run_one(id, None, &[], &[], None, f * 50) {
                    went.push((f * 50, x.owner, x.mod_));
                }
            }
        }
        let splash = "MOD_ARTILLERY_SPLASH";
        assert_eq!(
            went,
            [
                (15_100, Some(0), splash),
                (15_100, None, splash),
                (15_100, Some(crate::game::host::ZEROED_ENTITY), splash),
            ]
        );
    }

    /// A UO vehicle shell that strikes nothing goes off on `fire_rocket`'s
    /// 15 s fuse, its splash charged to `parent`, the vehicle, where its
    /// direct hit would name the driver; a freed vehicle swept `parent` to
    /// NULL (game.mp.uo 0x5e2fc, 0x5fd6e, combat doc 13.7).
    #[test]
    fn a_vehicle_shell_s_fuse_blast_is_charged_to_the_vehicle() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let (tank, freed) = (EntId(200, 0), EntId(201, 0));
        let ents = &mut host.ents;
        let ids: Vec<EntId> = [tank, freed]
            .into_iter()
            .map(|vehicle| {
                let shell = Shell {
                    drop_at_ms: None,
                    speed: 1000.0,
                    damage: 300,
                    vehicle: Some(vehicle),
                };
                vm.with_cx(|cx| {
                    ms.fire_rocket(ents, cx, 3, 42, Vec3::ZERO, Vec3::X, shell, 6, false, 0)
                })
                .unwrap()
            })
            .collect();
        ms.free_owner_refs(3);
        ms.free_vehicle_refs(freed);
        let mut went = Vec::new();
        for f in 1..=400 {
            for &id in &ids {
                if let Some(Turn::Exploded(x)) = ms.run_one(id, None, &[], &[], None, f * 50) {
                    went.push((f * 50, x.owner, x.mod_));
                }
            }
        }
        let splash = "MOD_PROJECTILE_SPLASH";
        assert_eq!(
            went,
            [
                (ROCKET_LIFE_MS, Some(200), splash),
                (ROCKET_LIFE_MS, None, splash),
            ]
        );
    }

    /// A rocket's fuse blast is its `splashMethodOfDeath`, which
    /// `fire_rocket` sets to 6 (`MOD_PROJECTILE_SPLASH`; UO 0x5fc74, 1.1d
    /// 0x54614, combat doc 13.6).
    #[test]
    fn a_rocket_that_strikes_nothing_blasts_as_a_projectile() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let shell = Shell {
            drop_at_ms: None,
            speed: 1000.0,
            damage: 300,
            vehicle: None,
        };
        let ents = &mut host.ents;
        let id = vm
            .with_cx(|cx| ms.fire_rocket(ents, cx, 0, 42, Vec3::ZERO, Vec3::X, shell, 6, false, 0))
            .unwrap();
        let x = (1..=400)
            .find_map(|f| match ms.run_one(id, None, &[], &[], None, f * 50) {
                Some(Turn::Exploded(x)) => Some((f * 50, x.mod_)),
                _ => None,
            })
            .expect("its fuse ran out");
        assert_eq!(x, (ROCKET_LIFE_MS, "MOD_PROJECTILE_SPLASH"));
    }

    /// A shell that strikes on the very turn a script free is due still
    /// goes off: `G_RunMissile` returns after `G_MissileImpact` turns the
    /// `eType` (UO 0x5f574, 1.1d 0x54354) before `G_RunThink`, and
    /// `G_RunEntity` runs no think on a `freeAfterEvent` entity (UO
    /// 0x59b5c), so it rides out its event (combat doc 13.8).
    #[test]
    fn a_shell_striking_as_its_free_falls_due_rides_out_its_event() {
        let world = vcod_common::collision::test_world(&[]);
        let fire = |ms: &mut Missiles| {
            let (mut vm, mut host) = crate::game::testing::fixture();
            let shell = Shell {
                drop_at_ms: Some(50),
                speed: 10_000.0,
                damage: 2000,
                vehicle: None,
            };
            let ents = &mut host.ents;
            vm.with_cx(|cx| ms.fire_artillery(ents, cx, Some(0), 6, Vec3::Z * 1000.0, shell, 4, 0))
                .unwrap()
        };
        let new = || Missiles {
            uo: true,
            falls: true,
            ..Missiles::default()
        };
        let strike = |ms: &mut Missiles, id: EntId| {
            (1..=60)
                .find(|f| {
                    matches!(
                        ms.run_one(id, Some(&world), &[], &[], None, f * 50),
                        Some(Turn::Exploded(_))
                    )
                })
                .map(|f| f * 50)
        };
        let mut ms = new();
        let id = fire(&mut ms);
        let at = strike(&mut ms, id).expect("it struck the floor");
        let mut ms = new();
        let id = fire(&mut ms);
        ms.schedule_free(id, at);
        assert_eq!(strike(&mut ms, id), Some(at), "the free did not take it");
        for t in (at + 50..=at + EVENT_VALID_MS).step_by(50) {
            assert!(
                matches!(
                    ms.run_one(id, Some(&world), &[], &[], None, t),
                    Some(Turn::Flew)
                ),
                "{t}"
            );
        }
        assert!(matches!(
            ms.run_one(id, Some(&world), &[], &[], None, at + EVENT_VALID_MS + 50),
            Some(Turn::Freed)
        ));
    }

    /// One snapshot of a missile as a net probe's `VEH missile` line logs
    /// it: the snapshot's time, `pos` (type, `trTime`, base and delta
    /// bits), `groundEntityNum`, `eType`, then `apos` (type, `trTime`, base
    /// and delta bits).
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct WireRow {
        time: i32,
        pos: (i32, i32, [u32; 3], [u32; 3]),
        ground: i32,
        e_type: i32,
        apos: (i32, i32, [u32; 3], [u32; 3]),
    }

    /// The first missile's snapshots in a capture's `VEH missile` lines.
    fn wire_rows(text: &str) -> Vec<WireRow> {
        let mut num = None;
        let mut out = Vec::new();
        for l in text
            .lines()
            .filter(|l| l.starts_with("VEH ") && l.contains(" missile "))
        {
            let t: Vec<&str> = l.split(' ').collect();
            if *num.get_or_insert(t[3]) != t[3] {
                continue;
            }
            let at = |k: &str| t.iter().position(|w| *w == k).unwrap() + 1;
            let int = |i: usize| t[i].parse::<i32>().unwrap();
            let fb = |i: usize| t[i].parse::<f32>().unwrap().to_bits();
            let hex = |i: usize| u32::from_str_radix(t[i].trim_start_matches("0x"), 16).unwrap();
            let (b, a, d, p) = (at("bits"), at("apos"), at("atraj"), at("traj"));
            out.push(WireRow {
                time: int(1),
                pos: (
                    int(at("type")),
                    int(p),
                    [hex(b), hex(b + 1), hex(b + 2)],
                    [fb(p + 1), fb(p + 2), fb(p + 3)],
                ),
                ground: int(at("ground")),
                e_type: int(at("etype")),
                apos: (
                    int(d),
                    int(d + 1),
                    [fb(a), fb(a + 1), fb(a + 2)],
                    [fb(d + 2), fb(d + 3), fb(d + 4)],
                ),
            });
        }
        out
    }

    /// What retail drew at each bounce of a flight, off its `apos` deltas:
    /// a wall's whole-degree step when `AngleNormalize360(rate + step)`
    /// gives the new rate, otherwise a floor's factor, the ratio of the
    /// rates, with whether it reversed the rate (12.7).
    fn contact_draws(rows: &[WireRow]) -> Vec<Bounce> {
        use vcod_common::pmove::aim::angle_normalize_360;
        let mut out = Vec::new();
        for w in rows.windows(2) {
            let (a, b) = (w[0].apos, w[1].apos);
            if a == b || b.0 != TR_LINEAR {
                continue;
            }
            let (old, new) = (f32::from_bits(a.3[0]), f32::from_bits(b.3[0]));
            let wall = (-63..=64)
                .find(|k| angle_normalize_360((f64::from(*k) + f64::from(old)) as f32) == new);
            out.push(match wall {
                Some(k) => Bounce::Wall(k),
                None => Bounce::Floor(f64::from(new) / f64::from(old)),
            });
        }
        out
    }

    #[derive(Debug, Clone, Copy)]
    enum Bounce {
        Wall(i32),
        /// The signed factor: negative where the pitch lay within 80 of
        /// the surface's.
        Floor(f64),
    }

    /// A United Offensive frag's flight in `fixture` (a net probe's lines
    /// off retail 1.51b) replayed from its launch on wire: each frame
    /// ours steps it on the map's collision under UO's stores, with each
    /// contact's draw pinned to what retail's `apos` shows, and every
    /// snapshot until the explode carries retail's `pos`, ground and
    /// `apos` to the bit. Returns how many snapshots it compared.
    fn replay_uo_flight(fixture: &str, map: &str) -> Option<usize> {
        let fs = vcod_common::testing::uo_fs()?;
        let bsp_path = fs.resolve_map(map).expect("map in the paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        world
            .collision
            .set_arith(vcod_common::patch::Arith::for_protocol(
                &vcod_common::net::protocol::PROTOCOL_V22,
            ));
        let rows: Vec<WireRow> = wire_rows(fixture)
            .into_iter()
            .filter(|r| r.e_type == 4)
            .collect();
        let launch = rows[0];
        let (vm, host) = &mut crate::game::testing::fixture();
        let mut ms = Missiles {
            uo: true,
            falls: true,
            ..Default::default()
        };
        let base = Vec3::from_array(launch.pos.2.map(f32::from_bits));
        armed(&mut ms, host, vm, base, Vec3::ZERO, launch.pos.1);
        let m = ms.missiles_mut().next().unwrap();
        m.traj.delta = Vec3::from_array(launch.pos.3.map(f32::from_bits));
        m.apos = Trajectory {
            tr_type: launch.apos.0,
            tr_time: launch.apos.1,
            tr_duration: 0,
            base: Vec3::from_array(launch.apos.2.map(f32::from_bits)),
            delta: Vec3::from_array(launch.apos.3.map(f32::from_bits)),
        };
        m.explode_at_ms = i32::MAX;
        let contacts = contact_draws(&rows);
        // 1.5b's and UO's floor factor, `rand() / 2^31 * 0.3 + 0.85`
        // (12.7): 1.1d's 0.55 to 0.85 could not draw these.
        for c in &contacts {
            if let Bounce::Floor(f) = c {
                assert!((0.85..=1.15).contains(&f.abs()), "{map}: {contacts:?}");
            }
        }
        let mut draws = Draws {
            pinned: contacts
                .iter()
                .map(|c| match *c {
                    Bounce::Wall(k) => f64::from(k),
                    Bounce::Floor(f) => f.abs(),
                })
                .collect(),
            ..Default::default()
        };
        let mut ours = Vec::new();
        let mut now = launch.pos.1;
        while now < rows.last().unwrap().time {
            now += 50;
            m.step(Some(&world.collision), &[], &[], None, &mut draws, now);
            let bits = |v: Vec3| v.to_array().map(f32::to_bits);
            // UO sends `apos.trBase` as 16-bit angles (`bits -100`,
            // coduo-diff.md 3): the 65536ths `MSG_WriteDeltaEntity` keeps.
            let wire = |v: Vec3| {
                v.to_array().map(|a| {
                    let units = (f64::from(a) * f64::from(65536.0f32 / 360.0)) as i64;
                    (f32::from(units as u16 as i16) * (360.0 / 65536.0)).to_bits()
                })
            };
            ours.push(WireRow {
                time: now,
                pos: (
                    m.traj.tr_type,
                    m.traj.tr_time,
                    bits(m.traj.base),
                    bits(m.traj.delta),
                ),
                ground: m.ground,
                e_type: 4,
                apos: (
                    m.apos.tr_type,
                    m.apos.tr_time,
                    wire(m.apos.base),
                    bits(m.apos.delta),
                ),
            });
        }
        let mut compared = 0;
        for r in &rows {
            let o = ours.iter().find(|o| o.time == r.time);
            assert_eq!(
                o,
                Some(r),
                "{map}: the snapshot at {} differs (left ours, right retail); {contacts:?}",
                r.time
            );
            compared += 1;
        }
        Some(compared)
    }

    /// A probe's flights on mp_brecourt's field, thrown from the same spot
    /// up and to the north-east (yaw 45, pitch 45 up) and to the
    /// north-west (yaw 135, 20 up): each lands on the terrain and rolls to
    /// rest over four floor contacts (12.5, 12.7).
    #[test]
    fn uo_frags_roll_on_brecourts_terrain_like_retail() {
        for fixture in [
            include_str!("../../tests/fixtures/missiles/mp_brecourt-frag-roll-uo-45.txt"),
            include_str!("../../tests/fixtures/missiles/mp_brecourt-frag-roll-uo-135.txt"),
        ] {
            let Some(n) = replay_uo_flight(fixture, "mp_brecourt") else {
                return;
            };
            assert!(n > 60, "{n} snapshots");
        }
    }

    /// mp_uo_hurtgen: a frag thrown west off the spot lands on a terrain
    /// face too steep to rest on (normal z 0.59) and slides down it,
    /// struck every frame, over 21 floor contacts before it rests on the
    /// gentler grade below (12.5, 12.7).
    #[test]
    fn a_uo_frag_slides_down_hurtgens_terrain_like_retail() {
        let fixture = include_str!("../../tests/fixtures/missiles/mp_uo_hurtgen-frag-slide-uo.txt");
        let Some(n) = replay_uo_flight(fixture, "mp_uo_hurtgen") else {
            return;
        };
        assert!(n > 60, "{n} snapshots");
        let contacts = contact_draws(&wire_rows(fixture));
        let floors: Vec<f64> = contacts
            .iter()
            .filter_map(|c| match c {
                Bounce::Floor(f) => Some(*f),
                Bounce::Wall(_) => None,
            })
            .collect();
        assert_eq!(floors.len(), 21, "{contacts:?}");
        // Some struck within 80 of the face's pitch and reversed, some not.
        assert!(floors.iter().any(|f| *f < 0.0) && floors.iter().any(|f| *f > 0.0));
    }
}
