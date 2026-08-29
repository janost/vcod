//! The scriptent mover verbs `mp_pavlov`'s closure reaches:
//! `movegravity`/`rotatevelocity`, two of the ten in the `scriptent` method
//! table alongside `moveto`/`movex`/`movey`/`movez`/`rotateto`/`rotatepitch`/
//! `rotateyaw`/`rotateroll` (docs/research/cod11-gsc-object-model.md section
//! 9). Both record a target velocity and a duration on the entity rather
//! than moving it: there is no per-frame integration or wire path for a
//! mover yet, so a script that calls one and reads `.origin` back on the
//! next tick sees nothing move. Stage 5 gives entities a tick and folds
//! this into it.
//!
//! Real call sites from the extracted stock corpus fix the signatures:
//! `self moveGravity((x, y, z), 12)` (the corpus's own comment reads
//! `(x,y,z),time`) and `self rotateVelocity((250,250,250), 1, 0, 0)`
//! (`(x,y,z),time,accel,decel`) — `rotateVelocity`'s first argument is a
//! target vector, not a yaw angle.

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

fn as_f32(v: &Value) -> Result<f32, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i as f32),
        Value::Float(f) => Ok(*f),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

/// `self moveGravity(velocity, time)`: sets the entity's target velocity
/// and how long, in milliseconds, retail's mover takes to reach it under
/// gravity. Both stored; nothing integrates them into `.origin` yet.
pub fn move_gravity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let (Some(Value::Vector(v)), Some(time)) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType(
            "moveGravity takes a velocity and a time",
        ));
    };
    let time = as_f32(time)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.velocity = *v;
    e.move_time = time;
    Ok(Value::Undefined)
}

/// `self rotateVelocity(velocity, time, accel, decel)`: sets the entity's
/// target velocity and duration the same way `moveGravity` does. `accel`
/// and `decel` shape the ease-in/ease-out curve retail's integrator applies
/// between the entity's current and target velocity; validated here for
/// the right argument shape but not stored, since there is no per-frame
/// integrator yet for an acceleration curve to mean anything to. Stage 5's
/// mover tick is what gives them somewhere to go.
pub fn rotate_velocity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let (Some(Value::Vector(v)), Some(time), Some(accel), Some(decel)) =
        (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "rotateVelocity takes a velocity, a time, accel and decel",
        ));
    };
    let time = as_f32(time)?;
    let _accel = as_f32(accel)?;
    let _decel = as_f32(decel)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.velocity = *v;
    e.move_time = time;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// Both verbs store the target velocity and the duration; call shapes
    /// match the corpus's own examples (`self moveGravity((x,y,z), 12)`,
    /// `self rotateVelocity((-200,-175,-150), 1, 0, 0)`).
    #[test]
    fn movegravity_and_rotatevelocity_store_the_target_velocity_and_time() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));

            let v1 = Value::Vector([250.0, 250.0, 250.0]);
            move_gravity(&mut host, cx, t, &[v1, Value::Int(12)]).unwrap();
            let after1 = host.ents.get(e).unwrap();
            assert_eq!(after1.velocity, [250.0, 250.0, 250.0]);
            assert_eq!(after1.move_time, 12.0);

            let v2 = Value::Vector([-200.0, -175.0, -150.0]);
            rotate_velocity(
                &mut host,
                cx,
                t,
                &[v2, Value::Int(1), Value::Int(0), Value::Int(0)],
            )
            .unwrap();
            let after2 = host.ents.get(e).unwrap();
            assert_eq!(after2.velocity, [-200.0, -175.0, -150.0]);
            assert_eq!(after2.move_time, 1.0);
        });
    }
}
