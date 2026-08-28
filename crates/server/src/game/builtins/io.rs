//! Builtins that only produce output. `iPrintLn` reaches clients in stage 4;
//! until then all three go to the server log, which is what the probe reads.

use vcod_gsc::{Cx, ErrorKind, Value};

pub fn print_line(cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    let mut out = String::new();
    for a in args {
        match a {
            Value::String(s) => out.push_str(cx.resolve(*s)),
            Value::Int(i) => out.push_str(&i.to_string()),
            Value::Float(f) => out.push_str(&f.to_string()),
            other => out.push_str(&format!("{other:?}")),
        }
    }
    log::info!("script: {out}");
    Ok(Value::Undefined)
}
