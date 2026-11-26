//! Vector and scalar math builtins. None of these touch the entity table or
//! the configstrings; the random draws step `GameHost::holdrand`, the rest
//! read only their arguments.

use std::f64::consts::PI;

use super::args::{float, int, vector};
use crate::game::host::GameHost;
use glam::Vec3;
use vcod_common::x87::{self, F80};
use vcod_gsc::{Cx, ErrorKind, Target, Value};

use super::Builtin;

pub const NAMES: &[(&str, Builtin)] = &[
    ("distance", distance),
    ("distancesquared", distance_squared),
    ("length", length),
    ("lengthsquared", length_squared),
    ("closer", closer),
    ("vectordot", vector_dot),
    ("vectornormalize", vector_normalize),
    ("vectortoangles", vector_to_angles),
    ("anglestoforward", angles_to_forward),
    ("anglestoright", angles_to_right),
    ("anglestoup", angles_to_up),
    ("randomint", random_int),
    ("randomfloat", random_float),
    ("randomintrange", random_int_range),
    ("randomfloatrange", random_float_range),
    ("sin", sin),
    ("cos", cos),
    ("tan", tan),
    ("asin", asin),
    ("acos", acos),
    ("atan", atan),
];

/// How a build's `distance`, `distanceSquared`, `length`,
/// `vectorNormalize`, `anglesTo*` and trig builtins round
/// (cod11-gsc-object-model.md 9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathRounding {
    /// 1.1d: differences, squares, sums and roots on the x87 stack until
    /// one store; `fsin` and `fcos`.
    Kept,
    /// 1.5b and UO: each difference stored as a float, the squares' sum
    /// handed to libm's `sqrt` as a double; libm's `sin` and `cos`.
    Stored,
    /// CoD2 and CoD4: every step a float, unverified against their binaries.
    Float,
}

