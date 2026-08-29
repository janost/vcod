//! Vector and scalar math builtins. None of these touch the entity table or
//! the configstrings, so every one takes its arguments and returns a value
//! with no `GameHost` state to consult.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("distance", distance),
    ("length", length),
    ("vectornormalize", vector_normalize),
    ("vectortoangles", vector_to_angles),
    ("anglestoforward", angles_to_forward),
    ("randomfloat", random_float),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

fn vec_len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `distance(a, b)`: the straight-line distance between two points.
pub fn distance(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Vector(a)), Some(Value::Vector(b))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType("distance takes two vectors"));
    };
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    Ok(Value::Float(vec_len(d)))
}

/// `length(v)`: the vector's own magnitude.
pub fn length(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("length takes a vector"));
    };
    Ok(Value::Float(vec_len(*v)))
}

/// `vectorNormalize(v)`: a unit vector in the same direction. Retail's
/// `VectorNormalize` returns `(0,0,0)` unchanged on a zero-length input
/// rather than dividing by zero, so this does the same.
pub fn vector_normalize(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("vectorNormalize takes a vector"));
    };
    let len = vec_len(*v);
    if len == 0.0 {
        return Ok(Value::Vector([0.0, 0.0, 0.0]));
    }
    Ok(Value::Vector([v[0] / len, v[1] / len, v[2] / len]))
}

/// `vectorToAngles(v)`: `vectoangles` from the Q3 lineage
/// (`bg_lib.c`/`math.c`), which every RTCW/Q3-descended engine including
/// CoD 1.1 carries unchanged. Angles come back `[pitch, yaw, roll]` with
/// roll always 0.
pub fn vector_to_angles(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("vectorToAngles takes a vector"));
    };
    let (yaw, pitch);
    if v[0] == 0.0 && v[1] == 0.0 {
        yaw = 0.0;
        pitch = if v[2] > 0.0 { 90.0 } else { 270.0 };
    } else {
        let mut y = if v[0] != 0.0 {
            v[1].atan2(v[0]).to_degrees()
        } else if v[1] > 0.0 {
            90.0
        } else {
            -90.0
        };
        if y < 0.0 {
            y += 360.0;
        }
        let fwd = (v[0] * v[0] + v[1] * v[1]).sqrt();
        let mut p = v[2].atan2(fwd).to_degrees();
        if p < 0.0 {
            p += 360.0;
        }
        yaw = y;
        pitch = p;
    }
    Ok(Value::Vector([-pitch, yaw, 0.0]))
}

/// `anglesToForward(angles)`: `AngleVectors`'s forward half, same lineage as
/// `vectorToAngles` above and its inverse on a unit vector.
pub fn angles_to_forward(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(a)) = args.first() else {
        return Err(ErrorKind::BadType("anglesToForward takes an angles vector"));
    };
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sy, cy) = a[1].to_radians().sin_cos();
    Ok(Value::Vector([cp * cy, cp * sy, -sp]))
}

/// `randomFloat(range)`: a real draw in `[0, range)`, seeded from wall clock
/// jitter rather than a stored generator; `GameHost` carries no rng field
/// (Task 9's field budget is `cvars` and `world`), so there is nowhere to
/// keep a seed between calls. Good enough for gameplay variance, not for
/// anything that needs a reproducible sequence.
pub fn random_float(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let range = match args.first() {
        Some(Value::Int(i)) => *i as f32,
        Some(Value::Float(f)) => *f,
        _ => return Err(ErrorKind::BadType("randomFloat takes a number")),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let unit = nanos as f32 / u32::MAX as f32;
    Ok(Value::Float(unit * range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// Retail's math builtins, checked against values the retail captures
    /// pin where they exist and against the definition where they do not.
    #[test]
    fn the_vector_builtins_do_what_their_names_say() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = Value::Vector([0.0, 0.0, 0.0]);
            let b = Value::Vector([3.0, 4.0, 0.0]);
            assert_eq!(
                distance(&mut host, cx, None, &[a, b]).unwrap(),
                Value::Float(5.0)
            );
            assert_eq!(
                length(&mut host, cx, None, &[b]).unwrap(),
                Value::Float(5.0)
            );
            let Value::Vector(n) = vector_normalize(&mut host, cx, None, &[b]).unwrap() else {
                panic!()
            };
            assert!((n[0] - 0.6).abs() < 1e-6 && (n[1] - 0.8).abs() < 1e-6);
        });
    }

    /// `vectorToAngles` and `anglesToForward` are inverses on a unit vector.
    #[test]
    fn vector_to_angles_round_trips_through_angles_to_forward() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let v = Value::Vector([1.0, 0.0, 0.0]);
            let ang = vector_to_angles(&mut host, cx, None, &[v]).unwrap();
            let Value::Vector(f) = angles_to_forward(&mut host, cx, None, &[ang]).unwrap() else {
                panic!()
            };
            assert!((f[0] - 1.0).abs() < 1e-5 && f[1].abs() < 1e-5 && f[2].abs() < 1e-5);
        });
    }
}
