//! Cvar builtin: reads `GameHost::cvars`.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[("getcvar", get_cvar)];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
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
}
