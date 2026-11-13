//! Every stock CoD4 1.7 MP map under every stock gametype, end to end:
//! per map, vcod-server loads it on a thread of the test with six
//! shooting bots and `sv_mapRotation` naming the map once per gametype
//! (`war` first, `war` again last), and one in-process client joins
//! through the stock menus and plays it through [`Cod4Client::pump`] (the
//! live path: session, prediction, HUD, events, sounds and effects) into
//! a headless renderer. Each gametype runs to its 30 s time limit,
//! intermission and `map_rotate`.
//!
//! Per (map, gametype) it records: whether the level loaded, every
//! script error (`file::func:line`, counted), whether the match ended
//! (`game["state"]` `postgame`) and the server rotated on its own, the
//! bots' stalled bot-ticks (bot_match's rule: 12 s inside 150 units and
//! not holding on purpose), whether the client spawned, the client's and
//! the server's own warnings, and a panic. Writes `summary.md` (the
//! table, then every distinct error and warning ranked by count) and
//! `warnings.txt` under `VCOD_SWEEP_OUT` (default `tmp/cod4-map-sweep`).
//!
//! `COD4_DIR=... cargo test -p vcod --bins -- --ignored cod4_map_sweep
//! --nocapture`, run alone (it installs the process's logger). Under
//! lavapipe set `VK_DRIVER_FILES`, `WGPU_BACKEND=vulkan`,
//! `GALLIVM_PERF=nopt`; without an adapter the client steps its session
//! only. `VCOD_SWEEP_MAPS=mp_a,mp_b`, `VCOD_SWEEP_GAMETYPES=war,dm`,
//! `VCOD_SWEEP_JOBS` (maps at once, 3; one map a process keeps a run near 3 GB), `VCOD_SWEEP_LEVEL_SECS` (the
//! wall seconds a level gets before the sweep forces `map_rotate`, 120).
//! Each job is a server thread, a client thread and the renderer's; keep
//! the whole run inside eight cores with `taskset -c 0-7`, which also
//! caps the nav build's workers (`available_parallelism`).
//!
//! The `cod4_live_*` tests at the end share the sweep's [`rig`] and run
//! in the suite on `run_held`'s clock (docs/dev/testing.md, CoD4 live
//! tests).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::Vec3;

use super::cod4::Cod4Session;
use super::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
use super::cod4_menus::Join as MenuJoin;
use crate::play::input::Action;
use vcod_cod4::net::conn::Cp;

const STOCK_GAMETYPES: [&str; 6] = ["war", "dm", "sd", "sab", "dom", "koth"];
pub(super) const BOTS: usize = 6;
/// bot_match's stall rule: a window of 240 50 ms ticks inside 150 units.
const STALL_WINDOW: usize = 240;
const STALL_RADIUS: f32 = 150.0;

/// Every warning and error logged: the logging thread's name and the line.
pub(super) static LOG: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Each map's gametype under way, by map, so a line lands on its pair.
static CURRENT: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// The soak (`cod4_soak`) also keeps vcod's own debug lines that name a
/// problem (an unknown alias, a missing tag, a menu item not found).
pub(super) static NOTES: AtomicBool = AtomicBool::new(false);

struct Collect;

impl log::Log for Collect {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
            || (NOTES.load(Ordering::Relaxed)
                && m.level() <= log::Level::Debug
                && m.target().starts_with("vcod"))
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{} {}: {}", r.level(), r.target(), r.args());
        if r.level() > log::Level::Warn && !names_a_problem(&r.args().to_string()) {
            return;
        }
        let thread = std::thread::current().name().unwrap_or("").to_string();
        LOG.lock().unwrap().push((thread, line));
    }
    fn flush(&self) {}
}

/// A debug or info line the soak files as a note. A server command's
/// echo names its strings (`mc_mission_fail`), a game log line its
/// players, not a problem.
fn names_a_problem(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    // A server command's echo, a chat line, or a count of none.
    if l.contains("server command")
        || l.contains(" cmd ")
        || l.contains("script: ")
        || l.contains("errors 0")
        || l.contains(" 0 errors")
    {
        return false;
    }
    [
        "unknown",
        "missing",
        "not found",
        "no such",
        "fail",
        "error",
        "invalid",
        "cannot",
        "not in the",
        "no alias",
    ]
    .iter()
    .any(|k| l.contains(k))
}
static COLLECT: Collect = Collect;

/// One (map, gametype).
#[derive(Default, Clone)]
pub(super) struct Pair {
    pub(super) loaded: bool,
    /// `postgame` seen.
    pub(super) ended: bool,
    /// The next level loaded without the sweep forcing it.
    pub(super) rotated: bool,
    pub(super) forced: bool,
    pub(super) secs: f32,
    /// Script errors by place, and how many went past the VM's record cap.
    pub(super) errors: BTreeMap<String, u32>,
    pub(super) errors_past_cap: u32,
    pub(super) playing: u64,
    pub(super) stalled: u64,
    pub(super) client_spawned: bool,
    pub(super) client_warn: BTreeMap<String, u32>,
    pub(super) server_warn: BTreeMap<String, u32>,
    pub(super) panics: Vec<String>,
    pub(super) soak: SoakPair,
}

/// What the soak adds to a pair.
#[derive(Default, Clone)]
pub(super) struct SoakPair {
    /// The debug lines that name a problem ([`names_a_problem`]).
    pub(super) client_notes: BTreeMap<String, u32>,
    pub(super) server_notes: BTreeMap<String, u32>,
    /// wgpu validation errors, by text.
    pub(super) wgpu: BTreeMap<String, u32>,
    pub(super) spawns: u32,
    pub(super) deaths: u32,
    pub(super) frags: u32,
    pub(super) scores: u32,
    pub(super) menus: u32,
    /// Nav paths set out on, and the waypoints reached.
    pub(super) paths: u32,
    pub(super) reached: u32,
    pub(super) frames: u64,
    pub(super) renders: u64,
    /// Client frames whose CPU side passed [`STALL_MS`], by the part that
    /// took longest, and the worst.
    pub(super) stalls: BTreeMap<String, u32>,
    pub(super) worst_ms: f32,
    /// The renderer's own `render` past [`STALL_MS`] (lavapipe: slow by
    /// nature, so reported apart).
    pub(super) render_stalls: u32,
    /// Audio cues whose alias is not in the table (the soak's audio has
    /// no output, so nothing counts as played).
    pub(super) audio_miss: u64,
    /// Server ticks past [`STALL_MS`] (a load excluded), those of them
    /// past it on a CPU, the worst, and the seconds into the level it came.
    pub(super) server_stalls: u32,
    pub(super) server_cpu_stalls: u32,
    pub(super) server_worst_ms: f32,
    pub(super) server_worst_at: f32,
    /// Frames past the level's first 5 s that built a mark material or
    /// loaded an fx image (first use in play), and the worst such frame.
    pub(super) builds: u32,
    pub(super) build_worst_ms: f32,
    /// The process's resident set at the pair's first and last sample, MB.
    pub(super) rss: Option<(u64, u64)>,
}

/// A frame or tick past this is a stall a player sees.
pub(super) const STALL_MS: f32 = 100.0;

/// The process's start, which the tick and frame lines past
/// `VCOD_SOAK_TICK_LOG_MS` stamp, so a server tick lines up with the
/// client frame beside it.
static RUN_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// `VCOD_SOAK_CLIENT_LOG_MS`: each client frame whose CPU side or render
/// passed it goes to the log with its parts (off by default).
static CLIENT_LOG_MS: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
    std::env::var("VCOD_SOAK_CLIENT_LOG_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(f32::INFINITY)
});

/// The run's knobs: the sweep's, and the soak's on top.
#[derive(Clone, Copy)]
pub(super) struct Opts {
    /// The wall seconds a level gets before the run forces `map_rotate`.
    pub(super) level_secs: u64,
    pub(super) soak: Option<SoakOpts>,
}

#[derive(Clone, Copy)]
pub(super) struct SoakOpts {
    /// The level's time limit in minutes.
    pub(super) minutes: f32,
    /// Renders a second (`VCOD_SOAK_RENDER_HZ`): the rest of the frame runs
    /// every frame.
    pub(super) render_hz: f32,
}

/// The process's resident set in MB.
pub(super) fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096 / (1 << 20))
}

/// The bytes malloc holds for live allocations, MB (glibc's `mallinfo2`;
/// 0 elsewhere): set against [`rss_mb`], what the allocator keeps free.
pub(super) fn heap_mb() -> u64 {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        #[repr(C)]
        struct Mallinfo2 {
            arena: usize,
            ordblks: usize,
            smblks: usize,
            hblks: usize,
            hblkhd: usize,
            usmblks: usize,
            fsmblks: usize,
            uordblks: usize,
            fordblks: usize,
            keepcost: usize,
        }
        unsafe extern "C" {
            fn mallinfo2() -> Mallinfo2;
        }
        // SAFETY: glibc's mallinfo2 takes nothing and returns by value.
        let m = unsafe { mallinfo2() };
        ((m.uordblks + m.hblkhd) / (1 << 20)) as u64
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    0
}

