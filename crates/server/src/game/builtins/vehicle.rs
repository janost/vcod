//! United Offensive's vehicle builtins: the 31 methods `ScriptVehicle_GetMethod`
//! (game.mp.uo 0x8df6e) adds at 0xaec80, `precacheVehicle` and
//! `getNumVehicles`, and the entity methods the stock vehicle scripts call
//! (`spawnDuplicate`, `verifyPosition`, `doDamage`, ...)
//! (docs/research/coduo-vehicles.md 13). UO MP hands a script no vehicle
//! node, so the path methods (`attachPath` .. `resumeSpeed`) only check
//! their arguments and raise their errors (17).

use super::entity::entity_receiver;
use crate::game::host::GameHost;
use crate::game::vehicle::{Gun, Jolt, TurretTarget};
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("attachpath", attach_path),
    ("startpath", start_path),
    ("setswitchnode", set_switch_node),
    ("setwaitnode", set_wait_node),
    ("setwaitspeed", set_wait_speed),
    ("setspeed", set_speed),
    ("resumespeed", resume_speed),
    ("freevehicle", free_vehicle),
    ("precachevehicle", precache_vehicle),
    ("getnumvehicles", get_num_vehicles),
    ("joltbody", jolt_body),
    ("getwheelsurface", get_wheel_surface),
    ("getspeedmph", get_speed_mph),
    ("getvehicleowner", get_vehicle_owner),
    ("startenginesound", start_engine_sound),
    ("stopenginesound", stop_engine_sound),
    ("makevehicleusable", make_vehicle_usable),
    ("makevehicleunusable", make_vehicle_unusable),
    ("addvehicletocompass", add_vehicle_to_compass),
    ("removevehiclefromcompass", remove_vehicle_from_compass),
    ("setturrettargetvec", set_turret_target_vec),
    ("setturrettargetent", set_turret_target_ent),
    ("clearturrettarget", clear_turret_target),
    ("fireturret", fire_turret),
    ("firealtturret", fire_alt_turret),
    ("firegunner", fire_gunner),
    ("isturretready", is_turret_ready),
    ("get_fire_time", get_fire_time),
    ("getaltheat", get_alt_heat),
    ("getgunnerheat", get_gunner_heat),
    ("getaltoverheating", get_alt_overheating),
    ("getgunneroverheating", get_gunner_overheating),
    ("getdismountspot", get_dismount_spot),
    ("spawnduplicate", spawn_duplicate),
    ("unlinkfromworld", unlink_from_world),
    ("linkintoworld", link_into_world),
    ("verifyposition", verify_position),
    ("clearvehicleposition", clear_vehicle_position),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// `fn_7c280`: the receiver must be a `script_vehicle` with a record.
fn vehicle(host: &GameHost, recv: Option<Target>) -> Result<EntId, ErrorKind> {
    let id = entity_receiver(recv)?;
    if host.vehicles.live.contains_key(&id) {
        Ok(id)
    } else {
        Err(ErrorKind::Custom("entity is not a script_vehicle".into()))
    }
}

fn number(v: Option<&Value>) -> Option<f32> {
    match v? {
        Value::Int(i) => Some(*i as f32),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// `precacheVehicle(type)` (0x8b51c): `RegisterItem` on the three guns,
/// only while the level initializes.
fn precache_vehicle(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    if !host.initializing {
        return Err(ErrorKind::Custom(
            "precacheVehicle must be called before any wait statements in the level script".into(),
        ));
    }
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("precacheVehicle takes a vehicle type"));
    };
    let name = cx.resolve(*name).to_string();
    let Some(i) = host.vehicles.infos.find(&name) else {
        return Err(ErrorKind::Custom(format!(
            "Cannot find vehicle info for [{name}]"
        )));
    };
    let info = &host.vehicles.infos.list[i];
    let guns = [
        info.turret_weapon.clone(),
        info.turret_alt_weapon.clone(),
        info.gunner_weapon.clone(),
    ];
    for g in guns.iter().filter(|g| !g.is_empty()) {
        host.register_item(&g.to_ascii_lowercase())?;
    }
    Ok(Value::Undefined)
}

/// `getNumVehicles()`: the live vehicles.
fn get_num_vehicles(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(host.vehicles.live.len() as i32))
}

/// `joltBody(point, force [, speedFrac, decel])` (0x8e5dc).
fn jolt_body(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let (Some(Value::Vector(from)), Some(force)) = (args.first(), number(args.get(1))) else {
        return Err(ErrorKind::BadType("joltBody takes a point and a force"));
    };
    let speed_frac = number(args.get(2)).unwrap_or(0.0);
    if !(0.0..=1.0).contains(&speed_frac) {
        return Err(ErrorKind::Custom(
            "speed fraction must be from 0 to 1".into(),
        ));
    }
    let decel = number(args.get(3)).unwrap_or(0.0) * crate::game::vehicle::info::MPH;
    if decel < 0.0 {
        return Err(ErrorKind::Custom("deceleration must be positive".into()));
    }
    if let Some(v) = host.vehicles.live.get_mut(&id) {
        v.jolts.push(Jolt {
            from: *from,
            force,
            speed_frac,
            decel,
        });
    }
    Ok(Value::Undefined)
}

