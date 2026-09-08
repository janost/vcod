//! The scriptent mover verbs `mp_pavlov`'s closure reaches:
//! `movegravity`/`rotatevelocity`, two of the ten in the `scriptent` method
//! table alongside `moveto`/`movex`/`movey`/`movez`/`rotateto`/`rotatepitch`/
//! `rotateyaw`/`rotateroll` (docs/research/cod11-gsc-object-model.md section
//! 9).
//!
//! Neither moves anything and neither stores anything. There is no per-frame
//! integration and no wire path for a mover yet, so the only job here is to
//! accept the call shapes the stock corpus uses instead of raising `BadType`
//! at a script that is doing nothing wrong.
//!
//! What each verb means is no longer open: `docs/research/cod11-movers.md`
//! has the units, the trapezoidal motion law and the trajectory each one puts
//! on the wire, measured against retail. The integrator that reads it is the
//! next piece of work; until it exists, storing a velocity here would be
//! state nothing evaluates.
//!
//! Real call sites from the extracted stock corpus fix the signatures:
//! `self moveGravity((x, y, z), 12)` and
//! `self rotateVelocity((250,250,250), 1, 0, 0)`
//! (`(x,y,z),time,accel,decel`). `rotateVelocity`'s first argument is an
//! angular velocity in degrees per second, not a yaw angle, and the time
//! argument is seconds (the movers doc, sections 2 and 6).

use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("movegravity", move_gravity),
    ("rotatevelocity", rotate_velocity),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The receiver has to be a live entity and the first argument a vector, the
/// shape both verbs share; `arity` is how many arguments the verb takes.
fn check_call(
    host: &GameHost,
    recv: Option<Target>,
    args: &[Value],
    arity: usize,
    what: &'static str,
) -> Result<(), ErrorKind> {
    let id = entity_receiver(recv)?;
    if args.len() < arity || !matches!(args.first(), Some(Value::Vector(_))) {
        return Err(ErrorKind::BadType(what));
    }
    host.ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(())
}

/// `self moveGravity(velocity, time)`. Accepted and dropped; see the module
/// comment.
pub fn move_gravity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    check_call(
        host,
        recv,
        args,
        2,
        "moveGravity takes a velocity and a time",
    )?;
    Ok(Value::Undefined)
}

/// `self rotateVelocity(velocity, time, accel, decel)`. Accepted and dropped;
/// see the module comment.
pub fn rotate_velocity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    check_call(
        host,
        recv,
        args,
        4,
        "rotateVelocity takes a velocity, a time, accel and decel",
    )?;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// Both verbs accept the corpus's own call shapes (`self
    /// moveGravity((x,y,z), 12)`, `self rotateVelocity((-200,-175,-150), 1,
    /// 0, 0)`) and reject a call that is missing the vector or an argument.
    #[test]
    fn the_movers_accept_the_corpus_call_shapes_and_reject_the_rest() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));

            let v1 = Value::Vector([250.0, 250.0, 250.0]);
            assert!(move_gravity(&mut host, cx, t, &[v1, Value::Int(12)]).is_ok());
            assert!(move_gravity(&mut host, cx, t, &[v1]).is_err());

            let v2 = Value::Vector([-200.0, -175.0, -150.0]);
            let full = [v2, Value::Int(1), Value::Int(0), Value::Int(0)];
            assert!(rotate_velocity(&mut host, cx, t, &full).is_ok());
            assert!(rotate_velocity(&mut host, cx, t, &full[..3]).is_err());
            assert!(rotate_velocity(&mut host, cx, t, &[Value::Int(0); 4]).is_err());

            // No receiver is a type error, same as any other entity method.
            assert!(move_gravity(&mut host, cx, None, &[v1, Value::Int(12)]).is_err());
        });
    }
}
