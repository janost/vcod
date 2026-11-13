//! A vehicle's guns firing (docs/research/coduo-vehicles.md 13):
//! `CMD_VEH_FireTurret` (game.mp.uo 0x8ee02), `CMD_VEH_FireAltTurret`
//! (0x8f57e) and `G_VEH_FireGunner` (0x8fc7a). The host aims the round off
//! the posed vehicle; the server traces it against the bodies and hands the
//! result back to [`landed`].

use super::physics::angle_subtract;
use super::{Gun, seat};
use crate::game::builtins::math::vectoangles;
use crate::game::combat::{UO_BULLET_RANGE, gun_random, rocket_dir, spread_point};
use crate::game::host::GameHost;
use glam::{Quat, Vec3};
use vcod_common::net::event_ids::{EV_FIRE_WEAPON, EV_FIRE_WEAPONB, EV_UO_FIRE_WEAPONC};
use vcod_common::patch::Arith;
use vcod_gsc::{Cx, EntId};

/// The driver's aim point reaches this far along the view (0x86c57).
const AIM_REACH: f32 = 10240.0;
/// The reader's locational trace mask (0x86c57): solid, glass, 0x80 and
/// 0x2000, so no vehicle's hull or body stops the aim.
const AIM_MASK: u32 = 0x2091;
/// A turret round bends toward the driver's aim point by at most this much
/// pitch and yaw off the barrel (0x8ee02).
const NUDGE_PITCH: f32 = 5.0;
const NUDGE_YAW: f32 = 2.0;
/// The main gun jolts the hull this hard (0x8ee02).
const MAIN_JOLT: f32 = 0.75;
/// The MG loops' hold after each round (`rec+0x2e4`, `0x2f0`).
const LOOP_HOLD_MS: i32 = 200;
/// `CONTENTS_BODY`, which the barrel test drops from the clipmask.
const CONTENTS_BODY: u32 = 0x2000000;
/// Event parm bit 7: the server drew neither impact nor tracer, the client
/// traces the round itself.
pub const CLIENT_TRACED: i32 = 0x80;

/// One round to trace.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub vehicle: EntId,
    pub gun: Gun,
    /// The seated player the round is charged to.
    pub attacker: usize,
    pub weapon: usize,
    pub muzzle: Vec3,
    /// A shell's launch direction; a bullet's forward, before its spread.
    pub dir: Vec3,
    /// Where a bullet's round ends, spread in (`BG_Bullet_Endpos`); a
    /// shell's muzzle.
    pub end: Vec3,
    pub damage: i32,
    pub rifle_bullet: bool,
    /// A projectile weapon's shell rather than a bullet.
    pub projectile: bool,
    /// `tag_barrel`'s forward: the main gun jolts the hull back along it.
    pub barrel: Vec3,
}

/// The bend toward the driver's aim point as the fire calls run it
/// (`CMD_VEH_FireTurret` 0x8f0fe..0x8f2ae, the alt gun's alike):
/// `aim - muzzle` stored and `VectorNormalize`d, both directions through
/// `vectoangles`, their `AnglesSubtract` clamped to `NUDGE_PITCH` and
/// `NUDGE_YAW`, the barrel's angles less that through `AnglesSubtract`
/// again (roll 0), and `AngleVectors`' forward of the result.
fn bend(barrel: Vec3, muzzle: Vec3, aim: [f32; 3]) -> Vec3 {
    let want = vcod_common::x87::normalize_stored(Vec3::from(aim) - muzzle);
    let have = vectoangles(barrel.to_array());
    let want = vectoangles(want.to_array());
    let dp = angle_subtract(have[0], want[0]).clamp(-NUDGE_PITCH, NUDGE_PITCH);
    let dy = angle_subtract(have[1], want[1]).clamp(-NUDGE_YAW, NUDGE_YAW);
    Vec3::from(crate::game::spawn::angle_forward([
        angle_subtract(have[0], dp),
        angle_subtract(have[1], dy),
        0.0,
    ]))
}

/// What the driver's reader leaves for the guns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// `rec+0x28c`.
    pub point: [f32; 3],
    /// `rec+0x270`.
    pub barrel_blocked: bool,
}

