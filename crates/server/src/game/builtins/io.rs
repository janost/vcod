//! Builtins that only produce output. All three go to the server log, which
//! is what the probe reads; `iPrintLn` does not reach clients yet.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Value};

pub fn print_line(host: &mut GameHost, cx: &Cx, args: &[Value]) -> Result<Value, ErrorKind> {
    let line = render(cx, args);
    log::info!("script: {line}");
    // Retail's log splits a multi-line string into separate log lines; a
    // capture diff reads it the same way.
    for l in line.lines() {
        host.script_log.push(l.to_string());
    }
    Ok(Value::Undefined)
}

/// The rendered log line, split out from `print_line` so a test can pin it
/// without a log-capturing harness.
fn render(cx: &Cx, args: &[Value]) -> String {
    let mut out = String::new();
    for a in args {
        match cx.format_number(*a) {
            Some(s) => out.push_str(&s),
            // Undefined, entities, structs, arrays, function pointers and
            // anim references: none of these render for concatenation
            // either, but a print is a log line, not a runtime error, so
            // it prints something rather than failing the thread.
            None => out.push_str(&format!("{a:?}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use vcod_gsc::{Atom, EntId, Host, Target};

    /// A `Host` that hands `render`'s output back to the test instead of
    /// the server log, so the assertion below does not need a log-capturing
    /// harness. Its `builtin` ignores `name` and always renders `args`,
    /// since the test only needs one call site.
    struct CaptureHost(RefCell<String>);

    impl Host for CaptureHost {
        fn builtin(
            &mut self,
            cx: &mut Cx,
            _name: Atom,
            _recv: Option<Target>,
            args: &[Value],
        ) -> Result<Value, ErrorKind> {
            *self.0.borrow_mut() = render(cx, args);
            Ok(Value::Undefined)
        }

        fn get_field(&mut self, _cx: &mut Cx, _ent: EntId, _field: Atom) -> Value {
            Value::Undefined
        }

        fn set_field(
            &mut self,
            _cx: &mut Cx,
            _ent: EntId,
            _field: Atom,
            _value: Value,
        ) -> Result<(), ErrorKind> {
            unreachable!("not exercised by this test")
        }
    }

    /// `f32::to_string` and `Cx::format_number`'s `%g` renderer diverge for
    /// `1.0 / 3.0`: `"0.33333334"` vs. `"0.333333"`. `print_line` must use
    /// the latter, the same renderer `env::set_cull_fog` uses, so a number
    /// prints identically no matter which builtin printed it.
    #[test]
    fn print_line_renders_floats_through_format_number() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = CaptureHost(RefCell::new(String::new()));
        let src = "main() { anyBuiltin(1.0 / 3.0); }";
        let ast = vcod_gsc::parse::parse_file(src).unwrap();
        let fns = vcod_gsc::compile::compile_file(&ast, "test", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let f = vm.func_ref("test", "main");
        vm.call_now(&mut host, 0, f, None, Vec::new()).unwrap();

        assert_eq!(host.0.into_inner(), "0.333333");
    }
}