fn vec_len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn two_vectors(args: &[Value], usage: &'static str) -> Result<([f32; 3], [f32; 3]), ErrorKind> {
    match (args.first(), args.get(1)) {
        (Some(Value::Vector(a)), Some(Value::Vector(b))) => Ok((*a, *b)),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

/// `a - b` as the build's `VectorDistance` and `VectorDistanceSquared`
/// take it: on the stack (1.1d) or each stored as a float (1.5b, UO).
fn differences(r: MathRounding, a: [f32; 3], b: [f32; 3]) -> [F80; 3] {
    std::array::from_fn(|i| match r {
        MathRounding::Kept => F80::from_f32(a[i]) - F80::from_f32(b[i]),
        _ => F80::from_f32(a[i] - b[i]),
    })
}

/// The squares summed left to right on the stack.
fn sum_squares(d: [F80; 3]) -> F80 {
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// The root of [`sum_squares`] as a float: `fsqrt` on the stack (1.1d), or
/// libm's `sqrt` of the sum stored as a double (1.5b, UO).
fn root(r: MathRounding, sum: F80) -> f32 {
    match r {
        MathRounding::Kept => sum.sqrt().to_f32(),
        _ => sum.to_f64().sqrt() as f32,
    }
}

/// `distance(a, b)` (1.1d 0x5e848, 1.5b 0x5faf6, UO 0x6b7e5):
/// `VectorDistance` (1.1d 0x3d7ec, 1.5b 0x32d79, UO 0x39fc9) as
/// [`differences`], [`sum_squares`] and [`root`] round it, one store into
/// `Scr_AddFloat`. VERIFIED (the instructions), and `probe_math` on 1.1d,
/// 1.5b and UO.
pub fn distance(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (a, b) = two_vectors(args, "distance takes two vectors")?;
    let r = host.profile.math;
    if r == MathRounding::Float {
        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        return Ok(Value::Float(vec_len(d)));
    }
    Ok(Value::Float(root(r, sum_squares(differences(r, a, b)))))
}

/// `distanceSquared(a, b)` (1.1d 0x5e890, 1.5b 0x5fb4e, UO 0x6b83d):
/// `VectorDistanceSquared` (1.1d 0x3d830, 1.5b 0x32dee, UO 0x3a03e) leaves
/// [`sum_squares`] of the [`differences`] on the stack and the handler
/// stores it once. VERIFIED (the instructions), and `probe_math` on 1.1d,
/// 1.5b and UO.
pub fn distance_squared(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (a, b) = two_vectors(args, "distanceSquared takes two vectors")?;
    let r = host.profile.math;
    if r == MathRounding::Float {
        let d: f32 = (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum();
        return Ok(Value::Float(d));
    }
    Ok(Value::Float(sum_squares(differences(r, a, b)).to_f32()))
}

/// `vectorDot(a, b)`: the three products summed on the x87 stack and
/// rounded to a float once (1.1d 0x5e9d4, 1.5b 0x5fcdc, UO 0x6b9cb, CoD2
/// `Scr_VectorDot` 0x8114240; CoD4 binds it by name). VERIFIED, and
/// `probe_vectordot` pins the single rounding on 1.1d, 1.5b and UO.
pub fn vector_dot(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "vectorDot takes two vectors";
    let (a, b) = (
        vector(args.first(), USAGE)?.to_array(),
        vector(args.get(1), USAGE)?.to_array(),
    );
    let dot: f64 = (0..3).map(|i| f64::from(a[i]) * f64::from(b[i])).sum();
    Ok(Value::Float(dot as f32))
}

/// `lengthSquared(v)`: the three squares summed on the x87 stack and rounded
/// to a float once (1.1d 0x5e920, 1.5b 0x5fbfd, UO 0x6b8ec, CoD2 0x811418c;
/// CoD4 has its own). VERIFIED, `probe_closer` on 1.1d, 1.5b and UO
/// (cod11-gsc-object-model.md 9.2).
pub fn length_squared(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let v = vector(args.first(), "lengthSquared takes a vector")?.to_array();
    let sum: f64 = v.iter().map(|&c| f64::from(c) * f64::from(c)).sum();
    Ok(Value::Float(sum as f32))
}

/// Where `closer(ref, a, b)` rounds its two squared distances to floats
/// before the compare (cod11-gsc-object-model.md 9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloserRounding {
    /// 1.1d: the differences and the second distance stay on the x87
    /// stack; only the first distance is stored as a float.
    FirstOnly,
    /// 1.5b, UO and CoD2: each difference and each distance is a float.
    Floats,
    /// CoD4 1.7 (0x80c206e): nothing is stored.
    None,
}

/// `closer(ref, a, b)`: 1 when `a`'s squared distance to `ref` is under
/// `b`'s, rounded as the profile's [`CloserRounding`] says. The products
/// of two floats are exact in `f64`; the 80-bit stack can differ from it
/// only by a double rounding (INFERRED).
pub fn closer(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "closer takes three vectors";
    let r = vector(args.first(), USAGE)?.to_array();
    let a = vector(args.get(1), USAGE)?.to_array();
    let b = vector(args.get(2), USAGE)?.to_array();
    let rounding = host.profile.closer;
    let dist = |p: [f32; 3]| -> f64 {
        (0..3)
            .map(|i| {
                let d = match rounding {
                    CloserRounding::Floats => f64::from(p[i] - r[i]),
                    _ => f64::from(p[i]) - f64::from(r[i]),
                };
                d * d
            })
            .sum()
    };
    let float = |d: f64| f64::from(d as f32);
    let (near, far) = match rounding {
        CloserRounding::FirstOnly => (float(dist(a)), dist(b)),
        CloserRounding::Floats => (float(dist(a)), float(dist(b))),
        CloserRounding::None => (dist(a), dist(b)),
    };
    Ok(Value::Int(i32::from(far > near)))
}

/// `length(v)` (1.1d 0x5e8d8, 1.5b 0x5fba6, UO 0x6b895): the squares
/// summed on the stack inside the handler, then [`root`]. VERIFIED (the
/// instructions), and `probe_math` on 1.1d, 1.5b and UO.
pub fn length(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("length takes a vector"));
    };
    let r = host.profile.math;
    if r == MathRounding::Float {
        return Ok(Value::Float(vec_len(*v)));
    }
    Ok(Value::Float(root(r, sum_squares(v.map(F80::from_f32)))))
}

/// `vectorNormalize(v)` (1.1d 0x5ea28, 1.5b 0x5fd38, UO 0x6ba27): a copy
/// through the build's in-place `VectorNormalize`, its length dropped:
/// [`x87::normalize_in_place`] on 1.1d (0x3d8d8), [`x87::normalize_stored`]
/// on 1.5b (0x32f73) and UO (0x3a1c3). A zero vector comes back as it went
/// in. VERIFIED (the instructions), and `probe_math` on 1.1d, 1.5b and UO.
pub fn vector_normalize(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("vectorNormalize takes a vector"));
    };
    let n = match host.profile.math {
        MathRounding::Kept => x87::normalize_in_place(Vec3::from_array(*v)),
        MathRounding::Stored => x87::normalize_stored(Vec3::from_array(*v)),
        MathRounding::Float => {
            let len = vec_len(*v);
            if len == 0.0 {
                return Ok(Value::Vector([0.0, 0.0, 0.0]));
            }
            return Ok(Value::Vector([v[0] / len, v[1] / len, v[2] / len]));
        }
    };
    Ok(Value::Vector(n.to_array()))
}