/// The wheel names `getWheelSurface` takes, in `tag_wheel_*` order
/// (scr_const 0x88..0x92).
const WHEELS: [&str; 6] = [
    "front_left",
    "front_right",
    "back_left",
    "back_right",
    "middle_left",
    "middle_right",
];

/// `getWheelSurface(name)` (0x8e7c1): the surface type the wheel's last
/// suspension trace stored (`rec+0x158`), `"none"` for 0. A dedicated
/// server's `trap_SurfaceTypeToName` answers `"default"` for any other
/// (coduo-vehicles.md 17).
fn get_wheel_surface(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    // `Scr_GetConstString(0)` reads the name ahead of the kind test; a
    // number reads as its text and names no wheel.
    let name = match args.first() {
        Some(Value::String(s)) => Some(cx.resolve(*s).to_string()),
        Some(Value::Int(_) | Value::Float(_)) => None,
        _ => return Err(ErrorKind::BadType("getWheelSurface takes a wheel name")),
    };
    let v = &host.vehicles.live[&id];
    let info = host.vehicles.info_of(v);
    let four = info.kind == crate::game::vehicle::info::KIND_FOUR_WHEEL;
    if !four && info.kind != crate::game::vehicle::info::KIND_TANK {
        return Err(ErrorKind::Custom(format!(
            "Vehicle type [{}] has no wheels",
            info.name
        )));
    }
    let Some(i) = name.and_then(|n| WHEELS.iter().position(|w| *w == n)) else {
        return Err(ErrorKind::Custom(
            "Valid wheel names are: [front_left, front_right, back_left, back_right, middle_left, middle_right]"
                .into(),
        ));
    };
    if four && i > 3 {
        return Err(ErrorKind::Custom("Vehicle has no middle wheels".into()));
    }
    let surface = if v.phys.s.wheel_surface[i] == 0.0 {
        "none"
    } else {
        "default"
    };
    Ok(Value::String(cx.intern_exact(surface)))
}

/// `getSpeedMPH()` (0x8e97f): `speed` over 17.6.
fn get_speed_mph(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let speed = cx.intern_folded("speed");
    let s = match host.get_field(cx, id, speed) {
        Value::Float(f) => f,
        Value::Int(i) => i as f32,
        _ => 0.0,
    };
    Ok(Value::Float(s / crate::game::vehicle::info::MPH))
}

/// `getVehicleOwner()` (0x8e9cb): the driver, or nothing.
fn get_vehicle_owner(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let driver = host.vehicles.live[&id].seats[1];
    Ok(driver
        .and_then(|slot| host.ents.handle(slot as u32))
        .map_or(Value::Undefined, Value::Entity))
}

fn set_engine(host: &mut GameHost, recv: Option<Target>, on: bool) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    host.vehicles.live.get_mut(&id).unwrap().engine_on = on;
    Ok(Value::Undefined)
}

fn start_engine_sound(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_engine(host, recv, true)
}

fn stop_engine_sound(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_engine(host, recv, false)
}

/// `makeVehicleUsable()` (0x8ea21): contents and spawnflags gain the use bit.
fn make_vehicle_usable(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let v = host.vehicles.live.get_mut(&id).unwrap();
    v.contents |= crate::game::vehicle::CONTENTS_USABLE;
    Ok(Value::Undefined)
}

/// `makeVehicleUnusable()` (0x8ea63): refused while driven.
fn make_vehicle_unusable(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let v = host.vehicles.live.get_mut(&id).unwrap();
    if v.seats[1].is_some() {
        return Err(ErrorKind::Custom("Vehicle is already in use".into()));
    }
    v.contents &= !crate::game::vehicle::CONTENTS_USABLE;
    Ok(Value::Undefined)
}

fn set_compass(host: &mut GameHost, recv: Option<Target>, on: bool) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    host.vehicles.live.get_mut(&id).unwrap().on_compass = on;
    Ok(Value::Undefined)
}

fn add_vehicle_to_compass(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_compass(host, recv, true)
}

fn remove_vehicle_from_compass(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    set_compass(host, recv, false)
}

/// The turret methods' shared health check.
fn living_vehicle(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
) -> Result<EntId, ErrorKind> {
    let id = vehicle(host, recv)?;
    if health_of(host, cx, id) <= 0 {
        return Err(ErrorKind::Custom(
            "Vehicle must have health to control the turret".into(),
        ));
    }
    Ok(id)
}

