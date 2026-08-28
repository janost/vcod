//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::rc::Rc;

use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{Host, Loader, ScriptSource, Vm};

/// Reads `.gsc` out of the mounted paks. `Loader` hands `read` a canonical
/// path (lowercase, forward slashes, no extension; see `vcod_gsc::canonical`);
/// the pak stores e.g. `maps/MP/_load.gsc`, and `Pk3Fs::read` already
/// lowercases its lookup key, so appending `.gsc` is the only work here.
struct PakScripts(Rc<Pk3Fs>);

impl ScriptSource for PakScripts {
    fn read(&self, canonical: &str) -> Option<String> {
        let bytes = self.0.read(&format!("{canonical}.gsc"))?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Owns one map's loaded script closure and the `Vm` it lives in.
///
/// Always a fresh `Vm` per map, never a reload into a live one:
/// `Vm::install` rejects a duplicate `FuncRef` rather than silently
/// overwriting, so re-loading a shared gametype script into a `Vm` that
/// already has it would fail loudly. Every stock gametype references the
/// same shared scripts, so a map change that reused one long-lived `Vm`
/// would hit this on the first shared file. A map change therefore builds
/// a new `ScriptRuntime` and drops the old one; the heap goes with it,
/// which is also what the VM's no-garbage-collection design assumes.
pub struct ScriptRuntime {
    vm: Vm,
    entry: String,
}

impl ScriptRuntime {
    /// Loads the map script's closure. `map` is a bare name, e.g. "mp_pavlov".
    pub fn load(fs: Rc<Pk3Fs>, map: &str) -> anyhow::Result<ScriptRuntime> {
        let entry = format!("maps/mp/{map}");
        let mut vm = Vm::new();
        let mut loader = Loader::new(Box::new(PakScripts(fs)));
        loader
            .load(&mut vm, &entry)
            .map_err(|e| anyhow::anyhow!("loading {entry}: {e:?}"))?;

        // The closure calls far more builtins than the host answers yet.
        // Listing them once here beats discovering them one aborted thread
        // at a time, and a missing builtin must not stop the map serving:
        // a thread that reaches one dies, the rest of the script runs.
        let missing = loader.missing_builtins(&vm, &|n| crate::game::host::is_builtin(n));
        if !missing.is_empty() {
            log::info!(
                "gsc: {entry} calls {} builtins the host does not implement: {}",
                missing.len(),
                missing.join(" ")
            );
        }
        Ok(ScriptRuntime { vm, entry })
    }

    /// Starts `maps/mp/<map>::main` as a thread.
    pub fn start_map_main(&mut self, host: &mut dyn Host, now_ms: i32) {
        let main = self.vm.func_ref(&self.entry, "main");
        self.vm.start_thread(host, now_ms, main, None, vec![]);
    }

    /// One server frame of script.
    pub fn run_frame(&mut self, host: &mut dyn Host, now_ms: i32) {
        for e in self.vm.run_frame(host, now_ms) {
            log::warn!("script error: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map script's closure resolves out of the stock paks and `main`
    /// starts. Its body calls builtins the host does not implement yet, so
    /// this asserts only that loading and starting work, not that the
    /// thread survives.
    #[test]
    fn the_map_script_closure_loads_from_the_paks() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let rt = ScriptRuntime::load(Rc::new(fs), "mp_pavlov");
        assert!(rt.is_ok(), "{:?}", rt.err());
    }
}
