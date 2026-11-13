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

use crate::configstrings::{CsRange, script_menu_index};
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::{GameHost, SimOp, WeaponOp};
use crate::game::say::SayMode;
use vcod_common::pmove;
use vcod_gsc::{Cx, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("setclientcvar", set_client_cvar),
    ("openmenu", open_menu),
    ("openmenunomouse", open_menu_no_mouse),
    ("giveweapon", give_weapon),
    ("givemaxammo", give_max_ammo),
    ("setspawnweapon", set_spawn_weapon),
    ("switchtoweapon", switch_to_weapon),
    ("takeallweapons", take_all_weapons),
    ("takeweapon", take_weapon),
    ("getweaponslotweapon", get_weapon_slot_weapon),
    ("setweaponslotweapon", set_weapon_slot_weapon),
    ("setweaponslotammo", set_weapon_slot_ammo),
    ("setweaponslotclipammo", set_weapon_slot_clip_ammo),
    ("positionwouldtelefrag", position_would_telefrag),
    ("setviewmodel", set_view_model),
    ("getviewmodel", get_view_model),
    ("shellshock", shell_shock),
    ("stopshellshock", stop_shell_shock),
    ("usebuttonpressed", use_button_pressed),
    ("isonground", is_on_ground),
    ("islookingat", is_looking_at),
    ("getcurrentweapon", get_current_weapon),
    ("cloneplayer", clone_player),
    ("dropitem", drop_item),
    ("closemenu", close_menu),
    ("freezecontrols", freeze_controls),
    ("setorigin", set_player_origin),
    ("setplayerangles", set_player_angles),
    ("sayall", say_all),
    ("sayteam", say_team),
    ("pingplayer", ping_player),
    ("allowcomplaint", allow_complaint),
    ("getguid", get_guid),
    ("disableweapon", disable_weapon),
    ("enableweapon", enable_weapon),
    ("allowspectateteam", allow_spectate_team),
    ("autodemostart", auto_demo_start),
    ("autodemostop", auto_demo_stop),
    ("autoscreenshot", auto_screenshot),
    ("addtestclient", add_test_client),
];

/// `addTestClient()` (game.mp 0x5f6e8 over cod_lnxded `SV_AddTestClient`
/// 0x808753c): the first free slot connects as `bot<n>`, its connect callback
/// runs inside the call and its `begin` notify lands once the calling
/// thread's step returns (docs/research/cod11-gsc-object-model.md,
/// "addTestClient"). `undefined` with no free slot, and for the counter's
/// first bot inside `sv_reconnectlimit` of the start: its address, port 0,
/// matches every never-used slot's in `SV_DirectConnect`'s reconnect check.
/// On 1.1 the client sends no cmds, so nothing ever thinks for it; CoD2's
/// thinks on a random cmd each frame (`Cod2Level::test_client_cmd`).
pub fn add_test_client(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let taken = |host: &GameHost, slot: usize| {
        host.client_roster.get(slot).is_some_and(Option::is_some)
            || host.test_clients.added.iter().any(|(s, _, _)| *s == slot)
            || host
                .ents
                .handle(slot as u32)
                .and_then(|id| host.ents.get(id))
                .is_some_and(|e| e.client.is_some())
    };
    let Some(slot) = (0..host.client_roster.len()).find(|&s| !taken(host, s)) else {
        return Ok(Value::Undefined);
    };
    let port = host.test_clients.counter;
    host.test_clients.counter += 1;
    // `sv_reconnectlimit`'s 3 s default, against `lastConnectTime` 0.
    if port == 0 && host.test_clients.svs_time_ms < 3000 {
        return Ok(Value::Undefined);
    }
    let name = format!("bot{port}");
    let enters_world = host.profile.test_client_enters_world;
    if enters_world {
        // `ClientConnect`'s fresh `gclient_t` and `pers.connected` 1, as
        // `Cod2Level::connect_at` gives a real client.
        host.cod2_mut().clients[slot] = Default::default();
        host.client_roster[slot] = Some(crate::game::scoreboard::RosterSlot {
            connecting: true,
            ..Default::default()
        });
    }
    let Some(id) = host.connect_client(cx, slot, &name) else {
        return Ok(Value::Undefined);
    };
    host.test_clients.added.push((slot, port as u16, name));
    let me = Target::Entity(id);
    let connect = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerConnect",
    );
    cx.spawn(connect, Some(me), Vec::new());
    if enters_world {
        // CoD2's `SV_AddTestClient` (lnxded 0x80913da) ends in
        // `SV_ClientEnterWorld`: `ClientBegin`'s `pers.connected` 2.
        if let Some(r) = host.client_roster[slot].as_mut() {
            r.connecting = false;
            r.active = true;
        }
        host.ranks_dirty = true;
    }
    let begin = cx.intern_folded(crate::game::script::BEGIN_NOTIFY);
    cx.notify(me, begin, Vec::new());
    Ok(Value::Entity(id))
}

/// 1.5's `self getGuid()` (game.mp 0x4110b): `trap_GetGuid`, the guid the
/// engine took from the client's challenge (cod_lnxded 1.5b 0x808a1d0),
/// which only the CD key authorize server fills. vcod authorizes nobody,
/// so it is 0, what the 1.5b server logs for a LAN client ("guid 0").
pub fn get_guid(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    client_receiver(host, recv)?;
    if !args.is_empty() {
        return Err(ErrorKind::BadType("USAGE: self getGuid()"));
    }
    Ok(Value::Int(0))
}

/// 1.5's `self disableWeapon()` (game.mp 0x408e0): `pm_flags` 0x100000,
/// which 1.5's pmove holsters on as it does on a ladder.
pub fn disable_weapon(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_sim_ops
        .push((slot, SimOp::WeaponDisabled(true)));
    Ok(Value::Undefined)
}

/// 1.5's `self enableWeapon()` (game.mp 0x40983): the bit off again.
pub fn enable_weapon(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.client_sim_ops
        .push((slot, SimOp::WeaponDisabled(false)));
    Ok(Value::Undefined)
}

/// 1.5's `self allowSpectateTeam(team, allowed)` (game.mp 0x40f93): clears
/// (allowed) or sets (refused) the team's bit in the client's spectate mask,
/// `none` 1, `axis` 2, `allies` 4, `freelook` 0x10, the bits
/// `G_ClientCanSpectateTeam` (0x3a36d) tests by `1 << team`.
pub fn allow_spectate_team(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let bit = match args.first() {
        Some(Value::String(s)) => match cx.resolve(*s) {
            "none" => 1,
            "axis" => 2,
            "allies" => 4,
            "freelook" => 0x10,
            _ => 0,
        },
        _ => 0,
    };
    if bit == 0 {
        return Err(ErrorKind::BadType(
            "team must be \"axis\", \"allies\", \"none\", or \"freelook\"",
        ));
    }
    let allowed = args
        .get(1)
        .ok_or(ErrorKind::BadType(
            "allowSpectateTeam takes a team and a bool",
        ))?
        .as_bool()?;
    let mask = &mut host.client_no_spectate[slot];
    if allowed {
        *mask &= !bit;
    } else {
        *mask |= bit;
    }
    Ok(Value::Undefined)
}

/// 1.5's `autoDemoStart`, `autoDemoStop` and `autoScreenshot` (game.mp
/// 0x411bb, 0x41295, 0x4136f): with `g_autodemo` (`g_autoscreenshot` for
/// the last) non-zero, the reliable `C`, `D` or `E` and a 1 back; otherwise
/// nothing and no return value.
fn auto_capture(
    host: &mut GameHost,
    recv: Option<Target>,
    args: &[Value],
    cvar: &str,
    command: &str,
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    if !args.is_empty() {
        return Err(ErrorKind::BadType("takes no arguments"));
    }
    if super::cvar::atoi(host.cvars.get(cvar)) == 0 {
        return Ok(Value::Undefined);
    }
    host.client_commands.push((slot, command.to_string()));
    Ok(Value::Int(1))
}

pub fn auto_demo_start(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    auto_capture(host, recv, args, "g_autodemo", "C")
}

pub fn auto_demo_stop(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    auto_capture(host, recv, args, "g_autodemo", "D")
}

pub fn auto_screenshot(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    auto_capture(host, recv, args, "g_autoscreenshot", "E")
}

/// `self useButtonPressed()`: whether the client's last usercmd held the
/// use button, which every stock `respawn()` loop polls. `Server` mirrors
/// the buttons onto the host before the frame.
///
/// Retail stores each cmd's buttons on the client (`ClientThink_real`
/// 0x40129) and the builtin tests that word (0x44ed2), so a frame with
/// several cmds answers with its last one, not an OR
/// (docs/research/cod11-gsc-object-model.md, 23.5). The touch pass reads
/// each cmd's own bits, so a tick replaying [use, no-use] fires the trigger on
/// the first cmd while this answers 0. The two agree on the cmd the frame ends
/// on, as retail's do: the notified thread runs at the next script frame, when
/// `client+0x21e8` holds the frame's last cmd.
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

