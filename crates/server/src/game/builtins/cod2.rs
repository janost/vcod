//! Builtins only CoD2's set answers (`BuiltinSet::cod2`): names CoD2 adds,
//! and CoD1 names whose CoD2 behaviour vcod has measured where CoD1's is
//! not. Addresses are cod2_lnxded's (cod2-gsc-diff.md section 1).

use crate::game::host::{GameHost, WeaponOp};
use vcod_gsc::{Cx, EntId, ErrorKind, Target, Value};

use super::client::client_receiver;
use super::precache::{before_any_wait, name_arg};
use super::score::Builtin;

pub const NAMES: &[(&str, Builtin)] = &[
    ("assert", developer_statement),
    ("assertex", developer_statement),
    ("assertmsg", developer_statement),
    ("issplitscreen", is_split_screen),
    ("istalking", is_talking),
    ("precacheitem", precache_item),
    ("precacheturret", precache_turret),
    ("spawnturret", spawn_turret),
    ("precacherumble", empty),
    ("playrumbleonpos", empty),
    ("playlooprumbleonpos", empty),
    ("stopallrumbles", empty),
    ("playrumble", empty),
    ("playlooprumble", empty),
    ("stoprumble", empty),
    ("matchend", empty),
    ("setplayerteamrank", empty),
    ("sendranks", empty),
    ("endparty", empty),
    ("print", developer_statement),
    ("line", developer_statement),
    ("bullettracepassed", bullet_trace_passed),
    ("closeingamemenu", close_in_game_menu),
    ("cloneplayer", clone_player),
    ("finishplayerdamage", finish_player_damage),
    ("obituary", obituary),
    ("dropitem", drop_item),
    ("getammocount", get_ammo_count),
    ("getcurrentoffhand", get_current_offhand),
    ("getcurrentweapon", get_current_weapon),
    ("giveweapon", give_weapon),
    ("getplayerangles", get_player_angles),
    ("int", cast_int),
    ("radiusdamage", radius_damage),
    ("randomint", random_int),
    ("randomfloat", random_float),
    ("randomintrange", random_int_range),
    ("randomfloatrange", random_float_range),
    ("setnormalhealth", set_normal_health),
    ("setstablemissile", set_stable_missile),
    ("setteamscore", set_team_score),
    ("setwaypoint", set_waypoint),
    ("setweaponclipammo", set_weapon_clip_ammo),
    ("switchtooffhand", switch_to_offhand),
    ("updatescores", update_scores),
    ("vectordot", vector_dot),
];

/// The console builtins whose PC handlers are empty, VERIFIED:
/// `Scr_PrecacheRumble` 0x8114a36, `Scr_PlayRumbleOnPos` 0x8117906,
/// `Scr_PlayLoopRumbleOnPos` 0x811790c, `Scr_StopAllRumbles` 0x8117912,
/// `ScrCmd_PlayRumble` 0x8111e90, `ScrCmd_PlayLoopRumble` 0x8111e96,
/// `ScrCmd_StopRumble` 0x8111e9c, `GScr_MatchEnd` 0x811792c,
/// `GScr_SetPlayerTeamRank` 0x8117932, `GScr_SendXboxLiveRanks` 0x8117938,
/// `GScr_EndXboxLiveLobby` 0x811793e.
fn empty(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `assert`, `assertEx`, `assertMsg`, `print` and `line`: `developer` 1
/// records, which retail compiles as if inside `/# #/` when used as a
/// statement, so below `developer_script 1` they never run
/// (cod2-gsc-diff.md 1.1, 3.3, probe `probe_c2g`; `GScr_line` 0x8110c02
/// is empty anyway). vcod still evaluates the arguments.
fn developer_statement(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `isSplitScreen()` (`Scr_IsSplitscreen` 0x8117918): pushes 0, VERIFIED.
fn is_split_screen(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(0))
}

/// `self isTalking()` (`PlayerCmd_IsTalking` 0x80fd15a): a voice frame
/// relayed within `g_voiceChatTalkingDuration` (500 ms) of `level.time`, a
/// negative age not (cod2-voice.md 3).
fn is_talking(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let window = crate::game::builtins::cvar::atoi(host.cvars.get("g_voiceChatTalkingDuration"));
    let now = host.level_time_ms;
    let on = host.cod2().clients[slot]
        .last_voice_time
        .is_some_and(|t| (0..window).contains(&(now - t)));
    Ok(Value::Int(i32::from(on)))
}

/// CoD2's weapon index for `name`, registering it on first use: CoD2 has no
/// fixed weapon list, `BG_FindWeaponIndexForName` misses and 0x80ebc24
/// loads `weapons/mp/<name>` and appends it (0x80eb8bc), so the index is
/// the order the level first named each weapon. VERIFIED, read off
/// `G_GetWeaponIndexForName` 0x8120dd0 and 0x80ebc24; that the directory is
/// `mp` is INFERRED (the `weapons/%s/%s` format at 0x810f518). Past
/// `level.initializing` 0x8120dd0 only looks the name up, so a weapon the
/// load never named stays unregistered (cod2-protocol.md 15.3). `None` for
/// `none`, an empty name, or a weapon with no file.
pub fn register_weapon(host: &mut GameHost, name: &str) -> Option<usize> {
    if name.is_empty() || name.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(i) = host
        .cod2()
        .weapons
        .iter()
        .position(|w| w.eq_ignore_ascii_case(name))
    {
        return Some(i + 1);
    }
    if !host.initializing {
        return None;
    }
    let fs = host.fs.clone()?;
    let mut def = vcod_common::weapon::load(&*fs, name).ok()?;
    let text = fs.read(&format!("weapons/mp/{name}")).unwrap_or_default();
    let keys = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&text));
    // 1.1's loader floors `clipSize` at 1; CoD2's binoculars keep their 0,
    // so the shared clip slot 0 stays empty on the wire (cod2-protocol.md
    // 15.6).
    if keys.get("clipSize").is_some_and(|v| v.trim() == "0") {
        def.clip_size = 0;
    }
    host.cod2_mut().offhands.push(
        keys.get("offhandClass")
            .is_some_and(|c| !c.is_empty() && !c.eq_ignore_ascii_case("none")),
    );
    host.cod2_mut().weapons.push(name.to_string());
    // The table keeps every def in index order; a level names a few dozen.
    let mut defs: Vec<_> = host.weapons.defs().iter().skip(1).cloned().collect();
    defs.resize(host.cod2().weapons.len() - 1, None);
    defs.push(Some(def));
    host.weapons = std::rc::Rc::new(crate::weapons::WeaponTable::cod2(
        &host.cod2().weapons,
        defs,
    ));
    g_register_weapon(host, &keys);
    Some(host.cod2().weapons.len())
}

