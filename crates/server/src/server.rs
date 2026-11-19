//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The server state machine, transport-free. `main.rs` owns the socket, the
//! tests own a queue. Ported from RTCW-MP sv_main.c / sv_client.c; reply
//! strings come from docs/research/cod11-server-handshake.md.

use crate::client::{Client, ClientState, PacketCommand, Queued, QueuedCmd, sanitize_name};
use crate::compass;
use crate::configstrings;
use crate::console;
use crate::engine::bans::{BanFile, guid_of};
use crate::engine::core::rand_from;
use crate::follow;
use crate::game::combat::Effect;
use crate::game::host::{ClientEvent, SpawnMode};
use crate::game::say::SayMode;
use crate::game::script;
use crate::game::script::MenuRef;
use crate::game::stuck::{StuckRules, StuckView, stuck_in_client};
use crate::game::temp_entity;
use crate::spectate::{ClientSim, PmType};
use crate::world::{TestEntities, World};
use vcod_common::asset_source::RawFile;

pub mod bot_admin;
mod bot_vehicles;
mod bot_world;
mod breach;
mod client_msg;
mod cod1_frame;
mod cod1_game;
mod cod1_think;
mod configstring_sync;
mod connectionless;
mod console_cmds;
mod follow_frame;
mod frame;
mod level;
mod map_cycle;
mod sim_ops;
mod slots;
mod snapshots;
mod test_hooks;
mod vehicles;
pub(crate) use breach::cone_clear;
use breach::{
    BREACH_MODEL_NEAR, BREACH_SECTORS, BREACH_STAND_FAR, BREACH_STAND_IDEAL, breach_aim,
    first_passes, held_rocket, shell_aim, shell_lands,
};
pub(crate) use client_msg::parse_menu_response;
use cod1_game::{Cod1Game, Cod1Module, FrameOut, Lend};
use follow_frame::{follow_end_frame, pass_followers_on};
use sim_ops::{
    apply_callback_ops, apply_sim_ops, apply_weapon_ops, deliver_turret_rounds, fall_damage,
    followable, link_client, mirror_for_callback, mirror_roster, mirror_vitals, mirror_weapons,
    push_mover, push_team, relink, run_client_link, scoreboard, session_of,
};
use slots::{write_download, write_pending_commands};
use snapshots::{compass_friend, fog_cull_of};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::movetrace::CONTENTS_CORPSE;
use vcod_common::net::connectionless::{Info, build_oob, parse_connect, parse_oob};
use vcod_common::net::gamestate::Gamestate;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, CLC_BITS, CLC_CLIENT_COMMAND, CLC_EOF, CLC_MOVE, CLC_MOVE_NO_DELTA, MsgReader, MsgWriter,
    NULL_USERCMD, UserCmd, read_delta_usercmd_in,
};
use vcod_common::net::netchan::{ClientMessage, MAX_RELIABLE_COMMANDS, ServerNetchan};
use vcod_common::net::protocol::WireGame;
use vcod_common::net::protocol::{CS_SERVERINFO, Cod1Wire, PROTOCOL_V1};
use vcod_common::net::{com_hash_key, flags, info_value_for_key, snapshot};
use vcod_common::pmove::FallHeights;
use vehicles::{
    VehicleFireCtx, apply_seat_ops, dobj_stamps, place_rider, run_over, vehicle_fire,
    vehicle_frame, vehicle_player_damage,
};

