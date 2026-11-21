//! CoD4 1.7's player methods the connect, the team and class menus and the
//! spawn reach (docs/research/cod4-gsc.md 7, "The connect, the menus and the spawn").
//! Each command's text is the walk capture's (`play-1.7-walk.txt`) where
//! the capture carries one.

use super::args::{int, noop};
use super::cod2::{is_talking, update_scores};
use vcod_gsc::{Cx, ErrorKind, Target, Value};

use super::Builtin;
use super::client::client_receiver;
use crate::game::host::GameHost;

pub const NAMES: &[(&str, Builtin)] = &[
    ("setclientdvar", super::client::set_client_cvar),
    ("setclientdvars", set_client_dvars),
    ("setrank", set_rank),
    ("closeingamemenu", close_in_game_menu),
    ("updatescores", update_scores),
    ("setteamscore", set_team_score),
    // Nothing a 1.7 server keeps: no log line on 1.7's dedicated build
    // (INFERRED).
    ("logstring", noop),
    ("addtestclient", add_test_client),
    // Rumble: client-side.
    ("stoprumble", noop),
    ("setperk", set_perk),
    ("pingplayer", ping_player),
    ("unsetperk", unset_perk),
    ("hasperk", has_perk),
    ("istalking", is_talking),
    ("clearperks", clear_perks),
    ("giveweapon", give_weapon),
    ("takeweapon", take_weapon),
    ("takeallweapons", take_all_weapons),
    ("setspawnweapon", set_spawn_weapon),
    ("switchtoweapon", switch_to_weapon),
    ("switchtooffhand", switch_to_offhand),
    ("hasweapon", has_weapon),
    ("getcurrentweapon", get_current_weapon),
    ("getcurrentoffhand", get_current_offhand),
    ("getweaponslist", get_weapons_list),
    ("setweaponammoclip", set_weapon_ammo_clip),
    ("setweaponammostock", set_weapon_ammo_stock),
    ("getweaponammoclip", get_weapon_ammo_clip),
    ("getweaponammostock", get_weapon_ammo_stock),
    ("givemaxammo", give_max_ammo),
    ("givestartammo", give_start_ammo),
    ("setactionslot", set_action_slot),
    ("setoffhandsecondaryclass", set_offhand_secondary_class),
    ("getoffhandsecondaryclass", get_offhand_secondary_class),
    ("setmovespeedscale", set_move_speed_scale),
    ("isweaponcliponly", is_weapon_clip_only),
    ("weaponclass", weapon_class),
    ("weaponclipsize", weapon_clip_size),
    ("weaponmaxammo", weapon_max_ammo),
    ("weaponstartammo", weapon_start_ammo),
    ("weaponinventorytype", weapon_inventory_type),
    ("weaponaltweaponname", weapon_alt_weapon_name),
    ("disableweapons", disable_weapons),
    ("enableweapons", enable_weapons),
    ("freezecontrols", freeze_controls),
    ("setpulsefx", set_pulse_fx),
    ("shellshock", shell_shock),
    ("setnormalhealth", set_normal_health),
    ("stopshellshock", stop_shell_shock),
    // The scoreboard's order within a team; vcod's scoreboard sorts by
    // score itself.
    ("setplayerteamrank", noop),
];

/// `self disableWeapons()` / `enableWeapons()`: `ps.weapFlags` 0x80
/// (the flag the weapon HUD hides on, cod4-hud.md 9).
fn disable_weapons(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    host.cod4_mut().clients[slot].weap_flags |= 0x80;
    Ok(Value::Undefined)
}

fn enable_weapons(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    host.cod4_mut().clients[slot].weap_flags &= !0x80;
    Ok(Value::Undefined)
}

/// `self freezeControls(bool)`: the frozen flag, which `ClientEndFrame` turns
/// into `PMF_FROZEN` for pmove.
fn freeze_controls(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let on = a
        .first()
        .ok_or(ErrorKind::BadType("freezeControls takes a boolean"))?
        .as_bool()?;
    host.cod4_mut().clients[slot].frozen = on;
    Ok(Value::Undefined)
}

