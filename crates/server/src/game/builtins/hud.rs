//! HUD element builtins: the three allocators and the methods that write the
//! half of a `g_hudelems` record no script field reaches
//! (`crate::game::entity::HudState`). Which client is sent which element is
//! `crate::game::wire::hud_elems`; the wire side is
//! docs/research/cod11-hud-protocol.md section 5.
//!
//! A HUD element is a script object with its own field table
//! (docs/research/cod11-gsc-object-model.md sections 3 and 9), so field
//! access on the value `newHudElem` returns already works: `host.rs` routes
//! by entity number and `ObjectTable` keeps the HUD range in its own vector.
//!
//! Still missing from the method table: `setClock`/`setClockUp`, the three
//! `*OverTime` animators and `reset`. No stock gametype's bootstrap calls
//! one.

use crate::game::builtins::entity::entity_receiver;
use crate::game::entity::{HudState, FIRST_HUD_ELEM};
use crate::game::host::GameHost;
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS, TEAM_SPECTATOR};
use vcod_gsc::{Cx, EntId, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("newhudelem", new_hud_elem),
    ("newclienthudelem", new_client_hud_elem),
    ("newteamhudelem", new_team_hud_elem),
    ("settimer", set_timer),
    ("settimerup", set_timer_up),
    ("settenthstimer", set_tenths_timer),
    ("settenthstimerup", set_tenths_timer_up),
    ("settext", set_text),
    ("setvalue", set_value),
    ("setshader", set_shader),
    ("destroy", destroy),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The `hudelem_t` type codes: the tag on the union its payload fields make
/// up, each the immediate its method stores at `+0x0`. 0 is a free record,
/// which is why a fresh element is `TEXT` and not 0.
mod elem_type {
    pub const TEXT: i32 = 1;
    pub const VALUE: i32 = 2;
    pub const SHADER: i32 = 3;
    pub const TIMER_DOWN: i32 = 4;
    pub const TIMER_UP: i32 = 5;
    pub const TENTHS_DOWN: i32 = 6;
    pub const TENTHS_UP: i32 = 7;
}

/// The receiver a hudelem method carries. `HudElem_GetMethod` (0x4be38) is
/// only consulted for a HUD element, so a gentity receiver here is the type
/// error retail's own dispatch would raise by never finding the name.
fn hud_receiver(host: &GameHost, recv: Option<Target>) -> Result<EntId, ErrorKind> {
    let id = entity_receiver(recv)?;
    if id.0 < FIRST_HUD_ELEM || host.ents.get(id).is_none() {
        return Err(ErrorKind::BadType("needs a HUD element receiver"));
    }
    Ok(id)
}

/// `newHudElem()` (`game.mp.i386.so` 0x4b184, `functions[77]`): the first
/// free `g_hudelems` record, owned by no client (retail writes `0x3ff`,
/// `ENTITYNUM_NONE`, into the record's owner field at 0x4b249), so every
/// client is sent it. Faithful, down to failing rather than returning when
/// the pool is full; `ObjectTable::spawn_hud_elem` carries the defaults.
pub fn new_hud_elem(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Entity(host.ents.spawn_hud_elem(cx)?))
}

/// `newClientHudElem(player)` (`GScr_NewClientHudElem` 0x4b298): the same
/// record `newHudElem` takes, except that the owner field at +0x70 gets the
/// player's entity number instead of `0x3ff`, so the element is drawn for
/// that client alone. Retail's `not a client` param error (0x746f1) is the
/// receiver check here.
pub fn new_client_hud_elem(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Entity(id)) = args.first() else {
        return Err(ErrorKind::BadType("newClientHudElem takes a player"));
    };
    if !host.ents.get(*id).is_some_and(|e| e.client.is_some()) {
        return Err(ErrorKind::BadType("not a client"));
    }
    let owner = id.0;
    let elem = host.ents.spawn_hud_elem(cx)?;
    with_hud(host, elem, |h| h.owner = owner)?;
    Ok(Value::Entity(elem))
}