/// A jemalloc heap profile (`prof.dump`) when the process runs under
/// `LD_PRELOAD` jemalloc with `MALLOC_CONF=prof:true`; nothing otherwise.
/// The soak takes one at each level's load, for `jeprof --base`.
fn jemalloc_dump() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        type Mallctl = unsafe extern "C" fn(
            *const std::ffi::c_char,
            *mut std::ffi::c_void,
            *mut usize,
            *mut std::ffi::c_void,
            usize,
        ) -> i32;
        unsafe extern "C" {
            fn dlsym(
                handle: *mut std::ffi::c_void,
                name: *const std::ffi::c_char,
            ) -> *mut std::ffi::c_void;
        }
        // SAFETY: RTLD_DEFAULT (null) lookup of a C symbol; when present it
        // is jemalloc's `mallctl`, and "prof.dump" with no old or new value
        // writes the profile to `prof_prefix`, failing harmlessly without prof.
        unsafe {
            let f = dlsym(std::ptr::null_mut(), c"mallctl".as_ptr());
            if !f.is_null() {
                let mallctl: Mallctl = std::mem::transmute(f);
                mallctl(
                    c"prof.dump".as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                );
            }
        }
    }
}

/// A warning with its numbers taken out, so repeats of one cause count
/// as one row.
pub(super) fn shape(line: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

/// The stock MP maps: every `mp_*.ff` but the `_load` screens.
fn stock_maps(install: &vcod_cod4::install::Install) -> Vec<String> {
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    maps.sort();
    maps
}

pub(super) fn env_list(key: &str) -> Option<Vec<String>> {
    let v = std::env::var(key).ok()?;
    Some(v.split(',').map(|s| s.trim().to_string()).collect())
}

pub(super) fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "minutes per map; COD4_DIR"]
fn cod4_map_sweep() {
    let Some(game) = Cod4::new() else {
        return;
    };
    let opts = Opts {
        level_secs: env_num("VCOD_SWEEP_LEVEL_SECS", 120),
        soak: None,
    };
    run(&game, opts, &STOCK_GAMETYPES, "cod4-map-sweep");
}

/// The soak: the sweep's server and client, with the client playing as a
/// player would (`play_soak`) through `main`'s whole frame, render and HUD
/// included, for `VCOD_SOAK_MINUTES` (3) a level under war, dm, sd, koth
/// and sab. On top of the sweep's columns it counts what the player did
/// and every wgpu validation error, problem debug line (unknown alias,
/// missing tag, menu item not found), audio miss, stalled frame and
/// server tick past 100 ms (each to the log with its phases, as is every
/// tick past `VCOD_SOAK_TICK_LOG_MS`; `VCOD_SOAK_TICK_WATCH=1` adds what
/// it slept on, `VCOD_SOAK_CLIENT_LOG_MS` logs the client's slow frames
/// beside them), and the resident set over each level. Writes
/// `soak.md` under `VCOD_SWEEP_OUT` (default `tmp/cod4-soak`).
/// `tools/cod4_soak.sh` runs it a map a process.
#[test]
#[ignore = "15 to 20 minutes per map; COD4_DIR"]
fn cod4_soak() {
    let Some(game) = Cod4::new() else {
        return;
    };
    soak(&game, &SOAK_GAMETYPES, "cod4-soak");
}

const SOAK_GAMETYPES: [&str; 5] = ["war", "dm", "sd", "koth", "sab"];

/// The soak's knobs off the environment (`VCOD_SOAK_MINUTES`,
/// `VCOD_SOAK_RENDER_HZ`, `VCOD_SWEEP_LEVEL_SECS`), then [`run`].
pub(super) fn soak(game: &dyn Game, gametypes: &[&str], out_dir: &str) {
    let minutes = std::env::var("VCOD_SOAK_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.0f32);
    let opts = Opts {
        // The prematch, a `sab` overtime and the intermission on top.
        level_secs: env_num("VCOD_SWEEP_LEVEL_SECS", (minutes * 60.0) as u64 + 150),
        soak: Some(SoakOpts {
            minutes,
            render_hz: std::env::var("VCOD_SOAK_RENDER_HZ")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10.0),
        }),
    };
    NOTES.store(true, Ordering::Relaxed);
    std::sync::LazyLock::force(&RUN_START);
    run(game, opts, gametypes, out_dir);
}

/// A game the sweep and the soak run: its stock maps, its server loaded
/// with the bots in, and its client's side.
pub(super) trait Game: Sync {
    /// The stock MP maps.
    fn stock_maps(&self) -> Vec<String>;
    /// The server on `map` under `gametypes[0]`, every gametype in
    /// `sv_mapRotation` once (the first again last) at the run's time
    /// limit, loaded, [`BOTS`] shooting bots in and their graph built:
    /// the server and the bots' slots.
    fn start(
        &self,
        map: &str,
        gametypes: &[String],
        opts: Opts,
    ) -> anyhow::Result<(Box<dyn SoakServer>, Vec<usize>)>;
    /// The client's side on `addr` until `shared.done`; `Err` when the
    /// join itself fails.
    fn play(&self, map: &str, addr: SocketAddr, shared: &Shared, opts: Opts) -> anyhow::Result<()>;
}

/// The server as the run drives it from its own thread.
pub(super) trait SoakServer {
    fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant);
    fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)>;
    /// One server frame: the console's lines, then the frame.
    fn tick(&mut self, now: Instant);
    fn frame_ms(&self) -> u64;
    /// `sv_serverid`: its high nibble steps on a map load.
    fn server_id(&self) -> i32;
    fn gametype(&self) -> String;
    /// The bots' nav graph once built.
    fn graph(&mut self) -> Option<Arc<vcod_server::nav::NavGraph>>;
    /// The level's state, `None` with no level up.
    fn sample(&mut self, slots: &[usize]) -> Option<Sample>;
    /// `map_rotate` queued on the console.
    fn rotate(&mut self);
}

/// One look at the level, once a second.
pub(super) struct Sample {
    /// The match's state as the log names it.
    pub(super) state: String,
    /// The match is over (CoD4's `postgame`, CoD2's `level.mapended`).
    pub(super) ended: bool,
    /// The VM's abort list (`... and N more` past its cap).
    pub(super) errors: Vec<String>,
    /// Each bot slot: its origin while it plays, and whether it holds
    /// still on purpose (engaged, in cover, retreating, on an objective).
    pub(super) bodies: Vec<(usize, Option<Vec3>, bool)>,
}

fn run(game: &dyn Game, opts: Opts, default_gametypes: &[&str], out_dir: &str) {
    if log::set_logger(&COLLECT).is_ok() {
        log::set_max_level(match opts.soak {
            Some(_) => log::LevelFilter::Debug,
            None => log::LevelFilter::Warn,
        });
    }
    let maps = env_list("VCOD_SWEEP_MAPS").unwrap_or_else(|| game.stock_maps());
    let gametypes = env_list("VCOD_SWEEP_GAMETYPES")
        .unwrap_or_else(|| default_gametypes.iter().map(|g| g.to_string()).collect());
    let jobs = env_num("VCOD_SWEEP_JOBS", 3).max(1) as usize;
    let out = std::env::var_os("VCOD_SWEEP_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tmp")
                .join(out_dir)
        });
    std::fs::create_dir_all(&out).unwrap();
    let started = Instant::now();
    let queue = Mutex::new(maps.iter().rev().cloned().collect::<Vec<_>>());
    let results: Mutex<BTreeMap<String, BTreeMap<String, Pair>>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|sc| {
        for _ in 0..jobs {
            sc.spawn(|| {
                loop {
                    let Some(map) = queue.lock().unwrap().pop() else {
                        return;
                    };
                    let pairs = run_map(game, &map, &gametypes, opts);
                    eprintln!(
                        "sweep: {map} done at {:.0} s",
                        started.elapsed().as_secs_f32()
                    );
                    results.lock().unwrap().insert(map, pairs);
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    let report = report(&results, &gametypes);
    let (name, report) = match opts.soak {
        Some(_) => (
            "soak.md",
            format!("{}\n{report}", soak_report(&results, &gametypes)),
        ),
        None => ("summary.md", report),
    };
    println!("{report}");
    std::fs::write(out.join(name), &report).unwrap();
    let mut all = String::new();
    for (map, pairs) in &results {
        for (g, p) in pairs {
            for (w, n) in p.client_warn.iter().chain(&p.server_warn) {
                let _ = writeln!(all, "{map} {g} {n}x {w}");
            }
            for (w, n) in p.soak.client_notes.iter().chain(&p.soak.server_notes) {
                let _ = writeln!(all, "{map} {g} {n}x note {w}");
            }
            for (w, n) in &p.soak.wgpu {
                let _ = writeln!(all, "{map} {g} {n}x wgpu {w}");
            }
            for (e, n) in &p.errors {
                let _ = writeln!(all, "{map} {g} {n}x script {e}");
            }
        }
    }
    std::fs::write(out.join("warnings.txt"), all).unwrap();
}

/// What the server thread tells the client thread.
pub(super) struct Shared {
    /// The pairs, filled by both threads.
    pub(super) pairs: Mutex<BTreeMap<String, Pair>>,
    /// The server finished its rotation (or gave up): the client leaves.
    pub(super) done: AtomicBool,
    /// The client left: the server stops.
    pub(super) client_gone: AtomicBool,
    /// The bots' nav graph once built, which the soak's player walks.
    pub(super) graph: Mutex<Option<Arc<vcod_server::nav::NavGraph>>>,
}

/// One map: the server and the client on their own threads, the log's
/// lines filed by the gametype under way.
fn run_map(game: &dyn Game, map: &str, gametypes: &[String], opts: Opts) -> BTreeMap<String, Pair> {
    let shared = Shared {
        pairs: Mutex::new(BTreeMap::new()),
        done: AtomicBool::new(false),
        client_gone: AtomicBool::new(false),
        graph: Mutex::new(None),
    };
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let shared = &shared;
    let (sv, cl) = std::thread::scope(|sc| {
        let sv = std::thread::Builder::new()
            .name(format!("sweep-sv-{map}"))
            .spawn_scoped(sc, move || {
                serve(game, map, gametypes, opts, shared, addr_tx)
            })
            .unwrap();
        let addr = addr_rx.recv_timeout(Duration::from_secs(600));
        let cl = addr.ok().map(|addr| {
            std::thread::Builder::new()
                .name(format!("sweep-cl-{map}"))
                .spawn_scoped(sc, move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        game.play(map, addr, shared, opts)
                    }));
                    shared.client_gone.store(true, Ordering::Relaxed);
                    r
                })
                .unwrap()
        });
        (sv.join(), cl.map(|h| h.join()))
    });
    let current = CURRENT
        .lock()
        .unwrap()
        .get(map)
        .cloned()
        .unwrap_or_default();
    let mut pairs = std::mem::take(&mut *shared.pairs.lock().unwrap());
    let panic_text = |e: Box<dyn std::any::Any + Send>| {
        e.downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "panic".into())
    };
    if let Err(e) = sv {
        let t = panic_text(e);
        pairs
            .entry(current.clone())
            .or_default()
            .panics
            .push(format!("server: {t}"));
    }
    match cl {
        Some(Ok(Err(e))) | Some(Err(e)) => {
            let t = panic_text(e);
            pairs
                .entry(current.clone())
                .or_default()
                .panics
                .push(format!("client: {t}"));
        }
        Some(Ok(Ok(Err(e)))) => {
            pairs
                .entry(current.clone())
                .or_default()
                .panics
                .push(format!("client error: {e:#}"));
        }
        _ => {}
    }
    // The log's lines, by thread and the pair under way when the line was
    // filed: lines carry no pair, so `serve` files its pair change as a
    // marker line.
    let (sv_name, cl_name) = (format!("sweep-sv-{map}"), format!("sweep-cl-{map}"));
    let mut log = LOG.lock().unwrap();
    let mut gametype = String::new();
    log.retain(|(thread, line)| {
        if thread == &sv_name
            && let Some(g) = line.strip_prefix("MARK ")
        {
            gametype = g.to_string();
            return false;
        }
        let side = if thread == &sv_name {
            false
        } else if thread == &cl_name {
            true
        } else {
            return true;
        };
        let p = pairs.entry(gametype.clone()).or_default();
        // The VM's lines are counted off its abort list.
        if line.contains("gsc: thread aborted") || line.contains("gsc: script error") {
            return false;
        }
        let note = !(line.starts_with("WARN") || line.starts_with("ERROR"));
        let w = match (side, note) {
            (true, false) => &mut p.client_warn,
            (false, false) => &mut p.server_warn,
            (true, true) => &mut p.soak.client_notes,
            (false, true) => &mut p.soak.server_notes,
        };
        *w.entry(shape(line)).or_default() += 1;
        false
    });
    pairs
}