/// `<hudelem> setPulseFX(letterTime, decayStart, decayDuration)`
/// (`HECmd_SetPulseFX`): the birth on `level.time`, the three
/// times (each refused below 0), and the owner's next `soundID`, 1..31
/// round (the walk capture's hint reads 1 after the spawn, VERIFIED live).
/// An element no client owns takes 0: vcod's choice; what 1.7 reads there
/// is not yet confirmed (INFERRED).
fn set_pulse_fx(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let [l, s, d] = a else {
        return Err(ErrorKind::BadType(
            "USAGE: <hudelem> SetPulseFX( <speed>, <decayStart>, <decayDuration> );",
        ));
    };
    let mut t = [0; 3];
    for (o, v) in t.iter_mut().zip([l, s, d]) {
        *o = int(Some(v), "setPulseFX takes times")?;
        if *o < 0 {
            return Err(ErrorKind::BadType("setPulseFX takes times of 0 or more"));
        }
    }
    let owner = host.ents.get(id).and_then(|e| e.hud).map_or(0, |h| h.owner) as usize;
    let sound = match host.cod4_mut().clients.get_mut(owner) {
        Some(c) => {
            c.hud_sound_id = (c.hud_sound_id + 1) % 32;
            if c.hud_sound_id == 0 {
                c.hud_sound_id = 1;
            }
            c.hud_sound_id
        }
        None => 0,
    };
    let now = host.level_time_ms;
    super::hud::with_hud(host, id, |h| {
        h.pulse = [now, t[0], t[1], t[2]];
        h.sound_id = sound;
    })?;
    Ok(Value::Undefined)
}

/// `self shellShock(name, duration)` (`GScr_ShellShock`, `cod4_lnxded`
/// 0x80c4a36): the shared slot scan and duration over configstrings
/// 1955.. (`super::client::shell_shock`), then on a live player
/// `PMF_SHELLSHOCKED` (0x10000), which the end frame sets
/// (docs/research/cod4-combat.md 10.8).
fn shell_shock(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    super::client::shell_shock(host, cx, recv, a)?;
    let slot = client_receiver(host, recv)?.index();
    if host.clients[slot].vitals.health > 0 {
        host.cod4_mut().clients[slot].shock_pmf = Some(true);
    }
    Ok(Value::Undefined)
}

/// `self setNormalHealth(frac)` on a player (`ScrCmd_SetNormalHealth`,
/// `cod4_lnxded` 0x80c3aa2): `frac` capped at 1, the health
/// `floor(maxHealth * frac + 0.5)` when that is above 0 (else the console's
/// "ERROR: Cannot setnormalhealth to 0 or below."), and `J "0"` to the
/// player. `_healthoverlay.gsc`'s regeneration runs on it.
fn set_normal_health(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let to = client_receiver(host, recv)?;
    let slot = to.index();
    let frac = match a.first() {
        Some(Value::Int(i)) => *i as f32,
        Some(Value::Float(f)) => *f,
        _ => return Err(ErrorKind::BadType("setNormalHealth takes a number")),
    }
    .min(1.0);
    let max = host.clients[slot].vitals.max_health;
    let health = (f64::from(max) * f64::from(frac) + 0.5).floor() as i32;
    host.client_commands.push(to, "J \"0\"".into());
    if health > 0 {
        host.clients[slot].vitals.health = health;
    }
    Ok(Value::Undefined)
}

/// `self stopShellShock()` (0x80c49cc): the three fields 0 and
/// `PMF_SHELLSHOCKED` off.
fn stop_shell_shock(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    super::client::stop_shell_shock(host, cx, recv, a)?;
    let slot = client_receiver(host, recv)?.index();
    host.cod4_mut().clients[slot].shock_pmf = Some(false);
    Ok(Value::Undefined)
}

use crate::game::cod4::weapons::{self as w4, Loadout};

/// A weapon name argument's number; `None` for a name never registered
/// (`G_GetWeaponIndexForName` past the level's start finds no weapon, 0).
pub(super) fn weapon_arg(
    host: &GameHost,
    cx: &Cx,
    v: Option<&Value>,
) -> Result<Option<usize>, ErrorKind> {
    let Some(&Value::String(n)) = v else {
        return Err(ErrorKind::BadType("takes a weapon name"));
    };
    Ok(host.cod4().table.index(cx.resolve(n)))
}

