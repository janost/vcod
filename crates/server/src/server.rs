//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The server state machine, transport-free. `main.rs` owns the socket, the
//! tests own a queue. Ported from RTCW-MP sv_main.c / sv_client.c; reply
//! strings come from docs/research/cod11-server-handshake.md.

use crate::client::{Client, ClientState, CmdKind, Queued, QueuedCmd, sanitize_name};
use crate::compass;
use crate::configstrings;
use crate::console;
use crate::follow;
use crate::game::combat::Effect;
use crate::game::host::{ClientEvent, SpawnMode};
use crate::game::say::SayMode;
use crate::game::script;
use crate::game::stuck::{StuckView, stuck_in_client};
use crate::game::temp_entity;
use crate::spectate::{ClientSim, PmType};
use crate::world::{TestEntities, World};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::movetrace::CONTENTS_CORPSE;
use vcod_common::net::connectionless::{Info, build_oob, parse_connect, parse_oob};
use vcod_common::net::gamestate::{self, Gamestate};
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, CLC_BITS, CLC_CLIENT_COMMAND, CLC_EOF, CLC_MOVE, CLC_MOVE_NO_DELTA, MsgReader, MsgWriter,
    NULL_USERCMD, UserCmd, read_delta_usercmd,
};
use vcod_common::net::netchan::{ClientMessage, MAX_RELIABLE_COMMANDS, ServerNetchan};
use vcod_common::net::protocol::{CS_SERVERINFO, PROTOCOL_V1, Protocol};
use vcod_common::net::{com_hash_key, info_value_for_key, snapshot};
use vcod_common::pmove::FallHeights;

/// `MAX_CHALLENGES`, server.h:198.
const MAX_CHALLENGES: usize = 1024;
/// A challenge nobody asked about for this long is dropped when a new one
/// is issued, so a spoof flood has to keep up with real clients to evict them.
const CHALLENGE_TTL: Duration = Duration::from_secs(60);
/// `sv_timeout` default (cod_lnxded 0x80d56c0).
const TIMEOUT: Duration = Duration::from_secs(240);
/// `sv_zombietime` default (cod_lnxded 0x80d56cf): how long a dropped slot
/// stays `CS_ZOMBIE` past the client's last packet.
const ZOMBIE_TIME: Duration = Duration::from_secs(2);
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
const MAX_PACKET_USERCMDS: u8 = vcod_common::net::MAX_MOVE_CMDS as u8;
/// Queued-but-unreplayed usercmds per client; past this a flood drops the
/// oldest rather than building latency.
const MAX_PENDING_CMDS: usize = 64;
/// pmove steps per snapshot tick; a flood beyond this fast-forwards.
const MAX_CMDS_PER_TICK: usize = 32;
/// `SV_ClientCommand`'s flood window (`cod_lnxded` 0x8086f5f, `add eax,0x320`):
/// a non-exempt client command opens 800 ms during which every further
/// non-exempt one from an active client is dropped before the game sees it
/// (`docs/protocol-1.1.md`, "Client commands are flood-protected").
const FLOOD_WINDOW_MS: i32 = 800;
/// How often a bot re-runs its enemy search (range gate + LOS traces).
/// The cached verdict is at most this stale.
const ENEMY_REFRESH_MS: i32 = 100;
/// A* nodes a tick may expand across all bots; a plan left unfinished
/// resumes next tick (bot-navigation.md, section 3).
const BOT_PLAN_BUDGET: u32 = 4000;
/// A retrieval objective as the bots see it, with its carrier's slot.
/// A retrieval objective as [`Server::bot_re`] reads it: the view with the
/// carrier's feet in `carrier_at` for every team, the carrier's slot, and
/// the compass record's `teamNum`.
type ReObjCarried = (crate::bots::ReObjView, Option<usize>, i32);
/// The tick pace (`1000 / sv_fps`), also the dt floor a fresh sim starts from.
pub(crate) const FRAME_MS: i32 = 50;
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
    /// Debug bots in play. Each takes a real slot; 0 is off.
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
    fn new(fs: &vcod_common::pk3::Pk3Fs, feed: i32) -> Self {
        let dll = |name: &str| fs.source_pak(name).map(|p| p.pure_checksum(feed));
        PureCheck {
            cgame: dll("cgame_mp_x86.dll"),
            ui: dll("ui_mp_x86.dll"),
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

/// A client command whose whole effect is to release a script thread.
/// `client_command` parses it out of the packet and `tick` runs it, so it
/// lands on the frame the rest of the script frame runs on.
enum ScriptCommand {
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
}

/// How long a fall holds off `P_DamageFeedback`'s `EV_PAIN`: `ClientEvents`
/// stores `level.time + 200` into `pain_debounce_time` (player-clip doc 8.8).
const FALL_PAIN_DEBOUNCE_MS: i32 = 200;

/// The server generator's next value, 0..2^31.
fn rand_from(rng: &mut u64) -> i32 {
    (vcod_common::rng::xorshift(rng) >> 33) as i32 & 0x7fff_ffff
}

/// `ClientEvents`' fall damage before `G_Damage`'s location multiplier: the
/// landing pain's percent of `max_health` (`ps.stats[2]`), 1.1 past 99,
/// truncated (player-clip doc 8.8). The x87 product is exact in `f64`, which
/// is what keeps 25 percent of 100 at 24.
fn fall_damage(percent: i32, max_health: i32) -> i32 {
    let share = if percent > 99 {
        f64::from(1.1f32)
    } else {
        f64::from(percent) * f64::from(0.01f32)
    };
    (f64::from(max_health) * share) as i32
}

/// What each client holds, from the host onto its sim, and the sim's origin
/// back to script.
fn mirror_weapons(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    for (slot, c) in clients.iter_mut().enumerate() {
        if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
            mirror_weapons_of(sim, rt, slot);
        }
    }
}

/// [`mirror_weapons`] for one client.
fn mirror_weapons_of(sim: &mut ClientSim, rt: &mut script::ScriptRuntime, slot: usize) {
    let w = rt.client_weapons(slot);
    sim.ps.weapons_held = w.held;
    sim.ps.weapon_slots = w.slots;
    sim.ps.weapon = w.current;
    sim.viewmodel_index = rt.client_viewmodel(slot);
    // The body, head and helmet the character script dressed the
    // client in: what a shot at it is traced against.
    if let Some(a) = rt.client_assembly(slot)
        && a != sim.assembly
    {
        sim.assembly = a;
    }
    // And back the other way: the sim owns where a player is, so the
    // script's copy is written from it every frame.
    rt.set_client_origin(slot, sim.origin());
}

/// The weapon ops script queued, each applied once.
fn apply_weapon_ops(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    weapons: &crate::weapons::WeaponTable,
) {
    for (slot, op) in rt.take_weapon_ops() {
        if let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        {
            apply_weapon_op(sim, op, weapons);
        }
    }
}

/// A standing player with its feet at `feet`, as a blast candidate in slot 0.
fn standing_blast_victim(feet: [f32; 3]) -> crate::game::combat::BlastVictim {
    use vcod_common::pmove::{HALF_WIDTH, Stance};
    let feet = glam::Vec3::from(feet);
    crate::game::combat::BlastVictim {
        slot: 0,
        origin: feet,
        link_origin: feet.trunc(),
        mins: glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0),
        maxs: glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height()),
        eye: feet + glam::Vec3::Z * Stance::Stand.view_height(),
    }
}

/// What script did to each sim, applied once: events, `setOrigin`,
/// `setPlayerAngles` and the damage the callback did.
fn apply_sim_ops(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    for (slot, op) in rt.take_sim_ops() {
        let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            continue;
        };
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, now_ms);
    }
}

/// One op of [`apply_sim_ops`].
#[allow(clippy::too_many_arguments)]
fn apply_sim_op(
    sim: &mut ClientSim,
    rt: &mut script::ScriptRuntime,
    slot: usize,
    op: crate::game::host::SimOp,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    use crate::game::host::SimOp;
    match op {
        SimOp::Event { event, parm } => sim.add_event(event, parm),
        // The host's copy too: the weapon mirror put the pre-teleport
        // origin back over the builtin's write, and a callback ahead of the
        // client's next cmd reads it.
        SimOp::SetOrigin { origin } => {
            sim.teleport(origin);
            rt.set_client_origin(slot, sim.origin());
        }
        SimOp::SetViewAngles { angles } => sim.set_view_angle(angles),
        SimOp::Damaged { .. } => {
            let index = sim.ps.weapon as usize;
            let inputs = anims.map(|anims| crate::spectate::AnimInputs {
                anims,
                weapon: crate::items::item_name(index).unwrap_or_default(),
                weapon_class: weapons.class(index),
            });
            sim.take_damage(&op, inputs.as_ref(), rng, now_ms);
        }
    }
}

/// An attack's effects in the order it raised them: an impact goes on the
/// wire and a hit runs the damage callback there and then, so the flesh pair
/// a leg's callback raises numbers below the next leg's wall impact.
fn apply_effects(rt: &mut script::ScriptRuntime, effects: Vec<Effect>, now_ms: i32) {
    for e in effects {
        match e {
            Effect::Impact(te) => rt.push_temp_entity(te),
            Effect::Hit(h) => rt.deliver_hits(vec![h], now_ms),
        }
    }
}

/// One gunner's rounds delivered inside its own `ClientEndFrame`, as
/// `turret_think_client` -> `Bullet_Fire` -> `G_Damage` runs the callback
/// there (combat doc 16.2): what it queued is applied before the next
/// slot's turn, so a later gunner's trace passes a corpse it made and a
/// higher slot's aim trace and pose read the move. A victim above the
/// gunner takes its feedback this frame; one below had its end frame
/// already and takes it on the next, and a gunner below that dies keeps
/// the gun until its own next turn releases it.
#[allow(clippy::too_many_arguments)]
fn deliver_turret_rounds(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    effects: Vec<Effect>,
    gunner: usize,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    if effects.is_empty() {
        return;
    }
    let feedback_now: Vec<usize> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Hit(h) if h.victim > gunner => Some(h.victim),
            _ => None,
        })
        .collect();
    mirror_roster(clients, rt);
    apply_effects(rt, effects, now_ms);
    mirror_weapons(clients, rt);
    apply_weapon_ops(clients, rt, weapons);
    apply_sim_ops(clients, rt, anims, weapons, rng, now_ms);
    mirror_vitals(clients, rt);
    for slot in feedback_now {
        if let Some(sim) = clients[slot].as_mut().and_then(|c| c.sim.as_mut()) {
            sim.end_frame(now_ms);
        }
    }
    // A body the callback killed or moved, for the later slots' aim traces.
    for (slot, c) in clients.iter().enumerate() {
        let sim = c.as_ref().and_then(|c| c.sim.as_ref());
        rt.set_client_body(slot, sim.and_then(|s| s.hit_body(slot)));
        rt.set_client_dobj(slot, sim.and_then(|s| s.dobj(slot)));
    }
}

/// The host's health onto each playing sim. Neither `ClientEndFrame`'s
/// intermission arm nor `SpectatorClientEndFrame` copies `ent->health` into
/// the playerstate, so both keep the zero their own spawn left (map-cycle
/// doc, 6.2; `spectate.rs`, `become_spectator`).
/// What the follow reads for `slot`: the script's fields when the slot has
/// a client entity, the sim's own mode when it has none.
fn session_of(
    rt: Option<&mut script::ScriptRuntime>,
    slot: usize,
    sim: &ClientSim,
) -> follow::Session {
    rt.and_then(|rt| rt.client_session(slot))
        .unwrap_or_else(|| follow::Session::unscripted(sim.pm_type == PmType::Spectator))
}

/// Whether `slot` holds a client a follow may copy: `GetFollowPlayerState`
/// (`game.mp.i386.so` 0x415c4) refuses one whose own-view bit is off.
fn followable(clients: &[Option<Client>], slot: usize) -> bool {
    clients
        .get(slot)
        .and_then(Option::as_ref)
        .and_then(|c| c.sim.as_ref())
        .is_some_and(|s| s.own_view)
}

/// `ClientEndFrame`'s follow half for one slot, in slot order: a playing or
/// dead client takes the own-view bit a follow copies, or is spawned where a
/// copy still in its playerstate left it, and a spectator runs
/// `SpectatorClientEndFrame` (0x40760). A forced follow's copy is taken
/// `archivetime` back, retried 50 ms younger each time it fails down to a
/// live one, and the age it settled on is written back; a forced follow that
/// finds nothing writes -1 back into `spectatorclient`, and so does a follow
/// that finds nothing to copy, which is `StopFollowing`.
fn follow_end_frame(
    clients: &mut [Option<Client>],
    slot: usize,
    rt: &mut script::ScriptRuntime,
    archive: &crate::archive::Archive,
    collision: Option<&vcod_common::collision::CollisionWorld>,
    now: i32,
) {
    use crate::archive::Source;
    let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) else {
        return;
    };
    let session = session_of(Some(rt), slot, sim);
    fn sim_mut(clients: &mut [Option<Client>], slot: usize) -> &mut ClientSim {
        clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap()
    }
    match session.state {
        follow::SessionState::Playing | follow::SessionState::Dead => {
            let c = clients[slot].as_mut().unwrap();
            let cmd_angles = c.last_cmd.angles;
            let sim = c.sim.as_mut().unwrap();
            // The copy is still the playerstate, whose `clientNum` is not
            // this client's: `ClientSpawn` at its feet and yaw (0x40f82),
            // whose own think runs the client up to the frame's clock.
            if let (true, Some(copied)) = (sim.follow.on, sim.follow.copied) {
                let playing = session.state == follow::SessionState::Playing;
                let world = collision.map(vcod_common::movetrace::MoveWorld::bare);
                sim.spawn_from_copy(&copied, playing, cmd_angles, world);
                c.last_processed_st = now;
                rt.engine_client_spawn(slot, copied.origin, copied.angles[1]);
            }
            sim.own_view = true;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Intermission => {
            let sim = sim_mut(clients, slot);
            sim.own_view = false;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Spectator => {}
    }
    sim_mut(clients, slot).own_view = false;
    let mut age = session.archive_ms;
    let mut forced = session.spectator_client;
    let mut target = sim_mut(clients, slot).follow.target;
    let mut source = Source::None;
    if forced >= 0 {
        let t = forced as usize;
        target = Some(t);
        loop {
            age = age.max(0);
            source = archive.player_state(t, &mut age, followable(clients, t));
            if source != Source::None || age == 0 {
                break;
            }
            age -= 50;
        }
        if source == Source::None {
            rt.set_client_spectator_client(slot, -1);
            forced = -1;
            target = None;
        }
    }
    if source == Source::None
        && let Some(t) = target
    {
        source = archive.player_state(t, &mut age, followable(clients, t));
    }
    if age != session.archive_ms {
        rt.set_client_archive_ms(slot, age);
    }
    let copied = match (target, source) {
        (Some(t), Source::Live) => {
            let ts = clients[t].as_ref().and_then(|c| c.sim.as_ref()).unwrap();
            Some(follow::Copied {
                eye: ts.ps.view().eye.into(),
                angles: ts.view_angles(),
                origin: ts.origin(),
                velocity: ts.ps.velocity.into(),
                teleport_bit: ts.teleport_bit(),
                frame: None,
            })
        }
        (Some(_), Source::Archived { frame, view }) => Some(follow::Copied {
            eye: view.eye,
            angles: view.angles,
            origin: view.origin,
            velocity: std::array::from_fn(|i| {
                view.ps.field_f32(&PROTOCOL_V1, &format!("velocity[{i}]"))
            }),
            teleport_bit: view.ps.field_i32(&PROTOCOL_V1, "eFlags")
                & crate::spectate::EF_TELEPORT_BIT
                != 0,
            frame: Some(frame),
        }),
        _ => None,
    };
    match copied {
        Some(copied) => {
            sim_mut(clients, slot).follow = follow::Follow {
                target,
                on: true,
                forced: forced >= 0,
                copied: Some(copied),
            };
        }
        None => {
            sim_mut(clients, slot).stop_following(collision);
            if session.spectator_client != -1 {
                rt.set_client_spectator_client(slot, -1);
            }
        }
    }
}

/// `ClientDisconnect`'s pass over the spectators following `gone`
/// (0x42b25..0x42b5b): each cycles on to the next client, or stops.
fn pass_followers_on(
    clients: &mut [Option<Client>],
    gone: usize,
    mut rt: Option<&mut script::ScriptRuntime>,
    collision: Option<&vcod_common::collision::CollisionWorld>,
) {
    for slot in 0..clients.len() {
        let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) else {
            continue;
        };
        if sim.follow.target != Some(gone) {
            continue;
        }
        let session = session_of(rt.as_deref_mut(), slot, sim);
        if session.state != follow::SessionState::Spectator {
            continue;
        }
        let next = (session.spectator_client < 0)
            .then(|| follow::cycle(Some(gone), 1, clients.len(), |t| followable(clients, t)))
            .flatten();
        let sim = clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap();
        match next {
            Some(t) => sim.follow.target = Some(t),
            None => {
                sim.stop_following(collision);
                if let (Some(rt), true) = (rt.as_deref_mut(), session.spectator_client != -1) {
                    rt.set_client_spectator_client(slot, -1);
                }
            }
        }
    }
}

/// `DeathmatchScoreboardMessage` (`.so` 0x459c0) for these clients: the
/// script's own, or every row zero without one.
fn scoreboard(clients: &[Option<Client>], rt: Option<&mut script::ScriptRuntime>) -> String {
    match rt {
        Some(rt) => {
            mirror_roster(clients, rt);
            rt.scoreboard()
        }
        None => {
            let rows = roster(clients)
                .enumerate()
                .filter_map(|(slot, r)| {
                    r.map(|r| crate::game::scoreboard::Row {
                        slot,
                        connecting: r.connecting,
                        spectator: false,
                        score: 0,
                        deaths: 0,
                        ping: r.ping,
                        icon: 0,
                    })
                })
                .collect();
            crate::game::scoreboard::text(rows, [0, 0])
        }
    }
}

/// Each slot's connection and follow target, as the scoreboard and
/// `player_die`'s walk read them.
fn roster(
    clients: &[Option<Client>],
) -> impl Iterator<Item = Option<crate::game::scoreboard::RosterSlot>> + '_ {
    clients.iter().map(|c| {
        c.as_ref().map(|c| crate::game::scoreboard::RosterSlot {
            connecting: c.state == ClientState::Connected,
            active: c.state == ClientState::Active,
            following: c.sim.as_ref().and_then(|s| s.follow.target),
            ping: c.ping,
        })
    })
}

/// [`roster`] onto the host, ahead of every script entry that can kill: the
/// walk runs inside the VM, the moment the killed callback returns.
fn mirror_roster(clients: &[Option<Client>], rt: &mut script::ScriptRuntime) {
    rt.host.client_roster = roster(clients).collect();
    for (name, c) in rt.host.client_names.iter_mut().zip(clients) {
        name.clear();
        if let Some(c) = c {
            name.push_str(&c.name);
        }
    }
}

/// The host's vitals onto every sim.
fn mirror_vitals(clients: &mut [Option<Client>], rt: &script::ScriptRuntime) {
    for (slot, c) in clients.iter_mut().enumerate() {
        if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
            mirror_vitals_of(sim, rt, slot);
        }
    }
}

/// [`mirror_vitals`] for one client.
fn mirror_vitals_of(sim: &mut ClientSim, rt: &script::ScriptRuntime, slot: usize) {
    if sim.pm_type != crate::spectate::PmType::Normal {
        return;
    }
    let v = rt.client_vitals(slot);
    sim.health = v.health;
    sim.max_health = v.max_health;
    if v.dead {
        sim.die();
    } else {
        sim.dead = false;
    }
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
    /// A `clipOnly` weapon this cmd spent the last round of (combat doc, 1.5
    /// step 9); pmove cannot take it because the script host owns
    /// `ps.weapons`.
    take: Option<u8>,
    /// The eye and `ps.viewangles` the cmd left, for the use key's aim.
    eye: [f32; 3],
    view: [f32; 3],
    /// The stance a mount saves for the release.
    stance: vcod_common::pmove::Stance,
}

/// One client's cmds since its animation was last picked: the events they
/// raised, the last of them, and what it held before the first.
#[derive(Default)]
struct Round {
    held: Option<u8>,
    switched: bool,
    events: Vec<vcod_common::pmove::PmEvent>,
    last_cmd: Option<UserCmd>,
}

/// What a damage or kill callback reads of a client that no builtin can
/// reach, off its sim as its cmds so far left it.
fn mirror_for_callback(
    rt: &mut script::ScriptRuntime,
    sim: &ClientSim,
    proto: &'static Protocol,
    slot: usize,
    st: i32,
) {
    // What `cloneplayer` and the dropped cook read.
    rt.set_client_entity_state(slot, Some(sim.to_entity(proto, slot, st)));
    rt.set_client_grenade_ms(slot, sim.ps.grenade_time_left_ms);
    rt.set_client_height(slot, (sim.ps.maxs() - sim.ps.mins()).z);
    // What `dropItem` hands the dropped weapon, and where it starts.
    rt.set_client_ammo(slot, sim.ps.ammo, sim.ps.ammoclip);
    rt.set_client_dobj(slot, sim.dobj(slot));
}

/// What a callback queued for `slot`, applied to its sim before its next cmd:
/// retail's script writes the playerstate there and then.
fn apply_callback_ops(
    rt: &mut script::ScriptRuntime,
    sim: &mut ClientSim,
    slot: usize,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    let (weapon_ops, sim_ops) = rt.take_ops_of(slot);
    for op in weapon_ops {
        apply_weapon_op(sim, op, weapons);
    }
    for op in sim_ops {
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, now_ms);
    }
    mirror_weapons_of(sim, rt, slot);
    mirror_vitals_of(sim, rt, slot);
}

/// `ClientThink_real`'s link after a cmd's move and its shots, at the
/// snapped origin with the contents the last end frame wrote (combat doc
/// 14.1, 14.7).
fn link_client(rt: &mut script::ScriptRuntime, slot: usize, sim: &ClientSim) {
    // The intermission arm returns ahead of the link; a spectator's think
    // unlinks, which its contents 0 does here.
    if sim.pm_type == crate::spectate::PmType::Intermission {
        return;
    }
    let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
    let bounds = (sim.ps.mins().into(), sim.ps.maxs().into());
    // A death earlier this tick linked `player_die`'s corpse contents, which
    // `r.contents` holds until the end frame and the sim takes only at the
    // next mirror.
    let contents = if rt.client_vitals(slot).dead && !sim.dead {
        rt.host.area.contents(slot as u32)
    } else {
        sim.contents as i32
    };
    rt.host
        .link_client(slot, sim.link_origin().into(), bounds, contents, playing);
}

/// `slot`'s entry in the movers' body list, off its sim: a death's
/// `CONTENTS_CORPSE` is outside every mover's mask.
fn relink(bodies: &mut Vec<vcod_common::movetrace::Body>, clients: &[Option<Client>], slot: usize) {
    bodies.retain(|b| b.entity != slot as u32);
    if let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) {
        bodies.extend(sim.body(slot as u32));
    }
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
    /// table for a client to pull. Retail ends the process on this
    /// (`Com_Error`) and so does vcod.
    Fatal(anyhow::Error),
}

/// What `G_ShutdownGame(1)` and `G_InitGame` print on a restart, ahead of
/// the scripts (handshake doc, "Console commands over rcon"). The team count
/// is `G_FindTeams` over `team` keys, which no stock map carries.
const RESTART_GAME_BANNER: &str = "==== RestartGame ====\n\
    ------- Game Initialization -------\n\
    gamename: main\n\
    gamedate: Nov 13 2003\n\
    0 teams with 0 entities\n\
    -----------------------------------\n";

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
    /// `CS_ZOMBIE` slots by index: a dropped client kept for
    /// [`ZOMBIE_TIME`] past its last packet so the drop notice reaches it.
    /// The game has already disconnected it, so it lives beside `clients`
    /// rather than in it; a slot is free only when both are `None`.
    zombies: Vec<Option<Client>>,
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
    anims: Option<Rc<vcod_common::animtree::PlayerAnims>>,
    /// Every weapon file, parsed once at load: the animscript tests
    /// `weaponClass` every frame and the frame loop must not read a pk3.
    /// `Rc` so a snapshot/move closure can hold it without borrowing `self`.
    weapon_table: Rc<crate::weapons::WeaponTable>,
    /// The number of the last client packet executed, bots' included: what
    /// `replay_moves` orders every client's cmds and `kill`s by.
    packet_seq: u64,
    /// When the last frame's messages went out (`Server::frame_sent`), and
    /// the frame time of the one sent before it.
    last_send: Option<(Instant, i32)>,
    /// The frame time of the newest frame sent, for `last_send`.
    sent_time_ms: i32,
    /// The ack time of the packet being handled, when it differs from
    /// `sv_time_ms` (`Server::handle_packet_at`).
    ack_time_ms: Option<i32>,
    /// The blasts this frame's missile pass set off, for the radius damage
    /// pass to charge (`crate::game::missile::Explosion`).
    pending_explosions: Vec<crate::game::missile::Explosion>,
    /// Retail's `+set name value`: applied last in `cvars`, over
    /// `default_mp.cfg` and the config's own, so a run can turn a script
    /// cvar such as `scr_friendlyfire` on without a code change.
    cvar_overrides: Vec<(String, String)>,
    /// `bg_fallDamageMinHeight` and `bg_fallDamageMaxHeight` as the script's
    /// cvar table last read, what every move lands with and systeminfo
    /// carries.
    fall_heights: FallHeights,
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
    /// The pak cvars of the mounted search path, systeminfo's lists.
    paks: configstrings::PakLists,
    /// The pure checksums a `cp` is checked against, keyed with this
    /// level's `checksumFeed`; `None` until a level loads.
    pure_check: Option<PureCheck>,
    /// The grafted player skeletons those shots trace against, built on first
    /// use (`crate::game::hitrig`).
    hit_rigs: crate::game::hitrig::HitRigs,
    /// Weapons the moves themselves switched to, by slot: only a change the
    /// machine made, so a playerstate reset from outside a move is not one.
    /// `tick` writes each back onto the script host before the mirror reads
    /// it, which is what keeps a switch from being undone the frame after it
    /// lands.
    weapon_changes: Vec<(usize, u8)>,
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
    /// ([`Self::overlay_script`]); `None` unless `--gametype-script` or a test set it.
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
    /// The level's navigation graph, built on the first tick with bots.
    nav: Option<std::sync::Arc<crate::nav::NavGraph>>,
    /// The build of `nav` under way, and whether the tick waits for it or
    /// ticks on with the bots off the graph until it lands.
    nav_job: Option<crate::nav::NavJob>,
    nav_background: bool,
    /// Per bot, its walk along `nav`.
    bot_paths: BTreeMap<usize, crate::nav::Follower>,
    /// Each bombzone's `(mins, maxs)` and the stand `site_stand` picked in
    /// it, for the life of the nav graph.
    bot_sites: Vec<([f32; 3], [f32; 3], [f32; 3])>,
    /// The shots and blasts since the bots last listened, in sim order;
    /// `step_bots` takes them at the top of the next tick.
    bot_noises: Vec<crate::bots::Noise>,
    /// The frames the killcam replays, kept while script has `setarchive`
    /// on and cleared by every level load (`crate::archive`).
    archive: crate::archive::Archive,
    /// `dedicated`. Retail's default is 2, which heartbeats; vcod's is 1 on
    /// purpose, so dev and test runs stay off the master list. `--set
    /// dedicated=2` opts in.
    dedicated: i32,
    masters: crate::master::Masters,
    /// `NET_StringToAdr` for the masters; a test swaps in its own.
    resolver: fn(&str) -> Option<SocketAddr>,
    /// `rconPassword`; empty refuses every rcon.
    rcon_password: String,
    rcon: crate::rcon::Rcon,
    /// The rcon output being collected, `Com_BeginRedirect` to
    /// `Com_EndRedirect`.
    redirect: Option<crate::rcon::Redirect>,
    /// A `quit` ran: the flatline is in the outbox and the binary exits once
    /// it has flushed it.
    quit: bool,
    /// A `killserver` ran: `sv_running` is 0, so no packet is read and no
    /// frame runs until a console `map` loads a level again.
    killed: bool,
    /// `sv_cheats`: set by `devmap`, cleared by `map`, carried in
    /// systeminfo.
    cheats: bool,
    bans: crate::bans::Bans,
}