/// Files a marker line the log split reads: the server thread's pair
/// changed to `gametype`, from the log's line `at` on (the end without
/// one). A level's load logs before the sweep sees it loaded: its lines
/// (a spawn in solid) belong to the new pair, not the last.
fn mark(map: &str, gametype: &str, at: Option<usize>) {
    CURRENT
        .lock()
        .unwrap()
        .insert(map.to_string(), gametype.to_string());
    let thread = std::thread::current().name().unwrap_or("").to_string();
    let mut log = LOG.lock().unwrap();
    let at = at.unwrap_or(log.len()).min(log.len());
    log.insert(at, (thread, format!("MARK {gametype}")));
}

/// The server's side: load `map` under the first gametype with the bots
/// in, run every gametype in rotation until it comes back to the first.
fn serve(
    game: &dyn Game,
    map: &str,
    gametypes: &[String],
    opts: Opts,
    shared: &Shared,
    addr_tx: std::sync::mpsc::Sender<SocketAddr>,
) {
    let first = gametypes[0].clone();
    mark(map, &first, None);
    let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        game.start(map, gametypes, opts)
    }));
    let (mut sv, slots) = match started {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let mut pairs = shared.pairs.lock().unwrap();
            let p = pairs.entry(first.clone()).or_default();
            p.panics.push(format!("load: {e:#}"));
            return;
        }
        Err(e) => std::panic::resume_unwind(e),
    };
    *shared.graph.lock().unwrap() = sv.graph();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    addr_tx.send(sock.local_addr().unwrap()).unwrap();

    let mut gametype = first.clone();
    let mut level_id = sv.server_id();
    let mut level_start = Instant::now();
    let mut ended_at: Option<Instant> = None;
    let mut forced = false;
    let mut levels = 1;
    let mut hist: HashMap<usize, Vec<Vec3>> = HashMap::new();
    let mut tick = 0u64;
    shared
        .pairs
        .lock()
        .unwrap()
        .entry(gametype.clone())
        .or_default()
        .loaded = true;
    let mut next = Instant::now();
    let mut buf = [0u8; 4096];
    let level_secs = opts.level_secs;
    // Each tick past `VCOD_SOAK_TICK_LOG_MS` (the stall line) goes to the
    // log with its phases (`vcod_server::tick_profile`).
    let tick_log_ms = std::env::var("VCOD_SOAK_TICK_LOG_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(STALL_MS);
    if opts.soak.is_some() {
        vcod_server::tick_profile::enable();
    }
    // `VCOD_SOAK_TICK_WATCH=1`: what a logged tick slept on, and what the
    // process's other threads did meanwhile (`tick_profile::Watch`).
    let watch = (opts.soak.is_some()
        && std::env::var("VCOD_SOAK_TICK_WATCH").as_deref() == Ok("1"))
    .then(vcod_server::tick_profile::Watch::start)
    .flatten();
    let deadline = Instant::now()
        + Duration::from_secs((level_secs + 60) * (gametypes.len() as u64 + 1) + 120);
    while !shared.client_gone.load(Ordering::Relaxed) {
        if Instant::now() > deadline {
            shared.done.store(true, Ordering::Relaxed);
        }
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            sv.handle_packet(from, &buf[..n], Instant::now());
        }
        let now = Instant::now();
        if now < next {
            for (to, p) in sv.take_outgoing() {
                let _ = sock.send_to(&p, to);
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        next += Duration::from_millis(sv.frame_ms());
        if next < now {
            next = now;
        }
        // The log's end before a load the console or the frame may run.
        let log_at = LOG.lock().unwrap().len();
        // The tick that builds the bots' graph blocks on it by design
        // (`wait_for_graph`); a live server builds it in the background.
        let graph_ready = shared.graph.lock().unwrap().is_some();
        let sched = opts.soak.and_then(|_| vcod_server::tick_profile::sched());
        if let Some(w) = &watch {
            w.begin();
        }
        let t0 = Instant::now();
        sv.tick(now);
        let tick_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let phases = vcod_server::tick_profile::take();
        let waits = watch.as_ref().map(|w| w.end()).unwrap_or_default();
        let split = vcod_server::tick_profile::Split::since(sched, tick_ms);
        for (to, p) in sv.take_outgoing() {
            let _ = sock.send_to(&p, to);
        }
        tick += 1;
        let id = sv.server_id();
        let counted = opts.soak.is_some() && graph_ready && id == level_id;
        if counted && tick_ms > tick_log_ms {
            eprintln!(
                "server tick {map} {gametype} at {:.1} s (run {:.1} s): {tick_ms:.1} ms ({}): {}",
                level_start.elapsed().as_secs_f32(),
                RUN_START.elapsed().as_secs_f32(),
                split.map_or_else(|| "no schedstat".into(), |s| s.to_string()),
                vcod_server::tick_profile::format(&phases, 1.0)
            );
            if !waits.is_empty() {
                eprintln!("  waits: {waits}");
            }
        }
        if counted && tick_ms > STALL_MS {
            let mut pairs = shared.pairs.lock().unwrap();
            let p = &mut pairs.entry(gametype.clone()).or_default().soak;
            p.server_stalls += 1;
            // Past it on a CPU: the server's own work, not a wait on the
            // client and lavapipe that share its process and cores.
            p.server_cpu_stalls += u32::from(split.is_some_and(|s| s.run_ms > STALL_MS));
            if tick_ms > p.server_worst_ms {
                p.server_worst_ms = tick_ms;
                p.server_worst_at = level_start.elapsed().as_secs_f32();
            }
        }
        if id != level_id && id & 0xf0 != level_id & 0xf0 {
            // A map loaded: close the level, open the next.
            {
                let mut pairs = shared.pairs.lock().unwrap();
                let p = pairs.entry(gametype.clone()).or_default();
                p.rotated = !forced && p.ended;
                p.forced = forced;
                p.secs = level_start.elapsed().as_secs_f32();
            }
            level_id = id;
            levels += 1;
            if opts.soak.is_some() {
                // The resident set level by level, a repeated gametype too.
                eprintln!(
                    "soak rss: {map} level {levels} after {gametype}: {} MB, malloc in use {} MB",
                    rss_mb(),
                    heap_mb()
                );
                jemalloc_dump();
            }
            gametype = sv.gametype();
            mark(map, &gametype, Some(log_at));
            level_start = Instant::now();
            ended_at = None;
            forced = false;
            hist.clear();
            if levels > gametypes.len() {
                // Back to the first: the rotation ran round.
                shared.done.store(true, Ordering::Relaxed);
            } else {
                shared
                    .pairs
                    .lock()
                    .unwrap()
                    .entry(gametype.clone())
                    .or_default()
                    .loaded = true;
            }
        } else if id != level_id {
            level_id = id;
        }
        if levels > gametypes.len() || !tick.is_multiple_of(20) {
            continue;
        }
        if opts.soak.is_some()
            && let Some(g) = sv.graph()
        {
            *shared.graph.lock().unwrap() = Some(g);
        }
        let Some(level) = sv.sample(&slots) else {
            continue;
        };
        // The script errors so far this level.
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut past = 0;
        for e in &level.errors {
            if let Some(rest) = e.strip_prefix("... and ") {
                past = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                continue;
            }
            *counts.entry(e.clone()).or_default() += 1;
        }
        // Bots holding still without a reason.
        let (mut playing, mut stalled) = (0, 0);
        for &(slot, at, holds) in &level.bodies {
            let h = hist.entry(slot).or_default();
            let Some(at) = at else {
                h.clear();
                continue;
            };
            // One sample per 20 ticks: the window is 12 samples.
            playing += 20;
            h.push(at);
            let w = STALL_WINDOW / 20;
            if h.len() > w {
                h.remove(0);
            }
            if h.len() == w && h.iter().all(|o| o.distance(at) < STALL_RADIUS) && !holds {
                stalled += 20;
            }
        }
        {
            let mut pairs = shared.pairs.lock().unwrap();
            let p = pairs.entry(gametype.clone()).or_default();
            p.errors = counts;
            p.errors_past_cap = past;
            p.playing += playing;
            p.stalled += stalled;
            if opts.soak.is_some() {
                let mb = rss_mb();
                let first = p.soak.rss.map_or(mb, |r| r.0);
                p.soak.rss = Some((first, mb));
            }
            if level.ended && !p.ended {
                p.ended = true;
                ended_at = Some(Instant::now());
            }
        }
        // A level that never ends, or ends and never rotates, is moved on.
        let stuck = ended_at.map_or(
            level_start.elapsed() > Duration::from_secs(level_secs),
            |t| t.elapsed() > Duration::from_secs(60),
        );
        if stuck && !forced {
            forced = true;
            log::warn!("sweep: {map} {gametype} forced on ({})", level.state);
            sv.rotate();
        }
        if shared.done.load(Ordering::Relaxed) {
            break;
        }
    }
    shared.done.store(true, Ordering::Relaxed);
    // Let the client hear its last packets.
    let until = Instant::now() + Duration::from_secs(10);
    while !shared.client_gone.load(Ordering::Relaxed) && Instant::now() < until {
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            sv.handle_packet(from, &buf[..n], Instant::now());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// CoD4 1.7 under the run.
struct Cod4 {
    install: vcod_cod4::install::Install,
}

impl Cod4 {
    /// `None` without `COD4_DIR`.
    fn new() -> Option<Cod4> {
        Some(Cod4 {
            install: vcod_cod4::testing::cod4_install()?,
        })
    }
}

impl Game for Cod4 {
    fn stock_maps(&self) -> Vec<String> {
        stock_maps(&self.install)
    }

    fn start(
        &self,
        map: &str,
        gametypes: &[String],
        opts: Opts,
    ) -> anyhow::Result<(Box<dyn SoakServer>, Vec<usize>)> {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let first = gametypes[0].clone();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: map.into(),
                gametype: first.clone(),
                hostname: "sweep".into(),
                max_clients: 12,
            },
            7,
        );
        let mut sets: Vec<(String, String)> = vec![
            ("sv_pure".into(), "0".into()),
            ("scr_game_playerwaittime".into(), "1".into()),
            ("scr_game_matchstarttime".into(), "1".into()),
        ];
        for g in STOCK_GAMETYPES {
            let minutes = opts.soak.map_or(0.5, |s| s.minutes);
            sets.push((format!("scr_{g}_timelimit"), minutes.to_string()));
            sets.push((format!("scr_{g}_roundlimit"), "1".into()));
        }
        sets.push(("sv_mapRotation".into(), rotation(map, gametypes)));
        for (k, v) in &sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&self.install))?;
        let slots = sv.add_bots(BOTS, true);
        if let Some(b) = sv.bots_mut() {
            b.wait_for_graph();
        }
        Ok((Box::new(sv), slots))
    }

    fn play(&self, map: &str, addr: SocketAddr, shared: &Shared, opts: Opts) -> anyhow::Result<()> {
        let root = self.install.root();
        match opts.soak {
            Some(soak) => play_soak(root, map, addr, shared, soak),
            None => play(root, map, addr, shared),
        }
    }
}

