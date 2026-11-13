//! CoD4 1.7's hardpoint vehicles on a [`Cod4Level`]: the airstrike's
//! planes (`spawnPlane`, `ET_PLANE` 13) and the helicopter
//! (`spawnHelicopter`, `ET_HELICOPTER` 12) with its script-driven flight,
//! turret and guns (docs/research/cod4-gsc.md 3.2). The flight is 1.7's
//! think at 0x809e38a (`VEH_UpdateMoveToGoal`, the turn and tilt inlined):
//! the move, the yaw, the body's pitch and roll off the acceleration and
//! the hover's wander; then the vehicle think's move trace and touch
//! (0x809fbd6). Its collision and tags pose the turret and barrel as its
//! controller does (0x80bab1e).

use std::collections::BTreeMap;

use super::combat::WEAPTYPE_PROJECTILE;
use glam::{Quat, Vec3};
use vcod_cod4::net::delta::EntityState;
use vcod_common::turretpose::angles_quat;
use vcod_gsc::{EntId, Value};

use super::clip::Cod4Clip;
use super::combat::{BULLET_PRIORITY, BULLET_RANGE};
use super::level::Cod4Level;
use super::{Cod4HostState, ModelTags};

/// 1.7's `ET_HELICOPTER` and `ET_PLANE`.
pub const ET_HELICOPTER: u32 = 12;
pub const ET_PLANE: u32 = 13;
/// `TR_INTERPOLATE`: `VEH_SetPosition` sends both trajectories so.
const TR_INTERPOLATE: u32 = 1;
use crate::game::builtins::cod4_vehicle::TR_PHYSICS;
/// The vehicle think's step: 0.05 s whatever `sv_fps`, as its think runs
/// every 50 ms of level time (`nextthink = level.time + 50`, 0x80a03bb;
/// cod4-server-frame.md 8).
const DT: f32 = 0.05;
/// How far on the think's `nextthink` lies.
const THINK_MS: i32 = 50;
/// `EV_FIRE_WEAPON` (1.7's 26, `vcod_cod4::pmove::weapon::ev`): the
/// single-barrel `fireWeapon`'s event on the vehicle.
const EV_FIRE_WEAPON: u32 = 26;
/// Miles an hour in units a second: the script's speeds are mph.
pub const MPH: f32 = 17.6;
/// The pitch's and the roll's top rate, degrees a second
/// (`VEH_InitPhysics`, 0x809d13a).
const MAX_TILT_VEL: f32 = 45.0;

/// The vehicle file's fields the helicopter reads (`vehicles/<name>`, a
/// rawfile of `common_mp`; `cobra_mp` reads `turretRotRate` 80, spans 120,
/// 120, 25 up, 120 down, `accel` 20 and every `*Damage` key 1, VERIFIED).
/// The defaults are `cobra_mp`'s.
#[derive(Clone, Copy, Debug)]
pub struct VehInfo {
    pub turret_rot_rate: f32,
    pub span_left: f32,
    pub span_right: f32,
    pub span_up: f32,
    pub span_down: f32,
    /// `accel`, mph a second times 17.6 at the load (`G_LoadVehicle`).
    pub accel: f32,
    /// What `G_VehImmuneToDamage` lets through: `bulletDamage`,
    /// `armorPiercingDamage`, `grenadeDamage`, `projectileDamage`,
    /// `projectileSplashDamage`, `heavyExplosiveDamage`.
    pub bullet_damage: bool,
    pub armor_piercing_damage: bool,
    pub grenade_damage: bool,
    pub projectile_damage: bool,
    pub projectile_splash_damage: bool,
    pub heavy_explosive_damage: bool,
    /// `suspensionTravel`, which each think sends in `s.time2` times
    /// 1000 (0x80a03b8).
    pub suspension_travel: f32,
}

impl Default for VehInfo {
    fn default() -> Self {
        VehInfo {
            turret_rot_rate: 80.0,
            span_left: 120.0,
            span_right: 120.0,
            span_up: 25.0,
            span_down: 120.0,
            accel: 20.0 * MPH,
            bullet_damage: true,
            armor_piercing_damage: true,
            grenade_damage: true,
            projectile_damage: true,
            projectile_splash_damage: true,
            heavy_explosive_damage: true,
            suspension_travel: 0.0,
        }
    }
}

impl VehInfo {
    /// The `VEHICLEFILE\key\value\...` text's turret fields; anything
    /// missing keeps the default.
    pub fn parse(text: &str) -> VehInfo {
        let mut v = VehInfo::default();
        let mut parts = text.split('\\').skip(1);
        while let (Some(k), Some(val)) = (parts.next(), parts.next()) {
            let Ok(x) = val.trim().parse::<f32>() else {
                continue;
            };
            match k {
                "turretRotRate" => v.turret_rot_rate = x,
                "turretHorizSpanLeft" => v.span_left = x,
                "turretHorizSpanRight" => v.span_right = x,
                "turretVertSpanUp" => v.span_up = x,
                "turretVertSpanDown" => v.span_down = x,
                "accel" => v.accel = x * MPH,
                "bulletDamage" => v.bullet_damage = x != 0.0,
                "armorPiercingDamage" => v.armor_piercing_damage = x != 0.0,
                "grenadeDamage" => v.grenade_damage = x != 0.0,
                "projectileDamage" => v.projectile_damage = x != 0.0,
                "projectileSplashDamage" => v.projectile_splash_damage = x != 0.0,
                "heavyExplosiveDamage" => v.heavy_explosive_damage = x != 0.0,
                "suspensionTravel" => v.suspension_travel = x,
                _ => {}
            }
        }
        v
    }

    /// The `turretWeapon` of a `VEHICLEFILE` text: what `s.weapon` starts
    /// as (`VEH_InitEntity`, 0x809c650).
    pub fn turret_weapon(text: &str) -> Option<&str> {
        let mut parts = text.split('\\').skip(1);
        while let (Some(k), Some(val)) = (parts.next(), parts.next()) {
            if k == "turretWeapon" {
                return Some(val.trim());
            }
        }
        None
    }

    /// `G_VehImmuneToDamage`: whether a hit of this means of
    /// death (and damage flags, 2 armor piercing) passes the vehicle by; a
    /// grenade's blast reads the heavy key for a `heavy` explosion;
    /// `MOD_EXPLOSIVE` always hurts and anything else (melee, impact,
    /// crush) never does.
    pub fn immune(&self, mod_: &str, dflags: i32, heavy: bool) -> bool {
        match mod_ {
            "MOD_PISTOL_BULLET" | "MOD_RIFLE_BULLET" => {
                !self.bullet_damage && (dflags & 2 == 0 || !self.armor_piercing_damage)
            }
            "MOD_GRENADE" | "MOD_GRENADE_SPLASH" if heavy => !self.heavy_explosive_damage,
            "MOD_GRENADE" | "MOD_GRENADE_SPLASH" => !self.grenade_damage,
            "MOD_PROJECTILE" => !self.projectile_damage,
            "MOD_PROJECTILE_SPLASH" => !self.projectile_splash_damage,
            "MOD_EXPLOSIVE" => false,
            _ => true,
        }
    }
}

/// Where `setTurretTargetEnt` or `setTurretTargetVec` points the gun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TurretTarget {
    Ent(EntId, Vec3),
    Pos(Vec3),
}

/// One helicopter: its entity, owner and what the script's methods set
/// and the think reads.
#[derive(Clone, Debug)]
pub struct Heli4 {
    pub id: EntId,
    pub owner: usize,
    /// `lerp.u.vehicle.teamAndOwnerIndex`: team | owner slot << 2.
    pub team_owner: u32,
    /// The model `spawnHelicopter` set (its tags place the guns) and its
    /// configstring index, which 1.7 sends in `un2` with `index` 0.
    pub model: String,
    pub model_index: u32,
    pub info: VehInfo,
    /// `un1.helicopterStage`, `setDamageStage`'s.
    pub stage: u32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub vel: Vec3,
    /// The velocity change a think, smoothed: what the body tilts by.
    pub smoothed_change: Vec3,
    pub speed: f32,
    /// Pitch, yaw and roll rates, degrees a second.
    pub angle_rates: [f32; 3],
    /// `setSpeed`'s.
    pub pace: Pace,
    /// `setYawSpeed`'s, and the slow-down past the goal yaw.
    pub yaw: YawTurn,
    pub max_pitch: f32,
    pub max_roll: f32,
    pub turning_ability: f32,
    pub near_goal_dist: f32,
    /// `setAirResistance`: the speed at which the fake drag is full.
    pub air_resistance: f32,
    pub hover: Hover,
    /// The entity's `health`, 99999 from the first think
    /// (`Heli_InitFirstThink`); damage takes it down.
    pub health: i32,
    /// `flags & 2`: a goal was set.
    pub moving: bool,
    pub goal: Vec3,
    pub stop_at_goal: bool,
    pub stopping: bool,
    /// `moveState`: hovering once a stop goal was reached (else moving).
    pub hovering: bool,
    pub goal_yaw: Option<f32>,
    pub target_yaw: Option<f32>,
    /// `s.weapon`, `setVehWeapon`'s.
    pub weapon: u32,
    pub gun_pitch: f32,
    pub gun_yaw: f32,
    pub target: Option<TurretTarget>,
    /// `VEH_UpdateAim`'s turning state (vehicle +0x1e4): 2 while the gun
    /// turns far, run down a think at a time to `turret_rotate_stopped`.
    pub turret_turning: u8,
    /// Rounds fired, which vary the spread's draw within a frame.
    pub shots: u32,
    /// Spawned this frame: `VEH_InitEntity` sets `nextthink` a frame on
    /// (0x809c650), so the first think is the next frame's.
    pub fresh: bool,
    /// `nextthink`: the level time the next think is due on.
    pub next_think: i32,
    /// A think has run: `s.time2` carries `suspensionTravel` from then on.
    pub thought: bool,
}

/// A `fireWeapon` round for the level's next vehicle pass: the
/// helicopter, the weapon, the tag it leaves from, the target given, and
/// the missile entity a projectile weapon already handed the script.
#[derive(Clone, Debug)]
pub struct VehShot {
    pub heli: EntId,
    pub weapon: u32,
    pub tag: Option<String>,
    pub target: Option<EntId>,
    /// `fireWeapon`'s third argument: where on the target a guided rocket
    /// steers.
    pub target_offset: Vec3,
    pub missile: Option<EntId>,
}

