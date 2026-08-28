//! Builtins that write map-wide engine state: fog and the ambient track.

use vcod_gsc::{Cx, ErrorKind, Value};

/// `setCullFog(near, far, r, g, b, startDist)` -> configstring 12, which
/// carries seven fields for those six arguments: the slot after the far
/// distance is the density, and a density >= 1 selects linear fog, which is
/// what `setCullFog` means. Retail writes `1` there on both captured maps
/// (docs/research/cod11-server-handshake.md, "Map-dependent"); the field's
/// meaning is in docs/protocol-1.1.md, "Configstring indices". `setExpFog`
/// will write its own density in the same slot, not this constant.
pub fn set_cull_fog(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    debug_assert!(cs.len() > 12, "configstring table shorter than slot 12");
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

/// `ambientPlay(alias [, fade])` -> configstring 3 (`GScr_AmbientPlay`
/// 0x5ae96). The `t` field is `level.time + fade * 1000`
/// (docs/research/cod11-sound-system.md, "Per gsc call"); an optional second
/// argument is accepted and dropped, and `t` is written as `0`. Only three
/// shipped SP scripts pass a fade, no MP map does, and the level clock the
/// real value needs is not wired to the host yet.
pub fn ambient_play(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    debug_assert!(cs.len() > 3, "configstring table shorter than slot 3");
    let Some(Value::String(a)) = args.first() else {
        return Err(ErrorKind::BadType("ambientPlay needs an alias"));
    };
    cs[3] = format!("n\\{}\\t\\0", cx.resolve(*a));
    Ok(Value::Undefined)
}