/// `newTeamHudElem(team)` (`GScr_NewTeamHudElem` 0x4b3d0): owned by every
/// client the way `newHudElem`'s is, but drawn only for the clients whose
/// `clientState.team` matches the record's team field at +0x74. The three
/// names it takes and the numbers it stores (0x4b3ed, 0x4b400, 0x4b41d) are
/// the three `.sessionteam` assigns bar `none`
/// (docs/research/clientstate-wire-format.md, "`team`"), so a `none` client
/// -- which is every player under `dm` -- matches no team element.
pub fn new_team_hud_elem(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(a)) = args.first() else {
        return Err(ErrorKind::BadType("newTeamHudElem takes a team name"));
    };
    let team = match cx.resolve(*a) {
        "allies" => TEAM_ALLIES,
        "axis" => TEAM_AXIS,
        "spectator" => TEAM_SPECTATOR,
        _ => return Err(ErrorKind::BadType("not a team name")),
    };
    let elem = host.ents.spawn_hud_elem(cx)?;
    with_hud(host, elem, |h| h.team = team)?;
    Ok(Value::Entity(elem))
}

/// `<hudelem> setText(text)` (0x4c590, hudelem method 0): the string an
/// element draws, stored at +0x68 as the index `G_LocalizedStringIndex`
/// answers, under element type 1. It shares the prologue every payload
/// method has, clearing the block the other types use. A localized literal
/// is accepted the way `precacheString` accepts one: `respawn()` passes
/// `&"MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN"`.
pub fn set_text(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = hud_receiver(host, recv)?;
    let [Value::String(a) | Value::Localized(a)] = args else {
        return Err(ErrorKind::BadType("setText takes one string"));
    };
    let name = cx.resolve(*a).to_string();
    let index = host
        .allocators
        .localized_index(&mut host.configstrings, &name)?;
    with_hud(host, id, |h| {
        h.clear_payload();
        h.elem_type = elem_type::TEXT;
        h.text = index;
    })?;
    Ok(Value::Undefined)
}

/// `<hudelem> setValue(number)` (0x4c684, method 8): type 2 with the float
/// at +0x64.
pub fn set_value(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = hud_receiver(host, recv)?;
    let v = match args {
        [Value::Int(i)] => *i as f32,
        [Value::Float(f)] => *f,
        _ => return Err(ErrorKind::BadType("setValue takes one number")),
    };
    with_hud(host, id, |h| {
        h.clear_payload();
        h.elem_type = elem_type::VALUE;
        h.value = v;
    })?;
    Ok(Value::Undefined)
}

/// `<hudelem> setShader(name, width, height)` (0x4b5b0, method 1): type 3
/// with the material index `G_ShaderIndex` answers at +0x38 and the size at
/// +0x30/+0x34. Retail takes exactly three parameters (0x4b5d5).
pub fn set_shader(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = hud_receiver(host, recv)?;
    let [name, w, h] = args else {
        return Err(ErrorKind::BadType("setShader takes a name and a size"));
    };
    let (Value::String(a) | Value::Localized(a)) = name else {
        return Err(ErrorKind::BadType("setShader takes a shader name"));
    };
    let (Some(w), Some(h)) = (as_int(*w), as_int(*h)) else {
        return Err(ErrorKind::BadType("setShader takes a numeric size"));
    };
    let name = cx.resolve(*a).to_string();
    let index = host
        .allocators
        .shader_index(&mut host.configstrings, &name)?;
    with_hud(host, id, |s| {
        s.clear_payload();
        s.elem_type = elem_type::SHADER;
        s.shader = index;
        s.width = w;
        s.height = h;
    })?;
    Ok(Value::Undefined)
}

/// `<hudelem> destroy()` (0x4c9d0, hudelem method 13): frees the record.
/// `ObjectTable::free` takes the HUD path for a number in that range, which
/// is retail's `HudElem_Free` rather than `G_FreeEntity`. Retail's free
/// zeroes the record's type, which is what drops it out of the count the
/// next snapshot carries; the record going away here is the same fact.
pub fn destroy(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = hud_receiver(host, recv)?;
    host.ents.free(id);
    Ok(Value::Undefined)
}

/// Edits one record's [`HudState`]. The receiver check has already run, so
/// a record missing here is a bug rather than a script error.
fn with_hud(
    host: &mut GameHost,
    id: EntId,
    f: impl FnOnce(&mut HudState),
) -> Result<(), ErrorKind> {
    let Some(state) = host.ents.get_mut(id).and_then(|e| e.hud.as_mut()) else {
        return Err(ErrorKind::BadType("needs a HUD element receiver"));
    };
    f(state);
    Ok(())
}

/// A size argument, which retail reads with `Scr_GetInt`.
fn as_int(v: Value) -> Option<i32> {
    match v {
        Value::Int(i) => Some(i),
        Value::Float(f) => Some(f as i32),
        _ => None,
    }
}