/// `vectorToAngles(v)`: `vectoangles` (1.1d 0x3de94). Unlike Q3's, the
/// pitch is `atan2(z, |xy|) * -180 / PI` with 360 added while negative, so
/// both angles come back in [0, 360): a vector 16 degrees down reads 16,
/// one up 345, straight up 270 and straight down 90 (`.rodata` 0x72ae8,
/// 0x72b08). Each angle is stored as a float before the 360 is added.
/// Roll is 0. Measured: `client-probes/probe_dmgtouch` on retail UO logs
/// the aim it builds this way (cod11-combat.md 14.8).
pub fn vector_to_angles(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("vectorToAngles takes a vector"));
    };
    Ok(Value::Vector(vectoangles(*v)))
}

/// `vectoangles` on the x87: the atans in double, each angle stored as a
/// float and then wrapped into [0, 360). The engine's own callers use it
/// too: a missile's launch angles and the damage feedback's two bytes.
pub fn vectoangles(v: [f32; 3]) -> [f32; 3] {
    if v[0] == 0.0 && v[1] == 0.0 {
        let pitch = if v[2] > 0.0 { 270.0 } else { 90.0 };
        return [pitch, 0.0, 0.0];
    }
    let wrap = |a: f64| {
        let a = a as f32;
        if a < 0.0 {
            (a as f64 + 360.0) as f32
        } else {
            a
        }
    };
    let (x, y, z) = (v[0] as f64, v[1] as f64, v[2] as f64);
    let yaw = wrap(y.atan2(x) * 180.0 / std::f64::consts::PI);
    let fwd = (x * x + y * y).sqrt() as f32 as f64;
    let pitch = wrap(z.atan2(fwd) * -180.0 / std::f64::consts::PI);
    [pitch, yaw, 0.0]
}

/// 1.7's `vectoangles` (0x8191a72), the missiles' and `vectorToAngles`'
/// (0x80be762): each `atan2` times 180 (the pitch's -180) over the double
/// pi on the stack, stored as a float, then 360 added to a negative one and
/// 0 to the rest before its one store, so a -0 comes back +0 (`fcmovnb` at
/// 0x8191ae5 and 0x8191b2a, the adds at 0x8191b31 and 0x8191b36). The
/// pitch's flat length stays unrounded into its `atan2` (`fstpl`,
/// 0x8191afc), where CoD1's [`vectoangles`] stores a float (cod4-gsc.md
/// 3.2, "Missiles under 1.7"). The one port of 0x8191a72.
pub fn vectoangles_1_7(v: [f32; 3]) -> [f32; 3] {
    use vcod_common::x87::F80;
    if v[0] == 0.0 && v[1] == 0.0 {
        return [if v[2] > 0.0 { 270.0 } else { 90.0 }, 0.0, 0.0];
    }
    let pi = F80::from_f64(std::f64::consts::PI);
    let degrees = |atan: f64, k: f32| {
        let a = (F80::from_f64(atan) * F80::from_f32(k) / pi).to_f32();
        let add = if a < 0.0 { 360.0 } else { 0.0 };
        (F80::from_f32(add) + F80::from_f32(a)).to_f32()
    };
    let (x, y) = (F80::from_f32(v[0]), F80::from_f32(v[1]));
    let flat = (x * x + y * y).sqrt().to_f64();
    [
        degrees(f64::from(v[2]).atan2(flat), -180.0),
        degrees(f64::from(v[1]).atan2(f64::from(v[0])), 180.0),
        0.0,
    ]
}

