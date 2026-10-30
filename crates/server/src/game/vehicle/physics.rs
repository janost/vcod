//! A vehicle's 50 ms step: `Scr_Vehicle_Think`'s motion half (game.mp.uo
//! 0x8c7cc and 0x87cd3), the turret and gunner aim, and the entity fields
//! the step writes (docs/research/coduo-vehicles.md 12).
//!
//! The parent owns the entity, the seats, the guns and the sounds. Per
//! think it calls [`Physics::motion`], runs the driver's and the gunner's
//! fire (retail's 0x86c57 and 0x871a6 sit between the two halves), then
//! [`Physics::finish`]; [`Physics::think`] runs both for a caller with no
//! fire of its own. Every step is a fixed 0.05 s.

use glam::{DVec3, Vec3};

use super::info::{KIND_ARTILLERY, KIND_FOUR_WHEEL, KIND_TANK, VehicleInfo};
use vcod_common::x87::angles_to_axis;

/// The step, seconds (0.05 f32 at 0xa6d54 and kin).
pub const STEP: f32 = 0.05;
pub const ENTITYNUM_WORLD: u32 = 1022;
pub const ENTITYNUM_NONE: u32 = 1023;
/// `ent+0x1a0` at spawn: what the hull's ground trace and slide clip.
pub const CLIPMASK: u32 = 0x201;
const CONTENTS_BODY: u32 = 0x2000000;
const CONTENTS_VEHICLE: u32 = 0x800000;
/// The wheel traces' masks, driven and not (0x81331).
const WHEEL_MASK_DRIVEN: u32 = 0x10211;
const WHEEL_MASK: u32 = 0x211;

/// The six wheel tags in `G_UpdateVehicleTags`' order (table 0xae978).
pub const WHEEL_TAGS: [&str; 6] = [
    "tag_wheel_front_left",
    "tag_wheel_front_right",
    "tag_wheel_back_left",
    "tag_wheel_back_right",
    "tag_wheel_middle_left",
    "tag_wheel_middle_right",
];

/// The unstick search's 26 offsets (0xa66c0).
const UNSTICK: [[f32; 3]; 26] = [
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [0.0, -1.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [-1.0, 0.0, 0.0],
    [0.0, -1.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 0.0, -1.0],
    [0.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
];

/// `vehCheckSolidOffsets` (0xa6660): the six collision probes, x forward.
const PROBES: [[f32; 3]; 6] = [
    [1.2, -1.0, 0.0],
    [1.3, 0.0, 0.0],
    [1.2, 1.0, 0.0],
    [-1.25, 1.0, 0.0],
    [-1.35, 0.0, 0.0],
    [-1.25, -1.0, 0.0],
];

/// What a trace stopped on, `trap_Trace`'s `trace_t` as the step reads it.
#[derive(Clone, Copy, Debug)]
pub struct VehTrace {
    pub fraction: f32,
    pub endpos: Vec3,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub startsolid: bool,
    pub allsolid: bool,
    /// [`ENTITYNUM_WORLD`] for world geometry, [`ENTITYNUM_NONE`] for a miss.
    pub entity: u32,
}

/// What the step asks about an entity a trace hit.
#[derive(Clone, Copy, Debug, Default)]
pub struct HitEntity {
    /// `r.currentOrigin`.
    pub origin: Vec3,
    /// A gentity with a client (a player body).
    pub client: bool,
    /// Classname `script_model` (scr_const 0x7e): the slide and the probes
    /// notify it `trigger` with the vehicle.
    pub script_model: bool,
    /// `eType` 12: a vehicle, which the step never climbs.
    pub vehicle: bool,
    /// The struck vehicle's `maxSpeed`, when it has a pool record.
    pub vehicle_max_speed: Option<f32>,
    /// The struck vehicle's velocity (`rec+0xe8`).
    pub vehicle_velocity: Vec3,
    /// The struck vehicle's sway kick still runs (`rec+0x2b0` not 0).
    pub vehicle_jolting: bool,
}

/// What a step's hit does to another vehicle (0x827ab's push of a
/// `script_vehicle`): a sway kick, and its velocity as the push leaves it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehiclePush {
    pub entity: u32,
    /// The kick's direction and scale, with a 175 decel.
    pub jolt: Option<(Vec3, f32)>,
    pub velocity: Vec3,
}

/// The world as the step traces it. Every trace passes the vehicle itself
/// (its own collmap brush model and DObj), as retail's pass entity does.
pub trait VehicleWorld {
    /// `trap_Trace`: a box sweep, a point when `mins == maxs == 0`, a
    /// position test when `start == end`. Bodies clip only under
    /// `CONTENTS_BODY` (0x2000000) and vehicles' collmaps only under
    /// 0x800000, as their `r.contents` say.
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> VehTrace;
    fn entity(&self, num: u32) -> HitEntity;
}

/// [`VehicleWorld`] over the collision world alone: no bodies, every hit
/// the world or a brush model's entity.
pub struct CollisionVehicleWorld<'a> {
    pub world: &'a vcod_common::collision::CollisionWorld,
    /// The vehicle's own brush model, which its traces pass through.
    pub skip: Option<vcod_common::collision::Pusher>,
}

impl VehicleWorld for CollisionVehicleWorld<'_> {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> VehTrace {
        let t = self
            .world
            .box_sweep(start, end, mins, maxs, mask, self.skip);
        VehTrace {
            fraction: t.fraction,
            endpos: t.endpos,
            normal: t.normal,
            surface_flags: t.surface_flags,
            startsolid: t.startsolid,
            allsolid: t.allsolid,
            entity: self.world.entity_num(&t),
        }
    }

    fn entity(&self, _num: u32) -> HitEntity {
        HitEntity::default()
    }
}

/// What the model gives the step: the wheel tags' model-space translations
/// (`G_DObjGetLocalTagMatrix`, [`WHEEL_TAGS`] order, `None` where the model
/// has no such tag) and whether the turret and gunner tags exist.
#[derive(Clone, Copy, Debug, Default)]
pub struct VehicleModel {
    pub wheels: [Option<Vec3>; 6],
    /// `tag_barrel`: the turret aims only with one.
    pub has_barrel: bool,
    /// `tag_secondary_gun`: the gunner aims only with one.
    pub has_secondary_gun: bool,
}

impl VehicleModel {
    /// The tags off an xmodel's bones (`pos` is model space).
    pub fn from_bones(bones: &[vcod_common::model::Bone]) -> VehicleModel {
        let find = |n: &str| bones.iter().find(|b| b.name.eq_ignore_ascii_case(n));
        VehicleModel {
            wheels: WHEEL_TAGS.map(|t| find(t).map(|b| b.pos)),
            has_barrel: find("tag_barrel").is_some(),
            has_secondary_gun: find("tag_secondary_gun").is_some(),
        }
    }
}

/// The driver's half of the stored usercmd and playerstate.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriveInput {
    pub forward: i8,
    pub right: i8,
    pub up: i8,
    /// The latched buttons (`client+0x462c`).
    pub buttons: i32,
    /// `ps.viewangles`, degrees.
    pub view: [f32; 3],
    /// `ps.fWeaponPosFrac != 0`.
    pub ads: bool,
    /// `ps.eFlags & 0x400000`: popped out of the hatch, input frozen.
    pub popped_out: bool,
}

/// Everything one think reads besides the record.
#[derive(Clone, Copy, Debug)]
pub struct StepInput {
    /// `level.time`.
    pub now: i32,
    /// The seat-1 occupant's input; `None` with no driver.
    pub driver: Option<DriveInput>,
    /// The seat-2 occupant's `ps.viewangles`.
    pub gunner_view: Option<[f32; 3]>,
    /// `ent+0x17e == 2`.
    pub driven: bool,
    pub health: i32,
    /// Script `capturing`: freezes driver input and the turret.
    pub capturing: bool,
    /// Script `closespeed`, a `maxSpeed` multiplier; 0 is unset.
    pub closespeed: f32,
    /// Script `harc`, the turret yaw clamp; 0 is unset.
    pub harc: f32,
    /// `r.bmodel` (ent+0x104): a vehicle with no collmap runs no collision
    /// probes. An unlinked one still does (coduo-vehicles.md 15.2).
    pub bmodel: bool,
    /// `r.maxs`, the collmap's bounds: the probes' broad box.
    pub bounds_maxs: Vec3,
    /// `level.maxclients`: a probe hit below it is a body.
    pub max_clients: u32,
    /// Another pool vehicle within 1024 that itself needs physics
    /// (`VEH_PhysicsNotRequired`'s neighbour test).
    pub neighbour_moving: bool,
    /// `g_vehicleTexScrollScale`; 0 or less takes the file's.
    pub tex_scroll_scale: f32,
    /// `g_vehicleHorns`.
    pub horns: bool,
    /// `rec+0x264`, the alt weapon's overheat flag.
    pub alt_overheated: bool,
}

impl Default for StepInput {
    fn default() -> Self {
        StepInput {
            now: 0,
            driver: None,
            gunner_view: None,
            driven: false,
            health: 1,
            capturing: false,
            closespeed: 0.0,
            harc: 0.0,
            bmodel: true,
            bounds_maxs: Vec3::ZERO,
            max_clients: 64,
            neighbour_moving: false,
            tex_scroll_scale: 0.0,
            horns: true,
            alt_overheated: false,
        }
    }
}

/// A body a probe struck (`VEH_PlayerCollision`), with the vehicle as it
/// stood: the parent runs [`player_push`] on it.
#[derive(Clone, Copy, Debug)]
pub struct ClientStrike {
    pub entity: u32,
    pub vehicle_origin: Vec3,
    pub vehicle_velocity: Vec3,
}

/// A hit the probes undid the step for (0x827ab's tail).
#[derive(Clone, Copy, Debug)]
pub struct Collision {
    pub endpos: Vec3,
    pub normal: Vec3,
    pub entity: u32,
    /// `vehicle_collision`'s two vectors (normal, then the point 24 units
    /// back along it), when the notify fires.
    pub notify: Option<(Vec3, Vec3)>,
    /// Where the impact alias plays, when it does.
    pub sound: Option<Vec3>,
    /// `G_Damage` on the vehicle itself, MOD 0x18, under
    /// `g_vehicleEnableCollisionDamage`.
    pub self_damage: Option<i32>,
    /// Seven seconds wedged: 999999 damage to itself.
    pub stuck_kill: bool,
}

/// What one think leaves the parent to act on.
#[derive(Clone, Debug, Default)]
pub struct StepOutput {
    pub struck_clients: Vec<ClientStrike>,
    /// `script_model`s to notify `trigger` with the vehicle.
    pub touched: Vec<u32>,
    pub collisions: Vec<Collision>,
    /// The origin or angles moved: `G_SetOrigin`/`G_SetAngle` from
    /// [`Physics::origin`] and [`Physics::angles`], trType 1, relink.
    pub moved: bool,
    /// The driver's `ps.eFlags & 0x200000` after the think, when there is
    /// one.
    pub driver_flag_200000: Option<bool>,
    /// The horn sounded: the parent adds 0.04 to the alt heat.
    pub horn: bool,
    /// `turret_on_target`.
    pub turret_on_target: bool,
    /// Other vehicles the probes struck, for the parent to push.
    pub pushes: Vec<VehiclePush>,
}

