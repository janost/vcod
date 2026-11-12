//! CoD4 1.7's grenades on a [`Cod4Level`] (docs/research/cod4-combat.md
//! 10): `G_UseOffHand`'s throw and the grenade launcher's shot
//! (`Weapon_Throw_Grenade`, `Weapon_GrenadeLauncher_Fire`, `G_FireGrenade`),
//! `G_RunMissile`'s gravity arc through the clip map and the bodies' hit
//! boxes, `MissileImpact` and `BounceMissile`'s bounce and rest by the
//! weapon's surface factors, the fuse's `G_ExplodeMissile` (its event
//! entity, `G_RadiusDamage` with its five line-of-sight traces) and an
//! impact grenade's burst. `cod4::wire_ab`'s frag
//! rows compare the walk capture's throw.
//!
//! The flashbang's burst notifies `flashbang` on each player it reaches
//! (`G_FlashbangBlast`).
//!
//! The rockets, a player's and a helicopter's (`G_FireRocket`), fly the
//! same move and impact through the missile trajectory step's acceleration,
//! curvature and guidance, and an unguided one wobbles and bends to the
//! attractors (docs/research/cod4-gsc.md 3.2, "Missiles under 1.7").
//!
//! Left out: water (`missileWaterMaxDepth`, the splash), glass, the live
//! grenade's pickup (the live grenade pickup and the throw back), damage
//! to entities other than players, Javelin's climb, and the actors'
//! grenade danger.

use glam::Vec3;
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::pmove::weapon::{WEAPTYPE_GRENADE, WeaponDef};
use vcod_common::rng::GlibcRand;
use vcod_gsc::{EntId, Value};

use super::clip::Cod4Clip;
use super::combat::{BULLET_PRIORITY, WEAPTYPE_PROJECTILE};
use super::level::{Cod4Level, PsExt};

/// `trType`: still, straight (the spin) and the arc.
pub const TR_STATIONARY: u32 = 0;
pub const TR_LINEAR: u32 = 2;
pub const TR_GRAVITY: u32 = 5;

/// `ENTITYNUM_NONE` and `ENTITYNUM_WORLD`.
const NONE: u32 = 1023;
const WORLD: u32 = 1022;
/// The grenade's clip mask (`G_InitGrenadeEntity`, MP).
const MASK_GRENADE: u32 = 0x280_6891;
/// `CanDamage`'s mask and the explode's ground trace's.
pub(super) const MASK_RADIUS_LOS: u32 = 0x80_2011;
/// The flashbang's `CanDamage` mask (the flashbang blast).
const MASK_FLASH_LOS: u32 = 0x801;
const MASK_EXPLODE_DOWN: u32 = 0x811;
/// `SURF_NOIMPACT`: a missile meeting it is freed without a burst.
const SURF_NOIMPACT: u32 = 4;
/// `g_minGrenadeDamageSpeed`.
const MIN_GRENADE_DAMAGE_SPEED: f32 = 400.0;
/// The `nextthink` `G_ExplodeMissile` gives a smoke grenade still in the
/// air: 50 itself, not `level.time + 50`, so it runs again next frame
/// whatever `sv_fps` (0x80940f1).
const SMOKE_RETRY_THINK: i32 = 50;
/// `EVENT_VALID_MSEC`: an event entity lives this past its event.
const EVENT_VALID_MS: i32 = 300;

/// The events this module raises, 1.7's numbers (cod4-netfields.md 9).
pub mod ev {
    pub const GRENADE_BOUNCE: u32 = 44;
    pub const GRENADE_EXPLODE: u32 = 45;
    pub const ROCKET_EXPLODE: u32 = 46;
    pub const ROCKET_EXPLODE_NOMARKS: u32 = 47;
    pub const FLASHBANG_EXPLODE: u32 = 48;
    pub const CUSTOM_EXPLODE: u32 = 49;
    pub const CUSTOM_EXPLODE_NOMARKS: u32 = 50;
    pub const CHANGE_TO_DUD: u32 = 51;
    pub const DUD_EXPLODE: u32 = 52;
}

/// `weapProjExposion_t`: 0 grenade, 1 rocket, 2 flashbang, 3
/// none, 4 dud, 5 smoke, 6 heavy.
const PROJEXP_FLASHBANG: i32 = 2;
const PROJEXP_DUD: i32 = 4;
const PROJEXP_SMOKE: i32 = 5;
/// `OFFHAND_CLASS_SMOKE_GRENADE`.
const OFFHAND_SMOKE: i32 = 2;

/// `trajectory_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Traj {
    pub tr_type: u32,
    pub time: i32,
    pub base: Vec3,
    pub delta: Vec3,
}

impl Traj {
    /// `BG_EvaluateTrajectory` (0x80536ba): the time `0.001f` times the
    /// ms, every axis `delta t + base` on the stack, gravity's `-400 t t`
    /// added to the kept z.
    pub fn at(&self, t: i32) -> Vec3 {
        let dt = MS * f64::from(t - self.time);
        let lin = |b: f32, d: f32| f64::from(d) * dt + f64::from(b);
        match self.tr_type {
            TR_LINEAR => mad(self.base, dt, self.delta),
            TR_GRAVITY => Vec3::new(
                lin(self.base.x, self.delta.x) as f32,
                lin(self.base.y, self.delta.y) as f32,
                (lin(self.base.z, self.delta.z) + -400.0 * dt * dt) as f32,
            ),
            _ => self.base,
        }
    }

    /// `BG_EvaluateTrajectoryDelta` (0x8053124): gravity's `-800 t` on
    /// the stack.
    pub fn delta_at(&self, t: i32) -> Vec3 {
        match self.tr_type {
            TR_LINEAR => self.delta,
            TR_GRAVITY => {
                let dt = MS * f64::from(t - self.time);
                let mut r = self.delta;
                r.z = (dt * -800.0 + f64::from(r.z)) as f32;
                r
            }
            _ => Vec3::ZERO,
        }
    }
}

/// `G_rand`: glibc's `rand()` (`cod4_lnxded` calls it through the PLT at
/// 0x804aa5c), 0..2^31, off the level's one stream
/// ([`super::Cod4HostState::rand`], cod4-combat.md 10.9).
fn g_rand(r: &mut GlibcRand) -> i32 {
    r.next_u31() as i32
}

/// `G_flrand` (0x80cb548): `(max - min) rand() * -1/2^31 + min` on the
/// stack (the float at 0x8214ac4 is negative), so a draw falls in
/// `(2 min - max, min]`, never past `min`.
pub(super) fn g_flrand(r: &mut GlibcRand, min: f32, max: f32) -> f32 {
    let scale = f64::from(-1.0f32 / 2_147_483_648.0);
    ((f64::from(max) - f64::from(min)) * f64::from(g_rand(r)) * scale + f64::from(min)) as f32
}

/// `G_irand` (0x80cb522): `min + rand() (max - min) / (int)0x80000000` in
/// 32-bit ints, the product wrapping: `G_irand(0, 2)` is 0 but for a
/// draw of 2^30.
fn g_irand(r: &mut GlibcRand, min: i32, max: i32) -> i32 {
    min + g_rand(r).wrapping_mul(max - min) / i32::MIN
}

/// `G_random` (0x80ca2ee): `rand() / 2^31`, kept on the stack.
fn g_random(r: &mut GlibcRand) -> f64 {
    f64::from(g_rand(r)) * f64::from(1.0f32 / 2_147_483_648.0)
}

/// The trajectories' seconds a ms, the float 0.001 (0x8209640).
const MS: f64 = 0.001f32 as f64;

/// `Vec3Mad` with the scale kept on the stack.
fn mad(a: Vec3, s: f64, b: Vec3) -> Vec3 {
    Vec3::new(
        (f64::from(a.x) + s * f64::from(b.x)) as f32,
        (f64::from(a.y) + s * f64::from(b.y)) as f32,
        (f64::from(a.z) + s * f64::from(b.z)) as f32,
    )
}

/// One grenade in flight or at rest: its script entity, the thrower's
/// slot and the entity state the wire carries.
#[derive(Clone, Debug)]
pub struct Missile4 {
    pub id: EntId,
    pub weapon: u32,
    /// `r.ownerNum` and `parent`: the thrower's slot.
    pub owner: Option<usize>,
    pub pos: Traj,
    pub apos: Traj,
    /// `r.currentOrigin`, `r.currentAngles`.
    pub origin: Vec3,
    pub angles: Vec3,
    pub launch_time: i32,
    pub ground: u32,
    pub surf_type: u32,
    pub surface_normal: Vec3,
    pub e_flags: u32,
    pub next_think: i32,
    pub birth: i32,
    /// `missile.travelDist`, for an impact grenade's arming distance.
    pub travel: f32,
    /// A frag is also a pickup item: `index` the item, `clientNum` 64.
    pub pickup: bool,
    pub event_seq: u32,
    pub events: [u32; 4],
    pub parms: [u32; 4],
    /// `freeAfterEvent`: an impact's burst rides this entity until then.
    pub free_at: Option<i32>,
    /// `missile.team`: the thrower's `sessionteam` at the throw, the
    /// flashbang's fourth `flashbang` argument.
    pub team: String,
    /// `G_FireRocket`'s missile (handler 9): it flies by
    /// The missile trajectory step, and its hit and splash read `MOD_PROJECTILE`
    /// and `MOD_PROJECTILE_SPLASH`.
    pub rocket: bool,
    /// The vehicle that fired it (`r.ownerNum`), which its move passes.
    pub launcher: Option<EntId>,
    /// The entity's own flags (+0x180): [`FL_WOBBLING`], [`FL_STEADY`].
    pub flags: u32,
    /// `missile.time` (+0x1b4): ms from `pos.trTime` to the next wobble.
    pub wait: f32,
    /// +0x1d0: a curving rocket's curvature, else the last wobble's angle
    /// rates.
    pub curve: Vec3,
    /// `missileTargetEnt` and `missile.targetOffset`: what a guided
    /// rocket steers at.
    pub target: Option<EntId>,
    pub target_offset: Vec3,
    /// A Hellfire's climb is over (`pos.trDuration` 1, 0x8096a62).
    pub climbed: bool,
}

/// The missile wobbles (`G_RunMissile`, 0x8097ad7).
pub const FL_WOBBLING: u32 = 0x1_0000;
/// Neither wobble nor attractors: a weapon with no
/// `destabilizationRateTime`, or a parent with the flag (0x80970fb).
pub const FL_STEADY: u32 = 0x2_0000;
/// `TR_INTERPOLATE`: a wobbling, curving or guided rocket's trajectories,
/// which the missile trajectory step steps itself.
pub const TR_INTERPOLATE: u32 = 1;
/// `MAX_MISSILE_ATTRACTORS`.
pub const MAX_ATTRACTORS: usize = 32;

/// What fires a rocket ([`Cod4Level::g_fire_rocket`]): the parent's slot
/// and vehicle, whether a client fired it, the team it carries, where it
/// starts and along what, the gun's velocity, and a guided rocket's target
/// and offset.
pub struct RocketShot {
    pub owner: Option<usize>,
    pub launcher: Option<EntId>,
    pub client: bool,
    pub team: String,
    pub start: Vec3,
    pub dir: Vec3,
    pub gun_vel: Vec3,
    pub target: Option<EntId>,
    pub target_offset: Vec3,
}

/// One of the 32 attractor slots (0x8361a80, 0x1c bytes each): an
/// attractor or a repulsor, on an entity's `r.currentOrigin` or a point,
/// its strength and reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attractor {
    pub attract: bool,
    pub entity: Option<EntId>,
    pub point: Vec3,
    pub strength: f32,
    pub reach: f32,
}

impl Missile4 {
    /// A missile just fired at `now` from `start`, still, unflagged and
    /// drawn at once; its launcher sets the rest.
    fn fresh(
        id: EntId,
        weapon: u32,
        owner: Option<usize>,
        start: Vec3,
        now: i32,
        team: String,
    ) -> Self {
        Self {
            id,
            weapon,
            owner,
            pos: Traj {
                tr_type: TR_STATIONARY,
                time: now,
                base: start,
                delta: Vec3::ZERO,
            },
            apos: Traj::default(),
            origin: start,
            angles: Vec3::ZERO,
            launch_time: now,
            ground: NONE,
            surf_type: 0,
            surface_normal: Vec3::ZERO,
            e_flags: 0,
            next_think: 0,
            birth: now,
            travel: 0.0,
            pickup: false,
            event_seq: 0,
            events: [0; 4],
            parms: [0; 4],
            free_at: None,
            team,
            rocket: false,
            launcher: None,
            flags: 0,
            wait: 0.0,
            curve: Vec3::ZERO,
            target: None,
            target_offset: Vec3::ZERO,
            climbed: false,
        }
    }

