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
use crate::game::host::GameHost;
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
    ("positionwouldtelefrag", position_would_telefrag),
    ("setviewmodel", set_view_model),
    ("getviewmodel", get_view_model),
];

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
fn client_receiver(host: &GameHost, recv: Option<Target>) -> Result<usize, ErrorKind> {
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

/// `self giveWeapon(name)` (`PlayerCmd_giveWeapon` 0x43020): hold the weapon
/// and fill the slot its weapon file's `weaponSlot` names. Object model doc,
/// section 20.
///
/// Two of retail's effects are left out. Its ammo top-up cannot reach a
/// client, `ps.ammo` having no netfield in 1.1. And its
/// `Can not give player weapon without having an empty weapon slot` error is
/// not reproduced because whether re-giving a held weapon counts as an
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
    Ok(Value::Undefined)
}

/// `self giveMaxAmmo(name)` (`PlayerCmd_giveMaxAmmo` 0x43134): retail tops up
/// `ps.ammo` and touches nothing else, and `ps.ammo` has no netfield, so
/// there is nothing here for the wire to carry. Object model doc, section 20.
pub fn give_max_ammo(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    client_receiver(host, recv)?;
    weapon_argument(cx, args)?;
    Ok(Value::Undefined)
}

/// `self setSpawnWeapon(name)` (0x452a4): `ps.weapon = index`, for a weapon
/// the player already holds. `ps.weaponstate` is retail's other store and is
/// already 0 in a null playerstate. Object model doc, section 20.
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
    }
    Ok(Value::Undefined)
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
}