/// A follower's frame: the followed client's number, its playerstate with
/// the follow flags patched in, its eye, and whether it is a replay.
struct FollowFrame {
    target: usize,
    ps: msg::PlayerState,
    eye: [f32; 3],
    replay: bool,
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

/// `ClientEndFrame`'s call of `G_GetNonPVSFriendlyInfo` for `slot`
/// (`crate::compass`): `None` when its session takes no end frame that
/// makes the call (spectator, intermission), else the packed teammate, its
/// slot and its ping bit, or `Some(None)` for nobody. The viewer needs a
/// team other than none or spectator; a candidate is a playing client on
/// it whose entity is not in a snapshot from the viewer's leaned eye.
fn compass_friend(
    clients: &[Option<Client>],
    slot: usize,
    sessions: &[Option<follow::SessionState>],
    teams: &[i32],
    vis: Option<&vcod_common::bsp::Visibility>,
    time: i32,
    p: &Protocol,
) -> Option<Option<(i32, u32, bool)>> {
    use follow::SessionState::{Dead, Playing};
    let sim = clients[slot].as_ref()?.sim.as_ref()?;
    if !matches!(sessions[slot], Some(Playing | Dead)) {
        return None;
    }
    let team = teams[slot];
    if team == script::TEAM_NONE || team == script::TEAM_SPECTATOR {
        return Some(None);
    }
    let eye: [f32; 3] = sim.ps.view().eye.into();
    let mut pinged = false;
    let found = compass::next_friend(sim.last_friend, [eye[0], eye[1]], |n| {
        let c = clients.get(n)?.as_ref()?;
        let other = c.sim.as_ref()?;
        if sessions[n] != Some(Playing) || other.pm_type != PmType::Normal || teams[n] != team {
            return None;
        }
        // No map: nothing to cull against, so everything is in view.
        let vis = vis?;
        let e = other.to_entity(p, n, c.last_processed_st);
        if crate::world::in_snapshot(vis, eye, &e, time, p) {
            return None;
        }
        pinged = other.ping;
        Some(compass::Candidate {
            at: [other.ps.origin.x, other.ps.origin.y],
            yaw: other.view_angles()[1],
        })
    });
    Some(found.map(|(info, n)| (info, n, pinged)))
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

/// `Info_RemoveKey`: the info string without `key`'s pair.
fn remove_info_key(info: &str, key: &str) -> String {
    let mut out = String::new();
    let mut parts = info.strip_prefix('\\').unwrap_or(info).split('\\');
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k != key {
            out.push_str(&format!("\\{k}\\{v}"));
        }
    }
    out
}

/// One message off a client the snapshot pass does not build for: its
/// unacked server commands and its last frame repeated under the new
/// sequence. The packets come back for the caller to address.
fn resend_last_frame(
    c: &mut Client,
    sv_time_ms: i32,
    proto: &'static Protocol,
    huff: &Huffman,
    baselines: &HashMap<u32, msg::EntityState>,
) -> Vec<Vec<u8>> {
    let mut w = MsgWriter::new(huff);
    write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
    c.reliable_sent = c.netchan.reliable_sequence as i32;
    let message_num = c.netchan.outgoing_sequence;
    if let Some(last) = c.sent_frame(message_num.wrapping_sub(1)) {
        let frame = snapshot::Snapshot {
            server_time: sv_time_ms,
            message_num,
            delta_num: -1,
            ..last.clone()
        };
        let base = c
            .sent_frame(c.message_ack.max(0) as u32)
            .filter(|b| (1..=255).contains(&message_num.saturating_sub(b.message_num)));
        w.write_byte(snapshot::SVC_SNAPSHOT);
        snapshot::write(&mut w, proto, base, &frame, baselines);
        c.record_frame(frame);
    }
    c.netchan
        .transmit(c.last_client_command, &w.into_ops(), huff)
}

impl Server {
    /// A server whose generator starts from a fixed seed, so a test run
    /// replays the same bot picks, script `randomInt`s and spawn choices.
    pub fn new(cfg: ServerConfig, now: Instant) -> Self {
        Self::with_seed(cfg, now, 0x9e37_79b9_7f4a_7c15)
    }

    /// `new` with the generator seeded from `seed`; the binary passes the
    /// wall clock.
    pub fn with_seed(mut cfg: ServerConfig, now: Instant, seed: u64) -> Self {
        cfg.max_clients = cfg.max_clients.min(MAX_CLIENTS);
        let seed = seed | 1;
        let server_id = 0x10;
        let fall = FallHeights::default();
        let mut sv = Server {
            configstrings: configstrings::static_configstrings(
                &cfg,
                server_id,
                fall,
                false,
                &Default::default(),
            ),
            sent_configstrings: configstrings::static_configstrings(
                &cfg,
                server_id,
                fall,
                false,
                &Default::default(),
            ),
            clients: (0..cfg.max_clients).map(|_| None).collect(),
            zombies: (0..cfg.max_clients).map(|_| None).collect(),
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
            packet_seq: 0,
            last_send: None,
            sent_time_ms: 0,
            ack_time_ms: None,
            pending_explosions: Vec::new(),
            cvar_overrides: Vec::new(),
            fall_heights: fall,
            pending_script_commands: Vec::new(),
            hitlocs: crate::game::combat::HitLocTable::default(),
            fs: None,
            paks: Default::default(),
            pure_check: None,
            hit_rigs: Default::default(),
            weapon_changes: Vec::new(),
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
            nav: None,
            nav_job: None,
            nav_background: false,
            bot_paths: BTreeMap::new(),
            bot_sites: Vec::new(),
            bot_noises: Vec::new(),
            archive: Default::default(),
            dedicated: 1,
            masters: Default::default(),
            resolver: crate::master::resolve,
            rcon_password: String::new(),
            rcon: Default::default(),
            redirect: None,
            quit: false,
            killed: false,
            cheats: false,
            bans: Default::default(),
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
        rand_from(&mut self.rng)
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

    /// [`Self::handle_packet`] for a packet read off the socket at
    /// `arrived`. Retail reads its socket every 5 ms and runs a frame in well
    /// under one, so a move message is stamped with the newest frame that had
    /// gone out when it arrived; vcod's tick takes long enough that one read
    /// at the next tick would stamp a packet that came in during the tick a
    /// frame late. docs/research/cod11-server-handshake.md, "Pings".
    pub fn handle_packet_at(
        &mut self,
        from: SocketAddr,
        pkt: &[u8],
        now: Instant,
        arrived: Instant,
    ) {
        self.ack_time_ms = self
            .last_send
            .filter(|&(at, _)| arrived < at)
            .map(|(_, before)| before);
        self.handle_packet(from, pkt, now);
        self.ack_time_ms = None;
    }

    /// Marks the frame just ticked as sent at `at`: the outbox it left has
    /// gone onto the socket.
    pub fn frame_sent(&mut self, at: Instant) {
        self.last_send = Some((at, self.sent_time_ms));
        self.sent_time_ms = self.sv_time_ms;
    }

    /// `SV_PacketEvent`.
    pub fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        // `Com_EventLoop` hands a packet to `SV_PacketEvent` only while
        // `sv_running` is set.
        if self.killed {
            return;
        }
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
            // Rate limited by its own server-wide window, not the buckets.
            "rcon" => self.svc_remote_command(from, raw, now),
            // `SV_ConnectionlessPacket` (0x808c827) matches it and does
            // nothing; a client leaves through the netchan `disconnect`.
            "disconnect" => {}
            other => log::debug!("{from}: unhandled oob {other:?}"),
        }
    }

    /// `g_password` as the game module reads it: the running level's table,
    /// or the `--set` that the first load will stamp. Not latched.
    fn game_password(&self) -> String {
        self.live_cvar("g_password", "")
    }

    /// A cvar as the engine or the game reads it at this moment: the running
    /// level's table, else the `--set` the first load will stamp, else
    /// `default`.
    fn live_cvar(&self, name: &str, default: &str) -> String {
        match self.script.as_ref() {
            Some(rt) => rt.cvars().get(name).to_string(),
            None => self.pending_cvar(name, default),
        }
    }

    /// `sv_privateClients` read as `->integer`: the slots below it are
    /// reserved for a client whose userinfo `password` is
    /// `sv_privatePassword` (docs/research/cod11-server-handshake.md,
    /// "Private slots").
    fn private_clients(&self) -> i32 {
        crate::game::builtins::cvar::atoi(&self.live_cvar("sv_privateClients", "0"))
    }

    /// `getinfo`/`getstatus`'s `pswrd`: 1 for any non-empty `g_password`,
    /// `none` included (0x808c3ae, 0x808bf23).
    fn pswrd(&self) -> u8 {
        u8::from(!self.game_password().is_empty())
    }

    /// `ClientConnect`'s password test (game.mp.i386.so 0x425da): an empty
    /// or `none` `g_password` lets anyone in, otherwise the userinfo's
    /// `password` must match it case-sensitively. Bots are exempt, as a
    /// retail test client's `ip` reads `localhost`
    /// (docs/research/cod11-server-handshake.md, "g_password").
    fn password_denied(&self, userinfo: &str, is_bot: bool) -> bool {
        let pw = self.game_password();
        !is_bot
            && !pw.is_empty()
            && !pw.eq_ignore_ascii_case("none")
            && info_value_for_key(userinfo, "password").unwrap_or("") != pw
    }

    /// `SVC_Info` (cod_lnxded 0x808c1ac). Key order is retail's;
    /// `minPing`/`maxPing`/`game` only appear when the matching cvar is set.
    /// The private slots are left out of both counts: `clients` counts the
    /// occupied slots from `sv_privateClients` up, `sv_maxclients` is the
    /// rest.
    fn svc_info(&mut self, from: SocketAddr, challenge: &str) {
        let private = self.private_clients();
        let public_clients = self
            .clients
            .iter()
            .skip(private.max(0) as usize)
            .flatten()
            .count();
        let mut i = Info::new();
        i.set("challenge", challenge_arg(challenge))
            .set("protocol", PROTOCOL_V1.version)
            .set("hostname", &self.cfg.hostname)
            .set("mapname", &self.cfg.map)
            .set("clients", public_clients)
            .set("sv_maxclients", self.cfg.max_clients as i32 - private)
            .set("gametype", self.live_gametype())
            .set("pure", u8::from(self.paks.pure))
            .set("sv_allowAnonymous", 0)
            .set("pswrd", self.pswrd());
        self.send_oob(from, &format!("infoResponse\n{i}"));
    }

    /// `SVC_Status` (0x808bd50).
    fn svc_status(&mut self, from: SocketAddr, challenge: &str) {
        let mut i = self.serverinfo();
        i.set("challenge", challenge_arg(challenge))
            .set("pswrd", self.pswrd());
        let mut lines = String::new();
        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_ref() else {
                continue;
            };
            let (ping, name) = (c.ping, c.name.clone());
            let score = self.client_score(slot);
            lines.push_str(&format!("{score} {ping} \"{name}\"\n"));
        }
        self.send_oob(from, &format!("statusResponse\n{i}\n{lines}"));
    }

    /// `SVC_RemoteCommand` (0x808c404). The reply is whatever the command
    /// printed, in `print\n` packets of up to `rcon::OUTPUT_BUF` bytes; a
    /// rate-limited request gets nothing.
    fn svc_remote_command(&mut self, from: SocketAddr, raw: &[u8], now: Instant) {
        use crate::rcon::Verdict;
        let text = String::from_utf8_lossy(raw.get(4..).unwrap_or_default());
        let args = crate::game::say::tokenize(&text);
        let verdict = self.rcon.check(&self.rcon_password, &args, now);
        let shown = args.get(2).map_or("", String::as_str);
        match verdict {
            Verdict::Limited => return,
            Verdict::Run(_) => log::info!("Rcon from {from}:\n{shown}"),
            _ => log::info!("Bad rcon from {from}:\n{shown}"),
        }
        self.redirect = Some(crate::rcon::Redirect::default());
        match verdict {
            Verdict::NoPassword => self.print(crate::rcon::NO_PASSWORD),
            Verdict::BadPassword => self.print(crate::rcon::BAD_PASSWORD),
            Verdict::Run(Some(line)) => {
                self.exec_console_line(&line, now);
            }
            Verdict::Run(None) | Verdict::Limited => {}
        }
        // A `quit` exits before the redirect is flushed, and a `killserver`
        // shuts the server down under it: no reply either way.
        if self.quit || self.killed {
            self.redirect = None;
            return;
        }
        for reply in self.redirect.take().unwrap_or_default().finish() {
            self.send_oob(from, &reply);
        }
    }