/// `self setOrigin(origin)`, a player method only (0x43480): the origin one
/// unit above the argument, relinked, the teleport bit flipped, velocity kept
/// (docs/research/cod11-gsc-object-model.md, 23.2). The script's copy moves
/// now so a read later this frame sees it; the sim's is queued.
pub fn set_player_origin(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(&Value::Vector([x, y, z])) = args.first() else {
        return Err(ErrorKind::BadType("setOrigin takes a vector"));
    };
    let origin = [x, y, z + 1.0];
    let field = cx.intern_folded("origin");
    host.set_field(cx, entity_receiver(recv)?, field, Value::Vector(origin))?;
    // An unlink, then a link with the contents the entity holds (0x53480):
    // the player goes to the head of its node's list even when it lands in
    // the node it left (combat doc 14.7).
    let contents = host.area.contents(slot as u32);
    host.area.unlink(slot as u32);
    let playing = !host.client_vitals[slot].dead;
    host.link_client(slot, origin, host.client_box(slot), contents, playing);
    host.client_link_origin[slot] = origin;
    host.client_sim_ops
        .push((slot, SimOp::SetOrigin { origin }));
    Ok(Value::Undefined)
}

/// `self setPlayerAngles(angles)`, a player method (0x44df0): a straight
/// `SetClientViewAngle` call. The script's `angles` moves now to what the
/// prone arm leaves, as retail's `r.currentAngles` does (0x4209f); the
/// sim's view is queued with the script's, which the sim holds the same.
pub fn set_player_angles(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(&Value::Vector(angles)) = args.first() else {
        return Err(ErrorKind::BadType("setPlayerAngles takes a vector"));
    };
    let held = host.client_prone_cone[slot].map_or(angles, |cone| cone.hold(angles));
    let field = cx.intern_folded("angles");
    host.set_field(cx, entity_receiver(recv)?, field, Value::Vector(held))?;
    // The DObj stands in `r.currentAngles` until the next cmd.
    if let Some(s) = host.dobj_posed.get_mut(slot) {
        s.set_angles(Some(held));
    }
    host.client_sim_ops
        .push((slot, SimOp::SetViewAngles { angles }));
    Ok(Value::Undefined)
}

/// `self isOnGround()` (`PlayerCmd_isOnGround`, 0x45014): `ps.groundEntityNum
/// != 0x3ff` (docs/research/cod11-gsc-object-model.md, 23.5). `Server`
/// mirrors `ps.on_ground` onto the host alongside `client_pm_type`.
pub fn is_on_ground(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    Ok(Value::Int(host.client_on_ground[slot] as i32))
}

/// `self isLookingAt(ent)` (0x4576c): whether `ent` is what the last aim
/// trace entered. No trace or cone of its own
/// (docs/research/cod11-gsc-object-model.md, 23.1).
pub fn is_looking_at(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let looking = match args.first() {
        Some(Value::Entity(id)) => host.client_lookat[slot] == Some(*id),
        _ => false,
    };
    Ok(Value::Int(i32::from(looking)))
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
    let name = crate::items::item_name(host.defs, index).unwrap_or("none");
    Ok(Value::String(cx.intern_exact(name)))
}

/// `self cloneplayer()` (`.so` 0x4450c): the corpse the gametype leaves
/// behind, into the next body-queue slot
/// (`docs/research/cod11-combat.md` section 5.2). The state copied is the
/// mirror `Server::replay_moves` wrote for this client before the script
/// frame, and the queue re-reads it from the sim once more at the snapshot
/// build, so a death animation raised after this call still reaches the wire.
/// The body stands on the player's `origin` field, `r.currentOrigin`, which
/// is snapped when the death runs inside the victim's own cmd (5.5).
///
/// Returns the body's entity (`GScr_AddEntity` at 0x446d1): a `playSound`
/// on it rides the corpse's own ring, which the queue's state takes at the
/// snapshot build, and a `delete` takes it off the wire.
pub fn clone_player(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(mut state) = host.client_entity_states[slot].clone() else {
        // A client with no sim: connected, never entered the world.
        return Ok(Value::Undefined);
    };
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    let field = cx.intern_folded("origin");
    if let Value::Vector(at) = host.get_field(cx, entity_receiver(recv)?, field) {
        for (axis, v) in at.into_iter().enumerate() {
            if let Some(i) =
                vcod_common::net::msg::EntityState::field_index(p, &format!("pos.trBase[{axis}]"))
            {
                state.fields[i] = v.to_bits() as i32;
            }
        }
    }
    let now = host.level_time_ms;
    let world = host.world.clone();
    let number = host.bodies.push(
        state,
        Some(slot),
        now,
        world.as_ref().map(|w| &w.collision),
        p,
    );
    Ok(Value::Entity(host.ents.spawn_body(cx, number)))
}

/// `self dropItem(name [, tag])` (`.so` 0x43684): a weapon name goes to
/// `Drop_Weapon` with `tag`, `"tag_weapon_right"` by default (`.rodata`
/// 0x731d4); the weapon is taken, its rounds ride the item, and a
/// `clipOnly` weapon with an empty clip or a weapon not held leaves nothing.
/// Any other name goes through `BG_FindItem` to `Drop_Item`, which launches
/// the item with nothing taken off the player
/// (docs/research/cod11-items.md section 8). Returns the item, as
/// `GScr_AddEntity` (0x4375b) hands it back, or undefined when nothing
/// dropped or `BG_FindItem` found nothing, which is how `"none"` (what
/// `getCurrentWeapon` reports for `ps.weapon` 0, and stock `dm.gsc:531`
/// passes on every death with no guard) and a misspelt name both end.
pub fn drop_item(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("dropItem takes an item name"));
    };
    let name = cx.resolve(*name).to_string();
    let Some(index) = host.weapon_index(&name).filter(|&i| i != 0) else {
        return match crate::items::classname_index(host.defs, &name) {
            Some(row) => Ok(Value::Entity(crate::game::item::drop_item(
                host, cx, slot, row,
            )?)),
            None => Ok(Value::Undefined),
        };
    };
    let tag = match args.get(1) {
        Some(Value::String(t)) => cx.resolve(*t).to_string(),
        _ => "tag_weapon_right".to_string(),
    };
    let before = crate::game::item::inventory(host, slot);
    let mut inv = before;
    let weapons = host.weapons.clone();
    let dropped = crate::game::pickup::drop_weapon(&mut inv, &weapons, index as u8);
    crate::game::item::write_back(host, slot, &before, &inv);
    let Some(d) = dropped else {
        return Ok(Value::Undefined);
    };
    let at = crate::game::item::DropAt::Thrown { tag };
    let id = crate::game::item::launch_weapon(host, cx, slot, d, at)?;
    Ok(Value::Entity(id))
}

/// `self sayAll(message [, args...])` (0x45884): the message packed by
/// `Scr_ConstructMessageString` behind a `\x14`, so the client localizes it,
/// and said through `G_Say` as the receiver's own `say`
/// (docs/research/cod11-chat.md).
pub fn say_all(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    script_say(host, cx, recv, args, SayMode::All)
}

/// `self sayTeam(message [, args...])` (0x45920): [`say_all`] as a
/// `say_team`.
pub fn say_team(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    script_say(host, cx, recv, args, SayMode::Team)
}

fn script_say(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    mode: SayMode,
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let mut text = format!("\u{14}{}", super::message::construct(host, cx, args));
    // The 0x3ff-byte buffer behind the leading byte.
    while text.len() > 0x3ff {
        text.pop();
    }
    host.say(cx, slot, None, mode, &text);
    Ok(Value::Undefined)
}

/// `self pingPlayer()` (`PlayerCmd_pingPlayer` 0x450a4): `eFlags` 0x80000
/// for 3 s, the chat flash a teammate's compass shows
/// (docs/research/cod11-hud-protocol.md, "Compass friendlies").
pub fn ping_player(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let until = host.level_time_ms + 3000;
    if let Some(p) = host.client_ping_until.get_mut(slot) {
        *p = until;
    }
    Ok(Value::Undefined)
}

