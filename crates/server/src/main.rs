//! `vcod-server`, a CoD 1.1 dedicated server. Answers browsers, accepts retail
//! clients, runs the stock gametype scripts on the gsc VM and sends
//! delta-compressed snapshots.

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches, Parser};
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::Protocol;
use vcod_common::pk3::Pk3Fs;
use vcod_server::engine::{Arrival, Engine, GameModule};
use vcod_server::{Server, ServerConfig};

#[derive(Parser)]
#[command(about = "Call of Duty (2003) 1.1 dedicated server, in progress", version = env!("VCOD_VERSION"))]
struct Args {
    /// Map name, e.g. mp_carentan
    map: String,
    /// Game install; defaults to $COD_DIR, else the executable's directory
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// Pk3 subdirectory, main for CoD1 or uo for United Offensive
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// Which game the install is: cod1, cod2 or cod4. Defaults to what the
    /// game dir holds. cod2 runs its own engine (`vcod_server::cod2`):
    /// players join, move, fight, chat and vote, rcon admins them, and the
    /// maps cycle. cod4 runs a 1.7 level (`vcod_server::cod4`) the same
    /// way: the stock gametype scripts, pmove, combat, bots and the cycle.
    #[arg(long)]
    game: Option<vcod_common::game::Game>,
    /// UDP port
    #[arg(long, default_value_t = 28960)]
    port: u16,
    /// sv_hostname
    #[arg(long, default_value = "vcod")]
    hostname: String,
    /// sv_maxclients
    #[arg(long, default_value_t = 8)]
    max_clients: usize,
    /// g_gametype: the script under maps/mp/gametypes to run; dm, or war
    /// for cod4, where a `--set g_gametype=` wins over it
    #[arg(long)]
    gametype: Option<String>,
    /// A gametype script from disk instead of the paks: overlaid at
    /// maps/mp/gametypes/<stem> and run as gametype <stem>. For the client
    /// probes in crates/gsc/tests/fixtures/semantics/client-probes/.
    #[arg(long)]
    gametype_script: Option<std::path::PathBuf>,
    /// CoD4: a directory whose loose .gsc files each level reads over the
    /// zones' scripts (maps/mp/mp_crash.gsc, say), as retail reads a mod's.
    /// For a probe or a perf run's scripted scene.
    #[arg(long)]
    script_dir: Option<std::path::PathBuf>,
    /// Scripted entities that exercise the packet-entity wire path. 0 is off.
    #[arg(long, default_value_t = 0)]
    test_entities: usize,
    /// Debug bots in play. Each takes a real client slot; 0 is off.
    #[arg(long, default_value_t = 0)]
    bots: usize,
    /// Whether the bots fight. Without it they only roam.
    #[arg(long)]
    bots_shoot: bool,
    /// A cvar to set, retail's `+set name value`; repeatable, e.g.
    /// `--set scr_friendlyfire=1`. Every one is set before the scripts load;
    /// one placed after the map is set again once the level has loaded, over
    /// what the scripts wrote, as retail runs `+map x +set ...`.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,
    /// File that keeps `banUser`/`banClient` bans across runs: retail's
    /// `ban.txt`, a guid and the banned name a line, the guid an off-LAN
    /// client's IPv4 address as an integer (CoD4: the client's GUID). A list in vcod's older
    /// `<ip> <name>` lines is rewritten on load. Without it a ban lasts
    /// until the process exits.
    #[arg(long)]
    ban_file: Option<std::path::PathBuf>,
    /// Load only the install's stock retail paks (the patch's numbered paks,
    /// their `localized_*` ones, UO's map pack) and ignore every other pk3:
    /// third-party maps, `zzz_*` mods, downloads. Refuses `fs_game`.
    #[arg(long)]
    vanilla: bool,
    /// Leave stdin alone. By default each line on stdin runs as a server
    /// command, as on retail's dedicated servers.
    #[arg(long)]
    no_console: bool,

    /// Log one line per snapshot per client: send interval, the serverTime and
    /// commandTime a client predicts from, the usercmds consumed, and whether
    /// the frame went out as a delta. Also one `tick:` line per second: the
    /// slowest tick and how far the loop ran behind its schedule.
    #[arg(long)]
    trace: bool,
}

/// `sv_fps 20`, the loop's slot until the game names another
/// (`GameModule::wall_frame_ms`).
const FRAME: Duration = Duration::from_millis(50);

