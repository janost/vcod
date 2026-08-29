//! The scriptent mover verbs `mp_pavlov`'s closure reaches:
//! `movegravity`/`rotatevelocity`, two of the ten in the `scriptent` method
//! table alongside `moveto`/`movex`/`movey`/`movez`/`rotateto`/`rotatepitch`/
//! `rotateyaw`/`rotateroll` (docs/research/cod11-gsc-object-model.md section
//! 9). Both record a velocity on the entity rather than moving it: there is
//! no per-frame integration or wire path for a mover yet, so a script that
//! calls one and reads `.origin` back on the next tick sees nothing move.
//! Stage 5 gives entities a tick and folds this velocity into it.

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

/// `self moveGravity(velocity)`: sets the entity's velocity outright.
pub fn move_gravity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("moveGravity takes a velocity"));
    };
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.velocity = *v;
    Ok(Value::Undefined)
}

/// `self rotateVelocity(angle)`: rotates the entity's current velocity
/// about the yaw axis by `angle` degrees and stores the result.
pub fn rotate_velocity(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let angle = match args.first() {
        Some(Value::Int(i)) => *i as f32,
        Some(Value::Float(f)) => *f,
        _ => return Err(ErrorKind::BadType("rotateVelocity takes an angle")),
    };
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    let (s, c) = angle.to_radians().sin_cos();
    let v = e.velocity;
    e.velocity = [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]];
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `moveGravity` sets the velocity outright; `rotateVelocity` turns
    /// whatever is already there about the yaw axis.
    #[test]
    fn movegravity_sets_and_rotatevelocity_turns_the_stored_velocity() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            let v = Value::Vector([100.0, 0.0, 0.0]);
            move_gravity(&mut host, cx, t, &[v]).unwrap();
            assert_eq!(host.ents.get(e).unwrap().velocity, [100.0, 0.0, 0.0]);

            rotate_velocity(&mut host, cx, t, &[Value::Int(90)]).unwrap();
            let after = host.ents.get(e).unwrap().velocity;
            assert!((after[0]).abs() < 1e-3, "{after:?}");
            assert!((after[1] - 100.0).abs() < 1e-3, "{after:?}");
        });
    }
}