/// `self allowComplaint(attacker)` (0x43578): opens the receiver's
/// teamkill complaint about `attacker` ([`GameHost::allow_complaint`]).
pub fn allow_complaint(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let victim = client_receiver(host, recv)?;
    let attacker = client_receiver(
        host,
        args.first().and_then(|a| match a {
            Value::Entity(id) => Some(Target::Entity(*id)),
            _ => None,
        }),
    )?;
    host.allow_complaint(victim, attacker);
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

/// `self freezeControls(bool)` (0x456ec): the player check, `Scr_GetBool`,
/// and a store to `client+0x21e0` that nothing reads, so in 1.1 MP it freezes
/// nothing, as a live run confirms (docs/research/cod11-mantle.md,
/// "`pm_flags` 0x4000").
pub fn freeze_controls(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    client_receiver(host, recv)?;
    let Some(v) = args.first() else {
        return Err(ErrorKind::BadType("freezeControls takes a boolean"));
    };
    v.as_bool()?;
    Ok(Value::Undefined)
}

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The model configstring slot `ps.viewmodelIndex` counts from. `G_ModelIndex`
/// (0x66ed8) hands out 1-based offsets from it, so index 1 is the first slot
/// of `CsRange::Model`.
fn model_index_base(host: &GameHost) -> usize {
    host.allocators.layout().models()
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

/// `self setClientCvar(name, value [, args...])`: the reliable command
/// `v <name> "<value>"` (0x446e0, format string `v %s "%s"`). The name is
/// never quoted and the value always is, so retail rewrites a `"` inside the
/// value as `'` (0x447b2) rather than letting it close the argument early. A
/// localized value takes the `Scr_GetType == 2` branch (0x44750) into
/// `Scr_ConstructMessageString` (0x44765), which is what packs the
/// substitution arguments after it (`super::message`).
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
    let value = match value {
        Value::Localized(_) => super::message::construct(host, cx, &args[1..]),
        _ => cx
            .format_number(value)
            .ok_or(ErrorKind::BadType("setClientCvar takes a renderable value"))?,
    }
    .replace('"', "'");
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
    send_menu(host, cx, recv, args, "")
}

/// `self openMenuNoMouse(menu)` (0x454b4): `openMenu` with `t %i 1`, which
/// the cgame opens as `UIMENU_SCRIPT_POPUP_NO_MOUSE` (front-end doc 13).
pub fn open_menu_no_mouse(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    send_menu(host, cx, recv, args, " 1")
}

fn send_menu(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    tail: &str,
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(Value::String(menu)) = args.first() else {
        return Err(ErrorKind::BadType("openMenu takes a menu name"));
    };
    let menu = cx.resolve(*menu).to_string();
    let index = script_menu_index(&host.configstrings, host.allocators.layout(), &menu)
        .ok_or(ErrorKind::BadType("that menu was not precached"))?;
    host.client_commands
        .push((slot, format!("t {index}{tail}")));
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
fn weapon_argument(
    host: &mut GameHost,
    cx: &Cx,
    args: &[Value],
) -> Result<(String, usize), ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("that builtin takes a weapon name"));
    };
    let name = cx.resolve(*name).to_string();
    let index = host
        .weapon_index(&name)
        .ok_or(ErrorKind::BadType("no such weapon"))?;
    Ok((name, index))
}

/// `BG_GivePlayerWeapon`'s `RegisterItem(weapon, 1)` (0x36a38): a weapon
/// the player does not hold yet is registered, which past
/// `level.initializing` is a script error unless it was precached.
fn register_given(
    host: &mut GameHost,
    slot: usize,
    name: &str,
    index: usize,
) -> Result<(), ErrorKind> {
    if host.client_weapons[slot].holds(index) {
        return Ok(());
    }
    host.register_item(name)
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
    let (name, index) = weapon_argument(host, cx, args)?;
    register_given(host, slot, &name, index)?;
    let weapons = host.weapons.clone();
    host.client_weapons[slot].give_chain(&weapons, index, weapons.slot(index));
    if let Some(def) = weapons.get(index) {
        host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
        if !def.clip_only {
            host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: def.start_ammo as i16,
                },
            );
        }
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
    let (_, index) = weapon_argument(host, cx, args)?;
    let weapons = host.weapons.clone();
    if host.profile.game == vcod_common::game::Game::Cod2 {
        // CoD2's `PlayerCmd_giveMaxAmmo` (Mac 0x1c2a02): a held weapon's
        // reserve topped up to `maxAmmo` through `Add_Ammo`, the clip left.
        if let Some(def) = weapons.get(index)
            && host.client_weapons[slot].holds(index)
        {
            let held = host
                .client_ammo
                .get(slot)
                .and_then(|a| a.ammo.get(def.ammo_index))
                .copied()
                .map_or(0, i32::from);
            let add = def.max_ammo as i32 - held;
            if add > 0 {
                cod2_add_ammo(host, slot, def, add, false);
            }
        }
        return Ok(Value::Undefined);
    }
    if let Some(def) = weapons.get(index) {
        // A `clipOnly` weapon has no reserve: the frag's file reads
        // `clipOnly 1` with `maxAmmo 3`, and retail's spawn line carries
        // `clip=6:3` with no `ammo` entry for index 6 (combat doc, 9.2).
        if !def.clip_only {
            host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: def.max_ammo as i16,
                },
            );
        }
        host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
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
    let (_, index) = weapon_argument(host, cx, args)?;
    let w = &mut host.client_weapons[slot];
    if w.holds(index) {
        w.current = index as u8;
        host.weapon_op(slot, WeaponOp::SetCurrent(index as u8));
    }
    Ok(Value::Undefined)
}

/// `self switchToWeapon(name)` (`PlayerCmd_switchToWeapon`, player method 5,
/// 0x44c34): for a carried weapon, `a <index>` to the client, whose cgame
/// selects it and asks for it in its cmds, and 1; for any other, 0 (combat
/// doc, 9.8). The playerstate is the client's to change; a bot answers the
/// `a` as a cgame would (`Bot::observe`).
pub fn switch_to_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let (_, index) = weapon_argument(host, cx, args)?;
    if !host.client_weapons[slot].holds(index) {
        return Ok(Value::Int(0));
    }
    host.client_commands.push((slot, format!("a {index}")));
    Ok(Value::Int(1))
}

/// `self takeWeapon(name)` (`PlayerCmd_takeWeapon`, 1.1 game.mp 0x4493c, 1.5
/// 0x3d749): the weapon's reserve and clip emptied and the weapon dropped
/// with its `altWeapon` modes, whether or not it was held; `ps.weapon` is
/// left for the weapon machine to put away. A name with no index is
/// retail's weapon 0, which holds nothing. 1.5's `_teams::givePistol` calls
/// it on every spawn.
pub fn take_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("takeWeapon takes a weapon name"));
    };
    let Some(index) = host.weapon_index(cx.resolve(*name)) else {
        return Ok(Value::Undefined);
    };
    let weapons = host.weapons.clone();
    if let Some(def) = weapons.get(index) {
        let (clip_index, ammo_index) = (def.clip_index, def.ammo_index);
        host.weapon_op(
            slot,
            WeaponOp::SetAmmo {
                ammo_index,
                rounds: 0,
            },
        );
        host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index,
                rounds: 0,
            },
        );
    }
    host.client_weapons[slot].take_chain(&weapons, index);
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
    host.weapon_op(slot, WeaponOp::TakeAll);
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
    let name = crate::items::item_name(host.defs, index).unwrap_or("none");
    Ok(Value::String(cx.intern_exact(name)))
}

/// `self setWeaponSlotWeapon(slot, name)` (player method 33, 0x43df4): the
/// weapon given and put in the slot the caller names rather than the one its
/// own file names, with a full clip and its `startAmmo` in reserve. Stock
/// script runs it ahead of both setters and a `switchToWeapon` on the weapon
/// it just placed (`sd.gsc` 540-543), so a weapon set into a slot has to
/// come out usable.
///
/// `"none"` is not a weapon name but weapon index 0, which empties the slot:
/// retail compares the argument against `"none"` ahead of its name lookup
/// (0x43ebd), and stock script feeds `getWeaponSlotWeapon`'s own `"none"`
/// straight back in -- `sd.gsc`'s `spawnPlayer` re-gives the `pers["weapon2"]`
/// `endRound` stashed, which is `"none"` for a player carrying one primary.
/// Either way whatever stood in the slot is taken first (0x43f6b).
pub fn set_weapon_slot_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let client = client_receiver(host, recv)?;
    let slot = slot_argument(cx, args)?;
    let rest = args.get(1..).unwrap_or_default();
    let index = match rest.first() {
        Some(Value::String(a)) if cx.resolve(*a).eq_ignore_ascii_case("none") => 0,
        _ => weapon_argument(host, cx, rest)?.1,
    };
    let weapons = host.weapons.clone();
    let standing = host.client_weapons[client].slots[slot] as usize;
    host.client_weapons[client].take_chain(&weapons, standing);
    if index == 0 {
        return Ok(Value::Undefined);
    }
    let name = host.weapon_name(index).unwrap_or_default();
    let held_before = host.client_weapons[client].holds(index);
    register_given(host, client, &name, index)?;
    host.client_weapons[client].give_chain(&weapons, index, slot);
    if host.profile.game == vcod_common::game::Game::Cod2 {
        if let Some(def) = weapons.get(index) {
            cod2_start_ammo(host, client, def, !held_before);
        }
        return Ok(Value::Undefined);
    }
    if let Some(def) = weapons.get(index) {
        host.weapon_op(
            client,
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
        if !def.clip_only {
            host.weapon_op(
                client,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: def.start_ammo as i16,
                },
            );
        }
    }
    Ok(Value::Undefined)
}

