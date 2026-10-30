//! CoD4 1.7's hardpoint vehicles' builtins: the airstrike's `spawnPlane`,
//! the helicopter's `spawnHelicopter` and its vehicle methods (the 1.7
//! vehicle table at 0x821be40, cod4-gsc.md 2), the missile attractors
//! and repulsors the helicopter's flares use, and the two physics calls
//! (docs/research/cod4-gsc.md 3.2). The level flies and fires what these
//! set (`game::cod4::vehicle`). Semantics are INFERRED
//! (`GScr_SpawnPlane`, `GScr_SpawnHelicopter`, `CMD_VEH_*`).

use glam::Vec3;
use vcod_gsc::{Cx, ErrorKind, Host, Target, Value};

use super::entity::entity_receiver;
use super::score::Builtin;
use crate::game::cod4::vehicle::{Heli4, MPH, TurretTarget, VehInfo, VehShot};
use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};

pub const NAMES: &[(&str, Builtin)] = &[
    ("spawnplane", spawn_plane),
    ("spawnhelicopter", spawn_helicopter),
    ("freehelicopter", free_helicopter),
    ("setspeed", set_speed),
    ("getspeed", get_speed),
    ("getspeedmph", get_speed_mph),
    ("setyawspeed", set_yaw_speed),
    ("setmaxpitchroll", set_max_pitch_roll),
    ("setturningability", set_turning_ability),
    ("sethoverparams", set_hover_params),
    ("setairresistance", set_air_resistance),
    ("setneargoalnotifydist", set_near_goal_notify_dist),
    ("setvehgoalpos", set_veh_goal_pos),
    ("setgoalyaw", set_goal_yaw),
    ("cleargoalyaw", clear_goal_yaw),
    ("settargetyaw", set_target_yaw),
    ("cleartargetyaw", clear_target_yaw),
    ("setvehweapon", set_veh_weapon),
    ("fireweapon", fire_weapon),
    ("setturrettargetvec", set_turret_target_vec),
    ("setturrettargetent", set_turret_target_ent),
    ("clearturrettarget", clear_turret_target),
    ("setdamagestage", set_damage_stage),
    ("missile_createattractorent", missile_create_attractor_ent),
    (
        "missile_createattractororigin",
        missile_create_attractor_origin,
    ),
    ("missile_createrepulsorent", missile_create_repulsor_ent),
    (
        "missile_createrepulsororigin",
        missile_create_repulsor_origin,
    ),
    ("missile_deleteattractor", missile_delete_attractor),
    ("missile_settarget", missile_set_target),
    ("physicslaunch", physics_launch),
    ("physicsexplosionsphere", physics_explosion_sphere),
    ("physicsexplosioncylinder", physics_explosion_cylinder),
];

/// `EV_PHYS_EXPLOSION_SPHERE`: 1.7's 57 (`vcod_cod4::net::events`).
const EV_PHYS_EXPLOSION_SPHERE: i32 = 57;
/// `EV_PHYS_EXPLOSION_CYLINDER`: 58.
const EV_PHYS_EXPLOSION_CYLINDER: i32 = 58;
/// `TR_PHYSICS`.
pub const TR_PHYSICS: u32 = 8;

fn num(v: Option<&Value>, what: &'static str) -> Result<f32, ErrorKind> {
    match v {
        Some(&Value::Int(i)) => Ok(i as f32),
        Some(&Value::Float(f)) => Ok(f),
        _ => Err(ErrorKind::BadType(what)),
    }
}

fn vec(v: Option<&Value>, what: &'static str) -> Result<Vec3, ErrorKind> {
    match v {
        Some(&Value::Vector(v)) => Ok(Vec3::from(v)),
        _ => Err(ErrorKind::BadType(what)),
    }
}