/// `SNAPFLAG_NOT_ACTIVE`, or'd at cod_lnxded 0x808dfe3.
const SNAPFLAG_NOT_ACTIVE: u32 = 2;
/// `sv_timeout` default (cod_lnxded 0x80d56c0).
const TIMEOUT_DEFAULT: &str = "240";
/// `sv_zombietime` default (cod_lnxded 0x80d56cf): how long a dropped slot
/// stays `CS_ZOMBIE` past the client's last packet.
const ZOMBIE_TIME: Duration = Duration::from_secs(2);
/// `sv_reconnectlimit` default. Also how long a slot must have been silent
/// before a connect carrying a different challenge may reclaim it.
const RECONNECT_LIMIT: Duration = Duration::from_secs(3);
const MAX_PACKET_USERCMDS: u8 = vcod_common::net::MAX_MOVE_CMDS as u8;
/// Queued-but-unreplayed usercmds per client; past this a flood drops the
/// oldest rather than building latency.
const MAX_PENDING_CMDS: usize = 64;
/// Cmds a client replays per tick; past this only its newest runs on, over
/// the whole span the dropped ones covered (vcod's flood bound, not retail's).
const MAX_CMDS_PER_TICK: usize = 32;
/// `SV_ClientCommand`'s flood window (`cod_lnxded` 0x8086f5f, `add eax,0x320`):
/// a non-exempt client command opens 800 ms during which every further
/// non-exempt one from an active client is dropped before the game sees it
/// (`docs/protocol-1.1.md`, "Client commands are flood-protected").
const FLOOD_WINDOW_MS: i32 = 800;
/// How long the script's brush links hold still before the bots' graph
/// is rebuilt over them (`Server::relink_nav`).
const NAV_RELINK_MS: i32 = 2000;
/// A retrieval objective as [`Server::bot_re`] reads it: the view with the
/// carrier's feet in `carrier_at` for every team, the carrier's slot, and
/// the compass record's `teamNum`.
type ReObjCarried = (crate::bots::ReObjView, Option<usize>, i32);
/// The tick pace (`1000 / sv_fps`), also the dt floor a fresh sim starts from.
pub(crate) const FRAME_MS: i32 = 50;

/// `SV_SendClientGameState`'s message buffer (`MSG_Init` at 0x8085fa7):
/// the plain bytes a gamestate can fill (cod11-server-handshake.md,
/// "The gamestate goes out fragmented"). 1.5b's is the same; United
/// Offensive's is twice it ([`MAX_GAMESTATE_BYTES_UO`]).
const MAX_GAMESTATE_BYTES: usize = 0x4000;
/// United Offensive's `SV_SendClientGameState` (coduo_lnxded 0x808bc48)
/// builds the gamestate in a 32768-byte buffer (coduo-diff.md 7).
const MAX_GAMESTATE_BYTES_UO: usize = 0x8000;
/// Retail's `MAX_CLIENTS`. Client slots index a 6-bit wire field
/// (clientState entries; `ps.clientNum` gets 8), so more than 64 would
/// collide silently.
pub(crate) use vcod_common::net::protocol::MAX_CLIENTS;

#[derive(Clone)]
pub struct ServerConfig {
    pub map: String,
    pub hostname: String,
    pub max_clients: usize,
    /// `g_gametype` as text (`dm`, `tdm`, `sd`).
    pub gametype: String,
    /// Scripted entities that exercise the packet-entity wire path. 0 is off.
    pub test_entities: usize,
    /// Log one line per snapshot per client with the numbers that drive a
    /// client's prediction. Off by default; a busy server would flood.
    pub trace: bool,
    /// Bots added at start (docs/bots.md). Each takes a real slot; 0 is off.
    pub bots: usize,
    /// Whether the bots fight. Without it they only roam.
    pub bots_shoot: bool,
}

/// What `SV_VerifyPaks_f` (cod_lnxded 0x808674c) checks a `cp` against: the
/// pure checksums of the paks holding the two client DLLs and of every
/// non-localized pak (`FS_LoadedPakPureChecksums`), keyed with the level's
/// `checksumFeed` (docs/research/cod11-server-handshake.md, "Pak checksums").
struct PureCheck {
    cgame: Option<i32>,
    ui: Option<i32>,
    loaded: Vec<i32>,
    feed: i32,
}

impl PureCheck {
    fn new(fs: &vcod_common::pk3::Pk3Fs, feed: i32, proto: &Cod1Wire) -> Self {
        let dll = |name: &str| fs.source_pak(name).map(|p| p.pure_checksum(feed));
        PureCheck {
            cgame: dll(proto.pure_dlls[0]),
            ui: dll(proto.pure_dlls[1]),
            loaded: fs
                .paks()
                .filter(|p| !p.localized)
                .map(|p| p.pure_checksum(feed))
                .collect(),
            feed,
        }
    }

    fn verify(&self, cmd: &str) -> bool {
        let args: Vec<&str> = cmd.split_whitespace().collect();
        vcod_common::pak_checksum::verify_pure(&args, self.cgame, self.ui, &self.loaded, self.feed)
    }
}

