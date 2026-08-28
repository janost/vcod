//! Runs each committed semantics probe through vcod-gsc and diffs its
//! output against what the retail server printed. Retail is right; see
//! tests/fixtures/semantics/README.md.

use std::collections::BTreeMap;
use vcod_gsc::{
    Atom, Cx, EntId, ErrorKind, FuncRef, Host, Loader, ScriptSource, Target, Value, Vm,
};

/// A probe's file plus a stub for the one script it calls across files.
/// `SetupCallbacks` only stores default callbacks and defines damage-flag
/// constants on `level`; nothing a probe measures depends on it.
struct ProbeSource {
    path: String,
    text: String,
}

impl ScriptSource for ProbeSource {
    fn read(&self, canonical: &str) -> Option<String> {
        if canonical == self.path {
            return Some(self.text.clone());
        }
        if canonical == "maps/mp/gametypes/_callbacksetup" {
            return Some("SetupCallbacks() {}\n".to_string());
        }
        None
    }
}

/// Only what a probe calls: `logPrint` collects, `isDefined` answers.
#[derive(Default)]
struct ProbeHost {
    lines: Vec<String>,
}

impl Host for ProbeHost {
    fn builtin(
        &mut self,
        cx: &mut Cx,
        name: Atom,
        _recv: Option<Target>,
        args: &[Value],
    ) -> Result<Value, ErrorKind> {
        match cx.resolve_folded(name) {
            "logprint" | "println" | "print" => {
                if let Some(Value::String(a)) = args.first() {
                    for line in cx.resolve(*a).lines() {
                        self.lines.push(line.to_string());
                    }
                }
                Ok(Value::Undefined)
            }
            "isdefined" => Ok(Value::Int(
                !matches!(args.first(), None | Some(Value::Undefined)) as i32,
            )),
            _ => Err(ErrorKind::MissingBuiltin(name)),
        }
    }

    fn get_field(&mut self, _cx: &mut Cx, _e: EntId, _f: Atom) -> Value {
        Value::Undefined
    }

    fn set_field(&mut self, _cx: &mut Cx, _e: EntId, _f: Atom, _v: Value) -> Result<(), ErrorKind> {
        Ok(())
    }
}

/// What one probe produced here: its `PROBE ` lines, the message of the
/// first error that stopped a thread (if any), and which of `run_probe`'s
/// two paths produced it. Shaped to match a capture section so the two
/// compare directly.
struct Run {
    lines: Vec<String>,
    error: Option<String>,
    /// `"call_now"` or `"start_thread"`; carried into failure messages so a
    /// reader is not puzzled by which path an error could or couldn't have
    /// come from -- see the two paths' doc comments below.
    path: &'static str,
}

/// Loads `name`'s probe fresh and returns its `main`, ready to run.
fn install(name: &str) -> Result<(Vm, FuncRef), String> {
    let path = format!("maps/mp/gametypes/{name}");
    let text = std::fs::read_to_string(format!("tests/fixtures/semantics/{name}.gsc"))
        .unwrap_or_else(|e| panic!("read {name}.gsc: {e}"));
    let mut vm = Vm::new();
    let mut loader = Loader::new(Box::new(ProbeSource {
        path: path.clone(),
        text,
    }));
    loader
        .load(&mut vm, &path)
        .map_err(|e| format!("{name} does not load: {e:?}"))?;
    let main = vm.func_ref(&path, "main");
    Ok((vm, main))
}

/// Runs `main` through `call_now`, which returns its error instead of only
/// logging it (`start_thread`'s doc comment), so a probe that dies
/// synchronously -- most of them, since nothing before their first fatal
/// expression suspends -- surfaces that error here. A probe built around a
/// `wait` (`probe_notify`) cannot run this way: `call_now` rejects a
/// suspend as `ErrorKind::SuspendedInImmediateCall`, at which point this
/// falls back to a fresh `start_thread` plus 200 frames of 50 ms, which
/// covers probe_notify's two 0.5 s waits with room to spare. That fallback
/// path inherits `start_thread`'s own blind spot -- an error in its
/// immediate run has nowhere to go either -- but nothing in the corpus
/// needs both a `wait` and a synchronous-death measurement in one file.
fn run_probe(name: &str) -> Result<Run, String> {
    let (mut vm, main) = install(name)?;
    let mut host = ProbeHost::default();
    match vm.call_now(&mut host, 0, main, None, Vec::new()) {
        Ok(_) => Ok(Run {
            lines: host.lines,
            error: None,
            path: "call_now",
        }),
        Err(e) if e.kind == ErrorKind::SuspendedInImmediateCall => {
            let (mut vm, main) = install(name)?;
            let mut host = ProbeHost::default();
            let mut error = None;
            vm.start_thread(&mut host, 0, main, None, Vec::new());
            for frame in 1..=200 {
                for e in vm.run_frame(&mut host, frame * 50) {
                    if error.is_none() {
                        error = Some(format!("{:?}", e.kind));
                    }
                }
            }
            Ok(Run {
                lines: host.lines,
                error,
                path: "start_thread",
            })
        }
        Err(e) => Ok(Run {
            lines: host.lines,
            error: Some(format!("{:?}", e.kind)),
            path: "call_now",
        }),
    }
}

/// `retail-captures.txt` parsed into `probe name -> (PROBE lines, fatal)`.
fn captures() -> BTreeMap<String, (Vec<String>, Option<String>)> {
    let text = include_str!("fixtures/semantics/retail-captures.txt");
    let mut out = BTreeMap::new();
    let mut name = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            name = rest.trim().to_string();
            out.insert(name.clone(), (Vec::new(), None));
        } else if let Some(msg) = line.strip_prefix("PROBE_FATAL ") {
            out.get_mut(&name).expect("a section header first").1 = Some(msg.to_string());
        } else if line.starts_with("PROBE ") {
            out.get_mut(&name)
                .expect("a section header first")
                .0
                .push(line.to_string());
        }
    }
    out
}

/// `probe_ents` measures getentarray's return order and needs real map
/// entities; the object model arrives in stage 2. Its recorded order is
/// what that implementation has to reproduce.
const NOT_YET_RUNNABLE: &[&str] = &["probe_ents"];

#[ignore = "fails until the divergences in the semantics fix task are done"]
#[test]
fn vcod_matches_retail_on_every_probe() {
    let mut failures = Vec::new();
    for (name, (retail_lines, retail_fatal)) in captures() {
        if NOT_YET_RUNNABLE.contains(&name.as_str()) {
            continue;
        }
        let ours = match run_probe(&name) {
            Ok(ours) => ours,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if ours.lines != retail_lines {
            failures.push(format!(
                "{name} ({}): output differs\n  retail: {retail_lines:#?}\n  vcod:   {:#?}",
                ours.path, ours.lines
            ));
        }
        // Where retail dies, vcod raises an error and aborts the thread
        // instead of the server; what has to match is that it stopped at the
        // same point, not how loudly.
        match (retail_fatal.is_some(), ours.error.is_some()) {
            (true, false) => failures.push(format!(
                "{name} ({}): retail died ({}) and vcod carried on",
                ours.path,
                retail_fatal.unwrap()
            )),
            (false, true) => failures.push(format!(
                "{name} ({}): vcod raised {} and retail did not",
                ours.path,
                ours.error.unwrap()
            )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
