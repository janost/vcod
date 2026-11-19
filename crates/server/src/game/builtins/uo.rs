//! The builtins United Offensive's stock MP scripts call that 1.1's do not:
//! UO's own additions and the 1.1 ones no 1.1 gametype reached
//! (docs/research/coduo-diff.md 13 and 20). Addresses are
//! `game.mp.uo.i386.so`'s. The vehicle methods are `builtins::vehicle`'s.

use super::client::client_receiver;
use super::entity::entity_receiver;
use crate::game::host::{GameHost, SimOp};
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Target, Value};

use super::Builtin;

pub const NAMES: &[(&str, Builtin)] = &[
    ("getfullclipammo", get_full_clip_ammo),
    ("isvalidplayer", is_valid_player),
    ("distancesquared", distance_squared),
    ("iprintlnbold", iprint_line_bold),
    ("getweaponslotammo", get_weapon_slot_ammo),
    ("getweaponslotclipammo", get_weapon_slot_clip_ammo),
    ("attackbuttonpressed", attack_button_pressed),
    ("isinvehicle", is_in_vehicle),
    ("viewkick", view_kick),
    ("settakedamage", set_take_damage),
    ("setbounds", set_bounds),
    ("setcontents", set_contents),
    ("playlocalannouncersound", play_local_announcer_sound),
    ("rewindfx", rewind_fx),
    ("getstance", get_stance),
    ("print3d", print_3d),
    ("dodamage", do_damage),
    ("dodamagemod", do_damage_mod),
    ("spawntriggermount", spawn_trigger_mount),
];

/// `getFullClipAmmo(weapon)` (0x6782e): the weapon file's `clipSize`.
pub fn get_full_clip_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("getFullClipAmmo takes a weapon name"));
    };
    let size = host
        .defs
        .weapon_index(cx.resolve(*name))
        .and_then(|i| host.weapons.get(i))
        .map_or(0, |d| d.clip_size as i32);
    Ok(Value::Int(size))
}

/// `spawnTriggerMount(mins, maxs, crouch)` (0x67974): a `trigger_mount`
/// over the absolute box the two corners span, each axis sorted, at origin 0,
/// linked by `SP_trigger_mount_no_brush` (0x77754) with contents 0x4000, or
/// 0x400000 and spawnflag 1 when `crouch` is true. An LMG deploys on it
/// (docs/research/coduo-movement.md 11).
pub fn spawn_trigger_mount(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Vector(a)), Some(Value::Vector(b)), Some(crouch)) =
        (args.first(), args.get(1), args.get(2))
    else {
        return Err(ErrorKind::BadType(
            "spawnTriggerMount takes two vectors and a boolean",
        ));
    };
    let crouch = crouch.as_bool()?;
    let mins = std::array::from_fn(|i| a[i].min(b[i]));
    let maxs = std::array::from_fn(|i| a[i].max(b[i]));
    let id = host.ents.spawn(cx)?;
    let classname = cx.intern_folded("classname");
    let name = Value::String(cx.intern_exact("trigger_mount"));
    host.set_field(cx, id, classname, name)?;
    if crouch {
        let spawnflags = cx.intern_folded("spawnflags");
        host.set_field(cx, id, spawnflags, Value::Int(1))?;
    }
    host.triggers.register(
        id,
        crate::game::trigger::TriggerKind::Mount { crouch },
        crate::game::trigger::TriggerShape::boxed(mins, maxs),
        0,
    );
    host.link_trigger(cx, id);
    Ok(Value::Entity(id))
}

/// `isValidPlayer(ent)` (0x67176): 1 for an entity in use with a client
/// whose connection state reads 2, anything else 0. vcod's client entity
/// lives from `ClientConnect`, so a client still loading reads 1 here
/// (INFERRED: the state at `client+0x4520` is `CON_CONNECTED`).
pub fn is_valid_player(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let valid = match args.first() {
        Some(Value::Entity(id)) => host.ents.get(*id).is_some_and(|e| e.client.is_some()),
        _ => false,
    };
    Ok(Value::Int(i32::from(valid)))
}

/// `distanceSquared(a, b)` (0x6b83d), `VectorDistanceSquared` as a float.
pub fn distance_squared(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Vector(a)), Some(Value::Vector(b))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType("distanceSquared takes two vectors"));
    };
    let d: f32 = (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum();
    Ok(Value::Float(d))
}