    /// `G_AddEvent`.
    fn add_event(&mut self, event: u32, parm: u32) {
        let k = (self.event_seq & 3) as usize;
        self.events[k] = event;
        self.parms[k] = parm & 0xff;
        self.event_seq += 1;
    }

    /// `G_SetOrigin`: at rest where it stands.
    fn set_origin(&mut self, o: Vec3) {
        self.pos = Traj {
            tr_type: TR_STATIONARY,
            time: 0,
            base: o,
            delta: Vec3::ZERO,
        };
        self.origin = o;
    }

    /// `G_SetAngle`.
    fn set_angle(&mut self, a: Vec3) {
        self.apos = Traj {
            tr_type: TR_STATIONARY,
            time: 0,
            base: a,
            delta: Vec3::ZERO,
        };
        self.angles = a;
    }
}

/// The event entity a fuse's burst spawns (`ET_GENERAL`, `eFlags` 0x20),
/// freed 300 ms past its event; a smoke grenade's lives on as a timed
/// object until `time2`.
#[derive(Clone, Debug)]
pub struct Burst4 {
    pub id: EntId,
    pub weapon: u32,
    pub origin: Vec3,
    pub event: u32,
    pub parm: u32,
    pub surf_type: u32,
    pub e_flags: u32,
    pub time: i32,
    /// A smoke cloud: `customExplode.startTime` and `time2`.
    pub smoke_until: Option<i32>,
}

/// `vectoangles`.
fn vectoangles(v: Vec3) -> Vec3 {
    Vec3::from(crate::game::builtins::math::vectoangles(v.to_array()))
}

/// `AngleNormalize360` (0x8190540): `x = a * 0.0027777778f` on the stack,
/// its floor stored, `(x - floor) * 360` kept, less 360 when it reaches
/// 360, one store.
fn normalize360(a: f32) -> f32 {
    let x = f64::from(a) * f64::from(0.002_777_777_8_f32);
    let v = (x - f64::from(x.floor() as f32)) * 360.0;
    (if v < 360.0 { v } else { v - 360.0 }) as f32
}

/// `AngleDelta`.
fn angle_delta(a1: f32, a2: f32) -> f32 {
    let d = f64::from(a1) - f64::from(a2);
    (d - 360.0 * (d * (1.0 / 360.0) + 0.5).floor()) as f32
}

/// `PitchForYawOnNormal`.
fn pitch_for_yaw_on_normal(yaw: f32, n: Vec3) -> f32 {
    if n.z == 0.0 {
        return 270.0;
    }
    let (s, c) = yaw.to_radians().sin_cos();
    let f = (n.x * c + n.y * s) / n.z;
    (f64::from(f.atan()) * 180.0 / std::f64::consts::PI) as f32
}

/// The no-draw time: the launch hides the missile 20..50 ms.
fn no_draw_ms(speed: f32) -> i32 {
    ((speed * -35.0 / 600.0 + 85.0) as i32).clamp(20, 50)
}

/// A missile trace's contact.
#[derive(Clone, Copy, Debug)]
pub(super) struct Hit {
    pub(super) fraction: f32,
    normal: Vec3,
    surface_flags: u32,
    /// The entity met: a client's slot, a brush entity or the world.
    entity: u32,
    startsolid: bool,
    hit_location: u8,
}

impl Cod4Level {
    /// `G_UseOffHand` on `EV_USE_OFFHAND`: the equipped offhand thrown from
    /// the eye along the view.
    pub(super) fn use_offhand(&mut self, slot: usize) {
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        let Some(d) = self.combat.defs.get(ps.off_hand_index as usize).cloned() else {
            return;
        };
        if ps.off_hand_index == 0 || d.weap_type != WEAPTYPE_GRENADE {
            return;
        }
        self.throw_grenade(slot, ps.off_hand_index, &d);
    }

    /// `Weapon_Throw_Grenade`: the offhand thrown from the eye with its
    /// spin and the cook's fuse (doc 10.2).
    pub(super) fn throw_grenade(&mut self, slot: usize, weapon: u32, d: &WeaponDef) {
        self.lob(slot, weapon, d, Lob::Hand);
    }

    /// `Weapon_GrenadeLauncher_Fire` (0x80a2f30): no spin and no fuse (it
    /// bursts on its first impact past its arming distance), and stable
    /// ([`FL_STEADY`], 0x80a2f9b) so `G_RunMissile` never wobbles it.
    pub(super) fn launch_grenade(&mut self, slot: usize, weapon: u32, d: &WeaponDef) {
        self.lob(slot, weapon, d, Lob::Launcher);
    }

    /// A player's grenade out of the eye along the view (doc 10.2); its
    /// move then gains the player's own velocity along the toss, after
    /// `G_FireGrenade`'s truncation.
    fn lob(&mut self, slot: usize, weapon: u32, d: &WeaponDef, by: Lob) {
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        let [view, _, _] = self.muzzle_axis(slot, &ps);
        let eye = super::combat::view_origin(&ps, self.level_time());
        let toss = toss_velocity(view, d, by.level_push(d));
        let (spin, fuse) = by.spin_and_fuse(d);
        let Some(mut m) = self.new_grenade(slot, eye, toss, weapon, d, spin, fuse) else {
            return;
        };
        if by == Lob::Launcher {
            m.flags |= FL_STEADY;
        }
        let along = toss.normalize_or_zero();
        m.pos.delta += along * ps.velocity.dot(along);
        self.rt.host.cod4_mut().missiles.push(m);
    }

    /// `Weapon_RocketLauncher_Fire` (0x80a2d80) for a player: the aim
    /// spread's disc 16 ahead of the eye (two `G_random`s), the rocket
    /// fired from the eye with no gun velocity and no target (0x80a31b0),
    /// the firer pushed back 64 along the view.
    pub(super) fn fire_rocket(&mut self, slot: usize, weapon: u32, d: &WeaponDef, spread: f32) {
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        let [forward, right, up] = self.muzzle_axis(slot, &ps);
        let dir = self.rocket_dir(forward, right, up, spread);
        let start = super::combat::view_origin(&ps, self.level_time());
        let team = self
            .rt
            .client_field(slot, "sessionteam")
            .unwrap_or_default();
        let Some(id) = self.rt.with_host(|h, cx| h.ents.spawn(cx).ok()) else {
            return;
        };
        let shot = RocketShot {
            owner: Some(slot),
            launcher: None,
            client: true,
            team,
            start,
            dir,
            gun_vel: Vec3::ZERO,
            target: None,
            target_offset: Vec3::ZERO,
        };
        self.g_fire_rocket(id, weapon, d, shot);
        if let Some(p) = self.players[slot].as_mut() {
            for k in 0..3 {
                let name = format!("velocity[{k}]");
                let v = f32::from_bits(p.ps.word(&name)) - 64.0 * forward[k];
                p.ps.set(&name, v.to_bits());
            }
        }
    }

    /// `Weapon_RocketLauncher_Fire`'s aim: 16 ahead of the eye, moved off
    /// the view by a point on the spread's disc (radius `tan(spread) 16`),
    /// not normalized.
    pub(super) fn rocket_dir(&mut self, forward: Vec3, right: Vec3, up: Vec3, spread: f32) -> Vec3 {
        let disc = (spread * DEG_TO_RAD).tan() * 16.0;
        let (across, lift) = spread_disc(&mut self.rt.host.cod4_mut().rand, disc);
        forward * 16.0 + right * across + up * lift
    }

    /// `G_FireRocket` (0x8096d04, cod4-gsc.md 3.2): the `rocket` entity `id`
    /// at `shot.start` along `shot.dir`. Javelin's climb (guidance 3) is
    /// left out; no stock MP weapon has it.
    pub(super) fn g_fire_rocket(
        &mut self,
        id: EntId,
        weapon: u32,
        d: &WeaponDef,
        shot: RocketShot,
    ) {
        let now = self.level_time();
        self.name_class(id, "rocket");
        let dir = shot.dir.normalize_or_zero();
        let facing = vectoangles(dir);
        let speed = d.projectile_speed as f32;
        let curve = (d.projectile_curvature > 0.0).then(|| {
            draw_curvature(
                &mut self.rt.host.cod4_mut().rand,
                facing,
                d.projectile_curvature,
            )
        });
        let accelerates = d.time_to_accelerate > 0.0;
        let guided = d.guided_missile_type != 0;
        // 0x8096f69: an accelerating rocket leaves from rest. The gun's
        // velocity is added either way, each component truncated.
        let own = if accelerates { Vec3::ZERO } else { dir * speed };
        // A rocket the game steps itself (accelerating, curving, or guided,
        // 0x80970af) flies `TR_INTERPOLATE`; the rest fly straight.
        let self_stepped = accelerates || curve.is_some() || guided;
        // Only a client's rocket hides at launch: a helicopter's shows at once.
        let hidden = if shot.client { no_draw_ms(speed) } else { 0 };
        let steady = if d.destabilization_rate_time == 0.0 {
            FL_STEADY
        } else {
            0
        };
        let base = Missile4::fresh(id, weapon, shot.owner, shot.start, now, shot.team);
        let mut m = Missile4 {
            pos: Traj {
                tr_type: if self_stepped {
                    TR_INTERPOLATE
                } else {
                    TR_LINEAR
                },
                delta: (own + shot.gun_vel).trunc(),
                ..base.pos
            },
            launch_time: base.launch_time + hidden,
            next_think: now + ((d.proj_lifetime * 1000.0) as i32).min(60_000),
            e_flags: 0x400,
            flags: base.flags | steady,
            // 0x80970d1: the first wobble waits out `destabilizeDistance`.
            wait: (f64::from(d.destabilize_distance) / f64::from(d.projectile_speed) * 1000.0)
                as f32,
            curve: curve.unwrap_or(base.curve),
            rocket: true,
            launcher: shot.launcher,
            target: shot.target,
            target_offset: shot.target_offset,
            ..base
        };
        m.set_angle(facing);
        if guided {
            m.apos.tr_type = TR_INTERPOLATE;
        }
        self.rt.host.cod4_mut().missiles.push(m);
    }