/// A registered weapon's number, else retail's `unknown weapon` error.
fn known_weapon(host: &GameHost, cx: &Cx, v: Option<&Value>) -> Result<usize, ErrorKind> {
    weapon_arg(host, cx, v)?.ok_or_else(|| {
        let name = match v {
            Some(&Value::String(n)) => cx.resolve(n).to_string(),
            _ => String::new(),
        };
        ErrorKind::Custom(format!("unknown weapon '{name}'"))
    })
}

/// The receiver's slot, its loadout taken out for the call, and the
/// weapon table.
pub(super) fn with_loadout<R>(
    host: &mut GameHost,
    slot: usize,
    f: impl FnOnce(&mut Loadout, &w4::Cod4Weapons) -> R,
) -> R {
    let c4 = host.cod4_mut();
    let mut l = std::mem::take(&mut c4.clients[slot].loadout);
    let r = f(&mut l, &c4.table);
    c4.clients[slot].loadout = l;
    r
}

/// `self giveWeapon(name[, model])`: the weapon and its alt chain on
/// `gunModel[model]` (the camo, [`w4::WeaponDef4::camo`]), the equipped
/// offhand when it is the first (`C <index>`, the capture's `C 40`), a `J
/// "1"` and its start ammo (the capture's 60 + 30 for `m16_gl_mp`, 30 + 15
/// for `beretta_mp`).
fn give_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let to = client_receiver(host, recv)?;
    let slot = to.index();
    let Some(weapon) = weapon_arg(host, cx, a.first())? else {
        return Ok(Value::Undefined);
    };
    let model = match a.get(1) {
        Some(&Value::Int(m)) => m.into(),
        _ => 0,
    };
    let given = with_loadout(host, slot, |l, t| {
        let had = l.has(weapon);
        let camo = t.get(weapon).map_or(0, |d| d.camo(model));
        let given = l.give(t, weapon, camo);
        if given.is_some() {
            l.initialize_ammo(t, weapon, had);
        }
        given
    });
    if let Some(offhand) = given {
        if let Some(o) = offhand {
            host.client_commands.push(to, format!("C {o}"));
        }
        host.client_commands.push(to, "J \"1\"".into());
    }
    Ok(Value::Undefined)
}

fn take_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    if let Some(weapon) = weapon_arg(host, cx, a.first())? {
        with_loadout(host, slot, |l, t| l.take(t, weapon));
    }
    Ok(Value::Undefined)
}

/// `self takeAllWeapons()`: every weapon, the current and offhand ones.
fn take_all_weapons(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    with_loadout(host, slot, |l, t| {
        let held: Vec<usize> = l.held().collect();
        for w in held {
            l.take(t, w);
        }
    });
    Ok(Value::Undefined)
}

/// `self setSpawnWeapon(name)`: a held weapon becomes `ps.weapon`, its
/// `weaponold` bit set, and the client is told `a <index>` (the capture's
/// `a 27`).
fn set_spawn_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let to = client_receiver(host, recv)?;
    let slot = to.index();
    let Some(weapon) = weapon_arg(host, cx, a.first())? else {
        return Ok(Value::Undefined);
    };
    let ok = with_loadout(host, slot, |l, _| {
        let ok = l.has(weapon);
        if ok {
            l.weapon = weapon;
            l.set_old(weapon);
        }
        ok
    });
    if ok {
        host.client_commands.push(to, format!("a {weapon}"));
    }
    Ok(Value::Undefined)
}

/// `self switchToWeapon(name)`: `a <index>` for a held weapon, true; the
/// client's own switch moves `ps.weapon`.
fn switch_to_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let to = client_receiver(host, recv)?;
    let slot = to.index();
    let weapon = known_weapon(host, cx, a.first())?;
    let held = host.cod4().clients[slot].loadout.has(weapon);
    if held {
        host.client_commands.push(to, format!("a {weapon}"));
    }
    Ok(Value::Int(i32::from(held)))
}