/// `sv_mapRotation` naming `map` once per gametype after the first, then
/// the first again.
pub(super) fn rotation(map: &str, gametypes: &[String]) -> String {
    gametypes
        .iter()
        .skip(1)
        .chain(std::iter::once(&gametypes[0]))
        .map(|g| format!("gametype {g} map {map}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl SoakServer for vcod_server::cod4::Cod4Server {
    fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod4::Cod4Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        vcod_server::cod4::Cod4Server::take_outgoing(self)
    }

    fn tick(&mut self, now: Instant) {
        use vcod_server::cod4::Cod4Game;
        use vcod_server::engine::GameModule;
        <Cod4Game as GameModule>::drain_console(self, now);
        self.tick_frames(now, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod4::FRAME_MS as u64
    }

    fn server_id(&self) -> i32 {
        vcod_server::cod4::Cod4Server::server_id(self)
    }

    fn gametype(&self) -> String {
        self.cvar("g_gametype").to_string()
    }

    fn graph(&mut self) -> Option<Arc<vcod_server::nav::NavGraph>> {
        self.bots_mut().and_then(|b| b.graph().cloned())
    }

    fn sample(&mut self, slots: &[usize]) -> Option<Sample> {
        let state = self.level_mut()?.game_value("state");
        let errors = self.level()?.rt.aborts();
        let holds: HashMap<usize, bool> = self
            .bots_mut()
            .map(|b| {
                b.bots()
                    .iter()
                    .map(|(s, b)| {
                        let h = b.engaged()
                            || b.holds_cover()
                            || b.retreating() != 0
                            || b.on_objective();
                        (*s, h)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let level = self.level()?;
        let bodies = slots
            .iter()
            .filter_map(|&slot| {
                let ps = level.pm_state(slot)?;
                let live = ps.pm_type == 0
                    && ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0
                    && !level.rt.client_vitals(slot).dead
                    && state == "playing";
                let at = live.then(|| Vec3::from(<[f32; 3]>::from(ps.origin)));
                Some((slot, at, holds.get(&slot).copied().unwrap_or(false)))
            })
            .collect();
        Some(Sample {
            ended: state == "postgame",
            state,
            errors,
            bodies,
        })
    }

    fn rotate(&mut self) {
        self.core.console.push_back("map_rotate".into());
    }
}
/// The client and what it draws and plays through, joined and loaded.
struct Rig {
    c: crate::cod4_client::Cod4Client,
    fs: vcod_common::pk3::Pk3Fs,
    renderer: Option<(
        crate::renderer::Renderer,
        crate::renderer::pipeline_check::Errors,
    )>,
    shell: crate::console::shell::Shell,
    localized: vcod_common::localize::Localized,
    ui: crate::frontend::Ui,
    audio: crate::audio::AudioSystem,
    fx: crate::fx::sim::FxSystem,
}

/// Joins `addr` through the stock menus and loads `map` as `main` does,
/// into a headless 320 x 240 renderer when there is an adapter. Waits on
/// the thread's `sim` clock under `run_held`, else the wall.
fn rig(root: &Path, map: &str, addr: std::net::SocketAddr) -> anyhow::Result<Rig> {
    use crate::audio::{AudioOpts, AudioSystem, bank};
    use crate::renderer::Renderer;
    let join = MenuJoin {
        team: Some("autoassign".into()),
        class: Some(super::cod4_menus::DEFAULT_CLASS.into()),
    };
    let mut s = Cod4Session::connect(addr, "sweeper", Cp::Bare, join)?;
    let t0 = clk_now();
    while s.gamestate().is_none() && clk_since(t0) < Duration::from_secs(60) {
        s.step(clk_since(t0).as_secs_f64() * 1000.0)?;
        clk_sleep(Duration::from_millis(16));
    }
    anyhow::ensure!(s.gamestate().is_some(), "no gamestate");
    let mut m = crate::cod4_fly::load(root, None, map, &s.entity_assets())?;
    let mut fs = crate::cod4_fly::open_fs(root, None, Some(map))?;
    for (p, w) in std::mem::take(&mut m.sound_files) {
        fs.overlay(&p, w);
    }
    let mut renderer = Renderer::headless_sized(&fs, false, 1, (320, 240));
    if let Some((r, _)) = &mut renderer {
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_material_lib(m.materials.clone());
        r.load_world(&m.bsp, &fs)?;
    }
    let shell = crate::console::shell::Shell::new();
    let localized = vcod_common::localize::Localized::load(&fs);
    let ui = crate::frontend::Ui::without_menus(&fs);
    let mut audio = AudioSystem::with_aliases(
        &fs,
        AudioOpts {
            enabled: false,
            volume: shell.cvar_f32("mss_volume"),
            rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
        },
        Default::default(),
    );
    audio.on_gamestate(map);
    let mut fx = crate::fx::sim::FxSystem::new();
    let mut c = crate::cod4_client::Cod4Client::new(
        s,
        std::mem::take(&mut m.models),
        std::mem::take(&mut m.parts),
        std::mem::take(&mut m.weapons),
        m.bodies.take(),
        m.av.take(),
        m.world.take(),
        std::mem::take(&mut m.look),
    );
    // The level's first-use builds behind the load, as `main`'s join runs
    // them (`App::cod4_prewarm`).
    if let Some((r, _)) = &mut renderer {
        c.prewarm(&mut crate::cod2_client::Cod2Frame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r,
            console_open: false,
            time: 0.0,
            dt: 0.0,
        });
    }
    Ok(Rig {
        c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        audio,
        fx,
    })
}

/// A menu left up past its auto answer (a new gametype's team menu)
/// takes auto-assign, or the first class.
fn answer_menu(c: &mut crate::cod4_client::Cod4Client, since: &mut Option<Instant>) {
    let now = clk_now();
    let at = *since.get_or_insert(now);
    if now.saturating_duration_since(at) > Duration::from_secs(3)
        && let Some(gs) = c.session.gamestate().cloned()
    {
        let responses = c.session.menus.open().unwrap().responses.clone();
        let pick = ["autoassign", super::cod4_menus::DEFAULT_CLASS]
            .iter()
            .find_map(|r| responses.iter().position(|x| x == r))
            .or((!responses.is_empty()).then_some(0));
        if let Some(i) = pick
            && let Some(cmd) = c.session.menus.choose(i, &gs)
        {
            c.session.command(&cmd);
        }
        *since = Some(now);
    }
}

/// The client's side: join, then play through the live pump until the
/// server is done; `Err` when the join itself fails.
fn play(root: &Path, map: &str, addr: std::net::SocketAddr, shared: &Shared) -> anyhow::Result<()> {
    use crate::cod2_client::Cod2Frame;
    let Rig {
        mut c,
        fs,
        mut renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, map, addr)?;
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = Instant::now();
    let mut last = start;
    // The gametype the client's gamestate names, and when its menu went up.
    let mut menu_since: Option<Instant> = None;
    while !shared.done.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        // A gamestate naming another map loads it, as `main.rs` does.
        if let Some(new) = c.session.take_map_change() {
            log::warn!("sweep: the client changed map to {new}");
        }
        if let Some((r, _)) = &mut renderer {
            let mut cx = Cod2Frame {
                fs: &fs,
                shell: &shell,
                localized: &localized,
                audio: &mut audio,
                fx: &mut fx,
                ui: &ui,
                r,
                console_open: false,
                time,
                dt,
            };
            c.pump(&mut cx, &mut cam);
            let (f, rt, u) = crate::camera::basis(cam.yaw, cam.pitch);
            audio.set_listener(cam.pos, f, rt, u, u32::MAX);
            fx.step(dt, time, None);
            let positions = c.entity_positions();
            audio.step(&positions, &HashMap::new(), None);
        } else {
            c.session.step(time as f64 * 1000.0)?;
            c.session.take_fired();
            c.session.take_lines();
        }
        c.session.take_commands();
        let gametype = c.session.gametype().unwrap_or_default();
        let spawned = c.session.spawn().is_some();
        if spawned {
            menu_since = None;
            if let Some(p) = shared.pairs.lock().unwrap().get_mut(&gametype) {
                p.client_spawned = true;
            }
            // Fire in bursts and walk now and then: the own weapon's
            // events, sounds and effects run.
            let ms = (time * 1000.0) as u32;
            c.session.input.key(Action::Attack, ms % 1200 < 300);
            c.session.input.key(Action::Forward, ms % 5000 < 1500);
        } else if c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        } else {
            c.session.input.release_all();
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    if let Some((_, errors)) = &renderer {
        for e in errors.lock().unwrap().iter() {
            log::warn!("wgpu: {e}");
        }
    }
    Ok(())
}

/// The soak's player: the inputs a person sends, decided once a frame
/// before the pump.
pub(super) struct Player {
    rng: u64,
    pub(super) gametype: String,
    /// Since the gamestate's gametype changed: a load's frames are
    /// stalls of their own.
    pub(super) level_at: f32,
    alive: bool,
    /// The nav path left to walk, nearest first.
    path: Vec<Vec3>,
    /// Where the feet were when they last moved, and when.
    moved: (Vec3, f32),
    next_frag: f32,
    frag_until: f32,
    next_scores: f32,
    scores_until: f32,
    next_menu: f32,
    menu_until: Option<f32>,
    menu_since: Option<Instant>,
}

/// pmove's standing eye height: the feet under the drawn eye.
const EYE: f32 = 60.0;

impl Player {
    /// When the level `gametype` names began: now when the frame just
    /// run brought it (its gamestate's loads land in that frame).
    pub(super) fn level_at(&self, gametype: &str, now: f32) -> f32 {
        match gametype == self.gametype {
            true => self.level_at,
            false => now,
        }
    }

    pub(super) fn new(seed: u64) -> Player {
        Player {
            rng: seed | 1,
            gametype: String::new(),
            level_at: 0.0,
            alive: false,
            path: Vec::new(),
            moved: (Vec3::ZERO, 0.0),
            next_frag: 0.0,
            frag_until: 0.0,
            next_scores: 0.0,
            scores_until: 0.0,
            next_menu: 0.0,
            menu_until: None,
            menu_since: None,
        }
    }

    fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 32) as u32
    }

    /// `lo..hi` seconds.
    fn secs(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.rand() % 1000) as f32 / 1000.0
    }

    /// A random node's path from the feet, or none off the graph.
    fn plan(&mut self, graph: &vcod_server::nav::NavGraph, feet: Vec3) -> Vec<Vec3> {
        let Some(from) = graph.nearest(feet.into()) else {
            return Vec::new();
        };
        if graph.is_empty() {
            return Vec::new();
        }
        let to = self.rand() % graph.len() as u32;
        let mut path: Vec<Vec3> = graph
            .path_toward(from, to)
            .unwrap_or_default()
            .into_iter()
            .map(|n| graph.nodes[n as usize])
            .collect();
        path.reverse();
        path
    }

    /// This frame's keys, view turn, board and menu.
    pub(super) fn act(
        &mut self,
        c: &mut dyn Pilot,
        cam: &mut crate::camera::FlyCamera,
        time: f32,
        graph: Option<&vcod_server::nav::NavGraph>,
        p: &mut SoakPair,
    ) {
        let gametype = c.gametype();
        if gametype != self.gametype {
            self.gametype = gametype;
            self.level_at = time;
            self.alive = false;
        }
        let pm_type = c.pm_type();
        let spawned = c.spawned();
        // The Esc menu, opened while playing, goes again after two
        // seconds (`onEsc`).
        if let Some(until) = self.menu_until {
            if time >= until {
                self.menu_until = None;
                if c.menu_open() {
                    c.close_menu();
                }
            }
            return;
        }
        if spawned && !self.alive {
            self.alive = true;
            p.spawns += 1;
            self.path.clear();
            self.moved = (cam.pos, time);
            self.next_frag = time + self.secs(4.0, 12.0);
            self.next_scores = time + self.secs(6.0, 20.0);
            self.next_menu = time + self.secs(10.0, 40.0);
        }
        if !spawned {
            if self.alive && matches!(pm_type, 6..=8) {
                p.deaths += 1;
            }
            if self.alive {
                self.alive = false;
                c.release_all();
                if self.scores_until > time {
                    c.set_scores(false);
                    self.scores_until = 0.0;
                }
            }
            if c.menu_open() {
                c.answer_menu(&mut self.menu_since);
            } else {
                self.menu_since = None;
                // A dead or spectating player taps fire: a respawn
                // button, a followed player cycled.
                c.key(Action::Attack, (time * 1000.0) as u32 % 1000 < 100);
            }
            return;
        }
        self.menu_since = None;
        let feet = cam.pos - Vec3::Z * EYE;
        // The path: the next waypoint once within reach, a new path when
        // it ran out or the feet held still four seconds.
        if feet.distance(self.moved.0) > 16.0 {
            self.moved = (feet, time);
        }
        let still = time - self.moved.1;
        while let Some(w) = self.path.last() {
            if (*w - feet).truncate().length() < 40.0 && (w.z - feet.z).abs() < 72.0 {
                self.path.pop();
                p.reached += 1;
            } else {
                break;
            }
        }
        if (self.path.is_empty() || still > 4.0)
            && let Some(g) = graph
        {
            self.path = self.plan(g, feet);
            self.moved = (feet, time);
            if !self.path.is_empty() {
                p.paths += 1;
            }
        }
        let ms = (time * 1000.0) as u32;
        match self.path.last() {
            Some(w) => {
                let d = *w - feet;
                cam.yaw = d.y.atan2(d.x);
                cam.pitch = 0.0;
            }
            // Off the graph: wander.
            None => cam.yaw += 0.02,
        }
        c.key(Action::Forward, true);
        c.key(Action::Jump, still > 1.5 && ms % 700 < 50);
        c.key(Action::Attack, ms % 1500 < 250);
        // A frag now and then: held 0.4 s and let go.
        if time >= self.next_frag {
            self.next_frag = time + self.secs(10.0, 20.0);
            self.frag_until = time + 0.4;
            p.frags += 1;
        }
        c.key(Action::Frag, time < self.frag_until);
        // The board for a second and a half.
        if time >= self.next_scores {
            self.next_scores = time + self.secs(15.0, 30.0);
            self.scores_until = time + 1.5;
            c.set_scores(true);
            p.scores += 1;
        } else if self.scores_until > 0.0 && time >= self.scores_until {
            self.scores_until = 0.0;
            c.set_scores(false);
        }
        if time >= self.next_menu && !c.menu_open() {
            self.next_menu = time + self.secs(25.0, 45.0);
            c.release_all();
            if c.open_menu() {
                p.menus += 1;
                self.menu_until = Some(time + 2.0);
            }
        }
    }
}

/// What [`Player`] reads off and does to a game's client.
pub(super) trait Pilot {
    /// The gametype the client's gamestate names.
    fn gametype(&self) -> String;
    /// The newest snapshot's own `pm_type`.
    fn pm_type(&self) -> u32;
    /// Playing in the world (not dead, spectating or in a menu's team pick).
    fn spawned(&self) -> bool;
    fn menu_open(&self) -> bool;
    /// A menu left up: answered (or closed) once it was up a while.
    fn answer_menu(&mut self, since: &mut Option<Instant>);
    /// A `+button` bind's edge.
    fn key(&mut self, action: Action, down: bool);
    fn release_all(&mut self);
    /// The scoreboard's key.
    fn set_scores(&mut self, down: bool);
    /// Esc while playing: true when a menu came up.
    fn open_menu(&mut self) -> bool;
    /// Esc on the menu it opened.
    fn close_menu(&mut self);
}

impl Pilot for crate::cod4_client::Cod4Client {
    fn gametype(&self) -> String {
        self.session.gametype().unwrap_or_default()
    }

    fn pm_type(&self) -> u32 {
        self.session
            .client()
            .snapshots
            .last()
            .and_then(|f| f.ps.get("pm_type"))
            .unwrap_or(0)
    }

    fn spawned(&self) -> bool {
        self.session.spawn().is_some()
    }

    fn menu_open(&self) -> bool {
        self.session.menus.open().is_some()
    }

    fn answer_menu(&mut self, since: &mut Option<Instant>) {
        answer_menu(self, since);
    }

    fn key(&mut self, action: Action, down: bool) {
        self.session.input.key(action, down);
    }

    fn release_all(&mut self) {
        self.session.input.release_all();
    }

    fn set_scores(&mut self, down: bool) {
        crate::cod4_client::Cod4Client::set_scores(self, down);
    }

    fn open_menu(&mut self) -> bool {
        self.menu_key(winit::keyboard::KeyCode::Escape);
        self.session.menus.open().is_some()
    }

    fn close_menu(&mut self) {
        self.menu_key(winit::keyboard::KeyCode::Escape);
    }
}

/// The soak's client: [`Player`] decides the frame's input, then the
/// frame runs as `main`'s CoD4 arm does (`Session::pump`, `scene`,
/// `draw_view`, `ViewTail`, the HUD quads), `render` at most `render_hz`
/// times a second. Each frame's CPU side, the render apart, is timed.
fn play_soak(
    root: &Path,
    map: &str,
    addr: std::net::SocketAddr,
    shared: &Shared,
    opts: SoakOpts,
) -> anyhow::Result<()> {
    use crate::cod2_client::Cod2Frame;
    let Rig {
        c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, map, addr)?;
    let Some((mut r, errors)) = renderer else {
        anyhow::bail!("the soak draws: no adapter (set VK_DRIVER_FILES for lavapipe)");
    };
    // `VCOD_FRAME_PROFILE=<path>` profiles the frames by stage as `main`
    // does (`tools/cod4_perf.py`), one map a run.
    crate::frame_prof::init();
    let mut sess = crate::session::Session::Cod4(Box::new(c));
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let seed = map.bytes().fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    let mut me = Player::new(seed);
    let start = Instant::now();
    let mut last = start;
    let every = Duration::from_secs_f32(1.0 / opts.render_hz.max(0.1));
    let mut next_render = start;
    let mut fx_ms = 0.0;
    let mut audio_was = audio.stats();
    while !shared.done.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = start.elapsed().as_secs_f32();
        let graph = shared.graph.lock().unwrap().clone();
        let c = sess.cod4_mut().unwrap();
        if let Some(new) = c.session.take_map_change() {
            log::warn!("sweep: the client changed map to {new}");
        }
        let mut frame = SoakPair::default();
        me.act(c, &mut cam, time, graph.as_deref(), &mut frame);
        let warm = Tally::warm(&r);
        // `main`'s `Mode::Cod2` arm, which a CoD4 session runs.
        let t0 = Instant::now();
        let mut hud_quads = Vec::new();
        let mut vm = None;
        let mut cx = Cod2Frame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time,
            dt,
        };
        let pumped = sess.pump(&mut cx, &mut cam);
        let t1 = Instant::now();
        if pumped {
            (vm, hud_quads) = sess.scene(&mut cx, &mut cam);
        }
        let t2 = Instant::now();
        let aspect = r.aspect();
        let widescreen = shell.cvar_f32("vc_widescreen") != 0.0;
        let fly_fov = crate::camera::view_fov_x(crate::camera::DEFAULT_FOV_DEG, aspect, widescreen);
        let view = sess.draw_view(&fs, &mut r, &cam, None, vm, (fly_fov, aspect, widescreen));
        let t2a = Instant::now();
        let (drawn, vm) = crate::ViewTail {
            fs: &fs,
            shell: &shell,
            audio: &mut audio,
            fx: &mut fx,
            session: &sess,
            r: &mut r,
            time,
            dt,
            aspect,
            cull: crate::renderer::CullMode::On,
        }
        .run(&cam, view, fly_fov, &mut hud_quads, &mut fx_ms);
        let t2b = Instant::now();
        r.set_hud_quads(&fs, hud_quads);
        let t3 = Instant::now();
        let mut render_ms = 0.0;
        if now >= next_render {
            r.render(drawn, vm);
            render_ms = t3.elapsed().as_secs_f32() * 1000.0;
            next_render = now + every;
            frame.renders = 1;
        }
        crate::frame_prof::end_frame();
        let c = sess.cod4_mut().unwrap();
        c.session.take_commands();
        // What the frame cost and raised.
        let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f32() * 1000.0;
        let parts = [
            ("pump", ms(t0, t1)),
            ("hud", ms(t1, t2)),
            ("view weapon", ms(t2, t2a)),
            ("fx and sound", ms(t2a, t2b)),
            ("hud upload", ms(t2b, t3)),
        ];
        let gametype = c.session.gametype().unwrap_or_default();
        let tally = Tally {
            parts: &parts,
            render_ms,
            time,
            level_at: me.level_at(&gametype, time),
            errors: &errors,
            audio: &audio,
            warm: [warm, Tally::warm(&r)],
        };
        tally.add(&mut frame, &mut audio_was, shared, &gametype);
        let spent = now.elapsed();
        std::thread::sleep(Duration::from_millis(16).saturating_sub(spent));
    }
    crate::frame_prof::flush();
    sess.cod4_mut().unwrap().session.disconnect();
    Ok(())
}