    /// `G_FireGrenade` (doc 10.2): a grenade from `start` at `dir`, stored
    /// in the missile list; its index there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fire_grenade(
        &mut self,
        slot: usize,
        start: Vec3,
        dir: Vec3,
        weapon: u32,
        d: &WeaponDef,
        rotate: bool,
        time: i32,
    ) -> Option<usize> {
        let m = self.new_grenade(slot, start, dir, weapon, d, rotate, time)?;
        let missiles = &mut self.rt.host.cod4_mut().missiles;
        missiles.push(m);
        Some(missiles.len() - 1)
    }

    /// The grenade [`Self::fire_grenade`] stores: the `grenade` entity on
    /// the arc (speed truncated whole, hidden by its untruncated length),
    /// tumbling when `spin` asks and the weapon spins, its fuse
    /// (the grenade timer init) and, unless it arms by distance,
    /// `grenade_fire` on the thrower.
    #[allow(clippy::too_many_arguments)]
    fn new_grenade(
        &mut self,
        slot: usize,
        start: Vec3,
        vel: Vec3,
        weapon: u32,
        d: &WeaponDef,
        spin: bool,
        fuse: i32,
    ) -> Option<Missile4> {
        let now = self.level_time();
        let id = self.rt.with_host(|h, cx| h.ents.spawn(cx).ok())?;
        self.name_class(id, "grenade");
        let team = self
            .rt
            .client_field(slot, "sessionteam")
            .unwrap_or_default();
        let facing = vectoangles(vel);
        let mut m = Missile4::fresh(id, weapon, Some(slot), start, now, team);
        m.pos.tr_type = TR_GRAVITY;
        m.pos.delta = vel.trunc();
        m.launch_time = now + no_draw_ms(vel.length());
        m.e_flags = BOUNCING;
        m.pickup = d.offhand_class == 1;
        if spin && d.rotate {
            let tumble = draw_spin(&mut self.rt.host.cod4_mut().rand, facing);
            m.apos = Traj {
                time: now,
                ..tumble
            };
            m.angles = tumble.base;
        } else {
            m.set_angle(facing);
        }
        // A grenade armed by distance keeps the thrower's cook unless it
        // was given a fuse of its own.
        let timed = d.projectile_activate_dist == 0 || fuse > 0;
        let cooked = self.pm_state(slot).map_or(0, |ps| ps.grenade_time_left);
        if timed {
            if let Some(p) = self.players[slot].as_mut() {
                p.ps.set("grenadeTimeLeft", 0);
            }
            self.rt.host.cod4_mut().clients[slot].held_grenade = None;
        }
        let burn = (timed && d.timed_detonation).then_some(if cooked != 0 { cooked } else { fuse });
        m.next_think = grenade_deadline(now, burn);
        if d.projectile_activate_dist == 0
            && let Some(thrower) = self.rt.client_entity(slot)
        {
            let name = self
                .rt
                .with_host(|_, cx| Value::String(cx.intern_exact(&d.name)));
            self.rt.host.engine_notifies.push((
                thrower,
                "grenade_fire",
                vec![Value::Entity(id), name],
            ));
        }
        Some(m)
    }

    /// Sets a fresh missile entity's `classname`.
    fn name_class(&mut self, id: EntId, class: &str) {
        self.rt.with_host(|h, cx| {
            let key = cx.intern_folded("classname");
            let value = Value::String(cx.intern_exact(class));
            let _ = vcod_gsc::Host::set_field(h, cx, id, key, value);
        });
    }

    /// `G_MissileTrace`: a point through the clip map and the brush
    /// entities under `mask`, then (with `CONTENTS_BODY` in it) every other
    /// playing body's hit boxes before the world; a start in solid reads
    /// fraction 0 back along the move.
    pub(super) fn missile_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        pass: Option<usize>,
        mask: u32,
    ) -> Hit {
        let mut best = match self.clip.as_deref() {
            Some(clip) => {
                let world = Cod4Clip {
                    clip,
                    bodies: &[],
                    brushes: &self.brushes,
                };
                let mut t = vcod_cod4::pmove::PmWorld::trace(
                    &world,
                    start,
                    end,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    NONE,
                    mask & !vcod_cod4::pmove::CONTENTS_BODY,
                );
                // A point trace meets the static models too
                // (`CM_PointTraceStaticModels`, cod4-collision.md 4).
                let mut ct = vcod_common::clipmap::ClipTrace::clear();
                ct.fraction = t.fraction;
                if clip
                    .trace_static_models(start, end, mask, &mut ct)
                    .is_some()
                    && ct.fraction < t.fraction
                {
                    t.fraction = ct.fraction;
                    t.normal = ct.normal;
                    t.surface_flags = ct.surface_flags;
                    t.entity_num = WORLD;
                    t.startsolid = ct.startsolid;
                }
                Hit {
                    fraction: t.fraction,
                    normal: t.normal,
                    surface_flags: t.surface_flags,
                    entity: if t.fraction < 1.0 || t.startsolid {
                        t.entity_num
                    } else {
                        NONE
                    },
                    startsolid: t.startsolid,
                    hit_location: 0,
                }
            }
            None => Hit {
                fraction: 1.0,
                normal: Vec3::ZERO,
                surface_flags: 0,
                entity: NONE,
                startsolid: false,
                hit_location: 0,
            },
        };
        if mask & vcod_cod4::pmove::CONTENTS_BODY != 0 {
            let origins = std::collections::HashMap::new();
            if let Some((slot, h, normal)) =
                self.body_hit(start, end, pass, &BULLET_PRIORITY, &origins, best.fraction)
            {
                best = Hit {
                    fraction: h.fraction,
                    normal,
                    surface_flags: 0,
                    entity: slot as u32,
                    startsolid: h.start_solid,
                    hit_location: h.hit_location,
                };
            }
        }
        if best.startsolid {
            best.fraction = 0.0;
            best.normal = (start - end).normalize_or_zero();
        }
        best
    }

    /// `G_RunFrame`'s missiles, by entity number: each runs its move and
    /// its think; the event entities past their 300 ms are freed.
    pub(super) fn run_missiles(&mut self) {
        // `detonate`: `G_ExplodeMissile` with the new parent, if any.
        for (id, parent) in std::mem::take(&mut self.rt.host.cod4_mut().detonations) {
            if let (Some(i), Some(owner)) = (self.missile_index(id), parent) {
                self.rt.host.cod4_mut().missiles[i].owner = owner;
            }
            self.explode_missile(id);
        }
        let now = self.level_time();
        let mut order: Vec<(u32, EntId)> = self
            .rt
            .host
            .cod4()
            .missiles
            .iter()
            .map(|m| (m.id.0, m.id))
            .collect();
        order.sort_by_key(|o| o.0);
        for (_, id) in order {
            let Some(i) = self.missile_index(id) else {
                continue;
            };
            let m = &self.rt.host.cod4().missiles[i];
            if let Some(at) = m.free_at {
                if now - at > EVENT_VALID_MS {
                    self.rt.host.cod4_mut().missiles.remove(i);
                    self.rt.host.ents.free(id);
                }
                continue;
            }
            self.run_missile(id);
            if let Some(i) = self.missile_index(id) {
                let m = &self.rt.host.cod4().missiles[i];
                if m.free_at.is_none() && m.next_think > 0 && m.next_think <= now {
                    self.explode_missile(id);
                }
            }
        }
        let c4 = self.rt.host.cod4_mut();
        let mut freed = Vec::new();
        c4.bursts.retain(|b| match b.smoke_until {
            Some(t) if now < t => true,
            _ if now - b.time <= EVENT_VALID_MS => true,
            _ => {
                freed.push(b.id);
                false
            }
        });
        for id in freed {
            self.rt.host.ents.free(id);
        }
    }

    fn missile_index(&self, id: EntId) -> Option<usize> {
        self.rt.host.cod4().missiles.iter().position(|m| m.id == id)
    }

    /// `G_RunMissile` (0x809727e), one frame of a missile (cod4-combat.md
    /// 10.2): a grenade resting on anything but the world drops once
    /// nothing holds it; the step to this frame's point is traced, a
    /// bouncing grenade near a floor is set onto it, and the step ends in
    /// the air, on a no-impact surface (freed) or in `MissileImpact`.
    fn run_missile(&mut self, id: EntId) {
        let now = self.level_time();
        let Some(mut m) = self.missile_copy(id) else {
            return;
        };
        let Some(d) = self.combat.defs.get(m.weapon as usize).cloned() else {
            return;
        };
        if m.pos.tr_type == TR_STATIONARY && m.ground != WORLD && self.lost_ground(&m) {
            m.pos = Traj {
                tr_type: TR_GRAVITY,
                time: now,
                base: m.origin,
                delta: Vec3::ZERO,
            };
        }
        let start = m.origin;
        let goal = if m.rocket {
            self.missile_trajectory(&mut m, &d, now)
        } else {
            m.pos.at(now)
        };
        if (goal - start).length() < 0.001 {
            return self.store_missile(m);
        }
        let (mut hit, mut stop) = self.step_trace(&m, goal);
        m.origin = stop;
        let settles = hit.fraction == 1.0 || hit.normal.z > 0.7;
        if m.e_flags & BOUNCING != 0
            && settles
            && let Some((floor, on)) = self.floor_below(&m)
        {
            m.pos.base.z += on.z + SNAP_DOWN - m.origin.z;
            m.origin = Vec3::new(on.x, on.y, on.z + SNAP_DOWN);
            (hit, stop) = (floor, on);
        }
        if d.projectile_activate_dist > 0 {
            m.travel += (stop - start).length();
        }
        if hit.fraction == 1.0 {
            self.fly_on(&mut m, &d, now);
            return self.store_missile(m);
        }
        self.store_missile(m);
        if hit.surface_flags & SURF_NOIMPACT != 0 {
            self.free_missile(id);
        } else {
            let heading = (goal - start).normalize_or_zero();
            self.missile_impact(id, &hit, heading, stop, &d);
        }
    }

    /// A copy of missile `id` as the list holds it.
    fn missile_copy(&self, id: EntId) -> Option<Missile4> {
        let i = self.missile_index(id)?;
        Some(self.rt.host.cod4().missiles[i].clone())
    }

    /// Whether a resting grenade's ground has gone: nothing within
    /// [`GROUND_PROBE`] behind its surface normal, a helicopter's
    /// collision included (a grenade riding one, `vehicle::carry_riders`).
    fn lost_ground(&mut self, m: &Missile4) -> bool {
        let below = m.origin - m.surface_normal * GROUND_PROBE;
        self.missile_trace(m.origin, below, m.owner, MASK_GRENADE)
            .fraction
            == 1.0
            && self
                .heli_trace(m.origin, below, 1.0, None, MASK_GRENADE)
                .is_none()
    }

    /// The frame's step from `r.currentOrigin` to `goal`: the contact (a
    /// helicopter's when it is nearer than the world's and the bodies')
    /// and where the step stops.
    fn step_trace(&mut self, m: &Missile4, goal: Vec3) -> (Hit, Vec3) {
        let mut hit = self.missile_trace(m.origin, goal, m.owner, MASK_GRENADE);
        if let Some(h) = self.heli_trace(m.origin, goal, hit.fraction, m.launcher, MASK_GRENADE) {
            hit = Hit {
                fraction: h.fraction,
                normal: h.normal,
                surface_flags: h.surface_flags,
                entity: h.id.0,
                startsolid: false,
                hit_location: 0,
            };
        }
        (hit, m.origin.lerp(goal, hit.fraction))
    }

    /// The floor a bouncing grenade is set onto: a trace from
    /// [`SNAP_UP`] over it to [`SNAP_DOWN`] under it, its contact and
    /// where it stopped.
    fn floor_below(&mut self, m: &Missile4) -> Option<(Hit, Vec3)> {
        let top = m.origin + Vec3::Z * SNAP_UP;
        let bottom = m.origin - Vec3::Z * SNAP_DOWN;
        let floor = self.missile_trace(top, bottom, m.owner, MASK_GRENADE);
        (floor.fraction != 1.0).then(|| (floor, top.lerp(bottom, floor.fraction)))
    }

    /// A step that met nothing: a moving missile leaves its ground, and
    /// an unguided projectile wobbles and bends to the attractors
    /// (0x80979dd).
    fn fly_on(&mut self, m: &mut Missile4, d: &WeaponDef, now: i32) {
        if m.pos.delta.length() == 0.0 {
            return;
        }
        m.ground = NONE;
        let free_rocket = d.weap_type == WEAPTYPE_PROJECTILE
            && m.flags & FL_STEADY == 0
            && d.guided_missile_type == 0;
        if free_rocket {
            self.wobble_and_attract(m, d, now);
        }
    }

    /// The missile trajectory step (0x8095928, cod4-gsc.md 3.2): where a rocket is
    /// this frame. Past its launch and off `TR_LINEAR` it moves itself a
    /// frame at a time: speeding up along its facing, curving, steering
    /// once it may, `trBase` stepped along `trDelta`. Anything else
    /// evaluates its trajectory.
    fn missile_trajectory(&mut self, m: &mut Missile4, d: &WeaponDef, now: i32) -> Vec3 {
        if now <= m.launch_time || m.pos.tr_type == TR_LINEAR {
            return m.pos.at(now);
        }
        let ramp = f64::from(d.time_to_accelerate);
        if ramp > 0.0 {
            let [facing, _, _] = vcod_common::x87::angle_vectors(m.angles);
            let top = f64::from(d.projectile_speed);
            let along = dot64(facing, m.pos.delta);
            // A frame's share of the speed, the last step landing on it.
            let gain = if along < top {
                top / ramp * FRAME_S
            } else {
                top - along
            };
            m.pos.delta = mad(m.pos.delta, gain, facing);
        }
        if d.projectile_curvature > 0.0 {
            m.pos.delta = mad(m.pos.delta, FRAME_S, m.curve);
        }
        // 0x8095c4f: the ramp's time left, kept on the stack.
        let settled = ramp + f64::from(now - m.pos.time) * f64::from(-0.001f32) <= 0.0;
        if settled && matches!(d.guided_missile_type, 1 | 2) {
            let aim = m.target.and_then(|t| self.missile_target_origin(t));
            if let Some(at) = aim {
                m.steer_at(d, at + m.target_offset);
            }
        }
        m.pos.base = mad(m.pos.base, FRAME_S, m.pos.delta);
        if settled && m.apos.tr_type == TR_INTERPOLATE {
            let facing = vectoangles(m.pos.delta);
            m.angles = facing;
            m.apos = Traj {
                tr_type: TR_INTERPOLATE,
                time: 0,
                base: facing,
                delta: Vec3::ZERO,
            };
        }
        m.pos.base
    }

    /// Where a guided rocket's target stands (`r.currentOrigin`): a
    /// helicopter's body, else the entity's origin; `None` once freed,
    /// which leaves the rocket unguided (`G_FreeEntity` clears the handle,
    /// 0x80cc1d7).
    fn missile_target_origin(&mut self, id: EntId) -> Option<Vec3> {
        if let Some(h) = self.rt.host.cod4().helis.iter().find(|h| h.id == id) {
            return Some(h.origin);
        }
        self.rt.with_host(|h, cx| {
            h.ents.get(id)?;
            let o = cx.intern_folded("origin");
            match vcod_gsc::Host::get_field(h, cx, id, o) {
                Value::Vector(v) => Some(Vec3::from(v)),
                _ => None,
            }
        })
    }

    /// `G_RunMissile`'s wobble (0x8097a05, cod4-gsc.md 3.2) for an unguided
    /// projectile whose move met nothing. Each time its wait runs out the
    /// angles draw new rates and the wait restarts at
    /// `destabilizationRateTime`; from the first draw on, every frame turns
    /// the angles a frame of the rates and sends the rocket along them at
    /// its speed, then lets the attractors bend it. Before the first draw
    /// nothing runs, the attractors included (0x809872f).
    fn wobble_and_attract(&mut self, m: &mut Missile4, d: &WeaponDef, now: i32) {
        if (m.wait as i32).wrapping_add(m.pos.time) < now {
            let first = m.flags & FL_WOBBLING == 0;
            let rand = &mut self.rt.host.cod4_mut().rand;
            m.curve = wobble_rates(rand, m.curve, d.destabilization_curvature_max, first);
            m.pos.time = now;
            m.wait = (1000.0 * f64::from(d.destabilization_rate_time)) as f32;
            m.flags |= FL_WOBBLING;
        } else if m.flags & FL_WOBBLING == 0 {
            return;
        }
        let turned = mad(m.apos.base, FRAME_S, m.curve);
        m.set_angle(turned);
        m.apos.tr_type = TR_INTERPOLATE;
        let [ahead, _, _] = vcod_common::x87::angle_vectors(turned);
        m.pos.tr_type = TR_INTERPOLATE;
        m.pos.base = m.origin;
        m.pos.delta = ahead * d.projectile_speed as f32;
        self.attract(m, d);
    }

    /// The attractor pass (0x8097be3..0x809882d, cod4-gsc.md 3.2): the
    /// pulls of every slot in use, a frame of their sum added to the
    /// velocity, which keeps its speed; `apos.trBase` alone follows it.
    fn attract(&mut self, m: &mut Missile4, d: &WeaponDef) {
        let v = m.pos.delta;
        let len = v.length();
        if len < 1e-5 {
            return;
        }
        let heading = v / len;
        let speed = f64::from(d.projectile_speed);
        let mut total = [0.0f64; 3];
        for slot in 0..MAX_ATTRACTORS {
            let Some(a) = self.rt.host.cod4().attractors[slot] else {
                continue;
            };
            let Some(at) = self.attractor_point(slot, &a) else {
                continue;
            };
            if let Some(p) = pull(&a, at - m.pos.base, heading, speed) {
                for (t, p) in total.iter_mut().zip(p) {
                    *t += p;
                }
            }
        }
        if total == [0.0; 3] {
            return;
        }
        let bent = Vec3::from_array(std::array::from_fn(|k| {
            (f64::from(v[k]) + total[k] * FRAME_S) as f32
        }));
        let way = bent.normalize_or_zero();
        m.pos.delta = way * d.projectile_speed as f32;
        m.apos.base = vectoangles(way);
    }

    /// Where attractor `slot` stands: its point, or its entity's origin. A
    /// slot whose entity is gone is cleared, as `G_FreeEntity` clears it
    /// on retail (0x8093402).
    fn attractor_point(&mut self, slot: usize, a: &Attractor) -> Option<Vec3> {
        let Some(e) = a.entity else {
            return Some(a.point);
        };
        let at = self.missile_target_origin(e);
        if at.is_none() {
            self.rt.host.cod4_mut().attractors[slot] = None;
        }
        at
    }

    /// The missile back in the list, its script entity's `origin` on
    /// `r.currentOrigin` (what a callback reads off an inflictor:
    /// `_weapons.gsc`'s `onWeaponDamage` scales the concussion by it).
    fn store_missile(&mut self, m: Missile4) {
        let Some(i) = self.missile_index(m.id) else {
            return;
        };
        let (id, origin) = (m.id, m.origin);
        self.rt.host.cod4_mut().missiles[i] = m;
        self.rt.with_host(|h, cx| {
            let field = cx.intern_folded("origin");
            let _ = vcod_gsc::Host::set_field(h, cx, id, field, Value::Vector(origin.to_array()));
        });
    }

    /// The missile gone at once (`G_FreeEntity`), its `death` notified.
    pub(super) fn free_missile(&mut self, id: EntId) {
        if let Some(i) = self.missile_index(id) {
            self.rt.host.cod4_mut().missiles.remove(i);
        }
        self.rt.host.engine_notifies.push((id, "death", Vec::new()));
        self.rt.host.cod4_mut().free_next.push(id);
    }

    /// `MissileImpact` (0x8094e2e, called at 0x80977e7; cod4-combat.md
    /// 10.2). An unarmed bouncing grenade off anything that takes no
    /// damage bounces, and a dud that comes to rest goes as
    /// `EV_CHANGE_TO_DUD`. A player who takes damage is hit for the
    /// weapon's `damage` (a grenade only above `g_minGrenadeDamageSpeed`),
    /// then an unarmed missile bounces off him as off flesh. Anything else
    /// bursts where it struck. An impact grenade short of its
    /// `projectileActivateDist` is a dud for good.
    fn missile_impact(&mut self, id: EntId, tr: &Hit, dir: Vec3, endpos: Vec3, d: &WeaponDef) {
        let Some(mut m) = self.missile_copy(id) else {
            return;
        };
        let load = Payload::of(d, &m);
        if load.dud {
            m.travel = SPENT;
        }
        m.surf_type = surface_type(tr.surface_flags);
        let player = (tr.entity < 64).then_some(tr.entity as usize);
        let victim = player.filter(|&slot| self.rt.client_vitals(slot).takedamage);
        match victim {
            Some(slot) => {
                let Some(back) = self.impact_damage(m, slot, tr, d, load.mod_) else {
                    return;
                };
                m = back;
                if !load.armed {
                    let flesh = Hit {
                        surface_flags: if tr.surface_flags == 0 {
                            SURF_FLESH
                        } else {
                            tr.surface_flags
                        },
                        ..*tr
                    };
                    self.bounce_off(&mut m, &flesh, d);
                    return self.store_missile(m);
                }
            }
            None if m.e_flags & BOUNCING != 0 && !load.armed => {
                self.bounce_off(&mut m, tr, d);
                if d.projectile_activate_dist > 0 && m.pos.tr_type == TR_STATIONARY {
                    m.add_event(ev::CHANGE_TO_DUD, normal_byte(tr.normal));
                    m.free_at = Some(self.level_time());
                }
                return self.store_missile(m);
            }
            None => {}
        }
        self.impact_burst(m, tr, dir, endpos, d, &load, player);
    }

    /// The weapon's `damage` on the player a missile struck, as long as
    /// it has a thrower and a grenade is fast enough; the missile back
    /// after the callbacks, `None` once they freed it.
    fn impact_damage(
        &mut self,
        m: Missile4,
        victim: usize,
        tr: &Hit,
        d: &WeaponDef,
        mod_: &'static str,
    ) -> Option<Missile4> {
        let v = m.pos.delta_at(self.level_time());
        let fast = d.weap_type != WEAPTYPE_GRENADE || MIN_GRENADE_DAMAGE_SPEED < v.length();
        let Some(owner) = m.owner.filter(|_| d.damage != 0 && fast) else {
            return Some(m);
        };
        let location = if mod_ == "MOD_IMPACT" {
            tr.hit_location
        } else {
            0
        };
        let (id, at) = (m.id, m.origin);
        self.through_callbacks(m, |lvl| {
            lvl.damage_from(
                owner,
                victim,
                Some(id),
                d,
                d.damage,
                0,
                mod_,
                v,
                at,
                location,
            );
        })
    }

    /// Stores `m`, runs `f`, whose script callbacks may move or free it,
    /// and takes the missile back.
    fn through_callbacks(&mut self, m: Missile4, f: impl FnOnce(&mut Self)) -> Option<Missile4> {
        let id = m.id;
        self.store_missile(m);
        f(self);
        self.missile_copy(id)
    }

    /// An impact's burst: a struck helicopter takes the weapon's
    /// `damage` first; the burst's event rides the missile, which stops
    /// at `endpos`, hides and is freed 300 ms on, `death` notified; an
    /// armed one's splash (players, helicopters, other entities) follows,
    /// then `projectile_impact` on its thrower and a flashbang's blast.
    #[allow(clippy::too_many_arguments)]
    fn impact_burst(
        &mut self,
        mut m: Missile4,
        tr: &Hit,
        dir: Vec3,
        endpos: Vec3,
        d: &WeaponDef,
        load: &Payload,
        player: Option<usize>,
    ) {
        let id = m.id;
        let heli = self
            .rt
            .host
            .cod4()
            .helis
            .iter()
            .find(|h| h.id.0 == tr.entity)
            .map(|h| h.id);
        if let Some(hid) = heli
            && load.armed
            && d.damage != 0
        {
            let (v, at, owner) = (m.pos.delta_at(self.level_time()), m.origin, m.owner);
            let Some(back) = self.through_callbacks(m, |lvl| {
                lvl.damage_heli(hid, owner, d, d.damage, 0, load.mod_, v, at);
            }) else {
                return;
            };
            m = back;
        }
        let marks = player.is_none() && tr.hit_location == 0;
        if let Some(event) = impact_event(load.kind, marks) {
            let up = if d.proj_explosion_normal_up {
                Vec3::Z
            } else {
                tr.normal
            };
            m.add_event(event, normal_byte(up));
        }
        m.free_at = Some(self.level_time());
        m.e_flags = (m.e_flags ^ 2) | HIDDEN;
        m.set_origin(endpos);
        let (owner, team) = (m.owner, m.team.clone());
        self.store_missile(m);
        self.rt.host.engine_notifies.push((id, "death", Vec::new()));
        if !load.armed {
            return;
        }
        if d.explosion_inner_damage != 0
            && let Some(owner) = owner
        {
            let cone = cone_cos(d);
            self.radius_damage(endpos, id, owner, d, cone, dir, player, load.splash);
            self.radius_damage_helis(endpos, owner, d, heli, load.splash);
            self.radius_damage_entities(endpos, owner, d, cone, dir, load.splash);
            if let Some(thrower) = self.rt.client_entity(owner) {
                let args = self.rt.with_host(|_, cx| {
                    vec![
                        Value::String(cx.intern_exact(&d.name)),
                        Value::Vector(endpos.to_array()),
                        Value::Int(d.explosion_radius),
                    ]
                });
                self.rt
                    .host
                    .engine_notifies
                    .push((thrower, "projectile_impact", args));
            }
        }
        if load.kind == PROJEXP_FLASHBANG {
            self.flashbang_blast(endpos, d, owner, &team);
        }
    }

    /// `BounceMissile` with its event: `EV_GRENADE_BOUNCE` and the
    /// struck surface's type when the bounce is loud and the move did not
    /// start in solid.
    fn bounce_off(&mut self, m: &mut Missile4, tr: &Hit, d: &WeaponDef) {
        if self.bounce_missile(m, tr, d) && !tr.startsolid {
            m.add_event(ev::GRENADE_BOUNCE, surface_type(tr.surface_flags));
        }
    }

    /// `BounceMissile` (0x80946ba, cod4-combat.md 10.2): the velocity at
    /// the moment of contact mirrored in the surface; a bouncing grenade's
    /// scaled by its surface type's bounce factors and laid to rest on a
    /// floor that holds it once under 20 units a second. Anything still
    /// moving is pushed a tenth off the surface (never up), its arc and
    /// tumble restarted there. True when the move changed by over 100, or
    /// at rest.
    fn bounce_missile(&mut self, m: &mut Missile4, tr: &Hit, d: &WeaponDef) -> bool {
        let now = self.level_time();
        let at = contact_time(self.previous_time(), now, tr.fraction);
        let before = m.pos.delta_at(at);
        let (mirrored, dot) = mirror(before, tr.normal);
        m.pos.delta = mirrored;
        // Sticky grenades hold to players only; nothing holds to a missile.
        let holds = (d.stickiness == 0 || tr.entity >= 64)
            && !self
                .rt
                .host
                .cod4()
                .missiles
                .iter()
                .any(|o| o.id.0 == tr.entity);
        let floor = tr.normal.z > 0.7;
        if holds && (floor || d.stickiness == 3) {
            m.ground = tr.entity;
        }
        if m.e_flags & BOUNCING != 0 {
            if let Some(k) = bounce_factor(d, surface_type(tr.surface_flags), before, dot) {
                m.pos.delta = (m.pos.delta.as_dvec3() * k).as_vec3();
            }
            if holds && floor && m.pos.delta.length() < 20.0 {
                let still = m.origin;
                m.set_origin(still);
                let lying = tumble_on_contact(
                    &mut m.apos,
                    at,
                    tr.normal,
                    Landing::Rest,
                    &mut self.rt.host.cod4_mut().rand,
                );
                m.set_angle(lying);
                m.surface_normal = tr.normal;
                return true;
            }
        }
        m.origin = push_off(m.origin, tr.normal);
        m.pos.base = m.origin;
        m.pos.time = now;
        let tumble = tumble_on_contact(
            &mut m.apos,
            at,
            tr.normal,
            Landing::Bounce,
            &mut self.rt.host.cod4_mut().rand,
        );
        m.apos = Traj {
            base: tumble,
            time: now,
            ..m.apos
        };
        (m.pos.delta - before).length() > 100.0
    }

    /// `G_ExplodeMissile`, a grenade's fuse running out (cod4-combat.md
    /// 10.3). A smoke grenade still in the air puts it off a frame at a
    /// time and is dropped unburst once a minute old. Anything else
    /// stops on its arc, `explode` goes to its script entity with the
    /// point, the burst's event entity goes up there, the splash runs when
    /// the weapon has inner damage and a thrower (players, helicopters,
    /// script entities, `MOD_GRENADE_SPLASH`, inside the cone the grenade
    /// faces), a flashbang blinds, and the grenade is freed.
    pub(super) fn explode_missile(&mut self, id: EntId) {
        let now = self.level_time();
        let Some(mut m) = self.missile_copy(id) else {
            return;
        };
        let Some(d) = self.combat.defs.get(m.weapon as usize).cloned() else {
            return;
        };
        if d.offhand_class == OFFHAND_SMOKE && m.ground == NONE {
            if now - m.birth > SMOKE_AIR_LIMIT_MS {
                self.free_missile(id);
            } else {
                m.next_think = SMOKE_RETRY_THINK;
                self.store_missile(m);
            }
            return;
        }
        m.set_origin(m.pos.at(now));
        m.e_flags |= HIDDEN;
        let (at, weapon, owner, facing) = (m.origin, m.weapon, m.owner, m.angles);
        let team = m.team.clone();
        self.store_missile(m);
        let point = vec![Value::Vector(at.to_array())];
        self.rt.host.engine_notifies.push((id, "explode", point));
        self.raise_burst(at, weapon, &d, now);
        if let Some(owner) = owner.filter(|_| d.explosion_inner_damage != 0) {
            let [ahead, _, _] = vcod_common::x87::angle_vectors(facing);
            let (cone, splash) = (cone_cos(&d), "MOD_GRENADE_SPLASH");
            self.radius_damage(at, id, owner, &d, cone, ahead, None, splash);
            self.radius_damage_helis(at, owner, &d, None, splash);
            self.radius_damage_entities(at, owner, &d, cone, ahead, splash);
        }
        if d.proj_explosion == PROJEXP_FLASHBANG {
            self.flashbang_blast(at, &d, owner, &team);
        }
        self.free_missile(id);
    }

    /// A fuse's burst: an `ET_GENERAL` event entity on the grenade's
    /// point truncated (`G_TempEntity`'s `SnapVector`, 10.3), carrying the
    /// weapon, the event its `projExplosion` raises at a fuse and what a
    /// 16-unit trace down meets: the surface's normal (straight up with
    /// `projExplosionNormalUp`) and type. A smoke cloud stays 61 s.
    fn raise_burst(&mut self, at: Vec3, weapon: u32, d: &WeaponDef, now: i32) {
        let floor = self.missile_trace(at, at - Vec3::Z * 16.0, None, MASK_EXPLODE_DOWN);
        let normal = if d.proj_explosion_normal_up {
            Vec3::Z
        } else {
            floor.normal
        };
        let Some(id) = self.rt.with_host(|h, cx| h.ents.spawn(cx).ok()) else {
            return;
        };
        let smoke = d.proj_explosion == PROJEXP_SMOKE;
        let event = explosion_events(d.proj_explosion).map_or(ev::CUSTOM_EXPLODE, |e| e.fuse);
        self.rt.host.cod4_mut().bursts.push(Burst4 {
            id,
            weapon,
            origin: at.trunc(),
            event,
            parm: normal_byte(normal),
            surf_type: surface_type(floor.surface_flags),
            e_flags: if smoke { HIDDEN | SMOKE_CLOUD } else { HIDDEN },
            time: now,
            smoke_until: smoke.then_some(now + SMOKE_CLOUD_MS),
        });
    }

    /// `G_FlashbangBlast` (10.8): every player open to damage within
    /// `explosionRadius` of `at` whom one `CanDamage` line under 0x801
    /// reaches, no cone, is notified `flashbang` with how far in from that
    /// radius he stands (1 inside `explosionRadiusMin`), how squarely his
    /// view faces the burst (`(dot + 1) / 2` from the eye), the thrower and
    /// the thrower's team; `_flashgrenades.gsc`'s `monitorFlash` makes a
    /// shellshock of it. Slot order here, where retail walks its area list.
    fn flashbang_blast(&mut self, at: Vec3, d: &WeaponDef, owner: Option<usize>, team: &str) {
        let full = (d.explosion_radius_min as f32).max(1.0);
        let reach = (d.explosion_radius as f32).max(full);
        let thrower = owner
            .and_then(|o| self.rt.client_entity(o))
            .map_or(Value::Undefined, Value::Entity);
        for slot in 0..self.players.len() {
            if !self.rt.client_vitals(slot).takedamage {
                continue;
            }
            let (Some(ps), Some(player)) = (self.pm_state(slot), self.rt.client_entity(slot))
            else {
                continue;
            };
            let dist = (ps.origin - at).length();
            if dist > reach || self.body_share(&ps, at, NO_CONE, Vec3::ZERO, MASK_FLASH_LOS) <= 0.0
            {
                continue;
            }
            let nearness = if dist <= full {
                1.0
            } else {
                1.0 - (dist - full) / (reach - full)
            };
            let [view, _, _] = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles));
            let eye = ps.origin + Vec3::Z * ps.view_height_current;
            let facing = (view.dot((at - eye).normalize_or_zero()) + 1.0) * 0.5;
            let team = self
                .rt
                .with_host(|_, cx| Value::String(cx.intern_exact(team)));
            let args = vec![Value::Float(nearness), Value::Float(facing), thrower, team];
            self.rt
                .host
                .engine_notifies
                .push((player, "flashbang", args));
        }
    }

    /// `G_RadiusDamage` on the players (10.3): each one playing, open to
    /// damage and not `ignore`, whose origin lies inside the blast's
    /// radius takes the falloff at his distance times his `CanDamage`
    /// share, radius flags (5), pushed from the blast toward a point 24
    /// over his feet. Slot order here, where retail walks its area list.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn radius_damage(
        &mut self,
        origin: Vec3,
        inflictor: EntId,
        attacker: usize,
        d: &WeaponDef,
        cone: f32,
        cone_dir: Vec3,
        ignore: Option<usize>,
        mod_: &'static str,
    ) {
        let radius = blast_radius(d);
        for slot in (0..self.players.len()).filter(|&s| Some(s) != ignore) {
            if !self.rt.client_vitals(slot).takedamage {
                continue;
            }
            let spectating = vcod_cod4::pmove::pm_type::SPECTATOR;
            let Some(ps) = self.pm_state(slot).filter(|ps| ps.pm_type < spectating) else {
                continue;
            };
            let off = ps.origin - origin;
            let dist2 = off.length_squared();
            if dist2 >= radius * radius {
                continue;
            }
            let share = self.body_share(&ps, origin, cone, cone_dir, MASK_RADIUS_LOS);
            if share <= 0.0 {
                continue;
            }
            let damage = (falloff(d, dist2.sqrt(), radius) * share) as i32;
            self.combat.splashes.push((slot, damage));
            let push = off + Vec3::Z * 24.0;
            self.damage_from(
                attacker,
                slot,
                Some(inflictor),
                d,
                damage,
                RADIUS_DFLAGS,
                mod_,
                push,
                origin,
                0,
            );
        }
    }

    /// `G_RadiusDamage` over the helicopters but `ignore` (the one a
    /// rocket struck): one inside the radius of the blast, measured to its
    /// box (its collision surfaces' bounds at its origin, unturned), takes
    /// the inner damage falling to the outer at the edge when a trace
    /// from the blast to the box's middle (0x802011, the helicopter not in
    /// it) is clear, radius flags (5), toward the middle. INFERRED:
    /// `CanDamage`'s corner traces and the turned box are left
    /// out.
    fn radius_damage_helis(
        &mut self,
        origin: Vec3,
        attacker: usize,
        d: &WeaponDef,
        ignore: Option<EntId>,
        mod_: &'static str,
    ) {
        let radius = (d.explosion_radius as f32).max(1.0);
        let c4 = self.rt.host.cod4();
        let boxes: Vec<(EntId, Vec3, Vec3)> = c4
            .helis
            .iter()
            .filter(|h| Some(h.id) != ignore)
            .filter_map(|h| {
                let surfs = c4.model_coll.get(&h.model.to_ascii_lowercase())?;
                let (lo, hi) = surfs.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), s| (lo.min(Vec3::from(s.mins)), hi.max(Vec3::from(s.maxs))),
                );
                Some((h.id, h.origin + lo, h.origin + hi))
            })
            .collect();
        for (id, lo, hi) in boxes {
            let dist = (origin.clamp(lo, hi) - origin).length();
            if dist >= radius {
                continue;
            }
            let mid = (lo + hi) * 0.5;
            if self
                .missile_trace(origin, mid, None, MASK_RADIUS_LOS)
                .fraction
                < 1.0
            {
                continue;
            }
            let damage = (d.explosion_inner_damage - d.explosion_outer_damage) as f32
                * (1.0 - dist / radius)
                + d.explosion_outer_damage as f32;
            self.damage_heli(
                id,
                Some(attacker),
                d,
                damage as i32,
                5,
                mod_,
                mid - origin,
                origin,
            );
        }
    }

    /// `CanDamage` on a client (10.3): the share of lines from `center`
    /// under `mask` that reach the middle of his body (half way from the
    /// feet to the eye) and the four points 15 to either side of it across
    /// the line from the blast, raised and lowered by half the eye's
    /// height.
    pub(super) fn body_share(
        &mut self,
        ps: &vcod_cod4::pmove::PlayerState,
        center: Vec3,
        cone: f32,
        cone_dir: Vec3,
        mask: u32,
    ) -> f32 {
        let feet = ps.origin;
        let eye = super::combat::view_origin(ps, self.level_time());
        let toward = (center - feet).with_z(0.0).normalize_or_zero();
        let side = Vec3::new(-toward.y, toward.x, 0.0) * BODY_SIDE;
        let lift = Vec3::Z * ((eye.z - feet.z) * 0.5);
        let points = aim_points((eye + feet) * 0.5, side, lift, BODY_CORNERS);
        self.blast_share(center, points, cone, cone_dir, mask, LineScore::Thirds)
    }

    /// `CanDamage` (0x80b1f60) over five points: the lines from `center`
    /// under `mask` that reach them unblocked, a point outside the cone
    /// (cosine `cone` about `cone_dir`; [`NO_CONE`] for none) left
    /// untried, scored by `score`.
    pub(super) fn blast_share(
        &mut self,
        center: Vec3,
        points: [Vec3; 5],
        cone: f32,
        cone_dir: Vec3,
        mask: u32,
        score: LineScore,
    ) -> f32 {
        let in_cone =
            |p: Vec3| cone == NO_CONE || (p - center).normalize_or_zero().dot(cone_dir) >= cone;
        let clear = points
            .into_iter()
            .filter(|&p| in_cone(p))
            .map(|p| self.missile_trace(center, p, None, mask).fraction == 1.0);
        score.of(clear)
    }

    /// The missiles and the bursts as 1.7 sends them: a grenade's `ET_MISSILE`
    /// (4) with its arcs, launch time, ground, surface type, flags, weapon,
    /// its pickup item and the event ring; a burst's `ET_GENERAL` (0) with
    /// its one event.
    pub(super) fn missile_entities(&self) -> Vec<EntityState> {
        let c4 = self.rt.host.cod4();
        let mut out = Vec::new();
        for m in &c4.missiles {
            let mut e = EntityState::default();
            e.0[0] = m.id.0;
            e.0[1] = 4;
            let table = EntityState::table(4);
            let mut set = |name: &str, v: u32| {
                if let Some(f) = table.iter().find(|f| f.name == name) {
                    e.0[f.word()] = v;
                }
            };
            set("lerp.pos.trType", m.pos.tr_type);
            set("lerp.pos.trTime", m.pos.time as u32);
            set("lerp.apos.trType", m.apos.tr_type);
            set("lerp.apos.trTime", m.apos.time as u32);
            for k in 0..3 {
                set(&format!("lerp.pos.trBase[{k}]"), m.pos.base[k].to_bits());
                set(&format!("lerp.pos.trDelta[{k}]"), m.pos.delta[k].to_bits());
                set(&format!("lerp.apos.trBase[{k}]"), m.apos.base[k].to_bits());
                set(
                    &format!("lerp.apos.trDelta[{k}]"),
                    m.apos.delta[k].to_bits(),
                );
            }
            set("lerp.u.missile.launchTime", m.launch_time as u32);
            set("lerp.eFlags", m.e_flags);
            set("groundEntityNum", m.ground);
            set("surfType", m.surf_type);
            set("weapon", m.weapon);
            if m.pickup {
                set("index", m.weapon);
                set("clientNum", 64);
            }
            set("eventSequence", m.event_seq);
            for k in 0..4 {
                set(&format!("events[{k}]"), m.events[k]);
                set(&format!("eventParms[{k}]"), m.parms[k]);
            }
            out.push(e);
        }
        for b in &c4.bursts {
            let mut e = EntityState::default();
            e.0[0] = b.id.0;
            e.0[1] = 0;
            let table = EntityState::table(0);
            let mut set = |name: &str, v: u32| {
                if let Some(f) = table.iter().find(|f| f.name == name) {
                    e.0[f.word()] = v;
                }
            };
            for k in 0..3 {
                set(&format!("lerp.pos.trBase[{k}]"), b.origin[k].to_bits());
            }
            set("lerp.eFlags", b.e_flags);
            set("weapon", b.weapon);
            set("surfType", b.surf_type);
            set("eventSequence", 1);
            set("events[0]", b.event);
            set("eventParms[0]", b.parm);
            if let Some(until) = b.smoke_until {
                set("lerp.u.anonymous.data[0]", b.time as u32);
                set("time2", until as u32);
            }
            out.push(e);
        }
        out
    }
}

