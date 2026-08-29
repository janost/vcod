//! Combat builtins: real damage queuing and a real collision trace, both
//! global calls (no receiver).

use crate::game::damage::DamageEvent;
use crate::game::host::GameHost;
use glam::Vec3;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("bullettrace", bullet_trace),
    ("radiusdamage", radius_damage),
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

/// `radiusDamage(origin, radius, maxDamage, minDamage)`. A builtin must
/// never reenter the VM, so this queues a `DamageEvent` rather than calling
/// `CodeCallback_PlayerDamage` inline; stage 6 drains the queue.
pub fn radius_damage(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let [Value::Vector(origin), radius, max_damage, min_damage] = args else {
        return Err(ErrorKind::BadType(
            "radiusDamage takes an origin, radius, max damage and min damage",
        ));
    };
    let radius = as_f32(radius)?;
    let max_damage = as_f32(max_damage)?;
    let min_damage = as_f32(min_damage)?;
    host.damage.push(DamageEvent {
        origin: *origin,
        radius,
        max_damage,
        min_damage,
        attacker: None,
    });
    Ok(Value::Undefined)
}

/// `bulletTrace(start, end, ...)`: a real trace against `host.world`'s
/// collision when the server has one. With no world (every unit test, and
/// any server before stage 10 populates `GameHost::world`) it reports a
/// clean miss: fraction 1, position the end point, rather than pretending a
/// hit was computed.
pub fn bullet_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Vector(from)), Some(Value::Vector(to))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType("bulletTrace takes a start and an end"));
    };
    let (from, to) = (*from, *to);

    let s = cx.new_struct();
    let fraction = cx.intern_folded("fraction");
    let position = cx.intern_folded("position");
    let normal = cx.intern_folded("normal");

    match &host.world {
        Some(world) => {
            let start = Vec3::new(from[0], from[1], from[2]);
            let end = Vec3::new(to[0], to[1], to[2]);
            let t = world.collision.shot_trace(start, end);
            cx.set_field(s, fraction, Value::Float(t.fraction));
            cx.set_field(
                s,
                position,
                Value::Vector([t.endpos.x, t.endpos.y, t.endpos.z]),
            );
            cx.set_field(
                s,
                normal,
                Value::Vector([t.normal.x, t.normal.y, t.normal.z]),
            );
        }
        None => {
            cx.set_field(s, fraction, Value::Float(1.0));
            cx.set_field(s, position, Value::Vector(to));
            cx.set_field(s, normal, Value::Vector([0.0, 0.0, 0.0]));
        }
    }
    Ok(Value::Struct(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// A builtin must never reenter the VM, so `radiusDamage` queues an
    /// event the server drains after `run_frame` rather than calling
    /// `CodeCallback_PlayerDamage` inline. That is a deliberate divergence
    /// and it is observable: a script that damages and then reads
    /// `self.health` sees the pre-callback value.
    #[test]
    fn radiusdamage_queues_rather_than_calling_back() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([0.0, 0.0, 0.0]);
            radius_damage(
                &mut host,
                cx,
                None,
                &[at, Value::Int(300), Value::Int(2000), Value::Int(50)],
            )
            .unwrap();
            assert_eq!(host.damage.len(), 1);
            assert_eq!(host.damage[0].radius, 300.0);
            assert_eq!(host.damage[0].max_damage, 2000.0);
        });
    }

    /// `bulletTrace` runs a real trace against the collision world when the
    /// server has one, and reports a clean miss when it does not: there is
    /// no map in a unit test, so `fraction` is 1 and `position` is the end
    /// point.
    #[test]
    fn bullettrace_with_no_world_reports_a_clean_miss() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let from = Value::Vector([0.0, 0.0, 0.0]);
            let to = Value::Vector([100.0, 0.0, 0.0]);
            let Value::Struct(s) =
                bullet_trace(&mut host, cx, None, &[from, to, Value::Int(0)]).unwrap()
            else {
                panic!()
            };
            let f = cx.intern_folded("fraction");
            assert_eq!(cx.get_field(s, f), Value::Float(1.0));
            let p = cx.intern_folded("position");
            assert_eq!(cx.get_field(s, p), Value::Vector([100.0, 0.0, 0.0]));
        });
    }
}