/// `<hudelem> setTimer(seconds)` (0x4b8e4, hudelem method 2): a countdown
/// clock. Retail takes exactly one parameter, converts it to milliseconds
/// with the x87 rounding mode set to round up (0x4b942, so 0.0005 s is
/// 1 ms), rejects anything not greater than zero, and stores the absolute
/// `level.time + ms` at +0x5c (0x4b9ef) under element type 4. The clock
/// `startGame` puts on screen is this.
pub fn set_timer(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    set_any_timer(host, recv, args, elem_type::TIMER_DOWN)
}

/// `setTimerUp` (0x4ba04, method 3): the same clock counting up.
pub fn set_timer_up(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    set_any_timer(host, recv, args, elem_type::TIMER_UP)
}

/// `setTenthsTimer` (0x4baf4, method 4): tenths of a second, counting down.
/// `dm.gsc`'s killcam clock is one.
pub fn set_tenths_timer(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    set_any_timer(host, recv, args, elem_type::TENTHS_DOWN)
}

/// `setTenthsTimerUp` (0x4bc14, method 5).
pub fn set_tenths_timer_up(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    set_any_timer(host, recv, args, elem_type::TENTHS_UP)
}

/// What the four timer methods share; only the type code differs, and the
/// client is what reads it as seconds or tenths, up or down.
fn set_any_timer(
    host: &mut GameHost,
    recv: Option<Target>,
    args: &[Value],
    ty: i32,
) -> Result<Value, ErrorKind> {
    let id = hud_receiver(host, recv)?;
    let seconds = match args {
        [Value::Int(i)] => *i as f32,
        [Value::Float(f)] => *f,
        _ => return Err(ErrorKind::BadType("a timer takes a time in seconds")),
    };
    let ms = (seconds * 1000.0).ceil() as i32;
    if ms <= 0 {
        return Err(ErrorKind::BadType("a timer's time must be above zero"));
    }
    let end = host.level_time_ms.wrapping_add(ms);
    with_hud(host, id, |h| {
        h.clear_payload();
        h.elem_type = ty;
        h.time = end;
    })?;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::entity::HUD_OWNER_ALL;
    use crate::game::testing::fixture;

    /// A HUD element is numbered in its own range and is not a gentity, so
    /// it must not show up in `getEntArray`'s walk.
    #[test]
    fn a_new_hud_elem_is_numbered_above_the_gentities() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = new_hud_elem(&mut host, cx, None, &[]).unwrap();
            let b = new_hud_elem(&mut host, cx, None, &[]).unwrap();
            assert_eq!(a, Value::Entity(EntId(FIRST_HUD_ELEM)));
            assert_eq!(b, Value::Entity(EntId(FIRST_HUD_ELEM + 1)));
            assert_eq!(host.ents.iter_inuse().count(), 0);
        });
    }

    /// The fields `startGame` writes on the clock round-trip through the HUD
    /// field table, and `fontscale` starts at retail's 1.0 rather than
    /// undefined.
    #[test]
    fn a_hud_elems_fields_route_to_the_hud_table() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let Value::Entity(id) = new_hud_elem(&mut host, cx, None, &[]).unwrap() else {
                panic!("newHudElem returns an object");
            };
            let x = cx.intern_folded("x");
            let scale = cx.intern_folded("fontscale");
            vcod_gsc::Host::set_field(&mut host, cx, id, x, Value::Int(320)).unwrap();
            assert_eq!(
                vcod_gsc::Host::get_field(&mut host, cx, id, x),
                Value::Int(320)
            );
            assert_eq!(
                vcod_gsc::Host::get_field(&mut host, cx, id, scale),
                Value::Float(1.0)
            );
        });
    }

    /// `setTimer` is a hudelem method: retail's dispatch never offers it to a
    /// gentity, and a time of zero is the error it raises.
    #[test]
    fn settimer_needs_a_hud_receiver_and_a_positive_time() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let hud = match new_hud_elem(&mut host, cx, None, &[]).unwrap() {
                Value::Entity(id) => id,
                _ => panic!("newHudElem returns an object"),
            };
            let arg = [Value::Int(60)];
            assert!(set_timer(&mut host, cx, Some(Target::Entity(e)), &arg).is_err());
            assert!(set_timer(&mut host, cx, Some(Target::Entity(hud)), &arg).is_ok());
            assert!(set_timer(&mut host, cx, Some(Target::Entity(hud)), &[Value::Int(0)]).is_err());
        });
    }

    /// The three allocators differ in exactly two record fields, and those
    /// two are what `wire::hud_elems` filters on.
    #[test]
    fn the_three_allocators_differ_in_owner_and_team() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 3).unwrap();
            let all = ent(new_hud_elem(&mut host, cx, None, &[]).unwrap());
            let mine = ent(new_client_hud_elem(&mut host, cx, None, &[Value::Entity(c)]).unwrap());
            let axis = Value::String(cx.intern_exact("axis"));
            let team = ent(new_team_hud_elem(&mut host, cx, None, &[axis]).unwrap());
            let state = |host: &GameHost, id| host.ents.get(id).unwrap().hud.unwrap();
            assert_eq!(state(&host, all).owner, HUD_OWNER_ALL);
            assert_eq!(state(&host, all).team, 0);
            assert_eq!(state(&host, mine).owner, 3);
            assert_eq!(state(&host, team).owner, HUD_OWNER_ALL);
            assert_eq!(state(&host, team).team, TEAM_AXIS);

            let bad = Value::String(cx.intern_exact("none"));
            assert!(new_team_hud_elem(&mut host, cx, None, &[bad]).is_err());
        });
    }

    /// Each payload method tags the record and clears what the last one
    /// wrote: an element that was a timer and becomes text carries no stale
    /// deadline, which is the block retail clears in every one of them.
    #[test]
    fn a_payload_method_tags_the_record_and_clears_the_last_one() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            host.level_time_ms = 5_000;
            let id = ent(new_hud_elem(&mut host, cx, None, &[]).unwrap());
            let recv = Some(Target::Entity(id));
            set_timer(&mut host, cx, recv, &[Value::Int(30)]).unwrap();
            let state = host.ents.get(id).unwrap().hud.unwrap();
            assert_eq!(state.elem_type, elem_type::TIMER_DOWN);
            assert_eq!(state.time, 35_000, "level.time + the rounded-up ms");

            let text = Value::Localized(cx.intern_exact("MPSCRIPT_KILLCAM"));
            set_text(&mut host, cx, recv, &[text]).unwrap();
            let state = host.ents.get(id).unwrap().hud.unwrap();
            assert_eq!(state.elem_type, elem_type::TEXT);
            assert_eq!(state.time, 0, "the deadline is cleared");
            // The first index the localized range hands out is 1, and the
            // configstring behind it is 1244 + n.
            assert_eq!(state.text, 1);
            assert_eq!(host.configstrings[1245], "MPSCRIPT_KILLCAM");

            let black = Value::String(cx.intern_exact("black"));
            let args = [black, Value::Int(640), Value::Int(112)];
            set_shader(&mut host, cx, recv, &args).unwrap();
            let state = host.ents.get(id).unwrap().hud.unwrap();
            assert_eq!(state.elem_type, elem_type::SHADER);
            assert_eq!((state.shader, state.width, state.height), (1, 640, 112));
            assert_eq!(state.text, 0, "the string is cleared");
            assert_eq!(host.configstrings[1501], "black");
        });
    }

    fn ent(v: Value) -> EntId {
        match v {
            Value::Entity(id) => id,
            other => panic!("expected a HUD element, got {other:?}"),
        }
    }

    /// `newClientHudElem` takes a player and nothing else: retail's own
    /// `not a client` param error is the check. What it hands back is a HUD
    /// element like `newHudElem`'s, and `destroy` gives the record back.
    #[test]
    fn a_client_hud_elem_needs_a_player_and_destroy_frees_it() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0).unwrap();
            let prop = host.ents.spawn(cx).unwrap();
            assert!(new_client_hud_elem(&mut host, cx, None, &[Value::Entity(prop)]).is_err());
            assert!(new_client_hud_elem(&mut host, cx, None, &[]).is_err());

            let Value::Entity(id) =
                new_client_hud_elem(&mut host, cx, None, &[Value::Entity(c)]).unwrap()
            else {
                panic!("newClientHudElem returns an object");
            };
            assert_eq!(id, EntId(FIRST_HUD_ELEM));
            let text = Value::Localized(cx.intern_exact("MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN"));
            let recv = Some(Target::Entity(id));
            set_text(&mut host, cx, recv, &[text]).unwrap();
            assert!(set_text(&mut host, cx, recv, &[Value::Int(1)]).is_err());
            assert!(set_text(&mut host, cx, Some(Target::Entity(prop)), &[text]).is_err());

            destroy(&mut host, cx, recv, &[]).unwrap();
            assert!(host.ents.get(id).is_none());
            // The freed record is the next one handed out.
            assert_eq!(
                new_client_hud_elem(&mut host, cx, None, &[Value::Entity(c)]).unwrap(),
                Value::Entity(EntId(FIRST_HUD_ELEM))
            );
        });
    }
}
