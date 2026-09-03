//! The per-client builtins the stock join sequence reaches: the two server
//! commands it sends, the three that arm a spawning player's weapons, and
//! the spawn-point test `_spawnlogic` filters candidates with.
//!
//! `setClientCvar` and `openMenu` write no game state: each renders one
//! reliable command and queues it on `GameHost::client_commands`, which
//! `Server` drains after `run_frame`. A builtin cannot reach the netchan,
//! and must not reenter the VM, so queueing is the only route out. Both wire
//! formats are measured, not inferred: the `v %s "%s"` and `t %i` format
//! strings are in `game.mp.i386.so`, and
//! `docs/research/cod11-hud-protocol.md` section 0.1 carries the live
//! capture of a stock dm join that produced them.

use crate::configstrings::{script_menu_index, weapon_index, CsRange};
use crate::game::builtins::entity::entity_receiver;
use crate::game::entity::ThinkFn;
use crate::game::host::{GameHost, WeaponOp};
use crate::weapons::weapon_slot;
use vcod_common::pmove;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("setclientcvar", set_client_cvar),
    ("openmenu", open_menu),
    ("giveweapon", give_weapon),
    ("givemaxammo", give_max_ammo),
    ("setspawnweapon", set_spawn_weapon),
    ("switchtoweapon", switch_to_weapon),
    ("takeallweapons", take_all_weapons),
    ("getweaponslotweapon", get_weapon_slot_weapon),
    ("setweaponslotammo", set_weapon_slot_ammo),
    ("setweaponslotclipammo", set_weapon_slot_clip_ammo),
    ("positionwouldtelefrag", position_would_telefrag),
    ("setviewmodel", set_view_model),
    ("getviewmodel", get_view_model),
    ("usebuttonpressed", use_button_pressed),
    ("getcurrentweapon", get_current_weapon),
    ("cloneplayer", clone_player),
    ("dropitem", drop_item),
    ("closemenu", close_menu),
];

/// How long a dropped weapon lives before the server frees it.
///
/// A deliberate divergence, and the only one in this file that retail
/// contradicts outright: retail's `Drop_Weapon` (`.so` 0x4dd40) hands the
/// entity to `LaunchItem` (0x4db98), whose only think is
/// `DroppedItemClearOwner` a second out, so a dropped weapon lives until
/// somebody picks it up. Nothing in the module frees one on a timer (the two
/// `0x7530` immediates in `.text` are in `Cmd_CallVote_f` and `fire_rocket`).
/// Pickup on touch is not this stage, so an item with retail's lifetime would
/// never leave; the timer bounds the entity table until the touch path exists
/// to replace it.
const DROPPED_ITEM_MS: i32 = 30_000;

/// `self useButtonPressed()`: whether the client's last usercmd held the
/// use button, which every stock `respawn()` loop polls. `Server` mirrors
/// the buttons onto the host before the frame.
pub fn use_button_pressed(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let held = host.client_buttons[slot] & vcod_common::net::msg::BUTTON_USE != 0;
    Ok(Value::Int(i32::from(held)))
}

/// `self getCurrentWeapon()` (`PlayerCmd_getCurrentWeapon`): the name of the
/// weapon in `ps.weapon`, or `"none"` when the slot holds nothing. Every
/// stock death path reads it to name the weapon the corpse drops.
///
/// A player on a ladder reports `"none"`: the holster writes 0 into
/// `ps.weapon`, and 0 is what retail reads back here too.
pub fn get_current_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let index = host.client_weapons[slot].current as usize;
    let name = crate::items::item_name(index).unwrap_or("none");
    Ok(Value::String(cx.intern_exact(name)))
}

/// `self cloneplayer()` (`.so` 0x4450c): the corpse the gametype leaves
/// behind, into the next body-queue slot
/// (`docs/research/cod11-combat.md` section 5.2). The state copied is the
/// mirror `Server::replay_moves` wrote for this client before the script
/// frame, and the queue re-reads it from the sim once more at the snapshot
/// build, so a death animation raised after this call still reaches the wire.
///
/// Returns `Value::Undefined` rather than the body entity: the body queue is
/// not in the object table, so there is no `EntId` to hand out, and every
/// stock gametype assigns the result without ever reading it back.
pub fn clone_player(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(state) = host.client_entity_states[slot].clone() else {
        // A client with no sim: connected, never entered the world.
        return Ok(Value::Undefined);
    };
    let now = host.level_time_ms;
    host.bodies.push(
        state,
        Some(slot),
        now,
        &vcod_common::net::protocol::PROTOCOL_V1,
    );
    Ok(Value::Undefined)
}

