//! Builtins that produce output. `println` and `logPrint` go to the server
//! log; `iPrintLn` is a reliable command to the clients.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

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

/// `iPrintLn(message [, args...])` and its receiver form: the reliable
/// command `f "<message>"` to every client, or to the receiver alone when
/// one is given (`Scr_MakeGameMessage` 0x5ccd8 with the word `f`, reached
/// from 0x5cd28 with client -1 and from 0x45594 with the receiver). The
/// message itself is packed by [`super::message::construct`].
///
/// Retail writes no log line here; this one is vcod's, because the probe
/// reads the server log to see what a run did.
pub fn iprint_line(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let text = super::message::construct(host, cx, args);
    log::info!("script: iprintln {text}");
    // The same rewrite `setClientCvar` does: the message is the line's only
    // quoted argument, so a `"` in it would close that argument early.
    let cmd = format!("f \"{}\"", text.replace('"', "'"));
    match recv {
        Some(_) => {
            let slot = super::client::client_receiver(host, recv)?;
            host.client_commands.push((slot, cmd));
        }
        None => {
            for slot in host.client_slots() {
                host.client_commands.push((slot, cmd.clone()));
            }
        }
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
mod iprintln_tests {
    use super::*;
    use crate::game::host::GameHost;

    /// The two forms, against the retail capture's own lines: the global
    /// `iprintln(&"MPSCRIPT_CONNECTED", self)` reaches every client as
    /// `f "MPSCRIPT_CONNECTED\x15vcod^7"`, and the receiver form reaches one
    /// (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`).
    #[test]
    fn iprintln_goes_out_as_the_f_reliable_command() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = GameHost::new(vec![String::new(); 2048]);
        let ents = &mut host.ents;
        let (a, b, key) = vm.with_cx(|cx| {
            let a = ents.spawn_client(cx, 0, None).expect("a client");
            let b = ents.spawn_client(cx, 1, None).expect("a client");
            for (id, name) in [(a, "vcod"), (b, "other")] {
                let name = Value::String(cx.intern_exact(name));
                if let Some(c) = ents.get_mut(id).and_then(|e| e.client.as_mut()) {
                    c[0] = name;
                }
            }
            (a, b, cx.intern_exact("MPSCRIPT_CONNECTED"))
        });
        vm.with_cx(|cx| {
            iprint_line(
                &mut host,
                cx,
                None,
                &[Value::Localized(key), Value::Entity(a)],
            )
            .unwrap();
            iprint_line(
                &mut host,
                cx,
                Some(Target::Entity(b)),
                &[Value::Localized(key)],
            )
            .unwrap();
        });
        assert_eq!(
            host.client_commands,
            vec![
                (0, "f \"MPSCRIPT_CONNECTED\u{15}vcod^7\"".to_string()),
                (1, "f \"MPSCRIPT_CONNECTED\u{15}vcod^7\"".to_string()),
                (1, "f \"MPSCRIPT_CONNECTED\u{15}\"".to_string()),
            ]
        );
    }
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