/// CoD4's `vectorToAngles(v)`: [`vectoangles_1_7`].
pub fn vector_to_angles_1_7(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(v)) = args.first() else {
        return Err(ErrorKind::BadType("vectorToAngles takes a vector"));
    };
    Ok(Value::Vector(vectoangles_1_7(*v)))
}

/// `vectoyaw` (1.1d 0x3dccc): the atan in double, stored as a float, and
/// 360 added on the stack when it is negative, so a caller that truncates
/// it reads the sum unrounded. 0 for a vector with no horizontal part.
pub(crate) fn vectoyaw(v: [f32; 3]) -> f64 {
    if v[1] == 0.0 && v[0] == 0.0 {
        return 0.0;
    }
    let yaw = (f64::from(v[1]).atan2(f64::from(v[0])) * 180.0 / std::f64::consts::PI) as f32;
    if yaw < 0.0 {
        f64::from(yaw) + 360.0
    } else {
        f64::from(yaw)
    }
}

/// `anglesToForward`, `anglesToRight` and `anglesToUp` (1.1d 0x5eb20,
/// 0x5eae4, 0x5eaa8; 1.5b 0x5fe85, 0x5fe2d, 0x5fdd5; UO 0x6bb74, 0x6bb1c,
/// 0x6bac4): one `AngleVectors` vector each, [`x87::angle_vectors`] on every
/// CoD1 build. VERIFIED (the calls), and `probe_math` on 1.1d, 1.5b and UO.
/// CoD2 and CoD4 keep a plain-float `AngleVectors`, unverified.
fn angle_vectors(host: &GameHost, args: &[Value]) -> Result<[[f32; 3]; 3], ErrorKind> {
    let Some(Value::Vector(a)) = args.first() else {
        return Err(ErrorKind::BadType("anglesTo* takes an angles vector"));
    };
    if host.profile.math != MathRounding::Float {
        return Ok(x87::angle_vectors(Vec3::from_array(*a)).map(|v| v.to_array()));
    }
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sy, cy) = a[1].to_radians().sin_cos();
    let (sr, cr) = a[2].to_radians().sin_cos();
    Ok([
        [cp * cy, cp * sy, -sp],
        [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    ])
}

pub fn angles_to_forward(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Vector(angle_vectors(host, args)?[0]))
}

pub fn angles_to_right(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Vector(angle_vectors(host, args)?[1]))
}

pub fn angles_to_up(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Vector(angle_vectors(host, args)?[2]))
}

/// The four draws step [`GameHost::script_irand`]'s generator: CoD2's
/// shares `holdrand` with the rocket's wobble (cod2-combat.md 2.6.1).
/// `randomInt(n)` (1.1d 0x5e470, 1.5b 0x5f6a8, UO 0x6b397, CoD2 0x8113ca4): `irand(0, n)`
/// off `holdrand`; `n` under 1 is an error and draws nothing (map-cycle
/// doc 4.8).
pub fn random_int(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let n = int(args.first(), "randomInt takes a number")?;
    if n <= 0 {
        return Err(ErrorKind::Custom(
            "RandomInt parm must be positive integer.".into(),
        ));
    }
    Ok(Value::Int(host.script_irand(0, n)))
}

/// `randomFloat(f)` (1.1d 0x5e4bc, 1.5b 0x5f715, UO 0x6b404, CoD2
/// 0x8113cfe): `flrand(0,
/// f)` off `holdrand`, any `f`.
pub fn random_float(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let f = float(args.first(), "randomFloat takes a number")?;
    Ok(Value::Float(host.script_flrand(0.0, f)))
}

