//! Sound builtins. Both allocate a sound-alias configstring, mirroring
//! `G_SoundAliasIndex` (docs/research/cod11-sound-system.md: `playSound` and
//! `playLoopSound` share the 525-779 range). Neither queues an audible
//! event: that rides `es.event`/`es.loopSound` on the wire, which needs an
//! entity state stage 5 builds. The configstring allocation is real work
//! this stage can do; the rest is honestly nothing yet.

use crate::configstrings::CsRange;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("playsound", play_sound),
    ("playloopsound", play_loop_sound),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

fn alloc_alias(host: &mut GameHost, cx: &mut Cx, args: &[Value]) -> Result<(), ErrorKind> {
    let Some(Value::String(alias)) = args.first() else {
        return Err(ErrorKind::BadType("takes a sound alias"));
    };
    let alias = *alias;
    let text = cx.resolve(alias).to_string();
    host.allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, &text)?;
    Ok(())
}

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
}