/// `bg_numItems` (rodata 0x8154ed8): configstring 8 carries one bit each.
const COD2_ITEMS: usize = 131;

/// Configstrings 7 and 8 off the registered weapons: `G_InitGame`'s tail
/// (0x8109096, past `level.initializing`) and `G_RunFrame` when a
/// registration dirtied them write the whole list once (0x8106ec0: the
/// names space-separated, 1-based, `vcod_cod2::net::protocol::CS_WEAPONS`)
/// and the item bits (0x8106f7c), never one registration at a time.
/// VERIFIED, the calls and the dirty words. A restart that registers the
/// same list writes nothing (no `d 7`, cod2-map-cycle.md 3).
pub fn write_weapon_configstrings(host: &mut GameHost) {
    if host.cod2().weapons.is_empty() {
        return;
    }
    host.configstrings.set(
        vcod_cod2::net::protocol::CS_WEAPONS,
        host.cod2().weapons.join(" "),
    );
    let registered = host.cod2().weapons.len() + 1;
    let digits: String = (0..COD2_ITEMS.div_ceil(4))
        .map(|d| {
            let nibble = (0..4)
                .filter(|b| d * 4 + b < registered.min(COD2_ITEMS))
                .fold(0u32, |n, b| n | 1 << b);
            char::from_digit(nibble, 16).unwrap_or('0')
        })
        .collect();
    host.configstrings.set(8, digits);
}

/// `G_RegisterWeapon` (0x8107216), the callback 0x80eb8bc runs on a new
/// weapon: its item flag, which configstring 8 carries (0x8106f7c: four
/// items a hex digit, low bit first, over `bg_numItems`; written by
/// [`write_weapon_configstrings`]), the two hint
/// strings (`G_GetHintStringIndex`), then `G_ModelIndex` of the world
/// model and of the projectile model. VERIFIED, the calls and their order.
/// Item 0's bit reads set on both live captures (cod2-protocol.md 15.3);
/// what sets it is not traced.
fn g_register_weapon(host: &mut GameHost, keys: &std::collections::HashMap<String, String>) {
    use crate::configstrings::CsRange;
    let layout = &vcod_cod2::net::PROTOCOL_118.cs;
    for key in ["useHintString", "dropHintString"] {
        if let Some(hint) = keys.get(key).filter(|h| !h.is_empty()) {
            crate::configstrings::hint_string_alloc(&mut host.configstrings, layout, hint);
        }
    }
    for key in ["worldModel", "projectileModel"] {
        if let Some(model) = keys.get(key).filter(|m| !m.is_empty()) {
            let _ = host
                .allocators
                .index(&mut host.configstrings, CsRange::Model, model);
        }
    }
}

/// `precacheItem(name)` (`Scr_PrecacheItem` 0x8114918): `G_FindItem`
/// (0x80df226) tries the static items past the weapons, then a weapon by
/// name. Of that only the weapon arm is here; no static item, no item
/// bit and no model index is registered yet.
fn precache_item(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    before_any_wait(host, "precacheItem")?;
    let name = name_arg(cx, args, "precacheItem takes an item name")?;
    if register_weapon(host, &name).is_none() {
        return Err(ErrorKind::Custom(format!("unknown item '{name}'")));
    }
    Ok(Value::Undefined)
}

/// `precacheTurret(weapon)` (lnxded 0x811141a): before any wait,
/// `G_GetWeaponIndexForName`. VERIFIED.
fn precache_turret(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    before_any_wait(host, "precacheTurret")?;
    let name = name_arg(cx, args, "precacheTurret takes a weapon name")?;
    host.weapon_index(&name);
    Ok(Value::Undefined)
}

/// `spawnTurret(classname, origin, weapon)` (lnxded 0x811138c): `G_Spawn`,
/// the classname and origin, `G_SpawnTurret` with no spawn keys, the
/// entity returned. VERIFIED. Its model comes from a later `setModel`.
fn spawn_turret(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let classname = string_arg(cx, args, 0, "spawnTurret takes a classname")?;
    let origin = vector_arg(args, 1, "spawnTurret takes an origin")?;
    let weapon = string_arg(cx, args, 2, "spawnTurret takes a weapon name")?;
    use vcod_gsc::Host;
    let id = host.ents.spawn(cx)?;
    for (f, v) in [
        ("classname", Value::String(cx.intern_exact(&classname))),
        ("origin", Value::Vector(origin)),
    ] {
        let a = cx.intern_folded(f);
        host.set_field(cx, id, a, v)?;
    }
    crate::game::spawn::spawn_turret(host, cx, id, &weapon, &Default::default())?;
    Ok(Value::Entity(id))
}

