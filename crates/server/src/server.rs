//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The server state machine, transport-free. `main.rs` owns the socket, the
//! tests own a queue. Ported from RTCW-MP sv_main.c / sv_client.c; reply
//! strings come from docs/research/cod11-server-handshake.md.

use crate::client::{sanitize_name, Client, ClientState};
use crate::configstrings;
use crate::console;
use crate::game::host::{ClientEvent, SpawnMode};
use crate::game::script;
use crate::game::temp_entity;
use crate::spectate::ClientSim;
use crate::world::{TestEntities, World};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::connectionless::{build_oob, parse_connect, parse_oob, Info};
use vcod_common::net::gamestate::{self, Gamestate};
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, read_delta_usercmd, MsgReader, MsgWriter, UserCmd, NULL_USERCMD,
};
use vcod_common::net::netchan::{ClientMessage, ServerNetchan, MAX_RELIABLE_COMMANDS};
use vcod_common::net::protocol::{Protocol, PROTOCOL_V1};
use vcod_common::net::{com_hash_key, info_value_for_key, snapshot};
use vcod_common::pmove::MAX_FRAME_MS;

/// `MAX_CHALLENGES`, server.h:198.
const MAX_CHALLENGES: usize = 1024;
/// A challenge nobody asked about for this long is dropped when a new one
/// is issued, so a spoof flood has to keep up with real clients to evict them.
const CHALLENGE_TTL: Duration = Duration::from_secs(60);
/// `sv_timeout` default (cod_lnxded 0x80d56c0).
const TIMEOUT: Duration = Duration::from_secs(240);
/// `sv_reconnectlimit` default. Also how long a slot must have been silent
/// before a connect carrying a different challenge may reclaim it.
const RECONNECT_LIMIT: Duration = Duration::from_secs(3);
/// ioq3 `SVC_RateLimitAddress`: per source ip, a burst of 10 connectionless
/// requests, one back per second.
const ADDR_BURST: u32 = 10;
const ADDR_PERIOD: Duration = Duration::from_secs(1);
/// ioq3 `outboundLeakyBucket`: all connectionless replies, 10 per 100 ms.
const GLOBAL_BURST: u32 = 10;
const GLOBAL_PERIOD: Duration = Duration::from_millis(100);
/// Source ips tracked at once; the least recently seen is evicted.
const MAX_BUCKETS: usize = 1024;
/// clc ops, 2 bits on the wire.
const CLC_MOVE: i32 = 0;
const CLC_MOVE_NO_DELTA: i32 = 1;
const CLC_CLIENT_COMMAND: i32 = 2;
const CLC_EOF: i32 = 3;
const MAX_PACKET_USERCMDS: u8 = 32;
/// Queued-but-unreplayed usercmds per client; past this a flood drops the
/// oldest rather than building latency.
const MAX_PENDING_CMDS: usize = 64;
/// pmove steps per snapshot tick; a flood beyond this fast-forwards.
const MAX_CMDS_PER_TICK: usize = 32;
/// `Pmove`'s catch-up (`game.mp.i386.so` 0x34492): a client further in arrears
/// than this has the excess dropped rather than simulated, which also bounds
/// the chop loop to 16 steps per cmd. `docs/protocol-1.1.md`, "How long a cmd
/// is simulated for", has the rest of retail's rule, including the two clamps
/// vcod does not apply.
const MAX_PMOVE_ARREARS_MS: i32 = 1000;
/// `SV_ClientCommand`'s flood window (`cod_lnxded` 0x8086f5f, `add eax,0x320`):
/// a non-exempt client command opens 800 ms during which every further
/// non-exempt one from an active client is dropped before the game sees it
/// (`docs/protocol-1.1.md`, "Client commands are flood-protected").
const FLOOD_WINDOW_MS: i32 = 800;
/// How often a bot re-runs its enemy search (range gate + LOS traces).
/// The cached verdict is at most this stale.
const ENEMY_REFRESH_MS: i32 = 100;
/// The tick pace (`1000 / sv_fps`), also the dt floor a fresh sim starts from.
const FRAME_MS: i32 = 50;
/// Retail's `MAX_CLIENTS`. Client slots index a 6-bit wire field
/// (clientState entries; `ps.clientNum` gets 8), so more than 64 would
/// collide silently.
pub(crate) const MAX_CLIENTS: usize = 64;

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
    /// Debug bots in play. Each takes a real slot; 0 is off.
    pub bots: usize,
    /// Whether the bots fight. Without it they only wander.
    pub bots_shoot: bool,
}

/// `challenge_t`. Entries past `CHALLENGE_TTL` go on the next insert; the
/// oldest slot is recycled when the table is still full.
struct Challenge {
    addr: SocketAddr,
    challenge: i32,
    time: Instant,
    connected: bool,
}

/// ioq3 `leakyBucket_t`: `used` tokens, one drained per period.
struct Bucket {
    last: Instant,
    used: u32,
}

impl Bucket {
    /// `SVC_RateLimit`. True when this request is over the limit.
    fn limited(&mut self, now: Instant, burst: u32, period: Duration) -> bool {
        let interval = now.saturating_duration_since(self.last);
        let expired = interval.as_micros() / period.as_micros();
        if expired > u128::from(self.used) {
            self.used = 0;
            self.last = now;
        } else {
            self.used -= expired as u32;
            self.last += period * expired as u32;
        }
        if self.used < burst {
            self.used += 1;
            return false;
        }
        true
    }
}

/// The connectionless reply limiter, so `getstatus` cannot be used as a
/// reflector: a bucket per source ip in front of one for all replies.
struct RateLimiter {
    global: Bucket,
    addrs: HashMap<IpAddr, Bucket>,
}

impl RateLimiter {
    fn new(now: Instant) -> Self {
        RateLimiter {
            global: Bucket { last: now, used: 0 },
            addrs: HashMap::new(),
        }
    }

    /// `SVC_RateLimitAddress`. The global bucket is untouched by a request
    /// this rejects.
    fn addr_limited(&mut self, from: IpAddr, now: Instant) -> bool {
        if !self.addrs.contains_key(&from) && self.addrs.len() >= MAX_BUCKETS {
            let oldest = *self.addrs.iter().min_by_key(|(_, b)| b.last).unwrap().0;
            self.addrs.remove(&oldest);
        }
        self.addrs
            .entry(from)
            .or_insert(Bucket { last: now, used: 0 })
            .limited(now, ADDR_BURST, ADDR_PERIOD)
    }

    /// Both buckets, in ioq3's order; true when the reply must not go out.
    fn reply_limited(&mut self, from: IpAddr, now: Instant) -> bool {
        self.addr_limited(from, now) || self.global.limited(now, GLOBAL_BURST, GLOBAL_PERIOD)
    }
}

/// One op of a client message, read out before any of it is applied.
enum ClientOp {
    Command {
        seq: i32,
        text: String,
    },
    /// Every usercmd of a move block, in wire order.
    Move(Vec<UserCmd>),
}

/// A client command whose whole effect is to start or release a script
/// thread. `client_command` parses it out of the packet and `tick` runs it,
/// so it lands on the frame the rest of the script frame runs on.
enum ScriptCommand {
    Kill,
    MenuResponse(i32, String),
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
}

/// One attack a client's weapon step took this tick. The weapon index each
/// arm carries is `ps.weapon` at the event, not whatever is in hand by the
/// time the trace runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Attack {
    Shot(Shot),
    /// `EV_FIRE_MELEE`, the swing's damage frame (combat doc, 2.5).
    Swing {
        slot: usize,
        weapon: u8,
    },
    /// `EV_FIRE_WEAPON` from a grenade: the fuse left rides the event parm
    /// (combat doc, 1.11).
    Throw {
        slot: usize,
        weapon: u8,
        fuse_left_ms: i32,
    },
}

/// One `WeaponOp` against a client's playerstate. The op is an edge the
/// script made, so it is applied once, where `client_weapons` is mirrored
/// every frame.
pub(crate) fn apply_weapon_op(
    sim: &mut crate::spectate::ClientSim,
    op: crate::game::host::WeaponOp,
    weapons: &crate::weapons::WeaponTable,
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
        // Through the putaway, so the drop and the raise both run: the
        // machine picks the target up when the drop ends.
        WeaponOp::SwitchTo(index) => {
            let Some(def) = weapons.get(ps.weapon as usize) else {
                return;
            };
            let mut events = Vec::new();
            vcod_common::pmove::weapon::begin_switch(ps, def, index, &mut events);
            for e in events {
                sim.add_event(e.event, e.parm);
            }
        }
    }
}

/// What one client's usercmd replay did this tick, for the trace line.
#[derive(Default, Clone, Copy)]
struct MoveSummary {
    processed: usize,
    first_cmd_st: Option<i32>,
    last_cmd_st: Option<i32>,
    /// The union of every replayed cmd's `buttons`, for the script's button
    /// builtins: retail latches each cmd in `ClientThink` (0x41540), so a tap
    /// inside a multi-cmd packet must not be lost to the packet's last cmd.
    buttons: u8,
}

/// Why a level load failed, which is what says whether the server can go on.
#[derive(Debug)]
pub enum LoadFailure {
    /// Refused before anything was torn down, so the level that is serving is
    /// untouched and the console line was a no-op.
    KeptLevel(anyhow::Error),
    /// Failed with the level already gone: no script to call `exitLevel`, no
    /// table for a client to pull. Retail ends the process on this
    /// (`Com_Error`) and so does vcod.
    Fatal(anyhow::Error),
}

pub struct Server {
    cfg: ServerConfig,
    huff: Huffman,
    proto: &'static Protocol,
    /// `sv_serverid`, `0x10` per map load plus the restart count in the low
    /// nibble. u8 because the client echoes it in a one-byte header field.
    server_id: u8,
    checksum_feed: i32,
    configstrings: Vec<String>,
    /// The table as the clients last heard it, which is what a mid-level
    /// change is diffed against ([`Server::broadcast_configstring_changes`]).
    /// Re-synced wherever a client is handed the whole table again.
    sent_configstrings: Vec<String>,
    challenges: Vec<Challenge>,
    clients: Vec<Option<Client>>,
    outbox: Vec<(SocketAddr, Vec<u8>)>,
    limiter: RateLimiter,
    rng: u64,
    /// The map's collision and spawn, loaded by the binary; tests run
    /// without. `Rc` so `GameHost.world` can point at the same map for
    /// `bulletTrace`, which needs real geometry to trace against.
    world: Option<Rc<World>>,
    /// `svs.time`, advanced one frame per tick.
    sv_time_ms: i32,
    /// Gamestate entity baselines a delta frame may omit an unchanged entity
    /// against; empty when `test_entities` is off.
    baselines: HashMap<u32, msg::EntityState>,
    /// Scripted entities driving the packet-entity path; `None` when
    /// `cfg.test_entities` is 0.
    test_entities: Option<TestEntities>,
    /// Where in the temp-entity block the next frame's events start. It
    /// rolls rather than resetting, so an event repeated in adjacent frames
    /// never lands on one number twice (`crate::game::temp_entity`).
    temp_cursor: u32,
    /// Wall clock of the previous tick, for the trace's send-interval column.
    last_tick: Option<Instant>,
    /// The map script; `None` until `load_scripts` succeeds. `tick` steps it
    /// once per frame.
    script: Option<crate::game::script::ScriptRuntime>,
    /// The player animtree, its wire index and the animscript. `None` on a
    /// host with no paks, where every client keeps index 0.
    anims: Option<vcod_common::animtree::PlayerAnims>,
    /// Every weapon file, parsed once at load: the animscript tests
    /// `weaponClass` every frame and the frame loop must not read a pk3.
    /// `Rc` so a snapshot/move closure can hold it without borrowing `self`.
    weapon_table: Rc<crate::weapons::WeaponTable>,
    /// Attacks this tick's moves took, in the order they happened. Filled by
    /// `replay_moves`, drained by `tick` into traces before the script runs.
    pending_attacks: Vec<Attack>,
    /// The blasts this frame's missile pass set off, for the radius damage
    /// pass to charge (`crate::game::missile::Explosion`).
    pending_explosions: Vec<crate::game::missile::Explosion>,
    /// Retail's `+set name value`: applied last in `cvars`, over
    /// `default_mp.cfg` and the config's own, so a run can turn a script
    /// cvar such as `scr_friendlyfire` on without a code change.
    cvar_overrides: Vec<(String, String)>,
    /// The client commands that start a script thread, in arrival order.
    /// `client_command` runs during `handle_packet`, a frame before `tick`
    /// advances `sv_time_ms`, so a thread started there would run on the
    /// previous frame's clock and every `cloneplayer` and `wait` in it would
    /// land a frame in the past. These queue instead and are drained in the
    /// tick's script slot.
    pending_script_commands: Vec<(usize, ScriptCommand)>,
    /// The hit-location damage multipliers out of the paks
    /// (`crate::game::combat::HitLocTable`); the default until `load_scripts`.
    hitlocs: crate::game::combat::HitLocTable,
    /// The paks, kept past `load_scripts` because a shot loads the victim's
    /// models to trace against. `None` on a host with none.
    fs: Option<Rc<vcod_common::pk3::Pk3Fs>>,
    /// The grafted player skeletons those shots trace against, built on first
    /// use (`crate::game::hitrig`).
    hit_rigs: crate::game::hitrig::HitRigs,
    /// Weapons the moves themselves switched to, by slot: only a change the
    /// machine made, so a playerstate reset from outside a move is not one.
    /// `tick` writes each back onto the script host before the mirror reads
    /// it, which is what keeps a switch from being undone the frame after it
    /// lands.
    weapon_changes: Vec<(usize, u8)>,
    /// Weapons a move spent the last round of, by slot: a `clipOnly` weapon
    /// with no reserve left is taken away (combat doc, 1.5 step 9), which
    /// pmove cannot do because the script host owns `ps.weapons`.
    weapon_takes: Vec<(usize, u8)>,
    /// Commands waiting to run, drained by `drain_console` at the top of
    /// `tick` (`Cbuf_Execute`, docs/research/cod11-map-cycle.md section 5.2).
    console: console::Console,
    /// `sv_mapRotationCurrent`, consumed one token per `map_rotate`.
    rotation: console::Rotation,
    /// `sv_mapRotation`, mirrored here by `set_cvar` (doc section 5).
    sv_map_rotation: String,
    /// `svs.snapFlagServerBit`, toggled by a map load or restart and sent as
    /// every snapshot's `snapFlags` (doc section 3 step 13, section 4
    /// step 4).
    snap_flag_server_bit: u32,
    /// The cvar table the outgoing level left, stashed when its script is
    /// dropped. Retail's `Cvar_Set` writes a process-global table that no
    /// level boundary clears, so a script's `setCvar` outlives both a
    /// restart and a map change; rebuilding from `default_mp.cfg` alone
    /// would lose it.
    carried_cvars: Option<crate::cvars::Cvars>,
    /// `g_gametype` and `sv_maxclients` as the running level actually loaded
    /// with, read off the table `load_scripts_with` built. Doc section 4
    /// step 3 compares retail's latched value against its live one; this is
    /// the same comparison's left-hand side, and it has to be the built
    /// value rather than `cfg`, since a `+set` override outranks `cfg` in
    /// [`Self::cvars`] and comparing against `cfg` escalates every restart
    /// for the whole run.
    level_cvars: Option<(String, String)>,
    /// `svs.time` of the last spawn or restart, for the same-frame guard
    /// (doc section 4 step 1).
    last_spawn_tick: Option<i32>,
    /// One `.gsc` answered from memory instead of the paks
    /// ([`Self::overlay_script`]); `None` in every production run.
    script_overlay: Option<(String, String)>,
    /// A level load that failed with the level already torn down
    /// ([`LoadFailure::Fatal`]), waiting for `main` to end the process on it.
    fatal: Option<anyhow::Error>,
    /// The debug bots, by slot. An entry whose client is gone goes with it.
    bots: BTreeMap<usize, crate::bots::Bot>,
    /// `--bots` spawns on the first tick with scripts up, once; a map change
    /// finds the bots already in their slots and rejoins them.
    bots_spawned: bool,
    /// Per bot, `(sv_time of the last look, the enemy that look found)`.
    /// A fresh LOS trace per bot per tick was the other half of the 24-bot
    /// CPU load; a bot does not need a new verdict every 50 ms.
    bot_enemies: BTreeMap<usize, (i32, Option<crate::bots::EnemyView>)>,
}

/// OOB argument text, minus a trailing line terminator.
fn oob_arg(rest: &[u8]) -> String {
    String::from_utf8_lossy(rest)
        .trim_end_matches(['\n', '\0'])
        .to_string()
}

/// The last script menu `Cmd_MenuResponse_f` (0x486d8) will look up, and the
/// last `GScr_GetScriptMenuIndex` (0x5c73c) will hand out: both walk
/// `CsRange::Menu`'s 32 slots.
const MAX_MENUS: i32 = 31;

/// `mr <serverId> <menuIndex> <response>`, exactly four arguments, the
/// index a slot in `CsRange::Menu`. The response passes through unparsed: it
/// is a string the gametype compares, not something the server reads.
///
/// Retail is looser than this in three places, none of which a stock
/// gametype reaches. Two are in `Cmd_MenuResponse_f` (0x486d8), INFERRED
/// from the disassembly rather than run live: a wrong argument count gets a
/// `("", "bad")` notify without the serverId even being read, and an index
/// past 31 gets argv[2]'s own digits in place of the menu name. Both produce
/// a menu name no `menuresponse` loop compares equal to, so dropping them
/// costs nothing a script can see. The third is the tokenizer: retail's
/// `Cmd_Argv` strips quotes, so `mr 7 3 "allies"` is a valid response there
/// and is rejected here. Nothing in the stock corpus quotes a menu response,
/// and unquoting without a measurement of what else that tokenizer does to
/// an argument would be inventing a format. The stale-serverId drop is the
/// one retail shares.
fn parse_menu_response(cmd: &str, server_id: i32) -> Option<(i32, String)> {
    let mut it = cmd.split_whitespace();
    if it.next()? != "mr" {
        return None;
    }
    let sid: i32 = it.next()?.parse().ok()?;
    let index: i32 = it.next()?.parse().ok()?;
    let response = it.next()?.to_string();
    if it.next().is_some() || sid != server_id || !(0..=MAX_MENUS).contains(&index) {
        return None;
    }
    Some((index, response))
}

/// `Cmd_Argv(1)`. The token goes back out inside an info string, so the info
/// separators come off it and the length is capped.
fn challenge_arg(arg: &str) -> String {
    const MAX: usize = 32;
    let mut out = String::with_capacity(MAX);
    for c in arg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| *c != '\\' && *c != '"')
    {
        if out.len() + c.len_utf8() > MAX {
            break;
        }
        out.push(c);
    }
    out
}

/// `SV_UpdateServerCommandsToClient`. The caller bounds `from_ack` to
/// `0..=reliable_sequence`, so the range is empty or inside the ring.
fn write_pending_commands(w: &mut MsgWriter, nc: &ServerNetchan, from_ack: i32) {
    for seq in (from_ack + 1)..=(nc.reliable_sequence as i32) {
        msg::write_server_command(
            w,
            seq,
            &nc.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)],
        );
    }
}

