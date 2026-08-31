//! HUD element builtins. `newHudElem` allocates one, `setTimer` is the only
//! verb `dm.gsc`'s bootstrap calls on it (`startGame` puts the round clock on
//! screen). The other twelve verbs in the hudelem method table, and
//! `newClientHudElem`/`newTeamHudElem`, wait for the clients they draw for.
//!
//! A HUD element is a script object with its own field table
//! (docs/research/cod11-gsc-object-model.md sections 3 and 9), so field
//! access on the value `newHudElem` returns already works: `host.rs` routes
//! by entity number and `ObjectTable` keeps the HUD range in its own vector.

use crate::game::builtins::entity::entity_receiver;
use crate::game::entity::FIRST_HUD_ELEM;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, EntId, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[("newhudelem", new_hud_elem), ("settimer", set_timer)];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The receiver a hudelem method carries. `HudElem_GetMethod` (0x4be38) is
/// only consulted for a HUD element, so a gentity receiver here is the type
/// error retail's own dispatch would raise by never finding the name.
fn hud_receiver(host: &GameHost, recv: Option<Target>) -> Result<EntId, ErrorKind> {
    let id = entity_receiver(recv)?;
    if id.0 < FIRST_HUD_ELEM || host.ents.get(id).is_none() {
        return Err(ErrorKind::BadType("needs a HUD element receiver"));
    }
    Ok(id)
}

/// `newHudElem()` (`game.mp.i386.so` 0x4b184, `functions[77]`): the first
/// free `g_hudelems` record, owned by no client (retail writes `0x3ff`,
/// `ENTITYNUM_NONE`, into the record's owner field at 0x4b249). Faithful,
/// down to failing rather than returning when the pool is full; see
/// `ObjectTable::spawn_hud_elem` for the defaults it does and does not seed.
pub fn new_hud_elem(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Entity(host.ents.spawn_hud_elem(cx)?))
}

/// `<hudelem> setTimer(seconds)` (0x4b8e4, hudelem method 2): a countdown
/// clock. Retail takes exactly one parameter, converts it to milliseconds
/// with the x87 rounding mode set to round-up (0x4b942, so 0.0005 s is 1 ms),
/// and rejects anything not greater than zero. What it stores makes the
/// record a tagged union: it zeroes the block at +0x30..+0x48 plus +0x60,
/// +0x64 and +0x68, writes the element type 4 at +0x0 and the absolute end
/// time `level.time + ms` at +0x5c. `setText` (0x4c590) and `setValue`
/// (0x4c684) share that prologue exactly, each clearing the same block
/// (including the +0x5c a timer uses) before writing type 1 with its string
/// at +0x68, or type 2 with its float at +0x64.
///
/// The call shape and both errors are faithful. Nothing is recorded, and
/// nothing needs clearing: not one of those offsets is in `HUD_FIELDS`, so
/// script can read none of them back. `label`, the one script-readable field
/// nearby, is at +0x2c and retail does not touch it here. The only consumer
/// of what this writes is `G_UpdateHudElemsToClients` (0x5121c), which needs
/// the HUD wire path and the clients a later stage brings.
pub fn set_timer(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    hud_receiver(host, recv)?;
    let seconds = match args {
        [Value::Int(i)] => *i as f32,
        [Value::Float(f)] => *f,
        _ => return Err(ErrorKind::BadType("setTimer takes a time in seconds")),
    };
    if (seconds * 1000.0).ceil() as i32 <= 0 {
        return Err(ErrorKind::BadType("setTimer's time must be above zero"));
    }
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// A HUD element is numbered in its own range and is not a gentity, so
    /// it must not show up in `getEntArray`'s walk.
    #[test]
    fn a_new_hud_elem_is_numbered_above_the_gentities() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = new_hud_elem(&mut host, cx, None, &[]).unwrap();
            let b = new_hud_elem(&mut host, cx, None, &[]).unwrap();
            assert_eq!(a, Value::Entity(EntId(FIRST_HUD_ELEM)));
            assert_eq!(b, Value::Entity(EntId(FIRST_HUD_ELEM + 1)));
            assert_eq!(host.ents.iter_inuse().count(), 0);
        });
    }

    /// The fields `startGame` writes on the clock round-trip through the HUD
    /// field table, and `fontscale` starts at retail's 1.0 rather than
    /// undefined.
    #[test]
    fn a_hud_elems_fields_route_to_the_hud_table() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let Value::Entity(id) = new_hud_elem(&mut host, cx, None, &[]).unwrap() else {
                panic!("newHudElem returns an object");
            };
            let x = cx.intern_folded("x");
            let scale = cx.intern_folded("fontscale");
            vcod_gsc::Host::set_field(&mut host, cx, id, x, Value::Int(320)).unwrap();
            assert_eq!(
                vcod_gsc::Host::get_field(&mut host, cx, id, x),
                Value::Int(320)
            );
            assert_eq!(
                vcod_gsc::Host::get_field(&mut host, cx, id, scale),
                Value::Float(1.0)
            );
        });
    }

    /// `setTimer` is a hudelem method: retail's dispatch never offers it to a
    /// gentity, and a time of zero is the error it raises.
    #[test]
    fn settimer_needs_a_hud_receiver_and_a_positive_time() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let hud = match new_hud_elem(&mut host, cx, None, &[]).unwrap() {
                Value::Entity(id) => id,
                _ => panic!("newHudElem returns an object"),
            };
            let arg = [Value::Int(60)];
            assert!(set_timer(&mut host, cx, Some(Target::Entity(e)), &arg).is_err());
            assert!(set_timer(&mut host, cx, Some(Target::Entity(hud)), &arg).is_ok());
            assert!(set_timer(&mut host, cx, Some(Target::Entity(hud)), &[Value::Int(0)]).is_err());
        });
    }
}