/// One op of a client message, read out before any of it is applied.
enum ClientOp {
    Command {
        seq: i32,
        text: String,
    },
    /// Every usercmd of a move block, in wire order, and whether it came
    /// as `clc_move` (true) or `clc_moveNoDelta`.
    Move {
        cmds: Vec<UserCmd>,
        delta: bool,
    },
}

/// One shot a client's weapon step took this tick: the queue between the
/// move that pulled the trigger and the trace `tick` answers it with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shot {
    pub slot: usize,
    /// `ps.weapon` at the shot, which the delayed trace must use rather than
    /// whatever the client is holding by the time it runs.
    pub weapon: u8,
    /// Whether the shot left a settled sight (`fWeaponPosFrac == 1.0`),
    /// which picks `adsSpread` over the hip spread (combat doc, 2.1).
    pub ads: bool,
    /// The aim the cmd's aim block left (`ClientSim::aim_angles`), which
    /// down a sight is the swayed gun rather than the view (combat doc, 15).
    pub aim: [f32; 2],
    /// The stance and the eye's leg the hip minimum is read off, at the
    /// cmd's `commandTime` against the frame's clock (combat doc, 2.1).
    pub stance: vcod_common::pmove::weapon::SpreadStance,
}

/// One attack a client's weapon step took this tick. The weapon index each
/// arm carries is `ps.weapon` at the event, not whatever is in hand by the
/// time the trace runs, and the aim is the cmd's, for the same reason.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Attack {
    Shot(Shot),
    /// `EV_FIRE_MELEE`, the swing's damage frame (combat doc, 2.5).
    Swing {
        slot: usize,
        weapon: u8,
        aim: [f32; 2],
    },
    /// `EV_FIRE_WEAPON` from a grenade: the fuse left rides the event parm
    /// (combat doc, 1.11).
    Throw {
        slot: usize,
        weapon: u8,
        fuse_left_ms: i32,
        aim: [f32; 2],
    },
    /// `EV_LANDING_PAIN_*`: the fall damage percent rides the event parm
    /// (player-clip doc, 8.8).
    Fall {
        slot: usize,
        percent: i32,
    },
    /// UO's `EV_GRENADE_SUICIDE`: a frag cooked past its fuse went off in
    /// the hand, and `ClientEvents` (game.mp.uo 0x40860, case 0xd2) runs
    /// `player_die` on the thrower with `MOD_SUICIDE`, as `kill` does.
    Suicide {
        slot: usize,
    },
    /// A UO `spotter` weapon's fire: `Weapon_ArtilleryStrike_Fire`
    /// (game.mp.uo 0x7b758) calls a strike where the aim lands.
    Strike(Shot),
    /// A `projectile` weapon's fire: `Weapon_RocketLauncher_Fire` (0x68c40)
    /// launches a rocket along the spread aim (combat doc 13.6).
    Rocket(Shot),
}

/// `Weapon_RocketLauncher_Fire`'s push on the shooter along the view
/// (`.rodata 0x79c80`, combat doc 13.6).
const ROCKET_KICK: f32 = -64.0;

/// How long an `fdc` report keeps a client burning.
const FLAME_REPORT_MS: i32 = 3000;

/// How long a fall holds off `P_DamageFeedback`'s `EV_PAIN`: `ClientEvents`
/// stores `level.time + 200` into `pain_debounce_time` (player-clip doc 8.8).
const FALL_PAIN_DEBOUNCE_MS: i32 = 200;

/// One `WeaponOp` against a client's playerstate. The op is an edge the
/// script made, so it is applied once, where `client_weapons` is mirrored
/// every frame.
pub(crate) fn apply_weapon_op(
    sim: &mut crate::spectate::ClientSim,
    op: crate::game::host::WeaponOp,
) {
    use crate::game::host::WeaponOp;
    let ps = &mut sim.ps;
    match op {
        WeaponOp::SetClip { clip_index, rounds } => {
            if let Some(slot) = ps.ammoclip.get_mut(clip_index) {
                *slot = rounds;
            }
        }
        WeaponOp::SetAmmo { ammo_index, rounds } => {
            if let Some(slot) = ps.ammo.get_mut(ammo_index) {
                *slot = rounds;
            }
        }
        WeaponOp::TakeAll => {
            ps.ammo = [0; vcod_common::pmove::weapon::NUM_AMMO];
            ps.ammoclip = [0; vcod_common::pmove::weapon::NUM_AMMO];
        }
        WeaponOp::SetCurrent(index) => {
            ps.weapon = index;
            ps.weaponstate = vcod_common::pmove::weapon::WEAPON_READY;
            ps.weapon_time_ms = 0;
        }
    }
}