pub(crate) fn health_of(host: &mut GameHost, cx: &mut Cx, id: EntId) -> i32 {
    let health = cx.intern_folded("health");
    match host.get_field(cx, id, health) {
        Value::Int(h) => h,
        Value::Float(f) => f as i32,
        _ => 0,
    }
}

fn set_turret_target_vec(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = living_vehicle(host, cx, recv)?;
    let Some(Value::Vector(p)) = args.first() else {
        return Err(ErrorKind::BadType("setTurretTargetVec takes a vector"));
    };
    host.vehicles.live.get_mut(&id).unwrap().target = Some(TurretTarget::Point(*p));
    Ok(Value::Undefined)
}

fn set_turret_target_ent(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = living_vehicle(host, cx, recv)?;
    let Some(Value::Entity(e)) = args.first() else {
        return Err(ErrorKind::BadType("setTurretTargetEnt takes an entity"));
    };
    let offset = match args.get(1) {
        Some(Value::Vector(o)) => *o,
        _ => [0.0; 3],
    };
    host.vehicles.live.get_mut(&id).unwrap().target = Some(TurretTarget::Entity(*e, offset));
    Ok(Value::Undefined)
}

fn clear_turret_target(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    host.vehicles.live.get_mut(&id).unwrap().target = None;
    Ok(Value::Undefined)
}

/// The three fire methods (`CMD_VEH_FireTurret` 0x8ee02,
/// `CMD_VEH_FireAltTurret` 0x8f57e, `G_VEH_FireGunner` 0x8fc7a): a shell
/// gun spawns its shell inside the call; a bullet gun queues the shot for
/// the server, which traces it against the bodies.
fn fire(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    gun: Gun,
) -> Result<Value, ErrorKind> {
    let id = living_vehicle(host, cx, recv)?;
    if crate::game::vehicle::fire::fires_shell(host, id, gun) {
        if let Some(shot) = crate::game::vehicle::fire::prepare(host, cx, id, gun) {
            crate::game::vehicle::fire::launch_shell(host, cx, &shot);
        }
    } else {
        host.vehicles.fire_ops.push((id, gun));
    }
    Ok(Value::Undefined)
}

fn fire_turret(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    fire(host, cx, recv, Gun::Main)
}

fn fire_alt_turret(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    fire(host, cx, recv, Gun::Alt)
}

fn fire_gunner(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    fire(host, cx, recv, Gun::Gunner)
}

/// `isTurretReady()` (0x901db) and `get_fire_time()` (0x90255): only on a
/// driven vehicle.
fn driven_timer(host: &GameHost, recv: Option<Target>) -> Result<i32, ErrorKind> {
    let id = vehicle(host, recv)?;
    let v = &host.vehicles.live[&id];
    if !v.driven {
        return Err(ErrorKind::Custom("Vehicle is not player driven".into()));
    }
    Ok(v.guns.timers[0])
}

fn is_turret_ready(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(i32::from(driven_timer(host, recv)? < 1)))
}

fn get_fire_time(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(driven_timer(host, recv)?))
}

fn guns(host: &GameHost, recv: Option<Target>) -> Result<crate::game::vehicle::Guns, ErrorKind> {
    let id = vehicle(host, recv)?;
    Ok(host.vehicles.live[&id].guns)
}

fn get_alt_heat(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(guns(host, recv)?.heat[0]))
}

fn get_gunner_heat(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(guns(host, recv)?.heat[1]))
}

fn get_alt_overheating(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(i32::from(guns(host, recv)?.overheating[0])))
}

fn get_gunner_overheating(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(i32::from(guns(host, recv)?.overheating[1])))
}

/// `getDismountSpot()` (0x903c6): the dismount search from the vehicle's
/// own origin; the origin when it finds nothing.
fn get_dismount_spot(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    let origin = crate::game::item::origin_of(host, cx, id);
    let spot = crate::game::vehicle::dismount_spot(
        host,
        id,
        origin,
        ([-15.0, -15.0, 15.0], [15.0, 15.0, 70.0]),
        None,
        None,
    );
    Ok(Value::Vector(spot.unwrap_or(origin)))
}

/// `spawnDuplicate()` (0x6e2c4): the spawn re-run on the backed-up spawn
/// vars, a fresh entity, its collmap paired at once.
fn spawn_duplicate(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(vars) = host.vehicles.live.get(&id).map(|v| v.spawn_vars.clone()) else {
        return Err(ErrorKind::Custom(
            "spawnduplicate: entity has no spawn vars".into(),
        ));
    };
    let dup = crate::game::spawn::spawn_from_vars(host, cx, &vars)?;
    Ok(Value::Entity(dup))
}

