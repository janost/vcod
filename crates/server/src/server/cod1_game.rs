//! The server's engine/game boundary. `Server` is `Engine<Cod1Game>`: the
//! shared engine state (`crate::engine::EngineCore`) beside [`Cod1Game`],
//! which holds the engine state CoD1 still keeps itself and its game module
//! ([`Cod1Module`], what retail's `game.mp.i386.so` owns: `G_InitGame`,
//! `ClientThink`, `G_RunFrame`, the script callbacks).

use super::*;

/// CoD1 behind the engine: its game module ([`Cod1Module`]) and the engine
/// state CoD1 keeps itself, beside [`crate::engine::EngineCore`].
pub struct Cod1Game {
    pub(super) cfg: ServerConfig,
    pub(super) proto: &'static Protocol,
    /// `sv_serverid`, `0x10` per map load plus the restart count in the low
    /// nibble. u8 because the client echoes it in a one-byte header field.
    pub(super) server_id: u8,
    /// `CS_ZOMBIE` slots by index: a dropped client kept for
    /// [`ZOMBIE_TIME`] past its last packet so the drop notice reaches it.
    /// The game has already disconnected it, so it lives beside `clients`
    /// rather than in it; a slot is free only when both are `None`.
    pub(super) zombies: Vec<Option<Client>>,
    pub(super) limiter: RateLimiter,
    /// [`Server::pin_stuck_rand`]'s generator.
    pub(super) stuck_rand: Option<vcod_common::rng::GlibcRand>,
    /// The map's collision and spawn, loaded by the binary; tests run
    /// without. `Rc` so `GameHost.world` can point at the same map for
    /// `bulletTrace`, which needs real geometry to trace against.
    pub(super) world: Option<Rc<World>>,
    /// How far 1.5's `svs.time` runs ahead of the game clock: the frames
    /// a `timescale` under 1 held the game still (cod15-protocol-diff.md,
    /// section 12). Always 0 on 1.1.
    pub(super) paused_ms: i32,
    /// Gamestate entity baselines a delta frame may omit an unchanged entity
    /// against; empty when `test_entities` is off.
    pub(super) baselines: HashMap<u32, msg::EntityState>,
    /// Wall clock of the previous tick, for the trace's send-interval column.
    pub(super) last_tick: Option<Instant>,
    /// The number of the last client packet executed, bots' included: what
    /// `replay_moves` orders every client's cmds and `kill`s by.
    pub(super) packet_seq: u64,
    /// When the last frame's messages went out (`Server::frame_sent`), and
    /// the frame time of the one sent before it.
    pub(super) last_send: Option<(Instant, i32)>,
    /// The frame time of the newest frame sent, for `last_send`.
    pub(super) sent_time_ms: i32,
    /// The ack time of the packet being handled, when it differs from
    /// `sv_time_ms` (`Server::handle_packet_acked`).
    pub(super) ack_time_ms: Option<i32>,
    /// Retail's `+set name value` as `Com_StartupVariable` applies it: last
    /// in the first level's `cvars`, over `default_mp.cfg` and the config's
    /// own, so a run can turn a script cvar such as `scr_friendlyfire` on
    /// without a code change. Also where a latched `sv_maxclients` waits.
    pub(super) cvar_overrides: Vec<(String, String)>,
    /// The paks, kept past `load_scripts` because a shot loads the victim's
    /// models to trace against. `None` on a host with none.
    pub(super) fs: Option<Rc<vcod_common::pk3::Pk3Fs>>,
    /// The pak cvars of the mounted search path, systeminfo's lists.
    pub(super) paks: configstrings::PakLists,
    /// The pure checksums a `cp` is checked against, keyed with this
    /// level's `checksumFeed`; `None` until a level loads.
    pub(super) pure_check: Option<PureCheck>,
    /// `sv_mapRotationCurrent`, consumed one token per `map_rotate`.
    pub(super) rotation: console::Rotation,
    /// `sv_mapRotation`, mirrored here by `set_cvar` (doc section 5).
    pub(super) sv_map_rotation: String,
    /// `svs.snapFlagServerBit`, toggled by a map load or restart and sent as
    /// every snapshot's `snapFlags` (doc section 3 step 13, section 4
    /// step 4).
    pub(super) snap_flag_server_bit: u32,
    /// The cvar table the outgoing level left, stashed when its script is
    /// dropped. Retail's `Cvar_Set` writes a process-global table that no
    /// level boundary clears, so a script's `setCvar` outlives both a
    /// restart and a map change; rebuilding from `default_mp.cfg` alone
    /// would lose it.
    pub(super) carried_cvars: Option<crate::cvars::Cvars>,
    /// `g_gametype` and `sv_maxclients` as the running level actually loaded
    /// with, read off the table `load_scripts_with` built. Doc section 4
    /// step 3 compares retail's latched value against its live one; this is
    /// the same comparison's left-hand side, and it has to be the built
    /// value rather than `cfg`, since a `+set` override outranks `cfg` in
    /// [`Self::cvars`] and comparing against `cfg` escalates every restart
    /// for the whole run.
    pub(super) level_cvars: Option<(String, String)>,
    /// `svs.time` of the last spawn or restart, for the same-frame guard
    /// (doc section 4 step 1).
    pub(super) last_spawn_tick: Option<i32>,
    /// A level load that failed with the level already torn down
    /// ([`LoadFailure::Fatal`]), waiting for `main` to end the process on it.
    pub(super) fatal: Option<anyhow::Error>,
    /// `SV_AddTestClient`'s process-wide `bot%d` counter (cod_lnxded
    /// 0x80e2f9c), which `addTestClient` advances through the host's copy.
    pub(super) test_client_counter: u32,
    /// The frames the killcam replays, kept while script has `setarchive`
    /// on and cleared by every level load (`crate::archive`).
    pub(super) archive: crate::archive::Archive,
    /// `dedicated`. Retail's default is 2, which heartbeats; vcod's is 1 on
    /// purpose, so dev and test runs stay off the master list. `--set
    /// dedicated=2` opts in.
    pub(super) dedicated: i32,
    /// `rconPassword`; empty refuses every rcon.
    pub(super) rcon_password: String,
    /// The rcon output being collected, `Com_BeginRedirect` to
    /// `Com_EndRedirect`.
    pub(super) redirect: Option<crate::rcon::Redirect>,
    /// A `killserver` ran: `sv_running` is 0, so no packet is read and no
    /// frame runs until a console `map` loads a level again.
    pub(super) killed: bool,
    /// The script log of the level a shutdown took down, for
    /// [`Self::script_log`].
    pub(super) shutdown_log: Vec<String>,
    /// `games_mp.log` while a level runs with `g_log` set.
    pub(super) game_log: crate::engine::gamelog::LevelLog,
    /// `sv_cheats`: set by `devmap`, cleared by `map`, carried in
    /// systeminfo.
    pub(super) cheats: bool,
    /// The game module's state.
    pub(super) module: Cod1Module,
    /// The moves the tick's first frame replayed, by slot, for its
    /// snapshots; the frame being run's, from `replay_moves` to
    /// `run_frame`; `--trace`'s wall gap since the last tick and the frames
    /// it was owed.
    pub(super) tick_moved: Option<Vec<MoveSummary>>,
    pub(super) frame_moved: Vec<MoveSummary>,
    pub(super) tick_wall: (Option<f32>, u32),
}