fn string(cx: &Cx, v: Option<&Value>, what: &'static str) -> Result<String, ErrorKind> {
    match v {
        Some(&Value::String(s)) => Ok(cx.resolve(s).to_string()),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// The owner argument: a player's slot and its `sess.cs.team` (0 free,
/// 1 axis, 2 allies, 3 spectator).
fn owner(host: &mut GameHost, cx: &mut Cx, v: Option<&Value>) -> Result<(usize, u32), ErrorKind> {
    let Some(&Value::Entity(id)) = v else {
        return Err(ErrorKind::Custom("Owner entity is not a player".into()));
    };
    if host.ents.get(id).is_none_or(|e| e.client.is_none()) {
        return Err(ErrorKind::Custom("Owner entity is not a player".into()));
    }
    let f = cx.intern_folded("sessionteam");
    let team = match host.get_field(cx, id, f) {
        Value::String(t) => match cx.resolve(t) {
            "axis" => 1,
            "allies" => 2,
            "spectator" => 3,
            _ => 0,
        },
        _ => 0,
    };
    Ok((id.0 as usize, team))
}

/// `spawnPlane(owner, classname, origin[, spawnflags])`: the classname's
/// spawn (a `script_model`), then `ET_PLANE` and the owner's team and
/// slot in `teamAndOwnerIndex`.
fn spawn_plane(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (slot, team) = owner(host, cx, a.first())?;
    let rest: Vec<Value> = a.iter().skip(1).copied().collect();
    let v = super::entity::spawn(host, cx, None, &rest)?;
    if let Value::Entity(id) = v {
        host.cod4_mut().planes.insert(id, team | (slot as u32) << 2);
    }
    Ok(v)
}

/// `spawnHelicopter(owner, origin, angles, vehicleInfo, model)`: a
/// `script_vehicle` with the model, `ET_HELICOPTER`, its turret read off
/// the `vehicles/<info>` file.
fn spawn_helicopter(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (slot, team) = owner(host, cx, a.first())?;
    let origin = vec(a.get(1), "spawnHelicopter takes an origin")?;
    let angles = vec(a.get(2), "spawnHelicopter takes angles")?;
    let info_name = string(cx, a.get(3), "spawnHelicopter takes a vehicle info")?;
    let model = string(cx, a.get(4), "spawnHelicopter takes a model")?;
    let id = host.ents.spawn(cx)?;
    let cn = cx.intern_folded("classname");
    let class = Value::String(cx.intern_exact("script_vehicle"));
    host.set_field(cx, id, cn, class)?;
    let og = cx.intern_folded("origin");
    host.set_field(cx, id, og, Value::Vector(origin.to_array()))?;
    let an = cx.intern_folded("angles");
    host.set_field(cx, id, an, Value::Vector(angles.to_array()))?;
    let m = Value::String(cx.intern_exact(&model));
    super::entity::set_model(host, cx, Some(Target::Entity(id)), &[m])?;
    let text = host
        .assets
        .as_ref()
        .and_then(|s| s.read(&format!("vehicles/{info_name}")))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let info = VehInfo::parse(&text);
    let mut h = Heli4::new(id, slot, team, origin, angles);
    // `s.index`: the model's configstring under the models' base (830).
    let (lo, hi) = crate::game::cod4::CS_LAYOUT_COD4.ranges[3];
    h.model_index = (lo..=hi)
        .find(|&i| host.configstrings[i].eq_ignore_ascii_case(&model))
        .map_or(0, |i| (i - (lo - 1)) as u32);
    h.model = model;
    h.weapon = VehInfo::turret_weapon(&text).map_or(0, |w| weapon_index(host, w));
    h.info = info;
    host.cod4_mut().helis.push(h);
    Ok(Value::Entity(id))
}

/// The receiver's helicopter, for a method to change.
fn heli(host: &mut GameHost, recv: Option<Target>) -> Result<&mut Heli4, ErrorKind> {
    let id = entity_receiver(recv)?;
    host.cod4_mut()
        .helis
        .iter_mut()
        .find(|h| h.id == id)
        .ok_or_else(|| ErrorKind::Custom(format!("entity {} is not a vehicle", id.0)))
}

/// `freeHelicopter()`: the vehicle state goes; the entity stays.
fn free_helicopter(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    host.cod4_mut().helis.retain(|h| h.id != id);
    Ok(Value::Undefined)
}

/// `setSpeed(mph[, accel[, decel]])`: units a second (17.6 a mph); the
/// deceleration half the acceleration unless given; a helicopter speeding
/// up caps its acceleration at its speed.
fn set_speed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let h = heli(host, recv)?;
    let speed = num(a.first(), "setSpeed takes a speed")? * MPH;
    if speed < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot set negative speed on vehicle".into(),
        ));
    }
    h.pace.speed = speed;
    if a.len() > 1 {
        h.pace.accel = num(a.get(1), "setSpeed takes an acceleration")? * MPH;
    }
    if h.speed < h.pace.speed && h.pace.accel > h.pace.speed {
        h.pace.accel = h.pace.speed;
    }
    h.pace.decel = if a.len() > 2 {
        num(a.get(2), "setSpeed takes a deceleration")? * MPH
    } else {
        h.pace.accel * 0.5
    };
    if h.pace.accel <= 0.0 || h.pace.decel <= 0.0 {
        h.pace.accel = MPH;
        h.pace.decel = MPH;
        return Err(ErrorKind::Custom(
            "Acceleration/deceleration must be > 0".into(),
        ));
    }
    Ok(Value::Undefined)
}