/// A string argument, resolved.
fn string_arg(cx: &Cx, args: &[Value], i: usize, what: &'static str) -> Result<String, ErrorKind> {
    match args.get(i) {
        Some(Value::String(s) | Value::Localized(s)) => Ok(cx.resolve(*s).to_string()),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// A vector argument.
fn vector_arg(args: &[Value], i: usize, what: &'static str) -> Result<[f32; 3], ErrorKind> {
    match args.get(i) {
        Some(Value::Vector(v)) => Ok(*v),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `Scr_GetFloat`: an int or a float.
fn float_arg(args: &[Value], i: usize, what: &'static str) -> Result<f32, ErrorKind> {
    match args.get(i) {
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Int(n)) => Ok(*n as f32),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `Scr_GetInt`: an int, or a float truncated.
fn int_arg(args: &[Value], i: usize, what: &'static str) -> Result<i32, ErrorKind> {
    match args.get(i) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Float(f)) => Ok(*f as i32),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `bulletTracePassed(start, end, hitCharacters, ignoreEnt)`
/// (`Scr_BulletTracePassed` 0x8113a4a): `G_LocationalTracePassed` under
/// mask 0x2802831, 0x802831 without characters, passing the entity in the
/// fourth argument when it is one; true when nothing stops the segment.
/// VERIFIED. vcod runs `bulletTrace`'s trace for it, whose mask is 1.1's.
pub(crate) fn bullet_trace_passed(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "bulletTracePassed takes a start, an end, hitCharacters and ignoreEnt";
    let from = vector_arg(args, 0, USAGE)?;
    let to = vector_arg(args, 1, USAGE)?;
    let chars = int_arg(args, 2, USAGE)? != 0;
    let ignore = match args.get(3) {
        Some(Value::Entity(id)) => Some(*id),
        _ => None,
    };
    let (hit, _) = super::combat::bullet_hit(
        host,
        cx,
        glam::Vec3::from(from),
        glam::Vec3::from(to),
        chars,
        ignore,
    );
    Ok(Value::Int(i32::from(hit.is_none())))
}

/// `self closeInGameMenu()` (`PlayerCmd_CloseInGameMenu` 0x80fbed2): the
/// reliable `K` (`"%c"` at 0x8157ff6 with 0x4b). VERIFIED.
fn close_in_game_menu(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_commands.push((slot, "K".to_string()));
    Ok(Value::Undefined)
}

/// `self getAmmoCount(weapon)` (`GScr_GetAmmoCount` 0x8111104):
/// `BG_WeaponAmmo`, the reserve plus the clip, 0 for a name no weapon
/// file backs. VERIFIED.
fn get_ammo_count(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = string_arg(cx, args, 0, "getAmmoCount takes a weapon name")?;
    let Some(index) = host.weapon_index(&name) else {
        return Ok(Value::Int(0));
    };
    let Some(def) = host.weapons.get(index) else {
        return Ok(Value::Int(0));
    };
    let a = &host.client_ammo[slot];
    Ok(Value::Int(
        i32::from(a.ammo[def.ammo_index]) + i32::from(a.clip[def.clip_index]),
    ))
}

/// `self getCurrentOffhand()` (`PlayerCmd_getCurrentOffhand` 0x80fa030):
/// the offhand weapon's name, `"none"` for a client not playing or with no
/// offhand. VERIFIED.
fn get_current_offhand(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let offhand = host.cod2().clients[slot].offhand as usize;
    let name = if host.playing(cx, slot) && offhand > 0 {
        host.weapon_name(offhand)
    } else {
        None
    };
    let name = name.unwrap_or_else(|| "none".to_string());
    Ok(Value::String(cx.intern_exact(&name)))
}

/// `self getCurrentWeapon()`: 1.1's, over CoD2's weapon list.
fn get_current_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let index = host.client_weapons[slot].current as usize;
    let name = host
        .weapon_name(index)
        .unwrap_or_else(|| "none".to_string());
    Ok(Value::String(cx.intern_exact(&name)))
}

/// `self getPlayerAngles()` (`PlayerCmd_getAngles` 0x80fa9ec):
/// `ps.viewangles`. VERIFIED.
fn get_player_angles(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    Ok(Value::Vector(host.cod2().clients[slot].view_angles))
}

/// `radiusDamage(origin, range, maxDamage, minDamage)` (`GScr_RadiusDamage`
/// lnxded 0x8114cbe, Mac 0x19947a): CoD2's `G_RadiusDamage` over its
/// bodies and damageable entities ([`crate::game::cod2_blast::radius_damage`]).
/// The four arguments are read by index; anything past them is ignored.
fn radius_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "radiusDamage takes an origin, a range, a max damage and a min damage";
    let origin = vector_arg(args, 0, USAGE)?;
    let range = float_arg(args, 1, USAGE)?;
    let max = float_arg(args, 2, USAGE)?;
    let min = float_arg(args, 3, USAGE)?;
    crate::game::cod2_blast::radius_damage(host, cx, glam::Vec3::from(origin), range, max, min);
    Ok(Value::Undefined)
}

/// `randomInt(n)` (0x8113ca4): `irand(0, n)` off `holdrand`, the
/// generator the rocket's wobble draws from (cod2-combat.md 2.6.1); `n`
/// under 1 is an error. VERIFIED.
fn random_int(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let n = int_arg(args, 0, "randomInt takes a number")?;
    if n <= 0 {
        return Err(ErrorKind::Custom(
            "RandomInt parm must be positive integer.".into(),
        ));
    }
    Ok(Value::Int(host.cod2().rand.irand(0, n)))
}

/// `randomFloat(f)` (0x8113cfe): `flrand(0, f)` off `holdrand`. VERIFIED.
fn random_float(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let f = float_arg(args, 0, "randomFloat takes a number")?;
    Ok(Value::Float(host.cod2().rand.flrand(0.0, f)))
}

/// `randomIntRange(lo, hi)` (0x8113d32): `irand(lo, hi)` off `holdrand`;
/// `hi` not above `lo` is an error. VERIFIED.
fn random_int_range(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "randomIntRange takes two numbers";
    let (lo, hi) = (int_arg(args, 0, USAGE)?, int_arg(args, 1, USAGE)?);
    if hi <= lo {
        return Err(ErrorKind::Custom(
            "RandomIntRange range must be positive integer.".into(),
        ));
    }
    Ok(Value::Int(host.cod2().rand.irand(lo, hi)))
}

/// `randomFloatRange(lo, hi)` (0x8113da0): `flrand(lo, hi)` off
/// `holdrand`; `lo` at or above `hi` is an error. VERIFIED.
fn random_float_range(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "randomFloatRange takes two numbers";
    let (lo, hi) = (float_arg(args, 0, USAGE)?, float_arg(args, 1, USAGE)?);
    if lo >= hi {
        return Err(ErrorKind::Custom(
            "Scr_RandomFloatRange range must be positive float.".into(),
        ));
    }
    Ok(Value::Float(host.cod2().rand.flrand(lo, hi)))
}

/// `self setStableMissile(on)` (`Scr_SetStableMissile` 0x811225a): a
/// player's `ent->flags` 0x20000 set or cleared, which `fire_rocket`
/// copies onto the rocket and which keeps it from wobbling
/// (cod2-combat.md 2.6). Anything but a player is an error. VERIFIED.
fn set_stable_missile(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)
        .map_err(|_| ErrorKind::Custom("Type should be a player".into()))?;
    let on = int_arg(args, 0, "setStableMissile takes a number")?;
    let flags = &mut host.cod2_mut().clients[slot].flags;
    if on != 0 {
        *flags |= crate::game::cod2_cmds::FL_STABLE_MISSILE;
    } else {
        *flags &= !crate::game::cod2_cmds::FL_STABLE_MISSILE;
    }
    Ok(Value::Undefined)
}

/// `int(x)` (`GScr_CastInt` 0x811400a): a float truncated (its `fistp`
/// runs under `| 0xc00`), a string through `atoi`, an int as it is;
/// anything else is `cannot cast %s to int`. VERIFIED.
pub(super) fn cast_int(
    _host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    match args.first() {
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Int(*f as i32)),
        Some(Value::String(s)) => Ok(Value::Int(super::cvar::atoi(cx.resolve(*s)))),
        Some(v) => Err(ErrorKind::Custom(format!(
            "cannot cast {} to int",
            type_name(v)
        ))),
        None => Err(ErrorKind::BadType("int takes one argument")),
    }
}

/// `self setNormalHealth(fraction)` (`ScrCmd_SetNormalHealth` 0x8111fd2):
/// the fraction capped at 1; a player's health is `floor(0.5 + maxhealth *
/// f)` and the client is sent `I "0"`, any other entity's is `maxhealth *
/// f` truncated, or `f` truncated with no maxhealth. A result below 1
/// prints an error and changes nothing. VERIFIED.
fn set_normal_health(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    use vcod_gsc::Host;
    let id = super::entity::entity_receiver(recv)?;
    let f = float_arg(args, 0, "setNormalHealth takes a number")?.min(1.0);
    let is_client = host.ents.get(id).is_some_and(|e| e.client.is_some());
    let health = if is_client {
        let slot = id.0 as usize;
        let max = host.client_vitals[slot].max_health;
        host.client_commands.push((slot, "I \"0\"".to_string()));
        (0.5 + max as f32 * f).floor() as i32
    } else {
        let field = cx.intern_folded("maxhealth");
        match host.get_field(cx, id, field) {
            Value::Int(m) if m != 0 => (m as f32 * f) as i32,
            _ => f as i32,
        }
    };
    if health < 1 {
        log::warn!("ERROR: Cannot setnormalhealth to 0 or below.");
        return Ok(Value::Undefined);
    }
    let field = cx.intern_folded("health");
    host.set_field(cx, id, field, Value::Int(health))?;
    Ok(Value::Undefined)
}

/// `setTeamScore(team, n)` (`GScr_SetTeamScore` 0x8115dda): CoD2 sends the
/// score to every client as `H <n>` (allies, `level+0x204`) or `G <n>`
/// (axis, `level+0x200`) where 1.1 wrote configstrings 5 and 6, and raises
/// the ranks flag (`level+0x210`). VERIFIED, the function and
/// `GScr_LoadConsts`' `allies` at 0x8853222.
fn set_team_score(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let team = match args.first() {
        Some(Value::String(s)) => cx.resolve(*s).to_string(),
        _ => String::new(),
    };
    let (i, letter) = match team.as_str() {
        "axis" => (0, 'G'),
        "allies" => (1, 'H'),
        _ => {
            return Err(ErrorKind::Custom(format!(
                "Illegal team string '{team}'. Must be allies, or axis."
            )));
        }
    };
    let n = int_arg(args, 1, "setTeamScore takes a number")?;
    host.team_scores[i] = n;
    // `SV_GameSendServerCommand(-1, ..)`: the engine's clients, so a
    // restart's `resetScores` reaches the clients its level has not yet
    // connected (cod2-map-cycle.md 2, sd).
    host.client_commands
        .push((crate::game::commands::ALL_CLIENTS, format!("{letter} {n}")));
    host.ranks_dirty = true;
    Ok(Value::Undefined)
}

/// `hudelem setWaypoint(bool)` (`HECmd_SetWaypoint` 0x81044c0): type 13
/// and the argument as a float at `+0x70`, CoD2's `value` word (the
/// netfield table's offset 112), which the client reads as a sprite of one
/// size on screen (cod2-hud.md 19). VERIFIED.
fn set_waypoint(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let on = int_arg(args, 0, "setWaypoint takes a boolean")?;
    super::hud::with_hud(host, id, |h| {
        h.elem_type = HE_TYPE_WAYPOINT;
        h.value = on as f32;
    })?;
    Ok(Value::Undefined)
}

/// CoD2's waypoint hudelem type.
pub const HE_TYPE_WAYPOINT: i32 = 13;

/// `self setWeaponClipAmmo(weapon, n)` (`PlayerCmd_SetWeaponClipAmmo`
/// 0x80fc91e): the weapon's clip, clamped to `0..=clipSize`; a name no
/// weapon file backs does nothing. VERIFIED.
fn set_weapon_clip_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = string_arg(cx, args, 0, "setWeaponClipAmmo takes a weapon name")?;
    let rounds = int_arg(args, 1, "setWeaponClipAmmo takes a round count")?;
    let Some(index) = host.weapon_index(&name) else {
        return Ok(Value::Undefined);
    };
    let Some(def) = host.weapons.get(index) else {
        return Ok(Value::Undefined);
    };
    let rounds = rounds.clamp(0, def.clip_size.min(i16::MAX as u32) as i32) as i16;
    let clip_index = def.clip_index;
    host.weapon_op(slot, WeaponOp::SetClip { clip_index, rounds });
    Ok(Value::Undefined)
}