/// One cmd that moved a client, as the host mirrors and the touch pass need it:
/// where the cmd left the client, the buttons it carried, the `pm_type` the
/// pass gates on, whether it left the client on the ground, a player's view
/// yaw (a spectator's `SpectatorThink` arm writes no angles), and the
/// `ps.weapon` the cmd left once the tick's moves have switched it and the
/// `clipOnly` weapon its last round emptied, both of which an item grab
/// reads.
struct Touched {
    slot: usize,
    origin: [f32; 3],
    buttons: u8,
    pm_type: i32,
    on_ground: bool,
    yaw: Option<f32>,
    weapon: Option<u8>,
    /// `ps.weaponslots` after a select-fire switch moved a slot to the mode
    /// now in hand (combat doc, 1.8); the host owns the slots otherwise.
    slots: Option<[u8; vcod_common::pmove::weapon::NUM_SLOTS]>,
    /// A `clipOnly` weapon this cmd spent the last round of (combat doc, 1.5
    /// step 9); pmove cannot take it because the script host owns
    /// `ps.weapons`.
    take: Option<u8>,
    /// The eye and `ps.viewangles` the cmd left, for the use key's aim.
    eye: [f32; 3],
    view: [f32; 3],
    /// The stance a mount saves for the release.
    stance: vcod_common::pmove::Stance,
    prone_cone: Option<vcod_common::pmove::ProneCone>,
    /// The entities the cmd's `Pmove` touched, for `ClientImpacts`.
    impacts: Vec<u32>,
}

/// One client's cmds since its animation was last picked: the events they
/// raised, the last of them, and what it held before the first.
#[derive(Default)]
struct Round {
    held: Option<u8>,
    switched: bool,
    /// Each event the round raised, with its cmd's `serverTime`.
    events: Vec<crate::spectate::AnimEvent>,
    last_cmd: Option<UserCmd>,
}

/// One candidate of a grenade's walk: a client, or an entity with no client.
enum BlastCandidate<'a> {
    Client(usize, &'a ClientSim),
    Entity(vcod_gsc::EntId),
}

/// What one client's usercmd replay did this tick, for the trace line.
#[derive(Default, Clone, Copy)]
struct MoveSummary {
    processed: usize,
    first_cmd_st: Option<i32>,
    last_cmd_st: Option<i32>,
    /// The last replayed cmd's buttons, for `useButtonPressed`: retail
    /// stores each cmd's buttons on the client and the builtin tests that
    /// word, so a tick that replayed several cmds answers with the last one,
    /// not an OR (docs/research/cod11-gsc-object-model.md, 23.5). `None`
    /// when the tick replayed nothing, so the mirror falls back to the
    /// client's last received cmd.
    last_buttons: Option<u8>,
}

/// Why a level load failed, which is what says whether the server can go on.
#[derive(Debug)]
pub enum LoadFailure {
    /// Refused before anything was torn down, so the level that is serving is
    /// untouched and the console line was a no-op.
    KeptLevel(anyhow::Error),
    /// Failed with the level already gone: no script to call `exitLevel`, no
    /// table for a client to pull. vcod ends the process; retail's path is
    /// unverified (see `Server::take_fatal`).
    Fatal(anyhow::Error),
}

/// The CoD1 server (1.1, 1.5 and UO): the engine over [`Cod1Game`].
pub type Server = crate::engine::Engine<Cod1Game>;

/// A follower's frame: the followed client's number, its playerstate with
/// the follow flags patched in, its eye, and whether it is a replay.
struct FollowFrame {
    target: usize,
    ps: msg::PlayerState,
    eye: [f32; 3],
    replay: bool,
}

/// [`Server::download_opener`]'s file reader.
type Opener = dyn Fn(&str) -> Result<crate::download::DownloadFile, crate::download::Refusal>;