/// `self dropItem(name)` (`.so` 0x43684): the named weapon on the ground
/// where the player stands. Retail resolves the name to a weapon index and
/// calls `Drop_Weapon`, which spawns a `bg_itemlist` entity through
/// `LaunchItem`; here the entity is spawned the way
/// `spawn_entities_from_string` spawns a map's `mpweapon_*`, so it reaches the
/// wire as the same `ET_ITEM` (`crate::game::wire`). `weaponinfo` carries the
/// weapon, which is what `kind_of` reads; the classname only has to open with
/// `mpweapon_` for it to look there.
///
/// Two of retail's steps are left out. It takes the weapon off the player
/// (`BG_TakePlayerWeapon`), which the stock death path does not need -- the
/// respawn re-gives the loadout -- and would disarm a player the gametype
/// meant to keep armed. And pickup on touch does not exist yet, which is what
/// `DROPPED_ITEM_MS` stands in for.
///
/// A name no weapon file backs raises, the same reading `weapon_argument`
/// takes for every other weapon builtin.
pub fn drop_item(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (name, _) = weapon_argument(cx, args)?;
    let origin = {
        let field = cx.intern_folded("origin");
        match host.get_field(cx, EntId(slot as u32), field) {
            Value::Vector(v) => v,
            _ => [0.0; 3],
        }
    };
    let id = host.ents.spawn(cx)?;
    let classname = crate::game::spawn::radiant_name(&name).unwrap_or("mpweapon_dropped");
    for (field, value) in [
        ("classname", Value::String(cx.intern_exact(classname))),
        ("weaponinfo", Value::String(cx.intern_exact(&name))),
        ("origin", Value::Vector(origin)),
    ] {
        let atom = cx.intern_folded(field);
        host.set_field(cx, id, atom, value)?;
    }
    host.register_item(&name);
    host.ents
        .schedule(id, ThinkFn::Free, host.level_time_ms + DROPPED_ITEM_MS);
    Ok(Value::Undefined)
}

/// `self closeMenu()` (`.so` 0x45574): the reliable command `u`, with no
/// argument -- the whole format string at 0x73204 is the one letter. The
/// client's handler is `0x3002de60`, "close script menus"
/// (`docs/research/cod11-hud-protocol.md` section 0).
pub fn close_menu(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_commands.push((slot, "u".to_string()));
    Ok(Value::Undefined)
}

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The model configstring slot `ps.viewmodelIndex` counts from. `G_ModelIndex`
/// (0x66ed8) hands out 1-based offsets from it, so index 1 is the first slot
/// of `CsRange::Model`.
fn model_index_base() -> usize {
    CsRange::Model.bounds().0 - 1
}

/// The receiver every builtin here but `positionWouldTelefrag` needs: an
/// entity with a `gclient_t`, which is retail's own `entity %i is not a
/// player` check (0x44703, 0x45413). The client's slot is its entity number,
/// and that is what a reliable command is addressed to.
pub(crate) fn client_receiver(host: &GameHost, recv: Option<Target>) -> Result<usize, ErrorKind> {
    let id = entity_receiver(recv)?;
    match host.ents.get(id) {
        Some(e) if e.client.is_some() => Ok(id.0 as usize),
        _ => Err(ErrorKind::BadType("that entity is not a player")),
    }
}

/// The separator a localized string carries between its key and its
/// substitution arguments, of which `setClientCvar`'s value has none, so it
/// ends up trailing. Measured: retail sent `v cg_objectiveText
/// "DM_KILL_OTHER_PLAYERS\x15"` for `setClientCvar("cg_objectiveText",
/// &"DM_KILL_OTHER_PLAYERS")` on both gate maps.
const LOCALIZED_SEP: char = '\u{15}';

/// `self setClientCvar(name, value)`: the reliable command `v <name>
/// "<value>"` (0x446e0, format string `v %s "%s"`). The name is never
/// quoted and the value always is, so retail rewrites a `"` inside the value
/// as `'` (0x447b2) rather than letting it close the argument early. A
/// localized value takes the `Scr_GetType == 2` branch (0x44750) into
/// `Scr_ConstructMessageString` (0x44765) instead of the plain string read,
/// which is where `LOCALIZED_SEP` comes from.
pub fn set_client_cvar(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (Some(Value::String(name)), Some(&value)) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType(
            "setClientCvar takes a cvar name and a value",
        ));
    };
    let name = cx.resolve(*name).to_string();
    let mut value = cx
        .format_number(value)
        .ok_or(ErrorKind::BadType("setClientCvar takes a renderable value"))?
        .replace('"', "'");
    if matches!(args.get(1), Some(Value::Localized(_))) {
        value.push(LOCALIZED_SEP);
    }
    host.client_commands
        .push((slot, format!("v {name} \"{value}\"")));
    Ok(Value::Undefined)
}

/// `self openMenu(menu)`: the reliable command `t <index>` (0x453f4, format
/// string `t %i`), where the index is the menu's slot within
/// `CsRange::Menu`, resolved by name through `GScr_GetScriptMenuIndex`. The
/// menu name itself never goes on the wire. Returns 1, as retail does for a
/// connected client (`Scr_AddInt(1)`, 0x45496).
pub fn open_menu(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(Value::String(menu)) = args.first() else {
        return Err(ErrorKind::BadType("openMenu takes a menu name"));
    };
    let menu = cx.resolve(*menu).to_string();
    let index = script_menu_index(&host.configstrings, &menu)
        .ok_or(ErrorKind::BadType("that menu was not precached"))?;
    host.client_commands.push((slot, format!("t {index}")));
    Ok(Value::Int(1))
}

/// The weapon name argument every weapon builtin takes, resolved to its
/// 1-based configstring 7 index.
///
/// Divergence, deliberate: retail narrows `BG_GetWeaponIndexForName`'s result
/// with `movzbl` and uses it with no zero check, so a name no weapon file
/// backs looks like it sets bit 0 and carries on (object model doc, section
/// 20). That reading is control flow, not a measurement, so this raises
/// instead of reproducing an inferred behaviour. The cost is that a typo kills
/// the calling thread where retail may keep it alive.
fn weapon_argument(cx: &Cx, args: &[Value]) -> Result<(String, usize), ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("that builtin takes a weapon name"));
    };
    let name = cx.resolve(*name).to_string();
    let index = weapon_index(&name).ok_or(ErrorKind::BadType("no such weapon"))?;
    Ok((name, index))
}

