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
use crate::spectate::{ClientSim, EventRing, PmType};
use glam::Vec3;
use vcod_common::collision::{sound_material, CollisionWorld, CONTENTS_WATER};
use vcod_common::net::events::dir_to_byte;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::net::trajectory::{
    Trajectory, DEFAULT_GRAVITY, TR_GRAVITY, TR_LINEAR, TR_STATIONARY,
};
use vcod_common::pmove::PlayerState;
use vcod_common::weapon::WeaponDef;
use vcod_gsc::{Cx, EntId, ErrorKind, Value};

/// `ET_MISSILE` (11.1).
pub const ET_MISSILE: i32 = 4;
/// `ET_GENERAL`, what the explode flips `eType` to (13.1).
pub const ET_GENERAL: i32 = 0;
pub const EV_GRENADE_BOUNCE: i32 = 177;
pub const EV_GRENADE_EXPLODE: i32 = 178;

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
/// How far the explode looks down for the normal its `eventParm` packs
/// (13.2).
const EXPLODE_TRACE_DOWN: f32 = 16.0;
/// `0x14`, the surface type the water branches of 12.1 and 13.2 both use.
const WATER_SURF_TYPE: i32 = 0x14;
/// How long an exploded missile stays on the wire before it is freed. Not in
/// the sections: the capture's three explodes each ride exactly 300 ms of
/// snapshots past their event and are gone on the next (entity 176,
/// serverTime 225150 to 225450).
const EVENT_VALID_MS: i32 = 300;
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

/// What one `G_RunMissile` did.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Stepped {
    /// It flew, or it was already at rest and did not move at all.
    Flew,
    /// It hit something and the contact was loud enough for event 177.
    Bounced,
    /// It hit something quietly, or came to rest on it.
    Settled,
}

/// One grenade in the air. The entity is the object table's, so a script
/// walking `getEntArray("grenade", "classname")` finds it, but what reaches
/// the wire is built here rather than by `crate::game::wire`: a missile's
/// state is a trajectory and an event ring, none of which lives in the
/// script fields.
pub struct Missile {
    pub id: EntId,
    /// The thrower's client slot, `r.ownerNum`: what the move trace passes
    /// through and who the blast is charged to.
    pub owner: usize,
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
    /// Set once the event has aged out and the entity has been freed.
    gone: bool,
}

/// One blast for the radius damage pass to charge.
pub struct Explosion {
    pub owner: usize,
    pub inflictor: EntId,
    pub weapon: u8,
    pub at: Vec3,
}

/// What one frame of the missile pass produced. `temp` is empty for a
/// grenade: both explode paths write the event onto the missile's own
/// entity rather than spawning a temp entity (13), and it is kept for the
/// missiles that do not.
pub struct MissileFrame {
    pub exploded: Vec<Explosion>,
    pub temp: Vec<crate::game::temp_entity::TempEntity>,
}

#[derive(Default)]
pub struct Missiles {
    live: Vec<Missile>,
    /// `flrand`'s state for the launch tumble. Its own, so a throw draws the
    /// same numbers whatever else the frame did.
    rng: u64,
}

/// Section 11.3: where a throw starts and how fast it leaves, from the
/// thrower's view. Returns `(origin, velocity)`.
pub fn throw_velocity(ps: &PlayerState, def: &WeaponDef) -> (Vec3, Vec3) {
    // The eye with lean, each component truncated toward zero -- retail sets
    // the x87 round-to-zero word for it, where the bullet's muzzle is
    // rounded instead (2.1).
    let origin = ps.view().eye.trunc();
    let (yaw, pitch) = (ps.yaw, ps.pitch);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let velocity = (forward * def.projectile_speed + Vec3::Z * def.projectile_speed_up).trunc();
    // The thrower's own motion along the throw, added after the truncation,
    // which is why a retail `trDelta` is not whole-numbered.
    let n = velocity.normalize_or_zero();
    (origin, velocity + n * ps.velocity.dot(n))
}

/// `BG_EvaluateTrajectoryDelta` for the two types a missile ever carries.
fn delta_at(traj: &Trajectory, at_ms: i32) -> Vec3 {
    match traj.tr_type {
        TR_GRAVITY => {
            let dt = (at_ms - traj.tr_time) as f32 * 0.001;
            traj.delta - Vec3::Z * (DEFAULT_GRAVITY * dt)
        }
        TR_LINEAR => traj.delta,
        _ => Vec3::ZERO,
    }
}