/// CoD2's `PlayerCmd_SetWeaponSlotWeapon` (Mac 0x1c0a08) past the give:
/// `startAmmo` less the reserve already held, through [`cod2_add_ammo`],
/// the clip filled for a weapon not held before. A launcher with
/// `startAmmo` 0 comes out empty and reloads (cod2-combat.md 9.7).
fn cod2_start_ammo(
    host: &mut GameHost,
    client: usize,
    def: &vcod_common::weapon::WeaponDef,
    new: bool,
) {
    let held = host
        .client_ammo
        .get(client)
        .and_then(|a| a.ammo.get(def.ammo_index))
        .copied()
        .map_or(0, i32::from);
    let add = def.start_ammo as i32 - held;
    if add > 0 {
        cod2_add_ammo(host, client, def, add, new);
    }
}

/// CoD2's `Add_Ammo` (Mac 0x1b0e30): `add` rounds into the reserve; with
/// `fill`, or for a `clipOnly` weapon, what the clip lacks moves out of the
/// reserve (a `clipOnly` reserve then empties and its clip is capped at
/// `clipSize`); the reserve is capped at `maxAmmo`.
fn cod2_add_ammo(
    host: &mut GameHost,
    client: usize,
    def: &vcod_common::weapon::WeaponDef,
    add: i32,
    fill: bool,
) {
    let a = host.client_ammo.get(client).copied().unwrap_or_default();
    let (ai, ci) = (def.ammo_index, def.clip_index);
    let mut ammo = a.ammo.get(ai).copied().map_or(0, i32::from) + add;
    let mut clip = a.clip.get(ci).copied().map_or(0, i32::from);
    if fill || def.clip_only {
        let take = (def.clip_size as i32 - clip).min(ammo);
        ammo -= take;
        clip += take;
    }
    if def.clip_only {
        ammo = 0;
        clip = clip.min(def.clip_size as i32);
    }
    ammo = ammo.min(def.max_ammo as i32);
    let r = |n: i32| n.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
    host.weapon_op(
        client,
        WeaponOp::SetClip {
            clip_index: ci,
            rounds: r(clip),
        },
    );
    host.weapon_op(
        client,
        WeaponOp::SetAmmo {
            ammo_index: ai,
            rounds: r(ammo),
        },
    );
}

/// `self setWeaponSlotAmmo(slot, rounds)` (player method 35, 0x44130): the
/// reserve of whatever weapon occupies the slot. Ammo is indexed by the
/// weapon's ammo *name*, not by the weapon, so two weapons sharing a name
/// share the reserve (docs/protocol-1.1.md, "How `ammo[]` and `ammoclip[]`
/// are indexed"). An empty slot names no weapon and so writes nothing. A
/// clip-only weapon's reserve is its clip (0x4420f); the count is floored at
/// 0 and capped at `BG_GetAmmoTypeMax` (0x44294), or the clip size on the
/// clip-only arm, so `sd.gsc`'s 999 lands as the file's `maxAmmo`
/// (object model doc, "The six slot accessors").
pub fn set_weapon_slot_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (client, slot, rounds) = slot_and_rounds(host, cx, recv, args)?;
    let index = host.client_weapons[client].slots[slot] as usize;
    if let Some(def) = host.weapons.get(index) {
        let op = if def.clip_only {
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: clamp_rounds(rounds, def.clip_size),
            }
        } else {
            WeaponOp::SetAmmo {
                ammo_index: def.ammo_index,
                rounds: clamp_rounds(rounds, def.max_ammo),
            }
        };
        host.weapon_op(client, op);
    }
    Ok(Value::Undefined)
}

/// `self setWeaponSlotClipAmmo(slot, rounds)` (player method 37, 0x443e4):
/// the same for the clip, which has its own name table and its own index
/// space. Floored at 0 (0x444d7) and capped at `BG_GetAmmoClipSize`
/// (0x444ed): `sd.gsc` writes 999 and retail's capture reads the kar98k at
/// 5, and a clip past its size never reads "not full", which is what
/// refused every rifle reload under `sd`.
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
            rounds: clamp_rounds(rounds, def.clip_size),
        };
        host.weapon_op(client, op);
    }
    Ok(Value::Undefined)
}

/// The two setters' shared clamp: never below 0, never above `cap`.
fn clamp_rounds(rounds: i16, cap: u32) -> i16 {
    rounds.max(0).min(cap.min(i16::MAX as u32) as i16)
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
/// bounding box at `origin` against the area tree, mask 0x2000000, and 1 for
/// the first entity it lists that is a client with `pm_type` below 6
/// (0x5a8f4..0x5a8fc). The tree holds a client's link as its last cmd, spawn
/// or `setOrigin` made it; a spectator and a dead player are not in it with
/// that contents bit (object model doc 20, combat doc 14.7).
pub fn position_would_telefrag(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(at)) = args.first() else {
        return Err(ErrorKind::BadType("positionWouldTelefrag takes an origin"));
    };
    let at = *at;
    // `PM_DEAD`: 6 on CoD1, 7 on 1.7, whose 6 is the living `PM_LASTSTAND`
    // (`GScr_positionWouldTelefrag`).
    let dead = if host.profile.game == vcod_common::game::Game::Cod4 {
        7
    } else {
        6
    };
    // `playerMins`/`playerMaxs`, the standing box the playerstate carries.
    let box_of = pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0);
    let (mins, maxs) = (box_of.mins(), box_of.maxs());
    let lo = std::array::from_fn(|i| at[i] + mins[i]);
    let hi = std::array::from_fn(|i| at[i] + maxs[i]);
    let hit = host
        .area
        .entities_in_box(lo, hi, vcod_common::movetrace::CONTENTS_BODY as i32)
        .into_iter()
        .any(|n| {
            (n as usize) < crate::server::MAX_CLIENTS
                && host
                    .ents
                    .handle(n)
                    .is_some_and(|id| host.ents.get(id).is_some_and(|e| e.client.is_some()))
                && host.client_pm_type.get(n as usize).copied().unwrap_or(0) < dead
        });
    Ok(Value::Int(hit.into()))
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
    // `G_ModelIndex("")` is 0 (0x66ee8).
    host.client_viewmodel[slot] = if name.is_empty() {
        0
    } else {
        let cs = host.allocators.index_precached(
            &mut host.configstrings,
            CsRange::Model,
            &name,
            host.initializing,
        )?;
        (cs - model_index_base(host)) as i32
    };
    Ok(Value::Undefined)
}

/// `self shellShock(name, duration)` (entity method 28, 0x5bb44): the
/// 1-based slot of `name` among the 15 shellshock configstrings, the level
/// time and the duration in ms go into `ps.shellshockIndex`, `Time` and
/// `Duration` (gclient +0x3dc..+0x3e4). The scan is `strcasecmp` over every
/// slot, empty ones too, and has no `level.initializing` gate: a name not
/// there is an error at any time (docs/research/cod11-gsc-language.md 9,
/// "Indexing past the first wait").
pub fn shell_shock(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let [Value::String(a), duration] = args else {
        return Err(ErrorKind::Custom(
            "USAGE: <player> shellshock(<shellshockname>, <duration>)\n".into(),
        ));
    };
    let name = cx.resolve(*a).to_string();
    let (first, last) = host.allocators.layout().bounds(CsRange::ShellShock);
    let Some(index) =
        (first..=last).position(|i| host.configstrings[i].eq_ignore_ascii_case(&name))
    else {
        return Err(ErrorKind::Custom(format!(
            "shellshock '{name}' was not precached\n"
        )));
    };
    let secs = match duration {
        Value::Int(i) => *i as f64,
        Value::Float(f) => f64::from(*f),
        _ => return Err(ErrorKind::BadType("shellshock takes a numeric duration")),
    };
    // `duration * 1000 + 0.5`, truncated, then an unsigned test against
    // 60000, so a negative duration fails it too (0x5bc7a..0x5bcbf).
    let ms = (secs * 1000.0 + 0.5) as i32;
    if ms as u32 > 60_000 {
        let shown = cx
            .format_number(Value::Float(ms as f32 * 0.001))
            .unwrap_or_default();
        return Err(ErrorKind::Custom(format!(
            "duration {shown} should be >= 0 and <= 60"
        )));
    }
    host.client_shellshock[slot] = [index as i32 + 1, host.level_time_ms, ms];
    Ok(Value::Undefined)
}

