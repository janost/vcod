//! The engine side of a CoD4 1.7 server (protocol 6 of `Game::Cod4`). The
//! handshake (challenge, connect, the seven
//! stats packets, the gamestate, the pure check) and the snapshots follow
//! docs/research/cod4-handshake.md; the frame order is 1.7's `SV_Frame`
//! (cod4-server-frame.md) through [`Cod4Game`]'s constants.
//!
//! The load walks the five common zones and the map's, and runs the level
//! (`game::cod4::level::Cod4Level`): the gametype's bootstrap, whose
//! configstrings and mirrored dvars go out in the gamestate, then each
//! client's `ClientConnect`, `ClientBegin` on its first move, its `mr` menu
//! answers, the spawns the script makes, and each cmd's pmove as it
//! arrives. A snapshot carries the level's playerstate, the script's
//! entities but the hidden ones and the playing clients' bodies, unculled
//! as retail's, and the roster (docs/research/cod4-pmove.md 15). Without an
//! install the level is bare: spectators float at the origin.

mod bots;
mod cycle;
mod download;
pub use crate::engine::gamelog;
mod oob;
mod voice;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use vcod_common::net::Msec;

use vcod_cod4::install::Install;
use vcod_cod4::net::clmsg::{self, ClOp, UserCmd};
use vcod_cod4::net::delta::{ClientState, ENTITYNUM_NONE, EntityState, PlayerState};
use vcod_cod4::net::fields::{CLIENT_STATE, PLAYER_STATE};
use vcod_cod4::net::huffman::Huffman;
use vcod_cod4::net::parse::{Gamestate, SVC_SERVERCOMMAND, UNUSED_HUFF};
use vcod_cod4::net::stats::{STAT_PACKETS, Stats};
use vcod_cod4::net::wire::{Cod4Wire, EOF, Frame4, PROTOCOL_17};
use vcod_common::net::codec::MsgWriter;
use vcod_common::net::netchan::{NetchanParams, ServerNetchan};
use vcod_common::text::latin1_bytes;

use crate::engine::{conn, link};

/// The frame at `sv_fps` 20, the dvar's default; a server steps
/// [`Cod4Server::frame_ms`].
pub const FRAME_MS: i32 = 50;
/// `MAX_RELIABLE_COMMANDS`: the netchan's 128-command rings (CoD2's,
/// `& 0x7f` in `SV_AddServerCommand`); the walk capture's walker
/// is sent 74 commands in 26 s.
const MAX_RELIABLE_COMMANDS: usize = NetchanParams::COD2.reliable_commands;
const PACKET_BACKUP: usize = crate::engine::slot::PACKET_BACKUP;
/// `TEAM_SPECTATOR` in a client state's `team`, as the spectator capture's
/// roster reads (VERIFIED live, `play-1.7-spec.txt`).
const TEAM_SPECTATOR: u32 = 3;
/// `PM_SPECTATOR`: the spectator capture's `pm_type` (VERIFIED live).
const PM_SPECTATOR: u32 = 4;
/// `SV_SpawnServer`'s three 100 ms frames before `SV_CreateBaseline`.
const SPAWN_FRAMES: i32 = 3;
const SPAWN_FRAME_MS: i32 = 100;
/// `SNAPFLAG_SERVERCOUNT`, flipped by every spawn.
const SNAPFLAG_SERVERCOUNT: u8 = 4;
/// `SV_CheckTimeouts`' stale calls before the drop (0x8175830).
const TIMEOUT_CALLS: u32 = 5;
/// The zones `com_startup` loads ahead of the map's (`HUD_ZONES` plus
/// `ui_mp`), in load order.
pub const COMMON_ZONES: [&str; 5] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "ui_mp",
    "common_mp",
    "localized_common_mp",
];

/// What `vcod-server <map> --game cod4` starts with.
#[derive(Clone, Debug)]
pub struct Cod4Config {
    pub map: String,
    pub gametype: String,
    pub hostname: String,
    pub max_clients: usize,
}

use crate::engine::slot::State;

/// A CoD4 client slot (`engine::slot::Conn`).
pub type Client = crate::engine::slot::Conn<(i32, Frame4), Cod4Client>;

/// What a CoD4 slot keeps beside the engine's connection.
pub struct Cod4Client {
    /// The playerstate's `commandTime`: the newest cmd thought.
    command_time: i32,
    /// The predicted origin and its time the newest move carried
    /// (`SV_UserMove`'s +0x63c and +0x648, 0x817270a..0x8172752).
    predicted: ([f32; 3], i32),
    /// The 0x2000-byte stats block and which of its seven packets came.
    stats: Stats,
    stat_packets: u8,
    /// The `getchallenge` hex the client sent, its GUID (VERIFIED live, the
    /// console's `guid "<hex>"`).
    guid: String,
    /// The file in flight (cod4-handshake.md 11).
    download: download::Download,
    /// The exponent of the next unacked back-off draw (slot +0x804f8,
    /// 0..=8); see [`Cod4Server::transmit`].
    stall_exp: u32,
    /// `SV_ClientCommand`'s flood count (slot +0x21458): game commands
    /// still let through inside the 800 ms window ([`flood_check`]).
    flood_count: i32,
}

/// The pure lists: `sv_iwds`/`sv_iwdNames` in search order and the keyed
/// sums a `cp` is checked against.
#[derive(Default)]
struct Iwds {
    sums: Vec<i32>,
    names: Vec<String>,
    keyed: Vec<i32>,
    /// `sv_referencedIwdNames` and `sv_referencedIwds`: the iwds the level
    /// read a file from, `<game>/<name>`, set only under an `fs_game`.
    referenced: Vec<(String, i32)>,
}

/// The CoD4 game module beside the [`crate::engine::EngineCore`].
pub struct Cod4Game {
    cfg: Cod4Config,
    cvars: crate::cvars::Cvars,
    huff: Huffman,
    server_id: i32,
    snap_flags: u8,
    /// Where every client floats: `mp_global_intermission`'s origin and
    /// angles.
    spot: ([f32; 3], [f32; 3]),
    iwds: Iwds,
    /// `sv_referencedFFNames` and their sizes, `sv_referencedFFCheckSums`.
    fastfiles: Vec<(String, String)>,
    /// The walked zones, for the level the gsc boot runs over.
    pub zones: Vec<Arc<vcod_cod4::assets::Assets>>,
    /// Entity baselines; empty until entities spawn.
    baselines: BTreeMap<u32, EntityState>,
    /// The running level; `None` on a bare server, whose cvars are
    /// [`Self::cvars`]'s own. A level owns the one cvar table while it runs.
    pub level: Option<crate::game::cod4::level::Cod4Level>,
    /// The install every level loads from; `None` on a bare server.
    install: Option<Install>,
    /// The five common zones, walked once: a map change walks only the
    /// map's zone, as retail keeps them loaded.
    common: Vec<Arc<vcod_cod4::assets::Assets>>,
    /// The level's zones, which a `fast_restart` runs the scripts over
    /// again.
    zones4: Option<std::rc::Rc<crate::game::cod4::Cod4Zones>>,
    /// `sv.start_frameTime`: the frame the level last spawned or restarted
    /// on; a second restart in it does nothing (cod4-map-cycle.md 2).
    start_frame: Option<i32>,
    /// `svs.tempBans`: the GUIDs `tempBanClient` kicked, vcod's address
    /// guid beside each (`oob::address_guid`), and when.
    temp_bans: Vec<(String, Option<String>, i32)>,
    /// The socket and the wait `SV_SpawnServer` uses mid-command; without
    /// it (a test) its OOB lines wait in the core's `outbox`.
    net_now: Option<crate::cod2::NetNow>,
    /// The seed each level's `srand` takes when a test pinned one
    /// ([`Cod4Server::pin_game_seed`]); otherwise a draw off the engine's own.
    game_seed: Option<u32>,
    /// A directory whose loose `.gsc` each level reads over the zones'
    /// ([`Cod4Server::set_script_overrides`]).
    script_overrides: Option<std::path::PathBuf>,
    /// vcod's bots (`bots.rs`), once one is added.
    bots: Option<crate::bots::cod4::Cod4Bots>,
    /// `level.logFile`: `games_mp.log` while a level runs with `g_log` set
    /// (`gamelog`).
    log: gamelog::LevelLog,
    /// `killserver` ran: no level, no packet answered (VERIFIED live:
    /// retail answers neither rcon nor `getinfo` after it).
    killed: bool,
    /// A load that failed with the level torn down, for the loop to end the
    /// process on ([`crate::engine::GameModule::take_fatal`]).
    fatal: Option<anyhow::Error>,
    /// The domains `dvar_bool`, `dvar_int` and `dvar_float` gave the dvars
    /// they made, keyed by the folded name (`oob::Domain`).
    domains: std::collections::HashMap<String, oob::Domain>,
    /// `holdrand`'s `irand`, the unacked back-off's draw. Retail's stream
    /// is the one the game draws from too (0x8272078); ours is the
    /// engine's own, so the draws differ and their bounds hold.
    hold: vcod_common::rng::HoldRand,
    /// [`Cod4Server::set_test_loss`].
    #[cfg(any(test, feature = "testing"))]
    test_loss: Option<TestLoss>,
}

/// A test's packet loss: whether to drop a slot's message, by its ops.
#[cfg(any(test, feature = "testing"))]
type TestLoss = Box<dyn FnMut(usize, &[ClOp]) -> bool>;

impl Cod4Game {
    /// The cvar table: the level's while one runs (its scripts and the
    /// engine share it, as retail's one dvar table), else the engine's.
    pub fn cvars(&self) -> &crate::cvars::Cvars {
        match &self.level {
            Some(l) => &l.rt.host.cvars,
            None => &self.cvars,
        }
    }

    pub fn cvars_mut(&mut self) -> &mut crate::cvars::Cvars {
        match &mut self.level {
            Some(l) => &mut l.rt.host.cvars,
            None => &mut self.cvars,
        }
    }
}

pub type Cod4Server = crate::engine::Engine<Cod4Game>;

/// Every snapshot of a turn is built from one roster.
type Cod4Snapshots = Option<(Vec<ClientState>, BTreeMap<u32, EntityState>)>;