impl Cod4Level {
    /// Test-facing: `slot` throws a grenade named `weapon` that lies at
    /// `at` on the world and bursts at once (`G_ExplodeMissile`), as a
    /// thrown one would at its fuse. False with no such weapon. The burst
    /// beats the throw's `grenade_fire` to the script, so a claymore or C4
    /// bursts with no `owner` and a kill by one aborts in `ch_kills`;
    /// those go through [`Self::rest_grenade_for_test`] and
    /// [`Self::explode_for_test`] a frame apart.
    pub fn burst_for_test(&mut self, slot: usize, weapon: &str, at: Vec3) -> bool {
        let Some(w) = self.combat.defs.iter().position(|d| d.name == weapon) else {
            return false;
        };
        let d = self.combat.defs[w].clone();
        let Some(i) = self.fire_grenade(slot, at, Vec3::ZERO, w as u32, &d, false, 1) else {
            return false;
        };
        let m = &mut self.rt.host.cod4_mut().missiles[i];
        m.ground = WORLD;
        m.set_origin(at);
        let id = m.id;
        self.explode_missile(id);
        true
    }
}

impl Cod4Level {
    /// Test-facing: the missile `id` bursts where it is now
    /// (`G_ExplodeMissile`). False when it is gone.
    pub fn explode_for_test(&mut self, id: EntId) -> bool {
        if !self.rt.host.cod4().missiles.iter().any(|m| m.id == id) {
            return false;
        }
        self.explode_missile(id);
        true
    }
}