fn get_speed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(heli(host, recv)?.speed))
}

fn get_speed_mph(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(heli(host, recv)?.speed / MPH))
}

/// `setYawSpeed(speed, accel[, decel[, overshoot]])`, degrees a second.
fn set_yaw_speed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let speed = num(a.first(), "setYawSpeed takes a speed")?;
    let accel = num(a.get(1), "setYawSpeed takes an acceleration")?;
    let decel = match a.get(2) {
        Some(v) => num(Some(v), "setYawSpeed takes a deceleration")?,
        None => accel,
    };
    let overshoot = a
        .get(3)
        .map(|v| num(Some(v), "setYawSpeed takes an overshoot"))
        .transpose()?;
    let h = heli(host, recv)?;
    h.yaw.top = speed;
    h.yaw.up = accel;
    h.yaw.down = decel;
    if let Some(o) = overshoot {
        if !(0.0..=1.0).contains(&o) {
            return Err(ErrorKind::Custom(
                "Overshoot must be in 0 to 1 range".into(),
            ));
        }
        h.yaw.overshoot = o;
    }
    if speed < 0.0 || accel < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot set negative yaw speed on vehicle".into(),
        ));
    }
    Ok(Value::Undefined)
}

fn set_max_pitch_roll(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let p = num(a.first(), "setMaxPitchRoll takes a pitch")?;
    let r = num(a.get(1), "setMaxPitchRoll takes a roll")?;
    let h = heli(host, recv)?;
    h.max_pitch = p;
    h.max_roll = r;
    Ok(Value::Undefined)
}

/// `setHoverParams(radius[, speed[, accel]])`: the radius in units, the
/// speed and rate as given (not mph).
fn set_hover_params(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let r = num(a.first(), "setHoverParams takes a radius")?;
    let speed = a
        .get(1)
        .map(|v| num(Some(v), "setHoverParams takes a speed"))
        .transpose()?;
    let accel = a
        .get(2)
        .map(|v| num(Some(v), "setHoverParams takes an acceleration"))
        .transpose()?;
    let h = heli(host, recv)?;
    h.hover.radius = r;
    if let Some(s) = speed {
        h.hover.speed = s;
    }
    if let Some(x) = accel {
        h.hover.rate = x;
    }
    Ok(Value::Undefined)
}

/// `setAirResistance(mph)`: the fake drag's full speed.
fn set_air_resistance(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = num(a.first(), "setAirResistance takes a speed")? * MPH;
    heli(host, recv)?.air_resistance = s;
    Ok(Value::Undefined)
}

fn set_turning_ability(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let t = num(a.first(), "setTurningAbility takes a number")?;
    heli(host, recv)?.turning_ability = t;
    Ok(Value::Undefined)
}

fn set_near_goal_notify_dist(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let d = num(a.first(), "setNearGoalNotifyDist takes a distance")?;
    heli(host, recv)?.near_goal_dist = d;
    Ok(Value::Undefined)
}

