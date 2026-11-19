//! CoD4 1.7's own builtins: the names the `war` bootstrap on mp_crash
//! reaches that no shared handler answers under its 1.7 name or semantics
//! (docs/research/cod4-gsc.md 3). CoD4 says dvar where CoD1 says cvar, so
//! the dvar calls take the cvar handlers.

use std::collections::HashMap;
use std::rc::Rc;

use vcod_gsc::{Cx, ErrorKind, Host, Target, Value};

use super::Builtin;
use super::cvar;
use crate::game::host::GameHost;
use vcod_common::net::protocol::CsRange;

pub const NAMES: &[(&str, Builtin)] = &[
    ("getdvar", cvar::get_cvar),
    ("getdvarint", cvar::get_cvar_int),
    ("getdvarfloat", cvar::get_cvar_float),
    ("setdvar", cvar::set_cvar),
    ("makedvarserverinfo", make_dvar_server_info),
    ("tablelookup", table_lookup),
    ("tablelookupistring", table_lookup_istring),
    ("getstat", get_stat),
    ("setstat", set_stat),
    ("visionsetnaked", vision_set_naked),
    ("visionsetnight", vision_set_night),
    ("setexpfog", set_exp_fog),
    ("issplitscreen", is_split_screen),
    ("getarraykeys", get_array_keys),
    ("tolower", to_lower),
    ("strtok", str_tok),
    ("issubstr", is_substr),
    ("getsubstr", get_substr),
    ("abs", abs),
    ("min", min),
    ("max", max),
    ("floor", floor),
    ("ceil", ceil),
    ("int", super::cod2::cast_int),
    ("vectordot", super::cod2::vector_dot),
    ("setgameendtime", set_game_end_time),
    ("isstring", is_string),
    ("randomint", random_int),
    ("randomfloat", random_float),
    ("randomintrange", random_int_range),
    ("randomfloatrange", random_float_range),
    ("assert", developer_statement),
    ("assertex", developer_statement),
    ("assertmsg", developer_statement),
    ("println", developer_statement),
    ("print", developer_statement),
    ("line", developer_statement),
    ("precacherumble", empty),
    ("playrumbleonposition", empty),
    ("precacheitem", precache_item),
    ("hidepart", hide_part),
    ("showpart", show_part),
    ("setcandamage", super::uo::set_take_damage),
    ("setmapcenter", set_map_center),
    ("getnorthyaw", get_north_yaw),
    ("setminimap", set_mini_map),
    ("precachelocationselector", precache_location_selector),
    ("placespawnpoint", place_spawn_point),
    ("logprint", log_print),
    ("getguid", get_guid),
];

/// `logPrint(text, ...)` (`GScr_LogPrint`, 0x80bd1ac): the arguments
/// joined up to 1023 bytes and handed to `G_LogPrintf` as they are, so a
/// line without its `\n` runs into the next (cod4-server-gaps.md,
/// "games_mp.log"). The test-facing `script_log` keeps its lines too.
fn log_print(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let mut text = super::io::render(cx, args);
    if text.len() > 1023 {
        let mut end = 1023;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    super::io::print_line(host, cx, args)?;
    crate::game::cod4::log_printf(host, text);
    Ok(Value::Undefined)
}

/// `self getGuid()`: the GUID the client's `getchallenge` named, the one
/// `status` lists (VERIFIED live: the J lines of `games_mp.log` carry it).
fn get_guid(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = super::client::client_receiver(host, recv)?;
    if !args.is_empty() {
        return Err(ErrorKind::BadType("USAGE: self getGuid()"));
    }
    let guid = host
        .cod4()
        .clients
        .get(slot)
        .map(|c| c.guid.clone())
        .unwrap_or_default();
    Ok(Value::String(cx.intern_exact(&guid)))
}

/// A developer-only statement (`assert*`, `print*`, `line`; the table's
/// developer column, cod4-gsc.md 2): nothing at `developer 0`.
fn developer_statement(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// A builtin whose PC effect is nothing the server keeps (rumble).
fn empty(_: &mut GameHost, _: &mut Cx, _: Option<Target>, _: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

fn number(v: Option<&Value>, what: &'static str) -> Result<f64, ErrorKind> {
    match v {
        Some(Value::Int(i)) => Ok(f64::from(*i)),
        Some(Value::Float(f)) => Ok(f64::from(*f)),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `abs(x)`: a float back.
fn abs(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Float(
        number(a.first(), "abs takes a number")?.abs() as f32
    ))
}

/// `min(a, b)` and `max(a, b)`: floats back.
fn min(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    let (x, y) = (
        number(a.first(), "min takes numbers")?,
        number(a.get(1), "min takes numbers")?,
    );
    Ok(Value::Float(x.min(y) as f32))
}

fn max(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    let (x, y) = (
        number(a.first(), "max takes numbers")?,
        number(a.get(1), "max takes numbers")?,
    );
    Ok(Value::Float(x.max(y) as f32))
}

fn floor(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Float(
        number(a.first(), "floor takes a number")?.floor() as f32,
    ))
}

fn ceil(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Float(
        number(a.first(), "ceil takes a number")?.ceil() as f32,
    ))
}

fn is_string(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(
        matches!(a.first(), Some(Value::String(_))) as i32
    ))
}

/// `randomInt(n)`: `[0, n)` off `GameHost::rng`; `n` under 1 reads 0.
/// 1.7's own generator is unread, so CoD4 keeps the xorshift draw rather
/// than CoD1's `holdrand` (math.rs).
fn random_int(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let n = match a.first() {
        Some(Value::Int(i)) => *i,
        Some(Value::Float(f)) => *f as i32,
        _ => return Err(ErrorKind::BadType("randomInt takes a number")),
    };
    if n <= 0 {
        return Ok(Value::Int(0));
    }
    Ok(Value::Int((host.rand_unit() * n as f32) as i32))
}

/// `randomFloat(f)`: `[0, f)` off `GameHost::rng`, as `random_int`.
fn random_float(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let f = number(a.first(), "randomFloat takes a number")?;
    Ok(Value::Float(host.rand_unit() * f as f32))
}

/// `randomIntRange(lo, hi)`: lo..hi, hi excluded, off the level's draw.
fn random_int_range(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Int(lo)), Some(Value::Int(hi))) = (a.first(), a.get(1)) else {
        return Err(ErrorKind::BadType("randomIntRange takes two ints"));
    };
    let span = (*hi - *lo).max(1);
    Ok(Value::Int(*lo + (host.rand_unit() * span as f32) as i32))
}