/// 1.7's `SV_FrameInternal` (0x8177010, cod4-server-frame.md): pings, then
/// `SV_PreFrame` (bots, configstrings 0 and 1, the mirror), then per owed
/// frame `G_RunFrame`, `SV_CheckTimeouts`, `SV_SendClientMessages` (the
/// fragments of every slot first, then the new messages, the archive at its
/// tail) and the heartbeat. Each cmd is thought as its packet arrives.
impl crate::engine::GameModule for Cod4Game {
    type Client = Client;
    type Wire = Cod4Wire;
    type Snapshots = Cod4Snapshots;

    const THINK: crate::engine::ThinkTiming = crate::engine::ThinkTiming::OnArrival;
    const HEAD: crate::engine::HeadOrder = crate::engine::HeadOrder::PingsBotsCs;
    const POST_EACH_FRAME: bool = true;
    const MIN_FRAMES: u32 = 0;
    const ARCHIVE_AFTER_MESSAGES: bool = true;
    const FRAGMENTS_FIRST: bool = true;

    fn wire(_e: &Cod4Server) -> &'static Cod4Wire {
        &PROTOCOL_17
    }
    fn status(
        e: &mut Cod4Server,
        q: crate::engine::StatusQuery,
        challenge: &str,
    ) -> Option<String> {
        if e.game.killed {
            return None;
        }
        Some(match q {
            crate::engine::StatusQuery::Info => e.svc_info(challenge),
            crate::engine::StatusQuery::Status => e.svc_status(challenge),
        })
    }
    fn connectionless(
        e: &mut Cod4Server,
        from: SocketAddr,
        pkt: &[u8],
        at: crate::engine::Arrival,
    ) {
        if !e.game.killed {
            e.connectionless(from, &pkt[4..], at.now);
        }
    }
    fn client_packet(
        e: &mut Cod4Server,
        from: SocketAddr,
        pkt: &[u8],
        _at: crate::engine::Arrival,
    ) {
        if !e.game.killed {
            e.client_packet(from, pkt);
        }
    }
    fn drop_sender(e: &mut Cod4Server, from: SocketAddr, pkt: &[u8], reason: &str) {
        if let Some(slot) = conn::sender_slot(&e.core, from, pkt) {
            e.drop_client(slot, reason);
        }
    }

    fn drain_console(e: &mut Cod4Server, _now: Instant) {
        let _p = crate::tick_profile::phase("console");
        e.run_console();
    }
    fn configstrings_head(e: &mut Cod4Server) {
        e.broadcast_configstrings();
    }
    fn bots(e: &mut Cod4Server) {
        e.step_bots();
    }
    fn calc_pings(e: &mut Cod4Server) {
        for c in e.core.clients.iter_mut().flatten() {
            c.link.ping = if c.bot {
                0
            } else if c.state == State::Active {
                link::calc_ping(&c.link.ping_ring)
            } else {
                999
            };
        }
    }
    fn frame_step_ms(e: &mut Cod4Server) -> i32 {
        e.frame_ms()
    }
    fn wall_frame_ms(e: &Cod4Server) -> i32 {
        e.frame_ms()
    }
    fn run_frame(e: &mut Cod4Server) -> bool {
        crate::tick_trace::step!("G_RunFrame");
        e.run_level_frame();
        true
    }
    /// Empty: the killcam's entity rounds are archived as each frame's
    /// snapshot world is built (`send_client_snapshot`, `archive_entities`).
    fn archive(_e: &mut Cod4Server) {}
    fn check_timeouts(e: &mut Cod4Server) {
        e.check_timeouts();
    }
    fn snapshots_begin(_e: &mut Cod4Server) -> Cod4Snapshots {
        None
    }
    fn message_due(e: &Cod4Server, slot: usize) -> bool {
        e.message_due(slot)
    }
    fn next_fragment(e: &mut Cod4Server, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        e.next_fragment(slot)
    }
    fn send_client_snapshot(e: &mut Cod4Server, slot: usize, snaps: &mut Cod4Snapshots) {
        let _p = crate::tick_profile::phase("messages/snapshot");
        if snaps.is_none() {
            let _p = crate::tick_profile::phase("messages/snapshot world");
            let roster = e.roster();
            // The roster's tag indices (`client_attachments`) are
            // `G_UpdateClientInfo`'s `G_TagIndex` inside retail's frame, so
            // their `d` goes out ahead of the snapshot that names them
            // (cod4-player-anims.md 5).
            e.flush_game_commands();
            let world = e.game.level.as_mut().map_or_else(BTreeMap::new, |l| {
                // The angles as 1.7's writer rounds them (`angle16_sent`).
                let w: BTreeMap<_, _> = l
                    .entities()
                    .into_iter()
                    .map(|(n, e)| (n, e.sent()))
                    .collect();
                l.archive_entities(&w);
                w
            });
            *snaps = Some((roster, world));
        }
        let (roster, world) = snaps.as_ref().unwrap();
        let (roster, mut world) = (roster.clone(), world.clone());
        if let Some(l) = e.game.level.as_ref() {
            match l.killcam_entities(slot) {
                // A killcam's follower sees the world as it was at the look
                // back, each temp entity by the mask it was archived with
                // (0x817a5b3), never a live one's.
                Some(old) => world = old,
                // The temp entities a single-client scope keeps from the
                // client the playerstate views as (`r.clientMask`,
                // cod4-combat.md 4).
                None => {
                    let viewer = l
                        .playerstate(slot)
                        .and_then(|p| p.get("clientNum"))
                        .unwrap_or(slot as u32);
                    for n in l.hidden_from(viewer as usize) {
                        world.remove(&n);
                    }
                }
            }
        }
        e.send_message(slot, &roster, &world);
        e.send_voice(slot);
    }
    fn heartbeat(e: &mut Cod4Server) {
        e.heartbeat();
    }
    fn take_fatal(e: &mut Cod4Server) -> Option<anyhow::Error> {
        e.game.fatal.take()
    }
}

impl conn::ConnGame for Cod4Game {
    type Frame = (i32, Frame4);
    type Extra = Cod4Client;

    fn cvar<'a>(e: &'a Cod4Server, name: &str) -> &'a str {
        e.game.cvars().get(name)
    }
    fn ignorable(text: &str) -> bool {
        ignorable(text)
    }
    fn downloading(c: &Client) -> bool {
        c.extra.download.active()
    }
    fn drop_client(e: &mut Cod4Server, slot: usize, reason: &str) {
        e.drop_client(slot, reason);
    }
    fn script(e: &mut Cod4Server) -> Option<&mut crate::game::script::ScriptRuntime> {
        e.game.level.as_mut().map(|l| &mut l.rt)
    }
    fn write_configstring(e: &mut Cod4Server, i: usize, value: &str) {
        e.write_configstring(i, value);
    }
    fn flushed(e: &mut Cod4Server) {
        e.drain_game_log();
    }
    fn write_server_command(w: &mut MsgWriter, seq: i32, text: &str) {
        w.write_byte(SVC_SERVERCOMMAND);
        w.write_long(seq);
        w.write_string_bytes(&latin1_bytes(text));
    }
    /// The ops and `svc_EOF` compressed with 1.7's tree.
    fn netchan_out(e: &mut Cod4Server, slot: usize, mut ops: Vec<u8>) -> Vec<Vec<u8>> {
        let c = e.core.clients[slot].as_mut().unwrap();
        ops.push(EOF);
        let block = e.game.huff.compress(&ops);
        c.link
            .netchan
            .transmit_block(c.link.last_client_command, &block)
    }
    /// 0x8178aff..0x8178c98: a client in the world more than 28 messages
    /// past its last acknowledge, LAN or not, is due `irand(0, 2^n) *
    /// snapshotMsec` on, n counting such messages from 0 up to 8 and back
    /// to 0 on any other (cod4-server-frame.md 12).
    fn back_off(e: &mut Cod4Server, slot: usize, sequence: i32) -> bool {
        let svs = e.core.sv_time_ms;
        let c = e.core.clients[slot].as_mut().unwrap();
        if c.state == State::Active && c.delta_message >= 0 && sequence - c.delta_message > 28 {
            let k = e.game.hold.irand(0, 1 << c.extra.stall_exp);
            c.link.next_message_ms = svs + k * c.rates.snapshot_msec;
            c.extra.stall_exp = (c.extra.stall_exp + 1).min(8);
            return true;
        }
        c.extra.stall_exp = 0;
        false
    }
    fn exec(e: &mut Cod4Server, line: &str, out: &mut crate::rcon::Redirect) {
        e.exec(line, out);
    }

    const PROTOCOL: i32 = oob::PROTOCOL as i32;
    const VERSION: &'static str = "1.7";
    const FF_KC: [&'static str; 2] = ["scr_team_fftype", "scr_game_allowkillcam"];
    fn names(e: &Cod4Server) -> [&str; 3] {
        let cfg = &e.game.cfg;
        [&cfg.hostname, &cfg.map, &cfg.gametype]
    }
    fn pure(e: &Cod4Server) -> bool {
        e.pure()
    }
    fn modded(e: &Cod4Server) -> bool {
        e.modded()
    }
    /// `G_GetClientScore` (0x81764a2; cod4-handshake.md 14).
    fn status_score(e: &mut Cod4Server, slot: usize) -> i32 {
        e.client_score(slot)
    }
    fn status_name(c: &Client) -> String {
        client_name(&c.link.userinfo)
    }
    /// `killserver` ends the redirect with the server: nothing answers.
    fn answers(e: &Cod4Server) -> bool {
        !e.game.killed
    }
    fn password_refused(e: &Cod4Server, password: &str) -> bool {
        let gp = e.game.cvars().get("g_password");
        !gp.is_empty() && gp != password
    }
    /// `SV_FreeClient` at the reconnect (0x8171eb3), so
    /// `ClientDisconnect`; a zombie's ran at its drop.
    fn free_level_slot(e: &mut Cod4Server, slot: usize) {
        if let Some(level) = e.game.level.as_mut() {
            level.disconnect(slot);
        }
    }
    fn find_player(
        e: &Cod4Server,
        by_name: bool,
        who: &str,
        out: &mut crate::rcon::Redirect,
    ) -> Option<usize> {
        e.find_client(by_name, who, out)
    }
    fn ban_client(e: &mut Cod4Server, slot: usize, out: &mut crate::rcon::Redirect) {
        e.ban_client(slot, out);
    }
}