/// `self stopShellShock()` (entity method 29, 0x5f468): the three
/// shellshock fields back to 0.
pub fn stop_shell_shock(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    if !args.is_empty() {
        return Err(ErrorKind::Custom(
            "USAGE: <player> stopshellshock()\n".into(),
        ));
    }
    host.client_shellshock[slot] = [0; 3];
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
        i => host.configstrings[model_index_base(host) + i as usize].as_str(),
    };
    let atom = cx.intern_exact(name);
    Ok(Value::String(atom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::{CsRange, weapon_index};
    use crate::game::entity::ThinkFn;
    use crate::game::testing::fixture;
    use vcod_gsc::EntId;

    /// A builtin's ammo op lands on the host's mirror at once, so a pickup
    /// or a `dropItem` later in the same frame reads it; the sim still gets
    /// the op to apply after the frame.
    #[test]
    fn a_weapon_op_updates_the_ammo_mirror_in_the_same_frame() {
        use crate::game::host::AmmoArrays;
        let (_vm, mut host) = fixture();
        host.weapon_op(
            0,
            WeaponOp::SetAmmo {
                ammo_index: 10,
                rounds: 400,
            },
        );
        host.weapon_op(
            0,
            WeaponOp::SetClip {
                clip_index: 10,
                rounds: 15,
            },
        );
        assert_eq!(host.client_ammo[0].ammo[10], 400);
        assert_eq!(host.client_ammo[0].clip[10], 15);
        assert_eq!(host.client_weapon_ops.len(), 2);
        host.weapon_op(0, WeaponOp::TakeAll);
        assert_eq!(host.client_ammo[0], AmmoArrays::default());
    }

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
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
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

    /// `shellshock` finds the name in any case and writes the 1-based slot,
    /// the level time and the rounded milliseconds; `""` matches the first
    /// empty slot, past 60 s and below 0 are the duration error, a name not
    /// precached is the error at any time, and `stopShellShock` zeroes all
    /// three (docs/research/cod11-gsc-language.md 9).
    #[test]
    fn shellshock_writes_the_three_fields_and_refuses_as_retail() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let recv = Some(Target::Entity(c));
            let (first, _) = CsRange::ShellShock.bounds();
            host.configstrings.set(first, "default");
            host.configstrings.set(first + 1, "groggy");
            host.level_time_ms = 1250;
            let s = |cx: &mut Cx, t: &str| Value::String(cx.intern_exact(t));
            let args = [s(cx, "GROGGY"), Value::Float(1.5)];
            shell_shock(&mut host, cx, recv, &args).unwrap();
            assert_eq!(host.client_shellshock[0], [2, 1250, 1500]);
            let args = [s(cx, ""), Value::Int(0)];
            shell_shock(&mut host, cx, recv, &args).unwrap();
            assert_eq!(host.client_shellshock[0], [3, 1250, 0]);

            let args = [s(cx, "default"), Value::Int(61)];
            assert_eq!(
                shell_shock(&mut host, cx, recv, &args),
                Err(ErrorKind::Custom(
                    "duration 61 should be >= 0 and <= 60".into()
                ))
            );
            let args = [s(cx, "default"), Value::Float(-0.25)];
            assert!(shell_shock(&mut host, cx, recv, &args).is_err());
            host.initializing = true;
            let args = [s(cx, "pain"), Value::Int(1)];
            assert_eq!(
                shell_shock(&mut host, cx, recv, &args),
                Err(ErrorKind::Custom(
                    "shellshock 'pain' was not precached\n".into()
                ))
            );

            stop_shell_shock(&mut host, cx, recv, &[]).unwrap();
            assert_eq!(host.client_shellshock[0], [0; 3]);
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

    /// `isOnGround` answers the host's mirror of `ps.on_ground`, and refuses
    /// a receiver with no `gclient_t` the way every other client builtin does.
    #[test]
    fn is_on_ground_reads_the_host_mirror_and_refuses_a_non_client() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let recv = Some(Target::Entity(c));
            host.client_on_ground[0] = true;
            assert_eq!(
                is_on_ground(&mut host, cx, recv, &[]).unwrap(),
                Value::Int(1)
            );
            host.client_on_ground[0] = false;
            assert_eq!(
                is_on_ground(&mut host, cx, recv, &[]).unwrap(),
                Value::Int(0)
            );

            let prop = host.ents.spawn(cx).unwrap();
            let non_client = Some(Target::Entity(prop));
            assert!(is_on_ground(&mut host, cx, non_client, &[]).is_err());
        });
    }

    /// `isLookingAt` answers the host's mirror of the last aim trace against
    /// the entity asked about, 0 for any other argument, and refuses a
    /// receiver with no `gclient_t`.
    #[test]
    fn is_looking_at_reads_the_host_mirror_of_the_aim_trace() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let recv = Some(Target::Entity(c));
            let zone = host.ents.spawn(cx).unwrap();
            let other = host.ents.spawn(cx).unwrap();
            host.client_lookat[0] = Some(zone);
            let ask = |host: &mut GameHost, cx: &mut Cx, args: &[Value]| {
                is_looking_at(host, cx, recv, args).unwrap()
            };
            assert_eq!(ask(&mut host, cx, &[Value::Entity(zone)]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, &[Value::Entity(other)]), Value::Int(0));
            assert_eq!(ask(&mut host, cx, &[Value::Int(3)]), Value::Int(0));
            host.client_lookat[0] = None;
            assert_eq!(ask(&mut host, cx, &[Value::Entity(zone)]), Value::Int(0));

            let non_client = Some(Target::Entity(other));
            assert!(is_looking_at(&mut host, cx, non_client, &[Value::Entity(zone)]).is_err());
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
    /// `Stance::height`) and the link grows it a unit, so two boxes 31 apart
    /// on one axis still touch and two 40 apart do not.
    ///
    /// The answer comes off the tree's links, not the entities' `origin`
    /// fields: a client set down without a link is not there, and one whose
    /// `pm_type` reads 6 or more is skipped (0x6a89f).
    #[test]
    fn a_spawn_point_inside_another_player_would_telefrag() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let body = vcod_common::movetrace::CONTENTS_BODY as i32;
            let a = host.ents.spawn_client(cx, 0, None).unwrap();
            set_origin(&mut host, cx, a, [0.0, 0.0, 0.0]);
            host.link_client(0, [0.0; 3], host.client_box(0), body, true);
            let b = host.ents.spawn_client(cx, 1, None).unwrap();
            set_origin(&mut host, cx, b, [512.0, 0.0, 0.0]);
            host.link_client(1, [512.0, 0.0, 0.0], host.client_box(1), body, true);
            let c = host.ents.spawn_client(cx, 2, None).unwrap();
            set_origin(&mut host, cx, c, [0.0, 512.0, 0.0]);
            let recv = Some(Target::Entity(b));
            let ask = |host: &mut GameHost, cx: &mut Cx, at: [f32; 3]| {
                position_would_telefrag(host, cx, recv, &[Value::Vector(at)]).unwrap()
            };

            assert_eq!(ask(&mut host, cx, [0.0, 0.0, 0.0]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, [24.0, 0.0, 0.0]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, [31.0, 0.0, 0.0]), Value::Int(1));
            assert_eq!(ask(&mut host, cx, [40.0, 0.0, 0.0]), Value::Int(0));
            assert_eq!(ask(&mut host, cx, [512.0, 512.0, 0.0]), Value::Int(0));
            // `c`'s origin says it stands here, but it never linked.
            assert_eq!(ask(&mut host, cx, [0.0, 512.0, 0.0]), Value::Int(0));
            host.client_pm_type[0] = 6;
            assert_eq!(ask(&mut host, cx, [0.0, 0.0, 0.0]), Value::Int(0));
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
            let e = host.ents.spawn_client(cx, 0, None).unwrap();
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
            let e = host.ents.spawn_client(cx, 3, None).unwrap();
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

    /// `openMenuNoMouse` sends the slot with a third argument, `t %i 1`
    /// (0x454b4), which the cgame reads as the no-mouse popup.
    #[test]
    fn open_menu_no_mouse_sends_the_slot_and_a_one() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 2, None).unwrap();
            host.allocators
                .index(&mut host.configstrings, CsRange::Menu, "weapon_russian")
                .unwrap();
            let menu = Value::String(cx.intern_exact("weapon_russian"));
            let r = open_menu_no_mouse(&mut host, cx, Some(Target::Entity(e)), &[menu]);
            assert!(matches!(r, Ok(Value::Int(1))));
            assert_eq!(host.client_commands, vec![(2, "t 0 1".to_string())]);
        });
    }

    /// A localized value takes its substitution arguments with it, which is
    /// what names the winner in the map-end banner: retail's capture reads
    /// `v cg_objectiveText "MPSCRIPT_WINS\x15vcod^7"` for
    /// `setClientCvar("cg_objectiveText", &"MPSCRIPT_WINS", playername)`
    /// (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`).
    #[test]
    fn a_localized_value_carries_its_substitution_arguments() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 0, None).unwrap();
            let name = Value::String(cx.intern_exact("vcod"));
            if let Some(c) = host.ents.get_mut(e).and_then(|x| x.client.as_mut()) {
                c[0] = name;
            }
            let cvar = Value::String(cx.intern_exact("cg_objectiveText"));
            let text = Value::Localized(cx.intern_exact("MPSCRIPT_WINS"));
            set_client_cvar(
                &mut host,
                cx,
                Some(Target::Entity(e)),
                &[cvar, text, Value::Entity(e)],
            )
            .unwrap();
            assert_eq!(
                host.client_commands,
                vec![(
                    0,
                    "v cg_objectiveText \"MPSCRIPT_WINS\u{15}vcod^7\"".to_string()
                )]
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
            let e = host.ents.spawn_client(cx, 0, None).unwrap();
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
            let client = host.ents.spawn_client(cx, 0, None).unwrap();
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
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
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
            let c = host.ents.spawn_client(cx, 3, None).unwrap();
            close_menu(&mut host, cx, Some(Target::Entity(c)), &[]).unwrap();
            assert_eq!(host.client_commands, vec![(3, "u".to_string())]);
            let prop = host.ents.spawn(cx).unwrap();
            assert!(close_menu(&mut host, cx, Some(Target::Entity(prop)), &[]).is_err());
        });
    }

    /// `Drop_Weapon` through `LaunchItem`: the carbine leaves the player's
    /// hands with its rounds, in the air with no ground yet, names its
    /// dropper in `clientNum` for 1000 ms, takes a ring slot, and comes back
    /// as the builtin's return value.
    #[test]
    fn dropitem_drops_the_weapon_with_its_rounds_and_the_owner_lockout() {
        let (mut vm, mut host) = fixture();
        host.weapons = std::rc::Rc::new(crate::game::pickup::tests_table());
        host.level_time_ms = 5_000;
        let carbine = weapon_index("m1carbine_mp").unwrap();
        let (clip_index, ammo_index) = {
            let d = host.weapons.get(carbine).unwrap();
            (d.clip_index, d.ammo_index)
        };
        host.client_weapons[0].give(carbine, 1);
        host.client_ammo[0].ammo[ammo_index] = 400;
        host.client_ammo[0].clip[clip_index] = 12;
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            set_origin(&mut host, cx, c, [16.0, -32.0, 8.0]);
            let name = Value::String(cx.intern_exact("m1carbine_mp"));
            let Value::Entity(id) =
                drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap()
            else {
                panic!("dropItem returns the item");
            };
            let item = host.ents.get(id).unwrap().item.unwrap();
            assert_eq!(item.index as usize, carbine);
            assert_eq!(item.clip, 12);
            assert_eq!(item.owner, Some(0));
            assert!(item.dropped);
            assert_eq!(item.ground, 0, "a dropItem drop is in the air");
            assert!(item.pos.is_some());
            let count = cx.intern_folded("count");
            assert_eq!(host.get_field(cx, id, count), Value::Int(400));
            let e = host.ents.get(id).unwrap();
            assert_eq!(e.think, Some(ThinkFn::ClearOwner));
            assert_eq!(e.nextthink, 6_000);
        });
        assert!(!host.client_weapons[0].holds(carbine));
        assert_eq!(host.client_ammo[0].ammo[ammo_index], 0);
        assert_eq!(host.client_ammo[0].clip[clip_index], 0);
        assert!(host.ents.run_thinks(6_000).is_empty());
        let (id, _) = host
            .ents
            .iter_inuse()
            .find(|(_, e)| e.item.is_some())
            .unwrap();
        assert_eq!(host.ents.get(id).unwrap().item.unwrap().owner, None);
    }

    /// `Drop_Weapon` on a weapon the player does not hold: no item, no ammo
    /// op, and the player's weapons as they were.
    #[test]
    fn dropitem_of_a_weapon_not_held_drops_nothing() {
        let (mut vm, mut host) = fixture();
        host.weapons = std::rc::Rc::new(crate::game::pickup::tests_table());
        let carbine = weapon_index("m1carbine_mp").unwrap();
        host.client_weapons[0].give(carbine, 1);
        let held = host.client_weapons[0];
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let name = Value::String(cx.intern_exact("fg42_mp"));
            assert_eq!(
                drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap(),
                Value::Undefined
            );
        });
        assert_eq!(host.ents.iter_inuse().count(), 1);
        assert!(host.client_weapon_ops.is_empty());
        assert_eq!(host.client_weapons[0], held);
    }

    /// A name neither a weapon file nor `bg_itemlist` backs drops nothing
    /// and returns undefined: `BG_FindItem`'s miss skips `Drop_Item`
    /// (0x43743).
    #[test]
    fn dropitem_of_a_name_nothing_backs_drops_nothing() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let name = Value::String(cx.intern_exact("blunderbuss_mp"));
            assert_eq!(
                drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap(),
                Value::Undefined
            );
            assert_eq!(host.ents.iter_inuse().count(), 1, "no item was spawned");
        });
    }

    /// A `bg_itemlist` row that is no weapon goes to `Drop_Item`: launched
    /// with nothing taken off the player and no spin.
    #[test]
    fn dropitem_of_health_launches_it_without_a_spin() {
        let (mut vm, mut host) = fixture();
        let held = host.client_weapons[0];
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let name = Value::String(cx.intern_exact("item_health"));
            let Value::Entity(id) =
                drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap()
            else {
                panic!("dropItem returns the item");
            };
            let item = host.ents.get(id).unwrap().item.unwrap();
            assert_eq!(item.index, 68);
            assert!(item.pos.is_some() && item.apos.is_none());
            let cn = cx.intern_folded("classname");
            let Value::String(s) = host.get_field(cx, id, cn) else {
                panic!("a classname");
            };
            assert_eq!(cx.resolve(s), "item_health");
        });
        assert_eq!(host.client_weapons[0], held);
    }

    /// `"none"` is what `getCurrentWeapon` reports for a holstered player,
    /// and it drops nothing at all: no entity, no ammo op, no error.
    #[test]
    fn dropitem_none_drops_nothing_and_does_not_raise() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let name = Value::String(cx.intern_exact("none"));
            assert_eq!(
                drop_item(&mut host, cx, Some(Target::Entity(c)), &[name]).unwrap(),
                Value::Undefined
            );
            assert_eq!(host.ents.iter_inuse().count(), 1, "no item was spawned");
            assert!(host.client_weapon_ops.is_empty(), "no ammo was cleared");
        });
    }

    /// Why that no-op matters: stock `dm.gsc:531` is
    /// `self dropItem(self getcurrentweapon());` with no guard, so a player
    /// killed while holstered hands `"none"` in. Raising there killed the
    /// callback thread before its `cloneplayer`, `wait` and `respawn()`, and
    /// the player stayed dead for the rest of the map.
    #[test]
    fn a_killed_callback_that_drops_none_runs_past_it() {
        use crate::game::combat::Hit;
        use crate::game::host::{ClientEvent, Vitals};
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};

        const CALLBACKS: &str = r#"
            main() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
                self dropItem("none");
                self.buried = "yes";
            }
        "#;

        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, CALLBACKS);
        for (slot, name) in [(0, "victim"), (1, "killer")] {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: name.into(),
            });
        }
        rt.run_frame(0);
        rt.host.client_vitals[0] = Vitals {
            health: 10,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        rt.deliver_hits(vec![Hit {
            victim: 0,
            attacker: 1,
            inflictor: None,
            damage: 45,
            dflags: 0,
            mod_: "MOD_RIFLE_BULLET",
            weapon: "m1carbine_mp".into(),
            point: [0.0; 3],
            dir: [1.0, 0.0, 0.0],
            hitloc: "torso_upper",
        }]);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "buried").as_deref(), Some("yes"));
    }

    /// `cloneplayer` copies the mirror `Server::replay_moves` wrote into the
    /// body queue's next slot. A client with no mirror -- connected but never
    /// in the world -- clones nothing.
    #[test]
    fn cloneplayer_pushes_the_mirrored_state_into_the_body_queue() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 2, None).unwrap();
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
            let Value::Entity(id) = clone_player(&mut host, cx, recv, &[]).unwrap() else {
                panic!("the corpse comes back as an entity");
            };

            let (number, body) = host.bodies.entities().next().unwrap();
            assert_eq!(number, crate::game::bodies::BODY_FIRST);
            assert_eq!(id.0, number);
            assert_eq!(body.field_i32(p, "eType"), crate::game::bodies::ET_CORPSE);
            assert_eq!(body.field_i32(p, "clientNum"), 2);
            assert_eq!(body.field_i32(p, "legsAnim"), 700);

            // `delete`'s free takes it off the wire.
            host.free_entity(id);
            assert_eq!(host.bodies.entities().count(), 0);
        });
    }

    /// A host with the shipped weapon files mounted: the slot builtins
    /// resolve a slot to the weapon standing in it, and the ops they push
    /// carry the ammo and clip indexes the table hands out. `None` without
    /// the paks, where no weapon file fills a slot at all.
    fn armed_fixture() -> Option<(vcod_gsc::Vm, GameHost)> {
        let fs = std::rc::Rc::new(vcod_common::testing::game_fs()?);
        let (vm, mut host) = fixture();
        host.weapons = std::rc::Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        host.fs = Some(fs);
        Some((vm, host))
    }

    /// `BG_GivePlayerWeapon` registers what it gives: past
    /// `level.initializing` a weapon nobody precached is `RegisterItem`'s
    /// script error and is not given, while one already registered (or
    /// already held) goes through.
    #[test]
    fn giving_an_unregistered_weapon_past_initialization_is_an_error() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 0, None).unwrap();
            let t = Some(Target::Entity(e));
            let colt = Value::String(cx.intern_exact("colt_mp"));
            give_weapon(&mut host, cx, t, &[colt]).unwrap();
            host.initializing = false;
            host.client_weapons[0] = Default::default();
            give_weapon(&mut host, cx, t, &[colt]).unwrap();
            let mp40 = Value::String(cx.intern_exact("mp40_mp"));
            assert_eq!(
                give_weapon(&mut host, cx, t, &[mp40]).err(),
                Some(ErrorKind::Custom(
                    "game tried to register the item 'mp40_mp' after initialization finished"
                        .into()
                ))
            );
            assert!(!host.client_weapons[0].holds(weapon_index("mp40_mp").unwrap()));
        });
    }

    /// The stock allies loadout on client 0, with the ops it pushed dropped:
    /// what the three tests below start from.
    fn loadout(host: &mut GameHost, cx: &mut Cx) -> EntId {
        let e = host.ents.spawn_client(cx, 0, None).unwrap();
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

            // `sd.gsc`'s 999 lands as the carbine file's `maxAmmo` 400 and
            // `clipSize` 15: both setters clamp (retail 0x442a4, 0x444ed).
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
                            rounds: 400
                        }
                    ),
                    (
                        0,
                        WeaponOp::SetClip {
                            clip_index,
                            rounds: 15
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

    /// `setWeaponSlotWeapon` is what precedes both setters in stock script
    /// (`sd.gsc` 540-543): it gives the weapon and puts it in the named slot,
    /// loaded, so the `switchToWeapon` on the line after it has something to
    /// switch to. Without it that whole sequence dies on the first line.
    #[test]
    fn setweaponslotweapon_gives_the_weapon_and_fills_the_slot() {
        let Some((mut vm, mut host)) = armed_fixture() else {
            return;
        };
        let thompson = weapon_index("thompson_mp").unwrap();
        let (clip_size, start_ammo) = {
            let def = host.weapons.get(thompson).expect("the thompson");
            (def.clip_size as i16, def.start_ammo as i16)
        };
        vm.with_cx(|cx| {
            let e = loadout(&mut host, cx);
            let t = Some(Target::Entity(e));
            // Never given, so there is nothing to switch to yet.
            let name = Value::String(cx.intern_exact("thompson_mp"));
            assert!(matches!(
                switch_to_weapon(&mut host, cx, t, &[name]),
                Ok(Value::Int(0))
            ));

            let primary = Value::String(cx.intern_exact("primary"));
            set_weapon_slot_weapon(&mut host, cx, t, &[primary, name]).unwrap();
            match get_weapon_slot_weapon(&mut host, cx, t, &[primary]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "thompson_mp"),
                v => panic!("{v:?}"),
            }
            assert!(host.client_weapons[0].holds(thompson));
            // `BG_GivePlayerWeapon` holds the select-fire mode beside it.
            let semi = weapon_index("thompson_semi_mp").unwrap();
            assert!(host.client_weapons[0].holds(semi));
            assert!(!host.client_weapons[0].slots.contains(&(semi as u8)));
            switch_to_weapon(&mut host, cx, t, &[name]).unwrap();

            // A bad slot name and a weapon no file backs are both errors.
            let bogus = Value::String(cx.intern_exact("holster"));
            assert!(set_weapon_slot_weapon(&mut host, cx, t, &[bogus, name]).is_err());
            let nothing = Value::String(cx.intern_exact("not_a_weapon_mp"));
            assert!(set_weapon_slot_weapon(&mut host, cx, t, &[primary, nothing]).is_err());
        });
        assert_eq!(
            host.client_weapon_ops,
            vec![
                (
                    0,
                    WeaponOp::SetClip {
                        clip_index: host.weapons.get(thompson).unwrap().clip_index,
                        rounds: clip_size
                    }
                ),
                (
                    0,
                    WeaponOp::SetAmmo {
                        ammo_index: host.weapons.get(thompson).unwrap().ammo_index,
                        rounds: start_ammo
                    }
                ),
            ]
        );
    }

    /// `"none"` is weapon index 0, not an unknown weapon: the slot empties
    /// and whatever stood in it is dropped. `sd.gsc`'s `spawnPlayer` re-gives
    /// the `pers["weapon2"]` `endRound` stashed, which reads `"none"` for a
    /// player carrying one primary, and an error there kills the whole spawn
    /// (object model doc, section 20).
    #[test]
    fn setweaponslotweapon_takes_the_slot_and_none_leaves_it_empty() {
        let Some((mut vm, mut host)) = armed_fixture() else {
            return;
        };
        let thompson = weapon_index("thompson_mp").unwrap();
        vm.with_cx(|cx| {
            let e = loadout(&mut host, cx);
            let t = Some(Target::Entity(e));
            let primary = Value::String(cx.intern_exact("primary"));
            let name = Value::String(cx.intern_exact("thompson_mp"));
            set_weapon_slot_weapon(&mut host, cx, t, &[primary, name]).unwrap();
            assert!(host.client_weapons[0].holds(thompson));

            let none = Value::String(cx.intern_exact("none"));
            set_weapon_slot_weapon(&mut host, cx, t, &[primary, none]).unwrap();
            assert!(!host.client_weapons[0].holds(thompson));
            match get_weapon_slot_weapon(&mut host, cx, t, &[primary]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "none"),
                v => panic!("{v:?}"),
            }
            // An already empty slot emptied again is a no-op, not an error.
            set_weapon_slot_weapon(&mut host, cx, t, &[primary, none]).unwrap();
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
        crate::server::apply_weapon_op(&mut sim, host.client_weapon_ops[0].1);
        assert_eq!(sim.ps.weapons_held, 0);
        assert_eq!(sim.ps.weapon_slots, [0; crate::weapons::NUM_SLOTS]);
        assert_eq!(sim.ps.ammo, [0; vcod_common::pmove::weapon::NUM_AMMO]);
        assert_eq!(sim.ps.ammoclip, [0; vcod_common::pmove::weapon::NUM_AMMO]);
    }

    /// `switchToWeapon` sends the client `a <colt>` and queues nothing for
    /// the sim: the cmds that then name the colt put the carbine away and
    /// raise the colt, `ps.weapon` reading it only from the raise on. The
    /// host's own copy is left alone -- the mirror would otherwise force the
    /// target weapon in ahead of the putaway.
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
            assert!(matches!(
                switch_to_weapon(&mut host, cx, t, &[arg]),
                Ok(Value::Int(1))
            ));
            // Never given: 0, and nothing sent (0x44cc4).
            let sten = Value::String(cx.intern_exact("sten_mp"));
            assert!(matches!(
                switch_to_weapon(&mut host, cx, t, &[sten]),
                Ok(Value::Int(0))
            ));
        });
        assert_eq!(host.client_commands, vec![(0, format!("a {colt}"))]);
        assert!(host.client_weapon_ops.is_empty());
        assert_eq!(
            host.client_weapons[0].current, carbine as u8,
            "the machine picks the weapon up, not the builtin"
        );

        let world = vcod_common::collision::test_world(&[]);
        let world = vcod_common::movetrace::MoveWorld::bare(&world);
        // The cgame selects what `a` names and asks for it in every cmd.
        let cmd = vcod_common::net::msg::UserCmd {
            weapon: colt as u8,
            ..vcod_common::net::msg::NULL_USERCMD
        };
        let mut sim = player_sim();
        sim.ps.weapons_held = host.client_weapons[0].held;
        sim.ps.weapon_slots = host.client_weapons[0].slots;
        sim.ps.weapon = host.client_weapons[0].current;
        let events = sim.step(&cmd, 0.05, Some(world), host.weapons.defs());
        assert_eq!(events[0].event, EV_PUTAWAY_WEAPON);
        assert_eq!(sim.ps.weapon, carbine as u8, "still the carbine, mid-drop");

        let mut raised = Vec::new();
        for _ in 0..40 {
            let events = sim.step(&cmd, 0.05, Some(world), host.weapons.defs());
            raised.extend(events.into_iter().map(|e| e.event));
        }
        assert!(raised.contains(&EV_RAISE_WEAPON), "raised: {raised:?}");
        assert!(!raised.contains(&EV_PUTAWAY_WEAPON), "one putaway, not two");
        assert_eq!(sim.ps.weapon, colt as u8);
    }

    /// `setPlayerAngles` reaches the sim as a queued op (the one `Server`
    /// applies with `ClientSim::set_view_angle`), and the script's own
    /// `angles` moves at once, as `SetClientViewAngle` writes
    /// `r.currentAngles` (0x4209f).
    #[test]
    fn setplayerangles_queues_the_view_for_the_sim() {
        use crate::game::host::ClientEvent;
        use crate::game::script::ScriptRuntime;
        let mut rt = ScriptRuntime::for_test(
            r#"
            main() {}
            look() {
                self setPlayerAngles((10, 90, 0));
                self.seen = self.angles[1];
            }
        "#,
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "p".into(),
        });
        rt.run_frame(0);
        let e = rt.client_entity(0).unwrap();
        rt.start_thread_for_test(e, "look");
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "seen").as_deref(), Some("90"));
        let ops = rt.take_sim_ops();
        assert_eq!(
            ops,
            vec![(
                0,
                SimOp::SetViewAngles {
                    angles: [10.0, 90.0, 0.0]
                }
            )]
        );
        let mut sim = crate::spectate::ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        let SimOp::SetViewAngles { angles } = ops[0].1 else {
            unreachable!()
        };
        sim.set_view_angle(angles);
        assert_eq!(sim.view_angles(), [10.0, 90.0, 0.0]);
    }

    /// On a prone player `SetClientViewAngle` stores the angles its prone
    /// arm held, not the script's, in `r.currentAngles` (1.1 0x4209f), so
    /// the thread reads them back held; the sim holds its view the same.
    #[test]
    fn setplayerangles_on_a_prone_player_reads_back_held() {
        use crate::game::host::ClientEvent;
        use crate::game::script::ScriptRuntime;
        use vcod_common::pmove::{MoveRules, ProneCone};
        let mut rt = ScriptRuntime::for_test(
            r#"
            main() {}
            look() {
                self setPlayerAngles((60, 100, 0));
                self.pitch = self.angles[0];
                self.yaw = self.angles[1];
            }
        "#,
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "p".into(),
        });
        rt.run_frame(0);
        let cone = ProneCone {
            direction: 10.0,
            torso_pitch: 0.0,
            rules: MoveRules::default(),
        };
        rt.set_client_prone_cone(0, Some(cone));
        let e = rt.client_entity(0).unwrap();
        rt.start_thread_for_test(e, "look");
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        let held = cone.hold([60.0, 100.0, 0.0]);
        // 55 and 15 degrees, each a whole number of shorts.
        assert!((held[1] - 55.0).abs() < 0.01 && (held[0] - 15.0).abs() < 0.01);
        let mut read = |f: &str| rt.client_field(0, f).unwrap().parse::<f32>().unwrap();
        // The field prints `%g`, six digits.
        let (pitch, yaw) = (read("pitch"), read("yaw"));
        assert!((pitch - held[0]).abs() < 1e-3, "{pitch}");
        assert!((yaw - held[1]).abs() < 1e-3, "{yaw}");
        let mut sim = crate::spectate::ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.become_player([0.0; 3], 0.0, [0; 3]);
        sim.ps.stance = vcod_common::pmove::Stance::Prone;
        sim.ps.prone_direction = 10.0;
        assert_eq!(ProneCone::of(&sim.ps), Some(cone));
        sim.set_view_angle([60.0, 100.0, 0.0]);
        assert_eq!(sim.view_angles(), held);
    }

    /// `SetClientViewAngle` reads the live playerstate's prone flag and
    /// cone (1.1 0x41e30), so a `spawn` earlier in the same thread, whose
    /// `ClientSpawn` stands the body up, leaves the next `setPlayerAngles`
    /// unheld. `spawn` is the only builtin that moves the cone, and it
    /// rewrites the host's mirror from its own think.
    #[test]
    fn setplayerangles_after_a_spawn_in_the_same_thread_reads_the_new_stance() {
        use crate::game::host::ClientEvent;
        use crate::game::script::ScriptRuntime;
        use vcod_common::pmove::{MoveRules, ProneCone};
        let mut rt = ScriptRuntime::for_test(
            r#"
            main() {}
            look() {
                self spawn((0, 0, 0), (0, 0, 0));
                self setPlayerAngles((60, 100, 0));
                self.pitch = self.angles[0];
                self.yaw = self.angles[1];
            }
        "#,
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "p".into(),
        });
        rt.run_frame(0);
        rt.set_client_prone_cone(
            0,
            Some(ProneCone {
                direction: 10.0,
                torso_pitch: 0.0,
                rules: MoveRules::default(),
            }),
        );
        let e = rt.client_entity(0).unwrap();
        rt.start_thread_for_test(e, "look");
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        let mut read = |f: &str| rt.client_field(0, f).unwrap().parse::<f32>().unwrap();
        assert!((read("yaw") - 100.0).abs() < 1e-3);
        assert!((read("pitch") - 60.0).abs() < 1e-3);
    }

    /// `freezeControls` checks its receiver, reads its bool and stores it
    /// where nothing reads it, so a frozen player moves on.
    #[test]
    fn freezecontrols_takes_a_bool_and_freezes_nothing() {
        use crate::game::host::ClientEvent;
        use crate::game::script::ScriptRuntime;
        let mut rt = ScriptRuntime::for_test(
            r#"
            main() {}
            freeze() {
                self freezeControls(true);
                self.frozen = 1;
            }
        "#,
        );
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "p".into(),
        });
        rt.run_frame(0);
        let e = rt.client_entity(0).unwrap();
        rt.start_thread_for_test(e, "freeze");
        rt.run_frame(50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "frozen").as_deref(), Some("1"));
        assert!(rt.take_sim_ops().is_empty());
    }

    /// 1.5's `allowSpectateTeam` sets a refused team's bit and clears an
    /// allowed one's, `G_ClientCanSpectateTeam`'s `1 << team` layout.
    #[test]
    fn allow_spectate_team_keeps_the_refused_bits() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            let recv = Some(Target::Entity(c));
            let mut call = |host: &mut GameHost, team: &str, allowed: bool| {
                let team = Value::String(cx.intern_exact(team));
                allow_spectate_team(host, cx, recv, &[team, Value::Int(i32::from(allowed))])
            };
            for team in ["allies", "axis", "freelook", "none"] {
                call(&mut host, team, false).unwrap();
            }
            assert_eq!(host.client_no_spectate[0], 0x17);
            call(&mut host, "allies", true).unwrap();
            assert_eq!(host.client_no_spectate[0], 0x13);
            assert!(call(&mut host, "spectator", true).is_err());
        });
    }

    /// `getGuid` is the authorize server's guid, 0 with none; the autodemo
    /// trio sends its command and returns 1 only under its cvar.
    #[test]
    fn the_15_client_builtins() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 2, None).unwrap();
            let recv = Some(Target::Entity(c));
            assert_eq!(get_guid(&mut host, cx, recv, &[]).unwrap(), Value::Int(0));
            assert_eq!(
                auto_demo_start(&mut host, cx, recv, &[]).unwrap(),
                Value::Undefined
            );
            assert!(host.client_commands.is_empty());
            host.cvars.set("g_autodemo", "1");
            assert_eq!(
                auto_demo_start(&mut host, cx, recv, &[]).unwrap(),
                Value::Int(1)
            );
            auto_demo_stop(&mut host, cx, recv, &[]).unwrap();
            auto_screenshot(&mut host, cx, recv, &[]).unwrap();
            assert_eq!(host.client_commands, [(2, "C".into()), (2, "D".into())]);
            disable_weapon(&mut host, cx, recv, &[]).unwrap();
            enable_weapon(&mut host, cx, recv, &[]).unwrap();
            assert!(matches!(
                host.client_sim_ops.as_slice(),
                [
                    (2, SimOp::WeaponDisabled(true)),
                    (2, SimOp::WeaponDisabled(false))
                ]
            ));
        });
    }
}