fn normalize_360(deg: f32) -> f32 {
    deg.rem_euclid(360.0)
}

/// `vectoangles`, Q3's, in degrees: pitch, yaw, roll with pitch negated.
fn vectoangles(v: Vec3) -> Vec3 {
    if v.x == 0.0 && v.y == 0.0 {
        return Vec3::new(if v.z > 0.0 { -90.0 } else { 90.0 }, 0.0, 0.0);
    }
    let yaw = normalize_360(v.y.atan2(v.x).to_degrees());
    let forward = (v.x * v.x + v.y * v.y).sqrt();
    let pitch = v.z.atan2(forward).to_degrees();
    Vec3::new(-pitch, yaw, 0.0)
}

impl Missile {
    /// One `G_RunMissile` (12): fly to where the arc says this frame, then
    /// bounce, rest or carry on. The caller checks the fuse afterwards, the
    /// way `G_RunThink` closes retail's frame.
    pub fn step(
        &mut self,
        world: Option<&CollisionWorld>,
        sims: &[(usize, &ClientSim)],
        now_ms: i32,
    ) -> Stepped {
        let prev_ms = std::mem::replace(&mut self.last_run_ms, now_ms);
        let to = self.traj.evaluate(now_ms);
        let travel = to - self.origin;
        // 12.1: a move shorter than this is not traced at all, which is what
        // makes a resting missile free.
        if travel.length() < 0.001 {
            return Stepped::Flew;
        }
        let Some(world) = world else {
            self.origin = to;
            return Stepped::Flew;
        };
        // 12.1's move trace. Retail's clipmask carries a live player's own
        // contents bit, so a player stops a grenade the way a wall does; the
        // link box is the broad phase retail's locational trace starts from,
        // and the normal a box contact reports is the flight reversed where
        // retail's would be the bone's.
        let mut tr = world.shot_trace(self.origin, to);
        let mut hit_player = false;
        if let Some((_, f)) = nearest_player(self.origin, to, self.owner, sims) {
            if f < tr.fraction {
                tr.fraction = f;
                tr.endpos = self.origin + travel * f;
                tr.normal = (self.origin - to).normalize_or_zero();
                tr.surface_flags = 0;
                hit_player = true;
            }
        }
        self.origin = tr.endpos;
        // 12.2's ground snap, into the same trace struct: 12.3 then reads
        // the downward trace and not the move, which is what makes a rolling
        // grenade bounce off the floor below it every frame.
        if tr.fraction >= 1.0 || tr.normal.z > GROUND_NORMAL_Z {
            let down = world.shot_trace(self.origin, self.origin - Vec3::Z * GROUND_SNAP);
            if down.fraction < 1.0 {
                let dz = down.endpos.z + GROUND_SNAP - self.origin.z;
                self.origin.z += dz;
                self.traj.base.z += dz;
                tr = down;
                hit_player = false;
            }
        }
        if tr.fraction >= 1.0 {
            // 12.3: nothing was hit, and a moving missile has no ground.
            if self.traj.delta != Vec3::ZERO {
                self.ground = ENTITYNUM_NONE as i32;
            }
            return Stepped::Flew;
        }
        // A sky brush frees the missile silently (12.3). Ours are not in the
        // collision world at all, so a grenade thrown at the sky flies on
        // and goes off on its fuse instead.
        self.surf_type = sound_material(tr.surface_flags);
        let in_water = world.point_contents(self.origin) & CONTENTS_WATER != 0;
        let loud = self.bounce(prev_ms, now_ms, &tr, hit_player || in_water);
        if loud {
            self.events.add(EV_GRENADE_BOUNCE, self.surf_type);
            Stepped::Bounced
        } else {
            Stepped::Settled
        }
    }