/// The game module's state for CoD1 (1.1, 1.5 and UO): the script, the
/// tables it loaded, the shot and blast bookkeeping, and the bots. The
/// engine keeps the clients, the world's collision and the shared
/// `rand()` stream and lends them to the game's calls.
pub struct Cod1Module {
    /// Test-facing: the next rockets' `gunrandom` pairs, a retail
    /// capture's, in place of `rng`'s ([`Server::pin_spread_draws`]).
    pub(super) spread_draws: std::collections::VecDeque<(f32, f32)>,
    /// The next level's script seed, `G_InitGame`'s `Rand_Init` argument:
    /// retail passes the wall clock at every load and restart, so no level
    /// repeats the last one's draws (map-cycle doc 4.8). Stepped at every
    /// load, from `RNG_SEED` under [`Server::new`].
    pub(super) level_seed: u64,
    /// [`Server::pin_spawns`]'s queues, handed to every level's script.
    pub(super) spawn_pins: Option<crate::game::host::SpawnPins>,
    /// Scripted entities driving the packet-entity path; `None` when
    /// `cfg.test_entities` is 0.
    pub(super) test_entities: Option<TestEntities>,
    /// The map script; `None` until `load_scripts` succeeds. `tick` steps it
    /// once per frame.
    pub(super) script: Option<crate::game::script::ScriptRuntime>,
    /// The player animtree, its wire index and the animscript. `None` on a
    /// host with no paks, where every client keeps index 0.
    pub(super) anims: Option<Rc<vcod_common::animtree::PlayerAnims>>,
    /// Every weapon file, parsed once at load: the animscript tests
    /// `weaponClass` every frame and the frame loop must not read a pk3.
    /// `Rc` so a snapshot/move closure can hold it without borrowing `self`.
    pub(super) weapon_table: Rc<crate::weapons::WeaponTable>,
    /// The blasts this frame's missile pass set off, for the radius damage
    /// pass to charge (`crate::game::missile::Explosion`).
    pub(super) pending_explosions: Vec<crate::game::missile::Explosion>,
    /// `bg_fallDamageMinHeight` and `bg_fallDamageMaxHeight` as the script's
    /// cvar table last read, what every move lands with and systeminfo
    /// carries.
    pub(super) fall_heights: FallHeights,
    /// The hit-location damage multipliers out of the paks
    /// (`crate::game::combat::HitLocTable`); the default until `load_scripts`.
    pub(super) hitlocs: crate::game::combat::HitLocTable,
    /// The grafted player skeletons shots trace against, built on first
    /// use (`crate::game::hitrig`).
    pub(super) hit_rigs: crate::game::hitrig::HitRigs,
    /// Weapons the moves themselves switched to, by slot: only a change the
    /// machine made, so a playerstate reset from outside a move is not one.
    /// `tick` writes each back onto the script host before the mirror reads
    /// it, which is what keeps a switch from being undone the frame after it
    /// lands.
    pub(super) weapon_changes: Vec<(usize, u8)>,
    /// One `.gsc` answered from memory instead of the paks
    /// ([`Server::overlay_script`]); `None` unless `--gametype-script` or a test set it.
    pub(super) script_overlay: Option<(String, String)>,
    /// The bots' brains and what the driver keeps per bot and per level
    /// (`crate::bots::driver`).
    pub(super) driver: crate::bots::driver::BotDriver,
    /// What `server/bot_admin.rs` keeps between passes: the manual count
    /// `--bots` seeds, the pinned bots, the name draw.
    pub(super) bot_admin: bot_admin::BotAdmin,
    /// `GameHost::brush_links` when `nav` was asked for, the count seen
    /// since and the frame it was first seen, and the rebuild under way
    /// once it has held [`NAV_RELINK_MS`].
    pub(super) nav_links: u32,
    pub(super) nav_relink: Option<(u32, i32)>,
    pub(super) nav_rebuild: Option<crate::nav::NavJob>,
    /// Bots' routes in a UO vehicle's driver's seat.
    pub(super) bot_drives: bot_vehicles::BotDrives,
    /// The graph's nodes UO vehicles parked since its build cover.
    pub(super) bot_parked: bot_vehicles::ParkedVehicles,
    /// Each `bas` base's origin and the graph's spots a rocket fired from
    /// bursts at it (`breach_stands`), for the life of the nav graph.
    pub(super) bot_breach_stands: Vec<([f32; 3], Vec<[f32; 3]>)>,
}