impl Server {
    pub fn new(mut cfg: ServerConfig, now: Instant) -> Self {
        cfg.max_clients = cfg.max_clients.min(MAX_CLIENTS);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64)
            | 1;
        let server_id = 0x10;
        let mut sv = Server {
            configstrings: configstrings::static_configstrings(&cfg, server_id),
            sent_configstrings: configstrings::static_configstrings(&cfg, server_id),
            clients: (0..cfg.max_clients).map(|_| None).collect(),
            cfg,
            huff: Huffman::new(),
            proto: &PROTOCOL_V1,
            server_id,
            checksum_feed: 0,
            challenges: Vec::new(),
            outbox: Vec::new(),
            limiter: RateLimiter::new(now),
            rng: seed,
            world: None,
            sv_time_ms: 0,
            baselines: HashMap::new(),
            test_entities: None,
            temp_cursor: 0,
            last_tick: None,
            script: None,
            anims: None,
            weapon_table: Rc::new(crate::weapons::WeaponTable::empty()),
            pending_attacks: Vec::new(),
            pending_explosions: Vec::new(),
            cvar_overrides: Vec::new(),
            pending_script_commands: Vec::new(),
            hitlocs: crate::game::combat::HitLocTable::default(),
            fs: None,
            hit_rigs: Default::default(),
            weapon_changes: Vec::new(),
            weapon_takes: Vec::new(),
            console: console::Console::new(),
            rotation: console::Rotation::default(),
            sv_map_rotation: String::new(),
            snap_flag_server_bit: 0,
            carried_cvars: None,
            level_cvars: None,
            last_spawn_tick: None,
            script_overlay: None,
            fatal: None,
            bots: BTreeMap::new(),
            bots_spawned: false,
            bot_enemies: BTreeMap::new(),
        };
        // `(rand() << 16) ^ rand() ^ Sys_Milliseconds()`, SV_SpawnServer 0x808a3e0.
        sv.checksum_feed = (sv.rand() << 16) ^ sv.rand() ^ (now.elapsed().as_millis() as i32);
        if sv.cfg.test_entities > 0 {
            let te = TestEntities::new(sv.cfg.test_entities, [0.0, 0.0, 64.0]);
            sv.baselines = te.baselines(sv.proto);
            sv.test_entities = Some(te);
        }
        sv
    }

    /// xorshift64*, masked to 31 bits like glibc's `rand()` so that
    /// `(rand() << 16) ^ rand()` wraps into the sign bit and challenges come
    /// out signed like retail's.
    fn rand(&mut self) -> i32 {
        (vcod_common::rng::xorshift(&mut self.rng) >> 33) as i32 & 0x7fff_ffff
    }

    pub fn configstring(&self, i: usize) -> &str {
        self.configstrings.get(i).map_or("", String::as_str)
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().flatten().count()
    }

    /// `sv_serverid`. The high nibble is the map load, the low one the
    /// restart count (docs/research/cod11-map-cycle.md, section 3 step 16).
    pub fn server_id(&self) -> u8 {
        self.server_id
    }

    pub fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.outbox)
    }

    fn send_oob(&mut self, to: SocketAddr, text: &str) {
        self.outbox.push((to, build_oob(text)));
    }

    /// `SV_PacketEvent`.
    pub fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        match parse_oob(pkt) {
            Some((cmd, rest)) => {
                let cmd = cmd.to_string();
                self.handle_oob(from, &cmd, rest, pkt, now);
            }
            None => self.handle_client_packet(from, pkt, now),
        }
    }

    /// `connect` re-parses `raw` because its body is compressed; only the
    /// browser queries read `rest` as text. The queries are rate limited
    /// before anything is looked at; a limited request is dropped silently.
    fn handle_oob(&mut self, from: SocketAddr, cmd: &str, rest: &[u8], raw: &[u8], now: Instant) {
        match cmd {
            "getinfo" | "getstatus" | "getchallenge" => {
                if self.limiter.reply_limited(from.ip(), now) {
                    log::debug!("{from}: {cmd} rate limited");
                    return;
                }
                match cmd {
                    "getinfo" => self.svc_info(from, &oob_arg(rest)),
                    "getstatus" => self.svc_status(from, &oob_arg(rest)),
                    _ => self.svc_get_challenge(from, now),
                }
            }
            "connect" => self.svc_direct_connect(from, raw, now),
            // Retail drops by source address alone; the bucket keeps a spoofer
            // from cycling a slot faster than the client can reconnect.
            "disconnect" => {
                if self.limiter.addr_limited(from.ip(), now) {
                    log::debug!("{from}: disconnect rate limited");
                    return;
                }
                self.oob_disconnect(from)
            }
            other => log::debug!("{from}: unhandled oob {other:?}"),
        }
    }

    /// `SVC_Info` (cod_lnxded 0x808c1ac). Key order is retail's;
    /// `minPing`/`maxPing`/`game` only appear when the matching cvar is set.
    fn svc_info(&mut self, from: SocketAddr, challenge: &str) {
        let mut i = Info::new();
        i.set("challenge", challenge_arg(challenge))
            .set("protocol", PROTOCOL_V1.version)
            .set("hostname", &self.cfg.hostname)
            .set("mapname", &self.cfg.map)
            .set("clients", self.client_count())
            .set("sv_maxclients", self.cfg.max_clients)
            .set("gametype", &self.cfg.gametype)
            .set("pure", 0)
            .set("sv_allowAnonymous", 0)
            .set("pswrd", 0);
        self.send_oob(from, &format!("infoResponse\n{i}"));
    }

    /// `SVC_Status` (0x808bd50).
    fn svc_status(&mut self, from: SocketAddr, challenge: &str) {
        let mut i = configstrings::serverinfo(&self.cfg);
        i.set("challenge", challenge_arg(challenge)).set("pswrd", 0);
        let mut lines = String::new();
        for c in self.clients.iter().flatten() {
            lines.push_str(&format!("0 0 \"{}\"\n", c.name));
        }
        self.send_oob(from, &format!("statusResponse\n{i}\n{lines}"));
    }

    /// `SV_GetChallenge` without the authorize-server detour. A client still
    /// asking keeps its entry fresh; stale entries go before a new insert.
    fn svc_get_challenge(&mut self, from: SocketAddr, now: Instant) {
        let challenge = match self
            .challenges
            .iter_mut()
            .find(|c| !c.connected && c.addr == from)
        {
            Some(c) => {
                c.time = now;
                c.challenge
            }
            None => {
                self.challenges
                    .retain(|c| now.duration_since(c.time) < CHALLENGE_TTL);
                let challenge = (self.rand() << 16) ^ self.rand();
                let entry = Challenge {
                    addr: from,
                    challenge,
                    time: now,
                    connected: false,
                };
                if self.challenges.len() < MAX_CHALLENGES {
                    self.challenges.push(entry);
                } else {
                    let oldest = (0..self.challenges.len())
                        .min_by_key(|&i| self.challenges[i].time)
                        .unwrap();
                    self.challenges[oldest] = entry;
                }
                challenge
            }
        };
        self.send_oob(from, &format!("challengeResponse {challenge}"));
    }

    /// `SV_DirectConnect` (sv_client.c:252) with CoD's rejection strings.
    fn svc_direct_connect(&mut self, from: SocketAddr, raw: &[u8], now: Instant) {
        let userinfo = match parse_connect(raw) {
            Ok(u) => u,
            Err(e) => {
                log::debug!("{from}: bad connect: {e}");
                return;
            }
        };
        let val = |k: &str| info_value_for_key(&userinfo, k).and_then(|v| v.parse::<i32>().ok());
        if val("protocol") != Some(self.proto.version as i32) {
            self.send_oob(from, "error\nEXE_SERVER_IS_DIFFERENT_VER 1.1");
            return;
        }
        let (Some(challenge), Some(qport)) = (val("challenge"), val("qport")) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        let qport = qport as u16;
        let same_peer = |c: &Client| {
            c.addr.ip() == from.ip() && (c.netchan.qport == qport || c.addr.port() == from.port())
        };
        if self
            .clients
            .iter()
            .flatten()
            .any(|c| same_peer(c) && now.duration_since(c.last_connect) < RECONNECT_LIMIT)
        {
            log::debug!("{from}: reconnect rejected, too soon");
            return;
        }
        // By ip alone, as retail does: the port may have moved behind a NAT.
        let Some(ci) = self
            .challenges
            .iter()
            .position(|c| c.addr.ip() == from.ip() && c.challenge == challenge)
        else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        self.challenges[ci].connected = true;
        let slot = match self
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer))
        {
            Some(i) => {
                // Retail hands the slot to any challenge issued to this ip, so
                // a neighbour behind the same NAT who knows the qport can take
                // over a live player. Only the slot's own challenge (the
                // client's connect retry) may replace a client still heard
                // from; a silent slot (crash, lost disconnect) is reclaimable.
                let c = self.clients[i].as_ref().unwrap();
                if c.netchan.challenge != challenge
                    && now.duration_since(c.last_packet) < RECONNECT_LIMIT
                {
                    log::info!(
                        "{from}: connect with a foreign challenge refused, client {i} is live"
                    );
                    return;
                }
                log::info!("{from}: reconnect");
                // Whoever held the slot is gone, so its script state has to
                // be torn down here: `check_timeouts` never reaches it once
                // the new `Client` overwrites the slot with a fresh
                // `last_packet`.
                if let Some(rt) = self.script.as_mut() {
                    rt.push_client_event(ClientEvent::Disconnect(i));
                }
                i
            }
            None => match self.clients.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    self.send_oob(from, "error\nEXE_SERVERISFULL");
                    return;
                }
            },
        };
        let client = Client::new(from, qport, challenge, userinfo, now);
        log::info!("client {slot} {:?} connected from {from}", client.name);
        let name = client.name.clone();
        self.clients[slot] = Some(client);
        if let Some(rt) = self.script.as_mut() {
            rt.push_client_event(ClientEvent::Connect { slot, name });
        }
        self.send_oob(from, "connectResponse");
    }

    /// OOB `disconnect` (0x808c827).
    fn oob_disconnect(&mut self, from: SocketAddr) {
        if let Some(slot) = self
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.addr == from))
        {
            self.drop_client(slot, "EXE_DISCONNECTED");
        }
    }

    /// The netchan half of `SV_PacketEvent`, then `SV_ExecuteClientMessage`.
    /// The slot changes only once the message has passed every check the
    /// plain header and the op stream allow (`Client::accept`), so a packet
    /// forged with the client's ip and qport cannot stall it behind a huge
    /// sequence, redirect its replies or keep a dead slot alive.
    fn handle_client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        if pkt.len() < 6 {
            log::debug!("{from}: netchan packet too short ({} bytes)", pkt.len());
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let Some(slot) = self.clients.iter().position(|c| {
            c.as_ref()
                .is_some_and(|c| c.addr.ip() == from.ip() && c.netchan.qport == qport)
        }) else {
            log::debug!("{from}: netchan packet from no client (qport {qport})");
            return;
        };
        let Some(c) = self.clients[slot].as_ref() else {
            return;
        };
        let Some(m) = c.netchan.process_in(pkt) else {
            return;
        };
        // messageAcknowledge names a message we sent, so it is below
        // outgoing_sequence; retail only rejects a negative one.
        if m.message_ack < 0 || m.message_ack >= c.netchan.outgoing_sequence as i32 {
            log::debug!(
                "client {slot}: messageAcknowledge {} out of range",
                m.message_ack
            );
            return;
        }
        // 0x808ca1a drops an ack too far behind. One ahead of what we sent is
        // bogus too; `write_pending_commands` walks `reliable_ack + 1 ..=
        // reliable_sequence`, so an unbounded value overflows or loops for ages.
        let reliable_seq = c.netchan.reliable_sequence as i32;
        if m.reliable_ack < 0
            || m.reliable_ack > reliable_seq
            || reliable_seq.saturating_sub(m.reliable_ack) > MAX_RELIABLE_COMMANDS as i32 - 1
        {
            log::debug!(
                "client {slot}: reliableAcknowledge {} out of range",
                m.reliable_ack
            );
            return;
        }

        if m.server_id != self.server_id {
            let c = self.clients[slot].as_mut().unwrap();
            if m.server_id & 0xf0 != self.server_id & 0xf0 {
                // A map change from the client's view (a fresh client, serverId
                // 0, looks the same). Resend the gamestate once its ack is past
                // the last one. Until snapshots exist the gamestate is our only
                // message, so a lost one never advances the ack and the client
                // falls to its own timeout.
                if i64::from(m.message_ack) > c.gamestate_message_num {
                    c.accept(&m, now);
                    self.send_gamestate(slot);
                }
            } else if c.state == ClientState::Primed {
                // Restart path; retail promotes to CS_ACTIVE here. No
                // usercmd came with it, so entry has no entering cmd.
                c.accept(&m, now);
                self.enter_world(slot, None);
            }
            return;
        }

        let base = c.last_cmd;
        let Some((ops, last_cmd)) = self.parse_client_ops(c, &m, base) else {
            log::debug!("client {slot}: message from {from} does not decode");
            return;
        };
        // The new delta base commits before any op runs; a message that fails
        // to decode left it alone above.
        self.clients[slot].as_mut().unwrap().last_cmd = last_cmd;
        let c = self.clients[slot].as_mut().unwrap();
        c.accept(&m, now);
        c.addr = from; // NAT may move the port; the qport is the identity
        for op in ops {
            match op {
                ClientOp::Command { seq, text } => {
                    if !self.client_command(slot, seq, text) {
                        return;
                    }
                }
                ClientOp::Move(last) => self.user_move(slot, last),
            }
        }
    }

    /// `SV_ExecuteClientMessage`'s op walk, read to the end before any op is
    /// applied. `None` when the stream does not decode: an overflow anywhere
    /// or a bad usercmd count, which is what a forged block looks like once
    /// the scramble key is wrong. On success the new delta base comes back
    /// with the ops: the last cmd decoded, for the next move message to
    /// chain from.
    fn parse_client_ops(
        &self,
        c: &Client,
        m: &ClientMessage,
        base: UserCmd,
    ) -> Option<(Vec<ClientOp>, UserCmd)> {
        let mut r = MsgReader::new(&m.ops, &self.huff);
        let mut ops = Vec::new();
        let mut prev = base;
        loop {
            let op = r.read_bits(2);
            if r.is_overflowed() {
                return None;
            }
            match op {
                CLC_CLIENT_COMMAND => {
                    let seq = r.read_long();
                    let text = r.read_string();
                    if r.is_overflowed() {
                        return None;
                    }
                    ops.push(ClientOp::Command { seq, text });
                }
                CLC_EOF => return Some((ops, prev)),
                CLC_MOVE | CLC_MOVE_NO_DELTA => {
                    let cmds = self.parse_move(c, &mut r, m, &mut prev)?;
                    ops.push(ClientOp::Move(cmds));
                    return Some((ops, prev));
                }
                // `read_bits(2)` yields 0..=3; unreachable.
                _ => return None,
            }
        }
    }

    /// `SV_UserMove`'s parse: the whole block, in order, so the sim can
    /// replay every cmd like retail's pmove does. `prev` enters as the
    /// client's stored delta base and leaves as the last cmd of the block.
    fn parse_move(
        &self,
        c: &Client,
        r: &mut MsgReader,
        m: &ClientMessage,
        prev: &mut UserCmd,
    ) -> Option<Vec<UserCmd>> {
        let count = r.read_byte();
        if !(1..=MAX_PACKET_USERCMDS).contains(&count) {
            return None;
        }
        let cmd = &c.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)];
        let key = self.checksum_feed ^ m.message_ack ^ com_hash_key(cmd, 32);
        let mut out = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match read_delta_usercmd(r, key, prev) {
                Ok(next) => {
                    out.push(next);
                    *prev = next;
                }
                Err(e) => {
                    log::debug!("usercmd not parsed: {e}");
                    return None;
                }
            }
        }
        Some(out)
    }

    /// `SV_ClientCommand`. Returns false when the client is gone.
    fn client_command(&mut self, slot: usize, seq: i32, s: String) -> bool {
        let Some(c) = self.clients[slot].as_mut() else {
            return false;
        };
        if seq <= c.last_client_command {
            return true;
        }
        if seq > c.last_client_command.saturating_add(1) {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return false;
        }
        // Split rather than slice; the command may arrive with leading whitespace.
        let trimmed = s.trim_start();
        let (word, args) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        // `sv_floodProtect`, which the systeminfo advertises as 1. The three
        // exemptions are prefix compares with the space, so a bare `score`
        // is not one of them and opens the window like any other command;
        // that is what dropped every other `kill` the round-restart probe
        // sent within 800 ms of its own `score`. Engine commands run either
        // way, only the game's dispatch is skipped.
        let exempt = ["team ", "score ", "mr "]
            .iter()
            .any(|p| trimmed.starts_with(p));
        let mut client_ok = true;
        if !exempt {
            if c.state == ClientState::Active && self.sv_time_ms < c.next_reliable_ms {
                client_ok = false;
                log::debug!("client text ignored for {}: {trimmed}", c.name);
            }
            c.next_reliable_ms = self.sv_time_ms.wrapping_add(FLOOD_WINDOW_MS);
        }
        let engine_command = matches!(word, "disconnect" | "userinfo");
        if !client_ok && !engine_command {
            c.last_client_command = seq;
            c.netchan.last_client_command_string = s;
            return true;
        }
        match word {
            "disconnect" => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            // The entity's `.name` is not updated with it: script sees the
            // connect-time name and a rename goes stale there, which stage
            // 6's obituaries and scoreboard are the first to notice.
            "userinfo" => {
                let ui = args.trim().trim_matches('"').to_string();
                if let Some(c) = self.clients[slot].as_mut() {
                    if let Some(name) = info_value_for_key(&ui, "name") {
                        c.name = sanitize_name(name);
                    }
                    c.userinfo = ui;
                }
            }
            // DeathmatchScoreboardMessage (.so 0x459c0); grammar in
            // docs/research/cod11-hud-protocol.md section 3.
            "score" => {
                let text = self.scoreboard();
                self.send_server_command(slot, &text);
            }
            // `Cmd_Kill_f`: the same death `self suicide()` gives, asked for
            // by the client. The retail hit capture's death half is this
            // command (combat doc, section 8). Queued rather than run: it
            // starts `CodeCallback_PlayerKilled`, and script runs in the
            // tick's script slot on the tick's clock.
            "kill" => self
                .pending_script_commands
                .push((slot, ScriptCommand::Kill)),
            // `Cmd_MenuResponse_f`: the client answering a menu `openMenu`
            // opened. Unlike the entry notify this fires straight through:
            // nothing is armed by it, and a notify no thread is parked on is
            // simply lost, which is what retail does too. Queued with `kill`,
            // since the notify releases threads.
            "mr" => {
                if let Some((index, response)) =
                    parse_menu_response(trimmed, i32::from(self.server_id))
                {
                    self.pending_script_commands
                        .push((slot, ScriptCommand::MenuResponse(index, response)));
                }
            }
            other => log::debug!("client {slot}: command {other:?} ignored"),
        }
        let Some(c) = self.clients[slot].as_mut() else {
            return false;
        };
        c.last_client_command = seq;
        c.netchan.last_client_command_string = s;
        true
    }

    /// `SV_UserMove` past the parse: queue the block for the next tick's
    /// replay. A flood past the cap drops the oldest cmds, not the newest.
    fn user_move(&mut self, slot: usize, cmds: Vec<UserCmd>) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        // The first usercmd after a gamestate is what puts a client in the
        // world; `begin` is not a CoD client command. docs/protocol-1.1.md,
        // "Entering the world".
        if c.state == ClientState::Primed {
            let first = cmds[0];
            self.enter_world(slot, Some(&first));
        }
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.pending.extend(cmds);
        let excess = c.pending.len().saturating_sub(MAX_PENDING_CMDS);
        c.pending.drain(..excess);
    }

    /// `SV_SendClientGameState`.
    fn send_gamestate(&mut self, slot: usize) {
        let is_bot = self.clients[slot].as_ref().is_some_and(|c| c.is_bot);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Primed;
        // The client restarts its usercmd chain on every gamestate it
        // receives, so the base it deltas against restarts here too.
        c.last_cmd = NULL_USERCMD;
        c.gamestate_message_num = i64::from(c.netchan.outgoing_sequence);
        let mut w = MsgWriter::new(&self.huff);
        write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
        let gs = Gamestate {
            configstrings: self.configstrings.clone(),
            baselines: self.baselines.clone(),
            client_num: slot as i32,
            checksum_feed: self.checksum_feed,
            server_command_sequence: c.netchan.reliable_sequence as i32,
        };
        gamestate::write(&mut w, self.proto, &gs);
        let ops = w.into_ops();
        log::info!("client {slot}: gamestate, {} bytes", ops.len());
        // A bot pulls its own gamestate in step_bots and reads nothing back.
        if is_bot {
            return;
        }
        for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
            self.outbox.push((c.addr, pkt));
        }
    }

    /// `SV_AddServerCommand` plus an immediate message, so a drop notice
    /// reaches the client before its slot is freed. A client whose acks fall
    /// a whole ring behind has stopped consuming reliables; overwriting an
    /// unsacked slot would desync both ends' scramble keys, so that is fatal
    /// (`EXE_LOSTRELIABLECOMMANDS`), mirroring the reference client's own
    /// outbound guard.
    fn send_server_command(&mut self, slot: usize, cmd: &str) {
        let overflow = match self.clients[slot].as_ref() {
            Some(c) => {
                i64::from(c.netchan.reliable_sequence) - i64::from(c.reliable_ack)
                    >= MAX_RELIABLE_COMMANDS as i64
            }
            None => return,
        };
        if overflow {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return;
        }
        self.write_server_command(slot, cmd);
    }

    /// The unconditional tail of [`Self::send_server_command`]. The drop
    /// notice goes out through here, so freeing the slot cannot recurse and
    /// the notice is never guarded away.
    /// Every entity the map puts on the wire, before any client's cull. The
    /// gates read it: `entities_ab` diffs it against the trace's union, and
    /// `entity_vis_ab` culls it at each sample's origin. A snapshot's own list
    /// is already culled, so reading one back would test the cull twice and
    /// the object table not at all.
    pub fn all_entities(&mut self) -> BTreeMap<u32, msg::EntityState> {
        let proto = self.proto;
        self.script
            .as_mut()
            .map_or_else(BTreeMap::new, |rt| rt.packet_entities(proto))
    }

    /// Puts a client where the caller says, the way the `spawn` builtin does.
    /// Test-facing: a gate about what two clients see of each other needs them
    /// somewhere known, and both spawn weighted-random.
    pub fn place_client(&mut self, slot: usize, origin: [f32; 3], yaw_deg: f32) {
        let Some(c) = self.clients.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        if let Some(sim) = c.sim.as_mut() {
            // The spawn builtin's reset is followed by the script's ammo
            // ops in the same frame; nothing follows this one, so the ammo
            // rides across.
            let (ammo, clip) = (sim.ps.ammo, sim.ps.ammoclip);
            sim.become_player(origin, yaw_deg, [0; 3]);
            sim.ps.ammo = ammo;
            sim.ps.ammoclip = clip;
        }
    }

    /// The blasts the last tick's missile pass set off, after the same
    /// tick's radius damage pass charged them. Test-facing: the replay in
    /// `tests/common` counts them per frame.
    pub fn pending_explosions(&self) -> &[crate::game::missile::Explosion] {
        &self.pending_explosions
    }

    /// `DeathmatchScoreboardMessage` (`.so` 0x459c0): `b <numRows> <axis>
    /// <allies>{ <client> <score> <ping> <time> <icon>}*`, one row per online
    /// client (`docs/research/cod11-hud-protocol.md` section 3).
    ///
    /// The score, the deaths and the status icon come from the script's own
    /// client fields, which is where every gametype writes them. `ping` is 0:
    /// the netchan keeps no round-trip estimate, and 0 renders as a number
    /// where retail's `-1` renders as "-" for a client still connecting.
    fn scoreboard(&mut self) -> String {
        let online: Vec<usize> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(slot, c)| c.as_ref().map(|_| slot))
            .collect();
        // Tokens 2 and 3 are the two team scores, axis before allies, the
        // order `DeathmatchScoreboardMessage` pushes them in (map-cycle doc,
        // 6.3). Both retail captures read 0 in each: `dm` writes neither.
        let [axis, allies] = self
            .script
            .as_ref()
            .map_or([0, 0], |rt| rt.host.team_scores);
        let mut text = format!("b {} {axis} {allies}", online.len());
        for slot in online {
            let mut field = |name: &str| {
                self.client_field(slot, name)
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0)
            };
            let score = field("score");
            let deaths = field("deaths");
            let icon = self.status_icon_index(slot);
            text.push_str(&format!(" {slot} {score} 0 {deaths} {icon}"));
        }
        text
    }

    /// A client's `.statusicon` as the 1-based index into `CsRange::StatusIcon`
    /// the scoreboard row carries, or 0 when the field is empty or names an
    /// icon nothing precached. The client resolves `20 + n` for an `n` in
    /// `1..8` (hud protocol doc, section 3).
    fn status_icon_index(&mut self, slot: usize) -> usize {
        let Some(name) = self
            .client_field(slot, "statusicon")
            .filter(|n| !n.is_empty())
        else {
            return 0;
        };
        let (first, last) = crate::configstrings::CsRange::StatusIcon.bounds();
        self.configstrings[first..=last]
            .iter()
            .position(|cs| *cs == name)
            .map_or(0, |i| i + 1)
    }

    /// Whether a shot fired from eye height at `origin` along `yaw_deg`
    /// reaches `dist` without hitting anything. Test-facing, like
    /// `place_client`: a gate that puts one client in front of another has to
    /// say the sightline between them is real, or a spawn point that moved
    /// into a wall reads as "the damage path is broken". A server with no
    /// collision world answers yes.
    pub fn test_clear_line(&self, origin: [f32; 3], yaw_deg: f32, dist: f32) -> bool {
        let Some(world) = self.world.as_ref() else {
            return true;
        };
        let stand = vcod_common::pmove::Stance::Stand.view_height();
        let eye = glam::Vec3::from(origin) + glam::Vec3::Z * stand;
        let (s, c) = yaw_deg.to_radians().sin_cos();
        let end = eye + glam::Vec3::new(c, s, 0.0) * dist;
        world.collision.shot_trace(eye, end).fraction >= 1.0
    }

    /// Test-facing, beside `test_clear_line`: `CanDamage`'s fraction for a
    /// standing player whose feet are at `feet`, from a blast at `at`
    /// (combat doc, 14.3). 0 is a player the blast cannot see at all.
    pub fn test_can_damage(&self, at: [f32; 3], feet: [f32; 3]) -> f32 {
        let Some(world) = self.world.as_ref() else {
            return 1.0;
        };
        use vcod_common::pmove::{Stance, HALF_WIDTH};
        let feet = glam::Vec3::from(feet);
        let v = crate::game::combat::BlastVictim {
            slot: 0,
            origin: feet,
            mins: glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0),
            maxs: glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height()),
            eye: feet + glam::Vec3::Z * Stance::Stand.view_height(),
        };
        crate::game::combat::can_damage(glam::Vec3::from(at), &v, &world.collision)
    }

    /// Test-facing: where a standing player dropped at `p` comes to rest,
    /// or `None` when it starts inside geometry or finds no floor within
    /// 256 units. What a test needs to put a second client somewhere the map
    /// actually holds one.
    pub fn test_ground_under(&self, p: [f32; 3]) -> Option<[f32; 3]> {
        let world = self.world.as_ref()?;
        use vcod_common::pmove::{Stance, HALF_WIDTH};
        let mins = glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
        let maxs = glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
        let start = glam::Vec3::from(p);
        let t = world
            .collision
            .box_trace(start, start - glam::Vec3::Z * 256.0, mins, maxs);
        (!t.startsolid && !t.allsolid && t.fraction < 1.0).then_some(t.endpos.into())
    }

    /// Clones a client into the body queue and returns the corpse's entity
    /// number. Test-facing, like `place_client`: `cloneplayer` is the script
    /// path that will call this, and it does not exist yet.
    pub fn test_push_body(&mut self, slot: usize) -> Option<u32> {
        let c = self.clients.get(slot)?.as_ref()?;
        let state = c
            .sim
            .as_ref()?
            .to_entity(self.proto, slot, c.last_processed_st);
        let (now, proto) = (self.sv_time_ms, self.proto);
        let collision = self.world.as_ref().map(|w| &w.collision);
        self.script
            .as_mut()
            .map(|rt| rt.bodies_mut().push(state, None, now, collision, proto))
    }

    /// Raises one event for the next snapshot build. Test-facing, like
    /// `test_push_body`: the script builtins are what raise these in earnest.
    pub fn test_push_temp_entity(&mut self, te: temp_entity::TempEntity) {
        if let Some(rt) = self.script.as_mut() {
            rt.push_temp_entity(te);
        }
    }

    /// Puts a client back to spectating, where every client starts and where
    /// a dead one waits. Test-facing, like `place_client`.
    pub fn spectate_client(&mut self, slot: usize, origin: [f32; 3]) {
        let Some(c) = self.clients.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        if let Some(sim) = c.sim.as_mut() {
            sim.become_spectator(origin, 0.0, [0; 3]);
        }
    }

    /// Every entity the scripts see as `classname` "player", with the origin
    /// the script side reads, for the gate that pins both. Test-facing: the
    /// server itself never asks.
    pub fn script_players(&mut self) -> Vec<(usize, [f32; 3])> {
        let Some(rt) = self.script.as_mut() else {
            return Vec::new();
        };
        let live: Vec<usize> = (0..self.clients.len())
            .filter(|slot| self.clients[*slot].is_some())
            .collect();
        let mut out = Vec::new();
        for slot in live {
            if rt.client_field(slot, "classname").as_deref() == Some("player") {
                out.push((slot, rt.client_origin(slot)));
            }
        }
        out
    }

    fn write_server_command(&mut self, slot: usize, cmd: &str) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.netchan.reliable_sequence += 1;
        let seq = c.netchan.reliable_sequence;
        c.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)] = cmd.to_string();
        // A bot reads the ring in step_bots; nothing leaves for its socket.
        if c.is_bot {
            return;
        }
        let mut w = MsgWriter::new(&self.huff);
        write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
        // The netchan appends the `svc_EOF`.
        for pkt in c
            .netchan
            .transmit(c.last_client_command, &w.into_ops(), &self.huff)
        {
            self.outbox.push((c.addr, pkt));
        }
    }

    /// `SV_DropClient` (cod_lnxded 0x8085cf4). No zombie state; nothing here
    /// needs the grace period.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        self.write_server_command(slot, &format!("w \"{reason}\""));
        let Some(c) = self.clients[slot].take() else {
            return;
        };
        if let Some(rt) = self.script.as_mut() {
            rt.push_client_event(ClientEvent::Disconnect(slot));
        }
        // Match on ip, not `ch.addr == c.addr`; NAT may have moved the port
        // since the challenge was issued and the slot would stay `connected`.
        for ch in &mut self.challenges {
            if ch.addr.ip() == c.addr.ip() && ch.challenge == c.netchan.challenge {
                ch.connected = false;
            }
        }
        log::info!("client {slot} {:?} dropped: {reason}", c.name);
    }

    /// `SV_CheckTimeouts` with `sv_timeout` and no timeoutCount hysteresis.
    fn check_timeouts(&mut self, now: Instant) {
        let stale: Vec<usize> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.as_ref()
                    .filter(|c| !c.is_bot && now.duration_since(c.last_packet) > TIMEOUT)
                    .map(|_| i)
            })
            .collect();
        for slot in stale {
            self.drop_client(slot, "EXE_TIMEDOUT");
        }
    }

    /// The bots' pass of the tick: join like a client, then move like one.
    /// Runs before `replay_moves`, so a bot's cmd for this frame is in its
    /// queue when the sim reads it. Four passes, because the brains need the
    /// whole of `self` to think and their writes land after.
    fn step_bots(&mut self) {
        if self.cfg.bots == 0 {
            return;
        }
        if !self.bots_spawned {
            self.bots_spawned = true;
            self.spawn_bots();
        }
        let sid = i32::from(self.server_id);
        // The team table once per frame; `client_team` needs the script's
        // mutable face, so the enemy pass reads this instead.
        let teams: Vec<i32> = (0..self.clients.len())
            .map(|slot| self.script.as_mut().map_or(0, |rt| rt.client_team(slot)))
            .collect();

        // Pass 1: the reliable commands each bot just received, answered the
        // way a menu-opening client would execute them client-side.
        let mut replies: Vec<(usize, String)> = Vec::new();
        let mut joined: Vec<usize> = Vec::new();
        let mut entering: Vec<usize> = Vec::new();
        let slots: Vec<usize> = self.bots.keys().copied().collect();
        for &slot in &slots {
            let Some(c) = self.clients[slot].as_ref() else {
                continue;
            };
            let Some(bot) = self.bots.get_mut(&slot) else {
                continue;
            };
            let fresh: Vec<(i32, &str)> = (bot.last_seen_seq + 1
                ..=c.netchan.reliable_sequence as i32)
                .map(|seq| {
                    (
                        seq,
                        c.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)].as_str(),
                    )
                })
                .collect();
            bot.last_seen_seq = c.netchan.reliable_sequence as i32;
            replies.extend(bot.observe(sid, &fresh).into_iter().map(|r| (slot, r)));
            match c.state {
                ClientState::Connected => joined.push(slot),
                // Primed and no sim yet: the entering cmd, which is not
                // simulated (enter_world consumes it).
                ClientState::Primed if c.sim.is_none() => entering.push(slot),
                _ => {}
            }
        }

        // Pass 2: what each bot's body sees, for the brains. The enemy
        // lookup refreshes at ~10 Hz per bot and is cached in between.
        let views: Vec<(usize, crate::bots::BotView)> = slots
            .iter()
            .filter_map(|slot| {
                let fresh = self
                    .bot_enemies
                    .get(slot)
                    .is_none_or(|(t, _)| self.sv_time_ms.wrapping_sub(*t) >= ENEMY_REFRESH_MS);
                let enemy = if fresh {
                    let e = self.bot_enemy(*slot, &teams);
                    self.bot_enemies.insert(*slot, (self.sv_time_ms, e));
                    e
                } else {
                    self.bot_enemies.get(slot)?.1
                };
                let view = self.bot_view(*slot, enemy)?;
                Some((*slot, view))
            })
            .collect();

        // Pass 3: the tick's cmd.
        let mut moves: Vec<(usize, UserCmd)> = Vec::new();
        for (slot, view) in views {
            if let Some(bot) = self.bots.get_mut(&slot) {
                let cmd = bot.think(&view);
                moves.push((slot, cmd));
            }
        }

        // Pass 4: everything lands.
        self.bots.retain(|slot, _| self.clients[*slot].is_some());
        self.bot_enemies
            .retain(|slot, _| self.clients[*slot].is_some());
        for (slot, r) in replies {
            if let Some(bot) = self.bots.get_mut(&slot) {
                let seq = bot.next_command_seq;
                bot.next_command_seq += 1;
                // Consumes its own ack bookkeeping; a bot is its own client.
                self.client_command(slot, seq, r);
            }
        }
        for slot in joined {
            if let Some(bot) = self.bots.get_mut(&slot) {
                bot.rearm();
            }
            self.send_gamestate(slot);
        }
        for (slot, cmd) in moves {
            // Stamped here, not in the brain: the cmd rides the server's own
            // clock, one 50 ms step per tick.
            let cmd = UserCmd {
                server_time: self.sv_time_ms,
                ..cmd
            };
            // Enters the world on the Primed pass, queues on Active.
            self.user_move(slot, vec![cmd]);
        }
        for slot in entering {
            // The entering cmd carries the tick's clock: entry sets the
            // client's cmd base to it, so the first simulated cmd steps a
            // sane 50 ms.
            self.user_move(
                slot,
                vec![UserCmd {
                    server_time: self.sv_time_ms,
                    ..NULL_USERCMD
                }],
            );
        }
        // The bot consumed everything; a real client's ack would say the same.
        for &slot in self.bots.keys() {
            if let Some(c) = self.clients[slot].as_mut() {
                c.reliable_ack = c.netchan.reliable_sequence as i32;
            }
        }
    }

    /// Takes the first free slots for the configured bot count.
    fn spawn_bots(&mut self) {
        for i in 0..self.cfg.bots {
            let Some(slot) = self.clients.iter().position(Option::is_none) else {
                log::warn!("no free slot for bot {}", i + 1);
                return;
            };
            let team = if i % 2 == 0 { "allies" } else { "axis" };
            let mut bot = crate::bots::Bot::new(team, self.cfg.bots_shoot, self.rand() as u64);
            let name = format!("bot{}", i + 1);
            bot.name = name.clone();
            let userinfo = format!("\\name\\{name}\\snaps\\20\\rate\\25000\\cl_anonymous\\0");
            // A loopback address nothing listens on; every transmit to a bot
            // is skipped, the addr only has to be unique.
            let addr = SocketAddr::from(([127, 0, 0, 1], 65535 - i as u16));
            let mut client = Client::new(
                addr,
                0x7000 + i as u16,
                self.rand(),
                userinfo,
                Instant::now(),
            );
            client.is_bot = true;
            self.clients[slot] = Some(client);
            self.bots.insert(slot, bot);
            if let Some(rt) = self.script.as_mut() {
                rt.push_client_event(ClientEvent::Connect {
                    slot,
                    name: name.clone(),
                });
            }
            log::info!("client {slot} {name:?} connected (bot, team {team})");
        }
    }

    /// What one bot's body sees this tick, from its sim and the weapon
    /// table. `None` between levels, where the null cmd is all a bot can
    /// send.
    fn bot_view(
        &self,
        slot: usize,
        enemy: Option<crate::bots::EnemyView>,
    ) -> Option<crate::bots::BotView> {
        let sim = self.clients[slot].as_ref()?.sim.as_ref()?;
        let weapons = self.weapon_table.clone();
        let def = weapons.get(sim.ps.weapon as usize);
        let grenade = (1u8..64).find(|i| {
            sim.ps.weapons_held >> i & 1 == 1
                && weapons
                    .get(*i as usize)
                    .is_some_and(|d| d.weapon_type == "grenade")
        });
        Some(crate::bots::BotView {
            origin: sim.ps.origin.into(),
            delta_angles: sim.delta_angles(),
            view: sim.view_angles(),
            weapon: sim.ps.weapon,
            weapons_held: sim.ps.weapons_held,
            clip: def.map_or(-1, |d| sim.ps.ammoclip[d.clip_index]),
            fire_time_ms: def.map_or(0, |d| (d.fire_time * 1000.0) as i32),
            busy_ms: sim.ps.weapon_time_ms,
            dead: sim.dead,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            enemy,
            grenade,
        })
    }

    /// The nearest live, playing client on another team with a clear eye
    /// line. Team rules: axis and allies only fight each other; everyone
    /// else is fair game to everyone (`dm` carries no teams).
    fn bot_enemy(&self, slot: usize, teams: &[i32]) -> Option<crate::bots::EnemyView> {
        if !self.cfg.bots_shoot {
            return None;
        }
        let me = self.clients[slot].as_ref()?.sim.as_ref()?;
        if me.pm_type != crate::spectate::PmType::Normal || me.dead {
            return None;
        }
        let my_team = teams.get(slot).copied().unwrap_or(0);
        let team_enemy = |their: i32| match my_team {
            script::TEAM_AXIS => their != script::TEAM_AXIS,
            script::TEAM_ALLIES => their != script::TEAM_ALLIES,
            _ => true,
        };
        let my_eye: glam::Vec3 = me.eye_origin().into();
        let world = self.world.as_ref().map(|w| &w.collision);
        // In-range candidates nearest first; the first one the sightline
        // reaches is the nearest visible enemy, so the trace loop stops
        // early instead of scoring every candidate in a crowd.
        let mut candidates: Vec<(f32, glam::Vec3, crate::bots::EnemyView)> = Vec::new();
        for (i, c) in self.clients.iter().enumerate() {
            if i == slot {
                continue;
            }
            let Some(s) = c.as_ref().and_then(|c| c.sim.as_ref()) else {
                continue;
            };
            if s.pm_type != crate::spectate::PmType::Normal || s.dead {
                continue;
            }
            if !team_enemy(teams.get(i).copied().unwrap_or(0)) {
                continue;
            }
            let d = crate::bots::dist_sq(me.ps.origin.into(), s.ps.origin.into());
            if d > crate::bots::SHOOT_RANGE * crate::bots::SHOOT_RANGE {
                continue;
            }
            candidates.push((
                d,
                s.ps.view().eye,
                crate::bots::EnemyView {
                    origin: (s.ps.origin + glam::Vec3::Z * 40.0).into(),
                },
            ));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, eye, enemy) in candidates {
            // The brain aims at the chest, but the eye-high trace is what
            // proves the line: a ray at a feet origin slopes into the floor
            // before it scores a bone.
            let clear = match world {
                Some(w) => w.shot_trace(my_eye, eye).fraction >= 1.0,
                None => true,
            };
            if clear {
                return Some(enemy);
            }
        }
        None
    }

    /// Test-facing, for the bot gates: the slots the bots hold.
    pub fn bot_slots(&self) -> Vec<usize> {
        self.bots.keys().copied().collect()
    }

    /// Test-facing: where a bot's body is and what it is doing.
    pub fn bot_body(&self, slot: usize) -> Option<crate::bots::BotBody> {
        let sim = self.clients[slot].as_ref()?.sim.as_ref()?;
        Some(crate::bots::BotBody {
            origin: sim.ps.origin.into(),
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            dead: sim.dead,
            health: sim.health,
            clip: {
                let d = self.weapon_table.get(sim.ps.weapon as usize);
                d.map_or(-1, |d| sim.ps.ammoclip[d.clip_index])
            },
        })
    }

    /// Test-facing: the script team value a bot landed in.
    pub fn bot_team(&mut self, slot: usize) -> i32 {
        self.script.as_mut().map_or(0, |rt| rt.client_team(slot))
    }

    /// Swap in the map built by the binary; tests run without one.
    pub fn load_world(&mut self, world: World) {
        self.world = Some(Rc::new(world));
    }

    /// The cvar table a gametype script starts with: the engine defaults,
    /// then `default_mp.cfg`, then `g_gametype`, `sv_hostname` and
    /// `sv_maxclients` mirroring this run's `ServerConfig`, and `debug` at
    /// retail's default off, which is what `_utility.gsc`'s exploder logic
    /// (`getCvar("debug") != "1"`) expects when nobody has set it. None of
    /// them are flagged into the 140/204 mirror; only `makeCvarServerInfo`
    /// does that.
    ///
    /// `default_mp.cfg` is where the stock `scr_*` values come from: the
    /// engine execs it at startup, it ships in `localized_english_pak0.pk3`,
    /// and it sets 45 cvars, 38 of them the `scr_*` the stock gametype
    /// scripts read. All but one are invisible in a configstring capture,
    /// because the file's value and the script's own
    /// `makeCvarServerInfo` default agree; `scr_allow_fg42` is the one that
    /// does not, `0` in the file against the script's `"1"`, and
    /// `makeCvarServerInfo` keeps the value already there. It is not
    /// cosmetic: `!getCvar("scr_allow_fg42")` is what makes
    /// `_teams::restrictPlacedWeapons` `delete()` the map's placed fg42s, so
    /// the value moves entity numbering as well as the mirror.
    ///
    /// Past the first level the table comes from the one the outgoing level
    /// left ([`Self::carried_cvars`]) rather than from `default_mp.cfg`
    /// again: retail's is process-global and a script's `setCvar` survives
    /// both boundaries. The config mirrors and the `+set` overrides are
    /// replayed on top either way, so a `--set` still outranks a script.
    fn cvars(&self, fs: &vcod_common::pk3::Pk3Fs) -> crate::cvars::Cvars {
        let mut cvars = match self.carried_cvars.clone() {
            Some(c) => c,
            None => {
                let mut cvars = crate::cvars::Cvars::new();
                match fs.read("default_mp.cfg") {
                    Some(bytes) => {
                        let n = cvars.exec_cfg(&String::from_utf8_lossy(&bytes));
                        log::debug!("default_mp.cfg: {n} cvars set");
                    }
                    // A mount set without the localized pak loses every stock
                    // `scr_*` value, so say so rather than run on script defaults.
                    None => log::warn!(
                        "default_mp.cfg not in the mounted paks; stock scr_* defaults lost"
                    ),
                }
                cvars
            }
        };
        cvars.set("g_gametype", &self.cfg.gametype);
        cvars.set("sv_hostname", &self.cfg.hostname);
        cvars.set("sv_maxclients", &self.cfg.max_clients.to_string());
        cvars.set("debug", "0");
        for (name, value) in &self.cvar_overrides {
            cvars.set(name, value);
        }
        cvars
    }

    /// Loads and runs the gametype and map scripts. Called once per level,
    /// before any client is let back in. The script keeps allocating
    /// configstrings after that (any `setModel`, `loadFX`, `playSound` or
    /// `ambientPlay` from a thread that has passed a `wait`), so the table is
    /// not final at gamestate time; `tick` copies the script's table back
    /// every frame and [`Self::broadcast_configstring_changes`] is what
    /// carries a later allocation to a client that already has its gamestate.
    ///
    /// The table is cloned in, not moved: `ScriptRuntime::load` fails on a
    /// missing `.gsc`, an unresolvable map, a bad BSP or a failed entity
    /// spawn, and the error path has to leave `self.configstrings` exactly as
    /// it was, so the error the caller reports is about the script rather
    /// than about a half-cleared table. `main.rs` exits on it.
    pub fn load_scripts(&mut self, fs: Rc<vcod_common::pk3::Pk3Fs>) -> anyhow::Result<()> {
        self.load_scripts_with(fs, false, crate::game::script::Carry::default(), false)
    }

    /// The test seam for a gametype script that ships in no pak: `path` is a
    /// canonical script path (no extension) and `text` its source, answered
    /// instead of the paks on every later level load. Its only callers are
    /// the semantics probes in `tests/semantics_ents.rs`, which need the real
    /// console and restart paths under them.
    #[doc(hidden)]
    pub fn overlay_script(&mut self, path: &str, text: &str) {
        self.script_overlay = Some((path.to_string(), text.to_string()));
    }

    /// Every line the running level's script has passed to `logPrint`.
    /// Test-facing: a new level starts a new log, so a caller spanning a
    /// restart collects this per level.
    pub fn script_log(&self) -> &[String] {
        self.script.as_ref().map_or(&[], |rt| rt.script_log())
    }

    /// `SV_InitGameProgs(savePersist)` (docs/research/cod11-map-cycle.md,
    /// section 3 step 19): [`Self::load_scripts`] with retail's
    /// `G_InitGame(levelTime, seed, restart, savePersist)` arguments and the
    /// two tables `savePersist` decides the fate of, `game[]` and one
    /// `pers[]` per client slot. `save_persist` is the whole of that
    /// decision: both boundaries lift the two tables and both drop them
    /// here when the outgoing level did not ask for them (section 1, and
    /// the three `probe_persist_*` captures).
    ///
    /// `restart` skips re-reading what a restart cannot have changed: the
    /// paks are the same and so is the map, so the animtree, the weapon
    /// files and the hit-location table stay as they are. That is the whole
    /// of the difference between the two paths here; retail's is the game
    /// module it keeps loaded (section 4.1).
    pub fn load_scripts_with(
        &mut self,
        fs: Rc<vcod_common::pk3::Pk3Fs>,
        restart: bool,
        carry: crate::game::script::Carry,
        save_persist: bool,
    ) -> anyhow::Result<()> {
        let cvars = self.cvars(&fs);
        self.level_cvars = Some((
            cvars.get("g_gametype").to_string(),
            cvars.get("sv_maxclients").to_string(),
        ));
        self.fs = Some(fs.clone());
        if !restart {
            self.anims = match vcod_common::animtree::PlayerAnims::load(&fs) {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("player anims: {e:#}, players will not animate");
                    None
                }
            };
            self.weapon_table = Rc::new(crate::weapons::WeaponTable::load(&fs));
            self.hitlocs = crate::game::combat::HitLocTable::load(&fs);
        }
        // `game[]` and `pers[]` are the script's and go only under
        // `savePersist`; the item registry is the engine's and survives a
        // restart either way, and a map change builds a fresh one (the
        // spawn path hands an empty `Carry`).
        let carry = crate::game::script::Carry {
            game: carry.game.filter(|_| save_persist),
            pers: if save_persist { carry.pers } else { Vec::new() },
            items: carry.items.filter(|_| restart),
        };
        let source = crate::game::script::PakScripts::new(fs.clone(), self.script_overlay.clone());
        let rt = crate::game::script::ScriptRuntime::load_from(
            Box::new(source),
            fs,
            &self.cfg.map,
            &self.cfg.gametype,
            self.configstrings.clone(),
            cvars,
            self.world.clone(),
            self.weapon_table.clone(),
            self.sv_time_ms,
            carry,
        )?;
        let mut configstrings = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut configstrings)
            .map_err(|e| anyhow::anyhow!("writing the cvar mirror: {e:?}"))?;
        self.configstrings = configstrings;
        self.sync_sent_configstrings();
        self.script = Some(rt);
        Ok(())
    }

    /// Every script thread that has died of an error, rendered
    /// `file::func:line: Kind`. Empty is the healthy reading: a thread that
    /// aborts stops running, silently as far as the wire is concerned, so
    /// this is what a test checks a clean bootstrap against.
    pub fn script_aborts(&self) -> Vec<String> {
        self.script.as_ref().map_or_else(Vec::new, |rt| rt.aborts())
    }

    /// One field off a client's script entity, rendered as text. `None` when
    /// no script is loaded or the slot holds no client entity. This is the
    /// only way into a joined client's script state from outside the crate,
    /// and the tests that assert what the stock scripts left there are its
    /// only callers.
    pub fn client_field(&mut self, slot: usize, name: &str) -> Option<String> {
        self.script.as_mut()?.client_field(slot, name)
    }

    /// One key out of a client's `.pers`, rendered the same way.
    pub fn client_pers(&mut self, slot: usize, key: &str) -> Option<String> {
        self.script.as_mut()?.client_pers(slot, key)
    }

    /// One cvar as the running script reads it. `None` before
    /// `load_scripts`. Test-facing: `cfg.gametype` picks which gametype
    /// script loads and this table is what that script's own `getCvar`
    /// answers from, so a rotation that wrote only one of the two is
    /// invisible everywhere else.
    pub fn script_cvar(&self, name: &str) -> Option<String> {
        Some(self.script.as_ref()?.cvars().get(name).to_string())
    }

    /// A `+set` for the next `load_scripts`. `sv_mapRotation` and
    /// `g_gametype` also mirror into their own fields, so `--set` and
    /// `map_rotate`'s `gametype` token both work through one value
    /// (docs/research/cod11-map-cycle.md section 5).
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        // In place when the name is already there, so the last write wins
        // whether it came from `--set` or from a rotation token, and a
        // rotation running for days does not grow the list a map at a time.
        match self.cvar_overrides.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value.to_string(),
            None => self
                .cvar_overrides
                .push((name.to_string(), value.to_string())),
        }
        match name {
            "sv_mapRotation" => self.sv_map_rotation = value.to_string(),
            "g_gametype" => {
                self.cfg.gametype = value.to_string();
                self.refresh_serverinfo();
            }
            _ => {}
        }
    }

    /// `SV_Frame`'s cvar flush, the serverinfo half: a write to a cvar the
    /// serverinfo string carries is followed by
    /// `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (docs/research/cod11-map-cycle.md, 4.3). Both tables, because the
    /// script owns its own copy between level loads and `tick` reads that one
    /// back over this one every frame.
    fn refresh_serverinfo(&mut self) {
        let info = configstrings::serverinfo(&self.cfg).to_string();
        if let Some(slot) = self.configstrings.get_mut(0) {
            *slot = info.clone();
        }
        if let Some(rt) = self.script.as_mut() {
            if let Some(slot) = rt.host.configstrings.get_mut(0) {
                *slot = info;
            }
        }
    }

    /// What the next level load would stamp for `name`, given the config's
    /// own value: a `+set` override if one names it, since [`Self::cvars`]
    /// replays the override list last.
    fn pending_cvar(&self, name: &str, from_cfg: &str) -> String {
        self.cvar_overrides
            .iter()
            .find(|(n, _)| n == name)
            .map_or_else(|| from_cfg.to_string(), |(_, v)| v.clone())
    }

    /// `SV_SpawnServer` (docs/research/cod11-map-cycle.md, section 3), in the
    /// order that list numbers. No gamestate is pushed: every client is
    /// demoted to `CS_CONNECTED` and pulls one off the high-nibble branch of
    /// `handle_client_packet` on its next message (section 3.1).
    ///
    /// The bsp is parsed before anything is torn down, so a `map` naming a
    /// map the paks do not have leaves the level that is serving alone
    /// ([`LoadFailure::KeptLevel`]); the script load past the teardown has no
    /// such way back and is [`LoadFailure::Fatal`].
    /// Retail's 250 ms sleep of step 4 has no counterpart here: this runs
    /// inside a tick that owns the whole server, and sleeping would only
    /// delay the same work.
    pub fn spawn_server(&mut self, map: &str) -> Result<(), LoadFailure> {
        let kept = |e: anyhow::Error| LoadFailure::KeptLevel(e);
        let fs = self
            .fs
            .clone()
            .ok_or_else(|| kept(anyhow::anyhow!("no paks are mounted")))?;
        // Step 15's `CM_LoadMap`, pulled ahead of the teardown.
        let bsp_path = fs
            .resolve_map(map)
            .ok_or_else(|| kept(anyhow::anyhow!("map {map} not found in the mounted paks")))?;
        let bsp_bytes = fs
            .read(&bsp_path)
            .ok_or_else(|| kept(anyhow::anyhow!("reading {bsp_path}")))?;
        let bsp = vcod_common::bsp::parse(&bsp_bytes).map_err(kept)?;

        // Step 2: the flag is read off the outgoing level and handed to the
        // incoming one's `G_InitGame`, along with the two tables it decides
        // the fate of.
        let (save_persist, carry) = self.lift_persistence();
        // Step 3: every client whose state is `CS_PRIMED` or above, which is
        // every one that has a gamestate to be told is stale.
        let text = format!("loadingnewmap\n{map}\n{}", self.cfg.gametype);
        let told: Vec<SocketAddr> = self
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state != ClientState::Connected)
            .map(|c| c.addr)
            .collect();
        for addr in told {
            self.send_oob(addr, &text);
        }
        // Step 5: the game module is unloaded, its object table with it, and
        // step 8's `memset(&sv, 0, ...)` takes everything the level queued.
        self.script = None;
        self.pending_attacks.clear();
        self.pending_explosions.clear();
        self.pending_script_commands.clear();
        self.weapon_changes.clear();
        self.weapon_takes.clear();
        // Step 9.
        self.checksum_feed = (self.rand() << 16) ^ self.rand() ^ self.sv_time_ms;
        // Step 13.
        self.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 15's `Cvar_Set("mapname", ...)` and the collision with it.
        self.cfg.map = map.to_string();
        self.load_world(World::from_bsp(&bsp, Some(&fs)));
        // Step 16, then steps 7 and 10 with step 24 folded in: the table is
        // rebuilt empty around the serverinfo and systeminfo the new id
        // belongs in, which is where the client reads the id back from.
        self.server_id = console::next_map_id(self.server_id);
        self.configstrings = configstrings::static_configstrings(&self.cfg, self.server_id);
        // Step 19. Past the teardown: a failure here leaves no level.
        self.load_scripts_with(fs, false, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        // Step 20: three frames, 100 ms of `svs.time` each.
        for _ in 0..3 {
            self.sv_time_ms = self.sv_time_ms.wrapping_add(100);
            if let Some(rt) = self.script.as_mut() {
                rt.run_frame(self.sv_time_ms);
            }
        }
        // What those frames allocated, before anything reads the table back:
        // every client here is `CS_CONNECTED` and pulls the whole table in
        // the gamestate it asks for, so the diff has nothing to say about a
        // level boundary (the restart path re-syncs the same way).
        if let Some(rt) = self.script.as_ref() {
            self.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        self.sync_sent_configstrings();
        // Step 21.
        self.rebuild_baselines();
        // Step 22, after the settle frames and the baselines: `ClientConnect`
        // again for everyone still on a netchan, then back to `CS_CONNECTED`.
        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            let name = c.name.clone();
            c.reset_for_level();
            if let Some(rt) = self.script.as_mut() {
                rt.reconnect_client(slot, name, self.sv_time_ms);
            }
        }
        self.last_spawn_tick = Some(self.sv_time_ms);
        log::info!(
            "map {map} ({}), serverId {:#04x}, {} clients kept",
            self.cfg.gametype,
            self.server_id,
            self.client_count()
        );
        Ok(())
    }

    /// What the outgoing level asked to keep and the two tables it names
    /// (map-cycle doc, section 1). Also stashes its cvar table, which is
    /// process-global on retail and outlives both boundaries whatever
    /// `savePersist` says. `false` and an empty carry when no level is
    /// running.
    fn lift_persistence(&mut self) -> (bool, crate::game::script::Carry) {
        let Some(rt) = self.script.as_ref() else {
            return (false, crate::game::script::Carry::default());
        };
        self.carried_cvars = Some(rt.cvars().clone());
        (rt.host.save_persist, rt.take_carry())
    }

    /// `SV_MapRestart_f` (map-cycle doc, section 4), in the order that
    /// numbered list. The level is re-inited in place: no gamestate, the
    /// netchan and both reliable rings kept, and only a client that was
    /// already `CS_ACTIVE` re-entered (step 11); a `CS_PRIMED` one is
    /// promoted later by its own next message (4.4).
    pub fn map_restart(&mut self) -> Result<(), LoadFailure> {
        // Step 1: a second restart in one frame, or one straight after a
        // spawn, is a no-op.
        if self.last_spawn_tick == Some(self.sv_time_ms) {
            return Ok(());
        }
        // Step 2.
        if self.script.is_none() {
            log::info!("Server is not running.");
            return Ok(());
        }
        let Some(fs) = self.fs.clone() else {
            return Err(LoadFailure::KeptLevel(anyhow::anyhow!(
                "no paks are mounted"
            )));
        };
        // Step 3: the escalation. A level that asked to keep its
        // persistence gets the in-place restart even across one of these.
        let save_persist = self.script.as_ref().is_some_and(|rt| rt.host.save_persist);
        if !save_persist {
            let changed = self
                .level_cvars
                .as_ref()
                .and_then(|(gametype, max_clients)| {
                    if *gametype != self.pending_cvar("g_gametype", &self.cfg.gametype) {
                        Some("g_gametype")
                    } else if *max_clients
                        != self.pending_cvar("sv_maxclients", &self.cfg.max_clients.to_string())
                    {
                        Some("sv_maxclients")
                    } else {
                        None
                    }
                });
            if let Some(name) = changed {
                log::info!("{name} variable change -- restarting.");
                let map = self.cfg.map.clone();
                return self.spawn_server(&map);
            }
        }
        let (_, carry) = self.lift_persistence();
        // Step 4's six counters: what this level queued and nothing else,
        // since a restart keeps the map and its collision.
        self.pending_attacks.clear();
        self.pending_explosions.clear();
        self.pending_script_commands.clear();
        self.weapon_changes.clear();
        self.weapon_takes.clear();
        self.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 5: only the low nibble moves.
        self.server_id = console::next_restart_id(self.server_id);
        // `sv.configstrings` survives a restart: only a spawn clears it, so
        // every slot the outgoing level allocated keeps its index and its
        // text, and the incoming level's `G_ModelIndex` finds the same slot
        // (map-cycle doc, 4.6). The static slots are rebuilt around the new
        // id on top; 4.3 is what carries slot 1 to a client that already
        // has a gamestate.
        for (i, s) in configstrings::static_configstrings(&self.cfg, self.server_id)
            .into_iter()
            .enumerate()
        {
            if !s.is_empty() {
                self.configstrings[i] = s;
            }
        }
        // Step 7: `SV_RestartGameProgs(savePersist)`. Past the teardown: the
        // outgoing level's script is gone and a failure here leaves none.
        self.load_scripts_with(fs, true, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        // Step 8: three frames, 100 ms of `svs.time` each.
        for _ in 0..3 {
            self.sv_time_ms = self.sv_time_ms.wrapping_add(100);
            if let Some(rt) = self.script.as_mut() {
                rt.run_frame(self.sv_time_ms);
            }
        }
        // What those frames allocated, before anything reads the table back.
        if let Some(rt) = self.script.as_ref() {
            self.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        // The incoming level's whole table, in one go: what a client keeps of
        // it is what the burst below re-sends, and the diff has no business
        // replaying a level boundary slot by slot (doc 4.5).
        self.sync_sent_configstrings();
        // The ambient the new level set, ahead of the `n` because retail's
        // settle frames flush it before step 9 does
        // (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`, seq 42-44).
        if !self.configstring(3).is_empty() {
            for slot in 0..self.clients.len() {
                if self.clients[slot].is_some() {
                    self.send_configstring_update(slot, 3);
                }
            }
        }
        // Steps 9 to 11: `n`, `ClientConnect` again, and back into the world
        // for a client that was already in it.
        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            let was_active = c.state == ClientState::Active;
            let name = c.name.clone();
            let entering = c.last_cmd;
            c.reset_for_restart();
            // Through the guarded path, like every other reliable: a client
            // whose acks have fallen a ring behind is dropped rather than
            // handed an overwritten slot.
            self.send_server_command(slot, "n");
            // That guard can drop the slot; a client that is gone has no
            // `ClientConnect` to run and no world to enter.
            if self.clients[slot].is_none() {
                continue;
            }
            if let Some(rt) = self.script.as_mut() {
                rt.reconnect_client(slot, name, self.sv_time_ms);
            }
            if was_active {
                // The sim `enter_world` builds carries a clear
                // `EF_TELEPORT_BIT`, which is what retail's re-run
                // `ClientConnect` leaves: the incoming level's first spawn is
                // the flip that puts the client at 24 (map-cycle doc, 8.2).
                self.enter_world(slot, Some(&entering));
            }
        }
        // 4.3: `SV_Frame`'s cvar flush carrying the bumped `sv_serverid`,
        // which is what puts the client on the new id.
        for slot in 0..self.clients.len() {
            if self.clients[slot].is_some() {
                self.send_configstring_update(slot, 1);
            }
        }
        self.last_spawn_tick = Some(self.sv_time_ms);
        log::info!(
            "map_restart {} ({}), serverId {:#04x}, {} clients kept",
            self.cfg.map,
            self.cfg.gametype,
            self.server_id,
            self.client_count()
        );
        Ok(())
    }

    /// `SV_CreateBaseline` (map-cycle doc, section 3 step 21), with the
    /// divergence 3.3 records: only `--test-entities` is baselined.
    fn rebuild_baselines(&mut self) {
        self.baselines = match self.test_entities.as_ref() {
            Some(te) => te.baselines(self.proto),
            None => HashMap::new(),
        };
    }

    /// `SV_SetConfigstring`'s per-client half, the `d <index> <text>` server
    /// command. Two callers: the restart burst, which re-sends slots 3 and 1
    /// to a client that keeps its gamestate (map-cycle doc, 4.3), and
    /// [`Self::broadcast_configstring_changes`] once a frame. A map change
    /// reaches neither: it re-syncs the table after its settle frames, so the
    /// diff has nothing to say about the boundary, and every client it leaves
    /// behind is `CS_CONNECTED`, which the diff skips. That is what keeps a
    /// map change off the reliable stream entirely, the gamestate each client
    /// pulls carrying the whole table instead (3.1).
    fn send_configstring_update(&mut self, slot: usize, index: usize) {
        let cmd = format!("d {index} {}", self.configstring(index));
        self.send_server_command(slot, &cmd);
    }

    /// `SV_SetConfigstring`'s per-frame half: every slot the running level
    /// changed goes out as `d <index> <text>` to every client, which is what
    /// retail's broadcast gate lets through while `sv.state` reads 2
    /// (map-cycle doc, 3.1). The script allocates all through the level -- a
    /// `playSound` or `precacheString` from a thread past a `wait` -- and the
    /// index a later `s` or event carries is worthless to a client whose slot
    /// is still empty.
    ///
    /// Called after the script frame and before the frame's own client
    /// commands, so the `d` naming an alias precedes the `s` that plays it.
    /// A `CS_CONNECTED` client is skipped: it has no table to patch, and the
    /// gamestate it pulls carries every slot allocated by then. UNVERIFIED:
    /// what retail's own broadcast does with such a client; its gate is on
    /// `sv.state`, not on the client's.
    fn broadcast_configstring_changes(&mut self) {
        let changed: Vec<usize> = (0..self.configstrings.len())
            .filter(|&i| self.sent_configstrings.get(i) != self.configstrings.get(i))
            .collect();
        if changed.is_empty() {
            return;
        }
        self.sync_sent_configstrings();
        for slot in 0..self.clients.len() {
            let has_table = self.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected);
            if !has_table {
                continue;
            }
            for &i in &changed {
                self.send_configstring_update(slot, i);
            }
        }
    }

    /// Marks the whole table as already known to every client. The two level
    /// boundaries hand it over wholesale -- a map change in the gamestate
    /// each client pulls, a restart in the burst 4.3 describes -- so neither
    /// leaves anything for the diff above to send.
    fn sync_sent_configstrings(&mut self) {
        self.sent_configstrings = self.configstrings.clone();
    }

    /// What [`Self::drain_console`] does with a level load's outcome.
    /// Returns whether the console may keep running.
    fn absorb_load(&mut self, what: &str, r: Result<(), LoadFailure>) -> bool {
        match r {
            Ok(()) => true,
            Err(LoadFailure::KeptLevel(e)) => {
                log::error!("{what}: {e:#}");
                true
            }
            Err(LoadFailure::Fatal(e)) => {
                self.fatal = Some(e.context(what.to_string()));
                false
            }
        }
    }

    /// The load failure that left the server with no level, taken. The binary
    /// polls it after every tick and ends the process on it, which is what
    /// retail's `Com_Error` does: nothing can call `exitLevel` any more and
    /// every client would pull an empty gamestate.
    pub fn take_fatal(&mut self) -> Option<anyhow::Error> {
        self.fatal.take()
    }

    /// Queues a line for `drain_console` to run at the top of the next tick,
    /// `Cbuf_AddText`'s `EXEC_APPEND`.
    pub fn push_console(&mut self, line: &str) {
        self.console.push_back(line.to_string());
    }

    /// `Cbuf_Execute` for the three commands the map cycle uses; anything
    /// else logs and drops. A command pushed by a builtin during the script
    /// pass runs here, at the top of the next tick, which is `EXEC_APPEND`.
    ///
    /// A load that failed before the teardown -- no paks, a map the paks do
    /// not have, a bsp that would not parse -- leaves the level that is
    /// serving alone and is logged. One that failed after it -- the map or
    /// gametype scripts -- has no level left to fall back to, so it stops the
    /// drain and ends the process through [`Self::take_fatal`].
    fn drain_console(&mut self) {
        while let Some(line) = self.console.pop_front() {
            match console::Command::parse(&line) {
                console::Command::Map(map) => {
                    // `map <the map already serving>` is a restart, not a
                    // spawn (doc section 4.2).
                    let r = if map.eq_ignore_ascii_case(&self.cfg.map) && self.script.is_some() {
                        self.map_restart()
                    } else {
                        self.spawn_server(&map)
                    };
                    if !self.absorb_load(&format!("map {map}"), r) {
                        return;
                    }
                }
                console::Command::MapRestart => {
                    let r = self.map_restart();
                    if !self.absorb_load("map_restart", r) {
                        return;
                    }
                }
                console::Command::MapRotate => {
                    let full = self.sv_map_rotation.clone();
                    let before = self.cfg.gametype.clone();
                    let mut gametype = before.clone();
                    let next = self.rotation.rotate(&full, &mut gametype);
                    for w in self.rotation.warnings.drain(..) {
                        log::warn!("{w}");
                    }
                    if gametype != before {
                        // Retail's plain `Cvar_Set` (doc section 5.2), so
                        // through `set_cvar`: writing `cfg.gametype` alone
                        // picks the new gametype's script while `cvars`
                        // replays an earlier `--set g_gametype` over the
                        // table that script then reads.
                        self.set_cvar("g_gametype", &gametype);
                        // A gametype change discards what exitLevel(true)
                        // asked to keep (doc section 5.2).
                        if let Some(rt) = self.script.as_mut() {
                            rt.host.save_persist = false;
                        }
                    }
                    match next {
                        console::Rotate::Map(m) => self.console.push_front(format!("map {m}")),
                        console::Rotate::Restart => self.console.push_front("map_restart".into()),
                    }
                }
                console::Command::Unknown(l) => log::warn!("console: unknown command {l:?}"),
            }
        }
    }

    const FALLBACK_SPAWN: ([f32; 3], f32) = ([0.0, 0.0, 64.0], 0.0);

    /// `SV_ClientEnterWorld` for a spectator: park the sim at the spawn, start
    /// snapping. `entering` is the usercmd that brought the client in, which
    /// is `None` on the paths that have none, as ioq3's is null there; its
    /// angles are what the spawn's `delta_angles` subtract.
    fn enter_world(&mut self, slot: usize, entering: Option<&UserCmd>) {
        let spawn = self
            .world
            .as_ref()
            .map_or(Self::FALLBACK_SPAWN, |w| w.spawn);
        let cmd_angles = entering.map_or(NULL_USERCMD.angles, |c| c.angles);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Active;
        c.sim = Some(ClientSim::spectator(spawn.0, spawn.1, cmd_angles));
        // The entering cmd is not simulated: retail's execute loop skips
        // every cmd at or before `lastUsercmd`, which entry has just set to
        // it. The clock is therefore the client's own, as retail's is: a
        // first cmd stamped far in the future freezes that client's sim until
        // its clock catches up, and only that client's. With no entering cmd,
        // one frame back, so the first cmd's dt is a sane 50 ms rather than
        // the whole age of the client's clock.
        c.last_processed_st =
            entering.map_or(self.sv_time_ms.wrapping_sub(FRAME_MS), |c| c.server_time);
        log::info!("client {slot} {:?} begin (spectator)", c.name);
        // `ClientBegin`: the notify that releases the connect callback's
        // `waittill("begin")`. The event queues rather than fires here, so it
        // is drained after the `Connect` that armed the wait.
        if let Some(rt) = self.script.as_mut() {
            rt.push_client_event(ClientEvent::Begin(slot));
        }
    }

    pub fn tick(&mut self, now: Instant) {
        self.drain_console();
        self.check_timeouts(now);
        self.step_bots();
        self.sv_time_ms = self.sv_time_ms.wrapping_add(FRAME_MS);
        // Wall gap between ticks: sv_time always advances exactly FRAME_MS, so
        // a gap far off it means the frames the client interpolates between
        // are not arriving at the rate their timestamps claim.
        let wall_ms = self
            .last_tick
            .map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0);
        self.last_tick = Some(now);

        // Every client's pending moves first, so the world is at this frame
        // before script or anyone's snapshot reads it.
        let moved = self.replay_moves();
        let weapons = self.weapon_table.clone();

        // Then the frame's shots, against the world the moves left: each is
        // a trace, an impact for the snapshot and, on a player, a hit the
        // damage callback is handed before the script frame runs
        // (`crate::game::combat`).
        let mut hits = Vec::new();
        let mut impacts = Vec::new();
        let mut throws: Vec<(usize, u8, i32, glam::Vec3, glam::Vec3, i32)> = Vec::new();
        {
            let sims: Vec<(usize, &crate::spectate::ClientSim)> = self
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let collision = self.world.as_ref().map(|w| &w.collision);
            // The locational trace's context, absent on a host with no paks
            // or no animtree; a shot then lands at hit location `none`.
            let mut bones = match (self.fs.as_deref(), self.anims.as_ref()) {
                (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                    fs,
                    anims,
                    rigs: &mut self.hit_rigs,
                    now_ms: self.sv_time_ms,
                }),
                _ => None,
            };
            for attack in self.pending_attacks.drain(..) {
                let (slot, weapon) = match attack {
                    Attack::Shot(s) => (s.slot, s.weapon),
                    Attack::Swing { slot, weapon } => (slot, weapon),
                    Attack::Throw {
                        slot,
                        weapon,
                        fuse_left_ms,
                    } => {
                        // The throw leaves the thrower's eye at the weapon
                        // file's speed (combat doc, 11.3); the missile pass
                        // below is what spawns it.
                        if let (Some((_, me)), Some(def)) = (
                            sims.iter().find(|(s, _)| *s == slot),
                            weapons.get(weapon as usize),
                        ) {
                            let (origin, velocity) =
                                crate::game::missile::throw_velocity(&me.ps, def);
                            // The projectile's model, indexed when the item was
                            // registered (`GameHost::register_item`). A miss
                            // means the map load stopped registering it and
                            // the client has no model to draw the grenade
                            // with, which is silent on the wire.
                            let name = def.projectile_model.as_deref().unwrap_or_default();
                            let model =
                                crate::configstrings::weapon_model_index(&self.configstrings, name);
                            if model == 0 && !name.is_empty() {
                                log::warn!(
                                    "the grenade client {slot} threw carries {name:?}, \
                                     which nothing precached"
                                );
                            }
                            throws.push((slot, weapon, model, origin, velocity, fuse_left_ms));
                        }
                        continue;
                    }
                };
                let Some(def) = weapons.get(weapon as usize) else {
                    continue;
                };
                let name = crate::items::item_name(weapon as usize).unwrap_or_default();
                let r = match attack {
                    Attack::Shot(shot) => crate::game::combat::bullet_fire(
                        slot,
                        def,
                        name,
                        shot.ads,
                        &sims,
                        collision,
                        &self.hitlocs,
                        bones.as_mut(),
                        &mut self.rng,
                    ),
                    Attack::Swing { .. } => crate::game::combat::melee_fire(
                        slot,
                        def,
                        name,
                        weapon,
                        &sims,
                        collision,
                        &self.hitlocs,
                        bones.as_mut(),
                        &mut self.rng,
                    ),
                    Attack::Throw { .. } => continue,
                };
                impacts.extend(r.impact);
                hits.extend(r.hit);
            }
        }

        let mut client_commands = Vec::new();
        let mut console_lines: Vec<String> = Vec::new();
        let mut ranks_dirty = false;
        // A client that dropped between the packet and here has nothing left
        // to run its command against.
        let queued: Vec<(usize, ScriptCommand)> = std::mem::take(&mut self.pending_script_commands)
            .into_iter()
            .filter(|(slot, _)| self.clients[*slot].is_some())
            .collect();
        if let Some(rt) = self.script.as_mut() {
            for (slot, c) in self.clients.iter().enumerate() {
                if let Some(c) = c {
                    rt.set_client_buttons(slot, c.last_cmd.buttons | moved[slot].buttons);
                }
            }
            for te in impacts {
                rt.push_temp_entity(te);
            }
            // The throws the weapon step queued, then one `G_RunMissile`
            // each. Retail's missile pass runs ahead of the damage
            // callbacks, and a usercmd is executed between frames, so a
            // grenade thrown on this tick was armed on the last one and has
            // already flown a frame by the time the snapshot goes out
            // (`docs/research/cod11-combat.md` sections 11 and 12).
            for (slot, weapon, model, origin, velocity, fuse_left_ms) in throws {
                rt.fire_grenade(
                    slot,
                    weapon,
                    model,
                    origin,
                    velocity,
                    fuse_left_ms,
                    self.sv_time_ms.wrapping_sub(FRAME_MS),
                );
            }
            let sims: Vec<(usize, &crate::spectate::ClientSim)> = self
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let frame = rt.run_missiles(
                self.world.as_ref().map(|w| &w.collision),
                &sims,
                self.sv_time_ms,
            );
            for te in frame.temp {
                rt.push_temp_entity(te);
            }
            // What the radius damage pass charges, on this same frame.
            self.pending_explosions = frame.exploded;
            // Each blast becomes hits before `deliver_hits` runs, so a
            // grenade damages on the frame it goes off (combat doc, 14.1).
            let collision = self.world.as_ref().map(|w| &w.collision);
            for x in &self.pending_explosions {
                let Some(def) = weapons.get(x.weapon as usize) else {
                    continue;
                };
                let victims: Vec<crate::game::combat::BlastVictim> = sims
                    .iter()
                    .filter(|(_, s)| s.pm_type == crate::spectate::PmType::Normal && !s.dead)
                    .map(|(slot, s)| crate::game::combat::BlastVictim {
                        slot: *slot,
                        origin: s.ps.origin,
                        mins: s.ps.mins(),
                        maxs: s.ps.maxs(),
                        eye: s.ps.view().eye,
                    })
                    .collect();
                hits.extend(crate::game::combat::radius_damage(
                    x.at,
                    def.explosion_radius,
                    def.explosion_inner_damage as f32,
                    def.explosion_outer_damage as f32,
                    Some(x.owner),
                    Some(x.inflictor),
                    crate::items::item_name(x.weapon as usize).unwrap_or_default(),
                    "MOD_GRENADE_SPLASH",
                    &victims,
                    collision,
                ));
            }
            // The client commands the packet pass queued, on this frame's
            // clock: retail runs `Cmd_Kill_f` ahead of the damage callbacks
            // too, and a thread started here sees `level.time` already
            // advanced, which is what a `cloneplayer` in it needs.
            for (slot, cmd) in queued {
                match cmd {
                    ScriptCommand::Kill => {
                        rt.kill_client(slot, self.sv_time_ms);
                    }
                    ScriptCommand::MenuResponse(index, response) => {
                        rt.menu_response(slot, index, &response)
                    }
                }
            }
            rt.deliver_hits(hits, self.sv_time_ms);
            rt.run_frame(self.sv_time_ms);
            console_lines = rt.take_console();
            client_commands = rt.take_client_commands();
            ranks_dirty = rt.take_ranks_dirty();
            // The script owns the table while it runs and allocates into it
            // from any thread, so the server re-reads it rather than trusting
            // the copy `load_scripts` took. A whole-table copy per frame is
            // cheap next to a snapshot, and there is no single write choke
            // point on the host's table to hang a dirty flag off. The cvar
            // mirror gets the same treatment: a thread past a `wait` can
            // still call `setCvar`.
            self.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
            // `self spawn(origin, angles)` moves the sim, which no builtin
            // can reach; this is where the queue lands. Before the weapons,
            // because a spawn resets the whole playerstate and would wipe the
            // clip the same frame's `giveWeapon` just handed out.
            for s in rt.take_client_spawns() {
                let Some(c) = self.clients.get_mut(s.slot).and_then(Option::as_mut) else {
                    continue;
                };
                // The client's own angles at this moment, not zero: a
                // spectator that looked around before answering the weapon
                // menu carries them, and `spawn_delta_angles` subtracts them
                // so the spawn preserves the view instead of force-turning it.
                let cmd_angles = c.last_cmd.angles;
                let Some(sim) = c.sim.as_mut() else {
                    continue;
                };
                match s.mode {
                    SpawnMode::Player => sim.become_player(s.origin, s.yaw_deg, cmd_angles),
                    SpawnMode::Spectator => sim.become_spectator(s.origin, s.yaw_deg, cmd_angles),
                    SpawnMode::Intermission => {
                        sim.become_intermission(s.origin, s.yaw_deg, cmd_angles)
                    }
                }
            }
            // The machine's own switches first: `pickup` writes `ps.weapon`
            // when a drop ends, and the mirror below would put the old
            // weapon back and start the identical putaway again.
            for (slot, weapon) in self.weapon_changes.drain(..) {
                rt.set_client_weapon(slot, weapon);
            }
            // The take after the switch, so a weapon the machine moved to is
            // not the one dropped.
            for (slot, weapon) in self.weapon_takes.drain(..) {
                rt.take_client_weapon(slot, weapon);
            }
            // What the client holds comes across the same way the
            // configstrings do: re-read every frame, because any thread can
            // have changed them. The held bits have to be among them --
            // `PM_Weapon` disarms a player who no longer owns `ps.weapon`
            // (combat doc, section 1.8) -- and `ps.weapon` with them, so a
            // sim reset outside a move comes back armed. The write-back
            // above is what makes that safe: the host's copy already carries
            // whatever the machine switched to this tick.
            for (slot, c) in self.clients.iter_mut().enumerate() {
                if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
                    let w = rt.client_weapons(slot);
                    sim.ps.weapons_held = w.held;
                    sim.ps.weapon_slots = w.slots;
                    sim.ps.weapon = w.current;
                    sim.viewmodel_index = rt.client_viewmodel(slot);
                    // The body, head and helmet the character script dressed
                    // the client in: what a shot at it is traced against.
                    if let Some(a) = rt.client_assembly(slot) {
                        if a != sim.assembly {
                            sim.assembly = a;
                        }
                    }
                    // And back the other way: the sim owns where a player is,
                    // so the script's copy is written from it every frame.
                    rt.set_client_origin(slot, sim.origin());
                }
            }
            // The ammo and the current weapon, which are edges rather than
            // state: applying a full clip every frame would make the weapon
            // bottomless.
            for (slot, op) in rt.take_weapon_ops() {
                let Some(sim) = self
                    .clients
                    .get_mut(slot)
                    .and_then(Option::as_mut)
                    .and_then(|c| c.sim.as_mut())
                else {
                    continue;
                };
                apply_weapon_op(sim, op, &weapons);
            }
            // What `finishPlayerDamage` did to each sim, applied once, then
            // the health mirror and the frame's damage feedback, in that
            // order: `P_DamageFeedback` reads the health the hit left.
            let anims = self.anims.as_ref();
            for (slot, op) in rt.take_sim_ops() {
                let Some(sim) = self
                    .clients
                    .get_mut(slot)
                    .and_then(Option::as_mut)
                    .and_then(|c| c.sim.as_mut())
                else {
                    continue;
                };
                let index = sim.ps.weapon as usize;
                let inputs = anims.map(|anims| crate::spectate::AnimInputs {
                    anims,
                    weapon: crate::items::item_name(index).unwrap_or_default(),
                    weapon_class: weapons.class(index),
                });
                sim.take_damage(&op, inputs.as_ref(), &mut self.rng, self.sv_time_ms);
            }
            for (slot, c) in self.clients.iter_mut().enumerate() {
                if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
                    // Neither `ClientEndFrame`'s intermission arm nor
                    // `SpectatorClientEndFrame` copies `ent->health` into the
                    // playerstate, so both keep the zero their own spawn left
                    // (map-cycle doc, 6.2; `spectate.rs`, `become_spectator`).
                    if sim.pm_type == crate::spectate::PmType::Normal {
                        let v = rt.client_vitals(slot);
                        sim.health = v.health;
                        sim.max_health = v.max_health;
                        sim.dead = v.dead;
                    }
                    sim.end_frame(self.sv_time_ms);
                }
            }
        }
        // `trap_SendConsoleCommand`'s `EXEC_APPEND`: what a builtin queued
        // this frame runs at the top of the next tick, never mid-frame.
        self.console.extend(console_lines);
        // Ahead of the frame's client commands, so a slot the script just
        // allocated is named before anything points at it.
        self.broadcast_configstring_changes();
        // Outside the borrow: `setClientCvar` and `openMenu` queue rather
        // than send, and this is where the queue reaches the netchan.
        for (slot, cmd) in client_commands {
            self.send_server_command(slot, &cmd);
        }
        // `G_RunFrame`'s inlined drain (map-cycle doc, 6.3): a frame that
        // moved a score pushes the scoreboard to every client in
        // intermission, and no frame pushes one otherwise. The map-change
        // capture carries no `b` its probe did not ask for, so a timer here
        // would be a command retail never sends.
        if ranks_dirty {
            let watching: Vec<usize> = self
                .clients
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.as_ref()
                        .and_then(|c| c.sim.as_ref())
                        .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission)
                })
                .map(|(slot, _)| slot)
                .collect();
            for slot in watching {
                let text = self.scoreboard();
                self.send_server_command(slot, &text);
            }
        }

        // Every entity built once, then culled and written per client.
        self.send_snapshots(&moved, wall_ms);
        // The weapon changes are already drained when a script is loaded,
        // and a server without one has nothing to write them to.
        self.weapon_changes.clear();
        self.weapon_takes.clear();
    }

    /// SV_UserMove for every client: one pmove step per queued usercmd, dt off
    /// the cmd clocks, matching the client's own prediction, then the anims the
    /// resulting state implies. Returns what each slot replayed, for the trace
    /// line `send_snapshots` writes. The shots, swings and throws the weapon
    /// step took land in `pending_attacks`, which the combat path drains.
    fn replay_moves(&mut self) -> Vec<MoveSummary> {
        use vcod_common::pmove::weapon::{EV_FIRE_MELEE, EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT};
        let collision = self.world.as_ref().map(|w| &w.collision);
        let weapons = self.weapon_table.clone();
        let mut moved = vec![MoveSummary::default(); self.clients.len()];
        for (slot, m) in moved.iter_mut().enumerate() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            let Some(sim) = c.sim.as_mut() else {
                continue;
            };
            // What the client held going in, so a switch the machine made is
            // told apart from a playerstate reset between ticks.
            let held = sim.ps.weapon;
            // Stale cmds (dt <= 0) are skipped whole; a long one is chopped
            // rather than clamped away; a flood past the per-tick cap resyncs
            // to the newest cmd and keeps only the tail.
            let mut last_cmd = None::<UserCmd>;
            // Every event the tick's moves raised, for the animation events:
            // a tick that ran several moves still raises each one.
            let mut events = Vec::new();
            while !c.pending.is_empty() {
                let cmd = c.pending[0];
                if m.processed >= MAX_CMDS_PER_TICK {
                    // The resync keeps the newest two cmds and sets the base
                    // as if only the last replays, so the penultimate may
                    // double-count one frame; harmless for flight.
                    c.last_processed_st =
                        c.pending.last().unwrap().server_time.wrapping_sub(FRAME_MS);
                    c.pending.drain(..c.pending.len().saturating_sub(2));
                    break;
                }
                c.pending.remove(0);
                let dt_ms = cmd.server_time.wrapping_sub(c.last_processed_st);
                if dt_ms <= 0 {
                    continue;
                }
                // A hitching client's gap is simulated, not discarded: retail
                // walks `commandTime` up to the cmd's clock in steps of at most
                // `MAX_FRAME_MS`, each its own `PmoveSingle`, and drops only
                // the arrears past `MAX_PMOVE_ARREARS_MS`.
                let mut base = c.last_processed_st;
                if dt_ms > MAX_PMOVE_ARREARS_MS {
                    base = cmd.server_time - MAX_PMOVE_ARREARS_MS;
                }
                let mut raised = Vec::new();
                while base != cmd.server_time {
                    let msec = (cmd.server_time - base).min(MAX_FRAME_MS as i32);
                    base += msec;
                    // Each step runs on a cmd stamped at its own end, which is
                    // what the loop hands `PmoveSingle` (0x344e4) and what that
                    // then leaves in `commandTime` (0x34074).
                    let step = UserCmd {
                        server_time: base,
                        ..cmd
                    };
                    raised.extend(sim.step(&step, msec as f32 / 1000.0, collision, weapons.defs()));
                }
                for e in &raised {
                    let weapon = sim.ps.weapon;
                    let grenade = weapons
                        .get(weapon as usize)
                        .is_some_and(|d| d.weapon_type == "grenade");
                    match e.event {
                        // A grenade's fire event is the throw, and the parm
                        // is what is left of the fuse (combat doc, 1.11).
                        EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if grenade => {
                            self.pending_attacks.push(Attack::Throw {
                                slot,
                                weapon,
                                fuse_left_ms: e.parm,
                            })
                        }
                        EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => {
                            self.pending_attacks.push(Attack::Shot(Shot {
                                slot,
                                weapon,
                                ads: sim.ps.weapon_pos_frac == 1.0,
                            }))
                        }
                        EV_FIRE_MELEE => self.pending_attacks.push(Attack::Swing { slot, weapon }),
                        _ => {}
                    }
                    // A `clipOnly` weapon with nothing left is taken away
                    // (combat doc, 1.5 step 9), and 1.8's switch path then
                    // takes `ps.weapon` to 0 on its own. Hung off the last
                    // shot, not off `EV_NOAMMO`, which a dry trigger raises
                    // too and keeps the weapon.
                    if e.event == EV_FIRE_WEAPON_LASTSHOT {
                        if let Some(def) = weapons.get(weapon as usize) {
                            if def.clip_only && sim.ps.ammo[def.ammo_index] == 0 {
                                self.weapon_takes.push((slot, weapon));
                            }
                        }
                    }
                }
                events.extend(raised);
                last_cmd = Some(cmd);
                c.last_processed_st = cmd.server_time;
                m.first_cmd_st.get_or_insert(cmd.server_time);
                m.last_cmd_st = Some(cmd.server_time);
                m.buttons |= cmd.buttons;
                m.processed += 1;
            }
            if sim.ps.weapon != held {
                self.weapon_changes.push((slot, sim.ps.weapon));
            }
            // The animation the client should be playing, from the state the
            // moves just produced and the input that produced it.
            if let (Some(anims), Some(cmd)) = (self.anims.as_ref(), last_cmd) {
                let index = sim.ps.weapon as usize;
                let weapon = crate::items::item_name(index).unwrap_or_default();
                let class = self.weapon_table.class(index);
                sim.update_anims(
                    &crate::spectate::AnimInputs {
                        anims,
                        weapon,
                        weapon_class: class,
                    },
                    &cmd,
                    self.sv_time_ms,
                    &events,
                    &mut self.rng,
                );
            }
        }
        // The state each player ended the tick in, mirrored onto the host for
        // `cloneplayer`: a builtin cannot reach a sim, and the corpse is the
        // dying player's entity state (`crate::game::bodies`). Written here,
        // before the script frame, because that is the frame the script clones
        // in; `send_snapshots` re-reads a newborn body afterwards so the death
        // animation the script raised after the clone still lands on it.
        let proto = self.proto;
        if let Some(rt) = self.script.as_mut() {
            for (slot, c) in self.clients.iter().enumerate() {
                let sim = c
                    .as_ref()
                    .and_then(|c| Some((c.sim.as_ref()?, c.last_processed_st)));
                rt.set_client_entity_state(slot, sim.map(|(s, st)| s.to_entity(proto, slot, st)));
                // The cook a death drops, off the same state: both kill paths
                // run later in this tick, so what they read is this frame's
                // and not the last one's.
                rt.set_client_grenade_ms(slot, sim.map_or(0, |(s, _)| s.ps.grenade_time_left_ms));
            }
        }
        moved
    }

    /// One snapshot per active client per tick, the main loop pacing calls at
    /// sv_fps: a delta against the frame the client last acked when one is
    /// still in its ring, uncompressed otherwise. Every entity state is built
    /// once from the world the moves and the script frame left, then culled
    /// and written per client.
    fn send_snapshots(&mut self, moved: &[MoveSummary], wall_ms: Option<f32>) {
        // One clientState entry per online client, rebuilt each frame; slot ==
        // index. `snapshot::write` deltas this against each client's own
        // base roster, or sends it full when that client has none.
        // The body model rides here, not on the entity: without it another
        // client is sent a player it can name but cannot draw
        // (`docs/research/clientstate-wire-format.md`).
        // The team rides here too: it is what the receiving client colours
        // names and tells friend from foe with, and script owns it through
        // `.sessionteam`. Without a script there is nothing to ask, so the
        // slot keeps the value retail's `ClientConnect` leaves in the field.
        type Roster = (i32, Vec<(i32, i32)>, i32);
        let per_slot: Vec<Roster> = match self.script.as_mut() {
            Some(rt) => (0..self.clients.len())
                .map(|slot| {
                    (
                        rt.client_model_index(slot),
                        rt.client_attachments(slot),
                        rt.client_team(slot),
                    )
                })
                .collect(),
            None => vec![(0, Vec::new(), script::TEAM_SPECTATOR); self.clients.len()],
        };
        let roster: BTreeMap<u32, msg::ClientState> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref()?;
                let (model, attachments, team) = &per_slot[i];
                let mut cs = msg::ClientState::named(self.proto, 0, *team, &c.name);
                if let Some(idx) = msg::ClientState::field_index(self.proto, "modelindex") {
                    cs.fields[idx] = *model;
                }
                for (n, (am, at)) in attachments.iter().enumerate() {
                    for (name, v) in [
                        (format!("attachModelIndex[{n}]"), am),
                        (format!("attachTagIndex[{n}]"), at),
                    ] {
                        if let Some(idx) = msg::ClientState::field_index(self.proto, &name) {
                            cs.fields[idx] = *v;
                        }
                    }
                }
                Some((i as u32, cs))
            })
            .collect();
        // The map's own entities, plus whatever --test-entities adds: the
        // scripted ones exist to drive the wire path and have no gameplay
        // meaning, so they sit beside the object table's rather than
        // replacing it.
        let mut entities: BTreeMap<u32, msg::EntityState> = self
            .script
            .as_mut()
            .map_or_else(BTreeMap::new, |rt| rt.packet_entities(self.proto));
        if let Some(te) = self.test_entities.as_ref() {
            entities.extend(te.at(self.proto, self.sv_time_ms));
        }
        let collision_vis = self.world.as_ref().map(|w| &w.vis);

        // One entity per client with a sim, so a client can be told what the
        // others are doing. The receiver's own is dropped below: retail sends
        // a client no entity for itself.
        let client_entities: BTreeMap<u32, msg::EntityState> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let sim = c.as_ref()?.sim.as_ref()?;
                // A spectator, an intermission camera and a dead player are
                // each unlinked or `SVF_NOCLIENT`, so nobody is sent an
                // entity for one ([`crate::spectate::ClientSim::linked`]).
                if sim.linked() {
                    Some((
                        i as u32,
                        sim.to_entity(self.proto, i, c.as_ref()?.last_processed_st),
                    ))
                } else {
                    None
                }
            })
            .collect();

        // A body born this frame is re-read from its source client's sim
        // once, here: the script clones the player before it raises the
        // death animation, so the state the clone copied is a frame stale
        // (`crate::game::bodies`). Then the queue's corpses join the map's
        // entities; they are culled per client like anything else.
        let (now, proto) = (self.sv_time_ms, self.proto);
        let collision = self.world.as_ref().map(|w| &w.collision);
        let clients = &self.clients;
        if let Some(rt) = self.script.as_mut() {
            // Straight off the sim, not `client_entities`: the source is
            // dead by now and has no entity there.
            rt.bodies_mut().refresh_newborn(
                now,
                |slot| {
                    let c = clients.get(slot)?.as_ref()?;
                    Some(c.sim.as_ref()?.to_entity(proto, slot, c.last_processed_st))
                },
                collision,
                proto,
            );
            entities.extend(rt.bodies().entities());
        }

        // Every event the script raised this frame, as one-frame entities
        // out of the reserved block. Draining them here is what frees them,
        // the way retail frees a `G_TempEntity` the frame after it is sent.
        let temps = self
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.take_temp_entities());
        let cursor = self.temp_cursor;
        let temp_states: Vec<(u32, msg::EntityState)> = temps
            .iter()
            .take(temp_entity::TEMP_COUNT as usize)
            .enumerate()
            .map(|(i, te)| {
                let n = temp_entity::number_at(cursor, i);
                (n, temp_entity::build(te, n, self.proto))
            })
            .collect();
        self.temp_cursor = temp_entity::advance(cursor, temp_states.len());

        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            // No socket to write to; the bot's slot still counts toward the
            // roster and entity lists the real clients are sent.
            if c.is_bot {
                continue;
            }
            let Some(sim) = c.sim.as_ref() else {
                continue;
            };
            // Exactly the serverTime of the last cmd the sim consumed, and
            // nothing else: the client replays everything past it, so a
            // commandTime we never simulated drops that slice of its input
            // and its prediction judders (docs/protocol-1.1.md).
            let command_time = c.last_processed_st;
            let message_num = c.netchan.outgoing_sequence;

            // Retail sends a client only what its own position can see, so
            // the list is per client rather than one list cloned into every
            // frame (docs/protocol-1.1.md, "Which entities a client is sent").
            let mut sendable = entities.clone();
            sendable.extend(
                client_entities
                    .iter()
                    .filter(|(n, _)| **n != slot as u32)
                    .map(|(n, e)| (*n, e.clone())),
            );
            // A scoped temp entity is culled like any other entity; a
            // broadcast one skips the cull, which is what retail's
            // `SVF_BROADCAST` does (docs/protocol-1.1.md, "Which entities a
            // client is sent").
            for (te, (n, e)) in temps.iter().zip(&temp_states) {
                if temp_entity::visible_to(te, slot) && te.scope != temp_entity::Scope::Broadcast {
                    sendable.insert(*n, e.clone());
                }
            }
            let mut visible = match collision_vis {
                Some(vis) => {
                    crate::world::visible_entities(vis, sim.eye_origin(), &sendable, self.proto)
                }
                None => sendable,
            };
            for (te, (n, e)) in temps.iter().zip(&temp_states) {
                if te.scope == temp_entity::Scope::Broadcast {
                    visible.insert(*n, e.clone());
                }
            }
            // A missile carries `SVF_BROADCAST` too (combat doc, 11.1), so
            // it skips the cull the way a broadcast temp entity does.
            if let Some(rt) = self.script.as_ref() {
                visible.extend(rt.missiles().entities(self.proto));
            }

            let mut ps = sim.to_wire(self.proto, slot as i32, command_time);
            // The script's HUD elements, filtered for this client the way
            // `HudElem_UpdateClient` filters them; retail rebuilds both
            // arrays into the playerstate once per client per frame, so
            // they are read here rather than carried on the sim.
            let team = per_slot.get(slot).map_or(script::TEAM_SPECTATOR, |p| p.2);
            if let Some(rt) = self.script.as_ref() {
                let (archived, current) = rt.hud_elems(slot, team);
                ps.arrays.hud_archived = archived;
                ps.arrays.hud_current = current;
            }
            let frame = snapshot::Snapshot {
                server_time: self.sv_time_ms,
                message_num,
                delta_num: -1,
                snap_flags: self.snap_flag_server_bit,
                ps,
                entities: visible,
                clients: roster.clone(),
                valid: true,
            };

            // The base is the frame the client last acked, if it is still in
            // the ring and close enough for the byte-wide deltaNum offset.
            // Safe at a full SV_PACKET_BACKUP depth (no margin, unlike
            // retail's SV_WriteSnapshotToClient, which keeps PACKET_BACKUP -
            // 3) only because both this ring and the client's own read a slot
            // before either side can overwrite it.
            let base = c
                .sent_frame(c.message_ack.max(0) as u32)
                .filter(|b| {
                    let back = message_num.saturating_sub(b.message_num);
                    (1..=255).contains(&back)
                })
                .cloned();

            let mut w = MsgWriter::new(&self.huff);
            write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
            w.write_byte(snapshot::SVC_SNAPSHOT);
            snapshot::write(&mut w, self.proto, base.as_ref(), &frame, &self.baselines);
            c.record_frame(frame);

            let ops = w.into_ops();

            if self.cfg.trace {
                // The prediction contract: a client replays every usercmd
                // newer than ps.commandTime, so `lead` at or below zero means
                // we claim to have simulated past our own frame clock and the
                // client has nothing left to replay. Retail's captures run a
                // lead of 0..34 ms, never negative (examples/snapshot_timing).
                let lead = self.sv_time_ms - command_time;
                let queued = c.pending.len();
                let m = moved.get(slot).copied().unwrap_or_default();
                let processed = m.processed;
                let span = match (m.first_cmd_st, m.last_cmd_st) {
                    (Some(a), Some(b)) => format!("{a}..{b}"),
                    _ => "-".into(),
                };
                let ack_behind = message_num as i64 - i64::from(c.message_ack);
                let base_desc = match base.as_ref() {
                    Some(b) => format!("d{}", message_num - b.message_num),
                    None => "uncompressed".into(),
                };
                // The walker's own state, for chasing a prediction error a
                // client reports against a headless run that cannot see it.
                let walk = c.sim.as_ref().map_or(String::new(), |s| {
                    format!(
                        " z {:.2} vz {:.1} ground {} ads {} frac {:.3}",
                        s.ps.origin.z,
                        s.ps.velocity.z,
                        u8::from(s.ps.on_ground),
                        u8::from(s.ps.ads_active),
                        s.ps.weapon_pos_frac
                    )
                });
                log::info!(
                    "trace c{slot} msg {message_num} wall {} sv {} ct {} lead {} \
cmds {processed} span {span} queued {queued} ack {} behind {ack_behind} {base_desc} {} B{walk}",
                    wall_ms.map_or("-".into(), |w| format!("{w:.1}")),
                    self.sv_time_ms,
                    command_time,
                    lead,
                    c.message_ack,
                    ops.len(),
                );
            }
            for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
                self.outbox.push((c.addr, pkt));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use vcod_common::collision::test_world;
    use vcod_common::net::connectionless::{
        build_connect, build_oob, info_value_for_key, parse_oob,
    };
    use vcod_common::net::msg::write_delta_usercmd;
    use vcod_common::net::netchan::Netchan;
    use vcod_common::net::snapshot::{SnapshotRing, SVC_SNAPSHOT};

    const QPORT: u16 = 0x2001;

    fn cfg() -> ServerConfig {
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
    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }
    /// Installs a hand-built runtime the way `load_scripts` does: the script
    /// owns the configstring table from there on, so it starts from the
    /// server's rather than from `for_test`'s empty one, and the server's
    /// copy is taken back through the cvar mirror. Without it the first tick
    /// reads every static slot as cleared and broadcasts the difference.
    fn install_script(sv: &mut Server, mut rt: crate::game::script::ScriptRuntime) {
        for (i, cs) in sv.configstrings.iter().enumerate() {
            if rt.host.configstrings[i].is_empty() {
                rt.host.configstrings[i] = cs.clone();
            }
        }
        let mut cs = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut cs)
            .expect("the cvar mirror fits");
        sv.configstrings = cs;
        sv.sync_sent_configstrings();
        sv.script = Some(rt);
    }
    fn oob(cmd: &str) -> Vec<u8> {
        build_oob(cmd)
    }
    fn reply(sv: &mut Server) -> (SocketAddr, String, Vec<u8>) {
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one packet");
        let (to, pkt) = out.remove(0);
        let (cmd, rest) = parse_oob(&pkt).expect("oob reply");
        (to, cmd.to_string(), rest.to_vec())
    }
    fn reply_text(sv: &mut Server) -> (String, String) {
        let (_, cmd, rest) = reply(sv);
        (cmd, String::from_utf8_lossy(&rest).trim().to_string())
    }
    fn challenge_for(sv: &mut Server, from: SocketAddr, now: Instant) -> i32 {
        sv.handle_packet(from, &oob("getchallenge"), now);
        let (cmd, body) = reply_text(sv);
        assert_eq!(cmd, "challengeResponse");
        body.parse().expect("a numeric challenge")
    }
    fn connect_pkt(challenge: i32, qport: u16, protocol: u32) -> Vec<u8> {
        build_connect(&format!(
            "\\name\\vcod\\protocol\\{protocol}\\qport\\{qport}\\challenge\\{challenge}"
        ))
    }
    fn connected(sv: &mut Server, from: SocketAddr, now: Instant) -> Netchan {
        let challenge = challenge_for(sv, from, now);
        sv.handle_packet(
            from,
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(reply_text(sv).0, "connectResponse");
        Netchan::new(QPORT, challenge)
    }
    fn server_commands(nc: &mut Netchan, pkt: &[u8], huff: &Huffman) -> Vec<String> {
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

    #[test]
    fn getinfo_echoes_the_challenge_and_names_the_map() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo 4242"), now);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("4242"));
        assert_eq!(info_value_for_key(&info, "mapname"), Some("mp_carentan"));
        assert_eq!(info_value_for_key(&info, "protocol"), Some("1"));
        assert_eq!(info_value_for_key(&info, "clients"), Some("0"));
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("4"));
        assert_eq!(info_value_for_key(&info, "gametype"), Some("dm"));
        assert_eq!(info_value_for_key(&info, "pswrd"), Some("0"));
        // Key order is retail's; pin the whole string.
        assert_eq!(
            info,
            "\\challenge\\4242\\protocol\\1\\hostname\\vcod test\\mapname\\mp_carentan\\clients\\0\\sv_maxclients\\4\\gametype\\dm\\pure\\0\\sv_allowAnonymous\\0\\pswrd\\0"
        );
    }

    #[test]
    fn getinfo_sanitizes_the_challenge_argument() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo a\\pure\\1 extra"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("apure1"));
        assert_eq!(info_value_for_key(&info, "pure"), Some("0"));
    }

    #[test]
    fn getstatus_has_serverinfo_and_no_player_lines() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let text = String::from_utf8_lossy(&rest);
        let (info, players) = text.split_once('\n').unwrap();
        assert_eq!(info_value_for_key(info, "sv_hostname"), Some("vcod test"));
        assert_eq!(info_value_for_key(info, "challenge"), Some("7"));
        assert_eq!(players, "");
    }

    #[test]
    fn getchallenge_is_stable_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, cmd, a) = reply(&mut sv);
        assert_eq!(cmd, "challengeResponse");
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, _, b) = reply(&mut sv);
        assert_eq!(a, b);
        sv.handle_packet(addr(6), &oob("getchallenge"), now);
        let (_, _, c) = reply(&mut sv);
        assert_ne!(a, c);
        assert!(String::from_utf8_lossy(&a).trim().parse::<i32>().is_ok());
    }

    /// `mr <serverId> <menuIndex> <response>`, exactly four arguments.
    /// Retail's stale-serverId drop is the one this reproduces exactly; the
    /// other three shapes it drops, retail turns into a notify no stock
    /// gametype tests (see `parse_menu_response`). The response comes back
    /// verbatim: the gametype compares it against `"allies"` and weapon
    /// names, so anything done to it here would be done behind the script's
    /// back.
    #[test]
    fn mr_needs_four_args_a_live_serverid_and_a_bounded_index() {
        assert_eq!(
            parse_menu_response("mr 7 3 allies", 7),
            Some((3, "allies".to_string()))
        );
        assert_eq!(
            parse_menu_response("mr 7 0 mosin_nagant_mp", 7),
            Some((0, "mosin_nagant_mp".to_string())),
            "a weapon response is a string, not a token the server knows"
        );
        assert!(
            parse_menu_response("mr 6 3 allies", 7).is_none(),
            "stale serverId"
        );
        assert!(
            parse_menu_response("mr 7 32 allies", 7).is_none(),
            "index out of range"
        );
        assert!(parse_menu_response("mr 7 3", 7).is_none(), "three args");
        assert!(
            parse_menu_response("mr 7 3 allies extra", 7).is_none(),
            "five args"
        );
    }

    #[test]
    fn serverinfo_is_configstring_zero() {
        let sv = Server::new(cfg(), Instant::now());
        let cs0 = sv.configstring(0);
        assert_eq!(info_value_for_key(cs0, "mapname"), Some("mp_carentan"));
        assert_eq!(
            info_value_for_key(sv.configstring(1), "sv_serverid"),
            Some("16")
        );
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    /// The `weaponClass` table the animation machine reads every frame, and
    /// the seam it is built across: `WEAPON_LIST` is 1-based on the wire, so
    /// `items::item_name` subtracts one to name it. Misaligned by one, every
    /// pistol player animates as a rifleman and nothing says so. Needs the
    /// paks -- the classes come from the weapon files.
    #[test]
    fn the_weapon_class_table_lines_up_with_the_wire_index() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut sv = Server::new(cfg(), Instant::now());
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for (weapon, class) in [
            ("colt_mp", "pistol"),
            ("thompson_mp", "smg"),
            ("m1carbine_mp", "rifle"),
        ] {
            let index = crate::configstrings::weapon_index(weapon).expect("in WEAPON_LIST");
            assert_eq!(
                sv.weapon_table.class(index),
                class,
                "{weapon} at wire index {index}"
            );
            // The same index the frame loop hands the animscript.
            assert_eq!(crate::items::item_name(index), Some(weapon));
        }
        assert_eq!(
            sv.weapon_table.class(0),
            "",
            "slot 0 is the wire's no-weapon"
        );
    }

    /// A failed load reports the error and leaves the table untouched; the
    /// caller (`main.rs`) exits on it. Stage 2 kept serving here, which is no
    /// longer the right answer: the table is mostly script output now.
    #[test]
    fn a_failed_script_load_reports_and_changes_nothing() {
        let mut sv = Server::new(cfg(), Instant::now());
        let before: Vec<String> = sv.configstrings.clone();
        // No paks, so the map script does not resolve and `load` fails at its
        // first step.
        let err = sv.load_scripts(Rc::new(vcod_common::pk3::Pk3Fs::empty()));
        assert!(err.is_err());
        assert_eq!(sv.configstrings, before);
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    /// The script keeps allocating configstrings after map load, from any
    /// thread that has passed a `wait`. The server re-reads the script's
    /// table each tick, so such an allocation reaches it.
    #[test]
    fn a_configstring_allocated_after_a_wait_reaches_the_server() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.5; loadfx(\"fx/impacts/newimps/minefield.efx\"); }",
            ),
        );
        // sv_time starts at 0 and each tick advances it one 50 ms frame, so
        // the thread is still suspended after the first.
        sv.tick(now);
        assert_eq!(sv.configstring(781), "");
        for _ in 0..12 {
            sv.tick(now);
        }
        assert_eq!(sv.configstring(781), "fx/impacts/newimps/minefield.efx");
    }

    /// `--set g_gametype` reaches the serverinfo configstring, not only the
    /// script's cvar table: retail's cvar flush follows every serverinfo
    /// write with `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (map-cycle doc 4.3), and `main.rs` replays the `--set` list after
    /// `Server::new` has already stamped the table.
    #[test]
    fn a_set_of_a_serverinfo_cvar_reaches_configstring_0() {
        let mut sv = Server::new(cfg(), Instant::now());
        assert!(sv.configstring(0).contains("g_gametype\\dm"));
        sv.set_cvar("g_gametype", "sd");
        assert!(
            sv.configstring(0).contains("g_gametype\\sd"),
            "serverinfo: {:?}",
            sv.configstring(0)
        );
    }

    /// `SV_SetConfigstring` broadcasts a slot the running level changed to
    /// every client that already has its gamestate (map-cycle doc, 3.1 and
    /// 4.3). The announcer is what needs it: `playLocalSound` allocates its
    /// alias slot and then sends `s <idx>`, so a client that never heard the
    /// `d` reads an empty slot and plays nothing.
    #[test]
    fn a_configstring_allocated_mid_level_reaches_a_connected_client() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.5; p = getentarray(\"player\", \"classname\"); \
             p[0] playlocalsound(\"MP_announcer_allies_win\"); }",
            ),
        );
        let mut nc = begun(&mut sv, now);
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..16 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let d = seen
            .iter()
            .position(|c| c == "d 525 MP_announcer_allies_win")
            .unwrap_or_else(|| panic!("no `d 525` in {seen:?}"));
        let s = seen
            .iter()
            .position(|c| c == "s 1")
            .unwrap_or_else(|| panic!("no `s 1` in {seen:?}"));
        assert!(
            d < s,
            "the announcer's `s 1` came before its `d 525`: {seen:?}"
        );
    }

    /// A `CS_CONNECTED` client has no table to patch: its gamestate has not
    /// gone out, and the one it pulls carries every slot the level has
    /// allocated by then. So the per-frame diff skips it and sends to the
    /// clients that do. UNVERIFIED: whether retail gates its own broadcast
    /// per client this way; `SV_SetConfigstring`'s gate is on `sv.state`
    /// (map-cycle doc, 3.1) and nothing measured says what it does with a
    /// client below `CS_PRIMED`.
    #[test]
    fn the_configstring_broadcast_skips_a_client_with_no_gamestate() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let _nc = connected(&mut sv, addr(5), now);
        sv.sync_sent_configstrings();
        assert_eq!(
            sv.clients[0].as_ref().unwrap().state,
            ClientState::Connected
        );

        let seq = sv.clients[0].as_ref().unwrap().netchan.reliable_sequence;
        sv.configstrings[524] = "MP_announcer_allies_win".to_string();
        sv.broadcast_configstring_changes();
        assert_eq!(
            sv.clients[0].as_ref().unwrap().netchan.reliable_sequence,
            seq,
            "a client with no gamestate was sent a `d`"
        );

        // The same slot moving again once the gamestate has gone out does
        // reach it, so the skip above is the state and not the diff.
        sv.clients[0].as_mut().unwrap().state = ClientState::Primed;
        sv.configstrings[524] = "MP_announcer_axis_win".to_string();
        sv.broadcast_configstring_changes();
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!(c.netchan.reliable_sequence, seq + 1);
        assert_eq!(
            c.netchan.reliable[c.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1)],
            "d 524 MP_announcer_axis_win"
        );
    }

    /// Doc 3 step 20: the settle frames allocate into the table, and every
    /// client on the far side of a spawn pulls the whole of it in its own
    /// gamestate. So the copy the per-frame diff works against is re-synced
    /// after those frames, the way the restart path re-syncs after its own;
    /// without it the first tick after a map change queues a `d` per slot the
    /// three frames touched.
    #[test]
    fn a_spawn_re_syncs_the_table_the_broadcast_diffs_against() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "settle".into(),
                ..cfg()
            },
            now,
        );
        // A gametype whose settle frames allocate: the `wait` is due in the
        // first of the three.
        sv.overlay_script(
            "maps/mp/gametypes/settle",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             wait 0.05; precacheShader(\"gfx/hud/settle\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        // What the next tick reads back out of the script, which is what it
        // diffs the sent copy against.
        let rt = sv.script.as_ref().expect("the spawn left no script");
        let mut live = rt.configstrings().to_vec();
        rt.cvars().write_mirror(&mut live).expect("the mirror fits");
        assert!(
            live.iter().any(|s| s == "gfx/hud/settle"),
            "the settle frames allocated nothing, so this proves nothing"
        );
        let stale = |table: &[String]| -> Vec<usize> {
            (0..live.len())
                .filter(|&i| table.get(i) != live.get(i))
                .collect()
        };
        assert!(
            stale(&sv.sent_configstrings).is_empty(),
            "the spawn left slots {:?} for the diff to send",
            stale(&sv.sent_configstrings)
        );
        assert!(
            stale(&sv.configstrings).is_empty(),
            "the server's own copy is a frame behind the script at slots {:?}",
            stale(&sv.configstrings)
        );
    }

    /// `map_rotate`'s `gametype` token is a `Cvar_Set` (doc section 5.2), so
    /// it has to outrank an earlier `--set g_gametype`: the token picks which
    /// gametype script loads, and the cvar table is what that script's own
    /// `getCvar` answers from. Writing only `cfg.gametype` splits the two.
    #[test]
    fn a_rotation_gametype_token_outranks_an_earlier_set() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        sv.set_cvar("sv_mapRotation", "gametype tdm map mp_brecourt");
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs).expect("load the scripts");
        assert_eq!(sv.script_cvar("g_gametype").as_deref(), Some("dm"));

        sv.push_console("map_rotate");
        sv.tick(now);
        assert_eq!(sv.cfg.map, "mp_brecourt", "the rotation did not load");
        assert_eq!(sv.cfg.gametype, "tdm");
        assert!(
            sv.configstring(0).contains("g_gametype\\tdm"),
            "serverinfo: {:?}",
            sv.configstring(0)
        );
        assert_eq!(
            sv.script_cvar("g_gametype").as_deref(),
            Some("tdm"),
            "the rotated level's script still reads the value `--set` left"
        );
    }

    /// Doc section 4 step 3: a `g_gametype` that moved since the level
    /// loaded turns a restart into a full spawn of the same map, so the
    /// serverId's high nibble climbs where a restart would have moved the
    /// low one. A level that asked to keep its persistence is exempt.
    #[test]
    fn a_gametype_change_escalates_a_restart_to_a_spawn() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let mut now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(fs).expect("load the scripts");
        let before = sv.server_id;

        // No change yet: the plain restart path.
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(sv.server_id, console::next_restart_id(before));

        // The rotation's `gametype` token, then a restart: the level is
        // still running dm, so this escalates.
        let id = sv.server_id;
        sv.set_cvar("g_gametype", "tdm");
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.server_id,
            console::next_map_id(id),
            "a gametype change did not escalate to a spawn"
        );
        assert_eq!(
            sv.cfg.map, "mp_carentan",
            "the escalation moved off the map"
        );
        assert_eq!(sv.script_cvar("g_gametype").as_deref(), Some("tdm"));
    }

    /// A load that fails after the teardown leaves no level to serve and no
    /// console line that could bring one back, which is what retail ends the
    /// process for: the server records it and `main` exits on it. A load that
    /// fails before the teardown -- a map the paks do not have -- keeps the
    /// level that is serving and records nothing.
    #[test]
    fn a_script_load_that_fails_after_the_teardown_is_fatal() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");

        // `resolve_map` fails ahead of everything the spawn tears down.
        sv.push_console("map mp_nosuchmap");
        sv.tick(now);
        assert!(
            sv.take_fatal().is_none(),
            "a map the paks do not have took the server down"
        );
        assert_eq!(sv.cfg.map, "mp_carentan", "the failed map load moved off");

        // The rotation's `gametype` token names a gametype with no script,
        // and the `map` token behind it escalates to a spawn whose
        // `load_scripts_with` fails with the level already gone.
        now += Duration::from_millis(50);
        sv.set_cvar("sv_mapRotation", "gametype nosuch map mp_carentan");
        sv.push_console("map_rotate");
        sv.tick(now);
        let e = sv
            .take_fatal()
            .expect("a gametype with no script was survivable");
        assert!(format!("{e:#}").contains("nosuch"), "{e:#}");
        assert!(sv.take_fatal().is_none(), "the failure was reported twice");
    }

    /// Doc section 4 step 11: `SV_ClientEnterWorld` runs only for a client
    /// that reads `CS_ACTIVE`. A `CS_PRIMED` one keeps its state through
    /// the restart and is promoted by its own next message instead (4.4),
    /// which is the branch the second half exercises.
    #[test]
    fn a_primed_client_is_not_re_entered_by_a_restart() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = active(&mut sv, now);
        assert_eq!(sv.clients[0].as_ref().unwrap().state, ClientState::Primed);

        sv.map_restart().expect("the restart failed");
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!(
            c.state,
            ClientState::Primed,
            "the restart entered a primed client"
        );
        assert!(
            c.sim.is_none(),
            "a primed client came out of the restart with a sim"
        );

        // Its next message still carries the old serverId: same high nibble,
        // differing low one, which is the promotion branch.
        let huff = Huffman::new();
        let ack = nc.incoming_sequence as i32;
        let pkt = nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!(c.state, ClientState::Active);
        assert!(c.sim.is_some());
    }

    /// The other half of step 3: a `+set` that names one of the two cvars is
    /// not a change. `cvars` replays the override list after stamping the
    /// config's own value, so `--max-clients 4 --set sv_maxclients=8` loads
    /// the level with 8; comparing that against the config's 4 would escalate
    /// every restart for the whole run.
    #[test]
    fn a_set_override_of_sv_maxclients_is_not_a_gametype_change() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        assert_eq!(sv.cfg.max_clients, 4);
        sv.set_cvar("sv_maxclients", "8");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("sv_maxclients").as_deref(), Some("8"));

        let before = sv.server_id;
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.server_id,
            console::next_restart_id(before),
            "the override escalated a restart to a spawn"
        );
    }

    /// Doc section 4 step 1: a second restart inside one frame is a no-op,
    /// and so is one straight after a spawn.
    #[test]
    fn a_second_restart_in_one_frame_is_a_no_op() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let before = sv.server_id;
        sv.push_console("map_restart");
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.server_id,
            console::next_restart_id(before),
            "the second restart in the frame was not a no-op"
        );
    }

    #[test]
    fn a_valid_connect_takes_a_slot() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.version),
            now,
        );
        let (to, cmd, _) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    /// A reconnect overwrites a slot the server may still believe is live --
    /// `TIMEOUT` is 240 s, `RECONNECT_LIMIT` 3 -- and the new `Client` carries
    /// a fresh `last_packet`, so `check_timeouts` will never reach the old
    /// one. Without the queued `Disconnect` nothing tears its script state
    /// down and it leaks for the life of the map.
    #[test]
    fn a_reconnect_disconnects_the_client_it_replaces() {
        use crate::game::script::{ScriptRuntime, CALLBACK_SETUP};
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            ScriptRuntime::for_test_at(
                CALLBACK_SETUP,
                "main() { level.gone = 0; level.callbackPlayerDisconnect = ::d; }\n\
             CodeCallback_PlayerDisconnect() { [[level.callbackPlayerDisconnect]](); }\n\
             d() { level.gone = level.gone + 1; }\n",
            ),
        );
        let nc = connected(&mut sv, addr(5), now);
        sv.tick(now);
        let gone = |sv: &mut Server| sv.script.as_mut().unwrap().level_field("gone");
        assert_eq!(gone(&mut sv), vcod_gsc::Value::Int(0));

        // Past sv_reconnectlimit, the same peer's connect takes the slot back.
        let t = now + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(
            addr(5),
            &connect_pkt(nc.challenge, QPORT, PROTOCOL_V1.version),
            t,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        sv.tick(t);
        assert_eq!(gone(&mut sv), vcod_gsc::Value::Int(1));
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_connect_on_the_wrong_protocol_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 6), now);
        assert_eq!(
            reply_text(&mut sv),
            (
                "error".to_string(),
                "EXE_SERVER_IS_DIFFERENT_VER 1.1".to_string()
            )
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_connect_with_a_challenge_we_never_issued_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(challenge.wrapping_add(1), QPORT, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_full_server_rejects_the_next_connect() {
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                max_clients: 1,
                ..cfg()
            },
            now,
        );
        connected(&mut sv, addr(5), now);
        assert_eq!(sv.client_count(), 1);

        // Neither qport nor port matches, so not a reconnect of the first client.
        let challenge = challenge_for(&mut sv, addr(6), now);
        sv.handle_packet(
            addr(6),
            &connect_pkt(challenge, QPORT + 1, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_SERVERISFULL".to_string())
        );
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_message_with_an_out_of_range_reliable_acknowledge_is_ignored() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let ops = w.into_ops();

        // serverId 0 asks for a gamestate; only the ack makes this inadmissible.
        let pkt = nc.build_out(0, 0, i32::MIN, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(sv.take_outgoing().is_empty(), "bogus ack got a reply");

        let pkt = nc.build_out(0, 0, 1, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(sv.take_outgoing().is_empty(), "ack ahead of us got a reply");

        // messageAcknowledge names a message we sent; we have sent none.
        let pkt = nc.build_out(0, 1, 0, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            sv.take_outgoing().is_empty(),
            "messageAcknowledge ahead of us got a reply"
        );

        let pkt = nc.build_out(0, 0, 0, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate for a good ack"
        );
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_gap_in_the_client_command_sequence_drops_the_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();

        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(2); // skips sequence 1, which we never sent
        w.write_string("say hi");
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(i32::from(sv.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);

        assert_eq!(sv.client_count(), 0);
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected the drop notice");
        let (to, pkt) = out.remove(0);
        assert_eq!(to, addr(5));
        assert_eq!(
            server_commands(&mut nc, &pkt, &huff),
            vec!["w \"EXE_LOSTRELIABLECOMMANDS\"".to_string()]
        );
    }

    /// Score requests pipelined faster than the client acks their replies
    /// would wrap the reliable ring and overwrite unsacked slots, corrupting
    /// the wire and the scramble key. Past 64 unacked commands the client is
    /// dropped instead, and the slot works for a fresh connect. The whole
    /// burst rides one message: a later message would fail the incoming
    /// ack-range check before any op ran.
    #[test]
    fn score_spam_past_the_reliable_ring_drops_the_client() {
        let now = Instant::now();
        let huff = Huffman::new();
        let scores = |sv: &mut Server, nc: &mut Netchan, first: i32, last: i32| {
            let mut w = MsgWriter::new(&huff);
            for seq in first..=last {
                w.write_bits(CLC_CLIENT_COMMAND, 2);
                w.write_long(seq);
                w.write_string("score");
            }
            w.write_bits(CLC_EOF, 2);
            sv.handle_packet(
                addr(5),
                &nc.build_out(
                    i32::from(sv.server_id),
                    nc.incoming_sequence as i32,
                    0,
                    &w.into_ops(),
                    &huff,
                )
                .unwrap(),
                now,
            );
        };

        // Exactly the ring's worth of unacked commands is not yet fatal.
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        // Each reply is scrambled with the last client command string, so the
        // receiving netchan needs its own sent-command ring filled (reply 1
        // went out before any command was stored and stays undecodable here).
        for s in 2..=70 {
            nc.reliable[(s as usize) & 63] = "score".to_string();
        }
        scores(&mut sv, &mut nc, 1, 64);
        assert_eq!(sv.client_count(), 1, "a full ring is not yet fatal");
        assert!(
            !sv.take_outgoing().is_empty(),
            "the ring-full boundary must still answer"
        );

        // One pipelined request too many drops the client, notice last.
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        for s in 2..=70 {
            nc.reliable[(s as usize) & 63] = "score".to_string();
        }
        scores(&mut sv, &mut nc, 1, 65);
        assert_eq!(sv.client_count(), 0);
        let mut notices = Vec::new();
        for (_, pkt) in sv.take_outgoing() {
            let res = nc.process_in(&pkt, &huff);
            if let Ok(Some(msg)) = res {
                let mut r = MsgReader::new(&msg[4..], &huff);
                while !r.is_overflowed() {
                    match r.read_byte() {
                        msg::SVC_SERVER_COMMAND => {
                            r.read_long();
                            notices.push(r.read_big_string());
                        }
                        _ => break,
                    }
                }
            }
        }
        assert_eq!(
            notices.last().map(String::as_str),
            Some("w \"EXE_LOSTRELIABLECOMMANDS\"")
        );

        // The freed slot takes a fresh client.
        let t2 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        connected(&mut sv, addr(5), t2);
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_score_request_gets_a_deathmatch_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();

        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        w.write_string("score");
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(
                i32::from(sv.server_id),
                nc.incoming_sequence as i32,
                0,
                &w.into_ops(),
                &huff,
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);

        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one reply frame");
        assert_eq!(out[0].0, addr(5));
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].starts_with("b 1 0 0 0 0 0 0 0"), "{:?}", cmds[0]);
    }

    /// The scoreboard's tokens 2 and 3 are the two team scores, axis
    /// before allies, and `setTeamScore` mirrors each into configstring 5
    /// and 6 (map-cycle doc, 6.3).
    #[test]
    fn the_scoreboard_carries_the_team_scores_axis_first() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { setTeamScore(\"axis\", 3); setTeamScore(\"allies\", -9999); }",
            ),
        );
        assert_eq!(sv.scoreboard(), "b 0 3 -9999");
        let cs = sv.script.as_ref().unwrap().configstrings();
        assert_eq!((cs[5].as_str(), cs[6].as_str()), ("3", "-9999"));
    }

    fn count_replies(sv: &mut Server, to: SocketAddr) -> usize {
        sv.take_outgoing().iter().filter(|(a, _)| *a == to).count()
    }

    #[test]
    fn getstatus_is_rate_limited_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let a = SocketAddr::from(([10, 0, 0, 1], 5));
        let b = SocketAddr::from(([10, 0, 0, 2], 5));
        for _ in 0..ADDR_BURST + 1 {
            sv.handle_packet(a, &oob("getstatus x"), now);
        }
        assert_eq!(count_replies(&mut sv, a), ADDR_BURST as usize);
        // One global period on, so the burst above is not what limits b.
        let t1 = now + GLOBAL_PERIOD;
        sv.handle_packet(b, &oob("getstatus x"), t1);
        assert_eq!(count_replies(&mut sv, b), 1);
        // The same bucket covers getinfo; one token comes back per period.
        sv.handle_packet(a, &oob("getinfo x"), t1);
        assert_eq!(count_replies(&mut sv, a), 0);
        let t2 = now + ADDR_PERIOD;
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 1);
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 0);
    }

    #[test]
    fn connectionless_replies_share_a_global_bucket() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut answered = 0;
        for i in 0..GLOBAL_BURST + 5 {
            let from = SocketAddr::from(([10, 1, (i >> 8) as u8, i as u8], 5));
            sv.handle_packet(from, &oob("getchallenge"), now);
            answered += count_replies(&mut sv, from);
        }
        assert_eq!(answered, GLOBAL_BURST as usize);
        let late = SocketAddr::from(([10, 2, 0, 1], 5));
        sv.handle_packet(late, &oob("getchallenge"), now + GLOBAL_PERIOD);
        assert_eq!(count_replies(&mut sv, late), 1);
    }

    #[test]
    fn the_address_table_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        for i in 0..MAX_BUCKETS as u32 + 8 {
            let from = SocketAddr::from(([10, (i >> 16) as u8, (i >> 8) as u8, i as u8], 5));
            // Spread over time so the global bucket stays open and the
            // eviction order is defined.
            sv.handle_packet(from, &oob("getinfo x"), now + GLOBAL_PERIOD * i);
        }
        assert_eq!(sv.limiter.addrs.len(), MAX_BUCKETS);
        // The least recently seen address went first.
        assert!(!sv
            .limiter
            .addrs
            .contains_key(&std::net::IpAddr::from([10, 0, 0, 0])));
    }

    #[test]
    fn oob_disconnect_honours_the_address_limit() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        connected(&mut sv, addr(5), now);
        // The challenge took one token; spend the rest on getinfo.
        for _ in 1..ADDR_BURST {
            sv.handle_packet(addr(5), &oob("getinfo x"), now);
        }
        sv.take_outgoing();
        sv.handle_packet(addr(5), &oob("disconnect"), now);
        assert_eq!(
            sv.client_count(),
            1,
            "a rate-limited disconnect went through"
        );
        sv.handle_packet(addr(5), &oob("disconnect"), now + ADDR_PERIOD);
        assert_eq!(sv.client_count(), 0);
    }

    /// A forged packet carrying the victim's ip and qport must not advance the
    /// netchan, move the address or refresh the timeout; the real client's
    /// next packet still goes through.
    #[test]
    fn a_spoofed_packet_leaves_the_client_untouched() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut eof = MsgWriter::new(&huff);
        eof.write_bits(CLC_EOF, 2);
        let eof = eof.into_ops();
        let later = now + Duration::from_secs(5);
        let spoofer = addr(6);
        let mut forged = Netchan::new(QPORT, nc.challenge);
        let snapshot = |sv: &Server| {
            let c = sv.clients[0].as_ref().unwrap();
            (c.netchan.incoming_sequence, c.addr, c.last_packet)
        };
        let before = snapshot(&sv);

        // Header checks: an ack we never sent.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged.build_out(0, 0, 1, &eof, &huff).unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a bad ack committed state");

        // A stale serverId with nothing to resend.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged
            .build_out(i32::from(sv.server_id) ^ 0x01, 0, 0, &eof, &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a stale serverId committed state");

        // Right header, ops that end inside a command.
        forged.outgoing_sequence = 0x7fff_fffe;
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        let pkt = forged
            .build_out(i32::from(sv.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(
            snapshot(&sv),
            before,
            "a truncated op stream committed state"
        );
        assert!(sv.take_outgoing().is_empty());

        // The legitimate client's first message, sequence 1, asks for the gamestate.
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, later);
        assert_eq!(snapshot(&sv), (1, addr(5), later));
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate after the spoofs"
        );
    }

    #[test]
    fn a_foreign_challenge_cannot_replace_a_live_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let eof = w.into_ops();
        let own = nc.challenge;

        // The client is heard from just before the attempt.
        let t1 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, t1);
        sv.take_outgoing();

        // A neighbour behind the same NAT holds a valid challenge for this ip
        // and knows the qport. Past sv_reconnectlimit, retail would hand it the slot.
        let foreign = challenge_for(&mut sv, addr(7), t1);
        let t2 = t1 + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.version),
            t2,
        );
        assert!(sv.take_outgoing().is_empty(), "the takeover got a reply");
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!((c.addr, c.netchan.challenge), (addr(5), own));

        // The client's own connect retry, same challenge, still resets its slot.
        sv.handle_packet(addr(5), &connect_pkt(own, QPORT, PROTOCOL_V1.version), t2);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);

        // Once the slot has been silent for sv_reconnectlimit, a crashed client
        // coming back with a new challenge may reclaim it.
        let t3 = t2 + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.version),
            t3,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!((c.addr, c.netchan.challenge), (addr(7), foreign));
    }

    #[test]
    fn stale_challenges_expire_when_a_new_one_is_issued() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let old = challenge_for(&mut sv, addr(5), now);
        // Asking again refreshes the entry rather than issuing a new one.
        let t1 = now + CHALLENGE_TTL - Duration::from_secs(1);
        assert_eq!(challenge_for(&mut sv, addr(5), t1), old);
        let t2 = now + CHALLENGE_TTL + Duration::from_secs(1);
        challenge_for(&mut sv, addr(6), t2);
        assert_eq!(sv.challenges.len(), 2, "a refreshed entry was expired");

        let t3 = t1 + CHALLENGE_TTL + Duration::from_secs(1);
        challenge_for(&mut sv, addr(8), t3);
        assert_eq!(sv.challenges.len(), 2);
        assert!(sv.challenges.iter().all(|c| c.addr != addr(5)));
        sv.handle_packet(addr(5), &connect_pkt(old, QPORT, PROTOCOL_V1.version), t3);
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
    }

    /// clc_move ops carrying one usercmd, keyed the way the server decodes it.
    fn move_ops(checksum_feed: i32, message_ack: i32, cmd: UserCmd) -> Vec<u8> {
        move_ops_cmds(checksum_feed, message_ack, &[cmd])
    }

    /// clc_move ops carrying several usercmds, chained as a real client
    /// chains them: each deltas from its predecessor.
    fn move_ops_cmds(checksum_feed: i32, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
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
    struct MoveChain {
        checksum_feed: i32,
        prev: UserCmd,
    }

    impl MoveChain {
        fn new(checksum_feed: i32) -> Self {
            MoveChain {
                checksum_feed,
                prev: NULL_USERCMD,
            }
        }

        fn ops(&mut self, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
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
    /// clc_move carrying one cmd whose pitch and yaw ride change bit 0
    /// (omitted, unchanged from the server's stored cmd), what a retail client
    /// sends when the mouse did not move this packet. Forward/right stay
    /// announced; each serverTime is absolute so the test isolates the angle
    /// base from the serverTime base. `n` cmds, 20 ms apart from `st`: one is
    /// enough to prove the field decodes, but a probe of *which* base it
    /// decoded against needs the resulting velocity to actually get there --
    /// `pmove::spectator_move`'s `accelerate` blends toward the wishdir
    /// rather than snapping to it, so a single cmd's movement is still mostly
    /// the previous leg's residual velocity. A burst gives the (correct or
    /// wrongly-reset) wishdir enough simulated time to dominate.
    fn move_ops_omit_angles(checksum_feed: i32, message_ack: i32, st: i32, n: i32) -> Vec<u8> {
        let huff = Huffman::new();
        let key = checksum_feed ^ message_ack ^ com_hash_key("", 32);
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(n as u8);
        for i in 0..n {
            let cmd_st = st + i * 20;
            w.write_bits(0, 1); // serverTime: 32-bit absolute
            w.write_long(cmd_st);
            // Not the whole-cmd shortcut, and the branch bit picks the compact one.
            w.write_bits((key & 1) ^ 1, 1);
            w.write_bits(key & 1, 1);
            let ckey = key ^ cmd_st;
            w.write_bits(ckey & 1, 1); // buttons bit 0, announced as 0
            w.write_bits(0, 1); // pitch omitted
            w.write_bits(0, 1); // yaw omitted
            w.write_bits(1, 1); // forward/right announced
            w.write_bits(1 ^ (ckey & 0xf), 4); // forward 127: bucket 1
        }
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// clc_move whose count byte is past the usercmd cap: the header decodes,
    /// the move block cannot.
    fn garbled_move_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(u8::MAX);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Ack-only ops: an empty command/move run still ends in clc_EOF, which is
    /// what `parse_client_ops` demands before it commits anything.
    fn ack_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Connect, ask for the gamestate (a fresh client sends serverId 0),
    /// drain it, ack past it. Returns the live client netchan; slot 0 is
    /// Primed, and stays there until its first usercmd.
    fn active(sv: &mut Server, now: Instant) -> Netchan {
        let huff = Huffman::new();
        let mut nc = connected(sv, addr(5), now);
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff).unwrap();
        }
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap(),
            now,
        );
        nc
    }

    /// `active` plus the first usercmd, which is what puts a client in the
    /// world, so slot 0 has a sim and is sent snapshots. A real client sends
    /// no command to get here.
    fn begun(sv: &mut Server, now: Instant) -> Netchan {
        let huff = Huffman::new();
        let mut nc = active(sv, now);
        let ack = nc.incoming_sequence as i32;
        let ops = move_ops(sv.checksum_feed, ack, NULL_USERCMD);
        let pkt = nc
            .build_out(i32::from(sv.server_id), ack, 0, &ops, &huff)
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        nc
    }

    /// Tick once and parse the newest queued frame as a client would.
    fn latest_snapshot(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
    ) -> vcod_common::net::snapshot::Snapshot {
        let huff = Huffman::new();
        let p = &PROTOCOL_V1;
        sv.tick(now);
        let mut snap = None;
        for (_, pkt) in sv.take_outgoing() {
            if let Ok(Some(msgbytes)) = nc.process_in(&pkt, &huff) {
                let num = nc.incoming_sequence;
                let mut r = MsgReader::new(&msgbytes[4..], &huff);
                while !r.is_overflowed() {
                    if r.read_byte() == SVC_SNAPSHOT {
                        snap = Some(ring.parse_into(&mut r, p, num).unwrap().clone());
                        break;
                    }
                }
            }
        }
        snap.expect("a snapshot arrived")
    }

    /// `ps.commandTime` must name a usercmd we actually simulated, never a
    /// later moment. A client replays every cmd past commandTime, so claiming
    /// to have simulated further than we have silently drops that slice of
    /// its input on every frame and its prediction judders (retail trace,
    /// 2026-08-28: commandTime ran 11-24 ms ahead on 100% of frames).
    #[test]
    fn command_time_never_runs_ahead_of_the_last_simulated_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let p = &PROTOCOL_V1;

        // A client whose cmd clock trails the server's, as a real one's does:
        // it runs its own serverTime behind so it has frames to interpolate.
        let mut at = now;
        let mut cmd_time = 0i32;
        for _ in 0..8 {
            at += std::time::Duration::from_millis(50);
            cmd_time += 40; // 10 ms per frame behind the server's 50
            let ack = nc.incoming_sequence as i32;
            sv.handle_packet(
                addr(5),
                &nc.build_out(
                    0x10,
                    ack,
                    0,
                    &move_ops(
                        sv.checksum_feed,
                        ack,
                        UserCmd {
                            server_time: cmd_time,
                            forward: 127,
                            ..Default::default()
                        },
                    ),
                    &Huffman::new(),
                )
                .unwrap(),
                at,
            );
            let s = latest_snapshot(&mut sv, &mut nc, &mut ring, at);
            let ct = s.ps.field_i32(p, "commandTime");
            assert!(
                ct <= cmd_time,
                "commandTime {ct} is ahead of the newest cmd we simulated ({cmd_time})"
            );
        }
    }

    #[test]
    fn a_client_in_the_world_receives_snapshots_and_flies_forward() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // A flat floor to fly over; without a world the sim freezes.
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);

        let mut ring = SnapshotRing::new();
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, now);
        assert!(s.valid && s.delta_num == -1);
        let p = &PROTOCOL_V1;
        assert_eq!(s.ps.field_i32(p, "pm_type"), 4);
        assert_eq!(s.clients[&0].name(p), "vcod");

        // Forward nudge; yaw 0 faces +X.
        let later = now + std::time::Duration::from_millis(50);
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 50,
                        forward: 127,
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, later);
        let moved = s2.ps.origin(p)[0] - s.ps.origin(p)[0];
        assert!(moved > 1.0, "should have flown +X, dx {moved}");
    }

    /// Entry takes its `delta_angles` from the cmd that entered the world,
    /// not from zero. `SV_ClientEnterWorld` stores `cmds[0]` in
    /// `lastUsercmd`, and `ClientSpawn` reads it back through
    /// `trap_GetUsercmd` into `pers.cmd` on the line before
    /// `SetClientViewAngle` subtracts it (RTCW-MP `g_client.c`, `sv_game.c`).
    /// A client that was already looking somewhere when it entered keeps that
    /// view; zeroing the pair snaps it, and the move basis rotates with it.
    /// Every other entry in the suite enters with zero angles, so this is the
    /// only thing holding the argument in place.
    #[test]
    fn entry_takes_delta_angles_from_the_entering_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 90.0),
        });
        let mut nc = active(&mut sv, now);

        // Pitch 22.5 degrees, yaw 45: a client that looked around on the
        // loading screen before its first cmd went out.
        let entering = UserCmd {
            server_time: 50,
            angles: [4096, 8192, 0],
            ..Default::default()
        };
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(sv.checksum_feed, ack, entering),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );

        let mut ring = SnapshotRing::new();
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, now);
        let p = &PROTOCOL_V1;
        // `ANGLE2SHORT(90) - cmd.angles[1]` on the yaw, `-cmd.angles[i]` on
        // the other two, all as the 16-bit wire words the field carries.
        assert_eq!(s.ps.field_i32(p, "delta_angles[0]"), -4096 & 0xffff);
        assert_eq!(s.ps.field_i32(p, "delta_angles[1]"), 16_384 - 8192);
        assert_eq!(s.ps.field_i32(p, "delta_angles[2]"), 0);
    }

    /// Entry is the first usercmd after the gamestate, so every later usercmd
    /// runs past a client that is already in the world; re-entering one would
    /// park its sim back at the spawn mid-flight. The `Primed` guard is all
    /// that stops it.
    #[test]
    fn a_later_move_leaves_a_flying_client_where_it_is() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let p = &PROTOCOL_V1;
        latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Two messages, so the base has to chain across them the way the
        // client's does; `begun` entered on a null cmd, which is where a
        // fresh chain starts.
        let mut chain = MoveChain::new(sv.checksum_feed);
        let later = now + std::time::Duration::from_millis(50);
        let huff = Huffman::new();
        let ack = nc.incoming_sequence as i32;
        let ops = chain.ops(
            ack,
            &[UserCmd {
                server_time: 50,
                forward: 127,
                ..Default::default()
            }],
        );
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ops, &huff).unwrap(),
            now,
        );
        let flying = latest_snapshot(&mut sv, &mut nc, &mut ring, later);
        assert!(
            flying.ps.origin(p)[0] > 1.0,
            "the client never left the spawn"
        );

        let ack = nc.incoming_sequence as i32;
        let ops = chain.ops(
            ack,
            &[UserCmd {
                server_time: 100,
                forward: 127,
                ..Default::default()
            }],
        );
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ops, &huff).unwrap(),
            later,
        );
        let after = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            later + std::time::Duration::from_millis(50),
        );
        assert!(
            after.ps.origin(p)[0] >= flying.ps.origin(p)[0],
            "a later move teleported the client back: {} -> {}",
            flying.ps.origin(p)[0],
            after.ps.origin(p)[0]
        );
    }

    /// A block whose mouse turns mid-flight: per-cmd replay must cover both
    /// headings. The old last-cmd-only step integrated once at yaw 90 and
    /// never moved along +X.
    #[test]
    fn turning_cmds_integrate_per_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let t0 = now;
        let t1 = now + std::time::Duration::from_millis(50);
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, t0);

        // Forward at yaw 0 (faces +X), then the mouse swings to yaw 90 (+Y).
        let ack = nc.incoming_sequence as i32;
        let cmds = [
            UserCmd {
                server_time: 50,
                forward: 127,
                angles: [0, 0, 0],
                ..Default::default()
            },
            UserCmd {
                server_time: 100,
                forward: 127,
                angles: [0, 16384, 0],
                ..Default::default()
            },
        ];
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_cmds(sv.checksum_feed, ack, &cmds),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s.ps.origin(&PROTOCOL_V1);
        let o2 = s2.ps.origin(&PROTOCOL_V1);
        assert!(
            o2[0] - o1[0] > 2.0,
            "cmd A should have flown +X, dx {}",
            o2[0] - o1[0]
        );
        assert!(
            o2[1] - o1[1] > 2.0,
            "cmd B should have flown +Y, dy {}",
            o2[1] - o1[1]
        );
    }

    /// A cmd from before the processed clock is stale: skipped whole, no
    /// motion, no panic.
    #[test]
    fn stale_cmds_are_skipped() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let mut tick_at = now + std::time::Duration::from_millis(50);

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert!(
            s1.ps.origin(&PROTOCOL_V1)[0] > 1.0,
            "the fresh cmd must move the sim first, at {}",
            s1.ps.origin(&PROTOCOL_V1)[0]
        );

        // A reordered/duplicated cmd from before serverTime 500.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 0,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        tick_at += std::time::Duration::from_millis(50);
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert_eq!(
            s2.ps.origin(&PROTOCOL_V1),
            s1.ps.origin(&PROTOCOL_V1),
            "a stale cmd must not move the sim"
        );
    }

    /// More cmds than a tick may replay arrive back to back; the queue stays
    /// bounded and snapshotting carries on.
    #[test]
    fn flood_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let cmds: Vec<UserCmd> = (0..100)
            .map(|i| UserCmd {
                server_time: i * 20,
                forward: 127,
                ..Default::default()
            })
            .collect();
        for chunk in cmds.chunks(MAX_PACKET_USERCMDS as usize) {
            let ack = nc.incoming_sequence as i32;
            sv.handle_packet(
                addr(5),
                &nc.build_out(0x10, ack, 0, &chain.ops(ack, chunk), &Huffman::new())
                    .unwrap(),
                now,
            );
        }
        let s = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(50),
        );
        assert!(s.valid);
        assert!(
            sv.clients[0].as_ref().unwrap().pending.len() <= 2,
            "flood left {} cmds queued",
            sv.clients[0].as_ref().unwrap().pending.len()
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(100),
        );
        assert!(s2.valid, "snapshots must continue");
    }

    /// A retail client omits unchanged angle fields (change bit 0) instead of
    /// announcing them, so the server must decode them against the cmd it
    /// last stored for this client, not `NULL_USERCMD` -- decoding against
    /// the null base resets the move basis to yaw 0 for a frame (the
    /// "spectator flash" bug, AGENTS.md's gotchas). `ps.yaw` steers
    /// `pmove::spectator_move`'s wishdir directly, so a burst of `forward`
    /// cmds sent right after the omission is a direct probe of which base
    /// the server used: yaw -45 accelerates the spectator diagonally
    /// (velocity settles near `vx == -vy`), yaw 0 (the bug) only along +x
    /// (`vy` stays small). A single cmd is not enough to tell them apart --
    /// `accelerate` blends toward the wishdir rather than snapping to it, so
    /// one frame is still mostly the previous leg's momentum; a 20-cmd burst
    /// gives the (correct or wrongly-reset) wishdir time to dominate, which
    /// I confirmed empirically (`vy` -283 with the real base, -20 with the
    /// bug forced, at this burst length). `viewangles` no longer carries
    /// this -- it stays unwritten, docs/protocol-1.1.md "Spectator view
    /// angles" -- so this checks the property the field used to stand in
    /// for directly.
    #[test]
    fn omitted_angles_decode_against_the_stored_last_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Turn to yaw -45, announced like vcod's own client always does.
        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s1.ps.origin(&PROTOCOL_V1);
        assert!(
            o1[0] > 0.1 && o1[1] < -0.1,
            "cmd 1's announced yaw -45 must steer the spectator diagonally from spawn, origin {o1:?}"
        );

        // Next packet: the mouse did not move for 20 frames, so pitch and yaw
        // ride change bit 0 off the cmd the server just stored, every frame.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.checksum_feed, ack, 520, 20),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        // Anti-vacuous: the burst must actually have been applied.
        let dx = s2.ps.origin(&PROTOCOL_V1)[0] - o1[0];
        assert!(dx > 1.0, "the omitted-angle burst was not applied, dx {dx}");
        // The discriminator: only a base of yaw -45 leaves a large negative
        // vy once the burst settles; a base reset to yaw 0 does not.
        let vy = s2.ps.field_f32(&PROTOCOL_V1, "velocity[1]");
        assert!(
            vy < -50.0,
            "omitted angles must keep the stored yaw -45, not reset to 0: vy {vy}"
        );
    }

    /// Press then release, end to end: the release must decode up=0 against
    /// the server's stored base, or the sim replays the jump forever.
    #[test]
    fn released_upmove_stops_the_climb() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let p = &PROTOCOL_V1;

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        up: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let vz = s1.ps.field_f32(p, "velocity[2]");
        assert!(vz > 100.0, "holding up must climb, vz {vz}");

        // A burst of release frames: friction alone has to bring the climb
        // back to rest. With spectator friction 5.0 the decay is geometric
        // (scale ~0.75/tick at the stopspeed boundary); 32 frames (the wire
        // per-message cap) take a 237 u/s climb to ~0.02.
        let releases: Vec<UserCmd> = (0..32)
            .map(|i| UserCmd {
                server_time: 550 + i * 50,
                ..Default::default()
            })
            .collect();
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &chain.ops(ack, &releases), &Huffman::new())
                .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        let vz = s2.ps.field_f32(p, "velocity[2]");
        // Q3's PM_Friction zeroes only xy below speed 1, leaving a ~0.18 u/s
        // vertical floor (retail-faithful); anything under half a unit is rest.
        assert!(
            vz.abs() < 0.5,
            "released upmove must decay to rest, vz {vz}"
        );
    }

    /// A move message that fails to decode is discarded whole: the next good
    /// one still chains from the last successfully decoded cmd, not
    /// `NULL_USERCMD` -- same base-decode property and the same burst probe
    /// as `omitted_angles_decode_against_the_stored_last_cmd`, with a
    /// garbled packet spliced in first.
    #[test]
    fn a_garbled_message_does_not_poison_the_base() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s1.ps.origin(&PROTOCOL_V1);
        assert!(
            o1[0] > 0.1 && o1[1] < -0.1,
            "cmd 1's announced yaw -45 must steer the spectator diagonally from spawn, origin {o1:?}"
        );

        // Garbage that cannot parse, then a good packet whose angles are
        // omitted: they must come off the pre-failure base.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &garbled_move_ops(), &Huffman::new())
                .unwrap(),
            t1,
        );
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.checksum_feed, ack, 520, 20),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        assert!(s2.valid, "snapshots must continue");
        // Anti-vacuous: the burst must actually have been applied.
        let dx = s2.ps.origin(&PROTOCOL_V1)[0] - o1[0];
        assert!(dx > 1.0, "the omitted-angle burst was not applied, dx {dx}");
        // The discriminator: only a base of yaw -45 leaves a large negative
        // vy once the burst settles; a base reset to yaw 0 does not.
        let vy = s2.ps.field_f32(&PROTOCOL_V1, "velocity[1]");
        assert!(
            vy < -50.0,
            "the garbled packet must not reset the base to yaw 0: vy {vy}"
        );
    }

    #[test]
    fn an_unacked_client_gets_no_snapshots() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let _nc = connected(&mut sv, addr(5), now);
        sv.take_outgoing(); // drop the gamestate frames
        sv.tick(now);
        assert!(
            sv.take_outgoing().is_empty(),
            "nothing before the gamestate is acked"
        );
    }
}