impl Cod4Level {
    /// Test-facing: `slot` lets go of a grenade named `weapon` at `start`
    /// moving at `vel`, with `fuse_ms` on its fuse (`G_FireGrenade`, no
    /// spin); its entity, `None` with no such weapon.
    pub fn fire_grenade_for_test(
        &mut self,
        slot: usize,
        weapon: &str,
        start: Vec3,
        vel: Vec3,
        fuse_ms: i32,
    ) -> Option<EntId> {
        let w = self.combat.defs.iter().position(|d| d.name == weapon)?;
        let d = self.combat.defs[w].clone();
        let i = self.fire_grenade(slot, start, vel, w as u32, &d, false, fuse_ms)?;
        Some(self.rt.host.cod4().missiles[i].id)
    }
}

impl Cod4Level {
    /// Drops a frag from `at` with no throw velocity, for the client's live tests.
    pub fn drop_frag_for_test(&mut self, slot: usize, at: Vec3, fuse_ms: i32) -> bool {
        let Some(w) = self
            .combat
            .defs
            .iter()
            .position(|d| d.name == "frag_grenade_mp")
        else {
            return false;
        };
        let d = self.combat.defs[w].clone();
        self.fire_grenade(slot, at, Vec3::ZERO, w as u32, &d, false, fuse_ms)
            .is_some()
    }

