//! Cvar and level-state builtins: reads and writes `GameHost::cvars`, plus
//! the handful of level-clock and no-op builtins `dm.gsc`'s bootstrap calls
//! alongside them. Coercion answers (`atoi`/`atof`, case folding) come from
//! `probe_cvar`'s retail capture.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("getcvar", get_cvar),
    ("getcvarint", get_cvar_int),
    ("getcvarfloat", get_cvar_float),
    ("setcvar", set_cvar),
    ("makecvarserverinfo", make_cvar_server_info),
    ("gettime", get_time),
    ("randomint", random_int),
    ("resettimeout", reset_timeout),
    ("setarchive", set_archive),
    ("exitlevel", exit_level),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// C `atoi`: the longest numeric prefix, 0 when there is none. Rust's
/// `parse` rejects `"12abc"`, which retail reads as 12.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .position(|(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .unwrap_or(s.len());
    s[..end].parse().unwrap_or(0)
}

/// C `atof`, same prefix rule with a decimal point and an exponent.
fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let ok = c.is_ascii_digit()
            || (i == 0 && (c == '-' || c == '+'))
            || (c == '.' && !s[..i].contains('.'))
            || ((c == 'e' || c == 'E') && i > 0 && !s[..i].contains(['e', 'E']))
            || ((c == '-' || c == '+') && matches!(s.as_bytes().get(i - 1), Some(b'e' | b'E')));
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    s[..end].parse().unwrap_or(0.0)
}

/// `getCvar(name)`: an empty string for a name that is not set, which is
/// what retail's `Cvar_VariableString` returns for an unregistered cvar.
pub fn get_cvar(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("getCvar takes a cvar name"));
    };
    let name = *name;
    let text = cx.resolve(name).to_string();
    let value = host.cvars.get(&text).to_string();
    Ok(Value::String(cx.intern_exact(&value)))
}

/// `getCvarInt(name)`: `atoi` on the cvar's string value, 0 for an unset
/// name (`probe_cvar`'s `cvar_int_of_empty`).
pub fn get_cvar_int(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("getCvarInt takes a cvar name"));
    };
    let text = cx.resolve(*name).to_string();
    Ok(Value::Int(atoi(host.cvars.get(&text))))
}

/// `getCvarFloat(name)`: `atof` on the cvar's string value.
pub fn get_cvar_float(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("getCvarFloat takes a cvar name"));
    };
    let text = cx.resolve(*name).to_string();
    Ok(Value::Float(atof(host.cvars.get(&text))))
}

/// `setCvar(name, value)`: renders the value through `Cx::format_number`,
/// not Rust's own formatter, so a float renders the way `%g` does.
/// `dm.gsc` calls `setCvar("scr_allow_vote", level.allowvote)` with an int
/// and `updateScriptCvars` writes floats; both go through the same
/// renderer `println` and `setCullFog` use.
pub fn set_cvar(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::String(name)), Some(&value)) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType("setCvar takes a name and a value"));
    };
    let name = cx.resolve(*name).to_string();
    let rendered = cx
        .format_number(value)
        .ok_or(ErrorKind::BadType("setCvar takes a renderable value"))?;
    host.cvars.set(&name, &rendered);
    Ok(Value::Undefined)
}

/// `makeCvarServerInfo(name, default)`: flags the cvar into the 140/204
/// mirror, registering it with `default` only when it does not already
/// exist, which is what lets a command-line override survive
/// `_teams::initGlobalCvars`.
pub fn make_cvar_server_info(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::String(name)), Some(&default)) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadType(
            "makeCvarServerInfo takes a name and a default value",
        ));
    };
    let name = cx.resolve(*name).to_string();
    let default = cx.format_number(default).ok_or(ErrorKind::BadType(
        "makeCvarServerInfo takes a renderable default",
    ))?;
    host.cvars.make_server_info(&name, &default);
    Ok(Value::Undefined)
}

/// `getTime()`: the level clock in milliseconds, the same units
/// `run_frame`'s `now_ms` carries throughout the host. Retail's own units
/// are unmeasured -- `probe_cvar` only established that the value is
/// non-negative -- so this is the reading chosen, not a measurement.
pub fn get_time(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(host.level_time_ms))
}

/// `randomInt(n)`: a uniform draw in `[0, n)`, so `randomInt(1)` is always
/// 0 (measured). `n <= 0` returns 0 rather than dividing by zero.
pub fn random_int(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let n = match args.first() {
        Some(Value::Int(i)) => *i,
        Some(Value::Float(f)) => *f as i32,
        _ => return Err(ErrorKind::BadType("randomInt takes a number")),
    };
    if n <= 0 {
        return Ok(Value::Int(0));
    }
    Ok(Value::Int((host.rand_unit() * n as f32) as i32))
}

