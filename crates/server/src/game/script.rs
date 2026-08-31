//! The server's script runtime: owns the VM, resolves scripts out of the
//! paks, and steps threads once per server frame.

use std::rc::Rc;

use crate::game::host::GameHost;
use crate::game::spawn::spawn_entities_from_string;
use vcod_common::pk3::Pk3Fs;
use vcod_gsc::{Loader, ScriptSource, Vm};

/// Every gametype script includes this file, and it is where the engine's
/// entry points into script live, `CodeCallback_StartGameType` among them.
const CALLBACK_SETUP: &str = "maps/mp/gametypes/_callbacksetup";

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
    gametype_entry: String,
}

impl ScriptRuntime {
    /// Loads the gametype and map script closures, spawns the map's entities
    /// and runs the bootstrap. `map` and `gametype` are bare names, e.g.
    /// "mp_pavlov" and "dm". `configstrings` and `cvars` seed the host;
    /// `world`, once the caller has one, lets `bulletTrace` trace against the
    /// real map geometry.
    pub fn load(
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        world: Option<Rc<crate::world::World>>,
        now_ms: i32,
    ) -> anyhow::Result<ScriptRuntime> {
        Self::load_from(
            Box::new(PakScripts(fs.clone())),
            fs,
            map,
            gametype,
            configstrings,
            cvars,
            world,
            now_ms,
        )
    }