/// A trace's contact with a helicopter's collision ([`Cod4Level::heli_trace`]).
#[derive(Clone, Copy, Debug)]
pub struct HeliHit {
    pub id: EntId,
    pub fraction: f32,
    /// The struck triangle's plane normal in the world.
    pub normal: Vec3,
    pub surface_flags: u32,
}

/// `PROJEXP_HEAVY`: a heavy explosion reads `heavyExplosiveDamage`.
const PROJEXP_HEAVY: i32 = 6;

/// The move trace's mask (`cod4_lnxded` 0x80a055f).
const MASK_VEHICLE_MOVE: u32 = 0x2081;
/// `ENTITYNUM_WORLD` and `ENTITYNUM_NONE`.
const WORLD: u32 = 1022;
const NONE: u32 = 1023;
/// `stickiness` "stick to all" (`WeaponDef` +0x5a4), what the touch pass
/// carries (0x80a046a).
const STICK_TO_ALL: i32 = 1;

/// A bone's move off its bind place: the turn and the shift that take a
/// point of the model's space at the bind pose to its posed place.
pub(crate) type BoneMove = (Quat, Vec3);

/// The helicopter's DObj as its controller poses it (handler 23's,
/// `cod4_lnxded` 0x80bab1e) on every pose: `tag_body` set to (bodyPitch, 0,
/// bodyRoll), `tag_turret` to (0, gunYaw, 0) and `tag_barrel` to
/// (gunPitch, 0, 0), each a local tag (0x81abb2a) whose slot takes the
/// angles' quat and a zero shift (0x81b010a..0x81b0127). The set angles
/// stand in for the bone's own bind turn, its place kept: on the cobra
/// `tag_barrel` sits 7.3 degrees nose up at the bind pose, and retail's
/// `tag_flash` points along the gun's pitch (`play-1.7-heli-collide.txt`).
/// A helicopter leaves its body angles 0 (`VEH_InitEntity` 0x809c6d6, and
/// the captures). For each bone in `tags`' order its move, `None` where
/// nothing on its chain changed.
pub(crate) fn heli_pose(
    tags: &ModelTags,
    parents: &[i32],
    gun_pitch: f32,
    gun_yaw: f32,
) -> Vec<Option<BoneMove>> {
    let mut posed: Vec<(Vec3, Quat)> = Vec::with_capacity(tags.len());
    let mut moves: Vec<Option<BoneMove>> = Vec::with_capacity(tags.len());
    for (b, (name, p, q)) in tags.iter().enumerate() {
        let set = match name.as_str() {
            "tag_body" => Some(angles_quat([0.0; 3])),
            "tag_turret" => Some(angles_quat([0.0, gun_yaw, 0.0])),
            "tag_barrel" => Some(angles_quat([gun_pitch, 0.0, 0.0])),
            _ => None,
        };
        let parent = parents
            .get(b)
            .copied()
            .filter(|&i| i >= 0 && (i as usize) < b)
            .map(|i| i as usize);
        let ((bp, bq), (pp, pq)) = match parent {
            Some(i) => ((tags[i].1, tags[i].2), posed[i]),
            None => ((Vec3::ZERO, Quat::IDENTITY), (Vec3::ZERO, Quat::IDENTITY)),
        };
        let local = (bq.inverse() * (*p - bp), bq.inverse() * *q);
        let parent_moved = parent.is_some_and(|i| moves[i].is_some());
        let changed = set.is_some_and(|s| s != local.1);
        if !changed && !parent_moved {
            posed.push((*p, *q));
            moves.push(None);
            continue;
        }
        let local_q = set.unwrap_or(local.1);
        let (wp, wq) = (pp + pq * local.0, pq * local_q);
        posed.push((wp, wq));
        let rot = wq * q.inverse();
        moves.push(Some((rot, wp - rot * *p)));
    }
    moves
}

/// [`heli_pose`] for one helicopter; empty when its model has no tags.
fn heli_pose_of(c4: &Cod4HostState, h: &Heli4) -> Vec<Option<BoneMove>> {
    let key = h.model.to_ascii_lowercase();
    match (c4.model_tags.get(&key), c4.model_rigs.get(&key)) {
        (Some(tags), Some(rig)) => heli_pose(tags, &rig.parents, h.gun_pitch, h.gun_yaw),
        _ => Vec::new(),
    }
}

/// A helicopter's tag posed ([`heli_pose`]) in the world: its origin and
/// turn; `None` when the model has no such tag.
pub(crate) fn heli_tag(c4: &Cod4HostState, h: &Heli4, tag: &str) -> Option<(Vec3, Quat)> {
    let tags = c4.model_tags.get(&h.model.to_ascii_lowercase())?;
    let i = tags
        .iter()
        .position(|(n, _, _)| n.eq_ignore_ascii_case(tag))?;
    let (p, q) = heli_bone(c4, h, i)?;
    Some((h.origin + h.quat() * p, h.quat() * q))
}

/// Bone `i` of a helicopter's model posed ([`heli_pose`]), in the model's
/// space: its place and turn.
fn heli_bone(c4: &Cod4HostState, h: &Heli4, i: usize) -> Option<(Vec3, Quat)> {
    let (_, p, q) = *c4.model_tags.get(&h.model.to_ascii_lowercase())?.get(i)?;
    Some(match heli_pose_of(c4, h).get(i).copied().flatten() {
        Some((rot, shift)) => (rot * p + shift, rot * q),
        None => (p, q),
    })
}

/// What `VEH_UpdateAim` aims from and in (cod4-gsc.md 3.2, "The turret"):
/// `tag_barrel`'s place (`G_DObjGetWorldTagPos` 0x80cbfc4, the bone's
/// place through the body's [`axis4`] plus the origin, 0x819016e) and the
/// angles of bone 0's matrix (`G_DObjGetWorldTagMatrix` 0x80cbe9a, the
/// bone's turn as a matrix times the body's axis, then
/// [`axis_to_angles4`]): the vehicle's flash-tag slot
/// (+0x320) is never written, so it names bone 0. `None` without a
/// `tag_barrel`, when the aim does not run (0x80a004c).
fn heli_aim_from(c4: &Cod4HostState, h: &Heli4) -> Option<(Vec3, Vec3)> {
    let tags = c4.model_tags.get(&h.model.to_ascii_lowercase())?;
    let barrel = tags
        .iter()
        .position(|(n, _, _)| n.eq_ignore_ascii_case("tag_barrel"))?;
    let axis = axis4(h.angles);
    // `MatrixMultiply` (0x818effc): each row's last product first.
    let through = |m: Vec3| {
        let c = |k: usize| {
            ((d(m.z) * d(axis[2][k]) + d(m.y) * d(axis[1][k])) + d(m.x) * d(axis[0][k])) as f32
        };
        Vec3::new(c(0), c(1), c(2))
    };
    let (p, _) = heli_bone(c4, h, barrel)?;
    let at = |k: usize| {
        (((d(p.x) * d(axis[0][k]) + d(p.y) * d(axis[1][k])) + d(p.z) * d(axis[2][k]))
            + d(h.origin[k])) as f32
    };
    let place = Vec3::new(at(0), at(1), at(2));
    let (_, q) = heli_bone(c4, h, 0)?;
    // The bone's turn as 1.7 builds it: each row the turned unit axis.
    let world = [Vec3::X, Vec3::Y, Vec3::Z].map(|e| through(q * e));
    Some((place, axis_to_angles4(world)))
}

/// The first helicopter but `skip` whose model collision the segment
/// meets before `limit`: `SV_ClipMoveToEntity`'s `svFlags & 4` arm,
/// `DObjGeomTraceline` over the model's `collSurfs` under `mask`
/// (`r.contents` 0x2080, `VEH_InitEntity`, INFERRED). A surface whose bone
/// the controller moved ([`heli_pose`]) takes the segment back into the
/// bind pose's space (`XModelTraceLineAnimated`, 0x81c63e8..0x81c671b);
/// the rest trace it as it is (the bind pose compare, 0x81c6298). The
/// server runs no rotor anim.
pub(crate) fn heli_trace(
    host: &crate::game::host::GameHost,
    start: Vec3,
    end: Vec3,
    limit: f32,
    skip: Option<EntId>,
    mask: u32,
) -> Option<HeliHit> {
    let c4 = host.cod4();
    let mut order: Vec<&Heli4> = c4.helis.iter().filter(|h| Some(h.id) != skip).collect();
    order.sort_by_key(|h| h.id.0);
    let mut best: Option<HeliHit> = None;
    let mut tr = vcod_common::clipmap::ClipTrace::clear();
    tr.fraction = limit;
    for h in order {
        if mask & 0x2080 == 0 || host.ents.get(h.id).is_none() {
            continue;
        }
        let Some(surfs) = c4.model_coll.get(&h.model.to_ascii_lowercase()) else {
            continue;
        };
        // The broad test: the sphere round every surface's box.
        let radius = surfs
            .iter()
            .flat_map(|s| [Vec3::from(s.mins), Vec3::from(s.maxs)])
            .map(|v| v.abs().length())
            .fold(0.0f32, f32::max);
        let seg = end - start;
        let t =
            ((h.origin - start).dot(seg) / seg.length_squared().max(1e-6)).clamp(0.0, tr.fraction);
        if (start + seg * t).distance(h.origin) > radius {
            continue;
        }
        let axis = vcod_common::x87::angles_to_axis(h.angles);
        let local = |p: Vec3| {
            let d = p - h.origin;
            Vec3::new(d.dot(axis[0]), d.dot(axis[1]), d.dot(axis[2]))
        };
        let pose = heli_pose_of(c4, h);
        if trace_posed(surfs, &pose, local(start), local(end), mask, &mut tr) {
            let n = tr.normal;
            best = Some(HeliHit {
                id: h.id,
                fraction: tr.fraction,
                normal: axis[0] * n.x + axis[1] * n.y + axis[2] * n.z,
                surface_flags: tr.surface_flags,
            });
        }
    }
    best
}

/// `XModelTraceLine` over `surfs` posed by `pose` ([`heli_pose`]), the
/// segment in the model's space: a surface whose bone moved takes the
/// segment back into the bind pose's space (`XModelTraceLineAnimated`,
/// 0x81c63e8..0x81c671b), the rest trace it as it is (the bind pose
/// compare, 0x81c6298). Whether any surface was met, `tr` holding the
/// nearest. Its normal is the triangle's own, in the bind pose's space
/// even on a moved bone: the hit stores the plane's normal as it is
/// (0x81c6940..0x81c6954).
fn trace_posed(
    surfs: &[vcod_cod4::assets::model::XModelCollSurf],
    pose: &[Option<BoneMove>],
    start: Vec3,
    end: Vec3,
    mask: u32,
    tr: &mut vcod_common::clipmap::ClipTrace,
) -> bool {
    let mut met = false;
    for s in surfs {
        let moved = pose.get(s.bone as usize).copied().flatten();
        let (a, b) = match moved {
            Some((rot, shift)) => (
                rot.inverse() * (start - shift),
                rot.inverse() * (end - shift),
            ),
            None => (start, end),
        };
        met |= vcod_cod4::collision::trace_line(std::slice::from_ref(s), a, b, mask, tr);
    }
    met
}