/// `randomFloatRange(lo, hi)`.
fn random_float_range(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let lo = number(a.first(), "randomFloatRange takes numbers")?;
    let hi = number(a.get(1), "randomFloatRange takes numbers")?;
    Ok(Value::Float(
        (lo + (hi - lo) * f64::from(host.rand_unit())) as f32,
    ))
}

/// The table, search column, value and result column a `tableLookup`
/// call names, and the row it finds: the first whose search column
/// equals the value, case-blind.
fn lookup(host: &mut GameHost, cx: &Cx, a: &[Value]) -> Result<Option<String>, ErrorKind> {
    let (Some(&Value::String(table)), Some(&col), Some(&value), Some(&out)) =
        (a.first(), a.get(1), a.get(2), a.get(3))
    else {
        return Err(ErrorKind::BadType(
            "tableLookup takes a table, a column, a value and a column",
        ));
    };
    let col = match col {
        Value::Int(i) => i,
        _ => return Err(ErrorKind::BadType("tableLookup's search column is an int")),
    };
    let out = match out {
        Value::Int(i) => i,
        _ => return Err(ErrorKind::BadType("tableLookup's result column is an int")),
    };
    let value = cx.format_number(value).ok_or(ErrorKind::BadType(
        "tableLookup's value is a string or a number",
    ))?;
    let name = cx.resolve(table).to_ascii_lowercase();
    let c4 = host.cod4_mut();
    let Some(t) = c4.string_tables.get(&name).cloned() else {
        return Err(ErrorKind::Custom(format!(
            "string table {name} is not loaded"
        )));
    };
    let cols = t.columns as usize;
    let cell = |row: usize, c: i32| -> Option<&str> {
        let c = usize::try_from(c).ok().filter(|&c| c < cols)?;
        t.values.get(row * cols + c)?.as_deref()
    };
    // A search column off the table matches no row.
    let Some(search) = usize::try_from(col).ok().filter(|&c| c < cols) else {
        return Ok(None);
    };
    // The first row whose cell equals `value` ignoring ASCII case: the
    // walk's answer, through the column's index.
    let key = (name, search);
    let current = c4
        .table_index
        .get(&key)
        .is_some_and(|(at, _)| Rc::ptr_eq(at, &t));
    if !current {
        let mut index = HashMap::new();
        for r in 0..t.rows as usize {
            if let Some(v) = cell(r, col) {
                index.entry(v.to_ascii_lowercase()).or_insert(r);
            }
        }
        c4.table_index.insert(key.clone(), (t.clone(), index));
    }
    Ok(c4.table_index[&key]
        .1
        .get(&value.to_ascii_lowercase())
        .map(|&r| cell(r, out).unwrap_or("").to_string()))
}

/// `tableLookup(table, searchColumn, value, resultColumn)`: the found row's
/// cell, `""` when no row matches.
fn table_lookup(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let found = lookup(host, cx, a)?.unwrap_or_default();
    Ok(Value::String(cx.intern_exact(&found)))
}

/// `tableLookupIString`: the cell as a localized string.
fn table_lookup_istring(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let found = lookup(host, cx, a)?.unwrap_or_default();
    Ok(Value::Localized(cx.intern_exact(&found)))
}

/// `self getStat(index)` (`GScr_GetStat` 0x80c55f6): the slot's stats
/// block, the one the client sent in its seven `stats` packets (a fresh
/// profile's until it is in, as a test client's), as the script left it.
/// A receiver with no client, an index past 3499 unsigned and a slot no
/// longer connected (`sess.connected`, client +0x2f8c, at most 0) are
/// script errors, in that order; 3498 and 3499 read 0
/// (`SV_GetClientStat` 0x816de58; cod4-gsc.md, "The stats, the loadout,
/// the perks").
fn get_stat(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let Some(&Value::Int(i)) = a.first() else {
        return Err(ErrorKind::BadType("getStat takes an int"));
    };
    let slot = stat_player(host, id, "getstat")?;
    if !(0..=3499).contains(&i) {
        return Err(ErrorKind::Custom(format!("getstat: invalid index {i}")));
    }
    if host.client_roster[slot].is_none() {
        return Err(ErrorKind::Custom(
            "getstat: called on a disconnected player".into(),
        ));
    }
    Ok(Value::Int(
        host.cod4().stats[slot].get(i as u16).unwrap_or(0) as i32,
    ))
}