impl Cod4Server {
    /// The loop's socket and wait for `SV_SpawnServer`'s send and sleep.
    pub fn set_net_now(&mut self, net: crate::cod2::NetNow) {
        self.game.net_now = Some(net);
    }

    /// A server with no level: [`Self::load`] brings one up.
    pub fn new(mut cfg: Cod4Config, seed: u64) -> Self {
        cfg.max_clients = cfg.max_clients.clamp(1, crate::server::MAX_CLIENTS);
        let mut cvars = crate::cvars::Cvars::new();
        for (name, value) in oob::ENGINE_CVARS {
            cvars.register(name, value);
            cvars.set(name, value);
        }
        cvars.remirror(&PROTOCOL_17.cs, oob::MIRRORED);
        // 1.7's archived dvars, not the CoD1 table's (`writeconfig`).
        cvars.clear_flags(crate::cvars::flag::ARCHIVE);
        for name in oob::ARCHIVED {
            cvars.respell(name);
            cvars.add_flags(name, crate::cvars::flag::ARCHIVE);
        }
        cvars.set("sv_hostname", &cfg.hostname);
        cvars.set("ui_hostname", &cfg.hostname);
        cvars.set("sv_maxclients", &cfg.max_clients.to_string());
        cvars.set("g_gametype", &cfg.gametype);
        cvars.set("mapname", &cfg.map);
        let mut core = crate::engine::EngineCore::new(
            &PROTOCOL_17.transport,
            cfg.max_clients,
            seed | 1,
            crate::master::Masters::new(vcod_common::net::master::COD4_MASTER),
        );
        core.configstrings = vec![String::new(); PROTOCOL_17.transport.max_configstrings];
        let mut s = crate::engine::Engine {
            core,
            game: Cod4Game {
                cfg,
                cvars,
                huff: Huffman::new(),
                server_id: 0,
                snap_flags: 0,
                spot: ([0.0; 3], [0.0; 3]),
                iwds: Iwds::default(),
                fastfiles: Vec::new(),
                zones: Vec::new(),
                baselines: BTreeMap::new(),
                level: None,
                install: None,
                common: Vec::new(),
                zones4: None,
                start_frame: None,
                temp_bans: Vec::new(),
                net_now: None,
                game_seed: None,
                script_overrides: None,
                bots: None,
                log: Default::default(),
                killed: false,
                fatal: None,
                hold: vcod_common::rng::HoldRand(seed as u32),
                #[cfg(any(test, feature = "testing"))]
                test_loss: None,
                // `sv_fps`, registered by `SV_Init` (0x81735c8).
                domains: [(
                    "sv_fps".to_string(),
                    oob::Domain::Int(
                        *crate::game::cod4::SV_FPS_DOMAIN.start(),
                        *crate::game::cod4::SV_FPS_DOMAIN.end(),
                    ),
                )]
                .into(),
            },
        };
        s.core.checksum_feed = s.rand();
        s.write_engine_configstrings();
        s.core.sent_configstrings = s.core.configstrings.clone();
        s
    }

    /// Test-facing: every level's `srand` (`G_InitGame`, 0x80b56ee) takes
    /// `seed`, as retail's took its wall clock's milliseconds, so a gate
    /// replays a capture's `rand()` from the seed recovered off it
    /// (cod4-combat.md 10.9).
    pub fn pin_game_seed(&mut self, seed: u32) {
        self.game.game_seed = Some(seed);
    }

