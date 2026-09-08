//! The ten `scriptent` mover verbs (`docs/research/cod11-gsc-object-model.md`
//! section 9). Each one builds a plan in `crate::game::mover`, which is both
//! the simulation and the wire format; nothing is integrated here.
//!
//! Every unit and every curve comes from `docs/research/cod11-movers.md`,
//! measured against retail: the time argument is seconds, the axis verbs take
//! deltas and the target verbs absolutes, `accel` and `decel` are seconds of
//! ramp on a trapezoidal velocity profile, `moveGravity`'s vector is an
//! initial velocity and `rotateVelocity`'s an angular velocity in degrees per
//! second.
//!
//! Real call sites from the extracted stock corpus fix the signatures:
//! `self moveGravity((x, y, z), 12)` and
//! `self rotateVelocity((250,250,250), 1, 0, 0)`.

use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use crate::game::mover::Ramp;
use glam::Vec3;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("moveto", move_to),
    ("movex", move_x),
    ("movey", move_y),
    ("movez", move_z),
    ("movegravity", move_gravity),
    ("rotateto", rotate_to),
    ("rotatepitch", rotate_pitch),
    ("rotateyaw", rotate_yaw),
    ("rotateroll", rotate_roll),
    ("rotatevelocity", rotate_velocity),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The classnames retail's own handlers accept. Anything else is the fatal
/// `entity N is not a script_brushmodel, script_model, or script_origin`
/// (movers doc, section 1), which cost the probe that measured it a run.
const MOVABLE: [&str; 3] = ["script_brushmodel", "script_model", "script_origin"];

fn number(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Int(i)) => Some(*i as f32),
        Some(Value::Float(f)) => Some(*f),
        _ => None,
    }
}

fn vector(v: Option<&Value>) -> Option<Vec3> {
    match v {
        Some(Value::Vector(v)) => Some(Vec3::from_array(*v)),
        _ => None,
    }
}

/// The receiver, checked against retail's class gate, and the `(duration,
/// accel, decel)` triple every verb but `moveGravity` ends with. `first` is
/// how many arguments come before the duration.
fn call(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    first: usize,
    what: &'static str,
) -> Result<(EntId, Ramp), ErrorKind> {
    let id = entity_receiver(recv)?;
    let classname = {
        let atom = cx.intern_folded("classname");
        match host.get_field(cx, id, atom) {
            Value::String(s) => cx.resolve(s).to_string(),
            _ => String::new(),
        }
    };
    if !MOVABLE.contains(&classname.as_str()) {
        return Err(ErrorKind::BadType(
            "not a script_brushmodel, script_model, or script_origin",
        ));
    }
    let Some(secs) = number(args.get(first)) else {
        return Err(ErrorKind::BadType(what));
    };
    let accel = number(args.get(first + 1)).unwrap_or(0.0);
    let decel = number(args.get(first + 2)).unwrap_or(0.0);
    Ok((id, Ramp::new(secs, accel, decel)))
}

/// The entity's own `origin`/`angles`, which the integrator keeps current.
fn field_vec(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> Vec3 {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::Vector(v) => Vec3::from_array(v),
        _ => Vec3::ZERO,
    }
}

/// `self moveTo(destination, time [, accel, decel])`: an absolute point.
pub fn move_to(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (id, m) = call(host, cx, recv, args, 1, "moveTo takes a point and a time")?;
    let Some(dest) = vector(args.first()) else {
        return Err(ErrorKind::BadType("moveTo takes a point and a time"));
    };
    let from = field_vec(host, cx, id, "origin");
    let now = host.level_time_ms;
    host.movers.move_to(id, now, from, dest, m);
    Ok(Value::Undefined)
}

/// `self moveX(distance, time [, accel, decel])` and its two siblings: a
/// signed displacement along one axis, not a coordinate (movers doc, 3).
fn move_axis(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    axis: usize,
    what: &'static str,
) -> Result<Value, ErrorKind> {
    let (id, m) = call(host, cx, recv, args, 1, what)?;
    let Some(d) = number(args.first()) else {
        return Err(ErrorKind::BadType(what));
    };
    let from = field_vec(host, cx, id, "origin");
    let mut dest = from;
    dest[axis] += d;
    let now = host.level_time_ms;
    host.movers.move_to(id, now, from, dest, m);
    Ok(Value::Undefined)
}

