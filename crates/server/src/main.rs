//! `vcod-server`, the dedicated server for CoD1 1.1 and 1.5, United
//! Offensive, CoD2 1.3 and CoD4 1.7. Answers browsers, accepts retail
//! clients, runs the stock gametype scripts on the gsc VM and sends
//! delta-compressed snapshots.

use anyhow::{Context, Result, bail};
use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use vcod_common::game::Game;
use vcod_common::net::protocol::Cod1Wire;
use vcod_common::pk3::Pk3Fs;
use vcod_server::engine::{Arrival, Engine, GameModule};
use vcod_server::{Server, ServerConfig};

#[derive(Parser)]
#[command(about = env!("CARGO_PKG_DESCRIPTION"), version = env!("VCOD_VERSION"))]
struct Args {
    /// Map name, e.g. mp_carentan
    map: String,
    /// Game install; defaults to $COD_DIR, else the executable's directory
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// CoD1 and CoD2: the data directory under the install, main, or uo
    /// for United Offensive
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// Which game the install is: cod1 (1.1, 1.5 and UO), cod2 or cod4.
    /// Defaults to what the game dir holds
    #[arg(long)]
    game: Option<vcod_common::game::Game>,
    /// UDP port
    #[arg(long, default_value_t = 28960)]
    port: u16,
    /// sv_hostname
    #[arg(long, default_value = "vcod")]
    hostname: String,
    /// sv_maxclients, 1 to 64; bots count
    #[arg(long, default_value_t = 8,
          value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=64))]
    max_clients: usize,
    /// g_gametype: the script under maps/mp/gametypes to run; dm, or war
    /// for cod4, where a `--set g_gametype=` wins over it
    #[arg(long)]
    gametype: Option<String>,
    /// CoD1 and CoD2: a gametype script from disk instead of the paks: overlaid at
    /// maps/mp/gametypes/<stem> and run as gametype <stem>. For the client
    /// probes in crates/gsc/tests/fixtures/semantics/client-probes/.
    #[arg(long, help_heading = "Developer")]
    gametype_script: Option<std::path::PathBuf>,
    /// CoD4: a directory whose loose .gsc files each level reads over the
    /// zones' scripts (maps/mp/mp_crash.gsc, say), as retail reads a mod's.
    /// For a probe or a perf run's scripted scene.
    #[arg(long, help_heading = "Developer")]
    script_dir: Option<std::path::PathBuf>,
    /// CoD1: scripted entities that exercise the packet-entity wire path.
    /// 0 is off.
    #[arg(long, default_value_t = 0, help_heading = "Developer")]
    test_entities: usize,
    /// Bots to add at start. Each takes a client slot; 0 is off. They fight
    /// unless --bots-peaceful (docs/bots.md).
    #[arg(long, default_value_t = 0)]
    bots: usize,
    /// The --bots only roam and play objectives: `vc_bot_shoot 0`.
    #[arg(long)]
    bots_peaceful: bool,
    /// Fighting bots, the default now; kept so old command lines run.
    #[arg(long, hide = true, conflicts_with = "bots_peaceful")]
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
    /// Where the bots' navigation graphs are kept between runs, keyed by
    /// the map file and this binary. Default: navcache/ beside the binary,
    /// unless that is inside the game dir, where none is kept.
    #[arg(long)]
    nav_cache: Option<std::path::PathBuf>,
    /// CoD1 and UO: load only the install's stock retail paks (the patch's numbered paks,
    /// their `localized_*` ones, UO's map pack) and ignore every other pk3:
    /// third-party maps, `zzz_*` mods, downloads. Refuses `fs_game`.
    #[arg(long)]
    vanilla: bool,
    /// Leave stdin alone. By default each line on stdin runs as a server
    /// command, as on retail's dedicated servers.
    #[arg(long)]
    no_console: bool,
    /// stdin is the pipe of the client that started this server (Start New
    /// Server): its end, the client gone, runs `quit`.
    #[arg(long, hide = true, conflicts_with = "no_console")]
    quit_on_eof: bool,

    /// Log one line per snapshot per client: send interval, the serverTime and
    /// commandTime a client predicts from, the usercmds consumed, and whether
    /// the frame went out as a delta. Also one `tick:` line per second: the
    /// slowest tick and how far the loop ran behind its schedule.
    #[arg(long, help_heading = "Developer")]
    trace: bool,
}

