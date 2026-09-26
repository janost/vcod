//! Effect builtins. `loadFX` allocates an effect configstring, mirroring
//! `G_EffectIndex`, and `playFX` raises the temp entity the client plays it
//! off. `playFXOnTag` registers its tag and stops there: its event needs the
//! tagged entity's own ring, which nothing on this server writes yet.

use crate::configstrings::CsRange;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub const EV_PLAY_FX: i32 = 191;
pub const EV_PLAY_FX_DIR: i32 = 192;

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
/// range and hand back its effect id, which is what `level._effect[...]`
/// stores and `playFX` takes back.
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
    let id = host
        .allocators
        .effect_index(&mut host.configstrings, &text)?;
    Ok(Value::Int(id))
}

/// `playFX(fx, origin[, forward])` (`.so` 0x5b148): a global call, no
/// receiver. A `G_TempEntity` at `origin` carrying `EV_PLAY_FX` with the
/// effect id in `eventParm`, or with a forward vector `EV_PLAY_FX_DIR` and
/// its `DirToByte` in `scale` (`docs/research/cod11-events-and-fx.md`,
/// section 2). The effect's own `Sound` block is what the client plays,
/// so without this event a scripted explosion is silent as well as unseen.
pub fn play_fx(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Int(id)), Some(Value::Vector(origin)), 2..=3) =
        (args.first(), args.get(1), args.len())
    else {
        return Err(ErrorKind::BadType(
            "playFX takes an fx handle, an origin and an optional forward vector",
        ));
    };
    let (event, scale) = match args.get(2) {
        Some(Value::Vector(forward)) => {
            let dir = glam::Vec3::from(*forward).normalize_or_zero();
            if dir == glam::Vec3::ZERO {
                return Err(ErrorKind::BadType(
                    "playFx called with (0 0 0) forward direction",
                ));
            }
            let byte = vcod_common::net::events::dir_to_byte(dir.to_array());
            (EV_PLAY_FX_DIR, byte)
        }
        Some(_) => return Err(ErrorKind::BadType("playFX's forward must be a vector")),
        None => (EV_PLAY_FX, 0),
    };
    host.temp_entities.push(TempEntity {
        event,
        parm: id & 0xff,
        surf_type: 0,
        other: 0,
        attacker: 0,
        weapon: 0,
        client_num: 0,
        scale,
        origin: *origin,
        scope: Scope::Pvs,
    });
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
    /// and hands back its effect id, the offset from 780 that `G_EffectIndex`
    /// returns and `EV_PLAY_FX` carries, not the configstring number.
    #[test]
    fn loadfx_allocates_an_effect_configstring_and_returns_its_id() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let p = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            assert_eq!(load_fx(&mut host, cx, None, &[p]).unwrap(), Value::Int(1));
            assert_eq!(host.configstrings[781], "fx/impacts/newimps/minefield.efx");
            let again = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            assert_eq!(
                load_fx(&mut host, cx, None, &[again]).unwrap(),
                Value::Int(1)
            );
        });
    }

    /// `playFX` is a temp entity at the origin: `EV_PLAY_FX` with the id in
    /// `eventParm`, or with a forward vector `EV_PLAY_FX_DIR` and the
    /// vector's `DirToByte` in `scale`. A zero forward is retail's error.
    #[test]
    fn playfx_raises_the_effect_event_at_the_origin() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([10.0, 20.0, 30.0]);
            play_fx(&mut host, cx, None, &[Value::Int(3), at]).unwrap();
            let up = Value::Vector([0.0, 0.0, 5.0]);
            play_fx(&mut host, cx, None, &[Value::Int(3), at, up]).unwrap();
            let zero = Value::Vector([0.0; 3]);
            assert!(play_fx(&mut host, cx, None, &[Value::Int(3), at, zero]).is_err());
        });
        let [plain, dir] = &host.temp_entities[..] else {
            panic!("two events, not {}", host.temp_entities.len());
        };
        assert_eq!((plain.event, plain.parm, plain.scale), (EV_PLAY_FX, 3, 0));
        assert_eq!(plain.origin, [10.0, 20.0, 30.0]);
        assert_eq!(plain.scope, Scope::Pvs);
        let up = vcod_common::net::events::dir_to_byte([0.0, 0.0, 1.0]);
        assert_eq!((dir.event, dir.parm, dir.scale), (EV_PLAY_FX_DIR, 3, up));
    }
}
