//! Effect builtins. `loadFX` allocates a real effect configstring, mirroring
//! `G_EffectIndex`. `playFX`/`playFXOnTag` trigger an effect at a point or
//! on an attach tag; retail plays it through an `EV_*` on the entity, which
//! needs the wire path stage 5 builds. Until then there is no channel for a
//! played effect to leave the builtin through, so both validate their
//! arguments and stop: honest about doing nothing observable rather than
//! inventing a queue this stage has no reader for.

use crate::configstrings::CsRange;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("loadfx", load_fx),
    ("playfx", play_fx),
    ("playfxontag", play_fx_on_tag),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// `loadFX(path)`: intern the effect path into the effect configstring
/// range and hand back the slot, which is what `level._effect[...]` stores
/// and `playFX` takes back.
pub fn load_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(path)) = args.first() else {
        return Err(ErrorKind::BadType("loadFX takes an effect path"));
    };
    let path = *path;
    let text = cx.resolve(path).to_string();
    let slot = host
        .allocators
        .index(&mut host.configstrings, CsRange::Effect, &text)?;
    Ok(Value::Int(slot as i32))
}

/// `playFX(fx, origin)`: a global call, no receiver. `fx` is a slot handed
/// back by `loadFX`.
pub fn play_fx(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Int(_)), Some(Value::Vector(_))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType(
            "playFX takes an fx handle and an origin",
        ));
    };
    Ok(Value::Undefined)
}

/// `self playFXOnTag(fx, tag)`: same effect handle, anchored to an attach
/// tag instead of a point. The tag name still goes through `G_TagIndex`
/// (`CsRange::Tag`), which is real work this stage can do even though the
/// effect itself has nowhere to go yet.
pub fn play_fx_on_tag(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let _id = entity_receiver(recv)?;
    let (Some(Value::Int(_)), Some(Value::String(tag))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType(
            "playFXOnTag takes an fx handle and a tag",
        ));
    };
    let tag = *tag;
    let text = cx.resolve(tag).to_string();
    host.allocators
        .index(&mut host.configstrings, CsRange::Tag, &text)?;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `loadFX` interns the effect path into the effect configstring range
    /// and hands back the slot, which is what `level._effect[...]` stores
    /// and `playFX` takes back.
    #[test]
    fn loadfx_allocates_an_effect_configstring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let p = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            let slot = load_fx(&mut host, cx, None, &[p]).unwrap();
            assert_eq!(slot, Value::Int(781));
            assert_eq!(host.configstrings[781], "fx/impacts/newimps/minefield.efx");
            let again = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            assert_eq!(
                load_fx(&mut host, cx, None, &[again]).unwrap(),
                Value::Int(781)
            );
        });
    }
}