    /// Test-facing: a client message of the running level whose ops `lose`
    /// returns true for (its slot, the ops) is dropped before the netchan
    /// takes it, as one lost on the wire.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_test_loss(&mut self, lose: impl FnMut(usize, &[ClOp]) -> bool + 'static) {
        self.game.test_loss = Some(Box::new(lose));
    }

    /// Each level reads the loose `.gsc` under `dir` over the zones'
    /// scripts, as retail reads a mod's under an `fs_game` (a probe gsc).
    pub fn set_script_overrides(&mut self, dir: std::path::PathBuf) {
        self.game.script_overrides = Some(dir);
    }

    /// The seed of the next level's `srand`.
    pub(crate) fn game_seed(&mut self) -> u32 {
        match self.game.game_seed {
            Some(s) => s,
            None => self.rand() as u32,
        }
    }

    /// `SV_SpawnServer`: the zones (the five common ones, then the map's),
    /// `default_mp.cfg`, the level (the gametype's bootstrap over the
    /// zones, the engine's cvars handed to it), three 100 ms frames, the
    /// baselines, the intermission spot off the map's entities and the
    /// pure lists. `None` runs a bare server: no script, the spectators
    /// float at the origin and nothing is pure.
    pub fn load(&mut self, install: Option<&Install>) -> anyhow::Result<()> {
        // `fs_game` (a `+set` ahead of the load) picks the mod's zone and
        // iwds (cod4-fastfile.md 13).
        // The Linux server keeps `fs_game` lowercased (VERIFIED live,
        // cod4-fastfile.md 13).
        let fs_game = self.game.cvars().get("fs_game").to_ascii_lowercase();
        self.game.cvars_mut().set("fs_game", &fs_game);
        self.game.install = install.map(|i| i.clone().with_fs_game(Some(&fs_game)));
        self.load_level(Default::default())
    }

    /// [`Self::load`]'s spawn over the kept install, the last level's
    /// `carry` handed to the new one.
    fn load_level(&mut self, carry: crate::game::script::Carry) -> anyhow::Result<()> {
        use anyhow::Context;
        self.game.killed = false;
        self.shutdown_game_log();
        // The last level hands the one cvar table back.
        if let Some(mut old) = self.game.level.take() {
            self.game.cvars = std::mem::take(&mut old.rt.host.cvars);
        }
        self.game.server_id = conn::spawn_server_id(self.game.server_id);
        self.game.snap_flags ^= SNAPFLAG_SERVERCOUNT;
        self.game.start_frame = Some(self.core.sv_time_ms);
        self.game.zones4 = None;
        let server_id = self.game.server_id.to_string();
        self.game.cvars.set("sv_serverid", &server_id);
        // The level's `usermaps/<map>/` iwds join the search path.
        let install = self
            .game
            .install
            .clone()
            .map(|i| i.with_map(Some(&self.game.cfg.map)));
        let Some(install) = install.as_ref() else {
            // `SV_SpawnServer`'s frames ahead of the baselines.
            self.core.sv_time_ms += SPAWN_FRAMES * SPAWN_FRAME_MS;
            self.write_engine_configstrings();
            self.core.sent_configstrings = self.core.configstrings.clone();
            return Ok(());
        };
        let t0 = Instant::now();
        let lang = "english";
        if self.game.common.is_empty() {
            let mod_zone = install.mod_zone().map(|_| "mod");
            for name in COMMON_ZONES.into_iter().chain(mod_zone) {
                let path = install.zone_path(lang, name)?;
                let zone = vcod_cod4::zone::Zone::open_path(&path)
                    .and_then(|z| vcod_cod4::walk::walk(z, Default::default()))
                    .with_context(|| path.display().to_string())?;
                self.game.common.push(Arc::new(zone.assets));
            }
        }
        let mut zones = self.game.common.clone();
        let (map, walked) = vcod_cod4::map::Map::load(install, lang, &self.game.cfg.map)
            .with_context(|| format!("loading {}", self.game.cfg.map))?;
        zones.push(Arc::new(walked.assets));
        self.game.zones = zones;
        // The engine's `default_mp.cfg` under the command line's sets, as
        // CoD2's server runs its own (cod2-protocol.md 15.4). Retail execs
        // it and its `exec`s "from disk", out of the iwds, before any zone
        // loads (cod4-gsc.md 7).
        let dirs = install.iwd_dirs();
        let iwds = vcod_common::pk3::Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..])?;
        let cfg = iwd_cfg(&iwds, "default_mp.cfg", 0);
        let (gametype, mapname) = (self.game.cfg.gametype.clone(), self.game.cfg.map.clone());
        self.game.cvars_mut().exec_cfg_under_sets(&cfg);
        self.game.cvars_mut().set("g_gametype", &gametype);
        self.game.cvars_mut().set("mapname", &mapname);
        if let Some(spot) = intermission_spot(&map.entities) {
            self.game.spot = spot;
        }
        // The scripts read `code_post_gfx_mp`, `common_mp` and the map's
        // zone (cod4-gsc.md 4), and a mod's `mod.ff` over all three.
        let script_zones: Vec<Arc<vcod_cod4::assets::Assets>> = COMMON_ZONES
            .iter()
            .zip(&self.game.zones)
            .filter(|(n, _)| crate::game::cod4::BASE_ZONES.contains(n))
            .map(|(_, z)| z.clone())
            .chain(self.game.zones.last().cloned())
            .collect();
        let mod_zone = self.game.common.get(COMMON_ZONES.len()).cloned();
        let zones4 =
            crate::game::cod4::Cod4Zones::from_zones(install, script_zones, mod_zone, map)?;
        let now = self.core.sv_time_ms;
        let info = self.serverinfo();
        self.init_game_log(&info, now);
        // A level owns the one cvar table while it runs; a failed load
        // hands it back.
        let cvars = self.game.cvars.clone();
        let seed = self.game_seed();
        let mut level = crate::game::cod4::level::Cod4Level::load_with(
            &zones4,
            install,
            &mapname,
            &gametype,
            cvars,
            false,
            self.game.script_overrides.clone(),
            now,
            carry,
            None,
            seed,
        )?;
        self.game.cvars = Default::default();
        self.game.zones4 = Some(std::rc::Rc::new(zones4));
        // `SV_SpawnServer`'s frames ahead of the baselines.
        for _ in 0..SPAWN_FRAMES {
            self.core.sv_time_ms += SPAWN_FRAME_MS;
            level.run_frame(self.core.sv_time_ms);
        }
        level.rt.host.configstrings.take_writes();
        level.rt.take_client_commands_stamped();
        // `SV_CreateBaseline`.
        self.game.baselines = level
            .baselines()
            .into_iter()
            .map(|(n, e)| (n, e.sent()))
            .collect();
        for (i, v) in level.rt.configstrings().iter().enumerate().skip(2) {
            if let Some(slot) = self.core.configstrings.get_mut(i) {
                slot.clone_from(v);
            }
        }
        self.game.level = Some(level);
        self.game.fastfiles = referenced_fastfiles(install, lang, &self.game.cfg.map);
        self.game.iwds = pure_lists(&install.iwd_dirs(), self.core.checksum_feed)?;
        if let (Some(g), Some(z)) = (install.fs_game(), &self.game.zones4) {
            let usermap = install.usermap_dir(&self.game.cfg.map);
            self.game.iwds.referenced =
                referenced_iwds(z.source.iwds(), g, usermap.as_deref(), &self.game.cfg.map);
        }
        self.write_engine_configstrings();
        self.core.sent_configstrings = self.core.configstrings.clone();
        log::info!(
            "cod4: {} {} up in {:.0} ms, {} iwds, {} baselines, intermission at {:?}",
            self.game.cfg.map,
            self.game.cfg.gametype,
            t0.elapsed().as_secs_f64() * 1000.0,
            self.game.iwds.names.len(),
            self.game.baselines.len(),
            self.game.spot.0
        );
        Ok(())
    }

    /// Keeps `ban.txt` at `path` (`--ban-file`); without it the bans last
    /// for the run.
    pub fn set_ban_file(&mut self, path: std::path::PathBuf) {
        self.core.bans = crate::engine::bans::BanFile::open(path);
    }

    /// `+set name value` before the level loads, or an rcon `set`.
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        self.game.cvars_mut().set(name, value);
        // CoD1's name sets a dvar 1.7 never reads (cod4-handshake.md 14.2).
        if name.eq_ignore_ascii_case("rconPassword") {
            log::warn!("CoD4 reads rcon_password, not rconPassword: rcon stays off");
        }
        if name.eq_ignore_ascii_case("sv_hostname") {
            self.game.cfg.hostname = value.to_string();
            self.game.cvars_mut().set("ui_hostname", value);
        }
        // `+set g_gametype` ahead of the first load is that level's, as
        // `SV_SpawnServer` takes the latched dvar; once a level runs the
        // latch waits for the next `map` (`cycle.rs`).
        if name.eq_ignore_ascii_case("g_gametype") && self.game.level.is_none() {
            self.game.cfg.gametype = value.to_string();
        }
        self.write_engine_configstrings();
    }

    pub fn cvar(&self, name: &str) -> &str {
        self.game.cvars().get(name)
    }

    /// `1000 / sv_fps`: how far each frame steps `svs.time` and the wall
    /// time it takes. Read every `SV_Frame`, as 1.7 does: an rcon
    /// `sv_fps 40` steps the next frame 25 (cod4-server-frame.md 8).
    pub fn frame_ms(&self) -> i32 {
        crate::game::cod4::frame_ms(self.game.cvars())
    }

    pub fn configstring(&self, i: usize) -> &str {
        &self.core.configstrings[i]
    }

    pub fn server_id(&self) -> i32 {
        self.game.server_id
    }

    fn pure(&self) -> bool {
        self.int("sv_pure") != 0
    }

    /// Configstrings 0, 1 and 2 and the dvar mirror (20..147, 148..275).
    fn write_engine_configstrings(&mut self) {
        self.core.configstrings[0] = self.serverinfo();
        self.core.configstrings[1] = self.systeminfo();
        // `CS_GAME_VERSION`: `cod` on every 1.7 capture.
        self.core.configstrings[2] = "cod".into();
        if let Err(e) = self.game.cvars().write_mirror(&mut self.core.configstrings) {
            log::warn!("cod4: {e:?}");
        }
    }

    /// `Dvar_InfoString(0x404)`: configstring 0 and `getstatus`'s info,
    /// case-folded order.
    fn serverinfo(&self) -> String {
        serverinfo_of(self.game.cvars())
    }

    /// `G_InitGame`'s log (0x80b5782..0x80b5ac7): `g_log` empty prints
    /// `Not logging to disk.`; else the file opens (`g_logSync`'s mode)
    /// and takes the separator and `InitGame: <serverinfo>` at `now`, the
    /// new `level.time` (VERIFIED live, games_mp.log). With no home path
    /// (a test's server) nothing is written.
    fn init_game_log(&mut self, info: &str, now: i32) {
        let cv = match &self.game.level {
            Some(l) => &l.rt.host.cvars,
            None => &self.game.cvars,
        };
        self.game.log.init(cv, cv.get("fs_game"), info, now);
    }

    /// The level's `G_LogPrintf` lines since the last call into the file;
    /// without one they are dropped, as retail's `G_LogPrintf` returns.
    fn drain_game_log(&mut self) {
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let lines = std::mem::take(&mut level.rt.host.game_log);
        let _p = crate::tick_profile::phase("game log");
        self.game.log.write(lines);
    }

    /// `G_ShutdownGame` (0x80b5ce8): `ShutdownGame:` and the separator at
    /// the level's last `level.time`, then the file closed (VERIFIED live).
    fn shutdown_game_log(&mut self) {
        let (lines, at) = match self.game.level.as_mut() {
            Some(l) => (
                std::mem::take(&mut l.rt.host.game_log),
                l.rt.host.level_time_ms,
            ),
            None => (Vec::new(), self.core.sv_time_ms),
        };
        self.game.log.shutdown(lines, at, gamelog::SHUTDOWN);
    }

    /// Configstring 1: the systeminfo dvars, the pure lists (none off
    /// pure), the fastfiles, `sv_serverid`, and `\fs_game\\` last with
    /// `fs_game` empty (VERIFIED live on all four captures).
    fn systeminfo(&self) -> String {
        let mut pairs: Vec<(String, String)> = oob::SYSTEMINFO
            .iter()
            .map(|n| (n.to_string(), self.game.cvars().get(n).to_string()))
            .collect();
        if self.pure() {
            pairs.push((
                "sv_iwds".into(),
                self.game
                    .iwds
                    .sums
                    .iter()
                    .map(|s| format!("{s} "))
                    .collect(),
            ));
            pairs.push(("sv_iwdNames".into(), self.game.iwds.names.join(" ")));
        }
        pairs.push((
            "sv_referencedFFCheckSums".into(),
            self.game
                .fastfiles
                .iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        pairs.push((
            "sv_referencedFFNames".into(),
            self.game
                .fastfiles
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        let refs = &self.game.iwds.referenced;
        pairs.push((
            "sv_referencedIwdNames".into(),
            refs.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        pairs.push((
            "sv_referencedIwds".into(),
            refs.iter().map(|(_, s)| format!("{s} ")).collect(),
        ));
        pairs.push(("sv_serverid".into(), self.game.server_id.to_string()));
        pairs.push(("sv_pure".into(), u8::from(self.pure()).to_string()));
        // A set `fs_game` sorts with the rest; an empty one trails
        // (VERIFIED live, cod4-fastfile.md 13).
        let fs_game = self.game.cvars().get("fs_game").to_string();
        if !fs_game.is_empty() {
            pairs.push(("fs_game".into(), fs_game.clone()));
        }
        let mut s = info_sorted(pairs.into_iter());
        if fs_game.is_empty() {
            s.push_str("\\fs_game\\\\");
        }
        s
    }

    /// `SV_MasterHeartbeat("COD-4")` (0x81780e4): at `dedicated 2` only,
    /// every 180 s (cod4-server-frame.md 5), to `sv_master1..5`, with
    /// `master.cod.pm` while `sv_master1` is empty, on the CoD4 port,
    /// through the masters every game beats ([`crate::master::Masters`]).
    fn heartbeat(&mut self) {
        let cvars = self.game.cvars();
        self.core
            .masters
            .sync_cvars(vcod_common::net::master::COD4_MASTER.host, |n| cvars.get(n));
        let dedicated = self.int("dedicated");
        let mut resolve = self.core.resolver;
        let beats = self.core.masters.heartbeat(
            dedicated,
            self.core.sv_time_ms,
            crate::master::HEARTBEAT_GAME_COD4,
            &mut resolve,
        );
        self.core.outbox.extend(beats);
    }

    /// A netchan packet: its slot by address and qport, the netchan, then
    /// `SV_ExecuteClientMessage`.
    fn client_packet(&mut self, from: SocketAddr, pkt: &[u8]) {
        let conn::Incoming::Message(slot, m) = conn::incoming(&mut self.core, from, pkt) else {
            return;
        };
        let svs = self.core.sv_time_ms;
        let stale = i32::from(m.server_id) != self.game.server_id & 0xff;
        let c = self.core.clients[slot].as_ref().unwrap();
        // A message of the running level reads to its `clc_EOF` before the
        // netchan takes it (`engine::conn::incoming`): a forged or garbled
        // one runs nothing and moves neither the sequence nor the address.
        let ops = if c.state > State::Connected && !stale {
            let key = clmsg::usercmd_key(
                self.core.checksum_feed,
                m.message_ack,
                &latin1_bytes(
                    &c.link.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)],
                ),
            );
            let plain = self.game.huff.decompress(&m.ops);
            let max = c.link.netchan.max_commands();
            // `MSG_SetDefaultUserCmd` of the slot's playerstate
            // (`SV_UserMove` 0x8172664, cod4-netfields.md 14.5).
            let base = self
                .game
                .level
                .as_ref()
                .and_then(|l| l.players.get(slot)?.as_ref())
                .map_or_else(UserCmd::default, |p| {
                    clmsg::default_usercmd(&p.ps, clmsg::CmdBaseSide::Server)
                });
            match clmsg::read_ops(plain, key, &base) {
                Ok(ops) if commands(&ops) > max => {
                    log::debug!("cod4: client {slot}: more than {max} commands in a message");
                    return;
                }
                Ok(ops) if matches!(ops.last(), Some(ClOp::Eof)) => ops,
                Ok(_) => {
                    log::debug!("cod4: client {slot}: message short of clc_EOF");
                    return;
                }
                Err(e) => {
                    log::debug!("cod4: client {slot}: {e:#}");
                    return;
                }
            }
        } else {
            Vec::new()
        };
        #[cfg(any(test, feature = "testing"))]
        if !ops.is_empty()
            && let Some(lose) = &mut self.game.test_loss
            && lose(slot, &ops)
        {
            return;
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        conn::commit(c, from, &m);
        if c.state == State::Zombie {
            return;
        }
        c.link.last_packet_ms = svs;
        if c.state == State::Connected {
            // `SV_SendClientGameState`: only once all seven stats
            // packets are in; with none yet, the client is asked again.
            if c.extra.stat_packets == (1 << STAT_PACKETS) - 1 {
                self.send_gamestate(slot);
            } else if c.extra.stat_packets == 0 {
                self.send_oob(from, "requeststats\n");
            }
            return;
        }
        if stale && i32::from(m.server_id) & 0xf0 == self.game.server_id & 0xf0 {
            // A message from before a `fast_restart` (0x816c466 kept the
            // high nibble): nothing read, a loading client into the world
            // (`SV_ExecuteClientMessage`, cod4-map-cycle.md 2).
            if c.state == State::Primed {
                self.enter_world(slot);
            }
            return;
        }
        if stale {
            // Another level's message: only its commands, which run as from
            // the old server (its usercmds are keyed to the last feed), then
            // the gamestate again once the client has acknowledged past the
            // last one (VERIFIED live).
            let plain = self.game.huff.decompress(&m.ops);
            let ops = clmsg::read_commands(plain);
            if commands(&ops) > c.link.netchan.max_commands() {
                return;
            }
            for op in ops {
                if let ClOp::Command { sequence, text } = op
                    && !self.client_command(slot, sequence, &text, true)
                {
                    return;
                }
            }
            let c = self.core.clients[slot].as_ref().unwrap();
            if m.message_ack > c.gamestate_message {
                self.send_gamestate(slot);
            }
            return;
        }
        for op in ops {
            match op {
                ClOp::Command { sequence, text } => {
                    if !self.client_command(slot, sequence, &text, false) {
                        return;
                    }
                }
                ClOp::Move {
                    delta,
                    cmds,
                    origin,
                    origin_time,
                } => {
                    let cmds: Vec<UserCmd> = cmds.into_iter().map(|(c, _)| c).collect();
                    let predicted = (origin.map(f32::from_bits), origin_time);
                    self.user_move(slot, m.message_ack, delta, &cmds, predicted);
                    return;
                }
                ClOp::Eof => return,
            }
        }
    }

    /// One `clc_clientCommand`. False when the client was dropped. One
    /// from the last level's messages (`from_old`) runs only the engine's
    /// `disconnect` and `userinfo` (Q3's `SV_ExecuteClientCommand` rule,
    /// INFERRED).
    fn client_command(&mut self, slot: usize, seq: i32, text: &[u8], from_old: bool) -> bool {
        let text: String = text.iter().map(|&b| char::from(b)).collect();
        match self.take_client_command(slot, seq, &text) {
            conn::CommandSeq::Old => return true,
            conn::CommandSeq::Lost => return false,
            conn::CommandSeq::Next => {}
        }
        let floodprotect = self.int("sv_floodprotect");
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        log::debug!("cod4: client {slot} command {seq} {text:?}");
        let game_ok = flood_check(c, floodprotect, svs, &text) && !from_old;
        let args = crate::game::say::tokenize(&text);
        match args.first().map(String::as_str) {
            Some(c) if from_old && c != "disconnect" && c != "userinfo" => {}
            Some("cp") => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let ok = vcod_cod4::net::pure::verify_cp(
                    &argv,
                    &self.game.iwds.keyed,
                    self.core.checksum_feed,
                );
                log::info!("cod4: client {slot} cp {}", if ok { "ok" } else { "bad" });
                c.pure = Some(ok);
            }
            Some("disconnect") => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            Some("download" | "nextdl" | "stopdl" | "donedl" | "retransdl" | "wwwdl") => {
                return self.download_command(slot, &args);
            }
            Some(m @ ("muteplayer" | "unmuteplayer")) => {
                let mute = m == "muteplayer";
                c.voice
                    .mute_command(args.get(1).map_or("", String::as_str), mute);
            }
            // `SV_UpdateUserinfo_f` (0x8170536): the engine's copy, then
            // `ClientUserinfoChanged` (0x80a8068) for the game's name.
            Some("userinfo") => {
                let info = args.get(1).map_or("", String::as_str);
                c.link.userinfo = info.chars().take(MAX_USERINFO).collect();
                // Never the LAN rate: `SV_UserinfoChanged` gives it only
                // below `dedicated 2` (0x817038f), which cod4_lnxded's
                // read-only `dedicated` never is (cod4-handshake.md 15).
                c.rates = link::Rates::of(&c.link.userinfo, false, 1);
                c.voice.send = voice::wants_voice(&c.link.userinfo);
                let name = game_name(&c.link.userinfo);
                if let Some(level) = self.game.level.as_mut() {
                    level.userinfo_changed(slot, &name);
                }
            }
            Some(_) if game_ok => self.game_command(slot, &args, &text),
            Some(_) => log::debug!("client text ignored for {slot}: {text}"),
            None => {}
        }
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| c.state != State::Zombie)
    }

    /// The engine's download commands (the table at 0x826d398, cod4-handshake.md
    /// 11). False when the client was dropped.
    fn download_command(&mut self, slot: usize, args: &[String]) -> bool {
        let svs = self.core.sv_time_ms;
        let arg = args.get(1).map_or("", String::as_str);
        let atoi = crate::game::builtins::cvar::atoi;
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        match args[0].as_str() {
            "download" => c.extra.download.begin(arg),
            "nextdl" => {
                if !c.extra.download.next_dl(atoi(arg), svs) {
                    self.drop_client(slot, "broken download");
                    return false;
                }
            }
            "stopdl" => {
                if c.extra.download.active() {
                    log::info!(
                        "clientDownload: {slot} : file {:?} aborted",
                        c.extra.download.name
                    );
                }
                c.extra.download.close();
            }
            // 0x816fd00: the transfer closed, then `SV_SendClientGameState`.
            "donedl" => {
                log::info!("clientDownload: {slot} Done");
                c.extra.download.close();
                // One gamestate a message turn, however many `donedl`s ask.
                if !conn::gamestate_last(c) {
                    self.send_gamestate(slot);
                }
            }
            "retransdl" => c.extra.download.retransmit(atoi(arg)),
            _ => match c.extra.download.wwwdl(arg, &client_name(&c.link.userinfo)) {
                download::Www::None => {}
                download::Www::Gamestate if conn::gamestate_last(c) => {}
                download::Www::Gamestate => self.send_gamestate(slot),
                download::Www::Drop(reason) => {
                    self.drop_client(slot, reason);
                    return false;
                }
            },
        }
        true
    }

    /// `ClientCommand` for what the engine leaves to the game: `mr`
    /// (`mr <serverId> <menu> <response>`, the walk capture's `mr 16 8
    /// allies`, cod4-netfields.md 8), `say`, `say_team`, `score`, `kill`,
    /// the votes and `setviewpos`.
    fn game_command(&mut self, slot: usize, args: &[String], text: &str) {
        let server_id = self.game.server_id;
        let pings: Vec<Option<i32>> = self
            .core
            .clients
            .iter()
            .map(|c| c.as_ref().map(|c| c.link.ping))
            .collect();
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let cmd = args[0].to_ascii_lowercase();
        match cmd.as_str() {
            // `ClientCommand`: at the intermission only `say`,
            // `say_team` and `score`.
            c if !matches!(c, "say" | "say_team" | "score") && level.at_intermission(slot) => {}
            "mr" => {
                if let Some((menu, response)) = crate::server::parse_menu_response(text, server_id)
                {
                    level.menu_response(slot, &menu, &response);
                }
            }
            "say" | "say_team" => {
                let mode = if cmd == "say" {
                    crate::game::say::SayMode::All
                } else {
                    crate::game::say::SayMode::Team
                };
                let rest = text
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, r)| r);
                if let Some(line) = crate::game::say::concat_args(rest) {
                    for (to, line) in level.rt.say(slot, None, mode, &line) {
                        self.send_to(to, &line);
                    }
                }
            }
            "score" => {
                let text = level.scoreboard(&pings);
                self.send_server_command(slot, &text);
            }
            "callvote" | "vote" => {
                for (to, line) in level.vote_command(slot, args) {
                    match to {
                        Some(to) => self.send_server_command(to, &line),
                        None => self.send_to_all(&line),
                    }
                }
            }
            "kill" => {
                if let Some(reply) = level.kill_command(slot) {
                    self.send_server_command(slot, &reply);
                }
            }
            "setviewpos"
                if crate::game::builtins::cvar::atoi(level.rt.host.cvars.get("sv_cheats")) != 0 =>
            {
                let n = |i: usize| {
                    args.get(i)
                        .map_or(0.0, |a| crate::game::builtins::cvar::atof(a))
                };
                if (4..=6).contains(&args.len()) {
                    let mut angles = [0.0; 3];
                    if args.len() >= 5 {
                        angles[1] = n(4);
                    }
                    if args.len() == 6 {
                        angles[0] = n(5);
                    }
                    level.set_view_pos(slot, [n(1), n(2), n(3)], angles);
                }
            }
            _ => log::debug!("cod4: client {slot}: game command {cmd:?} not handled"),
        }
        self.flush_game_commands();
    }

    /// `G_RunFrame` on the frame clock, then the commands and
    /// configstrings it raised out to their clients.
    fn run_level_frame(&mut self) {
        let now = self.core.sv_time_ms;
        let fps = crate::game::cod4::sv_fps(self.game.cvars());
        if let Some(level) = self.game.level.as_mut() {
            level.set_sv_fps(fps);
            level.rt.set_network_time(now);
            level.run_frame(now);
        }
        let _p = crate::tick_profile::phase("frame/flush");
        self.flush_game_commands();
    }

    /// `SV_SetConfigstring` of a game write: the table and `d <index>
    /// <string>` to every client past `CS_CONNECTED`. 0 and 1 are the
    /// engine's.
    fn write_configstring(&mut self, i: usize, value: &str) {
        if i < 2 || i >= self.core.configstrings.len() || self.core.configstrings[i] == value {
            return;
        }
        self.core.configstrings[i] = value.to_string();
        if let Some(sent) = self.core.sent_configstrings.get_mut(i) {
            *sent = value.to_string();
        }
        self.send_to_all(&format!("d {i} {value}"));
    }

    /// `SV_UserMove` (0x8172560): the delta base and ping, a primed client
    /// into the world, the pure check (0x81727b8: no `cp` drops with
    /// `EXE_CANNOTVALIDATEPURECLIENT`, a failed one `EXE_UNPURECLIENTDETECTED`,
    /// cod4-handshake.md 8), then each cmd newer than the last thought.
    fn user_move(
        &mut self,
        slot: usize,
        message_ack: i32,
        delta: bool,
        cmds: &[UserCmd],
        predicted: ([f32; 3], i32),
    ) {
        crate::tick_trace::step!("SV_UserMove slot {slot}");
        let pure = self.pure();
        let now = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        c.delta_message = if delta { message_ack } else { -1 };
        // `cmdCount < 1` (0x81725bd) and `cmdCount > MAX_PACKET_USERCMDS`
        // (0x81725c3) print and return before a cmd is read.
        if cmds.is_empty() || cmds.len() > clmsg::MAX_PACKET_USERCMDS {
            return;
        }
        c.extra.predicted = predicted;
        if message_ack >= 0 {
            c.link.ping_ring[message_ack as usize % PACKET_BACKUP].1 = now;
        }
        if c.state == State::Primed {
            self.enter_world(slot);
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        if pure {
            match c.pure {
                None => return self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT"),
                Some(false) => return self.drop_client(slot, "EXE_UNPURECLIENTDETECTED"),
                Some(true) => {}
            }
        }
        let newest = cmds.last().map_or(0, |c| c.server_time);
        for cmd in cmds {
            let Some(c) = self.core.clients[slot].as_mut() else {
                return;
            };
            if cmd.server_time > newest || cmd.server_time <= c.last_cmd_time {
                continue;
            }
            // `SV_ClientThink` refuses a cmd 20 s or more ahead (0x816deca).
            if cmd.server_time - now >= 20_000 {
                continue;
            }
            c.last_cmd_time = cmd.server_time;
            // No pmove yet: the cmd moves the clock and the view.
            c.extra.command_time = cmd.server_time;
            if let Some(level) = self.game.level.as_mut() {
                use crate::game::cod4::level::Inactivity;
                match level.inactivity_timer(slot, cmd) {
                    Inactivity::Drop => return self.drop_client(slot, "GAME_DROPPEDFORINACTIVITY"),
                    // `SV_GameSendServerCommand(.., "%c \"GAME_INACTIVEDROPWARNING\"", 'c')`
                    // (0x80a4e10), VERIFIED live.
                    Inactivity::Warn => level
                        .rt
                        .host
                        .command_to(slot, "c \"GAME_INACTIVEDROPWARNING\"".to_string()),
                    Inactivity::Keep => {}
                }
                level.client_think(slot, cmd);
            }
        }
        self.flush_game_commands();
    }

    /// `SV_ClientEnterWorld`: `CS_ACTIVE` and `ClientBegin`.
    fn enter_world(&mut self, slot: usize) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = State::Active;
        log::info!("cod4: client {slot} entered the world");
        if let Some(level) = self.game.level.as_mut() {
            level.begin(slot);
        }
    }

    /// `SV_SendClientGameState`: the unacknowledged commands, the
    /// configstrings by the default rule, the baselines, then
    /// `CS_CLIENTLOADING`.
    fn send_gamestate(&mut self, slot: usize) {
        let feed = self.core.checksum_feed;
        let c = self.core.clients[slot].as_mut().unwrap();
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        conn::write_pending_commands::<Cod4Game>(&mut w, c);
        let gs = Gamestate::from_table(
            c.link.netchan.reliable_sequence as i32,
            &self.core.configstrings,
            &self.game.baselines,
            slot as i32,
            feed,
        );
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        self.transmit_gamestate(slot, w.into_ops());
    }

    /// `SV_DropClient` (0x817096e): the slot turns `CS_ZOMBIE`; everyone
    /// else reads `e "\x15<name>^7 \x14<reason>"` (`EXE_LEFTGAME` for a
    /// disconnect; no `\x14` for a reason the server's strings lack,
    /// cod4-handshake.md 2) and `K <slot>`; a client in the world its reason as
    /// `w "<reason>"` on the zombie's next message (VERIFIED live for
    /// `EXE_CANNOTVALIDATEPURECLIENT`), one still loading as an OOB
    /// `disconnect <reason>`.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if c.state == State::Zombie {
            return;
        }
        let was = c.state;
        c.state = State::Zombie;
        let name = client_name(&c.link.userinfo);
        let addr = c.link.addr;
        log::info!("{slot}:{name} {reason}");
        self.core.challenges.release(addr);
        if let Some(level) = self.game.level.as_mut() {
            level.disconnect(slot);
        }
        self.flush_game_commands();
        let shown = if reason.eq_ignore_ascii_case("EXE_DISCONNECTED") {
            "EXE_LEFTGAME"
        } else {
            reason
        };
        let mark = if self.code_string(shown) {
            "\u{14}"
        } else {
            ""
        };
        self.send_to_all(&format!("e \"\u{15}{name}^7 {mark}{shown}\""));
        self.send_to_all(&format!("K {slot}"));
        if was == State::Active {
            self.send_server_command(slot, &format!("w \"{reason}\""));
        } else {
            self.send_oob(addr, &format!("disconnect {reason}"));
        }
    }

    /// Whether `localized_code_post_gfx_mp`, the one localized zone the
    /// dedicated server loads, holds `key` (`SEH_StringEd_GetString`,
    /// 0x817f4b0).
    fn code_string(&self, key: &str) -> bool {
        let at = COMMON_ZONES
            .iter()
            .position(|&z| z == "localized_code_post_gfx_mp");
        at.and_then(|i| self.game.common.get(i))
            .is_some_and(|z| z.localize.iter().any(|l| l.name.eq_ignore_ascii_case(key)))
    }

    /// `SV_CheckTimeouts` (0x8175830, cod4-server-frame.md 3): a future
    /// `lastPacketTime` clamped, a zombie silent `sv_zombietime` freed, a
    /// client silent past `sv_timeout` (`sv_connectTimeout` while loading)
    /// dropped on its sixth stale call.
    fn check_timeouts(&mut self) {
        let svs = self.core.sv_time_ms;
        let active = svs - self.int("sv_timeout") * 1000;
        let loading = svs - self.int("sv_connectTimeout") * 1000;
        let zombie = svs - self.int("sv_zombietime") * 1000;
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            c.link.last_packet_ms = c.link.last_packet_ms.min(svs);
            let last = c.link.last_packet_ms;
            match c.state {
                State::Zombie if last < zombie => {
                    log::debug!("Going from CS_ZOMBIE to CS_FREE for client #{slot}");
                    self.core.clients[slot] = None;
                }
                State::Zombie => {}
                s => {
                    let limit = if s == State::Active { active } else { loading };
                    if last >= limit {
                        c.link.timeout_count = 0;
                    } else {
                        c.link.timeout_count += 1;
                        if c.link.timeout_count > TIMEOUT_CALLS {
                            self.drop_client(slot, "EXE_TIMEDOUT");
                        }
                    }
                }
            }
        }
    }

    /// Every changed configstring as `d <index> <string>` to the clients
    /// holding a table (`SV_SetConfigstring`, Q3's form).
    fn broadcast_configstrings(&mut self) {
        self.write_engine_configstrings();
        let changed: Vec<usize> = (0..self.core.configstrings.len())
            .filter(|&i| self.core.sent_configstrings.get(i) != Some(&self.core.configstrings[i]))
            .collect();
        if changed.is_empty() {
            return;
        }
        self.core.sent_configstrings = self.core.configstrings.clone();
        for i in changed {
            let cmd = format!("d {i} {}", self.core.configstrings[i]);
            self.send_to_all(&cmd);
        }
    }

    /// One client state per client with a gamestate: a spectator with its
    /// userinfo's name, as the spectator capture's roster reads (team 3,
    /// `attachedVehEntNum` 1023, VERIFIED live).
    fn roster(&mut self) -> Vec<ClientState> {
        if let Some(level) = self.game.level.as_mut() {
            return self
                .core
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    c.as_ref().filter(|c| c.state != State::Connected)?;
                    // The game's name, which `ClientUserinfoChanged` keeps.
                    let name = level.client_name(i).to_string();
                    Some(level.client_state(i, &name))
                })
                .collect();
        }
        let word = |name: &str| CLIENT_STATE.iter().find(|f| f.name == name).unwrap().word();
        self.core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref().filter(|c| c.state != State::Connected)?;
                let mut cs = ClientState::default();
                cs.0[0] = i as u32;
                cs.0[word("team")] = TEAM_SPECTATOR;
                cs.0[word("attachedVehEntNum")] = ENTITYNUM_NONE;
                let mut name = [0u8; 16];
                for (d, s) in name
                    .iter_mut()
                    .zip(client_name(&c.link.userinfo).bytes().take(15))
                {
                    *d = s;
                }
                let at = word("name[0]");
                for k in 0..4 {
                    cs.0[at + k] = u32::from_le_bytes(name[k * 4..k * 4 + 4].try_into().unwrap());
                }
                Some(cs)
            })
            .collect()
    }

    /// `SV_SendClientSnapshot`: an active client or a zombie gets its
    /// server commands and a snapshot, any other a bare message.
    fn send_message(
        &mut self,
        slot: usize,
        roster: &[ClientState],
        world: &BTreeMap<u32, EntityState>,
    ) {
        let svs = self.core.sv_time_ms;
        let frame = self.frame_ms();
        let (origin, angles) = self.game.spot;
        let map_center = vcod_cod4::net::wire::map_center(
            self.core.configstrings[vcod_cod4::net::parse::CS_MAPCENTER as usize].as_bytes(),
        );
        let flags_base = self.game.snap_flags;
        let client_archive = self.int("sv_clientArchive") != 0;
        let max_rate = self.int("sv_maxRate");
        let dl_cvars = download::DownloadCvars {
            allow: self.int("sv_allowDownload") != 0,
            pure: self.pure(),
            www: self.int("sv_wwwDownload") != 0,
            www_base_url: self.game.cvars().get("sv_wwwBaseURL").to_string(),
            www_disconnected: self.int("sv_wwwDlDisconnected") != 0,
        };
        let level_ps = self
            .game
            .level
            .as_ref()
            .and_then(|l| l.playerstate(slot))
            .cloned();
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        if matches!(c.state, State::Active | State::Zombie) {
            conn::write_pending_commands::<Cod4Game>(&mut w, c);
            let mut ps = level_ps.unwrap_or_else(|| {
                spectator_ps(slot, origin, angles, c.extra.command_time.max(svs - frame))
            });
            ps.sent_angles();
            ps.origin_sent = !(client_archive && origin_echoed(&ps, c.extra.predicted));
            // Every entity but the one the playerstate views from: retail
            // culls none by position (cod4-pmove.md 15).
            let viewer = ps.word(vcod_cod4::ps_word!("clientNum"));
            let to = Frame4 {
                server_time: Msec::from_raw(svs),
                map_center,
                ps,
                entities: world
                    .values()
                    .filter(|e| e.number() != viewer)
                    .copied()
                    .collect(),
                clients: roster.to_vec(),
            };
            conn::write_snapshot(
                &PROTOCOL_17,
                &mut w,
                c,
                to,
                flags_base,
                &self.game.baselines,
            );
        }
        if c.extra.download.active() && c.state != State::Zombie {
            let rate = c.rates.send_rate(max_rate);
            let budget = crate::engine::download::blocks_per_message(rate, c.rates.snapshot_msec);
            let wants_www =
                vcod_common::net::info_value_for_key(&c.link.userinfo, "cl_wwwDownload")
                    .is_some_and(|v| v != "0");
            let root = self.game.install.as_ref().map(|i| i.root().to_path_buf());
            let open = |name: &str| {
                download::servable(root.as_deref()?, name)
                    .and_then(|p| crate::download::DownloadFile::open(&p))
            };
            c.extra
                .download
                .write(&mut w, svs, budget, &dl_cvars, wants_www, &open);
        }
        c.link.ping_ring[c.link.netchan.outgoing_sequence as usize % PACKET_BACKUP] = (svs, -1);
        let ops = w.into_ops();
        self.transmit(slot, ops);
    }

    /// A new client in `slot`, a spectator in `CS_CONNECTED`.
    fn admit(
        &mut self,
        slot: usize,
        from: SocketAddr,
        qport: u16,
        challenge: i32,
        guid: String,
        userinfo: &str,
    ) {
        let svs = self.core.sv_time_ms;
        let mut link = crate::engine::slot::Link::new(
            from,
            ServerNetchan::with_params(qport, challenge, NetchanParams::COD2),
            userinfo.to_string(),
        );
        link.next_message_ms = svs;
        link.last_packet_ms = svs;
        if let Some(level) = self.game.level.as_mut() {
            level.connect(slot, &game_name(userinfo), self.game.spot);
            level.set_guid(slot, &guid);
        }
        // `Sys_IsLANAddress` with the subnet table (0x81d59c8), the
        // pacing's and the ping limits' test; only the challenge takes
        // the subnet-blind one (cod4-handshake.md 15).
        let lan = link::Lan::Cod2.is_lan(from.ip());
        self.core.clients[slot] = Some(Client::connected(
            link,
            svs,
            link::Rates::of(userinfo, false, 1),
            lan,
            voice::client_voice(userinfo),
            Cod4Client {
                command_time: svs,
                predicted: ([0.0; 3], 0),
                stats: Stats(Box::new([0; vcod_cod4::net::stats::STATS_BYTES])),
                stat_packets: 0,
                guid,
                download: download::Download::default(),
                stall_exp: 0,
                flood_count: 0,
            },
        ));
    }

    /// A client's stats block as the server holds it.
    pub fn stats(&self, slot: usize) -> Option<&Stats> {
        self.core
            .clients
            .get(slot)?
            .as_ref()
            .map(|c| &c.extra.stats)
    }

    /// The GUID a client gave with its `getchallenge`.
    pub fn guid(&self, slot: usize) -> Option<&str> {
        self.core
            .clients
            .get(slot)?
            .as_ref()
            .map(|c| c.extra.guid.as_str())
    }
}