impl SoakPair {
    /// A frame's counts added in.
    fn add(&mut self, f: &SoakPair) {
        for (k, n) in &f.wgpu {
            *self.wgpu.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &f.stalls {
            *self.stalls.entry(k.clone()).or_default() += n;
        }
        self.spawns += f.spawns;
        self.deaths += f.deaths;
        self.frags += f.frags;
        self.scores += f.scores;
        self.menus += f.menus;
        self.paths += f.paths;
        self.reached += f.reached;
        self.frames += f.frames;
        self.renders += f.renders;
        self.worst_ms = self.worst_ms.max(f.worst_ms);
        self.render_stalls += f.render_stalls;
        self.audio_miss += f.audio_miss;
        self.builds += f.builds;
        self.build_worst_ms = self.build_worst_ms.max(f.build_worst_ms);
    }
}

/// One soak frame's cost and what it raised, added to its pair.
pub(super) struct Tally<'a> {
    /// The frame's CPU parts by name, in ms; the render apart.
    pub(super) parts: &'a [(&'static str, f32)],
    pub(super) render_ms: f32,
    pub(super) time: f32,
    /// When the client's gametype last changed: [`Player::level_at`],
    /// or the frame's own time when the frame brought a new one.
    pub(super) level_at: f32,
    pub(super) errors: &'a crate::renderer::pipeline_check::Errors,
    pub(super) audio: &'a crate::audio::AudioSystem,
    /// The renderer's mark materials and fx images before and after the
    /// frame ([`Tally::warm`]).
    pub(super) warm: [(usize, usize); 2],
}

