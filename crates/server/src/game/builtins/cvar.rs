//! Cvar and level-state builtins: reads and writes `GameHost::cvars`, plus
//! the handful of level-clock and no-op builtins `dm.gsc`'s bootstrap calls
//! alongside them. Coercion answers (`atoi`/`atof`, case folding) come from
//! `probe_cvar`'s retail capture.

use crate::game::host::{GameHost, LevelLatch};
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
    ("map_restart", map_restart),
    ("setclientnamemode", set_client_name_mode),
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
/// renderer `println` and `setCullFog` use. A localized key renders the way
/// `setClientCvar`'s does, `KEY\x15`: `_teams::scoreboard`'s
/// `setcvar("g_TeamName_Allies", &"MPSCRIPT_AMERICAN")` reaches the
/// gamestate as `MPSCRIPT_AMERICAN\x15`
/// (`tests/fixtures/configstrings/mp_carentan-sd.txt`, slots 223 and 224).
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
    let rendered = match value {
        Value::Localized(_) => super::message::construct(host, cx, &[value]),
        _ => cx
            .format_number(value)
            .ok_or(ErrorKind::BadType("setCvar takes a renderable value"))?,
    };
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

/// Who owns a client's name: `level+0x210` on retail, written only by
/// `setClientNameMode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClientNameMode {
    /// `"auto_change"`, level+0x210 = 0: `ClientUserinfoChanged` (0x421eb)
    /// takes the name straight from the userinfo. The engine default.
    #[default]
    Auto,
    /// `"manual_change"`, level+0x210 = 1: the same branch is skipped and
    /// the name stays whatever script last set.
    Manual,
}

/// `setClientNameMode(mode)` (`game.mp.i386.so` 0x5f208). Retail resolves
/// the argument as a const string, matches it against `auto_change` and
/// `manual_change` (`scr_const+0xfc` and `+0xfe`, named by `GScr_LoadConsts`
/// 0x58550), stores 0 or 1 in `level+0x210`, and raises `Scr_Error("Unknown
/// mode")` on anything else.
///
/// The store and the error are faithful; the two readers are not reachable
/// yet. `ClientUserinfoChanged` (0x421eb) and the name-change path at
/// 0x5ba99 are both client code, which arrives in a later stage, so
/// `GameHost::client_name_mode` is recorded and nothing reads it.
pub fn set_client_name_mode(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(mode)) = args.first() else {
        return Err(ErrorKind::BadType("setClientNameMode takes a mode string"));
    };
    // String values intern exactly, so this compares the spelling the script
    // wrote against retail's two const strings, which is the comparison
    // retail makes.
    host.client_name_mode = match cx.resolve(*mode) {
        "auto_change" => ClientNameMode::Auto,
        "manual_change" => ClientNameMode::Manual,
        _ => return Err(ErrorKind::BadType("setClientNameMode: unknown mode")),
    };
    Ok(Value::Undefined)
}

/// `exitLevel([savePersist])` (map-cycle doc, sections 1 and 2): the shared
/// one-shot latch, then `ExitLevel` itself -- the queued `map_rotate`, both
/// team scores and every connected client's score back to 0, and the log
/// line. Nothing else: no intermission state, no camera and no timer.
pub fn exit_level(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    latch(host, LevelLatch::ExitLevel, args)?;
    host.console.push("map_rotate".to_string());
    host.team_scores = [0, 0];
    host.zero_client_scores();
    host.script_log.push("ExitLevel: executed".to_string());
    Ok(Value::Undefined)
}

/// `map_restart([savePersist])` (map-cycle doc, section 1): the same latch
/// and the same optional argument, queueing the other console command.
pub fn map_restart(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    latch(host, LevelLatch::MapRestart, args)?;
    host.console.push("map_restart".to_string());
    Ok(Value::Undefined)
}