    /// `G_BounceMissile` (12.4). Returns whether the contact was loud enough
    /// for the caller to raise 177.
    fn bounce(
        &mut self,
        prev_ms: i32,
        now_ms: i32,
        tr: &vcod_common::collision::Trace,
        soft: bool,
    ) -> bool {
        // The impact time is interpolated in whole milliseconds, and the
        // fraction is whichever trace 12.3 read -- the downward one on a
        // ground snap, where it is a fraction of 1.5 units and not of the
        // frame. That is retail's arithmetic, and the capture's bounce
        // deltas only come out on it.
        let at = prev_ms + ((now_ms - prev_ms) as f32 * tr.fraction) as i32;
        let v = delta_at(&self.traj, at);
        let n = tr.normal;
        let dot = v.dot(n);
        let reflected = v - n * (2.0 * dot);
        if n.z > GROUND_NORMAL_Z {
            self.ground = ENTITYNUM_WORLD as i32;
        }
        let damped = if soft {
            reflected * BOUNCE_SOFT
        } else {
            // The tangential component keeps 0.75, the normal one 0.3.
            (reflected + n * dot) * BOUNCE_FRICTION - n * (dot * BOUNCE_RESTITUTION)
        };
        self.traj.delta = damped;
        // The angles land wherever the tumble had reached at the impact.
        let landed = self.apos.evaluate(at);
        if n.z > GROUND_NORMAL_Z && damped.length() < REST_SPEED {
            // `G_SetOrigin` and `G_SetAngle`: trType 0, trTime 0, no delta.
            self.traj = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                base: self.origin,
                delta: Vec3::ZERO,
            };
            self.apos = Trajectory {
                tr_type: TR_STATIONARY,
                tr_time: 0,
                tr_duration: 0,
                // Flat on the surface: the capture's resting grenades read
                // pitch 0 with the yaw and roll the tumble left.
                base: Vec3::new(0.0, landed.y, normalize_360(landed.z)),
                delta: Vec3::ZERO,
            };
            return false;
        }
        let mut nudge = n * BOUNCE_NUDGE;
        nudge.z = nudge.z.min(0.0);
        self.origin += nudge;
        self.traj.base = self.origin;
        self.traj.tr_time = now_ms;
        // `G_MissileLandAngles` has not been read: the base it writes is the
        // tumble at the impact time, which the capture pins, and the pitch
        // rate it redraws with is not modelled.
        self.apos.base = Vec3::new(
            normalize_360(landed.x),
            normalize_360(landed.y),
            normalize_360(landed.z),
        );
        self.apos.tr_time = now_ms;
        !soft && (damped - v).length() > BOUNCE_EVENT_SPEED
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
            let down = world.shot_trace(org, org - Vec3::Z * EXPLODE_TRACE_DOWN);
            normal = down.normal;
            self.surf_type = sound_material(down.surface_flags);
            if world.point_contents(org) & CONTENTS_WATER != 0 {
                self.surf_type = WATER_SURF_TYPE;
            }
        }
        self.events
            .add(EV_GRENADE_EXPLODE, dir_to_byte(normal.into()));
        self.exploded_ms = Some(now_ms);
        Explosion {
            owner: self.owner,
            inflictor: self.id,
            weapon: self.weapon,
            at: org,
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
        set("eType", if exploded { ET_GENERAL } else { ET_MISSILE });
        set("eFlags", if exploded { EFLAGS_EXPLODED } else { 0 });
        set("weapon", i32::from(self.weapon));
        set("surfType", self.surf_type);
        set("groundEntityNum", self.ground);
        self.events.write(&mut set);
        self.traj.write(&mut e, p, "pos");
        self.apos.write(&mut e, p, "apos");
        e
    }
}