/// Where a point riding a body goes when the body moves from (`from`,
/// `from_angles`) to (`to`, `to_angles`): kept in the body's frame
/// (`cod4_lnxded` 0x809c462: the offset through the old axis transposed
/// times the new, added to the new origin), its turn with it.
fn carried(
    (from, from_angles): (Vec3, Vec3),
    (to, to_angles): (Vec3, Vec3),
    (at, angles): (Vec3, Vec3),
) -> (Vec3, Vec3) {
    let turn = angles_quat(to_angles.to_array()) * angles_quat(from_angles.to_array()).inverse();
    let q = turn * angles_quat(angles.to_array());
    (
        to + turn * (at - from),
        Vec3::from(vcod_common::turretpose::quat_angles(q)),
    )
}

/// `AngleNormalize180` the vehicle code's way: `(x - floor(x + 0.5)) * 360`.
fn norm180(a: f32) -> f32 {
    let x = a * (1.0 / 360.0);
    (x - (x + 0.5).floor()) * 360.0
}

/// `speed` toward `target` by at most `accel * dt`.
fn accelerate(speed: f32, target: f32, accel: f32, dt: f32) -> f32 {
    if target <= speed {
        (speed - accel * dt).max(target)
    } else {
        (speed + accel * dt).min(target)
    }
}

/// `vectoyaw`.
fn vectoyaw(v: Vec3) -> f32 {
    crate::game::builtins::math::vectoangles(v.to_array())[1]
}

/// A float on the x87 stack (an `f64` here).
fn d(x: f32) -> f64 {
    f64::from(x)
}

/// The double `M_PI / 180` 1.7's angle helpers multiply by (0x820a288).
const DEG2RAD: f64 = 0.017_453_292_519_943_295;

/// The float `1 / 360` of 1.7's inlined angle normalisers (0x8209630).
const INV_360: f32 = 0.002_777_777_8;

/// `AngleNormalize180` with the turns kept on the stack:
/// `(x - floor(x + 0.5)) * 360`, `x` the angle times [`INV_360`].
fn norm180_kept(a: f64) -> f32 {
    let x = a * d(INV_360);
    ((x - (x + 0.5).floor()) * 360.0) as f32
}

/// `AnglesToAxis` (`cod4_lnxded` 0x818e910): forward, left, up. Each
/// angle times [`DEG2RAD`] stored as a float through `sinf` and `cosf`;
/// forward's products stored as floats, left's and up's sums of products
/// kept until one store.
fn axis4(a: Vec3) -> [Vec3; 3] {
    let sc = |deg: f32| {
        let r = (d(deg) * DEG2RAD) as f32;
        (d(r.sin()), d(r.cos()))
    };
    let (sy, cy) = sc(a.y);
    let (sp, cp) = sc(a.x);
    let (sr, cr) = sc(a.z);
    let forward = Vec3::new((cy * cp) as f32, (sy * cp) as f32, -sp as f32);
    let (spr, spc) = (sp * sr, sp * cr);
    let left = Vec3::new(
        (cy * spr - sy * cr) as f32,
        (spr * sy + cy * cr) as f32,
        (cp * sr) as f32,
    );
    let up = Vec3::new(
        (sy * sr + cy * spc) as f32,
        (spc * sy - sr * cy) as f32,
        (cr * cp) as f32,
    );
    [forward, left, up]
}

/// `vectoangles` (`cod4_lnxded` 0x8191a72): each `atan2` in double times
/// 180 (or -180) over the double pi, stored as a float, 360 added to a
/// negative one and 0 to the rest (so a -0 comes back +0); the pitch's
/// flat length kept as a double.
fn vectoangles4(v: Vec3) -> Vec3 {
    if v.y == 0.0 && v.x == 0.0 {
        return Vec3::new(if -v.z < 0.0 { 270.0 } else { 90.0 }, 0.0, 0.0);
    }
    let pi = std::f64::consts::PI;
    let wrap = |a: f32| (d(if a >= 0.0 { 0.0 } else { 360.0 }) + d(a)) as f32;
    let yaw = (d(v.y).atan2(d(v.x)) * 180.0 / pi) as f32;
    let flat = (d(v.x) * d(v.x) + d(v.y) * d(v.y)).sqrt();
    let pitch = (d(v.z).atan2(flat) * -180.0 / pi) as f32;
    Vec3::new(wrap(pitch), wrap(yaw), 0.0)
}

/// `AxisToAngles` (`cod4_lnxded` 0x81951fe): pitch and yaw off the
/// forward row ([`vectoangles4`]); the roll off the left row turned back
/// through the yaw and the pitch, each `sinf`/`cosf` of a stored radian.
fn axis_to_angles4(m: [Vec3; 3]) -> Vec3 {
    let a = vectoangles4(m[0]);
    let l = m[1];
    let sc = |deg: f32| {
        let r = (-d(deg) * DEG2RAD) as f32;
        (d(r.sin()), d(r.cos()))
    };
    let (s1, c1) = sc(a.y);
    let t = (d((d(l.x) * c1) as f32) - d(l.y) * s1) as f32;
    let v = (d(l.x) * s1 + c1 * d(l.y)) as f32;
    let (s2, c2) = sc(a.x);
    let w = d(t) * c2 + d(l.z) * s2;
    let z = d(l.z) * c2 - d(t) * s2;
    let roll = if v == 0.0 && w == 0.0 {
        -(if -z >= 0.0 { 90.0 } else { -90.0 })
    } else {
        let r = (z.atan2((d(v) * d(v) + w * w).sqrt()) * -180.0 / std::f64::consts::PI) as f32;
        if v < 0.0 {
            (if r < 0.0 { 180.0 } else { -180.0 } + d(r)) as f32
        } else {
            -r
        }
    };
    Vec3::new(a.x, a.y, roll)
}

/// `to - from` normalized as `VEH_UpdateAim` does (0x80a00c3..0x80a0119):
/// the length and its reciprocal kept, each stored difference times it
/// stored.
fn normalize4(to: Vec3, from: Vec3) -> Vec3 {
    let dv = [0, 1, 2].map(|k| d(to[k]) - d(from[k]));
    let len = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
    let inv = if -len < 0.0 { 1.0 / len } else { 1.0 };
    Vec3::from(dv.map(|x| (d(x as f32) * inv) as f32))
}

/// `AnglesSubtract`'s angle (`cod4_lnxded` 0x819397a): `a - b`
/// normalised on the stack, stored.
fn angle_sub4(a: f32, b: f32) -> f32 {
    norm180_kept(d(a) - d(b))
}

/// The turret's angle step (`cod4_lnxded` 0x8194154): `tgt` brought
/// within 180 of `cur` by whole turns, then `cur` toward it by `rate`
/// a think unless the gap is within a thousandth or the step would pass
/// it (then the target itself), normalised on the stack.
fn track4(tgt: f32, cur: f32, rate: f32) -> f32 {
    let (mut a, b) = (d(tgt), d(cur));
    while a - b > 180.0 {
        a -= 360.0;
    }
    while a - b < -180.0 {
        a += 360.0;
    }
    let gap = a - b;
    let step = if gap > 0.0 { d(rate) } else { -d(rate) } * d(DT);
    let far = gap.abs() as f32 > 0.001 && step.abs() as f32 <= gap.abs() as f32;
    norm180_kept(if far { b + step } else { a })
}

/// A gun angle held to `-below ..= above` as `VEH_UpdateAim` holds it
/// (0x80a0235..0x80a0285): the top first, then the bottom off the angle
/// as it came.
fn clamp4(a: f32, below: f32, above: f32) -> f32 {
    if -d(below) - d(a) >= 0.0 {
        -below
    } else if d(a) - d(above) < 0.0 {
        a
    } else {
        above
    }
}

/// A speed and the rates up to it and down from it, units a second.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pace {
    pub speed: f32,
    pub accel: f32,
    pub decel: f32,
}

/// The yaw's turn: its top rate and the rates up to it and back down
/// (degrees a second), the share of the stopping arc it may run past,
/// and the slow-down to a fifth once it swung past the goal yaw, held
/// while the wanted yaw stays `slow_for`. `VEH_InitPhysics` (0x809d13a)
/// starts it at 90, 25, 15 and 0.1.
#[derive(Clone, Copy, Debug)]
pub struct YawTurn {
    pub top: f32,
    pub up: f32,
    pub down: f32,
    pub overshoot: f32,
    slow_for: f32,
    slowed: bool,
}

/// `setHoverParams`' radius, speed and rate (units, not mph), the wander
/// off the goal the hover flies to, and whether a hovering body has
/// settled level, after which it tilts at the hover's rate.
#[derive(Clone, Copy, Debug)]
pub struct Hover {
    pub radius: f32,
    pub speed: f32,
    pub rate: f32,
    pub wander: Vec3,
    settled: bool,
}

impl Hover {
    /// The hover flies at its own speed and rate, braking at half the rate.
    fn pace(&self) -> Pace {
        Pace {
            speed: self.speed,
            accel: self.rate,
            decel: self.rate * 0.5,
        }
    }

    /// A new wander: `G_flrand(-radius, radius)` on x, y and z in turn,
    /// less half the last one; none with no radius.
    fn next_wander(&self, rand: &mut vcod_common::rng::GlibcRand) -> Vec3 {
        if self.radius == 0.0 {
            return Vec3::ZERO;
        }
        let r = self.radius;
        let draw: [f32; 3] = std::array::from_fn(|_| super::missile::g_flrand(rand, -r, r));
        Vec3::from(draw) - self.wander * 0.5
    }
}

/// One angle's turn for a think: the top rate, the rates up to it and
/// down from it, and the share of the stopping arc it may overshoot.
struct Rates {
    top: f32,
    up: f32,
    down: f32,
    overshoot: f32,
}