/// `self switchToOffhand(name)`: a held offhand becomes the equipped one,
/// `C <index>`; true when held.
fn switch_to_offhand(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let to = client_receiver(host, recv)?;
    let slot = to.index();
    let weapon = known_weapon(host, cx, a.first())?;
    let held = host.cod4().clients[slot].loadout.has(weapon);
    if held {
        host.cod4_mut().clients[slot].loadout.offhand = weapon;
        host.client_commands.push(to, format!("C {weapon}"));
    }
    Ok(Value::Int(i32::from(held)))
}

fn has_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let held =
        weapon_arg(host, cx, a.first())?.is_some_and(|w| host.cod4().clients[slot].loadout.has(w));
    Ok(Value::Int(i32::from(held)))
}

/// A weapon number's name, `"none"` for 0.
fn weapon_name(host: &GameHost, cx: &mut Cx, w: usize) -> Value {
    let name = host
        .cod4()
        .table
        .get(w)
        .map_or("none".to_string(), |d| d.name.clone());
    Value::String(cx.intern_exact(&name))
}

fn get_current_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let w = host.cod4().clients[slot].loadout.weapon;
    Ok(weapon_name(host, cx, w))
}

fn get_current_offhand(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let w = host.cod4().clients[slot].loadout.offhand;
    Ok(weapon_name(host, cx, w))
}

/// `self getWeaponsList()`: every held weapon's name, lowest number first.
fn get_weapons_list(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let held: Vec<usize> = host.cod4().clients[slot].loadout.held().collect();
    let out = cx.new_array();
    for (i, w) in held.into_iter().enumerate() {
        let v = weapon_name(host, cx, w);
        cx.set_index(out, vcod_gsc::ArrayKey::Int(i as i32), v);
    }
    Ok(Value::Array(out))
}

/// `self setWeaponAmmoClip(name, count)`: the clip, clamped to 0 and the
/// clip size.
fn set_weapon_ammo_clip(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let n = int(a.get(1), "setWeaponAmmoClip takes a count")?;
    if let Some(w) = weapon_arg(host, cx, a.first())? {
        with_loadout(host, slot, |l, t| {
            let d = &t.defs[w];
            l.ammoclip[d.clip_slot] = n.clamp(0, d.clip_size.max(0));
        });
    }
    Ok(Value::Undefined)
}

/// `self setWeaponAmmoStock(name, count)`: the ammo slot (a clip-only
/// weapon's clip), clamped to 0 and the player's max.
fn set_weapon_ammo_stock(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let n = int(a.get(1), "setWeaponAmmoStock takes a count")?;
    if let Some(w) = weapon_arg(host, cx, a.first())? {
        with_loadout(host, slot, |l, t| {
            let d = &t.defs[w];
            if d.clip_only {
                l.ammoclip[d.clip_slot] = n.min(d.clip_size).max(0);
            } else {
                let max = l.ammo_max(t, w);
                l.ammo[d.ammo_slot] = n.min(max).max(0);
            }
        });
    }
    Ok(Value::Undefined)
}

fn get_weapon_ammo_clip(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let n = weapon_arg(host, cx, a.first())?.map_or(0, |w| {
        let c4 = host.cod4();
        c4.clients[slot].loadout.ammoclip[c4.table.defs[w].clip_slot]
    });
    Ok(Value::Int(n))
}

fn get_weapon_ammo_stock(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let n = weapon_arg(host, cx, a.first())?.map_or(0, |w| {
        let c4 = host.cod4();
        let d = &c4.table.defs[w];
        let l = &c4.clients[slot].loadout;
        if d.clip_only {
            l.ammoclip[d.clip_slot]
        } else {
            l.ammo[d.ammo_slot]
        }
    });
    Ok(Value::Int(n))
}

/// `self giveMaxAmmo(name)`: a held weapon's ammo slot up to the max.
fn give_max_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    if let Some(w) = weapon_arg(host, cx, a.first())? {
        with_loadout(host, slot, |l, t| {
            if l.has(w) {
                let give = l.ammo_max(t, w) - l.ammo[t.defs[w].ammo_slot];
                if give > 0 {
                    l.add_ammo(t, w, give, false);
                }
            }
        });
    }
    Ok(Value::Undefined)
}