/// `setVehGoalPos(pos[, stop])`: the flight heads there, stopping on it
/// with a true `stop`; the speeds must be set first.
fn set_veh_goal_pos(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let goal = vec(a.first(), "setVehGoalPos takes a position")?;
    let stop = match a.get(1) {
        Some(v) => v.as_bool()?,
        None => false,
    };
    let h = heli(host, recv)?;
    h.goal = goal;
    h.stop_at_goal = stop;
    h.moving = true;
    h.hovering = false;
    h.stopping = false;
    if h.pace.speed == 0.0 || h.pace.accel == 0.0 || h.pace.decel == 0.0 {
        return Err(ErrorKind::Custom(
            "Speed and acceleration must not be zero before setting goal pos".into(),
        ));
    }
    Ok(Value::Undefined)
}

fn set_goal_yaw(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let y = num(a.first(), "setGoalYaw takes a yaw")?;
    heli(host, recv)?.goal_yaw = Some(y);
    Ok(Value::Undefined)
}

fn clear_goal_yaw(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    heli(host, recv)?.goal_yaw = None;
    Ok(Value::Undefined)
}

fn set_target_yaw(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let y = num(a.first(), "setTargetYaw takes a yaw")?;
    heli(host, recv)?.target_yaw = Some(y);
    Ok(Value::Undefined)
}

fn clear_target_yaw(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    heli(host, recv)?.target_yaw = None;
    Ok(Value::Undefined)
}

/// The registered weapon a name names, 0 for none
/// (`BG_FindWeaponIndexForName`).
fn weapon_index(host: &GameHost, name: &str) -> u32 {
    host.cod4()
        .table
        .index(&name.to_ascii_lowercase())
        .map_or(0, |i| i as u32)
}

/// `setVehWeapon(name)`: `s.weapon`.
fn set_veh_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = string(cx, a.first(), "setVehWeapon takes a weapon")?;
    let w = weapon_index(host, &name);
    heli(host, recv)?.weapon = w;
    Ok(Value::Undefined)
}

/// `fireWeapon([tag[, target[, offset]]])`: one round of `s.weapon` from
/// the tag, which the level's next vehicle pass flies; a projectile
/// weapon hands back its missile entity at once.
fn fire_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let weapon = heli(host, recv)?.weapon;
    if weapon == 0 {
        return Err(ErrorKind::Custom("Invalid weapon specified".into()));
    }
    let tag = match a.first() {
        Some(v) => Some(string(cx, Some(v), "fireWeapon takes a tag")?.to_ascii_lowercase()),
        None => None,
    };
    let target = match a.get(1) {
        Some(&Value::Entity(t)) => Some(t),
        _ => None,
    };
    let projectile = host
        .cod4()
        .table
        .defs
        .get(weapon as usize)
        .map(|d| d.name.clone())
        .and_then(|n| host.cod4().weapon_files.get(&n).cloned())
        .and_then(|f| f.pm.as_ref().map(|p| p.weap_type))
        == Some(crate::game::cod4::combat::WEAPTYPE_PROJECTILE);
    let missile = if projectile {
        let m = host.ents.spawn(cx)?;
        let cn = cx.intern_folded("classname");
        let class = Value::String(cx.intern_exact("rocket"));
        host.set_field(cx, m, cn, class)?;
        Some(m)
    } else {
        None
    };
    host.cod4_mut().vehicle_shots.push(VehShot {
        heli: id,
        weapon,
        tag,
        target,
        target_offset: match a.get(2) {
            Some(&Value::Vector(v)) => Vec3::from(v),
            _ => Vec3::ZERO,
        },
        missile,
    });
    Ok(missile.map_or(Value::Undefined, Value::Entity))
}

fn set_turret_target_vec(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let p = vec(a.first(), "setTurretTargetVec takes a position")?;
    heli(host, recv)?.target = Some(TurretTarget::Pos(p));
    Ok(Value::Undefined)
}