pub fn move_x(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    move_axis(host, cx, recv, args, 0, "moveX takes a distance and a time")
}

pub fn move_y(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    move_axis(host, cx, recv, args, 1, "moveY takes a distance and a time")
}

pub fn move_z(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    move_axis(host, cx, recv, args, 2, "moveZ takes a distance and a time")
}

/// `self moveGravity(velocity, time)`: an initial velocity in units per
/// second on a gravity trajectory. The time is when `movedone` comes, not
/// when the motion stops (movers doc, section 5).
pub fn move_gravity(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "moveGravity takes a velocity and a time";
    let (id, m) = call(host, cx, recv, args, 1, what)?;
    let Some(v) = vector(args.first()) else {
        return Err(ErrorKind::BadType(what));
    };
    let from = field_vec(host, cx, id, "origin");
    let now = host.level_time_ms;
    host.movers.move_gravity(id, now, from, v, m.secs);
    Ok(Value::Undefined)
}

/// `self rotateTo(angles, time [, accel, decel])`: an absolute angle set.
pub fn rotate_to(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "rotateTo takes angles and a time";
    let (id, m) = call(host, cx, recv, args, 1, what)?;
    let Some(dest) = vector(args.first()) else {
        return Err(ErrorKind::BadType(what));
    };
    let from = field_vec(host, cx, id, "angles");
    let now = host.level_time_ms;
    host.movers.rotate_to(id, now, from, dest, m);
    Ok(Value::Undefined)
}

/// `self rotateYaw(degrees, time [, accel, decel])` and its two siblings: a
/// signed delta on one axis, which is what a second call on one entity
/// measured (movers doc, section 3).
fn rotate_axis(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
    axis: usize,
    what: &'static str,
) -> Result<Value, ErrorKind> {
    let (id, m) = call(host, cx, recv, args, 1, what)?;
    let Some(d) = number(args.first()) else {
        return Err(ErrorKind::BadType(what));
    };
    let from = field_vec(host, cx, id, "angles");
    let mut dest = from;
    dest[axis] += d;
    let now = host.level_time_ms;
    host.movers.rotate_to(id, now, from, dest, m);
    Ok(Value::Undefined)
}

pub fn rotate_pitch(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "rotatePitch takes an angle and a time";
    rotate_axis(host, cx, recv, args, 0, what)
}

pub fn rotate_yaw(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "rotateYaw takes an angle and a time";
    rotate_axis(host, cx, recv, args, 1, what)
}

pub fn rotate_roll(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "rotateRoll takes an angle and a time";
    rotate_axis(host, cx, recv, args, 2, what)
}