/// What CoD1's `SV_SendClientMessages` builds once for every client's
/// snapshot ([`Server::snapshots_begin`]).
pub struct Cod1Snapshots {
    live: crate::archive::WorldFrame,
    /// Each slot's team, for the HUD filters.
    teams: Vec<i32>,
    follow_frames: Vec<Option<FollowFrame>>,
    /// The archived frame each client's `archivetime` names, and its age.
    sources: Vec<Option<(i32, i32)>>,
    open: Box<Opener>,
    www: Option<crate::download::WwwConfig>,
    max_rate: i32,
    moved: Vec<MoveSummary>,
    wall: (Option<f32>, u32),
}

pub use vcod_common::net::protocol::install_protocol;

impl Server {
    /// A server whose generator starts from a fixed seed, so a test run
    /// replays the same bot picks, script `randomInt`s and spawn choices.
    /// It runs `sv_pure 0`, as `tools/run_server.sh` runs the retail
    /// servers the gates replay; its test clients send no `cp`. A
    /// `set_cvar("sv_pure", "1")` turns the check back on.
    pub fn new(cfg: ServerConfig, now: Instant) -> Self {
        let mut sv = Self::with_seed(cfg, now, crate::game::host::RNG_SEED);
        sv.set_cvar("sv_pure", "0");
        sv
    }

    /// `new` with the generator seeded from `seed`; the binary passes the
    /// wall clock.
    pub fn with_seed(mut cfg: ServerConfig, _now: Instant, seed: u64) -> Self {
        cfg.max_clients = cfg.max_clients.min(MAX_CLIENTS);
        let seed = seed | 1;
        let (bots, bots_shoot) = (cfg.bots, cfg.bots_shoot);
        let server_id = 0x10;
        let fall = FallHeights::default();
        let mut core = crate::engine::EngineCore::new(
            &PROTOCOL_V1.transport,
            cfg.max_clients,
            seed,
            Default::default(),
        );
        core.configstrings = configstrings::static_configstrings(
            &cfg,
            server_id,
            fall,
            false,
            &Default::default(),
            None,
            &PROTOCOL_V1,
        );
        core.sent_configstrings = core.configstrings.clone();
        let game = Cod1Game {
            zombies: (0..cfg.max_clients).map(|_| None).collect(),
            cfg,
            proto: &PROTOCOL_V1,
            server_id,
            module: Cod1Module::new(seed, bots, bots_shoot, fall),
            stuck_rand: None,
            world: None,
            paused_ms: 0,
            baselines: HashMap::new(),
            last_tick: None,
            packet_seq: 0,
            last_send: None,
            sent_time_ms: 0,
            ack_time_ms: None,
            cvar_overrides: Vec::new(),
            fs: None,
            paks: Default::default(),
            systeminfo: None,
            pure_check: None,
            rotation: console::Rotation::default(),
            sv_map_rotation: String::new(),
            // The first map's load toggled it from zero, as every later
            // one does (`spawn_server`).
            snap_flag_server_bit: console::SNAPFLAG_SERVERCOUNT,
            carried_cvars: None,
            level_cvars: None,
            last_spawn_tick: None,
            fatal: None,
            test_client_counter: 0,
            udp_slots: 0,
            archive: Default::default(),
            dedicated: 1,
            rcon_password: String::new(),
            redirect: None,
            killed: false,
            shutdown_log: Vec::new(),
            game_log: Default::default(),
            cheats: false,
            tick_moved: None,
            frame_moved: Vec::new(),
            tick_wall: (None, 0),
        };
        let mut sv = Server { core, game };
        // `(rand() << 16) ^ rand() ^ Sys_Milliseconds()`, SV_SpawnServer
        // 0x808a3e0. The server clock stands in for the uptime, as at a map
        // change: the seed carries the binary's randomness, and a seeded
        // test gets the same feed every run.
        sv.core.checksum_feed = (sv.rand() << 16) ^ sv.rand() ^ sv.core.sv_time_ms;
        if sv.game.cfg.test_entities > 0 {
            let te = TestEntities::new(sv.game.cfg.test_entities, [0.0, 0.0, 64.0]);
            sv.game.baselines = te.baselines(sv.game.proto);
            sv.game.module.test_entities = Some(te);
        }
        // `--bots-peaceful` keeps its bots from firing.
        if sv.game.cfg.bots > 0 && !sv.game.cfg.bots_shoot {
            sv.set_cvar(crate::botmgmt::cvar::SHOOT, "0");
        }
        sv
    }