/// How far behind the schedule one tick still catches up before the loop
/// resynchronises instead: a dedicated `Com_Frame` hands `SV_Frame` at
/// most 5000 ms (cod_lnxded 0x806d0c5).
const MAX_CATCH_UP: Duration = Duration::from_secs(5);

/// Without a bound a flood keeps the socket readable, `tick` never runs and
/// the outbox is never flushed.
const MAX_PACKETS_PER_FRAME: usize = 256;

/// Packets the reader thread holds for the tick; past this it blocks and the
/// socket's own buffer drops the excess.
const PACKET_QUEUE: usize = 4 * MAX_PACKETS_PER_FRAME;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;
    // `Com_AddStartupCommands` queues the command line in order, so a set
    // after `+map` runs again once the level is up (map-cycle doc 4.5).
    let map_index = matches.index_of("map").unwrap_or(0);
    let sets_after_map: Vec<bool> = matches
        .indices_of("set")
        .map(|i| i.map(|i| i > map_index).collect())
        .unwrap_or_default();
    log::info!(
        "vcod-server {}{}",
        vcod_server::VERSION,
        if args.vanilla { " (vanilla)" } else { "" }
    );
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("vcod-server {} panicked", vcod_server::VERSION);
        default_panic(info);
    }));
    let trace = args.trace;
    let console = (!args.no_console).then(|| {
        vcod_server::stdin_console::StdinConsole::spawn(std::io::BufReader::new(std::io::stdin()))
    });
    let game = vcod_common::game::Game::resolve(args.game, &args.game_dir);
    if game == vcod_common::game::Game::Cod2 {
        let (mut server, net) = cod2_server(args)?;
        return serve(&mut server, net, console, trace);
    }
    if game == vcod_common::game::Game::Cod4 {
        let (mut server, net) = cod4_server(args)?;
        return serve(&mut server, net, console, trace);
    }
    game.ensure_supported()?;
    let (mut server, net) = cod1_server(args, &sets_after_map)?;
    serve(&mut server, net, console, trace)
}

/// CoD1 (1.1, 1.5, UO): the install's paks, the map and its scripts
/// loaded, listening.
fn cod1_server(args: Args, sets_after_map: &[bool]) -> Result<(Server, Net)> {
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
    // `FS_SetRestrictions` (cod_lnxded 0x80713e8, 1.5b 0x8075af8, UO
    // 0x807625a): a non-zero `fs_restrict` runs the demo's data only. 1.1
    // and 1.5 search `demomain` for `main`, UO keeps its base, and every pak
    // must be the demo's, so a full install refuses to start
    // (docs/research/cod11-server-handshake.md, "fs_restrict").
    let restricted = last_set(&args.set, "fs_restrict")
        .is_some_and(|v| vcod_server::game::builtins::cvar::atof(v) != 0.0);
    let base = if restricted && args.mod_dir == "main" {
        "demomain"
    } else {
        args.mod_dir.as_str()
    };
    if restricted {
        log::info!("Running in restricted demo mode.");
    }
    let dir = args.game_dir.join(base);
    // `--set fs_game=<dir>` layers that directory over the base, as
    // retail's `+set fs_game` does; the last one wins, as `--set` does.
    let fs_game = last_set(&args.set, "fs_game").filter(|v| !v.is_empty());
    if args.vanilla
        && let Some(g) = fs_game
    {
        bail!("--vanilla runs no mod, but fs_game is {g}");
    }
    let game = fs_game.map(|g| args.game_dir.join(g));
    let fs = std::rc::Rc::new(
        Pk3Fs::open_search(&dir, game.as_deref(), None, args.vanilla)
            .with_context(|| format!("opening game data in {}", dir.display()))?,
    );
    if restricted {
        if let Some(p) = fs.paks().find(|p| p.checksum as u32 != DEMO_PAK_CHECKSUM) {
            bail!("Corrupted pak0.pk3: {}", p.checksum as u32);
        }
        if !fs.contains("default_mp.cfg") {
            bail!(
                "Couldn't load default_mp.cfg.  Make sure Call of Duty is run from the correct folder."
            );
        }
    }
    // A missing or corrupt map fails here rather than at first use.
    let bsp = vcod_common::mapworld::find(&*fs, &args.map)
        .with_context(|| format!("loading the map from {}", dir.display()))?;

    let (gametype, overlay) = gametype_overlay(&args)?;

    let (sock, packets) = listen(args.port)?;
    log::info!("vcod-server: {} on udp/{}", args.map, args.port);
    let seed = wall_seed();
    let protocol = protocol(&args.set, &fs)?;
    log::info!(
        "speaking protocol {} (patch {})",
        protocol.transport.version,
        protocol.transport.short_version
    );
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
    server.set_protocol(protocol);
    server.build_nav_in_background(true);
    server.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    if let Some((stem, text)) = &overlay {
        server.overlay_script(&format!("maps/mp/gametypes/{stem}"), text);
    }
    // The engine cvars a retail server fills in from its command line and
    // host, for `cvarlist` and a query to read; `--set` still outranks them.
    let base = args.game_dir.display().to_string();
    for (name, value) in [
        ("net_port", args.port.to_string()),
        ("fs_basepath", base.clone()),
        ("fs_homepath", base),
    ] {
        server.set_cvar(name, &value);
    }
    if let Some(path) = args.ban_file {
        server.set_bans(vcod_server::engine::bans::BanFile::open(path));
    }
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
    for (pair, _) in args.set.iter().zip(sets_after_map).filter(|(_, a)| **a) {
        if let Some((name, value)) = pair.split_once('=') {
            server.push_set(name, value);
        }
    }
    Ok((server, (sock, packets)))
}