/// `randomIntRange(lo, hi)` (1.1d 0x5e4f0, 1.5b 0x5f757, UO 0x6b446,
/// CoD2 0x8113d32):
/// `irand(lo, hi)` off `holdrand`; `hi` not above `lo` is an error.
pub fn random_int_range(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "randomIntRange takes two numbers";
    let (lo, hi) = (int(args.first(), USAGE)?, int(args.get(1), USAGE)?);
    if hi <= lo {
        return Err(ErrorKind::Custom(
            "RandomIntRange range must be positive integer.".into(),
        ));
    }
    Ok(Value::Int(host.script_irand(lo, hi)))
}

/// `randomFloatRange(lo, hi)` (1.1d 0x5e554, 1.5b 0x5f7d9, UO 0x6b4c8,
/// CoD2 0x8113da0):
/// `flrand(lo, hi)` off `holdrand`; `hi` not above `lo` is an error, a NaN
/// bound is not.
pub fn random_float_range(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "randomFloatRange takes two numbers";
    let (lo, hi) = (float(args.first(), USAGE)?, float(args.get(1), USAGE)?);
    if lo >= hi {
        return Err(ErrorKind::Custom(
            "Scr_RandomFloatRange range must be positive float.".into(),
        ));
    }
    Ok(Value::Float(host.script_flrand(lo, hi)))
}

/// Degrees to radians as every CoD1 handler does it, `x * M_PI / 180` on
/// the stack (1.1d 0x5e5ef, `.rodata` 0x78a90 and 0x78a98; 1.5b and UO the
/// same two doubles off the GOT).
fn radians(deg: f32) -> F80 {
    F80::from_f32(deg) * F80::from_f64(PI) / F80::from_f64(180.0)
}

/// An `asin`/`acos`/`atan` result back to degrees: the float radians times
/// 180 (a float) over `M_PI` (a double) on the stack, stored once (1.1d
/// 0x5e745..0x5e756).
fn degrees(rad: f32) -> Value {
    Value::Float((F80::from_f32(rad) * F80::from_f32(180.0) / F80::from_f64(PI)).to_f32())
}

/// CoD2's and CoD4's: the radians in doubles, unverified.
fn float_radians(deg: f32) -> f64 {
    deg as f64 * PI / 180.0
}

/// CoD2's and CoD4's: the product rounded to a float first, unverified.
fn float_degrees(rad: f64) -> Value {
    Value::Float(((rad as f32 * 180.0) as f64 / PI) as f32)
}

/// `asin`/`acos` refuse anything outside [-1, 1] with `"%g out of range"`
/// (1.1 .rodata 0x77500).
fn unit_arg(args: &[Value], usage: &'static str) -> Result<f32, ErrorKind> {
    let x = float(args.first(), usage)?;
    if !(-1.0..=1.0).contains(&x) {
        return Err(ErrorKind::Custom(format!(
            "{} out of range",
            vcod_gsc::format_g(x)
        )));
    }
    Ok(x)
}

/// `sin(degrees)` (1.1d 0x5e5dc, 1.5b 0x5f862, UO 0x6b551): [`radians`]
/// into `fsin` on 1.1d ([`F80::fsin`]), into libm's `sin` as a double on
/// 1.5b and UO, the result stored once. VERIFIED (the instructions), and
/// `probe_math` on 1.1d, 1.5b and UO.
pub fn sin(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = float(args.first(), "sin takes a number")?;
    Ok(Value::Float(match host.profile.math {
        MathRounding::Kept => radians(x).fsin().to_f32(),
        MathRounding::Stored => radians(x).to_f64().sin() as f32,
        MathRounding::Float => float_radians(x).sin() as f32,
    }))
}

/// `cos(degrees)` (1.1d 0x5e618, 1.5b 0x5f8ac, UO 0x6b59b): [`sin`]'s
/// shape with `fcos` and libm's `cos`. VERIFIED (the instructions), and
/// `probe_math` on 1.1d, 1.5b and UO.
pub fn cos(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = float(args.first(), "cos takes a number")?;
    Ok(Value::Float(match host.profile.math {
        MathRounding::Kept => radians(x).fcos().to_f32(),
        MathRounding::Stored => radians(x).to_f64().cos() as f32,
        MathRounding::Float => float_radians(x).cos() as f32,
    }))
}