/// `iPrintLnBold(...)`, the function (0x66e86) to everyone and the player
/// method (0x489fd) to one: `Scr_MakeGameMessage` with `g`, the bold
/// centre print, where `iPrintLn` sends `f`.
pub fn iprint_line_bold(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let text = super::message::construct(host, cx, args);
    let cmd = format!("g \"{}\"", text.replace('"', "'"));
    match recv {
        Some(_) => {
            let slot = client_receiver(host, recv)?;
            host.client_commands.push((slot, cmd));
        }
        None => {
            for slot in host.client_slots() {
                host.client_commands.push((slot, cmd.clone()));
            }
        }
    }
    Ok(Value::Undefined)
}

/// The weapon in a named slot, 0 for none (`getWeaponSlotAmmo`'s and
/// `getWeaponSlotClipAmmo`'s shared head).
fn slot_weapon(
    host: &GameHost,
    cx: &Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<(usize, usize), ErrorKind> {
    let client = client_receiver(host, recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("that builtin takes a weapon slot name"));
    };
    let slot = crate::weapons::SLOT_NAMES
        .iter()
        .position(|s| *s == cx.resolve(*name))
        .filter(|i| *i > 0)
        .ok_or_else(|| {
            ErrorKind::Custom(format!(
                "Unknown weaponslot name {}. Valid weaponslots are \"primary\", \"primaryb\", \"pistol\", \"grenade\", and \"smokegrenade\"",
                cx.resolve(*name)
            ))
        })?;
    Ok((client, host.client_weapons[client].slots[slot] as usize))
}

/// `self getWeaponSlotAmmo(slot)` (0x4838a): the reserve of the slot's
/// weapon, or its clip for a clip-only one; 0 for an empty slot.
pub fn get_weapon_slot_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (client, weapon) = slot_weapon(host, cx, recv, args)?;
    let Some(def) = host.weapons.get(weapon) else {
        return Ok(Value::Int(0));
    };
    let a = &host.client_ammo[client];
    let rounds = if def.clip_only {
        a.clip[def.clip_index]
    } else {
        a.ammo[def.ammo_index]
    };
    Ok(Value::Int(i32::from(rounds)))
}

/// `self getWeaponSlotClipAmmo(slot)` (0x486f3): the slot's weapon's clip.
pub fn get_weapon_slot_clip_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (client, weapon) = slot_weapon(host, cx, recv, args)?;
    let Some(def) = host.weapons.get(weapon) else {
        return Ok(Value::Int(0));
    };
    Ok(Value::Int(i32::from(
        host.client_ammo[client].clip[def.clip_index],
    )))
}

/// `self attackButtonPressed()`: the attack bit of the client's last cmd,
/// as `useButtonPressed` reads the use bit.
pub fn attack_button_pressed(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let held = host.client_buttons[slot] & vcod_common::net::msg::BUTTON_ATTACK != 0;
    Ok(Value::Int(i32::from(held)))
}

/// `self isInVehicle()` (`PlayerCmd_isInVehicle` 0x46ca0): 1 in a seat.
/// Retail reads the entity's copy of `eFlags`, which an entry sets only at
/// the frame's end; the stock scripts wait 0.1 s before asking. A rider
/// put out where he sat reads 1 until his end frame, one a dismount moved
/// 0 at once (coduo-vehicles.md 11.2).
pub fn is_in_vehicle(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let seated =
        host.vehicles.seat_of(slot).is_some() || host.vehicles.stale_seated.contains(&slot);
    Ok(Value::Int(i32::from(seated)))
}

/// `self viewKick(force, origin)` (0x6db55): what a hit leaves for
/// `P_DamageFeedback`, without the hit. The client's damage this frame is
/// `(force * maxHealth + 50) / 100` and its direction the player's origin
/// less `origin`, so the end frame turns it into `damageCount` `force`, the
/// view kick and the pain event, as a bullet's would (`_shellshock_gmi`).
pub fn view_kick(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (Some(force), Some(Value::Vector(from))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::Custom(
            "USAGE: <player> viewkick <force 0-127> <source position>\n".into(),
        ));
    };
    let force = match force {
        Value::Int(i) => *i,
        Value::Float(f) => *f as i32,
        _ => return Err(ErrorKind::BadType("viewkick takes a force")),
    };
    if force < 0 {
        return Err(ErrorKind::Custom(format!("viewkick: damage {force} < 0\n")));
    }
    host.client_sim_ops
        .push((slot, SimOp::ViewKick { force, from: *from }));
    Ok(Value::Undefined)
}