/// `self switchToOffhand(weapon)` (`PlayerCmd_switchToOffhand`
/// 0x80fa2ac): an unknown name is a parameter error; for a held weapon
/// the reliable `C <index>` (0x8120e44) and true, else false. VERIFIED.
fn switch_to_offhand(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = string_arg(cx, args, 0, "switchToOffhand takes a weapon name")?;
    let Some(index) = host.weapon_index(&name) else {
        return Err(ErrorKind::Custom(format!("unknown weapon '{name}'")));
    };
    if !host.client_weapons[slot].holds(index) {
        return Ok(Value::Int(0));
    }
    host.client_commands.push((slot, format!("C {index}")));
    Ok(Value::Int(1))
}

/// `self updateScores()` (`GScr_UpdateScores` 0x81169a2): the two team
/// scores to this client alone, `H` allies then `G` axis. VERIFIED.
fn update_scores(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id: EntId = super::entity::entity_receiver(recv)?;
    let slot = id.0 as usize;
    let [axis, allies] = host.team_scores;
    host.client_commands.push((slot, format!("H {allies}")));
    host.client_commands.push((slot, format!("G {axis}")));
    Ok(Value::Undefined)
}

/// `vectorDot(a, b)` (`Scr_VectorDot` 0x8114240): the dot product on the
/// x87 stack, stored as a float. VERIFIED.
pub(super) fn vector_dot(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "vectorDot takes two vectors";
    let (a, b) = (vector_arg(args, 0, USAGE)?, vector_arg(args, 1, USAGE)?);
    let dot: f64 = (0..3).map(|i| f64::from(a[i]) * f64::from(b[i])).sum();
    Ok(Value::Float(dot as f32))
}