/// `self giveWeapon(name)` (`PlayerCmd_giveWeapon` 0x43020): hold the weapon,
/// fill the slot its weapon file's `weaponSlot` names, and load it with a
/// full clip and its `startAmmo` in reserve. Object model doc, section 20;
/// the ammo pair is RTCW's `Add_Ammo` shape, and every stock loadout calls
/// `giveMaxAmmo` right after, which is where the reserve the spawn capture
/// carries comes from.
///
/// Retail's `Can not give player weapon without having an empty weapon slot`
/// error is not reproduced: whether re-giving a held weapon counts as an
/// occupied slot is unmeasured, and a wrong guess kills the spawning thread.
pub fn give_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (name, index) = weapon_argument(cx, args)?;
    let weapon_slot = weapon_slot(host.fs.as_deref(), &name).unwrap_or(0);
    host.client_weapons[slot].give(index, weapon_slot);
    if let Some(def) = host.weapons.get(index) {
        let (clip, ammo) = (
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
            WeaponOp::SetAmmo {
                ammo_index: def.ammo_index,
                rounds: def.start_ammo as i16,
            },
        );
        host.client_weapon_ops.push((slot, clip));
        host.client_weapon_ops.push((slot, ammo));
    }
    Ok(Value::Undefined)
}

/// `self giveMaxAmmo(name)` (`PlayerCmd_giveMaxAmmo` 0x43134): the weapon's
/// `maxAmmo` in reserve and a full clip. Object model doc, section 20; the
/// numbers are the retail spawn capture's, `clip=3:7,6:3,10:15
/// ammo=3:56,10:400` for the stock allies loadout
/// (`crates/server/src/weapons.rs`, `the_first_index_handed_out_is_one`).
pub fn give_max_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (_, index) = weapon_argument(cx, args)?;
    if let Some(def) = host.weapons.get(index) {
        let (ammo, clip) = (
            WeaponOp::SetAmmo {
                ammo_index: def.ammo_index,
                rounds: def.max_ammo as i16,
            },
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
        host.client_weapon_ops.push((slot, ammo));
        host.client_weapon_ops.push((slot, clip));
    }
    Ok(Value::Undefined)
}

/// `self setSpawnWeapon(name)` (0x452a4): `ps.weapon = index`, for a weapon
/// the player already holds, with `ps.weaponstate` -- retail's other store --
/// back to ready. Object model doc, section 20.
pub fn set_spawn_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (_, index) = weapon_argument(cx, args)?;
    let w = &mut host.client_weapons[slot];
    if w.holds(index) {
        w.current = index as u8;
        host.client_weapon_ops
            .push((slot, WeaponOp::SetCurrent(index as u8)));
    }
    Ok(Value::Undefined)
}

/// `self switchToWeapon(name)` (`PlayerCmd_switchToWeapon`, player method 5):
/// the same change a usercmd's weapon byte asks for, so it goes through the
/// putaway and the raise rather than swapping the weapon in place (combat
/// doc, section 1.8). The host's own `current` is deliberately left alone:
/// the frame mirror copies it into `ps.weapon`, so writing the target here
/// would put the weapon in the player's hands ahead of its own drop.
///
/// A weapon the player does not hold raises. Retail's `PM_Weapon` would
/// disarm the player on the next frame instead (`COM_BitCheck` on
/// `ps.weapons`), which loses the name of the mistake.
pub fn switch_to_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (_, index) = weapon_argument(cx, args)?;
    if !host.client_weapons[slot].holds(index) {
        return Err(ErrorKind::BadType("that player does not hold that weapon"));
    }
    host.client_weapon_ops
        .push((slot, WeaponOp::SwitchTo(index as u8)));
    Ok(Value::Undefined)
}

/// `self takeAllWeapons()` (`PlayerCmd_takeAllWeapons`, player method 2):
/// the held bits and every slot cleared here, and the ammo and clip arrays
/// cleared in the sim by the op, since those are the playerstate's alone.
pub fn take_all_weapons(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_weapons[slot] = crate::weapons::PlayerWeapons::default();
    host.client_weapon_ops.push((slot, WeaponOp::TakeAll));
    Ok(Value::Undefined)
}

/// `self getWeaponSlotWeapon(slot)` (player method 32, 0x43cf4): the name of
/// the weapon standing in that slot, `"none"` for an empty one. The inverse
/// of the `weaponSlot` key a weapon file carries (object model doc, section
/// 20).
pub fn get_weapon_slot_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let client = client_receiver(host, recv)?;
    let slot = slot_argument(cx, args)?;
    let index = host.client_weapons[client].slots[slot] as usize;
    let name = crate::items::item_name(index).unwrap_or("none");
    Ok(Value::String(cx.intern_exact(name)))
}

/// `self setWeaponSlotAmmo(slot, rounds)` (player method 35, 0x44130): the
/// reserve of whatever weapon occupies the slot. Ammo is indexed by the
/// weapon's ammo *name*, not by the weapon, so two weapons sharing a name
/// share the reserve (docs/protocol-1.1.md, "How `ammo[]` and `ammoclip[]`
/// are indexed"). An empty slot names no weapon and so writes nothing.
pub fn set_weapon_slot_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (client, slot, rounds) = slot_and_rounds(host, cx, recv, args)?;
    let index = host.client_weapons[client].slots[slot] as usize;
    if let Some(def) = host.weapons.get(index) {
        let op = WeaponOp::SetAmmo {
            ammo_index: def.ammo_index,
            rounds,
        };
        host.client_weapon_ops.push((client, op));
    }
    Ok(Value::Undefined)
}