/// The nearest live player box the segment crosses, and where along it.
/// The owner is skipped: retail passes `r.ownerNum` as the trace's
/// pass-entity, so a thrower cannot bounce a grenade off itself.
fn nearest_player(
    start: Vec3,
    end: Vec3,
    owner: usize,
    sims: &[(usize, &ClientSim)],
) -> Option<(usize, f32)> {
    sims.iter()
        .filter(|(slot, sim)| *slot != owner && sim.pm_type == PmType::Normal && !sim.dead)
        .filter_map(|(slot, sim)| {
            let lo = sim.ps.origin + sim.ps.mins();
            let hi = sim.ps.origin + sim.ps.maxs();
            crate::game::combat::ray_box(start, end, lo, hi).map(|f| (*slot, f))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

impl Missiles {
    /// `fire_grenade` (11): the entity, the two trajectories and the fuse.
    /// `now_ms` is the `level.time` the throw ran at, which is the frame
    /// *before* the one the missile first moves on: retail executes a
    /// usercmd in `SV_ClientThink`, between frames, so the first
    /// `G_RunMissile` after a throw is a full frame of flight (the capture's
    /// first `!missile` line is already past its first bounce).
    #[allow(clippy::too_many_arguments)]
    pub fn fire_grenade(
        &mut self,
        ents: &mut ObjectTable,
        cx: &mut Cx,
        model: i32,
        owner: usize,
        weapon: u8,
        origin: Vec3,
        velocity: Vec3,
        fuse_left_ms: i32,
        now_ms: i32,
    ) -> Result<EntId, ErrorKind> {
        let id = ents.spawn(cx)?;
        // The classname a script's `getEntArray` looks for. `crate::game::wire`
        // puts nothing on the wire for it, which is what keeps the entity
        // from being sent twice.
        if let crate::game::fields::Route::Engine { slot, .. } =
            crate::game::fields::route_entity("classname")
        {
            if let Some(e) = ents.get_mut(id) {
                e.engine[slot] = Value::String(cx.intern_exact("grenade"));
            }
        }
        let mut angles = vectoangles(velocity);
        angles.x = normalize_360(angles.x - LAUNCH_PITCH_LEAD);
        let tumble = Vec3::new(
            self.flrand(-TUMBLE_SPREAD, TUMBLE_SPREAD) + TUMBLE_PITCH,
            0.0,
            self.flrand(-TUMBLE_SPREAD, TUMBLE_SPREAD) + TUMBLE_ROLL,
        );
        let fuse = if fuse_left_ms > 0 {
            fuse_left_ms
        } else {
            DEFAULT_FUSE_MS
        };
        self.live.push(Missile {
            id,
            owner,
            weapon,
            model,
            traj: Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now_ms,
                tr_duration: 0,
                base: origin,
                delta: velocity,
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
            gone: false,
        });
        Ok(id)
    }

    /// One frame of every missile: the move, then its own fuse, then the
    /// exploded ones that have ridden out their event.
    pub fn run(
        &mut self,
        ents: &mut ObjectTable,
        world: Option<&CollisionWorld>,
        sims: &[(usize, &ClientSim)],
        now_ms: i32,
    ) -> MissileFrame {
        let mut frame = MissileFrame {
            exploded: Vec::new(),
            temp: Vec::new(),
        };
        for m in &mut self.live {
            if let Some(at) = m.exploded_ms {
                // `freeAfterEvent`: the entity rides the wire until its
                // event has aged out, and is freed with it.
                if now_ms.wrapping_sub(at) > EVENT_VALID_MS {
                    m.gone = true;
                    ents.free(m.id);
                }
                continue;
            }
            m.step(world, sims, now_ms);
            if now_ms >= m.explode_at_ms {
                frame.exploded.push(m.explode(world, now_ms));
            }
        }
        self.live.retain(|m| !m.gone);
        frame
    }

    /// The live missiles, by entity number, for the snapshot build. They are
    /// `SVF_BROADCAST` (11.1), so the caller adds them past its own PVS cull.
    pub fn entities<'a>(
        &'a self,
        p: &'a Protocol,
    ) -> impl Iterator<Item = (u32, EntityState)> + 'a {
        self.live.iter().map(move |m| (m.id.0, m.to_entity(p)))
    }

    #[cfg(test)]
    fn missiles(&self) -> &[Missile] {
        &self.live
    }

    #[cfg(test)]
    fn missiles_mut(&mut self) -> impl Iterator<Item = &mut Missile> {
        self.live.iter_mut()
    }

    /// `flrand(lo, hi)`, off this pool's own state.
    fn flrand(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * crate::game::host::rand_unit(&mut self.rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::collision::SURFACE_CLIP_EPSILON;
    use vcod_common::net::protocol::PROTOCOL_V1;

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
        let ents = &mut host.ents;
        vm.with_cx(|cx| ms.fire_grenade(ents, cx, 0, 0, 8, origin, velocity, 4000, now))
            .unwrap()
    }

    /// A grenade thrown across a floor bounces, loses speed and settles a
    /// ground snap above the surface: retail's rest is `endpos.z + 1.5`
    /// (section 12.2), and `endpos` sits `SURFACE_CLIP_EPSILON` off the
    /// plane.
    #[test]
    fn a_grenade_bounces_and_comes_to_rest_on_a_floor() {
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
        let mut bounces = 0;
        for f in 1..=80 {
            for m in ms.missiles_mut() {
                if m.step(Some(&world), &[], f * 50) == Stepped::Bounced {
                    bounces += 1;
                }
            }
        }
        let m = &ms.missiles()[0];
        assert!(bounces >= 1, "the floor bounced it at least once");
        assert_eq!(m.traj.tr_type, TR_STATIONARY, "it came to rest");
        let rest = SURFACE_CLIP_EPSILON + GROUND_SNAP;
        assert!(
            (m.traj.base.z - rest).abs() < 0.5,
            "rested at {} rather than {rest}",
            m.traj.base.z
        );
        assert_eq!(m.apos.tr_type, TR_STATIONARY, "the tumble stopped too");
    }

    /// The fuse is absolute: armed at `t` with `f` left, it explodes on the
    /// first frame at or past `t + f` and not before.
    #[test]
    fn the_fuse_is_absolute() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let mut ms = Missiles::default();
        let id = armed(&mut ms, &mut host, &mut vm, Vec3::ZERO, Vec3::ZERO, 10_000);
        assert!(ms
            .run(&mut host.ents, None, &[], 13_950)
            .exploded
            .is_empty());
        let frame = ms.run(&mut host.ents, None, &[], 14_000);
        assert_eq!(frame.exploded.len(), 1);
        assert_eq!(frame.exploded[0].inflictor, id);
        assert_eq!(frame.exploded[0].owner, 0);
        assert!(
            frame.temp.is_empty(),
            "the explode rides the missile's ring"
        );
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
        ms.run(&mut host.ents, None, &[], 4000);
        let e = ms.entities(p).next().unwrap().1;
        assert_eq!(e.field_i32(p, "eType"), ET_GENERAL);
        assert_eq!(e.field_i32(p, "eFlags"), EFLAGS_EXPLODED);
        assert_eq!(e.field_i32(p, "events[0]"), EV_GRENADE_EXPLODE);
        assert_eq!(e.field_i32(p, "eventSequence"), 1);
        assert!(
            host.ents.get(id).is_some(),
            "the entity rides its own event"
        );

        ms.run(&mut host.ents, None, &[], 4300);
        assert_eq!(ms.entities(p).count(), 1, "it rides one more 300 ms");
        ms.run(&mut host.ents, None, &[], 4350);
        assert_eq!(ms.entities(p).count(), 0, "and leaves the wire after that");
        assert!(host.ents.get(id).is_none(), "freed with it");
        // The number goes back to the pool: the next spawn takes it.
        let next = vm.with_cx(|cx| host.ents.spawn(cx)).unwrap();
        assert_eq!(next, id);
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
                if m.step(Some(&world), &[], f * 50) == Stepped::Bounced {
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
            stepped = ms
                .missiles_mut()
                .next()
                .unwrap()
                .step(Some(&world), &sims, f * 50);
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
        let (origin, velocity) = throw_velocity(&ps, &frag());
        assert!(
            (origin - Vec3::new(1200.0, 1760.0, 204.0)).length() < 0.01,
            "{origin}"
        );
        assert!(
            (velocity - Vec3::new(0.0, 960.0, 120.0)).length() < 0.01,
            "{velocity}"
        );
    }

    /// The thrower's own motion along the throw is added after the truncation
    /// (11.3), so a runner's grenade goes further than a stander's.
    #[test]
    fn the_throwers_speed_along_the_throw_is_added() {
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.velocity = Vec3::new(190.0, 0.0, 0.0);
        let (_, velocity) = throw_velocity(&ps, &frag());
        let n = Vec3::new(960.0, 0.0, 120.0).normalize();
        let want = Vec3::new(960.0, 0.0, 120.0) + n * 190.0 * n.x;
        assert!((velocity - want).length() < 0.01, "{velocity} want {want}");
    }
}