/// The physics state `s` (`rec+0xb8`, 0xb8 bytes); `rec+0x170` holds a
/// copy of the last committed one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub origin: Vec3,
    /// `r.currentOrigin` at the think's start.
    pub prev_origin: Vec3,
    pub angles: Vec3,
    pub prev_angles: Vec3,
    pub velocity: Vec3,
    /// x forward, in the yaw-only frame.
    pub local_velocity: Vec3,
    /// Degrees/s: pitch, yaw, roll.
    pub angular_velocity: Vec3,
    /// The four-wheel steer angle, degrees.
    pub steer: f32,
    pub sway_pitch_vel: f32,
    pub sway_roll_vel: f32,
    pub sway_pitch: f32,
    pub sway_roll: f32,
    pub wheel_z: [f32; 6],
    /// `(surfaceFlags & 0x1f00000) >> 20` per wheel, 0 for none or a miss.
    pub wheel_surface: [f32; 6],
}

/// The body jolt (`rec+0x2a4..0x2bc`, 0x7d492).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Jolt {
    pub pitch: f32,
    pub roll: f32,
    pub duration: f32,
    pub remaining: f32,
    pub phase: f32,
    /// The speed cap and accel while a hit's lockout runs.
    pub speed: f32,
    pub accel: f32,
}

/// One vehicle's pool record, the physics part, and the entity fields the
/// step owns.
#[derive(Clone, Debug)]
pub struct Physics {
    pub s: State,
    /// `rec+0x170`.
    pub committed: State,
    pub model: VehicleModel,
    pub clipmask: u32,
    pub jolt: Jolt,
    /// `rec+0x24c`: 2 turning, 1 stopping, 0. The parent picks the loop
    /// sound off it.
    pub turret_spin: i32,
    /// `rec+0x254`.
    pub gunner_spin: i32,
    /// `rec+0x2d4`/`0x2d8`: idle and engine blend.
    pub engine_blend: [f32; 2],
    last_forward: i32,
    forward_since: i32,
    coast_until: i32,
    coast: (i8, i8),
    pub last_fraction: f32,
    /// `rec+0x3bc`: input frozen until then after a hit.
    pub lockout_until: i32,
    wheelbase: f32,
    last_notify: i32,
    last_sound: i32,
    last_entity: u32,
    last_entity_dist: f32,
    stuck_clock: i32,
    last_hit: i32,
    last_hit_normal: Vec3,
    pub jolt_time: i32,
    /// `es.angles2`: turret pitch, turret yaw off the hull, gunner pitch.
    pub angles2: [f32; 3],
    /// `es.origin2[1]`: gunner yaw off `angles2[1]`.
    pub gunner_yaw: f32,
    /// `es.origin2[0]`, `[2]`: sway pitch and roll packed 0..255, stored as
    /// ints in the float slots.
    pub sway_packed: [i32; 2],
    pub leanf: f32,
    /// `es.time`, the track scroll clock; the wire keeps 16 bits.
    pub time: i32,
    pub time2: i32,
    /// `ent+0x1f0`, script `speed`.
    pub speed: f32,
}

impl Physics {
    /// How long the hull has been striking something with no second's break,
    /// ms: the clock the stuck kill reads at 7000 (`stuck_clock`, 0x857dd).
    pub fn contact_ms(&self, now: i32) -> i32 {
        if self.last_hit < now - 1000 {
            0
        } else {
            (now - self.stuck_clock).max(0)
        }
    }

    /// The record as `G_SpawnVehicle` leaves it.
    pub fn spawn(origin: Vec3, angles: Vec3, model: VehicleModel) -> Physics {
        let s = State {
            origin,
            prev_origin: origin,
            angles,
            prev_angles: angles,
            ..State::default()
        };
        Physics {
            s,
            committed: s,
            model,
            clipmask: CLIPMASK,
            jolt: Jolt::default(),
            turret_spin: 0,
            gunner_spin: 0,
            engine_blend: [0.0; 2],
            last_forward: 0,
            forward_since: 0,
            coast_until: 0,
            coast: (0, 0),
            last_fraction: 0.0,
            lockout_until: 0,
            wheelbase: 0.0,
            last_notify: 0,
            last_sound: 0,
            last_entity: ENTITYNUM_NONE,
            last_entity_dist: 0.0,
            stuck_clock: 0,
            last_hit: 0,
            last_hit_normal: Vec3::ZERO,
            jolt_time: 0,
            angles2: [0.0; 3],
            gunner_yaw: 0.0,
            sway_packed: [127, 127],
            leanf: 0.0,
            time: 0,
            time2: 0,
            speed: 0.0,
        }
    }

    pub fn origin(&self) -> Vec3 {
        self.s.origin
    }

    pub fn angles(&self) -> Vec3 {
        self.s.angles
    }

    /// `Scr_Vehicle_Init`'s physics (0x8c5ab), at spawn + 50: a four-wheel
    /// or tank settles on its wheels (the plane sets its height, a third of
    /// the way to the plane's pitch and roll), then the previous pose is the
    /// settled one. The committed copy (`rec+0x170`) keeps `VEH_InitPhysics`'
    /// spawn pose, so a vehicle that settled runs its first think's
    /// collision probes (coduo-vehicles.md 15.2).
    pub fn init(&mut self, info: &VehicleInfo, world: &impl VehicleWorld, driven: bool) {
        if info.kind == KIND_FOUR_WHEEL || info.kind == KIND_TANK {
            self.suspension(info, world, driven);
        }
        self.s.prev_origin = self.s.origin;
        self.s.prev_angles = self.s.angles;
    }

    /// `VEH_UnlinkPlayer` of the driver: a tank whose forward was held over
    /// 750 ms keeps it 3 s.
    pub fn driver_left(&mut self, info: &VehicleInfo, now: i32) {
        if info.kind != KIND_TANK {
            return;
        }
        self.coast_until = now + 3000;
        let fwd = if self.forward_since < now - 750 {
            self.last_forward as i8
        } else {
            0
        };
        self.coast = (fwd, 0);
    }

    /// Both halves of a think, for a caller with no fire of its own.
    pub fn think(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        input: &StepInput,
    ) -> StepOutput {
        let mut out = self.motion(info, world, input);
        self.finish(info, input, &mut out);
        out
    }

    /// `Scr_Vehicle_Think` (0x8c7cc) up to the entity update: the drive
    /// step, the collision probes and their retry, the stuck clock.
    pub fn motion(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        input: &StepInput,
    ) -> StepOutput {
        let mut out = StepOutput::default();
        let (cur_o, cur_a) = (self.s.origin, self.s.angles);
        self.s.prev_origin = cur_o;
        self.s.prev_angles = cur_a;
        if info.kind != KIND_ARTILLERY {
            for pass in 1..=2 {
                self.drive(info, world, input, &mut out);
                let clear = self.collide(info, world, input, pass, &mut out);
                self.committed = self.s;
                if clear {
                    break;
                }
            }
        }
        if self.last_hit < input.now - 1000 {
            self.stuck_clock = input.now;
        }
        out.moved = self.s.origin != cur_o || self.s.angles != cur_a;
        out
    }

    /// The think's tail after the fire: turret aim (0x85b32), gunner aim
    /// (0x86285), sway pack (0x86735), leanf (0x86a29), track clock
    /// (0x86aa1), `time2`.
    pub fn finish(&mut self, info: &VehicleInfo, input: &StepInput, out: &mut StepOutput) {
        self.turret_aim(info, input, out);
        self.gunner_aim(info, input);
        self.pack_sway();
        self.leanf = if info.steer_wheels {
            -self.s.steer
        } else {
            0.0
        };
        self.track_clock(info, input);
        self.time2 = trunc(f64::from(1000.0f32) * f64::from(info.suspension_travel));
    }

    /// 0x87cd3: the input bytes, then the step unless nothing moves, then
    /// the speed and engine blend.
    fn drive(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        input: &StepInput,
        out: &mut StepOutput,
    ) {
        let now = input.now;
        let tank = info.kind == KIND_TANK;
        let (mut f, mut r, mut u) = (0i8, 0i8, 0i8);
        if input.health >= 1 {
            match &input.driver {
                None => {
                    if now < self.coast_until {
                        (f, r) = self.coast;
                    }
                }
                Some(d) => {
                    let mut flag = true;
                    if !input.capturing && self.lockout_until < now && !d.popped_out {
                        (f, r, u) = (d.forward, d.right, d.up);
                        if f != 0 && self.last_forward == 0 {
                            self.forward_since = now;
                        }
                        self.last_forward = i32::from(f);
                        // SU-152 and Elefant: no traverse, so ADS turns the hull.
                        if tank
                            && info.turret.horiz_left == 0.0
                            && info.turret.horiz_right == 0.0
                            && d.ads
                        {
                            u = 127;
                        }
                        if tank || u > 0 {
                            flag = false;
                        }
                        if tank && f < 0 {
                            r = r.wrapping_neg();
                        }
                    }
                    out.driver_flag_200000 = Some(flag);
                }
            }
        }

        if f != 0 || r != 0 || u != 0 || !self.physics_not_required(input.neighbour_moving) {
            let (accel, aa) = self.targets(info, input, f, r, u);
            let g = self.ground_check(info, world, input.driver.is_some());
            let s = &mut self.s;
            if g.walkable {
                s.angular_velocity = madv(s.angular_velocity, STEP, aa);
            }
            let yaw = an180(mad(s.prev_angles.y, STEP, s.angular_velocity.y));
            s.angles = Vec3::new(0.0, yaw, 0.0);
            if tank {
                // The turret holds its world yaw while the hull turns.
                self.angles2[1] = mad(self.angles2[1], -STEP, s.angular_velocity.y);
            }
            let ax = angles_to_axis(s.angles);
            if g.walkable {
                let t = transform(accel, &ax);
                s.velocity = madv(s.velocity, STEP, t);
                self.ground_friction(tank);
            }
            if self.s.velocity != Vec3::ZERO || !g.walkable {
                let g = self.ground_check(info, world, input.driver.is_some());
                if g.walkable {
                    self.walkable_move(info, world, &g, out);
                } else {
                    self.airborne_move(info, world, &g, out);
                }
            }
            self.s.local_velocity = transpose_transform(self.s.velocity, &ax);
            if info.kind == KIND_FOUR_WHEEL || tank {
                self.suspension(info, world, input.driven);
            }
        }

        let s = &self.s;
        self.speed = s.local_velocity.x.abs();
        let a = s.angular_velocity.y.abs() / (info.rot_rate + info.rot_rate);
        let b = s.local_velocity.x.abs() / info.max_speed;
        let x = if a < b {
            if 1.0 <= b { 1.0 } else { b }
        } else if 1.0 <= a {
            1.0
        } else {
            a
        };
        self.engine_blend[0] = step_prop(1.0 - x, self.engine_blend[0], 4.0);
        self.engine_blend[1] = step_prop(x, self.engine_blend[1], 4.0);
    }