/// `self setWeaponSlotClipAmmo(slot, rounds)` (player method 37, 0x443e4):
/// the same for the clip, which has its own name table and its own index
/// space.
pub fn set_weapon_slot_clip_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (client, slot, rounds) = slot_and_rounds(host, cx, recv, args)?;
    let index = host.client_weapons[client].slots[slot] as usize;
    if let Some(def) = host.weapons.get(index) {
        let op = WeaponOp::SetClip {
            clip_index: def.clip_index,
            rounds,
        };
        host.client_weapon_ops.push((client, op));
    }
    Ok(Value::Undefined)
}

/// The receiver, slot and round count the two setters share.
fn slot_and_rounds(
    host: &GameHost,
    cx: &Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<(usize, usize, i16), ErrorKind> {
    let client = client_receiver(host, recv)?;
    let slot = slot_argument(cx, args)?;
    let rounds = match args.get(1) {
        Some(Value::Int(n)) => *n as i16,
        Some(Value::Float(f)) => *f as i16,
        _ => return Err(ErrorKind::BadType("that builtin takes a round count")),
    };
    Ok((client, slot, rounds))
}

/// The slot-name argument the three slot builtins take, as its index in
/// [`crate::weapons::SLOT_NAMES`]. `"none"` is index 0 and names no slot a
/// weapon can stand in, so it is refused with every other unknown name, the
/// way retail's `Unknown weaponslot name %s` is (object model doc, section
/// 20).
fn slot_argument(cx: &Cx, args: &[Value]) -> Result<usize, ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("that builtin takes a weapon slot name"));
    };
    let name = cx.resolve(*name);
    crate::weapons::SLOT_NAMES
        .iter()
        .position(|s| *s == name)
        .filter(|i| *i > 0)
        .ok_or(ErrorKind::BadType("no such weapon slot"))
}

/// `positionWouldTelefrag(origin)` (free function 96, 0x5a834): the player
/// bounding box at `origin` against every player already standing there.
/// Object model doc, section 20.
///
/// Three of retail's inputs are missing here, and all three err the same way.
/// Its area query returns only entities linked into the world, and nothing
/// links a client entity yet. Its `pm_type <= 5` filter has nothing to read,
/// `pm_type` living in the sim rather than the object table, so a dead or
/// spectating player is not told apart from a live one. And a client entity's
/// `origin` field is only written by `spawn`, never synced from the sim, so
/// this tests where each client last spawned rather than where it now is.
/// Each makes the answer 1 where retail's is 0, never the reverse, so a spawn
/// point is refused a little too eagerly rather than handed out on top of
/// somebody.
pub fn position_would_telefrag(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(at)) = args.first() else {
        return Err(ErrorKind::BadType("positionWouldTelefrag takes an origin"));
    };
    let at = *at;
    // The same box the playerstate carries, taken from the playerstate
    // rather than written out a second time.
    let box_of = pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0);
    let (mins, maxs) = (box_of.mins(), box_of.maxs());
    let clients: Vec<EntId> = host
        .ents
        .iter_inuse()
        .filter(|(_, e)| e.client.is_some())
        .map(|(id, _)| id)
        .collect();
    let origin = cx.intern_folded("origin");
    for id in clients {
        let Value::Vector(other) = host.get_field(cx, id, origin) else {
            continue;
        };
        let overlaps = (0..3)
            .all(|i| at[i] + mins[i] < other[i] + maxs[i] && at[i] + maxs[i] > other[i] + mins[i]);
        if overlaps {
            return Ok(Value::Int(1));
        }
    }
    Ok(Value::Int(0))
}

/// `self setViewmodel(name)` (player method 17, 0x4512c): the name becomes a
/// model configstring index through `G_ModelIndex` and is kept on the
/// `gclient_t`, from where `ClientEndFrame` copies it into
/// `ps.viewmodelIndex`. Object model doc, section 20.
pub fn set_view_model(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("setViewmodel takes a model name"));
    };
    let name = cx.resolve(*name).to_string();
    let cs = host
        .allocators
        .index(&mut host.configstrings, CsRange::Model, &name)?;
    host.client_viewmodel[slot] = (cs - model_index_base()) as i32;
    Ok(Value::Undefined)
}