    /// Test-facing: a grenade named `weapon` of `slot`'s at rest on the
    /// world at `at` with `fuse_ms` left, as a thrown one lies once it
    /// settles; its entity, `None` with no such weapon. Its
    /// `grenade_fire` reaches the script with the frame's notifies, so a
    /// claymore or C4 bursting a frame later carries the `owner`
    /// `_weapons.gsc` gives it.
    pub fn rest_grenade_for_test(
        &mut self,
        slot: usize,
        weapon: &str,
        at: Vec3,
        fuse_ms: i32,
    ) -> Option<EntId> {
        let w = self.combat.defs.iter().position(|d| d.name == weapon)?;
        let d = self.combat.defs[w].clone();
        let i = self.fire_grenade(slot, at, Vec3::ZERO, w as u32, &d, false, fuse_ms)?;
        let m = &mut self.rt.host.cod4_mut().missiles[i];
        m.ground = WORLD;
        m.pos.tr_type = TR_STATIONARY;
        m.set_origin(at);
        Some(m.id)
    }
}

/// A frame in seconds, the float 0.05 widened.
const FRAME_S: f64 = 0.05f32 as f64;

/// Degrees to radians as the float the game multiplies by.
const DEG_TO_RAD: f32 = 0.017_453_292;

/// `missileHellfireMaxSlope` and `missileHellfireUpAccel`, left at their
/// defaults (cod4-gsc.md 3.2).
const HELLFIRE_MAX_SLOPE: f64 = 0.5;
const HELLFIRE_UP_ACCEL: f64 = 1000.0;

/// `atan`'s share of a right angle: `2 / pi` as the float at 0x8215f88,
/// a few bits over.
const TWO_OVER_PI: f32 = f32::from_bits(0x3f22_f98c);

/// A dot product kept on the stack.
fn dot64(a: Vec3, b: Vec3) -> f64 {
    f64::from(a.x) * f64::from(b.x)
        + f64::from(a.y) * f64::from(b.y)
        + f64::from(a.z) * f64::from(b.z)
}

/// The two ways a player's grenade leaves him.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lob {
    Hand,
    Launcher,
}

impl Lob {
    /// The push along the level view: a thrown grenade's
    /// `projectileSpeedForward`; a launcher has none.
    fn level_push(self, d: &WeaponDef) -> i32 {
        match self {
            Lob::Hand => d.projectile_speed_forward,
            Lob::Launcher => 0,
        }
    }

    /// Whether it may tumble, and its fuse.
    fn spin_and_fuse(self, d: &WeaponDef) -> (bool, i32) {
        match self {
            Lob::Hand => (true, d.fuse_time),
            Lob::Launcher => (false, 0),
        }
    }
}

/// A grenade's velocity out of the hand (doc 10.2): `projectileSpeed`
/// along the view, `projectileSpeedUp` straight up and `level_push` along
/// the view's flat heading.
fn toss_velocity(view: Vec3, d: &WeaponDef, level_push: i32) -> Vec3 {
    let mut v = view * d.projectile_speed as f32;
    v.z += d.projectile_speed_up as f32;
    // Added only when there is one, so a zero keeps its sign.
    if level_push != 0 {
        let flat = view.truncate().normalize_or_zero() * level_push as f32;
        v.x += flat.x;
        v.y += flat.y;
    }
    v
}

/// `G_InitGrenadeMovement`'s tumble (0x8093012, doc 10.2): the pitch
/// tipped back 120 and wrapped, a pitch rate and a roll rate, each a
/// `G_flrand` turned by a coin's `2 G_irand(0, 2) - 1`. The time is the
/// caller's.
fn draw_spin(rand: &mut GlibcRand, facing: Vec3) -> Traj {
    let mut rate = |lo: f32, hi: f32| {
        let size = g_flrand(rand, lo, hi);
        size * (2 * g_irand(rand, 0, 2) - 1) as f32
    };
    let pitch_rate = rate(320.0, 800.0);
    let roll_rate = rate(180.0, 540.0);
    Traj {
        tr_type: TR_LINEAR,
        time: 0,
        base: Vec3::new(normalize360(facing.x - 120.0), facing.y, facing.z),
        delta: Vec3::new(pitch_rate, 0.0, roll_rate),
    }
}

/// When a grenade bursts: `fuse_ms` past `now` for a timed one, else (or
/// at a deadline of 0) 30 s on; never past 60 s.
fn grenade_deadline(now: i32, fuse_ms: Option<i32>) -> i32 {
    let at = fuse_ms
        .map(|ms| now + ms)
        .filter(|&t| t != 0)
        .unwrap_or(now + 30_000);
    at.min(now + 60_000)
}

/// A point on the spread's disc of `radius`: an angle and a share of the
/// radius off two `G_random`s, in that order.
fn spread_disc(rand: &mut GlibcRand, radius: f32) -> (f32, f32) {
    let turn = (g_random(rand) * 360.0) as f32 * DEG_TO_RAD;
    let share = g_random(rand) as f32;
    let (sin, cos) = turn.sin_cos();
    (share * cos * radius, share * sin * radius)
}

/// A curving rocket's bend (0x8096fe7): a direction across its facing and
/// a size up to `curvature`, off two `G_random`s. Both draws are stored,
/// the angle's radians kept until stored, and each axis's sideways term
/// stored before the upward one joins it.
fn draw_curvature(rand: &mut GlibcRand, facing: Vec3, curvature: f32) -> Vec3 {
    let [_, right, up] = vcod_common::x87::angle_vectors(facing);
    let turn = g_random(rand) as f32;
    let size = f64::from((f64::from(g_random(rand) as f32) * f64::from(curvature)) as f32);
    let radians = (f64::from(turn) * 360.0 * std::f64::consts::PI / 180.0) as f32;
    let across = scale_stored(right, f64::from(radians.cos()) * size);
    mad(across, f64::from(radians.sin()) * size, up)
}

/// `v s` per axis, each product stored as a float.
fn scale_stored(v: Vec3, s: f64) -> Vec3 {
    Vec3::from_array(v.to_array().map(|x| (f64::from(x) * s) as f32))
}

/// A wobble's new angle rates of up to `max` a second (cod4-gsc.md 3.2):
/// any sign the first time; after that pitch turns against the last yaw
/// rate's sign and yaw with the last pitch rate's.
fn wobble_rates(rand: &mut GlibcRand, last: Vec3, max: f32, first: bool) -> Vec3 {
    let low = if first { -1.0 } else { 0.0 };
    let mut rates = Vec3::from_array(std::array::from_fn(|_| g_flrand(rand, low, 1.0) * max));
    if !first {
        if last.y < 0.0 {
            rates.x = -rates.x;
        }
        if last.x > 0.0 {
            rates.y = -rates.y;
        }
    }
    rates
}

