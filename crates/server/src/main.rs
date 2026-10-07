//! `vcod-server`, a CoD 1.1 dedicated server. Answers browsers, accepts retail
//! clients, runs the stock gametype scripts on the gsc VM and sends
//! delta-compressed snapshots.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use vcod_common::pk3::Pk3Fs;
use vcod_server::{Server, ServerConfig};

#[derive(Parser)]
#[command(about = "Call of Duty (2003) 1.1 dedicated server, in progress")]
struct Args {
    /// Map name, e.g. mp_carentan
    map: String,
    /// Game install; defaults to $COD_DIR, else the executable's directory
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// Pk3 subdirectory, main for CoD1 or uo for United Offensive
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// UDP port
    #[arg(long, default_value_t = 28960)]
    port: u16,
    /// sv_hostname
    #[arg(long, default_value = "vcod")]
    hostname: String,
    /// sv_maxclients
    #[arg(long, default_value_t = 8)]
    max_clients: usize,
    /// g_gametype: the script under maps/mp/gametypes to run
    #[arg(long, default_value = "dm")]
    gametype: String,
    /// A gametype script from disk instead of the paks: overlaid at
    /// maps/mp/gametypes/<stem> and run as gametype <stem>. For the client
    /// probes in crates/gsc/tests/fixtures/semantics/client-probes/.
    #[arg(long)]
    gametype_script: Option<std::path::PathBuf>,
    /// Scripted entities that exercise the packet-entity wire path. 0 is off.
    #[arg(long, default_value_t = 0)]
    test_entities: usize,
    /// Debug bots in play. Each takes a real client slot; 0 is off.
    #[arg(long, default_value_t = 0)]
    bots: usize,
    /// Whether the bots fight. Without it they only roam.
    #[arg(long)]
    bots_shoot: bool,
    /// A cvar to set before the scripts load, retail's `+set name value`;
    /// repeatable, e.g. `--set scr_friendlyfire=1`.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,

    /// Log one line per snapshot per client: send interval, the serverTime and
    /// commandTime a client predicts from, the usercmds consumed, and whether
    /// the frame went out as a delta. Also one `tick:` line per second: the
    /// slowest tick and how far the loop ran behind its schedule.
    #[arg(long)]
    trace: bool,
}

/// `sv_fps 20`.
const FRAME: Duration = Duration::from_millis(50);

/// How far behind the schedule the loop still runs ticks back to back
/// before it resynchronises instead.
const MAX_CATCH_UP: Duration = Duration::from_secs(1);