/// The slot of `getStat`'s or `setStat`'s receiver, or `<name>: entity
/// must be a player entity` (0x821f090, 0x821ee1c).
fn stat_player(host: &GameHost, id: vcod_gsc::EntId, name: &str) -> Result<usize, ErrorKind> {
    match host.ents.get(id) {
        Some(e) if e.client.is_some() => Ok(id.0 as usize),
        _ => Err(ErrorKind::Custom(format!(
            "{name}: entity must be a player entity"
        ))),
    }
}

/// `self setStat(index, value)` (`GScr_SetStat` 0x80c5490): a receiver
/// with no client, then `developer_script` on (`setstat: developer_script
/// is false.`, the dvar's bool read at 0x80c54e4), then a call without two
/// arguments, an index past 3499 or a byte stat (under 2000) set past 255
/// is a script error; otherwise `SV_SetClientStat` (0x816e712) writes a
/// changed stat into the slot's block and sends the client the reliable
/// `N <index> <value>` (`%c %i %i` at 0x8217f78 with 0x4e), which its
/// profile takes. The `impure scripts loaded.` refusal ahead of the dvar's
/// is never met: its flag needs a loose script read with `useFastFile` 0
/// (cod4-gsc.md, "The stats, the loadout, the perks").
fn set_stat(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let slot = stat_player(host, id, "setstat")?;
    if host
        .cvars
        .get("developer_script")
        .trim()
        .parse::<i32>()
        .is_ok_and(|v| v != 0)
    {
        return Err(ErrorKind::Custom(
            "setstat: developer_script is false.".into(),
        ));
    }
    if a.len() != 2 {
        return Err(ErrorKind::Custom(format!(
            "setstat: takes 2 arguments, got {}.",
            a.len()
        )));
    }
    let (Some(&Value::Int(i)), Some(&Value::Int(v))) = (a.first(), a.get(1)) else {
        return Err(ErrorKind::BadType("setStat takes two ints"));
    };
    if !(0..=3499).contains(&i) {
        return Err(ErrorKind::Custom(format!("setstat: invalid index {i}")));
    }
    if i < 2000 && !(0..=255).contains(&v) {
        return Err(ErrorKind::Custom(format!(
            "setstat: index {i} is a byte value, and you're trying to set it to {v}"
        )));
    }
    let stats = &mut host.cod4_mut().stats[slot];
    // 3498 and 3499 pass the builtin's check and `SV_SetClientStat` drops.
    if stats.get(i as u16).is_some_and(|old| old != v as u32) {
        stats.set(i as u16, v as u32);
        host.client_commands.push((slot, format!("N {i} {v}")));
    }
    Ok(Value::Undefined)
}

/// `visionSetNaked(name, transitionTime)` and `visionSetNight`: their
/// configstring (824, 825) at `"<name>" <ms>` (`cod4_lnxded`'s `"%s" %i`,
/// 0x82193a8), the transition in ms. The capture's `"default_night" 1000`
/// for `_load.gsc`'s one-argument `VisionSetNight("default_night")` says a
/// missing time is a second.
fn vision_set(
    host: &mut GameHost,
    cx: &mut Cx,
    a: &[Value],
    slot: usize,
    usage: &'static str,
) -> Result<Value, ErrorKind> {
    let Some(&Value::String(name)) = a.first() else {
        return Err(ErrorKind::BadType(usage));
    };
    let secs = match a.get(1) {
        None => 1.0,
        v => number(v, usage)?,
    };
    let text = format!("\"{}\" {}", cx.resolve(name), (secs * 1000.0) as i32);
    host.configstrings.set(slot, &text);
    Ok(Value::Undefined)
}

fn vision_set_naked(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    vision_set(
        host,
        cx,
        a,
        crate::game::cod4::CS_VISIONSET_NAKED,
        "USAGE: VisionSetNaked( <visionset name>, <transition time> )",
    )
}

fn vision_set_night(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    vision_set(
        host,
        cx,
        a,
        crate::game::cod4::CS_VISIONSET_NIGHT,
        "USAGE: VisionSetNight( <visionset name>, <transition time> )",
    )
}

/// `setExpFog(startDist, halfwayDist, r, g, b, transitionTime)`: the fog
/// slot (9) through `"%g %g %g %g %g %.0f"` (0x821c072): the start, the
/// density `ln 2 / halfwayDist`, the colour and the time in ms. The
/// capture's `500 0.000198042 ...` for mp_crash's `setExpFog(500, 3500,
/// ...)` is `ln 2 / 3500`. The two distance checks are the binary's
/// messages (0x821da24, 0x821da58).
fn set_exp_fog(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str =
        "USAGE: setExpFog(<startDist>, <halfwayDist>, <red>, <green>, <blue>, <transition time>)";
    if a.len() != 6 {
        return Err(ErrorKind::BadType(USAGE));
    }
    let v: Vec<f32> = a
        .iter()
        .map(|x| number(Some(x), USAGE).map(|f| f as f32))
        .collect::<Result<_, _>>()?;
    if v[0] < 0.0 {
        return Err(ErrorKind::BadType(
            "setExpFog: startDist must be greater or equal to 0",
        ));
    }
    if v[1] <= 0.0 {
        return Err(ErrorKind::BadType(
            "setExpFog: halfwayDist must be greater than 0",
        ));
    }
    let density = std::f32::consts::LN_2 / v[1];
    let g = vcod_gsc::format_g;
    let text = format!(
        "{} {} {} {} {} {:.0}",
        g(v[0]),
        g(density),
        g(v[2]),
        g(v[3]),
        g(v[4]),
        v[5] * 1000.0
    );
    let slot = host.allocators.layout().fog;
    host.configstrings.set(slot, &text);
    let _ = cx;
    Ok(Value::Undefined)
}