/// The type name a cast error spells (`var_typename`, the names 1.1 prints
/// for the same types; INFERRED for CoD2's table).
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Undefined => "undefined",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Localized(_) => "localized string",
        Value::Vector(_) => "vector",
        Value::Entity(_) | Value::Struct(_) | Value::Array(_) => "object",
        Value::Function(_) => "function",
        _ => "value",
    }
}

/// `self giveWeapon(weapon)` (`PlayerCmd_giveWeapon` 0x80f9b8c, Mac
/// 0x1c2438; `G_GivePlayerWeapon` 0x8120b26): 1.1's give, but a weapon
/// that needs a slot takes the first empty one of CoD2's two
/// (`BG_GetEmptySlotForWeapon`, ps+0x555 then +0x556), and the rounds are
/// `Add_Ammo` of `startAmmo` less the reserve held, filling the clip first
/// for a weapon not held before. For an offhand weapon (`offhandClass`
/// set) the playerstate's offhand when it has none, sent as `C <index>`
/// (0x8120e44). VERIFIED. Not done: an empty offhand passing to the next
/// (`BG_GetFirstAvailableOffhand`), and the script error with both slots
/// full (the weapon is given with no slot).
fn give_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = string_arg(cx, args, 0, "giveWeapon takes a weapon name")?;
    let had = host
        .weapon_index(&name)
        .is_some_and(|i| host.client_weapons[slot].holds(i));
    let before = host.client_weapons[slot];
    let rounds = host.client_ammo[slot];
    super::client::give_weapon(host, cx, recv, args)?;
    let Some(index) = host.weapon_index(&name) else {
        return Ok(Value::Undefined);
    };
    let offhand = host
        .cod2()
        .offhands
        .get(index - 1)
        .copied()
        .unwrap_or(false);
    let weapons = host.weapons.clone();
    let w = &mut host.client_weapons[slot];
    w.slots = before.slots;
    if !had
        && !offhand
        && matches!(weapons.slot(index), 1 | 2)
        && let Some(s) = crate::game::pickup::empty_slot_for(&before, 1)
    {
        w.slots[s] = index as u8;
    }
    if let Some(def) = weapons.get(index) {
        let (a, c) = (def.ammo_index, def.clip_index);
        let (mut ammo, mut clip) = (i32::from(rounds.ammo[a]), i32::from(rounds.clip[c]));
        let count = def.start_ammo as i32 - ammo;
        if count > 0 {
            ammo += count;
            if !had || def.clip_only {
                let moved = (def.clip_size as i32 - clip).min(ammo);
                ammo -= moved;
                clip += moved;
            }
            ammo = if def.clip_only {
                0
            } else {
                ammo.min(def.max_ammo as i32)
            };
            clip = clip.min(def.clip_size as i32);
        }
        // Over the 1.1 give's full clip and `startAmmo`.
        host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index: c,
                rounds: clip as i16,
            },
        );
        if !def.clip_only {
            host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: a,
                    rounds: ammo as i16,
                },
            );
        }
    }
    if offhand && host.cod2().clients[slot].offhand == 0 {
        host.cod2_mut().clients[slot].offhand = index as u8;
        host.client_commands.push((slot, format!("C {index}")));
    }
    // A weapon the player lacked: `I "1"` after the give (0x80f9b8c,
    // cod2-protocol.md 15.5).
    if !had && host.client_weapons[slot].holds(index) {
        host.client_commands.push((slot, "I \"1\"".to_string()));
    }
    Ok(Value::Undefined)
}