/// Without a bound a flood keeps the socket readable, `tick` never runs and
/// the outbox is never flushed.
const MAX_PACKETS_PER_FRAME: usize = 256;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    if args.test_entities > vcod_server::world::MAX_TEST_ENTITIES {
        bail!(
            "--test-entities {} exceeds {}, the most that fits below ENTITYNUM_WORLD",
            args.test_entities,
            vcod_server::world::MAX_TEST_ENTITIES
        );
    }
    if args.bots > args.max_clients {
        bail!(
            "--bots {} exceeds --max-clients {}; each bot takes a real slot",
            args.bots,
            args.max_clients
        );
    }
    let dir = args.game_dir.join(&args.mod_dir);
    let fs = std::rc::Rc::new(
        Pk3Fs::open(&dir).with_context(|| format!("opening game data in {}", dir.display()))?,
    );
    let Some(bsp_path) = fs.resolve_map(&args.map) else {
        bail!("map {} not found in {}", args.map, dir.display());
    };
    // A corrupt map fails here rather than at first use.
    let bsp_bytes = fs.read(&bsp_path).context("reading the bsp")?;
    let bsp = vcod_common::bsp::parse(&bsp_bytes).context("parsing the bsp")?;

    let overlay = match &args.gametype_script {
        Some(path) => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("--gametype-script needs a file name")?
                .to_string();
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Some((stem, text))
        }
        None => None,
    };
    let gametype = overlay
        .as_ref()
        .map_or(args.gametype.clone(), |(s, _)| s.clone());

    let sock = UdpSocket::bind(("0.0.0.0", args.port))
        .with_context(|| format!("binding udp/{}", args.port))?;
    sock.set_nonblocking(true)?;
    log::info!("vcod-server: {} on udp/{}", args.map, args.port);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let trace = args.trace;
    let mut server = Server::with_seed(
        ServerConfig {
            map: args.map,
            hostname: args.hostname,
            max_clients: args.max_clients,
            gametype,
            test_entities: args.test_entities,
            bots: args.bots,
            bots_shoot: args.bots_shoot,
            trace: args.trace,
        },
        Instant::now(),
        seed,
    );
    server.build_nav_in_background(true);
    server.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    if let Some((stem, text)) = &overlay {
        server.overlay_script(&format!("maps/mp/gametypes/{stem}"), text);
    }
    // Retail's `dedicated` default: heartbeat the masters. `--set dedicated=1`
    // keeps a LAN run off the list.
    server.set_cvar("dedicated", "2");
    for pair in &args.set {
        let Some((name, value)) = pair.split_once('=') else {
            bail!("--set takes NAME=VALUE, got {pair:?}");
        };
        server.set_cvar(name, value);
    }
    // A failed script load is fatal, not a warning. The configstring table is
    // mostly script output now, so a server that kept serving would hand
    // clients a gamestate with no team menu, no status icons and no shaders,
    // which reads as a protocol bug rather than a script failure.
    if let Err(e) = server.load_scripts(fs.clone()) {
        log::error!("loading the map and gametype scripts: {e:#}");
        std::process::exit(1);
    }
    // Ctrl-C runs `quit`, so the masters get their flatline; a second one
    // exits at once.
    let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        ctrlc::set_handler(move || {
            if flag.swap(true, std::sync::atomic::Ordering::Relaxed) {
                std::process::exit(130);
            }
        })
        .context("installing the Ctrl-C handler")?;
    }
    let mut quit_queued = false;
    let mut buf = vec![0u8; 65536];
    // A fixed schedule, not a sleep after each tick: `SV_Frame` runs one
    // game frame per `sv_fps` slice of wall time and catches up when a
    // frame overran, so `svs.time` tracks the wall clock. Sleeping the
    // remainder of each tick lets every overshoot accumulate, which read as
    // a serverTime 5-10% slow against a probe's wall clock under load.
    let mut next = Instant::now();
    let mut stats = TickStats::default();
    loop {
        let now = Instant::now();
        let late = now.saturating_duration_since(next);
        for _ in 0..MAX_PACKETS_PER_FRAME {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                break;
            };
            server.handle_packet(from, &buf[..n], now);
        }
        if interrupted.load(std::sync::atomic::Ordering::Relaxed) && !quit_queued {
            quit_queued = true;
            server.push_console("quit");
        }
        server.tick(now);
        if trace {
            stats.add(now.elapsed(), late);
        }
        // A level load that failed with the level already torn down: there is
        // no script to end the level and no table for a client to pull, which
        // is what retail's `Com_Error` ends the process for.
        if let Some(e) = server.take_fatal() {
            log::error!("{e:#}");
            std::process::exit(1);
        }
        for (to, pkt) in server.take_outgoing() {
            if let Err(e) = sock.send_to(&pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }
        if server.quit_requested() {
            return Ok(());
        }
        next += FRAME;
        if trace && stats.ticks == 20 {
            stats.log_and_reset();
        }
        let after = Instant::now();
        if next > after {
            std::thread::sleep(next - after);
        } else if after - next > MAX_CATCH_UP {
            // A stall past the catch-up bound (a debugger, a suspend) is
            // dropped rather than replayed as a burst of ticks.
            next = after;
        }
    }
}

/// `--trace`'s per-second tick summary: the slowest `Server::tick` and the
/// worst lag of a tick's start behind its slot on the fixed schedule.
#[derive(Default)]
struct TickStats {
    ticks: u32,
    sum: Duration,
    max: Duration,
    late: Duration,
}

impl TickStats {
    fn add(&mut self, took: Duration, late: Duration) {
        self.ticks += 1;
        self.sum += took;
        self.max = self.max.max(took);
        self.late = self.late.max(late);
    }

    fn log_and_reset(&mut self) {
        log::info!(
            "tick: mean {:.2} ms max {:.2} ms late {:.2} ms",
            self.sum.as_secs_f64() * 1000.0 / f64::from(self.ticks.max(1)),
            self.max.as_secs_f64() * 1000.0,
            self.late.as_secs_f64() * 1000.0
        );
        *self = TickStats::default();
    }
}