/// `--nav-cache`, else `navcache/` beside the binary when that is not
/// inside the game install.
fn nav_cache_dir(args: &Args) -> Option<std::path::PathBuf> {
    if let Some(dir) = &args.nav_cache {
        return Some(dir.clone());
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    let game = args.game_dir.canonicalize().ok()?;
    (!dir.starts_with(&game)).then(|| dir.join("navcache"))
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
    // Only tests and gates read the script log.
    vcod_server::game::host::KEEP_SCRIPT_LOG.store(false, std::sync::atomic::Ordering::Relaxed);
    let matches = Args::command().get_matches();
    let args = Args::from_arg_matches(&matches)?;
    let game = Game::resolve(args.game, &args.game_dir);
    // Everything that can fail without the game's data fails here, before
    // a level loads for seconds.
    check_game_flags(&matches, game)?;
    check_args(&args)?;
    vcod_server::nav::set_disk_cache(nav_cache_dir(&args));
    if game == Game::Cod1 {
        set_protocol(&args.set)?;
    }
    // A restricted (demo) install reads `demomain`, which `cod1_server` checks.
    if last_set(&args.set, "fs_restrict").is_none() {
        let chosen = vcod_common::game_dir::chosen_by(from_command_line(&matches, "game_dir"));
        vcod_common::game_dir::check_install(&args.game_dir, game, &args.mod_dir, chosen)?;
    }
    let tty = tty_console_wanted(&args, game);
    init_log(tty);
    let sock = bind(last_set(&args.set, "net_ip"), args.port)?;
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
        // A boundary catches it and the server runs on (`engine::contain`).
        if vcod_server::engine::contain::catching() {
            if vcod_server::engine::contain::loud() {
                log::error!("{info}");
            }
            return;
        }
        #[cfg(unix)]
        vcod_server::tty_console::restore();
        eprintln!("vcod-server {} panicked", vcod_server::VERSION);
        default_panic(info);
    }));
    let trace = args.trace;
    // Ahead of the console, which may put the terminal in raw mode.
    let interrupt = Interrupt::install()?;
    let console = (!args.no_console).then(|| {
        if args.quit_on_eof {
            vcod_server::stdin_console::StdinConsole::spawn_child(std::io::BufReader::new(
                std::io::stdin(),
            ))
        } else {
            stdin_console(tty)
        }
    });
    if game == Game::Cod2 {
        let (mut server, net) = cod2_server(args, sock)?;
        return serve(&mut server, net, console, trace, interrupt);
    }
    if game == Game::Cod4 {
        let (mut server, net) = cod4_server(args, sock)?;
        return serve(&mut server, net, console, trace, interrupt);
    }
    let (mut server, net) = cod1_server(args, sock, &sets_after_map)?;
    serve(&mut server, net, console, trace, interrupt)
}

/// Ctrl-C and SIGTERM. Before the frame loop runs one restores the
/// terminal and exits; in the loop the first runs `quit`, so the masters
/// get their flatline, and a second exits at once.
struct Interrupt {
    hit: std::sync::Arc<std::sync::atomic::AtomicBool>,
    serving: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Interrupt {
    fn install() -> Result<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let hit = std::sync::Arc::new(AtomicBool::new(false));
        let serving = std::sync::Arc::new(AtomicBool::new(false));
        let (h, s) = (hit.clone(), serving.clone());
        ctrlc::set_handler(move || {
            if !s.load(Ordering::Relaxed) || h.swap(true, Ordering::Relaxed) {
                #[cfg(unix)]
                vcod_server::tty_console::restore();
                std::process::exit(130);
            }
        })
        .context("installing the Ctrl-C handler")?;
        Ok(Interrupt { hit, serving })
    }
}

