//! `vcod-server`, a CoD 1.1 dedicated server in progress. Answers browsers,
//! accepts connections, sends the gamestate and uncompressed snapshots with
//! spectator flight.

use anyhow::{bail, Context, Result};
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
    /// Scripted entities that exercise the packet-entity wire path. 0 is off.
    #[arg(long, default_value_t = 0)]
    test_entities: usize,

    /// Log one line per snapshot per client: send interval, the serverTime and
    /// commandTime a client predicts from, the usercmds consumed, and whether
    /// the frame went out as a delta.
    #[arg(long)]
    trace: bool,
}

/// `sv_fps 20`.
const FRAME: Duration = Duration::from_millis(50);

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

    let sock = UdpSocket::bind(("0.0.0.0", args.port))
        .with_context(|| format!("binding udp/{}", args.port))?;
    sock.set_nonblocking(true)?;
    log::info!("vcod-server: {} on udp/{}", args.map, args.port);
    let mut server = Server::new(
        ServerConfig {
            map: args.map,
            hostname: args.hostname,
            max_clients: args.max_clients,
            gametype: args.gametype,
            test_entities: args.test_entities,
            trace: args.trace,
        },
        Instant::now(),
    );
    server.load_world(vcod_server::world::World::from_bsp(&bsp));
    // A failed script load is fatal, not a warning. The configstring table is
    // mostly script output now, so a server that kept serving would hand
    // clients a gamestate with no team menu, no status icons and no shaders,
    // which reads as a protocol bug rather than a script failure.
    if let Err(e) = server.load_scripts(fs.clone()) {
        log::error!("loading the map and gametype scripts: {e:#}");
        std::process::exit(1);
    }
    let mut buf = vec![0u8; 65536];
    loop {
        let now = Instant::now();
        for _ in 0..MAX_PACKETS_PER_FRAME {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                break;
            };
            server.handle_packet(from, &buf[..n], now);
        }
        server.tick(now);
        for (to, pkt) in server.take_outgoing() {
            if let Err(e) = sock.send_to(&pkt, to) {
                log::debug!("send to {to}: {e}");
            }
        }
        std::thread::sleep(FRAME.saturating_sub(now.elapsed()));
    }
}