/// `self rotateVelocity(degreesPerSecond, time, accel, decel)`: the angle
/// covered is the velocity times the duration, which is where a `(0,180,0)`
/// over two seconds landing back on zero came from.
pub fn rotate_velocity(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let what = "rotateVelocity takes a velocity, a time, accel and decel";
    let (id, m) = call(host, cx, recv, args, 1, what)?;
    let Some(v) = vector(args.first()) else {
        return Err(ErrorKind::BadType(what));
    };
    if args.len() < 4 {
        return Err(ErrorKind::BadType(what));
    }
    let from = field_vec(host, cx, id, "angles");
    let now = host.level_time_ms;
    host.movers.rotate_velocity(id, now, from, v, m);
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// A `script_origin` at a known spot, the shape every test here starts
    /// from.
    fn mover(host: &mut GameHost, cx: &mut Cx, origin: [f32; 3]) -> EntId {
        let e = host.ents.spawn(cx).unwrap();
        let class = cx.intern_folded("classname");
        let v = Value::String(cx.intern_exact("script_origin"));
        host.set_field(cx, e, class, v).unwrap();
        let o = cx.intern_folded("origin");
        host.set_field(cx, e, o, Value::Vector(origin)).unwrap();
        let a = cx.intern_folded("angles");
        host.set_field(cx, e, a, Value::Vector([0.0; 3])).unwrap();
        e
    }

    /// The retail trace, replayed frame for frame. `moveto((600,0,100), 1)`
    /// called at level time 1050 read the start origin again at 1100, walked
    /// x in 25-unit steps from 1150, and raised `movedone` at 2100 with the
    /// origin at 600: one frame behind the trajectory throughout
    /// (docs/research/cod11-movers.md, sections 2, 4 and 8).
    #[test]
    fn moveto_reproduces_the_retail_per_frame_trace() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = mover(&mut host, cx, [100.0, 0.0, 100.0]);
            host.level_time_ms = 1050;
            let dest = Value::Vector([600.0, 0.0, 100.0]);
            move_to(
                &mut host,
                cx,
                Some(Target::Entity(e)),
                &[dest, Value::Int(1)],
            )
            .unwrap();

            let mut done = Vec::new();
            for frame in 22..=45 {
                host.level_time_ms = frame * 50;
                done.extend(crate::game::mover::run(&mut host, cx));
                let x = field_vec(&mut host, cx, e, "origin").x;
                let want = (100.0 + 0.5 * (frame - 22) as f32 * 50.0).min(600.0);
                assert!(
                    (x - want).abs() < 0.01,
                    "level time {}: x {x} want {want}",
                    frame * 50
                );
                assert_eq!(
                    !done.is_empty(),
                    frame * 50 >= 2100,
                    "movedone at level time {}",
                    frame * 50
                );
            }
            assert_eq!(done.len(), 1, "one movedone: {done:?}");
            assert_eq!(done[0].event, crate::game::mover::MOVEDONE);
        });
    }

    /// `movex` is a delta and `rotateyaw` is one too: a second call runs the
    /// yaw from 90 to 180 rather than staying at 90, which is the case the
    /// retail probe had to call twice to see.
    #[test]
    fn the_axis_verbs_take_deltas() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = mover(&mut host, cx, [100.0, 0.0, 100.0]);
            let t = Some(Target::Entity(e));

            move_x(&mut host, cx, t, &[Value::Int(500), Value::Int(2)]).unwrap();
            host.level_time_ms = 2050;
            crate::game::mover::run(&mut host, cx);
            assert!((field_vec(&mut host, cx, e, "origin").x - 600.0).abs() < 0.01);

            for (start, want) in [(2050, 90.0), (5000, 180.0)] {
                host.level_time_ms = start;
                rotate_yaw(&mut host, cx, t, &[Value::Int(90), Value::Int(2)]).unwrap();
                host.level_time_ms = start + 2050;
                crate::game::mover::run(&mut host, cx);
                let yaw = field_vec(&mut host, cx, e, "angles").y;
                assert!((yaw - want).abs() < 0.01, "yaw {yaw} want {want}");
            }
        });
    }

    /// Retail's class gate: a mover verb on anything else is an error, not a
    /// no-op. Both stock call shapes still go through.
    #[test]
    fn a_mover_verb_is_refused_on_anything_but_the_three_classes() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = mover(&mut host, cx, [0.0; 3]);
            let t = Some(Target::Entity(e));

            let v = Value::Vector([250.0, 250.0, 250.0]);
            assert!(move_gravity(&mut host, cx, t, &[v, Value::Int(12)]).is_ok());
            let full = [v, Value::Int(1), Value::Int(0), Value::Int(0)];
            assert!(rotate_velocity(&mut host, cx, t, &full).is_ok());
            assert!(rotate_velocity(&mut host, cx, t, &full[..3]).is_err());
            assert!(move_gravity(&mut host, cx, t, &[v]).is_err());
            assert!(move_gravity(&mut host, cx, None, &[v, Value::Int(12)]).is_err());

            let item = host.ents.spawn(cx).unwrap();
            let class = cx.intern_folded("classname");
            let w = Value::String(cx.intern_exact("mpweapon_panzerfaust"));
            host.set_field(cx, item, class, w).unwrap();
            assert!(move_z(
                &mut host,
                cx,
                Some(Target::Entity(item)),
                &[Value::Int(96), Value::Int(2)]
            )
            .is_err());
        });
    }
}