fn set_turret_target_ent(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(&Value::Entity(t)) = a.first() else {
        return Err(ErrorKind::BadType("setTurretTargetEnt takes an entity"));
    };
    let off = match a.get(1) {
        Some(v) => vec(Some(v), "setTurretTargetEnt takes an offset")?,
        None => Vec3::ZERO,
    };
    heli(host, recv)?.target = Some(TurretTarget::Ent(t, off));
    Ok(Value::Undefined)
}

fn clear_turret_target(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    heli(host, recv)?.target = None;
    Ok(Value::Undefined)
}

/// `setDamageStage(n)`: `un1.helicopterStage`, the client's smoke and
/// fire (3 whole .. 0 wrecked).
fn set_damage_stage(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = num(a.first(), "setDamageStage takes a stage")? as u32;
    heli(host, recv)?.stage = s & 7;
    Ok(Value::Undefined)
}

/// `missile_create{Attractor,Repulsor}{Ent,Origin}(what, strength,
/// maxDist)` (0x8093486, 0x8096c28, 0x8096b5c, 0x8096a80): the first free
/// of the 32 slots takes the entity (or the point), the strength and the
/// reach, and its number comes back. `G_RunMissile` bends an unguided
/// rocket by every slot in use (`Cod4Level::attract`).
fn create_attractor(
    host: &mut GameHost,
    a: &[Value],
    attract: bool,
    on_entity: bool,
) -> Result<Value, ErrorKind> {
    // `G_FreeEntity` clears the slots naming the entity it frees
    // (0x8093402); vcod clears them when a slot is next looked for.
    for slot in 0..crate::game::cod4::missile::MAX_ATTRACTORS {
        let gone = host.cod4().attractors[slot]
            .and_then(|a| a.entity)
            .is_some_and(|e| host.ents.get(e).is_none());
        if gone {
            host.cod4_mut().attractors[slot] = None;
        }
    }
    let Some(slot) = host.cod4().attractors.iter().position(Option::is_none) else {
        return Err(ErrorKind::Custom(format!(
            "Ran out of attractor/repulsors.  Max allowed: {}",
            crate::game::cod4::missile::MAX_ATTRACTORS
        )));
    };
    let (entity, point) = if on_entity {
        match a.first() {
            Some(&Value::Entity(e)) => (Some(e), Vec3::ZERO),
            _ => return Err(ErrorKind::BadType("not an entity")),
        }
    } else {
        (None, vec(a.first(), "not a vector")?)
    };
    let strength = num(a.get(1), "not a float")?;
    let reach = num(a.get(2), "not a float")?;
    if reach <= 0.0 {
        return Err(ErrorKind::Custom(
            "maxDist must be greater than zero".into(),
        ));
    }
    host.cod4_mut().attractors[slot] = Some(crate::game::cod4::missile::Attractor {
        attract,
        entity,
        point,
        strength,
        reach,
    });
    Ok(Value::Int(slot as i32))
}

fn missile_create_attractor_ent(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    create_attractor(host, a, true, true)
}

fn missile_create_attractor_origin(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    create_attractor(host, a, true, false)
}

fn missile_create_repulsor_ent(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    create_attractor(host, a, false, true)
}

fn missile_create_repulsor_origin(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    create_attractor(host, a, false, false)
}

/// `missile_deleteAttractor(n)` (0x80933b0): slot `n` free again.
fn missile_delete_attractor(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let n = match a.first() {
        Some(&Value::Int(n)) => n,
        _ => return Err(ErrorKind::BadType("not an int")),
    };
    let slots = &mut host.cod4_mut().attractors;
    match usize::try_from(n).ok().and_then(|n| slots.get_mut(n)) {
        Some(slot) => *slot = None,
        None => return Err(ErrorKind::Custom("Invalid attractor or repulsor".into())),
    }
    Ok(Value::Undefined)
}

/// `rocket missile_setTarget([ent[, offset]])` (0x80c5f34): the rocket's
/// guidance steers at the entity plus the offset (none without one).
fn missile_set_target(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let cn = cx.intern_folded("classname");
    let rocket =
        matches!(host.get_field(cx, id, cn), Value::String(s) if cx.resolve(s) == "rocket");
    if !rocket {
        return Err(ErrorKind::Custom(format!(
            "Entity {} is not a rocket\n",
            id.0
        )));
    }
    let target = match a.first() {
        Some(&Value::Entity(e)) => Some(e),
        _ => None,
    };
    let offset = match a.get(1) {
        Some(&Value::Vector(v)) => Vec3::from(v),
        _ => Vec3::ZERO,
    };
    if let Some(m) = host.cod4_mut().missiles.iter_mut().find(|m| m.id == id) {
        m.target = target;
        m.target_offset = offset;
    }
    Ok(Value::Undefined)
}