    /// `VEH_PhysicsNotRequired(ent, 1)` (0x87a13).
    fn physics_not_required(&self, neighbour_moving: bool) -> bool {
        let s = &self.s;
        s.angular_velocity == Vec3::ZERO
            && s.velocity == Vec3::ZERO
            && s.sway_pitch == 0.0
            && s.sway_roll == 0.0
            && s.sway_pitch_vel == 0.0
            && s.sway_roll_vel == 0.0
            && s.local_velocity == Vec3::ZERO
            && !neighbour_moving
    }

    /// 0x7d709: the linear acceleration (local frame) and the angular
    /// acceleration; steps the steer and the body sway.
    fn targets(
        &mut self,
        info: &VehicleInfo,
        input: &StepInput,
        f: i8,
        r: i8,
        u: i8,
    ) -> (Vec3, Vec3) {
        let now = input.now;
        let four = info.kind == KIND_FOUR_WHEEL;
        let (mut vmax, acc) = if self.jolt.remaining <= 0.0 || self.lockout_until <= now {
            let mut v = 0.8f32 * info.max_speed;
            if input.closespeed != 0.0 {
                v *= input.closespeed;
            }
            (v, info.accel)
        } else {
            (self.jolt.speed, self.jolt.accel)
        };
        let s = &mut self.s;
        let lv = s.local_velocity;
        let mut tv = Vec3::ZERO;
        tv.x = if f >= 1 {
            lv.x + STEP * acc
        } else if f < 0 {
            let t = lv.x - STEP * acc;
            let floor = 0.5f32 * -vmax;
            if t < floor { floor } else { t }
        } else if four && lv.x != 0.0 {
            if 0.0 < lv.x {
                if 0.0 < lv.x - 10.0 { lv.x - 10.0 } else { 0.0 }
            } else if lv.x + 10.0 < 0.0 {
                lv.x + 10.0
            } else {
                0.0
            }
        } else {
            0.0
        };
        let yr = s.angular_velocity.y;
        tv.y = if four && lv.x.abs() < 400.0 {
            (yr * 0.75 * tv.x.abs() * (1.0 - lv.x.abs() / 400.0)) / 80.0
        } else {
            0.0
        };
        tv.z = lv.z;
        let pf = if s.angles.x < 0.0 {
            1.0 - s.angles.x / -90.0
        } else {
            1.0 + s.angles.x / 90.0
        };
        vmax *= pf;
        if vmax != 0.0 {
            let l = len(tv);
            if l != 0.0 && vmax < l {
                tv = normalize(tv) * vmax;
            }
        }
        let mut accel = (tv - lv) / STEP;
        if acc < len(accel) {
            accel = normalize(accel) * acc;
        }
        if self.wheelbase == 0.0 {
            let fl = self.model.wheels[0].unwrap_or(Vec3::ZERO);
            let bl = self.model.wheels[2].unwrap_or(Vec3::ZERO);
            self.wheelbase = len(fl - bl);
        }

        let (rot, ra) = (info.rot_rate, info.rot_accel);
        let mut aa = Vec3::ZERO;
        if !four {
            if u < 1 {
                let t = if r < 0 {
                    yr + STEP * ra
                } else if r < 1 {
                    0.0
                } else {
                    yr - STEP * ra
                };
                let t = t.clamp(-rot, rot);
                aa.y = ((t - yr) / STEP).clamp(-ra, ra);
            } else {
                let view_yaw = input.driver.map_or(0.0, |d| d.view[1]);
                let d = angle_subtract(view_yaw, s.prev_angles.y);
                let t = ((f64::from(d) / f64::from(STEP)) as f32).clamp(-rot, rot);
                aa.y = (t - yr) / STEP;
            }
        } else {
            let wlen = len(s.velocity);
            let k = 1.0 - wlen / info.max_speed;
            let mut d0 = if 0.0 < k {
                (k * 0.9 + 0.1).clamp(0.0, 1.0)
            } else {
                0.1
            };
            let g = |side: f32, low: f32| {
                if 0.0 < k {
                    let x = k * 0.95 + 0.05;
                    if x < 0.0 {
                        0.0 * side
                    } else if 1.0 < x {
                        60.0 * side
                    } else {
                        x * (60.0 * side)
                    }
                } else {
                    low
                }
            };
            let mut c = if r < 0 {
                g(-1.0, -3.0)
            } else if r < 1 {
                0.0
            } else {
                g(1.0, 3.0)
            };
            match &input.driver {
                Some(d) if u >= 1 => {
                    d0 = 2.0;
                    if r < 0 {
                        c = -60.0;
                    } else if r > 0 {
                        c = 60.0;
                    }
                    let t = angle_subtract(d.view[1] - c, s.prev_angles.y).clamp(-60.0, 60.0);
                    c = t;
                    if f < 0 {
                        let diff = (s.steer - t).abs();
                        if 10.0 < diff {
                            let q = (diff - 10.0) / 60.0;
                            accel.x = if q < 1.0 {
                                accel.x * (1.0 - q)
                            } else {
                                accel.x * 0.0
                            };
                        }
                    } else {
                        c = -t;
                    }
                }
                Some(d) if d.buttons & 0x20 != 0 => {
                    d0 = 2.0;
                    c = if r < 0 {
                        -60.0
                    } else if r < 1 {
                        0.0
                    } else {
                        60.0
                    };
                }
                _ => {}
            }
            if (c < 0.0 && 0.0 < s.steer) || (0.0 < c && s.steer < 0.0) {
                d0 += d0;
            }
            if s.steer < c {
                s.steer = mad(s.steer, 4.0, d0);
                if c < s.steer {
                    s.steer = c;
                }
            } else if c < s.steer {
                s.steer = mad(s.steer, -4.0, d0);
                if s.steer < c {
                    s.steer = c;
                }
            }

            let ax = angles_to_axis(s.angles);
            let t = transform(accel, &ax);
            let mut nv = madv(s.velocity, STEP, t);
            if len(nv) < 10.0 {
                nv = Vec3::ZERO;
            }
            let fwd = dot(ax[0], nv);
            let q0 = fwd.abs() / self.wheelbase;
            let q = if 0.0 < k {
                (f64::from(q0) * (f64::from(0.3f32) + f64::from(0.7f32) * f64::from(k))) as f32
            } else {
                (f64::from(q0) * 0.3) as f32
            };
            let q = q.clamp(0.0, 3.0);
            let t = if 0.0 < fwd {
                -s.steer * q
            } else if fwd < 0.0 {
                s.steer * q
            } else {
                0.0
            };
            let t = if t < -rot * q {
                -rot * q
            } else if rot * q < t {
                rot * q
            } else {
                t
            };
            let mut cc = if s.wheel_surface[..4].iter().all(|&w| w == 0.0) {
                0.0
            } else if len(nv) != 0.0 {
                let x = (fwd / wlen).abs() + (fwd / wlen).abs();
                if x < 2.0 { x } else { 2.0 }
            } else {
                2.0
            };
            if (t - yr).abs() < 10.0 {
                cc = 1.0;
            }
            let a = ((t - yr) * cc) / STEP;
            aa.y = if a < -ra * cc {
                -ra * cc
            } else if ra * cc < a {
                ra * cc
            } else {
                a
            };
        }
        aa.x = ((0.0 - s.angular_velocity.x) / STEP).clamp(-ra, ra);
        aa.z = ((0.0 - s.angular_velocity.z) / STEP).clamp(-ra, ra);

        sway(s, info, f, accel.x);
        (accel, aa)
    }

    /// 0x7fc99: the hull's ground trace, unsticking first when it starts
    /// all in solid.
    fn ground_check(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        driver: bool,
    ) -> Ground {
        let (mins, maxs) = hull(info);
        let o = self.s.origin;
        let start = Vec3::new(o.x, o.y, 0.25 + o.z);
        let end = if driver {
            Vec3::new(o.x, o.y, o.z - 0.25)
        } else {
            Vec3::new(o.x, o.y, o.z - info.suspension_travel)
        };
        let mut tr = world.trace(start, end, mins, maxs, self.clipmask);
        let mut ok = true;
        if tr.allsolid {
            match self.unstick(info, world) {
                Some(t) => tr = t,
                None => ok = false,
            }
        }
        let v = self.s.velocity;
        let touching = ok && tr.fraction != 1.0 && (v.z <= 0.0 || dot(v, tr.normal) <= 10.0);
        Ground {
            touching,
            walkable: touching && tr.normal.z >= 0.7,
            normal: tr.normal,
        }
    }

    /// 0x7faaa: the first of 26 unit offsets the hull fits at, then a unit
    /// down from there.
    fn unstick(&mut self, info: &VehicleInfo, world: &impl VehicleWorld) -> Option<VehTrace> {
        let (mins, maxs) = hull(info);
        let o = self.s.origin;
        let p = UNSTICK
            .iter()
            .map(|d| o + Vec3::from(*d))
            .find(|p| !world.trace(*p, *p, mins, maxs, self.clipmask).startsolid)?;
        let tr = world.trace(p, Vec3::new(p.x, p.y, p.z - 1.0), mins, maxs, self.clipmask);
        self.s.origin = tr.endpos;
        Some(tr)
    }

    /// `VEH_GroundFriction` (0x7fe81).
    fn ground_friction(&mut self, tank: bool) {
        let s = &mut self.s;
        let ax = angles_to_axis(s.angles);
        let l = transpose_transform(s.velocity, &ax);
        let fr = if l.y.abs() < 20.0 { l.y.abs() } else { 20.0 };
        let n = normalize2(s.velocity);
        s.velocity = madv(s.velocity, STEP * -fr, n);
        let l = transpose_transform(s.velocity, &ax);
        if tank && 20.0 < l.y.abs() && l.x.abs() < l.y.abs() {
            let side = if 0.0 < l.y { 20.0 } else { -20.0 };
            let mut v = Vec3::ZERO + ax[0] * l.x;
            v += ax[1] * side;
            v += ax[2] * l.z;
            s.velocity = v;
        }
        if 100.0 < l.x && 1.0 < l.y {
            let sp = len(s.velocity);
            let mut d = ax[0] * sp - s.velocity;
            let mut dl = normalize_in_place(&mut d);
            if 5.0 < dl {
                dl = 5.0;
            }
            s.velocity = madv(s.velocity, dl, d);
            normalize_in_place(&mut s.velocity);
            s.velocity *= sp;
        }
    }

    /// 0x81175: on walkable ground the velocity follows the plane at its
    /// old speed, then steps.
    fn walkable_move(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        g: &Ground,
        out: &mut StepOutput,
    ) {
        let old = self.s.velocity;
        let s0 = len(old);
        let mut v = clip(old, g.normal);
        if 0.0 < dot(v, old) {
            normalize_in_place(&mut v);
            v *= s0;
        }
        self.s.velocity = v;
        if v.x != 0.0 || v.y != 0.0 {
            self.step_slide(info, world, g, false, out);
        }
    }