/// `Dvar_InfoString(0x404)` over `cvars`: configstring 0, `getstatus`'s
/// info and the log's `InitGame` line, case-folded order.
fn serverinfo_of(cvars: &crate::cvars::Cvars) -> String {
    info_sorted(
        oob::SERVERINFO
            .iter()
            .map(|n| (n.to_string(), cvars.get(n).to_string())),
    )
}

/// A config file out of the iwds with each `exec <name>` line replaced by
/// that file's text, as `Cbuf` runs it; a file no iwd holds reads as
/// empty. `server_map.cfg` (in `iw_00.iwd`) sets `sv_mapRotation`. `iw_00.iwd` holds two
/// `default_mp_gamesettings.cfg`, and retail reads the later one: no
/// `scr_game_playerwaittime` line, so the prematch waits the script's 15 s
/// (VERIFIED live, rcon and the game end time 619800).
fn iwd_cfg(fs: &dyn vcod_common::asset_source::AssetSource, name: &str, depth: u32) -> String {
    let Some(raw) = fs.read(name) else {
        return String::new();
    };
    let mut out = String::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        let t = crate::game::say::tokenize(line);
        match t.as_slice() {
            [exec, file, ..] if exec.eq_ignore_ascii_case("exec") && depth < 8 => {
                out.push_str(&iwd_cfg(fs, file, depth + 1));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Whether a command goes `SV_CMD_CAN_IGNORE`, read off its letter: the
/// menu notify `J` (the walk capture's four `J "1"` in a row, never
/// squashed, VERIFIED live), the team and player scores `G`, `H`, `I`, the
/// game messages `c`, `e`, `f`, `g` and the `iPrintLn` whose letter a
/// player argument cost it (the capture's ` "MP_CONNECTED\x15walker^7"`),
/// the chat lines `h`, `i`. Everything else is `SV_CMD_RELIABLE`.
fn ignorable(text: &str) -> bool {
    matches!(
        text.as_bytes().first(),
        Some(b'J' | b'G' | b'H' | b'I' | b'c' | b'e' | b'f' | b'g' | b'h' | b'i' | b' ')
    )
}

/// The `clc_clientCommand`s among a message's ops.
fn commands(ops: &[ClOp]) -> usize {
    ops.iter()
        .filter(|op| matches!(op, ClOp::Command { .. }))
        .count()
}

/// The engine's copy of a userinfo's `name`: its first 15 characters
/// (`SV_UserinfoChanged` 0x8170320 copies it into 0x10 bytes).
fn client_name(userinfo: &str) -> String {
    vcod_common::net::info_value_for_key(userinfo, "name")
        .unwrap_or("")
        .chars()
        .take(15)
        .collect()
}

/// The game's copy (`ClientUserinfoChanged` 0x80a8068, cod4-gsc.md 12):
/// leading spaces dropped, at most 15 characters, `UnnamedPlayer` when
/// empty.
fn game_name(userinfo: &str) -> String {
    let name: String = vcod_common::net::info_value_for_key(userinfo, "name")
        .unwrap_or("")
        .trim_start_matches(' ')
        .chars()
        .take(15)
        .collect();
    if name.is_empty() {
        "UnnamedPlayer".into()
    } else {
        name
    }
}

/// `SV_UpdateUserinfo_f` (0x8170536) copies the userinfo into 0x400 bytes.
const MAX_USERINFO: usize = 1023;

/// `MSG_WriteDeltaPlayerstate`'s test for leaving origin and velocity out
/// (`cod4_lnxded` 0x813f5b5..0x813f615): the move's predicted origin within
/// 0.1 of `ps`'s (its square at most the float 0.01 at 0x8209644) and its
/// time `commandTime` (cod4-netfields.md 3).
fn origin_echoed(ps: &PlayerState, (origin, time): ([f32; 3], i32)) -> bool {
    let at = vcod_cod4::ps_word!(3 "origin");
    let d2: f64 = (0..3)
        .map(|k| f64::from(f32::from_bits(ps.word(at[k]))) - f64::from(origin[k]))
        .map(|d| d * d)
        .sum();
    d2 <= f64::from(0.01f32) && ps.word(vcod_cod4::ps_word!("commandTime")) as i32 == time
}

/// A spectator floating at the intermission spot: the fields the retail
/// spectator's first snapshot carries (`play-1.7-spec.txt`, VERIFIED live:
/// `pm_type` 4, `eFlags` 2, gravity 800, `moveSpeedScaleMultiplier` 1, the
/// depth of field's 6 and 1.8, `throwBackGrenadeOwner` and
/// `viewlocked_entNum` 1023, `stats[3]` 63, `stats[4]` 1).
fn spectator_ps(slot: usize, origin: [f32; 3], angles: [f32; 3], command_time: i32) -> PlayerState {
    let mut ps = PlayerState::default();
    let mut set = |name: &str, v: u32| {
        let f = PLAYER_STATE
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no playerstate field {name}"));
        ps.words[f.word()] = v;
    };
    set("commandTime", command_time as u32);
    for i in 0..3 {
        set(&format!("origin[{i}]"), origin[i].to_bits());
        // The wire's half-short rounding reads the capture's yaw 65 as
        // 65.00061 (`angle16_sent`, VERIFIED live).
        let a = angles[i];
        set(&format!("viewangles[{i}]"), a.to_bits());
        set(&format!("delta_angles[{i}]"), a.to_bits());
    }
    set("eFlags", 2);
    set("moveSpeedScaleMultiplier", 1.0f32.to_bits());
    set("throwBackGrenadeOwner", ENTITYNUM_NONE);
    set("viewlocked_entNum", ENTITYNUM_NONE);
    set("gravity", 800);
    set("dofNearBlur", 6.0f32.to_bits());
    set("dofFarBlur", 1.8f32.to_bits());
    set("clientNum", slot as u32);
    set("pm_type", PM_SPECTATOR);
    ps.stats[3] = 63;
    ps.stats[4] = 1;
    ps
}

/// `mp_global_intermission`'s origin and angles.
fn intermission_spot(ents: &[Vec<(String, String)>]) -> Option<([f32; 3], [f32; 3])> {
    let e = ents.iter().find(|e| {
        e.iter()
            .any(|(k, v)| k == "classname" && v == "mp_global_intermission")
    })?;
    let vec = |k: &str| {
        e.iter()
            .find(|(a, _)| a == k)
            .and_then(|(_, v)| vcod_common::bsp::parse_vec3(v))
    };
    Some((vec("origin")?, vec("angles").unwrap_or([0.0; 3])))
}

/// `sv_referencedFFNames` and their byte sizes: `code_post_gfx_mp`, the
/// map, `common_mp` on every 1.7 capture (VERIFIED, cod4-handshake.md 8),
/// then a mod's `<fs_game>/mod`; a usermap as `usermaps/<map>` and, when
/// it has one, `usermaps/<map>_load`, both sizes under its slot. The mod's
/// and the usermap's shapes are `DB_ReferencedFastFiles`', INFERRED
/// (cod4-fastfile.md 13).
fn referenced_fastfiles(install: &Install, lang: &str, map: &str) -> Vec<(String, String)> {
    use vcod_cod4::install::ZoneOrigin;
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut out = Vec::new();
    for n in ["code_post_gfx_mp", map, "common_mp"] {
        let Some(f) = install.zone_file(lang, n) else {
            out.push((n.to_string(), "0".to_string()));
            continue;
        };
        if f.origin != ZoneOrigin::Usermaps {
            out.push((n.to_string(), size(&f.path).to_string()));
            continue;
        }
        let load = install
            .zone_file(lang, &format!("{n}_load"))
            .filter(|l| l.origin == ZoneOrigin::Usermaps);
        match load {
            Some(l) => {
                let sums = format!("{} {}", size(&f.path), size(&l.path));
                out.push((format!("usermaps/{n} usermaps/{n}_load"), sums));
            }
            None => out.push((format!("usermaps/{n}"), size(&f.path).to_string())),
        }
    }
    if let (Some(g), Some(p)) = (install.fs_game(), install.mod_zone()) {
        out.push((format!("{g}/mod"), size(&p).to_string()));
    }
    out
}

/// The non-localized iwds of `iwds` a file was read from, lowest priority
/// first (`main/iw_11 mods/modwarfare/z_modwarfare` on retail, VERIFIED
/// live), each as `<game>/<name>` and its checksum; the `fs_game`
/// directory's named under `fs_game`, a usermap's (`usermap`) under
/// `usermaps/<map>` (`FS_AddGameDirectory`'s name for it,
/// INFERRED).
fn referenced_iwds(
    iwds: &vcod_common::pk3::Pk3Fs,
    fs_game: &str,
    usermap: Option<&Path>,
    map: &str,
) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = iwds
        .paks_read()
        .filter(|(p, read)| *read && !p.localized)
        .map(|(p, _)| {
            let game = if p.game.eq_ignore_ascii_case("main") {
                "main".to_string()
            } else if usermap.is_some_and(|u| p.path.parent() == Some(u)) {
                format!("usermaps/{map}")
            } else {
                fs_game.to_string()
            };
            let sum = vcod_cod4::net::pure::iwd_checksum(&p.crcs, 0);
            (format!("{game}/{}", p.name.to_ascii_lowercase()), sum)
        })
        .collect();
    out.reverse();
    out
}

/// `FS_LoadedIwdChecksums` and `FS_LoadedIwdNames` over the non-localized
/// iwds of [`Install::iwd_dirs`] (lowest priority first), search order
/// first: the mod's, a usermap's, then `main`'s last name first (`iw_13`
/// leads, VERIFIED live), and each sum keyed with the gamestate's feed
/// for the `cp` check.
fn pure_lists(dirs: &[PathBuf], feed: i32) -> anyhow::Result<Iwds> {
    let mut out = Iwds::default();
    for dir in dirs.iter().rev() {
        pure_lists_in(dir, feed, &mut out)?;
    }
    Ok(out)
}

fn pure_lists_in(main: &Path, feed: i32, out: &mut Iwds) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut names: Vec<String> = std::fs::read_dir(main)
        .with_context(|| main.display().to_string())?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.ends_with(".iwd") && !l.starts_with("localized_")
        })
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.to_ascii_lowercase()));
    for n in names {
        let path = main.join(&n);
        let f = std::fs::File::open(&path).with_context(|| path.display().to_string())?;
        let crcs = vcod_common::pak_checksum::entry_crcs(std::io::BufReader::new(f))
            .with_context(|| path.display().to_string())?;
        out.sums.push(vcod_cod4::net::pure::iwd_checksum(&crcs, 0));
        out.keyed
            .push(vcod_cod4::net::pure::iwd_checksum(&crcs, feed));
        out.names.push(n.trim_end_matches(".iwd").to_string());
    }
    Ok(())
}

