//! Builtins that write map-wide engine state: fog and the ambient track.

use vcod_gsc::{Cx, ErrorKind, Value};

/// `setCullFog(near, far, r, g, b, startDist)` -> configstring 12, which
/// carries seven fields for those six arguments: retail inserts a literal
/// `1` after the far distance (docs/research/clientstate-wire-format.md).
pub fn set_cull_fog(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    if args.len() != 6 {
        return Err(ErrorKind::BadType("setCullFog takes six arguments"));
    }
    let mut parts = Vec::with_capacity(7);
    for (i, a) in args.iter().enumerate() {
        parts.push(
            cx.format_number(*a)
                .ok_or(ErrorKind::BadType("setCullFog needs numbers"))?,
        );
        if i == 1 {
            parts.push("1".to_string());
        }
    }
    cs[12] = parts.join(" ");
    Ok(Value::Undefined)
}

/// `ambientPlay(alias)` -> configstring 3 (`GScr_AmbientPlay` 0x5ae96).
pub fn ambient_play(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    let Some(Value::String(a)) = args.first() else {
        return Err(ErrorKind::BadType("ambientPlay needs an alias"));
    };
    cs[3] = format!("n\\{}\\t\\0", cx.resolve(*a));
    Ok(Value::Undefined)
}