/// `self getViewmodel()` (player method 18, 0x451b4): the stored index back
/// through `G_ModelName`, so what comes out is the configstring's text, not
/// whatever string was passed in. A client with no viewmodel yet reads as the
/// empty string; retail's `G_ModelName(0)` is unmeasured.
pub fn get_view_model(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = match host.client_viewmodel[slot] {
        0 => "",
        i => host.configstrings[model_index_base() + i as usize].as_str(),
    };
    let atom = cx.intern_exact(name);
    Ok(Value::String(atom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::CsRange;
    use crate::game::testing::fixture;

    fn set_origin(host: &mut GameHost, cx: &mut Cx, id: EntId, at: [f32; 3]) {
        let field = cx.intern_folded("origin");
        host.set_field(cx, id, field, Value::Vector(at)).unwrap();
    }

    /// `setViewmodel` stores a model configstring index, not the name: the
    /// first allocation lands in `CsRange::Model`'s first slot, which is
    /// index 1, and `getViewmodel` reads the configstring's text back rather
    /// than echoing what was passed in. `ps.viewmodelIndex` is that index
    /// (object model doc, section 20).
    #[test]
    fn set_viewmodel_stores_a_model_index_and_reads_the_name_back() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let recv = Some(Target::Entity(c));
            let name = Value::String(cx.intern_exact("xmodel/viewmodel_hands_us"));
            set_view_model(&mut host, cx, recv, &[name]).unwrap();

            assert_eq!(host.client_viewmodel[0], 1);
            let (first, _) = CsRange::Model.bounds();
            assert_eq!(host.configstrings[first], "xmodel/viewmodel_hands_us");
            match get_view_model(&mut host, cx, recv, &[]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "xmodel/viewmodel_hands_us"),
                v => panic!("{v:?}"),
            }
        });
    }

    /// Retail keeps both under the player method table, so an entity with no
    /// `gclient_t` raises rather than growing a viewmodel of its own.
    #[test]
    fn set_viewmodel_refuses_a_non_client() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let recv = Some(Target::Entity(e));
            let name = Value::String(cx.intern_exact("xmodel/viewmodel_hands_us"));
            assert!(set_view_model(&mut host, cx, recv, &[name]).is_err());
            assert!(get_view_model(&mut host, cx, recv, &[]).is_err());
        });
    }

    /// A spawn origin overlapping a live player is refused; one clear of every
    /// player is allowed. The stage gate has a single client and can never tell
    /// these apart, which is why this test exists rather than relying on it.
    ///
    /// The receiver `b` deliberately stands 512 units away from every origin
    /// asked about, and the player that occupies them is `a`. An
    /// implementation that consulted only the receiver would answer 0 to all
    /// four questions and fail here; one that walks every client answers the
    /// first two 1 and the last two 0. Putting both players on the same spot
    /// would let receiver-only pass, which is the whole failure mode this test
    /// exists to catch.
    ///
    /// The two near cases pin the box rather than just its existence: the
    /// player box is 30 units across and 70 tall (`pmove::HALF_WIDTH`,
    /// `Stance::height`), so two boxes 24 apart on one axis overlap and two 40
    /// apart do not. A width used where a height belongs, or the reverse,
    /// moves one of these.
    #[test]
    fn a_spawn_point_inside_another_player_would_telefrag() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = host.ents.spawn_client(cx, 0).unwrap();
            set_origin(&mut host, cx, a, [0.0, 0.0, 0.0]);
            let b = host.ents.spawn_client(cx, 1).unwrap();
            set_origin(&mut host, cx, b, [512.0, 0.0, 0.0]);
            let recv = Some(Target::Entity(b));
            let ask = |host: &mut GameHost, cx: &mut Cx, at: [f32; 3]| {
                position_would_telefrag(host, cx, recv, &[Value::Vector(at)]).unwrap()
            };

            assert_eq!(ask(&mut host, cx, [0.0, 0.0, 0.0]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, [24.0, 0.0, 0.0]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, [40.0, 0.0, 0.0]), Value::Int(0));
            assert_eq!(ask(&mut host, cx, [512.0, 512.0, 0.0]), Value::Int(0));
        });
    }

    /// Only a client counts. A map entity sitting on the spawn point is not a
    /// player, so it must not refuse it: retail's test is `ent->client`
    /// non-null, not "something is there".
    #[test]
    fn a_map_entity_on_the_spot_does_not_telefrag() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let prop = host.ents.spawn(cx).unwrap();
            set_origin(&mut host, cx, prop, [0.0, 0.0, 0.0]);
            let here = Value::Vector([0.0, 0.0, 0.0]);
            assert_eq!(
                position_would_telefrag(&mut host, cx, None, &[here]).unwrap(),
                Value::Int(0)
            );
        });
    }

    /// The three weapon builtins, in the order `spawnPlayer` calls them, and
    /// the wire words they leave behind. `setSpawnWeapon` only takes a weapon
    /// the player already holds, which is retail's `COM_BitCheck` guard, so
    /// asking for one that was never given leaves `ps.weapon` where it was.
    #[test]
    fn the_spawn_loadout_builds_the_captured_weapon_words() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 0).unwrap();
            let t = Some(Target::Entity(e));
            let named = |cx: &mut Cx, n: &str| Value::String(cx.intern_exact(n));
            for w in ["colt_mp", "fraggrenade_mp", "m1carbine_mp"] {
                let arg = named(cx, w);
                give_weapon(&mut host, cx, t, &[arg]).unwrap();
                give_max_ammo(&mut host, cx, t, &[arg]).unwrap();
            }
            let carbine = named(cx, "m1carbine_mp");
            set_spawn_weapon(&mut host, cx, t, &[carbine]).unwrap();
            assert_eq!(host.client_weapons[0].held as u32 as i32, 4368);
            assert_eq!(host.client_weapons[0].current, 12);

            // Never given, so never selected.
            let sten = named(cx, "sten_mp");
            set_spawn_weapon(&mut host, cx, t, &[sten]).unwrap();
            assert_eq!(host.client_weapons[0].current, 12);

            // And a name no weapon file backs is an error, not a silent zero.
            let bogus = named(cx, "not_a_weapon_mp");
            assert!(give_weapon(&mut host, cx, t, &[bogus]).is_err());
        });
    }

    /// The two wire formats, against the verbatim capture in
    /// `docs/research/cod11-hud-protocol.md` section 0.1: the cvar value is
    /// quoted and its name is not, and the menu travels as its
    /// configstring slot rather than by name.
    #[test]
    fn the_join_commands_render_the_way_retail_sent_them() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 3).unwrap();
            let t = Some(Target::Entity(e));
            host.allocators
                .index(&mut host.configstrings, CsRange::Menu, "team_russiangerman")
                .unwrap();
            host.allocators
                .index(&mut host.configstrings, CsRange::Menu, "weapon_russian")
                .unwrap();

            let name = Value::String(cx.intern_exact("g_scriptMainMenu"));
            let value = Value::String(cx.intern_exact("team_russiangerman"));
            set_client_cvar(&mut host, cx, t, &[name, value]).unwrap();
            let tab = Value::String(cx.intern_exact("scr_showweapontab"));
            set_client_cvar(&mut host, cx, t, &[tab, Value::Int(0)]).unwrap();
            let objective = Value::String(cx.intern_exact("cg_objectiveText"));
            let text = Value::Localized(cx.intern_exact("DM_KILL_OTHER_PLAYERS"));
            set_client_cvar(&mut host, cx, t, &[objective, text]).unwrap();
            let menu = Value::String(cx.intern_exact("weapon_russian"));
            open_menu(&mut host, cx, t, &[menu]).unwrap();

            assert_eq!(
                host.client_commands,
                vec![
                    (3, "v g_scriptMainMenu \"team_russiangerman\"".to_string()),
                    (3, "v scr_showweapontab \"0\"".to_string()),
                    (
                        3,
                        "v cg_objectiveText \"DM_KILL_OTHER_PLAYERS\u{15}\"".to_string()
                    ),
                    (3, "t 1".to_string()),
                ]
            );
        });
    }

    /// The value is the only quoted argument on the line, so a `"` inside it
    /// would close the argument where it stands and leave the rest of the
    /// value as trailing tokens. Retail rewrites each one to `'` (0x447b2);
    /// this is the one behaviour of `setClientCvar` no capture shows, since
    /// no stock call passes a quote.
    #[test]
    fn a_quote_in_a_cvar_value_cannot_close_its_own_argument() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 0).unwrap();
            let name = Value::String(cx.intern_exact("cg_objectiveText"));
            let value = Value::String(cx.intern_exact("say \"hi\" now"));
            set_client_cvar(&mut host, cx, Some(Target::Entity(e)), &[name, value]).unwrap();
            assert_eq!(
                host.client_commands,
                vec![(0, "v cg_objectiveText \"say 'hi' now\"".to_string())]
            );
        });
    }

    /// A menu nobody precached has no slot to send, which is retail's
    /// `Menu '%s' was not precached` script error (0x5c78b), and a
    /// non-client entity is not addressable at all.
    #[test]
    fn an_unprecached_menu_and_a_non_client_receiver_are_errors() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let client = host.ents.spawn_client(cx, 0).unwrap();
            let menu = Value::String(cx.intern_exact("team_russiangerman"));
            assert!(open_menu(&mut host, cx, Some(Target::Entity(client)), &[menu]).is_err());

            let prop = host.ents.spawn(cx).unwrap();
            host.allocators
                .index(&mut host.configstrings, CsRange::Menu, "team_russiangerman")
                .unwrap();
            assert!(open_menu(&mut host, cx, Some(Target::Entity(prop)), &[menu]).is_err());
            assert!(host.client_commands.is_empty());
        });
    }

    /// The weapon name comes out of the host's own `client_weapons`, and a
    /// slot holding nothing reads `"none"` -- which is what a player on a
    /// ladder reports, the holster having written 0 into `ps.weapon`.
    #[test]
    fn getcurrentweapon_names_what_the_client_holds() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let recv = Some(Target::Entity(c));
            let name =
                |host: &mut GameHost, cx: &mut Cx| match get_current_weapon(host, cx, recv, &[])
                    .unwrap()
                {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => panic!("{v:?}"),
                };
            assert_eq!(name(&mut host, cx), "none");
            host.client_weapons[0].current = weapon_index("m1carbine_mp").unwrap() as u8;
            assert_eq!(name(&mut host, cx), "m1carbine_mp");
        });
    }

    /// `closeMenu` queues the bare `u` the retail builtin sends (`.so`
    /// 0x45574, format string 0x73204), and refuses a receiver with no
    /// `gclient_t` the way every other player method here does.
    #[test]
    fn closemenu_queues_the_bare_u_command() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 3).unwrap();
            close_menu(&mut host, cx, Some(Target::Entity(c)), &[]).unwrap();
            assert_eq!(host.client_commands, vec![(3, "u".to_string())]);
            let prop = host.ents.spawn(cx).unwrap();
            assert!(close_menu(&mut host, cx, Some(Target::Entity(prop)), &[]).is_err());
        });
    }

    /// A dropped weapon is spawned as the placed weapon it is: a
    /// `mpweapon_*` classname with the weapon in `weaponinfo`, at the
    /// player's own origin, with the free think armed. `wire::kind_of` reads
    /// exactly those two fields, so this is what puts it on the wire as an
    /// `ET_ITEM`.
    #[test]
    fn dropitem_spawns_a_placed_weapon_where_the_player_stands() {
        let (mut vm, mut host) = fixture();
        host.level_time_ms = 5_000;
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            set_origin(&mut host, cx, c, [16.0, -32.0, 8.0]);
            let name = Value::String(cx.intern_exact("m1carbine_mp"));
            drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap();

            let (id, _) = host.ents.iter_inuse().find(|(id, _)| *id != c).unwrap();
            let read = |host: &mut GameHost, cx: &mut Cx, f: &str| {
                let atom = cx.intern_folded(f);
                host.get_field(cx, id, atom)
            };
            match read(&mut host, cx, "classname") {
                Value::String(a) => assert_eq!(cx.resolve(a), "mpweapon_m1carbine"),
                v => panic!("{v:?}"),
            }
            match read(&mut host, cx, "weaponinfo") {
                Value::String(a) => assert_eq!(cx.resolve(a), "m1carbine_mp"),
                v => panic!("{v:?}"),
            }
            assert_eq!(
                read(&mut host, cx, "origin"),
                Value::Vector([16.0, -32.0, 8.0])
            );
            let e = host.ents.get(id).unwrap();
            assert_eq!(e.think, Some(ThinkFn::Free));
            assert_eq!(e.nextthink, 5_000 + DROPPED_ITEM_MS);
        });
    }

    /// A name no weapon file backs raises rather than spawning an item with
    /// no weapon on it, the same reading every other weapon builtin takes.
    #[test]
    fn dropitem_refuses_a_weapon_nothing_backs() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let name = Value::String(cx.intern_exact("blunderbuss_mp"));
            assert!(drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).is_err());
            assert_eq!(host.ents.iter_inuse().count(), 1, "no item was spawned");
        });
    }

    /// `cloneplayer` copies the mirror `Server::replay_moves` wrote into the
    /// body queue's next slot. A client with no mirror -- connected but never
    /// in the world -- clones nothing.
    #[test]
    fn cloneplayer_pushes_the_mirrored_state_into_the_body_queue() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 2).unwrap();
            let recv = Some(Target::Entity(c));
            assert_eq!(
                clone_player(&mut host, cx, recv, &[]).unwrap(),
                Value::Undefined
            );
            assert_eq!(host.bodies.entities().count(), 0, "no mirror, no corpse");

            let mut state = vcod_common::net::msg::EntityState::null(p);
            state.number = 2;
            let set = |s: &mut vcod_common::net::msg::EntityState, n: &str, v: i32| {
                s.fields[vcod_common::net::msg::EntityState::field_index(p, n).unwrap()] = v;
            };
            set(&mut state, "clientNum", 2);
            set(&mut state, "legsAnim", 700);
            host.client_entity_states[2] = Some(state);
            clone_player(&mut host, cx, recv, &[]).unwrap();

            let (number, body) = host.bodies.entities().next().unwrap();
            assert_eq!(number, crate::game::bodies::BODY_FIRST);
            assert_eq!(body.field_i32(p, "eType"), crate::game::bodies::ET_CORPSE);
            assert_eq!(body.field_i32(p, "clientNum"), 2);
            assert_eq!(body.field_i32(p, "legsAnim"), 700);
        });
    }

    /// A host with the shipped weapon files mounted: the slot builtins
    /// resolve a slot to the weapon standing in it, and the ops they push
    /// carry the ammo and clip indexes the table hands out. `None` without
    /// the paks, where no weapon file fills a slot at all.
    fn armed_fixture() -> Option<(vcod_gsc::Vm, GameHost)> {
        let fs = std::rc::Rc::new(vcod_common::testing::game_fs()?);
        let (vm, mut host) = fixture();
        host.weapons = std::rc::Rc::new(crate::weapons::WeaponTable::load(&fs));
        host.fs = Some(fs);
        Some((vm, host))
    }

    /// The stock allies loadout on client 0, with the ops it pushed dropped:
    /// what the three tests below start from.
    fn loadout(host: &mut GameHost, cx: &mut Cx) -> EntId {
        let e = host.ents.spawn_client(cx, 0).unwrap();
        let t = Some(Target::Entity(e));
        for w in ["colt_mp", "fraggrenade_mp", "m1carbine_mp"] {
            let arg = Value::String(cx.intern_exact(w));
            give_weapon(host, cx, t, &[arg]).unwrap();
        }
        let carbine = Value::String(cx.intern_exact("m1carbine_mp"));
        set_spawn_weapon(host, cx, t, &[carbine]).unwrap();
        host.client_weapon_ops.clear();
        e
    }

    /// A player standing on the test floor, for the tests that run the
    /// weapon machine: only `(Normal, Some(world))` steps it.
    fn player_sim() -> crate::spectate::ClientSim {
        let angles = vcod_common::net::msg::NULL_USERCMD.angles;
        let mut sim = crate::spectate::ClientSim::spectator([0.0, 0.0, 8.0], 0.0, angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, angles);
        sim
    }

    /// The four slot builtins against the stock loadout: a slot names the
    /// weapon whose file put it there, an empty one names `"none"`, and the
    /// two setters address that weapon's own ammo and clip index rather than
    /// its weapon number. A name the slot table does not have is an error,
    /// the way retail's `Unknown weaponslot name %s` is.
    #[test]
    fn the_slot_builtins_read_and_load_the_weapon_in_a_slot() {
        let Some((mut vm, mut host)) = armed_fixture() else {
            return;
        };
        let carbine = weapon_index("m1carbine_mp").unwrap();
        let (ammo_index, clip_index) = {
            let def = host.weapons.get(carbine).expect("the carbine");
            (def.ammo_index, def.clip_index)
        };
        vm.with_cx(|cx| {
            let e = loadout(&mut host, cx);
            let t = Some(Target::Entity(e));
            let name = |host: &mut GameHost, cx: &mut Cx, slot: &str| {
                let arg = Value::String(cx.intern_exact(slot));
                match get_weapon_slot_weapon(host, cx, t, &[arg]).unwrap() {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => panic!("{v:?}"),
                }
            };
            assert_eq!(name(&mut host, cx, "primary"), "m1carbine_mp");
            assert_eq!(name(&mut host, cx, "pistol"), "colt_mp");
            assert_eq!(name(&mut host, cx, "grenade"), "fraggrenade_mp");
            assert_eq!(name(&mut host, cx, "primaryb"), "none");

            let primary = Value::String(cx.intern_exact("primary"));
            let args = [primary, Value::Int(999)];
            set_weapon_slot_ammo(&mut host, cx, t, &args).unwrap();
            set_weapon_slot_clip_ammo(&mut host, cx, t, &args).unwrap();
            assert_eq!(
                host.client_weapon_ops,
                vec![
                    (
                        0,
                        WeaponOp::SetAmmo {
                            ammo_index,
                            rounds: 999
                        }
                    ),
                    (
                        0,
                        WeaponOp::SetClip {
                            clip_index,
                            rounds: 999
                        }
                    ),
                ]
            );

            let bogus = Value::String(cx.intern_exact("holster"));
            assert!(get_weapon_slot_weapon(&mut host, cx, t, &[bogus]).is_err());
            let args = [bogus, Value::Int(1)];
            assert!(set_weapon_slot_ammo(&mut host, cx, t, &args).is_err());
            assert!(set_weapon_slot_clip_ammo(&mut host, cx, t, &args).is_err());
        });
    }

    /// `takeAllWeapons` empties the host's copy, which is what the mirror
    /// puts in `ps.weapons` and `ps.weaponslots` the next frame, and pushes
    /// the op that empties the sim's ammo and clip arrays.
    #[test]
    fn takeallweapons_empties_the_host_copy_and_the_sims_arrays() {
        let Some((mut vm, mut host)) = armed_fixture() else {
            return;
        };
        vm.with_cx(|cx| {
            let e = loadout(&mut host, cx);
            take_all_weapons(&mut host, cx, Some(Target::Entity(e)), &[]).unwrap();
        });
        assert_eq!(
            host.client_weapons[0],
            crate::weapons::PlayerWeapons::default()
        );
        assert_eq!(host.client_weapon_ops, vec![(0, WeaponOp::TakeAll)]);

        let mut sim = player_sim();
        sim.ps.ammo[3] = 56;
        sim.ps.ammoclip[10] = 15;
        // The mirror `Server` runs before the ops, then the op itself.
        sim.ps.weapons_held = host.client_weapons[0].held;
        sim.ps.weapon_slots = host.client_weapons[0].slots;
        crate::server::apply_weapon_op(&mut sim, host.client_weapon_ops[0].1, &host.weapons);
        assert_eq!(sim.ps.weapons_held, 0);
        assert_eq!(sim.ps.weapon_slots, [0; crate::weapons::NUM_SLOTS]);
        assert_eq!(sim.ps.ammo, [0; vcod_common::pmove::weapon::NUM_AMMO]);
        assert_eq!(sim.ps.ammoclip, [0; vcod_common::pmove::weapon::NUM_AMMO]);
    }

    /// `switchToWeapon` asks for the same change a usercmd's weapon byte
    /// does, so the weapon goes away and comes back rather than teleporting
    /// into the player's hands: the op raises `EV_PUTAWAY_WEAPON` and the
    /// machine raises `EV_RAISE_WEAPON` when the drop ends, `ps.weapon`
    /// reading the colt only from there on. The host's own copy is left
    /// alone -- the mirror would otherwise force the target weapon in ahead
    /// of the putaway.
    #[test]
    fn switchtoweapon_goes_through_the_putaway_and_the_raise() {
        use vcod_common::pmove::weapon::{EV_PUTAWAY_WEAPON, EV_RAISE_WEAPON};
        let Some((mut vm, mut host)) = armed_fixture() else {
            return;
        };
        let (carbine, colt) = (
            weapon_index("m1carbine_mp").unwrap(),
            weapon_index("colt_mp").unwrap(),
        );
        vm.with_cx(|cx| {
            let e = loadout(&mut host, cx);
            let t = Some(Target::Entity(e));
            let arg = Value::String(cx.intern_exact("colt_mp"));
            switch_to_weapon(&mut host, cx, t, &[arg]).unwrap();
            // Never given, so there is nothing to switch to.
            let sten = Value::String(cx.intern_exact("sten_mp"));
            assert!(switch_to_weapon(&mut host, cx, t, &[sten]).is_err());
        });
        assert_eq!(
            host.client_weapon_ops,
            vec![(0, WeaponOp::SwitchTo(colt as u8))]
        );
        assert_eq!(
            host.client_weapons[0].current, carbine as u8,
            "the machine picks the weapon up, not the builtin"
        );

        let world = vcod_common::collision::test_world(&[]);
        let cmd = vcod_common::net::msg::NULL_USERCMD;
        let mut sim = player_sim();
        sim.ps.weapons_held = host.client_weapons[0].held;
        sim.ps.weapon_slots = host.client_weapons[0].slots;
        sim.ps.weapon = host.client_weapons[0].current;
        crate::server::apply_weapon_op(&mut sim, host.client_weapon_ops[0].1, &host.weapons);
        assert_eq!(sim.events[0], EV_PUTAWAY_WEAPON);
        assert_eq!(sim.ps.weapon, carbine as u8, "still the carbine, mid-drop");

        let mut raised = Vec::new();
        for _ in 0..40 {
            let events = sim.step(&cmd, 0.05, Some(&world), host.weapons.defs());
            raised.extend(events.into_iter().map(|e| e.event));
        }
        assert!(raised.contains(&EV_RAISE_WEAPON), "raised: {raised:?}");
        assert!(!raised.contains(&EV_PUTAWAY_WEAPON), "one putaway, not two");
        assert_eq!(sim.ps.weapon, colt as u8);
    }
}
