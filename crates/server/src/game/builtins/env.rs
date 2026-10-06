//! Builtins that write map-wide engine state: fog and the ambient track.

use vcod_gsc::{Cx, ErrorKind, Value};

/// `setCullFog(near, far, r, g, b, transitionTime)` -> configstring 12, which
/// carries seven fields for those six arguments: the slot after the far
/// distance is the density, and a density >= 1 selects linear fog, which is
/// what `setCullFog` means. Retail writes `1` there on both captured maps
/// (docs/research/cod11-server-handshake.md, "Map-dependent"); the field's
/// meaning is in docs/protocol-1.1.md, "Configstring indices".
pub fn set_cull_fog(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    if args.len() != 6 {
        return Err(ErrorKind::BadType("setCullFog takes six arguments"));
    }
    let a = floats(args, "setCullFog needs numbers")?;
    write_fog(cs, cx, [a[0], a[1], 1.0, a[2], a[3], a[4]], a[5])
}

/// `setExpFog(density, r, g, b, transitionTime)` (0x5b774) -> configstring 12
/// as `0 1 <density> <r> <g> <b> <ms>`: near 0 and far 1 are constants, and
/// the density below 1 is what selects exponential fog. Retail refuses a
/// density outside (0, 1), a colour outside [0, 1] and a negative time
/// (docs/research/cod11-gametypes-re-bel.md, section 2).
pub fn set_exp_fog(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    if args.len() != 5 {
        return Err(ErrorKind::BadType("setExpFog takes five arguments"));
    }
    let a = floats(args, "setExpFog needs numbers")?;
    if !(a[0] > 0.0 && a[0] < 1.0) {
        return Err(ErrorKind::BadType(
            "setExpFog: distance must be greater than 0 and less than 1",
        ));
    }
    write_fog(cs, cx, [0.0, 1.0, a[0], a[1], a[2], a[3]], a[4])
}

fn floats(args: &[Value], err: &'static str) -> Result<Vec<f32>, ErrorKind> {
    args.iter()
        .map(|a| match a {
            Value::Int(i) => Ok(*i as f32),
            Value::Float(f) => Ok(*f),
            _ => Err(ErrorKind::BadType(err)),
        })
        .collect()
}

/// The two fog builtins' shared tail: retail's `"%g %g %g %g %g %g %.0f"`
/// with the transition time in milliseconds, through `G_setfog` (0x48fa4),
/// which is `trap_SetConfigstring(12, ...)`. Both builtins reject a colour
/// outside [0, 1] and a negative time.
fn write_fog(cs: &mut [String], cx: &Cx, head: [f32; 6], seconds: f32) -> Result<Value, ErrorKind> {
    debug_assert!(cs.len() > 12, "configstring table shorter than slot 12");
    if head[3..].iter().any(|c| !(0.0..=1.0).contains(c)) {
        return Err(ErrorKind::BadType(
            "red/green/blue color components must be in the range [0, 1]",
        ));
    }
    if seconds < 0.0 {
        return Err(ErrorKind::BadType("transition time must be >= 0 seconds"));
    }
    let mut parts: Vec<String> = head
        .iter()
        .map(|f| cx.format_number(Value::Float(*f)).expect("a float formats"))
        .collect();
    parts.push(format!("{:.0}", seconds * 1000.0));
    cs[12] = parts.join(" ");
    Ok(Value::Undefined)
}

/// `ambientPlay(alias [, fade])` -> configstring 3 (`GScr_AmbientPlay`
/// 0x5ae96). The `t` field is `level.time + fade * 1000`
/// (docs/research/cod11-sound-system.md, "Per gsc call"); an optional second
/// argument is accepted and dropped, and `t` is written as `0`. Only three
/// shipped SP scripts pass a fade, no MP map does, and `host.level_time_ms`
/// is not threaded to this builtin, which is one half of the restart
/// divergence in docs/research/cod11-map-cycle.md 4.5: a pinned `t` never
/// moves, so a restart has nothing to rebroadcast for slot 3.
pub fn ambient_play(cs: &mut [String], cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    debug_assert!(cs.len() > 3, "configstring table shorter than slot 3");
    let Some(Value::String(a)) = args.first() else {
        return Err(ErrorKind::BadType("ambientPlay needs an alias"));
    };
    cs[3] = format!("n\\{}\\t\\0", cx.resolve(*a));
    Ok(Value::Undefined)
}