fn number(v: &Value) -> Result<i32, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i32),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

/// `self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags,
/// sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, psOffsetTime)`
/// (`PlayerCmd_finishPlayerDamage`, Mac 0x1c2e32, lnxded 0x80fb170):
/// 1.1's with a tenth argument. A bullet weapon's hit raises
/// `EV_BULLET_HIT_SMALL`/`LARGE` (182/183, by `rifleBullet`) for every
/// client but the victim and `EV_BULLET_HIT_CLIENT_SMALL`/`LARGE`
/// (185/186) for the victim alone, both on flesh at the point; the health
/// comes off, `damage` is notified with the damage and the attacker, and
/// a kill runs `player_die`: `death` with the attacker, then the killed
/// callback with `psOffsetTime` and `deathAnimDuration`, the death anim
/// drawn off the clause the client's movetype picks
/// (`game::cod2::Cod2ClientState::death_anims`). The playerstate's half
/// (knockback, the damage timer, `pm_type`) goes out as a `SimOp` for
/// `Cod2Level` (`game::cod2_combat`).
fn finish_player_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    use crate::game::host::SimOp;
    use crate::game::temp_entity::{Scope, TempEntity};
    let slot = client_receiver(host, recv)?;
    if args.len() < 9 {
        return Err(ErrorKind::BadType("finishPlayerDamage takes ten arguments"));
    }
    let (inflictor, attacker, mod_, weapon, hitloc) =
        (&args[0], &args[1], &args[4], &args[5], &args[8]);
    let damage = number(&args[2])?;
    let dflags = number(&args[3])?;
    let time_offset = args.get(9).map(number).transpose()?.unwrap_or(0);
    if damage < 1 {
        return Ok(Value::Undefined);
    }
    let point = match &args[6] {
        Value::Vector(p) => *p,
        _ => [0.0; 3],
    };
    let dir: [f32; 3] = match &args[7] {
        // `Vec3NormalizeTo` (lnxded 0x80fb170's call), its stores and all.
        Value::Vector(d) => vcod_cod2::pmove::vec3_normalize_to(glam::Vec3::from(*d)).into(),
        _ => [0.0; 3],
    };
    let attacker_slot = match attacker {
        Value::Entity(a) if host.ents.get(*a).is_some_and(|e| e.client.is_some()) => {
            Some(a.0 as usize)
        }
        _ => None,
    };
    let world = host.ents.world(cx);
    let attacker_ent = match attacker {
        Value::Entity(a) => *a,
        _ => world,
    };
    // A god (`ent->flags` 1) is knocked back and nothing more.
    let flags = host.cod2().clients[slot].flags;
    if flags & crate::game::cod2_cmds::FL_GODMODE != 0 {
        host.client_sim_ops.push((
            slot,
            SimOp::Damaged {
                damage,
                point,
                dir,
                knockback: dflags & crate::game::cod2_combat::DFLAG_NO_KNOCKBACK == 0,
                attacker: attacker_slot,
                dead_yaw: 0,
                fatal: false,
            },
        ));
        return Ok(Value::Undefined);
    }
    // A bullet weapon's two flesh impacts.
    let rifle = match weapon {
        Value::String(w) => host
            .weapon_index(cx.resolve(*w))
            .and_then(|i| host.weapons.get(i))
            .filter(|d| d.weapon_type == "bullet")
            .map(|d| d.sounds.rifle_bullet),
        _ => None,
    };
    if let Some(rifle) = rifle {
        let byte = vcod_common::net::events::dir_to_byte(dir);
        let large = i32::from(rifle);
        // In 1.1's numbering, which the snapshot maps to CoD2's 182/183
        // and 185/186 (`cod2_wire::event_to_cod2`).
        use vcod_common::net::event_ids as v1;
        for (event, scope, client_num, scale) in [
            (
                v1::EV_BULLET_HIT_SMALL + large,
                Scope::AllBut(slot),
                0,
                byte,
            ),
            (
                v1::EV_BULLET_HIT_CLIENT_SMALL + large,
                Scope::Only(slot),
                slot as i32,
                0,
            ),
        ] {
            host.add_temp_entity(
                cx,
                TempEntity {
                    event,
                    parm: if scale != 0 { byte } else { 0 },
                    surf_type: 7,
                    other: attacker_ent.0,
                    attacker: 0,
                    weapon: 0,
                    client_num,
                    scale,
                    origin: point,
                    scope,
                    quake: None,
                    origin2: None,
                },
            );
        }
    }
    let v = &mut host.client_vitals[slot];
    // A demigod keeps one point.
    let damage = if flags & crate::game::cod2_cmds::FL_DEMI_GODMODE != 0 && v.health <= damage {
        v.health - 1
    } else {
        damage
    };
    let fatal = !v.dead && v.health - damage <= 0;
    v.health = (v.health - damage).max(-999);
    if let Some(r) = recv {
        let event = cx.intern_folded("damage");
        cx.notify(
            r,
            event,
            vec![Value::Int(damage), Value::Entity(attacker_ent)],
        );
    }
    host.client_sim_ops.push((
        slot,
        SimOp::Damaged {
            damage,
            point,
            dir,
            knockback: dflags & crate::game::cod2_combat::DFLAG_NO_KNOCKBACK == 0,
            attacker: attacker_slot,
            dead_yaw: dead_yaw(host, slot, attacker_slot),
            fatal,
        },
    ));
    if !fatal {
        return Ok(Value::Undefined);
    }
    host.die(slot);
    let duration = draw_death_anim(host, slot);
    host.cod2_mut().callback = crate::game::host::Cod2CallbackArgs {
        time_offset,
        death_anim_duration: duration,
    };
    // An entity inflictor reads as the attacker argument and no entity as
    // the world, as 1.1's (`Scr_GetEntity(1)`, Mac 0x1c2e32).
    let attacker_v = Value::Entity(attacker_ent);
    let inflictor_v = match inflictor {
        Value::Entity(_) => attacker_v,
        _ => Value::Entity(world),
    };
    if let Some(r) = recv {
        let death = cx.intern_folded("death");
        cx.notify(r, death, vec![attacker_v]);
    }
    // `player_die` (lnxded 0x810175a): a weapon killed by an attacker on a
    // turret reads as the turret's weapon (cod2-turrets.md 11).
    let held = match weapon {
        Value::String(w) => host.weapon_index(cx.resolve(*w)).is_some_and(|i| i != 0),
        _ => false,
    };
    let gun_weapon = attacker_slot
        .filter(|_| held)
        .and_then(|a| host.turrets.values().find(|r| r.owner == Some(a)))
        .map(|r| r.weapon.clone());
    let weapon = match gun_weapon {
        Some(name) => Value::String(cx.intern_exact(&name)),
        None => *weapon,
    };
    let killed = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerKilled",
    );
    cx.spawn_then(
        killed,
        recv,
        host.callback_args(
            "CodeCallback_PlayerKilled",
            vec![
                inflictor_v,
                attacker_v,
                Value::Int(damage),
                *mod_,
                weapon,
                Value::Vector(dir),
                *hitloc,
            ],
        ),
        slot as u32,
    );
    host.cod2_mut().callback = Default::default();
    Ok(Value::Undefined)
}

