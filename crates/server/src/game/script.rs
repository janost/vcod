//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::rc::Rc;

use crate::game::host::GameHost;
use crate::game::spawn::spawn_entities_from_string;
use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{Loader, ScriptSource, Vm};

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

/// Owns one map's loaded script closure, the `Vm` it lives in, and the
/// `GameHost` the object table and configstrings live in. The host has to
/// outlive map load: a thread can spawn an entity in one frame and read it
/// back in the next, so a host that died at the end of `load` would throw
/// the object table away under it.
///
/// Always a fresh `Vm`/`GameHost` per map, never a reload into a live one:
/// `Vm::install` rejects a duplicate `FuncRef` rather than silently
/// overwriting, so re-loading a shared gametype script into a `Vm` that
/// already has it would fail loudly. Every stock gametype references the
/// same shared scripts, so a map change that reused one long-lived `Vm`
/// would hit this on the first shared file. A map change therefore builds
/// a new `ScriptRuntime` and drops the old one; the heap goes with it,
/// which is also what the VM's no-garbage-collection design assumes.
pub struct ScriptRuntime {
    vm: Vm,
    host: GameHost,
    entry: String,
}

impl ScriptRuntime {
    /// Loads the map script's closure, spawns the map's entities and starts
    /// `main`. `map` is a bare name, e.g. "mp_pavlov". `configstrings` and
    /// `cvars` seed the host; `world`, once the caller has one, lets
    /// `bulletTrace` trace against the real map geometry.
    pub fn load(
        fs: Rc<Pk3Fs>,
        map: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        world: Option<Rc<crate::world::World>>,
        now_ms: i32,
    ) -> anyhow::Result<ScriptRuntime> {
        let entry = format!("maps/mp/{map}");
        let mut vm = Vm::new();
        let mut loader = Loader::new(Box::new(PakScripts(fs.clone())));
        loader
            .load(&mut vm, &entry)
            .map_err(|e| anyhow::anyhow!("loading {entry}: {e:?}"))?;

        let mut host = GameHost::new(configstrings);
        host.cvars = cvars;
        host.world = world;
        host.level_time_ms = now_ms;

        // `_load.gsc::main`, in mp_pavlov's own closure, calls `getEntArray`
        // in its first statements, so the object table must hold every map
        // entity before `main` runs: the entity lump goes in first, then the
        // pre-scan (which only reads the loader, not the table), and only
        // then does `main` start.
        let bsp_path = fs
            .resolve_map(map)
            .ok_or_else(|| anyhow::anyhow!("map {map} not found in the mounted paks"))?;
        let bsp_bytes = fs
            .read(&bsp_path)
            .ok_or_else(|| anyhow::anyhow!("reading {bsp_path}"))?;
        let bsp = vcod_common::bsp::parse(&bsp_bytes)?;
        vm.with_cx(|cx| spawn_entities_from_string(&mut host, cx, &bsp.entities))
            .map_err(|e| anyhow::anyhow!("spawning {map}'s entities: {e:?}"))?;

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

        let mut rt = ScriptRuntime { vm, host, entry };
        rt.start_map_main(now_ms);
        Ok(rt)
    }

    /// Starts `maps/mp/<map>::main` as a thread. Internal: called once by
    /// `load` (and by `for_test`'s own compile-and-start), after the object
    /// table exists.
    fn start_map_main(&mut self, now_ms: i32) {
        let main = self.vm.func_ref(&self.entry, "main");
        self.vm
            .start_thread(&mut self.host, now_ms, main, None, vec![]);
    }

    /// The configstrings the script wrote (e.g. `setCullFog`, `ambientPlay`)
    /// by the end of `load`. `Server::load_scripts` copies these back into
    /// its own table once, before the first gamestate goes out.
    pub fn configstrings(&self) -> &[String] {
        &self.host.configstrings
    }

    /// The cvar table as the script left it. `Server::tick` reads it back
    /// every frame for the same reason it re-reads the configstrings: a
    /// thread past a `wait` can still call `setCvar`.
    pub fn cvars(&self) -> &crate::cvars::Cvars {
        &self.host.cvars
    }

    /// One server frame of script.
    pub fn run_frame(&mut self, now_ms: i32) {
        self.host.level_time_ms = now_ms;
        for e in self.vm.run_frame(&mut self.host, now_ms) {
            log::warn!("script error: {e:?}");
        }
        // Stage 6 drains `damage` into the damage callback; until then, drop
        // it here each frame so a long-running map's radiusDamage calls
        // don't grow the queue without bound.
        if !self.host.damage.is_empty() {
            log::debug!(
                "gsc: dropping {} queued damage event(s), the damage callback is not wired yet",
                self.host.damage.len()
            );
            self.host.damage.clear();
        }
        // Stage 6 ends the map on this; until then, log once and clear it
        // so a script that called exitLevel does not spam every frame after.
        if self.host.exit_level {
            log::debug!("gsc: exitLevel() called, ending the map is not wired yet");
            self.host.exit_level = false;
        }
    }
}

#[cfg(test)]
impl ScriptRuntime {
    /// Compiles `src` as `maps/mp/test`, builds a host with an empty object
    /// table (no paks, no bsp, no missing-builtin pre-scan), and starts
    /// `main`. For this module's own tests.
    pub fn for_test(src: &str) -> ScriptRuntime {
        let mut vm = Vm::new();
        let ast = vcod_gsc::parse::parse_file(src).expect("test script parses");
        let fns = vcod_gsc::compile::compile_file(&ast, "maps/mp/test", vm.interner_mut())
            .expect("test script compiles");
        vm.install(fns).expect("test script installs");
        let host = GameHost::new(vec![String::new(); 2048]);
        let mut rt = ScriptRuntime {
            vm,
            host,
            entry: "maps/mp/test".to_string(),
        };
        rt.start_map_main(0);
        rt
    }

    /// Reads a folded field off `level`.
    pub fn level_field(&mut self, name: &str) -> vcod_gsc::Value {
        let level = self.vm.level_id();
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            cx.get_field(level, atom)
        })
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
        let rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_pavlov",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            0,
        );
        assert!(rt.is_ok(), "{:?}", rt.err());
    }

    /// The object table has to survive past map load, so the runtime owns the
    /// host. A thread that spawns an entity in one frame must find it in the
    /// next.
    #[test]
    fn an_entity_spawned_in_one_frame_survives_into_the_next() {
        let mut rt = ScriptRuntime::for_test(
            "main() { level.e = spawn(\"script_origin\", (0,0,0)); wait 0.05; \
             level.n = level.e getEntityNumber(); }",
        );
        rt.run_frame(0);
        rt.run_frame(100);
        let n = rt.level_field("n");
        assert_eq!(n, vcod_gsc::Value::Int(72));
    }
}