/// `tan(degrees)` (1.1d 0x5e654, 1.5b 0x5f8f6, UO 0x6b5e5): [`radians`]
/// stored as a float, its `fsincos` stored as two floats (UO's through
/// 0x6f0f7), sine over cosine; `"divide by 0"` (0x774f4) when the cosine is
/// 0. VERIFIED (the instructions), and `probe_math` on 1.1d, 1.5b and UO.
pub fn tan(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = float(args.first(), "tan takes a number")?;
    let (s, c) = if host.profile.math == MathRounding::Float {
        let r = float_radians(x) as f32;
        (r.sin(), r.cos())
    } else {
        let r = f64::from(radians(x).to_f32());
        (r.sin() as f32, r.cos() as f32)
    };
    if c == 0.0 {
        return Err(ErrorKind::Custom("divide by 0".into()));
    }
    Ok(Value::Float(s / c))
}

/// `asin(x)` in degrees (1.1d 0x5e6d0, 1.5b 0x5f978, UO 0x6b667): 1.1d's
/// `fpatan` of `x` over `sqrt(1 - x*x)` stored as a double, 1.5b's and UO's
/// libm `asin`, then the float radians through [`degrees`]. VERIFIED (the
/// instructions), and `probe_math` on 1.1d, 1.5b and UO.
pub fn asin(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = unit_arg(args, "asin takes a number")? as f64;
    Ok(match host.profile.math {
        MathRounding::Kept => degrees(x.atan2((1.0 - x * x).sqrt()) as f32),
        MathRounding::Stored => degrees(x.asin() as f32),
        MathRounding::Float => float_degrees(x.atan2((1.0 - x * x).sqrt())),
    })
}

/// `acos(x)` in degrees (1.1d 0x5e768, 1.5b 0x5fa0f, UO 0x6b6fe): [`asin`]'s
/// shape, `fpatan` of `sqrt(1 - x*x)` over `x` on 1.1d, libm's `acos` on
/// 1.5b and UO. VERIFIED (the instructions), and `probe_math` on 1.1d, 1.5b
/// and UO.
pub fn acos(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = unit_arg(args, "acos takes a number")? as f64;
    Ok(match host.profile.math {
        MathRounding::Kept => degrees((1.0 - x * x).sqrt().atan2(x) as f32),
        MathRounding::Stored => degrees(x.acos() as f32),
        MathRounding::Float => float_degrees((1.0 - x * x).sqrt().atan2(x)),
    })
}

