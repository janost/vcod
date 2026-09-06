//! Sound builtins. All three allocate a sound-alias configstring, mirroring
//! `G_SoundAliasIndex` (docs/research/cod11-sound-system.md: they share the
//! 525-779 range). `playSound` and `playLoopSound` queue no audible event:
//! that rides `es.event`/`es.loopSound` on the wire, which needs an entity
//! state stage 5 builds. `playLocalSound` is the one that reaches a client,
//! as the reliable command `s <idx>`.

use crate::configstrings::CsRange;
use crate::game::builtins::client::client_receiver;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("playsound", play_sound),
    ("playloopsound", play_loop_sound),
    ("playlocalsound", play_local_sound),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The alias's configstring slot, allocated on first use the way retail's
/// `G_SoundAliasIndex` allocates it.
fn alloc_alias(host: &mut GameHost, cx: &mut Cx, args: &[Value]) -> Result<usize, ErrorKind> {
    let Some(Value::String(alias)) = args.first() else {
        return Err(ErrorKind::BadType("takes a sound alias"));
    };
    let alias = *alias;
    let text = cx.resolve(alias).to_string();
    host.allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, &text)
}

/// `CS_SOUNDS`, what an `s <idx>` index counts from: the alias range starts
/// one slot above it, so the first alias travels as 1
/// (docs/protocol-1.1.md, `s <idx>`).
const CS_SOUNDS: usize = 524;

/// `<ent> playSound(alias)`: `G_SoundAliasIndex` -> `G_PlaySoundAlias`.
pub fn play_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let _id = entity_receiver(recv)?;
    alloc_alias(host, cx, args)?;
    Ok(Value::Undefined)
}

/// `<ent> playLoopSound(alias)`: `es.loopSound = idx` in retail; stage 5
/// gives entities an `es` to write it into.
pub fn play_loop_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let _id = entity_receiver(recv)?;
    alloc_alias(host, cx, args)?;
    Ok(Value::Undefined)
}

/// `<player> playLocalSound(alias)` (sound doc, section 9): the reliable
/// command `s <idx>` to that client and nobody else, non-positional. The
/// receiver must be a player, as retail's own check is.
pub fn play_local_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let idx = alloc_alias(host, cx, args)? - CS_SOUNDS;
    host.client_commands.push((slot, format!("s {idx}")));
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `playSound` allocates a sound-alias configstring, mirroring
    /// `G_SoundAliasIndex`.
    #[test]
    fn playsound_allocates_a_sound_alias_configstring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let a = Value::String(cx.intern_exact("minefield_click"));
            play_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "minefield_click");
        });
    }

    /// `playLocalSound` is `s <idx>` to the receiving client alone, and the
    /// index counts from `CS_SOUNDS` rather than from the alias range: the
    /// first alias travels as 1.
    #[test]
    fn playlocalsound_sends_s_idx_to_that_client_only() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 3, None).unwrap();
            let a = Value::String(cx.intern_exact("MP_announcer_allies_win"));
            play_local_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.configstrings[525], "MP_announcer_allies_win");
            assert_eq!(host.client_commands, vec![(3, "s 1".to_string())]);
            // A second call on the same alias reuses the slot.
            play_local_sound(&mut host, cx, Some(Target::Entity(e)), &[a]).unwrap();
            assert_eq!(host.client_commands.len(), 2);
            assert_eq!(host.client_commands[1], (3, "s 1".to_string()));
            // Not a player: retail's own `ent->client` check.
            let plain = host.ents.spawn(cx).unwrap();
            assert!(play_local_sound(&mut host, cx, Some(Target::Entity(plain)), &[a]).is_err());
        });
    }
}