/// `unlinkFromWorld()` (0x69286) and `linkIntoWorld()` (0x692b7):
/// `trap_UnlinkEntity`, `trap_LinkEntity`.
fn unlink_from_world(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    crate::game::vehicle::set_linked(host, cx, id, false);
    Ok(Value::Undefined)
}

fn link_into_world(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    crate::game::vehicle::set_linked(host, cx, id, true);
    Ok(Value::Undefined)
}

/// `verifyPosition([reset])` (0x692e8): on a vehicle, nothing but itself in
/// its box under 0x2800000 (a player or another vehicle).
fn verify_position(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let clear = crate::game::vehicle::position_clear(host, cx, id);
    Ok(Value::Int(i32::from(clear)))
}

/// `clearVehiclePosition()` (0x6950e): whatever stands in the vehicle's box
/// (mask 0x4000100: corpses and dropped items) is deleted as `delete()`
/// does it: notified `death` with itself, unlinked and freed 100 ms on (a
/// dead player's corpse link is only notified).
fn clear_vehicle_position(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    for e in crate::game::vehicle::clutter_in_box(host, cx, id) {
        crate::game::builtins::entity::delete(host, cx, Some(Target::Entity(e)), &[])?;
    }
    Ok(Value::Undefined)
}

/// `GScr_GetVehicleNodeIndex` (0x9836c): a node is a class-2 object, and
/// only `getVehicleNode`, `getVehicleNodeArray` and `getAllVehicleNodes`
/// (0x984d2..0x98726) make one. UO MP registers none of the three, so an
/// entity is never a node and anything else is not an entity
/// (coduo-vehicles.md 17).
fn vehicle_node(args: &[Value], i: usize) -> ErrorKind {
    match args.get(i) {
        Some(Value::Entity(_)) => ErrorKind::Custom("Not a vehicle node".into()),
        _ => ErrorKind::BadType("not an entity"),
    }
}

/// `ent+0x17e == 2` (`CMD_VEH_*`'s shared test): a player has driven it.
fn path_usable(host: &GameHost, id: EntId) -> Result<(), ErrorKind> {
    if host.vehicles.live[&id].driven {
        return Err(ErrorKind::Custom(
            "Vehicle is invalid on path after it's been used".into(),
        ));
    }
    Ok(())
}

fn mph_arg(args: &[Value], i: usize) -> Result<f32, ErrorKind> {
    number(args.get(i))
        .map(|x| x * crate::game::vehicle::info::MPH)
        .ok_or(ErrorKind::BadType("vehicle speeds take a number"))
}

/// `attachPath(node)` (0x8dfe9): the node is read ahead of the use test.
fn attach_path(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    vehicle(host, recv)?;
    Err(vehicle_node(args, 0))
}

/// `startPath()` (0x8e21c): no node was ever attached.
fn start_path(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    path_usable(host, id)?;
    Err(ErrorKind::Custom(
        "Can't start path on a vehicle that hasn't been attached".into(),
    ))
}

/// `setSwitchNode(from, to)` (0x8e299): both nodes ahead of the use test.
fn set_switch_node(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    vehicle(host, recv)?;
    Err(vehicle_node(args, 0))
}

/// `setWaitNode(node)` (0x8e328): the use test, then the node.
fn set_wait_node(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    path_usable(host, id)?;
    Err(vehicle_node(args, 0))
}

/// `setWaitSpeed(mph)` (0x8e394): stores `rec+0x23c`, which only the path
/// think (0x88561) reads.
fn set_wait_speed(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    path_usable(host, id)?;
    if mph_arg(args, 0)? < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot have a negative wait speed on a vehicle".into(),
        ));
    }
    Ok(Value::Undefined)
}

/// `setSpeed(mph, accel)` (0x8e435): stores `rec+0x274..0x27c` for the
/// path think alone, both read ahead of either test.
fn set_speed(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    path_usable(host, id)?;
    let (speed, accel) = (mph_arg(args, 0)?, mph_arg(args, 1)?);
    if speed < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot set negative speed on vehicle".into(),
        ));
    }
    if accel < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot set negative acceleration on vehicle".into(),
        ));
    }
    Ok(Value::Undefined)
}

/// `resumeSpeed(accel)` (0x8e52e): as `setSpeed`'s second half.
fn resume_speed(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    path_usable(host, id)?;
    if mph_arg(args, 0)? < 0.0 {
        return Err(ErrorKind::Custom(
            "Cannot set negative acceleration on vehicle".into(),
        ));
    }
    Ok(Value::Undefined)
}

/// `freeVehicle()` (0x8e72f): `G_FreeVehicle`, then the entity stays as
/// `eType` 13 `script_vehicle_corpse`, notified `death` and handed a new
/// script object (coduo-vehicles.md 17).
fn free_vehicle(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = vehicle(host, recv)?;
    crate::game::vehicle::free_to_corpse(host, cx, id);
    Ok(Value::Undefined)
}