    /// `load` with the script source supplied. `fs` still resolves the map's
    /// BSP, so `source` only decides where `.gsc` text comes from: a test
    /// overlays one file on the paks and gets the real bootstrap rather than
    /// a hand-written copy of it that a change here would not reach.
    #[allow(clippy::too_many_arguments)]
    pub fn load_from(
        source: Box<dyn ScriptSource>,
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        configstrings: Vec<String>,
        cvars: crate::cvars::Cvars,
        world: Option<Rc<crate::world::World>>,
        now_ms: i32,
    ) -> anyhow::Result<ScriptRuntime> {
        let entry = format!("maps/mp/{map}");
        let gametype_entry = format!("maps/mp/gametypes/{gametype}");
        let mut vm = Vm::new();
        // One `Loader`, not two: its `loaded` set dedupes the files both
        // closures share, and `Vm::install` rejects a duplicate `FuncRef`, so
        // a second `Loader` would fail on the first shared file.
        let mut loader = Loader::new(source);
        loader
            .load(&mut vm, &gametype_entry)
            .map_err(|e| anyhow::anyhow!("loading {gametype_entry}: {e:?}"))?;
        loader
            .load(&mut vm, &entry)
            .map_err(|e| anyhow::anyhow!("loading {entry}: {e:?}"))?;

        let mut host = GameHost::new(configstrings);
        host.cvars = cvars;
        host.world = world;
        host.fs = Some(fs.clone());
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
                "gsc: {entry} and {gametype_entry} call {} builtins the host does not implement: {}",
                missing.len(),
                missing.join(" ")
            );
        }

        let mut rt = ScriptRuntime {
            vm,
            host,
            entry,
            gametype_entry,
        };
        rt.start_bootstrap(now_ms)?;
        Ok(rt)
    }

    /// The gametype's `main`, then the map's, then
    /// `CodeCallback_StartGameType`, whose own header in `_callbacksetup.gsc`
    /// reads "Called by code after the level's main script function has run".
    ///
    /// The order of the two `main`s is `probe_bootstrap`'s measurement, not an
    /// inference: `mp_pavlov.gsc` sets `game["allies"] = "russian"`, and the
    /// probe reads that key back as undefined from the gametype's `main` and
    /// as "russian" from `Callback_StartGameType`.
    ///
    /// `start_thread`, not `call_now`: retail runs these as script threads,
    /// and `call_now` turns any `wait` into `SuspendedInImmediateCall`.
    /// `start_thread` steps the new thread to its first suspend before
    /// returning, which the same probe measured, so the tables are complete
    /// when `load` returns.
    fn start_bootstrap(&mut self, now_ms: i32) -> anyhow::Result<()> {
        let gametype_entry = self.gametype_entry.clone();
        self.start(&gametype_entry, "main", now_ms)?;

        let entry = self.entry.clone();
        self.start(&entry, "main", now_ms)?;

        // `CodeCallback_StartGameType` is defined in `_callbacksetup`, not in
        // the gametype script; it guards on `level.gametypestarted` and then
        // calls `level.callbackStartGameType`.
        self.start(CALLBACK_SETUP, "CodeCallback_StartGameType", now_ms)
    }

    /// Starts one entry point as a thread, checking it is installed first.
    /// `Vm::start_thread` panics on a `FuncRef` that is not, and `--gametype`
    /// is user input, so a gametype script that loads but defines no `main`
    /// (or never pulls in `_callbacksetup`) has to fail the way a missing
    /// file does: an error `main.rs` exits on, not a panic.
    fn start(&mut self, path: &str, name: &str, now_ms: i32) -> anyhow::Result<()> {
        let f = self.vm.func_ref(path, name);
        if !self.vm.has_function(f) {
            anyhow::bail!("{path}.gsc defines no {name}()");
        }
        self.vm
            .start_thread(&mut self.host, now_ms, f, None, vec![]);
        Ok(())
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

    /// Every thread that has died of an error since map load, rendered
    /// `file::func:line: Kind`. The bootstrap starts its threads with
    /// `Vm::start_thread`, whose signature has no error channel, so this is
    /// the only way a caller sees an aborted bootstrap thread; `run_frame`
    /// logs the errors from its own pass and they land here too.
    ///
    /// The VM stops recording past its own cap, so a run that blew through
    /// it gets a final line saying how many are missing rather than a list
    /// that quietly claims to be complete.
    pub fn aborts(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .vm
            .aborts()
            .iter()
            .map(|e| self.vm.describe(e))
            .collect();
        let dropped = self.vm.abort_count() as usize - out.len();
        if dropped > 0 {
            out.push(format!("... and {dropped} more, past the record cap"));
        }
        out
    }

    /// Every line the script has passed to `logPrint`, in order. Retail
    /// writes these to `games_mp.log`, which is where the probe captures in
    /// `crates/gsc/tests/fixtures/semantics/` came from.
    pub fn script_log(&self) -> &[String] {
        &self.host.script_log
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
            gametype_entry: String::new(),
        };
        let main = rt.vm.func_ref(&rt.entry, "main");
        rt.vm.start_thread(&mut rt.host, 0, main, None, vec![]);
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

    /// Both closures load into one `Vm` through one `Loader`, which is what
    /// keeps `Vm::install`'s duplicate rejection from firing on the first
    /// file the two share (`_utility`, `_teams` and the rest).
    #[test]
    fn the_map_and_gametype_closures_both_load() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_pavlov",
            "dm",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            0,
        );
        assert!(rt.is_ok(), "{:?}", rt.err());
    }

    /// `mp_pavlov.gsc` sets `game["allies"] = "russian"`, and dm's
    /// `Callback_StartGameType` turns that into `team_russiangerman`, so
    /// this pins that the map's `main` ran before the code callback and that
    /// all three precache families reached the table. It does **not** pin
    /// the order of the two `main`s: `dm.gsc`'s own `main` never reads
    /// `game["allies"]` and its `Callback_StartGameType` defaults the key
    /// when unset, so 1180 reads the same either way. That order is pinned
    /// by `probe_bootstrap_matches_retail` in
    /// `crates/server/tests/semantics_ents.rs`, which drives this same
    /// bootstrap with the probe as its gametype.
    #[test]
    fn the_bootstrap_precaches_the_russian_team_menu_on_mp_pavlov() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let rt = ScriptRuntime::load(
            Rc::new(fs),
            "mp_pavlov",
            "dm",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            0,
        )
        .expect("load mp_pavlov on dm");
        assert_eq!(rt.configstrings()[1180], "team_russiangerman");
        assert_eq!(rt.configstrings()[1181], "weapon_russian");
        assert_eq!(rt.configstrings()[21], "gfx/hud/hud@status_dead.tga");
        assert_eq!(
            rt.configstrings()[1501],
            "levelshots/layouts/hud@layout_mp_pavlov"
        );
    }

    /// A gametype script that resolves but defines no `main` reached
    /// `Vm::start_thread`'s "target must be installed" panic. `--gametype` is
    /// user input, so it has to come back as an error `main.rs` exits on,
    /// same as a gametype whose file does not exist at all.
    #[test]
    fn a_gametype_without_a_main_is_an_error_not_a_panic() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        /// The paks with one gametype path answered by a script that has
        /// every part of a gametype except the entry point.
        struct NoMain(Rc<Pk3Fs>);
        impl ScriptSource for NoMain {
            fn read(&self, canonical: &str) -> Option<String> {
                if canonical == "maps/mp/gametypes/nomain" {
                    return Some("notMain() {}\n".to_string());
                }
                PakScripts(self.0.clone()).read(canonical)
            }
        }
        let fs = Rc::new(fs);
        let err = ScriptRuntime::load_from(
            Box::new(NoMain(fs.clone())),
            fs,
            "mp_pavlov",
            "nomain",
            vec![String::new(); 2048],
            crate::cvars::Cvars::new(),
            None,
            0,
        );
        let Err(err) = err else {
            panic!("a gametype with no main must not load");
        };
        assert!(
            err.to_string().contains("defines no main()"),
            "unexpected error: {err:#}"
        );
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