/// An info string in `Dvar`'s case-folded order; empty values left out.
fn info_sorted(pairs: impl Iterator<Item = (String, String)>) -> String {
    let mut pairs: Vec<(String, String)> = pairs.collect();
    // By the folded key, compared without building it.
    pairs.sort_by(|(a, _), (b, _)| {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    });
    let mut info = vcod_common::net::connectionless::Info::new();
    for (k, v) in &pairs {
        info.set(k, v);
    }
    info.to_string()
}

/// `SV_ClientCommand`'s flood rule (`cod4_lnxded` 0x8170c97..0x8170f97,
/// cod4-handshake.md 16): whether `text` may reach the game. Under
/// `sv_floodprotect` n, an active client's command other than one opening
/// `team `, `score` or `mr ` (case for case) is refused when no count is
/// left and it comes inside 800 ms of the last counted one; each counted
/// one, refused or not, sets the count to n - 1 past the window or takes
/// one off it inside, and opens a new window. The engine's own commands
/// run either way.
fn flood_check(c: &mut Client, floodprotect: i32, svs: i32, text: &str) -> bool {
    let exempt = [&b"team "[..], b"score", b"mr "]
        .iter()
        .any(|p| text.as_bytes().starts_with(p));
    if floodprotect == 0 || c.state != State::Active || exempt {
        return true;
    }
    let ok = c.extra.flood_count != 0 || svs >= c.link.next_reliable_ms;
    c.extra.flood_count = if svs >= c.link.next_reliable_ms {
        floodprotect.saturating_sub(1)
    } else {
        (c.extra.flood_count - 1).max(0)
    };
    c.link.next_reliable_ms = svs.wrapping_add(800);
    ok
}