    pub fn configstring(&self, i: usize) -> &str {
        self.core.configstrings.get(i).map_or("", String::as_str)
    }

    pub fn client_count(&self) -> usize {
        self.core.clients.iter().flatten().count()
    }

    /// `sv.time`, the game clock the last frame ran at (`level.time`).
    pub fn game_time_ms(&self) -> i32 {
        self.core.sv_time_ms
    }

    /// Starts a fresh server's game clock at `ms` rather than 0, before its
    /// first load: a retail capture taken on a server that had been up a
    /// while starts its level there (coduo-diff.md 20).
    pub fn start_clock_at(&mut self, ms: i32) {
        self.core.sv_time_ms = ms;
    }

    /// `sv_serverid`. The high nibble is the map load, the low one the
    /// restart count (docs/research/cod11-map-cycle.md, section 3 step 16).
    pub fn server_id(&self) -> u8 {
        self.game.server_id
    }

    /// `svs.time`: what the network runs on (pacing, pings, timeouts, the
    /// flood window, heartbeats), which a 1.5 match timeout leaves running
    /// while the game clock stands.
    fn svs_time_ms(&self) -> i32 {
        self.core.sv_time_ms.wrapping_add(self.game.paused_ms)
    }

    fn send_oob(&mut self, to: SocketAddr, text: &str) {
        self.core.outbox.push((to, build_oob(text)));
    }

    /// What a move message read off the socket at `arrived` acks against:
    /// the frame sent before the newest when it arrived ahead of that one,
    /// else `None`, the frame clock. Retail reads its socket every 5 ms and
    /// runs a frame in well under one, so a move message is stamped with the
    /// newest frame that had gone out when it arrived; vcod's tick takes long
    /// enough that one read at the next tick would stamp a packet that came
    /// in during the tick a frame late. docs/research/cod11-server-handshake.md,
    /// "Pings".
    fn ack_stamp(&self, arrived: Instant) -> Option<i32> {
        self.game
            .last_send
            .filter(|&(at, _)| arrived < at)
            .map(|(_, before)| before)
    }

    /// `SV_PacketEvent` with the move acks stamped `ack_time_ms` (`None`,
    /// the frame clock): the one entry under [`Self::handle_packet`] and
    /// [`Self::packet_read`]. `at.read` is when retail's event loop would
    /// have handled the packet: its arrival, or later when the server was
    /// busy with a frame or an earlier packet. rcon's window reads it, as
    /// retail's reads the clock at the handling (cod11-server-handshake.md,
    /// "rcon").
    pub fn handle_packet_acked(
        &mut self,
        from: SocketAddr,
        pkt: &[u8],
        at: crate::engine::Arrival,
        ack_time_ms: Option<i32>,
    ) {
        self.game.ack_time_ms = ack_time_ms;
        crate::engine::packet(self, from, pkt, at);
        self.game.ack_time_ms = None;
    }

    /// Marks the frame just ticked as sent at `at`: the outbox it left has
    /// gone onto the socket.
    pub fn frame_sent(&mut self, at: Instant) {
        self.game.last_send = Some((at, self.game.sent_time_ms));
        self.game.sent_time_ms = self.svs_time_ms();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) use vcod_common::collision::test_world;
    pub(super) use vcod_common::net::connectionless::{build_connect, build_oob, parse_oob};
    pub(super) use vcod_common::net::msg::write_delta_usercmd;
    pub(super) use vcod_common::net::netchan::Netchan;
    pub(super) use vcod_common::net::protocol::PROTOCOL_V6;
    pub(super) use vcod_common::net::protocol::PROTOCOL_V22;
    pub(super) use vcod_common::net::snapshot::{SVC_SNAPSHOT, SnapshotRing};

    pub(super) const QPORT: u16 = 0x2001;

    pub(super) fn cfg() -> ServerConfig {
        ServerConfig {
            map: "mp_carentan".into(),
            hostname: "vcod test".into(),
            max_clients: 4,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
            bots: 0,
            bots_shoot: false,
        }
    }