/// The driver's reader's aim point (0x86c57, `rec+0x28c`): from the chase
/// camera (the gunsight when `sighted`) 10240 along `view`, stopped by a
/// locational trace under 0x2091, which meets no vehicle and no body.
/// Before that trace a `trap_Trace` runs from `tag_barrel` to `tag_flash`
/// under the vehicle's clipmask less `CONTENTS_BODY`, passing the vehicle;
/// when it meets anything the reader sets `rec+0x270` and keeps the far
/// point untraced (0x870e9..0x87192). Retail's think stores both after the
/// frame's threads ran, so a gsc `setPlayerAngles` on the driver turns the
/// aim.
pub fn aim_point(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    view: [f32; 3],
    sighted: bool,
) -> Option<Aim> {
    let posed = seat::posed(host, cx, id)?;
    let eye_tag = if sighted {
        "tag_aimdownbarrel"
    } else {
        "tag_chasecam"
    };
    let (eye, _) = posed.tag(eye_tag)?;
    let fwd = Vec3::from(crate::game::spawn::angle_forward(view));
    let mut aim = eye + fwd * AIM_REACH;
    let v = host.vehicles.live.get(&id)?;
    let mask = v.phys.clipmask & !CONTENTS_BODY;
    let skip = v
        .collmap
        .map(|_| vcod_common::collision::Pusher::Copy(id.0));
    let mut barrel_blocked = false;
    if let Some(w) = host.world.as_ref() {
        if let (Some((barrel, _)), Some((flash, _))) =
            (posed.tag("tag_barrel"), posed.tag("tag_flash"))
        {
            barrel_blocked = w
                .collision
                .box_sweep(barrel, flash, Vec3::ZERO, Vec3::ZERO, mask, skip)
                .fraction
                != 1.0;
        }
        if !barrel_blocked {
            aim = w.collision.point_trace(eye, aim, AIM_MASK, true).endpos;
        }
    }
    Some(Aim {
        point: aim.to_array(),
        barrel_blocked,
    })
}

/// Where a gun's round leaves from and along, before any spread.
struct Line {
    muzzle: Vec3,
    dir: Vec3,
    /// The flash tag's frame.
    rot: Quat,
    /// `tag_barrel`'s forward.
    barrel: Vec3,
}

/// The fire calls' muzzle and direction for `gun` off the posed vehicle and
/// the driver's last aim point.
fn line(host: &mut GameHost, cx: &mut Cx, id: EntId, gun: Gun) -> Option<Line> {
    let v = host.vehicles.live.get(&id)?;
    let aim_point = v.aim_point;
    let barrel_blocked = v.barrel_blocked;
    let posed = seat::posed(host, cx, id)?;
    let flash = match gun {
        Gun::Main => "tag_flash",
        Gun::Alt => "tag_altfire",
        Gun::Gunner => "tag_secondary_flash",
    };
    let (muzzle, rot): (Vec3, Quat) = posed.tag(flash)?;
    let mut dir = rot * Vec3::X;
    // The main and alt guns bend toward the driver's aim point; the
    // gunner's MG fires along its barrel. A blocked barrel bends nothing.
    if gun != Gun::Gunner
        && !barrel_blocked
        && let Some(aim) = aim_point
    {
        dir = bend(dir, muzzle, aim);
    }
    let barrel_tag = posed.tag("tag_barrel");
    let barrel = barrel_tag.map_or(dir, |(_, r)| r * Vec3::X);
    // With the barrel blocked every gun fires from its flash pulled back
    // along the round by the barrel's length (`rec+0x26c`, `tag_flash` to
    // `tag_barrel` as `Scr_Vehicle_Init` posed them; both ride the barrel,
    // so the current pose measures the same), ahead of the spread
    // (0x8f33c, 0x8fa79, 0x8ffaf).
    let muzzle = match (barrel_blocked, barrel_tag, posed.tag("tag_flash")) {
        (true, Some((b, _)), Some((f, _))) => muzzle - dir * f.distance(b),
        _ => muzzle,
    };
    Some(Line {
        muzzle,
        dir,
        rot,
        barrel,
    })
}

/// The muzzle a main-gun shell would leave from now and its direction (a
/// shell has no spread), and where the driver's chase camera sits: what a
/// bot in the driver's seat lays the gun with. `None` without a turret
/// gun or its tags.
pub fn main_gun_sight(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    let Line { muzzle, dir, .. } = line(host, cx, id, Gun::Main)?;
    let (eye, _) = seat::posed(host, cx, id)?.tag("tag_chasecam")?;
    Some((muzzle.to_array(), dir.to_array(), eye.to_array()))
}