impl Rates {
    /// `angle` toward `goal` the short way: the rate climbs toward the top
    /// until the stopping arc (less the overshoot) covers what is left,
    /// then falls toward 0; a rate already turning away climbs back. Once
    /// both the rate and the gap fit inside a think, it lands on `goal`.
    fn steer(&self, angle: &mut f32, rate: &mut f32, goal: f32) {
        let left = norm180(goal - *angle);
        if left == 0.0 && *rate == 0.0 {
            return;
        }
        let speed = rate.abs();
        let arc = (1.0 - self.overshoot) * speed * 0.5 * (speed / self.down);
        let (toward, by) = if left * *rate < 0.0 || arc < left.abs() {
            (self.top, self.up)
        } else {
            (0.0, self.down)
        };
        if speed >= by * DT || left.abs() >= speed * DT {
            let target = if left < 0.0 { -toward } else { toward };
            *rate = accelerate(*rate, target, by, DT);
            *angle = norm180(*rate * DT + *angle);
        } else {
            *angle = goal;
            *rate = 0.0;
        }
    }
}

/// The horizontal part.
fn flat(v: Vec3) -> glam::Vec2 {
    glam::Vec2::new(v.x, v.y)
}

impl Heli4 {
    /// `G_SpawnHelicopter`: at rest at the spot, facing its angles, with
    /// `VEH_InitPhysics`' and `VEH_InitVehicle`'s rates.
    pub fn new(id: EntId, owner: usize, team: u32, origin: Vec3, angles: Vec3) -> Heli4 {
        Heli4 {
            id,
            owner,
            team_owner: team | (owner as u32) << 2,
            model: String::new(),
            model_index: 0,
            info: VehInfo::default(),
            stage: 0,
            origin,
            angles,
            vel: Vec3::ZERO,
            smoothed_change: Vec3::ZERO,
            speed: 0.0,
            angle_rates: [0.0; 3],
            pace: Pace::default(),
            yaw: YawTurn {
                top: 90.0,
                up: 25.0,
                down: 15.0,
                overshoot: 0.1,
                slow_for: -1.0,
                slowed: false,
            },
            max_pitch: 25.0,
            max_roll: 25.0,
            turning_ability: 0.5,
            near_goal_dist: 0.0,
            air_resistance: 60.0 * MPH,
            hover: Hover {
                radius: 30.0,
                speed: 0.8 * MPH,
                rate: 0.5 * MPH,
                wander: Vec3::ZERO,
                settled: false,
            },
            health: 99_999,
            moving: false,
            goal: origin,
            stop_at_goal: false,
            stopping: false,
            hovering: false,
            goal_yaw: None,
            target_yaw: None,
            weapon: 0,
            gun_pitch: 0.0,
            gun_yaw: 0.0,
            target: None,
            turret_turning: 0,
            shots: 0,
            fresh: true,
            next_think: 0,
            thought: false,
        }
    }

    /// The body's turn.
    pub fn quat(&self) -> Quat {
        angles_quat(self.angles.to_array())
    }

