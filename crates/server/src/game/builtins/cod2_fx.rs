//! CoD2's `playFx` and `playLoopedFx`, which take an up vector CoD1's do
//! not and write different fields (`BuiltinSet::cod2`; cod2-efx.md 6.6).
//! `loadFx` and `playFxOnTag` are CoD1's (`builtins::fx`) over CoD2's
//! configstring blocks. Addresses are cod2_lnxded's.

use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};
use glam::Vec3;
use vcod_common::net::event_ids::EV_PLAY_FX;
use vcod_common::net::events::dir_to_byte;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

use super::fx::{LoopedFx, spawn_looped_fx};
use super::math::vectoangles;
use super::score::Builtin;

pub const NAMES: &[(&str, Builtin)] = &[("playfx", play_fx), ("playloopedfx", play_looped_fx)];

fn number(v: Option<&Value>, usage: &'static str) -> Result<f32, ErrorKind> {
    match v {
        Some(Value::Int(n)) => Ok(*n as f32),
        Some(Value::Float(f)) => Ok(*f),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

fn vector(v: Option<&Value>, usage: &'static str) -> Result<Vec3, ErrorKind> {
    match v {
        Some(Value::Vector(v)) => Ok(Vec3::from(*v)),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

/// `v` scaled by `1 / |v|`, `None` for a zero vector.
fn unit(v: Vec3) -> Option<Vec3> {
    let len = v.length();
    (len != 0.0).then(|| v * (1.0 / len))
}

/// `playFx(fx, origin[, forward[, up]])` (`Scr_PlayFX` 0x81150a4): a
/// `G_TempEntity` at `origin` raising `EV_PLAY_FX` (194) with the id's low
/// byte in `eventParm` and the effect's angles in `apos.trBase`: (-90, 0,
/// 0), straight up, without a forward; `vectoangles(forward)` with one;
/// `AxisToAngles` of forward, `up x forward` and up with both, the up made
/// square to the forward when the two are less than 0.9999 apart
/// (0x3f7fbe77) and refused when within 0.001 (0x3a83126f) of parallel.
/// VERIFIED. The event goes out in vcod's 1.1 numbering and the angles in
/// the temp entity's `origin2` ([`TempEntity::origin2`]).
pub fn play_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "Incorrect number of parameters";
    if !(2..=4).contains(&args.len()) {
        return Err(ErrorKind::BadType(USAGE));
    }
    let Some(Value::Int(id)) = args.first() else {
        return Err(ErrorKind::BadType("playFx takes an effect id from loadFx"));
    };
    let origin = vector(args.get(1), USAGE)?;
    let angles = match args.len() {
        2 => [-90.0, 0.0, 0.0],
        n => {
            let fwd = unit(vector(args.get(2), USAGE)?).ok_or(ErrorKind::BadType(
                "playFx called with (0 0 0) forward direction",
            ))?;
            if n == 3 {
                vectoangles(fwd.to_array())
            } else {
                let mut up = unit(vector(args.get(3), USAGE)?).ok_or(ErrorKind::BadType(
                    "playFx called with (0 0 0) up direction",
                ))?;
                let mut left = up.cross(fwd);
                let len = left.length();
                if len < 0.001 {
                    return Err(ErrorKind::BadType(
                        "playFx called an up direction 0 or 180 degrees from forward",
                    ));
                }
                if len < 0.9999 {
                    left *= 1.0 / len;
                    up = fwd.cross(left);
                }
                axis_to_angles([fwd, left, up])
            }
        }
    };
    host.add_temp_entity(
        cx,
        TempEntity {
            event: EV_PLAY_FX,
            parm: id & 0xff,
            surf_type: 0,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: origin.to_array(),
            scope: Scope::Pvs,
            quake: None,
            origin2: Some(angles),
        },
    );
    Ok(Value::Undefined)
}

/// `AxisToAngles` (0x80a9558): pitch and yaw are `vectoangles` of the
/// forward; the left vector turned by minus the yaw about z and minus the
/// pitch about y gives the roll: its pitch (`-atan2(z, |xy|)` in degrees,
/// the -180.0 at 0x814d0a0) negated, or pushed past 180 when the turned
/// left's y is negative (the 180.0 and -180.0 at 0x814d210). VERIFIED for the
/// steps; the sines in double stored as floats.
fn axis_to_angles(axis: [Vec3; 3]) -> [f32; 3] {
    let [pitch, yaw, _] = vectoangles(axis[0].to_array());
    let sin_cos = |deg: f32| {
        let a = (std::f64::consts::PI / 180.0 * f64::from(-deg)) as f32;
        let a = f64::from(a);
        (a.sin() as f32, a.cos() as f32)
    };
    let l = axis[1];
    let (s, c) = sin_cos(yaw);
    let x = c * l.x - s * l.y;
    let y = c * l.y + s * l.x;
    let (s, c) = sin_cos(pitch);
    let x2 = c * x + s * l.z;
    let z = c * l.z - s * x;
    let r = if x2 == 0.0 && y == 0.0 {
        if z > 0.0 { -90.0 } else { 90.0 }
    } else {
        let h = (y * y + x2 * x2).sqrt();
        (f64::from(z).atan2(f64::from(h)) * -180.0 / std::f64::consts::PI) as f32
    };
    let roll = if y < 0.0 {
        r + if r >= 0.0 { -180.0 } else { 180.0 }
    } else {
        -r
    };
    [pitch, yaw, roll]
}

/// `playLoopedFx(fx, delay, origin[, cull[, forward[, up]]])`
/// (`Scr_PlayLoopedFX` 0x8115466): CoD1's `ET_FX` entity (`eType` 8,
/// `svFlags` 8) with an optional up vector, whose `DirToByte` goes in
/// `eventParm`; the effect id's low byte in `scale`, the forward in
/// `origin2`, the cull in `angles2[0]` and the delay in `angles2[1]` as
/// `floor(delay * 1000 + 0.5)` ms (0x81185e6, the 1000.0 at 0x815c9e0 and
/// the 0.5 at 0x815e188), which must come to at least 1. A zero forward
/// or up is an error. VERIFIED.
pub fn play_looped_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "USAGE: playLoopedFx <effect id from loadFx> <repeat delay> <vector position of effect> <optional cull distance (0 = never cull)> <optional forward vector> <optional up vector>";
    if !(3..=6).contains(&args.len()) {
        return Err(ErrorKind::BadType(USAGE));
    }
    let Some(Value::Int(fx)) = args.first() else {
        return Err(ErrorKind::BadType(USAGE));
    };
    let up = match args.get(5) {
        Some(v) => unit(vector(Some(v), USAGE)?).ok_or(ErrorKind::BadType(
            "playLoopedFx called with (0 0 0) up direction",
        ))?,
        None => Vec3::ZERO,
    };
    let forward = match args.get(4) {
        Some(v) => unit(vector(Some(v), USAGE)?).ok_or(ErrorKind::BadType(
            "playLoopedFx called with (0 0 0) forward direction",
        ))?,
        None => Vec3::ZERO,
    };
    let cull = match args.get(3) {
        Some(v) => number(Some(v), USAGE)?,
        None => 0.0,
    };
    let origin = vector(args.get(2), USAGE)?;
    let period_ms = (number(args.get(1), USAGE)? * 1000.0 + 0.5).floor() as i32;
    if period_ms < 1 {
        return Err(ErrorKind::BadType(
            "playLoopedFx called with a repeat under 0.0005s",
        ));
    }
    spawn_looped_fx(
        host,
        cx,
        origin.to_array(),
        LoopedFx {
            id: fx & 0xff,
            forward: forward.to_array(),
            cull,
            period_ms,
            up: dir_to_byte(up.to_array()) & 0xff,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AxisToAngles` undoes `AnglesToAxis`, roll included.
    #[test]
    fn axis_to_angles_reads_back_a_turned_axis() {
        for angles in [[10.0, 90.0, 30.0], [-20.0, 200.0, -45.0], [0.0, 0.0, 0.0]] {
            let axis = vcod_common::x87::angles_to_axis(Vec3::from(angles));
            let got = axis_to_angles(axis);
            let want = vectoangles(axis[0].to_array());
            assert!((got[0] - want[0]).abs() < 1e-3, "{angles:?} {got:?}");
            assert!((got[1] - want[1]).abs() < 1e-3, "{angles:?} {got:?}");
            assert!((got[2] - angles[2]).abs() < 1e-2, "{angles:?} {got:?}");
        }
    }

    /// CoD2's `playFx` raises `EV_PLAY_FX` with the id in `eventParm` and
    /// angles in `apos.trBase` on CoD2's wire: straight up without a
    /// forward, the forward's angles with one, a rolled axis with an up.
    /// A zero forward, an up along the forward and a fifth argument are
    /// errors.
    #[test]
    fn playfx_puts_its_angles_in_apos() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([10.5, 20.0, -30.5]);
            let fwd = Value::Vector([0.0, 2.0, 0.0]);
            let up = Value::Vector([1.0, 1.0, 1.0]);
            play_fx(&mut host, cx, None, &[Value::Int(3), at]).unwrap();
            play_fx(&mut host, cx, None, &[Value::Int(3), at, fwd]).unwrap();
            play_fx(&mut host, cx, None, &[Value::Int(3), at, fwd, up]).unwrap();
            let zero = Value::Vector([0.0; 3]);
            assert!(play_fx(&mut host, cx, None, &[Value::Int(3), at, zero]).is_err());
            assert!(play_fx(&mut host, cx, None, &[Value::Int(3), at, fwd, fwd]).is_err());
            assert!(play_fx(&mut host, cx, None, &[Value::Int(3), at, fwd, up, up]).is_err());
        });
        assert_eq!(host.temp_entities.len(), 3);
        let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
        let apos = |i: usize| {
            let te = &host.temp_entities[i].te;
            assert_eq!(
                (te.event, te.parm, te.origin),
                (EV_PLAY_FX, 3, [10.5, 20.0, -30.5])
            );
            let es = crate::game::temp_entity::build(te, 70, p);
            assert_eq!(es.field_f32(p, "origin2[0]"), 0.0);
            assert_eq!(es.field_f32(p, "pos.trBase[0]"), 10.0);
            [0, 1, 2].map(|k| es.field_f32(p, &format!("apos.trBase[{k}]")))
        };
        assert_eq!(apos(0), [-90.0, 0.0, 0.0]);
        assert_eq!(apos(1), [0.0, 90.0, 0.0]);
        // Up (1, 1, 1) made square to the forward +y: (1, 0, 1), rolled 45.
        let rolled = apos(2);
        assert_eq!(&rolled[..2], &[0.0, 90.0]);
        assert!((rolled[2] - 45.0).abs() < 1e-3, "{rolled:?}");
    }

    /// CoD2's `playLoopedFx` takes a sixth argument, the up, whose
    /// `DirToByte` goes in `eventParm`; a delay under half a ms is an
    /// error, and the delay rounds to whole ms.
    #[test]
    fn playloopedfx_carries_the_up_in_eventparm() {
        let (mut vm, mut host) = crate::game::testing::fixture();
        let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
        let (fx, states) = vm.with_cx(|cx| {
            let at = Value::Vector([8624.0, 881.0, 61.0]);
            let fwd = Value::Vector([0.0, 0.0, 10.0]);
            let up = Value::Vector([1.0, 0.0, 0.0]);
            let args = [Value::Int(4), Value::Float(0.3), at, Value::Int(0), fwd, up];
            let fx = play_looped_fx(&mut host, cx, None, &args).unwrap();
            let fast = [Value::Int(4), Value::Float(0.0004), at];
            assert!(play_looped_fx(&mut host, cx, None, &fast).is_err());
            let zero = Value::Vector([0.0; 3]);
            let bad = [
                Value::Int(4),
                Value::Float(1.0),
                at,
                Value::Int(0),
                fwd,
                zero,
            ];
            assert!(play_looped_fx(&mut host, cx, None, &bad).is_err());
            (fx, crate::game::wire::packet_entities(&mut host, cx, p))
        });
        let Value::Entity(fx) = fx else {
            panic!("{fx:?}");
        };
        let es = &states[&fx.0];
        assert_eq!(es.field_i32(p, "scale"), 4);
        assert_eq!(es.field_f32(p, "origin2[2]"), 1.0);
        assert_eq!(es.field_f32(p, "angles2[1]"), 300.0);
        assert_eq!(es.field_i32(p, "eventParm"), dir_to_byte([1.0, 0.0, 0.0]));
    }
}