/// `isSplitScreen()`: 0 on a dedicated server.
fn is_split_screen(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Int(0))
}

/// `getArrayKeys(array)`: a new array of its keys, the most recently
/// added first: `_globallogic.gsc`'s precache of `game["strings"]` over
/// them lands in the capture's localized slots in the reverse of the
/// keys' first assignments (cod4-gsc.md 3).
fn get_array_keys(
    _: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(&Value::Array(arr)) = a.first() else {
        return Err(ErrorKind::BadType("getArrayKeys takes an array"));
    };
    let keys = cx.array_keys_newest_first(arr);
    let out = cx.new_array();
    for (i, k) in keys.into_iter().enumerate() {
        let v = match k {
            vcod_gsc::ArrayKey::Int(n) => Value::Int(n),
            vcod_gsc::ArrayKey::Str(s) => Value::String(s),
        };
        cx.set_index(out, vcod_gsc::ArrayKey::Int(i as i32), v);
    }
    Ok(Value::Array(out))
}

fn string_arg<'a>(cx: &'a Cx, v: Option<&Value>, what: &'static str) -> Result<&'a str, ErrorKind> {
    match v {
        Some(&Value::String(s)) => Ok(cx.resolve(s)),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `toLower(s)`: ASCII lowercase.
fn to_lower(
    _: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = string_arg(cx, a.first(), "toLower takes a string")?.to_ascii_lowercase();
    Ok(Value::String(cx.intern_exact(&s)))
}

/// `strTok(s, delimiters)`: the runs between any of the delimiter
/// characters, empty runs dropped.
fn str_tok(
    _: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = string_arg(cx, a.first(), "strTok takes two strings")?.to_string();
    let d = string_arg(cx, a.get(1), "strTok takes two strings")?.to_string();
    let out = cx.new_array();
    for (i, t) in s
        .split(|c| d.contains(c))
        .filter(|t| !t.is_empty())
        .enumerate()
    {
        let v = Value::String(cx.intern_exact(t));
        cx.set_index(out, vcod_gsc::ArrayKey::Int(i as i32), v);
    }
    Ok(Value::Array(out))
}

/// `isSubStr(s, sub)`.
fn is_substr(
    _: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = string_arg(cx, a.first(), "isSubStr takes two strings")?;
    let sub = string_arg(cx, a.get(1), "isSubStr takes two strings")?;
    Ok(Value::Int(s.contains(sub) as i32))
}

/// `getSubStr(s, start[, end])`: bytes `start..end`, clamped to the string.
fn get_substr(
    _: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let s = string_arg(cx, a.first(), "getSubStr takes a string")?
        .as_bytes()
        .to_vec();
    let idx = |v: Option<&Value>, default: usize| -> Result<usize, ErrorKind> {
        match v {
            None => Ok(default),
            Some(&Value::Int(i)) => Ok((i.max(0) as usize).min(s.len())),
            _ => Err(ErrorKind::BadType("getSubStr takes int bounds")),
        }
    };
    let start = idx(a.get(1), 0)?;
    let end = idx(a.get(2), s.len())?.max(start);
    let text = String::from_utf8_lossy(&s[start..end]).into_owned();
    Ok(Value::String(cx.intern_exact(&text)))
}

/// Adds `name` to the level's weapon list if it is new and writes the list
/// to [`crate::game::cod4::CS_WEAPONFILES`], space-separated: the capture's
/// list is `defaultweapon_mp` then the bootstrap's `precacheItem` order.
/// Its world and projectile models are indexed; its effects are not yet.
pub fn register_weapon(host: &mut GameHost, name: &str) -> Result<usize, ErrorKind> {
    let c4 = host.cod4_mut();
    if let Some(i) = c4.weapons.iter().position(|w| w == name) {
        return Ok(i + 1);
    }
    // The held bits and the 128-slot ammo arrays end at weapon 127; past
    // it vcod refuses with its own error (retail's limit is not recorded).
    if c4.weapons.len() + 1 >= crate::game::cod4::MAX_WEAPONS {
        return Err(ErrorKind::Custom(format!(
            "too many weapons registered: {name} would be weapon {}, the limit is {}",
            c4.weapons.len() + 1,
            crate::game::cod4::MAX_WEAPONS - 1
        )));
    }
    c4.weapons.push(name.to_string());
    let index = c4.weapons.len();
    let def = c4
        .weapon_files
        .get(name)
        .map(|f| f.def.clone())
        .unwrap_or_else(|| crate::game::cod4::weapons::WeaponDef4 {
            name: name.to_string(),
            ..Default::default()
        });
    let n = c4.table.register(def);
    debug_assert_eq!(n, index);
    let file = c4.weapon_files.get(name).cloned();
    // `G_RegisterWeapon`: the use hint's and the drop hint's
    // slots (mp_backlot's SAW takes 277 and 278, VERIFIED live,
    // cod4-gsc.md 9.2), each world model, then the projectile's.
    for hint in file
        .iter()
        .flat_map(|f| [&f.use_hint, &f.drop_hint])
        .flatten()
    {
        let layout = host.allocators.layout();
        if crate::configstrings::hint_string_alloc(&mut host.configstrings, layout, hint).is_none()
        {
            return Err(ErrorKind::Custom(
                "Too many different hintstring values on weapons. Max allowed is 32 different strings"
                    .into(),
            ));
        }
    }
    for m in file.iter().flat_map(|f| f.models.iter()) {
        host.allocators
            .index(&mut host.configstrings, CsRange::Model, m)?;
    }
    let list = host.cod4().weapons.join(" ");
    host.configstrings
        .set(crate::game::cod4::CS_WEAPONFILES, &list);
    host.configstrings
        .set(crate::game::cod4::CS_ITEMS, registered_bits(index));
    // Its alt weapon follows it (the capture's `g36c_gl_mp gl_g36c_mp`).
    if let Some(alt) = file.as_ref().and_then(|f| f.alt.as_deref()) {
        register_weapon(host, alt)?;
    }
    Ok(index)
}

/// Configstring 2314: 128 weapon bits, four to a lowercase hex digit, bit
/// `i & 3` of digit `i / 4`, weapon 0 and weapons 1..=`count` set. The
/// capture's 113 weapons read `ffffffffffffffffffffffffffff3000`.
fn registered_bits(count: usize) -> String {
    (0..crate::game::cod4::MAX_WEAPONS / 4)
        .map(|d| {
            let nibble = (0..4)
                .filter(|b| d * 4 + b <= count)
                .fold(0u32, |n, b| n | 1 << b);
            char::from_digit(nibble, 16).unwrap()
        })
        .collect()
}

/// A map's `weapon_<name>` pickup: the spawn pass registers the weapon in
/// lump order, ahead of the scripts' precaches (mp_crash's eight pickups
/// open the capture's weapon list).
pub fn register_weapon_pickup(host: &mut GameHost, name: &str) -> Result<(), ErrorKind> {
    if host.cod4().weapon_files.contains_key(name) {
        let index = register_weapon(host, name)?;
        pickup_ammo_draws(host, index);
    }
    Ok(())
}

/// `G_SpawnItem`'s ammo for a weapon pickup (0x8090072): `rand() % (max -
/// min + 1) + min` over the def's `dropAmmoMin..Max` (the pair ordered,
/// none when the larger is negative), then, with a reserve and a clip of
/// more than one, `rand() % (clip + 1)`; the alt weapon the same when it
/// shares no ammo cap. The script deletes the stock maps' pickups before
/// the baselines, so only the draws are kept: mp_crash's eight make 17
/// (VERIFIED, a traced run, cod4-combat.md 10.9).
fn pickup_ammo_draws(host: &mut GameHost, index: usize) {
    let draws = |d: &crate::game::cod4::weapons::WeaponDef4| {
        let (lo, hi) = (
            d.drop_ammo_min.min(d.drop_ammo_max),
            d.drop_ammo_min.max(d.drop_ammo_max),
        );
        (hi >= 0).then_some((lo, hi - lo + 1, d.clip_size))
    };
    let table = &host.cod4().table;
    let Some(d) = table.get(index) else {
        return;
    };
    let alt = table
        .get(d.alt)
        .filter(|a| d.alt > 0 && a.shared_ammo_cap.is_none());
    let runs = [draws(d), alt.and_then(draws)];
    let c4 = host.cod4_mut();
    for (lo, span, clip) in runs.into_iter().flatten() {
        let stock = lo + c4.rand() % span;
        if stock > 0 && clip != 1 {
            c4.rand();
        }
    }
}

/// `precacheItem(weapon)`: a weapon file the zones carry, registered
/// ([`register_weapon`]).
fn precache_item(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = string_arg(cx, a.first(), "precacheItem takes a weapon name")?.to_ascii_lowercase();
    if !host.initializing {
        return Err(ErrorKind::Custom(
            "precacheItem must be called before any wait statements in the gametype or level script".into(),
        ));
    }
    if !host.cod4().weapon_files.contains_key(&name) {
        return Err(ErrorKind::Custom(format!("unknown weapon '{name}'")));
    }
    register_weapon(host, &name)?;
    Ok(Value::Undefined)
}

/// `self hidePart(tag[, model])` and `showPart`: the bone's bit in the
/// entity's `s.partBits`, `0x80000000 >> (i % 32)` of word `i / 32` for
/// the model's `i`th bone: the destructible cars' ten hidden parts read
/// the walk capture's 0x242a, 0x92480000 that way (VERIFIED live,
/// `cod4::wire_ab`). A tag the model lacks changes nothing.
fn part(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
    hide: bool,
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let Some(&Value::String(tag)) = a.first() else {
        return Err(ErrorKind::BadType("hidePart takes a tag name"));
    };
    let tag = cx.resolve(tag).to_ascii_lowercase();
    let model = match a.get(1) {
        Some(&Value::String(m)) => cx.resolve(m).to_string(),
        _ => {
            let f = cx.intern_folded("model");
            match vcod_gsc::Host::get_field(host, cx, id, f) {
                Value::String(m) => cx.resolve(m).to_string(),
                _ => String::new(),
            }
        }
    };
    let c4 = host.cod4_mut();
    let Some(bones) = c4.model_bones.get(&model.to_ascii_lowercase()).cloned() else {
        return Ok(Value::Undefined);
    };
    let Some(i) = bones.iter().position(|b| b.eq_ignore_ascii_case(&tag)) else {
        return Ok(Value::Undefined);
    };
    if i >= 128 {
        return Ok(Value::Undefined);
    }
    let entry = c4.part_bits.entry(id.0).or_insert((id, [0; 4]));
    if entry.0 != id {
        *entry = (id, [0; 4]);
    }
    let bit = 0x8000_0000u32 >> (i % 32);
    if hide {
        entry.1[i / 32] |= bit;
    } else {
        entry.1[i / 32] &= !bit;
    }
    Ok(Value::Undefined)
}

fn hide_part(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    part(host, cx, recv, a, true)
}

fn show_part(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    part(host, cx, recv, a, false)
}

/// `makeDvarServerInfo(name[, default])`: the default is optional on 1.7
/// (`_globallogic.gsc` calls it with the name alone).
fn make_dvar_server_info(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = string_arg(cx, a.first(), "makeDvarServerInfo takes a dvar name")?.to_string();
    let default = match a.get(1) {
        None => String::new(),
        Some(&v) => cx.format_number(v).ok_or(ErrorKind::BadType(
            "makeDvarServerInfo takes a default value",
        ))?,
    };
    host.cvars.make_server_info(&name, &default);
    Ok(Value::Undefined)
}

fn vector_arg(v: Option<&Value>, what: &'static str) -> Result<[f32; 3], ErrorKind> {
    match v {
        Some(&Value::Vector(v)) => Ok(v),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// `setMapCenter(origin)`: configstring 12 at `"%f %f %f"` (the capture's
/// `429.000000 126.500000 137.000000`).
fn set_map_center(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let v = vector_arg(a.first(), "setMapCenter takes a vector")?;
    host.configstrings.set(
        crate::game::cod4::CS_MAPCENTER,
        format!("{:.6} {:.6} {:.6}", v[0], v[1], v[2]),
    );
    Ok(Value::Undefined)
}

/// `getNorthYaw()`: the worldspawn's `northyaw` as the spawn pass wrote it
/// to configstring 822, read as a float.
fn get_north_yaw(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let text = host.configstrings[crate::game::cod4::CS_NORTHYAW].as_str();
    Ok(Value::Float(cvar::atof(text)))
}

/// `setMiniMap(material, upperLeftX, upperLeftY, lowerRightX, lowerRightY)`:
/// configstring 823 at `"%s" %f %f %f %f` (0x821c3e6), the material
/// precached as a server material first.
fn set_mini_map(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = string_arg(
        cx,
        a.first(),
        "setMiniMap takes a material and four numbers",
    )?
    .to_string();
    let mut v = [0.0f32; 4];
    for (i, x) in v.iter_mut().enumerate() {
        *x = number(a.get(i + 1), "setMiniMap takes a material and four numbers")? as f32;
    }
    host.configstrings.set(
        crate::game::cod4::CS_MINIMAP,
        format!("\"{name}\" {:.6} {:.6} {:.6} {:.6}", v[0], v[1], v[2], v[3]),
    );
    Ok(Value::Undefined)
}

/// `precacheLocationSelector(material)`: the next of the three slots from
/// configstring 827 (the capture's `map_artillery_selector`); a fourth name
/// is a script error (0x821d718), a second precache of one name only a
/// developer print (0x80bc5f2), which every `fast_restart` round hits as
/// the kept configstrings still hold it (cod4-gsc.md 4).
fn precache_location_selector(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let name = string_arg(cx, a.first(), "precacheLocationSelector takes a material")?.to_string();
    let base = crate::game::cod4::CS_LOC_SEL_MTLS;
    let mut free = None;
    for slot in base..base + 3 {
        let have = host.configstrings[slot].clone();
        if have.eq_ignore_ascii_case(&name) {
            log::debug!("Script tried to precache the location selector '{name}' more than once");
            return Ok(Value::Undefined);
        }
        if have.is_empty() && free.is_none() {
            free = Some(slot);
        }
    }
    let slot = free.ok_or_else(|| {
        ErrorKind::Custom("Too many location selectors precached. Max allowed is 3".into())
    })?;
    host.configstrings.set(slot, name);
    Ok(Value::Undefined)
}

/// `setGameEndTime(ms)`: configstring 11 at the level time the match ends
/// (the capture's `619800`, a 10 minute limit).
fn set_game_end_time(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(&Value::Int(ms)) = a.first() else {
        return Err(ErrorKind::BadType("setGameEndTime takes an int"));
    };
    host.configstrings
        .set(crate::game::cod4::CS_GAMEENDTIME, ms.to_string());
    Ok(Value::Undefined)
}

/// `self placeSpawnPoint()` (1.7's 0x80c56c6, cod4-collision.md 5): the
/// player's capsule (-15,-15,0)..(15,15,70) swept 128 up under
/// `MASK_PLAYERSOLID`, then 262144 down from where it stopped, the entity
/// set where the second stopped, with a warning when the capsule is all
/// solid there. Each stop is `Vec3Lerp(start, end,
/// fraction)` rounded once (INFERRED: CoD2's kept arithmetic); the shared
/// `placeSpawnPoint`'s shifted Q3 end point lands the walk capture's spawn
/// 2 bits under retail's 224.25427 (VERIFIED, `cod4::wire_ab`). A level with
/// no clip map leaves the origin alone.
fn place_spawn_point(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let origin_field = cx.intern_folded("origin");
    let Value::Vector(origin) = host.get_field(cx, id, origin_field) else {
        return Err(ErrorKind::BadType("a spawnpoint needs an origin"));
    };
    let Some(clip) = host.cod4().clip.clone() else {
        return Ok(Value::Undefined);
    };
    let (placed, solid) = placed_spawn(&clip, glam::Vec3::from(origin));
    if solid {
        // 1.7's line names the spot before the drop, truncated
        // (cod4-collision.md 5).
        let [x, y, z] = origin.map(|c| c as i32);
        log::warn!(
            "WARNING: Spawn point entity {} is in solid at ({x}, {y}, {z})",
            id.0
        );
    }
    host.set_field(cx, id, origin_field, Value::Vector(placed.to_array()))?;
    Ok(Value::Undefined)
}

/// Where [`place_spawn_point`] puts a spawnpoint at `origin`, and whether
/// the capsule starts solid there.
pub fn placed_spawn(
    clip: &vcod_cod4::collision::ClipMap,
    origin: glam::Vec3,
) -> (glam::Vec3, bool) {
    use vcod_cod4::pmove::{MASK_PLAYERSOLID, PLAYER_MAXS, PLAYER_MINS};
    let lerp = |a: glam::Vec3, b: glam::Vec3, f: f32| {
        glam::Vec3::from(std::array::from_fn::<f32, 3, _>(|i| {
            (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * f64::from(f)) as f32
        }))
    };
    let trace = |a, b| clip.trace(a, b, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    let up = origin + glam::Vec3::new(0.0, 0.0, 128.0);
    let start = lerp(origin, up, trace(origin, up).fraction);
    let down = start - glam::Vec3::new(0.0, 0.0, 262_144.0);
    let placed = lerp(start, down, trace(start, down).fraction);
    (placed, trace(placed, placed).startsolid)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A CoD4 host with no zones, and a player in slot 0.
    pub(crate) fn cod4_host(cx: &mut Cx) -> (GameHost, Option<Target>) {
        let mut host = GameHost::new(
            &crate::game::profile::COD4,
            vec![String::new(); crate::game::cod4::MAX_CONFIGSTRINGS],
        );
        host.connect_client(cx, 0, "a").unwrap();
        host.client_roster[0] = Some(Default::default());
        let me = Some(Target::Entity(host.ents.handle(0).unwrap()));
        (host, me)
    }

    fn s(cx: &mut Cx, t: &str) -> Value {
        Value::String(cx.intern_exact(t))
    }

    fn text(cx: &Cx, v: Value) -> String {
        match v {
            Value::String(a) | Value::Localized(a) => cx.resolve(a).to_string(),
            v => panic!("{v:?} is no string"),
        }
    }

    /// `getSubStr`'s byte bounds clamp into the string and an end before
    /// the start is empty; `strTok` drops the empty runs between any of
    /// its delimiters.
    #[test]
    fn substrings_and_tokens_slice_by_bytes_and_delimiters() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, _) = cod4_host(cx);
            let abc = s(cx, "abcdef");
            let mut sub = |a: &[Value]| get_substr(&mut host, cx, None, a).map(|v| text(cx, v));
            let i = Value::Int;
            assert_eq!(sub(&[abc, i(1), i(4)]).unwrap(), "bcd");
            assert_eq!(sub(&[abc, i(4)]).unwrap(), "ef");
            assert_eq!(sub(&[abc, i(-2), i(3)]).unwrap(), "abc");
            assert_eq!(sub(&[abc, i(3), i(100)]).unwrap(), "def");
            assert_eq!(sub(&[abc, i(4), i(2)]).unwrap(), "");
            assert!(sub(&[abc, Value::Float(1.0)]).is_err());

            let (line, delims) = (s(cx, ",a,,b c,"), s(cx, ", "));
            let Value::Array(arr) = str_tok(&mut host, cx, None, &[line, delims]).unwrap() else {
                panic!("strTok gave no array");
            };
            let toks: Vec<String> = (0..cx.array_len(arr) as i32)
                .map(|k| text(cx, cx.get_index(arr, vcod_gsc::ArrayKey::Int(k))))
                .collect();
            assert_eq!(toks, ["a", "b", "c"]);
        });
    }

    fn table(rows: &[[&str; 3]]) -> Rc<vcod_cod4::assets::misc::StringTable> {
        Rc::new(vcod_cod4::assets::misc::StringTable {
            name: "mp/t.csv".into(),
            columns: 3,
            rows: rows.len() as u32,
            values: rows.iter().flatten().map(|c| Some(c.to_string())).collect(),
        })
    }

    /// `tableLookup`: the first row whose search column matches case-blind,
    /// a number value matched as its text, `""` for no row or a column off
    /// the table, an error for a table not loaded; and a table loaded anew
    /// under the same name rebuilds the column's index.
    #[test]
    fn table_lookup_finds_the_first_row_and_follows_a_reloaded_table() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, _) = cod4_host(cx);
            let t = table(&[["1", "M16", "5"], ["2", "ak47", "6"], ["2", "dup", "7"]]);
            host.cod4_mut().string_tables.insert("mp/t.csv".into(), t);
            let name = s(cx, "MP/T.csv");
            let i = Value::Int;
            let (m16, gone) = (s(cx, "m16"), s(cx, "mp/none.csv"));
            let mut look = |col: i32, v: Value, out: i32| {
                table_lookup(&mut host, cx, None, &[name, i(col), v, i(out)]).map(|v| text(cx, v))
            };
            assert_eq!(look(1, m16, 0).unwrap(), "1");
            assert_eq!(look(0, i(2), 1).unwrap(), "ak47");
            assert_eq!(look(0, i(2), 3).unwrap(), "");
            assert_eq!(look(3, i(2), 1).unwrap(), "");
            assert_eq!(look(0, i(9), 1).unwrap(), "");
            assert!(table_lookup(&mut host, cx, None, &[gone, i(0), i(1), i(1)]).is_err());

            let t = table(&[["2", "g3", "8"]]);
            host.cod4_mut().string_tables.insert("mp/t.csv".into(), t);
            let found = table_lookup(&mut host, cx, None, &[name, i(0), i(2), i(1)]).unwrap();
            assert_eq!(text(cx, found), "g3");
            let loc = table_lookup_istring(&mut host, cx, None, &[name, i(0), i(2), i(2)]);
            assert!(matches!(loc, Ok(Value::Localized(_))));
        });
    }

    /// `setStat` writes a changed stat and tells the client `N <i> <v>`; a
    /// byte stat past 255, an index past 3499, a third argument and
    /// `developer_script` on are errors; 3498 and 3499 read 0 and keep
    /// nothing. `getStat` refuses a slot no longer connected.
    #[test]
    fn stats_keep_their_bounds_and_tell_the_client() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, me) = cod4_host(cx);
            let i = Value::Int;
            let mut set = |host: &mut GameHost, a: &[Value]| set_stat(host, cx, me, a);
            assert_eq!(set(&mut host, &[i(5), i(200)]), Ok(Value::Undefined));
            assert_eq!(set(&mut host, &[i(5), i(200)]), Ok(Value::Undefined));
            assert_eq!(set(&mut host, &[i(2500), i(100_000)]), Ok(Value::Undefined));
            assert_eq!(set(&mut host, &[i(3498), i(7)]), Ok(Value::Undefined));
            assert!(set(&mut host, &[i(5), i(256)]).is_err());
            assert!(set(&mut host, &[i(1999), i(-1)]).is_err());
            assert!(set(&mut host, &[i(3500), i(1)]).is_err());
            assert!(set(&mut host, &[i(5), i(1), i(1)]).is_err());
            let sent: Vec<&str> = host.client_commands.iter().map(|c| c.1.as_str()).collect();
            assert_eq!(sent, ["N 5 200", "N 2500 100000"]);
            let mut get = |host: &mut GameHost, n: i32| get_stat(host, cx, me, &[i(n)]);
            assert_eq!(get(&mut host, 5), Ok(i(200)));
            assert_eq!(get(&mut host, 2500), Ok(i(100_000)));
            assert_eq!(get(&mut host, 3498), Ok(i(0)));
            assert!(get(&mut host, -1).is_err());
            assert!(get(&mut host, 3500).is_err());
            host.client_roster[0] = None;
            assert!(get(&mut host, 5).is_err());
            host.cvars.set("developer_script", "1");
            assert!(set_stat(&mut host, cx, me, &[i(6), i(1)]).is_err());
        });
    }

    /// Configstring 2314's weapon bits: the capture's 113 weapons, and
    /// weapon 0 alone.
    #[test]
    fn the_registered_bits_read_as_the_capture() {
        assert_eq!(registered_bits(113), "ffffffffffffffffffffffffffff3000");
        assert_eq!(registered_bits(0), format!("1{}", "0".repeat(31)));
        assert_eq!(registered_bits(127), "f".repeat(32));
    }

    /// `setExpFog`'s density is `ln 2` over the halfway distance (the
    /// capture's `0.000198042` for 3500) and its time is in ms;
    /// `visionSetNaked` writes `"<name>" <ms>`, a missing time a second.
    #[test]
    fn fog_and_vision_sets_write_the_capture_formats() {
        let mut vm = vcod_gsc::Vm::new();
        vm.with_cx(|cx| {
            let (mut host, _) = cod4_host(cx);
            let f = Value::Float;
            let fog = [f(500.0), f(3500.0), f(0.5), f(0.25), f(0.75), f(2.0)];
            set_exp_fog(&mut host, cx, None, &fog).unwrap();
            let slot = host.allocators.layout().fog;
            assert_eq!(
                host.configstrings[slot],
                "500 0.000198042 0.5 0.25 0.75 2000"
            );
            let mut bad = fog;
            bad[1] = f(0.0);
            assert!(set_exp_fog(&mut host, cx, None, &bad).is_err());
            bad = fog;
            bad[0] = f(-1.0);
            assert!(set_exp_fog(&mut host, cx, None, &bad).is_err());
            assert!(set_exp_fog(&mut host, cx, None, &fog[..5]).is_err());

            let crash = s(cx, "mp_crash");
            vision_set_naked(&mut host, cx, None, &[crash, f(2.5)]).unwrap();
            let night = s(cx, "default_night");
            vision_set_night(&mut host, cx, None, &[night]).unwrap();
            use crate::game::cod4::{CS_VISIONSET_NAKED, CS_VISIONSET_NIGHT};
            assert_eq!(host.configstrings[CS_VISIONSET_NAKED], "\"mp_crash\" 2500");
            assert_eq!(
                host.configstrings[CS_VISIONSET_NIGHT],
                "\"default_night\" 1000"
            );
        });
    }

    /// Weapon 127 is the last the held bits and the 128-slot ammo arrays
    /// hold: the 128th registration is a script error, and 127 can be
    /// given and counted.
    #[test]
    fn a_cod4_level_registers_at_most_127_weapons() {
        let mut host = GameHost::new(
            &crate::game::profile::COD4,
            vec![String::new(); crate::game::cod4::MAX_CONFIGSTRINGS],
        );
        let max = crate::game::cod4::MAX_WEAPONS - 1;
        for i in 1..=max {
            assert_eq!(register_weapon(&mut host, &format!("w{i}_mp")), Ok(i));
        }
        assert!(matches!(
            register_weapon(&mut host, "one_too_many_mp"),
            Err(ErrorKind::Custom(_))
        ));
        assert_eq!(register_weapon(&mut host, "w7_mp"), Ok(7));
        let c4 = host.cod4();
        let d = c4.table.get(max).unwrap();
        assert!(d.ammo_slot < 128 && d.clip_slot < 128);
        let mut l = crate::game::cod4::weapons::Loadout::default();
        assert!(l.give(&c4.table, max, 0).is_some());
        assert!(l.has(max));
    }
}