#[cfg(test)]
mod blast_tests;
#[cfg(test)]
mod capture_ab;
#[cfg(test)]
mod combat_ab;
#[cfg(test)]
mod conn_tests;
#[cfg(test)]
mod console_tests;
#[cfg(test)]
mod damage_soak;
#[cfg(test)]
mod destructible_tests;
#[cfg(test)]
mod disconnect_tests;
#[cfg(test)]
mod download_tests;
#[cfg(test)]
mod gamelog_tests;
#[cfg(test)]
mod gametype_tests;
#[cfg(test)]
mod hardpoint_ab;
#[cfg(test)]
mod hardpoint_tests;
#[cfg(test)]
mod join_tests;
#[cfg(test)]
mod mod_tests;
#[cfg(test)]
mod oob_ab;
#[cfg(test)]
mod pen_pin_tests;
#[cfg(test)]
mod perk_tests;
#[cfg(test)]
mod planefx_ab;
#[cfg(test)]
mod prematch_ab;
#[cfg(test)]
mod shock_tests;
#[cfg(test)]
mod shot_aim_ab;
#[cfg(test)]
mod stat_tests;
#[cfg(test)]
mod test_harness;
#[cfg(test)]
mod vote_tests;
#[cfg(test)]
mod wire_ab;

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    /// `master::resolve`'s shape: the name's host, port 0.
    fn resolve(name: &str) -> Option<SocketAddr> {
        match name {
            "master.cod.pm" => Some("198.51.100.4:0".parse().unwrap()),
            "198.51.100.9" => Some("198.51.100.9:0".parse().unwrap()),
            "198.51.100.7" => Some("198.51.100.7:0".parse().unwrap()),
            _ => None,
        }
    }

    /// An empty `sv_master1` beats `master.cod.pm` on 20810 at `dedicated
    /// 2`, and nothing at the default `dedicated 1`; a resolved name takes
    /// the CoD4 port, and a set `sv_master1` beats its host at the next
    /// beat the `heartbeat` command forces.
    #[test]
    fn dedicated_2_beats_master_cod_pm_by_default() {
        let cfg = Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        };
        let mut sv = Cod4Server::new(cfg, 7);
        sv.core.resolver = resolve;
        sv.heartbeat();
        assert!(sv.core.outbox.is_empty());
        sv.set_cvar("dedicated", "2");
        sv.heartbeat();
        let (to, pkt) = sv.core.outbox.pop().expect("a heartbeat");
        assert_eq!(to, "198.51.100.4:20810".parse().unwrap());
        assert!(pkt.ends_with(b"heartbeat COD-4\n"));
        sv.heartbeat();
        assert!(sv.core.outbox.is_empty(), "the next is 180 s on");
        sv.set_cvar("sv_master1", "198.51.100.9");
        sv.push_console("heartbeat");
        sv.run_console();
        sv.heartbeat();
        let (to, _) = sv.core.outbox.pop().expect("a forced heartbeat");
        assert_eq!(to, "198.51.100.9:20810".parse().unwrap());
        // `sv_master2` beats alongside the default `sv_master1`.
        sv.set_cvar("sv_master1", "");
        sv.set_cvar("sv_master2", "198.51.100.7");
        sv.push_console("heartbeat");
        sv.run_console();
        sv.core.sv_time_ms += 1000;
        sv.heartbeat();
        let to: Vec<SocketAddr> = sv.core.outbox.drain(..).map(|(to, _)| to).collect();
        let want: [SocketAddr; 2] = [
            "198.51.100.4:20810".parse().unwrap(),
            "198.51.100.7:20810".parse().unwrap(),
        ];
        assert_eq!(to, want);
    }
}
