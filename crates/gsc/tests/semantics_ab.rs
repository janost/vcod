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

/// Only what a probe calls: `logPrint` collects, `isDefined` answers,
/// `spawnStruct` makes a struct.
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
            "spawnstruct" => Ok(Value::Struct(cx.new_struct())),
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

    fn roots(&self, _out: &mut Vec<Value>) {}
}

/// What one probe produced here: its `PROBE ` lines, the message of the
/// first error that stopped a thread (if any), and which of `run_probe`'s
/// two paths produced it. Shaped to match a capture section so the two
/// compare directly.
struct Run {
    lines: Vec<String>,
    error: Option<String>,
    /// `"call_now"`, `"start_thread"` or, for a script that did not compile,
    /// `"compile error: <msg>"`; carried into failure messages so a reader
    /// is not puzzled by which path an error could or couldn't have come
    /// from. See the two paths' doc comments below.
    path: String,
}

/// Loads `name`'s probe fresh and returns its `main`, ready to run, or the
/// compile error that rejected it.
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
        .map_err(|e| format!("compile error: {} (line {})", e.msg, e.line))?;
    let main = vm.func_ref(&path, "main");
    Ok((vm, main))
}

/// Runs `main` through `call_now`, which returns its error instead of only
/// logging it (`start_thread`'s doc comment), so a probe that dies
/// synchronously (most of them, since nothing before their first fatal
/// expression suspends) surfaces that error here. A probe built around a
/// `wait` (`probe_notify`) cannot run this way: `call_now` rejects a
/// suspend as `ErrorKind::SuspendedInImmediateCall`, at which point this
/// falls back to a fresh `start_thread` plus 200 frames of 50 ms, which
/// covers probe_notify's two 0.5 s waits with room to spare. That fallback
/// path inherits `start_thread`'s own blind spot (an error in its
/// immediate run has nowhere to go either), but nothing in the corpus
/// needs both a `wait` and a synchronous-death measurement in one file.
///
/// A compile error voids the whole script on retail before `main()` runs:
/// its section holds only [`COMPILE_ERROR`], and a run here that does not
/// compile has no lines and that error.
fn run_probe(name: &str) -> Result<Run, String> {
    let (mut vm, main) = match install(name) {
        Ok(loaded) => loaded,
        Err(path) => {
            return Ok(Run {
                lines: Vec::new(),
                error: Some(COMPILE_ERROR.to_string()),
                path,
            });
        }
    };
    let mut host = ProbeHost::default();
    match vm.call_now(&mut host, main, None, Vec::new()) {
        Ok(_) => Ok(Run {
            lines: host.lines,
            error: None,
            path: "call_now".into(),
        }),
        Err(e) if e.kind == ErrorKind::SuspendedInImmediateCall => {
            let (mut vm, main) = install(name)?;
            let mut host = ProbeHost::default();
            let mut error = None;
            vm.start_thread(&mut host, main, None, Vec::new());
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
                path: "start_thread".into(),
            })
        }
        Err(e) => Ok(Run {
            lines: host.lines,
            error: Some(format!("{:?}", e.kind)),
            path: "call_now".into(),
        }),
    }
}

/// What `run_probe.sh` prints, as `PROBE_ERROR`, for a script retail would
/// not compile.
const COMPILE_ERROR: &str = "script compile error";

/// `retail-captures.txt` parsed into `probe name -> (PROBE lines, fatal)`;
/// a compile error is a fatal of [`COMPILE_ERROR`].
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
        } else if line == format!("PROBE_ERROR {COMPILE_ERROR}") {
            out.get_mut(&name).expect("a section header first").1 = Some(COMPILE_ERROR.into());
        } else if line.starts_with("PROBE ") {
            out.get_mut(&name)
                .expect("a section header first")
                .0
                .push(line.to_string());
        }
    }
    out
}