/// `setTakeDamage(bool)` (0x691d2): `ent->takedamage` (+0x17d). A script
/// entity with it set takes `G_Damage` from blasts and `doDamage`
/// (`GameHost::damage_nonclient`); a vehicle's is its record's
/// (`Vehicle::takedamage`); a client's lives on the sim and is not moved
/// here.
pub fn set_take_damage(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let [v] = args else {
        return Err(ErrorKind::Custom("SetTakeDamage true/false\n".into()));
    };
    if let Some(veh) = host.vehicles.live.get_mut(&id) {
        veh.takedamage = v.as_bool()?;
    } else if host.ents.get(id).is_some_and(|e| e.client.is_none()) {
        if v.as_bool()? {
            host.script_takedamage.insert(id);
        } else {
            host.script_takedamage.remove(&id);
        }
    }
    Ok(Value::Undefined)
}

/// `self doDamage(damage, source[, attacker[, inflictor[, mod]]])`
/// (0x68e67): `G_Damage` from `source`, the direction from it to the
/// entity, `attacker` the world when absent, `inflictor` the attacker, `mod`
/// `MOD_UNKNOWN`. A vehicle or a player goes through
/// `vehicle::g_damage`; any other entity takes damage only where
/// `setTakeDamage` opened it or it is a turret.
pub fn do_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    if args.len() < 2 {
        return Err(ErrorKind::Custom(
            "DoDamage damageAmount sourceVec [attacker] [inflictor] [mod]\n".into(),
        ));
    }
    let entity = |v: Option<&Value>| match v {
        Some(Value::Entity(e)) => Ok(Some(*e)),
        None => Ok(None),
        Some(_) => Err(ErrorKind::BadType("doDamage takes entities")),
    };
    let attacker = entity(args.get(2))?;
    let inflictor = entity(args.get(3))?.or(attacker);
    let mod_ = match args.get(4) {
        Some(Value::String(a)) => Some(cx.resolve(*a).to_string()),
        Some(_) => return Err(ErrorKind::BadType("doDamage takes a means of death")),
        None => None,
    };
    entity_damage(host, cx, recv, &args[..2], attacker, inflictor, mod_)
}

/// `self doDamageMod(damage, source, mod)` (0x69042): `doDamage` with no
/// attacker or inflictor.
pub fn do_damage_mod(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let [_, _, Value::String(m), ..] = args else {
        return Err(ErrorKind::Custom(
            "DoDamageMod damageAmount sourceVec mod\n".into(),
        ));
    };
    let mod_ = Some(cx.resolve(*m).to_string());
    entity_damage(host, cx, recv, &args[..2], None, None, mod_)
}

/// The shared tail: `G_IndexForMeansOfDeath` falls back to `MOD_UNKNOWN`,
/// and the world stands in for a missing attacker or inflictor.
fn entity_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    attacker: Option<EntId>,
    inflictor: Option<EntId>,
    mod_: Option<String>,
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let damage = match args[0] {
        Value::Int(i) => i,
        Value::Float(f) => f as i32,
        _ => return Err(ErrorKind::BadType("doDamage takes a damage")),
    };
    let Value::Vector(source) = args[1] else {
        return Err(ErrorKind::BadType("doDamage takes a source vector"));
    };
    let mod_ = mod_
        .filter(|m| host.defs.mod_names.contains(&m.as_str()))
        .unwrap_or_else(|| "MOD_UNKNOWN".to_string());
    if host.vehicles.live.contains_key(&id) || host.ents.get(id).is_some_and(|e| e.client.is_some())
    {
        crate::game::vehicle::g_damage(host, cx, id, inflictor, attacker, source, damage, 0, &mod_);
        return Ok(Value::Undefined);
    }
    if host.ents.get(id).is_none_or(|e| e.client.is_some())
        || !(host.script_takedamage.contains(&id) || host.turrets.contains_key(&id))
    {
        return Ok(Value::Undefined);
    }
    let origin = cx.intern_folded("origin");
    let Value::Vector(at) = host.get_field(cx, id, origin) else {
        return Ok(Value::Undefined);
    };
    let world = host.ents.world(cx);
    let attacker = attacker.unwrap_or(world);
    let hit = crate::game::host::EntityHit {
        inflictor: inflictor.unwrap_or(world),
        mod_,
        dir: [at[0] - source[0], at[1] - source[1], at[2] - source[2]],
        point: source,
    };
    for (event, args) in host.damage_nonclient(cx, id, damage, attacker, &hit) {
        let event = cx.intern_folded(event);
        cx.notify(Target::Entity(id), event, args);
    }
    Ok(Value::Undefined)
}