/// One attractor's or repulsor's push across the rocket's line, per
/// second (cod4-gsc.md 3.2); `off` runs from the rocket to it. None for
/// one behind the rocket, past its reach, or an attractor on the line.
fn pull(a: &Attractor, off: Vec3, heading: Vec3, speed: f64) -> Option<[f64; 3]> {
    let ahead = dot64(heading, off);
    let dist = f64::from(off.length());
    let reach = f64::from(a.reach);
    if ahead <= 0.0 || dist > reach {
        return None;
    }
    let (way, wide) = across_line(a.attract, mad(off, -ahead, heading))?;
    let wide = f64::from(wide);
    let share = f64::from(((wide / ahead).atan() * f64::from(TWO_OVER_PI)) as f32);
    let falloff = (1.0 - dist / reach) * f64::from(a.strength);
    let push = if a.attract {
        // At most what turns the rocket onto it inside a frame's 20th.
        (share * falloff).min(wide * speed / ahead * 20.0)
    } else {
        (share - 1.0) * falloff
    };
    Some(way.to_array().map(|t| f64::from(t) * push))
}

/// Which way a pull goes off the rocket's line, given `side`, the part of
/// the offset across it, and how far off the line it counts the point.
/// A repulsor on the line, or one whose side points up, pushes straight
/// down; the one above counts as on the line (0x8097ce4 zeroes the
/// length). An attractor on the line has no side to pull to.
fn across_line(attract: bool, side: Vec3) -> Option<(Vec3, f32)> {
    let len = side.length();
    if len < 1e-5 {
        return (!attract).then_some((Vec3::NEG_Z, len));
    }
    let way = side / len;
    Some(if !attract && way.z > 0.0 {
        (Vec3::NEG_Z, 0.0)
    } else {
        (way, len)
    })
}

/// A guided rocket's target seen from its flat heading (0x8095cde): the
/// flat speed and heading of its velocity, and the target ahead, to the
/// left and above.
struct Bearing {
    flat_speed: f32,
    heading: [f32; 2],
    ahead: f32,
    left: f32,
    above: f32,
}

impl Bearing {
    fn new(v: Vec3, from: Vec3, to: Vec3) -> Self {
        let flat_speed = (v.x * v.x + v.y * v.y).sqrt();
        let heading = if flat_speed == 0.0 {
            [0.0; 2]
        } else {
            [v.x / flat_speed, v.y / flat_speed]
        };
        let off = to - from;
        let [hx, hy] = heading.map(f64::from);
        let (ox, oy) = (f64::from(off.x), f64::from(off.y));
        Self {
            flat_speed,
            heading,
            ahead: (hx * ox + hy * oy) as f32,
            left: (hy * ox - hx * oy) as f32,
            above: off.z,
        }
    }

    fn flat_dist(&self) -> f32 {
        (self.ahead * self.ahead + self.left * self.left).sqrt()
    }

    /// The radius of the turn through the target, signed by its side;
    /// `f32::MAX` for one straight ahead or behind.
    fn turn_radius(&self) -> f64 {
        if self.left == 0.0 {
            return f64::from(f32::MAX);
        }
        let (a, l) = (f64::from(self.ahead), f64::from(self.left));
        (a * a + l * l) / (l + l)
    }

    /// A frame of the sideways push, x and y: toward the turn through a
    /// target ahead unless that is tighter than the rocket can turn; a
    /// full turn to the side of one behind unless its turn is within 60 of
    /// the tightest. The x term is stored once ahead, twice behind.
    fn side_push(&self, speed: f64, max_accel: f64) -> (f64, f64) {
        let radius = self.turn_radius();
        let tightest = speed * speed / max_accel;
        let [hx, hy] = self.heading.map(f64::from);
        if self.ahead > 0.0 {
            if self.left == 0.0 || tightest > radius.abs() {
                return (0.0, 0.0);
            }
            let fs = f64::from(self.flat_speed);
            let accel = (2.0 * fs * fs / radius).clamp(-max_accel, max_accel);
            (
                f64::from((hy * accel * FRAME_S) as f32),
                -hx * accel * FRAME_S,
            )
        } else {
            if tightest + 60.0 > radius.abs() {
                return (0.0, 0.0);
            }
            let accel = if self.left > 0.0 {
                max_accel
            } else {
                -max_accel
            };
            let x = f64::from((f64::from((hy * accel) as f32) * FRAME_S) as f32);
            (x, -hx * accel * FRAME_S)
        }
    }

    /// A frame of the push toward the height at which the flat run meets
    /// the target; none when it is behind.
    fn dive_push(&self, vz: f32, max_accel: f64) -> f64 {
        let dist = self.flat_dist();
        let toward = self.ahead / dist;
        if toward < 0.0 {
            return 0.0;
        }
        let rise = (self.flat_speed * toward).abs();
        let want = (f64::from(rise) * f64::from(self.above) / f64::from(dist)) as f32;
        ((f64::from(want) - f64::from(vz)) * 20.0).clamp(-max_accel, max_accel) * FRAME_S
    }

    /// A Hellfire's climb (0x80968eb): a frame of the push toward the
    /// climb that reaches the target's height, within the max slope; None
    /// once it would climb no faster than it does.
    fn climb_push(&self, vz: f32, speed: f64, max_accel: f64) -> Option<f64> {
        let run = if self.ahead > 0.0 && self.flat_speed != 0.0 {
            self.ahead
        } else {
            self.flat_dist()
        };
        let secs = f64::from(run) / speed;
        let rise = ((f64::from(self.above) / secs + 0.5 * max_accel * secs) * f64::from(0.9f32))
            .min(f64::from(self.flat_speed) * HELLFIRE_MAX_SLOPE);
        let vz = f64::from(vz);
        if rise <= vz {
            return None;
        }
        Some(((rise - vz) * 20.0).min(HELLFIRE_UP_ACCEL).max(-max_accel) * FRAME_S)
    }
}

impl Missile4 {
    /// The Sidewinder and Hellfire steering (cod4-gsc.md 3.2): both
    /// pushes worked out off the velocity, then added to it.
    fn steer_at(&mut self, d: &WeaponDef, target: Vec3) {
        let v = self.pos.delta;
        let b = Bearing::new(v, self.pos.base, target);
        let speed = f64::from(d.projectile_speed);
        let max_accel = f64::from(d.max_steering_accel);
        let (px, py) = b.side_push(speed, max_accel);
        let pz = if b.flat_dist() == 0.0 {
            0.0
        } else if d.guided_missile_type == 2 && !self.climbed {
            b.climb_push(v.z, speed, max_accel).unwrap_or_else(|| {
                self.climbed = true;
                0.0
            })
        } else {
            b.dive_push(v.z, max_accel)
        };
        self.pos.delta = Vec3::new(
            (f64::from(v.x) + px) as f32,
            (f64::from(v.y) + py) as f32,
            (f64::from(v.z) + pz) as f32,
        );
    }
}

/// The damage cone's cosine; a cone of 180 or wider reaches all round.
fn cone_cos(d: &WeaponDef) -> f32 {
    if d.damage_cone_angle < 180.0 {
        (f64::from(d.damage_cone_angle) * f64::from(DEG_TO_RAD)).cos() as f32
    } else {
        -1.0
    }
}

/// `eFlags` 0x1000000, which `G_InitGrenadeEntity` sets on a grenade that
/// bounces (cod4-combat.md 10.2).
const BOUNCING: u32 = 0x100_0000;
/// How far behind its surface normal a resting grenade looks for its
/// ground (`G_RunMissile`).
const GROUND_PROBE: f32 = 1.635;
/// The floor snap's trace, over and under the step's end (10.2).
const SNAP_UP: f32 = 0.135;
const SNAP_DOWN: f32 = 1.5;
/// Surface type 7, flesh, which a body's hit box leaves unset.
const SURF_FLESH: u32 = 7 << 20;
/// The travel a dud is left with, so it never arms.
const SPENT: f32 = -1.0e10;

/// A surface's type, `surfaceFlags` bits 20..24.
fn surface_type(flags: u32) -> u32 {
    (flags >> 20) & 0x1f
}

/// A direction as the event parm's byte.
fn normal_byte(n: Vec3) -> u32 {
    vcod_common::net::events::dir_to_byte(n.to_array()) as u32
}

/// What an impact sets off: whether it bursts (`projImpactExplode`, a
/// dud never), its `weapProjExplosion`, the hit's and the splash's means
/// of death.
struct Payload {
    dud: bool,
    armed: bool,
    kind: i32,
    mod_: &'static str,
    splash: &'static str,
}

impl Payload {
    fn of(d: &WeaponDef, m: &Missile4) -> Self {
        let dud = d.projectile_activate_dist > 0 && m.travel < d.projectile_activate_dist as f32;
        let armed = d.proj_impact_explode && !dud;
        let (mod_, splash) = match (armed, m.rocket) {
            (false, _) => ("MOD_IMPACT", ""),
            (true, true) => ("MOD_PROJECTILE", "MOD_PROJECTILE_SPLASH"),
            (true, false) => ("MOD_GRENADE", "MOD_GRENADE_SPLASH"),
        };
        Self {
            dud,
            armed,
            kind: if dud { PROJEXP_DUD } else { d.proj_explosion },
            mod_,
            splash,
        }
    }
}

/// The events one `weapProjExplosion` raises: at a fuse's burst, and at
/// an impact on a surface and on a player or a hit box (none for smoke).
struct ExplosionEvents {
    fuse: u32,
    impact: Option<(u32, u32)>,
}

/// [`ExplosionEvents`] by `weapProjExplosion`, 0 grenade to 6 heavy
/// (cod4-combat.md 10.3, 10.8).
const EXPLOSION_EVENTS: [ExplosionEvents; 7] = {
    const fn row(fuse: u32, surface: u32, body: u32) -> ExplosionEvents {
        ExplosionEvents {
            fuse,
            impact: Some((surface, body)),
        }
    }
    [
        row(
            ev::GRENADE_EXPLODE,
            ev::GRENADE_EXPLODE,
            ev::GRENADE_EXPLODE,
        ),
        row(
            ev::ROCKET_EXPLODE,
            ev::ROCKET_EXPLODE,
            ev::ROCKET_EXPLODE_NOMARKS,
        ),
        row(
            ev::FLASHBANG_EXPLODE,
            ev::FLASHBANG_EXPLODE,
            ev::FLASHBANG_EXPLODE,
        ),
        row(
            ev::CUSTOM_EXPLODE,
            ev::CUSTOM_EXPLODE,
            ev::CUSTOM_EXPLODE_NOMARKS,
        ),
        row(ev::CUSTOM_EXPLODE, ev::DUD_EXPLODE, ev::DUD_EXPLODE),
        ExplosionEvents {
            fuse: ev::CUSTOM_EXPLODE,
            impact: None,
        },
        row(
            ev::GRENADE_EXPLODE,
            ev::GRENADE_EXPLODE,
            ev::GRENADE_EXPLODE,
        ),
    ]
};

fn explosion_events(kind: i32) -> Option<&'static ExplosionEvents> {
    usize::try_from(kind)
        .ok()
        .and_then(|k| EXPLOSION_EVENTS.get(k))
}

fn impact_event(kind: i32, marks: bool) -> Option<u32> {
    let (on_surface, on_body) = explosion_events(kind)?.impact?;
    Some(if marks { on_surface } else { on_body })
}

/// `eFlags` 0x20, which a spent grenade and its burst carry (10.3).
const HIDDEN: u32 = 0x20;
/// The `eFlags` a smoke cloud's burst adds (10.3).
const SMOKE_CLOUD: u32 = 0x9_0000;
/// How long a smoke cloud's burst stays.
const SMOKE_CLOUD_MS: i32 = 61_000;
/// How long a smoke grenade may stay in the air past its fuse.
const SMOKE_AIR_LIMIT_MS: i32 = 60_000;
/// A blast's damage flags, the radius flags 5 (10.3).
const RADIUS_DFLAGS: i32 = 5;
/// The cone cosine of a blast that reaches all round.
const NO_CONE: f32 = -1.0;
/// How far to either side of a body's middle `CanDamage` aims.
const BODY_SIDE: f32 = 15.0;
/// The order `CanDamage` takes its four corners in, as (side, height)
/// signs: a body's flips the height first, an entity's box the side.
const BODY_CORNERS: [(f32, f32); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)];
pub(super) const BOX_CORNERS: [(f32, f32); 4] =
    [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];