impl Tally<'_> {
    /// The mark materials built and fx images loaded so far.
    pub(super) fn warm(r: &crate::renderer::Renderer) -> (usize, usize) {
        (r.mark_shaders(), r.fx_images())
    }

    /// `frame` (what the player did this frame) closed and added to
    /// `gametype`'s pair; `audio_was` the audio's counts a frame ago.
    pub(super) fn add(
        &self,
        frame: &mut SoakPair,
        audio_was: &mut crate::audio::AudioStats,
        shared: &Shared,
        gametype: &str,
    ) {
        let cpu: f32 = self.parts.iter().map(|p| p.1).sum();
        if cpu.max(self.render_ms) > *CLIENT_LOG_MS {
            eprintln!(
                "client frame {gametype} (run {:.1} s): cpu {cpu:.1} ms render {:.1} ms: {:?}",
                RUN_START.elapsed().as_secs_f32(),
                self.render_ms,
                self.parts
            );
        }
        frame.frames = 1;
        frame.worst_ms = cpu;
        if cpu > STALL_MS {
            let part = self
                .parts
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap()
                .0;
            // What the player did this frame, when anything.
            let did = [
                (frame.spawns > 0, "spawn"),
                (frame.scores > 0, "board"),
                (frame.menus > 0, "esc"),
                (frame.frags > 0, "frag"),
                (self.time - self.level_at < 5.0, "level start"),
            ]
            .iter()
            .filter(|d| d.0)
            .map(|d| d.1)
            .collect::<Vec<_>>()
            .join(", ");
            let key = match did.is_empty() {
                true => part.to_string(),
                false => format!("{part} ({did})"),
            };
            frame.stalls.insert(key, 1);
        }
        if self.warm[1] != self.warm[0] && self.time - self.level_at > 5.0 {
            frame.builds = 1;
            frame.build_worst_ms = cpu;
        }
        frame.render_stalls = (self.render_ms > STALL_MS) as u32;
        for e in self.errors.lock().unwrap().drain(..) {
            *frame.wgpu.entry(shape(&e)).or_default() += 1;
        }
        let a = self.audio.stats();
        frame.audio_miss = a.misses - audio_was.misses;
        *audio_was = a;
        if let Some(p) = shared.pairs.lock().unwrap().get_mut(gametype) {
            if frame.spawns > 0 {
                p.client_spawned = true;
            }
            p.soak.add(frame);
        }
    }
}