/// Whether `id` was given on the command line rather than defaulted.
fn from_command_line(matches: &ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

/// The flags only some games read, by clap id, and those games.
const GAME_FLAGS: [(&str, &[Game]); 5] = [
    ("vanilla", &[Game::Cod1]),
    ("mod_dir", &[Game::Cod1, Game::Cod2]),
    ("gametype_script", &[Game::Cod1, Game::Cod2]),
    ("test_entities", &[Game::Cod1]),
    ("script_dir", &[Game::Cod4]),
];

/// Refuses a flag `game` would accept and ignore.
fn check_game_flags(matches: &ArgMatches, game: Game) -> Result<()> {
    for (id, games) in GAME_FLAGS {
        // `--mod-dir main` names the default, which every game reads.
        let default =
            id == "mod_dir" && matches.get_one::<String>(id).map(String::as_str) == Some("main");
        if !games.contains(&game) && from_command_line(matches, id) && !default {
            let names: Vec<String> = games.iter().map(|g| format!("{g:?}")).collect();
            bail!(
                "--{} is {} only; this is {game:?}",
                id.replace('_', "-"),
                names.join(" and ")
            );
        }
    }
    Ok(())
}

impl Args {
    /// Whether the --bots fire: unless --bots-peaceful.
    fn bots_fire(&self) -> bool {
        self.bots_shoot || !self.bots_peaceful
    }
}

/// The argument checks that need no game data.
fn check_args(args: &Args) -> Result<()> {
    if let Some(pair) = args.set.iter().find(|p| !p.contains('=')) {
        bail!("--set takes NAME=VALUE, got {pair:?}");
    }
    if args.bots > args.max_clients {
        bail!(
            "--bots {} exceeds --max-clients {}; each bot takes a real slot",
            args.bots,
            args.max_clients
        );
    }
    if args.test_entities > vcod_server::world::MAX_TEST_ENTITIES {
        bail!(
            "--test-entities {} exceeds {}, the most that fits below the map's entities",
            args.test_entities,
            vcod_server::world::MAX_TEST_ENTITIES
        );
    }
    let first_test = vcod_server::world::FIRST_TEST_ENT as usize;
    if args.test_entities > 0 && args.max_clients > first_test {
        bail!(
            "--test-entities needs --max-clients {first_test} or fewer: its entities start at {first_test}"
        );
    }
    Ok(())
}

/// Whether stdin gets CoD1's tty line editor: a terminal, `ttycon` not
/// set to 0, and CoD1, as CoD2 1.3 and CoD4 1.7 force `ttycon` off
/// (cod11-server-handshake.md, "The stdin console").
fn tty_console_wanted(args: &Args, game: vcod_common::game::Game) -> bool {
    #[cfg(unix)]
    {
        !args.no_console
            && game == vcod_common::game::Game::Cod1
            && last_set(&args.set, "ttycon")
                .is_none_or(|v| vcod_server::game::builtins::cvar::atof(v) != 0.0)
            && vcod_server::tty_console::is_tty()
    }
    #[cfg(not(unix))]
    {
        let _ = (args, game);
        false
    }
}

/// The log on stderr, control characters escaped. Under the tty console
/// it goes through the writer that takes the typed line down around each
/// record, coloured when stderr is a terminal unless `RUST_LOG_STYLE`
/// says otherwise.
fn init_log(tty: bool) {
    let mut b = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    vcod_server::logfmt::escape_controls(&mut b);
    #[cfg(unix)]
    if tty {
        use std::io::IsTerminal;
        b.target(env_logger::Target::Pipe(Box::new(
            vcod_server::tty_console::LogStderr,
        )));
        if std::env::var_os("RUST_LOG_STYLE").is_none() && std::io::stderr().is_terminal() {
            b.write_style(env_logger::WriteStyle::Always);
        }
    }
    let _ = tty;
    b.init();
}

/// The stdin console: the tty editor when [`tty_console_wanted`], else
/// the line reader.
fn stdin_console(tty: bool) -> vcod_server::stdin_console::StdinConsole {
    use vcod_server::stdin_console::StdinConsole;
    #[cfg(unix)]
    if tty {
        match StdinConsole::spawn_tty() {
            Ok(c) => return c,
            Err(e) => log::warn!("tty console: {e}"),
        }
    }
    let _ = tty;
    StdinConsole::spawn(std::io::BufReader::new(std::io::stdin()))
}

/// CoD1 (1.1, 1.5, UO): the install's paks, the map and its scripts
/// loaded, listening.
fn cod1_server(args: Args, sock: UdpSocket, sets_after_map: &[bool]) -> Result<(Server, Net)> {
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
    let bsp = vcod_common::mapworld::find(&*fs, &args.map).with_context(|| {
        match vcod_common::game_dir::uo_map_hint(&args.game_dir, &args.mod_dir) {
            Some(hint) => format!("loading the map from {} ({hint})", dir.display()),
            None => format!("loading the map from {}", dir.display()),
        }
    })?;

    let (gametype, overlay) = gametype_overlay(&args)?;
    let protocol = protocol(&args.set, &fs)?;
    let map_source = fs.map_checksum(&args.map).unwrap_or(0);

    let (sock, packets) = listen(sock)?;
    log::info!("vcod-server: {} on udp/{}", args.map, args.port);
    master_hint(&args.set);
    let seed = wall_seed();
    log::info!(
        "speaking protocol {} (patch {})",
        protocol.transport.version,
        protocol.transport.short_version
    );
    if let Some(w) = vcod_common::net::protocol::patch_warning_of(fs.paks()) {
        log::warn!("{w}");
    }
    let bots_shoot = args.bots_fire();
    let mut server = Server::with_seed(
        ServerConfig {
            map: args.map,
            hostname: args.hostname,
            max_clients: args.max_clients,
            gametype,
            test_entities: args.test_entities,
            bots: args.bots,
            bots_shoot,
            trace: args.trace,
        },
        Instant::now(),
        seed,
    );
    server.set_protocol(protocol);
    server.build_nav_in_background(true);
    let mut world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
    world.collision.set_source(map_source);
    server.load_world(world);
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
    for (name, value) in args.set.iter().filter_map(|p| p.split_once('=')) {
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
fn cod2_server(args: Args, sock: UdpSocket) -> Result<(vcod_server::cod2::Cod2Server, Net)> {
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
    for (name, value) in args.set.iter().filter_map(|p| p.split_once('=')) {
        server.set_cvar(vcod_server::rcon::login_cvar_name(name), value);
    }
    server.set_cvar("net_port", &args.port.to_string());
    // `download` serves from `fs_homepath`, then `fs_basepath` (cod2-protocol.md 17.5).
    server.set_cvar("fs_basepath", &args.game_dir.to_string_lossy());
    server.load(Some(fs)).context("loading the level")?;
    if args.bots > 0 {
        server.add_bots(args.bots, args.bots_fire());
    }
    let (sock, packets) = listen(sock)?;
    let out = sock.try_clone().context("cloning the socket")?;
    server.set_net_now(vcod_server::cod2::NetNow {
        send: Box::new(move |to, pkt| {
            if let Err(e) = out.send_to(pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }),
        sleep: Box::new(net_sleep),
    });
    log::info!("vcod-server: CoD2 1.3 {} on udp/{}", args.map, args.port);
    if let Some(w) = vcod_common::game_dir::patch_warning(&args.game_dir, Game::Cod2) {
        log::warn!("{w}");
    }
    master_hint(&args.set);
    Ok((server, (sock, packets)))
}

/// `--game cod4`: a CoD4 1.7 level (`vcod_server::cod4`) over the
/// install's zones and iwds, listening.
fn cod4_server(args: Args, sock: UdpSocket) -> Result<(vcod_server::cod4::Cod4Server, Net)> {
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
    for (name, value) in args.set.iter().filter_map(|p| p.split_once('=')) {
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
        server.add_bots(args.bots, args.bots_fire());
    }
    let (sock, packets) = listen(sock)?;
    let out = sock.try_clone().context("cloning the socket")?;
    server.set_net_now(vcod_server::cod2::NetNow {
        send: Box::new(move |to, pkt| {
            if let Err(e) = out.send_to(pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }),
        sleep: Box::new(net_sleep),
    });
    log::info!("vcod-server: CoD4 1.7 {} on udp/{}", args.map, args.port);
    master_hint(&args.set);
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

/// Binds `port` on `net_ip`'s address: every interface when unset, empty
/// or `localhost`, as retail's default does. The level loads after this,
/// so a port in use fails at once; nothing reads it until [`listen`].
fn bind(net_ip: Option<&str>, port: u16) -> Result<UdpSocket> {
    let ip = bind_ip(net_ip)?;
    let sock = UdpSocket::bind((ip, port))
        .with_context(|| format!("binding {ip}:{port}: another server may hold it; pass --port"))?;
    if !ip.is_unspecified() {
        log::info!("net_ip: bound {ip}:{port}");
    }
    Ok(sock)
}

/// `net_ip`'s address: an IPv4 address or a name that has one.
fn bind_ip(net_ip: Option<&str>) -> Result<std::net::Ipv4Addr> {
    use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
    let Some(v) = net_ip
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != "localhost")
    else {
        return Ok(Ipv4Addr::UNSPECIFIED);
    };
    if let Ok(ip) = v.parse() {
        return Ok(ip);
    }
    (v, 0)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| {
            a.find_map(|a| match a {
                SocketAddr::V4(a) => Some(*a.ip()),
                SocketAddr::V6(_) => None,
            })
        })
        .with_context(|| format!("net_ip {v:?} is no IPv4 address or host name"))
}

/// Starts the reader thread on the bound socket. A thread reads the socket
/// as packets arrive so each carries its arrival time
/// (`engine::Arrival`); the loop handles them while it waits for
/// the next frame, and those that came in during a tick that overran its
/// slot at the next tick's start. Either way each is answered before the
/// loop moves on (`handle_queued`).
fn listen(sock: UdpSocket) -> Result<Net> {
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
    interrupt: Interrupt,
) -> Result<()> {
    interrupt
        .serving
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let interrupted = interrupt.hit;
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
    vcod_server::engine::contain::enable();
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
        server.exec_console(now);
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
        // no script to end the level and no table for a client to pull
        // (`Server::take_fatal` says how far that follows retail). Or the
        // level panicked too often to load it again (`engine::contain`).
        if let Some(e) = server.fatal() {
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

/// Whether `--set dedicated=` asks for 2, the only value that heartbeats.
/// vcod's default is 1 (retail's is 2), so an unset one does not.
fn heartbeats(sets: &[String]) -> bool {
    last_set(sets, "dedicated").is_some_and(|v| v.trim().parse::<i32>().ok() == Some(2))
}

/// Says at startup that a server below `dedicated 2` stays off the master list.
fn master_hint(sets: &[String]) {
    if !heartbeats(sets) {
        log::info!("not listed on the master server: --set dedicated=2 sends heartbeats");
    }
}

/// The protocol to host: `--set protocol=<1|6|22>`, else the install's
/// ([`vcod_server::server::install_protocol`]). One per process, as each
/// retail binary speaks one.
fn protocol(sets: &[String], fs: &Pk3Fs) -> Result<&'static Cod1Wire> {
    Ok(set_protocol(sets)?.unwrap_or_else(|| vcod_server::server::install_protocol(fs)))
}

/// CoD1's `--set protocol=`, if given: 1, 6 or 22.
fn set_protocol(sets: &[String]) -> Result<Option<&'static Cod1Wire>> {
    let Some(v) = last_set(sets, "protocol") else {
        return Ok(None);
    };
    v.trim()
        .parse()
        .ok()
        .and_then(|v| Cod1Wire::by_version(Game::Cod1, v))
        .map(Some)
        .with_context(|| {
            format!("--set protocol={v}: vcod speaks 1 (1.1), 6 (1.5) and 22 (UO 1.51)")
        })
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

    #[test]
    fn only_dedicated_2_heartbeats() {
        let sets = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(!heartbeats(&sets(&[])));
        assert!(!heartbeats(&sets(&["dedicated=1"])));
        assert!(heartbeats(&sets(&["Dedicated=2"])));
        assert!(!heartbeats(&sets(&["dedicated=2", "dedicated=1"])));
        assert!(!heartbeats(&sets(&["dedicated=yes"])));
    }

    fn parse(argv: &[&str]) -> (ArgMatches, Args) {
        let m = Args::command()
            .try_get_matches_from(std::iter::once("vcod-server").chain(argv.iter().copied()))
            .unwrap();
        let a = Args::from_arg_matches(&m).unwrap();
        (m, a)
    }

    /// A flag the game would accept and ignore is refused, by name; a
    /// default is not a flag given.
    #[test]
    fn flags_for_another_game_are_refused() {
        let (m, _) = parse(&["mp_crash", "--vanilla", "--mod-dir", "uo"]);
        assert!(check_game_flags(&m, Game::Cod1).is_ok());
        let e = check_game_flags(&m, Game::Cod4).unwrap_err().to_string();
        assert_eq!(e, "--vanilla is Cod1 only; this is Cod4");
        let (m, _) = parse(&["mp_crash", "--mod-dir", "uo"]);
        assert!(check_game_flags(&m, Game::Cod2).is_ok());
        let e = check_game_flags(&m, Game::Cod4).unwrap_err().to_string();
        assert_eq!(e, "--mod-dir is Cod1 and Cod2 only; this is Cod4");
        // Start New Server passes the default on every game.
        let (m, _) = parse(&["mp_crash", "--mod-dir", "main"]);
        assert!(check_game_flags(&m, Game::Cod4).is_ok());
        let (m, _) = parse(&["mp_crash"]);
        for g in [Game::Cod1, Game::Cod2, Game::Cod4] {
            assert!(check_game_flags(&m, g).is_ok());
        }
    }

    /// `--bots` fight; `--bots-peaceful` keeps them from firing, and the old
    /// `--bots-shoot` still parses.
    #[test]
    fn bots_fire_unless_peaceful() {
        let fire = |argv: &[&str]| parse(argv).1.bots_fire();
        assert!(fire(&["mp_crash", "--bots", "2"]));
        assert!(!fire(&["mp_crash", "--bots", "2", "--bots-peaceful"]));
        assert!(fire(&["mp_crash", "--bots", "2", "--bots-shoot"]));
    }

    /// The checks that need no game data, and clap's slot range.
    #[test]
    fn bad_arguments_fail_before_the_level() {
        let (_, a) = parse(&["mp_crash", "--set", "g_gametype"]);
        assert!(
            check_args(&a)
                .unwrap_err()
                .to_string()
                .contains("NAME=VALUE")
        );
        let (_, a) = parse(&["mp_crash", "--bots", "9"]);
        assert!(
            check_args(&a)
                .unwrap_err()
                .to_string()
                .contains("--max-clients 8")
        );
        let (_, a) = parse(&["mp_crash", "--bots", "9", "--max-clients", "12"]);
        assert!(check_args(&a).is_ok());
        let (_, a) = parse(&["mp_crash", "--set", "protocol=5"]);
        assert!(set_protocol(&a.set).is_err());
        let (_, a) = parse(&["mp_crash", "--set", "protocol=22"]);
        assert!(set_protocol(&a.set).unwrap().is_some());
        for n in ["0", "65"] {
            let r = Args::command().try_get_matches_from(["vcod-server", "x", "--max-clients", n]);
            assert!(r.is_err(), "--max-clients {n}");
        }
    }

    /// `net_ip` as retail reads it: unset, empty and `localhost` are every
    /// interface.
    #[test]
    fn net_ip_picks_the_bound_address() {
        use std::net::Ipv4Addr;
        for v in [None, Some(""), Some("localhost")] {
            assert_eq!(bind_ip(v).unwrap(), Ipv4Addr::UNSPECIFIED, "{v:?}");
        }
        assert_eq!(bind_ip(Some("127.0.0.1")).unwrap(), Ipv4Addr::LOCALHOST);
        // A NUL fails before any lookup, so the test needs no resolver.
        assert!(bind_ip(Some("bad\0ip")).is_err());
        let a = parse(&["x", "--set", "net_ip=1.2.3.4", "--set", "net_ip=127.0.0.1"]).1;
        assert_eq!(last_set(&a.set, "net_ip"), Some("127.0.0.1"));
    }

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