    /// 0x812c4: off the ground, gravity.
    fn airborne_move(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        g: &Ground,
        out: &mut StepOutput,
    ) {
        if g.touching {
            self.s.velocity = clip(self.s.velocity, g.normal);
        }
        self.step_slide(info, world, g, true, out);
    }

    /// `VEH_SlideMove` (0x8036e), Q3's `PM_SlideMove` with the module's
    /// clip, a horizontal result and four bumps.
    fn slide_move(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        g: &Ground,
        gravity: bool,
        mut touched: Option<&mut Vec<u32>>,
    ) -> bool {
        let (mins, maxs) = hull(info);
        let s = &mut self.s;
        let mut time = STEP;
        let mut end_v = Vec3::ZERO;
        if gravity {
            end_v = Vec3::new(s.velocity.x, s.velocity.y, s.velocity.z - 40.0);
            s.velocity.z = 0.5 * (s.velocity.z + end_v.z);
            if g.touching {
                s.velocity = clip(s.velocity, g.normal);
            }
        }
        let mut planes: Vec<Vec3> = Vec::with_capacity(5);
        if g.touching {
            planes.push(g.normal);
        }
        planes.push(normalize2(s.velocity));
        let mut bump = 0;
        while bump < 4 {
            let end = madv(s.origin, time, s.velocity);
            let tr = world.trace(s.origin, end, mins, maxs, self.clipmask);
            if tr.startsolid {
                s.velocity.z = 0.0;
                return true;
            }
            if 0.0 < tr.fraction {
                s.origin = tr.endpos;
            }
            if tr.fraction == 1.0 {
                break;
            }
            if let Some(list) = touched.as_deref_mut()
                && tr.entity != ENTITYNUM_WORLD
                && list.len() < 4
            {
                list.push(tr.entity);
            }
            time -= time * tr.fraction;
            if planes.len() > 4 {
                s.velocity = Vec3::ZERO;
                return true;
            }
            bump += 1;
            if planes
                .iter()
                .any(|p| f64::from(0.99f32) < dot64(tr.normal, *p))
            {
                s.velocity += tr.normal;
                continue;
            }
            planes.push(tr.normal);
            for i in 0..planes.len() {
                if f64::from(0.1f32) <= dot64(s.velocity, planes[i]) {
                    continue;
                }
                let mut cv = clip(s.velocity, planes[i]);
                let mut ce = clip(end_v, planes[i]);
                for j in 0..planes.len() {
                    if j == i || f64::from(0.1f32) <= dot64(cv, planes[j]) {
                        continue;
                    }
                    cv = clip(cv, planes[j]);
                    ce = clip(ce, planes[j]);
                    if 0.0 <= dot64(cv, planes[i]) {
                        continue;
                    }
                    let mut dir = cross(planes[i], planes[j]);
                    normalize_in_place(&mut dir);
                    let d = dot(dir, s.velocity);
                    cv = dir * d;
                    let d = dot(dir, end_v);
                    ce = dir * d;
                    for (k, pk) in planes.iter().enumerate() {
                        if k != i && k != j && dot64(cv, *pk) < f64::from(0.1f32) {
                            s.velocity = Vec3::ZERO;
                            return true;
                        }
                    }
                }
                s.velocity = Vec3::new(cv.x, cv.y, 0.0);
                end_v = Vec3::new(ce.x, ce.y, 0.0);
                break;
            }
        }
        if gravity {
            s.velocity = end_v;
        }
        bump != 0
    }

    /// 0x80d93, Q3's `PM_StepSlideMove`: slide, and when that hit, try the
    /// same from `stepsize` up, never over a vehicle.
    fn step_slide(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        g: &Ground,
        gravity: bool,
        out: &mut StepOutput,
    ) {
        let (mins, maxs) = hull(info);
        let (start_o, start_v) = (self.s.origin, self.s.velocity);
        let mut touched = Vec::new();
        if !self.slide_move(info, world, g, gravity, Some(&mut touched)) {
            return;
        }
        let down = Vec3::new(start_o.x, start_o.y, start_o.z - info.stepsize);
        let tr = world.trace(start_o, down, mins, maxs, self.clipmask);
        if !(self.s.velocity.z <= 0.0 || (tr.fraction != 1.0 && 0.7 <= tr.normal.z)) {
            return;
        }
        for &e in &touched {
            let hit = world.entity(e);
            if hit.vehicle {
                return;
            }
            if hit.script_model {
                out.touched.push(e);
            }
        }
        let up = Vec3::new(start_o.x, start_o.y, start_o.z + info.stepsize);
        let tr = world.trace(start_o, up, mins, maxs, self.clipmask);
        if tr.startsolid {
            return;
        }
        let up_end = tr.endpos;
        self.s.origin = up_end;
        self.s.velocity = start_v;
        self.slide_move(info, world, g, gravity, None);
        let o = self.s.origin;
        let down = Vec3::new(o.x, o.y, o.z + (start_o.z - up_end.z));
        let tr = world.trace(o, down, mins, maxs, self.clipmask);
        if !tr.startsolid {
            self.s.origin = tr.endpos;
        }
        if tr.fraction < 1.0 {
            self.s.velocity = clip(self.s.velocity, tr.normal);
        }
    }