/// `resetTimeout()`: unmeasured no-op. No probe exercises it and nothing in
/// the corpus depends on a timeout it would reset.
pub fn reset_timeout(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `setArchive(name, value)`: unmeasured no-op. Nothing in the corpus reads
/// a cvar's archive flag back.
pub fn set_archive(
    _host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `exitLevel()`: sets a flag `ScriptRuntime::run_frame` reads and drains
/// each frame. No stage in this sub-project acts on it; stage 6 ("the
/// score limit ends the map") is where it does.
pub fn exit_level(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    host.exit_level = true;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `getCvar` answers from the server's cvar table and returns an empty
    /// string for one that is not set, which is what retail does.
    #[test]
    fn getcvar_answers_from_the_server_and_empty_for_unknown() {
        let (mut vm, mut host) = fixture();
        host.cvars.set("scr_dm_scorelimit", "50");
        vm.with_cx(|cx| {
            let k = Value::String(cx.intern_exact("scr_dm_scorelimit"));
            match get_cvar(&mut host, cx, None, &[k]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "50"),
                v => panic!("{v:?}"),
            }
            let miss = Value::String(cx.intern_exact("no_such_cvar"));
            match get_cvar(&mut host, cx, None, &[miss]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), ""),
                v => panic!("{v:?}"),
            }
        });
    }

    /// `getCvarInt`/`getCvarFloat` are `atoi`/`atof`: a non-numeric string is
    /// 0 and a numeric prefix parses, both measured by probe_cvar.
    #[test]
    fn getcvarint_and_getcvarfloat_are_atoi_and_atof() {
        let (mut vm, mut host) = fixture();
        host.cvars.set("probe_text", "abc");
        host.cvars.set("probe_trailing", "12abc");
        host.cvars.set("probe_third", "0.3333333333");
        vm.with_cx(|cx| {
            let k = |cx: &mut vcod_gsc::Cx, s: &str| Value::String(cx.intern_exact(s));
            let a = k(cx, "probe_text");
            assert_eq!(
                get_cvar_int(&mut host, cx, None, &[a]).unwrap(),
                Value::Int(0)
            );
            let b = k(cx, "probe_trailing");
            assert_eq!(
                get_cvar_int(&mut host, cx, None, &[b]).unwrap(),
                Value::Int(12)
            );
            let c = k(cx, "no_such_cvar");
            assert_eq!(
                get_cvar_int(&mut host, cx, None, &[c]).unwrap(),
                Value::Int(0)
            );
            let d = k(cx, "probe_third");
            assert_eq!(
                get_cvar_float(&mut host, cx, None, &[d]).unwrap(),
                Value::Float(0.3333333333_f32)
            );
        });
    }

    /// `setCvar` renders a number through `Cx::format_number`, not Rust's
    /// formatter: `dm.gsc` calls `setCvar("scr_allow_vote", level.allowvote)`
    /// with an int, and `updateScriptCvars` writes floats.
    #[test]
    fn setcvar_renders_numbers_the_way_retail_does() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let name = Value::String(cx.intern_exact("scr_allow_vote"));
            set_cvar(&mut host, cx, None, &[name, Value::Int(1)]).unwrap();
            assert_eq!(host.cvars.get("scr_allow_vote"), "1");
            let third = Value::String(cx.intern_exact("probe_third"));
            set_cvar(&mut host, cx, None, &[third, Value::Float(1.0 / 3.0)]).unwrap();
            assert_eq!(host.cvars.get("probe_third"), "0.333333");
        });
    }

    /// `makeCvarServerInfo` puts the cvar in the 140/204 mirror and leaves an
    /// existing value alone, which is what lets a command-line override
    /// survive `_teams::initGlobalCvars`.
    #[test]
    fn makecvarserverinfo_flags_without_overwriting() {
        let (mut vm, mut host) = fixture();
        host.cvars.set("scr_allow_fg42", "0");
        vm.with_cx(|cx| {
            let name = Value::String(cx.intern_exact("scr_allow_fg42"));
            let default = Value::String(cx.intern_exact("1"));
            make_cvar_server_info(&mut host, cx, None, &[name, default]).unwrap();
        });
        let mut cs = vec![String::new(); 2048];
        host.cvars.write_mirror(&mut cs).unwrap();
        assert_eq!(host.cvars.get("scr_allow_fg42"), "0");
        assert!(cs[140..=203].contains(&"scr_allow_fg42".to_string()));
    }

    /// `randomInt(n)` is `[0, n)`, so `randomInt(1)` is always 0 and
    /// `randomInt(0)` cannot divide by zero.
    #[test]
    fn randomint_is_half_open() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            for _ in 0..64 {
                assert_eq!(
                    random_int(&mut host, cx, None, &[Value::Int(1)]).unwrap(),
                    Value::Int(0)
                );
                match random_int(&mut host, cx, None, &[Value::Int(2)]).unwrap() {
                    Value::Int(n) => assert!((0..2).contains(&n), "{n} out of range"),
                    v => panic!("{v:?}"),
                }
            }
            assert_eq!(
                random_int(&mut host, cx, None, &[Value::Int(0)]).unwrap(),
                Value::Int(0)
            );
        });
    }
}