fn give_start_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    if let Some(w) = weapon_arg(host, cx, a.first())? {
        with_loadout(host, slot, |l, t| {
            if l.has(w) {
                l.initialize_ammo(t, w, false);
            }
        });
    }
    Ok(Value::Undefined)
}

/// `self setActionSlot(n, "weapon"|"altMode"|"nightvision"|"", [weapon])`:
/// `ps.actionSlotType[n - 1]` (the capture's 3 for `nightvision`, 2 for
/// `altMode`) and a weapon's number in its param.
fn set_action_slot(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let n = int(a.first(), "setActionSlot takes a slot")?;
    if !(1..=4).contains(&n) {
        return Err(ErrorKind::Custom(format!(
            "Invalid slot ({n}) given, expecting 1 - 4\n"
        )));
    }
    let i = (n - 1) as usize;
    let Some(&Value::String(kind)) = a.get(1) else {
        return Err(ErrorKind::BadType("setActionSlot takes an option"));
    };
    let kind = cx.resolve(kind).to_ascii_lowercase();
    let (ty, param) = match kind.as_str() {
        "weapon" => {
            let w = weapon_arg(host, cx, a.get(2))?.ok_or_else(|| {
                ErrorKind::Custom("Unknown weapon name given to setActionSlot.\n".into())
            })?;
            (w4::ACTIONSLOT_WEAPON, w as u32)
        }
        "altmode" => (w4::ACTIONSLOT_ALTWEAPON, 0),
        "nightvision" => (w4::ACTIONSLOT_NIGHTVISION, 0),
        "" => (w4::ACTIONSLOT_NOTHING, 0),
        _ => {
            return Err(ErrorKind::Custom(
                "Invalid option: expected \"weapon\", \"altweapon\", or \"nightvision\".\n".into(),
            ));
        }
    };
    let l = &mut host.cod4_mut().clients[slot].loadout;
    l.action_slot_type[i] = ty;
    if ty == w4::ACTIONSLOT_WEAPON {
        l.action_slot_param[i] = param;
    }
    Ok(Value::Undefined)
}

/// `self setOffhandSecondaryClass("smoke"|"flash")`: `ps.offhandSecondary`
/// 0 or 1.
fn set_offhand_secondary_class(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let Some(&Value::String(kind)) = a.first() else {
        return Err(ErrorKind::Custom(
            "Incorrect number of parameters.\n".into(),
        ));
    };
    let v = match cx.resolve(kind) {
        "smoke" => 0,
        "flash" => 1,
        _ => {
            return Err(ErrorKind::Custom(
                "Must specify either 'smoke' or 'flash' class to set secondary offhand to.\n"
                    .into(),
            ));
        }
    };
    host.cod4_mut().clients[slot].loadout.offhand_secondary = v;
    Ok(Value::Undefined)
}

fn get_offhand_secondary_class(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let v = host.cod4().clients[slot].loadout.offhand_secondary;
    let name = if v == 1 { "flash" } else { "smoke" };
    Ok(Value::String(cx.intern_exact(name)))
}

/// `self setMoveSpeedScale(scale)`: `ps.moveSpeedScaleMultiplier` (the
/// capture's 0.95 for a rifle class).
fn set_move_speed_scale(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let v = match a.first() {
        Some(&Value::Int(i)) => i as f32,
        Some(&Value::Float(f)) => f,
        _ => return Err(ErrorKind::BadType("setMoveSpeedScale takes a number")),
    };
    host.cod4_mut().clients[slot].loadout.move_speed_scale = v;
    Ok(Value::Undefined)
}

/// One def field of a named weapon; an unknown name is retail's error.
fn def_of<R>(
    host: &GameHost,
    cx: &Cx,
    a: &[Value],
    f: impl FnOnce(&w4::WeaponDef4) -> R,
) -> Result<R, ErrorKind> {
    let w = known_weapon(host, cx, a.first())?;
    Ok(f(&host.cod4().table.defs[w]))
}

fn is_weapon_clip_only(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(i32::from(def_of(host, cx, a, |d| d.clip_only)?)))
}