/// `setBounds(mins, maxs)` (0x69716): `r.mins` (+0x108) and `r.maxs`
/// (+0x114), then the link (0x6976c), so the walks test the new world box;
/// a `script_model` stays filed under its model's bounds. An entity with no
/// link shape keeps none.
pub fn set_bounds(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let (Some(Value::Vector(mins)), Some(Value::Vector(maxs))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType("setBounds takes two vectors"));
    };
    if let Some(shape) = host.ents.get_mut(id).and_then(|e| e.link.as_mut()) {
        shape.mins = *mins;
        shape.maxs = *maxs;
    }
    host.link_entity(cx, id);
    Ok(Value::Undefined)
}

/// `setContents(int)` (0x696b9; 1.1d 0x5dcec, its link 0x5dd4a): `r.contents`
/// set and the entity relinked, returning the old contents. vcod relinks
/// through the link shape, which an entity with none (a model never linked)
/// lacks; the new contents then stay unread, and the old read 0.
pub fn set_contents(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::Int(contents)) = args.first() else {
        return Err(ErrorKind::BadType("setContents takes an int"));
    };
    let old = host
        .ents
        .get(id)
        .and_then(|e| e.link.as_ref())
        .map_or(0, |l| l.contents);
    host.set_link_contents(id, *contents);
    host.link_entity(cx, id);
    Ok(Value::Int(old))
}

/// `self playLocalAnnouncerSound(alias)` (0x494d8): `S <idx>` to that
/// player alone, the alias indexed as `playLocalSound`'s `s` is; dom's
/// capture calls.
pub fn play_local_announcer_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let idx = super::sound::alloc_alias(host, cx, args)? - super::sound::sounds_base(host);
    host.client_commands.push((slot, format!("S {idx}")));
    Ok(Value::Undefined)
}

/// `rewindFx(ent, seconds)` (0x6c620): `F <ms>` to the entity's client, the
/// time an int times 1000; `_killcam_gmi` rewinds the viewer's effects by
/// its `archiveTime`.
pub fn rewind_fx(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let [Value::Entity(id), t] = args else {
        return Err(ErrorKind::Custom("USAGE: rewindfx <entity> <time>".into()));
    };
    let secs = match t {
        Value::Int(i) => *i,
        Value::Float(f) => *f as i32,
        _ => return Err(ErrorKind::BadType("rewindfx takes a time")),
    };
    let slot = client_receiver(host, Some(Target::Entity(*id)))?;
    host.client_commands
        .push((slot, format!("F {}", secs.wrapping_mul(1000))));
    Ok(Value::Undefined)
}

/// `self getStance()` (0x68590): `"prone"` under `PMF_PRONE`, else
/// `"crouch"` under `PMF_DUCKED`, else `"stand"`, off the playerstate's
/// `pm_flags` (vcod's mirrored stance); a non-player is the error
/// `GetStance is only defined for players.`
pub fn get_stance(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    use vcod_common::pmove::Stance;
    let slot = client_receiver(host, recv)
        .map_err(|_| ErrorKind::Custom("GetStance is only defined for players.".into()))?;
    let stance = match host.client_stance[slot] {
        Stance::Prone => "prone",
        Stance::Crouch => "crouch",
        Stance::Stand => "stand",
    };
    Ok(Value::String(cx.intern_exact(stance)))
}