    /// One think's flight (cod4-gsc.md 3.2; 1.7's think at 0x809e38a, the
    /// turn and the tilt inlined in it): toward the goal, or while
    /// hovering toward the goal plus the wander. Returns the notifies in
    /// the order they go out. `rand` is the level's `G_rand`, which a new
    /// wander draws from.
    pub fn think(&mut self, rand: &mut vcod_common::rng::GlibcRand) -> Vec<&'static str> {
        let mut notes = Vec::new();
        if !self.moving {
            self.hold();
            return notes;
        }
        // Read once: the goal check below can start the hover, which
        // steers from the next think on.
        let hovering = self.hovering;
        let aim = if hovering {
            self.goal + self.hover.wander
        } else {
            self.goal
        };
        let to_aim = aim - self.origin;
        let dist = to_aim.length();
        if dist > 0.0 {
            let dir = to_aim / dist;
            self.fly(to_aim, dist, hovering);
            let yaw = self.wanted_yaw(dir, hovering);
            self.turn_yaw(yaw, &mut notes);
            self.tilt(hovering);
        }
        if hovering {
            let near = self.hover.radius * 0.25;
            if (aim - self.origin).length_squared() < near * near {
                self.hover.wander = self.hover.next_wander(rand);
            }
        } else {
            self.goal_notifies(dist, &mut notes);
        }
        notes
    }

    /// The translation: the frame's velocity change toward `to_aim`,
    /// eased, and the move by the mean of the old and new velocity.
    fn fly(&mut self, to_aim: Vec3, dist: f32, hovering: bool) {
        let before = self.vel;
        // A stop goal or the hover flies only the frame's tail at the new
        // velocity once the brake has to start; the head keeps the old.
        let braked_for = if self.stop_at_goal || hovering {
            self.brake_time(dist)
        } else {
            DT
        };
        let pace = if hovering {
            self.hover.pace()
        } else {
            self.pace
        };
        let (target, rate) = if self.stopping {
            (0.0, pace.decel)
        } else {
            (pace.speed, pace.accel)
        };
        let new_speed = accelerate(self.speed, target, rate, braked_for);
        let wanted = to_aim / dist * new_speed;
        let mut change = wanted - before;
        // Below the set speed a turn never brakes: the change's part
        // against the horizontal motion is dropped.
        let along_motion = flat(before).normalize_or_zero();
        if !self.stopping
            && self.pace.speed >= self.speed
            && flat(wanted).dot(flat(before)) > 0.0
            && flat(change).dot(flat(before)) < 0.0
        {
            let back = flat(change).dot(along_motion);
            change.x -= along_motion.x * back;
            change.y -= along_motion.y * back;
        }
        let limit = rate * braked_for;
        let size = change.length();
        if limit < size && size > 0.0 {
            change *= limit / size;
        }
        if !hovering {
            change = self.ease_horizontal(to_aim, before, change, rate);
            change = self.ease_vertical(to_aim.z, before.z, change);
        }
        self.vel = before + change;
        self.speed = self.vel.length();
        // Half way to this frame's change, held to its limit unless
        // stopping at the goal.
        let half_way = self.smoothed_change * 0.5 + change * 0.5;
        let size = half_way.length();
        self.smoothed_change = if limit < size && !self.stop_at_goal {
            half_way * (limit / size)
        } else {
            half_way
        };
        if braked_for < DT {
            self.origin += before * (DT - braked_for);
        }
        self.origin += (before + self.vel) * 0.5 * braked_for;
    }

    /// Whether the brake starts this frame (`stopping`), and how much of
    /// the frame is left once it does: the next speed's stopping distance
    /// against the distance left.
    fn brake_time(&mut self, dist: f32) -> f32 {
        let next = accelerate(self.speed, self.pace.speed, self.pace.accel, DT);
        let stop_dist = next * 0.5 * (next / self.pace.decel);
        let far_yet = stop_dist < dist - next * DT || self.speed <= 0.0;
        self.stopping = !far_yet;
        if far_yet || stop_dist >= dist {
            return DT;
        }
        let coast = (dist - stop_dist) / self.speed;
        (DT - coast).clamp(0.0, DT)
    }

    /// Outside the hover, the horizontal change toward a goal: a stop
    /// sheds the speed it needs to arrive at rest; otherwise a turn whose
    /// radius the rate cannot hold at this speed brakes by the turning
    /// ability (fully toward a stop).
    fn ease_horizontal(&self, to_aim: Vec3, vel: Vec3, change: Vec3, rate: f32) -> Vec3 {
        let gap = flat(to_aim);
        let gap_len = gap.length();
        if gap_len < 1.0 {
            return change;
        }
        let now = flat(vel);
        let speed = now.length();
        let next = now + flat(change);
        let next_speed = next.length();
        if self.stop_at_goal && next_speed > 0.0 {
            let shed = speed * speed / (gap_len * 2.0) * DT;
            if (next_speed - speed).abs() < shed {
                let scaled = next * ((speed - shed) / next_speed);
                return Vec3::new(scaled.x - vel.x, scaled.y - vel.y, change.z);
            }
            return change;
        }
        let off_line = gap.perp_dot(now).abs();
        if gap_len >= off_line || speed == 0.0 {
            return change;
        }
        let (side, ahead) = (off_line / speed, gap.dot(now) / speed);
        let radius = (side * side + ahead * ahead) / (side * 2.0);
        if !(radius > 1.0 && rate * radius < speed * speed) {
            return change;
        }
        let ability = if self.stop_at_goal {
            1.0
        } else {
            self.turning_ability
        };
        let brake = (speed * speed / radius).min(speed);
        let k = -ability * brake / speed * DT;
        Vec3::new(change.x + k * vel.x, change.y + k * vel.y, change.z)
    }

    /// Outside the hover, a climb or descent that would overshoot the
    /// goal's height brakes, at most three frames of the set rate a frame.
    fn ease_vertical(&self, rise: f32, climb: f32, change: Vec3) -> Vec3 {
        let unopposed = climb * change.z >= 0.0 || rise * climb <= 0.0;
        if unopposed || change.z.abs() < 0.001 || climb.abs() < 0.001 {
            return change;
        }
        let to_rest = climb * DT / -change.z;
        let to_height = rise / (climb * 0.5);
        if to_height == 0.0 || to_height >= to_rest {
            return change;
        }
        let needed = -climb * DT / to_height;
        if change.z * change.z < needed * needed {
            let most = self.pace.accel * DT * 3.0;
            return Vec3::new(change.x, change.y, needed.clamp(-most, most));
        }
        change
    }

    /// The yaw this think steers to: the goal yaw once a stop or the hover
    /// is close enough to swing into it, else the target yaw, else the
    /// hover holds its heading, else along the move.
    fn wanted_yaw(&self, dir: Vec3, hovering: bool) -> f32 {
        if let Some(goal_yaw) = self.goal_yaw
            && (self.stopping || hovering)
        {
            let top = self.yaw.top;
            let stop_time = self.speed / self.pace.decel;
            let swing_time = top / self.yaw.up * 2.0;
            let swing_angle = top * 0.5 * swing_time;
            let left = norm180(goal_yaw - self.angles.y).abs();
            let due = stop_time <= swing_time
                || (left > swing_angle && stop_time <= swing_time + (left - swing_angle) / top);
            if due {
                return goal_yaw;
            }
        }
        self.target_yaw.unwrap_or_else(|| {
            if hovering {
                self.angles.y
            } else {
                vectoyaw(dir)
            }
        })
    }

    /// The yaw toward `wanted` at `setYawSpeed`'s rates, a fifth of them
    /// once the rate has swung past the goal yaw (until `wanted` moves);
    /// `goal_yaw` when the heading reaches or crosses the goal yaw.
    fn turn_yaw(&mut self, wanted: f32, notes: &mut Vec<&'static str>) {
        if self.yaw.slow_for != wanted {
            self.yaw.slow_for = wanted;
            self.yaw.slowed = false;
        }
        let off_goal = self
            .goal_yaw
            .map(|g| self.angles.y - g)
            .filter(|d| d.abs() > 0.001);
        let rate_before = self.angle_rates[1];
        let scale = if self.yaw.slowed { 0.2 } else { 1.0 };
        let overshoot = if self.goal_yaw.is_some() || self.target_yaw.is_some() {
            self.yaw.overshoot
        } else {
            0.0
        };
        let rates = Rates {
            top: self.yaw.top,
            up: self.yaw.up * scale,
            down: self.yaw.down * scale,
            overshoot,
        };
        rates.steer(&mut self.angles.y, &mut self.angle_rates[1], wanted);
        let Some(goal_yaw) = self.goal_yaw else {
            return;
        };
        if self.angle_rates[1] * rate_before < 0.0 {
            self.yaw.slowed = true;
        }
        if let Some(was) = off_goal {
            let now = self.angles.y - goal_yaw;
            if now * was < 0.0 || now.abs() < 0.001 {
                notes.push("goal_yaw");
            }
        }
    }

    /// Pitch and roll toward the smoothed change plus the fake drag, along
    /// and across the heading: `maxPitch` and `maxRoll` times
    /// `f + (1 - f) 0.1` (`f` the change against the file's `accel`),
    /// scaled down while braking, at a rate `45 g + (1 - g)` (`g` the set
    /// rate's, or once a hovering body sits level the hover's).
    fn tilt(&mut self, hovering: bool) {
        let ground = flat(self.vel);
        let drag_share = ground.length().min(self.air_resistance) / self.air_resistance;
        let drag = ground.normalize_or_zero() * (drag_share * drag_share * 5.0);
        let mut push = self.smoothed_change;
        push.x += drag.x;
        push.y += drag.y;
        let push_rate = flat(push).length() / DT;
        let f = self.accel_share(push_rate);
        let scale = f + (1.0 - f) * 0.1;
        // Braking, the lean eases off once the change would stop the
        // body sooner than it settles (2.5 s at full rate, 3.5 at none).
        let mut ease = 1.0;
        if self.stopping && push_rate > 0.0 {
            let braking_for = ground.length() / push_rate;
            let settle = (1.0 - f) * 3.5 + f * 2.5;
            ease = (braking_for / settle).min(1.0);
        }

        let level = self.angles.x.abs() <= 5.0
            && self.angles.z.abs() <= 5.0
            && self.angle_rates[0].abs() <= 3.0
            && self.angle_rates[2].abs() <= 3.0;
        self.hover.settled = hovering && (self.hover.settled || level);
        let g = self.accel_share(if self.hover.settled {
            self.hover.rate
        } else {
            self.pace.accel
        });
        let up = g * 45.0 + (1.0 - g);
        let rates = Rates {
            top: MAX_TILT_VEL,
            up,
            down: up * 0.4,
            overshoot: 0.0,
        };

        let heading = self.angles.y * 0.017_453_292;
        let facing = glam::Vec2::new(heading.cos(), heading.sin());
        let dir = flat(push.normalize_or_zero());
        let pitch = self.max_pitch * (dir.dot(facing) * ease) * scale;
        rates.steer(&mut self.angles.x, &mut self.angle_rates[0], pitch);
        let roll = self.max_roll * (dir.perp_dot(facing) * ease) * scale;
        rates.steer(&mut self.angles.z, &mut self.angle_rates[2], roll);
    }

    /// `x` over the vehicle file's `accel`, held to 0..1.
    fn accel_share(&self, x: f32) -> f32 {
        x.clamp(0.0, self.info.accel) / self.info.accel
    }

    /// Outside the hover: `near_goal` on every think that starts nearer
    /// than `setNearGoalNotifyDist`, and `goal` once the goal is reached:
    /// a pass-through goal within a frame's flight, a stop goal inside the
    /// hover radius under 2 mph (with no radius, at rest), which starts
    /// the hover.
    fn goal_notifies(&mut self, dist: f32, notes: &mut Vec<&'static str>) {
        if self.near_goal_dist != 0.0 && dist < self.near_goal_dist {
            notes.push("near_goal");
        }
        let radius = self.hover.radius;
        let reached = match (self.stop_at_goal, radius == 0.0) {
            (false, _) => dist <= self.speed * DT,
            (true, true) => self.speed == 0.0 && (self.stopping || dist == 0.0),
            (true, false) => dist <= radius && self.speed < 2.0 * MPH,
        };
        if !reached {
            return;
        }
        if self.stop_at_goal {
            self.hovering = true;
            if radius == 0.0 {
                self.vel = Vec3::ZERO;
                self.smoothed_change = Vec3::ZERO;
            }
        }
        notes.push("goal");
    }

    /// The no-goal think (`cod4_lnxded` 0x808ba22, run in place of
    /// `VEH_UpdateMoveToGoal` while flag 2 is clear, 0x80a048f..0x80a04a0):
    /// 1.7's player-helicopter step with its four stick bytes zeroed
    /// (0x808ba6a..0x808ba76). Only a goal sets flag 2 and nothing clears
    /// it, so it runs from rest, where it keeps the place, levels pitch and
    /// roll to +0 (the tilt's spring at rest, plus the hover jitter's +0)
    /// and normalises the yaw on the stack (0x808c719..0x808c798;
    /// cod4-gsc.md 3.2, "The no-goal think").
    fn hold(&mut self) {
        self.angles = Vec3::new(0.0, super::missile::normalize180(self.angles.y), 0.0);
    }

    /// `VEH_UpdateAim`'s turret (`cod4_lnxded` 0x809ff9b..0x80a038c): the
    /// target's direction from `barrel` (the posed `tag_barrel`) in the
    /// axis of `frame` (bone 0's posed angles), and each gun angle tracked to
    /// it at `turretRotRate` (0x8194154), held inside the file's spans.
    /// Returns the notifies: `turret_rotate_stopped` two thinks after the
    /// gun last turned far, then `turret_on_target` once both angles were
    /// within a degree of it, else `turret_not_on_target`; the caller adds
    /// the sight notify after it (cod4-gsc.md 3.2, "The turret").
    pub fn aim(&mut self, barrel: Vec3, frame: Vec3, target: Vec3) -> Vec<&'static str> {
        let dir = normalize4(target, barrel);
        let toward = axis4(vectoangles4(dir));
        let frame = axis4(frame);
        let el = |j: usize| {
            let (a, b) = (toward[0], frame[j]);
            (d(a.z) * d(b.z) + d(a.y) * d(b.y) + d(a.x) * d(b.x)) as f32
        };
        let rel = vectoangles4(Vec3::new(el(0), el(1), el(2)));
        let off = (
            angle_sub4(rel.x, self.gun_pitch).abs(),
            angle_sub4(rel.y, self.gun_yaw).abs(),
        );
        let rate = self.info.turret_rot_rate;
        let (pitch, yaw) = (
            track4(rel.x, self.gun_pitch, rate),
            track4(rel.y, self.gun_yaw, rate),
        );
        self.gun_pitch = clamp4(pitch, self.info.span_up, self.info.span_down);
        self.gun_yaw = clamp4(yaw, self.info.span_right, self.info.span_left);
        let mut notes = Vec::new();
        // Still turning: a gun far off on an angle the spans left alone.
        let free = |a: f32, b: f32| norm180_kept(d(a) - d(b)) == 0.0;
        if (off.0 >= 2.0 && free(pitch, self.gun_pitch))
            || (off.1 >= 2.0 && free(yaw, self.gun_yaw))
        {
            self.turret_turning = 2;
        } else if self.turret_turning == 2 {
            self.turret_turning = 1;
        } else if self.turret_turning == 1 {
            notes.push("turret_rotate_stopped");
            self.turret_turning = 0;
        }
        if off.0 >= 1.0 || off.1 >= 1.0 {
            notes.push("turret_not_on_target");
        } else {
            notes.push("turret_on_target");
        }
        notes
    }

    /// With no target `VEH_UpdateAim` only lets the turning state run down
    /// (0x80a03d4..0x80a03fe), notifying nothing.
    pub fn aim_idle(&mut self) {
        self.turret_turning = self.turret_turning.saturating_sub(1);
    }

    /// The gun's forward in the world: the body's turn, then the turret's.
    pub fn gun_forward(&self) -> Vec3 {
        let [f, _, _] =
            vcod_common::x87::angle_vectors(Vec3::new(self.gun_pitch, self.gun_yaw, 0.0));
        self.quat() * f
    }

    /// Its entity state: the flight's place, the gun, the model, and from
    /// its `script_vehicle` the loop sound and the event ring (the guns'
    /// `EV_FIRE_WEAPON`, the script's `playFxOnTag`).
    pub fn entity(
        &self,
        loop_sound: i32,
        ring: &vcod_common::pmove::cmd::EventRing,
    ) -> EntityState {
        let mut e = EntityState::default();
        e.0[0] = self.id.0;
        e.0[1] = ET_HELICOPTER;
        let table = EntityState::table(ET_HELICOPTER);
        let mut set = |name: &str, v: u32| {
            if let Some(f) = table.iter().find(|f| f.name == name) {
                e.0[f.word()] = v;
            }
        };
        set("lerp.pos.trType", TR_INTERPOLATE);
        set("lerp.apos.trType", TR_INTERPOLATE);
        // No velocity rides `pos.trDelta`: 1.7 sends it 0 throughout.
        for k in 0..3 {
            set(&format!("lerp.pos.trBase[{k}]"), self.origin[k].to_bits());
            set(&format!("lerp.apos.trBase[{k}]"), self.angles[k].to_bits());
        }
        set("lerp.u.vehicle.teamAndOwnerIndex", self.team_owner);
        set("lerp.u.vehicle.gunPitch", self.gun_pitch.to_bits());
        set("lerp.u.vehicle.gunYaw", self.gun_yaw.to_bits());
        set("un1.helicopterStage", self.stage);
        set("weapon", self.weapon);
        set("un2", self.model_index);
        // The think's store, so 0 until the first think has run.
        if self.thought {
            set(
                "time2",
                (self.info.suspension_travel * 1000.0) as i32 as u32,
            );
        }
        set("loopSound", loop_sound as u32);
        set("eventSequence", ring.seq as u32);
        for k in 0..4 {
            set(&format!("events[{k}]"), ring.events[k] as u32);
            set(&format!("eventParms[{k}]"), ring.parms[k] as u32);
        }
        e
    }
}