/// `player_die`'s `BG_AnimScriptEvent(ps, ANIM_ET_DEATH, 0, 1)` (lnxded
/// 0x80d96ee): `rand() % count` off the game's `rand()` among the `death`
/// clause's anims for the client's movetype, kept as its death anim;
/// returns the `deathAnimDuration` the killed callback is handed (0 with
/// no clause, which draws nothing).
pub fn draw_death_anim(host: &mut GameHost, slot: usize) -> i32 {
    let state = host.cod2().clients[slot];
    let n = usize::from(state.death_anim_count);
    let (anim, duration) = if n == 0 {
        (0, 0)
    } else {
        let drawn = state.death_anims[host.cod2().rand.rand() as usize % n];
        host.cod2_mut()
            .death_pins
            .get_mut(slot)
            .and_then(|q| q.pop_front())
            .and_then(|pin| state.death_anims[..n].iter().find(|a| a.0 == pin))
            .copied()
            .unwrap_or(drawn)
    };
    host.cod2_mut().clients[slot].death_anim = anim;
    duration
}

/// `player_die`'s `stats[1]` (Mac 0x1c4a7c): the yaw from the victim to
/// its attacker, the victim's own view yaw when it killed itself or no
/// client did. The inflictor's place is not read apart from the attacker's.
fn dead_yaw(host: &GameHost, slot: usize, attacker: Option<usize>) -> i32 {
    let own = host.cod2().clients[slot].view_angles[1] as i32;
    let Some(a) = attacker.filter(|&a| a != slot) else {
        return own;
    };
    let origin = |s: usize| -> Option<[f32; 3]> {
        let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
        let e = host.client_entity_states.get(s)?.as_ref()?;
        Some([0, 1, 2].map(|k| f32::from_bits(e.field_i32(p, &format!("pos.trBase[{k}]")) as u32)))
    };
    let (Some(to), Some(from)) = (origin(a), origin(slot)) else {
        return own;
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    super::math::vectoangles(d)[1] as i32
}

/// `self cloneplayer(deathAnimDuration)` (`PlayerCmd_ClonePlayer`, Mac
/// 0x1c3574): a corpse (`eType` 2) of the client where it stands, its
/// `eFlags` the playerstate's with `EF_DEAD` and the fresh-corpse bit
/// (0xa0000), its legs and torso the playerstate's after the death anim, in
/// the level's body queue; its think at `deathAnimDuration` takes the
/// fresh-corpse bit out, and it falls from the player's origin at its
/// velocity (x and y capped above at `g_clonePlayerMaxVelocity`) until it
/// rests ([`crate::game::cod2_corpse`]).
fn clone_player(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(state) = host.client_entity_states[slot].clone() else {
        return Ok(Value::Undefined);
    };
    let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
    let mut e = vcod_common::net::msg::EntityState::null(p);
    let get = |name: &str| state.field_i32(p, name);
    let mut set = |name: &str, v: i32| {
        if let Some(i) = vcod_common::net::msg::EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    set("eType", vcod_common::net::flags::ET_CORPSE);
    set("clientNum", slot as i32);
    set("eFlags", (get("eFlags") & !0x2) | 0xa_0000);
    // The playerstate's anims once `player_die`'s death event ran. A
    // callback thread that runs before the playerstate takes it (one the
    // kill's own cmd started) sees the anims from before: the death anim
    // goes on the legs and no anim on the torso, each toggled, as the
    // event will.
    let toggle = vcod_cod2::pmove::anim::ANIM_TOGGLEBIT;
    let flip = |w: i32| (w & toggle) ^ toggle;
    let death = host.cod2().clients[slot].death_anim;
    if death != 0 && get("legsAnim") & !toggle != death {
        set("legsAnim", death | flip(get("legsAnim")));
        set("torsoAnim", flip(get("torsoAnim")));
    } else {
        set("legsAnim", get("legsAnim"));
        set("torsoAnim", get("torsoAnim"));
    }
    // `G_SetAngle(clone, player r.currentAngles)`: (0, the view's yaw, 0)
    // off the player's last think, not the body's `apos`
    // (cod2-protocol.md 15.7).
    let yaw = host.cod2().clients[slot].view_angles[1];
    for axis in 0..3 {
        let name = format!("pos.trBase[{axis}]");
        set(&name, get(&name));
        let a = if axis == 1 { yaw } else { 0.0 };
        set(&format!("apos.trBase[{axis}]"), a.to_bits() as i32);
    }
    let now = host.level_time_ms;
    let world = host.world.clone();
    // The clone's think at `deathAnimDuration` takes the fresh-corpse bit
    // out (retail's corpse loses 0x80000 the death anim's length after its
    // birth, cod2-player-anims.md 10).
    let duration = match args.first() {
        Some(Value::Int(d)) => *d,
        Some(Value::Float(d)) => *d as i32,
        _ => 0,
    };
    let number = host.bodies.push_raw(
        e,
        now,
        world.as_ref().map(|w| &w.collision),
        p,
        Some((0x8_0000, duration)),
    );
    // The clone falls from `ps.origin` at its velocity (Mac 0x1c3574's
    // `G_SetOrigin`; `game::cod2_corpse`), not a shot's rewound
    // `r.currentOrigin`.
    let get_vec = |name: &str| {
        glam::Vec3::from_array(std::array::from_fn(|k| {
            f32::from_bits(state.field_i32(p, &format!("{name}[{k}]")) as u32)
        }))
    };
    let origin = host.cod2().clients[slot]
        .body
        .map_or_else(|| get_vec("pos.trBase"), |b| b.ps_origin);
    let velocity = get_vec("pos.trDelta");
    let max_velocity = match host.cvars.get("g_clonePlayerMaxVelocity") {
        "" => crate::game::cod2_corpse::MAX_VELOCITY,
        v => super::cvar::atof(v),
    };
    if let Some(body) = host.bodies.body_mut(number) {
        crate::game::cod2_corpse::launch(body, origin, velocity, now, max_velocity, p);
    }
    Ok(Value::Entity(host.ents.spawn_body(cx, number)))
}

/// `obituary(victim, attacker, sWeapon, sMeansOfDeath)` (`GScr_Obituary`,
/// Mac 0x1980cc): `EV_OBITUARY` (198) broadcast at the origin, the victim
/// in `otherEntityNum`, a player attacker in `attackerEntityNum` (1022
/// otherwise), and the parm the weapon's index, or `0x80 |` the means of
/// death for melee, a headshot, a crush, a fall or a suicide (7, 8, 9, 11,
/// 12). VERIFIED, the stores.
fn obituary(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (
        Some(Value::Entity(victim)),
        Some(attacker),
        Some(Value::String(weapon)),
        Some(Value::String(means)),
    ) = (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "obituary takes a victim, an attacker, a weapon name and a means of death",
        ));
    };
    let attacker = match attacker {
        Value::Entity(a) => a.0 as i32,
        _ => 1022,
    };
    let means = host.defs.mod_index(cx.resolve(*means)).unwrap_or(0);
    let parm = if matches!(means, 7 | 8 | 9 | 11 | 12) {
        0x80 | means
    } else {
        let name = cx.resolve(*weapon).to_string();
        host.weapon_index(&name).map_or(0, |i| i as i32)
    };
    host.add_temp_entity(
        cx,
        crate::game::temp_entity::TempEntity {
            // 1.1's number; the snapshot sends CoD2's 198.
            event: vcod_common::net::event_ids::EV_OBITUARY,
            parm,
            surf_type: 0,
            other: victim.0,
            attacker,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: [0.0; 3],
            scope: crate::game::temp_entity::Scope::Broadcast,
            quake: None,
            origin2: None,
        },
    );
    Ok(Value::Undefined)
}