    /// 0x81331: a ray down under each wheel, the plane through them, pitch
    /// and roll 30% of the way to it, the height from it when undriven;
    /// the angular velocity re-read off the stepped angles.
    fn suspension(&mut self, info: &VehicleInfo, world: &impl VehicleWorld, driven: bool) {
        let n = if info.kind == KIND_FOUR_WHEEL { 4 } else { 6 };
        let s = &mut self.s;
        let t = Vec3::new(s.origin.x, s.origin.y, s.prev_origin.z);
        let ax = angles_to_axis(s.angles);
        let mask = if driven {
            WHEEL_MASK_DRIVEN
        } else {
            WHEEL_MASK
        };
        let mut p = [Vec3::ZERO; 6];
        for (i, pi) in p.iter_mut().enumerate().take(n) {
            let tag = self.model.wheels[i].unwrap_or(Vec3::ZERO);
            let w = transform43(tag, &ax, t);
            let a = Vec3::new(w.x, w.y, 100.0 + w.z);
            let b = Vec3::new(w.x, w.y, w.z - 256.0);
            let tr = world.trace(a, b, Vec3::ZERO, Vec3::ZERO, mask);
            if tr.fraction < 1.0 {
                *pi = tr.endpos;
                s.wheel_surface[i] = ((tr.surface_flags & 0x1f00000) >> 20) as f32;
            } else {
                *pi = b;
                s.wheel_surface[i] = 0.0;
            }
            s.wheel_z[i] = pi.z;
        }
        let half =
            |a: Vec3, b: Vec3| Vec3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z));
        let mut right = half(p[1], p[3]) - half(p[0], p[2]);
        normalize_in_place(&mut right);
        let mut fwd = half(p[0], p[1]) - half(p[2], p[3]);
        normalize_in_place(&mut fwd);
        let nn = cross(right, fwd);
        let mut d = dot(nn, p[0]);
        let st = info.suspension_travel;
        for pi in &p[1..n] {
            let v = (dot64(nn, *pi) - f64::from(d)) as f32;
            if st < v {
                d = (dot64(nn, *pi) - f64::from(st)) as f32;
            }
        }
        let mut a1 = cross(nn, ax[0]);
        normalize_in_place(&mut a1);
        let mut a0 = cross(a1, nn);
        normalize_in_place(&mut a0);
        let ang = crate::game::spawn::axis_to_angles([a0.into(), a1.into(), ax[2].into()]);
        s.angles.x = wrap_prop(ang[0], s.prev_angles.x, 6.0).clamp(-60.0, 60.0);
        s.angles.z = wrap_prop(ang[2], s.prev_angles.z, 6.0).clamp(-60.0, 60.0);
        if !driven {
            s.origin.z = (-(f64::from(s.origin.y) * f64::from(nn.y)
                + f64::from(s.origin.x) * f64::from(nn.x)
                - f64::from(d))
                / f64::from(nn.z)) as f32;
        }
        s.angular_velocity = angles_subtract(s.angles, s.prev_angles) * 20.0;
    }

    /// 0x827ab: true when the step stands. A hit undoes it, bounces the
    /// velocity off the struck surface and kicks the yaw rate.
    fn collide(
        &mut self,
        info: &VehicleInfo,
        world: &impl VehicleWorld,
        input: &StepInput,
        pass: i32,
        out: &mut StepOutput,
    ) -> bool {
        let now = input.now;
        if !input.bmodel {
            return true;
        }
        let c = &self.committed;
        if input.driver.is_none() && self.s.angles == c.angles && self.s.origin == c.origin {
            return true;
        }
        let m = input.bounds_maxs.x.max(0.0).max(input.bounds_maxs.y);
        let bmins = Vec3::new(-(48.0 + m), -(48.0 + m), 0.0);
        let bmaxs = Vec3::new(48.0 + m, 48.0 + m, 16.0 + input.bounds_maxs.z);
        let o = self.s.origin;
        let p = Vec3::new(o.x, o.y, 24.0 + o.z);
        let tr = world.trace(p, p, bmins, bmaxs, self.clipmask | 0x2800000);
        if tr.fraction == 1.0 && !tr.startsolid {
            return true;
        }

        let pmins = Vec3::new(-24.0, -24.0, -1.0);
        let pmaxs = Vec3::new(24.0, 24.0, 10.0);
        let ax = angles_to_axis(self.s.angles);
        let mut clear = true;
        let mut best = 1.0f32;
        let mut least = 1.0f32;
        let mut best_tr: Option<VehTrace> = None;
        let mut best_probe = 0usize;
        let mut leaving = false;
        for (i, off) in PROBES.iter().enumerate() {
            let reach = info.maxs[0] - 24.0;
            let mut v = Vec3::new(off[0] * reach, off[1] * reach, off[2] * reach);
            if 0.0 < off[0] {
                v.x = info.collision_front_dist;
            } else if off[0] < 0.0 {
                v.x = -info.collision_back_dist;
            }
            let lift = 2.0 + info.stepsize;
            let mut delta = Vec3::new(
                (f64::from(lift) * f64::from(ax[2].x)
                    + f64::from(ax[0].x) * f64::from(v.x)
                    + f64::from(ax[1].x) * f64::from(v.y)
                    + f64::from(ax[2].x) * f64::from(v.z)) as f32,
                (f64::from(lift) * f64::from(ax[2].y)
                    + f64::from(ax[0].y) * f64::from(v.x)
                    + f64::from(ax[1].y) * f64::from(v.y)
                    + f64::from(ax[2].y) * f64::from(v.z)) as f32,
                (f64::from(lift) * f64::from(ax[2].z)
                    + f64::from(ax[0].z) * f64::from(v.x)
                    + f64::from(ax[1].z) * f64::from(v.y)
                    + f64::from(ax[2].z) * f64::from(v.z)) as f32,
            );
            let o = self.s.origin;
            let mut start = Vec3::new(o.x, o.y, o.z + info.suspension_travel);
            let mut done = 0.0f32;
            let mut tries = 0;
            let mut no_bodies = 0u32;
            let mut last: VehTrace;
            'probe: loop {
                // The inner climb: bodies struck and passed, walkable world
                // climbed along.
                loop {
                    if 1.0 <= done || tries > 3 {
                        break 'probe;
                    }
                    tries += 1;
                    let end = madv(start, 1.0 - done, delta);
                    let mask = (!no_bodies & (self.clipmask | CONTENTS_BODY)) | CONTENTS_VEHICLE;
                    last = world.trace(start, end, pmins, pmaxs, mask);
                    if 1.0 <= last.fraction {
                        break;
                    }
                    if last.entity < input.max_clients {
                        out.struck_clients.push(ClientStrike {
                            entity: last.entity,
                            vehicle_origin: self.s.origin,
                            vehicle_velocity: self.s.velocity,
                        });
                        no_bodies = CONTENTS_BODY;
                        continue;
                    }
                    if last.entity != ENTITYNUM_WORLD && world.entity(last.entity).script_model {
                        let mut away = normalize2(self.s.velocity);
                        away = -away;
                        self.jolt(info, away, 1.0, 0.0, 700.0, now);
                        self.lockout_until = now + 150;
                        self.s.velocity = Vec3::ZERO;
                        out.touched.push(last.entity);
                        return true;
                    }
                    if last.entity != ENTITYNUM_WORLD
                        || self.s.velocity.z <= -20.0
                        || last.normal.z < 0.7
                    {
                        break;
                    }
                    done += (1.0 - done) * last.fraction;
                    start = last.endpos + last.normal;
                    delta = clip(delta, last.normal);
                }
                if last.fraction == 1.0 {
                    break;
                }
                if last.entity == self.last_entity {
                    let e = world.entity(last.entity).origin - self.s.origin;
                    if dot64(e, self.s.velocity) <= 0.0 {
                        leaving = true;
                        break;
                    }
                }
                if done < least {
                    least = done;
                }
                if done < best {
                    clear = false;
                    best = done;
                    best_tr = Some(last);
                    best_probe = i;
                }
                if last.fraction == 0.0 {
                    break;
                }
            }
        }
        if clear {
            self.last_fraction = least;
            if !leaving && self.last_entity != ENTITYNUM_NONE {
                self.last_entity = ENTITYNUM_NONE;
                self.last_entity_dist = 99999.0;
            }
            return true;
        }
        let tr = best_tr.expect("a hit keeps its trace");
        self.last_fraction = best;
        let struck = (tr.entity != ENTITYNUM_NONE).then(|| world.entity(tr.entity));
        if tr.entity != ENTITYNUM_WORLD {
            self.last_entity = tr.entity;
            self.last_entity_dist = len(self.s.origin - world.entity(tr.entity).origin);
        }
        self.s = self.committed;
        self.last_hit = now;
        self.last_hit_normal = tr.normal;
        let n = tr.normal;
        let yr0 = self.s.angular_velocity.y.abs() / 100.0;
        let k0 = if yr0 < 0.0 { 0.0 } else { yr0 };
        let mut impact = k0;
        let mut into = 0.0f32;
        let wlen = len(self.s.velocity);
        if wlen != 0.0 {
            let r = (wlen / (0.75 * info.max_speed)).clamp(0.0, 1.0);
            let dir = normalize2(self.s.velocity);
            let dn = dot(dir, n);
            into = if dn < 0.0 { -dn * r } else { 0.0 };
            impact = if k0 + into < 0.5 { k0 + into } else { 0.5 };
        }
        if self.jolt.remaining <= 0.0 {
            let dir = normalize2(self.s.velocity);
            let scale = (4.0f32 * impact).clamp(0.2, 0.6);
            let accel = 400.0f32 - 300.0f32 * dot(dir, ax[0]).abs();
            self.jolt(info, n, scale, 0.0, accel, now);
        }

        // Another vehicle's push (`script_vehicle`, scr_const 0x82): the
        // parent owns every other record, so the step hands it the kick
        // and the velocity it leaves. Retail writes the other record at
        // once (0x839aa, 0x83ddd), so a second hit this think reads what
        // the first left.
        let mut pushed = false;
        if let Some(mut e) = struck.filter(|e| e.vehicle)
            && let Some(other_max) = e.vehicle_max_speed
        {
            if let Some(prior) = out.pushes.iter().rev().find(|p| p.entity == tr.entity) {
                e.vehicle_velocity = prior.velocity;
                e.vehicle_jolting |= prior.jolt.is_some();
            }
            let mut push = VehiclePush {
                entity: tr.entity,
                jolt: None,
                velocity: e.vehicle_velocity,
            };
            if !e.vehicle_jolting {
                let d = normalize(e.origin - self.s.origin);
                let scale = (impact + impact).clamp(0.2, 0.5);
                push.jolt = Some((d, scale));
            }
            // The share of the other's top speed this hit hands on: its
            // ratio to ours, times 100, times our speed's share of our own
            // top speed clamped to 0.5..1.
            let k = other_max / info.max_speed * 100.0;
            let share = (len(self.s.velocity) / info.max_speed).clamp(0.5, 1.0);
            if len(e.vehicle_velocity) < k * share {
                let d = normalize(e.origin - self.s.origin);
                let mut v = e.vehicle_velocity + d * (k * share);
                if other_max < len(v) {
                    v = normalize(v) * other_max;
                }
                push.velocity = v;
            }
            // Not moving away from it: one no faster-topped than us takes
            // the closing speed off ours, a faster-topped one stops us
            // (0x84341, 0x84361, the zeroing at 0x848d6). Moving away
            // leaves ours alone.
            if 0.0 <= dot(e.origin - self.s.origin, self.s.velocity) {
                if other_max <= info.max_speed {
                    let rel = len(push.velocity - self.s.velocity);
                    let d = normalize(e.origin - self.s.origin);
                    if 0.0 < dot(self.s.velocity, d) {
                        let cap = info.max_speed / 2.0;
                        self.s.velocity -= d * if cap < rel { cap } else { rel };
                        pushed = true;
                    }
                } else {
                    self.s.velocity = Vec3::ZERO;
                }
            }
            out.pushes.push(push);
        }

        let off = PROBES[best_probe];
        let mut dirv = Vec3::from(off);
        let mut hd = tr.endpos - self.s.origin;
        normalize_in_place(&mut hd);
        let sy = dot64(hd, ax[1]);
        let sx = dot64(hd, ax[0]);
        let (p1, m1) = (f64::from(0.1f32), f64::from(-0.1f32));
        let side = if p1 < sy {
            if sx < m1 {
                1.0
            } else if p1 < sx {
                -1.0
            } else {
                0.0
            }
        } else if sy < m1 {
            if sx < m1 {
                -1.0
            } else if p1 < sx {
                1.0
            } else {
                0.0
            }
        } else {
            0.0f32
        };
        if self.s.velocity != Vec3::ZERO {
            dirv = normalize2(self.s.velocity);
        }
        let ra = info.rot_accel / 70.0;
        let mag = self.s.angular_velocity.y.abs() + 0.5 * len(self.s.velocity);
        let kick = if mag < 0.0 {
            (f64::from(side) * 0.0 * f64::from(ra)) as f32
        } else if 80.0 < mag {
            (f64::from(side) * 80.0 * f64::from(ra)) as f32
        } else {
            side * mag * ra
        };
        if kick != 0.0 {
            let x = (dot(dirv, n).abs() - 0.5).abs();
            self.s.angular_velocity.y = (1.0 - (x + x)) * kick;
        }
        let soft = struck.is_none_or(|e| !e.client)
            && struck.is_none_or(|e| e.vehicle_max_speed.is_none_or(|m| m < info.max_speed));
        if !pushed && soft {
            let v = self.s.velocity;
            let h = len(Vec3::new(v.x, v.y, 0.0));
            let v2 = if 10.0 < h { h } else { 10.0 };
            let mut np = Vec3::new(n.x, n.y, 0.5 * n.z);
            if n.x == 0.0 && n.y == 0.0 && np.z == 0.0 {
                np = -hd;
            }
            if dot64(np, v) < 0.0 {
                let w = clip(v, np);
                let push = 0.85f32 * 150.0f32 * (v2 / info.max_speed);
                let push = if 40.0 < push { push } else { 40.0 };
                self.s.velocity = madv(w, push, np);
            } else {
                let c = v2.clamp(120.0, 160.0);
                self.s.velocity = np * c;
            }
        }
        self.lockout_until = now + 150;
        let mut cap = info.max_speed;
        if input.closespeed != 0.0 {
            cap *= input.closespeed;
        }
        if cap < len(self.s.velocity) {
            normalize_in_place(&mut self.s.velocity);
            self.s.velocity *= cap;
        }
        let mut hit = Collision {
            endpos: tr.endpos,
            normal: n,
            entity: tr.entity,
            notify: None,
            sound: None,
            self_damage: None,
            stuck_kill: false,
        };
        if 0.5 < into {
            hit.self_damage = Some(trunc(
                f64::from((impact - 0.5) + (impact - 0.5)) * f64::from(info.collision_damage),
            ));
        }
        // `G_SpawnVehicle`'s memset leaves the clock 0, so a fresh record's
        // first hit lands here too (0x857dd: level.time + 99999).
        if self.stuck_clock < now - 7000 {
            hit.stuck_kill = true;
            self.stuck_clock = now + 99_999;
        }
        let fresh = now - 200 < self.stuck_clock;
        if fresh && self.last_notify < now - 1000 && 0.25 < impact && len(n) != 0.0 {
            let back = -24.0f32;
            let pos = Vec3::new(
                tr.endpos.x + back * n.x,
                tr.endpos.y + back * n.y,
                tr.endpos.z + back * n.z,
            );
            hit.notify = Some((n, pos));
            self.last_notify = now;
        }
        if fresh && self.last_sound < now - 1000 && !info.sounds[6].is_empty() {
            let z = (2500.0f32 * (1.0 - impact)) + 0.0;
            hit.sound = Some(Vec3::new(tr.endpos.x, tr.endpos.y, tr.endpos.z + z));
            self.last_sound = now;
        }
        if pass > 1 {
            let mut d = self.s.origin - tr.endpos;
            normalize_in_place(&mut d);
            self.s.velocity = d * (info.max_speed / 4.0);
        }
        out.collisions.push(hit);
        false
    }

    /// 0x7d492: a sway kick off `dir` (pitch from its forward part, roll
    /// from its left part, the pair normalized), `2 * scale` seconds long;
    /// while it runs a hit's lockout caps the speed at `speed * speed_scale`
    /// and the accel at `accel`. The main gun fires one along the barrel's
    /// back (0.75, 0, 0), pain and `joltBody` theirs.
    pub fn jolt(
        &mut self,
        info: &VehicleInfo,
        dir: Vec3,
        scale: f32,
        speed_scale: f32,
        accel: f32,
        now: i32,
    ) {
        self.jolt_time = now;
        let sc = scale.clamp(0.0, 1.0);
        let ax = angles_to_axis(self.s.angles);
        let mut p = dot(dir, ax[0]);
        let mut r = -dot(dir, ax[1]);
        let l = (f64::from(p) * f64::from(p) + f64::from(r) * f64::from(r)).sqrt() as f32;
        if l != 0.0 {
            let il = 1.0 / l;
            p *= il;
            r *= il;
        }
        let amp = |v: f32| {
            if 30.0 < v.abs() {
                if 0.0 < v { 30.0 } else { -30.0 }
            } else {
                v
            }
        };
        self.jolt = Jolt {
            pitch: amp(p * info.max_body_pitch * sc),
            roll: amp(r * info.max_body_roll * sc),
            duration: sc + sc,
            remaining: sc + sc,
            phase: 0.0,
            speed: self.speed * speed_scale,
            accel,
        };
    }

    /// 0x85b32: the turret turns toward the driver's view in the swayed
    /// hull's frame at `turretRotRate`, clamped to its arcs.
    fn turret_aim(&mut self, info: &VehicleInfo, input: &StepInput, out: &mut StepOutput) {
        let rate = info.turret.rot_rate;
        if rate == 0.0 && !info.sounds[4].is_empty() {
            // A zero-rate vehicle with a spin alias (the jeep) has a horn.
            if !input.horns {
                return;
            }
            if input.driver.is_some_and(|d| d.buttons & 1 != 0) && !input.alt_overheated {
                self.turret_spin = 2;
                out.horn = true;
                return;
            }
            self.turret_spin = decay(self.turret_spin);
            return;
        }
        let Some(d) = input
            .driver
            .filter(|_| !input.capturing && input.health >= 1 && rate != 0.0)
        else {
            self.turret_spin = decay(self.turret_spin);
            return;
        };
        if !self.model.has_barrel {
            return;
        }
        let l = self.in_hull_frame([d.view[0], d.view[1], 0.0]);
        let cur = Vec3::new(self.angles2[0], self.angles2[1], 0.0);
        let err = angles_subtract(l, cur);
        let (ep, ey) = (err.x.abs(), err.y.abs());
        let sp = wrap_const(l.x, cur.x, rate);
        let sy = wrap_const(l.y, cur.y, rate);
        let t = &info.turret;
        let cp = clamp_lo_hi(sp, -t.vert_up, t.vert_down);
        let cy = if input.harc != 0.0 {
            clamp_lo_hi(sy, -input.harc, input.harc)
        } else {
            clamp_lo_hi(sy, -t.horiz_right, t.horiz_left)
        };
        self.angles2[0] = cp;
        self.angles2[1] = cy;
        let cut = angles_subtract(Vec3::new(sp, sy, 0.0), Vec3::new(cp, cy, 0.0));
        self.turret_spin = if (2.0 <= ep && cut.x == 0.0) || (2.0 <= ey && cut.y == 0.0) {
            2
        } else {
            decay(self.turret_spin)
        };
        if ep < 1.0 && ey < 1.0 {
            out.turret_on_target = true;
        }
    }

    /// 0x86285: the gunner's mount, the same way at `turretGunnerRotRate`;
    /// its yaw is kept off the turret's.
    fn gunner_aim(&mut self, info: &VehicleInfo, input: &StepInput) {
        let Some(v) = input.gunner_view.filter(|_| input.health >= 1) else {
            self.gunner_spin = decay(self.gunner_spin);
            return;
        };
        if !self.model.has_secondary_gun {
            return;
        }
        let l = self.in_hull_frame([v[0], v[1], 0.0]);
        let cur = Vec3::new(self.angles2[2], self.gunner_yaw + self.angles2[1], 0.0);
        let err = angles_subtract(l, cur);
        let (ep, ey) = (err.x.abs(), err.y.abs());
        let g = &info.gunner;
        let sp = wrap_const(l.x, cur.x, g.rot_rate);
        let sy = wrap_const(l.y, cur.y, g.rot_rate);
        let cp = clamp_lo_hi(sp, -g.vert_up, g.vert_down);
        let cy = clamp_lo_hi(sy, -g.horiz_right, g.horiz_left);
        self.angles2[2] = cp;
        self.gunner_yaw = cy - self.angles2[1];
        let cut = angles_subtract(Vec3::new(sp, sy, 0.0), Vec3::new(cp, cy, 0.0));
        self.gunner_spin = if (2.0 <= ep && cut.x == 0.0) || (2.0 <= ey && cut.y == 0.0) {
            2
        } else {
            decay(self.gunner_spin)
        };
    }

    /// A view's angles in the hull's frame, the hull swayed:
    /// `AxisToAngles(axis(view) * transpose(axis(hull)))`.
    fn in_hull_frame(&self, view: [f32; 3]) -> Vec3 {
        let s = &self.s;
        let hull = Vec3::new(
            s.angles.x + s.sway_pitch,
            s.angles.y,
            s.angles.z + s.sway_roll,
        );
        let va = angles_to_axis(Vec3::from(view));
        let ha = angles_to_axis(hull);
        let m = va.map(|row| {
            let r: [f32; 3] = [dot(row, ha[0]), dot(row, ha[1]), dot(row, ha[2])];
            r
        });
        Vec3::from(crate::game::spawn::axis_to_angles(m))
    }

    /// 0x86735: the jolt's sine, then the sway packed into `origin2[0]`
    /// and `[2]`.
    fn pack_sway(&mut self) {
        let (mut jp, mut jr) = (0.0f32, 0.0f32);
        let j = &mut self.jolt;
        if 0.0 < j.remaining {
            // The angle is a double (`fldl` pi, 0x867a8): a half turn's
            // sine is 1.2e-16, which packs to 127.
            let ph = (std::f64::consts::PI * f64::from(j.phase)) / 180.0;
            let k = (ph.sin() as f32) * (j.remaining / j.duration);
            jp = k * j.pitch;
            jr = k * j.roll;
            j.remaining -= STEP;
            if j.remaining < 0.0 {
                j.remaining = 0.0;
            }
            j.phase += 36.0;
        }
        self.sway_packed = [pack(jp + self.s.sway_pitch), pack(jr + self.s.sway_roll)];
    }

    /// 0x86aa1: the tracks' scroll clock off the forward speed, or the yaw
    /// rate on the spot.
    fn track_clock(&mut self, info: &VehicleInfo, input: &StepInput) {
        if !info.texture_scroll {
            self.time = -1;
            return;
        }
        let s = &self.s;
        let v = if s.local_velocity.x != 0.0 {
            s.local_velocity.x
        } else {
            s.angular_velocity.y
        };
        let scale = if 0.0 < input.tex_scroll_scale {
            input.tex_scroll_scale
        } else {
            info.texture_scroll_scale
        };
        let x = (v / 176.0) * STEP * scale;
        self.time = self
            .time
            .wrapping_add(trunc(f64::from(1000.0f32) * f64::from(x)));
    }
}