impl Cod4Level {
    /// The vehicle pass of `G_RunFrame`: the frame's `fireWeapon` rounds,
    /// then each helicopter's think by entity number (flight, the host
    /// entity's place, the turret), its notifies on the next frame's
    /// threads.
    pub(super) fn run_vehicles(&mut self) {
        let shots = std::mem::take(&mut self.rt.host.cod4_mut().vehicle_shots);
        for s in shots {
            self.vehicle_fire(&s);
        }
        let host = &mut self.rt.host;
        let gone: Vec<EntId> = host
            .cod4()
            .helis
            .iter()
            .map(|h| h.id)
            .filter(|id| host.ents.get(*id).is_none())
            .collect();
        host.cod4_mut().helis.retain(|h| !gone.contains(&h.id));
        let planes_gone: Vec<EntId> = host
            .cod4()
            .planes
            .keys()
            .filter(|id| host.ents.get(**id).is_none())
            .copied()
            .collect();
        for id in planes_gone {
            host.cod4_mut().planes.remove(&id);
        }
        let launched_gone: Vec<EntId> = host
            .cod4()
            .physics
            .keys()
            .filter(|id| host.ents.get(**id).is_none())
            .copied()
            .collect();
        for id in launched_gone {
            host.cod4_mut().physics.remove(&id);
        }
        let mut order: Vec<EntId> = host.cod4().helis.iter().map(|h| h.id).collect();
        order.sort_by_key(|id| id.0);
        for id in order {
            let Some(mut h) = self
                .rt
                .host
                .cod4()
                .helis
                .iter()
                .find(|h| h.id == id)
                .cloned()
            else {
                continue;
            };
            let now = self.level_time();
            if std::mem::take(&mut h.fresh) || now < h.next_think {
                if let Some(slot) = self
                    .rt
                    .host
                    .cod4_mut()
                    .helis
                    .iter_mut()
                    .find(|x| x.id == id)
                    .filter(|x| x.fresh)
                {
                    // `VEH_InitEntity`'s `level.time + 50` (0x809c6ed).
                    slot.fresh = false;
                    slot.next_think = now + THINK_MS;
                }
                continue;
            }
            h.next_think = now + THINK_MS;
            h.thought = true;
            let (from, from_angles) = (h.origin, h.angles);
            let mut notes: Vec<(&'static str, Vec<Value>)> = h
                .think(&mut self.rt.host.cod4_mut().rand)
                .into_iter()
                .map(|n| (n, Vec::new()))
                .collect();
            if let Some(n) = self.heli_collision(&h, from) {
                let arg = Value::Vector(n.to_array());
                notes.push(("script_vehicle_collision", vec![arg]));
            }
            self.carry_riders(&h, (from, from_angles));
            let target = match h.target {
                Some(TurretTarget::Pos(p)) => Some(p),
                Some(TurretTarget::Ent(e, off)) => self.entity_origin(e).map(|o| o + off),
                None => None,
            };
            match (target, heli_aim_from(self.rt.host.cod4(), &h)) {
                (Some(t), Some((barrel, frame))) => {
                    let aimed = h.aim(barrel, frame, t);
                    // On target at an entity, the sight trace from the
                    // barrel (`SV_SightTrace` under 0x801, 0x80a0398,
                    // passing the helicopter and the target; no body bit in
                    // the mask, so passing the helicopter alone is enough).
                    let seen = aimed.last() == Some(&"turret_on_target")
                        && matches!(h.target, Some(TurretTarget::Ent(..)))
                        && crate::game::builtins::cod4_trace::trace4(
                            &self.rt.host,
                            barrel,
                            t,
                            Vec3::ZERO,
                            Vec3::ZERO,
                            h.id.0,
                            0x801,
                        )
                        .is_some_and(|tr| tr.fraction >= 1.0 && !tr.startsolid);
                    let vis = if seen {
                        "turret_on_vistarget"
                    } else {
                        "turret_no_vis"
                    };
                    for n in aimed.into_iter().chain([vis]) {
                        notes.push((n, Vec::new()));
                    }
                }
                (None, Some(_)) => h.aim_idle(),
                _ => {}
            }
            let (origin, angles) = (h.origin, h.angles);
            if let Some(slot) = self
                .rt
                .host
                .cod4_mut()
                .helis
                .iter_mut()
                .find(|x| x.id == id)
            {
                *slot = h;
            }
            self.rt.with_host(|host, cx| {
                use vcod_gsc::Host;
                let o = cx.intern_folded("origin");
                let a = cx.intern_folded("angles");
                let _ = host.set_field(cx, id, o, Value::Vector(origin.to_array()));
                let _ = host.set_field(cx, id, a, Value::Vector(angles.to_array()));
            });
            for (n, args) in notes {
                self.rt.host.engine_notifies.push((id, n, args));
            }
        }
    }

    /// The vehicle think's move trace (`cod4_lnxded` 0x80a04aa..0x80a05e3):
    /// with a goal set (flag 2, 0x809fd13), a helicopter that moved sweeps
    /// its box from the last think's place to the new one under 0x2081,
    /// passing itself (`G_TraceCapsule`, 0x80b408a). The box is the DObj's
    /// radius on every axis (`DObjGetBounds`, set by `VEH_InitEntity`
    /// 0x809c725); one wider than tall would get its top raised to its
    /// width (0x80a0549..0x80a055c), which a cube never does. A contact with
    /// the world (hit id 1022, 0x80a05ad) returns the trace's normal, which
    /// the think notifies as `script_vehicle_collision` (0x80a05c0..0x80a05de).
    /// The flight goes on through it.
    fn heli_collision(&self, h: &Heli4, from: Vec3) -> Option<Vec3> {
        if !h.moving || h.origin == from {
            return None;
        }
        let clip = self.clip.as_deref()?;
        let rig = self
            .rt
            .host
            .cod4()
            .model_rigs
            .get(&h.model.to_ascii_lowercase())?
            .clone();
        let world = Cod4Clip {
            clip,
            bodies: &[],
            brushes: &self.brushes,
        };
        let r = Vec3::splat(rig.radius);
        let t = vcod_cod4::pmove::PmWorld::trace(
            &world,
            from,
            h.origin,
            -r,
            r,
            NONE,
            MASK_VEHICLE_MOVE,
        );
        (t.fraction < 1.0 && t.entity_num == WORLD).then_some(t.normal)
    }

    /// The vehicle think's touch pass (`cod4_lnxded` 0x809fd20..0x809ff85)
    /// as a helicopter meets it: of the missiles in its swept box only one
    /// resting on it (`groundEntityNum`, 0x809fe98) whose weapon sticks to
    /// all (`stickiness` 1, 0x80a046a) and that is linked to nothing (+0x218,
    /// 0x80a0400) rides along, once the body moved or turned by 0.001
    /// squared (0x80a0427..0x80a0456; [`carried`]). The other missiles'
    /// touch needs `r.bmodel` (0x809fea4), which a helicopter's xmodel
    /// never sets, and its own touch slot is empty (handler 23, 0x826cd40).
    fn carry_riders(&mut self, h: &Heli4, before: (Vec3, Vec3)) {
        let (from, from_angles) = before;
        let turned = Vec3::from_array(std::array::from_fn(|k| {
            norm180(h.angles[k] - from_angles[k])
        }));
        if (h.origin - from).length_squared() < 0.001 && turned.length_squared() < 0.001 {
            return;
        }
        let sticky: Vec<bool> = self
            .rt
            .host
            .cod4()
            .missiles
            .iter()
            .map(|m| {
                self.combat
                    .defs
                    .get(m.weapon as usize)
                    .is_some_and(|d| d.stickiness == STICK_TO_ALL)
            })
            .collect();
        let missiles = &mut self.rt.host.cod4_mut().missiles;
        for (m, _) in missiles
            .iter_mut()
            .zip(sticky)
            .filter(|(m, sticky)| *sticky && m.ground == h.id.0)
        {
            let (at, angles) = carried(
                (from, from_angles),
                (h.origin, h.angles),
                (m.origin, m.angles),
            );
            m.origin = at;
            m.pos.base = at;
            m.angles = angles;
            m.apos.base = angles;
        }
    }

    /// [`heli_trace`] on this level.
    pub(super) fn heli_trace(
        &self,
        start: Vec3,
        end: Vec3,
        limit: f32,
        skip: Option<EntId>,
        mask: u32,
    ) -> Option<HeliHit> {
        heli_trace(&self.rt.host, start, end, limit, skip, mask)
    }

    /// `G_Damage` on a helicopter: unless the vehicle file makes
    /// it immune to the means of death (`G_VehImmuneToDamage`), at least 1
    /// off its `health`, then `damage` notified with (damage, attacker,
    /// direction, point, means of death, model, tag, part, damage flags),
    /// the last three empty strings; `death` once health is gone. No hit
    /// location multiplier applies to an entity. `Helicopter_Pain`'s jolt
    /// scales by the file's `maxBodyPitch` and `maxBodyRoll`, 0 on
    /// `cobra_mp`, so it moves nothing and is left out.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn damage_heli(
        &mut self,
        id: EntId,
        attacker: Option<usize>,
        d: &vcod_cod4::pmove::weapon::WeaponDef,
        damage: i32,
        dflags: i32,
        mod_: &'static str,
        dir: Vec3,
        point: Vec3,
    ) {
        let attacker_ent = attacker.and_then(|a| self.rt.client_entity(a));
        let Some(h) = self
            .rt
            .host
            .cod4_mut()
            .helis
            .iter_mut()
            .find(|h| h.id == id)
        else {
            return;
        };
        if h.info
            .immune(mod_, dflags, d.proj_explosion == PROJEXP_HEAVY)
        {
            return;
        }
        let damage = damage.max(1);
        h.health -= damage;
        let dead = h.health <= 0;
        if dead {
            h.health = h.health.max(-999);
        }
        let who = attacker_ent.map_or(Value::Undefined, Value::Entity);
        let args = self.rt.with_host(|_, cx| {
            let empty = Value::String(cx.intern_exact(""));
            vec![
                Value::Int(damage),
                who,
                Value::Vector(dir.normalize_or_zero().to_array()),
                Value::Vector(point.to_array()),
                Value::String(cx.intern_exact(mod_)),
                empty,
                empty,
                empty,
                Value::Int(dflags),
            ]
        });
        self.rt.host.engine_notifies.push((id, "damage", args));
        if dead {
            self.rt.host.engine_notifies.push((id, "death", vec![who]));
        }
    }