/// `weaponClass(name)` (0x80bd81a): `"none"` for a name no weapon has,
/// `"none"` itself included, never an error (cod4-combat.md 10.10).
fn weapon_class(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = match weapon_arg(host, cx, a.first())? {
        Some(w) if w != 0 => {
            let c = host.cod4().table.defs[w].weap_class;
            w4::WEAP_CLASSES.get(c as usize).copied().unwrap_or("")
        }
        _ => "none",
    };
    Ok(Value::String(cx.intern_exact(name)))
}

fn weapon_clip_size(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(def_of(host, cx, a, |d| d.clip_size)?))
}

fn weapon_max_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(def_of(host, cx, a, |d| d.max_ammo)?))
}

fn weapon_start_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(def_of(host, cx, a, |d| d.start_ammo)?))
}

/// `weaponInventoryType(name)`: `primary`, `offhand`, `item` or `altmode`.
fn weapon_inventory_type(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let t = def_of(host, cx, a, |d| d.inventory_type)?;
    let name = ["primary", "offhand", "item", "altmode"]
        .get(t as usize)
        .copied()
        .unwrap_or("");
    Ok(Value::String(cx.intern_exact(name)))
}

fn weapon_alt_weapon_name(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let alt = def_of(host, cx, a, |d| d.alt)?;
    Ok(weapon_name(host, cx, alt))
}

/// `bg_perkNames`, the perk bits' order: the 20-row pointer table at
/// cod4_lnxded 0x820de40. VERIFIED. A name past it (`specialty_null`, the
/// `specialty_weapon_*` class slots) sets no bit.
pub const PERKS: [&str; 20] = [
    "specialty_gpsjammer",
    "specialty_bulletaccuracy",
    "specialty_fastreload",
    "specialty_rof",
    "specialty_holdbreath",
    "specialty_bulletpenetration",
    "specialty_grenadepulldeath",
    "specialty_pistoldeath",
    "specialty_quieter",
    "specialty_parabolic",
    "specialty_longersprint",
    "specialty_detectexplosive",
    "specialty_explosivedamage",
    "specialty_exposeenemy",
    "specialty_bulletdamage",
    "specialty_extraammo",
    "specialty_twoprimaries",
    "specialty_armorvest",
    "specialty_fraggrenade",
    "specialty_specialgrenade",
];

/// The receiver's slot and the perk's bit, `None` for a name off the table.
fn perk_arg(
    host: &GameHost,
    cx: &Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<(usize, Option<u32>), ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let Some(&Value::String(name)) = a.first() else {
        return Err(ErrorKind::BadType("takes a perk name"));
    };
    let name = cx.resolve(name);
    let bit = PERKS
        .iter()
        .position(|p| p.eq_ignore_ascii_case(name))
        .map(|i| 1u32 << i);
    Ok((slot, bit))
}

/// `self pingPlayer()` (`cod4_lnxded` 0x80a8810): `eFlags` 0x400000 for
/// 3 s, the chat flash a teammate's compass shows (cod4-hud.md 15.9).
fn ping_player(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let until = host.level_time_ms + 3000;
    host.cod4_mut().clients[slot].compass_ping_until = until;
    Ok(Value::Undefined)
}

/// `self setPerk(name)`: the playerstate's `perks` bit.
fn set_perk(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (slot, bit) = perk_arg(host, cx, recv, a)?;
    host.cod4_mut().clients[slot].perks |= bit.unwrap_or(0);
    Ok(Value::Undefined)
}

fn unset_perk(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (slot, bit) = perk_arg(host, cx, recv, a)?;
    host.cod4_mut().clients[slot].perks &= !bit.unwrap_or(0);
    Ok(Value::Undefined)
}

fn has_perk(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (slot, bit) = perk_arg(host, cx, recv, a)?;
    let on = bit.is_some_and(|b| host.cod4().clients[slot].perks & b != 0);
    Ok(Value::Int(i32::from(on)))
}

fn clear_perks(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    host.cod4_mut().clients[slot].perks = 0;
    Ok(Value::Undefined)
}