/// The soak's table, then its own findings ranked by count.
fn soak_report(results: &BTreeMap<String, BTreeMap<String, Pair>>, gametypes: &[String]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| map | gametype | secs | spawns | deaths | frags | board | esc | paths (wp) | frames | stalls | worst ms | renders (slow) | wgpu | audio miss | notes c/s | server stalls, on CPU (worst ms at s) | RSS MB | first-use builds (worst ms) |"
    );
    let _ = writeln!(o, "|{}", "---|".repeat(19));
    for (map, pairs) in results {
        for g in gametypes {
            let empty = Pair::default();
            let p = pairs.get(g).unwrap_or(&empty);
            let s = &p.soak;
            let rss = s
                .rss
                .map_or("-".to_string(), |(a, b)| format!("{a} -> {b}"));
            let _ = writeln!(
                o,
                "| {map} | {g} | {:.0} | {} | {} | {} | {} | {} | {} ({}) | {} | {} | {:.0} | {} ({}) | {} | {} | {}/{} | {}, {} ({:.0} at {:.0}) | {rss} | {} ({:.0}) |",
                p.secs,
                s.spawns,
                s.deaths,
                s.frags,
                s.scores,
                s.menus,
                s.paths,
                s.reached,
                s.frames,
                s.stalls.values().sum::<u32>(),
                s.worst_ms,
                s.renders,
                s.render_stalls,
                s.wgpu.values().sum::<u32>(),
                s.audio_miss,
                s.client_notes.values().sum::<u32>(),
                s.server_notes.values().sum::<u32>(),
                s.server_stalls,
                s.server_cpu_stalls,
                s.server_worst_ms,
                s.server_worst_at,
                s.builds,
                s.build_worst_ms,
            );
        }
    }
    let mut rank = |title: &str, f: &dyn Fn(&SoakPair) -> Vec<(String, u32)>| {
        let mut all: BTreeMap<String, (u32, Vec<String>)> = BTreeMap::new();
        for (map, pairs) in results {
            for (g, p) in pairs {
                for (k, n) in f(&p.soak) {
                    let e = all.entry(k).or_default();
                    e.0 += n;
                    e.1.push(format!("{map}/{g}"));
                }
            }
        }
        let mut v: Vec<_> = all.into_iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
        let _ = writeln!(o, "\n## {title}\n");
        for (k, (n, at)) in v {
            let pairs = if at.len() > 6 {
                format!("{} pairs", at.len())
            } else {
                at.join(", ")
            };
            let _ = writeln!(o, "- {n} x `{k}` ({pairs})");
        }
    };
    let list = |m: &BTreeMap<String, u32>| m.iter().map(|(k, n)| (k.clone(), *n)).collect();
    rank("wgpu validation errors", &|s| list(&s.wgpu));
    rank("Client notes", &|s| list(&s.client_notes));
    rank("Server notes", &|s| list(&s.server_notes));
    rank("Client stalls, by part", &|s| list(&s.stalls));
    o
}

/// The table, then every distinct script error and warning ranked by
/// count over the whole sweep with the pairs it came on.
fn report(results: &BTreeMap<String, BTreeMap<String, Pair>>, gametypes: &[String]) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "| map | gametype | load | end | rotate | client spawn | script errors | stall % | client warn | server warn | panic |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|---|---|---|---|---|---|");
    let yn = |b: bool| if b { "yes" } else { "**no**" };
    for (map, pairs) in results {
        for g in gametypes {
            let empty = Pair::default();
            let p = pairs.get(g).unwrap_or(&empty);
            let errors: u32 = p.errors.values().sum::<u32>() + p.errors_past_cap;
            let _ = writeln!(
                o,
                "| {map} | {g} | {} | {} | {}{} | {} | {errors} ({} distinct) | {:.1} | {} | {} | {} |",
                yn(p.loaded),
                yn(p.ended),
                yn(p.rotated),
                if p.forced { " (forced)" } else { "" },
                yn(p.client_spawned),
                p.errors.len(),
                100.0 * p.stalled as f64 / p.playing.max(1) as f64,
                p.client_warn.values().sum::<u32>(),
                p.server_warn.values().sum::<u32>(),
                if p.panics.is_empty() {
                    "".to_string()
                } else {
                    p.panics.join("; ").replace('|', "/")
                },
            );
        }
    }
    let mut rank = |title: &str, f: &dyn Fn(&Pair) -> Vec<(String, u32)>| {
        let mut all: BTreeMap<String, (u32, Vec<String>)> = BTreeMap::new();
        for (map, pairs) in results {
            for (g, p) in pairs {
                for (k, n) in f(p) {
                    let e = all.entry(k).or_default();
                    e.0 += n;
                    e.1.push(format!("{map}/{g}"));
                }
            }
        }
        let mut v: Vec<_> = all.into_iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
        let _ = writeln!(o, "\n## {title}\n");
        for (k, (n, at)) in v {
            let pairs = if at.len() > 6 {
                format!("{} pairs", at.len())
            } else {
                at.join(", ")
            };
            let _ = writeln!(o, "- {n} x `{k}` ({pairs})");
        }
    };
    rank("Script errors", &|p| {
        p.errors.iter().map(|(k, n)| (k.clone(), *n)).collect()
    });
    rank("Client warnings", &|p| {
        p.client_warn.iter().map(|(k, n)| (k.clone(), *n)).collect()
    });
    rank("Server warnings", &|p| {
        p.server_warn.iter().map(|(k, n)| (k.clone(), *n)).collect()
    });
    o
}

/// vcod-server's war on mp_crash for a live test, loaded: `hostname`
/// names it, `bots` non-shooting bots join.
fn crash_war(
    install: &vcod_cod4::install::Install,
    hostname: &str,
    cheats: bool,
    bots: usize,
) -> vcod_server::cod4::Cod4Server {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: hostname.into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    if cheats {
        sv.set_cvar("sv_cheats", "1");
    }
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.load(Some(install)).unwrap();
    if bots > 0 {
        sv.add_bots(bots, false);
    }
    sv
}

/// [`run_held`]'s frame hook for these tests: the console each frame, as
/// the dedicated server's loop drains it.
fn drain(sv: &mut vcod_server::cod4::Cod4Server, at: Instant, _: &mut Instant) -> bool {
    use vcod_server::engine::GameModule;
    <vcod_server::cod4::Cod4Game as GameModule>::drain_console(sv, at);
    true
}

/// The soak's first-use hitches: a surface's first bullet mark built its
/// material (images, stages, pipelines) in the frame it landed, and an
/// effect's first play loaded its sprite images, 15 to 250 ms each on
/// lavapipe. With the level's effects taken the client builds both up
/// front: a live client on vcod-server's mp_crash then fires into the
/// ground, its tracers fly and marks land, and no mark shader registers
/// and no fx image loads in play. Runs on [`run_held`]'s clock. Needs
/// `COD4_DIR` and an adapter.
#[test]
fn cod4_live_first_impact_marks_build_nothing_in_play() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "marks", false, 0);
    let Some(seen) = run_held(&mut sv, move |addr| first_marks(&root, addr), drain) else {
        return;
    };
    let warm = seen.warm.expect("the level's effects were never taken");
    assert!(seen.marks > 0, "no impact mark landed");
    assert_eq!(seen.shaders, warm.0, "a mark shader registered in play");
    assert_eq!(seen.images, warm.1, "an fx image loaded in play");
    assert!(warm.0 > 0 && warm.1 > 0, "nothing built with the level");
}

struct MarksSeen {
    /// Mark shaders and fx images once the level's effects were taken.
    warm: Option<(usize, usize)>,
    marks: usize,
    /// Mark shaders and fx images at the end.
    shaders: usize,
    images: usize,
}

/// [`cod4_live_first_impact_marks_build_nothing_in_play`]'s client; `None`
/// without an adapter.
fn first_marks(root: &Path, addr: SocketAddr) -> Option<MarksSeen> {
    use crate::cod2_client::Cod2Frame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut warm: Option<(usize, usize)> = None;
    let mut marks = 0;
    let mut held = false;
    while clk_since(start) < Duration::from_secs(120) && marks == 0 {
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = now.duration_since(start).as_secs_f32();
        let frozen = c
            .session
            .client()
            .snapshots
            .last()
            .and_then(|x| x.ps.get("pm_flags"))
            .is_none_or(|f| f & 0x800 != 0);
        if c.session.spawn().is_some() && !frozen {
            // Down at the ground ahead, in bursts.
            cam.pitch -= 0.05;
            let want = (time * 1000.0) as u32 % 800 < 200;
            if want != held {
                c.session.input.key(Action::Attack, want);
                held = want;
            }
        } else if c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = Cod2Frame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time,
            dt,
        };
        c.pump(&mut cx, &mut cam);
        let (f, rt, u) = crate::camera::basis(cam.yaw, cam.pitch);
        let marks_due = fx.take_marks(time);
        r.set_marks(&fs, &marks_due, &[], (time * 1000.0) as i32, time, true);
        fx.step(dt, time, None);
        audio.set_listener(cam.pos, f, rt, u, u32::MAX);
        c.session.take_commands();
        if warm.is_none() && c.effects_taken() {
            warm = Some((r.mark_shaders(), r.fx_images()));
        }
        if warm.is_some() {
            marks = r.mark_count();
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(MarksSeen {
        warm,
        marks,
        shaders: r.mark_shaders(),
        images: r.fx_images(),
    })
}

/// The level's first-use builds run with the load, not in its first frames
/// (cod4-client-perf.md 17): a live client joins vcod-server's mp_crash
/// war beside two bots, and once [`rig`]'s prewarm has run the script
/// menus are loaded and every model the play then draws (bodies, heads,
/// weapons) was uploaded already; only a model with parts hidden (a
/// shot-up car) or the stand-in box may upload in play. Runs on
/// [`run_held`]'s clock. Needs `COD4_DIR` and an adapter.
#[test]
fn cod4_live_join_builds_its_models_with_the_load() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "prewarm", false, 2);
    let Some(seen) = run_held(&mut sv, move |addr| join_models(&root, addr), drain) else {
        return;
    };
    assert!(seen.menus, "the script menus waited for the first frame");
    assert!(
        seen.bodies >= 2,
        "the bots were never posed ({})",
        seen.bodies
    );
    let late: Vec<_> = seen
        .after
        .difference(&seen.warm)
        .filter(|k| !k.contains('#') && *k != BOX_KEY)
        .collect();
    assert!(late.is_empty(), "uploaded in play: {late:?}");
}

