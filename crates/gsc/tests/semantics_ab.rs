//! Runs each committed semantics probe through vcod-gsc and diffs its
//! output against what the retail server printed. Retail is right; see
//! tests/fixtures/semantics/README.md.

use std::collections::BTreeMap;
use vcod_gsc::{Atom, Cx, EntId, ErrorKind, Host, Loader, ScriptSource, Target, Value, Vm};

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
        // `Cx` only exposes `resolve`, which returns an atom's as-written
        // spelling (`atom.rs`), not its case-folded form; every probe
        // writes `logPrint`, so matching the raw spelling against these
        // lowercase literals would silently miss it and mask every probe
        // behind a false MissingBuiltin.
        match cx.resolve(name).to_ascii_lowercase().as_str() {
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

/// What one probe produced here: its `PROBE ` lines, and the message of the
/// first error that stopped a thread, if any. Shaped to match a capture
/// section so the two compare directly.
struct Run {
    lines: Vec<String>,
    error: Option<String>,
}

/// Installs the probe, starts `main`, and steps 200 frames of 50 ms, which
/// covers probe_notify's two 0.5 s waits with room to spare.
fn run_probe(name: &str) -> Run {
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
        .unwrap_or_else(|e| panic!("{name} does not load: {e:?}"));

    let mut host = ProbeHost::default();
    let mut error = None;
    let main = vm.func_ref(&path, "main");
    vm.start_thread(&mut host, 0, main, None, Vec::new());
    for frame in 1..=200 {
        for e in vm.run_frame(&mut host, frame * 50) {
            if error.is_none() {
                error = Some(format!("{:?}", e.kind));
            }
        }
    }
    Run {
        lines: host.lines,
        error,
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
        let ours = run_probe(&name);
        if ours.lines != retail_lines {
            failures.push(format!(
                "{name}: output differs\n  retail: {retail_lines:#?}\n  vcod:   {:#?}",
                ours.lines
            ));
        }
        // Where retail dies, vcod raises an error and aborts the thread
        // instead of the server; what has to match is that it stopped at the
        // same point, not how loudly.
        match (retail_fatal.is_some(), ours.error.is_some()) {
            (true, false) => failures.push(format!(
                "{name}: retail died ({}) and vcod carried on",
                retail_fatal.unwrap()
            )),
            (false, true) => failures.push(format!(
                "{name}: vcod raised {} and retail did not",
                ours.error.unwrap()
            )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