/// `atan(x)` in degrees (1.1d 0x5e804, 1.5b 0x5faa6, UO 0x6b795): `fpatan`
/// over 1 (1.1d) or libm's `atan` (1.5b, UO) stored as a float, then
/// [`degrees`]. VERIFIED (the instructions), and `probe_math` on 1.1d, 1.5b
/// and UO.
pub fn atan(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let x = float(args.first(), "atan takes a number")? as f64;
    Ok(match host.profile.math {
        MathRounding::Float => float_degrees(x.atan()),
        _ => degrees(x.atan() as f32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// 1.7's `vectoangles` keeps the flat length a double: at (1.74,
    /// -899.78, -143.4) its pitch is 9.055189 (0x4110e20e, the float of
    /// the exact angle), where CoD1's float length gives 0x4110e20d. The
    /// yaw agrees.
    #[test]
    fn cod4_vectoangles_keeps_the_flat_length_a_double() {
        let v = [0x3fde_b852, 0xc460_f1ec, 0xc30f_6666].map(f32::from_bits);
        let (cod1, cod4) = (vectoangles(v), vectoangles_1_7(v));
        assert_eq!(cod4[0].to_bits(), 0x4110_e20e);
        assert_eq!(cod1[0].to_bits(), 0x4110_e20d);
        assert_eq!(cod4[1], cod1[1]);
        assert_eq!(vectoangles_1_7([0.0, 0.0, 1.0]), [270.0, 0.0, 0.0]);
    }

    /// 0x8191a72 adds 0 to a non-negative angle before its store, so a
    /// level forward vector's -0 pitch comes back +0.
    #[test]
    fn cod4_vectoangles_turns_a_negative_zero_positive() {
        let a = vectoangles_1_7([1.0, 0.0, 0.0]);
        assert_eq!(a[0].to_bits(), 0, "pitch +0");
        assert_eq!(a[1].to_bits(), 0, "yaw +0");
    }

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

    /// Retail's wrap: down is a small positive pitch, up wraps past 270.
    #[test]
    fn vector_to_angles_wraps_both_angles_like_retail() {
        assert_eq!(vectoangles([0.0, 0.0, 1.0]), [270.0, 0.0, 0.0]);
        assert_eq!(vectoangles([0.0, 0.0, -1.0]), [90.0, 0.0, 0.0]);
        let down = vectoangles([-300.0, 0.0, -85.92]);
        assert!(
            (down[0] - 15.99).abs() < 0.01 && down[1] == 180.0,
            "{down:?}"
        );
        let up = vectoangles([0.0, -1.0, 1.0]);
        assert!((up[0] - 315.0).abs() < 1e-4 && (up[1] - 270.0).abs() < 1e-4);
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

    /// The four script draws step one `holdrand` LCG (map-cycle doc 4.8):
    /// from a seed each is the retail formula on the next state's bits
    /// 17..31, and a refused call draws nothing.
    #[test]
    fn the_random_builtins_draw_holdrand() {
        let (mut vm, mut host) = fixture();
        host.holdrand = vcod_common::rng::HoldRand(12345);
        let mut s = 12345u32;
        let mut next = || {
            s = s.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
            (s >> 17) as i32
        };
        vm.with_cx(|cx| {
            let n = |v: i32| Value::Int(v);
            let f = |v: f32| Value::Float(v);
            assert!(random_int(&mut host, cx, None, &[n(0)]).is_err());
            assert!(random_int_range(&mut host, cx, None, &[n(5), n(5)]).is_err());
            assert!(random_float_range(&mut host, cx, None, &[f(2.0), n(1)]).is_err());
            let r = next();
            assert_eq!(
                random_int(&mut host, cx, None, &[n(100)]),
                Ok(n((100 * r) >> 15))
            );
            let r = next();
            let want = (2.5 * f64::from(r) / 32768.0) as f32;
            assert_eq!(random_float(&mut host, cx, None, &[f(2.5)]), Ok(f(want)));
            let r = next();
            assert_eq!(
                random_int_range(&mut host, cx, None, &[n(-3), n(7)]),
                Ok(n(((10 * r) >> 15) - 3))
            );
            let r = next();
            let want = (2.0 * f64::from(r) / 32768.0 - 1.0) as f32;
            assert_eq!(
                random_float_range(&mut host, cx, None, &[n(-1), n(1)]),
                Ok(f(want))
            );
        });
    }

    /// The trig builtins take and give degrees, and the inverse ones refuse
    /// a value off [-1, 1] as retail does.
    #[test]
    fn the_trig_builtins_work_in_degrees() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let f = |v: Result<Value, ErrorKind>| match v.unwrap() {
                Value::Float(f) => f,
                other => panic!("{other:?}"),
            };
            assert!((f(sin(&mut host, cx, None, &[Value::Int(30)])) - 0.5).abs() < 1e-6);
            assert!((f(cos(&mut host, cx, None, &[Value::Int(60)])) - 0.5).abs() < 1e-6);
            assert!((f(tan(&mut host, cx, None, &[Value::Float(45.0)])) - 1.0).abs() < 1e-6);
            assert!((f(asin(&mut host, cx, None, &[Value::Float(0.5)])) - 30.0).abs() < 1e-4);
            assert!((f(acos(&mut host, cx, None, &[Value::Float(0.5)])) - 60.0).abs() < 1e-4);
            assert!((f(atan(&mut host, cx, None, &[Value::Int(1)])) - 45.0).abs() < 1e-4);
            assert_eq!(
                acos(&mut host, cx, None, &[Value::Int(2)]),
                Err(ErrorKind::Custom("2 out of range".into()))
            );
        });
    }
}