/// Probes that run in another crate because they need state `vcod-gsc` alone
/// does not have. `probe_ents` needs an object model and a real map, both of
/// which live in `crates/server`; it runs there, in
/// `crates/server/tests/semantics_ents.rs`, against this same capture file.
/// `probe_delete` needs the same two things to measure the deferred-free
/// window on real spawned entities, so it is claimed there too. `probe_cvar`
/// and `probe_not_string` call `getCvar`/`setCvar`/`getCvarInt`/
/// `getCvarFloat`/`getTime`/`randomInt`, which this file's `ProbeHost`
/// cannot answer without a fake cvar table (the real one is
/// `crates/server`'s `Cvars`, which `vcod-gsc` may not depend on), so they
/// run there too, against a real `GameHost`.
///
/// `probe_bootstrap` measures the order of the two `main()`s against the
/// real `mp_pavlov.gsc`, so `ProbeSource`'s stub-everything-else answer
/// cannot serve it: it needs the pak-backed source the server crate has,
/// with the probe overlaid on the gametype path.
///
/// The three `probe_persist_*` probes and `probe_hostname` need a fourth
/// thing: an engine that can end a level. Each calls `map_restart` or
/// `exitLevel`, which queue a console line the server drains, and measures
/// what the level that comes back still has; `crates/server`'s `Server` is
/// the only host with that machinery.
///
/// The seven `probe_stale_*` probes free an entity or a hudelem and read
/// the handle back, which needs the object table and its deferred free.
///
/// `probe_loadfx_wait`, the `probe_precache_*` and the `probe_late_*`
/// probes call game builtins across the end of `level.initializing`, which
/// only the server's bootstrap clears.
///
/// `probe_placespawn` drops every spawnpoint through the map's collision,
/// which is `vcod-common`'s; its two-decimal capture stays as evidence, and
/// `crates/server/tests/placespawn_ab.rs` gates the bit-exact
/// `client-probes/probe_placebits` on every stock map instead.
const RUN_IN_SERVER_CRATE: &[&str] = &[
    "probe_attach_badindex",
    "probe_attach_badtag",
    "probe_attach_slots",
    "probe_attach_twice",
    "probe_attach_twice_notag",
    "probe_bootstrap",
    "probe_cvar",
    "probe_delete",
    "probe_detach_missing",
    "probe_ents",
    "probe_hostname",
    "probe_late_attach",
    "probe_late_index",
    "probe_late_label",
    "probe_late_objective",
    "probe_late_player",
    "probe_late_setmodel",
    "probe_late_setshader",
    "probe_late_settext",
    "probe_late_shellshock",
    "probe_late_shockspan",
    "probe_late_statusicon",
    "probe_late_viewmodel",
    "probe_loadfx_wait",
    "probe_not_string",
    "probe_persist_exit",
    "probe_persist_exit_save",
    "probe_persist_restart",
    "probe_placespawn",
    "probe_precache_empty",
    "probe_precache_icon_full",
    "probe_precache_unknown_item",
    "probe_precache_wait",
    "probe_precache_wait_item",
    "probe_precache_wait_shader",
    "probe_precache_wait_shellshock",
    "probe_precache_wait_string",
    "probe_stale_ent_method",
    "probe_stale_ent_read",
    "probe_stale_ent_write",
    "probe_stale_handle",
    "probe_stale_hud_method",
    "probe_stale_hud_read",
    "probe_stale_hud_write",
    "probe_stale_self",
    "probe_stale_self_hud",
    "probe_testclient",
    "probe_vectordot",
];

#[test]
fn vcod_matches_retail_on_every_probe() {
    let mut failures = Vec::new();
    for (name, (retail_lines, retail_fatal)) in captures() {
        if RUN_IN_SERVER_CRATE.contains(&name.as_str()) {
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
        let retail_compile = retail_fatal.as_deref() == Some(COMPILE_ERROR);
        if retail_compile != (ours.error.as_deref() == Some(COMPILE_ERROR)) {
            failures.push(format!(
                "{name} ({}): one side failed to compile, the other did not",
                ours.path
            ));
        }
        // Where retail dies, vcod raises an error and aborts the thread
        // instead of the server. This only checks that both sides stopped,
        // not that they stopped in the same place or said the same thing:
        // the line comparison above is what catches a differing stopping
        // point, since a side that ran on emits the lines the other didn't.
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

/// A skip name that matches no capture section is inert, and an inert skip
/// silently stops covering the probe it was meant to exempt.
#[test]
fn every_skipped_probe_names_a_real_capture_section() {
    let names: Vec<String> = captures().keys().cloned().collect();
    for n in RUN_IN_SERVER_CRATE {
        assert!(
            names.iter().any(|c| c == n),
            "{n} is skipped but is not a capture section"
        );
    }
}

/// Every probe file has a capture section and every capture section has a
/// probe file: a probe added without a retail measurement, or a section left
/// behind by a deleted probe, is a gap in the A/B that nothing else notices.
/// A section this file skips into another crate has to be named by that
/// crate's test, or it is measured nowhere.
#[test]
fn every_probe_file_and_capture_section_are_paired() {
    // `captures()` is a BTreeMap, so its keys come out sorted.
    let sections: Vec<String> = captures().keys().cloned().collect();
    assert!(!sections.is_empty(), "retail-captures.txt has no sections");
    let mut files: Vec<String> = std::fs::read_dir("tests/fixtures/semantics")
        .expect("read the semantics fixture directory")
        .map(|e| e.expect("a directory entry").file_name())
        .filter_map(|n| n.to_string_lossy().strip_suffix(".gsc").map(str::to_string))
        .collect();
    files.sort();
    assert_eq!(sections, files, "probe files and capture sections differ");

    let server_test = std::fs::read_to_string("../server/tests/semantics_ents.rs")
        .expect("read the server crate's probe test");
    for n in RUN_IN_SERVER_CRATE {
        assert!(
            server_test.contains(n),
            "{n} is skipped here but the server crate's test never names it"
        );
    }
}