    /// `SV_GetChallenge` without the authorize-server detour. A client still
    /// asking keeps its entry fresh; stale entries go before a new insert.
    /// A banned address off the LAN gets the error retail relays from an
    /// authorize denial; retail never asks the authorize server about a LAN
    /// client, so a ban does not reach one there either.
    fn svc_get_challenge(&mut self, from: SocketAddr, now: Instant) {
        if let std::net::IpAddr::V4(ip) = from.ip()
            && !crate::client::is_lan(from.ip())
            && self.bans.contains(ip)
        {
            self.send_oob(from, "error\nEXE_ERR_BAD_CDKEY");
            return;
        }
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
            self.send_oob(from, "error\nEXE_SERVER_IS_DIFFERENT_VER\x151.1\n");
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
            .chain(&self.zombies)
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
        let reconnect = self
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer));
        if let Some(i) = reconnect {
            // Retail hands the slot to any challenge issued to this ip, so
            // a neighbour behind the same NAT who knows the qport can take
            // over a live player. Only the slot's own challenge (the
            // client's connect retry) may replace a client still heard
            // from; a silent slot (crash, lost disconnect) is reclaimable.
            let c = self.clients[i].as_ref().unwrap();
            if c.netchan.challenge != challenge
                && now.duration_since(c.last_packet) < RECONNECT_LIMIT
            {
                log::info!("{from}: connect with a foreign challenge refused, client {i} is live");
                return;
            }
        }
        // A zombie of this peer is a reconnect into its own slot; the game
        // saw it leave at the drop, so there is nothing to tear down.
        let Some(slot) = reconnect.or_else(|| {
            self.zombies
                .iter()
                .position(|c| c.as_ref().is_some_and(same_peer))
                .or_else(|| {
                    // A new client: a matching `sv_privatePassword` (an
                    // empty one matches an absent `password`) searches from
                    // slot 0, anyone else from `sv_privateClients` (0x8085a03).
                    let private_pw = self.live_cvar("sv_privatePassword", "");
                    let start =
                        if info_value_for_key(&userinfo, "password").unwrap_or("") == private_pw {
                            0
                        } else {
                            self.private_clients().max(0) as usize
                        };
                    self.free_slot_from(start)
                })
        }) else {
            log::debug!("Rejected a connection.");
            self.send_oob(from, "error\nEXE_SERVERISFULL");
            return;
        };
        // `ClientConnect`'s verdict, after the engine's own checks
        // (0x8085baa); a denied connect leaves the slot as it was.
        if self.password_denied(&userinfo, false) {
            log::debug!("Game rejected a connection: GAME_INVALIDPASSWORD.");
            self.send_oob(from, "error\nGAME_INVALIDPASSWORD");
            return;
        }
        if reconnect.is_some() {
            log::info!("{from}: reconnect");
            // Whoever held the slot is gone, so its script state has to be
            // torn down here: `check_timeouts` never reaches it once the new
            // `Client` overwrites the slot with a fresh `last_packet`.
            if let Some(rt) = self.script.as_mut() {
                rt.push_client_event(ClientEvent::Disconnect(slot));
            }
        } else {
            self.zombies[slot] = None;
        }
        // `Info_SetValueForKey(userinfo, "ip", NET_AdrToString(from))`
        // (0x8085498, key at 0x80d43da): any `ip` the client sent goes, the real one is last.
        let userinfo = format!(
            "{}\\ip\\{}:{}",
            remove_info_key(&userinfo, "ip"),
            from.ip(),
            from.port() as i16
        );
        let client = Client::new(from, qport, challenge, userinfo, now);
        log::info!("client {slot} {:?} connected from {from}", client.name);
        let name = client.name.clone();
        self.clients[slot] = Some(client);
        if let Some(rt) = self.script.as_mut() {
            rt.push_client_event(ClientEvent::Connect { slot, name });
        }
        self.send_oob(from, "connectResponse");
        // The first client, or the last the server holds (0x8085cd3).
        let count = self.client_count();
        if count == 1 || count == self.clients.len() {
            self.masters.force();
        }
    }

    /// A slot neither a client nor a zombie holds.
    fn free_slot(&self) -> Option<usize> {
        self.free_slot_from(0)
    }

    /// The first free slot at `start` or above.
    fn free_slot_from(&self, start: usize) -> Option<usize> {
        (start..self.clients.len())
            .find(|&i| self.clients[i].is_none() && self.zombies[i].is_none())
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
        let peer = |c: &Option<Client>| {
            c.as_ref()
                .is_some_and(|c| c.addr.ip() == from.ip() && c.netchan.qport == qport)
        };
        let Some(slot) = self.clients.iter().position(peer) else {
            match self.zombies.iter().position(peer) {
                Some(slot) => self.zombie_packet(slot, pkt),
                None => log::debug!("{from}: netchan packet from no client (qport {qport})"),
            }
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
        self.packet_seq += 1;
        let mut commands_done = false;
        for op in ops {
            if !commands_done && matches!(op, ClientOp::Move(_)) {
                commands_done = true;
                if !self.pure_after_commands(slot) {
                    return;
                }
            }
            match op {
                ClientOp::Command { seq, text } => {
                    if !self.client_command(slot, seq, text) {
                        return;
                    }
                }
                ClientOp::Move(last) => self.user_move(slot, last),
            }
        }
        if !commands_done {
            self.pure_after_commands(slot);
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
            let op = r.read_bits(CLC_BITS);
            if r.is_overflowed() {
                return None;
            }
            match op {
                CLC_CLIENT_COMMAND => {
                    let seq = r.read_long();
                    let text = r.read_server_string();
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
                // `read_bits(CLC_BITS)` yields 0..=3; unreachable.
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
        let engine_command = matches!(
            word,
            "disconnect"
                | "userinfo"
                | "cp"
                | "vdr"
                | "download"
                | "nextdl"
                | "stopdl"
                | "donedl"
                | "retransdl"
        );
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
            // `SV_VerifyPaks_f` and `SV_ResetPureClient_f`, run whatever
            // `sv_pure` says; only the drops read it.
            "cp" => {
                let ok = self.pure_check.as_ref().is_some_and(|p| p.verify(trimmed));
                if let Some(c) = self.clients[slot].as_mut() {
                    c.pure = if ok {
                        crate::client::Pure::Authentic
                    } else {
                        crate::client::Pure::Unpure
                    };
                }
            }
            "vdr" => {
                if let Some(c) = self.clients[slot].as_mut() {
                    c.pure = crate::client::Pure::Unchecked;
                }
            }
            // The download ucmds (cod_lnxded 0x8087a64, 0x8086168, 0x8087960,
            // 0x80879fc, 0x8087a2c).
            "download" => {
                let name = args.split_whitespace().next().unwrap_or("");
                if let Some(c) = self.clients[slot].as_mut() {
                    c.download = Some(crate::download::ServerDownload::new(name));
                }
            }
            "nextdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                let now = self.sv_time_ms;
                let Some(c) = self.clients[slot].as_mut() else {
                    return false;
                };
                use crate::download::NextDl;
                match c.download.as_mut().map(|d| d.next_dl(block, now)) {
                    Some(NextDl::Completed) => c.download = None,
                    Some(NextDl::Acked) | None => {}
                    Some(NextDl::Broken) => {
                        self.drop_client(slot, "broken download");
                        return false;
                    }
                }
            }
            "stopdl" => {
                if let Some(c) = self.clients[slot].as_mut() {
                    c.download = None;
                }
            }
            "donedl" => self.send_gamestate(slot),
            "retransdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                if let Some(d) = self.clients[slot]
                    .as_mut()
                    .and_then(|c| c.download.as_mut())
                {
                    d.retransmit(block);
                }
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
            // `Cmd_Say_f` (0x47050): nothing without an argument.
            "say" | "say_team" => {
                let mode = if word == "say" {
                    SayMode::All
                } else {
                    SayMode::Team
                };
                if let Some(text) = crate::game::say::concat_args(args) {
                    self.say(slot, None, mode, &text);
                }
            }
            // `Cmd_Tell_f` (0x47210): to a client in use, then the speaker's
            // own copy.
            "tell" => {
                let (to, text) = args
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .unwrap_or((args.trim(), ""));
                if let Ok(to) = to.parse::<usize>()
                    && self.clients.get(to).is_some_and(Option::is_some)
                {
                    let text = crate::game::say::concat_args(text).unwrap_or_default();
                    self.say(slot, Some(to), SayMode::Tell, &text);
                    self.say(slot, Some(slot), SayMode::Tell, &text);
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
            // command (combat doc, section 8). `SV_ExecuteClientMessage`
            // (0x80872ec) runs a packet's client commands ahead of its
            // usercmds, so the move pass runs it ahead of the cmds that came
            // with it (combat doc, 9.2).
            "kill" => {
                let packet = self.packet_seq;
                if let Some(c) = self.clients[slot].as_mut() {
                    c.kill_at.get_or_insert(packet);
                }
            }
            // `Cmd_MenuResponse_f`: the client answering a menu `openMenu`
            // opened. Unlike the entry notify this fires straight through:
            // nothing is armed by it, and a notify no thread is parked on is
            // simply lost, which is what retail does too. Queued, since the
            // notify releases threads.
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
        // `SV_UserMove` (0x8086fa4) drops a client a pure server has had no
        // `cp` from, after the entry and before any cmd runs.
        if self.paks.pure
            && self.clients[slot]
                .as_ref()
                .is_some_and(|c| !c.is_bot && c.pure == crate::client::Pure::Unchecked)
        {
            self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT");
            return;
        }
        let packet = self.packet_seq;
        let acked = self.ack_time_ms.unwrap_or(self.sv_time_ms);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.stamp_acked(acked);
        c.pending
            .extend(cmds.into_iter().map(|cmd| QueuedCmd { packet, cmd }));
        let excess = c.pending.len().saturating_sub(MAX_PENDING_CMDS);
        c.pending.drain(..excess);
    }

    /// `SV_ExecuteClientMessage` (0x80872ec) once a message's client
    /// commands ran: a pure server drops a client whose `cp` failed. False
    /// when it did.
    fn pure_after_commands(&mut self, slot: usize) -> bool {
        let unpure = self.paks.pure
            && self.clients[slot]
                .as_ref()
                .is_some_and(|c| c.pure == crate::client::Pure::Unpure);
        if unpure {
            self.drop_client(slot, "EXE_UNPURECLIENTDETECTED");
        }
        !unpure
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
        c.reliable_sent = c.netchan.reliable_sequence as i32;
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
        c.stamp_sent(c.netchan.outgoing_sequence, self.sv_time_ms);
        for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
            self.outbox.push((c.addr, pkt));
        }
    }

    /// `SV_AddServerCommand` (cod_lnxded 0x808b680): queued, squashed or
    /// dropped by [`Client::queue_server_command`], and sent with the
    /// client's next snapshot or gamestate. A client whose acks fall a whole
    /// ring behind has stopped consuming reliables; overwriting an unacked
    /// slot would desync both ends' scramble keys, so that is fatal, with
    /// retail's `EXE_SERVERCOMMANDOVERFLOW` (docs/protocol-1.1.md, "The
    /// server command queue").
    fn send_server_command(&mut self, slot: usize, cmd: &str) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        match c.queue_server_command(cmd) {
            Queued::Overflow => self.drop_client(slot, "EXE_SERVERCOMMANDOVERFLOW"),
            Queued::Dropped => log::debug!("client {slot}: dropped {cmd:?}"),
            Queued::Added | Queued::Replaced => {}
        }
    }

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
        // What a spawn into play leaves; the next end frame rewrites it off
        // the script's `sessionstate`.
        if let Some(rt) = self.script.as_mut() {
            rt.set_client_takedamage(slot, true);
        }
    }

    /// Mounts `slot` on the turret numbered `gun` as a use press would, from
    /// `mount_origin` and past the reach and arc tests. Test-facing, like
    /// `place_client`: it puts a second gunner on a map with one reachable
    /// gun. False when either is missing or the gun is manned.
    pub fn test_mount(&mut self, slot: usize, gun: u32, mount_origin: [f32; 3]) -> bool {
        let Some(rt) = self.script.as_mut() else {
            return false;
        };
        let Some(sim) = self
            .clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            return false;
        };
        let Some(turret) = rt.host.turrets.keys().copied().find(|id| id.0 == gun) else {
            return false;
        };
        if rt.host.turrets[&turret].busy != 0 {
            return false;
        }
        rt.host
            .turret_ops
            .push(crate::game::turret::TurretOp::Mount { slot, turret });
        let mounts = rt.take_turret_mounts(
            slot,
            mount_origin,
            vcod_common::pmove::Stance::Stand,
            sim.view_angles(),
        );
        for (turret, stance, view) in &mounts {
            crate::game::turret::mount_sim(sim, *turret, *stance, *view);
        }
        !mounts.is_empty()
    }

    /// Moves a client's playerstate origin and nothing else: no teleport
    /// bit, no event, no view. Test-facing: a replay starts from a retail
    /// capture's settled origin where the join left ours elsewhere.
    pub fn test_set_client_origin(&mut self, slot: usize, origin: [f32; 3]) {
        if let Some(sim) = self
            .clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        {
            sim.ps.origin = origin.into();
        }
    }

    /// Queues a client `setorigin` as the script frame would, applied at the
    /// next tick's sim-op pass: after that tick's moves and before its end
    /// frame. Test-facing: the stuck gate overlaps two players the way
    /// `probe_bump.gsc` did on retail.
    pub fn test_script_set_origin(&mut self, slot: usize, origin: [f32; 3]) {
        let rt = self
            .script
            .as_mut()
            .expect("a script runtime to queue the setorigin on");
        rt.host
            .client_sim_ops
            .push((slot, crate::game::host::SimOp::SetOrigin { origin }));
    }

    /// Moves the entity numbered `num` as script would. Test-facing, like
    /// `test_mount`: carentan's second gun sits out of the first one's arc.
    pub fn test_place_entity(&mut self, num: u32, origin: [f32; 3], angles: [f32; 3]) {
        if let Some(rt) = self.script.as_mut() {
            rt.place_entity(num, origin, angles);
        }
    }

    /// The blasts the last tick's missile pass set off, after the same
    /// tick's radius damage pass charged them. Test-facing: the replay in
    /// `tests/common` counts them per frame.
    pub fn pending_explosions(&self) -> &[crate::game::missile::Explosion] {
        &self.pending_explosions
    }

    /// [`scoreboard`] for this server's clients.
    fn scoreboard(&mut self) -> String {
        scoreboard(&self.clients, self.script.as_mut())
    }

    /// `G_Say`: retail's `trap_SendServerCommand` queues the line from
    /// inside `ClientCommand`, and so does this. Nothing without a script, which is
    /// where the teams and session states live.
    fn say(&mut self, slot: usize, target: Option<usize>, mode: SayMode, text: &str) {
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        mirror_roster(&self.clients, rt);
        for (to, cmd) in rt.say(slot, target, mode, text) {
            self.send_server_command(to, &cmd);
        }
    }

    /// Test-facing: `self pingPlayer()` on `slot`, with no script to call
    /// it.
    pub fn test_ping_player(&mut self, slot: usize) {
        if let Some(rt) = self.script.as_mut() {
            let until = rt.host.level_time_ms + 3000;
            if let Some(p) = rt.host.client_ping_until.get_mut(slot) {
                *p = until;
            }
        }
    }

    /// Test-facing: the seed the spread and the melee rolls draw from, so a
    /// gate that needs a shot to land where it did once can have it again.
    pub fn test_seed_rng(&mut self, seed: u64) {
        self.rng = seed | 1;
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
    /// (combat doc, 14.3), against the world alone: the script models the
    /// live blast also meets are left out. 0 is a player the blast cannot
    /// see at all.
    pub fn test_can_damage(&self, at: [f32; 3], feet: [f32; 3]) -> f32 {
        let Some(world) = self.world.as_ref() else {
            return 1.0;
        };
        let v = standing_blast_victim(feet);
        crate::game::combat::can_damage(glam::Vec3::from(at), &v, &world.collision, &[], &[], None)
    }

    /// Test-facing, beside `test_can_damage`: the same standing victim at
    /// `feet`, against the world, the script models and every live player's
    /// posed body but `slot`'s own, which is the victim's pass entity
    /// (combat doc, 14.4).
    pub fn test_can_damage_among_players(
        &mut self,
        at: [f32; 3],
        feet: [f32; 3],
        slot: usize,
    ) -> f32 {
        let Some(world) = self.world.as_ref() else {
            return 1.0;
        };
        let bodies: Vec<crate::game::combat::HitBody> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref()?.sim.as_ref()?.hit_body(i))
            .collect();
        let models = self
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.placed_script_models());
        let mut bones = match (self.fs.as_deref(), self.anims.as_deref()) {
            (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                fs,
                anims,
                rigs: &mut self.hit_rigs,
                now_ms: self.sv_time_ms,
            }),
            _ => None,
        };
        let mut v = standing_blast_victim(feet);
        v.slot = slot;
        crate::game::combat::can_damage(
            glam::Vec3::from(at),
            &v,
            &world.collision,
            &models,
            &bodies,
            bones.as_mut(),
        )
    }

    /// Test-facing: the length of the bots' planned route from the graph
    /// node nearest `from` to the one nearest `to`, or `None` when either
    /// point is off the graph, no route exists, or the graph is not built
    /// yet. What a bot scenario needs to put a walker where it can reach its
    /// goal. On the graph means within a pitch of a node on the same floor:
    /// `nearest` alone answers from a bank for a point down in a ditch.
    pub fn test_nav_route(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        let g = self.nav.as_ref()?;
        let on = |p: [f32; 3]| {
            let n = g.nearest(p)?;
            let d = g.nodes[n as usize] - glam::Vec3::from(p);
            (d.truncate().length() <= g.spacing && d.z.abs() < 18.0).then_some(n)
        };
        let path = g.path(on(from)?, on(to)?)?;
        Some(
            path.windows(2)
                .map(|w| g.nodes[w[0] as usize].distance(g.nodes[w[1] as usize]))
                .sum(),
        )
    }

    /// Test-facing: where a standing player dropped at `p` comes to rest,
    /// or `None` when it starts inside geometry or finds no floor within
    /// 256 units. What a test needs to put a second client somewhere the map
    /// actually holds one.
    pub fn test_ground_under(&self, p: [f32; 3]) -> Option<[f32; 3]> {
        let world = self.world.as_ref()?;
        use vcod_common::pmove::{HALF_WIDTH, Stance};
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

    /// `SV_DropClient` (cod_lnxded 0x8085cf4). The game hears the disconnect
    /// at once; the slot goes `CS_ZOMBIE` with the drop notice queued, and
    /// `send_zombies` keeps sending it until [`ZOMBIE_TIME`] past the
    /// client's last packet. A bot has nobody to tell and is freed now.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(mut c) = self.clients[slot].take() else {
            return;
        };
        let collision = self.world.as_ref().map(|w| &w.collision);
        pass_followers_on(&mut self.clients, slot, self.script.as_mut(), collision);
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
        log::debug!("Going to CS_ZOMBIE for {}", c.name);
        // Everyone else is told unless the client left on its own.
        if reason != "EXE_DISCONNECTED" {
            let notice = format!("e \"\u{15}{}^7 \u{14}{reason}\"", c.name);
            for other in 0..self.clients.len() {
                if self.clients[other]
                    .as_ref()
                    .is_some_and(|o| o.state != ClientState::Connected)
                {
                    self.send_server_command(other, &notice);
                }
            }
        }
        self.print(&format!("{slot}:{} {reason}\n", c.name));
        // Appended with no squash or guard: the notice must not be dropped,
        // and a full ring cannot drop a client twice.
        c.netchan.reliable_sequence += 1;
        let at = c.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
        c.netchan.reliable[at] = format!("w \"{reason}\"");
        c.reliable_kind[at] = CmdKind::Reliable;
        if !c.is_bot {
            self.zombies[slot] = Some(c);
        }
        // The last client gone (0x8085ebc).
        if self.client_count() == 0 {
            self.masters.force();
        }
    }

    /// A zombie's packet still runs the netchan and records the acks, so a
    /// drop notice the client has seen stops being resent; nothing in it is
    /// executed and it does not hold the slot open (0x808ca44).
    fn zombie_packet(&mut self, slot: usize, pkt: &[u8]) {
        let Some(z) = self.zombies[slot].as_mut() else {
            return;
        };
        let Some(m) = z.netchan.process_in(pkt) else {
            return;
        };
        let seq = z.netchan.reliable_sequence as i32;
        if (0..z.netchan.outgoing_sequence as i32).contains(&m.message_ack) {
            z.message_ack = m.message_ack;
        }
        if (0..=seq).contains(&m.reliable_ack)
            && seq - m.reliable_ack < MAX_RELIABLE_COMMANDS as i32
        {
            z.reliable_ack = m.reliable_ack;
        }
    }

    /// `SV_WriteDownloadToClient` (0x8086290) for every client with a
    /// download, in a message of its own: a downloading client is still
    /// primed and gets no snapshot. Only paks this server mounts and lists in
    /// `sv_referencedPakNames` go out; retail serves any file of the name.
    fn send_downloads(&mut self) {
        let allow = self
            .level_cvar("sv_allowDownload")
            .is_none_or(|v| v.trim().parse::<i32>().unwrap_or(0) != 0);
        let pure = self.paks.pure;
        let dedicated = self.dedicated;
        let now = self.sv_time_ms;
        let fs = self.fs.clone();
        let open = |name: &str| -> Result<Vec<u8>, crate::download::Refusal> {
            use crate::download::Refusal;
            let stem = name.strip_suffix(".pk3").unwrap_or(name);
            if vcod_common::net::download::is_stock_pak(stem) {
                return Err(Refusal::GamePak);
            }
            if !allow {
                return Err(Refusal::Disabled { pure });
            }
            fs.as_deref()
                .and_then(|fs| {
                    fs.paks()
                        .find(|p| p.qualified_name().eq_ignore_ascii_case(stem))
                })
                .and_then(|p| std::fs::read(&p.path).ok())
                .filter(|b| !b.is_empty())
                .ok_or(Refusal::NotFound)
        };
        for c in self.clients.iter_mut().flatten() {
            if c.download.is_none() {
                continue;
            }
            let budget = crate::download::blocks_per_message(c.rate(dedicated), 50);
            let mut w = MsgWriter::new(&self.huff);
            write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
            c.reliable_sent = c.netchan.reliable_sequence as i32;
            if let Some(dl) = c.download.as_mut()
                && !dl.write(&mut w, now, budget, open)
            {
                c.download = None;
            }
            let sent = c
                .netchan
                .transmit(c.last_client_command, &w.into_ops(), &self.huff);
            self.outbox.extend(sent.into_iter().map(|p| (c.addr, p)));
        }
    }

    /// `SV_SendClientSnapshot` for a zombie (0x808f844): its unacked server
    /// commands, the drop notice among them, and a snapshot. Retail skips
    /// `SV_BuildClientSnapshot` for a zombie and writes whatever stale frame
    /// sits in the ring slot; this repeats the last frame sent instead, which
    /// carries the new command sequence the same way.
    fn send_zombies(&mut self) {
        for z in self.zombies.iter_mut().flatten() {
            let sent =
                resend_last_frame(z, self.sv_time_ms, self.proto, &self.huff, &self.baselines);
            self.outbox.extend(sent.into_iter().map(|p| (z.addr, p)));
        }
    }

    /// `SV_FinalMessage` (0x808ad8c), which `SV_Shutdown` runs before the
    /// process exits: twice over, every client past `CS_ZOMBIE` is queued
    /// `e "<message>"` and a bare `w` and sent a snapshot carrying them.
    /// The `e` is dropped for a client not yet active.
    fn final_message(&mut self, message: &str) {
        let notice = format!("e \"{message}\"");
        for _ in 0..2 {
            for slot in 0..self.clients.len() {
                if self.clients[slot].as_ref().is_none_or(|c| c.is_bot) {
                    continue;
                }
                self.send_server_command(slot, &notice);
                self.send_server_command(slot, "w");
                let Some(c) = self.clients[slot].as_mut() else {
                    continue;
                };
                let sent =
                    resend_last_frame(c, self.sv_time_ms, self.proto, &self.huff, &self.baselines);
                self.outbox.extend(sent.into_iter().map(|p| (c.addr, p)));
            }
        }
    }

    /// `SV_CalcPings` (0x808cab8), once a frame after the packets and before
    /// the clock moves. Only an active client in the world is measured; a
    /// bot, which retail does not have, reads 0 as Q3's do.
    fn calc_pings(&mut self) {
        for c in self.clients.iter_mut().flatten() {
            c.ping = if c.is_bot {
                0
            } else if c.state != ClientState::Active || c.sim.is_none() {
                999
            } else {
                c.calc_ping()
            };
        }
    }

    /// `SV_CheckTimeouts` (0x808cbc0) with `sv_timeout` and no timeoutCount
    /// hysteresis. A zombie is freed [`ZOMBIE_TIME`] past its last packet; a
    /// timed-out client is freed at once, its notice never sent, as retail
    /// does.
    fn check_timeouts(&mut self, now: Instant) {
        for z in &mut self.zombies {
            if let Some(c) =
                z.take_if(|c| now.saturating_duration_since(c.last_packet) > ZOMBIE_TIME)
            {
                log::debug!("Going from CS_ZOMBIE to CS_FREE for {}", c.name);
            }
        }
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
            self.zombies[slot] = None;
        }
    }

    /// The bots' pass of the tick: join like a client, then move like one.
    /// Runs before `replay_moves`, so a bot's cmd for this frame is in its
    /// queue when the sim reads it. Four passes, because the brains need the
    /// whole of `self` to think and their writes land after.
    fn step_bots(&mut self) {
        let mut noises = std::mem::take(&mut self.bot_noises);
        // A script's `radiusDamage` (the S&D bomb, an exploder) is nobody's.
        if let Some(rt) = self.script.as_mut() {
            noises.extend(
                rt.take_blast_noises()
                    .into_iter()
                    .map(|at| crate::bots::Noise {
                        at: [at[0], at[1], at[2] + 40.0],
                        source: usize::MAX,
                        radius: crate::bots::HEAR_BLAST,
                    }),
            );
        }
        if self.cfg.bots == 0 {
            return;
        }
        if !self.bots_spawned {
            self.bots_spawned = true;
            self.spawn_bots();
        }
        if self.nav.is_none()
            && let Some(w) = self.world.as_ref()
        {
            let job = self
                .nav_job
                .get_or_insert_with(|| crate::nav::NavJob::start(&self.cfg.map, w));
            self.nav = job.poll(!self.nav_background);
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
        // `_teams::restrict` reads `!getcvar(...)`, a numeric coercion; with
        // no script nothing restricts.
        let cvars = self.script.as_ref().map(|rt| rt.cvars());
        let allowed = |name: &str| {
            cvars.is_none_or(|c| crate::game::builtins::cvar::atof(c.get(name)) != 0.0)
        };
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
            replies.extend(
                bot.observe(sid, &fresh, &allowed)
                    .into_iter()
                    .map(|r| (slot, r)),
            );
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
        let sd = self.bot_sd();
        let re = self.bot_re();
        let bel = self.bot_bel();
        let mut views: Vec<(usize, crate::bots::BotView)> = slots
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
                let mut view = self.bot_view(*slot, enemy)?;
                view.noise = crate::bots::loudest(&noises, *slot, view.origin);
                Some((*slot, view))
            })
            .collect();
        if let Some((attackers, defenders, objectives)) = &re {
            let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
            let slots: Vec<usize> = views.iter().map(|(s, _)| *s).collect();
            for (slot, view) in views.iter_mut() {
                let mine = team(*slot);
                let role = if mine == *attackers {
                    crate::bots::ObjRole::Attack
                } else if mine == *defenders {
                    crate::bots::ObjRole::Defend
                } else {
                    continue;
                };
                view.re = Some(crate::bots::ReView {
                    role,
                    rank: slots
                        .iter()
                        .filter(|s| team(**s) == mine && *s < slot)
                        .count(),
                    objectives: objectives
                        .iter()
                        .map(|(o, carrier, shown_to)| crate::bots::ReObjView {
                            mine: *carrier == Some(*slot),
                            // What the bot's compass shows.
                            carrier_at: o
                                .carrier_at
                                .filter(|_| *shown_to == 0 || *shown_to == mine),
                            ..*o
                        })
                        .collect(),
                });
            }
        }
        if let Some(markers) = &bel {
            for (slot, view) in views.iter_mut() {
                let mine = teams.get(*slot).copied().unwrap_or(0);
                if mine != script::TEAM_AXIS && mine != script::TEAM_ALLIES {
                    continue;
                }
                view.bel = Some(crate::bots::BelView {
                    hunted: mine == script::TEAM_ALLIES,
                    // What the compass shows the team (`objectives_for`).
                    markers: markers
                        .iter()
                        .filter(|(_, t, _)| *t == 0 || *t == mine)
                        .map(|(_, _, at)| *at)
                        .collect(),
                    // `make_obj_marker` numbers a player's record
                    // entnum + 1 (`bel.gsc` 1574).
                    trail: markers
                        .iter()
                        .find(|(n, _, _)| *n == *slot + 1)
                        .map(|(_, _, at)| *at),
                });
            }
        }
        if let Some((attackers, defenders, sd)) = &sd {
            let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
            // The team's bots that play, by slot, and each one's distance
            // to the bomb.
            let bomb_dist = |v: &crate::bots::BotView| {
                sd.bomb
                    .map_or(0.0, |b| crate::bots::dist_sq(v.origin, b.origin))
            };
            let side: Vec<(usize, i32, f32, bool)> = views
                .iter()
                .map(|(slot, v)| (*slot, team(*slot), bomb_dist(v), v.playing))
                .collect();
            for (slot, view) in views.iter_mut() {
                let mine = team(*slot);
                let role = if mine == *attackers {
                    crate::bots::ObjRole::Attack
                } else if mine == *defenders {
                    crate::bots::ObjRole::Defend
                } else {
                    continue;
                };
                let rank = side.iter().filter(|s| s.1 == mine && s.0 < *slot).count();
                // Nearest first, the lower slot on a tie.
                let lead = side
                    .iter()
                    .filter(|s| s.1 == mine && s.3)
                    .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)))
                    .is_some_and(|s| s.0 == *slot);
                view.sd = Some(crate::bots::SdView {
                    role,
                    rank,
                    lead,
                    ..sd.clone()
                });
            }
        }

        // Pass 3: the tick's waypoint toward each brain's goal, then its
        // cmd. A* is capped per tick across all bots; one that misses out
        // wanders for a tick and asks again.
        let mut moves: Vec<(usize, UserCmd)> = Vec::new();
        let mut budget = BOT_PLAN_BUDGET;
        for (slot, mut view) in views {
            let Some(goal) = self.bots.get(&slot).map(|b| b.goal(&view)) else {
                continue;
            };
            if let Some(g) = self.nav.clone() {
                let mut follower = self.bot_paths.remove(&slot).unwrap_or_default();
                let ps = self.clients[slot]
                    .as_ref()
                    .and_then(|c| c.sim.as_ref())
                    .map(|sim| sim.ps);
                let body = crate::nav::Footing {
                    still: ps.as_ref().is_some_and(crate::nav::ready_to_leap),
                    on_ground: ps.as_ref().is_some_and(|ps| ps.on_ground),
                };
                // The generator alone, so the world stays readable.
                let rng = &mut self.rng;
                let mut rand = || rand_from(rng);
                let at = view.origin;
                let world = self.world.as_ref().map(|w| &w.collision);
                view.waypoint =
                    follower.waypoint(&g, world, goal, at, body, &mut budget, &mut rand);
                if let Some(p) = follower.climb_line(&g) {
                    view.ladder_middle = Some(p.into());
                }
                view.jump = follower.jumping(&g);
                view.leap = follower.leaping(&g);
                view.stop = follower.holding(&g);
                view.lip = view.jump
                    && matches!((self.world.as_ref(), ps), (Some(w), Some(ps))
                        if crate::nav::lip_ahead(&w.collision, &ps));
                self.bot_paths.insert(slot, follower);
            }
            if let Some(bot) = self.bots.get_mut(&slot) {
                let cmd = bot.think(&view);
                moves.push((slot, cmd));
            }
        }

        // Pass 4: everything lands.
        self.bots.retain(|slot, _| self.clients[*slot].is_some());
        self.bot_enemies
            .retain(|slot, _| self.clients[*slot].is_some());
        self.bot_paths
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
            // Enters the world on the Primed pass, queues on Active. Each
            // bot's cmd is a packet of its own, after every real one.
            self.packet_seq += 1;
            self.user_move(slot, vec![cmd]);
        }
        for slot in entering {
            // The entering cmd carries the tick's clock: entry sets the
            // client's cmd base to it, so the first simulated cmd steps a
            // sane 50 ms.
            self.packet_seq += 1;
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
            let Some(slot) = self.free_slot() else {
                log::warn!("no free slot for bot {}", i + 1);
                return;
            };
            let team = if i % 2 == 0 { "allies" } else { "axis" };
            // `bel`'s menu takes axis only and the script deals the allied
            // places, so that is the team the bot asks for there.
            let bel = self.level_cvars.as_ref().is_some_and(|(g, _)| g == "bel");
            let asks = if bel { "axis" } else { team };
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
            log::info!("client {slot} {name:?} connected (bot, team {asks})");
        }
    }

    /// One slot's `StuckInClient` view: `None` for a free slot, `own_view`
    /// false for a connected client with no sim yet, a spectator or an
    /// intermission client.
    fn stuck_view(c: &Option<Client>) -> Option<StuckView> {
        let client = c.as_ref()?;
        let Some(sim) = client.sim.as_ref() else {
            return Some(StuckView {
                own_view: false,
                playing: false,
                health: 0,
                contents: 0,
                origin: glam::Vec3::ZERO,
                mins: glam::Vec3::ZERO,
                maxs: glam::Vec3::ZERO,
                vel_xy: glam::Vec2::ZERO,
                speed: 0.0,
            });
        };
        Some(StuckView {
            own_view: sim.pm_type == PmType::Normal,
            playing: sim.pm_type == PmType::Normal && !sim.dead,
            health: sim.health,
            contents: sim.contents,
            origin: sim.ps.origin,
            mins: sim.ps.mins(),
            maxs: sim.ps.maxs(),
            vel_xy: sim.ps.velocity.truncate(),
            speed: vcod_common::pmove::SPEED_RUN,
        })
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
        let held = |pick: &dyn Fn(&vcod_common::weapon::WeaponDef) -> bool| {
            (1u8..64).find(|i| {
                sim.ps.weapons_held >> i & 1 == 1 && weapons.get(*i as usize).is_some_and(pick)
            })
        };
        let grenade = held(&|d| d.weapon_type == "grenade");
        let pistol = held(&|d| d.weapon_slot == "pistol");
        let ms = |s: f32| (s * 1000.0) as i32;
        // A segmented reload's first round lands after its start segment
        // and one loop segment.
        let reload_ms = def.map_or(0, |d| {
            if d.segmented_reload {
                ms(d.reload_start_time + d.reload_time)
            } else if d.reload_empty_time > 0.0 {
                ms(d.reload_empty_time)
            } else {
                ms(d.reload_time)
            }
        });
        let draw_ms = pistol
            .and_then(|p| weapons.get(p as usize))
            .map_or(0, |p| ms(def.map_or(0.0, |d| d.drop_time) + p.raise_time));
        let loaded = (1u8..64)
            .filter(|&i| sim.ps.weapons_held >> i & 1 == 1)
            .filter(|&i| {
                weapons.get(i as usize).is_some_and(|d| {
                    let reserve = if d.clip_only {
                        0
                    } else {
                        sim.ps.ammo[d.ammo_index]
                    };
                    sim.ps.ammoclip[d.clip_index] as i32 + reserve as i32 > 0
                })
            })
            .fold(0u64, |m, i| m | 1 << i);
        Some(crate::bots::BotView {
            origin: sim.ps.origin.into(),
            delta_angles: sim.delta_angles(),
            view: sim.view_angles(),
            weapon: sim.ps.weapon,
            weapons_held: sim.ps.weapons_held,
            clip: def.map_or(-1, |d| sim.ps.ammoclip[d.clip_index]),
            fire_time_ms: def.map_or(0, |d| (d.fire_time * 1000.0) as i32),
            busy_ms: sim.ps.weapon_time_ms,
            automatic: def.is_some_and(|d| !d.semi_auto),
            has_ads: def.is_some_and(|d| d.aim_down_sight),
            sniper: def.is_some_and(|d| d.ads_overlay_shader.is_some()),
            ads_frac: sim.ps.weapon_pos_frac,
            speed: sim.ps.velocity.truncate().length(),
            velocity: sim.ps.velocity.into(),
            dead: sim.dead,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            enemy,
            grenade,
            waypoint: None,
            hazard_ahead: std::array::from_fn(|i| {
                self.world.as_ref().is_some_and(|w| {
                    let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
                    let ahead = glam::Vec3::new(c, s, 0.0) * crate::bots::HAZARD_LOOK;
                    crate::nav::hazard(&w.hazards, sim.ps.origin + ahead)
                })
            }),
            drop_ahead: std::array::from_fn(|i| {
                let standing = sim.ps.on_ground && !sim.ps.on_ladder;
                let world = self.world.as_ref().filter(|_| standing);
                world.and_then(|w| {
                    let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
                    let dir = glam::Vec3::new(c, s, 0.0);
                    let look = crate::bots::HAZARD_LOOK;
                    crate::nav::drop_ahead(&w.collision, sim.ps.origin, dir, look)
                })
            }),
            linked: sim.link_to.is_some(),
            on_ladder: sim.ps.on_ladder,
            ladder_normal: sim.ps.ladder_normal.into(),
            ladder_middle: self
                .nav
                .as_ref()
                .filter(|_| sim.ps.on_ladder)
                .and_then(|g| g.ladder_middle(sim.ps.origin))
                .map(Into::into),
            on_ground: sim.ps.on_ground,
            jump: false,
            leap: false,
            lip: false,
            stop: false,
            pistol,
            reload_ms,
            draw_ms,
            loaded,
            sd: None,
            re: None,
            bel: None,
            noise: None,
        })
    }

    /// The S&D objectives for this frame's bot views, on an `sd` level only:
    /// the attacking and defending team values and the view with a
    /// placeholder role. Read fresh every frame, since a `map_restart`
    /// respawns the zones under new handles.
    fn bot_sd(&mut self) -> Option<(i32, i32, crate::bots::SdView)> {
        if self.level_cvars.as_ref().is_none_or(|(g, _)| g != "sd") {
            return None;
        }
        let o = self.script.as_mut()?.sd_objectives();
        let team = |t: &str| match t {
            "axis" => script::TEAM_AXIS,
            "allies" => script::TEAM_ALLIES,
            _ => -1,
        };
        let sites = o
            .sites
            .iter()
            .map(|&(mins, maxs)| crate::bots::SiteView {
                mins,
                maxs,
                stand: self.site_stand(mins, maxs),
            })
            .collect();
        let bomb = o.bomb.map(|(origin, (lo, hi))| crate::bots::BombView {
            origin,
            aim: std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5),
        });
        Some((
            team(&o.attackers),
            team(&o.defenders),
            crate::bots::SdView {
                role: crate::bots::ObjRole::Attack,
                sites,
                bomb,
                rank: 0,
                lead: false,
            },
        ))
    }

    /// The live objective records as `(index, teamNum, origin)`, on a `bel`
    /// level only: the allied players' compass markers.
    fn bot_bel(&self) -> Option<Vec<(usize, i32, [f32; 3])>> {
        if self.level_cvars.as_ref().is_none_or(|(g, _)| g != "bel") {
            return None;
        }
        let rt = self.script.as_ref()?;
        Some(
            rt.host
                .objectives
                .iter()
                .enumerate()
                .filter(|(_, o)| o.state != 0)
                .map(|(i, o)| (i, o.team_num, o.origin_f32()))
                .collect(),
        )
    }

    /// The retrieval objectives for this frame's bot views, on an `re` level
    /// only: the attacking and defending team values and each objective,
    /// with `mine` and the compass filter left for the caller.
    fn bot_re(&mut self) -> Option<(i32, i32, Vec<ReObjCarried>)> {
        if self.level_cvars.as_ref().is_none_or(|(g, _)| g != "re") {
            return None;
        }
        let clients = self.clients.len();
        let o = self.script.as_mut()?.re_objectives(clients);
        let team = |t: &str| match t {
            "axis" => script::TEAM_AXIS,
            "allies" => script::TEAM_ALLIES,
            _ => -1,
        };
        let objectives = o
            .objectives
            .iter()
            .map(|r| {
                let mid = |(lo, hi): ([f32; 3], [f32; 3])| -> [f32; 3] {
                    std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5)
                };
                let (lo, hi) = r.goal;
                let goal = self.site_stand(lo, hi);
                // The farthest corner, flat, and a body's half width past it.
                let goal_clear = [lo[0], hi[0]]
                    .iter()
                    .flat_map(|&x| [lo[1], hi[1]].map(|y| (x - goal[0]).hypot(y - goal[1])))
                    .fold(0.0, f32::max)
                    + 16.0;
                let carrier_at = r.carrier.and_then(|slot| {
                    let sim = self.clients.get(slot)?.as_ref()?.sim.as_ref()?;
                    Some(sim.ps.origin.into())
                });
                let view = crate::bots::ReObjView {
                    pickup: r.pickup.map(mid),
                    goal,
                    goal_clear,
                    carried: r.carrier.is_some(),
                    mine: false,
                    carrier_at,
                    laid: r.laid,
                };
                (view, r.carrier, r.shown_to)
            })
            .collect();
        Some((team(&o.attackers), team(&o.defenders), objectives))
    }

    /// Where a bot stands in a zone (a bombzone, a retrieval goal): the
    /// graph node inside the bounds nearest their middle, from the
    /// component holding the most nodes so a bot anywhere can reach it,
    /// else the nearest such node to the middle. Cached per zone; the zones
    /// keep their bounds across rounds.
    fn site_stand(&mut self, mins: [f32; 3], maxs: [f32; 3]) -> [f32; 3] {
        let mid = [
            (mins[0] + maxs[0]) * 0.5,
            (mins[1] + maxs[1]) * 0.5,
            mins[2],
        ];
        if let Some(&(_, _, p)) = self
            .bot_sites
            .iter()
            .find(|(lo, hi, _)| *lo == mins && *hi == maxs)
        {
            return p;
        }
        let Some(g) = self.nav.clone() else {
            return mid;
        };
        let comp = g.components();
        let mut sizes: BTreeMap<u32, usize> = BTreeMap::new();
        for c in &comp {
            *sizes.entry(*c).or_default() += 1;
        }
        // Largest first, lower number on a tie, for a pick that does not
        // depend on map iteration.
        let main = sizes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(c, _)| *c);
        let mid_v = glam::Vec3::from(mid);
        let inside = |n: &glam::Vec3| {
            (0..2).all(|i| n[i] >= mins[i] + 16.0 && n[i] <= maxs[i] - 16.0)
                && n.z <= maxs[2]
                && n.z + 70.0 >= mins[2]
        };
        let best = |want_inside: bool| {
            g.nodes
                .iter()
                .enumerate()
                .filter(|(i, n)| Some(comp[*i]) == main && (!want_inside || inside(n)))
                .min_by(|a, b| a.1.distance(mid_v).total_cmp(&b.1.distance(mid_v)))
                .map(|(_, n)| (*n).into())
        };
        let p = best(true).or_else(|| best(false)).unwrap_or(mid);
        self.bot_sites.push((mins, maxs, p));
        p
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
                    slot: i,
                    origin: (s.ps.origin + glam::Vec3::Z * 40.0).into(),
                    velocity: s.ps.velocity.into(),
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

    /// Test-facing: the shots and blasts the bots hear next tick.
    pub fn bot_noises(&self) -> &[crate::bots::Noise] {
        &self.bot_noises
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
            on_ladder: sim.ps.on_ladder,
            on_ground: sim.ps.on_ground,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            dead: sim.dead,
            health: sim.health,
            clip: {
                let d = self.weapon_table.get(sim.ps.weapon as usize);
                d.map_or(-1, |d| sim.ps.ammoclip[d.clip_index])
            },
            unsticking: self.bots.get(&slot).is_some_and(|b| b.unsticking()),
        })
    }

    /// Test-facing: a bot's current waypoint node and the next, as points.
    pub fn bot_waypoints(&self, slot: usize) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
        let (Some(g), Some(f)) = (self.nav.as_ref(), self.bot_paths.get(&slot)) else {
            return (None, None);
        };
        let at = |n: Option<u32>| n.map(|n| g.nodes[n as usize].into());
        let (a, b) = f.current();
        (at(a), at(b))
    }

    /// Test-facing: the script team value a bot landed in.
    pub fn bot_team(&mut self, slot: usize) -> i32 {
        self.script.as_mut().map_or(0, |rt| rt.client_team(slot))
    }

    /// Builds the bots' navigation graph off the tick thread: the server
    /// keeps real time through the build and the bots wander until it lands.
    /// Off by default, so a test's bots find the graph on their first tick.
    pub fn build_nav_in_background(&mut self, on: bool) {
        self.nav_background = on;
    }

    /// Swap in the map built by the binary; tests run without one.
    pub fn load_world(&mut self, world: World) {
        self.world = Some(Rc::new(world));
        self.nav = None;
        self.nav_job = None;
        self.bot_paths.clear();
        self.bot_sites.clear();
        self.bot_noises.clear();
    }

    /// The cvar table a gametype script starts with: the engine defaults,
    /// then `default_mp.cfg`, then `g_gametype`, `sv_hostname` and
    /// `sv_maxclients` mirroring this run's `ServerConfig`, and the engine
    /// cvars `Server` keeps in its own fields. None of
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
        // A level load is where a latched value goes live.
        cvars.apply_latched();
        cvars.set("g_gametype", &self.cfg.gametype);
        // `SV_SpawnServer`'s `Cvar_Set("mapname", ...)` (map-cycle doc,
        // section 3 step 15), which a script's `getCvar("mapname")` reads.
        cvars.set("mapname", &self.cfg.map);
        cvars.set("sv_hostname", &self.cfg.hostname);
        cvars.set("sv_maxclients", &self.cfg.max_clients.to_string());
        cvars.set("dedicated", &self.dedicated.to_string());
        cvars.set("sv_pure", "0");
        cvars.set("sv_running", "1");
        cvars.set("sv_serverid", &self.server_id.to_string());
        cvars.set("sv_cheats", if self.cheats { "1" } else { "0" });
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
    /// instead of the paks on every later level load. `--gametype-script`
    /// uses it to run a client probe as the gametype, and the tests that need
    /// the real console and restart paths under a probe script call it
    /// directly.
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
        // `SV_SpawnServer` step 23. `sv_pure` reads the `--set` override;
        // vcod defaults it to 0 where retail's default is 1.
        let pure = cvars.get("sv_pure").trim().parse::<i32>().unwrap_or(0) != 0;
        self.paks = configstrings::PakLists::new(pure, fs.paks());
        if let Some(cs0) = self.configstrings.get_mut(0) {
            *cs0 = configstrings::with_sv_pure(cs0, pure);
        }
        self.pure_check = Some(PureCheck::new(&fs, self.checksum_feed));
        if !restart {
            self.anims = match vcod_common::animtree::PlayerAnims::load(&fs) {
                Ok(a) => Some(Rc::new(a)),
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
            area: carry.area.filter(|_| restart),
        };
        let source = crate::game::script::PakScripts::new(fs.clone(), self.script_overlay.clone());
        let mut rt = crate::game::script::ScriptRuntime::load_from(
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
        rt.set_player_anims(self.anims.clone());
        let mut configstrings = rt.configstrings().to_vec();
        rt.cvars()
            .write_mirror(&mut configstrings)
            .map_err(|e| anyhow::anyhow!("writing the cvar mirror: {e:?}"))?;
        self.configstrings = configstrings;
        self.script = Some(rt);
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
        // `SV_SpawnServer` and `SV_MapRestart` both clear it, the flag
        // included, and the new level's `main` turns it back on.
        self.archive.clear();
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
        self.apply_engine_cvar(name, value);
        // Latched: serverinfo keeps the running level's value until a load,
        // and takes this one at once only while no level has loaded.
        if name == "g_gametype" {
            self.cfg.gametype = value.to_string();
            self.refresh_serverinfo();
        }
    }

    /// The engine cvars `Server` keeps in its own fields. Names fold case.
    fn apply_engine_cvar(&mut self, name: &str, value: &str) {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = crate::master::Masters::index(&lower) {
            self.masters.set(i, value);
        }
        match lower.as_str() {
            "dedicated" => self.dedicated = value.trim().parse().unwrap_or(0),
            "rconpassword" => self.rcon_password = value.to_string(),
            "sv_maprotation" => self.sv_map_rotation = value.to_string(),
            "sv_hostname" => {
                self.cfg.hostname = value.to_string();
                self.refresh_serverinfo();
            }
            "sv_privateclients" => self.refresh_serverinfo(),
            _ => {}
        }
    }

    /// `Cvar_InfoString(CVAR_SERVERINFO)` with the live cvars the config
    /// does not hold.
    fn serverinfo(&self) -> Info {
        let mut i = configstrings::serverinfo(&self.live_cfg());
        i.set("sv_privateClients", self.private_clients())
            .set("sv_pure", u8::from(self.paks.pure));
        i
    }

    /// `SV_Frame`'s cvar flush, the serverinfo half: a write to a cvar the
    /// serverinfo string carries is followed by
    /// `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (docs/research/cod11-map-cycle.md, 4.3). Both tables, because the
    /// script owns its own copy between level loads and `tick` reads that one
    /// back over this one every frame.
    fn refresh_serverinfo(&mut self) {
        let info = self.serverinfo().to_string();
        if let Some(slot) = self.configstrings.get_mut(0) {
            *slot = info.clone();
        }
        if let Some(rt) = self.script.as_mut()
            && let Some(slot) = rt.host.configstrings.get_mut(0)
        {
            *slot = info;
        }
    }

    /// `G_UpdateCvars` for the two fall bounds and `SV_Frame`'s systeminfo
    /// flush behind them: the moves read the values the script's table holds
    /// now, and slot 1 carries them to the clients that predict with them.
    /// Both tables, as [`Self::refresh_serverinfo`] writes both.
    fn refresh_fall_heights(&mut self) {
        use crate::game::builtins::cvar::atof;
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        let cvars = rt.cvars();
        self.fall_heights = FallHeights {
            min: atof(cvars.get("bg_fallDamageMinHeight")),
            max: atof(cvars.get("bg_fallDamageMaxHeight")),
        };
        let info =
            configstrings::systeminfo(self.server_id, self.fall_heights, self.cheats, &self.paks)
                .to_string();
        if let Some(slot) = rt.host.configstrings.get_mut(1) {
            slot.clone_from(&info);
        }
        if let Some(slot) = self.configstrings.get_mut(1) {
            *slot = info;
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
        self.pending_explosions.clear();
        self.pending_script_commands.clear();
        self.weapon_changes.clear();
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
        self.configstrings = configstrings::static_configstrings(
            &self.cfg,
            self.server_id,
            self.fall_heights,
            self.cheats,
            &self.paks,
        );
        self.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
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
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
        // Step 21.
        self.rebuild_baselines();
        // Step 22, after the settle frames and the baselines: `ClientConnect`
        // again for everyone still on a netchan, then back to `CS_CONNECTED`.
        // A denial drops the client (0x808a76f).
        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_ref() else {
                continue;
            };
            if self.password_denied(&c.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                continue;
            }
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
        // The masters hear of the new map on the next frame (0x808a915).
        self.masters.force();
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
        self.pending_explosions.clear();
        self.pending_script_commands.clear();
        self.weapon_changes.clear();
        self.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 5: only the low nibble moves.
        self.server_id = console::next_restart_id(self.server_id);
        // `sv.configstrings` survives a restart: only a spawn clears it, so
        // every slot the outgoing level allocated keeps its index and its
        // text, and the incoming level's `G_ModelIndex` finds the same slot
        // (map-cycle doc, 4.6). The static slots are rebuilt around the new
        // id on top; 4.3 is what carries slot 1 to a client that already
        // has a gamestate.
        for (i, s) in configstrings::static_configstrings(
            &self.cfg,
            self.server_id,
            self.fall_heights,
            self.cheats,
            &self.paks,
        )
        .into_iter()
        .enumerate()
        {
            if !s.is_empty() {
                self.configstrings[i] = s;
            }
        }
        self.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
        // Step 7: `SV_RestartGameProgs(savePersist)`. Past the teardown: the
        // outgoing level's script is gone and a failure here leaves none.
        self.print(RESTART_GAME_BANNER);
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
        self.refresh_fall_heights();
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
            let Some(c) = self.clients[slot].as_ref() else {
                continue;
            };
            // A denial drops the client (0x8083f99).
            if self.password_denied(&c.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                self.print(&format!(
                    "SV_MapRestart_f: dropped client {slot} - denied!\n"
                ));
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

    /// `Cbuf_Execute`: every queued line through [`Self::exec_console_line`].
    /// A command pushed by a builtin during the script pass runs here, at the
    /// top of the next tick, which is `EXEC_APPEND`.
    fn drain_console(&mut self, now: Instant) {
        while let Some(line) = self.console.pop_front() {
            if !self.exec_console_line(&line, now) || self.quit {
                return;
            }
        }
    }

    /// `Cmd_ExecuteString` for the commands the map cycle and rcon use, then
    /// `Cvar_Command`; anything else logs and drops, and prints nothing, as
    /// retail's does for a command no one registered. `false` when a load left no level,
    /// which stops the drain.
    ///
    /// A load that failed before the teardown -- no paks, a map the paks do
    /// not have, a bsp that would not parse -- leaves the level that is
    /// serving alone and is logged. One that failed after it -- the map or
    /// gametype scripts -- has no level left to fall back to, so it ends the
    /// process through [`Self::take_fatal`].
    fn exec_console_line(&mut self, line: &str, now: Instant) -> bool {
        match console::Command::parse(line) {
            console::Command::Map { map, bsp, cheats } => {
                if self
                    .fs
                    .as_ref()
                    .is_some_and(|fs| fs.resolve_map(&map).is_none())
                {
                    self.print(&format!("Can't find map {bsp}\n"));
                    return true;
                }
                // `map <the map already serving>` is a restart, not a
                // spawn (doc section 4.2).
                let r = if map.eq_ignore_ascii_case(&self.cfg.map) && self.script.is_some() {
                    self.map_restart()
                } else {
                    self.spawn_server(&map)
                };
                if !self.absorb_load(&format!("map {map}"), r) {
                    return false;
                }
                // After the load, so the gamestate it sent still carries the
                // old value and the change follows as a configstring update.
                self.set_cheats(cheats);
                if self.script.is_some() {
                    self.killed = false;
                }
            }
            console::Command::MapRestart => {
                let r = self.map_restart();
                return self.absorb_load("map_restart", r);
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
            console::Command::Status => {
                let text = self.status_text(now);
                self.print(&text);
            }
            console::Command::ClientKick(arg) => self.client_kick(arg.as_deref(), now),
            console::Command::Kick(arg) => self.kick(arg.as_deref(), now),
            console::Command::DumpUser(arg) => self.dump_user(arg.as_deref()),
            console::Command::ServerInfo => {
                let info = self.configstrings.first().cloned().unwrap_or_default();
                self.print(&format!(
                    "Server info settings:\n{}",
                    console::info_print(&info)
                ));
            }
            console::Command::SystemInfo => {
                let info = self.configstrings.get(1).cloned().unwrap_or_default();
                self.print(&format!(
                    "System info settings:\n{}",
                    console::info_print(&info)
                ));
            }
            console::Command::Say(text) => {
                if let Some(text) = text {
                    self.console_say(&text);
                }
            }
            console::Command::Set { cmd, args: None } => {
                self.print(&format!("usage: {cmd} <variable> <value>\n"));
            }
            console::Command::Set {
                cmd,
                args: Some((name, value)),
            } => {
                self.console_set(&name, &value);
                // `Cvar_SetA_f`: the archive flag whatever the write did.
                if cmd == "seta"
                    && let Some(rt) = self.script.as_mut()
                {
                    rt.host.cvars.add_flags(&name, crate::cvars::flag::ARCHIVE);
                }
            }
            console::Command::Heartbeat => self.masters.force(),
            console::Command::Quit => {
                self.final_message("EXE_SERVERQUIT");
                let mut resolve = self.resolver;
                let beats = self
                    .masters
                    .shutdown(self.dedicated, self.sv_time_ms, &mut resolve);
                self.outbox.extend(beats);
                self.quit = true;
            }
            console::Command::KillServer => self.kill_server(),
            console::Command::BanUser(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banUser <player name>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_name(&arg) {
                    self.ban_slot(slot);
                }
            }
            console::Command::BanClient(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banClient <client number>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_num(&arg) {
                    self.ban_slot(slot);
                }
            }
            console::Command::CvarList(filter) => {
                let text = match self.script.as_ref() {
                    Some(rt) => rt.cvars().list(filter.as_deref()),
                    None => self
                        .carried_cvars
                        .clone()
                        .unwrap_or_default()
                        .list(filter.as_deref()),
                };
                self.print(&text);
            }
            console::Command::Unknown(argv) => {
                if !self.cvar_command(&argv) {
                    log::warn!("console: unknown command {line:?}");
                }
            }
        }
        true
    }

    /// `SV_KickNum_f` (0x8084be4) and `SV_GetPlayerByNum` (0x8083b9c). A
    /// zombie counts as a client here: kicking one only restarts its clock.
    fn client_kick(&mut self, arg: Option<&str>, now: Instant) {
        let Some(arg) = arg else {
            self.print("Usage: kicknum <client number>\n");
            return;
        };
        if let Some(n) = self.player_by_num(arg) {
            self.kick_slot(n, now);
        }
    }

    /// `SV_GetPlayerByNum` (0x8083b9c): the slot in use that `arg` names,
    /// or `None` after printing why not.
    fn player_by_num(&mut self, arg: &str) -> Option<usize> {
        if !arg.bytes().all(|b| b.is_ascii_digit()) {
            self.print(&format!("Bad slot number: {arg}\n"));
            return None;
        }
        let n = arg.parse::<usize>().unwrap_or(usize::MAX);
        if n >= self.clients.len() {
            self.print(&format!(
                "Bad client slot: {}\n",
                arg.parse::<i64>().unwrap_or(-1)
            ));
            return None;
        }
        if self.clients[n].is_none() && self.zombies[n].is_none() {
            self.print(&format!("Client {n} is not active\n"));
            return None;
        }
        Some(n)
    }

    /// The tail both bans share (0x8084394, 0x8084524). Retail sends
    /// `banUser <ip>` to the authorize server and leaves the client
    /// connected; vcod keeps the address in its own list
    /// (docs/research/cod11-server-handshake.md, "Bans").
    fn ban_slot(&mut self, slot: usize) {
        let Some(c) = self.clients[slot].as_ref().or(self.zombies[slot].as_ref()) else {
            return;
        };
        let name = if self.clients[slot].is_some() {
            c.name.clone()
        } else {
            String::new()
        };
        if let std::net::IpAddr::V4(ip) = c.addr.ip() {
            self.bans.add(ip);
        }
        self.print(&format!("{name} was banned from coming back\n"));
    }

    /// Replaces the ban list; the binary loads it from `--ban-file`.
    pub fn set_bans(&mut self, bans: crate::bans::Bans) {
        self.bans = bans;
    }

    /// `SV_KillServer_f` (0x8084d3c) into `SV_Shutdown("EXE_SERVERKILLED")`:
    /// the final message to every client, the flatline to the masters,
    /// every slot freed and the level gone. The process stays; it reads
    /// no packet and runs no frame until a console `map` loads a level
    /// (docs/research/cod11-server-handshake.md, "Shutdown").
    fn kill_server(&mut self) {
        self.final_message("EXE_SERVERKILLED");
        let mut resolve = self.resolver;
        let beats = self
            .masters
            .shutdown(self.dedicated, self.sv_time_ms, &mut resolve);
        self.outbox.extend(beats);
        // The cvar table is process-global and outlives the shutdown.
        let _ = self.lift_persistence();
        self.script = None;
        self.clients.iter_mut().for_each(|c| *c = None);
        self.zombies.iter_mut().for_each(|z| *z = None);
        self.challenges.clear();
        self.bots.clear();
        self.bots_spawned = false;
        self.pending_explosions.clear();
        self.pending_script_commands.clear();
        self.weapon_changes.clear();
        self.killed = true;
    }

    /// `sv_cheats`, which only `SV_Map_f` writes: into systeminfo on the
    /// next frame's flush, and into the cvar table a query reads.
    fn set_cheats(&mut self, on: bool) {
        self.cheats = on;
        if let Some(rt) = self.script.as_mut() {
            rt.host.cvars.set("sv_cheats", if on { "1" } else { "0" });
        }
        self.refresh_fall_heights();
    }

    /// The drop both kicks end in; a zombie's clock restarts, so a kicked
    /// zombie only lingers longer.
    fn kick_slot(&mut self, slot: usize, now: Instant) {
        self.drop_client(slot, "EXE_PLAYERKICKED");
        if let Some(z) = self.zombies[slot].as_mut() {
            z.last_packet = now;
        }
    }

    /// `SV_GetPlayerByName` (0x8083aa0): the first slot in use whose name
    /// matches `name` case-insensitively, as is or with its colour codes
    /// stripped. A zombie's name is gone with its userinfo, so it matches
    /// only an empty one.
    fn player_by_name(&mut self, name: &str) -> Option<usize> {
        let found = (0..self.clients.len()).find(|&slot| {
            let n = match (&self.clients[slot], &self.zombies[slot]) {
                (Some(c), _) => c.name.as_str(),
                (None, Some(_)) => "",
                (None, None) => return false,
            };
            n.eq_ignore_ascii_case(name)
                || crate::game::say::clean_name(n).eq_ignore_ascii_case(name)
        });
        if found.is_none() {
            self.print(&format!("Player {name} is not on the server\n"));
        }
        found
    }

    /// `SV_Kick_f` (0x8084288). The name is looked up first, so `kick all`
    /// prints the miss before it kicks every slot in use.
    fn kick(&mut self, arg: Option<&str>, now: Instant) {
        let Some(arg) = arg else {
            self.print("Usage: kick <player name>\nkick all = kick everyone\n");
            return;
        };
        if let Some(slot) = self.player_by_name(arg) {
            self.kick_slot(slot, now);
        } else if arg.eq_ignore_ascii_case("all") {
            for slot in 0..self.clients.len() {
                if self.clients[slot].is_some() || self.zombies[slot].is_some() {
                    self.kick_slot(slot, now);
                }
            }
        }
    }

    /// `SV_DumpUser_f` (0x8084cc0).
    fn dump_user(&mut self, arg: Option<&str>) {
        let Some(arg) = arg else {
            self.print("Usage: info <userid>\n");
            return;
        };
        let Some(slot) = self.player_by_name(arg) else {
            return;
        };
        let info = self.clients[slot]
            .as_ref()
            .map_or_else(String::new, |c| c.userinfo.clone());
        self.print(&format!(
            "userinfo\n--------\n{}",
            console::info_print(&info)
        ));
    }

    /// `SV_ConSay_f` (0x8084974): `h "\x15console: <text>"` to every client
    /// past `CS_CONNECTED`, as a droppable command.
    fn console_say(&mut self, text: &str) {
        let cmd = format!("h \"\u{15}console: {text}\"");
        log::info!("say: console: {text}");
        for slot in 0..self.clients.len() {
            if self.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected)
            {
                self.send_server_command(slot, &cmd);
            }
        }
    }

    /// The two `CVAR_LATCH` cvars `Server` keeps in its own fields, by
    /// their folded name: the registered spelling, the value the running
    /// level loaded with, and the value the next load takes.
    fn latched_cvar(&self, name: &str) -> Option<(&'static str, String, String)> {
        match name.to_ascii_lowercase().as_str() {
            "g_gametype" => Some((
                "g_gametype",
                self.live_gametype(),
                self.cfg.gametype.clone(),
            )),
            "sv_maxclients" => Some((
                "sv_maxclients",
                self.level_cvar("sv_maxclients")
                    .unwrap_or_else(|| self.cfg.max_clients.to_string()),
                self.pending_cvar("sv_maxclients", &self.cfg.max_clients.to_string()),
            )),
            _ => None,
        }
    }

    /// A cvar in the running level's table; `None` with no level or no
    /// value. The level stamps `g_gametype` and `sv_maxclients` at load and
    /// a console write never reaches them there, so this is the live half
    /// of the latch.
    fn level_cvar(&self, name: &str) -> Option<String> {
        let v = self.script.as_ref()?.cvars().get(name);
        (!v.is_empty()).then(|| v.to_string())
    }

    /// `g_gametype` as the running level loaded it; serverinfo, `getinfo`
    /// and `getstatus` carry this one until a load takes a latched value.
    fn live_gametype(&self) -> String {
        self.level_cvar("g_gametype")
            .unwrap_or_else(|| self.cfg.gametype.clone())
    }

    /// `cfg` with the running level's `g_gametype`.
    fn live_cfg(&self) -> ServerConfig {
        let mut cfg = self.cfg.clone();
        cfg.gametype = self.live_gametype();
        cfg
    }

    /// `Cvar_Set_f`: a read-only, init or (without `sv_cheats`) cheat cvar
    /// refuses, a latched one keeps its value until the next load and says
    /// so, anything else is written to the running level's table at once.
    /// A `--set` override of the same name follows, so the next load does
    /// not put the old value back.
    fn console_set(&mut self, name: &str, value: &str) {
        if let Some((canon, live, pending)) = self.latched_cvar(name) {
            let mut waiting = (pending != live).then_some(pending);
            if let Some(msg) = crate::cvars::latch(&mut waiting, &live, name, value) {
                self.print(&msg);
            }
            self.set_cvar(canon, waiting.as_deref().unwrap_or(&live));
            return;
        }
        let cheats = self.cheats;
        let outcome = match self.script.as_mut() {
            Some(rt) => rt.host.cvars.console_set(name, value, cheats),
            None => {
                self.set_cvar(name, value);
                return;
            }
        };
        match outcome {
            crate::cvars::ConsoleSet::Refused(msg) => {
                self.print(&msg);
                return;
            }
            crate::cvars::ConsoleSet::Latched(msg) => {
                if let Some(msg) = msg {
                    self.print(&msg);
                }
            }
            crate::cvars::ConsoleSet::Applied => self.apply_engine_cvar(name, value),
        }
        if let Some((_, v)) = self
            .cvar_overrides
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            *v = value.to_string();
        }
    }

    /// `Cvar_Command`: a line whose first word names a cvar prints it, or
    /// sets it to the second word. False when no such cvar exists, which
    /// leaves the line an unknown command.
    fn cvar_command(&mut self, argv: &[String]) -> bool {
        let Some(name) = argv.first() else {
            return false;
        };
        let Some((canon, value, default, waiting)) = self.script.as_ref().and_then(|rt| {
            let cv = rt.cvars();
            let (n, v, d) = cv.lookup(name)?;
            Some((
                n.to_string(),
                v.to_string(),
                d.to_string(),
                cv.latched(name).map(str::to_string),
            ))
        }) else {
            return false;
        };
        if let Some(v) = argv.get(1) {
            self.console_set(&canon, v);
            return true;
        }
        let waiting = match self.latched_cvar(&canon) {
            Some((_, live, pending)) => (pending != live).then_some(pending),
            None => waiting,
        };
        let mut text = format!("\"{canon}\" is:\"{value}^7\" default:\"{default}^7\"\n");
        if let Some(pending) = waiting {
            text.push_str(&format!("latched: \"{pending}\"\n"));
        }
        self.print(&text);
        true
    }

    /// A client's `score` script field, 0 without one.
    fn client_score(&mut self, slot: usize) -> i32 {
        self.script
            .as_mut()
            .and_then(|rt| rt.client_field(slot, "score"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// `SV_Status_f` (0x80846b4).
    fn status_text(&mut self, now: Instant) -> String {
        let mut out = format!(
            "map: {}\nnum score ping name            lastmsg address               qport rate\n\
             --- ----- ---- --------------- ------- --------------------- ----- -----\n",
            self.cfg.map
        );
        for slot in 0..self.clients.len() {
            let zombie = self.clients[slot].is_none();
            let score = if zombie { 0 } else { self.client_score(slot) };
            let Some(c) = self.clients[slot].as_ref().or(self.zombies[slot].as_ref()) else {
                continue;
            };
            let row = console::StatusRow {
                num: slot,
                score,
                ping: if zombie {
                    console::Ping::Zombie
                } else if c.state == ClientState::Connected {
                    console::Ping::Connecting
                } else {
                    console::Ping::Ms(c.ping)
                },
                // The drop nukes the userinfo, and the name with it.
                name: if zombie { "" } else { &c.name },
                last_msg_ms: now.saturating_duration_since(c.last_packet).as_millis() as i64,
                addr: c.addr,
                qport: c.netchan.qport,
                rate: c.rate(self.dedicated),
            };
            out.push_str(&row.to_string());
        }
        out.push('\n');
        out
    }

    /// `Com_Printf`: the log, and the rcon reply when one is being collected.
    fn print(&mut self, msg: &str) {
        if let Some(r) = self.redirect.as_mut() {
            r.print(msg);
        }
        log::info!("{}", msg.trim_end());
    }

    /// Whether a `quit` has run; the binary flushes the outbox and exits.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Replaces the master-name resolver, so a test can heartbeat without
    /// touching DNS.
    pub fn set_master_resolver(&mut self, resolve: fn(&str) -> Option<SocketAddr>) {
        self.resolver = resolve;
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
        // it. The clock is the client's own, held to the window
        // `replay_moves` holds every cmd to, so a handshake cmd stamped far
        // ahead cannot leave `commandTime` past every cmd that follows. With
        // no entering cmd, one frame back, so the first cmd's dt is a sane
        // 50 ms rather than the whole age of the client's clock.
        c.last_processed_st = entering.map_or(self.sv_time_ms.wrapping_sub(FRAME_MS), |c| {
            c.server_time.clamp(
                self.sv_time_ms.wrapping_sub(1000),
                self.sv_time_ms.wrapping_add(200),
            )
        });
        log::info!("client {slot} {:?} begin (spectator)", c.name);
        // `ClientBegin`: the notify that releases the connect callback's
        // `waittill("begin")`. The event queues rather than fires here, so it
        // is drained after the `Connect` that armed the wait.
        if let Some(rt) = self.script.as_mut() {
            rt.push_client_event(ClientEvent::Begin(slot));
        }
    }

    pub fn tick(&mut self, now: Instant) {
        self.drain_console(now);
        // `Com_Quit_f` exits inside the command; nothing after it runs. A
        // killed server runs no frame (`SV_Frame` returns while
        // `sv_running` is 0).
        if self.quit || self.killed {
            return;
        }
        self.check_timeouts(now);
        self.step_bots();
        self.calc_pings();
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
                    rt.set_client_buttons(
                        slot,
                        moved[slot].last_buttons.unwrap_or(c.last_cmd.buttons),
                    );
                }
            }
            // The client commands the packet pass queued, on this frame's
            // clock: a thread started here sees `level.time` already
            // advanced, which is what a `cloneplayer` in it needs.
            for (slot, cmd) in queued {
                match cmd {
                    ScriptCommand::MenuResponse(index, response) => {
                        rt.menu_response(slot, index, &response)
                    }
                }
            }
            mirror_roster(&self.clients, rt);
            rt.run_threads(self.sv_time_ms);
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
                // `ClientSpawn` lets go of a gun first (turrets doc 8). The
                // teleport's events go out only for a spawn into play, the
                // `sessionstate` the script set ahead of the spawn.
                // A gun deleted this frame queued its release for here; the
                // spawn comes after it, as `G_FreeTurret` ran first in retail.
                rt.apply_turret_releases(s.slot, sim);
                let temps = rt.release_turret(s.slot, sim);
                if s.mode == SpawnMode::Player {
                    for te in temps {
                        rt.push_temp_entity(te);
                    }
                }
                match s.mode {
                    SpawnMode::Player => {
                        sim.become_player(s.origin, s.yaw_deg, cmd_angles);
                        if let Some(w) = self.world.as_ref() {
                            sim.spawn_move(
                                vcod_common::movetrace::MoveWorld::bare(&w.collision),
                                self.sv_time_ms,
                            );
                        }
                        // `ClientSpawn` runs inside `self spawn()`, ahead of
                        // the loadout the script gives after it, so the
                        // think animates empty hands.
                        if let Some(anims) = self.anims.as_deref() {
                            let index = sim.ps.weapon as usize;
                            sim.update_anims(
                                &crate::spectate::AnimInputs {
                                    anims,
                                    weapon: crate::items::item_name(index).unwrap_or_default(),
                                    weapon_class: self.weapon_table.class(index),
                                },
                                &vcod_common::net::msg::NULL_USERCMD,
                                self.sv_time_ms,
                                &[],
                                &mut self.rng,
                            );
                        }
                    }
                    SpawnMode::Spectator => {
                        sim.become_spectator(s.origin, s.yaw_deg, cmd_angles);
                        if let Some(w) = self.world.as_ref() {
                            sim.spawn_move(
                                vcod_common::movetrace::MoveWorld::bare(&w.collision),
                                self.sv_time_ms,
                            );
                        }
                    }
                    SpawnMode::Intermission => {
                        sim.become_intermission(s.origin, s.yaw_deg, cmd_angles, self.sv_time_ms)
                    }
                }
                // The spawn's own think runs a player or a spectator up to
                // the frame's clock; the intermission arm runs no pmove.
                if s.mode != SpawnMode::Intermission {
                    c.last_processed_st = self.sv_time_ms;
                }
            }
            // The machine's own switches first: `pickup` writes `ps.weapon`
            // when a drop ends, and the mirror below would put the old
            // weapon back and start the identical putaway again.
            for (slot, weapon) in self.weapon_changes.drain(..) {
                rt.set_client_weapon(slot, weapon);
            }
            // What the client holds comes across the same way the
            // configstrings do: re-read every frame, because any thread can
            // have changed them. The held bits have to be among them --
            // `PM_Weapon` disarms a player who no longer owns `ps.weapon`
            // (combat doc, section 1.8) -- and `ps.weapon` with them, so a
            // sim reset outside a move comes back armed. The write-back
            // above is what makes that safe: the host's copy already carries
            // whatever the machine switched to this tick.
            mirror_weapons(&mut self.clients, rt);
            // The ammo and the current weapon, which are edges rather than
            // state: applying a full clip every frame would make the weapon
            // bottomless.
            apply_weapon_ops(&mut self.clients, rt, &weapons);
            // `linkTo` and `unlink`, before the re-anchor below so a link
            // made this frame is already pinned on this frame's wire: both
            // retail captures read the new `pm_type` on the next snapshot
            // after the cmd (object-model doc, 23.2).
            for (slot, op) in rt.take_link_ops() {
                let Some(sim) = self
                    .clients
                    .get_mut(slot)
                    .and_then(Option::as_mut)
                    .and_then(|c| c.sim.as_mut())
                else {
                    continue;
                };
                sim.link_to = match op {
                    crate::game::host::LinkOp::Link { parent, offset } => {
                        Some(crate::spectate::Link {
                            parent,
                            offset,
                            velocity: sim.ps.velocity.into(),
                        })
                    }
                    crate::game::host::LinkOp::Unlink => None,
                };
            }
            // `G_RunClient`'s re-anchor: a linked client's origin is the
            // parent's plus the offset it linked at, turned by the parent's
            // angles (`G_SetFixedLink`'s mode 2), and a held walk input
            // moves it not at all. The parent is read where it is this
            // frame, since retail runs it first (movers doc, section 13). Its velocity is whatever it linked with:
            // retail's plant capture holds 184,27 across the abort's two
            // linked seconds and 0 under 92 forward cmds (23.2). A parent
            // that is gone releases the link: `sd.gsc`'s plant success never
            // unlinks, the bombzone's `delete()` is what frees the record.
            for (slot, c) in self.clients.iter_mut().enumerate() {
                let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) else {
                    continue;
                };
                let Some(link) = sim.link_to else { continue };
                match rt.link_anchor(link.parent) {
                    Some((p, angles)) => {
                        let [f, l, u] =
                            vcod_common::pmove::aim::angles_to_axis(angles).map(glam::Vec3::from);
                        let [x, y, z] = link.offset;
                        sim.ps.origin = glam::Vec3::from(p) + f * x + l * y + u * z;
                        sim.ps.velocity = link.velocity.into();
                        // The mirror loop above ran before the re-anchor, so
                        // script's copy is written again here. The anchor is
                        // end-of-tick, where retail's is ahead of
                        // `ClientThink`, so this tick's per-cmd touch passes
                        // already ran at the un-anchored origins; the wire
                        // carries the anchored one either way.
                        rt.set_client_origin(slot, sim.origin());
                    }
                    None => sim.link_to = None,
                }
            }
            // What script did to each sim, applied once, then the health
            // mirror and the frame's damage feedback, in that order:
            // `P_DamageFeedback` reads the health the hit left.
            let anims = self.anims.as_deref();
            apply_sim_ops(
                &mut self.clients,
                rt,
                anims,
                &weapons,
                &mut self.rng,
                self.sv_time_ms,
            );
            // The movers' half of the entity pass: each brush model that
            // moved this frame carries its riders and shoves the bodies and
            // items in its way, or holds a frame when a body fits nowhere
            // (`G_MoverTeam`; movers doc, section 12). After the script's
            // own `setOrigin`s, which retail's threads run before the pass.
            if let Some(world) = self.world.as_ref() {
                for step in rt.take_mover_steps() {
                    let mut sims: Vec<(usize, &mut crate::spectate::ClientSim)> = self
                        .clients
                        .iter_mut()
                        .enumerate()
                        .filter_map(|(i, c)| Some((i, c.as_mut()?.sim.as_mut()?)))
                        .collect();
                    let before: Vec<glam::Vec3> = sims.iter().map(|(_, s)| s.ps.origin).collect();
                    if crate::push::push(&step, &mut sims, &world.collision) {
                        rt.push_items(&step);
                    } else {
                        rt.stall_mover(&step);
                    }
                    for ((slot, sim), was) in sims.iter().zip(before) {
                        if sim.ps.origin != was {
                            rt.set_client_origin(*slot, sim.origin());
                        }
                    }
                }
            }
            // `G_RunFrame`'s entity loop: items, links and missiles one
            // entity at a time by number (combat doc 14.7), after the frame's
            // threads and every client move they made (spawns, `setOrigin`s,
            // links, mover pushes), so a thread reads a grenade's last-frame
            // origin, the flight is traced past the bodies where script put
            // them, and a blast walk meets this frame's links of every entity
            // numbered below the grenade (14.7, 16.2). A grenade thrown on
            // this tick was spawned inside its cmd on the last frame's
            // `level.time`, so it has already flown a frame by the time the
            // snapshot goes out (combat doc 11.4). Each blast is walked on
            // its missile's turn, one victim's callback before the next
            // victim is measured (14.1, 14.5).
            self.pending_explosions.clear();
            let collision = self.world.as_ref().map(|w| &w.collision);
            let mut bones = match (self.fs.as_deref(), self.anims.as_deref()) {
                (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                    fs,
                    anims,
                    rigs: &mut self.hit_rigs,
                    now_ms: self.sv_time_ms,
                }),
                _ => None,
            };
            let sims: Vec<(usize, &crate::spectate::ClientSim)> = self
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            // Where a callback earlier in the walk set a client down: its
            // link moved off the one its last cmd made (`setOrigin` relinks
            // at once, combat doc 14.7), and the walk measures it there.
            let set_down =
                |rt: &script::ScriptRuntime, slot: usize, s: &crate::spectate::ClientSim| {
                    let link = glam::Vec3::from(rt.host.client_link_origin[slot]);
                    (link != s.link_origin()).then_some(link)
                };
            let victim =
                |rt: &script::ScriptRuntime, slot: usize, s: &crate::spectate::ClientSim| {
                    let eye = s.ps.view().eye;
                    let mut v = crate::game::combat::BlastVictim {
                        slot,
                        origin: s.ps.origin,
                        link_origin: s.link_origin(),
                        mins: s.ps.mins(),
                        maxs: s.ps.maxs(),
                        eye,
                    };
                    if let Some(at) = set_down(rt, slot, s) {
                        v = crate::game::combat::BlastVictim {
                            origin: at,
                            link_origin: at,
                            eye: at + (eye - s.ps.origin),
                            ..v
                        };
                    }
                    v
                };
            let mut pass = rt.begin_entity_pass();
            while let Some(x) = rt.run_entity_pass(&mut pass, collision, &sims, self.sv_time_ms) {
                self.bot_noises.push(crate::bots::Noise {
                    at: (x.at + glam::Vec3::Z * 40.0).into(),
                    source: x.owner,
                    radius: crate::bots::HEAR_BLAST,
                });
                let Some(def) = weapons.get(x.weapon as usize) else {
                    self.pending_explosions.push(x);
                    continue;
                };
                let blast = crate::game::combat::Blast::new(
                    x.at,
                    def.explosion_radius,
                    def.explosion_inner_damage as f32,
                    def.explosion_outer_damage as f32,
                    Some(x.owner),
                    Some(x.inflictor),
                    crate::items::item_name(x.weapon as usize).unwrap_or_default(),
                    "MOD_GRENADE_SPLASH",
                );
                // `trap_EntitiesInBox`' order (combat doc 14.7): the
                // clients and the turrets as the area tree lists them, taken
                // once. Each is measured on its turn, after every earlier
                // victim's callback (14.5).
                let (mins, maxs) = blast.search_box();
                let candidates: Vec<BlastCandidate> = rt
                    .host
                    .area
                    .entities_in_box(mins, maxs, -1)
                    .into_iter()
                    .filter_map(|n| {
                        if let Some(&(slot, s)) = sims.iter().find(|(slot, _)| *slot == n as usize)
                        {
                            return Some(BlastCandidate::Client(slot, s));
                        }
                        let id = rt.host.ents.handle(n)?;
                        Some(BlastCandidate::Entity(id))
                    })
                    .collect();
                // A client a callback of this walk killed is a corpse and
                // stops nothing.
                let live_bodies =
                    |rt: &script::ScriptRuntime| -> Vec<crate::game::combat::HitBody> {
                        sims.iter()
                            .filter(|(other, _)| !rt.client_vitals(*other).dead)
                            .filter_map(|&(other, s)| {
                                let mut body = s.hit_body(other)?;
                                if let Some(at) = set_down(rt, other, s) {
                                    body.origin = at;
                                }
                                Some(body)
                            })
                            .collect()
                    };
                for candidate in candidates {
                    match candidate {
                        BlastCandidate::Client(slot, s) => {
                            if !rt.client_vitals(slot).takedamage {
                                continue;
                            }
                            let v = victim(rt, slot, s);
                            if !blast.reaches(&v) {
                                continue;
                            }
                            let bodies = live_bodies(rt);
                            let models = rt.placed_script_models();
                            if let Some(hit) =
                                blast.hit(&v, collision, &models, &bodies, bones.as_mut())
                            {
                                rt.deliver_hits(vec![hit], self.sv_time_ms);
                            }
                        }
                        BlastCandidate::Entity(id) => {
                            let Some(v) = rt.blast_entities().into_iter().find(|v| v.id == id)
                            else {
                                continue;
                            };
                            if !blast.reaches_entity(&v) {
                                continue;
                            }
                            let bodies = live_bodies(rt);
                            let models = rt.placed_script_models();
                            if let Some(damage) =
                                blast.entity_damage(&v, collision, &models, &bodies, bones.as_mut())
                            {
                                rt.damage_entity(v.id, damage, x.owner);
                            }
                        }
                    }
                }
                self.pending_explosions.push(x);
            }
            // What the blasts' callbacks did to the sims, as above.
            mirror_weapons(&mut self.clients, rt);
            apply_weapon_ops(&mut self.clients, rt, &weapons);
            apply_sim_ops(
                &mut self.clients,
                rt,
                self.anims.as_deref(),
                &weapons,
                &mut self.rng,
                self.sv_time_ms,
            );
            mirror_vitals(&mut self.clients, rt);
            self.archive.set_on(rt.archive_on());
            // `G_RunFrame`'s own slot order (0x50ab0-0x50ad7), not arrival
            // order (docs/research/cod11-player-clip.md 4.2, 6). No link
            // follows a CORPSE write here. The follow half goes first, so a
            // spectator reads the own-view bit a lower slot took this frame
            // and a higher slot's from the last one, as retail's loop does.
            let collision = self.world.as_ref().map(|w| &w.collision);
            for slot in 0..self.clients.len() {
                follow_end_frame(
                    &mut self.clients,
                    slot,
                    rt,
                    &self.archive,
                    collision,
                    self.sv_time_ms,
                );
                // `takedamage` off the session state (0x40ed9, 0x40f93,
                // 0x4107c, and `SpectatorClientEndFrame` 0x40788).
                if let Some(session) = rt.client_session(slot) {
                    let playing = session.state == follow::SessionState::Playing;
                    rt.set_client_takedamage(slot, playing);
                }
                let Some(sim) = self.clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
                    continue;
                };
                sim.update_contents();
                let live = sim.pm_type == PmType::Normal && !sim.dead && sim.health > 0;
                if live {
                    let views: Vec<Option<StuckView>> =
                        self.clients.iter().map(Self::stuck_view).collect();
                    let rand = || (vcod_common::rng::xorshift(&mut self.rng) >> 33) as u32;
                    if let Some(push) = stuck_in_client(slot, &views, rand) {
                        let me = self.clients[slot]
                            .as_mut()
                            .and_then(|c| c.sim.as_mut())
                            .unwrap();
                        me.ps.velocity.x = push.self_vel.x;
                        me.ps.velocity.y = push.self_vel.y;
                        me.ps.knockback_ms = 300.0;
                        me.ps.knockback_flags |= vcod_common::pmove::PMF_TIME_KNOCKBACK;
                        // The caller marks only self a corpse (0x411b8); the
                        // partner marks itself on its own turn through the scan.
                        me.contents = CONTENTS_CORPSE;
                        let other = self.clients[push.other]
                            .as_mut()
                            .and_then(|c| c.sim.as_mut())
                            .unwrap();
                        other.ps.velocity.x = push.other_vel.x;
                        other.ps.velocity.y = push.other_vel.y;
                        other.ps.knockback_ms = 300.0;
                        other.ps.knockback_flags |= vcod_common::pmove::PMF_TIME_KNOCKBACK;
                    }
                }
                self.clients[slot]
                    .as_mut()
                    .and_then(|c| c.sim.as_mut())
                    .unwrap()
                    .end_frame(self.sv_time_ms);
            }
            // `ClientEndFrame`'s `pingPlayer` clear (0x41024) and its compass
            // teammate (0x411fc), per slot in slot order: a lower slot reads
            // a higher one's ping bit as that slot's last end frame left it.
            let sessions: Vec<Option<follow::SessionState>> = (0..self.clients.len())
                .map(|slot| rt.client_session(slot).map(|s| s.state))
                .collect();
            let teams: Vec<i32> = (0..self.clients.len())
                .map(|slot| rt.client_team(slot))
                .collect();
            for slot in 0..self.clients.len() {
                let Some(sim) = self.clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
                    continue;
                };
                sim.ping = rt.host.client_ping_until[slot] > rt.host.level_time_ms;
                let found = compass_friend(
                    &self.clients,
                    slot,
                    &sessions,
                    &teams,
                    self.world.as_ref().map(|w| &w.vis),
                    self.sv_time_ms,
                    self.proto,
                );
                let sim = self.clients[slot].as_mut().unwrap().sim.as_mut().unwrap();
                if let Some(found) = found {
                    sim.compass_friend = found.map_or(0, |(info, _, _)| info);
                    match found {
                        Some((_, n, ping)) => {
                            sim.last_friend = n;
                            sim.friend_ping = ping;
                        }
                        None => sim.last_friend = compass::NO_FRIEND,
                    }
                }
            }
            // `ClientEndFrame`'s aim trace and cursor hint, after the script
            // frame and the mirrors so they read the frame's final eye, aim
            // and items; the fire it raises wakes its waiters next frame
            // (object-model doc 23.1). The `pm_type` goes with it: a spawn
            // above changed it with no cmd, and the runtime's gate is what
            // clears a dead or spectating client's `isLookingAt` and hint.
            // Every body first, at the origin the sim ops and the re-anchor
            // left, since any of them can stand in another client's aim.
            for (slot, c) in self.clients.iter().enumerate() {
                let sim = c.as_ref().and_then(|c| c.sim.as_ref());
                rt.set_client_body(slot, sim.and_then(|s| s.hit_body(slot)));
                rt.set_client_dobj(slot, sim.and_then(|s| s.dobj(slot)));
            }
            // Per slot, `ClientEndFrame`'s tail: the aim trace and cursor
            // hint, `BG_PlayerAnimation`, then `turret_think_client`
            // (turrets doc 6.1). A gunner's rounds are traced and delivered
            // right there, so they meet a lower slot's new pose and a higher
            // slot's last-frame one, and a higher slot's turn sees what the
            // callback did (combat doc 16.1, 16.2).
            // `bg_swingSpeed` is a vmCvar the game refreshes every frame.
            let swing_speed = crate::game::builtins::cvar::atof(rt.cvars().get("bg_swingSpeed"));
            for slot in 0..self.clients.len() {
                let Some(c) = self.clients[slot].as_mut() else {
                    continue;
                };
                let buttons = moved
                    .get(slot)
                    .and_then(|m| m.last_buttons)
                    .unwrap_or(c.last_cmd.buttons);
                let Some(sim) = c.sim.as_mut() else { continue };
                rt.set_client_pm_type(slot, sim.wire_pm_type());
                // The link the script frame made is only on the sim from
                // here, so the ground reading script sees next frame is
                // taken again after it.
                rt.set_client_on_ground(slot, sim.on_ground());
                let rifle = self
                    .weapon_table
                    .get(sim.ps.weapon as usize)
                    .is_some_and(|d| d.sounds.rifle_bullet);
                rt.set_client_aim(slot, sim.ps.view().eye.into(), sim.aim_angles(), rifle);
                rt.aim_lookat(slot, self.sv_time_ms);
                let (hint, string) =
                    rt.cursor_hint_pass(slot, sim.ps.view().eye.into(), sim.view_angles());
                sim.cursor_hint = hint;
                if let Some(string) = string {
                    sim.cursor_hint_string = string;
                }
                (sim.head_icon, sim.head_icon_team) = rt.head_icon(slot);
                // `BG_PlayerAnimation` (0x41486) runs after this slot's
                // aim trace: a higher slot's trace meets this frame's
                // pose, a lower one's met the last (combat doc 16.1).
                sim.commit_pose(
                    FRAME_MS,
                    c.last_processed_st,
                    swing_speed,
                    self.anims.as_deref(),
                );
                rt.set_client_body(slot, sim.hit_body(slot));
                rt.set_client_dobj(slot, sim.dobj(slot));
                rt.apply_turret_releases(slot, sim);
                let Some(shot) = rt.turret_think_client(
                    slot,
                    sim,
                    buttons & vcod_common::net::msg::BUTTON_ATTACK != 0,
                    self.anims.as_deref().map(|a| (a, &mut self.hit_rigs)),
                ) else {
                    continue;
                };
                self.bot_noises.push(crate::bots::Noise {
                    at: shot.muzzle.into(),
                    source: shot.slot,
                    radius: crate::bots::HEAR_GUNFIRE,
                });
                let sims: Vec<(usize, &crate::spectate::ClientSim)> = self
                    .clients
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                    .collect();
                let collision = self.world.as_ref().map(|w| &w.collision);
                let mut bones = match (self.fs.as_deref(), self.anims.as_deref()) {
                    (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                        fs,
                        anims,
                        rigs: &mut self.hit_rigs,
                        now_ms: self.sv_time_ms,
                    }),
                    _ => None,
                };
                // The callback is told the gunner's own weapon (turrets doc
                // 12.6); `player_die` credits the gun.
                let carried = sims
                    .iter()
                    .find(|(s, _)| *s == shot.slot)
                    .map_or(0, |(_, sim)| sim.ps.weapon as usize);
                let r = crate::game::combat::bullet_fire_from(
                    shot.slot,
                    shot.muzzle,
                    shot.dir,
                    shot.damage,
                    shot.rifle_bullet,
                    crate::items::item_name(carried).unwrap_or_default(),
                    &sims,
                    collision,
                    &self.hitlocs,
                    bones.as_mut(),
                );
                deliver_turret_rounds(
                    &mut self.clients,
                    rt,
                    r.effects,
                    slot,
                    self.anims.as_deref(),
                    &weapons,
                    &mut self.rng,
                    self.sv_time_ms,
                );
            }
            rt.drop_turret_releases();
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
        }
        self.refresh_fall_heights();
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
        self.send_downloads();
        self.send_zombies();
        // `SV_Frame`'s last step (0x808d258).
        let mut resolve = self.resolver;
        let beats = self.masters.heartbeat(
            self.dedicated,
            self.sv_time_ms,
            crate::master::HEARTBEAT_GAME,
            &mut resolve,
        );
        self.outbox.extend(beats);
        // The weapon changes are already drained when a script is loaded,
        // and a server without one has nothing to write them to.
        self.weapon_changes.clear();
    }

    /// `SV_ExecuteClientMessage` for every packet since the last tick, in the
    /// order they arrived: a packet's `kill` first, then one pmove step per
    /// usercmd, dt off the cmd clocks, matching the client's own prediction.
    /// Each cmd is `ClientThink_real`: the move, then the shots and swings it
    /// raised, traced and delivered to the damage callback there and then
    /// against every client as its own packets so far left it, then the touch
    /// pass (combat doc, 16). A throw spawns its missile there too. Returns
    /// what each slot replayed, for the trace line `send_snapshots` writes.
    fn replay_moves(&mut self) -> Vec<MoveSummary> {
        use vcod_common::movetrace::{Body, MoveWorld};
        use vcod_common::pmove::weapon::{EV_FIRE_MELEE, EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT};
        use vcod_common::pmove::{EV_LANDING_PAIN_BASE, EV_LANDING_PAIN_LAST};
        // Every client's body, the mover's own rewritten after each of its
        // steps: retail relinks after each `Pmove`.
        let mut bodies: Vec<Body> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(s, c)| c.as_ref()?.sim.as_ref()?.body(s as u32))
            .collect();
        let weapons = self.weapon_table.clone();
        let now_ms = self.sv_time_ms;
        let fall_heights = self.fall_heights;
        let max_clients = self.clients.len();
        let mut moved = vec![MoveSummary::default(); max_clients];
        let mut rounds: Vec<Round> = (0..max_clients).map(|_| Round::default()).collect();
        // A client past the per-tick cap keeps the rest for the next tick.
        let mut capped = vec![false; max_clients];
        // `SpectatorThink` reads the other clients' own-view bits as the
        // last end frame left them, and each spectator's forced follow.
        let followable_now: Vec<bool> = (0..max_clients)
            .map(|slot| followable(&self.clients, slot))
            .collect();
        let spectating: Vec<Option<i32>> = (0..max_clients)
            .map(|slot| {
                let sim = self.clients[slot].as_ref()?.sim.as_ref()?;
                let s = session_of(self.script.as_mut(), slot, sim);
                (s.state == follow::SessionState::Spectator).then_some(s.spectator_client)
            })
            .collect();
        let mut use_held: Vec<bool> = (0..max_clients)
            .map(|slot| {
                self.script.as_ref().is_some_and(|rt| {
                    rt.client_old_buttons(slot) & vcod_common::net::msg::BUTTON_USE != 0
                })
            })
            .collect();
        loop {
            // The oldest packet's next item across every client, its `kill`
            // ahead of its cmds.
            let mut next: Option<(u64, bool, usize)> = None;
            for (slot, c) in self.clients.iter_mut().enumerate() {
                let Some(c) = c.as_mut() else {
                    continue;
                };
                if c.sim.is_none() {
                    c.kill_at = None;
                    continue;
                }
                let kill = c.kill_at.map(|p| (p, false));
                let cmd = c
                    .pending
                    .first()
                    .filter(|_| !capped[slot])
                    .map(|q| (q.packet, true));
                if let Some(item) = kill.into_iter().chain(cmd).min()
                    && next.is_none_or(|(p, is_cmd, _)| item < (p, is_cmd))
                {
                    next = Some((item.0, item.1, slot));
                }
            }
            let Some((_, is_cmd, slot)) = next else {
                break;
            };
            if !is_cmd {
                self.clients[slot].as_mut().unwrap().kill_at = None;
                self.close_round(slot, &mut rounds[slot]);
                self.run_kill(slot, &weapons);
                relink(&mut bodies, &self.clients, slot);
                continue;
            }
            let collision = self.world.as_ref().map(|w| &w.collision);
            let c = self.clients[slot].as_mut().unwrap();
            let m = &mut moved[slot];
            if m.processed >= MAX_CMDS_PER_TICK {
                // The resync keeps the newest two cmds and sets the base as
                // if only the last replays, so the penultimate may
                // double-count one frame; harmless for flight.
                c.last_processed_st = c
                    .pending
                    .last()
                    .unwrap()
                    .cmd
                    .server_time
                    .wrapping_sub(FRAME_MS);
                c.pending.drain(..c.pending.len().saturating_sub(2));
                capped[slot] = true;
                continue;
            }
            let mut cmd = c.pending.remove(0).cmd;
            // `ClientThink_real` holds the cmd's clock within 1000 ms behind
            // and 200 ms ahead of the `level.time` it runs on, the last
            // frame's (player-clip doc 8.12).
            let level_ms = now_ms.wrapping_sub(FRAME_MS);
            cmd.server_time = cmd
                .server_time
                .clamp(level_ms.wrapping_sub(1000), level_ms.wrapping_add(200));
            // Stale cmds (dt <= 0) are skipped whole; a long one is chopped
            // rather than clamped away.
            let dt_ms = cmd.server_time.wrapping_sub(c.last_processed_st);
            if dt_ms <= 0 {
                continue;
            }
            let sim = c.sim.as_mut().unwrap();
            sim.ps.fall_heights = fall_heights;
            let round = &mut rounds[slot];
            // What the client held going in, so a switch the machine made is
            // told apart from a playerstate reset between ticks.
            let held = *round.held.get_or_insert(sim.ps.weapon);
            let prev_buttons = std::mem::replace(&mut sim.last_buttons, cmd.buttons);
            // A spectator's buttons move its follow, and a follow the last
            // end frame landed runs no pmove at all.
            let mut frozen = false;
            if let Some(forced) = spectating[slot] {
                sim.spectator_think(
                    forced,
                    prev_buttons,
                    cmd.buttons,
                    max_clients,
                    |t| followable_now[t],
                    collision,
                );
                frozen = sim.follow.on;
            }
            let mut raised = Vec::new();
            let mut take = None;
            if !frozen {
                // The aim block runs once per cmd, on the whole cmd, before
                // the chop (`ClientThink_real` 0x40169-0x40456).
                sim.update_aim(dt_ms, now_ms, weapons.defs());
                // A hitching client's gap is simulated, not discarded: see
                // `cmd::chop`.
                for (step, dt) in vcod_common::pmove::cmd::chop(c.last_processed_st, &cmd) {
                    raised.extend(sim.step(
                        &step,
                        dt,
                        collision.map(|w| MoveWorld::new(w, &bodies, slot as u32)),
                        weapons.defs(),
                    ));
                    bodies.retain(|b| b.entity != slot as u32);
                    bodies.extend(sim.body(slot as u32));
                }
            }
            // `ClientEvents` (0x3fd24): this cmd's shots, swings, throws and
            // falls, in the order it raised them, fired below before its
            // touch pass.
            let mut attacks = Vec::new();
            for e in &raised {
                let weapon = sim.ps.weapon;
                let grenade = weapons
                    .get(weapon as usize)
                    .is_some_and(|d| d.weapon_type == "grenade");
                match e.event {
                    // A grenade's fire event is the throw, and the parm is
                    // what is left of the fuse (combat doc, 1.11).
                    EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if grenade => {
                        attacks.push(Attack::Throw {
                            slot,
                            weapon,
                            fuse_left_ms: e.parm,
                            aim: sim.aim_angles(),
                        })
                    }
                    EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => {
                        self.bot_noises.push(crate::bots::Noise {
                            at: (sim.ps.origin + glam::Vec3::Z * 40.0).into(),
                            source: slot,
                            radius: crate::bots::HEAR_GUNFIRE,
                        });
                        attacks.push(Attack::Shot(Shot {
                            slot,
                            weapon,
                            ads: sim.ps.weapon_pos_frac == 1.0,
                            aim: sim.aim_angles(),
                            stance: vcod_common::pmove::weapon::SpreadStance::of(
                                &sim.ps,
                                cmd.server_time,
                                now_ms,
                            ),
                        }));
                    }
                    EV_FIRE_MELEE => attacks.push(Attack::Swing {
                        slot,
                        weapon,
                        aim: sim.aim_angles(),
                    }),
                    ev if (EV_LANDING_PAIN_BASE..=EV_LANDING_PAIN_LAST).contains(&ev) => attacks
                        .push(Attack::Fall {
                            slot,
                            percent: e.parm,
                        }),
                    _ => {}
                }
                // A `clipOnly` weapon with nothing left is taken away (combat
                // doc, 1.5 step 9), and 1.8's switch path then takes
                // `ps.weapon` to 0 on its own. Hung off the last shot, not
                // off `EV_NOAMMO`, which a dry trigger raises too and keeps
                // the weapon.
                if e.event == EV_FIRE_WEAPON_LASTSHOT
                    && let Some(def) = weapons.get(weapon as usize)
                    && def.clip_only
                    && sim.ps.ammo[def.ammo_index] == 0
                {
                    take = Some(weapon);
                }
            }
            round.events.extend(raised);
            round.switched |= sim.ps.weapon != held;
            let t = Touched {
                slot,
                origin: sim.origin(),
                buttons: cmd.buttons,
                pm_type: sim.wire_pm_type(),
                on_ground: sim.on_ground(),
                yaw: (sim.pm_type == crate::spectate::PmType::Normal).then(|| sim.view_angles()[1]),
                weapon: round.switched.then_some(sim.ps.weapon),
                take,
                eye: sim.ps.view().eye.into(),
                view: sim.view_angles(),
                stance: sim.ps.stance,
            };
            round.last_cmd = Some(cmd);
            c.last_processed_st = cmd.server_time;
            m.first_cmd_st.get_or_insert(cmd.server_time);
            m.last_cmd_st = Some(cmd.server_time);
            m.last_buttons = Some(cmd.buttons);
            m.processed += 1;
            let use_down = cmd.buttons & vcod_common::net::msg::BUTTON_USE != 0;
            let pressed = use_down && !use_held[slot];
            use_held[slot] = use_down;
            // The anim a use press's mount would change is picked off the
            // state ahead of it.
            if pressed && self.script.is_some() {
                self.close_round(slot, &mut rounds[slot]);
            }
            // What the cmd left, on the host before the callbacks and the
            // touch pass read it: the host's copy is otherwise only mirrored
            // from the sim after the script frame.
            let proto = self.proto;
            if let Some(rt) = self.script.as_mut() {
                let c = self.clients[slot].as_ref().unwrap();
                let sim = c.sim.as_ref().unwrap();
                // The ammo among it is what the item pass reads; the pass
                // itself moves the host's copy as it grabs.
                mirror_for_callback(rt, sim, proto, slot, c.last_processed_st);
                // `r.currentOrigin` is the snapped `s.pos.trBase` through
                // `ClientEvents` and `G_TouchTriggers` (combat doc 2.1, 5.5):
                // what a callback or a death drop in them reads of the mover.
                rt.set_client_origin(t.slot, glam::Vec3::from(t.origin).trunc().into());
                if let Some(yaw) = t.yaw {
                    rt.set_client_yaw(t.slot, yaw);
                }
                rt.set_client_pm_type(t.slot, t.pm_type);
                rt.set_client_on_ground(t.slot, t.on_ground);
                // Ahead of `weapon_changes`, which lands after the script
                // frame: a grab tests `ps.weapon` as this cmd left it.
                if let Some(w) = t.weapon {
                    rt.set_client_weapon(t.slot, w);
                }
                // Retail takes it inside `PM_Weapon`, ahead of the touch.
                if let Some(w) = t.take {
                    rt.take_client_weapon(t.slot, w);
                }
            }
            for attack in attacks {
                self.fire(attack, &mut rounds, &mut bodies, &weapons);
            }
            // Retail runs the touch pass per usercmd inside `ClientThink_real`
            // (0x405b3), right after the link. The item half follows the
            // trigger half, and the use key after both.
            if let Some(rt) = self.script.as_mut() {
                // The link sits between `ClientEvents`, whose shots traced
                // against the last links, and `G_TouchTriggers` (0x40595).
                if let Some(sim) = self.clients[t.slot].as_ref().and_then(|c| c.sim.as_ref()) {
                    link_client(rt, t.slot, sim);
                }
                mirror_roster(&self.clients, rt);
                rt.touch_triggers_at(t.slot, now_ms, t.origin);
                // `ps.origin` back into `r.currentOrigin` past the touch
                // (0x405c7), ahead of the use key's `Cmd_Activate_f`. The
                // item half reads `ps.origin` either way.
                rt.set_client_origin(t.slot, t.origin);
                rt.item_pass(t.slot, t.buttons, t.eye, t.view);
                // The mount lands inside the use cmd (turrets doc 12.1), so
                // it reaches the sim before the next cmd's pass runs.
                for (turret, stance, view) in
                    rt.take_turret_mounts(t.slot, t.origin, t.stance, t.view)
                {
                    if let Some(sim) = self.clients[t.slot].as_mut().and_then(|c| c.sim.as_mut()) {
                        crate::game::turret::mount_sim(sim, turret, stance, view);
                    }
                }
            }
            // A `trigger_hurt` that killed the mover ran `player_die` inside
            // this touch pass, ahead of the cmds behind it.
            let died = self
                .script
                .as_ref()
                .is_some_and(|rt| rt.client_vitals(slot).dead)
                && self.clients[slot]
                    .as_ref()
                    .and_then(|c| c.sim.as_ref())
                    .is_some_and(|s| !s.dead && s.pm_type == crate::spectate::PmType::Normal);
            if died {
                self.close_round(slot, &mut rounds[slot]);
            }
            // What the passes queued for the mover reaches its sim before its
            // next cmd, as `Touch_Item` and `G_Damage` write the playerstate
            // inside this one (items doc, 13.2).
            if let (Some(rt), Some(sim)) = (
                self.script.as_mut(),
                self.clients[slot].as_mut().and_then(|c| c.sim.as_mut()),
            ) && (died || rt.has_ops_of(slot))
            {
                apply_callback_ops(
                    rt,
                    sim,
                    slot,
                    self.anims.as_deref(),
                    &weapons,
                    &mut self.rng,
                    now_ms,
                );
            }
            if died {
                relink(&mut bodies, &self.clients, slot);
            }
        }
        for (slot, round) in rounds.iter_mut().enumerate() {
            self.close_round(slot, round);
        }
        // The state each player ended the tick in, mirrored onto the host for
        // `cloneplayer` and for the bodies a scripted blast traces: a builtin
        // cannot reach a sim, and the corpse is the dying player's entity
        // state (`crate::game::bodies`). Written here,
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
                rt.set_client_body(slot, sim.and_then(|(s, _)| s.hit_body(slot)));
                rt.set_client_dobj(slot, sim.and_then(|(s, _)| s.dobj(slot)));
                if let Some((s, _)) = sim {
                    rt.set_client_height(slot, (s.ps.maxs() - s.ps.mins()).z);
                    rt.set_client_link_origin(slot, s.link_origin().into());
                }
            }
        }
        moved
    }

    /// The end of a run of one client's cmds: the animation the state and
    /// events they left imply, and the weapon the machine switched to.
    fn close_round(&mut self, slot: usize, round: &mut Round) {
        self.flush_anims(slot, round);
        if let Some(sim) = self.clients[slot].as_ref().and_then(|c| c.sim.as_ref())
            && round.held.is_some_and(|h| h != sim.ps.weapon)
        {
            self.weapon_changes.push((slot, sim.ps.weapon));
        }
        *round = Round::default();
    }

    /// The animation the client should be playing, from the state its cmds so
    /// far produced and the input that produced it.
    fn flush_anims(&mut self, slot: usize, round: &mut Round) {
        let events = std::mem::take(&mut round.events);
        let (Some(cmd), Some(anims)) = (round.last_cmd.take(), self.anims.as_deref()) else {
            return;
        };
        let Some(sim) = self.clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
            return;
        };
        let index = sim.ps.weapon as usize;
        sim.update_anims(
            &crate::spectate::AnimInputs {
                anims,
                weapon: crate::items::item_name(index).unwrap_or_default(),
                weapon_class: self.weapon_table.class(index),
            },
            &cmd,
            self.sv_time_ms,
            &events,
            &mut self.rng,
        );
    }

    /// `Cmd_Kill_f`, and what it did to the sim before the cmds behind it
    /// run: the drop takes the weapon they switch away from, and `EV_DEATH`
    /// goes on the ring ahead of their events. `pm_type` stays until the end
    /// frame, so they still move alive (combat doc, 9.2).
    fn run_kill(&mut self, slot: usize, weapons: &crate::weapons::WeaponTable) {
        let proto = self.proto;
        let now_ms = self.sv_time_ms;
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        mirror_roster(&self.clients, rt);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        let st = c.last_processed_st;
        let Some(sim) = c.sim.as_mut() else { return };
        mirror_for_callback(rt, sim, proto, slot, st);
        if !rt.kill_client(slot, now_ms) {
            return;
        }
        apply_callback_ops(
            rt,
            sim,
            slot,
            self.anims.as_deref(),
            weapons,
            &mut self.rng,
            now_ms,
        );
    }

    /// `FireWeapon`'s grenade arm: `fire_grenade` from the muzzle this cmd
    /// left, stamped with the `level.time` a cmd runs under, the frame before
    /// the one being built (combat doc, 11.3 and 11.4).
    fn throw(
        &mut self,
        slot: usize,
        weapon: u8,
        fuse_left_ms: i32,
        aim: [f32; 2],
        weapons: &crate::weapons::WeaponTable,
    ) {
        let (Some(me), Some(def)) = (
            self.clients[slot].as_ref().and_then(|c| c.sim.as_ref()),
            weapons.get(weapon as usize),
        ) else {
            return;
        };
        let (origin, velocity) = crate::game::missile::throw_velocity(&me.ps, aim, def);
        // The projectile's model, indexed when the item was registered
        // (`GameHost::register_item`). A miss means the map load stopped
        // registering it and the client has no model to draw the grenade
        // with, which is silent on the wire.
        let name = def.projectile_model.as_deref().unwrap_or_default();
        let model = crate::configstrings::weapon_model_index(&self.configstrings, name);
        if model == 0 && !name.is_empty() {
            log::warn!("the grenade client {slot} threw carries {name:?}, which nothing precached");
        }
        let level_ms = self.sv_time_ms.wrapping_sub(FRAME_MS);
        if let Some(rt) = self.script.as_mut() {
            rt.fire_grenade(
                slot,
                weapon,
                model,
                origin,
                velocity,
                fuse_left_ms,
                level_ms,
            );
        }
    }

    /// `ClientEvents`' landing-pain arm (player-clip doc, 8.8), inside the cmd
    /// that landed: `pain_debounce_time` to `level.time + 200`, which keeps
    /// the end frame's `EV_PAIN` off a fall, then `G_Damage` with the percent
    /// taken of `ps.stats[2]` and no inflictor, attacker, point or direction.
    fn fall(
        &mut self,
        slot: usize,
        percent: i32,
        rounds: &mut [Round],
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        // A zero share is the one `ClientEvents` skips (0x3fda8).
        if percent == 0 {
            return;
        }
        let proto = self.proto;
        let now_ms = self.sv_time_ms;
        self.close_round(slot, &mut rounds[slot]);
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        mirror_roster(&self.clients, rt);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        let st = c.last_processed_st;
        let Some(sim) = c.sim.as_mut() else { return };
        // A cmd runs on the last frame's `level.time`.
        let level_ms = now_ms.wrapping_sub(FRAME_MS);
        sim.debounce_pain(level_ms.wrapping_add(FALL_PAIN_DEBOUNCE_MS));
        let damage = crate::game::combat::located_damage(
            fall_damage(percent, sim.max_health),
            self.hitlocs.multiplier("none"),
        );
        mirror_for_callback(rt, sim, proto, slot, st);
        rt.deliver_fall(slot, damage, now_ms);
        apply_callback_ops(
            rt,
            sim,
            slot,
            self.anims.as_deref(),
            weapons,
            &mut self.rng,
            now_ms,
        );
        relink(bodies, &self.clients, slot);
    }

    /// `FireWeapon` inside the cmd that raised it: the trace against the
    /// world and every client as it stands now, and each impact and damage
    /// callback in the order the round met them, so a player one round kills
    /// is out of the way of the next (combat doc, 16). A throw spawns its
    /// missile here instead (11.4).
    fn fire(
        &mut self,
        attack: Attack,
        rounds: &mut [Round],
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        let (attacker, weapon) = match attack {
            Attack::Shot(s) => (s.slot, s.weapon),
            Attack::Swing { slot, weapon, .. } => (slot, weapon),
            Attack::Throw {
                slot,
                weapon,
                fuse_left_ms,
                aim,
            } => {
                self.throw(slot, weapon, fuse_left_ms, aim, weapons);
                return;
            }
            Attack::Fall { slot, percent } => {
                self.fall(slot, percent, rounds, bodies, weapons);
                return;
            }
        };
        let Some(def) = weapons.get(weapon as usize) else {
            return;
        };
        // The bodies are posed off the last end frame (combat doc, 16.1).
        let effects = {
            let sims: Vec<(usize, &crate::spectate::ClientSim)> = self
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let collision = self.world.as_ref().map(|w| &w.collision);
            // The locational trace's context, absent on a host with no paks
            // or no animtree; a shot then lands at hit location `none`.
            let mut bones = match (self.fs.as_deref(), self.anims.as_deref()) {
                (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                    fs,
                    anims,
                    rigs: &mut self.hit_rigs,
                    now_ms: self.sv_time_ms,
                }),
                _ => None,
            };
            let name = crate::items::item_name(weapon as usize).unwrap_or_default();
            match attack {
                Attack::Shot(shot) => crate::game::combat::bullet_fire(
                    attacker,
                    def,
                    name,
                    shot.ads,
                    &shot.stance,
                    shot.aim,
                    &sims,
                    collision,
                    &self.hitlocs,
                    bones.as_mut(),
                    &mut self.rng,
                ),
                Attack::Swing { aim, .. } => crate::game::combat::melee_fire(
                    attacker,
                    def,
                    name,
                    weapon,
                    aim,
                    &sims,
                    collision,
                    &self.hitlocs,
                    bones.as_mut(),
                    &mut self.rng,
                ),
                Attack::Throw { .. } | Attack::Fall { .. } => unreachable!(),
            }
            .effects
        };
        let proto = self.proto;
        let now_ms = self.sv_time_ms;
        for e in effects {
            let hit = match e {
                Effect::Impact(te) => {
                    if let Some(rt) = self.script.as_mut() {
                        rt.push_temp_entity(te);
                    }
                    continue;
                }
                Effect::Hit(h) => h,
            };
            let victim = hit.victim;
            // The victim's own cmds so far are its state now; the callback
            // lands between them and the ones behind.
            self.close_round(victim, &mut rounds[victim]);
            let Some(rt) = self.script.as_mut() else {
                continue;
            };
            mirror_roster(&self.clients, rt);
            let Some(c) = self.clients[victim].as_mut() else {
                continue;
            };
            let st = c.last_processed_st;
            let Some(sim) = c.sim.as_mut() else { continue };
            mirror_for_callback(rt, sim, proto, victim, st);
            rt.deliver_hits(vec![hit], now_ms);
            apply_callback_ops(
                rt,
                sim,
                victim,
                self.anims.as_deref(),
                weapons,
                &mut self.rng,
                now_ms,
            );
            relink(bodies, &self.clients, victim);
        }
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

        // What every snapshot this frame takes its entities from, and what
        // the archive keeps of the frame. A missile carries `SVF_BROADCAST`
        // (combat doc, 11.1), so it skips the cull the way a broadcast temp
        // entity does.
        let mut culled: BTreeMap<u32, Rc<msg::EntityState>> =
            entities.into_iter().map(|(n, e)| (n, Rc::new(e))).collect();
        culled.extend(client_entities.into_iter().map(|(n, e)| (n, Rc::new(e))));
        let live = crate::archive::WorldFrame {
            culled,
            temps: temps
                .iter()
                .zip(temp_states)
                .map(|(te, (_, e))| (te.scope, Rc::new(e)))
                .collect(),
            broadcast: self.script.as_ref().map_or_else(BTreeMap::new, |rt| {
                rt.missiles()
                    .entities(self.proto)
                    .map(|(n, e)| (n, Rc::new(e)))
                    .collect()
            }),
            roster: Rc::new(roster),
            time: self.sv_time_ms,
        };

        // A follower's frame is the followed client's: its playerstate with
        // the follow flags patched in, its eye for the cull and its number
        // for the single-client scopes (`SpectatorClientEndFrame` 0x40896,
        // `SV_BuildClientSnapshot` 0x808f25f). A copy the end frame took out
        // of the archive is that frame's, its times shifted by the age
        // (0x808ef7c). Built up front, since the loop below holds each slot
        // mutably.
        let now = self.sv_time_ms;
        let follow_frames: Vec<Option<FollowFrame>> = (0..self.clients.len())
            .map(|slot| {
                let sim = self.clients[slot].as_ref()?.sim.as_ref()?;
                let t = sim.follow.target.filter(|_| sim.follow.on)?;
                let own = sim.to_wire(self.proto, slot as i32, 0);
                let ef = msg::PlayerState::field_index(self.proto, "eFlags").unwrap();
                let (mut ps, eye, replay) = match sim.follow.copied.and_then(|c| c.frame) {
                    Some(index) => {
                        let f = self.archive.frame(index)?;
                        let view = f.clients.get(t)?.as_ref()?;
                        let ps = crate::archive::replayed_ps(view, now - f.time, self.proto);
                        (ps, view.eye, true)
                    }
                    None => {
                        let tc = self.clients.get(t)?.as_ref()?;
                        let ts = tc.sim.as_ref()?;
                        let mut ps = ts.to_wire(self.proto, t as i32, tc.last_processed_st);
                        // The end-frame loop runs in slot order.
                        if let (true, Some(last)) = (slot < t, ts.end_frame_wire.as_ref()) {
                            follow::before_end_frame(&mut ps, last, self.proto);
                        }
                        // `P_DamageFeedback`'s `EV_PAIN` is that end frame's too.
                        if let (true, Some(ring)) = (slot < t, ts.ring_before_pain) {
                            ring.write(&mut |name, v| {
                                let i = msg::PlayerState::field_index(self.proto, name).unwrap();
                                ps.fields[i] = v;
                            });
                        }
                        (ps, ts.eye_origin(), false)
                    }
                };
                follow::patch_wire(&mut ps, self.proto, sim.follow.forced, own.fields[ef]);
                Some(FollowFrame {
                    target: t,
                    ps,
                    eye,
                    replay,
                })
            })
            .collect();

        // What a follow leaves in the spectator's playerstate when it stops,
        // and each followable client's frame as this end frame left it.
        for (slot, f) in follow_frames.iter().enumerate() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            let command_time = c.last_processed_st;
            let Some(sim) = c.sim.as_mut() else {
                continue;
            };
            if let Some(f) = f {
                sim.follow_wire = Some(f.ps.clone());
            }
            sim.end_frame_wire = sim
                .own_view
                .then(|| sim.to_wire(self.proto, slot as i32, command_time));
            sim.ring_before_pain = None;
        }

        // `SV_BuildClientSnapshot` reads each client's `archivetime` again and
        // takes the entities and the roster from the frame it names, times
        // shifted by the age, whether or not the client follows anyone
        // (0x808f1ab..0x808f211); the trim is written back as it is there.
        let sources: Vec<Option<(i32, i32)>> = (0..self.clients.len())
            .map(|slot| {
                let rt = self.script.as_mut()?;
                let asked = rt.client_archive_ms(slot);
                let mut age = asked;
                let index = self.archive.lookup(&mut age);
                if age != asked {
                    rt.set_client_archive_ms(slot, age);
                }
                let f = self.archive.frame(index?)?;
                Some((index?, now - f.time))
            })
            .collect();

        for (slot, follow_frame) in follow_frames.iter().enumerate() {
            let follow_frame = follow_frame.as_ref();
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
            // The frame's `ps.clientNum`, which the single-client flags test
            // against and whose entity the frame leaves out.
            let client_num = follow_frame.map_or(slot, |f| f.target);

            // Retail sends a client only what its own position can see, so
            // the list is per client rather than one list cloned into every
            // frame (docs/protocol-1.1.md, "Which entities a client is sent").
            let eye = follow_frame.map_or_else(|| sim.eye_origin(), |f| f.eye);
            let (world, shift) = match sources[slot] {
                Some((index, shift)) => (&self.archive.frame(index).unwrap().world, shift),
                None => (&live, 0),
            };
            let visible = world.entities_for(client_num, eye, collision_vis, shift, self.proto);
            let roster = (*world.roster).clone();

            let mut ps = match follow_frame {
                Some(f) => f.ps.clone(),
                None => sim.to_wire(self.proto, client_num as i32, command_time),
            };
            // The script's HUD elements, filtered for this client the way
            // `HudElem_UpdateClient` filters them; retail rebuilds both
            // arrays into the playerstate once per client per frame, so
            // they are read here rather than carried on the sim. A follower
            // keeps the followed client's archived half and objectives, which
            // ride the copied playerstate, and gets its own unarchived half.
            let team_of = |s: usize| per_slot.get(s).map_or(script::TEAM_SPECTATOR, |p| p.2);
            let team = team_of(slot);
            if let Some(rt) = self.script.as_mut() {
                let (archived, current) = rt.hud_elems(slot, team);
                ps.arrays.hud_current = current;
                match follow_frame {
                    None => {
                        ps.arrays.hud_archived = archived;
                        ps.arrays.objectives = rt.objectives_for(slot, team);
                    }
                    Some(f) if !f.replay => {
                        let t = client_num;
                        ps.arrays.hud_archived = rt.hud_elems(t, team_of(t)).0;
                        ps.arrays.objectives = rt.objectives_for(t, team_of(t));
                    }
                    Some(_) => {}
                }
            }
            // `ClientSpawn`'s memset ran after the frame's HUD update
            // (the retail killcam's end frame reads both arrays empty).
            if sim.hud_cleared {
                ps.arrays.hud_archived.clear();
                ps.arrays.hud_current.clear();
            }
            let frame = snapshot::Snapshot {
                server_time: self.sv_time_ms,
                message_num,
                delta_num: -1,
                snap_flags: self.snap_flag_server_bit,
                ps,
                entities: visible,
                clients: roster,
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
            c.reliable_sent = c.netchan.reliable_sequence as i32;
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
            c.stamp_sent(message_num, self.sv_time_ms);
            for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
                self.outbox.push((c.addr, pkt));
            }
        }
        for c in self.clients.iter_mut().flatten() {
            if let Some(sim) = c.sim.as_mut() {
                sim.hud_cleared = false;
            }
        }
        self.archive_frame(live);
    }

    /// `SV_ArchiveSnapshot` (`cod_lnxded` 0x808fb84), after the frame's
    /// snapshots: the entities and roster they were built from, and every
    /// client's own view as `GetFollowPlayerState` would answer it, which a
    /// spectating or intermission client has none of.
    fn archive_frame(&mut self, world: crate::archive::WorldFrame) {
        if !self.archive.is_on() {
            return;
        }
        let clients = (0..self.clients.len())
            .map(|slot| {
                let c = self.clients[slot].as_ref()?;
                let sim = c.sim.as_ref().filter(|s| s.own_view)?;
                let mut ps = sim.to_wire(self.proto, slot as i32, c.last_processed_st);
                if let Some(rt) = self.script.as_mut() {
                    let team = rt.client_team(slot);
                    ps.arrays.hud_archived = rt.hud_elems(slot, team).0;
                    ps.arrays.objectives = rt.objectives_for(slot, team);
                }
                Some(Rc::new(crate::archive::ArchivedView {
                    ps,
                    eye: sim.ps.view().eye.into(),
                    angles: sim.view_angles(),
                    origin: sim.origin(),
                }))
            })
            .collect();
        self.archive.push(crate::archive::Frame {
            time: self.sv_time_ms,
            world,
            clients,
        });
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
    use vcod_common::net::snapshot::{SVC_SNAPSHOT, SnapshotRing};

    const QPORT: u16 = 0x2001;

    /// Every landing of the two 2026-10-05 retail runs, parm and maxhealth
    /// to damage (player-clip doc 8.10): the float `0.01` keeps each whole
    /// share just under, and past 99 the fall does 110 percent.
    #[test]
    fn fall_damage_is_retails_share_of_max_health() {
        for (percent, max, damage) in [
            (25, 100, 24),
            (40, 100, 39),
            (77, 100, 76),
            (43, 200, 85),
            (100, 100, 110),
            (13, 100, 12),
            (18, 200, 35),
            (41, 100, 40),
        ] {
            assert_eq!(fall_damage(percent, max), damage, "{percent}% of {max}");
        }
    }

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
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.5; p = getentarray(\"noclass\", \"classname\"); \
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

    /// The level script's cvar table carries `mapname`, the probes' only way
    /// to tell which map they run on; it used to read "".
    #[test]
    fn a_level_script_reads_the_map_name_cvar() {
        let sv = Server::new(cfg(), Instant::now());
        let cvars = sv.cvars(&vcod_common::pk3::Pk3Fs::empty());
        assert_eq!(cvars.get("mapname"), "mp_carentan");
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

    /// A `g_password` set while a client is on is checked again by the
    /// `ClientConnect` both level boundaries re-run; retail drops the client
    /// with `w "GAME_INVALIDPASSWORD"` (handshake doc, "g_password").
    #[test]
    fn a_new_g_password_drops_clients_at_the_next_restart_or_map() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        for spawn in [false, true] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_scripts(fs.clone()).expect("load the scripts");
            active(&mut sv, now);
            sv.console_set("g_password", "secret");
            sv.take_outgoing();
            sv.handle_packet(addr(7), &oob("getinfo x"), now);
            let (_, _, rest) = reply(&mut sv);
            assert_eq!(
                info_value_for_key(&String::from_utf8_lossy(&rest), "pswrd"),
                Some("1"),
                "g_password is live, not latched"
            );
            if spawn {
                sv.spawn_server("mp_carentan").expect("the map load failed");
            } else {
                sv.map_restart().expect("the restart failed");
            }
            assert!(sv.clients[0].is_none(), "spawn {spawn}: the client stayed");
            let z = sv.zombies[0].as_ref().expect("a zombie");
            let at = z.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
            assert_eq!(z.netchan.reliable[at], "w \"GAME_INVALIDPASSWORD\"");
        }
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
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
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
                "EXE_SERVER_IS_DIFFERENT_VER\x151.1".to_string()
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

    fn connect_with(
        sv: &mut Server,
        from: SocketAddr,
        extra: &str,
        now: Instant,
    ) -> (String, String) {
        let challenge = challenge_for(sv, from, now);
        let ui = format!(
            "\\name\\vcod{extra}\\protocol\\{}\\qport\\{QPORT}\\challenge\\{challenge}",
            PROTOCOL_V1.version
        );
        sv.handle_packet(from, &build_connect(&ui), now);
        reply_text(sv)
    }

    /// Retail 1.1d with `sv_privateClients 2` (handshake doc, "Private
    /// slots"): a client without `sv_privatePassword` searches from slot 2,
    /// one with it from 0, a reconnect keeps its slot either way, and an
    /// empty private password matches a client that sends none.
    #[test]
    fn private_slots_take_the_private_password() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("sv_privateClients", "2");
        sv.set_cvar("sv_privatePassword", "pp");
        let slot_of = |sv: &Server, from: SocketAddr| {
            sv.clients
                .iter()
                .position(|c| c.as_ref().is_some_and(|c| c.addr == from))
        };
        // One ip each: the reply limiter would refuse a sixth challenge to one.
        let addr = |n: u8| SocketAddr::from(([10, 0, 1, n], 28960));
        let full = ("error".to_string(), "EXE_SERVERISFULL".to_string());
        for (n, extra, slot) in [(5, "", 2), (6, "\\password\\wrong", 3)] {
            assert_eq!(
                connect_with(&mut sv, addr(n), extra, now).0,
                "connectResponse"
            );
            assert_eq!(slot_of(&sv, addr(n)), Some(slot));
        }
        assert_eq!(connect_with(&mut sv, addr(7), "", now), full);
        assert_eq!(
            connect_with(&mut sv, addr(8), "\\password\\pp", now).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(8)), Some(0));

        let browser = addr(20);
        sv.handle_packet(browser, &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "clients"), Some("2"));
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("2"));
        sv.handle_packet(browser, &oob("getstatus x"), now);
        let (_, _, rest) = reply(&mut sv);
        let status = String::from_utf8_lossy(&rest).to_string();
        let serverinfo = status.lines().next().unwrap_or("");
        assert_eq!(
            info_value_for_key(serverinfo, "sv_privateClients"),
            Some("2")
        );

        let later = now + RECONNECT_LIMIT + Duration::from_secs(1);
        assert_eq!(
            connect_with(&mut sv, addr(8), "", later).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(8)), Some(0), "a reconnect keeps its slot");

        sv.set_cvar("sv_privatePassword", "");
        assert_eq!(
            connect_with(&mut sv, addr(9), "", later).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(9)), Some(1));
    }

    /// Retail 1.1d with `g_password secret` (handshake doc, "g_password"):
    /// no password, a wrong one and a wrong case are refused before a slot
    /// is taken; the right one connects.
    #[test]
    fn g_password_gates_the_connect() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_password", "secret");
        let denied = ("error".to_string(), "GAME_INVALIDPASSWORD".to_string());
        for (i, extra) in ["", "\\password\\wrong", "\\password\\Secret"]
            .iter()
            .enumerate()
        {
            assert_eq!(
                connect_with(&mut sv, addr(5 + i as u16), extra, now),
                denied,
                "{extra:?}"
            );
        }
        assert_eq!(sv.client_count(), 0);
        let ok = connect_with(&mut sv, addr(9), "\\password\\secret", now);
        assert_eq!(ok.0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    /// `none` still reads `pswrd 1` but lets anyone in; an empty value is 0.
    #[test]
    fn pswrd_reports_any_g_password_and_none_admits_everyone() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let pswrd = |sv: &mut Server, query: &str| {
            sv.handle_packet(addr(5), &oob(&format!("{query} x")), now);
            let (_, _, rest) = reply(sv);
            let text = String::from_utf8_lossy(&rest).to_string();
            info_value_for_key(text.lines().next().unwrap_or(""), "pswrd").map(str::to_string)
        };
        for (value, flag) in [("", "0"), ("secret", "1"), ("none", "1")] {
            sv.set_cvar("g_password", value);
            assert_eq!(
                pswrd(&mut sv, "getinfo").as_deref(),
                Some(flag),
                "{value:?}"
            );
            assert_eq!(
                pswrd(&mut sv, "getstatus").as_deref(),
                Some(flag),
                "{value:?}"
            );
        }
        assert_eq!(connect_with(&mut sv, addr(6), "", now).0, "connectResponse");
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
        assert!(
            sv.take_outgoing().is_empty(),
            "the zombie's frame carries it"
        );
        sv.tick(now);
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected the drop notice");
        let (to, pkt) = out.remove(0);
        assert_eq!(to, addr(5));
        assert_eq!(
            server_commands(&mut nc, &pkt, &huff),
            vec!["w \"EXE_LOSTRELIABLECOMMANDS\"".to_string()]
        );
    }

    /// A pure server with made-up pak checksums, and an entered client's
    /// first move after the commands `cmds` gives for the level's
    /// `checksumFeed`.
    fn pure_server_after(cmds: impl Fn(i32) -> Vec<String>) -> Server {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.paks.pure = true;
        sv.pure_check = Some(PureCheck {
            cgame: Some(11),
            ui: Some(22),
            loaded: vec![5, 6],
            feed: sv.checksum_feed,
        });
        let mut nc = active(&mut sv, now);
        let cmds = cmds(sv.checksum_feed);
        if !cmds.is_empty() {
            let cmds: Vec<&str> = cmds.iter().map(String::as_str).collect();
            pipelined(&mut sv, &mut nc, &cmds, now);
        }
        if sv.client_count() == 1 {
            let huff = Huffman::new();
            let ack = nc.incoming_sequence as i32;
            let ops = move_ops(sv.checksum_feed, ack, NULL_USERCMD);
            let pkt = nc
                .build_out(i32::from(sv.server_id), ack, 0, &ops, &huff)
                .unwrap();
            sv.handle_packet(addr(5), &pkt, now);
        }
        sv
    }

    #[test]
    fn a_pure_server_keeps_only_clients_whose_cp_verifies() {
        use vcod_common::pak_checksum::pure_command;
        let good = |feed| vec![pure_command(Some(11), Some(22), &[6], feed)];
        assert_eq!(pure_server_after(good).client_count(), 1);
        // No `cp` before the first move: `EXE_CANNOTVALIDATEPURECLIENT`.
        assert_eq!(pure_server_after(|_| Vec::new()).client_count(), 0);
        // A pak the server lacks: `EXE_UNPURECLIENTDETECTED`.
        let bad = |feed| vec![pure_command(Some(11), Some(22), &[7], feed)];
        assert_eq!(pure_server_after(bad).client_count(), 0);
        // `vdr` forgets a good one.
        let reset = |feed| vec![pure_command(Some(11), Some(22), &[6], feed), "vdr".into()];
        assert_eq!(pure_server_after(reset).client_count(), 0);
    }

    /// One client command per sequence, all in one message.
    fn pipelined(sv: &mut Server, nc: &mut Netchan, cmds: &[&str], now: Instant) {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        for (i, cmd) in cmds.iter().enumerate() {
            w.write_bits(CLC_CLIENT_COMMAND, 2);
            w.write_long(i as i32 + 1);
            w.write_string(cmd);
            // The client's own ring, which keys the server's reply.
            nc.reliable[(i + 1) & 63] = cmd.to_string();
        }
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
    }

    /// Score requests pipelined faster than a snapshot goes out leave one
    /// scoreboard queued: each `b` replaces the unsent one before it.
    #[test]
    fn pipelined_score_requests_leave_one_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        let before = sv.clients[0].as_ref().unwrap().netchan.reliable_sequence;
        pipelined(&mut sv, &mut nc, &["score"; 65], now);
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!(c.netchan.reliable_sequence, before + 1);
        assert!(
            sv.take_outgoing().is_empty(),
            "nothing goes out before a snapshot"
        );
    }

    /// A client that stops acking is dropped once one more command would
    /// overwrite the oldest unacked slot, with retail's reason, the notice
    /// last; the slot then takes a fresh connect.
    #[test]
    fn a_full_ring_of_unacked_commands_drops_the_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        let room = {
            let c = sv.clients[0].as_ref().unwrap();
            MAX_RELIABLE_COMMANDS as i32 - (c.netchan.reliable_sequence as i32 - c.reliable_ack)
        };
        for i in 0..room {
            sv.send_server_command(0, &format!("v c{i} 1"));
        }
        assert_eq!(sv.client_count(), 1, "a full ring is not yet fatal");
        sv.send_server_command(0, "v one_more 1");
        assert_eq!(sv.client_count(), 0);
        let huff = Huffman::new();
        sv.tick(now);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "the zombie's message alone");
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(
            cmds.last().map(String::as_str),
            Some("w \"EXE_SERVERCOMMANDOVERFLOW\"")
        );
        let t2 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        connected(&mut sv, addr(5), t2);
        assert_eq!(sv.client_count(), 1);
    }

    /// The reply rides the next snapshot, not a message of its own.
    #[test]
    fn a_score_request_gets_a_deathmatch_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = begun(&mut sv, now);
        sv.tick(now);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        pipelined(&mut sv, &mut nc, &["score"], now);
        assert!(sv.take_outgoing().is_empty());
        sv.tick(now);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one snapshot");
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds.len(), 1);
        // No ack has landed after time 0 yet, so `SV_CalcPings` reads 999.
        assert!(cmds[0].starts_with("b 1 0 0 0 0 999 0 0"), "{:?}", cmds[0]);
    }

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
        assert!(
            !sv.limiter
                .addrs
                .contains_key(&std::net::IpAddr::from([10, 0, 0, 0]))
        );
    }

    /// Retail matches an OOB `disconnect` and drops nothing (0x808c827).
    #[test]
    fn an_oob_disconnect_is_ignored() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        connected(&mut sv, addr(5), now);
        sv.take_outgoing();
        sv.handle_packet(addr(5), &oob("disconnect"), now);
        assert_eq!(sv.client_count(), 1);
        assert!(sv.take_outgoing().is_empty());
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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

    /// A script's `playSound` on a player reaches that client's own event
    /// ring, and through it the playerstate: the builtin only queues a
    /// `SimOp`, so the drain after `run_frame` is what completes the path.
    /// `_minefields.gsc`'s warning click is exactly this call.
    #[test]
    fn a_script_playsound_on_a_player_reaches_that_client_s_ring() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { \
                   while (1) { \
                     wait 0.05; \
                     players = getentarray(\"noclass\", \"classname\"); \
                     if (players.size > 0) { \
                       players[0] playsound(\"minefield_click\"); \
                       return; \
                     } \
                   } \
                 }",
            ),
        );
        let _nc = begun(&mut sv, now);
        for _ in 0..4 {
            sv.tick(now);
        }
        let ring = sv.clients[0]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .expect("slot 0 has a sim")
            .ring;
        assert_eq!(ring.seq, 1, "one event, the slot written below it");
        assert_eq!(ring.events[0], 172, "EV_SOUND_ALIAS");
        let alias = ring.parms[0] as usize + 524;
        assert_eq!(sv.configstring(alias), "minefield_click");
    }

    /// A client holding its last frag and nothing else, in a server running
    /// `script`, with the `serverTime` its next cmd builds on.
    fn last_frag_in_hand(script: &str) -> Option<(Server, i32, usize)> {
        last_frag_in_hand_under(crate::game::script::ScriptRuntime::for_test(script))
    }

    /// `last_frag_in_hand` under a runtime the caller built.
    fn last_frag_in_hand_under(
        rt: crate::game::script::ScriptRuntime,
    ) -> Option<(Server, i32, usize)> {
        let fs = vcod_common::testing::game_fs()?;
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        });
        install_script(&mut sv, rt);
        sv.weapon_table = Rc::new(crate::weapons::WeaponTable::load(&fs));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 64.0], 0.0);
        let frag = crate::configstrings::weapon_index("fraggrenade_mp").unwrap();
        let def = sv.weapon_table.get(frag).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(frag, sv.weapon_table.slot(frag));
        held.current = frag as u8;
        let rt = sv.script.as_mut().unwrap();
        rt.host.weapons = sv.weapon_table.clone();
        rt.host.client_weapons[0] = held;
        let c = sv.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = frag as u8;
        sim.ps.ammoclip[def.clip_index] = 1;
        sim.ps.ammo[def.ammo_index] = 0;
        let st = c.last_processed_st;
        Some((sv, st, frag))
    }

    /// The cmds that hold the trigger through the pullback and then let go:
    /// the release is the throw, and the throw is the last round.
    fn frag_throw_cmd(i: i32, st: i32, frag: usize) -> UserCmd {
        use vcod_common::net::msg::BUTTON_ATTACK;
        UserCmd {
            server_time: st + 50 * (i + 1),
            weapon: frag as u8,
            buttons: if i < 20 { BUTTON_ATTACK } else { 0 },
            ..NULL_USERCMD
        }
    }

    /// A `clipOnly` weapon's last round takes it before that cmd's touch
    /// pass, as retail's `PM_Weapon` does ahead of `G_TouchTriggers`
    /// (combat doc 1.5 step 9): a grab on the same cmd must not see the frag
    /// as held and top it up. Read straight after `replay_moves`, which is
    /// where the touch pass ends and before the script frame.
    #[test]
    fn the_last_frag_is_taken_before_that_cmd_s_touch() {
        let Some((mut sv, st, frag)) = last_frag_in_hand("main() {}") else {
            return;
        };
        for i in 0..60 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.clients[0].as_mut().unwrap().pending.push(cmd.into());
            // The frame's clock keeps up with the cmds, which `replay_moves`
            // clamps to 200 ms past `level.time`.
            sv.sv_time_ms = cmd.server_time;
            sv.replay_moves();
            let rt = sv.script.as_ref().unwrap();
            let thrown = rt.missiles().entities(sv.proto).next().is_some();
            if thrown {
                assert!(
                    !rt.host.client_weapons[0].holds(frag),
                    "the spent frag was still held when its cmd's touch ran"
                );
                return;
            }
        }
        panic!("the frag was never thrown");
    }

    /// `FireWeapon` spawns the grenade inside the release cmd (combat doc,
    /// 11.4): a run queued behind the release in the same tick leaves the
    /// missile where the release stood, stamped with the frame before.
    #[test]
    fn a_throw_leaves_from_its_own_cmd_not_the_tick_s_last() {
        let Some((mut sv, st, frag)) = last_frag_in_hand("main() {}") else {
            return;
        };
        let now = Instant::now();
        for i in 0..20 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.clients[0].as_mut().unwrap().pending.push(cmd.into());
            sv.tick(now);
        }
        let stood = sv.clients[0]
            .as_ref()
            .unwrap()
            .sim
            .as_ref()
            .unwrap()
            .origin();
        let release = frag_throw_cmd(20, st, frag);
        let run = UserCmd {
            server_time: release.server_time + 300,
            forward: 127,
            ..release
        };
        let c = sv.clients[0].as_mut().unwrap();
        c.pending.push(release.into());
        c.pending.push(run.into());
        sv.tick(now);
        let ran = sv.clients[0]
            .as_ref()
            .unwrap()
            .sim
            .as_ref()
            .unwrap()
            .origin();
        assert!(
            (glam::Vec3::from(ran) - glam::Vec3::from(stood)).length() > 30.0,
            "the run behind the release moved {stood:?} to {ran:?}"
        );
        let rt = sv.script.as_ref().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.proto)
            .next()
            .expect("the release threw nothing");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.proto, "pos");
        assert_eq!(traj.tr_time, sv.sv_time_ms - FRAME_MS);
        assert_eq!(
            [traj.base.x, traj.base.y],
            [stood[0].trunc(), stood[1].trunc()],
            "the grenade left from {:?}, not the release's {stood:?}",
            traj.base
        );
    }

    /// A `trigger_hurt` death runs `player_die` inside the victim's own
    /// `G_TouchTriggers`, where `r.currentOrigin` is the snapped
    /// `s.pos.trBase` (combat doc 5.5): the live grenade and the weapon the
    /// killed callback drops both leave from whole units, truncated toward
    /// zero.
    #[test]
    fn a_trigger_hurt_death_drops_and_clones_at_the_snapped_origin() {
        use vcod_common::net::msg::BUTTON_ATTACK;
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
             dir, hitloc) { self finishPlayerDamage(inflictor, attacker, damage, flags, mod, \
             weapon, point, dir, hitloc); }\n\
             CodeCallback_PlayerKilled(inflictor, attacker, damage, mod, weapon, dir, hitloc) \
             { self dropItem(self getcurrentweapon()); body = self cloneplayer(); }\n",
        );
        let Some((mut sv, st, frag)) = last_frag_in_hand_under(rt) else {
            return;
        };
        sv.test_set_client_origin(0, [10.6, -20.3, 1.0]);
        let cook = |i: i32| UserCmd {
            server_time: st + 50 * (i + 1),
            weapon: frag as u8,
            buttons: BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        fn sim(sv: &Server) -> &ClientSim {
            sv.clients[0].as_ref().unwrap().sim.as_ref().unwrap()
        }
        let mut i = 0;
        while sim(&sv).ps.grenade_time_left_ms == 0 {
            assert!(i < 60, "the pin never came out");
            sv.clients[0].as_mut().unwrap().pending.push(cook(i).into());
            sv.replay_moves();
            i += 1;
        }
        let stood = sim(&sv).origin();
        assert!(
            stood[0].fract() != 0.0 && stood[1].fract() != 0.0,
            "the cook settled on the unit grid at {stood:?}"
        );
        let rt = sv.script.as_mut().unwrap();
        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.triggers_mut().register_hurt(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, -8.0], [64.0, 64.0, 64.0]),
            1000,
            0,
        );
        rt.link_trigger_for_test(hurt);
        sv.clients[0].as_mut().unwrap().pending.push(cook(i).into());
        sv.replay_moves();

        let snapped = glam::Vec3::from(stood).trunc();
        assert_eq!(snapped.truncate(), glam::Vec2::new(10.0, -20.0));
        let rt = sv.script.as_mut().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.proto)
            .next()
            .expect("the death dropped no grenade");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.proto, "pos");
        assert_eq!(
            traj.base,
            snapped + glam::Vec3::Z * 40.0,
            "the grenade left from {:?}, standing at {stood:?}",
            traj.base
        );
        let item = rt
            .host
            .ents
            .iter_inuse()
            .find(|(_, e)| e.item.is_some_and(|i| i.dropped))
            .map(|(id, _)| id)
            .expect("the killed callback dropped no weapon");
        let at = rt.entity_origin_of(item).unwrap();
        assert_eq!([at[0], at[1]], [snapped.x, snapped.y]);
        // `G_SetOrigin(body, self->r.currentOrigin)` (5.2): the corpse too.
        let (_, body) = rt.host.bodies.entities().next().expect("no corpse");
        let at = body.origin(sv.proto);
        assert_eq!([at[0], at[1]], [snapped.x, snapped.y]);
    }

    /// The take happens once, at the cmd's touch: a frag the script gives
    /// back in the frame it was thrown stays held.
    #[test]
    fn a_frag_given_back_in_the_frame_it_was_thrown_stays_held() {
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        let script = "main() { \
               while (1) { \
                 wait 0.05; \
                 g = getentarray(\"grenade\", \"classname\"); \
                 if (g.size > 0) { \
                   p = getentarray(\"noclass\", \"classname\"); \
                   p[0] giveweapon(\"fraggrenade_mp\"); \
                   logprint(\"regive\"); \
                   return; \
                 } \
               } \
             }";
        let Some((mut sv, st, frag)) = last_frag_in_hand(script) else {
            return;
        };
        let now = Instant::now();
        for i in 0..60 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.clients[0].as_mut().unwrap().pending.push(cmd.into());
            sv.tick(now);
            if sv.script_log().iter().any(|l| l.trim() == "regive") {
                assert!(
                    sv.script.as_ref().unwrap().host.client_weapons[0].holds(frag),
                    "the frag the script gave back was taken again"
                );
                return;
            }
        }
        panic!("the script never saw the grenade");
    }

    /// A use key on the cmd whose move finished a switch grabs with the new
    /// `ps.weapon` in hand: both primaries are full, so the swap drops the
    /// weapon being held, and with the colt the tick started on it would
    /// have dropped the carbine from the primary slot instead (section 5).
    #[test]
    fn a_grab_on_the_switch_cmd_swaps_out_the_new_weapon() {
        use vcod_common::net::msg::BUTTON_USE;
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.weapon_table = Rc::new(crate::weapons::WeaponTable::load(&fs));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let index = |n| crate::configstrings::weapon_index(n).unwrap();
        let (carbine, thompson, colt) = (
            index("m1carbine_mp"),
            index("thompson_mp"),
            index("colt_mp"),
        );
        let table = sv.weapon_table.clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        for (w, slot) in [(carbine, 1), (thompson, 2), (colt, 3)] {
            held.give(w, slot);
        }
        held.current = colt as u8;
        let rt = sv.script.as_mut().unwrap();
        rt.host.weapons = table.clone();
        rt.host.client_weapons[0] = held;
        rt.host.client_vitals[0] = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 50.0], 0);
        let c = sv.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = colt as u8;
        for w in [carbine, thompson, colt] {
            let d = table.get(w).unwrap();
            sim.ps.ammoclip[d.clip_index] = d.clip_size as _;
            sim.ps.ammo[d.ammo_index] = d.max_ammo as _;
        }
        // One cmd long enough for the colt's putaway to end inside it.
        let cmd = UserCmd {
            server_time: c.last_processed_st + 1000,
            weapon: thompson as u8,
            buttons: BUTTON_USE,
            ..NULL_USERCMD
        };
        c.pending.push(cmd.into());
        sv.sv_time_ms = cmd.server_time;
        sv.replay_moves();
        let sim = sv.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        assert_eq!(sim.ps.weapon, thompson as u8, "the switch did not land");
        let rt = sv.script.as_mut().unwrap();
        assert!(rt.host.ents.get(pf).unwrap().item.unwrap().taken);
        let dropped: Vec<usize> = rt
            .host
            .ents
            .iter_inuse()
            .filter_map(|(_, e)| e.item.filter(|i| i.dropped).map(|i| i.index as usize))
            .collect();
        assert_eq!(dropped, vec![thompson]);
    }

    /// `Touch_Item` writes the playerstate inside the toucher's cmd (items
    /// doc, 13.2): a walk-over ammo grab is in the sim, and its 148 on the ring,
    /// before the cmd behind it fires, with no hit needed to land it there.
    /// Read straight after `replay_moves`, ahead of the script frame.
    #[test]
    fn a_walk_over_pickup_lands_before_the_next_cmd() {
        use vcod_common::net::msg::BUTTON_ATTACK;
        use vcod_common::pmove::weapon::EV_FIRE_WEAPON;
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.weapon_table = Rc::new(crate::weapons::WeaponTable::load(&fs));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap();
        let def = sv.weapon_table.get(carbine).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(carbine, sv.weapon_table.slot(carbine));
        held.current = carbine as u8;
        let rt = sv.script.as_mut().unwrap();
        rt.host.weapons = sv.weapon_table.clone();
        rt.host.client_weapons[0] = held;
        rt.host.client_vitals[0] = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        rt.place_item("mpweapon_m1carbine", [0.0, 0.0, 1.0], 20);
        let c = sv.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = carbine as u8;
        sim.ps.ammoclip[def.clip_index] = def.clip_size as _;
        sim.ps.ammo[def.ammo_index] = 10;
        let st = c.last_processed_st;
        for (i, buttons) in [0, BUTTON_ATTACK].into_iter().enumerate() {
            let cmd = UserCmd {
                server_time: st + 50 * (i as i32 + 1),
                weapon: carbine as u8,
                buttons,
                ..NULL_USERCMD
            };
            c.pending.push(cmd.into());
        }
        sv.replay_moves();
        let sim = sv.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        assert_eq!(sim.ps.ammo[def.ammo_index], 30, "the grab's ammo");
        let ring = sim.ring;
        let drained: Vec<i32> = (0..ring.seq)
            .map(|i| ring.events[(i & 3) as usize])
            .collect();
        assert_eq!(
            drained,
            vec![crate::game::pickup::EV_AMMO_PICKUP, EV_FIRE_WEAPON],
            "the grab's event ahead of the shot behind it"
        );
    }

    /// `getPlant` reads a planter's `self.angles` for the direction of its
    /// first trace, and `ClientThink_real` writes a player's after every cmd:
    /// the view's yaw with pitch and roll 0
    /// (docs/research/cod11-gsc-object-model.md 23.6). The spawn's yaw is not
    /// what it reads once the client has turned.
    #[test]
    fn a_player_s_angles_field_carries_its_view_yaw() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let mut nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 64.0], 90.0);
        // Pitch 22.5 down, yaw 45 on the wire; the placed spawn's delta
        // turns the yaw into 135.
        let ack = nc.incoming_sequence as i32;
        let cmd = UserCmd {
            server_time: 100,
            angles: [4096, 8192, 0],
            ..Default::default()
        };
        let pkt = nc
            .build_out(
                i32::from(sv.server_id),
                ack,
                0,
                &move_ops(sv.checksum_feed, ack, cmd),
                &Huffman::new(),
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        sv.tick(now + Duration::from_millis(50));
        let rt = sv.script.as_mut().unwrap();
        let ent = rt.client_entity(0).expect("client 0 has an entity");
        assert_eq!(rt.field_str(ent, "angles"), "(0.00, 135.00, 0.00)");
    }

    /// The gap `_minefields.gsc` puts between the warning click and the
    /// blast, measured on our clock. The stock script (pak5.pk3,
    /// `maps/MP/_minefields.gsc`) is `playsound("minefield_click");
    /// wait(.5); wait(randomFloat(.5));` and then an `istouching` re-check,
    /// so retail's gap is 500 to 1000 ms by construction and every draw has
    /// to land in it. A `wait` wakes on the first frame past its deadline,
    /// which is what buys the extra frame at the top.
    ///
    /// The draws also have to differ: a `randomFloat` stuck at one value
    /// would sit inside that window and pass everything else here.
    ///
    /// Six draws on our clock read 550, 700, 850, 750, 800, 850 ms, which is
    /// what settles the open question of whether ours detonates sooner than
    /// retail: it does not, and neither `wait` nor `randomFloat` is short.
    #[test]
    fn the_minefield_kill_delay_is_half_to_one_second() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { \
                   while (1) { \
                     wait 0.05; \
                     players = getentarray(\"noclass\", \"classname\"); \
                     if (players.size > 0) { \
                       for (i = 0; i < 6; i++) { players[0] mine(); } \
                       return; \
                     } \
                   } \
                 } \
                 mine() { \
                   self playsound(\"minefield_click\"); \
                   logprint(\"click\"); \
                   wait(.5); \
                   wait(randomFloat(.5)); \
                   logprint(\"boom\"); \
                 }",
            ),
        );
        let _nc = begun(&mut sv, now);

        let mut drained = sv.script_log().len();
        let mut click: Option<i32> = None;
        let mut gaps: Vec<i32> = Vec::new();
        for frame in 0..200 {
            sv.tick(now);
            for line in &sv.script_log()[drained..] {
                match line.trim() {
                    "click" => click = Some(frame),
                    "boom" => gaps.push((frame - click.take().expect("a click first")) * FRAME_MS),
                    _ => {}
                }
            }
            drained = sv.script_log().len();
        }

        assert_eq!(gaps.len(), 6, "six kills ran to the blast");
        for gap in &gaps {
            assert!(
                (500..=1000 + FRAME_MS).contains(gap),
                "a kill waited {gap} ms; the stock script waits 500 to 1000, \
                 plus at most one frame of wake quantization. All: {gaps:?}"
            );
        }
        assert!(
            gaps.iter().any(|g| *g != gaps[0]),
            "every draw was {} ms, so randomFloat is not drawing",
            gaps[0]
        );
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
        // A clock the burst's last cmd is within 200 ms of: the earliest
        // cmds clamp up to 1000 ms behind it, and their frames run as one.
        sv.sv_time_ms = 1900;
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
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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

    /// A spectator in slot 0 on a real netchan and two players put straight
    /// into slots 1 and 2 with no socket, under a script that does nothing
    /// but give every client an entity. The players' `sessionstate` is set
    /// to playing by hand, which is what makes them followable.
    struct FollowRig {
        sv: Server,
        nc: Netchan,
        ring: SnapshotRing,
        chain: MoveChain,
        now: Instant,
        st: i32,
    }

    const FOLLOW_P1: [f32; 3] = [200.0, 0.0, 0.0];
    const FOLLOW_P2: [f32; 3] = [-300.0, 100.0, 0.0];

    impl FollowRig {
        fn new() -> Self {
            Self::with_script(crate::game::script::ScriptRuntime::for_test("main() {}"))
        }

        fn with_script(rt: crate::game::script::ScriptRuntime) -> Self {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::single_cluster(),
                spawn: ([0.0, 0.0, 64.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
            });
            install_script(&mut sv, rt);
            let nc = begun(&mut sv, now);
            let chain = MoveChain::new(sv.checksum_feed);
            let mut rig = FollowRig {
                sv,
                nc,
                ring: SnapshotRing::new(),
                chain,
                now,
                st: 0,
            };
            rig.add_player(1, FOLLOW_P1, 90.0);
            rig.add_player(2, FOLLOW_P2, 180.0);
            rig.step(0);
            for slot in 1..3 {
                rig.script().set_client_state_for_test(slot, "playing");
            }
            rig.step(0);
            rig
        }

        fn script(&mut self) -> &mut crate::game::script::ScriptRuntime {
            self.sv.script.as_mut().unwrap()
        }

        fn add_player(&mut self, slot: usize, origin: [f32; 3], yaw: f32) {
            let mut c = Client::new(
                addr(6 + slot as u16),
                0x3000 + slot as u16,
                0,
                format!("\\name\\p{slot}"),
                self.now,
            );
            c.is_bot = true;
            let mut sim = ClientSim::spectator(origin, yaw, [0; 3]);
            sim.become_player(origin, yaw, [0; 3]);
            c.sim = Some(sim);
            self.sv.clients[slot] = Some(c);
            self.script().push_client_event(ClientEvent::Connect {
                slot,
                name: format!("p{slot}"),
            });
        }

        fn sim(&self, slot: usize) -> &ClientSim {
            self.sv.clients[slot]
                .as_ref()
                .unwrap()
                .sim
                .as_ref()
                .unwrap()
        }

        fn sim_mut(&mut self, slot: usize) -> &mut ClientSim {
            self.sv.clients[slot]
                .as_mut()
                .unwrap()
                .sim
                .as_mut()
                .unwrap()
        }

        /// One cmd from the spectator holding `buttons`, and the frame it
        /// lands in.
        fn step(&mut self, buttons: u8) -> vcod_common::net::snapshot::Snapshot {
            self.st += 50;
            self.now += std::time::Duration::from_millis(50);
            let ack = self.nc.incoming_sequence as i32;
            let cmd = UserCmd {
                server_time: self.st,
                buttons,
                ..Default::default()
            };
            let ops = self.chain.ops(ack, &[cmd]);
            let pkt = self
                .nc
                .build_out(i32::from(self.sv.server_id), ack, 0, &ops, &Huffman::new())
                .unwrap();
            self.sv.handle_packet(addr(5), &pkt, self.now);
            latest_snapshot(&mut self.sv, &mut self.nc, &mut self.ring, self.now)
        }

        /// One frame with no input, and every server command it carried, as
        /// `<reliable sequence>:<text>`, each once: a command leaves in a
        /// packet of its own and again in the snapshot's.
        fn commands(&mut self) -> Vec<String> {
            self.st += 50;
            self.now += std::time::Duration::from_millis(50);
            let huff = Huffman::new();
            self.sv.tick(self.now);
            let mut out = Vec::new();
            for (_, pkt) in self.sv.take_outgoing() {
                if let Ok(Some(m)) = self.nc.process_in(&pkt, &huff) {
                    let mut r = MsgReader::new(&m[4..], &huff);
                    while !r.is_overflowed() && r.read_byte() == msg::SVC_SERVER_COMMAND {
                        let seq = r.read_long();
                        out.push(format!("{seq}:{}", r.read_big_string()));
                    }
                }
            }
            out.sort();
            out.dedup();
            out
        }

        /// A press and the release after it; the frame of the press.
        fn press(&mut self, buttons: u8) -> vcod_common::net::snapshot::Snapshot {
            let s = self.step(buttons);
            self.step(0);
            s
        }
    }

    fn ps_i32(s: &vcod_common::net::snapshot::Snapshot, name: &str) -> i32 {
        s.ps.field_i32(&PROTOCOL_V1, name)
    }

    /// Retail's run (docs/research/cod11-spectator-follow.md): an attack
    /// press from free flight lands on the first playing slot after 0, and
    /// the frame is that client's playerstate with the own-view bit swapped
    /// for the follow bit. The followed client's own entity is not sent,
    /// the other one still is.
    #[test]
    fn attack_rides_the_next_playing_client_with_its_playerstate() {
        let mut rig = FollowRig::new();
        let free = rig.step(0);
        assert_eq!(ps_i32(&free, "clientNum"), 0);
        assert!(free.entities.contains_key(&1) && free.entities.contains_key(&2));

        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x10000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P1);
        assert!(!s.entities.contains_key(&1), "the followed client's entity");
        assert!(s.entities.contains_key(&2));
    }

    #[test]
    fn attack_cycles_forward_and_melee_back() {
        let mut rig = FollowRig::new();
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 1);
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P2);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_MELEE), "clientNum"), 1);
        // A held button is one press.
        rig.step(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&rig.step(msg::BUTTON_ATTACK), "clientNum"), 2);
    }

    /// The sight bit's press ends the follow, and the spectator is left
    /// where `StopFollowing` puts it off the followed eye.
    #[test]
    fn a_sight_press_ends_a_free_follow_behind_the_eye() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let (eye, view) = (rig.sim(1).ps.view().eye, rig.sim(1).view_angles());
        let s = rig.step(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
        let (spot, _) = crate::follow::stop_spot(None, eye.into(), view);
        let o = s.ps.origin(&PROTOCOL_V1);
        assert!(
            (glam::Vec3::from(o) - glam::Vec3::from(spot)).length() < 0.5,
            "{o:?} against {spot:?}"
        );
        assert!(s.entities.contains_key(&1));
        // The release is the other edge, and there is nothing left to end.
        assert_eq!(ps_i32(&rig.step(0), "clientNum"), 0);
    }

    /// The single-client scope is tested against the frame's
    /// `ps.clientNum`, so the follower of a hit client is sent that client's
    /// own 176 and not the 174 everyone else gets.
    #[test]
    fn a_follower_is_sent_the_followed_clients_own_flesh_impact() {
        use crate::game::temp_entity::{Scope, TempEntity};
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let te = |event, scope| TempEntity {
            event,
            parm: 0,
            surf_type: 7,
            other: 2,
            attacker: 2,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: FOLLOW_P1,
            scope,
        };
        rig.sv.test_push_temp_entity(te(176, Scope::Only(1)));
        rig.sv.test_push_temp_entity(te(174, Scope::AllBut(1)));
        let s = rig.step(0);
        let events: Vec<i32> = s
            .entities
            .values()
            .map(|e| e.field_i32(&PROTOCOL_V1, "eType") - temp_entity::ET_EVENTS)
            .collect();
        assert!(events.contains(&176), "{events:?}");
        assert!(!events.contains(&174), "{events:?}");
    }

    /// A dead client keeps its own-view bit, so the follow stays on the
    /// body; the frame it goes spectator it has lost the bit and the
    /// follower is let go behind the dead eye (the retail sd run).
    #[test]
    fn a_dead_client_is_still_followed_and_a_spectating_one_lets_go() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        rig.script().set_client_state_for_test(1, "dead");
        rig.step(0);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_type"), 6);

        let (eye, view) = (rig.sim(1).ps.view().eye, rig.sim(1).view_angles());
        rig.script().set_client_state_for_test(1, "spectator");
        rig.sim_mut(1)
            .become_spectator([0.0, 0.0, 300.0], 0.0, [0; 3]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        let (spot, _) = crate::follow::stop_spot(None, eye.into(), view);
        let o = s.ps.origin(&PROTOCOL_V1);
        assert!(
            (glam::Vec3::from(o) - glam::Vec3::from(spot)).length() < 0.5,
            "{o:?} against {spot:?}"
        );
    }

    /// `player_die`'s walk (combat doc, 5.1 step 9) off a `suicide` inside
    /// the script frame: the spectator following the victim is sent the
    /// scoreboard the moment the killed callback returns, behind what that
    /// callback queued and ahead of what a thread started after the kill
    /// queues, with the rows as the callback left them. A death of a client
    /// it does not follow sends it nothing.
    #[test]
    fn a_script_death_sends_its_followers_the_scoreboard_when_the_callback_returns() {
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            r#"main() { thread killer(2); thread killer(1); }
               killer(n) {
                   while (!isdefined(level.kill) || level.kill != n) wait 0.05;
                   // Never spawned, so still G_InitGentity's "noclass".
                   players = getentarray("noclass", "classname");
                   for (i = 0; i < players.size; i++)
                       if (players[i] getentitynumber() == n)
                           victim = players[i];
                   victim suicide();
                   thread talker(victim, n);
               }
               talker(victim, n) { victim.score = 8; iprintln("talker" + n); }
               CodeCallback_PlayerKilled(a, b, c, d, e, f, g) {
                   self.score = 7;
                   iprintln("killed");
                   wait 0.05;
               }"#,
        );
        let mut rig = FollowRig::with_script(rt);
        rig.press(msg::BUTTON_ATTACK);
        let frame = |rig: &mut FollowRig, slot: i32| -> Vec<String> {
            rig.script()
                .set_level_field_for_test("kill", vcod_gsc::Value::Int(slot));
            let mut cmds: Vec<(u32, String)> = rig
                .commands()
                .into_iter()
                .map(|c| {
                    let (seq, text) = c.split_once(':').unwrap();
                    (seq.parse().unwrap(), text.to_string())
                })
                .collect();
            cmds.sort();
            cmds.into_iter().map(|(_, t)| t).collect()
        };
        let other = frame(&mut rig, 2);
        assert!(other.iter().any(|t| t.contains("talker2")), "{other:?}");
        assert!(other.iter().all(|t| !t.starts_with("b ")), "{other:?}");
        let texts = frame(&mut rig, 1);
        let at = |needle: &str| texts.iter().position(|t| t.contains(needle));
        let b = at("b ").expect("no scoreboard");
        assert_eq!(texts.iter().filter(|t| t.starts_with("b ")).count(), 1);
        assert!(at("killed").unwrap() < b, "{texts:?}");
        assert!(b < at("talker1").unwrap(), "{texts:?}");
        let cells: Vec<i64> = texts[b]
            .split(' ')
            .skip(4)
            .map(|n| n.parse().unwrap())
            .collect();
        let row = cells.chunks(5).find(|r| r[0] == 1).unwrap();
        assert_eq!(row[1], 7, "the row reads the callback's score: {texts:?}");
    }

    /// `ClientDisconnect` moves every follower of the leaving client on to
    /// the next one, and lets it go when there is none.
    #[test]
    fn a_dropped_client_passes_its_followers_on() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.sv.drop_client(1, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        rig.sv.drop_client(2, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
    }

    /// A script's `spectatorclient` is a forced follow: both bits on the
    /// frame, the buttons do nothing to it, and one naming a slot with
    /// nothing to follow is written back to -1.
    #[test]
    fn a_forced_follow_carries_both_bits_and_an_empty_slot_is_cleared() {
        let mut rig = FollowRig::new();
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(2));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 2);
        assert_eq!(ps_i32(&rig.step(msg::BUTTON_ADS), "clientNum"), 2);

        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(5));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(
            rig.script().client_field(0, "spectatorclient").as_deref(),
            Some("-1")
        );
    }

    /// With no archive (no `setarchive(true)`) a killcam's age finds no
    /// frame, the retry lowers it to 0 and the copy is the live one; the 0 is
    /// what the stock `archivetime <= delay` check reads back.
    #[test]
    fn with_the_archive_off_a_killcam_follows_live_and_reads_back_zero() {
        let mut rig = FollowRig::new();
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some("0")
        );
    }

    /// Steps `frames` frames with the archive on, players 1 and 2 moving
    /// 10 units a frame, and returns where each stood by server time.
    fn archive_moving(rig: &mut FollowRig, frames: i32) -> BTreeMap<i32, ([f32; 3], [f32; 3])> {
        rig.script().host.archive_on = true;
        let mut trail = BTreeMap::new();
        for i in 0..frames {
            let (a, b) = ([10.0 * i as f32, 0.0, 0.0], [0.0, 10.0 * i as f32, 0.0]);
            rig.sv.test_set_client_origin(1, a);
            rig.sv.test_set_client_origin(2, b);
            let s = rig.step(0);
            trail.insert(s.server_time, (a, b));
        }
        trail
    }

    /// The killcam proper: the forced follow's copy is the followed client
    /// as the archive had it `archivetime` ago, its `deltaTime` the age, and
    /// the entity list is that frame's too, times shifted (0x808ef7c,
    /// 0x808f130).
    #[test]
    fn a_killcam_replays_the_followed_client_from_archivetime_ago() {
        let mut rig = FollowRig::new();
        let trail = archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.sv.test_set_client_origin(1, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&s, "deltaTime"), 1000);
        let (then_1, then_2) = trail[&(s.server_time - 1000)];
        assert_eq!(s.ps.origin(&PROTOCOL_V1), then_1);
        assert!(!s.entities.contains_key(&1));
        let e2 = &s.entities[&2];
        assert_eq!(
            e2.origin(&PROTOCOL_V1),
            then_2,
            "entity 2 as the frame had it"
        );
        let tr = e2.field_i32(&PROTOCOL_V1, "pos.trTime");
        let archived_tr = rig
            .sv
            .archive
            .frame(rig.sv.archive.lookup(&mut 1000).unwrap())
            .unwrap()
            .world
            .culled[&2]
            .field_i32(&PROTOCOL_V1, "pos.trTime");
        if archived_tr != 0 {
            assert_eq!(tr, archived_tr + 1000);
        }
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some("1")
        );
        // The age holds: the next frame replays the next archived one.
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "deltaTime"), 1000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), trail[&(s.server_time - 1000)].0);
    }

    /// A stock killcam asks for nine seconds; a level younger than that
    /// trims the age to what it has, which is what the script reads back.
    #[test]
    fn a_young_archive_trims_the_age_the_script_reads_back() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 40);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        let age = ps_i32(&s, "deltaTime");
        assert!((1800..=2000).contains(&age), "{age}");
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some(format!("{}", age as f32 / 1000.0).as_str())
        );
    }

    /// A frame where the followed client had no view of its own (it was
    /// spectating) fails, and the age is lowered 50 ms at a time until one
    /// answers (0x407c1..0x40859).
    #[test]
    fn a_killcam_steps_younger_past_frames_the_followed_client_is_missing_from() {
        let mut rig = FollowRig::new();
        rig.script().host.archive_on = true;
        rig.script().set_client_state_for_test(1, "spectator");
        for _ in 0..40 {
            rig.step(0);
        }
        rig.script().set_client_state_for_test(1, "playing");
        for _ in 0..10 {
            rig.step(0);
        }
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        let age = ps_i32(&s, "deltaTime");
        assert!((450..=550).contains(&age), "{age}");
    }

    /// sd's killcam ends without the move back to `dead`: `spectatorclient`
    /// -1 and `archivetime` 0 leave the follow on the same client, live and
    /// no longer forced (the retail sd run, spectator-follow doc section 12).
    #[test]
    fn a_killcam_left_as_a_spectator_follows_the_same_client_live() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.step(0);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(-1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(0.0));
        rig.sv.test_set_client_origin(1, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x10000);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), [-50.0, -50.0, 0.0]);
    }

    /// A spectator with an `archivetime` and nobody to follow (sd's bomb
    /// camera) keeps its own view and is sent the archived frame's entities
    /// (0x808f130 reads the age for every client).
    #[test]
    fn an_archivetime_without_a_follow_replays_only_the_entities() {
        let mut rig = FollowRig::new();
        let trail = archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.sv.test_set_client_origin(2, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
        assert_eq!(
            s.entities[&2].origin(&PROTOCOL_V1),
            trail[&(s.server_time - 1000)].1
        );
    }

    /// The stock killcam's end: `spectatorclient` -1, `archivetime` 0 and
    /// `sessionstate` `dead` with the replay still in the playerstate is
    /// `ClientEndFrame`'s `ClientSpawn` arm, which leaves a dead client where
    /// the copy stood.
    #[test]
    fn the_killcam_s_end_spawns_a_dead_client_where_the_copy_stood() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        let last = rig.step(0);
        assert_eq!(ps_i32(&last, "clientNum"), 1);
        rig.script().host.client_vitals[0].dead = true;
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(-1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(0.0));
        rig.script().set_client_state_for_test(0, "dead");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x40000);
        // Horizontally: the rig has no floor under the copy for the spawn's
        // dead think to settle on.
        let (o, l) = (s.ps.origin(&PROTOCOL_V1), last.ps.origin(&PROTOCOL_V1));
        assert_eq!(o[..2], l[..2]);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
    }

    /// Script moving a free follower to `playing` with no spawn of its own:
    /// `ClientEndFrame` finds the copy still in the playerstate and spawns
    /// the client there, alive (spectator-follow doc, 12.7).
    #[test]
    fn a_follower_script_puts_in_play_is_spawned_where_the_copy_stood() {
        let mut rig = FollowRig::new();
        let last = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&last, "clientNum"), 1);
        rig.script().set_client_state_for_test(0, "playing");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x40000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P1);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
    }

    /// `StopFollowing` (0x46a28) writes the spot, the view, `clientNum` and
    /// a handful of weapon fields, and never the velocity; the rest of the
    /// copy stays in the spectator's playerstate until something writes it
    /// (the retail dm run: `health` 100 and `weapon` 9 on every free frame
    /// after the sight press).
    #[test]
    fn a_stopped_follow_keeps_the_rest_of_the_copy() {
        let mut rig = FollowRig::new();
        rig.script().host.client_vitals[1].health = 100;
        rig.script().set_client_weapon(1, 9);
        rig.sim_mut(1).ps.velocity = glam::Vec3::new(120.0, 0.0, 0.0);
        rig.sim_mut(1).ps.ducked = true;
        rig.press(msg::BUTTON_ATTACK);
        let copy = rig.step(0);
        assert_eq!((copy.ps.health(), ps_i32(&copy, "weapon")), (100, 9));
        assert_eq!(ps_i32(&copy, "pm_flags"), 0x10000 | 0x2);
        let stop = rig.step(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&stop, "clientNum"), 0);
        assert_eq!(ps_i32(&stop, "pm_type"), 4);
        // `StopFollowing` takes 0x10020 off the copy's flags and leaves the
        // rest (0x46bb1), and `SetClientViewAngle` pitches the view down 15,
        // which the stop cmd's own `PM_UpdateViewAngles` keeps: retail's stop
        // frame read pitch 15.
        assert_eq!(ps_i32(&stop, "pm_flags"), 0x2);
        let pitch = stop.ps.field_f32(&PROTOCOL_V1, "viewangles[0]");
        assert!((pitch - 15.0).abs() < 0.01, "{pitch}");
        let vx = stop.ps.field_f32(&PROTOCOL_V1, "velocity[0]");
        assert!(
            vx > 0.0 && vx < 120.0,
            "the copy's velocity, one flight step on: {vx}"
        );
        // The flight writes the eye heights: retail's stop frame read 0 for
        // both where the copy read 60.
        assert_eq!(ps_i32(&stop, "viewHeightTarget"), 0);
        assert_eq!(stop.ps.field_f32(&PROTOCOL_V1, "viewHeightCurrent"), 0.0);
        for s in [stop, rig.step(msg::BUTTON_ADS), rig.step(0)] {
            assert_eq!(ps_i32(&s, "clientNum"), 0);
            assert_eq!((s.ps.health(), ps_i32(&s, "weapon")), (100, 9));
        }
    }

    /// The same stop from the end frame, the followed client gone
    /// spectator: until the follower's next cmd runs `SpectatorThink`'s
    /// free-flight arm the frame keeps the copy's `pm_type` (the retail sd
    /// run: 6, then 4).
    #[test]
    fn an_end_frame_stop_keeps_the_copy_s_pm_type_until_the_next_cmd() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        rig.script().set_client_state_for_test(1, "dead");
        rig.step(0);
        rig.step(0);
        rig.script().set_client_state_for_test(1, "spectator");
        rig.sim_mut(1)
            .become_spectator([0.0, 0.0, 300.0], 0.0, [0; 3]);
        // This frame's cmd runs while the follow is still on; the stop is
        // the end frame's.
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
        assert_eq!(ps_i32(&rig.step(0), "pm_type"), 4);
    }

    /// `G_RunFrame`'s end-frame loop runs in slot order, so a follower
    /// numbered below its target copies it before the target's own
    /// `ClientEndFrame` has run: what that end frame writes reaches the
    /// follower a frame late. The retail dm run: the death frame read
    /// `health` 0 and `pm_type` 0 in slot 0 and 6 in slot 2.
    #[test]
    fn a_follower_below_its_target_reads_the_end_frame_s_fields_a_frame_late() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.health(), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&rig.step(0), "pm_type"), 6);
    }

    /// `P_DamageFeedback` puts `EV_PAIN` on the ring inside the target's own
    /// end frame, so a follower below it copies the ring without it and sees
    /// the pain a frame late (spectator-follow doc, 5).
    #[test]
    fn a_follower_below_its_target_sees_its_pain_a_frame_late() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let seq = ps_i32(&rig.step(0), "eventSequence");
        rig.script().host.client_vitals[1].health = 67;
        rig.script().host.client_vitals[1].max_health = 100;
        rig.script().host.client_sim_ops.push((
            1,
            crate::game::host::SimOp::Damaged {
                damage: 33,
                point: FOLLOW_P1,
                dir: [1.0, 0.0, 0.0],
                knockback: false,
                attacker: Some(2),
                attacker_origin: Some(FOLLOW_P2),
                fatal: false,
            },
        ));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.health(), 67);
        assert_eq!(ps_i32(&s, "eventSequence"), seq, "the pain rode early");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "eventSequence"), seq + 1);
        assert_eq!(ps_i32(&s, &format!("events[{}]", seq & 3)), 187);
        assert_eq!(ps_i32(&s, &format!("eventParms[{}]", seq & 3)), 67);
    }

    /// `ClientSpawn` runs the spawned client's own `ClientEndFrame`
    /// (0x42a75), whose playing arm sets the own-view bit, so a follower
    /// numbered below it copies the new life on the spawn's frame rather
    /// than finding nothing and letting go (the retail dm run: slot 0 read
    /// slot 1's respawn frame).
    #[test]
    fn a_follower_below_its_target_rides_the_target_s_respawn() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.sim_mut(1).become_player(FOLLOW_P2, 0.0, [0; 3]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P2);
    }

    fn rcon(sv: &mut Server, line: &str, now: Instant) -> Vec<String> {
        sv.handle_packet(addr(9), &oob(&format!("rcon {line}")), now);
        sv.take_outgoing()
            .into_iter()
            .map(|(to, pkt)| {
                assert_eq!(to, addr(9));
                String::from_utf8_lossy(&pkt[4..]).into_owned()
            })
            .collect()
    }

    /// The replies a retail server gave (docs/research/cod11-server-handshake.md,
    /// "rcon"): the password is checked after a server-wide 500 ms window
    /// that drops anything inside it unanswered.
    #[test]
    fn rcon_answers_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        assert_eq!(
            rcon(&mut sv, "x status", t),
            ["print\nNo rconpassword set on the server.\n"]
        );
        sv.set_cvar("rconPassword", "secret");
        let t = t + Duration::from_millis(500);
        assert_eq!(
            rcon(&mut sv, "wrong status", t),
            ["print\nBad rconpassword.\n"]
        );
        assert!(rcon(&mut sv, "secret status", t + Duration::from_millis(499)).is_empty());
        let t = t + Duration::from_millis(500);
        assert_eq!(
            rcon(&mut sv, "secret status", t),
            [
                "print\nmap: mp_carentan\nnum score ping name            lastmsg address               qport rate\n\
              --- ----- ---- --------------- ------- --------------------- ----- -----\n\n"
            ]
        );
        // A command nobody registered prints nothing, and so does an empty one.
        let t = t + Duration::from_millis(500);
        assert_eq!(rcon(&mut sv, "secret bogus_cmd", t), ["print\n"]);
        let t = t + Duration::from_millis(500);
        assert_eq!(rcon(&mut sv, "secret", t), ["print\n"]);
    }

    /// `clientkick` drops through `SV_DropClient`: the slot is a zombie that
    /// resends the notice every frame until it is acked, shows as `ZMBI`,
    /// and is freed `sv_zombietime` after the kick, not at it.
    #[test]
    fn a_kicked_client_lingers_as_a_zombie() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        let mut nc = active(&mut sv, t);
        sv.take_outgoing();
        let huff = Huffman::new();
        assert_eq!(
            rcon(&mut sv, "secret clientkick 0", t),
            ["print\n0:vcod EXE_PLAYERKICKED\n"]
        );
        assert_eq!(sv.client_count(), 0);
        sv.tick(t);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1);
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(
            cmds.last().map(String::as_str),
            Some("w \"EXE_PLAYERKICKED\"")
        );

        let t1 = t + Duration::from_millis(500);
        let status = rcon(&mut sv, "secret status", t1).concat();
        assert!(
            status.contains("  0     0 ZMBI                     500 127.0.0.1:5 "),
            "{status}"
        );

        // A different address finds no free slot while the zombie holds 0.
        sv.tick(t1);
        sv.take_outgoing();
        let seq = sv.zombies[0].as_ref().unwrap().netchan.reliable_sequence as i32;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, seq, &ack_ops(), &huff).unwrap(),
            t1,
        );
        sv.tick(t1);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "the zombie still gets frames");
        assert!(
            server_commands(&mut nc, &out[0].1, &huff).is_empty(),
            "the ack stopped the resend"
        );
        // Its packets do not keep it alive.
        sv.tick(t + ZOMBIE_TIME);
        assert!(sv.zombies[0].is_some());
        sv.take_outgoing();
        sv.tick(t + ZOMBIE_TIME + Duration::from_millis(50));
        assert!(sv.zombies[0].is_none());
        assert!(sv.take_outgoing().is_empty());
    }

    /// A zombie of the same peer is a reconnect into its own slot, and a
    /// stranger gets the next free one.
    #[test]
    fn a_zombie_slot_is_held_from_strangers_but_not_its_own_client() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let _nc = active(&mut sv, t);
        sv.drop_client(0, "EXE_DISCONNECTED");
        let _ = connected(&mut sv, SocketAddr::from(([10, 0, 0, 2], 5)), t);
        assert!(sv.clients[0].is_none() && sv.clients[1].is_some());
        let t2 = t + RECONNECT_LIMIT + Duration::from_millis(100);
        let _ = connected(&mut sv, addr(5), t2);
        assert!(sv.clients[0].is_some() && sv.zombies[0].is_none());
    }

    fn local_master(_: &str) -> Option<SocketAddr> {
        Some(addr(29463))
    }

    /// `dedicated 2` heartbeats on the first frame, again when the first
    /// client connects, and flatlines on `quit`; the port is always 20510.
    /// `kick`, `dumpuser`, `serverinfo` and `say` against the replies a
    /// retail server gave (handshake doc, "rcon commands").
    #[test]
    fn rcon_admin_commands_answer_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        let step = Duration::from_millis(500);
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += step;
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        assert_eq!(
            ask(&mut sv, "kick"),
            "print\nUsage: kick <player name>\nkick all = kick everyone\n"
        );
        assert_eq!(
            ask(&mut sv, "dumpuser nobody"),
            "print\nPlayer nobody is not on the server\n"
        );
        assert_eq!(ask(&mut sv, "dumpuser"), "print\nUsage: info <userid>\n");
        let dump = ask(&mut sv, "dumpuser VCOD");
        assert!(dump.starts_with("print\nuserinfo\n--------\nname                vcod\n"));
        assert!(
            dump.ends_with("ip                  127.0.0.1:5\n"),
            "{dump}"
        );
        let info = ask(&mut sv, "serverinfo");
        assert!(info.starts_with("print\nServer info settings:\ng_gametype          dm\n"));
        assert!(info.contains("sv_hostname         vcod test\n"), "{info}");
        assert_eq!(
            ask(&mut sv, "set foo"),
            "print\nusage: set <variable> <value>\n"
        );

        assert_eq!(ask(&mut sv, "say hello there"), "print\n");
        sv.tick(t);
        let out = sv.take_outgoing();
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds, ["h \"\u{15}console: hello there\""]);

        assert_eq!(
            ask(&mut sv, "kick vcod"),
            "print\n0:vcod EXE_PLAYERKICKED\n"
        );
        assert_eq!(sv.client_count(), 0);
        assert!(sv.zombies[0].is_some());
    }

    /// `set` writes the running level's table and a bare name reads it back
    /// in `Cvar_Command`'s format; `g_gametype` is latched until a load.
    #[test]
    fn rcon_set_and_query_cvars_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        if let Some(rt) = sv.script.as_mut() {
            rt.host.cvars.set("g_gametype", "dm");
            rt.host.cvars.set("scr_dm_timelimit", "30");
        }
        sv.set_cvar("rconpassword", "pw");
        let step = Duration::from_millis(500);
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += step;
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        assert_eq!(ask(&mut sv, "set foo a b c"), "print\n");
        assert_eq!(
            ask(&mut sv, "foo"),
            "print\n\"foo\" is:\"a b c^7\" default:\"a b c^7\"\n"
        );
        assert_eq!(ask(&mut sv, "foo baz"), "print\n");
        assert_eq!(
            ask(&mut sv, "FOO"),
            "print\n\"foo\" is:\"baz^7\" default:\"a b c^7\"\n"
        );
        assert_eq!(ask(&mut sv, "set scr_dm_timelimit 5"), "print\n");
        assert_eq!(sv.script_cvar("scr_dm_timelimit").as_deref(), Some("5"));
        assert_eq!(
            ask(&mut sv, "set g_gametype tdm"),
            "print\ng_gametype will be changed upon restarting.\n"
        );
        assert_eq!(
            ask(&mut sv, "g_gametype"),
            "print\n\"g_gametype\" is:\"dm^7\" default:\"dm^7\"\nlatched: \"tdm\"\n"
        );
        // Latched means serverinfo keeps the running level's value.
        assert!(sv.configstring(0).contains("\\g_gametype\\dm\\"));
        assert_eq!(ask(&mut sv, "set g_gametype tdm"), "print\n");
        assert_eq!(
            ask(&mut sv, "set g_gametype sd"),
            "print\ng_gametype will be changed upon restarting.\n"
        );
        // The live value clears the latch silently.
        assert_eq!(ask(&mut sv, "set g_gametype dm"), "print\n");
        assert_eq!(
            ask(&mut sv, "g_gametype"),
            "print\n\"g_gametype\" is:\"dm^7\" default:\"dm^7\"\n"
        );
        assert_eq!(
            ask(&mut sv, "set g_useGear 0"),
            "print\ng_useGear will be changed upon restarting.\n"
        );
        assert_eq!(
            ask(&mut sv, "g_useGear"),
            "print\n\"g_useGear\" is:\"1^7\" default:\"1^7\"\nlatched: \"0\"\n"
        );
        assert_eq!(ask(&mut sv, "set sv_hostname newname"), "print\n");
        assert!(sv.configstring(0).contains("\\sv_hostname\\newname"));
    }

    /// `quit` runs `SV_FinalMessage("EXE_SERVERQUIT")` and exits without
    /// answering the rcon that asked for it.
    #[test]
    fn quit_tells_the_clients_and_answers_nothing() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        sv.handle_packet(addr(9), &oob("rcon pw quit"), t);
        assert!(sv.quit_requested());
        let out = sv.take_outgoing();
        assert!(
            out.iter().all(|(to, _)| *to == addr(5)),
            "an rcon reply went out"
        );
        assert_eq!(out.len(), 2);
        assert_eq!(
            server_commands(&mut nc, &out[0].1, &huff),
            ["e \"EXE_SERVERQUIT\"", "w"]
        );
        sv.tick(t);
        assert!(sv.take_outgoing().is_empty());
    }

    /// `banUser` and `banClient` against the replies a retail server gave
    /// (handshake doc, "Bans"). The client stays connected, as on retail; a
    /// banned address off the LAN is refused its next challenge, and a LAN
    /// one is not, since retail never asks the authorize server about it.
    #[test]
    fn rcon_bans_answer_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let _nc = begun(&mut sv, t);
        sv.take_outgoing();
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += Duration::from_millis(500);
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        for (line, want) in [
            ("banUser", "Usage: banUser <player name>\n"),
            ("banUser a b", "Usage: banUser <player name>\n"),
            ("banUser nobody", "Player nobody is not on the server\n"),
            ("banClient", "Usage: banClient <client number>\n"),
            ("banClient 3", "Client 3 is not active\n"),
            ("banClient x", "Bad slot number: x\n"),
            ("banUser VCOD", "vcod was banned from coming back\n"),
            ("banClient 0", "vcod was banned from coming back\n"),
        ] {
            assert_eq!(ask(&mut sv, line), format!("print\n{want}"), "{line}");
        }
        assert_eq!(sv.client_count(), 1);
        assert!(sv.bans.contains(std::net::Ipv4Addr::LOCALHOST));
        sv.handle_packet(addr(6), &oob("getchallenge"), at);
        assert_eq!(reply(&mut sv).1, "challengeResponse");

        let wan = SocketAddr::from(([203, 0, 113, 7], 28960));
        sv.bans.add(std::net::Ipv4Addr::new(203, 0, 113, 7));
        sv.handle_packet(wan, &oob("getchallenge"), at);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (to, cmd.as_str(), rest.as_slice()),
            (wan, "error", &b"EXE_ERR_BAD_CDKEY"[..])
        );
    }

    /// `killserver` tells every client `EXE_SERVERKILLED`, answers nothing,
    /// and leaves a process that reads no packet; retail's rcon, `getinfo`
    /// and `status` all went unanswered after it. A console `map` brings
    /// the server back.
    #[test]
    fn killserver_shuts_down_and_keeps_the_process() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        sv.handle_packet(addr(9), &oob("rcon pw killserver"), t);
        assert!(!sv.quit_requested());
        let out = sv.take_outgoing();
        assert!(
            out.iter().all(|(to, _)| *to == addr(5)),
            "an rcon reply went out"
        );
        assert_eq!(
            server_commands(&mut nc, &out[0].1, &huff),
            ["e \"EXE_SERVERKILLED\"", "w"]
        );
        assert_eq!(sv.client_count(), 0);
        let later = t + Duration::from_secs(1);
        sv.handle_packet(addr(9), &oob("rcon pw status"), later);
        sv.handle_packet(addr(6), &oob("getinfo x"), later);
        sv.tick(later);
        assert!(sv.take_outgoing().is_empty());

        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping the restart");
            return;
        };
        sv.fs = Some(Rc::new(fs));
        sv.push_console("map mp_carentan");
        sv.tick(later);
        assert!(sv.take_fatal().is_none());
        sv.take_outgoing();
        sv.handle_packet(addr(6), &oob("getinfo x"), later);
        assert_eq!(reply(&mut sv).1, "infoResponse");
    }

    /// `devmap` is `map` plus `sv_cheats 1`, written after the load so the
    /// gamestate still carries the old value; `map` writes it back to 0.
    /// The replies and the systeminfo values are retail's (handshake doc,
    /// "Console commands over rcon").
    #[test]
    fn devmap_turns_cheats_on_and_map_turns_them_off() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += Duration::from_millis(500);
            let r = rcon(sv, &format!("pw {line}"), at).concat();
            sv.tick(at);
            sv.take_outgoing();
            r
        };
        assert_eq!(
            ask(&mut sv, "devmap"),
            "print\nCan't find map maps/mp/.bsp\n"
        );
        assert_eq!(
            ask(&mut sv, "devmap nosuchmap"),
            "print\nCan't find map maps/mp/nosuchmap.bsp\n"
        );
        assert_eq!(
            ask(&mut sv, "set timescale 2"),
            "print\ntimescale is cheat protected.\n"
        );
        ask(&mut sv, "devmap mp/mp_carentan");
        assert_eq!(sv.cfg.map, "mp_carentan");
        assert_eq!(
            ask(&mut sv, "sv_cheats"),
            "print\n\"sv_cheats\" is:\"1^7\" default:\"0^7\"\n"
        );
        assert!(sv.configstring(1).contains("\\sv_cheats\\1\\"));
        assert_eq!(ask(&mut sv, "set timescale 2"), "print\n");
        assert_eq!(
            ask(&mut sv, "set sv_cheats 0"),
            "print\nsv_cheats is read only.\n"
        );
        ask(&mut sv, "map mp_carentan");
        assert!(sv.configstring(1).contains("\\sv_cheats\\0\\"));
        assert_eq!(
            ask(&mut sv, "set timescale 1"),
            "print\ntimescale is cheat protected.\n"
        );
    }

    /// A restart over rcon, by `map_restart` or by `map` on the map already
    /// serving, answers with the game module's banner block; retail's reply
    /// was these bytes, client or not and whatever the argument (handshake
    /// doc, "Console commands over rcon").
    #[test]
    fn a_restart_over_rcon_prints_the_game_banner() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let banner = "print\n==== RestartGame ====\n------- Game Initialization -------\n\
                      gamename: main\ngamedate: Nov 13 2003\n0 teams with 0 entities\n\
                      -----------------------------------\n";
        let mut at = t;
        for line in ["map_restart", "map_restart 1", "map mp_carentan"] {
            at += Duration::from_millis(500);
            assert_eq!(rcon(&mut sv, &format!("pw {line}"), at), [banner], "{line}");
            sv.tick(at);
            sv.take_outgoing();
        }
    }

    /// `SV_CalcPings`: the round trip each acked message took, on the
    /// server's frame clock, averaged into `status`, `getstatus` and the
    /// scoreboard.
    #[test]
    fn ping_is_the_mean_ack_time_over_the_ring() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let mut nc = begun(&mut sv, t);
        let huff = Huffman::new();
        let mut last = NULL_USERCMD;
        // Each frame's snapshot is acked one frame later: 50 ms apiece.
        for i in 0..40 {
            sv.tick(t);
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            last.server_time = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            sv.tick(t);
            sv.take_outgoing();
            let ops = move_ops(sv.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.server_id), ack, 0, &ops, &huff)
                .unwrap();
            sv.handle_packet(addr(5), &pkt, t);
        }
        sv.tick(t);
        assert_eq!(sv.clients[0].as_ref().unwrap().ping, 50);
        sv.take_outgoing();
        sv.handle_packet(addr(7), &oob("getstatus x"), t);
        let (_, body) = reply_text(&mut sv);
        assert!(body.ends_with("\n0 50 \"vcod\""), "{body}");
    }

    #[test]
    fn an_ack_is_stamped_with_the_frame_sent_before_it_arrived() {
        // Each ack of frame i reaches the socket after frame i went out and
        // is read after frame i + 1. Every other one arrives before frame
        // i + 1 went out (0 ms), the rest after it (50 ms).
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut sv = Server::new(cfg(), t);
        let mut nc = begun(&mut sv, t);
        let huff = Huffman::new();
        let mut last = NULL_USERCMD;
        let mut clock = 0;
        for i in 0..40 {
            sv.tick(t);
            clock += 50;
            sv.frame_sent(ms(clock));
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            last.server_time = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            sv.tick(t);
            clock += 50;
            sv.frame_sent(ms(clock));
            sv.take_outgoing();
            let ops = move_ops(sv.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.server_id), ack, 0, &ops, &huff)
                .unwrap();
            let arrived = if i % 2 == 0 {
                ms(clock - 1)
            } else {
                ms(clock + 1)
            };
            sv.handle_packet_at(addr(5), &pkt, t, arrived);
        }
        sv.tick(t);
        assert_eq!(sv.clients[0].as_ref().unwrap().ping, 25);
    }

    #[test]
    fn dedicated_2_heartbeats_and_flatlines() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_master_resolver(local_master);
        let beats = |sv: &mut Server| -> Vec<String> {
            sv.take_outgoing()
                .into_iter()
                .filter(|(to, _)| *to == addr(crate::master::MASTER_PORT))
                .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
                .collect()
        };
        sv.tick(t);
        assert!(beats(&mut sv).is_empty(), "dedicated 1 stays quiet");
        sv.set_cvar("dedicated", "2");
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat COD-1\n"]);
        sv.tick(t);
        assert!(beats(&mut sv).is_empty());
        let _ = connected(&mut sv, addr(5), t);
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat COD-1\n"]);
        sv.push_console("quit");
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat flatline\n"]);
        assert!(sv.quit_requested());
    }
}
