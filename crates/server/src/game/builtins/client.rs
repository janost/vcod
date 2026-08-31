//! The two per-client server commands the stock join sequence sends.
//! Neither writes game state: each renders one reliable command and queues
//! it on `GameHost::client_commands`, which `Server` drains after
//! `run_frame`. A builtin cannot reach the netchan, and must not reenter the
//! VM, so queueing is the only route out.
//!
//! Both wire formats are measured, not inferred: the `v %s "%s"` and `t %i`
//! format strings are in `game.mp.i386.so`, and
//! `docs/research/cod11-hud-protocol.md` section 0.1 carries the live
//! capture of a stock dm join that produced them.

use crate::configstrings::script_menu_index;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] =
    &[("setclientcvar", set_client_cvar), ("openmenu", open_menu)];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The receiver both builtins need: an entity with a `gclient_t`, which is
/// retail's own `entity %i is not a player` check (0x44703, 0x45413). The
/// client's slot is its entity number, and that is what the reliable command
/// is addressed to.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::CsRange;
    use crate::game::testing::fixture;

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
