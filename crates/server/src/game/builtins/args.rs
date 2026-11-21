//! Argument readers every builtin family shares, and the do-nothing
//! handler. Each takes the usage text its error carries; which builtin
//! truncates a float and which refuses one stays with the caller.

use glam::Vec3;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

use crate::game::host::GameHost;

/// `Scr_GetFloat`'s rule: an int reads as its value.
pub fn as_float(v: &Value) -> Option<f32> {
    match *v {
        Value::Int(i) => Some(i as f32),
        Value::Float(f) => Some(f),
        _ => None,
    }
}

/// `Scr_GetInt`'s rule: a float truncates toward zero.
pub fn as_int(v: &Value) -> Option<i32> {
    match *v {
        Value::Int(i) => Some(i),
        Value::Float(f) => Some(f as i32),
        _ => None,
    }
}

pub fn as_vector(v: &Value) -> Option<Vec3> {
    match *v {
        Value::Vector(v) => Some(Vec3::from(v)),
        _ => None,
    }
}

/// [`as_float`], or `BadType(usage)`.
pub fn float(v: Option<&Value>, usage: &'static str) -> Result<f32, ErrorKind> {
    v.and_then(as_float).ok_or(ErrorKind::BadType(usage))
}

/// A number widened to f64 without passing through f32: a large int keeps
/// every bit.
pub fn double(v: Option<&Value>, usage: &'static str) -> Result<f64, ErrorKind> {
    match v {
        Some(&Value::Int(i)) => Ok(f64::from(i)),
        Some(&Value::Float(f)) => Ok(f64::from(f)),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

/// [`as_int`], or `BadType(usage)`.
pub fn int(v: Option<&Value>, usage: &'static str) -> Result<i32, ErrorKind> {
    v.and_then(as_int).ok_or(ErrorKind::BadType(usage))
}

pub fn vector(v: Option<&Value>, usage: &'static str) -> Result<Vec3, ErrorKind> {
    v.and_then(as_vector).ok_or(ErrorKind::BadType(usage))
}

/// A string argument; a localized string is refused.
pub fn string<'a>(
    cx: &'a Cx,
    v: Option<&Value>,
    usage: &'static str,
) -> Result<&'a str, ErrorKind> {
    match v {
        Some(&Value::String(s)) => Ok(cx.resolve(s)),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

/// A string or a localized string argument.
pub fn string_or_localized<'a>(
    cx: &'a Cx,
    v: Option<&Value>,
    usage: &'static str,
) -> Result<&'a str, ErrorKind> {
    match v {
        Some(&(Value::String(s) | Value::Localized(s))) => Ok(cx.resolve(s)),
        _ => Err(ErrorKind::BadType(usage)),
    }
}

/// A builtin with nothing for the server to do; each table row says why.
pub fn noop(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_as_scr_get_float_and_scr_get_int() {
        assert_eq!(as_float(&Value::Int(3)), Some(3.0));
        assert_eq!(as_int(&Value::Float(-2.9)), Some(-2));
        assert_eq!(as_int(&Value::Undefined), None);
        assert_eq!(double(Some(&Value::Int(16_777_217)), "u"), Ok(16_777_217.0));
        assert_eq!(float(None, "u"), Err(ErrorKind::BadType("u")));
    }
}
