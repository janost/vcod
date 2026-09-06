//! The two team-score builtins (docs/research/cod11-map-cycle.md, 6.3):
//! `level+0x1fc` is axis and `level+0x200` is allies, the setter mirrors each
//! into configstring 5 and 6, and the scoreboard's tokens 2 and 3 read them
//! back in that order.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("getteamscore", get_team_score),
    ("setteamscore", set_team_score),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// Axis 0, allies 1, and the configstring each mirrors into. Both builtins
/// compare against the same two `scr_const` slots and error on anything
/// else: `"Illegal team string '%s'. Must be allies, or axis."`.
fn team(cx: &Cx, v: Option<&Value>) -> Result<(usize, usize), ErrorKind> {
    let Some(Value::String(s)) = v else {
        return Err(ErrorKind::BadType("a team score takes a team string"));
    };
    match cx.resolve(*s) {
        "axis" => Ok((0, 5)),
        "allies" => Ok((1, 6)),
        _ => Err(ErrorKind::BadType(
            "illegal team string: must be allies, or axis",
        )),
    }
}

pub fn get_team_score(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (i, _) = team(cx, args.first())?;
    Ok(Value::Int(host.team_scores[i]))
}

/// The level word, the configstring, and the ranks flag the intermission
/// scoreboard drain reads.
pub fn set_team_score(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (i, cs) = team(cx, args.first())?;
    // `Scr_GetInt` truncates a float rather than refusing it.
    let n = match args.get(1) {
        Some(Value::Int(n)) => *n,
        Some(Value::Float(f)) => *f as i32,
        _ => return Err(ErrorKind::BadType("setTeamScore takes a number")),
    };
    host.team_scores[i] = n;
    host.configstrings[cs] = n.to_string();
    host.ranks_dirty = true;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// Which slot is which team, and the configstring each writes: axis is
    /// 5 and allies is 6.
    #[test]
    fn a_team_score_reads_back_and_mirrors_its_configstring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let axis = Value::String(cx.intern_exact("axis"));
            let allies = Value::String(cx.intern_exact("allies"));
            set_team_score(&mut host, cx, None, &[axis, Value::Int(3)]).unwrap();
            set_team_score(&mut host, cx, None, &[allies, Value::Int(-9999)]).unwrap();
            assert_eq!(host.team_scores, [3, -9999]);
            assert_eq!(host.configstrings[5], "3");
            assert_eq!(host.configstrings[6], "-9999");
            assert_eq!(
                get_team_score(&mut host, cx, None, &[axis]).unwrap(),
                Value::Int(3)
            );
            assert_eq!(
                get_team_score(&mut host, cx, None, &[allies]).unwrap(),
                Value::Int(-9999)
            );
            assert!(host.ranks_dirty, "a team score arms the scoreboard drain");
            let bogus = Value::String(cx.intern_exact("spectator"));
            assert!(get_team_score(&mut host, cx, None, &[bogus]).is_err());
            assert!(set_team_score(&mut host, cx, None, &[bogus, Value::Int(1)]).is_err());
        });
    }
}