/// `--game cod2`: the CoD2 engine (`vcod_server::cod2`) over the
/// install's iwds, the level loaded, listening.
fn cod2_server(args: Args) -> Result<(vcod_server::cod2::Cod2Server, Net)> {
    let fs = std::rc::Rc::new(
        Pk3Fs::open_iwd(&args.game_dir.join(&args.mod_dir), None)
            .with_context(|| format!("opening the iwds in {}", args.game_dir.display()))?,
    );
    let seed = wall_seed();
    let (gametype, overlay) = gametype_overlay(&args)?;
    let mut server = vcod_server::cod2::Cod2Server::new(
        vcod_server::cod2::Cod2Config {
            map: args.map.clone(),
            gametype,
            hostname: args.hostname.clone(),
            max_clients: args.max_clients,
            gametype_script: overlay
                .map(|(stem, text)| (format!("maps/mp/gametypes/{stem}"), text)),
        },
        seed,
        Instant::now(),
    );
    server.exec_default_cfg(&*fs);
    if let Some(path) = &args.ban_file {
        server.set_ban_file(path.clone());
    }
    // `games_mp.log` goes under the home path; `--set` outranks it.
    server.set_cvar("fs_homepath", &args.game_dir.to_string_lossy());
    for pair in &args.set {
        let Some((name, value)) = pair.split_once('=') else {
            bail!("--set takes NAME=VALUE, got {pair:?}");
        };
        server.set_cvar(vcod_server::rcon::login_cvar_name(name), value);
    }
    server.set_cvar("net_port", &args.port.to_string());
    // `download` serves from `fs_homepath`, then `fs_basepath` (cod2-protocol.md 17.5).
    server.set_cvar("fs_basepath", &args.game_dir.to_string_lossy());
    server.load(Some(fs)).context("loading the level")?;
    if args.bots > 0 {
        server.add_bots(args.bots, args.bots_shoot);
    }
    let (sock, packets) = listen(args.port)?;
    let out = sock.try_clone().context("cloning the socket")?;
    server.set_net_now(vcod_server::cod2::NetNow {
        send: Box::new(move |to, pkt| {
            if let Err(e) = out.send_to(pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }),
        sleep: Box::new(net_sleep),
    });
    log::info!("vcod-server: CoD2 {} on udp/{}", args.map, args.port);
    Ok((server, (sock, packets)))
}

/// `--game cod4`: a CoD4 1.7 level (`vcod_server::cod4`) over the
/// install's zones and iwds, listening.
fn cod4_server(args: Args) -> Result<(vcod_server::cod4::Cod4Server, Net)> {
    let install = vcod_cod4::install::Install::at(&args.game_dir)
        .with_context(|| format!("{} has no zone/ directory", args.game_dir.display()))?;
    let gametype = args.gametype.clone().unwrap_or_else(|| "war".into());
    let mut server = vcod_server::cod4::Cod4Server::new(
        vcod_server::cod4::Cod4Config {
            map: args.map.clone(),
            gametype,
            hostname: args.hostname.clone(),
            max_clients: args.max_clients,
        },
        wall_seed(),
    );
    // The paths a retail server fills in from its command line and host:
    // `games_mp.log` and `exec` read them; `--set` outranks them.
    let base = args.game_dir.display().to_string();
    server.set_cvar("fs_basepath", &base);
    server.set_cvar("fs_homepath", &base);
    for pair in &args.set {
        let Some((name, value)) = pair.split_once('=') else {
            bail!("--set takes NAME=VALUE, got {pair:?}");
        };
        server.set_cvar(vcod_server::rcon::login_cvar_name(name), value);
    }
    server.set_cvar("net_port", &args.port.to_string());
    if let Some(path) = &args.ban_file {
        server.set_ban_file(path.clone());
    }
    if let Some(dir) = &args.script_dir {
        server.set_script_overrides(dir.clone());
    }
    server.load(Some(&install)).context("loading the level")?;
    if args.bots > 0 {
        server.add_bots(args.bots, args.bots_shoot);
    }
    let (sock, packets) = listen(args.port)?;
    let out = sock.try_clone().context("cloning the socket")?;
    server.set_net_now(vcod_server::cod2::NetNow {
        send: Box::new(move |to, pkt| {
            if let Err(e) = out.send_to(pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }),
        sleep: Box::new(net_sleep),
    });
    log::info!("vcod-server: CoD4 {} on udp/{}", args.map, args.port);
    Ok((server, (sock, packets)))
}

/// `--gametype-script <file>`: the gametype to run, `<stem>`, and the
/// script overlaid at `maps/mp/gametypes/<stem>`; else `--gametype`.
fn gametype_overlay(args: &Args) -> Result<(String, Option<(String, String)>)> {
    let Some(path) = &args.gametype_script else {
        return Ok((args.gametype.clone().unwrap_or_else(|| "dm".into()), None));
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("--gametype-script needs a file name")?
        .to_string();
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((stem.clone(), Some((stem, text))))
}

/// The wall clock in nanoseconds: the server's seed, as retail seeds off
/// the clock.
fn wall_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Packets the reader thread has read, for a wait that a packet ends
/// (`NET_Sleep`'s `select`).
static ARRIVALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Up to `d`, or until the reader thread reads a packet.
fn net_sleep(d: std::time::Duration) {
    let seen = ARRIVALS.load(std::sync::atomic::Ordering::Relaxed);
    let until = Instant::now() + d;
    while Instant::now() < until && ARRIVALS.load(std::sync::atomic::Ordering::Relaxed) == seen {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Binds the port and starts the reader thread. A thread reads the socket
/// as packets arrive so each carries its arrival time
/// (`engine::Arrival`); the loop handles them while it waits for
/// the next frame, and those that came in during a tick that overran its
/// slot at the next tick's start. Either way each is answered before the
/// loop moves on (`handle_queued`).
fn listen(port: u16) -> Result<Net> {
    let sock = UdpSocket::bind(("0.0.0.0", port)).with_context(|| format!("binding udp/{port}"))?;
    let (packet_tx, packets) = std::sync::mpsc::sync_channel(PACKET_QUEUE);
    let reader = sock.try_clone().context("cloning the socket")?;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        loop {
            match reader.recv_from(&mut buf) {
                Ok((n, from)) => {
                    ARRIVALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if packet_tx
                        .send((Instant::now(), from, buf[..n].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) => log::debug!("recv: {e}"),
            }
        }
    });
    Ok((sock, packets))
}

/// The frame loop, until `quit`. `console` is the stdin console's lines,
/// `None` under `--no-console`.
fn serve<G: GameModule>(
    server: &mut Engine<G>,
    (sock, packets): Net,
    console: Option<vcod_server::stdin_console::StdinConsole>,
    trace: bool,
) -> Result<()> {
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
    // `VCOD_TICK_TRACE=<path>`: the tick's step log (`tick_trace`); the
    // loop runs on this thread.
    if let Some(path) = std::env::var_os("VCOD_TICK_TRACE") {
        vcod_server::tick_trace::install(std::path::Path::new(&path))
            .context("opening VCOD_TICK_TRACE")?;
    }
    // `VCOD_TICK_PROFILE_MS=<ms>`: each tick past it logged with its
    // phases and how its wall time split (`tick_profile`).
    let profile_ms: Option<f32> = std::env::var("VCOD_TICK_PROFILE_MS")
        .ok()
        .and_then(|v| v.parse().ok());
    if profile_ms.is_some() {
        vcod_server::tick_profile::enable();
    }
    let mut quit_queued = false;
    // A fixed schedule, not a sleep after each tick: `SV_Frame` runs one
    // game frame per `sv_fps` slice of wall time and catches up when a
    // frame overran, so `svs.time` tracks the wall clock. Sleeping the
    // remainder of each tick lets every overshoot accumulate, which read as
    // a serverTime 5-10% slow against a probe's wall clock under load. The
    // frames owed run in one tick with one round of snapshots, as
    // `SV_Frame`'s loop does (map-cycle doc, 4.7).
    let mut schedule = Schedule::new(Instant::now());
    let mut stats = TickStats::default();
    // When retail's event loop would be free to handle the next packet: the
    // end of the last one handled. Its frame runs in well under a
    // millisecond, so a tick does not hold it; an rcon `map_restart` does.
    let mut free_at = schedule.next;
    let mut send = |to: std::net::SocketAddr, pkt: &[u8]| {
        if let Err(e) = sock.send_to(pkt, to) {
            log::debug!("send to {to}: {e}");
        }
    };
    loop {
        let now = Instant::now();
        let sched = profile_ms.and_then(|_| vcod_server::tick_profile::sched());
        let late = now.saturating_duration_since(schedule.next);
        let packets_phase = vcod_server::tick_profile::phase("packets");
        for _ in 0..MAX_PACKETS_PER_FRAME {
            let Ok(queued) = packets.try_recv() else {
                break;
            };
            handle_queued(server, queued, now, &mut free_at, &mut send);
        }
        drop(packets_phase);
        if let Some(console) = &console {
            console.queue(server);
        }
        if interrupted.load(std::sync::atomic::Ordering::Relaxed) && !quit_queued {
            quit_queued = true;
            server.push_console("quit");
        }
        // The console first, then the count: what it ran (a `map_restart`)
        // is owed by this tick's frames, as `Com_Frame` charges the time
        // `Cbuf_Execute` took to the next `SV_Frame` (0x806d003, 0x806d0e4).
        G::drain_console(server, now);
        // CoD4's `sv_fps`, read every `SV_Frame` (cod4-server-frame.md 8).
        schedule.frame = Duration::from_millis(G::wall_frame_ms(server).max(1) as u64);
        let frames = schedule.owed(Instant::now());
        server.tick_frames(now, frames);
        if let Some(min) = profile_ms {
            let ms = now.elapsed().as_secs_f32() * 1000.0;
            let phases = vcod_server::tick_profile::take();
            if ms > min && frames > 0 {
                log::warn!(
                    "tick {ms:.1} ms, {frames} frames ({}): {}",
                    vcod_server::tick_profile::split(sched, ms),
                    vcod_server::tick_profile::format(&phases, 1.0)
                );
            }
        }
        if trace {
            stats.add(now.elapsed(), late, frames);
        }
        // A level load that failed with the level already torn down: there is
        // no script to end the level and no table for a client to pull, which
        // is what retail's `Com_Error` ends the process for.
        if let Some(e) = G::take_fatal(server) {
            log::error!("{e:#}");
            std::process::exit(1);
        }
        for (to, pkt) in server.take_outgoing() {
            send(to, &pkt);
        }
        G::frame_sent(server, Instant::now());
        if server.quit_requested() {
            vcod_server::tick_trace::finish().context("writing VCOD_TICK_TRACE")?;
            return Ok(());
        }
        schedule.ran(frames);
        if trace && stats.due() {
            stats.log_and_reset();
        }
        // Between frames the loop waits on the socket, as `Com_EventLoop`
        // does in `NET_Sleep`: a packet is handled when it arrives and what
        // it answers (a `connectResponse`, a gamestate) goes out at once, so
        // a client's reply to it can land before the next frame. Answered
        // only at the tick, a joining client's first message was a frame's
        // bare one rather than its gamestate.
        while let Some(left) = schedule.next.checked_duration_since(Instant::now()) {
            let Ok(queued) = packets.recv_timeout(left) else {
                break;
            };
            handle_queued(server, queued, Instant::now(), &mut free_at, &mut send);
        }
    }
}

/// The loop's fixed frame grid: slot `n` starts at `start + n * FRAME`
/// whatever each tick took or each wait overslept, so the snapshot cadence
/// carries only the wake-up's own lateness and never accumulates it
/// (protocol-1.1.md, "The server's frame cadence").
struct Schedule {
    /// When the next frame's slot starts.
    next: Instant,
    /// A slot's length.
    frame: Duration,
}

impl Schedule {
    fn new(start: Instant) -> Self {
        Self {
            next: start,
            frame: FRAME,
        }
    }

    /// The frames owed at `now`: the slot due plus every whole slot
    /// already past. A stall past `MAX_CATCH_UP` (a debugger, a suspend)
    /// is dropped rather than replayed, and the grid restarts at `now`.
    fn owed(&mut self, now: Instant) -> u32 {
        let behind = now.saturating_duration_since(self.next);
        if behind > MAX_CATCH_UP {
            self.next = now;
            return 1;
        }
        1 + (behind.as_millis() / self.frame.as_millis()) as u32
    }

    /// Moves the grid past the frames a tick ran.
    fn ran(&mut self, frames: u32) {
        self.next += self.frame * frames;
    }
}

/// A packet off the reader thread: its arrival time, sender and bytes.
type Queued = (Instant, std::net::SocketAddr, Vec<u8>);

/// The bound socket and the reader thread's packets ([`listen`]).
type Net = (UdpSocket, std::sync::mpsc::Receiver<Queued>);

/// One queued packet, handled as `Com_EventLoop` handles it: at the moment
/// retail's loop would be free for it (`free_at`, moved past it), and
/// answered at once through `send`, as `NET_SendPacket` writes the socket
/// inside the handler. A reply held for the tick that follows went out a
/// whole tick late when the last one overran its slot.
fn handle_queued<G: GameModule>(
    server: &mut Engine<G>,
    (arrived, from, pkt): Queued,
    now: Instant,
    free_at: &mut Instant,
    send: &mut impl FnMut(std::net::SocketAddr, &[u8]),
) {
    // Retail handles a packet within 5 ms of its arrival unless an earlier
    // packet still holds the loop; vcod reads them between ticks, so the
    // handling moment is rebuilt here.
    let read = arrived.max(*free_at);
    let started = Instant::now();
    server.packet_read(from, &pkt, Arrival { now, arrived, read });
    *free_at = read + started.elapsed();
    for (to, pkt) in server.take_outgoing() {
        send(to, &pkt);
    }
}

/// The protocol to host: `--set protocol=<1|6|22>`, else the install's
/// ([`vcod_server::server::install_protocol`]). One per process, as each
/// retail binary speaks one.
/// The CoD demo's `pak0.pk3` checksum, the only pak `fs_restrict` admits
/// (cod_lnxded 0x8071461; 1.5b and UO compare it xor 0x2261994).
const DEMO_PAK_CHECKSUM: u32 = 0xb1f5_95f5;

/// The last `--set name=value`'s value for `name`, as the last `+set` wins.
fn last_set<'a>(sets: &'a [String], name: &str) -> Option<&'a str> {
    sets.iter()
        .filter_map(|p| p.split_once('='))
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
        .next_back()
}

fn protocol(sets: &[String], fs: &Pk3Fs) -> Result<&'static Protocol> {
    if let Some(v) = last_set(sets, "protocol") {
        return v
            .trim()
            .parse()
            .ok()
            .and_then(|v| Protocol::by_version(vcod_common::game::Game::Cod1, v))
            .with_context(|| {
                format!("--set protocol={v}: vcod speaks 1 (1.1), 6 (1.5) and 22 (UO 1.51)")
            });
    }
    Ok(vcod_server::server::install_protocol(fs))
}

/// `--trace`'s per-second tick summary, logged once 20 frames have run: the
/// loop's ticks and the frames they ran (a catch-up tick runs several), the
/// slowest tick and the worst lag of a tick's start behind its slot on the
/// fixed schedule.
#[derive(Default)]
struct TickStats {
    ticks: u32,
    frames: u32,
    sum: Duration,
    max: Duration,
    late: Duration,
}

impl TickStats {
    fn add(&mut self, took: Duration, late: Duration, frames: u32) {
        self.ticks += 1;
        self.frames += frames;
        self.sum += took;
        self.max = self.max.max(took);
        self.late = self.late.max(late);
    }

    /// A second of frames has run, however many ticks it took.
    fn due(&self) -> bool {
        self.frames >= 1000 / FRAME.as_millis() as u32
    }

    fn log_and_reset(&mut self) {
        log::info!(
            "tick: {} ticks {} frames mean {:.2} ms max {:.2} ms late {:.2} ms",
            self.ticks,
            self.frames,
            self.sum.as_secs_f64() * 1000.0 / f64::from(self.ticks.max(1)),
            self.max.as_secs_f64() * 1000.0,
            self.late.as_secs_f64() * 1000.0
        );
        *self = TickStats::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// A catch-up tick counts every frame it ran: ten ticks of two frames
    /// are the second's summary, as twenty ticks of one are.
    #[test]
    fn tick_stats_count_frames_not_ticks() {
        let mut stats = TickStats::default();
        for _ in 0..9 {
            stats.add(Duration::from_millis(3), Duration::ZERO, 2);
        }
        assert!(!stats.due());
        stats.add(Duration::from_millis(5), Duration::ZERO, 2);
        assert!(stats.due());
        assert_eq!((stats.ticks, stats.frames), (10, 20));
        stats.log_and_reset();
        assert!(!stats.due());
    }

    /// The loop on a simulated clock: each wake overshoots its slot by up
    /// to 4 ms and each tick takes up to 8 ms. Every snapshot goes out within
    /// that much of its grid slot and 2000 ticks run exactly 2000 frames, so
    /// neither the wake nor the tick's length drifts the cadence.
    #[test]
    fn the_frame_grid_never_drifts() {
        let start = Instant::now();
        let mut schedule = Schedule::new(start);
        let mut rng = 0x2545_f491_u32;
        let mut draw = |max_us: u32| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Duration::from_micros(u64::from((rng >> 8) % max_us))
        };
        let mut now = start;
        let mut ran = 0;
        let mut sent = Vec::new();
        for _ in 0..2000 {
            now = now.max(schedule.next) + draw(4000);
            let frames = schedule.owed(now);
            assert_eq!(frames, 1);
            ran += frames;
            now += draw(8000);
            sent.push(now);
            schedule.ran(frames);
        }
        assert_eq!(schedule.next, start + FRAME * ran);
        for (n, at) in sent.iter().enumerate() {
            let off = at.duration_since(start + FRAME * n as u32);
            assert!(off < Duration::from_millis(12), "frame {n} {off:?} late");
        }
    }

    /// A tick that overran its slot is caught up in the next one, back on
    /// the grid; a stall past the catch-up bound restarts the grid instead.
    #[test]
    fn an_overrun_is_caught_up_on_the_grid() {
        let start = Instant::now();
        let mut schedule = Schedule::new(start);
        assert_eq!(schedule.owed(start), 1);
        schedule.ran(1);
        // The first tick ran 120 ms: slots 1 and 2 are both owed at once.
        let now = start + Duration::from_millis(120);
        assert_eq!(schedule.owed(now), 2);
        schedule.ran(2);
        assert_eq!(schedule.next, start + FRAME * 3);
        let stalled = start + Duration::from_secs(10);
        assert_eq!(schedule.owed(stalled), 1);
        schedule.ran(1);
        assert_eq!(schedule.next, stalled + FRAME);
    }

    /// A packet handled at a tick's start is answered there, not after the
    /// tick that follows: an rcon or `getinfo` reply goes out with no frame
    /// between it and its request.
    #[test]
    fn a_queued_packet_is_answered_before_the_tick() {
        let now = Instant::now();
        let mut server = Server::new(
            ServerConfig {
                map: "mp_carentan".into(),
                hostname: "vcod test".into(),
                max_clients: 4,
                gametype: "dm".into(),
                test_entities: 0,
                trace: false,
                bots: 0,
                bots_shoot: false,
            },
            now,
        );
        let from = SocketAddr::from(([127, 0, 0, 1], 5));
        let mut pkt = vec![0xff; 4];
        pkt.extend_from_slice(b"getinfo 4242");
        let mut sent = Vec::new();
        let mut free_at = now;
        handle_queued(
            &mut server,
            (now, from, pkt),
            now,
            &mut free_at,
            &mut |to, p| sent.push((to, p.to_vec())),
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, from);
        assert!(sent[0].1.windows(12).any(|w| w == b"infoResponse"));
        assert!(server.take_outgoing().is_empty());
        assert!(free_at >= now);
    }
}