/// The fire call's aim and round, `None` where retail refuses: no rider in
/// the seat, an overheated MG, a gun the vehicle lacks, a missing tag.
pub fn prepare(host: &mut GameHost, cx: &mut Cx, id: EntId, gun: Gun) -> Option<Shot> {
    let v = host.vehicles.live.get(&id)?;
    let (seat_no, weapon) = match gun {
        Gun::Main => (1, v.weapons[0]),
        Gun::Alt => (1, v.weapons[1]),
        Gun::Gunner => (2, v.weapons[2]),
    };
    let attacker = v.seats[seat_no]?;
    if weapon == 0
        || (gun == Gun::Alt && v.guns.overheating[0])
        || (gun == Gun::Gunner && v.guns.overheating[1])
    {
        return None;
    }
    let def = host.weapons.get(weapon)?.clone();
    let Line {
        muzzle,
        dir,
        rot,
        barrel,
    } = line(host, cx, id, gun)?;
    let projectile = def.weapon_type == "projectile";
    // Both arms get the flash tag's frame with the bent forward: its left
    // (`axis[1]`) as the spread's right, its up, then the muzzle.
    let frame = [dir, rot * Vec3::Y, rot * Vec3::Z];
    let (dir, end) = if projectile {
        // `Weapon_Artillery_Fire(vehicle, 0, frame)` (0x7b594): the rocket's
        // spread point 16 out at spread 0, `gunrandom` drawn all the same,
        // then normalized; `fire_rocket` normalizes it again.
        let draw = gun_random(&mut host.rng);
        (rocket_dir(&frame, 0.0, draw, Arith::Stored), muzzle)
    } else {
        // `minSpread + rand * maxSpread` degrees (sic) into `Bullet_Fire`'s
        // `BG_Bullet_Endpos`.
        let (lo, hi) = (def.min_spread.unwrap_or(0.0), def.max_spread.unwrap_or(0.0));
        let spread = lo + host.rand_unit() * hi;
        let draw = gun_random(&mut host.rng);
        let end = spread_point(&frame, muzzle, spread, UO_BULLET_RANGE, draw, Arith::Stored);
        (dir, end)
    };
    Some(Shot {
        vehicle: id,
        gun,
        attacker,
        weapon,
        muzzle,
        dir,
        end,
        damage: def.damage,
        rifle_bullet: def.sounds.rifle_bullet,
        projectile,
        barrel,
    })
}

/// Whether `gun` on vehicle `id` fires a shell (`weaponType` projectile),
/// which the fire call spawns itself ([`launch_shell`]); a bullet gun's
/// round waits for the server's trace.
pub fn fires_shell(host: &GameHost, id: EntId, gun: Gun) -> bool {
    let Some(v) = host.vehicles.live.get(&id) else {
        return false;
    };
    let weapon = match gun {
        Gun::Main => v.weapons[0],
        Gun::Alt => v.weapons[1],
        Gun::Gunner => v.weapons[2],
    };
    host.weapons
        .get(weapon)
        .is_some_and(|d| d.weapon_type == "projectile")
}

/// `Weapon_Artillery_Fire`'s `fire_rocket` inside the fire call, as
/// `CMD_VEH_FireTurret` runs it: the shell exists, with its number and
/// `classname`, before the calling thread's next instruction
/// (docs/research/cod11-combat.md 13.7). The driver is credited through
/// the vehicle.
pub fn launch_shell(host: &mut GameHost, cx: &mut Cx, shot: &Shot) {
    let Some(def) = host.weapons.get(shot.weapon).cloned() else {
        return;
    };
    let shell = crate::game::missile::Shell {
        drop_at_ms: None,
        speed: def.projectile_speed,
        damage: def.damage,
        vehicle: Some(shot.vehicle),
    };
    let kind = crate::game::missile::explosion_type(&def.proj_explosion_type);
    let now = host.level_time_ms;
    if let Err(e) = host.missiles.fire_rocket(
        &mut host.ents,
        cx,
        shot.attacker,
        shot.weapon as u8,
        shot.muzzle,
        shot.dir,
        shell,
        kind,
        true,
        now,
    ) {
        log::warn!("vehicle {} shell not spawned: {e:?}", shot.vehicle.0);
    }
    landed(host, shot, false);
}