/// `VEH_PlayerCollision`'s push (0x8213f) of a body at `player_origin`
/// moving at `player_velocity`: its new velocity, the damage and where the
/// `vehicle_hit_person` alias plays. `None` for a vehicle under 0.01 u/s.
/// The parent skips a seated player (`ps.eFlags & 0x100000`), and deals
/// the damage and plays the sound only 500 ms after the last
/// (`client+0x4728`).
pub fn player_push(
    vehicle_origin: Vec3,
    vehicle_velocity: Vec3,
    player_origin: Vec3,
    player_velocity: Vec3,
) -> Option<PlayerPush> {
    let speed = len(vehicle_velocity);
    if speed < 0.01 {
        return None;
    }
    let mut d = player_origin - vehicle_origin;
    normalize_in_place(&mut d);
    d *= speed;
    let dl = len(d);
    let f = if dl < 400.0 { dl / 400.0 } else { 1.0 };
    let damage = if f64::from(0.01f32) < dot64(d, vehicle_velocity) {
        150.0 * f
    } else {
        0.0
    };
    if len(d) < 200.0 {
        normalize_in_place(&mut d);
        d *= 200.0;
    }
    d *= 1.3;
    let mut pv = player_velocity + d;
    let dl = len(d);
    if 1.3 * dl < len(pv) {
        normalize_in_place(&mut pv);
        pv = Vec3::new(pv.x * 1.3 * dl, pv.y * 1.3 * dl, pv.z * 1.3 * dl);
    }
    let r = if dl < 400.0 {
        f64::from(dl / 400.0)
    } else {
        1.0
    };
    pv.z = (200.0 + 100.0 * r * r) as f32;
    Some(PlayerPush {
        velocity: pv,
        damage: trunc(f64::from(damage)),
        sound: Vec3::new(
            player_origin.x,
            player_origin.y,
            player_origin.z + (1500.0f32 * (1.0 - f) + 0.0),
        ),
    })
}

