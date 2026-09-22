//! The objective table's eight builtins, which write `GameHost::objectives`
//! and reach a client through playerstate block 4
//! (docs/research/cod11-gsc-object-model.md 23.3).
//!
//! Retail also marks an attached entity with `eFlags` bit 0x10 and clears it
//! on detach; no entity here carries `eFlags`, so only the record's `entNum`
//! moves.

use crate::game::host::{empty_objective, GameHost, ENTITYNUM_NONE};
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
use vcod_common::net::msg::MAX_OBJECTIVES;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("objective_add", objective_add),
    ("objective_delete", objective_delete),
    ("objective_state", objective_state),
    ("objective_icon", objective_icon),
    ("objective_position", objective_position),
    ("objective_onentity", objective_onentity),
    ("objective_current", objective_current),
    ("objective_team", objective_team),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

fn index_arg(v: Option<&Value>) -> Result<usize, ErrorKind> {
    match v {
        Some(Value::Int(i)) if (0..MAX_OBJECTIVES as i32).contains(i) => Ok(*i as usize),
        Some(Value::Int(_)) => Err(ErrorKind::BadType("objective index out of range")),
        _ => Err(ErrorKind::BadType("objective index must be an integer")),
    }
}

/// `ObjectiveStateIndexFromString` (0x5e008): empty 0, invisible 2,
/// current 4. The fourth value, 1, is `objective_current`'s alone.
fn state_arg(cx: &Cx, v: Option<&Value>) -> Result<i32, ErrorKind> {
    let Some(Value::String(a)) = v else {
        return Err(ErrorKind::BadType("objective state must be a string"));
    };
    match cx.resolve(*a) {
        "empty" => Ok(0),
        "invisible" => Ok(2),
        "current" => Ok(4),
        _ => Err(ErrorKind::BadType("unknown objective state")),
    }
}

/// Each component truncated toward zero, as retail stores it
/// (docs/research/cod11-gsc-object-model.md 23.3).
fn origin_arg(v: Option<&Value>) -> Result<[f32; 3], ErrorKind> {
    match v {
        Some(Value::Vector(o)) => Ok([o[0].trunc(), o[1].trunc(), o[2].trunc()]),
        _ => Err(ErrorKind::BadType("objective position takes a vector")),
    }
}

/// The icon name through `G_ShaderIndex`, with retail's two param errors on
/// the name itself (0x5a49c, 0x5a4b3).
fn icon_arg(host: &mut GameHost, cx: &Cx, v: Option<&Value>) -> Result<i32, ErrorKind> {
    let Some(Value::String(a)) = v else {
        return Err(ErrorKind::BadType("objective icon takes a shader name"));
    };
    let name = cx.resolve(*a).to_string();
    if name.len() > 63 || name.starts_with('^') {
        return Err(ErrorKind::BadType("bad objective icon name"));
    }
    host.allocators.shader_index(&mut host.configstrings, &name)
}

/// `objective_add(index, state, [origin], [icon])` (0x5a2d0): `teamNum` goes
/// to 0 on every path. Clearing the origin and the icon when the call omits
/// them diverges from retail, which leaves whatever the slot held; no stock
/// script re-adds a slot without both.
pub fn objective_add(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    if args.len() < 2 {
        return Err(ErrorKind::BadType(
            "objective_add takes an index and a state",
        ));
    }
    let i = index_arg(args.first())?;
    let state = state_arg(cx, args.get(1))?;
    let mut o = empty_objective();
    o.state = state;
    if args.len() > 2 {
        o.set_origin(origin_arg(args.get(2))?);
    }
    if args.len() > 3 {
        o.icon = icon_arg(host, cx, args.get(3))?;
    }
    host.objectives[i] = o;
    Ok(Value::Undefined)
}