/// What the fire call leaves on the vehicle once its round is out: the gun
/// in `s.weapon`, the fire event on its own ring (163 main, 164 alt, 165
/// gunner; a bullet's parm the weapon with bit 7 unless it struck a
/// player, a shell's 0), the gun's timer, an MG's heat and loop hold.
pub fn landed(host: &mut GameHost, shot: &Shot, struck_player: bool) {
    let Some(v) = host.vehicles.live.get_mut(&shot.vehicle) else {
        return;
    };
    let def = host.weapons.get(shot.weapon);
    let fire_ms = def.map_or(0, |d| vcod_common::weapon::stored_ms(d.fire_time));
    let heat = def.map_or(0.0, |d| d.fire_heat);
    v.wire.weapon = shot.weapon as i32;
    let parm = if shot.projectile {
        0
    } else {
        shot.weapon as i32 | if struck_player { 0 } else { CLIENT_TRACED }
    };
    let event = match shot.gun {
        Gun::Main => {
            v.guns.timers[0] = fire_ms;
            let info = host.vehicles.infos.list[v.info].clone();
            v.phys
                .jolt(&info, -shot.barrel, MAIN_JOLT, 0.0, 0.0, host.level_time_ms);
            EV_FIRE_WEAPON
        }
        Gun::Alt => {
            v.guns.timers[1] = fire_ms;
            v.guns.heat[0] += heat;
            v.guns.loop_hold[0] = LOOP_HOLD_MS;
            EV_FIRE_WEAPONB
        }
        Gun::Gunner => {
            v.guns.timers[2] = fire_ms;
            v.guns.heat[1] += heat;
            v.guns.loop_hold[1] = LOOP_HOLD_MS;
            EV_UO_FIRE_WEAPONC
        }
    };
    if let Some(e) = host.ents.get_mut(shot.vehicle) {
        e.events.add(event, parm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mp_kharkov's Panzer IV under `probe_vehicle_shell`, read under gdb
    /// on retail UO 1.51b (coduo-vehicles.md 13, "The shell's bits"): the
    /// flash tag's forward and origin, the driver's aim point, the bend's
    /// angles, the bent forward, `Weapon_Artillery_Fire`'s spread point 16
    /// out for each of the three shells' `gunrandom` pairs, and the
    /// `trDelta` `fire_rocket` truncates at 10000.
    #[test]
    fn a_uo_tank_shell_leaves_on_retails_bits() {
        let v = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        let barrel = v([0xbf78dce3, 0xbdc941b0, 0xbe5a0188]);
        let left = v([0x3e49d831, 0x3e18d830, 0xbf780d35]);
        let up = v([0x3e020b47, 0xbf7be108, 0xbe00bf80]);
        let muzzle = v([0xc4aef53e, 0x45c1ad08, 0x40e2db6a]);
        let aim = v([0xc4b81b42, 0x45c18e96, 0xc27985ae]).to_array();

        let want = vcod_common::x87::normalize_stored(Vec3::from(aim) - muzzle);
        assert_eq!(bits(want), [0xbf398ad8, 0xbd1a5de2, 0xbf301dc5]);
        let have = vectoangles(barrel.to_array());
        let want = vectoangles(want.to_array());
        assert_eq!(have.map(f32::to_bits), [0x4144aca2, 0x4339c5b6, 0]);
        assert_eq!(want.map(f32::to_bits), [0x422ddfa1, 0x4336fa03, 0]);
        let sub = [
            angle_subtract(have[0], want[0]),
            angle_subtract(have[1], want[1]),
        ];
        assert_eq!(sub.map(f32::to_bits), [0xc1f968f1, 0x4032ecc0]);

        // Clamped to (-5, 2) and taken off the barrel's angles: the yaw
        // wraps to -176.2.
        let fwd = bend(barrel, muzzle, aim);
        assert_eq!(bits(fwd), [0xbf73e64d, 0xbd80a6b4, 0xbe98305e]);

        let frame = [fwd, left, up];
        for draw in [
            [0x3df487c8, 0x3f5a556d],
            [0xbe941d87, 0x3eeec3b0],
            [0x3eb95d37, 0xbeaeb1e1],
        ] {
            let draw = (f32::from_bits(draw[0]), f32::from_bits(draw[1]));
            let pre = spread_point(&frame, Vec3::ZERO, 0.0, 16.0, draw, Arith::Stored);
            assert_eq!(bits(pre), [0xc173e64d, 0xbf80a6b4, 0xc098305e]);
            let dir = rocket_dir(&frame, 0.0, draw, Arith::Stored);
            assert_eq!(bits(dir), bits(fwd));
            let dir = crate::game::missile::rocket_launch_dir(dir, true);
            assert_eq!(
                bits((dir * 10000.0).trunc()),
                [0xc614dc00, 0xc41d0000, 0xc539c000]
            );
        }
    }
}