/// `ent physicsLaunch([contact, force])`: the entity's trajectories go
/// `TR_PHYSICS` from where it stands, the contact point and the force in
/// their deltas; the client's physics flies it. Only script models,
/// brush models, origins and lights take it. The part stops clipping and
/// taking damage (`r.contents` and `takedamage` 0, 0x809afb4).
fn physics_launch(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let cn = cx.intern_folded("classname");
    let class = match host.get_field(cx, id, cn) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    };
    if !matches!(
        class.as_str(),
        "script_brushmodel" | "script_model" | "script_origin" | "light"
    ) {
        return Err(ErrorKind::Custom(format!(
            "entity {} is not a script_brushmodel, script_model, script_origin, or light",
            id.0
        )));
    }
    let (contact, force) = if a.len() == 2 {
        (
            vec(a.first(), "physicsLaunch takes a contact point")?,
            vec(a.get(1), "physicsLaunch takes a force")?,
        )
    } else {
        (Vec3::ZERO, Vec3::ZERO)
    };
    let now = host.level_time_ms;
    let c4 = host.cod4_mut();
    if c4.physics.contains_key(&id) {
        return Err(ErrorKind::Custom(
            "physicslaunch called more than once for the same entity.".into(),
        ));
    }
    c4.physics
        .insert(id, (now, contact.to_array(), force.to_array()));
    super::entity::not_solid(host, cx, recv, &[])?;
    host.script_takedamage.remove(&id);
    Ok(Value::Undefined)
}

/// `physicsExplosionSphere(origin, outer, inner, magnitude)`: an
/// `EV_PHYS_EXPLOSION_SPHERE` temp entity, the outer radius its parm,
/// the inner radius and the magnitude in `lerp.u` words 0 and 1
/// (0x80bd00e). They ride `origin2` here, which
/// `Cod4Level::temp_entities4` writes to those words for this event.
fn physics_explosion_sphere(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    physics_explosion(host, cx, a, EV_PHYS_EXPLOSION_SPHERE)
}

/// `physicsExplosionCylinder`: [`physics_explosion_sphere`]'s arguments
/// and checks on an `EV_PHYS_EXPLOSION_CYLINDER`
/// (`Scr_PhysicsExplosionCylinder`), which the client measures
/// and pushes level (cod4-player-anims.md 10.1).
fn physics_explosion_cylinder(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    physics_explosion(host, cx, a, EV_PHYS_EXPLOSION_CYLINDER)
}

fn physics_explosion(
    host: &mut GameHost,
    cx: &mut Cx,
    a: &[Value],
    event: i32,
) -> Result<Value, ErrorKind> {
    if a.len() != 4 {
        return Err(ErrorKind::Custom("Incorrect number of parameters".into()));
    }
    let origin = vec(a.first(), "physicsExplosion* takes an origin")?;
    let outer = num(a.get(1), "physicsExplosion* takes a radius")? as i32;
    let inner = num(a.get(2), "physicsExplosion* takes a radius")?;
    let magnitude = num(a.get(3), "physicsExplosion* takes a magnitude")?;
    if inner < 0.0 {
        return Err(ErrorKind::Custom("Radius is negative".into()));
    }
    if inner > outer as f32 {
        return Err(ErrorKind::Custom(
            "Inner radius is outside the outer radius".into(),
        ));
    }
    host.add_temp_entity(
        cx,
        TempEntity {
            event,
            parm: outer,
            surf_type: 0,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: origin.to_array(),
            scope: Scope::Broadcast,
            quake: None,
            origin2: Some([inner, magnitude, 0.0]),
        },
    );
    Ok(Value::Undefined)
}