/// `addTestClient()`: `undefined`, retail's answer with no free slot. The
/// CoD4 engine installs no test client (cod4-server-gaps.md, Ranked 10);
/// the shared handler is CoD2's and panicked on a CoD4 host (BUG-064).
fn add_test_client(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `setTeamScore(team, score)`: the score kept and `H <score>` (allies) or
/// `G <score>` (axis) to everyone; no configstring, unlike CoD1's
/// INFERRED (the client's `G`/`H` handlers set the team scores).
fn set_team_score(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(&Value::String(team)) = a.first() else {
        return Err(ErrorKind::BadType("setTeamScore takes a team"));
    };
    let (i, letter) = match cx.resolve(team) {
        "allies" => (1, 'H'),
        "axis" => (0, 'G'),
        t => {
            return Err(ErrorKind::Custom(format!(
                "Illegal team string '{t}'. Must be allies, or axis."
            )));
        }
    };
    let n = match a.get(1) {
        Some(&Value::Int(n)) => n,
        Some(&Value::Float(f)) => f as i32,
        _ => return Err(ErrorKind::BadType("setTeamScore takes a number")),
    };
    host.team_scores[i] = n;
    host.client_commands.push(
        crate::game::commands::Recipient::All,
        format!("{letter} {n}"),
    );
    host.ranks_dirty = true;
    Ok(Value::Undefined)
}

/// `self setClientDvars(name, value, ...)`: one reliable `v` with every pair
/// as ` <name> "<value>"`, a `"` in a value turned `'` (the capture's `v
/// cg_hudGrenadeIconHeight "25" cg_hudGrenadeIconWidth "25" ..`). An odd
/// count is a script error.
fn set_client_dvars(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    if !a.len().is_multiple_of(2) {
        return Err(ErrorKind::Custom(
            "Not enough parameters to setclientdvar() - must be an even number of parameters (dvar, value, dvar, value, etc.)".into(),
        ));
    }
    let mut out = String::from("v");
    for pair in a.chunks(2) {
        let Value::String(name) = pair[0] else {
            return Err(ErrorKind::BadType("setClientDvars takes dvar names"));
        };
        let value = cx
            .format_number(pair[1])
            .ok_or(ErrorKind::BadType("setClientDvars takes renderable values"))?
            .replace('"', "'");
        out.push_str(&format!(" {} \"{value}\"", cx.resolve(name)));
    }
    host.client_commands.push(slot, out);
    Ok(Value::Undefined)
}

/// `self setRank(rank[, prestige])`: the client state's `rank` and
/// `prestige`, each below 256.
fn set_rank(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?.index();
    let int = |v: Option<&Value>| match v {
        Some(&Value::Int(i)) => Ok(i),
        _ => Err(ErrorKind::BadType("setRank takes ints")),
    };
    let rank = int(a.first())?;
    if !(0..256).contains(&rank) {
        return Err(ErrorKind::Custom(format!(
            "'{rank}' is an illegal rank value.  Must be less than 256.\n"
        )));
    }
    let prestige = match a.get(1) {
        None => None,
        v => Some(int(v)?),
    };
    if prestige.is_some_and(|p| !(0..256).contains(&p)) {
        return Err(ErrorKind::Custom(format!(
            "'{}' is an illegal prestige value.  Must be less than 256.\n",
            prestige.unwrap()
        )));
    }
    let c = &mut host.cod4_mut().clients[slot];
    c.rank = rank as u8;
    if let Some(p) = prestige {
        c.prestige = p as u8;
    }
    Ok(Value::Undefined)
}

/// `self closeInGameMenu()`: the reliable `L` (the capture's `u`, `L` pair
/// ahead of each menu answer's own commands).
fn close_in_game_menu(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_commands.push(slot, "L".into());
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::super::cod4::tests::cod4_host;
    use super::*;

    /// The perk bits follow `bg_perkNames` case-blind; a name off the
    /// table sets nothing and reads as unset.
    #[test]
    fn perks_set_their_table_bits() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, me) = cod4_host(cx);
            let p = |cx: &mut Cx, n: &str| [Value::String(cx.intern_exact(n))];
            let reload = p(cx, "Specialty_FastReload");
            let null = p(cx, "specialty_null");
            let vest = p(cx, "specialty_armorvest");
            set_perk(&mut host, cx, me, &reload).unwrap();
            set_perk(&mut host, cx, me, &vest).unwrap();
            set_perk(&mut host, cx, me, &null).unwrap();
            assert_eq!(host.cod4().clients[0].perks, 1 << 2 | 1 << 17);
            assert_eq!(has_perk(&mut host, cx, me, &reload), Ok(Value::Int(1)));
            assert_eq!(has_perk(&mut host, cx, me, &null), Ok(Value::Int(0)));
            unset_perk(&mut host, cx, me, &reload).unwrap();
            assert_eq!(host.cod4().clients[0].perks, 1 << 17);
            clear_perks(&mut host, cx, me, &[]).unwrap();
            assert_eq!(host.cod4().clients[0].perks, 0);
        });
    }

    /// `setRank`'s rank and prestige stay under 256; a bad prestige leaves
    /// the rank as it was.
    #[test]
    fn set_rank_keeps_rank_and_prestige_under_256() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, me) = cod4_host(cx);
            let i = Value::Int;
            set_rank(&mut host, cx, me, &[i(255), i(3)]).unwrap();
            set_rank(&mut host, cx, me, &[i(12)]).unwrap();
            let c = &host.cod4().clients[0];
            assert_eq!((c.rank, c.prestige), (12, 3));
            assert!(set_rank(&mut host, cx, me, &[i(256)]).is_err());
            assert!(set_rank(&mut host, cx, me, &[i(-1)]).is_err());
            assert!(set_rank(&mut host, cx, me, &[i(40), i(256)]).is_err());
            let c = &host.cod4().clients[0];
            assert_eq!((c.rank, c.prestige), (12, 3));
        });
    }

    /// `setClientDvars` sends its pairs in one `v`, a `"` in a value as
    /// `'`; an odd count is an error.
    #[test]
    fn set_client_dvars_pairs_its_arguments() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, me) = cod4_host(cx);
            let a = [
                Value::String(cx.intern_exact("cg_hudGrenadeIconHeight")),
                Value::Int(25),
                Value::String(cx.intern_exact("ui_x")),
                Value::String(cx.intern_exact("say \"hi\"")),
            ];
            set_client_dvars(&mut host, cx, me, &a).unwrap();
            assert!(set_client_dvars(&mut host, cx, me, &a[..3]).is_err());
            let sent: Vec<&str> = host.client_commands.iter().map(|c| c.1.as_str()).collect();
            assert_eq!(sent, ["v cg_hudGrenadeIconHeight \"25\" ui_x \"say 'hi'\""]);
        });
    }

    /// `setActionSlot`: slots 1 to 4; `weapon` keeps the weapon's number,
    /// `altMode` and `nightvision` their types, `""` clears; an unknown
    /// weapon or option is an error.
    #[test]
    fn set_action_slot_takes_its_four_slots_and_options() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, me) = cod4_host(cx);
            let m16 = super::super::cod4::register_weapon(&mut host, "m16_mp").unwrap();
            let v = |cx: &mut Cx, t: &str| Value::String(cx.intern_exact(t));
            let (weapon, alt, night, none) = (
                v(cx, "weapon"),
                v(cx, "altMode"),
                v(cx, "nightvision"),
                v(cx, ""),
            );
            let (gun, nope, odd) = (v(cx, "m16_mp"), v(cx, "nope_mp"), v(cx, "zoom"));
            let i = Value::Int;
            set_action_slot(&mut host, cx, me, &[i(1), night]).unwrap();
            set_action_slot(&mut host, cx, me, &[i(2), alt]).unwrap();
            set_action_slot(&mut host, cx, me, &[i(4), weapon, gun]).unwrap();
            let l = &host.cod4().clients[0].loadout;
            assert_eq!(l.action_slot_type[..], [3, 2, 0, 1]);
            assert_eq!(l.action_slot_param[3], m16 as u32);
            set_action_slot(&mut host, cx, me, &[i(1), none]).unwrap();
            assert_eq!(host.cod4().clients[0].loadout.action_slot_type[0], 0);
            for bad in [
                vec![i(0), night],
                vec![i(5), night],
                vec![i(3), weapon, nope],
                vec![i(3), odd],
            ] {
                assert!(set_action_slot(&mut host, cx, me, &bad).is_err(), "{bad:?}");
            }
        });
    }
}