    /// An entity's `origin` as the script reads it.
    fn entity_origin(&mut self, id: EntId) -> Option<Vec3> {
        self.rt.with_host(|host, cx| {
            use vcod_gsc::Host;
            host.ents.get(id)?;
            let o = cx.intern_folded("origin");
            match host.get_field(cx, id, o) {
                Value::Vector(v) => Some(Vec3::from(v)),
                _ => None,
            }
        })
    }

    /// A tag of the helicopter's model, posed ([`heli_tag`]), in the
    /// world; `None` when the model has no such tag.
    fn tag_world(&self, h: &Heli4, tag: &str) -> Option<Vec3> {
        heli_tag(self.rt.host.cod4(), h, tag).map(|(p, _)| p)
    }

    /// A tag's orientation in the world, posed ([`heli_tag`]); `None` when
    /// the model has no such tag.
    fn tag_axis(&self, h: &Heli4, tag: &str) -> Option<glam::Quat> {
        heli_tag(self.rt.host.cod4(), h, tag).map(|(_, q)| q)
    }

    /// `CMD_VEH_FireWeapon`'s round: a bullet weapon traced from the tag
    /// along the turret (the hip spread's disc around it), its body hit
    /// damaged as the owner's with the helicopter the inflictor; a
    /// projectile flies straight from the tag at the target (or ahead).
    fn vehicle_fire(&mut self, s: &VehShot) {
        let Some(h) = self
            .rt
            .host
            .cod4()
            .helis
            .iter()
            .find(|h| h.id == s.heli)
            .cloned()
        else {
            if let Some(m) = s.missile {
                self.rt.host.ents.free(m);
            }
            return;
        };
        let Some(d) = self.combat.defs.get(s.weapon as usize).cloned() else {
            return;
        };
        let start = s
            .tag
            .as_deref()
            .and_then(|t| self.tag_world(&h, t))
            .unwrap_or(h.origin);
        let now = self.level_time();
        if let Some(m) = s.missile {
            // `CMD_VEH_FireWeapon` (0x809d9e2): the rocket leaves along the
            // tag's axes (`G_DObjGetWorldTagMatrix`, here the bind pose
            // turned by the body) at the weapon's `adsSpread`, with the
            // body's velocity, steering at the target when it has one.
            let axis = s
                .tag
                .as_deref()
                .and_then(|t| self.tag_axis(&h, t))
                .unwrap_or_else(|| h.quat());
            let dir = self.rocket_dir(axis * Vec3::X, axis * Vec3::Y, axis * Vec3::Z, d.ads_spread);
            let shot = super::missile::RocketShot {
                owner: Some(h.owner),
                launcher: Some(s.heli),
                client: false,
                team: String::new(),
                start,
                dir,
                gun_vel: h.vel,
                target: s.target,
                target_offset: s.target_offset,
            };
            self.g_fire_rocket(m, s.weapon, &d, shot);
            self.bump_heli_event(s.heli);
            return;
        }
        if d.weap_type == WEAPTYPE_PROJECTILE {
            return;
        }
        let forward = h.gun_forward();
        let [_, right, up] = vcod_common::x87::angle_vectors(Vec3::from(
            crate::game::builtins::math::vectoangles(forward.to_array()),
        ));
        let aim = (d.hip_spread_stand_max * 0.017_453_292).tan() * BULLET_RANGE;
        let (x, y) = super::combat::random_dir(now + h.shots as i32);
        if let Some(x) = self
            .rt
            .host
            .cod4_mut()
            .helis
            .iter_mut()
            .find(|x| x.id == s.heli)
        {
            x.shots += 1;
        }
        let end = start + forward * BULLET_RANGE + right * (x * aim) + up * (y * aim);
        let dir = (end - start).normalize_or_zero();
        let origins = std::collections::HashMap::new();
        let c = self.locational_trace(start, end, usize::MAX, &BULLET_PRIORITY, &origins);
        self.bump_heli_event(s.heli);
        let Some(victim) = c.client else {
            return;
        };
        let hit = start + (end - start) * c.fraction;
        let damage = super::combat::bullet_damage(&d, hit.distance(start));
        let mod_ = if d.rifle_bullet {
            "MOD_RIFLE_BULLET"
        } else {
            "MOD_PISTOL_BULLET"
        };
        self.damage_from(
            h.owner,
            victim,
            Some(s.heli),
            &d,
            damage,
            0,
            mod_,
            dir,
            hit,
            c.hit_location,
        );
    }

    /// `EV_FIRE_WEAPON` on the helicopter's entity, beside the
    /// `playFxOnTag` events the script puts there.
    fn bump_heli_event(&mut self, id: EntId) {
        if let Some(e) = self.rt.host.ents.get_mut(id) {
            e.events.add(EV_FIRE_WEAPON as i32, 0);
        }
    }