/// How `CanDamage` scores its lines. A client takes a third a clear
/// line, all from three on (0x80b23a4..0x80b23c4); any other entity all
/// on its first clear line and nothing without one (0x80b2f46, the five
/// tries from 0x80b2bfa).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LineScore {
    Thirds,
    FirstClear,
}

impl LineScore {
    /// The scale for the lines' outcomes in order, `true` for a clear one;
    /// `FirstClear` stops at the first.
    pub(super) fn of(self, mut clear: impl Iterator<Item = bool>) -> f32 {
        match self {
            Self::Thirds => clear.filter(|&c| c).count().min(3) as f32 / 3.0,
            Self::FirstClear => f32::from(u8::from(clear.any(|c| c))),
        }
    }
}

/// `CanDamage`'s five points: `mid`, then `mid` pushed by `side` and
/// `up` with each pair of `corners`' signs.
pub(super) fn aim_points(mid: Vec3, side: Vec3, up: Vec3, corners: [(f32, f32); 4]) -> [Vec3; 5] {
    let mut points = [mid; 5];
    for (p, (s, u)) in points[1..].iter_mut().zip(corners) {
        *p = mid + side * s + up * u;
    }
    points
}

/// A blast's reach: `explosionRadius`, at least 1.
pub(super) fn blast_radius(d: &WeaponDef) -> f32 {
    (d.explosion_radius as f32).max(1.0)
}

/// A blast's damage at `dist` of its `radius`: the inner damage falling
/// linearly to the outer at the edge.
pub(super) fn falloff(d: &WeaponDef, dist: f32, radius: f32) -> f32 {
    (d.explosion_inner_damage - d.explosion_outer_damage) as f32 * (1.0 - dist / radius)
        + d.explosion_outer_damage as f32
}

/// The contact's time inside the frame: the frame's ms times the
/// fraction, stored as a float and truncated (0x80946ba, 0x80930e3).
fn contact_time(prev: i32, now: i32, fraction: f32) -> i32 {
    prev + ((now - prev) as f32 * fraction) as i32
}

/// `v` mirrored in the plane of `n`, and the stored dot: its first two
/// products are stored as floats, the third kept, and the mirror scales
/// by the kept sum (10.2).
fn mirror(v: Vec3, n: Vec3) -> (Vec3, f32) {
    let xy = f64::from((f64::from(v.x) * f64::from(n.x)) as f32);
    let xy = f64::from((f64::from(v.y) * f64::from(n.y) + xy) as f32);
    let along = f64::from(v.z) * f64::from(n.z) + xy;
    (mad(v, -2.0 * along, n), along as f32)
}

/// The scale a bouncing grenade's mirrored move takes: `parallelBounce`
/// and `perpendicularBounce` for the surface type, mixed by the strike's
/// cosine; `None` for a still move or one leaving the surface. The
/// length, the cosine and the scale stay on the stack (10.2).
fn bounce_factor(d: &WeaponDef, surf: u32, v: Vec3, dot: f32) -> Option<f64> {
    let speed = v.as_dvec3().length();
    let factor = |table: &[f32]| f64::from(table.get(surf as usize).copied().unwrap_or(0.0));
    (speed > 0.0 && dot <= 0.0).then(|| {
        let (along, across) = (factor(&d.parallel_bounce), factor(&d.perpendicular_bounce));
        along + (across - along) * (f64::from(dot) / -speed)
    })
}

/// A tenth of the normal off the surface, its upward part dropped,
/// added on the stack (the float 0.1 at 0x8209648).
fn push_off(at: Vec3, n: Vec3) -> Vec3 {
    let tenth = f64::from(0.1f32);
    let up = f64::from(n.z) * tenth;
    Vec3::new(
        (f64::from(at.x) + f64::from(n.x) * tenth) as f32,
        (f64::from(at.y) + f64::from(n.y) * tenth) as f32,
        (f64::from(at.z) + if up > 0.0 { 0.0 } else { up }) as f32,
    )
}

/// Which of `BounceMissile`'s two contacts turns the tumble.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Landing {
    /// The grenade lies down: the pitch laid on the surface, no draw.
    Rest,
    /// It flies on: the pitch rate draws a change.
    Bounce,
}

/// `MissileLandAngles` (0x80930d4; its rules in cod11-combat.md 12.7,
/// with 1.5's draw; called at 0x8094862 for a bounce, 0x8094c6e for the
/// rest): the tumble evaluated at the contact. Off a wall (normal z at
/// most 0.1) a bounce adds `(rand() & 0x7f) - 63` to the pitch rate. On
/// anything flatter a bounce scales the rate by `G_random() * 0.3 +
/// 0.85`, reversing it within 80 of the surface's pitch; the pitch lies
/// on the surface (upside down past 90) at rest or within 45, keeps past
/// 80 and eases a quarter of the way between.
fn tumble_on_contact(
    apos: &mut Traj,
    at: i32,
    normal: Vec3,
    how: Landing,
    rand: &mut GlibcRand,
) -> Vec3 {
    let mut angles = apos.at(at);
    let rate = &mut apos.delta.x;
    if normal.z <= 0.1 {
        if how == Landing::Bounce {
            let turn = (g_rand(rand) & 0x7f) - 63;
            *rate = normalize360(turn as f32 + *rate);
        }
        return angles;
    }
    let flat = pitch_for_yaw_on_normal(angles.y, normal);
    let off = angle_delta(flat, angles.x);
    let far = off.abs() >= 80.0;
    if how == Landing::Bounce {
        let k = g_random(rand) * f64::from(0.3f32) + f64::from(0.85f32);
        let scaled = (f64::from(*rate) * k) as f32;
        *rate = if far { scaled } else { -scaled };
    }
    let pitch = normalize180(angles.x);
    angles.x = normalize360(if how == Landing::Rest || off.abs() < 45.0 {
        if pitch.abs() <= 90.0 {
            flat
        } else {
            flat + 180.0
        }
    } else if far {
        pitch
    } else {
        off * 0.25 + pitch
    });
    angles
}

/// `AngleNormalize180` as `MissileLandAngles` reads it: the turns kept on
/// the stack, less the nearest whole one.
fn normalize180(a: f32) -> f32 {
    let turns = f64::from(a) * f64::from(0.002_777_777_8_f32);
    ((turns - (turns + 0.5).floor()) * 360.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_falls_under_400_t_squared() {
        let t = Traj {
            tr_type: TR_GRAVITY,
            time: 1000,
            base: Vec3::new(0.0, 0.0, 100.0),
            delta: Vec3::new(10.0, 0.0, 0.0),
        };
        // A second is 1000 times the float 0.001, a hair past 1.
        assert_eq!(t.at(2000), Vec3::new(10.0, 0.0, -300.00003));
        assert_eq!(t.delta_at(1500).z, -400.00003);
    }

    #[test]
    fn the_launch_hides_by_the_speed() {
        // The walk capture's frag: 940 out of the hand, launchTime 29 past
        // its trTime (VERIFIED live, cod4-combat.md 10.2).
        assert_eq!(no_draw_ms(Vec3::new(-664.0, -664.0, 120.0).length()), 29);
        assert_eq!(no_draw_ms(2400.0), 20);
        assert_eq!(no_draw_ms(0.0), 50);
    }

    fn rocket_at(base: Vec3, delta: Vec3) -> Missile4 {
        let mut m = Missile4::fresh(EntId(0, 0), 0, None, base, 0, String::new());
        m.pos.tr_type = TR_INTERPOLATE;
        m.pos.delta = delta;
        m
    }

    /// The steering's two guidance kinds, pinned to the bit: a Sidewinder
    /// ahead and behind its target, a Hellfire climbing and past its climb.
    #[test]
    fn the_steering_pushes_toward_the_target() {
        let def = |guidance| WeaponDef {
            projectile_speed: 6000,
            max_steering_accel: 2000.0,
            guided_missile_type: guidance,
            ..Default::default()
        };
        let cases = [
            (
                1,
                false,
                Vec3::new(5000.0, 300.0, 40.0),
                Vec3::new(4000.0, 2500.0, -900.0),
            ),
            (
                1,
                false,
                Vec3::new(5000.0, 300.0, 40.0),
                Vec3::new(-4000.0, 2500.0, -900.0),
            ),
            (
                1,
                false,
                Vec3::new(1500.0, -20.0, 0.0),
                Vec3::new(9000.0, -30.0, 500.0),
            ),
            (
                2,
                false,
                Vec3::new(3000.0, 1000.0, 0.0),
                Vec3::new(8000.0, 3000.0, 2000.0),
            ),
            (
                2,
                false,
                Vec3::new(3000.0, 1000.0, 900.0),
                Vec3::new(-8000.0, 3000.0, 20.0),
            ),
            (
                2,
                true,
                Vec3::new(3000.0, 1000.0, 900.0),
                Vec3::new(8000.0, 3000.0, 20.0),
            ),
            (
                1,
                false,
                Vec3::new(3000.0, 10.0, 0.0),
                Vec3::new(-100.0, 50000.0, 0.0),
            ),
            (
                1,
                false,
                Vec3::new(3000.0, 10.0, 0.0),
                Vec3::new(-5000.0, 50000.0, 0.0),
            ),
        ];
        let got: Vec<(Vec3, bool)> = cases
            .iter()
            .map(|&(g, climbed, v, tp)| {
                let mut m = rocket_at(Vec3::new(10.0, 20.0, 30.0), v);
                m.climbed = climbed;
                m.steer_at(&def(g), tp);
                (m.pos.delta, m.climbed)
            })
            .collect();
        let want: Vec<(Vec3, bool)> = vec![
            (Vec3::new(5000.0, 300.0, -60.0), false),
            (Vec3::new(5000.0, 300.0, 40.0), false),
            (Vec3::new(1500.0051, -19.611, 78.42386), false),
            (Vec3::new(2997.3872, 1007.83826, 50.0), false),
            (Vec3::new(3000.0, 1000.0, 950.0), false),
            (Vec3::new(2997.3872, 1007.83826, 800.0), true),
            (Vec3::new(2999.88, 46.014496, -0.002039221), false),
            (Vec3::new(2999.6667, 109.99945, 0.0), false),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn the_cone_opens_to_all_round_at_180() {
        let cos = |angle| {
            cone_cos(&WeaponDef {
                damage_cone_angle: angle,
                ..Default::default()
            })
        };
        assert_eq!(cos(180.0), -1.0);
        assert_eq!(cos(360.0), -1.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(cos(60.0), 0.5);
        assert_eq!(cos(100.0).to_bits(), 3_190_935_763);
    }

    #[test]
    fn a_client_takes_thirds_and_an_entity_its_first_clear_line() {
        let lines = [false, true, false, false, false];
        assert_eq!(LineScore::Thirds.of(lines.into_iter()), 1.0 / 3.0);
        assert_eq!(LineScore::FirstClear.of(lines.into_iter()), 1.0);
        assert_eq!(LineScore::Thirds.of([true; 5].into_iter()), 1.0);
        assert_eq!(LineScore::FirstClear.of([false; 5].into_iter()), 0.0);
        assert_eq!(LineScore::Thirds.of([false; 5].into_iter()), 0.0);
    }

    /// A repulsor above the line pushes as one on it: its offset off the
    /// line counts as 0 (0x8097ce4), so the full falloff, along straight
    /// down. One below pushes along its side by the angle's share.
    #[test]
    fn a_repulsor_above_counts_as_on_the_line() {
        let repulsor = Attractor {
            attract: false,
            entity: None,
            point: Vec3::ZERO,
            strength: 100.0,
            reach: 1000.0,
        };
        let falloff = (1.0 - f64::from(Vec3::new(300.0, 0.0, 300.0).length()) / 1000.0) * 100.0;
        let up = pull(&repulsor, Vec3::new(300.0, 0.0, 300.0), Vec3::X, 1500.0).unwrap();
        assert_eq!(up, [0.0, 0.0, falloff]);
        let down = pull(&repulsor, Vec3::new(300.0, 0.0, -300.0), Vec3::X, 1500.0).unwrap();
        let share = f64::from((1.0f64.atan() * f64::from(TWO_OVER_PI)) as f32);
        assert_eq!(down, [0.0, 0.0, -((share - 1.0) * falloff)]);
        assert!(pull(&repulsor, Vec3::new(-10.0, 0.0, 0.0), Vec3::X, 1500.0).is_none());
    }
}