impl Cod1Module {
    /// No level loaded yet; `seed` is the server's, for the level seeds and
    /// the bots' draws.
    pub(super) fn new(seed: u64, bots: usize, bots_shoot: bool, fall: FallHeights) -> Self {
        Cod1Module {
            spread_draws: Default::default(),
            level_seed: seed,
            spawn_pins: None,
            test_entities: None,
            script: None,
            anims: None,
            weapon_table: Rc::new(crate::weapons::WeaponTable::empty()),
            pending_explosions: Vec::new(),
            fall_heights: fall,
            hitlocs: crate::game::combat::HitLocTable::default(),
            hit_rigs: Default::default(),
            weapon_changes: Vec::new(),
            script_overlay: None,
            driver: Default::default(),
            bot_admin: bot_admin::BotAdmin::new(bots, bots_shoot, seed),
            nav_links: 0,
            nav_relink: None,
            nav_rebuild: None,
            bot_drives: Default::default(),
            bot_parked: Default::default(),
            bot_breach_stands: Vec::new(),
        }
    }
}

/// What the engine lends the game for one call: the client table (each
/// slot's `sim`, `last_cmd` and `flame` are the game's half of it), the
/// map, the paks, the protocol, the clocks and the process's `rand()`.
pub(super) struct Lend<'a> {
    pub(super) clients: &'a mut Vec<Option<Client>>,
    pub(super) world: &'a Option<Rc<World>>,
    pub(super) fs: &'a Option<Rc<vcod_common::pk3::Pk3Fs>>,
    pub(super) proto: &'static Protocol,
    /// `level.time` of the frame being run.
    pub(super) sv_time_ms: i32,
    /// `svs.time`, the network clock (`Server::svs_time_ms`).
    pub(super) svs_time_ms: i32,
    /// The `rand()` stream the engine's challenges draw from too.
    pub(super) rng: &'a mut u64,
    /// [`Server::pin_stuck_rand`]'s glibc `rand()`, when a test pinned it.
    pub(super) stuck_rand: &'a mut Option<vcod_common::rng::GlibcRand>,
    /// The engine's configstring table, as the last frame left it.
    pub(super) configstrings: &'a [String],
    /// The killcam's frames; the game turns it on and off.
    pub(super) archive: &'a mut crate::archive::Archive,
    /// `SV_AddTestClient`'s `bot%d` counter.
    pub(super) test_client_counter: &'a mut u32,
}