/// `print3d(origin, text[, colour[, alpha[, scale]]])` (0x66eb4): a debug
/// string for a local client's renderer (`trap_AddDebugString`), which a
/// dedicated server never draws.
pub fn print_3d(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `setBounds` and `setContents` each link (0x6976c, 0x6970b): the
    /// tree's world box takes the new bounds at once, and contents 0 takes
    /// the entity out.
    #[test]
    fn set_bounds_and_set_contents_relink() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, e, origin, Value::Vector([0.0; 3]))
                .unwrap();
            crate::game::spawn::link_script_model(&mut host, cx, e);
            assert_eq!(host.area.last_link(e.0).unwrap().absmax, [1.0; 3]);
            let recv = Some(Target::Entity(e));
            let bounds = [
                Value::Vector([-6.0, -6.0, 0.0]),
                Value::Vector([6.0, 6.0, 128.0]),
            ];
            set_bounds(&mut host, cx, recv, &bounds).unwrap();
            let link = host.area.last_link(e.0).unwrap();
            assert_eq!(link.absmin, [-7.0, -7.0, -1.0]);
            assert_eq!(link.absmax, [7.0, 7.0, 129.0]);
            assert_eq!(
                host.area
                    .entities_in_box([0.0, 0.0, 100.0], [0.0, 0.0, 100.0], 0x2080),
                [e.0]
            );
            let old = set_contents(&mut host, cx, recv, &[Value::Int(0)]).unwrap();
            assert_eq!(old, Value::Int(0x2080));
            assert!(host.area.last_link(e.0).is_none());
        });
    }

    /// A script model `setTakeDamage` opened takes `doDamage` as UO's `eType`
    /// 8 arm does: the six-argument notify, its `health` untouched.
    #[test]
    fn dodamage_notifies_a_script_model_with_six_arguments() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let classname = cx.intern_folded("classname");
            let model = Value::String(cx.intern_exact("script_model"));
            host.set_field(cx, e, classname, model).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, e, origin, Value::Vector([0.0, 0.0, 64.0]))
                .unwrap();
            let t = Some(Target::Entity(e));
            let hit = |cx: &mut Cx| {
                [
                    Value::Int(500),
                    Value::Vector([0.0, 0.0, 0.0]),
                    Value::Undefined,
                    Value::Undefined,
                    Value::String(cx.intern_exact("MOD_GRENADE_SPLASH")),
                ]
            };
            // Without takedamage nothing happens.
            let args = hit(cx);
            let args = [args[0], args[1]];
            do_damage(&mut host, cx, t, &args).unwrap();
            assert!(host.script_takedamage.is_empty());
            set_take_damage(&mut host, cx, t, &[Value::Int(1)]).unwrap();
            assert!(host.script_takedamage.contains(&e));
            let world = host.ents.world(cx);
            let hit = crate::game::host::EntityHit {
                inflictor: world,
                mod_: "MOD_GRENADE_SPLASH".into(),
                dir: [0.0, 0.0, 64.0],
                point: [0.0; 3],
            };
            let out = host.damage_nonclient(cx, e, 500, world, &hit);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].0, "damage");
            let a = &out[0].1;
            assert_eq!(a.len(), 6);
            assert_eq!(a[0], Value::Int(500));
            assert_eq!(a[2], Value::Vector([0.0, 0.0, 1.0]));
            assert!(matches!(a[4], Value::String(m) if cx.resolve(m) == "MOD_GRENADE_SPLASH"));
            let health = cx.intern_folded("health");
            assert_ne!(host.get_field(cx, e, health), Value::Int(-500));
        });
    }

    /// `setTakeDamage` on a vehicle writes its record's `takedamage`: off,
    /// `G_Damage` and the blast walk pass it by; on, the blast lists it
    /// once, as a vehicle (coduo-vehicles.md 13).
    #[test]
    fn set_take_damage_gates_a_vehicle() {
        use crate::game::vehicle::{Vehicle, info::VehicleInfo, vehicle_damage};
        let mut host = GameHost::new(&crate::game::profile::COD1_UO, vec![String::new(); 2048]);
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let health = cx.intern_folded("health");
            host.set_field(cx, e, health, Value::Int(100)).unwrap();
            host.vehicles
                .infos
                .list
                .push(std::rc::Rc::new(VehicleInfo::default()));
            host.vehicles.slots[0] = Some(e);
            host.vehicles.live.insert(e, Vehicle::for_test(0));
            let recv = Some(Target::Entity(e));
            let hit = |host: &mut GameHost, cx: &mut Cx| {
                vehicle_damage(
                    host,
                    cx,
                    e,
                    None,
                    None,
                    [0.0, 0.0, 1.0],
                    [0.0; 3],
                    50,
                    0,
                    "MOD_EXPLOSIVE",
                )
            };
            let listed = |host: &mut GameHost, cx: &mut Cx| {
                host.blast_entities(cx).iter().filter(|v| v.id == e).count()
            };
            set_take_damage(&mut host, cx, recv, &[Value::Int(0)]).unwrap();
            assert!(hit(&mut host, cx).is_empty());
            assert_eq!(host.get_field(cx, e, health), Value::Int(100));
            assert_eq!(listed(&mut host, cx), 0);
            set_take_damage(&mut host, cx, recv, &[Value::Int(1)]).unwrap();
            assert!(host.script_takedamage.is_empty());
            assert_eq!(listed(&mut host, cx), 1);
            assert_eq!(hit(&mut host, cx)[0].0, "damage");
            assert!(matches!(host.get_field(cx, e, health), Value::Int(h) if h < 100));
        });
    }

    /// The weapon slot builtins take the slot names but `none` (slot 0);
    /// an empty slot's weapon reads 0 rounds.
    #[test]
    fn a_weapon_slot_builtin_refuses_none_and_unknown_names() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 1, None).unwrap();
            let t = Some(Target::Entity(e));
            let ask = |host: &mut GameHost, cx: &mut Cx, name: &str| {
                let n = Value::String(cx.intern_exact(name));
                get_weapon_slot_ammo(host, cx, t, &[n])
            };
            assert_eq!(ask(&mut host, cx, "primaryb"), Ok(Value::Int(0)));
            assert_eq!(ask(&mut host, cx, "smokegrenade"), Ok(Value::Int(0)));
            for bad in ["none", "Primary", "rifle"] {
                assert!(
                    matches!(ask(&mut host, cx, bad), Err(ErrorKind::Custom(m)) if m.starts_with("Unknown weaponslot name")),
                    "{bad}"
                );
            }
            assert!(get_weapon_slot_ammo(&mut host, cx, t, &[Value::Int(1)]).is_err());
            let not_a_player = Some(Target::Entity(host.ents.spawn(cx).unwrap()));
            let n = Value::String(cx.intern_exact("primary"));
            assert!(get_weapon_slot_ammo(&mut host, cx, not_a_player, &[n]).is_err());
        });
    }

    /// `doDamage` with no attacker names the world as both attacker and
    /// inflictor; with an attacker alone the attacker is the inflictor too
    /// (coduo-diff.md 22). Read off the `damage` notify's second and sixth
    /// arguments.
    #[test]
    fn dodamage_defaults_its_attacker_and_inflictor() {
        let src = "hit(e, by) {\n\
                       e thread watch();\n\
                       if (isDefined(by)) e doDamage(5, (0, 0, 0), by);\n\
                       else e doDamage(5, (0, 0, 0));\n\
                   }\n\
                   watch() {\n\
                       self waittill(\"damage\", d, a, dir, p, m, i);\n\
                       self.seen_a = a getEntityNumber();\n\
                       self.seen_i = i getEntityNumber();\n\
                   }\n";
        let mut vm = vcod_gsc::Vm::new();
        let ast = vcod_gsc::parse::parse_file(src).unwrap();
        let fns = vcod_gsc::compile::compile_file(&ast, "maps/mp/t", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = GameHost::new(&crate::game::profile::COD1_UO, vec![String::new(); 2048]);
        let (e, by) = vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let classname = cx.intern_folded("classname");
            let model = Value::String(cx.intern_exact("script_model"));
            host.set_field(cx, e, classname, model).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, e, origin, Value::Vector([0.0, 0.0, 64.0]))
                .unwrap();
            host.script_takedamage.insert(e);
            (e, host.ents.spawn(cx).unwrap())
        });
        let hit = vm.func_ref("maps/mp/t", "hit");
        let seen = |vm: &mut vcod_gsc::Vm, host: &mut GameHost| {
            vm.with_cx(|cx| {
                let a = cx.intern_folded("seen_a");
                let i = cx.intern_folded("seen_i");
                (host.get_field(cx, e, a), host.get_field(cx, e, i))
            })
        };
        vm.start_thread(
            &mut host,
            hit,
            None,
            vec![Value::Entity(e), Value::Undefined],
        );
        vm.run_frame(&mut host, 50);
        let world = vm.with_cx(|cx| host.ents.world(cx)).0 as i32;
        assert_eq!(
            seen(&mut vm, &mut host),
            (Value::Int(world), Value::Int(world))
        );
        vm.start_thread(
            &mut host,
            hit,
            None,
            vec![Value::Entity(e), Value::Entity(by)],
        );
        vm.run_frame(&mut host, 100);
        let by = by.0 as i32;
        assert_eq!(seen(&mut vm, &mut host), (Value::Int(by), Value::Int(by)));
        assert!(vm.aborts().is_empty(), "{:?}", vm.aborts());
    }

    #[test]
    fn getstance_reads_the_mirrored_stance() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 2, None).unwrap();
            host.client_stance[2] = vcod_common::pmove::Stance::Prone;
            let v = get_stance(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
            assert!(matches!(v, Value::String(a) if cx.resolve(a) == "prone"));
            let other = host.ents.spawn(cx).unwrap();
            assert!(get_stance(&mut host, cx, Some(Target::Entity(other)), &[]).is_err());
        });
    }
}