/// `self dropItem(name [, tag])` (`PlayerCmd_dropItem`, Mac 0x1c053c):
/// 1.1's `Drop_Weapon` arithmetic and item (`builtins::client::drop_item`)
/// on CoD2's weapon rows; the launch is CoD2's (`game::cod2_items`). A
/// name no weapon or item backs drops nothing.
fn drop_item(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    super::client::drop_item(host, cx, recv, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    #[test]
    fn int_truncates_parses_and_refuses() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let s = Value::String(cx.intern_exact("12x"));
            assert_eq!(
                cast_int(&mut host, cx, None, &[Value::Float(-3.9)]),
                Ok(Value::Int(-3))
            );
            assert_eq!(cast_int(&mut host, cx, None, &[s]), Ok(Value::Int(12)));
            assert_eq!(
                cast_int(&mut host, cx, None, &[Value::Int(7)]),
                Ok(Value::Int(7))
            );
            assert!(cast_int(&mut host, cx, None, &[Value::Vector([0.0; 3])]).is_err());
            let dot = vector_dot(
                &mut host,
                cx,
                None,
                &[
                    Value::Vector([1.0, 2.0, 3.0]),
                    Value::Vector([4.0, 5.0, 6.0]),
                ],
            );
            assert_eq!(dot, Ok(Value::Float(32.0)));
        });
    }

    /// CoD2's team score goes out as `G`/`H` commands, not configstrings.
    #[test]
    fn a_team_score_is_a_command_to_everyone() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            host.connect_client(cx, 0, "a").unwrap();
            let allies = Value::String(cx.intern_exact("allies"));
            set_team_score(&mut host, cx, None, &[allies, Value::Int(4)]).unwrap();
            assert_eq!(host.team_scores, [0, 4]);
            let all = crate::game::commands::ALL_CLIENTS;
            assert_eq!(host.client_commands, [(all, "H 4".to_string())]);
            assert_eq!(host.configstrings[6], "");
            let bad = Value::String(cx.intern_exact("spectator"));
            assert!(set_team_score(&mut host, cx, None, &[bad, Value::Int(1)]).is_err());
        });
    }

    /// `setWaypoint` turns the element into type 13 and keeps its flag in
    /// `value`.
    #[test]
    fn set_waypoint_marks_the_element() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let Value::Entity(id) =
                super::super::hud::new_hud_elem(&mut host, cx, None, &[]).unwrap()
            else {
                panic!("newHudElem returns an object");
            };
            set_waypoint(&mut host, cx, Some(Target::Entity(id)), &[Value::Int(1)]).unwrap();
            let h = host.ents.get(id).unwrap().hud.unwrap();
            assert_eq!((h.elem_type, h.value), (HE_TYPE_WAYPOINT, 1.0));
        });
    }
}