    pub(super) fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    /// Installs a hand-built runtime the way `load_scripts` does: the script
    /// owns the configstring table from there on, so it starts from the
    /// server's rather than from `for_test`'s empty one, and the server's
    /// copy is taken back through the cvar mirror. Without it the first tick
    /// reads every static slot as cleared and broadcasts the difference.
    pub(super) fn install_script(sv: &mut Server, mut rt: crate::game::script::ScriptRuntime) {
        for (i, cs) in sv.core.configstrings.iter().enumerate() {
            if rt.host.configstrings[i].is_empty() {
                rt.host.configstrings.set(i, cs.clone());
            }
        }
        let mut cs = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut cs)
            .expect("the cvar mirror fits");
        sv.core.configstrings = cs;
        sv.sync_sent_configstrings();
        sv.game.module.script = Some(rt);
    }

    pub(super) fn oob(cmd: &str) -> Vec<u8> {
        build_oob(cmd)
    }

    pub(super) fn reply(sv: &mut Server) -> (SocketAddr, String, Vec<u8>) {
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one packet");
        let (to, pkt) = out.remove(0);
        let (cmd, rest) = parse_oob(&pkt).expect("oob reply");
        (to, cmd.to_string(), rest.to_vec())
    }

    pub(super) fn reply_text(sv: &mut Server) -> (String, String) {
        let (_, cmd, rest) = reply(sv);
        (cmd, String::from_utf8_lossy(&rest).trim().to_string())
    }

    pub(super) fn challenge_for(sv: &mut Server, from: SocketAddr, now: Instant) -> i32 {
        sv.handle_packet(from, &oob("getchallenge"), now);
        let (cmd, body) = reply_text(sv);
        assert_eq!(cmd, "challengeResponse");
        body.parse().expect("a numeric challenge")
    }

    pub(super) fn connect_pkt(challenge: i32, qport: u16, protocol: u32) -> Vec<u8> {
        build_connect(&format!(
            "\\name\\vcod\\protocol\\{protocol}\\qport\\{qport}\\challenge\\{challenge}"
        ))
    }

    pub(super) fn connected(sv: &mut Server, from: SocketAddr, now: Instant) -> Netchan {
        let challenge = challenge_for(sv, from, now);
        let version = sv.game.proto.transport.version;
        sv.handle_packet(from, &connect_pkt(challenge, QPORT, version), now);
        assert_eq!(reply_text(sv).0, "connectResponse");
        Netchan::new(QPORT, challenge)
    }

    pub(super) fn server_commands(nc: &mut Netchan, pkt: &[u8], huff: &Huffman) -> Vec<String> {
        let msg = nc
            .process_in(pkt, huff)
            .expect("a netchan packet")
            .expect("a whole message");
        let mut r = MsgReader::new(&msg[4..], huff);
        let mut out = Vec::new();
        while !r.is_overflowed() {
            match r.read_byte() {
                msg::SVC_SERVER_COMMAND => {
                    r.read_long();
                    out.push(r.read_big_string());
                }
                _ => break,
            }
        }
        out
    }

    pub(super) fn v6_server(now: Instant) -> Server {
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V6);
        sv
    }

    /// clc_move ops carrying one usercmd, keyed the way the server decodes it.
    pub(super) fn move_ops(checksum_feed: i32, message_ack: i32, cmd: UserCmd) -> Vec<u8> {
        move_ops_cmds(checksum_feed, message_ack, &[cmd])
    }

    /// clc_move ops carrying several usercmds, chained as a real client
    /// chains them: each deltas from its predecessor.
    pub(super) fn move_ops_cmds(checksum_feed: i32, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
        let huff = Huffman::new();
        let key = checksum_feed ^ message_ack ^ com_hash_key("", 32);
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(cmds.len() as u8);
        let mut prev = NULL_USERCMD;
        for cmd in cmds {
            write_delta_usercmd(&mut w, key, &prev, cmd);
            prev = *cmd;
        }
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Move-message encoder that keeps the delta base across messages, the
    /// way the real client chains its sent cmds. A fresh null base per
    /// message would let a changed-then-released field encode compact
    /// against nothing.
    pub(super) struct MoveChain {
        checksum_feed: i32,
        prev: UserCmd,
    }

    impl MoveChain {
        pub(super) fn new(checksum_feed: i32) -> Self {
            MoveChain {
                checksum_feed,
                prev: NULL_USERCMD,
            }
        }

        pub(super) fn ops(&mut self, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
            let huff = Huffman::new();
            let key = self.checksum_feed ^ message_ack ^ com_hash_key("", 32);
            let mut w = MsgWriter::new(&huff);
            w.write_bits(CLC_MOVE, 2);
            w.write_byte(cmds.len() as u8);
            for cmd in cmds {
                write_delta_usercmd(&mut w, key, &self.prev, cmd);
                self.prev = *cmd;
            }
            w.write_bits(CLC_EOF, 2);
            w.into_ops()
        }
    }

    /// Ack-only ops: an empty command/move run still ends in clc_EOF, which is
    /// what `parse_client_ops` demands before it commits anything.
    pub(super) fn ack_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Connect, ask for the gamestate (a fresh client sends serverId 0),
    /// drain it, ack past it. Returns the live client netchan; slot 0 is
    /// Primed, and stays there until its first usercmd.
    pub(super) fn active(sv: &mut Server, now: Instant) -> Netchan {
        active_at(sv, addr(5), now)
    }

    /// [`active`] from any address.
    pub(super) fn active_at(sv: &mut Server, from: SocketAddr, now: Instant) -> Netchan {
        let huff = Huffman::new();
        let mut nc = connected(sv, from, now);
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(from, &pkt, now);
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff).unwrap();
        }
        // The gamestate's later fragments, which ticks would send one a turn.
        for c in sv.core.clients.iter_mut().flatten() {
            for pkt in c.link.unsent.drain(..) {
                let _ = nc.process_in(&pkt, &huff).unwrap();
            }
        }
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            from,
            &nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap(),
            now,
        );
        nc
    }

    /// `active` plus the first usercmd, which is what puts a client in the
    /// world, so slot 0 has a sim and is sent snapshots. A real client sends
    /// no command to get here.
    pub(super) fn begun(sv: &mut Server, now: Instant) -> Netchan {
        let huff = Huffman::new();
        let mut nc = active(sv, now);
        let ack = nc.incoming_sequence as i32;
        let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
        let pkt = nc
            .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        nc
    }

    /// Tick once and parse the newest queued frame as a client would.
    pub(super) fn latest_snapshot(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
    ) -> vcod_common::net::snapshot::Snapshot {
        let p = sv.game.proto;
        latest_snapshot_as(sv, nc, ring, now, p)
    }

    /// [`latest_snapshot`] decoded through `p`.
    pub(super) fn latest_snapshot_as(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
        p: &Cod1Wire,
    ) -> vcod_common::net::snapshot::Snapshot {
        let huff = Huffman::new();
        sv.tick(now);
        let mut snap = None;
        for (_, pkt) in sv.take_outgoing() {
            if let Ok(Some(msgbytes)) = nc.process_in(&pkt, &huff) {
                let num = nc.incoming_sequence;
                let mut r = MsgReader::new(&msgbytes[4..], &huff);
                while !r.is_overflowed() {
                    match r.read_byte() {
                        SVC_SNAPSHOT => {
                            snap = Some(ring.parse_into(&mut r, p, num).unwrap().clone());
                            break;
                        }
                        // Unacked commands ride every message until acked.
                        msg::SVC_SERVER_COMMAND => {
                            r.read_long();
                            r.read_big_string();
                        }
                        _ => {}
                    }
                }
            }
        }
        snap.expect("a snapshot arrived")
    }

    /// Protocol 6 with `pm_flags` left in 1.5's layout, for asserting the
    /// wire bits.
    pub(super) static RAW_V6: Cod1Wire = Cod1Wire {
        pm_flags_to_v1: |f| f,
        ..PROTOCOL_V6
    };

    pub(super) fn rcon(sv: &mut Server, line: &str, now: Instant) -> Vec<String> {
        sv.handle_packet(addr(9), &oob(&format!("rcon {line}")), now);
        sv.take_outgoing()
            .into_iter()
            .map(|(to, pkt)| {
                assert_eq!(to, addr(9));
                String::from_utf8_lossy(&pkt[4..]).into_owned()
            })
            .collect()
    }

    pub(super) fn local_master(_: &str) -> Option<SocketAddr> {
        Some(addr(29463))
    }
}