/// `objective_delete(index)` (0x5e058): the level record is zeroed. The
/// client keeps its own copy, so what it is sent afterwards is state 0 over
/// the icon and origin an earlier frame copied there
/// ([`GameHost::objectives_for`]).
pub fn objective_delete(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    host.objectives[i] = empty_objective();
    Ok(Value::Undefined)
}

/// `objective_state(index, state)` (0x5a4f4): the new state, and a detach
/// when it is `empty` or `invisible`.
pub fn objective_state(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    let state = state_arg(cx, args.get(1))?;
    host.objectives[i].state = state;
    if state == 0 || state == 2 {
        host.objectives[i].ent_num = ENTITYNUM_NONE;
    }
    Ok(Value::Undefined)
}

/// `objective_icon(index, name)` (0x5a5d8).
pub fn objective_icon(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    host.objectives[i].icon = icon_arg(host, cx, args.get(1))?;
    Ok(Value::Undefined)
}

/// `objective_position(index, origin)` (0x5e128).
pub fn objective_position(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    let o = origin_arg(args.get(1))?;
    host.objectives[i].set_origin(o);
    Ok(Value::Undefined)
}

/// `objective_onentity(index, entity)` (0x5e230): the record follows an
/// entity instead of a fixed origin.
pub fn objective_onentity(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    let Some(Value::Entity(id)) = args.get(1) else {
        return Err(ErrorKind::BadType("objective_onentity takes an entity"));
    };
    if host.ents.get(*id).is_none() {
        return Err(ErrorKind::BadType("no such entity"));
    }
    host.objectives[i].ent_num = id.0 as i32;
    Ok(Value::Undefined)
}

/// `objective_current(...)` (0x5a6ac), variadic: every index named goes to
/// state 4, and every other record already reading 4 drops to 1, the
/// "active" state no name reaches.
pub fn objective_current(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let mut named = [false; MAX_OBJECTIVES];
    for a in args {
        named[index_arg(Some(a))?] = true;
    }
    for (o, named) in host.objectives.iter_mut().zip(named) {
        if named {
            o.state = 4;
        } else if o.state == 4 {
            o.state = 1;
        }
    }
    Ok(Value::Undefined)
}