/// What a game frame hands the engine to send.
pub(super) struct FrameOut {
    /// `CheckVote`'s lines for every client.
    pub(super) vote_lines: Vec<String>,
    /// `CheckMatchTimeout` wrote its configstrings.
    pub(super) timeout_written: bool,
    /// `trap_SendConsoleCommand`'s `EXEC_APPEND` lines.
    pub(super) console_lines: Vec<String>,
    /// Server commands for single clients, by slot.
    pub(super) client_commands: Vec<(usize, String)>,
    /// A score moved: the scoreboard goes to every client in intermission.
    pub(super) ranks_dirty: bool,
}

/// CoD1's `SV_Frame` (cod_lnxded 0x808cdf8) and `SV_PacketEvent` on the
/// engine's order. Each step's trace lines are CoD1's own where the engine
/// does not print them.
impl crate::engine::GameModule for Cod1Game {
    type Client = Client;
    type Wire = Protocol;
    type Snapshots = Cod1Snapshots;

    fn wire(e: &Server) -> &'static Protocol {
        e.game.proto
    }
    fn status(
        e: &mut Server,
        query: crate::engine::StatusQuery,
        challenge: &str,
    ) -> Option<String> {
        match query {
            crate::engine::StatusQuery::Info => e.svc_info(challenge),
            crate::engine::StatusQuery::Status => e.svc_status(challenge),
        }
    }

    fn packet_read(e: &mut Server, from: SocketAddr, pkt: &[u8], at: crate::engine::Arrival) {
        let ack = e.ack_stamp(at.arrived);
        e.handle_packet_acked(from, pkt, at, ack);
    }
    fn console_names(e: &Server) -> Vec<String> {
        e.console_names()
    }
    fn accepts_packets(e: &Server) -> bool {
        // `Com_EventLoop` hands a packet to `SV_PacketEvent` only while
        // `sv_running` is set.
        !e.game.killed
    }
    fn is_connectionless(pkt: &[u8]) -> bool {
        parse_oob(pkt).is_some()
    }
    fn connectionless(e: &mut Server, from: SocketAddr, pkt: &[u8], at: crate::engine::Arrival) {
        let Some((cmd, _)) = parse_oob(pkt) else {
            return;
        };
        let cmd = cmd.to_string();
        e.handle_oob(from, &cmd, pkt, at.now, at.read);
    }
    fn client_packet(e: &mut Server, from: SocketAddr, pkt: &[u8], at: crate::engine::Arrival) {
        e.handle_client_packet(from, pkt, at.now);
    }
    fn drain_console(e: &mut Server, now: Instant) {
        e.drain_console(now);
    }

    fn svs_time_ms(e: &Server) -> i32 {
        e.svs_time_ms()
    }
    fn frame_head(e: &mut Server, now: Instant, frames: u32) -> bool {
        e.frame_head(now, frames)
    }
    fn bots(e: &mut Server) {
        e.bot_frame();
    }
    fn calc_pings(e: &mut Server) {
        e.calc_pings();
    }
    fn frame_step_ms(e: &mut Server) -> i32 {
        e.frame_clock()
    }
    fn replay_moves(e: &mut Server) {
        e.game.frame_moved = e.replay_moves();
    }
    fn run_frame(e: &mut Server) -> bool {
        let moved = std::mem::take(&mut e.game.frame_moved);
        if !e.game_frame(&moved) {
            return false;
        }
        // The snapshots report the first frame's moves.
        e.game.tick_moved.get_or_insert(moved);
        true
    }
    fn archive(e: &mut Server) {
        e.archive_between();
    }
    fn check_timeouts(e: &mut Server) {
        e.check_timeouts();
    }
    fn snapshots_begin(e: &mut Server) -> Cod1Snapshots {
        // Every entity built once, then culled and written per client.
        let moved = e.game.tick_moved.take().unwrap_or_default();
        let wall = e.game.tick_wall;
        e.snapshots_begin(moved, wall)
    }
    fn message_due(e: &Server, slot: usize) -> bool {
        e.message_due(slot)
    }
    fn next_fragment(e: &mut Server, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        e.next_fragment(slot)
    }
    fn send_client_snapshot(e: &mut Server, slot: usize, snaps: &mut Cod1Snapshots) {
        e.send_client_snapshot(slot, snaps);
    }
    fn snapshots_end(e: &mut Server, snaps: Cod1Snapshots) {
        e.snapshots_end(snaps);
    }
    fn heartbeat(e: &mut Server) {
        e.drain_game_log();
        e.master_heartbeat();
    }

    fn take_fatal(e: &mut Server) -> Option<anyhow::Error> {
        e.take_fatal()
    }
    fn frame_sent(e: &mut Server, at: Instant) {
        e.frame_sent(at);
    }
}