/// What [`player_push`] does to a struck body.
#[derive(Clone, Copy, Debug)]
pub struct PlayerPush {
    pub velocity: Vec3,
    pub damage: i32,
    pub sound: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct Ground {
    touching: bool,
    walkable: bool,
    normal: Vec3,
}

/// 0x7d709's tail: the pitch sway chases the throttle and the
/// acceleration, the roll sway the yaw rate.
fn sway(s: &mut State, info: &VehicleInfo, f: i8, accel_x: f32) {
    let fm = f64::from(f).abs();
    let af = f64::from(info.accel_fwd_roll);
    let mut t = if f < 0 {
        let dv = if fm < 100.0 { (fm / 100.0) * af } else { af };
        (dv / 2.0) as f32
    } else if f == 0 {
        0.0
    } else {
        let m = if fm < 100.0 { -(fm / 100.0) } else { -1.0 };
        ((af * m) / 2.0) as f32
    };
    if accel_x < -10.0 {
        let dv = if fm < 100.0 { af * (fm / 100.0) } else { af };
        t = (f64::from(t) + dv / 2.0) as f32;
    } else if 10.0 < accel_x {
        let m = if fm < 100.0 { -(fm / 100.0) } else { -1.0 };
        t = (f64::from(t) + (af * m) / 2.0) as f32;
    }
    let p = s.sway_pitch;
    let b = 4.0 * (t - p);
    let w = (f64::from(0.2f32) + f64::from(p / info.accel_spring_tension).abs().powi(2)) as f32;
    let w = w.clamp(0.0, 1.0);
    let mut a = (f64::from(w) * f64::from(b)
        + (f64::from(b) + f64::from(s.sway_pitch_vel)) * f64::from(1.0 - w)) as f32;
    if a.abs() < 0.5 && p < 0.5 {
        a = 0.0;
    }
    let pv = s.sway_pitch_vel;
    let dv = a - pv;
    let pv = if dv < -3.0 {
        pv + -3.0
    } else if 3.0 < dv {
        pv + 3.0
    } else {
        pv + dv
    };
    s.sway_pitch_vel = pv.clamp(-16.0, 16.0);
    s.sway_pitch = mad(p, STEP, s.sway_pitch_vel);

    let yr = s.angular_velocity.y;
    let as_ = info.accel_side_roll;
    let mut t = if 0.0 < yr {
        if yr / 40.0 < 1.0 {
            as_ * (yr / 40.0)
        } else {
            as_
        }
    } else if yr < 0.0 {
        if -yr / 40.0 < 1.0 {
            -as_ * (-yr / 40.0)
        } else {
            -as_
        }
    } else {
        0.0
    };
    if f < 0 {
        t = -t;
    } else if f == 0 {
        t = 0.0;
    }
    let g = 4.0 * (t - s.sway_roll);
    let lo = STEP * -info.rot_rate;
    let hi = STEP * info.rot_rate;
    let rv = s.sway_roll_vel;
    s.sway_roll_vel = if g - rv < lo {
        rv + lo
    } else if hi < g - rv {
        rv + hi
    } else {
        rv + (g - rv)
    };
    s.sway_roll = mad(s.sway_roll, STEP, s.sway_roll_vel);
}

/// The info box halved (`inf+0x354`/`0x360`): the hull the slide moves.
fn hull(info: &VehicleInfo) -> (Vec3, Vec3) {
    (Vec3::from(info.half_mins()), Vec3::from(info.half_maxs()))
}

fn decay(spin: i32) -> i32 {
    match spin {
        2 => 1,
        1 => 0,
        s => s,
    }
}

fn clamp_lo_hi(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// `floor(clamp(127 + 127 * deg / 30, 0, 255))`, the sum kept on the x87
/// stack (0x86864): a sway a hair under 0 packs 126, where an f32 sum
/// rounds to 127.
fn pack(deg: f32) -> i32 {
    let x = 127.0 + (127.0 * f64::from(deg)) / 30.0;
    let c = if x <= 0.0 || x < 255.0 {
        if 0.0 < x { x } else { 0.0 }
    } else {
        255.0
    };
    c.floor() as i32
}

/// A `fistp` under a `| 0xc00` control word: toward zero, out of range
/// 0x80000000.
fn trunc(x: f64) -> i32 {
    if x.abs() < 2147483648.0 {
        x as i32
    } else {
        i32::MIN
    }
}

/// `AngleNormalize180` (0x3dcdf): through a 16-bit angle truncated off an
/// unrounded product, so every step quantizes.
pub fn an180(a: f32) -> f32 {
    let units = trunc(f64::from(a) * f64::from(65536.0f32 / 360.0)) & 0xffff;
    let a = units as f32 * (360.0 / 65536.0);
    if 180.0 < a { a - 360.0 } else { a }
}

/// `AngleSubtract` (0x3dba0): wrapped by repeated 360s.
pub fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while 180.0 < d {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

fn angles_subtract(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        angle_subtract(a.x, b.x),
        angle_subtract(a.y, b.y),
        angle_subtract(a.z, b.z),
    )
}

/// 0x7c525: `c` a fixed `0.05 * rate` toward `t`, landing on it.
fn step_const(t: f32, c: f32, rate: f32) -> f32 {
    let d = t - c;
    let rate = if d <= 0.0 { -rate } else { rate };
    let step = STEP * rate;
    if 0.001f32 < d.abs() {
        if d.abs() < step.abs() { t } else { c + step }
    } else {
        t
    }
}

/// 0x7c5c5: `c` `0.05 * rate` of the gap toward `t`.
fn step_prop(t: f32, c: f32, rate: f32) -> f32 {
    let d = t - c;
    let step = STEP * rate * d;
    if 0.001f32 < d.abs() {
        if d.abs() < step.abs() { t } else { c + step }
    } else {
        t
    }
}

fn wrap(mut t: f32, c: f32) -> f32 {
    while 180.0 < t - c {
        t -= 360.0;
    }
    while t - c < -180.0 {
        t += 360.0;
    }
    t
}

/// 0x7c63c: [`step_const`] on angles, quantized.
fn wrap_const(t: f32, c: f32, rate: f32) -> f32 {
    an180(step_const(wrap(t, c), c, rate))
}

/// 0x7c6cf: [`step_prop`] on angles, quantized.
fn wrap_prop(t: f32, c: f32, rate: f32) -> f32 {
    an180(step_prop(wrap(t, c), c, rate))
}

/// The module's clip (0x7f932): on steep planes, or for a velocity more
/// vertical than not, Q3's overbounce 1.01 (divided when leaving the plane);
/// on walkable ground the horizontal direction is kept.
fn clip(v: Vec3, n: Vec3) -> Vec3 {
    let h = f64::from(v.y) * f64::from(v.y) + f64::from(v.x) * f64::from(v.x);
    if n.z < 0.7 || h < f64::from(v.z) * f64::from(v.z) {
        let mut b = dot(v, n);
        b = if b < 0.0 { 1.01 * b } else { b / 1.01 };
        Vec3::new(v.x - n.x * b, v.y - n.y * b, v.z - n.z * b)
    } else {
        Vec3::new(
            v.x * n.z,
            v.y * n.z,
            (-f64::from(v.x) * f64::from(n.x) - f64::from(v.y) * f64::from(n.y)) as f32,
        )
    }
}

/// `a + k * b`, rounded once as the x87 stores it.
fn mad(a: f32, k: f32, b: f32) -> f32 {
    (f64::from(a) + f64::from(k) * f64::from(b)) as f32
}

fn madv(a: Vec3, k: f32, b: Vec3) -> Vec3 {
    Vec3::new(mad(a.x, k, b.x), mad(a.y, k, b.y), mad(a.z, k, b.z))
}

fn dot64(a: Vec3, b: Vec3) -> f64 {
    f64::from(a.z) * f64::from(b.z)
        + f64::from(a.y) * f64::from(b.y)
        + f64::from(a.x) * f64::from(b.x)
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    dot64(a, b) as f32
}

/// `sqrt` of a sum left on the stack, the inline length the module writes
/// everywhere.
fn len(v: Vec3) -> f32 {
    dot64(v, v).sqrt() as f32
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    let (a, b) = (a.as_dvec3(), b.as_dvec3());
    DVec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
    .as_vec3()
}

/// `VectorNormalize` (0x3a1c3): the squared length stored as a float, its
/// root as a float, the reciprocal as a float; a zero vector left alone.
fn normalize_in_place(v: &mut Vec3) -> f32 {
    let l = (f64::from(dot64(*v, *v) as f32)).sqrt() as f32;
    if l != 0.0 {
        let il = 1.0f32 / l;
        *v = Vec3::new(v.x * il, v.y * il, v.z * il);
    }
    l
}

fn normalize(v: Vec3) -> Vec3 {
    let mut v = v;
    normalize_in_place(&mut v);
    v
}

/// `VectorNormalize2` (0x3a466): a zero vector comes out zero.
fn normalize2(v: Vec3) -> Vec3 {
    let mut out = v;
    if normalize_in_place(&mut out) == 0.0 {
        return Vec3::ZERO;
    }
    out
}

/// `MatrixTransformVector`: `v`'s components scale the axes.
fn transform(v: Vec3, ax: &[Vec3; 3]) -> Vec3 {
    let (v, a0, a1, a2) = (
        v.as_dvec3(),
        ax[0].as_dvec3(),
        ax[1].as_dvec3(),
        ax[2].as_dvec3(),
    );
    (a0 * v.x + a1 * v.y + a2 * v.z).as_vec3()
}

/// `MatrixTransformVector43`: [`transform`] plus a translation.
fn transform43(v: Vec3, ax: &[Vec3; 3], t: Vec3) -> Vec3 {
    let (v, a0, a1, a2) = (
        v.as_dvec3(),
        ax[0].as_dvec3(),
        ax[1].as_dvec3(),
        ax[2].as_dvec3(),
    );
    (a0 * v.x + a1 * v.y + a2 * v.z + t.as_dvec3()).as_vec3()
}

/// `MatrixTransposeTransformVector`: `v` in the axes' frame.
fn transpose_transform(v: Vec3, ax: &[Vec3; 3]) -> Vec3 {
    Vec3::new(dot(v, ax[0]), dot(v, ax[1]), dot(v, ax[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The yaw truncates to 65536ths, toward zero: a coasting hull's rate
    /// bleeds on it (physics.md 3.5).
    #[test]
    fn an180_truncates_to_16_bit_units() {
        let unit = 360.0 / 65536.0;
        assert_eq!(an180(unit * 1.99), unit);
        assert_eq!(an180(-unit * 1.99), -unit);
        assert!((an180(190.0) + 170.0).abs() < 0.01);
        assert!(an180(179.9) > 179.8 && an180(181.0) < -178.9);
    }

    #[test]
    fn the_sway_packs_to_a_byte_around_127() {
        assert_eq!(pack(0.0), 127);
        assert_eq!(pack(-30.0), 0);
        assert_eq!(pack(-31.0), 0);
        assert_eq!(pack(30.0), 254);
        assert_eq!(pack(40.0), 255);
        assert_eq!(pack(-0.3), 125);
        assert_eq!(pack(-5e-7), 126);
    }

    /// The constant-rate chase lands on the target inside a step and
    /// wraps across 180.
    #[test]
    fn the_turret_chase_steps_at_its_rate_and_lands() {
        assert_eq!(step_const(10.0, 0.0, 30.0), 1.5);
        assert_eq!(step_const(1.0, 0.0, 30.0), 1.0);
        let y = wrap_const(-179.0, 179.0, 30.0);
        assert!((y - (-179.5)).abs() < 0.01, "{y}");
    }

    /// On walkable ground the clip keeps the horizontal heading and puts the
    /// vertical on the plane.
    #[test]
    fn the_walkable_clip_keeps_the_heading() {
        let n = Vec3::new(0.6, 0.0, 0.8);
        let c = clip(Vec3::new(10.0, 5.0, 0.0), n);
        assert_eq!(c.y / c.x, 0.5);
        assert!(dot64(c, n).abs() < 1e-5);
    }

    /// A flat floor at z 0 for the drive tests.
    struct Floor;
    impl VehicleWorld for Floor {
        fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, _maxs: Vec3, _mask: u32) -> VehTrace {
            let lo = |p: Vec3| p.z + mins.z;
            let miss = VehTrace {
                fraction: 1.0,
                endpos: end,
                normal: Vec3::ZERO,
                surface_flags: 0,
                startsolid: false,
                allsolid: false,
                entity: ENTITYNUM_NONE,
            };
            if lo(start) < 0.0 {
                return VehTrace {
                    fraction: 0.0,
                    endpos: start,
                    startsolid: true,
                    allsolid: lo(end) < 0.0,
                    entity: ENTITYNUM_WORLD,
                    ..miss
                };
            }
            if lo(end) >= 0.0 {
                return miss;
            }
            let f = lo(start) / (lo(start) - lo(end));
            VehTrace {
                fraction: f,
                endpos: start + (end - start) * f,
                normal: Vec3::Z,
                entity: ENTITYNUM_WORLD,
                surface_flags: 1 << 20,
                ..miss
            }
        }
        fn entity(&self, _num: u32) -> HitEntity {
            HitEntity::default()
        }
    }

    /// [`Floor`] with another vehicle's hull as a wall at x 400, entity
    /// 300, met under the vehicle bit.
    struct FloorAndTank;
    impl VehicleWorld for FloorAndTank {
        fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> VehTrace {
            let mut t = Floor.trace(start, end, mins, maxs, mask);
            let (a, b) = (start.x + maxs.x, end.x + maxs.x);
            if mask & CONTENTS_VEHICLE != 0 && start == end && a >= 400.0 {
                return VehTrace {
                    fraction: 0.0,
                    endpos: start,
                    startsolid: true,
                    entity: 300,
                    ..t
                };
            }
            if mask & CONTENTS_VEHICLE != 0 && a < 400.0 && b >= 400.0 {
                let f = (400.0 - a) / (b - a);
                if f < t.fraction {
                    t = VehTrace {
                        fraction: f,
                        endpos: start + (end - start) * f,
                        normal: Vec3::NEG_X,
                        surface_flags: 0,
                        startsolid: false,
                        allsolid: false,
                        entity: 300,
                    };
                }
            }
            t
        }
        fn entity(&self, num: u32) -> HitEntity {
            if num != 300 {
                return HitEntity::default();
            }
            HitEntity {
                origin: Vec3::new(470.0, 0.0, 0.125),
                vehicle: true,
                vehicle_max_speed: Some(panzer().max_speed),
                ..HitEntity::default()
            }
        }
    }

    /// A tank driven into a parked one of its own kind pushes it: the
    /// struck one is kicked and handed a velocity away from the driver,
    /// and the driver's own is cut by the closing speed
    /// (0x827ab's `script_vehicle` arm).
    #[test]
    fn a_tank_pushes_the_tank_it_drives_into() {
        let info = panzer();
        let mut p = Physics::spawn(Vec3::new(0.0, 0.0, 0.125), Vec3::ZERO, wheels());
        p.init(&info, &FloorAndTank, false);
        let drive = DriveInput {
            forward: 127,
            ..DriveInput::default()
        };
        let mut push = None;
        for k in 0..60 {
            let input = StepInput {
                now: 1000 + 50 * k,
                driver: Some(drive),
                driven: true,
                bmodel: true,
                bounds_maxs: Vec3::new(70.0, 70.0, 120.0),
                ..StepInput::default()
            };
            let out = p.think(&info, &FloorAndTank, &input);
            if let Some(&first) = out.pushes.first() {
                push = Some((first, p.s.velocity));
                break;
            }
        }
        let (push, ours) = push.expect("the probes met the parked tank");
        assert_eq!(push.entity, 300);
        let (dir, scale) = push.jolt.expect("a parked tank is kicked");
        assert!(
            dir.x > 0.99 && (0.2..=0.5).contains(&scale),
            "{dir} {scale}"
        );
        assert!(push.velocity.x > 0.0, "{:?}", push.velocity);
        assert!(ours.x < 0.8 * info.max_speed, "{ours}");
    }

    /// [`Floor`] with another vehicle's hull, entity 300, all round the
    /// spawn: every trace under the vehicle bit starts in it.
    struct FloorInTank;
    impl VehicleWorld for FloorInTank {
        fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> VehTrace {
            let t = Floor.trace(start, end, mins, maxs, mask);
            if mask & CONTENTS_VEHICLE == 0 {
                return t;
            }
            VehTrace {
                fraction: 0.0,
                endpos: start,
                startsolid: true,
                entity: 300,
                ..t
            }
        }
        fn entity(&self, num: u32) -> HitEntity {
            FloorAndTank.entity(num)
        }
    }

    /// A wreck's duplicate: spawned where the burning tank stands, it
    /// settles at init, so its first think fails the "nothing moved" test
    /// and its probes play the impact alias about 2500 up; unlinked
    /// changes nothing, no collmap skips the probes (coduo-vehicles.md
    /// 15.2).
    #[test]
    fn a_settled_vehicle_inside_another_plays_the_impact_once() {
        let mut info = panzer();
        info.sounds[6] = "vehicle_impact".into();
        let run = |bmodel: bool| {
            let mut p = Physics::spawn(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, wheels());
            p.init(&info, &Floor, false);
            (0..3)
                .map(|k| {
                    let input = StepInput {
                        now: 43400 + 50 * k,
                        bmodel,
                        bounds_maxs: Vec3::new(70.0, 70.0, 120.0),
                        ..StepInput::default()
                    };
                    p.think(&info, &FloorInTank, &input)
                        .collisions
                        .iter()
                        .filter_map(|c| c.sound)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let sounds = run(true);
        assert_eq!(sounds[0].len(), 1, "{sounds:?}");
        assert!(sounds[0][0].z > 2400.0, "{sounds:?}");
        assert!(sounds[1..].iter().all(Vec::is_empty), "{sounds:?}");
        assert!(run(false).iter().all(Vec::is_empty));
    }

    /// [`FloorAndTank`] with a struck vehicle twice our top speed.
    struct FloorAndFaster;
    impl VehicleWorld for FloorAndFaster {
        fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> VehTrace {
            FloorAndTank.trace(start, end, mins, maxs, mask)
        }
        fn entity(&self, num: u32) -> HitEntity {
            HitEntity {
                vehicle_max_speed: Some(2.0 * panzer().max_speed),
                ..FloorAndTank.entity(num)
            }
        }
    }

    /// Driving into a faster-topped vehicle stops ours dead: the hit's
    /// think ends with no more than the retry's one step of accel
    /// (0x848d6), where one no faster-topped only takes the closing speed.
    #[test]
    fn a_vehicle_driven_into_a_faster_topped_one_stops() {
        let info = panzer();
        let mut p = Physics::spawn(Vec3::new(0.0, 0.0, 0.125), Vec3::ZERO, wheels());
        p.init(&info, &FloorAndFaster, false);
        let drive = DriveInput {
            forward: 127,
            ..DriveInput::default()
        };
        let mut hit = None;
        for k in 0..60 {
            let input = StepInput {
                now: 1000 + 50 * k,
                driver: Some(drive),
                driven: true,
                bmodel: true,
                bounds_maxs: Vec3::new(70.0, 70.0, 120.0),
                ..StepInput::default()
            };
            let before = p.s.velocity;
            let out = p.think(&info, &FloorAndFaster, &input);
            if !out.pushes.is_empty() {
                hit = Some((before, p.s.velocity));
                break;
            }
        }
        let (before, after) = hit.expect("the probes met the vehicle");
        assert!(before.x > 100.0, "{before}");
        assert!(len(after) <= STEP * info.accel, "{before} -> {after}");
    }

    fn panzer() -> VehicleInfo {
        let mut i = VehicleInfo {
            kind: KIND_TANK,
            texture_scroll: true,
            texture_scroll_scale: 5.5,
            max_speed: 15.0 * super::super::info::MPH,
            accel: 12.55 * super::super::info::MPH,
            rot_rate: 50.0,
            rot_accel: 70.0,
            suspension_travel: 7.0,
            accel_fwd_roll: 2.5,
            accel_side_roll: 2.5,
            accel_spring_tension: 1.2,
            stepsize: 42.0,
            collision_front_dist: 105.0,
            collision_back_dist: 112.0,
            mins: [-70.0, -70.0, 0.0],
            maxs: [70.0, 70.0, 120.0],
            ..VehicleInfo::default()
        };
        i.turret.rot_rate = 30.0;
        i
    }

    fn wheels() -> VehicleModel {
        let w = |x: f32, y: f32| Some(Vec3::new(x, y, 0.0));
        VehicleModel {
            wheels: [
                w(80.0, 40.0),
                w(80.0, -40.0),
                w(-80.0, 40.0),
                w(-80.0, -40.0),
                w(0.0, 40.0),
                w(0.0, -40.0),
            ],
            has_barrel: true,
            has_secondary_gun: false,
        }
    }

    /// A think whose retry strikes the same vehicle again reads the record
    /// the first hit wrote (retail writes it at once, 0x839aa and 0x83ddd):
    /// the kick still runs, so no second one, and a velocity under the
    /// share gains the share again.
    #[test]
    fn a_second_hit_in_one_think_builds_on_the_first_push() {
        let info = panzer();
        let mut p = Physics::spawn(Vec3::new(0.0, 0.0, 0.125), Vec3::ZERO, wheels());
        p.init(&info, &FloorAndTank, false);
        let drive = DriveInput {
            forward: 127,
            ..DriveInput::default()
        };
        let mut doubles = Vec::new();
        for k in 0..80 {
            let input = StepInput {
                now: 1000 + 50 * k,
                driver: Some(drive),
                driven: true,
                bmodel: true,
                bounds_maxs: Vec3::new(70.0, 70.0, 120.0),
                ..StepInput::default()
            };
            let out = p.think(&info, &FloorAndTank, &input);
            if let [first, second] = out.pushes[..] {
                doubles.push((first, second));
            }
        }
        assert!(!doubles.is_empty(), "no think struck the tank twice");
        for (first, second) in &doubles {
            assert!(
                first.jolt.is_some() && second.jolt.is_none(),
                "{first:?} {second:?}"
            );
        }
        assert!(
            doubles
                .iter()
                .any(|(a, b)| b.velocity.x > 1.5 * a.velocity.x),
            "{doubles:?}"
        );
    }

    /// From rest under full throttle the Panzer gains `0.05 * accel` a
    /// step up to `0.8 * maxSpeed` (211.2), and the tracks scroll
    /// `trunc(1.5625 * v)` a step (physics.md 3.2, entity `time`).
    #[test]
    fn full_throttle_ramps_to_four_fifths_of_max_speed() {
        let info = panzer();
        let mut p = Physics::spawn(Vec3::new(0.0, 0.0, 0.125), Vec3::ZERO, wheels());
        p.init(&info, &Floor, false);
        let drive = DriveInput {
            forward: 127,
            ..DriveInput::default()
        };
        let mut speeds = Vec::new();
        let mut time = 0;
        for k in 0..25 {
            let input = StepInput {
                now: 1000 + 50 * k,
                driver: Some(drive),
                driven: true,
                ..StepInput::default()
            };
            p.think(&info, &Floor, &input);
            assert_eq!(
                p.time - time,
                trunc(
                    f64::from(1000.0f32) * f64::from((p.s.local_velocity.x / 176.0) * STEP * 5.5)
                )
            );
            time = p.time;
            speeds.push(p.speed);
        }
        assert!((speeds[0] - 11.044).abs() < 1e-3, "{speeds:?}");
        assert!((speeds[5] - 6.0 * 11.044).abs() < 1e-2, "{speeds:?}");
        assert!(
            (speeds.last().unwrap() - 0.8f32 * info.max_speed).abs() < 1e-3,
            "{speeds:?}"
        );
        assert_eq!(p.time2, 7000);
    }
}