    /// The planes and helicopters among `out`: a plane is its
    /// `script_model` under `ET_PLANE` with its team and owner; a
    /// helicopter goes out whole ([`Heli4::entity`]).
    pub(super) fn vehicle_entities(&self, out: &mut BTreeMap<u32, EntityState>) {
        let c4 = self.rt.host.cod4();
        for (id, team_owner) in &c4.planes {
            if self.rt.host.ents.get(*id).is_none() {
                continue;
            }
            if let Some(e) = out.get_mut(&id.0) {
                e.0[1] = ET_PLANE;
                if let Some(f) = EntityState::table(ET_PLANE)
                    .iter()
                    .find(|f| f.name == "lerp.u.vehicle.teamAndOwnerIndex")
                {
                    e.0[f.word()] = *team_owner;
                }
            }
        }
        // `physicsLaunch`: both trajectories `TR_PHYSICS` from the launch,
        // the contact point and the force in their deltas.
        for (id, (time, contact, force)) in &c4.physics {
            let Some(e) = out.get_mut(&id.0) else {
                continue;
            };
            let table = EntityState::table(e.etype());
            let mut set = |name: &str, v: u32| {
                if let Some(f) = table.iter().find(|f| f.name == name) {
                    e.0[f.word()] = v;
                }
            };
            set("lerp.pos.trType", TR_PHYSICS);
            set("lerp.apos.trType", TR_PHYSICS);
            set("lerp.pos.trTime", *time as u32);
            set("lerp.pos.trDuration", 0x7fff_ffff);
            for k in 0..3 {
                set(&format!("lerp.pos.trDelta[{k}]"), contact[k].to_bits());
                set(&format!("lerp.apos.trDelta[{k}]"), force[k].to_bits());
            }
        }
        for h in &c4.helis {
            let Some(ent) = self.rt.host.ents.get(h.id) else {
                continue;
            };
            out.insert(h.id.0, h.entity(ent.loop_sound, &ent.events));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heli() -> Heli4 {
        let mut h = Heli4::new(EntId(100, 0), 0, 2, Vec3::ZERO, Vec3::ZERO);
        h.pace.speed = 60.0 * MPH;
        h.pace.accel = 25.0 * MPH;
        h.pace.decel = h.pace.accel * 0.5;
        h.yaw.top = 75.0;
        h.yaw.up = 45.0;
        h.yaw.down = 45.0;
        h.near_goal_dist = 256.0;
        h.turning_ability = 0.9;
        h.max_pitch = 30.0;
        h.max_roll = 30.0;
        h
    }

    fn rng() -> vcod_common::rng::GlibcRand {
        vcod_common::rng::GlibcRand::new(1)
    }

    /// A stop goal is reached inside the hover radius under 2 mph; the
    /// hover then flies to wander points off the goal, new ones as it
    /// reaches each, and the body pitches nose down speeding up and up
    /// braking. 1.7's `G_flrand` draws in `(2 min - max, min]`, so each
    /// wander lies 30 to 90 down every axis (less half the last).
    #[test]
    fn a_stop_goal_hovers_round_the_goal() {
        let mut h = heli();
        let mut rand = rng();
        h.goal = Vec3::new(3000.0, 1000.0, 200.0);
        h.moving = true;
        h.stop_at_goal = true;
        let (mut near, mut goal) = (None, None);
        let (mut nose_down, mut nose_up) = (0.0f32, 0.0f32);
        for i in 0..2000 {
            let n = h.think(&mut rand);
            nose_down = nose_down.max(h.angles.x);
            nose_up = nose_up.min(h.angles.x);
            if near.is_none() && n.contains(&"near_goal") {
                near = Some(i);
            }
            if n.contains(&"goal") {
                goal = Some(i);
                break;
            }
        }
        let (near, goal) = (near.unwrap(), goal.unwrap());
        assert!(near < goal, "{near} {goal}");
        assert!(h.origin.distance(h.goal) <= 30.0, "{:?}", h.origin);
        assert!(h.speed < 2.0 * MPH, "{}", h.speed);
        assert!(h.hovering);
        assert!(nose_down > 3.0 && nose_up < -3.0, "{nose_down} {nose_up}");
        assert!(
            (h.angles.y - vectoyaw(h.goal)).abs() < 5.0,
            "{:?}",
            h.angles
        );
        // The hover: it keeps moving round the goal, no further notifies.
        let mut far = 0.0f32;
        let mut wanders = std::collections::BTreeSet::new();
        for _ in 0..2000 {
            assert!(h.think(&mut rand).is_empty());
            far = far.max(h.origin.distance(h.goal));
            wanders.insert(h.hover.wander.to_array().map(f32::to_bits));
        }
        assert!(far < 200.0, "{far}");
        assert!(wanders.len() >= 3, "{wanders:?}");
        assert!(
            h.angles.x.abs() < 5.0 && h.angles.z.abs() < 5.0,
            "{:?}",
            h.angles
        );
    }

    /// A turn rolls the body into it: a goal off to the left (+y) rolls
    /// it one way, to the right the other.
    #[test]
    fn a_turn_rolls_the_body() {
        let roll = |y: f32| {
            let mut h = heli();
            let mut rand = rng();
            h.vel = Vec3::new(60.0 * MPH, 0.0, 0.0);
            h.speed = h.vel.length();
            h.goal = Vec3::new(1500.0, y, 0.0);
            h.moving = true;
            let mut r = 0.0f32;
            for _ in 0..40 {
                h.think(&mut rand);
                if r.abs() < h.angles.z.abs() {
                    r = h.angles.z;
                }
            }
            r
        };
        let (left, right) = (roll(1500.0), roll(-1500.0));
        assert!(left.abs() > 2.0 && right.abs() > 2.0, "{left} {right}");
        assert!(left * right < 0.0, "{left} {right}");
    }

    #[test]
    fn a_pass_through_goal_notifies_without_stopping() {
        let mut h = heli();
        let mut rand = rng();
        h.goal = Vec3::new(2000.0, 0.0, 0.0);
        h.moving = true;
        let mut fired = false;
        for _ in 0..2000 {
            if h.think(&mut rand).contains(&"goal") {
                fired = true;
                break;
            }
        }
        assert!(fired);
        assert!(h.speed > 100.0, "{}", h.speed);
    }

    /// FNV-1a over every flight word and notify of `n` thinks.
    fn flight_digest(h: &mut Heli4, rand: &mut vcod_common::rng::GlibcRand, n: usize) -> u64 {
        let mut d = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |w: u64| {
            d ^= w;
            d = d.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for _ in 0..n {
            for note in h.think(rand) {
                note.bytes().for_each(|b| mix(b as u64));
            }
            let words = [h.origin, h.angles, h.vel, h.smoothed_change, h.hover.wander]
                .into_iter()
                .flat_map(|v| v.to_array())
                .chain(h.angle_rates)
                .chain([h.speed]);
            words.for_each(|f| mix(f.to_bits() as u64));
            mix(h.hovering as u64 | (h.stopping as u64) << 1);
        }
        d
    }

    /// Pins the flight's every word over the arms the hardpoint gates
    /// miss: a stop with no hover radius, a goal yaw, a target yaw, a
    /// climb and a descent, a sharp pass-through turn, a stop entered at
    /// speed.
    #[test]
    fn the_flight_holds_its_words() {
        let mut rand = rng();
        let mut digests = Vec::new();
        // A descent to a stop with no hover radius, turning to a goal yaw.
        let mut h = heli();
        h.hover.radius = 0.0;
        h.goal = Vec3::new(2500.0, -800.0, -400.0);
        h.goal_yaw = Some(135.0);
        h.moving = true;
        h.stop_at_goal = true;
        digests.push(flight_digest(&mut h, &mut rand, 900));
        assert!(h.hovering, "{h:?}");
        // A climbing pass-through under a target yaw, then a sharp turn.
        let mut h = heli();
        h.yaw.overshoot = 0.3;
        h.target_yaw = Some(-60.0);
        h.goal = Vec3::new(1800.0, 300.0, 600.0);
        h.moving = true;
        digests.push(flight_digest(&mut h, &mut rand, 120));
        h.target_yaw = None;
        h.goal = Vec3::new(1600.0, 2600.0, 500.0);
        digests.push(flight_digest(&mut h, &mut rand, 200));
        // A stop entered at full speed, then the hover with new params
        // and a goal yaw while hovering.
        let mut h = heli();
        h.vel = Vec3::new(0.0, 60.0 * MPH, 0.0);
        h.speed = h.vel.length();
        h.goal = Vec3::new(400.0, 1200.0, -50.0);
        h.moving = true;
        h.stop_at_goal = true;
        digests.push(flight_digest(&mut h, &mut rand, 600));
        assert!(h.hovering, "{h:?}");
        h.hover.radius = 60.0;
        h.hover.speed = 20.0;
        h.hover.rate = 10.0;
        h.air_resistance = 40.0 * MPH;
        h.goal_yaw = Some(-170.0);
        digests.push(flight_digest(&mut h, &mut rand, 400));
        // A turn too tight for the speed: the turning ability brakes it.
        let mut h = heli();
        h.vel = Vec3::new(60.0 * MPH, 0.0, 0.0);
        h.speed = h.vel.length();
        h.goal = Vec3::new(300.0, 900.0, 0.0);
        h.moving = true;
        digests.push(flight_digest(&mut h, &mut rand, 60));
        assert_eq!(
            digests,
            [
                0xd507_cc93_7afc_fb66,
                0x9d7c_0a93_156d_4dd1,
                0x82a9_3e34_3529_1deb,
                0x7c4e_8920_12d9_8e29,
                0xd060_6ce8_19ac_c22b,
                0x2954_9d76_e87e_07ac,
            ],
            "{digests:#x?}"
        );
    }

    #[test]
    fn the_turret_tracks_then_reports_on_target() {
        let mut h = heli();
        let target = Vec3::new(500.0, 500.0, -500.0);
        let mut first = None;
        for i in 0..200 {
            if h.aim(Vec3::ZERO, h.angles, target).last() == Some(&"turret_on_target") {
                first = Some(i);
                break;
            }
        }
        // 45 of yaw at 80 a second: about 12 frames.
        let i = first.unwrap();
        assert!((10..20).contains(&i), "{i}");
        let f = h.gun_forward();
        assert!(f.dot(target.normalize()) > 0.999, "{f:?}");
    }

    /// A gun's chain: the root, `tag_turret` at x 100, the turret joint at
    /// 110, `tag_barrel` at 120, the flash at 150, and a body bone off the
    /// root.
    fn gun_tags() -> (ModelTags, Vec<i32>) {
        let at = |name: &str, x: f32| (name.to_string(), Vec3::new(x, 0.0, 0.0), Quat::IDENTITY);
        let tags = vec![
            at("tag_origin", 0.0),
            at("tag_turret", 100.0),
            at("turret_animate_joint", 110.0),
            at("tag_barrel", 120.0),
            at("tag_flash", 150.0),
            at("body_animate_jnt", 0.0),
        ];
        (tags, vec![-1, 0, 1, 2, 3, 0])
    }

    /// The controller turns the turret's chain about `tag_turret` and the
    /// barrel's about `tag_barrel`; the body's bones stay put. A barrel
    /// tilted at the bind pose levels at gun pitch 0.
    #[test]
    fn the_controller_turns_the_turret_and_barrel_chains() {
        let (tags, parents) = gun_tags();
        let place = |pose: &[Option<BoneMove>], b: usize| {
            let p = tags[b].1;
            pose[b].map_or(p, |(rot, shift)| rot * p + shift)
        };
        let still = heli_pose(&tags, &parents, 0.0, 0.0);
        assert!(still.iter().all(Option::is_none));
        let yawed = heli_pose(&tags, &parents, 0.0, 90.0);
        assert!(yawed[0].is_none() && yawed[5].is_none());
        assert!(place(&yawed, 2).distance(Vec3::new(100.0, 10.0, 0.0)) < 1e-4);
        assert!(place(&yawed, 4).distance(Vec3::new(100.0, 50.0, 0.0)) < 1e-4);
        // Pitch down 90 about the barrel, which the yaw put at (100, 20).
        let both = heli_pose(&tags, &parents, 90.0, 90.0);
        assert!(place(&both, 3).distance(Vec3::new(100.0, 20.0, 0.0)) < 1e-4);
        assert!(place(&both, 4).distance(Vec3::new(100.0, 20.0, -30.0)) < 1e-3);
        let mut tilted = tags.clone();
        let up = angles_quat([-10.0, 0.0, 0.0]);
        tilted[3].2 = up;
        tilted[4] = (
            "tag_flash".into(),
            Vec3::new(120.0, 0.0, 0.0) + up * Vec3::X * 30.0,
            up,
        );
        let level = heli_pose(&tilted, &parents, 0.0, 0.0);
        let flash = level[4]
            .map(|(rot, shift)| rot * tilted[4].1 + shift)
            .unwrap();
        assert!(flash.distance(Vec3::new(150.0, 0.0, 0.0)) < 1e-3, "{flash}");
    }

    /// A wall on the turret joint at x 110 facing back, 20 square: at rest
    /// a ray along x meets it; with the turret yawed 90 it stands at y 10,
    /// where a ray along y meets it and the first misses. The contact's
    /// normal stays the bind pose's, as 1.7's does.
    #[test]
    fn a_turned_turrets_surface_traces_where_it_was_turned() {
        let (tags, parents) = gun_tags();
        let wall = vcod_cod4::assets::model::XModelCollSurf {
            tris: vec![[
                -1.0, 0.0, 0.0, -110.0, 0.0, 0.05, 0.0, -0.5, 0.0, 0.0, 0.05, -0.5,
            ]],
            mins: [109.0, -11.0, -11.0],
            maxs: [111.0, 11.0, 11.0],
            bone: 2,
            contents: 1,
            surface_flags: 0,
        };
        let surfs = [wall];
        let run = |pose: &[Option<BoneMove>], a: Vec3, b: Vec3| {
            let mut tr = vcod_common::clipmap::ClipTrace::clear();
            trace_posed(&surfs, pose, a, b, 1, &mut tr).then(|| (a.lerp(b, tr.fraction), tr.normal))
        };
        let along_x = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(200.0, 0.0, 0.0));
        let along_y = (Vec3::new(100.0, -50.0, 0.0), Vec3::new(100.0, 50.0, 0.0));
        let still = heli_pose(&tags, &parents, 0.0, 0.0);
        let (at, n) = run(&still, along_x.0, along_x.1).expect("the wall at rest");
        assert!((at.x - 110.0).abs() < 0.2 && n.distance(Vec3::NEG_X) < 1e-5);
        let yawed = heli_pose(&tags, &parents, 0.0, 90.0);
        assert!(run(&yawed, along_x.0, along_x.1).is_none());
        let (at, n) = run(&yawed, along_y.0, along_y.1).expect("the turned wall");
        assert!((at.y - 10.0).abs() < 0.2, "{at}");
        assert!(n.distance(Vec3::NEG_X) < 1e-5, "{n}");
    }

    /// A rider keeps its place and turn in the body's frame: 64 ahead of a
    /// body that moves 100 along x and turns 90 left ends 64 to its left
    /// of the new origin, turned with it.
    #[test]
    fn a_rider_turns_and_moves_with_the_body() {
        let (at, angles) = carried(
            (Vec3::ZERO, Vec3::ZERO),
            (Vec3::new(100.0, 0.0, 0.0), Vec3::new(0.0, 90.0, 0.0)),
            (Vec3::new(64.0, 0.0, 8.0), Vec3::new(0.0, 10.0, 0.0)),
        );
        assert!(at.distance(Vec3::new(100.0, 64.0, 8.0)) < 1e-3, "{at}");
        assert!((angles.y - 100.0).abs() < 1e-3, "{angles}");
    }

    #[test]
    fn the_vehicle_file_reads_its_turret() {
        let v = VehInfo::parse(
            "VEHICLEFILE\\type\\helicopter\\turretRotRate\\80\\turretVertSpanUp\\25",
        );
        assert_eq!(v.turret_rot_rate, 80.0);
        assert_eq!(v.span_up, 25.0);
    }
}