/// `objective_team(index, team)` (0x5e2c8). A record whose team is set
/// reaches only that team's clients ([`GameHost::objectives_for`]).
pub fn objective_team(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let i = index_arg(args.first())?;
    let Some(Value::String(a)) = args.get(1) else {
        return Err(ErrorKind::BadType("objective_team takes a team name"));
    };
    let team = match cx.resolve(*a) {
        "allies" => TEAM_ALLIES,
        "axis" => TEAM_AXIS,
        "none" => 0,
        _ => return Err(ErrorKind::BadType("not a team name")),
    };
    host.objectives[i].team_num = team;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
    use vcod_common::net::msg::Objective;
    use vcod_gsc::Value;

    fn s(cx: &mut Cx, t: &str) -> Value {
        Value::String(cx.intern_exact(t))
    }

    #[test]
    fn objective_add_writes_the_record_retail_writes() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let (state, icon) = (s(cx, "current"), s(cx, "gfx/hud/hud@objectiveA.tga"));
            let args = [
                Value::Int(0),
                state,
                Value::Vector([-146.0, 2490.0, 16.0]),
                icon,
            ];
            objective_add(&mut host, cx, None, &args).unwrap();
            let o = host.objectives[0];
            assert_eq!(o.state, 4);
            assert_eq!(o.origin_f32(), [-146.0, 2490.0, 16.0]);
            assert_eq!(o.ent_num, 0x3ff);
            assert_eq!(o.team_num, 0);
            assert_eq!(o.icon, 1, "the first shader index handed out");
            assert_eq!(host.configstrings[1501], "gfx/hud/hud@objectiveA.tga");
        });
    }

    /// Retail's plant capture reads slot 0 at `-176, 2473, -22`
    /// (docs/research/cod11-gsc-object-model.md 23.3). The fractional
    /// charge `(-176.8, 2473.1, -22.96)` is our `getPlant` reproduction
    /// (23.6), not a retail measurement; truncated it gives that slot,
    /// rounded it would not.
    #[test]
    fn objective_origins_truncate_toward_zero() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let state = s(cx, "current");
            let args = [
                Value::Int(0),
                state,
                Value::Vector([-176.8, 2473.6, -22.96]),
            ];
            objective_add(&mut host, cx, None, &args).unwrap();
            assert_eq!(host.objectives[0].origin_f32(), [-176.0, 2473.0, -22.0]);
            let args = [Value::Int(0), Value::Vector([10.9, -0.5, 0.99])];
            objective_position(&mut host, cx, None, &args).unwrap();
            assert_eq!(host.objectives[0].origin_f32(), [10.0, 0.0, 0.0]);
        });
    }

    #[test]
    fn objective_delete_clears_the_record_and_state_names_map() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let invisible = s(cx, "invisible");
            objective_add(&mut host, cx, None, &[Value::Int(3), invisible]).unwrap();
            assert_eq!(host.objectives[3].state, 2);
            let empty = s(cx, "empty");
            objective_state(&mut host, cx, None, &[Value::Int(3), empty]).unwrap();
            assert_eq!(host.objectives[3].state, 0);
            let done = s(cx, "done");
            assert!(objective_state(&mut host, cx, None, &[Value::Int(3), done]).is_err());
            let allies = s(cx, "allies");
            objective_team(&mut host, cx, None, &[Value::Int(3), allies]).unwrap();
            assert_eq!(host.objectives[3].team_num, 2);
            objective_delete(&mut host, cx, None, &[Value::Int(3)]).unwrap();
            assert_eq!(
                host.objectives[3],
                Objective {
                    ent_num: 0x3ff,
                    ..Objective::default()
                }
            );
            let current = s(cx, "current");
            assert!(objective_add(&mut host, cx, None, &[Value::Int(16), current]).is_err());
        });
    }

    /// Retail's filter runs per client onto that client's own copy, so the
    /// axis objective reaches the allied client as state 0 and a deleted slot
    /// keeps the icon and origin the copy already holds.
    #[test]
    fn objective_current_demotes_the_others_and_the_team_filter_blanks_the_state() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let current = s(cx, "current");
            let icon = s(cx, "gfx/hud/hud@objectiveB.tga");
            objective_add(&mut host, cx, None, &[Value::Int(0), current]).unwrap();
            let add_b = [
                Value::Int(1),
                current,
                Value::Vector([1792.0, 2080.0, 20.0]),
                icon,
            ];
            objective_add(&mut host, cx, None, &add_b).unwrap();
            objective_current(&mut host, cx, None, &[Value::Int(1)]).unwrap();
            assert_eq!((host.objectives[0].state, host.objectives[1].state), (1, 4));
            let axis_name = s(cx, "axis");
            objective_team(&mut host, cx, None, &[Value::Int(0), axis_name]).unwrap();

            // Slot 0 is the allied client, slot 1 the axis one.
            let allies = host.objectives_for(0, crate::game::script::TEAM_ALLIES);
            assert_eq!(
                allies[0].state, 0,
                "an axis objective is sent to allies as state 0"
            );
            assert_eq!(allies[1].state, 4);
            let axis = host.objectives_for(1, crate::game::script::TEAM_AXIS);
            assert_eq!(axis[0].state, 1);

            // The allied client has slot 1 whole by now, so the delete leaves
            // it the icon and the origin under a blanked state.
            let before = allies[1];
            assert_ne!(before.icon, 0);
            objective_delete(&mut host, cx, None, &[Value::Int(1)]).unwrap();
            let after = host.objectives_for(0, crate::game::script::TEAM_ALLIES)[1];
            assert_eq!(after.state, 0);
            assert_eq!(after.icon, before.icon);
            assert_eq!(after.origin_f32(), [1792.0, 2080.0, 20.0]);
        });
    }
}
