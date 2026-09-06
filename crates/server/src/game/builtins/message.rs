//! `Scr_ConstructMessageString` (`game.mp.i386.so` 0x594d0): the one string a
//! localized message and its substitution arguments are packed into, which
//! `setClientCvar`, `iPrintLn` and the pickup messages all send verbatim.
//!
//! The two separators and where each goes are in
//! `docs/research/cod11-hud-protocol.md`, "Localized strings on the wire".

use crate::game::host::GameHost;
use vcod_gsc::{Cx, Value};

/// The separator before an argument the client substitutes into the key it
/// follows (0x59797, 0x599e5).
const ARG_SEP: char = '\u{15}';

/// The separator between two message parts, either of which the client
/// localizes on its own (0x5967d, 0x59924).
const PART_SEP: char = '\u{14}';

/// Packs `args` the way retail's `Scr_ConstructMessageString` does.
///
/// A localized key joins with [`PART_SEP`]; anything else joins with
/// [`ARG_SEP`] when it follows a key, and a trailing [`ARG_SEP`] closes a
/// message whose last part was a key, which is why a lone `&"KEY"` goes out
/// as `KEY\x15`. A player entity renders as its `.name` with a `^7` colour
/// reset (0x767d9), which is what puts the winner's name in the map-end
/// banner.
pub fn construct(host: &GameHost, cx: &Cx, args: &[Value]) -> String {
    let mut out = String::new();
    // Retail opens with the flag set (0x594dc), which is what gives a leading
    // `\x15` to a message that opens with a number; the empty-buffer check is
    // what keeps a leading `\x14` off one that opens with a key.
    let mut after_key = true;
    for &a in args {
        match a {
            Value::Localized(k) => {
                if !out.is_empty() {
                    out.push(PART_SEP);
                }
                out.push_str(cx.resolve(k));
                after_key = true;
            }
            other => {
                let text = match other {
                    Value::Entity(id) => match client_name(host, cx, id) {
                        Some(n) => format!("{n}^7"),
                        // Retail raises "Entity is not a player" here; a
                        // message is not worth killing the thread over.
                        None => continue,
                    },
                    _ => match cx.format_number(other) {
                        Some(s) => s,
                        None => continue,
                    },
                };
                // A part with letters is another message the client localizes
                // and joins as one; anything else is a substitution argument
                // for the key before it (0x5985c).
                if text.chars().any(char::is_alphabetic) && !matches!(other, Value::Entity(_)) {
                    if !out.is_empty() {
                        out.push(PART_SEP);
                    }
                } else if after_key {
                    out.push(ARG_SEP);
                }
                out.push_str(&text);
                after_key = false;
            }
        }
    }
    if after_key {
        out.push(ARG_SEP);
    }
    out
}

/// A player entity's `.name`, `CLIENT_FIELDS[0]`.
fn client_name(host: &GameHost, cx: &Cx, id: vcod_gsc::EntId) -> Option<String> {
    match host.ents.get(id)?.client.as_ref()?.first()? {
        Value::String(a) => Some(cx.resolve(*a).to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host with one client entity named `vcod` in slot 0, the name the two
    /// retail captures were taken under.
    fn host_with_client(vm: &mut vcod_gsc::Vm) -> GameHost {
        let mut host = GameHost::new(vec![String::new(); 2048]);
        let ents = &mut host.ents;
        let id = vm.with_cx(|cx| {
            let id = ents.spawn_client(cx, 0, None).expect("a client entity");
            let name = Value::String(cx.intern_exact("vcod"));
            if let Some(c) = ents.get_mut(id).and_then(|e| e.client.as_mut()) {
                c[0] = name;
            }
            id
        });
        assert_eq!(id, vcod_gsc::EntId(0));
        host
    }

    /// The three shapes the two retail captures carry:
    /// `DM_KILL_OTHER_PLAYERS\x15` for a bare key,
    /// `MPSCRIPT_WINS\x15vcod^7` for a key and a player, and
    /// `MPSCRIPT_TIME_LIMIT_REACHED\x15` for the `f` line at the time limit
    /// (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`).
    #[test]
    fn a_localized_message_packs_the_way_retail_packs_it() {
        let mut vm = vcod_gsc::Vm::new();
        let host = host_with_client(&mut vm);
        let key = vm.with_cx(|cx| cx.intern_exact("MPSCRIPT_WINS"));
        assert_eq!(
            vm.with_cx(|cx| construct(&host, cx, &[Value::Localized(key)])),
            "MPSCRIPT_WINS\u{15}"
        );
        assert_eq!(
            vm.with_cx(|cx| construct(
                &host,
                cx,
                &[Value::Localized(key), Value::Entity(vcod_gsc::EntId(0))]
            )),
            "MPSCRIPT_WINS\u{15}vcod^7"
        );
    }

    /// A number after a key is a substitution argument (`\x15`), another key
    /// is a second message part (`\x14`): retail's own format strings spell
    /// both (`GAME_PICKUP_HEALTH\x15%i` and `GAME_PICKUP_AMMO\x14%s`).
    #[test]
    fn a_number_argument_and_a_second_key_take_different_separators() {
        let mut vm = vcod_gsc::Vm::new();
        let host = GameHost::new(vec![String::new(); 2048]);
        let (a, b) = vm.with_cx(|cx| (cx.intern_exact("GAME_A"), cx.intern_exact("GAME_B")));
        assert_eq!(
            vm.with_cx(|cx| construct(&host, cx, &[Value::Localized(a), Value::Int(25)])),
            "GAME_A\u{15}25"
        );
        assert_eq!(
            vm.with_cx(|cx| construct(&host, cx, &[Value::Localized(a), Value::Localized(b)])),
            "GAME_A\u{14}GAME_B\u{15}"
        );
    }
}