struct JoinSeen {
    /// The script menus were loaded with the level.
    menus: bool,
    /// The models uploaded with the load, and by the end.
    warm: std::collections::BTreeSet<String>,
    after: std::collections::BTreeSet<String>,
    /// The most bodies one frame posed.
    bodies: usize,
}

/// [`cod4_live_join_builds_its_models_with_the_load`]'s client; `None`
/// without an adapter.
fn join_models(root: &Path, addr: SocketAddr) -> Option<JoinSeen> {
    use crate::cod2_client::Cod2Frame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let menus = c.menus_loaded();
    let (warm, _) = c.uploads();
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut bodies = 0;
    let mut since_bodies = None;
    // Three seconds of play past the first frame that posed both bots.
    while clk_since(start) < Duration::from_secs(90)
        && since_bodies.is_none_or(|t: Instant| clk_since(t) < Duration::from_secs(3))
    {
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        if c.session.spawn().is_none() && c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = Cod2Frame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time: now.duration_since(start).as_secs_f32(),
            dt,
        };
        c.pump(&mut cx, &mut cam);
        c.session.take_commands();
        bodies = bodies.max(c.uploads().1);
        if bodies >= 2 && since_bodies.is_none() {
            since_bodies = Some(clk_now());
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(JoinSeen {
        menus,
        warm,
        after: c.uploads().0,
        bodies,
    })
}

/// A shot-up car builds no GPU data in play (cod4-client-perf.md 18): a
/// live client on vcod-server's mp_crash war walks (`setviewpos`) to
/// 160 units off the nearest destructible sedan and fires bursts at its
/// doors until its `partBits` take new states, and no dynamic model
/// uploads past the prewarm's, the stand-in box aside: each hidden-part
/// state draws the prewarmed upload less its hidden surfaces. Prints the
/// client frame of each new state. Runs on [`run_held`]'s clock. Needs
/// `COD4_DIR` and an adapter.
#[test]
fn cod4_live_shot_car_uploads_nothing_in_play() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let _slot = crate::renderer::pipeline_check::live_session_slot();
    let mut sv = crash_war(&install, "carparts", true, 0);
    let Some(seen) = run_held(&mut sv, move |addr| shoot_car(&root, addr), drain) else {
        return;
    };
    for (bits, ms) in &seen.firsts {
        println!("car state {bits:08x?}: its first frame's pump {ms:.1} ms");
    }
    assert!(seen.target, "no car spot found");
    assert!(
        seen.firsts.len() >= 2,
        "the car never broke: {:x?}",
        seen.firsts
    );
    let boxed = usize::from(!seen.warm_keys.contains(BOX_KEY) && seen.keys.contains(BOX_KEY));
    assert_eq!(
        seen.uploads,
        seen.warm + boxed,
        "a model uploaded in play: {:?}",
        seen.keys.difference(&seen.warm_keys).collect::<Vec<_>>()
    );
}

struct CarSeen {
    /// A car spot was found.
    target: bool,
    /// (state, the frame's pump wall ms) per new state drawn.
    firsts: Vec<([u32; 4], f64)>,
    /// Dynamic uploads and model keys after the prewarm, and at the end.
    warm: usize,
    warm_keys: std::collections::BTreeSet<String>,
    uploads: usize,
    keys: std::collections::BTreeSet<String>,
}

/// [`cod4_live_shot_car_uploads_nothing_in_play`]'s client; `None` without
/// an adapter.
fn shoot_car(root: &Path, addr: SocketAddr) -> Option<CarSeen> {
    use super::cod4::Visual;
    use crate::cod2_client::Cod2Frame;
    let Rig {
        mut c,
        fs,
        renderer,
        shell,
        localized,
        ui,
        mut audio,
        mut fx,
    } = rig(root, "mp_crash", addr).unwrap();
    let Some((mut r, _)) = renderer else {
        c.session.disconnect();
        return None;
    };
    let warm = r.dynamic_uploads();
    let (warm_keys, _) = c.uploads();
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let start = clk_now();
    let mut last = start;
    let mut menu_since = None;
    let mut held = false;
    // (car number, its spot's eye, the point aimed at), once placed
    let mut target: Option<(u32, Vec3, Vec3)> = None;
    let mut placed_at = None;
    let mut states: Vec<[u32; 4]> = Vec::new();
    let mut firsts: Vec<([u32; 4], f64)> = Vec::new();
    while clk_since(start) < Duration::from_secs(120) && firsts.len() < 4 {
        if placed_at.is_some_and(|t: Instant| clk_since(t) > Duration::from_secs(20)) {
            break;
        }
        let now = clk_now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        let time = now.duration_since(start).as_secs_f32();
        let frozen = c
            .session
            .client()
            .snapshots
            .last()
            .and_then(|x| x.ps.get("pm_flags"))
            .is_none_or(|f| f & 0x800 != 0);
        if c.session.spawn().is_some() && !frozen {
            match target {
                None => {
                    target = car_spot(&mut c);
                    if let Some((_, eye, aim)) = target {
                        let to = aim - eye;
                        c.session.command(&format!(
                            "setviewpos {} {} {} {}",
                            eye.x,
                            eye.y,
                            eye.z,
                            to.y.atan2(to.x).to_degrees()
                        ));
                        placed_at = Some(now);
                    }
                }
                Some((_, eye, aim))
                    if placed_at
                        .is_some_and(|t: Instant| clk_since(t) > Duration::from_secs(2)) =>
                {
                    let to = aim - eye;
                    cam.yaw = to.y.atan2(to.x);
                    cam.pitch = to.z.atan2(to.truncate().length());
                    let want = (time * 1000.0) as u32 % 800 < 150;
                    if want != held {
                        c.session.input.key(Action::Attack, want);
                        held = want;
                    }
                }
                Some(_) => {}
            }
        } else if c.session.menus.open().is_some() {
            answer_menu(&mut c, &mut menu_since);
        }
        let mut cx = Cod2Frame {
            fs: &fs,
            shell: &shell,
            localized: &localized,
            audio: &mut audio,
            fx: &mut fx,
            ui: &ui,
            r: &mut r,
            console_open: false,
            time,
            dt,
        };
        // Wall time, printed only: the frame's cost, not the play's clock.
        let t = Instant::now();
        c.pump(&mut cx, &mut cam);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        fx.step(dt, time, None);
        c.session.take_commands();
        if let Some((car, _, _)) = target
            && let Some(scene) = c.session.scene()
            && let Some(d) = scene.entities.iter().find(|d| {
                d.number == car
                    && matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_"))
            })
            && !states.contains(&d.part_bits)
        {
            states.push(d.part_bits);
            firsts.push((d.part_bits, ms));
        }
        clk_sleep(Duration::from_millis(16));
    }
    c.session.disconnect();
    Some(CarSeen {
        target: target.is_some(),
        firsts,
        warm,
        warm_keys,
        uploads: r.dynamic_uploads(),
        keys: c.uploads().0,
    })
}

const BOX_KEY: &str = "<box>";

/// The nearest destructible sedan the client draws and an eye 160 units
/// off it, on the floor with room to stand, that sees its doors over
/// open air: (its entity number, the eye, the point aimed at).
fn car_spot(c: &mut crate::cod4_client::Cod4Client) -> Option<(u32, Vec3, Vec3)> {
    use super::cod4::Visual;
    let world = c.session.world()?.clone();
    let scene = c.session.scene()?;
    let mut cars: Vec<_> = scene
        .entities
        .iter()
        .filter(|d| matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_")))
        .map(|d| (d.number, d.origin))
        .collect();
    cars.sort_by(|a, b| a.1.distance(scene.eye).total_cmp(&b.1.distance(scene.eye)));
    cars.iter().find_map(|&(n, car)| {
        (0..16).find_map(|k| {
            let y = (k as f32 * 22.5).to_radians();
            let spot = car + Vec3::new(y.cos(), y.sin(), 0.0) * 160.0;
            let (top, bottom) = (spot + Vec3::Z * 64.0, spot - Vec3::Z * 128.0);
            let down = world
                .clip
                .trace(top, bottom, Vec3::ZERO, Vec3::ZERO, 0x281_0011);
            if down.startsolid || down.fraction >= 1.0 {
                return None;
            }
            let floor = top.lerp(bottom, down.fraction) + Vec3::Z * 0.125;
            if (floor.z - car.z).abs() > 32.0 {
                return None;
            }
            let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
            if world
                .clip
                .trace(floor, floor, lo, hi, 0x281_0011)
                .startsolid
            {
                return None;
            }
            let aim = car + Vec3::Z * 22.0;
            let eye = floor + Vec3::Z * 60.0;
            let t = world
                .clip
                .trace(eye, aim, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
            (t.fraction == 1.0).then_some((n, eye, aim))
        })
    })
}