/// What the two share: the one-shot latch, whose error names whichever call
/// got there first, and `savePersist` off the optional first argument.
/// Retail reads it with `Scr_GetInt`, so a non-number is a script error.
fn latch(host: &mut GameHost, which: LevelLatch, args: &[Value]) -> Result<(), ErrorKind> {
    match host.level_latch {
        LevelLatch::None => {}
        LevelLatch::MapRestart => return Err(ErrorKind::BadType("map_restart already called")),
        LevelLatch::ExitLevel => return Err(ErrorKind::BadType("exitlevel already called")),
    }
    // The argument is read before the latch takes, so a call that raises
    // leaves the level able to end later; `Scr_GetInt` truncates a float
    // rather than testing it against zero.
    let save_persist = match args.first() {
        None => false,
        Some(Value::Int(i)) => *i != 0,
        Some(Value::Float(f)) => (*f as i32) != 0,
        Some(_) => return Err(ErrorKind::BadType("map_restart/exitLevel wants a number")),
    };
    host.level_latch = which;
    host.save_persist = save_persist;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
    use vcod_gsc::Host;

    /// `setClientNameMode` records retail's two modes and raises on anything
    /// else, the way `Scr_Error("Unknown mode")` does.
    #[test]
    fn setclientnamemode_records_the_mode_and_refuses_any_other() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let manual = Value::String(cx.intern_exact("manual_change"));
            set_client_name_mode(&mut host, cx, None, &[manual]).unwrap();
            assert_eq!(host.client_name_mode, ClientNameMode::Manual);
            let auto = Value::String(cx.intern_exact("auto_change"));
            set_client_name_mode(&mut host, cx, None, &[auto]).unwrap();
            assert_eq!(host.client_name_mode, ClientNameMode::Auto);
            let other = Value::String(cx.intern_exact("whenever"));
            assert!(set_client_name_mode(&mut host, cx, None, &[other]).is_err());
            assert_eq!(host.client_name_mode, ClientNameMode::Auto);
        });
    }

    /// The two level-ending builtins share one latch, take the same
    /// optional `savePersist` and queue different console lines
    /// (docs/research/cod11-map-cycle.md sections 1 and 2). The second call
    /// names whichever got there first, not the one being refused.
    #[test]
    fn map_restart_and_exitlevel_latch_once_and_stash_save_persist() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            map_restart(&mut host, cx, None, &[Value::Int(1)]).unwrap();
            assert_eq!(host.level_latch, LevelLatch::MapRestart);
            assert!(host.save_persist);
            assert_eq!(host.console, vec!["map_restart".to_string()]);
            // Both are refused now, and the message names the first call.
            let e = exit_level(&mut host, cx, None, &[]).unwrap_err();
            assert_eq!(e, ErrorKind::BadType("map_restart already called"));
            assert!(map_restart(&mut host, cx, None, &[]).is_err());
            // A refused call changes neither the flag nor the queue.
            assert!(host.save_persist);
            assert_eq!(host.console.len(), 1);
        });

        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            exit_level(&mut host, cx, None, &[Value::Int(0)]).unwrap();
            assert_eq!(host.level_latch, LevelLatch::ExitLevel);
            assert!(!host.save_persist);
            // `ExitLevel` queues the rotation, not a restart (section 2).
            assert_eq!(host.console, vec!["map_rotate".to_string()]);
            let e = map_restart(&mut host, cx, None, &[]).unwrap_err();
            assert_eq!(e, ErrorKind::BadType("exitlevel already called"));
        });
    }

    /// `ExitLevel`'s two passes and its log line (map-cycle doc, section 2):
    /// both team scores and every connected client's score back to 0, and
    /// no scoreboard pushed for it -- the passes write the field rather
    /// than calling the setter, so the ranks stay clean.
    #[test]
    fn exitlevel_zeroes_the_team_scores_and_every_client_score() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn_client(cx, 1, None).unwrap();
            let score = cx.intern_folded("score");
            host.set_field(cx, e, score, Value::Int(12)).unwrap();
            host.team_scores = [4, 7];
            host.ranks_dirty = false;

            exit_level(&mut host, cx, None, &[]).unwrap();

            assert_eq!(host.team_scores, [0, 0]);
            assert_eq!(host.get_field(cx, e, score), Value::Int(0));
            assert!(!host.ranks_dirty, "the outgoing level pushed a scoreboard");
            assert_eq!(
                host.script_log.last().map(String::as_str),
                Some("ExitLevel: executed")
            );
        });
    }

    /// No argument is `savePersist` 0, the same as an explicit zero, and a
    /// non-number is a script error where retail's `Scr_GetInt` raises one.
    #[test]
    fn save_persist_defaults_off_and_refuses_a_non_number() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            map_restart(&mut host, cx, None, &[]).unwrap();
            assert!(!host.save_persist);
        });
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let s = Value::String(cx.intern_exact("1"));
            assert!(exit_level(&mut host, cx, None, &[s]).is_err());
            assert_eq!(host.console.len(), 0);
            // The latch did not take, so the level can still end.
            assert_eq!(host.level_latch, LevelLatch::None);
            exit_level(&mut host, cx, None, &[Value::Int(1)]).unwrap();
            assert_eq!(host.level_latch, LevelLatch::ExitLevel);
            assert!(host.save_persist);
        });
        // `Scr_GetInt` truncates: 0.5 is 0, not "non-zero".
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            map_restart(&mut host, cx, None, &[Value::Float(0.5)]).unwrap();
            assert!(!host.save_persist);
        });
    }

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
                // The probe set "0.3333333333"; this is that rounded to f32.
                Value::Float(0.333_333_34_f32)
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
